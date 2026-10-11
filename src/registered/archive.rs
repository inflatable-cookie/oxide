//! Bounded verifier for a locked Cargo registry archive versus its extraction.
//!
//! The supported form is a single-member gzip of a ustar or legacy GNU tar
//! whose members share one canonical package prefix, matching a Cargo `.crate`. Verification
//! is read-only: it does not unpack, fetch, or scan a host registry. Matching
//! bytes do not install an issuer or prove native/proc-macro closure.

use super::{
    canonical_dir, digest, io, receipt_bound, relative, stamp, valid_digest, ArchiveEvidence,
    Attempt, ImmutableClass, ImmutableManifest, InputEvidenceOwner, Limits, Refusal, Result,
    EVIDENCE_VERSION,
};
use flate2::bufread::GzDecoder;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self as stdio, BufReader, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

/// Package-root Cargo extraction metadata. Never archive members and never attestations.
pub const CARGO_OK: &str = ".cargo-ok";
pub const CARGO_CHECKSUM_JSON: &str = ".cargo-checksum.json";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub struct Counters {
    pub archive_bytes: usize,
    pub decompressed_bytes: usize,
    pub extracted_bytes: usize,
    pub entries: usize,
}
impl Counters {
    fn total(self) -> usize {
        self.archive_bytes
            .saturating_add(self.decompressed_bytes)
            .saturating_add(self.extracted_bytes)
    }
}

/// Always returned, including on refusal, so the caller can see charged use.
#[derive(Debug)]
pub struct Outcome<T> {
    pub counters: Counters,
    pub value: Result<T>,
}

#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub archive: &'a Path,
    pub extracted_root: &'a Path,
    pub locked_checksum: &'a str,
}

/// Constructor-owned conversion authority. Task strings are not an issuer.
pub struct TrustedContext<'a> {
    owner: &'a dyn InputEvidenceOwner,
    issuer: &'a str,
    source_identity: &'a str,
    epoch: &'a str,
    receipt: &'a str,
    attempt: &'a Attempt,
    limits: Limits,
}
impl std::fmt::Debug for TrustedContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustedContext")
            .field("issuer", &self.issuer)
            .field("source_identity", &self.source_identity)
            .field("epoch", &self.epoch)
            .field("receipt", &self.receipt)
            .field("attempt", &self.attempt.binding.run_id)
            .field("owner", &(self.owner as *const dyn InputEvidenceOwner))
            .finish()
    }
}
impl<'a> TrustedContext<'a> {
    /// Build only from an already-accepted `InputEvidenceOwner` plus the
    /// receipt and attempt that owner will authenticate. Strings are not an issuer.
    pub fn new(
        owner: &'a dyn InputEvidenceOwner,
        issuer: &'a str,
        source_identity: &'a str,
        epoch: &'a str,
        receipt: &'a str,
        attempt: &'a Attempt,
        limits: Limits,
    ) -> Result<Self> {
        limits.validate()?;
        receipt_bound(issuer, limits)?;
        receipt_bound(source_identity, limits)?;
        receipt_bound(epoch, limits)?;
        receipt_bound(receipt, limits)?;
        Ok(Self {
            owner,
            issuer,
            source_identity,
            epoch,
            receipt,
            attempt,
            limits,
        })
    }
}

/// Produced only by `verify`. Fields stay private so callers cannot mint
/// registry evidence from an unverified inventory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedCorrespondence {
    package_prefix: String,
    locked_checksum: String,
    archive: PathBuf,
    extracted_root: PathBuf,
    files: BTreeMap<String, String>,
    directories: BTreeSet<String>,
    counters: Counters,
}
impl VerifiedCorrespondence {
    pub fn package_prefix(&self) -> &str {
        &self.package_prefix
    }
    pub fn locked_checksum(&self) -> &str {
        &self.locked_checksum
    }
    pub fn archive(&self) -> &Path {
        &self.archive
    }
    pub fn extracted_root(&self) -> &Path {
        &self.extracted_root
    }
    pub fn files(&self) -> &BTreeMap<String, String> {
        &self.files
    }
    pub fn directories(&self) -> &BTreeSet<String> {
        &self.directories
    }
    pub fn counters(&self) -> Counters {
        self.counters
    }

    /// Bind verified bytes to an owner-issued Registry manifest. The owner must
    /// accept the receipt; this conversion does not mint an issuer or infer
    /// native, generated or proc-macro provenance.
    pub fn into_registered(self, context: TrustedContext<'_>) -> Result<super::ImmutableManifest> {
        let TrustedContext {
            owner,
            issuer,
            source_identity,
            epoch,
            receipt,
            attempt,
            limits,
        } = context;
        if self.files.is_empty()
            || self.files.len() + self.directories.len() > limits.associations
            || !valid_digest(&self.locked_checksum)
        {
            return Err(Refusal::Unsupported);
        }
        canonical_dir(&self.extracted_root)?;
        if fs::canonicalize(&self.archive).map_err(io)? != self.archive {
            return Err(Refusal::Identity);
        }
        for name in self.files.keys().chain(self.directories.iter()) {
            relative(name, limits)?;
        }
        for value in self.files.values() {
            if !valid_digest(value) {
                return Err(Refusal::Identity);
            }
        }
        let issued = owner.immutable(receipt, attempt)?;
        if issued.class != ImmutableClass::Registry {
            return Err(Refusal::Unsupported);
        }
        if issued.version != EVIDENCE_VERSION
            || issued.issuer != issuer
            || issued.source_identity != source_identity
            || issued.epoch != epoch
            || issued.root != self.extracted_root
            || issued.files != self.files
            || issued.directories != self.directories
        {
            return Err(Refusal::Identity);
        }
        match &issued.archive {
            Some(archive)
                if archive.path == self.archive
                    && archive.locked_checksum == self.locked_checksum => {}
            _ => return Err(Refusal::Identity),
        }
        Ok(issued)
    }
}

/// Prove archive checksum and extraction membership/content. `counters` is the
/// shared preparation budget so far; this call continues it and never resets.
pub fn verify(
    request: &Request<'_>,
    limits: Limits,
    counters: Counters,
) -> Outcome<VerifiedCorrespondence> {
    let mut counters = counters;
    match verify_inner(request, limits, &mut counters) {
        Ok(value) => Outcome {
            counters,
            value: Ok(value),
        },
        Err(refusal) => Outcome {
            counters,
            value: Err(refusal),
        },
    }
}

fn verify_inner(
    request: &Request<'_>,
    limits: Limits,
    counters: &mut Counters,
) -> Result<VerifiedCorrespondence> {
    limits.validate()?;
    if !valid_digest(request.locked_checksum) {
        return Err(Refusal::Identity);
    }
    let archive = owned_file(request.archive, limits)?;
    canonical_dir(request.extracted_root)?;
    if request.extracted_root.as_os_str().len() > limits.path_bytes {
        return Err(Refusal::Budget);
    }
    let extracted_root = request.extracted_root.to_owned();
    let compressed = read_archive(&archive, limits, counters)?;
    let locked_checksum = digest(&compressed);
    if locked_checksum != request.locked_checksum {
        return Err(Refusal::Integrity);
    }
    let tar_bytes = decompress(&compressed, limits, counters)?;
    let package = tar_inventory(&tar_bytes, limits, counters)?;
    let name = extracted_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(Refusal::Identity)?;
    if name != package.prefix {
        return Err(Refusal::Identity);
    }
    let (files, directories) = extracted_inventory(&extracted_root, limits, counters)?;
    for (path, expected) in &package.files {
        match files.get(path) {
            None => return Err(Refusal::Membership),
            Some(found) if found != expected => return Err(Refusal::Integrity),
            Some(_) => {}
        }
    }
    for path in files.keys() {
        if !package.files.contains_key(path) {
            return Err(Refusal::Membership);
        }
    }
    if directories != package.directories {
        return Err(Refusal::Membership);
    }
    Ok(VerifiedCorrespondence {
        package_prefix: package.prefix,
        locked_checksum,
        archive,
        extracted_root,
        files,
        directories,
        counters: *counters,
    })
}

struct Package {
    prefix: String,
    files: BTreeMap<String, String>,
    directories: BTreeSet<String>,
}

fn owned_file(path: &Path, limits: Limits) -> Result<PathBuf> {
    if !path.is_absolute() || path.as_os_str().len() > limits.path_bytes {
        return Err(Refusal::Identity);
    }
    let canonical = fs::canonicalize(path).map_err(io)?;
    if canonical != path {
        return Err(Refusal::Identity);
    }
    Ok(canonical)
}

fn remaining(limits: Limits, counters: &Counters) -> Result<usize> {
    limits
        .collection_bytes
        .checked_sub(counters.total())
        .ok_or(Refusal::Budget)
}

fn read_archive(path: &Path, limits: Limits, counters: &mut Counters) -> Result<Vec<u8>> {
    let before = fs::symlink_metadata(path).map_err(io)?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err(Refusal::Unsupported);
    }
    if before.len() > limits.file_bytes as u64 {
        return Err(Refusal::Budget);
    }
    let limit = remaining(limits, counters)?;
    if before.len() > limit as u64 {
        return Err(Refusal::Budget);
    }
    let mut file = File::open(path).map_err(io)?;
    if stamp(&file.metadata().map_err(io)?)? != stamp(&before)? {
        return Err(Refusal::Unstable);
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(limit as u64)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > limit {
        return Err(Refusal::Budget);
    }
    counters.archive_bytes += bytes.len();
    let after = fs::symlink_metadata(path).map_err(io)?;
    if stamp(&before)? != stamp(&after)?
        || before.len() != after.len()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
        || bytes.len() as u64 != after.len()
    {
        return Err(Refusal::Unstable);
    }
    Ok(bytes)
}

fn decompress(compressed: &[u8], limits: Limits, counters: &mut Counters) -> Result<Vec<u8>> {
    if compressed.len() < 2 || compressed[0] != 0x1f || compressed[1] != 0x8b {
        return Err(Refusal::Unsupported);
    }
    let mut reader = BufReader::new(stdio::Cursor::new(compressed));
    let mut out = Vec::new();
    {
        let mut decoder = GzDecoder::new(&mut reader);
        let mut buf = [0u8; 8192];
        loop {
            let limit = remaining(limits, counters)?;
            if limit == 0 {
                return Err(Refusal::Budget);
            }
            if out.len() == limits.file_bytes {
                let mut extra = [0u8; 1];
                match decoder.read(&mut extra) {
                    Ok(0) => break,
                    Ok(n) => {
                        counters.decompressed_bytes += n;
                        return Err(Refusal::Budget);
                    }
                    Err(_) => return Err(Refusal::Integrity),
                }
            }
            let package_remaining = limits.file_bytes.saturating_sub(out.len());
            let want = buf.len().min(limit).min(package_remaining);
            match decoder.read(&mut buf[..want]) {
                Ok(0) => break,
                Ok(n) => {
                    counters.decompressed_bytes += n;
                    out.extend_from_slice(&buf[..n]);
                }
                Err(_) => return Err(Refusal::Integrity),
            }
        }
    }
    if !reader.buffer().is_empty() {
        return Err(Refusal::Unsupported);
    }
    if reader.into_inner().position() as usize != compressed.len() {
        return Err(Refusal::Unsupported);
    }
    if out.is_empty() {
        return Err(Refusal::Unsupported);
    }
    Ok(out)
}

fn tar_inventory(bytes: &[u8], limits: Limits, counters: &mut Counters) -> Result<Package> {
    let mut archive = tar::Archive::new(stdio::Cursor::new(bytes));
    let mut prefix = None;
    let mut files = BTreeMap::new();
    let mut directories = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for entry in archive.entries().map_err(|_| Refusal::Unsupported)? {
        bump_entries(counters, limits)?;
        let mut entry = entry.map_err(|_| Refusal::Integrity)?;
        // Cargo registry archives use both POSIX ustar and legacy GNU headers.
        // This format allowance does not relax the member checks below.
        if entry.header().as_ustar().is_none() && entry.header().as_gnu().is_none() {
            return Err(Refusal::Unsupported);
        }
        let kind = entry.header().entry_type();
        let is_dir = matches!(kind, tar::EntryType::Directory);
        let is_file = matches!(kind, tar::EntryType::Regular | tar::EntryType::Continuous);
        if !is_dir && !is_file {
            return Err(Refusal::Unsupported);
        }
        let path = entry.path().map_err(|_| Refusal::Unsupported)?;
        let text = path.to_str().ok_or(Refusal::Unsupported)?;
        let parts = parse_member(text, is_dir, limits)?;
        let this_prefix = parts[0].clone();
        match &prefix {
            None => prefix = Some(this_prefix),
            Some(expected) if *expected == this_prefix => {}
            Some(_) => return Err(Refusal::Unsupported),
        }
        let relative = parts[1..].join("/");
        if relative.is_empty() {
            if !is_dir {
                return Err(Refusal::Unsupported);
            }
            if !seen.insert(String::new()) {
                return Err(Refusal::Unsupported);
            }
            continue;
        }
        if is_sidecar(&relative) {
            return Err(Refusal::Unsupported);
        }
        if !seen.insert(relative.clone()) {
            return Err(Refusal::Unsupported);
        }
        if is_dir {
            if files.contains_key(&relative) {
                return Err(Refusal::Unsupported);
            }
            directories.insert(relative.clone());
            insert_parents(&relative, &files, &mut directories, limits, counters)?;
        } else {
            if directories.contains(&relative) {
                return Err(Refusal::Unsupported);
            }
            let size = entry.size();
            if size > limits.file_bytes as u64 {
                return Err(Refusal::Budget);
            }
            let mut content = Vec::new();
            (&mut entry)
                .take(size.saturating_add(1))
                .read_to_end(&mut content)
                .map_err(|_| Refusal::Integrity)?;
            if content.len() as u64 != size {
                return Err(Refusal::Integrity);
            }
            files.insert(relative.clone(), digest(&content));
            insert_parents(&relative, &files, &mut directories, limits, counters)?;
        }
    }
    let prefix = prefix.ok_or(Refusal::Unsupported)?;
    if files.is_empty() {
        return Err(Refusal::Unsupported);
    }
    if files.len().saturating_add(directories.len()) > limits.associations {
        return Err(Refusal::Budget);
    }
    Ok(Package {
        prefix,
        files,
        directories,
    })
}

fn extracted_inventory(
    root: &Path,
    limits: Limits,
    counters: &mut Counters,
) -> Result<(BTreeMap<String, String>, BTreeSet<String>)> {
    let mut files = BTreeMap::new();
    let mut directories = BTreeSet::new();
    walk(
        root,
        root,
        0,
        limits,
        counters,
        &mut files,
        &mut directories,
    )?;
    Ok((files, directories))
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    limits: Limits,
    counters: &mut Counters,
    files: &mut BTreeMap<String, String>,
    directories: &mut BTreeSet<String>,
) -> Result<()> {
    if depth > limits.depth {
        return Err(Refusal::Budget);
    }
    let before = fs::symlink_metadata(dir).map_err(io)?;
    if !before.is_dir() || before.file_type().is_symlink() {
        return Err(Refusal::Unsupported);
    }
    let before_id = dir_identity(&before)?;
    let children = list_dir_charged(dir, limits, counters)?;
    for path in &children {
        let metadata = fs::symlink_metadata(path).map_err(io)?;
        let name = path
            .strip_prefix(root)
            .map_err(io)?
            .to_str()
            .ok_or(Refusal::Unsupported)?;
        relative(name, limits)?;
        if metadata.file_type().is_symlink() {
            return Err(Refusal::Unsupported);
        }
        if metadata.is_dir() {
            directories.insert(name.to_owned());
            walk(root, path, depth + 1, limits, counters, files, directories)?;
        } else if metadata.is_file() {
            if metadata.nlink() != 1 {
                return Err(Refusal::Unsupported);
            }
            if is_sidecar(name) {
                continue;
            }
            files.insert(name.to_owned(), read_extracted(path, limits, counters)?);
        } else {
            return Err(Refusal::Unsupported);
        }
    }
    let after = fs::symlink_metadata(dir).map_err(io)?;
    if dir_identity(&after)? != before_id {
        return Err(Refusal::Unstable);
    }
    let after_children = list_dir_capped(dir, children.len().saturating_add(1))?;
    if after_children != children {
        return Err(Refusal::Unstable);
    }
    Ok(())
}

fn list_dir_charged(dir: &Path, limits: Limits, counters: &mut Counters) -> Result<Vec<PathBuf>> {
    let mut children = Vec::new();
    for entry in fs::read_dir(dir).map_err(io)? {
        bump_entries(counters, limits)?;
        children.push(entry.map_err(io)?.path());
    }
    children.sort();
    Ok(children)
}

fn list_dir_capped(dir: &Path, max: usize) -> Result<Vec<PathBuf>> {
    let mut children = Vec::new();
    for entry in fs::read_dir(dir).map_err(io)? {
        if children.len() >= max {
            return Err(Refusal::Unstable);
        }
        children.push(entry.map_err(io)?.path());
    }
    children.sort();
    Ok(children)
}

fn dir_identity(metadata: &fs::Metadata) -> Result<(u64, u64, u64, i64, i64, u64)> {
    let (mtime, dev, ino) = stamp(metadata)?;
    Ok((
        mtime,
        dev,
        ino,
        metadata.ctime(),
        metadata.ctime_nsec(),
        metadata.nlink(),
    ))
}

fn read_extracted(path: &Path, limits: Limits, counters: &mut Counters) -> Result<String> {
    let before = fs::symlink_metadata(path).map_err(io)?;
    if !before.is_file() || before.file_type().is_symlink() || before.nlink() != 1 {
        return Err(Refusal::Unsupported);
    }
    let limit = remaining(limits, counters)?.min(limits.file_bytes);
    if before.len() > limit as u64 {
        return Err(Refusal::Budget);
    }
    let mut file = File::open(path).map_err(io)?;
    if stamp(&file.metadata().map_err(io)?)? != stamp(&before)? {
        return Err(Refusal::Unstable);
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(limit as u64)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > limit {
        return Err(Refusal::Budget);
    }
    counters.extracted_bytes += bytes.len();
    let after = fs::symlink_metadata(path).map_err(io)?;
    if stamp(&before)? != stamp(&after)?
        || before.len() != after.len()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
        || before.nlink() != after.nlink()
        || bytes.len() as u64 != after.len()
    {
        return Err(Refusal::Unstable);
    }
    Ok(digest(&bytes))
}

fn bump_entries(counters: &mut Counters, limits: Limits) -> Result<()> {
    counters.entries = counters
        .entries
        .checked_add(1)
        .filter(|value| *value <= limits.associations)
        .ok_or(Refusal::Budget)?;
    Ok(())
}

fn parse_member(text: &str, is_dir: bool, limits: Limits) -> Result<Vec<String>> {
    if text.is_empty()
        || text.len() > limits.path_bytes
        || text.as_bytes().contains(&0)
        || text.contains('\\')
        || text.contains("//")
        || text.starts_with('/')
    {
        return Err(Refusal::Unsupported);
    }
    if !is_dir && text.ends_with('/') {
        return Err(Refusal::Unsupported);
    }
    let trimmed = text.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(Refusal::Unsupported);
    }
    let mut parts = Vec::new();
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(Refusal::Unsupported);
        }
        parts.push(part.to_owned());
    }
    if parts.len() > limits.depth {
        return Err(Refusal::Budget);
    }
    relative(&parts[0], limits)?;
    if parts.len() > 1 {
        relative(&parts[1..].join("/"), limits)?;
    }
    Ok(parts)
}

fn is_sidecar(name: &str) -> bool {
    name == CARGO_OK || name == CARGO_CHECKSUM_JSON
}

fn insert_parents(
    file: &str,
    files: &BTreeMap<String, String>,
    directories: &mut BTreeSet<String>,
    limits: Limits,
    counters: &mut Counters,
) -> Result<()> {
    let path = Path::new(file);
    let mut prefix = PathBuf::new();
    let mut first = true;
    for component in path.components() {
        let Component::Normal(part) = component else {
            return Err(Refusal::Unsupported);
        };
        if first {
            first = false;
            prefix.push(part);
            continue;
        }
        let name = prefix.to_str().ok_or(Refusal::Unsupported)?;
        if files.contains_key(name) {
            return Err(Refusal::Unsupported);
        }
        if directories.insert(name.to_owned()) {
            bump_entries(counters, limits)?;
        }
        prefix.push(part);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Attempt, Registration, UnitEvidence};
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Scratch {
        root: PathBuf,
    }
    impl Scratch {
        fn new() -> Self {
            let base = PathBuf::from(
                std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").expect("canonical task-owned scratch"),
            );
            assert_eq!(fs::canonicalize(&base).unwrap(), base);
            let root = base.join(format!(
                "archive-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            Self { root }
        }
        fn path(&self, name: &str) -> PathBuf {
            self.root.join(name)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    struct StubOwner;
    impl InputEvidenceOwner for StubOwner {
        fn immutable(&self, _: &str, _: &Attempt) -> Result<ImmutableManifest> {
            Err(Refusal::MissingAuthority)
        }
        fn units(&self, _: &Registration, _: &Attempt) -> Result<Vec<UnitEvidence>> {
            Err(Refusal::MissingAuthority)
        }
        fn seal_map(&self, _: &Attempt, _: &str) -> Result<String> {
            Err(Refusal::MissingAuthority)
        }
        fn verify_map(&self, _: &Attempt, _: &str, _: &str) -> Result<()> {
            Err(Refusal::MissingAuthority)
        }
    }

    struct AcceptingOwner {
        receipt: String,
        manifest: ImmutableManifest,
    }
    impl InputEvidenceOwner for AcceptingOwner {
        fn immutable(&self, receipt: &str, _: &Attempt) -> Result<ImmutableManifest> {
            if receipt != self.receipt {
                return Err(Refusal::MissingAuthority);
            }
            Ok(self.manifest.clone())
        }
        fn units(&self, _: &Registration, _: &Attempt) -> Result<Vec<UnitEvidence>> {
            Err(Refusal::MissingAuthority)
        }
        fn seal_map(&self, _: &Attempt, _: &str) -> Result<String> {
            Err(Refusal::MissingAuthority)
        }
        fn verify_map(&self, _: &Attempt, _: &str, _: &str) -> Result<()> {
            Err(Refusal::MissingAuthority)
        }
    }

    fn crate_bytes(prefix: &str, files: &[(&str, &[u8])], dirs: &[&str]) -> Vec<u8> {
        crate_bytes_with(prefix, files, dirs, None)
    }
    fn crate_bytes_with(
        prefix: &str,
        files: &[(&str, &[u8])],
        dirs: &[&str],
        extra: Option<fn(&mut tar::Builder<&mut Vec<u8>>)>,
    ) -> Vec<u8> {
        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            append_dir(&mut builder, &format!("{prefix}/"));
            for dir in dirs {
                append_dir(&mut builder, &format!("{prefix}/{dir}"));
            }
            for (name, bytes) in files {
                append_file(&mut builder, &format!("{prefix}/{name}"), bytes);
            }
            if let Some(extra) = extra {
                extra(&mut builder);
            }
            builder.finish().unwrap();
        }
        gzip(&tar_buf)
    }
    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = GzEncoder::new(&mut out, Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap();
        out
    }
    fn append_dir(builder: &mut tar::Builder<&mut Vec<u8>>, name: &str) {
        append_dir_header(builder, name, tar::Header::new_ustar);
    }
    fn append_file(builder: &mut tar::Builder<&mut Vec<u8>>, name: &str, bytes: &[u8]) {
        append_file_header(builder, name, bytes, tar::Header::new_ustar);
    }
    fn crate_bytes_with_header(
        prefix: &str,
        files: &[(&str, &[u8])],
        dirs: &[&str],
        new_header: fn() -> tar::Header,
    ) -> Vec<u8> {
        crate_bytes_with_header_and_extra(prefix, files, dirs, new_header, None)
    }
    fn crate_bytes_with_header_and_extra(
        prefix: &str,
        files: &[(&str, &[u8])],
        dirs: &[&str],
        new_header: fn() -> tar::Header,
        extra: Option<fn(&mut tar::Builder<&mut Vec<u8>>)>,
    ) -> Vec<u8> {
        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            append_dir_header(&mut builder, &format!("{prefix}/"), new_header);
            for dir in dirs {
                append_dir_header(&mut builder, &format!("{prefix}/{dir}"), new_header);
            }
            for (name, bytes) in files {
                append_file_header(&mut builder, &format!("{prefix}/{name}"), bytes, new_header);
            }
            if let Some(extra) = extra {
                extra(&mut builder);
            }
            builder.finish().unwrap();
        }
        gzip(&tar_buf)
    }
    fn append_dir_header(
        builder: &mut tar::Builder<&mut Vec<u8>>,
        name: &str,
        new_header: fn() -> tar::Header,
    ) {
        let mut header = new_header();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_path(name).unwrap();
        header.set_size(0);
        header.set_cksum();
        builder.append(&header, stdio::empty()).unwrap();
    }
    fn append_file_header(
        builder: &mut tar::Builder<&mut Vec<u8>>,
        name: &str,
        bytes: &[u8],
        new_header: fn() -> tar::Header,
    ) {
        let mut header = new_header();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_path(name).unwrap();
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        builder.append(&header, bytes).unwrap();
    }
    fn write_extracted(root: &Path, files: &[(&str, &[u8])], dirs: &[&str]) {
        fs::create_dir_all(root).unwrap();
        for dir in dirs {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        for (name, bytes) in files {
            if let Some(parent) = Path::new(name).parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(root.join(parent)).unwrap();
                }
            }
            fs::write(root.join(name), bytes).unwrap();
        }
    }
    fn layout() -> (
        &'static str,
        Vec<(&'static str, &'static [u8])>,
        Vec<&'static str>,
    ) {
        (
            "pkg-1.0.0",
            vec![
                ("Cargo.toml", b"[package]\nname=\"pkg\"\n" as &[u8]),
                ("src/lib.rs", b"pub fn n() -> u8 { 1 }\n"),
            ],
            vec!["src"],
        )
    }
    fn prepare(scratch: &Scratch) -> (PathBuf, PathBuf, String) {
        let (prefix, files, dirs) = layout();
        let archive = scratch.path("pkg-1.0.0.crate");
        let extracted = scratch.path(prefix);
        fs::write(&archive, crate_bytes(prefix, &files, &dirs)).unwrap();
        write_extracted(&extracted, &files, &dirs);
        let checksum = digest(&fs::read(&archive).unwrap());
        (
            fs::canonicalize(archive).unwrap(),
            fs::canonicalize(extracted).unwrap(),
            checksum,
        )
    }
    fn ask<'a>(archive: &'a Path, extracted: &'a Path, checksum: &'a str) -> Request<'a> {
        Request {
            archive,
            extracted_root: extracted,
            locked_checksum: checksum,
        }
    }
    fn once(request: &Request<'_>, limits: Limits) -> Outcome<VerifiedCorrespondence> {
        verify(request, limits, Counters::default())
    }
    fn attempt(scratch: &Scratch) -> Attempt {
        Attempt {
            binding: crate::RunBinding {
                consumer: "synthetic".into(),
                project: "owned".into(),
                component: "core".into(),
                workflow: "archive".into(),
                task_id: "task-1".into(),
                workspace_id: "workspace-1".into(),
                run_id: "run-1".into(),
                attempt_id: "attempt-1".into(),
                commit: "committed-1".into(),
                source_tree_digest: format!("sha256:{}", "ab".repeat(32)),
            },
            root_id: "root-1".into(),
            allocation_epoch: "epoch-1".into(),
            branch: "task-branch-1".into(),
            source_root: scratch.path("source"),
            slot: scratch.path("slot"),
            generation: "generation-1".into(),
            partition: "partition-1".into(),
            lease_id: "lease-1".into(),
            target: scratch.path("target"),
            temp: scratch.path("temp"),
            capture: scratch.path("capture"),
            authorization: "authenticated-attempt".into(),
            reused: false,
        }
    }
    fn issued(verified: &VerifiedCorrespondence, issuer: &str, epoch: &str) -> ImmutableManifest {
        ImmutableManifest {
            version: EVIDENCE_VERSION,
            issuer: issuer.to_owned(),
            source_identity: verified.package_prefix().to_owned(),
            epoch: epoch.to_owned(),
            class: ImmutableClass::Registry,
            root: verified.extracted_root().to_owned(),
            files: verified.files().clone(),
            directories: verified.directories().clone(),
            archive: Some(ArchiveEvidence {
                path: verified.archive().to_owned(),
                locked_checksum: verified.locked_checksum().to_owned(),
            }),
        }
    }
    fn write_crate(
        scratch: &Scratch,
        name: &str,
        prefix: &str,
        files: &[(&str, &[u8])],
        dirs: &[&str],
        extra: Option<fn(&mut tar::Builder<&mut Vec<u8>>)>,
    ) -> (PathBuf, String) {
        let path = scratch.path(name);
        fs::write(&path, crate_bytes_with(prefix, files, dirs, extra)).unwrap();
        let path = fs::canonicalize(path).unwrap();
        let checksum = digest(&fs::read(&path).unwrap());
        (path, checksum)
    }

    #[test]
    fn matching_crate_and_extraction_agree_and_sidecars_are_not_members() {
        let scratch = Scratch::new();
        let (archive, extracted, checksum) = prepare(&scratch);
        fs::write(extracted.join(CARGO_OK), b"not-an-attestation").unwrap();
        fs::write(
            extracted.join(CARGO_CHECKSUM_JSON),
            br#"{"package":"deadbeef","files":{"Cargo.toml":"nope"}}"#,
        )
        .unwrap();
        let outcome = once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS);
        let verified = outcome.value.expect("correspondence");
        assert_eq!(verified.package_prefix(), "pkg-1.0.0");
        assert_eq!(verified.locked_checksum(), checksum);
        assert_eq!(verified.files().len(), 2);
        assert!(verified.files().contains_key("Cargo.toml"));
        assert!(verified.files().contains_key("src/lib.rs"));
        assert!(verified.directories().contains("src"));
        assert!(!verified.files().contains_key(CARGO_OK));
        assert!(!verified.files().contains_key(CARGO_CHECKSUM_JSON));
        assert!(outcome.counters.archive_bytes > 0);
        assert!(outcome.counters.decompressed_bytes > 0);
        assert!(outcome.counters.extracted_bytes > 0);
    }

    #[test]
    fn matching_nested_sidecar_named_files_are_ordinary_members() {
        let scratch = Scratch::new();
        let nested_ok: &[u8] = b"nested-ok-bytes";
        let nested_sum: &[u8] = br#"{"files":{}}"#;
        let files: &[(&str, &[u8])] = &[
            ("Cargo.toml", b"[package]\nname=\"pkg\"\n"),
            ("src/lib.rs", b"pub fn n() -> u8 { 1 }\n"),
            ("src/.cargo-ok", nested_ok),
            ("src/.cargo-checksum.json", nested_sum),
        ];
        let (archive, checksum) = write_crate(
            &scratch,
            "nested-sidecar.crate",
            "pkg-1.0.0",
            files,
            &["src"],
            None,
        );
        let extracted = scratch.path("pkg-1.0.0");
        write_extracted(&extracted, files, &["src"]);
        fs::write(extracted.join(CARGO_OK), b"root-ok").unwrap();
        fs::write(extracted.join(CARGO_CHECKSUM_JSON), b"root-sum").unwrap();
        let extracted = fs::canonicalize(&extracted).unwrap();
        let verified = once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS)
            .value
            .expect("matching nested sidecar names are ordinary members");
        assert_eq!(verified.files().len(), 4);
        assert_eq!(
            verified.files().get("src/.cargo-ok"),
            Some(&digest(nested_ok))
        );
        assert_eq!(
            verified.files().get("src/.cargo-checksum.json"),
            Some(&digest(nested_sum))
        );
        assert!(!verified.files().contains_key(CARGO_OK));
        assert!(!verified.files().contains_key(CARGO_CHECKSUM_JSON));

        fs::write(extracted.join("src/.cargo-ok"), b"different").unwrap();
        assert_eq!(
            once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Integrity)
        );
        fs::remove_file(extracted.join("src/.cargo-ok")).unwrap();
        assert_eq!(
            once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Membership)
        );
    }

    #[test]
    fn checksum_membership_sidecar_and_path_cases_refuse_independently() {
        let scratch = Scratch::new();
        let (archive, extracted, checksum) = prepare(&scratch);
        assert_eq!(
            once(
                &ask(&archive, &extracted, "sha256:not-a-digest"),
                Limits::NUCLEUS
            )
            .value,
            Err(Refusal::Identity)
        );
        let wrong = format!("sha256:{}", "ab".repeat(32));
        assert_eq!(
            once(&ask(&archive, &extracted, &wrong), Limits::NUCLEUS).value,
            Err(Refusal::Integrity)
        );

        fs::write(extracted.join("extra.rs"), b"extra").unwrap();
        assert_eq!(
            once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Membership)
        );
        fs::remove_file(extracted.join("extra.rs")).unwrap();
        fs::remove_file(extracted.join("src/lib.rs")).unwrap();
        assert_eq!(
            once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Membership)
        );
        fs::write(extracted.join("src/lib.rs"), b"changed").unwrap();
        assert_eq!(
            once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Integrity)
        );
        fs::write(extracted.join("src/lib.rs"), b"pub fn n() -> u8 { 1 }\n").unwrap();
        fs::write(extracted.join("src/.cargo-ok"), b"nested sidecar is extra").unwrap();
        assert_eq!(
            once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Membership)
        );
        fs::remove_file(extracted.join("src/.cargo-ok")).unwrap();

        let other = scratch.path("other-1.0.0");
        write_extracted(
            &other,
            &[
                ("Cargo.toml", b"[package]\nname=\"pkg\"\n" as &[u8]),
                ("src/lib.rs", b"pub fn n() -> u8 { 1 }\n"),
            ],
            &["src"],
        );
        let other = fs::canonicalize(other).unwrap();
        assert_eq!(
            once(&ask(&archive, &other, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Identity)
        );

        let (sidecar_archive, sidecar_sum) = write_crate(
            &scratch,
            "sidecar.crate",
            "pkg-1.0.0",
            &[
                ("Cargo.toml", b"[package]\nname=\"pkg\"\n" as &[u8]),
                (CARGO_OK, b"ok"),
            ],
            &[],
            None,
        );
        assert_eq!(
            once(
                &ask(&sidecar_archive, &extracted, &sidecar_sum),
                Limits::NUCLEUS
            )
            .value,
            Err(Refusal::Unsupported)
        );

        let (symlink_crate, sum) = write_crate(
            &scratch,
            "symlink.crate",
            "pkg-1.0.0",
            &[("Cargo.toml", b"[package]\nname=\"pkg\"\n" as &[u8])],
            &[],
            Some(|builder| {
                let mut header = tar::Header::new_ustar();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_path("pkg-1.0.0/link").unwrap();
                header.set_link_name("Cargo.toml").unwrap();
                header.set_size(0);
                header.set_cksum();
                builder.append(&header, stdio::empty()).unwrap();
            }),
        );
        assert_eq!(
            once(&ask(&symlink_crate, &extracted, &sum), Limits::NUCLEUS).value,
            Err(Refusal::Unsupported)
        );

        let (hardlink_crate, sum) = write_crate(
            &scratch,
            "hardlink.crate",
            "pkg-1.0.0",
            &[("Cargo.toml", b"[package]\nname=\"pkg\"\n" as &[u8])],
            &[],
            Some(|builder| {
                let mut header = tar::Header::new_ustar();
                header.set_entry_type(tar::EntryType::Link);
                header.set_path("pkg-1.0.0/alias").unwrap();
                header.set_link_name("pkg-1.0.0/Cargo.toml").unwrap();
                header.set_size(0);
                header.set_cksum();
                builder.append(&header, stdio::empty()).unwrap();
            }),
        );
        assert_eq!(
            once(&ask(&hardlink_crate, &extracted, &sum), Limits::NUCLEUS).value,
            Err(Refusal::Unsupported)
        );

        let (duplicate, sum) = write_crate(
            &scratch,
            "duplicate.crate",
            "pkg-1.0.0",
            &[("Cargo.toml", b"one")],
            &[],
            Some(|builder| {
                append_file(builder, "pkg-1.0.0/Cargo.toml", b"two");
            }),
        );
        assert_eq!(
            once(&ask(&duplicate, &extracted, &sum), Limits::NUCLEUS).value,
            Err(Refusal::Unsupported)
        );

        let (dup_root, sum) = write_crate(
            &scratch,
            "dup-root.crate",
            "pkg-1.0.0",
            &[("Cargo.toml", b"x")],
            &[],
            Some(|builder| {
                append_dir(builder, "pkg-1.0.0/");
            }),
        );
        assert_eq!(
            once(&ask(&dup_root, &extracted, &sum), Limits::NUCLEUS).value,
            Err(Refusal::Unsupported)
        );

        let (two_prefix, sum) = write_crate(
            &scratch,
            "two-prefix.crate",
            "pkg-1.0.0",
            &[("Cargo.toml", b"x")],
            &[],
            Some(|builder| {
                append_file(builder, "other-1.0.0/Cargo.toml", b"y");
            }),
        );
        assert_eq!(
            once(&ask(&two_prefix, &extracted, &sum), Limits::NUCLEUS).value,
            Err(Refusal::Unsupported)
        );

        fs::write(scratch.path("plain.tar"), b"not-gzip").unwrap();
        let plain = fs::canonicalize(scratch.path("plain.tar")).unwrap();
        assert_eq!(
            once(
                &ask(&plain, &extracted, &digest(&fs::read(&plain).unwrap())),
                Limits::NUCLEUS
            )
            .value,
            Err(Refusal::Unsupported)
        );

        symlink(extracted.join("Cargo.toml"), extracted.join("link.rs")).unwrap();
        assert_eq!(
            once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Unsupported)
        );
        fs::remove_file(extracted.join("link.rs")).unwrap();
        UnixListener::bind(extracted.join("socket")).unwrap();
        assert_eq!(
            once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Unsupported)
        );
        fs::remove_file(extracted.join("socket")).unwrap();
        fs::hard_link(extracted.join("Cargo.toml"), extracted.join("alias.rs")).unwrap();
        assert_eq!(
            once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Unsupported)
        );
        fs::remove_file(extracted.join("alias.rs")).unwrap();
        fs::hard_link(
            extracted.join("src/lib.rs"),
            extracted.join("src/lib.rs.bak"),
        )
        .unwrap();
        assert_eq!(
            once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Unsupported)
        );

        let (prefix, files, dirs) = layout();
        let v7_root = scratch.path("v7-extract").join(prefix);
        write_extracted(&v7_root, &files, &dirs);
        let v7_root = fs::canonicalize(v7_root).unwrap();
        let v7_path = scratch.path("v7.crate");
        fs::write(
            &v7_path,
            crate_bytes_with_header(prefix, &files, &dirs, tar::Header::new_old),
        )
        .unwrap();
        let v7 = fs::canonicalize(v7_path).unwrap();
        let v7_sum = digest(&fs::read(&v7).unwrap());
        assert_eq!(
            once(&ask(&v7, &v7_root, &v7_sum), Limits::NUCLEUS).value,
            Err(Refusal::Unsupported)
        );
        let gnu_root = scratch.path("gnu-extract").join(prefix);
        write_extracted(&gnu_root, &files, &dirs);
        let gnu_root = fs::canonicalize(gnu_root).unwrap();
        let gnu_path = scratch.path("gnu.crate");
        fs::write(
            &gnu_path,
            crate_bytes_with_header(prefix, &files, &dirs, tar::Header::new_gnu),
        )
        .unwrap();
        let gnu = fs::canonicalize(gnu_path).unwrap();
        let gnu_sum = digest(&fs::read(&gnu).unwrap());
        let gnu_outcome = once(&ask(&gnu, &gnu_root, &gnu_sum), Limits::NUCLEUS);
        let gnu_verified = gnu_outcome
            .value
            .expect("observed legacy GNU tar headers have exact correspondence");
        assert_eq!(gnu_verified.files().len(), 2);
        assert!(gnu_verified.directories().contains("src"));
        assert_eq!(
            once(
                &ask(&gnu, &gnu_root, &format!("sha256:{}", "ab".repeat(32))),
                Limits::NUCLEUS
            )
            .value,
            Err(Refusal::Integrity)
        );

        let gnu_link = scratch.path("gnu-link.crate");
        fs::write(
            &gnu_link,
            crate_bytes_with_header_and_extra(
                prefix,
                &files,
                &dirs,
                tar::Header::new_gnu,
                Some(|builder| {
                    let mut header = tar::Header::new_gnu();
                    header.set_entry_type(tar::EntryType::Symlink);
                    header.set_path("pkg-1.0.0/link").unwrap();
                    header.set_link_name("Cargo.toml").unwrap();
                    header.set_size(0);
                    header.set_cksum();
                    builder.append(&header, stdio::empty()).unwrap();
                }),
            ),
        )
        .unwrap();
        let gnu_link = fs::canonicalize(gnu_link).unwrap();
        let gnu_link_sum = digest(&fs::read(&gnu_link).unwrap());
        assert_eq!(
            once(&ask(&gnu_link, &gnu_root, &gnu_link_sum), Limits::NUCLEUS).value,
            Err(Refusal::Unsupported)
        );

        let gnu_duplicate = scratch.path("gnu-duplicate.crate");
        fs::write(
            &gnu_duplicate,
            crate_bytes_with_header_and_extra(
                prefix,
                &[("Cargo.toml", b"first")],
                &[],
                tar::Header::new_gnu,
                Some(|builder| {
                    append_file_header(
                        builder,
                        "pkg-1.0.0/Cargo.toml",
                        b"second",
                        tar::Header::new_gnu,
                    );
                }),
            ),
        )
        .unwrap();
        let gnu_duplicate = fs::canonicalize(gnu_duplicate).unwrap();
        let gnu_duplicate_sum = digest(&fs::read(&gnu_duplicate).unwrap());
        assert_eq!(
            once(
                &ask(&gnu_duplicate, &gnu_root, &gnu_duplicate_sum),
                Limits::NUCLEUS
            )
            .value,
            Err(Refusal::Unsupported)
        );

        let mut gnu_trailing = fs::read(&gnu).unwrap();
        gnu_trailing.extend_from_slice(&gzip(b"second-gzip-member"));
        let gnu_trailing_path = scratch.path("gnu-trailing.crate");
        fs::write(&gnu_trailing_path, &gnu_trailing).unwrap();
        let gnu_trailing_path = fs::canonicalize(gnu_trailing_path).unwrap();
        let gnu_trailing_sum = digest(&fs::read(&gnu_trailing_path).unwrap());
        assert_eq!(
            once(
                &ask(&gnu_trailing_path, &gnu_root, &gnu_trailing_sum),
                Limits::NUCLEUS
            )
            .value,
            Err(Refusal::Unsupported)
        );
    }

    #[test]
    fn parse_member_refuses_escape_absolute_and_ambiguous_names() {
        let limits = Limits::NUCLEUS;
        assert_eq!(
            parse_member("/abs.rs", false, limits),
            Err(Refusal::Unsupported)
        );
        assert_eq!(
            parse_member("pkg-1.0.0/../x", false, limits),
            Err(Refusal::Unsupported)
        );
        assert_eq!(
            parse_member("pkg-1.0.0/./x", false, limits),
            Err(Refusal::Unsupported)
        );
        assert_eq!(
            parse_member("pkg-1.0.0//x", false, limits),
            Err(Refusal::Unsupported)
        );
        assert_eq!(
            parse_member("pkg-1.0.0\\x", false, limits),
            Err(Refusal::Unsupported)
        );
        assert_eq!(
            parse_member("pkg-1.0.0/\0x", false, limits),
            Err(Refusal::Unsupported)
        );
        assert!(parse_member("pkg-1.0.0/src/lib.rs", false, limits).is_ok());
    }

    #[test]
    fn directory_listing_refuses_before_collecting_past_the_entry_cap() {
        let scratch = Scratch::new();
        let (archive, extracted, checksum) = prepare(&scratch);
        for index in 0..64 {
            fs::write(extracted.join(format!("extra-{index}.rs")), b"x").unwrap();
        }
        let mut tight = Limits::NUCLEUS;
        tight.associations = 8;
        let refused = once(&ask(&archive, &extracted, &checksum), tight);
        assert_eq!(refused.value, Err(Refusal::Budget));
        assert_eq!(refused.counters.entries, 8);
    }

    #[test]
    fn implied_parent_directories_charge_the_inventory_cap() {
        let scratch = Scratch::new();
        let prefix = "pkg-1.0.0";
        let files = [("a/b/c/d/e.rs", b"fn e() {}\n" as &[u8])];
        let archive = scratch.path("deep.crate");
        fs::write(&archive, crate_bytes(prefix, &files, &[])).unwrap();
        let extracted = scratch.path(prefix);
        write_extracted(&extracted, &files, &[]);
        let archive = fs::canonicalize(archive).unwrap();
        let extracted = fs::canonicalize(extracted).unwrap();
        let checksum = digest(&fs::read(&archive).unwrap());
        let verified = once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS)
            .value
            .expect("implied parents");
        assert!(verified.files().contains_key("a/b/c/d/e.rs"));
        assert!(verified.directories().contains("a"));
        assert!(verified.directories().contains("a/b"));
        assert!(verified.directories().contains("a/b/c"));
        assert!(verified.directories().contains("a/b/c/d"));
        let mut tight = Limits::NUCLEUS;
        tight.associations = 5;
        let refused = once(&ask(&archive, &extracted, &checksum), tight);
        assert_eq!(refused.value, Err(Refusal::Budget));
        assert_eq!(refused.counters.entries, 5);
    }

    #[test]
    fn nested_empty_directories_record_implied_parents() {
        let scratch = Scratch::new();
        let prefix = "pkg-1.0.0";
        let files = [("Cargo.toml", b"[package]\nname=\"pkg\"\n" as &[u8])];
        let dirs = ["a/b"];
        let archive = scratch.path("nested.crate");
        fs::write(&archive, crate_bytes(prefix, &files, &dirs)).unwrap();
        let extracted = scratch.path(prefix);
        write_extracted(&extracted, &files, &dirs);
        let archive = fs::canonicalize(archive).unwrap();
        let extracted = fs::canonicalize(extracted).unwrap();
        let checksum = digest(&fs::read(&archive).unwrap());
        let verified = once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS)
            .value
            .expect("nested empty dirs");
        assert!(verified.directories().contains("a"));
        assert!(verified.directories().contains("a/b"));
        assert_eq!(verified.files().len(), 1);
    }

    #[test]
    fn shared_budget_counts_without_reset_and_refuses_bombs_and_truncation() {
        let scratch = Scratch::new();
        let (archive, extracted, checksum) = prepare(&scratch);
        let request = ask(&archive, &extracted, &checksum);
        let ok = verify(&request, Limits::NUCLEUS, Counters::default());
        let counters = ok.counters;
        assert!(ok.value.is_ok());
        assert!(counters.archive_bytes > 0);
        assert!(counters.decompressed_bytes > 0);
        assert!(counters.extracted_bytes > 0);

        let mut compressed_cap = Limits::NUCLEUS;
        compressed_cap.file_bytes = fs::metadata(&archive).unwrap().len() as usize - 1;
        let compressed_refusal = verify(&request, compressed_cap, Counters::default());
        assert_eq!(compressed_refusal.value, Err(Refusal::Budget));
        assert_eq!(compressed_refusal.counters.archive_bytes, 0);

        let mut decompressed_cap = Limits::NUCLEUS;
        decompressed_cap.file_bytes = fs::metadata(&archive).unwrap().len() as usize + 1;
        let decompressed_refusal = verify(&request, decompressed_cap, Counters::default());
        assert_eq!(decompressed_refusal.value, Err(Refusal::Budget));
        assert!(decompressed_refusal.counters.archive_bytes > 0);
        assert!(decompressed_refusal.counters.decompressed_bytes > 0);
        assert!(
            decompressed_refusal.counters.decompressed_bytes <= decompressed_cap.file_bytes + 1
        );

        let second = verify(&request, Limits::NUCLEUS, counters);
        assert!(second.value.is_ok());
        assert_eq!(second.counters.archive_bytes, counters.archive_bytes * 2);
        assert_eq!(
            second.counters.decompressed_bytes,
            counters.decompressed_bytes * 2
        );
        assert_eq!(
            second.counters.extracted_bytes,
            counters.extracted_bytes * 2
        );

        let mut exhausted = Limits::NUCLEUS;
        exhausted.collection_bytes = counters.total();
        let refused = verify(&request, exhausted, counters);
        assert_eq!(refused.value, Err(Refusal::Budget));
        assert_eq!(refused.counters, counters);

        let mut tight = Limits::NUCLEUS;
        tight.collection_bytes = counters.archive_bytes + counters.decompressed_bytes;
        let refused = verify(&request, tight, Counters::default());
        assert_eq!(refused.value, Err(Refusal::Budget));
        assert_eq!(refused.counters.archive_bytes, counters.archive_bytes);
        assert_eq!(
            refused.counters.decompressed_bytes,
            counters.decompressed_bytes
        );
        assert_eq!(refused.counters.extracted_bytes, 0);

        let mut compressed_only = Limits::NUCLEUS;
        compressed_only.collection_bytes = counters.archive_bytes;
        let refused = verify(&request, compressed_only, Counters::default());
        assert_eq!(refused.value, Err(Refusal::Budget));
        assert_eq!(refused.counters.archive_bytes, counters.archive_bytes);
        assert_eq!(refused.counters.decompressed_bytes, 0);

        let bomb_file = vec![0u8; 64 * 1024];
        let bomb_archive_path = scratch.path("bomb.crate");
        fs::write(
            &bomb_archive_path,
            crate_bytes("bomb-1.0.0", &[("Cargo.toml", bomb_file.as_slice())], &[]),
        )
        .unwrap();
        let bomb_root = scratch.path("bomb-1.0.0");
        write_extracted(&bomb_root, &[("Cargo.toml", bomb_file.as_slice())], &[]);
        let bomb_archive = fs::canonicalize(bomb_archive_path).unwrap();
        let bomb_root = fs::canonicalize(bomb_root).unwrap();
        let bomb_sum = digest(&fs::read(&bomb_archive).unwrap());
        let mut bomb_limits = Limits::NUCLEUS;
        bomb_limits.collection_bytes = 512;
        let bomb = verify(
            &ask(&bomb_archive, &bomb_root, &bomb_sum),
            bomb_limits,
            Counters::default(),
        );
        assert_eq!(bomb.value, Err(Refusal::Budget));
        assert!(bomb.counters.total() <= 512);
        assert!(bomb_root.join("Cargo.toml").is_file());

        let mut exact_limits = Limits::NUCLEUS;
        exact_limits.collection_bytes = counters.total();
        let exact = verify(&request, exact_limits, Counters::default());
        assert!(exact.value.is_ok());
        assert_eq!(exact.counters.total(), counters.total());
        let mut over_limits = Limits::NUCLEUS;
        over_limits.collection_bytes = counters.total() - 1;
        let over = verify(&request, over_limits, Counters::default());
        assert_eq!(over.value, Err(Refusal::Budget));
        assert!(over.counters.total() <= counters.total() - 1);
        let mut decomp_over = Limits::NUCLEUS;
        decomp_over.collection_bytes = counters.archive_bytes + counters.decompressed_bytes - 1;
        let decomp = verify(&request, decomp_over, Counters::default());
        assert_eq!(decomp.value, Err(Refusal::Budget));
        assert_eq!(decomp.counters.archive_bytes, counters.archive_bytes);
        assert!(decomp.counters.decompressed_bytes < counters.decompressed_bytes);
        assert_eq!(decomp.counters.extracted_bytes, 0);
        assert!(decomp.counters.total() <= decomp_over.collection_bytes);

        let raw = fs::read(&archive).unwrap();
        let truncated = scratch.path("truncated.crate");
        fs::write(&truncated, &raw[..raw.len() / 2]).unwrap();
        let truncated = fs::canonicalize(truncated).unwrap();
        let truncated_sum = digest(&fs::read(&truncated).unwrap());
        assert!(matches!(
            once(
                &ask(&truncated, &extracted, &truncated_sum),
                Limits::NUCLEUS
            )
            .value,
            Err(Refusal::Integrity | Refusal::Unsupported)
        ));

        let mut trailing = fs::read(&archive).unwrap();
        trailing.extend_from_slice(&gzip(b"second-gzip-member"));
        let extra_gzip = scratch.path("trailing.crate");
        fs::write(&extra_gzip, &trailing).unwrap();
        let extra_gzip = fs::canonicalize(extra_gzip).unwrap();
        let extra_sum = digest(&fs::read(&extra_gzip).unwrap());
        assert_eq!(
            once(&ask(&extra_gzip, &extracted, &extra_sum), Limits::NUCLEUS).value,
            Err(Refusal::Unsupported)
        );

        fs::write(extracted.join("src/lib.rs"), b"mutated-after-success").unwrap();
        assert_eq!(
            once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Integrity)
        );
    }

    #[test]
    fn conversion_requires_accepted_owner_and_does_not_infer_native_closure() {
        let scratch = Scratch::new();
        let (archive, extracted, checksum) = prepare(&scratch);
        let verified = once(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS)
            .value
            .unwrap();
        let attempt = attempt(&scratch);
        assert_eq!(
            TrustedContext::new(
                &StubOwner,
                "",
                "pkg",
                "epoch",
                "registry-receipt",
                &attempt,
                Limits::NUCLEUS
            )
            .unwrap_err(),
            Refusal::Authentication
        );
        assert_eq!(
            verified
                .clone()
                .into_registered(
                    TrustedContext::new(
                        &StubOwner,
                        "synthetic-owner",
                        "pkg-1.0.0",
                        "epoch-1",
                        "registry-receipt",
                        &attempt,
                        Limits::NUCLEUS,
                    )
                    .unwrap(),
                )
                .unwrap_err(),
            Refusal::MissingAuthority
        );

        let owner = AcceptingOwner {
            receipt: "registry-receipt".into(),
            manifest: issued(&verified, "synthetic-owner", "epoch-1"),
        };
        let manifest = verified
            .clone()
            .into_registered(
                TrustedContext::new(
                    &owner,
                    "synthetic-owner",
                    "pkg-1.0.0",
                    "epoch-1",
                    "registry-receipt",
                    &attempt,
                    Limits::NUCLEUS,
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(manifest.issuer, "synthetic-owner");
        assert_eq!(manifest.source_identity, "pkg-1.0.0");
        assert_eq!(manifest.class, ImmutableClass::Registry);
        assert_eq!(manifest.archive.as_ref().unwrap().path, archive);
        assert!(manifest.directories.contains("src"));
        assert_eq!(manifest.files.len(), 2);
        assert!(format!("{manifest:?}").contains("Registry"));

        let mut native = issued(&verified, "synthetic-owner", "epoch-1");
        native.class = ImmutableClass::ToolSdk;
        let native_owner = AcceptingOwner {
            receipt: "registry-receipt".into(),
            manifest: native,
        };
        assert_eq!(
            verified
                .into_registered(
                    TrustedContext::new(
                        &native_owner,
                        "synthetic-owner",
                        "pkg-1.0.0",
                        "epoch-1",
                        "registry-receipt",
                        &attempt,
                        Limits::NUCLEUS,
                    )
                    .unwrap(),
                )
                .unwrap_err(),
            Refusal::Unsupported
        );
    }
}
