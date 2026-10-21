//! Same-user leases over durable exclusive slots in a budget-only pool, used
//! by the `oxide` front door.
//!
//! A short-lived OS lock plus an atomic local ledger, no resident daemon,
//! HMAC, issuer, callback service or Cargo invocation. Same-UID callers are
//! trusted cooperators; private paths and safety checks still refuse foreign
//! paths and symlinked roots. A durably recorded idle deletion unlinks an
//! owned nested symlink entry without following its target. The standalone
//! lease CLI and the APFS sparse-image backend are retired (oxide#102); a
//! pool recording that backend refuses to open. Owning contract:
//! `docs/knowledge/contracts/workflow.md`.

mod backing;
mod pool;

use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub use pool::{managed, LeaseRequest};

#[cfg(test)]
mod tests;

/// Version of the lease JSON, receipts, pool config and ledger.
pub const LEASE_VERSION: u32 = 1;
/// Budget-only directory backend for the `oxide` front door: admission and
/// whole-idle-slot eviction against a soft budget, with no OS ceiling.
pub const BUDGET_BACKEND_DIRECTORY_V1: &str = "directory-budget-v1";
/// Removable-volume backend for the `oxide` front door: config, ledger and
/// lock internal, payload in a directory on a registered external APFS
/// volume. Budget-only like the directory backend.
pub const VOLUME_BACKEND_APFS_V1: &str = "apfs-volume-v1";
/// Finite slot count. Exhaustion refuses a new slot.
pub const MAX_SLOTS: usize = 32;
/// Finite ledger file size including the atomic temp.
pub const MAX_LEDGER_BYTES: u64 = 256 * 1024;
/// Finite holder identity.
pub const MAX_HOLDER_BYTES: usize = 128;
/// Finite canonical path.
pub const MAX_PATH_BYTES: usize = 1024;
/// Finite lease-id text.
pub const MAX_LEASE_ID_BYTES: usize = 160;
/// Cooperative inventory bounds for pool accounting (oxide#108), separate
/// from the inventory tool's defaults. Sized for real build caches: 32 slots
/// of a 60k-entry Cargo build directory (Farmyard's observed 60,657) stay
/// under the entry bound; depth covers build-script `out` trees; the
/// deadline allows a cold walk of a USB pool. A walk that hits any bound is
/// incomplete and refuses where complete accounting is required.
const ACCOUNTING_MAX_ENTRIES: u64 = 2_000_000;
const ACCOUNTING_MAX_DEPTH: usize = 64;
const ACCOUNTING_MAX_ELAPSED_MS: u64 = 60_000;

const CONFIG_NAME: &str = "config.json";
const LEDGER_NAME: &str = "ledger.json";
const LOCK_NAME: &str = "lock";
const IMAGE_NAME: &str = "image.sparseimage";
const RECEIPT_NAME: &str = "receipt.json";
const PAYLOAD_NAME: &str = "payload";
const SLOTS_NAME: &str = "slots";
/// Private control directory owned by the `oxide` front door. The lease CLI
/// tolerates it as a real directory and never reads or writes it.
pub const MANAGED_NAME: &str = "oxide-managed";

/// Caller-selected Cargo profile label. The CLI records it and returns it; it
/// does not force incremental, debug or feature flags.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    Dev,
    FullDebug,
}

impl Profile {
    pub fn parse(value: &str) -> Result<Self, LeaseError> {
        match value {
            "dev" => Ok(Self::Dev),
            "full-debug" => Ok(Self::FullDebug),
            _ => Err(LeaseError::InvalidInput(
                "profile must be dev or full-debug",
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::FullDebug => "full-debug",
        }
    }
}

/// Occupancy recorded in the ledger and shown by status.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotOccupancy {
    Held,
    Idle,
    /// Reserved. This version never infers abandoned from TTL, PID or holder
    /// loss; recover still asserts writer closure on a held lease.
    Abandoned,
}

/// Why a command refused. Mapped to a small exit set by [`LeaseError::exit_code`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LeaseError {
    Usage(&'static str),
    InvalidInput(&'static str),
    Busy,
    Full,
    UnknownAccounting,
    /// An entry the pool's shape does not allow, named by its place and an
    /// escaped name. Same `unknown_accounting` category and exit code.
    UnexpectedEntry {
        place: &'static str,
        entry: String,
    },
    /// A bounded accounting walk that admission, reuse or eviction needed
    /// did not complete. Same reason and exit code as `UnknownAccounting`.
    AccountingIncomplete(Box<AccountingScan>),
    UnsafePath(&'static str),
    HolderConflict,
    NotFound,
    BackingMismatch,
    LedgerExhausted,
    Unsupported(&'static str),
    Io(String),
}

impl LeaseError {
    /// 0 is success. 1 usage/invalid/io. 2 busy or holder conflict. 3 full,
    /// unknown accounting or ledger exhaustion. 4 unsafe path or mismatched
    /// backing. 5 unknown lease.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) | Self::InvalidInput(_) | Self::Io(_) | Self::Unsupported(_) => 1,
            Self::Busy | Self::HolderConflict => 2,
            Self::Full
            | Self::UnknownAccounting
            | Self::UnexpectedEntry { .. }
            | Self::AccountingIncomplete(_)
            | Self::LedgerExhausted => 3,
            Self::UnsafePath(_) | Self::BackingMismatch => 4,
            Self::NotFound => 5,
        }
    }

    pub fn reason(&self) -> &'static str {
        match self {
            Self::Usage(_) => "usage",
            Self::InvalidInput(_) => "invalid_input",
            Self::Busy => "busy",
            Self::Full => "full",
            Self::UnknownAccounting
            | Self::UnexpectedEntry { .. }
            | Self::AccountingIncomplete(_) => "unknown_accounting",
            Self::UnsafePath(_) => "unsafe_path",
            Self::HolderConflict => "holder_conflict",
            Self::NotFound => "not_found",
            Self::BackingMismatch => "backing_mismatch",
            Self::LedgerExhausted => "ledger_exhausted",
            Self::Unsupported(_) => "unsupported",
            Self::Io(_) => "io",
        }
    }
}

impl std::fmt::Display for LeaseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(message) | Self::InvalidInput(message) | Self::UnsafePath(message) => {
                formatter.write_str(message)
            }
            Self::Busy => formatter.write_str("compatible slot is held"),
            Self::Full => formatter.write_str("pool cap cannot admit a new slot"),
            Self::UnknownAccounting => {
                formatter.write_str("owned accounting is unknown or a foreign entry is present")
            }
            Self::UnexpectedEntry { place, entry } => {
                write!(
                    formatter,
                    "the {place} holds an unexpected entry \"{entry}\""
                )
            }
            Self::AccountingIncomplete(scan) => {
                write!(formatter, "owned accounting is incomplete: {scan}")
            }
            Self::HolderConflict => {
                formatter.write_str("holder is already bound to a different lease")
            }
            Self::NotFound => formatter.write_str("lease identity is not in this pool"),
            Self::BackingMismatch => {
                formatter.write_str("backing does not match the pinned pool descriptor")
            }
            Self::LedgerExhausted => {
                formatter.write_str("finite ledger or slot count is exhausted")
            }
            Self::Unsupported(message) => formatter.write_str(message),
            Self::Io(message) => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for LeaseError {}

/// Lease grant returned on stdout as versioned JSON.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseGrant {
    pub version: u32,
    pub lease_id: String,
    pub store_id: String,
    pub slot_id: String,
    pub incarnation: String,
    pub project: PathBuf,
    pub workspace: PathBuf,
    pub profile: Profile,
    pub holder: String,
    pub state: SlotOccupancy,
    pub reused_slot: bool,
    pub env: LeaseEnvironment,
}

/// Exact environment the caller should apply for this lease. Final target
/// output stays private to the workspace; intermediates live on the slot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseEnvironment {
    #[serde(rename = "CARGO_BUILD_BUILD_DIR")]
    pub cargo_build_build_dir: PathBuf,
    #[serde(rename = "CARGO_TARGET_DIR")]
    pub cargo_target_dir: PathBuf,
}

/// Idempotent release or recover receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseReceipt {
    pub version: u32,
    pub lease_id: String,
    pub state: SlotOccupancy,
    pub replay: bool,
}

/// Pool identity after init.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolInfo {
    pub version: u32,
    pub store_id: String,
    pub backend: String,
    pub cap_bytes: u64,
    pub verified_capacity_bytes: u64,
}

/// What [`managed::describe_volume`] read from an `apfs-volume-v1` pool's
/// config and ledger. The volume itself is probed by the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VolumeDescription {
    pub store_id: String,
    pub cap_bytes: u64,
    /// The volume the config records. The caller compares it with its own
    /// registration; nothing here re-verifies it.
    pub mount_point: PathBuf,
    pub volume_uuid: String,
    pub payload_dir: String,
    /// Recorded slot deletions an open would reconcile. Read, not run.
    pub evicting_slots: usize,
}

/// Status of one slot. No payload or secret content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotStatus {
    pub slot_id: String,
    pub incarnation: String,
    pub state: SlotOccupancy,
    pub project: PathBuf,
    pub workspace: PathBuf,
    pub profile: Profile,
    pub holder: Option<String>,
    pub lease_id: Option<String>,
    pub logical_bytes: Option<u64>,
    pub unique_allocated_bytes: Option<u64>,
    /// This slot's own walk, the one reuse needs complete.
    pub accounting: AccountingScan,
}

/// Finite pool status. Partial or unknown accounting is explicit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolStatus {
    pub version: u32,
    pub store_id: String,
    pub backend: String,
    pub cap_bytes: u64,
    pub payload_logical_bytes: Option<u64>,
    pub payload_unique_allocated_bytes: Option<u64>,
    pub coverage: AccountingCoverage,
    /// The whole-payload walk, the one new-slot admission needs complete.
    pub payload_accounting: AccountingScan,
    pub full: Option<bool>,
    pub slots: Vec<SlotStatus>,
}

/// Ledger membership of an opened pool, for callers that need no sizes.
/// It has no byte or coverage fields, so a walk it skipped cannot read as
/// zero or complete. Opening it is [`managed::status`]'s opening, including
/// recorded-deletion reconciliation, so it is not side-effect-free.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerView {
    pub store_id: String,
    pub backend: String,
    pub cap_bytes: u64,
    /// A recorded deletion could not complete at this open.
    pub reconciliation_blocked: bool,
    pub slots: Vec<LedgerSlot>,
}

/// One ledger record: [`SlotStatus`] without its accounting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerSlot {
    pub slot_id: String,
    pub incarnation: String,
    pub state: SlotOccupancy,
    pub project: PathBuf,
    pub workspace: PathBuf,
    pub profile: Profile,
    pub holder: Option<String>,
    pub lease_id: Option<String>,
}

impl From<&SlotStatus> for LedgerSlot {
    fn from(slot: &SlotStatus) -> Self {
        Self {
            slot_id: slot.slot_id.clone(),
            incarnation: slot.incarnation.clone(),
            state: slot.state,
            project: slot.project.clone(),
            workspace: slot.workspace.clone(),
            profile: slot.profile,
            holder: slot.holder.clone(),
            lease_id: slot.lease_id.clone(),
        }
    }
}

/// The complete walk reuse made of exactly this slot generation, under the
/// pool lock, before this lease took it. Nothing measures a new slot or a
/// held same-holder replay, and nothing remeasures after release.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReuseMeasurement {
    pub slot_id: String,
    pub incarnation: String,
    pub unique_allocated_bytes: u64,
}

/// A grant and, when reuse walked the slot, that measurement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Leased {
    pub grant: LeaseGrant,
    pub measured: Option<ReuseMeasurement>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingCoverage {
    Complete,
    Partial,
    Unknown,
}

/// Finder metadata a pool tolerates beside its own entries: exactly
/// `.DS_Store` (Finder's Desktop Services store; Apple's
/// `com.apple.desktopservices` `DSDontWriteNetworkStores` /
/// `DSDontWriteUSBStores` preferences govern where Finder writes it) and
/// AppleDouble sidecars `._<name>` with a nonempty `<name>` (dot_clean(1):
/// "Merge ._* files with corresponding native files"). The name alone
/// grants nothing: callers still require an owned regular file on the
/// pool's device, and its bytes stay in accounting.
fn is_finder_metadata(name: &std::ffi::OsStr) -> bool {
    let name = name.as_bytes();
    name == b".DS_Store" || (name.len() > 2 && name.starts_with(b"._"))
}

/// Whether a no-follow `metadata` is a Finder metadata file the pool may
/// hold: a regular caller-owned file on `device`.
fn is_tolerated_metadata_file(metadata: &fs::Metadata, device: u64) -> bool {
    metadata.file_type().is_file() && metadata.dev() == device && metadata.uid() == current_uid()
}

/// Longest entry name, in bytes, a diagnostic prints before eliding.
const MAX_ENTRY_LABEL_BYTES: usize = 64;

/// An entry name safe to print: printable ASCII except `"` and `\` as is,
/// every other byte (control, non-ASCII, non-UTF-8) as `\xNN`, at most
/// [`MAX_ENTRY_LABEL_BYTES`] input bytes followed by `...` when longer.
fn entry_label(name: &std::ffi::OsStr) -> String {
    use std::fmt::Write as _;
    let bytes = name.as_bytes();
    let mut label = String::new();
    for &byte in bytes.iter().take(MAX_ENTRY_LABEL_BYTES) {
        if byte == b' ' || (byte.is_ascii_graphic() && byte != b'"' && byte != b'\\') {
            label.push(char::from(byte));
        } else {
            let _ = write!(label, "\\x{byte:02x}");
        }
    }
    if bytes.len() > MAX_ENTRY_LABEL_BYTES {
        label.push_str("...");
    }
    label
}

fn unexpected_entry(place: &'static str, name: &std::ffi::OsStr) -> LeaseError {
    LeaseError::UnexpectedEntry {
        place,
        entry: entry_label(name),
    }
}

/// Why one accounting walk is not complete. Walk gaps come from the
/// inventory; root failures mean no walk ran.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingGapKind {
    EntryLimit,
    DepthLimit,
    ElapsedDeadline,
    MountBoundary,
    UnreadableEntry,
    MissingMetadata,
    UnsupportedMetadata,
    RootMissing,
    RootNotDirectory,
    RootUnreadable,
    RootSymlink,
}

impl AccountingGapKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EntryLimit => "entry_limit",
            Self::DepthLimit => "depth_limit",
            Self::ElapsedDeadline => "elapsed_deadline",
            Self::MountBoundary => "mount_boundary",
            Self::UnreadableEntry => "unreadable_entry",
            Self::MissingMetadata => "missing_metadata",
            Self::UnsupportedMetadata => "unsupported_metadata",
            Self::RootMissing => "root_missing",
            Self::RootNotDirectory => "root_not_directory",
            Self::RootUnreadable => "root_unreadable",
            Self::RootSymlink => "root_symlink",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountingGap {
    pub kind: AccountingGapKind,
    pub occurrences: u64,
}

/// What one bounded accounting walk covered: coverage, gap causes, entries
/// visited and elapsed time. No paths, names or contents.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountingScan {
    pub coverage: AccountingCoverage,
    pub gaps: Vec<AccountingGap>,
    pub entries_visited: u64,
    pub elapsed_ms: u64,
    /// The bounds this walk used.
    pub max_entries: u64,
    pub max_depth: usize,
    pub max_elapsed_ms: u64,
}

impl std::fmt::Display for AccountingScan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let coverage = match self.coverage {
            AccountingCoverage::Complete => "complete",
            AccountingCoverage::Partial => "partial",
            AccountingCoverage::Unknown => "unknown",
        };
        write!(formatter, "{coverage}")?;
        for (index, gap) in self.gaps.iter().enumerate() {
            let separator = if index == 0 { ", " } else { " + " };
            write!(
                formatter,
                "{separator}{} x{}",
                gap.kind.as_str(),
                gap.occurrences
            )?;
        }
        write!(
            formatter,
            ", {} entries in {} ms (bounds {} entries, depth {}, {} ms)",
            self.entries_visited,
            self.elapsed_ms,
            self.max_entries,
            self.max_depth,
            self.max_elapsed_ms
        )
    }
}

fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

fn io_error(error: std::io::Error) -> LeaseError {
    LeaseError::Io(error.to_string())
}

fn require_absolute(path: &Path, what: &'static str) -> Result<(), LeaseError> {
    if path.is_absolute() {
        Ok(())
    } else {
        let _ = what;
        Err(LeaseError::InvalidInput("paths must be absolute"))
    }
}

fn require_bounded_text(value: &str, max: usize, what: &'static str) -> Result<(), LeaseError> {
    if value.is_empty() || value.len() > max {
        let _ = what;
        return Err(LeaseError::InvalidInput(
            "identity text is empty or too long",
        ));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_graphic() && byte != b'/')
    {
        return Err(LeaseError::InvalidInput(
            "identity text must be printable ASCII without slash",
        ));
    }
    Ok(())
}

fn canonical_existing_dir(path: &Path, owner_only: bool) -> Result<PathBuf, LeaseError> {
    require_absolute(path, "path")?;
    if path.as_os_str().as_bytes().len() > MAX_PATH_BYTES {
        return Err(LeaseError::InvalidInput("path is too long"));
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|_| LeaseError::UnsafePath("path missing"))?;
    if metadata.file_type().is_symlink() {
        return Err(LeaseError::UnsafePath("symlinks are refused"));
    }
    if !metadata.file_type().is_dir() {
        return Err(LeaseError::InvalidInput("path must be a directory"));
    }
    if owner_only {
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(LeaseError::UnsafePath("directory must be owner-only"));
        }
        if metadata.uid() != current_uid() {
            return Err(LeaseError::UnsafePath(
                "directory must be owned by the caller",
            ));
        }
    }
    let canonical = fs::canonicalize(path).map_err(io_error)?;
    if canonical != path {
        return Err(LeaseError::UnsafePath(
            "path must be supplied in canonical form",
        ));
    }
    Ok(canonical)
}

fn ensure_private_dir(path: &Path) -> Result<(), LeaseError> {
    require_absolute(path, "directory")?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(LeaseError::UnsafePath("symlinks are refused"));
            }
            if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o077 != 0 {
                return Err(LeaseError::UnsafePath("directory must be owner-only"));
            }
            if metadata.uid() != current_uid() {
                return Err(LeaseError::UnsafePath(
                    "directory must be owned by the caller",
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(io_error)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
            if let Some(parent) = path.parent() {
                File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .map_err(io_error)?;
            }
        }
        Err(error) => return Err(io_error(error)),
    }
    let canonical = fs::canonicalize(path).map_err(io_error)?;
    if canonical != path {
        return Err(LeaseError::UnsafePath(
            "path must be supplied in canonical form",
        ));
    }
    Ok(())
}

fn persist_private_file(path: &Path, bytes: &[u8]) -> Result<(), LeaseError> {
    if bytes.len() as u64 > MAX_LEDGER_BYTES {
        return Err(LeaseError::LedgerExhausted);
    }
    let parent = path
        .parent()
        .ok_or(LeaseError::InvalidInput("file has no parent"))?;
    let mut temp_name = String::from("ledger.tmp-");
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

fn read_private_file(path: &Path) -> Result<Vec<u8>, LeaseError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != current_uid()
        || metadata.len() > MAX_LEDGER_BYTES
    {
        return Err(LeaseError::UnknownAccounting);
    }
    let file = File::open(path).map_err(io_error)?;
    let mut bytes = Vec::new();
    file.take(MAX_LEDGER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > MAX_LEDGER_BYTES {
        return Err(LeaseError::LedgerExhausted);
    }
    Ok(bytes)
}

fn random_hex(bytes: usize) -> Result<String, LeaseError> {
    let mut value = vec![0_u8; bytes];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut value))
        .map_err(io_error)?;
    let mut result = String::with_capacity(bytes * 2);
    for byte in value {
        use std::fmt::Write as _;
        write!(&mut result, "{byte:02x}")
            .map_err(|_| LeaseError::Io("token encoding failed".into()))?;
    }
    Ok(result)
}

fn remove_tree_owned(root: &Path, path: &Path) -> Result<(), LeaseError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() {
        if metadata.uid() != current_uid() {
            return Err(LeaseError::UnsafePath(
                "refusing to delete a foreign symlink",
            ));
        }
        let parent = path.parent().ok_or(LeaseError::UnknownAccounting)?;
        let parent_metadata = fs::symlink_metadata(parent).map_err(io_error)?;
        if parent_metadata.file_type().is_symlink()
            || !parent_metadata.file_type().is_dir()
            || parent_metadata.uid() != current_uid()
        {
            return Err(LeaseError::UnsafePath(
                "symlink parent must be an owned directory",
            ));
        }
        let canonical_root = fs::canonicalize(root).map_err(io_error)?;
        let canonical_parent = fs::canonicalize(parent).map_err(io_error)?;
        if canonical_parent == canonical_root || !canonical_parent.starts_with(&canonical_root) {
            return Err(LeaseError::UnsafePath(
                "deletion is limited to recorded owned slot paths",
            ));
        }
        return fs::remove_file(path).map_err(io_error);
    }
    if metadata.file_type().is_file() {
        if metadata.uid() != current_uid() {
            return Err(LeaseError::UnsafePath("refusing to delete a foreign file"));
        }
        return fs::remove_file(path).map_err(io_error);
    }
    if !metadata.file_type().is_dir() {
        return Err(LeaseError::UnknownAccounting);
    }
    let canonical = fs::canonicalize(path).map_err(io_error)?;
    let canonical_root = fs::canonicalize(root).map_err(io_error)?;
    if !canonical.starts_with(&canonical_root) || canonical == canonical_root {
        return Err(LeaseError::UnsafePath(
            "deletion is limited to recorded owned slot paths",
        ));
    }
    if metadata.uid() != current_uid() {
        return Err(LeaseError::UnsafePath(
            "refusing to delete a foreign directory",
        ));
    }
    for entry in fs::read_dir(path).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        remove_tree_owned(root, &entry.path())?;
    }
    fs::remove_dir(path).map_err(io_error)
}
