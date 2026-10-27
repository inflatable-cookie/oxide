//! Bounded `sources.json` verification cache.
//!
//! Owning design: `docs/knowledge/contracts/cold-slot-seeding.md` §4.3, §8.1
//! and §9.1. The cache records only facts an earlier full derivation
//! established, so it is never a trust root:
//!
//! - an archive is re-read unless its identity (device, inode, size and mtime)
//!   is unchanged from the recorded verification and the recorded archive
//!   SHA-256 equals the lock's checksum;
//! - the tree is still walked and its canonical digest recomputed on every hit,
//!   and a tree whose own root identity or digest changed is a miss;
//! - a missing, oversized, malformed, symlinked, foreign or unknown-version
//!   document is ignored (read as an empty cache), never repaired or adopted.
//!
//! The sidecar is bounded to 2,048 records and 8 MiB, written owner-only with
//! one atomic temp file plus rename and sync.

use super::digest::{validate_path, TreeDigest, MAX_PATH_BYTES};
use super::Refusal;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Schema of the sidecar document.
pub const CACHE_SCHEMA: &str = "oxide.seed-sources/1";
/// Most records the sidecar may carry.
pub const MAX_CACHE_RECORDS: usize = 2048;
/// Largest sidecar the cache reads or writes.
pub const MAX_CACHE_BYTES: u64 = 8 * 1024 * 1024;
/// Longest `store_id` and `cargo_home` field.
const MAX_STORE_ID_BYTES: usize = 64;
const MAX_HOME_BYTES: usize = 4096;
const NANOS_PER_SECOND: i64 = 1_000_000_000;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

/// One verified package: the archive identity that was authenticated and the
/// tree digest that was derived from it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheRecord {
    /// Absolute Cargo home the archive belongs to.
    pub cargo_home: String,
    /// Archive path relative to `cargo_home`.
    pub archive: String,
    pub archive_device: u64,
    pub archive_inode: u64,
    pub archive_size: u64,
    pub archive_mtime_secs: i64,
    pub archive_mtime_nanos: i64,
    /// Lowercase SHA-256 of the archive bytes.
    pub archive_sha256: String,
    pub tree_device: u64,
    pub tree_inode: u64,
    /// Lowercase canonical tree digest.
    pub tree_digest: String,
    /// Unix seconds of the last successful verification.
    pub verified_at: u64,
}

impl CacheRecord {
    /// The recorded tree digest, or `None` when the text is not a digest.
    pub fn digest(&self) -> Option<TreeDigest> {
        let digest = TreeDigest::from_hex(&self.tree_digest)?;
        if digest.to_hex() != self.tree_digest {
            return None;
        }
        Some(digest)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheDocument {
    schema: String,
    store_id: String,
    records: Vec<CacheRecord>,
}

/// The bounded verification cache for one pool store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceCache {
    store_id: String,
    records: Vec<CacheRecord>,
}

impl SourceCache {
    /// An empty cache for `store_id`.
    pub fn new(store_id: &str) -> Result<Self, Refusal> {
        if !valid_store_id(store_id) {
            return Err(Refusal::CacheInvalid);
        }
        Ok(Self {
            store_id: store_id.to_owned(),
            records: Vec::new(),
        })
    }

    /// Read the sidecar at `path`, or an empty cache when the document is
    /// missing, oversized, malformed, symlinked, foreign or of another store.
    pub fn read(path: &Path, store_id: &str) -> Self {
        let empty = || Self {
            store_id: store_id.to_owned(),
            records: Vec::new(),
        };
        if !valid_store_id(store_id) {
            return empty();
        }
        let Ok(mut file) = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
        else {
            return empty();
        };
        let Ok(metadata) = file.metadata() else {
            return empty();
        };
        if !metadata.file_type().is_file() || metadata.len() > MAX_CACHE_BYTES {
            return empty();
        }
        let mut bytes = Vec::new();
        if (&mut file)
            .take(MAX_CACHE_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)
            .is_err()
            || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_CACHE_BYTES
        {
            return empty();
        }
        let Ok(document) = serde_json::from_slice::<CacheDocument>(&bytes) else {
            return empty();
        };
        if document.schema != CACHE_SCHEMA
            || document.store_id != store_id
            || document.records.len() > MAX_CACHE_RECORDS
        {
            return empty();
        }
        for record in &document.records {
            if validate_record(record).is_err() {
                return empty();
            }
        }
        Self {
            store_id: store_id.to_owned(),
            records: document.records,
        }
    }

    /// The cache's store identity.
    pub fn store_id(&self) -> &str {
        &self.store_id
    }

    /// Number of records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the cache holds no record.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Records in bounded order.
    pub fn records(&self) -> &[CacheRecord] {
        &self.records
    }

    /// One record for an archive path, if any.
    pub fn lookup(&self, cargo_home: &str, archive: &str) -> Option<&CacheRecord> {
        self.records
            .iter()
            .find(|record| record.cargo_home == cargo_home && record.archive == archive)
    }

    /// Insert or replace the record for its archive, evicting the least
    /// recently verified record when the bounded list is full.
    pub fn insert(&mut self, record: CacheRecord) -> Result<(), Refusal> {
        validate_record(&record)?;
        if let Some(existing) = self.records.iter_mut().find(|existing| {
            existing.cargo_home == record.cargo_home && existing.archive == record.archive
        }) {
            *existing = record;
            return Ok(());
        }
        self.records.push(record);
        while self.records.len() > MAX_CACHE_RECORDS {
            let oldest = self
                .records
                .iter()
                .enumerate()
                .min_by(|(_, left), (_, right)| {
                    (left.verified_at, &left.cargo_home, &left.archive).cmp(&(
                        right.verified_at,
                        &right.cargo_home,
                        &right.archive,
                    ))
                })
                .map(|(index, _)| index);
            match oldest {
                Some(index) => {
                    self.records.remove(index);
                }
                None => break,
            }
        }
        Ok(())
    }

    /// Encode the document within its byte bound.
    pub fn encode(&self) -> Result<Vec<u8>, Refusal> {
        if self.records.len() > MAX_CACHE_RECORDS || !valid_store_id(&self.store_id) {
            return Err(Refusal::CacheInvalid);
        }
        for record in &self.records {
            validate_record(record)?;
        }
        let document = CacheDocument {
            schema: CACHE_SCHEMA.to_owned(),
            store_id: self.store_id.clone(),
            records: self.records.clone(),
        };
        let bytes = serde_json::to_vec(&document).map_err(|_| Refusal::CacheInvalid)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_CACHE_BYTES {
            return Err(Refusal::CacheInvalid);
        }
        Ok(bytes)
    }

    /// Write the sidecar atomically and owner-only.
    pub fn write(&self, path: &Path) -> Result<(), Refusal> {
        let bytes = self.encode()?;
        let parent = path.parent().ok_or(Refusal::CacheInvalid)?;
        let name = path
            .file_name()
            .ok_or(Refusal::CacheInvalid)?
            .to_string_lossy()
            .into_owned();
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".{name}.tmp-{}-{unique}", std::process::id()));
        let result: Result<(), std::io::Error> = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, path)?;
            fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
            return Err(Refusal::CacheUnwritable);
        }
        Ok(())
    }
}

fn valid_store_id(store_id: &str) -> bool {
    !store_id.is_empty()
        && store_id.len() <= MAX_STORE_ID_BYTES
        && !store_id.contains('\0')
        && store_id
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'/')
}

fn lowercase_hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_record(record: &CacheRecord) -> Result<(), Refusal> {
    if record.cargo_home.is_empty()
        || record.cargo_home.len() > MAX_HOME_BYTES
        || !record.cargo_home.starts_with('/')
        || record.cargo_home.contains('\0')
    {
        return Err(Refusal::CacheInvalid);
    }
    validate_path(&record.archive, MAX_PATH_BYTES).map_err(|_| Refusal::CacheInvalid)?;
    if !lowercase_hex(&record.archive_sha256, 64) {
        return Err(Refusal::CacheInvalid);
    }
    record.digest().ok_or(Refusal::CacheInvalid)?;
    if record.archive_mtime_nanos <= -NANOS_PER_SECOND
        || record.archive_mtime_nanos >= NANOS_PER_SECOND
    {
        return Err(Refusal::CacheInvalid);
    }
    Ok(())
}
