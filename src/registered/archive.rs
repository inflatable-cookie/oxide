//! Bounded verifier for a locked Cargo registry archive versus its extraction.
//!
//! The supported form is a single-member gzip of a ustar/pax tar whose members
//! share one canonical package prefix, matching a Cargo `.crate`. Verification
//! is read-only: it does not unpack, fetch, or scan a host registry. Matching
//! bytes do not install an issuer or prove native/proc-macro closure.

use super::{
    canonical_dir, digest, io, receipt_bound, relative, stamp, valid_digest, ArchiveEvidence,
    ImmutableClass, ImmutableManifest, InputEvidenceOwner, Limits, Refusal, Result,
    EVIDENCE_VERSION,
};
use flate2::bufread::GzDecoder;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self as stdio, BufReader, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

/// Local Cargo extraction metadata. Never archive members and never attestations.
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
    limits: Limits,
}
impl std::fmt::Debug for TrustedContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustedContext")
            .field("issuer", &self.issuer)
            .field("source_identity", &self.source_identity)
            .field("epoch", &self.epoch)
            .field("owner", &(self.owner as *const dyn InputEvidenceOwner))
            .finish()
    }
}
impl<'a> TrustedContext<'a> {
    /// Build only from an already-accepted `InputEvidenceOwner`.
    pub fn new(
        owner: &'a dyn InputEvidenceOwner,
        issuer: &'a str,
        source_identity: &'a str,
        epoch: &'a str,
        limits: Limits,
    ) -> Result<Self> {
        limits.validate()?;
        receipt_bound(issuer, limits)?;
        receipt_bound(source_identity, limits)?;
        receipt_bound(epoch, limits)?;
        Ok(Self {
            owner,
            issuer,
            source_identity,
            epoch,
            limits,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedCorrespondence {
    pub package_prefix: String,
    pub locked_checksum: String,
    pub archive: PathBuf,
    pub extracted_root: PathBuf,
    pub files: BTreeMap<String, String>,
    pub directories: BTreeSet<String>,
    pub counters: Counters,
}
impl VerifiedCorrespondence {
    /// Bind verified bytes into registered registry evidence. Does not infer
    /// native, generated or proc-macro provenance.
    pub fn into_registered(self, context: TrustedContext<'_>) -> Result<super::ImmutableManifest> {
        let TrustedContext {
            owner: _,
            issuer,
            source_identity,
            epoch,
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
        Ok(ImmutableManifest {
            version: EVIDENCE_VERSION,
            issuer: issuer.to_owned(),
            source_identity: source_identity.to_owned(),
            epoch: epoch.to_owned(),
            class: ImmutableClass::Registry,
            root: self.extracted_root,
            files: self.files,
            directories: self.directories,
            archive: Some(ArchiveEvidence {
                path: self.archive,
                locked_checksum: self.locked_checksum,
            }),
        })
    }
}

/// Prove archive checksum and extraction membership/content. One shared budget.
pub fn verify(request: &Request<'_>, limits: Limits) -> Outcome<VerifiedCorrespondence> {
    let mut counters = Counters::default();
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
        .take(limit as u64 + 1)
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
                match decoder.read(&mut buf[..1]) {
                    Ok(0) => break,
                    Ok(_) => return Err(Refusal::Budget),
                    Err(_) => return Err(Refusal::Integrity),
                }
            }
            let want = buf.len().min(limit);
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
            directories.insert(relative);
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
            insert_parents(&relative, &files, &mut directories)?;
        }
    }
    let prefix = prefix.ok_or(Refusal::Unsupported)?;
    if files.is_empty() {
        return Err(Refusal::Unsupported);
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
    for entry in fs::read_dir(dir).map_err(io)? {
        bump_entries(counters, limits)?;
        let path = entry.map_err(io)?.path();
        let metadata = fs::symlink_metadata(&path).map_err(io)?;
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
            walk(root, &path, depth + 1, limits, counters, files, directories)?;
        } else if metadata.is_file() {
            if is_sidecar(name) {
                continue;
            }
            files.insert(name.to_owned(), read_extracted(&path, limits, counters)?);
        } else {
            return Err(Refusal::Unsupported);
        }
    }
    Ok(())
}

fn read_extracted(path: &Path, limits: Limits, counters: &mut Counters) -> Result<String> {
    let before = fs::symlink_metadata(path).map_err(io)?;
    if !before.is_file() || before.file_type().is_symlink() {
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
        .take(limit as u64 + 1)
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
        directories.insert(name.to_owned());
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
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_path(name).unwrap();
        header.set_size(0);
        header.set_cksum();
        builder.append(&header, stdio::empty()).unwrap();
    }
    fn append_file(builder: &mut tar::Builder<&mut Vec<u8>>, name: &str, bytes: &[u8]) {
        let mut header = tar::Header::new_ustar();
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
        let outcome = verify(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS);
        let verified = outcome.value.expect("correspondence");
        assert_eq!(verified.package_prefix, "pkg-1.0.0");
        assert_eq!(verified.locked_checksum, checksum);
        assert_eq!(verified.files.len(), 2);
        assert!(verified.files.contains_key("Cargo.toml"));
        assert!(verified.files.contains_key("src/lib.rs"));
        assert!(verified.directories.contains("src"));
        assert!(!verified.files.contains_key(CARGO_OK));
        assert!(!verified.files.contains_key(CARGO_CHECKSUM_JSON));
        assert!(outcome.counters.archive_bytes > 0);
        assert!(outcome.counters.decompressed_bytes > 0);
        assert!(outcome.counters.extracted_bytes > 0);
        let manifest = verified
            .into_registered(
                TrustedContext::new(
                    &StubOwner,
                    "synthetic-owner",
                    "pkg-1.0.0",
                    "epoch-1",
                    Limits::NUCLEUS,
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(manifest.class, ImmutableClass::Registry);
        assert_eq!(manifest.root, extracted);
        assert_eq!(manifest.archive.unwrap().locked_checksum, checksum);
        assert!(manifest
            .files
            .get("src/lib.rs")
            .unwrap()
            .starts_with("sha256:"));
    }

    #[test]
    fn checksum_membership_sidecar_and_path_cases_refuse_independently() {
        let scratch = Scratch::new();
        let (archive, extracted, checksum) = prepare(&scratch);
        assert_eq!(
            verify(
                &ask(&archive, &extracted, "sha256:not-a-digest"),
                Limits::NUCLEUS
            )
            .value,
            Err(Refusal::Identity)
        );
        let wrong = format!("sha256:{}", "ab".repeat(32));
        assert_eq!(
            verify(&ask(&archive, &extracted, &wrong), Limits::NUCLEUS).value,
            Err(Refusal::Integrity)
        );

        fs::write(extracted.join("extra.rs"), b"extra").unwrap();
        assert_eq!(
            verify(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Membership)
        );
        fs::remove_file(extracted.join("extra.rs")).unwrap();
        fs::remove_file(extracted.join("src/lib.rs")).unwrap();
        assert_eq!(
            verify(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Membership)
        );
        fs::write(extracted.join("src/lib.rs"), b"changed").unwrap();
        assert_eq!(
            verify(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Integrity)
        );
        fs::write(extracted.join("src/lib.rs"), b"pub fn n() -> u8 { 1 }\n").unwrap();
        fs::write(extracted.join("src/.cargo-ok"), b"nested sidecar is extra").unwrap();
        assert_eq!(
            verify(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
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
            verify(&ask(&archive, &other, &checksum), Limits::NUCLEUS).value,
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
            verify(
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
            verify(&ask(&symlink_crate, &extracted, &sum), Limits::NUCLEUS).value,
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
            verify(&ask(&hardlink_crate, &extracted, &sum), Limits::NUCLEUS).value,
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
            verify(&ask(&duplicate, &extracted, &sum), Limits::NUCLEUS).value,
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
            verify(&ask(&two_prefix, &extracted, &sum), Limits::NUCLEUS).value,
            Err(Refusal::Unsupported)
        );

        fs::write(scratch.path("plain.tar"), b"not-gzip").unwrap();
        let plain = fs::canonicalize(scratch.path("plain.tar")).unwrap();
        assert_eq!(
            verify(
                &ask(&plain, &extracted, &digest(&fs::read(&plain).unwrap())),
                Limits::NUCLEUS
            )
            .value,
            Err(Refusal::Unsupported)
        );

        symlink(extracted.join("Cargo.toml"), extracted.join("link.rs")).unwrap();
        assert_eq!(
            verify(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Unsupported)
        );
        fs::remove_file(extracted.join("link.rs")).unwrap();
        UnixListener::bind(extracted.join("socket")).unwrap();
        assert_eq!(
            verify(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
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
    fn shared_budget_counts_without_reset_and_refuses_bombs_and_truncation() {
        let scratch = Scratch::new();
        let (archive, extracted, checksum) = prepare(&scratch);
        let request = ask(&archive, &extracted, &checksum);
        let ok = verify(&request, Limits::NUCLEUS);
        let counters = ok.counters;
        assert!(ok.value.is_ok());
        assert!(counters.archive_bytes > 0);
        assert!(counters.decompressed_bytes > 0);
        assert!(counters.extracted_bytes > 0);

        let mut tight = Limits::NUCLEUS;
        tight.collection_bytes = counters.archive_bytes + counters.decompressed_bytes;
        let refused = verify(&request, tight);
        assert_eq!(refused.value, Err(Refusal::Budget));
        assert_eq!(refused.counters.archive_bytes, counters.archive_bytes);
        assert_eq!(
            refused.counters.decompressed_bytes,
            counters.decompressed_bytes
        );
        assert_eq!(refused.counters.extracted_bytes, 0);

        let mut compressed_only = Limits::NUCLEUS;
        compressed_only.collection_bytes = counters.archive_bytes;
        let refused = verify(&request, compressed_only);
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
        let bomb = verify(&ask(&bomb_archive, &bomb_root, &bomb_sum), bomb_limits);
        assert_eq!(bomb.value, Err(Refusal::Budget));
        assert!(bomb.counters.total() <= 512);
        assert!(bomb_root.join("Cargo.toml").is_file());

        let raw = fs::read(&archive).unwrap();
        let truncated = scratch.path("truncated.crate");
        fs::write(&truncated, &raw[..raw.len() / 2]).unwrap();
        let truncated = fs::canonicalize(truncated).unwrap();
        let truncated_sum = digest(&fs::read(&truncated).unwrap());
        assert!(matches!(
            verify(
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
            verify(&ask(&extra_gzip, &extracted, &extra_sum), Limits::NUCLEUS).value,
            Err(Refusal::Unsupported)
        );

        fs::write(extracted.join("src/lib.rs"), b"mutated-after-success").unwrap();
        assert_eq!(
            verify(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS).value,
            Err(Refusal::Integrity)
        );
    }

    #[test]
    fn conversion_requires_accepted_owner_and_does_not_infer_native_closure() {
        let scratch = Scratch::new();
        let (archive, extracted, checksum) = prepare(&scratch);
        let verified = verify(&ask(&archive, &extracted, &checksum), Limits::NUCLEUS)
            .value
            .unwrap();
        assert_eq!(
            TrustedContext::new(&StubOwner, "", "pkg", "epoch", Limits::NUCLEUS).unwrap_err(),
            Refusal::Authentication
        );
        let manifest = verified
            .clone()
            .into_registered(
                TrustedContext::new(
                    &StubOwner,
                    "synthetic-owner",
                    "pkg-1.0.0",
                    "epoch-1",
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
    }
}
