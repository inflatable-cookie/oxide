//! Per-deletion custody for whole-slot deletion (oxide#181, frozen design
//! oxide#180 "Deferred whole-slot deletion").
//!
//! Every recorded `Evicting` slot owns one private, owner-only control
//! directory below `<pool>/oxide-managed/deletions/<nonce>`. It holds two
//! files:
//!
//! - `lock`: a stable, zero-length regular file whose `(device, inode)` the
//!   ledger record names. The deleter holds an exclusive `flock` on it for the
//!   entire deletion, including journal replay and final record removal. It is
//!   never unlinked or replaced while the record can be observed.
//! - `journal.json`: the bounded, atomically replaced, synced per-deletion
//!   journal (the oxide#155 capture journal, oxide#175 ordering) bound to the
//!   deletion nonce and the record's identities.
//!
//! The protocol is fixed:
//!
//! 1. Under the pool lock, create the directory, its lock file and the initial
//!    journal durably; sync both before the ledger record references them.
//! 2. Publish the `Evicting` record with the custody reference (the nonce and
//!    the lock file identity) in the same atomic ledger write that carries the
//!    `capacity_policy: 5` fence.
//! 3. Claim the exclusive lock **nonblocking while holding the pool lock**.
//!    Lock order is always pool lock then custody; a pool-lock holder never
//!    waits on custody.
//! 4. A busy lock is positive evidence of a live custodian: skip the record.
//!    A missing, replaced or unverifiable lock is unknown: keep the record.
//!    A successfully acquired lock is exclusive custody, including after a
//!    crash.
//! 5. After reclaiming the pool lock and dropping the matching record, remove
//!    the private control directory while custody is still held.
//!
//! The lock is never inherited into children (`O_CLOEXEC`) and no PID age,
//! timeout or presumed death grants custody.

use super::pool::{DeletionJournal, MAX_FOREIGN_CAPTURES, MAX_OUTSTANDING_CAPTURES};
use super::{
    current_uid, ensure_private_dir, io_error, random_hex, read_private_file_bounded, LeaseError,
    MANAGED_NAME, MAX_PATH_BYTES,
};
use crate::owned_delete::{
    CaptureRecord, ForeignCapture, RootCaptureJournal, RootCapturePhase, RootCapturePlan,
};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Private holdings directory below the front door's control directory.
pub(super) const HOLDINGS_NAME: &str = "deletions";
/// The stable lock file every custody directory holds.
pub(super) const LOCK_FILE: &str = "lock";
/// The per-deletion journal file.
pub(super) const JOURNAL_FILE: &str = "journal.json";
/// Version of the per-deletion journal document.
pub(super) const JOURNAL_VERSION: u32 = 1;
/// A nonce is 16 random bytes, printed as 32 lowercase hex characters.
pub(super) const NONCE_BYTES: usize = 16;
pub(super) const NONCE_TEXT_BYTES: usize = NONCE_BYTES * 2;
/// Worst valid per-deletion journal. The journal no longer shares the ledger's
/// byte bound: this is its own generous bound, above the maximal in-ledger
/// journal the writer could previously reach (96 outstanding captures and 32
/// foreign captures with maximal name chains) and bounded per object by the
/// same `is_valid` rule.
pub(super) const MAX_JOURNAL_BYTES: u64 = 4 * 1024 * 1024;

/// The ledger's reference to one deletion's private custody directory.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeletionCustody {
    /// The directory name; the nonce binds the journal and directory to this
    /// deletion.
    pub(super) nonce: String,
    /// Identity of the stable lock file at publication.
    pub(super) lock_device: u64,
    pub(super) lock_inode: u64,
}

impl DeletionCustody {
    pub(super) fn is_valid(&self) -> bool {
        valid_nonce(&self.nonce) && self.lock_inode != 0
    }
}

/// One nonce is exactly 32 lowercase hex characters.
fn valid_nonce(nonce: &str) -> bool {
    nonce.len() == NONCE_TEXT_BYTES
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// One bounded per-deletion journal document. Every identity the deletion was
/// published under is repeated here, so a claimant can prove the file it read
/// belongs to the record it holds.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct JournalDocument {
    version: u32,
    nonce: String,
    store_id: String,
    backend: String,
    slot: u64,
    incarnation: u64,
    /// The slot directory relative to the payload's `slots` root: the
    /// recorded `dir`, or the legacy `<id>` name.
    root: String,
    /// `None` until the seam plans the root capture, so the directory, the
    /// lock and an initial journal are durable before the ledger record that
    /// references them.
    capture: Option<DeletionJournal>,
}

impl JournalDocument {
    fn is_well_formed(&self) -> bool {
        self.version == JOURNAL_VERSION
            && valid_nonce(&self.nonce)
            && self.slot != 0
            && self.incarnation != 0
            && !self.root.is_empty()
            && self.root.len() <= MAX_PATH_BYTES
            && self.store_id.len() <= 64
            && self.backend.len() <= 64
            && self.capture.as_ref().is_none_or(DeletionJournal::is_valid)
    }
}

/// The identities a custody claim is checked against: the record the caller is
/// holding and the pool it belongs to.
#[derive(Clone, Debug)]
pub(super) struct CustodyIdentity {
    pub(super) store_id: String,
    pub(super) backend: String,
    pub(super) slot: u64,
    pub(super) incarnation: u64,
    pub(super) root: String,
}

/// What claiming one record's custody settled.
pub(super) enum Claim {
    /// A live custodian holds the exclusive lock. The record stays charged and
    /// this caller skips it without touching its payload.
    Busy,
    /// The lock, the directory or the journal is missing, replaced, unreadable
    /// or not the object the record names. The record stays charged.
    Unknown,
    /// Exclusive custody of exactly this deletion.
    Claimed(CustodyLock),
}

/// Exclusive custody of one recorded deletion: the held lock file, the
/// verified directory, and the current journal.
pub(super) struct CustodyLock {
    nonce: String,
    dir: PathBuf,
    journal_path: PathBuf,
    /// Identity of the directory this claim verified, re-checked before
    /// cleanup so a replaced directory is never removed.
    dir_device: u64,
    dir_inode: u64,
    lock_device: u64,
    lock_inode: u64,
    file: File,
    identity: CustodyIdentityLines,
    journal: Option<DeletionJournal>,
}

/// Owned copies of the identity facts a held custody keeps for verification
/// and journal rewriting.
#[derive(Clone, Debug)]
struct CustodyIdentityLines {
    store_id: String,
    backend: String,
    slot: u64,
    incarnation: u64,
    root: String,
}

impl CustodyIdentityLines {
    fn of(identity: &CustodyIdentity) -> Self {
        Self {
            store_id: identity.store_id.to_owned(),
            backend: identity.backend.to_owned(),
            slot: identity.slot,
            incarnation: identity.incarnation,
            root: identity.root.to_owned(),
        }
    }

    fn matches(&self, identity: &CustodyIdentity) -> bool {
        self.store_id == identity.store_id.as_str()
            && self.backend == identity.backend.as_str()
            && self.slot == identity.slot
            && self.incarnation == identity.incarnation
            && self.root == identity.root.as_str()
    }
}

impl CustodyLock {
    /// The custody reference this deletion publishes in the ledger.
    pub(super) fn record(&self) -> DeletionCustody {
        DeletionCustody {
            nonce: self.nonce.clone(),
            lock_device: self.lock_device,
            lock_inode: self.lock_inode,
        }
    }

    /// The current journal, as this custody last wrote it.
    pub(super) fn journal(&self) -> Option<&DeletionJournal> {
        self.journal.as_ref()
    }

    /// Replace the journal durably. A journal that is not one this writer
    /// produces is refused and leaves the previous file untouched. `None`
    /// writes the initial (or rolled-back) journal: the deletion has no
    /// capture plan yet.
    pub(super) fn replace_journal(
        &mut self,
        journal: Option<DeletionJournal>,
    ) -> Result<(), String> {
        if journal.as_ref().is_some_and(|journal| !journal.is_valid()) {
            return Err("the capture journal is not one a writer produces".to_owned());
        }
        let document = JournalDocument {
            version: JOURNAL_VERSION,
            nonce: self.nonce.clone(),
            store_id: self.identity.store_id.clone(),
            backend: self.identity.backend.clone(),
            slot: self.identity.slot,
            incarnation: self.identity.incarnation,
            root: self.identity.root.clone(),
            capture: journal.clone(),
        };
        let bytes = serde_json::to_vec(&document)
            .map_err(|error| format!("the deletion journal could not be encoded ({error})"))?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err("the deletion journal exceeds its byte bound".to_owned());
        }
        persist_journal(&self.journal_path, &bytes).map_err(|error| error.to_string())?;
        self.journal = journal;
        Ok(())
    }

    /// Remove this custody directory while custody is still held. The record
    /// that referenced it is already durably dropped, so no claimant can be
    /// waiting on it. A directory whose identity changed is left alone: an
    /// unverified object is never removed.
    pub(super) fn retire(mut self) {
        let Ok(metadata) = fs::symlink_metadata(&self.dir) else {
            return;
        };
        if metadata.file_type().is_symlink()
            || !metadata.file_type().is_dir()
            || metadata.dev() != self.dir_device
            || metadata.ino() != self.dir_inode
        {
            return;
        }
        let _ = fs::remove_file(&self.journal_path);
        let lock_path = self.dir.join(LOCK_FILE);
        // Keep the descriptor open until the directory is gone so no other
        // claimant can acquire this lock while the names are removed.
        let _ = fs::remove_file(&lock_path);
        if let Ok(directory) = File::open(&self.dir) {
            let _ = directory.sync_all();
        }
        if fs::remove_dir(&self.dir).is_ok() {
            if let Some(parent) = self.dir.parent() {
                if let Ok(directory) = File::open(parent) {
                    let _ = directory.sync_all();
                }
            }
        }
        // Drop the lock file's descriptor only after the names are gone.
        let _ = &self.file;
    }
}

/// Take the exclusive OS lock on `file` nonblocking. `Ok(true)` means this
/// open file description now holds it, `Ok(false)` means another holder has it
/// (a live custodian), and an error is unknown custody. The lock rides on the
/// descriptor, so it is released exactly when that `File` is dropped and never
/// inherited by a child (`O_CLOEXEC`).
fn lock_exclusive(file: &File) -> Result<bool, LeaseError> {
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return Ok(false);
    }
    Err(io_error(error))
}

fn custody_root(pool: &Path) -> PathBuf {
    pool.join(MANAGED_NAME).join(HOLDINGS_NAME)
}

fn custody_dir(pool: &Path, nonce: &str) -> PathBuf {
    custody_root(pool).join(nonce)
}

fn journal_path(pool: &Path, nonce: &str) -> PathBuf {
    custody_dir(pool, nonce).join(JOURNAL_FILE)
}

/// Encode one journal document for `identity`.
fn document_bytes(
    nonce: &str,
    identity: &CustodyIdentityLines,
    journal: Option<&DeletionJournal>,
) -> Result<Vec<u8>, LeaseError> {
    let document = JournalDocument {
        version: JOURNAL_VERSION,
        nonce: nonce.to_owned(),
        store_id: identity.store_id.clone(),
        backend: identity.backend.clone(),
        slot: identity.slot,
        incarnation: identity.incarnation,
        root: identity.root.clone(),
        capture: journal.cloned(),
    };
    let bytes = serde_json::to_vec(&document).map_err(|error| LeaseError::Io(error.to_string()))?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(LeaseError::LedgerExhausted);
    }
    Ok(bytes)
}

/// Atomically replace one custody journal: temp file, sync, rename, sync the
/// directory.
fn persist_journal(path: &Path, bytes: &[u8]) -> Result<(), LeaseError> {
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(LeaseError::LedgerExhausted);
    }
    let parent = path
        .parent()
        .ok_or(LeaseError::InvalidInput("journal has no parent"))?;
    let mut temp_name = String::from("journal.tmp-");
    temp_name.push_str(&random_hex(8)?);
    let temp_path = parent.join(temp_name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(&temp_path).map_err(io_error)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temp_path);
        return Err(io_error(error));
    }
    drop(file);
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(io_error(error));
    }
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)
}

/// Create this deletion's private control directory, stable lock file and
/// initial journal durably, then claim the lock. The caller holds the pool
/// lock and publishes the ledger record only after this returns.
pub(super) fn create(
    pool: &Path,
    identity: &CustodyIdentity,
    journal: Option<&DeletionJournal>,
) -> Result<CustodyLock, LeaseError> {
    let lines = CustodyIdentityLines::of(identity);
    let control = pool.join(MANAGED_NAME);
    ensure_private_dir(&control)?;
    let root = custody_root(pool);
    ensure_private_dir(&root)?;
    let mut attempt = 0;
    let nonce = loop {
        let nonce = random_hex(NONCE_BYTES)?;
        let dir = custody_dir(pool, &nonce);
        match fs::create_dir(&dir) {
            Ok(()) => break nonce,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && attempt < 4 => {
                attempt += 1;
                continue;
            }
            Err(error) => return Err(io_error(error)),
        }
    };
    let dir = custody_dir(pool, &nonce);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    let lock_path = dir.join(LOCK_FILE);
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let file = options.open(&lock_path).map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file()
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(LeaseError::UnsafePath(
            "the custody lock must be a private regular file",
        ));
    }
    // The creator takes the exclusive lock before it writes the journal or
    // returns a `CustodyLock`: from here on this deletion has one live
    // custodian, and a caller can never publish or replay under a lock it does
    // not hold. The file was just created owner-only in a fresh private
    // directory, so a busy lock is a state this writer never produces.
    if !lock_exclusive(&file)? {
        return Err(LeaseError::UnknownAccounting);
    }
    file.sync_all().map_err(io_error)?;
    let bytes = document_bytes(&nonce, &lines, journal)?;
    persist_journal(&journal_path(pool, &nonce), &bytes)?;
    let dir_metadata = fs::symlink_metadata(&dir).map_err(io_error)?;
    if dir_metadata.file_type().is_symlink()
        || !dir_metadata.file_type().is_dir()
        || dir_metadata.uid() != current_uid()
    {
        return Err(LeaseError::UnsafePath(
            "the custody directory must be an owned directory",
        ));
    }
    // The directory and both names are durable before the record references
    // them.
    for directory in [&dir, &root, &control] {
        File::open(directory)
            .and_then(|held| held.sync_all())
            .map_err(io_error)?;
    }
    // Re-verify the names and read the journal under the lock just taken. Any
    // mismatch is unknown, and the record is not yet published, so the caller
    // keeps nothing.
    match claim_file(
        &nonce,
        &dir,
        dir_metadata.dev(),
        dir_metadata.ino(),
        metadata.dev(),
        metadata.ino(),
        file,
    )? {
        Claim::Claimed(lock) => Ok(lock),
        Claim::Busy | Claim::Unknown => Err(LeaseError::UnknownAccounting),
    }
}

/// Claim one published record's custody. The caller holds the pool lock.
pub(super) fn claim(
    pool: &Path,
    custody: &DeletionCustody,
    identity: &CustodyIdentity,
) -> Result<Claim, LeaseError> {
    if !custody.is_valid() || !CustodyIdentityLines::of(identity).is_well_formed() {
        return Ok(Claim::Unknown);
    }
    let dir = custody_dir(pool, &custody.nonce);
    let dir_metadata = match fs::symlink_metadata(&dir) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(Claim::Unknown),
    };
    if dir_metadata.file_type().is_symlink()
        || !dir_metadata.file_type().is_dir()
        || dir_metadata.uid() != current_uid()
        || dir_metadata.permissions().mode() & 0o077 != 0
    {
        return Ok(Claim::Unknown);
    }
    let lock_path = dir.join(LOCK_FILE);
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let file = match options.open(&lock_path) {
        Ok(file) => file,
        Err(_) => return Ok(Claim::Unknown),
    };
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return Ok(Claim::Unknown),
    };
    if !metadata.is_file()
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.dev() != custody.lock_device
        || metadata.ino() != custody.lock_inode
    {
        return Ok(Claim::Unknown);
    }
    // Nonblocking: a live custodian is skipped, never waited for. A lock this
    // caller cannot take or verify is unknown custody: the record keeps its
    // charge rather than failing the whole open.
    match lock_exclusive(&file) {
        Ok(true) => {}
        Ok(false) => return Ok(Claim::Busy),
        Err(_) => return Ok(Claim::Unknown),
    }
    claim_file(
        &custody.nonce,
        &dir,
        dir_metadata.dev(),
        dir_metadata.ino(),
        metadata.dev(),
        metadata.ino(),
        file,
    )
}

/// Re-verify the directory, the already-held lock and the journal, and read
/// the journal. The caller must hold the exclusive lock on `file`; this
/// function never takes it. Anything the caller cannot verify is unknown
/// custody, never an empty journal.
#[allow(clippy::too_many_arguments)]
fn claim_file(
    nonce: &str,
    dir: &Path,
    dir_device: u64,
    dir_inode: u64,
    lock_device: u64,
    lock_inode: u64,
    file: File,
) -> Result<Claim, LeaseError> {
    // Re-read the directory and lock through the descriptors/paths we hold:
    // a replacement between the metadata read and the open is refused.
    let dir_metadata = match fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(Claim::Unknown),
    };
    let held = match file.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return Ok(Claim::Unknown),
    };
    if dir_metadata.dev() != dir_device
        || dir_metadata.ino() != dir_inode
        || held.dev() != lock_device
        || held.ino() != lock_inode
        || !held.is_file()
    {
        return Ok(Claim::Unknown);
    }
    let journal_path = dir.join(JOURNAL_FILE);
    let bytes = match read_private_file_bounded(&journal_path, MAX_JOURNAL_BYTES) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(Claim::Unknown),
    };
    let document: JournalDocument = match serde_json::from_slice(&bytes) {
        Ok(document) => document,
        Err(_) => return Ok(Claim::Unknown),
    };
    if !document.is_well_formed() || document.nonce != nonce {
        return Ok(Claim::Unknown);
    }
    Ok(Claim::Claimed(CustodyLock {
        nonce: nonce.to_owned(),
        dir: dir.to_path_buf(),
        journal_path,
        dir_device,
        dir_inode,
        lock_device,
        lock_inode,
        file,
        identity: CustodyIdentityLines {
            store_id: document.store_id,
            backend: document.backend,
            slot: document.slot,
            incarnation: document.incarnation,
            root: document.root,
        },
        journal: document.capture,
    }))
}

impl CustodyIdentityLines {
    fn is_well_formed(&self) -> bool {
        self.slot != 0
            && self.incarnation != 0
            && !self.root.is_empty()
            && self.root.len() <= MAX_PATH_BYTES
            && self.store_id.len() <= 64
            && self.backend.len() <= 64
    }
}

impl CustodyLock {
    /// Prove this held custody is exactly the record the caller is holding.
    /// A claimant rechecks this under the pool lock after acquiring custody.
    pub(super) fn matches(&self, identity: &CustodyIdentity) -> bool {
        self.identity.matches(identity)
    }
}

/// Read one published deletion's journal without taking custody: read-only
/// accounting uses this, it takes no lock, writes nothing and repairs nothing.
pub(super) fn peek(
    pool: &Path,
    custody: &DeletionCustody,
    identity: &CustodyIdentity,
) -> Result<DeletionJournal, LeaseError> {
    if !custody.is_valid() {
        return Err(LeaseError::UnknownAccounting);
    }
    let dir = custody_dir(pool, &custody.nonce);
    let metadata = fs::symlink_metadata(&dir).map_err(io_error)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(LeaseError::UnknownAccounting);
    }
    let lock = fs::symlink_metadata(dir.join(LOCK_FILE)).map_err(io_error)?;
    if !lock.is_file() || lock.dev() != custody.lock_device || lock.ino() != custody.lock_inode {
        return Err(LeaseError::UnknownAccounting);
    }
    let bytes = read_private_file_bounded(&dir.join(JOURNAL_FILE), MAX_JOURNAL_BYTES)?;
    let document: JournalDocument =
        serde_json::from_slice(&bytes).map_err(|_| LeaseError::UnknownAccounting)?;
    if !document.is_well_formed() || document.nonce != custody.nonce {
        return Err(LeaseError::UnknownAccounting);
    }
    let lines = CustodyIdentityLines {
        store_id: document.store_id,
        backend: document.backend,
        slot: document.slot,
        incarnation: document.incarnation,
        root: document.root,
    };
    if !lines.matches(identity) {
        return Err(LeaseError::UnknownAccounting);
    }
    document.capture.ok_or(LeaseError::UnknownAccounting)
}

impl RootCaptureJournal for CustodyLock {
    fn planned(&mut self, plan: &RootCapturePlan) -> Result<(), String> {
        // The first plan starts the journal; a replay that rolled back updates
        // the retained plan.
        let mut journal = self.journal.clone().unwrap_or(DeletionJournal {
            root: plan.clone(),
            captures: Vec::new(),
            foreign: Vec::new(),
        });
        journal.root = plan.clone();
        self.replace_journal(Some(journal))?;
        // The plan is durable and the payload is untouched.
        crate::proof::deletion_pause("journal-planned");
        Ok(())
    }

    fn captured(&mut self, plan: &RootCapturePlan) -> Result<(), String> {
        let mut journal = self.open()?;
        journal.root = RootCapturePlan {
            phase: RootCapturePhase::Captured,
            ..plan.clone()
        };
        self.replace_journal(Some(journal))?;
        // The exclusive rename is durable and the tree is untouched.
        crate::proof::deletion_pause("journal-captured");
        Ok(())
    }

    fn record_captured(&mut self, record: &CaptureRecord) -> Result<(), String> {
        let mut journal = self.open()?;
        if journal
            .captures
            .iter()
            .any(|existing| existing.private == record.private)
        {
            return Err("a capture record already names this private name".to_owned());
        }
        if journal.captures.len() >= MAX_OUTSTANDING_CAPTURES {
            return Err("the deletion has more outstanding captures than it may record".to_owned());
        }
        journal.captures.push(record.clone());
        self.replace_journal(Some(journal))
    }

    fn record_removed(&mut self, record: &CaptureRecord) -> Result<(), String> {
        let mut journal = self.open()?;
        let position = journal
            .captures
            .iter()
            .position(|existing| existing.private == record.private && existing.path == record.path)
            .ok_or("the recorded capture is not the one this deletion recorded")?;
        journal.captures.remove(position);
        self.replace_journal(Some(journal))
    }

    fn record_foreign(&mut self, capture: &ForeignCapture) -> Result<(), String> {
        let mut journal = self.open()?;
        if let Some(existing) = journal
            .foreign
            .iter_mut()
            .find(|existing| existing.private == capture.private)
        {
            *existing = capture.clone();
            return self.replace_journal(Some(journal));
        }
        if journal.foreign.len() >= MAX_FOREIGN_CAPTURES {
            return Err("the deletion has more foreign captures than it may record".to_owned());
        }
        journal.foreign.push(capture.clone());
        self.replace_journal(Some(journal))
    }

    fn record_root_removed(&mut self, plan: &RootCapturePlan) -> Result<(), String> {
        let mut journal = self.open()?;
        journal.root = RootCapturePlan {
            phase: RootCapturePhase::Removed,
            ..plan.clone()
        };
        self.replace_journal(Some(journal))
    }
}

impl CustodyLock {
    /// The journal this deletion has already planned. A capture record without
    /// one is a state the seam never produces.
    fn open(&self) -> Result<DeletionJournal, String> {
        self.journal
            .clone()
            .ok_or_else(|| "the recorded deletion has no journal".to_owned())
    }
}
