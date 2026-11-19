//! Bounded source verification for cold-slot seeding (oxide#166, slice 1).
//!
//! Owning design: `docs/knowledge/contracts/cold-slot-seeding.md` §4.3, §4.4,
//! §4.6 and §8.1. This module is the registry verification adapter the later
//! seeding slices use to prove that a donor unit's sources match the
//! recipient's:
//!
//! - [`RegistrySource`] locates the cached `.crate` archive and the extracted
//!   tree Cargo produced from it, refusing an absent or ambiguous registry
//!   index and any source-replaced (vendored) package;
//! - the archive's strict entry grammar derives the expected tree from the
//!   authenticated archive bytes, and the extracted tree is walked and compared
//!   entry by entry;
//! - [`TreeDigest`] is the canonical digest of that content;
//! - [`SourceCache`] is the bounded `sources.json` identity cache;
//!
//! Nothing here invokes Cargo, takes a pool or lease lock, reads control state
//! or writes product state. Every refusal is fail-closed: the caller marks the
//! package ineligible, and nothing is repaired, re-downloaded or fetched.

mod archive;
mod cache;
mod digest;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
mod tree;

pub use cache::{CacheRecord, SourceCache, CACHE_SCHEMA, MAX_CACHE_BYTES, MAX_CACHE_RECORDS};
pub use digest::{ContentHash, DigestScheme, EntryKind, SourceTree, TreeDigest, MAX_PATH_BYTES};

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MIB: u64 = 1024 * 1024;
/// Longest accepted `name-version` archive stem.
const MAX_TARBALL_BYTES: usize = 128;
/// Longest accepted absolute Cargo home in the cache.
const MAX_HOME_BYTES: usize = 4096;
/// Registry index directories examined while locating one package. The listing
/// is incremental and stops at the second match, so no directory can make the
/// lookup list or allocate without bound.
const MAX_REGISTRY_INDEX_ENTRIES: u64 = 1024;

/// Every finite bound one source verification draws on. The defaults are the
/// design's §8.1 figures; a caller sharing one producing-build budget lowers
/// [`SourceLimits::max_derived_bytes`] to what remains.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceLimits {
    /// Directory entries examined per archive derivation and per tree walk.
    pub max_entries: u64,
    /// File content bytes hashed from the extracted tree.
    pub max_content_bytes: u64,
    /// Compressed archive bytes read from one `.crate` file. The design's
    /// derived-content cap bounds decompression; this bounds the disk read.
    pub max_archive_bytes: u64,
    /// Decompressed tar-stream bytes consumed: 512-byte headers, block padding
    /// and the PAX/GNU extension payloads the `tar` reader decodes internally
    /// before it yields an entry. The derived-content cap bounds file content;
    /// this bounds the stream that carries it, including long names.
    pub max_tar_bytes: u64,
    /// Archive-derived content bytes for one package.
    pub max_derived_bytes: u64,
    /// Longest accepted relative path.
    pub max_path_bytes: usize,
    /// Deepest directory below the tree root.
    pub max_depth: usize,
    /// Cooperative deadline for the archive derivation and tree walk.
    pub deadline: Duration,
}

impl Default for SourceLimits {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            max_content_bytes: 128 * MIB,
            max_archive_bytes: 64 * MIB,
            max_tar_bytes: 256 * MIB,
            max_derived_bytes: 64 * MIB,
            max_path_bytes: MAX_PATH_BYTES,
            max_depth: 64,
            deadline: Duration::from_secs(4),
        }
    }
}

/// A fail-closed source-verification refusal. The caller marks the package
/// ineligible; no refusal is ever a cold seed attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// No cached archive exists for the package.
    MissingArchive,
    /// More than one registry index directory holds the cached archive.
    AmbiguousRegistryIndex,
    /// The archive exists but cannot be opened as a regular file.
    ArchiveUnreadable,
    /// More registry index directories were examined than the bound allows.
    RegistryIndexLimit,
    /// An archive entry violates the strict entry grammar.
    ArchiveGrammar(&'static str),
    /// The archive is unparseable or truncated.
    ArchiveCorrupt,
    /// The archive exceeded an entry, byte or deadline bound.
    ArchiveBudget,
    /// The archive bytes do not hash to the lock's checksum.
    ChecksumMismatch,
    /// The extracted tree does not exist beside the cached archive.
    MissingTree,
    /// The extracted tree cannot be read.
    TreeUnreadable,
    /// A tree entry violates the canonical grammar.
    TreeGrammar(&'static str),
    /// The extracted tree differs from the archive-derived tree.
    TreeMismatch(&'static str),
    /// The extracted tree exceeded an entry, content or deadline bound.
    TreeBudget,
    /// A cooperative deadline expired.
    Deadline,
    /// The package's paths or names cannot be represented.
    Unsupported,
    /// A cache record or document cannot be represented.
    CacheInvalid,
    /// The cache sidecar could not be written.
    CacheUnwritable,
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingArchive => formatter.write_str("the cached archive is missing"),
            Self::AmbiguousRegistryIndex => {
                formatter.write_str("the cached archive matches more than one registry index")
            }
            Self::ArchiveUnreadable => formatter.write_str("the cached archive cannot be read"),
            Self::RegistryIndexLimit => {
                formatter.write_str("the registry index listing exceeded its bound")
            }
            Self::ArchiveGrammar(reason) => write!(formatter, "archive entry refused: {reason}"),
            Self::ArchiveCorrupt => {
                formatter.write_str("the archive is not a readable tar archive")
            }
            Self::ArchiveBudget => formatter.write_str("the archive exceeded its bounds"),
            Self::ChecksumMismatch => {
                formatter.write_str("the archive does not match the lock checksum")
            }
            Self::MissingTree => formatter.write_str("the extracted source tree is missing"),
            Self::TreeUnreadable => formatter.write_str("the extracted source tree cannot be read"),
            Self::TreeGrammar(reason) => write!(formatter, "source entry refused: {reason}"),
            Self::TreeMismatch(reason) => {
                write!(
                    formatter,
                    "the source tree differs from the archive: {reason}"
                )
            }
            Self::TreeBudget => formatter.write_str("the source tree exceeded its bounds"),
            Self::Deadline => formatter.write_str("the source verification deadline expired"),
            Self::Unsupported => formatter.write_str("the source location cannot be represented"),
            Self::CacheInvalid => formatter.write_str("the verification cache record is invalid"),
            Self::CacheUnwritable => {
                formatter.write_str("the verification cache cannot be written")
            }
        }
    }
}

/// One located registry package: the cached archive and the tree Cargo
/// extracted from it, both under the same registry index directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegistrySource {
    cargo_home: PathBuf,
    tarball_name: String,
    archive: PathBuf,
    archive_relative: String,
    tree: PathBuf,
}

impl RegistrySource {
    /// Locate the cached archive and extracted tree for `<name>-<version>`.
    ///
    /// The archive is `$CARGO_HOME/registry/cache/<index>/<name>-<version>.crate`
    /// and the tree is `$CARGO_HOME/registry/src/<index>/<name>-<version>` for
    /// the one index directory that holds the archive. Zero matches is
    /// [`Refusal::MissingArchive`]; more than one is
    /// [`Refusal::AmbiguousRegistryIndex`]; a missing tree is
    /// [`Refusal::MissingTree`]. Symlinks are never followed.
    pub fn locate(cargo_home: &Path, tarball_name: &str) -> Result<Self, Refusal> {
        if !cargo_home.is_absolute()
            || cargo_home.as_os_str().len() > MAX_HOME_BYTES
            || cargo_home.to_str().is_none()
        {
            return Err(Refusal::Unsupported);
        }
        if !valid_tarball_name(tarball_name) {
            return Err(Refusal::Unsupported);
        }
        let cache_root = cargo_home.join("registry").join("cache");
        let src_root = cargo_home.join("registry").join("src");
        // Enumerate one index at a time, stopping at the second match: the
        // lookup is bounded in examined entries and never lists the directory
        // whole.
        let mut matches: Vec<(String, PathBuf)> = Vec::new();
        let reader = fs::read_dir(&cache_root).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Refusal::MissingArchive
            } else {
                Refusal::ArchiveUnreadable
            }
        })?;
        let mut examined: u64 = 0;
        for entry in reader {
            examined = examined.saturating_add(1);
            if examined > MAX_REGISTRY_INDEX_ENTRIES {
                return Err(Refusal::RegistryIndexLimit);
            }
            let entry = entry.map_err(|_| Refusal::ArchiveUnreadable)?;
            if !entry
                .file_type()
                .map_err(|_| Refusal::ArchiveUnreadable)?
                .is_dir()
            {
                continue;
            }
            let index = entry
                .file_name()
                .to_str()
                .ok_or(Refusal::Unsupported)?
                .to_owned();
            let candidate = cache_root
                .join(&index)
                .join(format!("{tarball_name}.crate"));
            match fs::symlink_metadata(&candidate) {
                Ok(metadata) => {
                    if !metadata.file_type().is_file() {
                        return Err(Refusal::ArchiveUnreadable);
                    }
                    matches.push((index, candidate));
                    if matches.len() > 1 {
                        return Err(Refusal::AmbiguousRegistryIndex);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(Refusal::ArchiveUnreadable),
            }
        }
        let (index, archive) = match matches.pop() {
            Some(found) => found,
            None => return Err(Refusal::MissingArchive),
        };
        let tree = src_root.join(&index).join(tarball_name);
        let metadata = fs::symlink_metadata(&tree).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Refusal::MissingTree
            } else {
                Refusal::TreeUnreadable
            }
        })?;
        if !metadata.file_type().is_dir() {
            return Err(Refusal::TreeUnreadable);
        }
        let archive_relative = format!("registry/cache/{index}/{tarball_name}.crate");
        Ok(Self {
            cargo_home: cargo_home.to_path_buf(),
            tarball_name: tarball_name.to_owned(),
            archive,
            archive_relative,
            tree,
        })
    }

    /// The Cargo home the package was located under.
    pub fn cargo_home(&self) -> &Path {
        &self.cargo_home
    }

    /// The `<name>-<version>` archive stem.
    pub fn tarball_name(&self) -> &str {
        &self.tarball_name
    }

    /// The cached `.crate` archive.
    pub fn archive(&self) -> &Path {
        &self.archive
    }

    /// The archive path relative to the Cargo home, as the cache records it.
    pub fn archive_relative(&self) -> &str {
        &self.archive_relative
    }

    /// The extracted source tree.
    pub fn tree(&self) -> &Path {
        &self.tree
    }

    /// Verify the package's archive and tree, consulting and updating the
    /// bounded cache.
    pub fn verify(
        &self,
        checksum: &str,
        limits: &SourceLimits,
        cache: &mut SourceCache,
        verified_at: u64,
    ) -> Result<VerifiedRegistrySource, Refusal> {
        let started = Instant::now();
        let checksum = normalize_checksum(checksum).ok_or(Refusal::ChecksumMismatch)?;
        let cargo_home = self
            .cargo_home
            .to_str()
            .ok_or(Refusal::Unsupported)?
            .to_owned();
        if let Some(hit) =
            self.cache_hit(&cargo_home, &checksum, limits, cache, verified_at, started)
        {
            return Ok(hit);
        }
        // The derivation runs while the archive bytes are hashed; its tree is
        // used only after the checksum matches, so an unauthenticated archive
        // can neither be compared with nor recorded for the tree.
        let opened = tree::open_regular(&self.archive)?;
        let identity = opened.identity;
        let derived = archive::derive(&opened.file, &self.tarball_name, limits, started)?;
        let sha256 = digest_hex(&derived.sha256);
        if sha256 != checksum {
            return Err(Refusal::ChecksumMismatch);
        }
        let after = opened
            .file
            .metadata()
            .map_err(|_| Refusal::ArchiveUnreadable)?;
        if tree::FileIdentity::of(&after) != identity {
            return Err(Refusal::ArchiveUnreadable);
        }
        let tree_identity = tree::tree_identity(&self.tree)?;
        let walked = tree::walk_extracted_tree(&self.tree, limits, started)?;
        tree::compare(&derived.tree, &walked.tree)?;
        let digest = walked
            .tree
            .digest(DigestScheme::RegistrySource)
            .map_err(|_| Refusal::Unsupported)?;
        if derived
            .tree
            .digest(DigestScheme::RegistrySource)
            .map_err(|_| Refusal::Unsupported)?
            != digest
        {
            return Err(Refusal::TreeMismatch("digest"));
        }
        cache.insert(CacheRecord {
            cargo_home,
            archive: self.archive_relative.clone(),
            archive_device: identity.device,
            archive_inode: identity.inode,
            archive_size: identity.size,
            archive_mtime_secs: identity.mtime_secs,
            archive_mtime_nanos: identity.mtime_nanos,
            archive_sha256: sha256.clone(),
            tree_device: tree_identity.device,
            tree_inode: tree_identity.inode,
            tree_digest: digest.to_hex(),
            verified_at,
        })?;
        Ok(VerifiedRegistrySource {
            digest,
            entries: u64::try_from(walked.tree.len()).unwrap_or(u64::MAX),
            content_bytes: walked.content_bytes,
            derived_bytes: derived.content_bytes,
            archive_sha256: sha256,
            cache_hit: false,
        })
    }

    fn cache_hit(
        &self,
        cargo_home: &str,
        checksum: &str,
        limits: &SourceLimits,
        cache: &mut SourceCache,
        verified_at: u64,
        started: Instant,
    ) -> Option<VerifiedRegistrySource> {
        let record = cache.lookup(cargo_home, &self.archive_relative)?.clone();
        if record.archive_sha256 != checksum {
            return None;
        }
        // §4.3 lists a missing or unreadable archive among the identity changes
        // that force a full re-derivation or a skip, so a hit must open the
        // archive (no follow) and take the descriptor's identity. This
        // establishes readability and identity only; no content byte is read,
        // so the hit still does not re-read or re-hash the archive.
        let identity = tree::open_regular(&self.archive).ok()?.identity;
        if identity.device != record.archive_device
            || identity.inode != record.archive_inode
            || identity.size != record.archive_size
            || identity.mtime_secs != record.archive_mtime_secs
            || identity.mtime_nanos != record.archive_mtime_nanos
        {
            return None;
        }
        let tree_identity = tree::tree_identity(&self.tree).ok()?;
        if tree_identity.device != record.tree_device || tree_identity.inode != record.tree_inode {
            return None;
        }
        let recorded = record.digest()?;
        let walked = tree::walk_extracted_tree(&self.tree, limits, started).ok()?;
        let digest = walked.tree.digest(DigestScheme::RegistrySource).ok()?;
        if digest != recorded {
            return None;
        }
        let mut touched = record;
        touched.verified_at = verified_at;
        cache.insert(touched).ok()?;
        Some(VerifiedRegistrySource {
            digest,
            entries: u64::try_from(walked.tree.len()).unwrap_or(u64::MAX),
            content_bytes: walked.content_bytes,
            derived_bytes: 0,
            archive_sha256: checksum.to_owned(),
            cache_hit: true,
        })
    }
}

/// A verified registry package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedRegistrySource {
    /// Canonical tree digest of the extracted tree.
    pub digest: TreeDigest,
    /// Canonical entries (directories and files).
    pub entries: u64,
    /// File content bytes hashed from the tree.
    pub content_bytes: u64,
    /// Archive-derived content bytes, charged against the producing-build
    /// archive budget.
    pub derived_bytes: u64,
    /// Lowercase SHA-256 of the authenticated archive bytes.
    pub archive_sha256: String,
    /// Whether the cache supplied the archive digest without re-reading it.
    pub cache_hit: bool,
}

fn valid_tarball_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_TARBALL_BYTES
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'+'))
}

fn normalize_checksum(checksum: &str) -> Option<String> {
    let bytes = checksum.as_bytes();
    if bytes.len() != 64 || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    Some(
        checksum
            .chars()
            .map(|character| character.to_ascii_lowercase())
            .collect(),
    )
}

fn digest_hex(digest: &[u8; 32]) -> String {
    digest::hex_bytes(digest)
}
