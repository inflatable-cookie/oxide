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

use crate::owned_delete::{
    capture_entry, capture_planned, descriptor_identity, names_of, observe_entry,
    open_directory_at, plan_root_capture, put_back, remove_captured_directory, unlink_captured,
    verify_captured, Capture, CaptureContext, RootCaptureJournal,
};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub use pool::{managed, LeaseRequest, SlotLayout};

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
/// Finite slot count. Exhaustion refuses a new slot. Slot headroom only:
/// it does not raise build concurrency or `max_active` (oxide#118).
pub const MAX_SLOTS: usize = 128;
/// Finite ledger file size including the atomic temp. The worst valid
/// 128-slot ledger with every bounded field at its ceiling, escaped by
/// compact JSON, fits this cap with no truncation (oxide#118).
pub const MAX_LEDGER_BYTES: u64 = 2 * 1024 * 1024;
/// Permanent capacity-policy fence in the library ledger (oxide#118). Its
/// presence tells an older binary that this ledger uses ranked idle
/// replacement; the fence is never removed. Version 2 (oxide#146) adds the
/// closed per-slot size record, so a version-2 ledger also refuses an older
/// binary's `deny_unknown_fields` parse.
pub const CAPACITY_POLICY_VERSION: u32 = 2;
/// The version-1 fence (oxide#118) stays readable and is upgraded by the next
/// revised write; a version-1 ledger never carries a record.
pub const LEGACY_CAPACITY_POLICY_VERSION: u32 = 1;
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
    /// Slot directory below `slots`, fixed at creation:
    /// `<project folder>/<profile>-<id>`. `None` is the legacy `<id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// The pool's idle rank (oxide#118): higher is more recently granted or
    /// released. Legacy records have rank 0.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rank: u64,
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
    /// The whole-payload audit walk: what `status`/`--measure` reports. It is
    /// **not** the admission figure, which is the recorded per-slot sum
    /// (`recorded_total()` in the library) and never reports a partial scan as
    /// a size.
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

/// A pool's recorded identity, ledger membership and recorded payload root,
/// read with no lock, no reconciliation and no write (oxide#128). Only a
/// diagnostic: it grants no admission, ownership or deletion authority and
/// never infers that a recorded deletion can finish.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PassiveSnapshot {
    pub store_id: String,
    pub backend: String,
    pub cap_bytes: u64,
    /// The payload root the backend records. For a removable pool this is
    /// the recorded mount point plus payload directory; nothing here proves
    /// the volume is mounted, writable or that the path resolves.
    pub payload: PathBuf,
    /// The recorded volume binding of an `apfs-volume-v1` pool.
    pub volume: Option<RecordedVolume>,
    /// Ledger membership in ledger order.
    pub slots: Vec<LedgerSlot>,
    /// Slots recorded `Evicting`, in ledger order: a deletion is pending
    /// reconciliation. They are neither idle nor held, and this snapshot
    /// never infers that their recorded deletion can finish.
    pub evicting_slot_ids: Vec<String>,
    /// The sum of the *current* recorded per-slot observations (oxide#146), or
    /// `None` when no idle slot carries a current one. This is the recorded
    /// admission basis, never free space; the oxide#128 sidecar facts are a
    /// separate diagnostic source and are never added to it.
    pub recorded_total: Option<u64>,
    /// Idle slots whose record is current.
    pub recorded_slots: usize,
    /// Idle slots with no current record: they make `recorded_total` a stated
    /// lower bound and are walked by the next admission that needs them.
    pub unrecorded_slots: usize,
}

/// The volume a removable pool's config records. Recorded facts only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedVolume {
    pub mount_point: PathBuf,
    pub volume_uuid: String,
    pub payload_dir: String,
}

/// A passive measurement of one pool and the recorded snapshot it read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeasuredPool {
    /// Recorded identity and membership, read before the walks.
    pub snapshot: PassiveSnapshot,
    /// The bounded walker result. Byte figures are the walker's; `coverage`
    /// is unknown while any slot is recorded `Evicting`, because this
    /// measurement reconciles nothing.
    pub status: PoolStatus,
}

/// A closed per-slot size observation, as recorded in the ledger. It is a
/// measurement taken at a closing transition, not a claim that the slot's
/// bytes are immutable, that its writer closed or that a volume is closed.
/// `current` says only whether it still matches its idle slot generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordedFact {
    pub unique_allocated_bytes: u64,
    pub logical_bytes: u64,
    pub entries: u64,
    pub at_ms: u64,
    /// This record still matches its idle slot generation.
    pub current: bool,
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
    /// As [`SlotStatus::dir`].
    pub dir: Option<String>,
    /// As [`SlotStatus::rank`].
    pub rank: u64,
    /// The closed size observation this record holds, if any.
    pub recorded: Option<RecordedFact>,
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
            dir: slot.dir.clone(),
            rank: slot.rank,
            // A walked status report shows its own figures; the recorded
            // observation is reported by the ledger view that default status
            // reads.
            recorded: None,
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
    /// The recorded slot directory the walk covered, so a recorded
    /// observation binds to exactly that directory (oxide#128).
    pub dir: Option<String>,
}

/// A grant and, when reuse walked the slot, that measurement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Leased {
    pub grant: LeaseGrant,
    pub measured: Option<ReuseMeasurement>,
    /// Set when a new slot on a removable volume was admitted with its pool
    /// size **unknown** because the pool's own walk stopped at its bounds.
    /// The note states what decided it: free space above the registered
    /// reserve, or — after every eligible idle whole slot was pruned and free
    /// space stayed at or above the 10 GiB hard floor — a warning that the
    /// prune target could not be recovered. It never reports the volume's
    /// used bytes as pool usage, and the pool cap stays unverified on this
    /// route. The caller prints it as a warning.
    pub admission_note: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingCoverage {
    Complete,
    Partial,
    Unknown,
}

/// The oxide#105 tolerated-metadata rule, shared with the deletion seam: the
/// name alone grants nothing, a caller still requires an owned regular file on
/// the pool's device, and its bytes stay in accounting.
pub(crate) use crate::owned_delete::{entry_label, is_finder_metadata, is_tolerated_metadata_file};

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
    /// Enumerated paths that disappeared before a deferred read. This does
    /// not establish that their inode allocation was freed or reclaimable.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub vanished: u64,
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
        if self.vanished > 0 {
            write!(
                formatter,
                ", vanished x{} (allocation reclamation not inferred)",
                self.vanished
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

fn is_zero(value: &u64) -> bool {
    *value == 0
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

/// Delete one verified owned entry and, for a directory, everything below it.
///
/// Every level is descriptor-relative: the entry is inspected with `fstatat`
/// against its held parent, opened with `O_NOFOLLOW` (a link is unlinked, never
/// followed) and removed through that parent's descriptor. The removal of a
/// directory is the shared seam's bounded race recovery (oxide#155): on
/// `ENOTEMPTY`, Finder metadata recreated after our own listing is verified and
/// unlinked, and only that, within a fixed finite bound. Anything else that
/// appeared stays a refusal, which the caller keeps as unfinished intent.
///
/// `journal` is the caller's durable record for the **root** of this tree. When
/// it is present the seam plans the private capture name, lets the caller make
/// the plan durable, performs the exclusive rename, and only then records the
/// captured phase — so a crash never leaves the root under a private name no
/// record names. Nested entries pass `None`: the root's record covers the whole
/// tree, and a leftover capture inside it is deleted with it.
fn remove_tree_owned(
    root: &Path,
    path: &Path,
    journal: Option<&mut dyn RootCaptureJournal>,
) -> Result<(), LeaseError> {
    let name = path.file_name().ok_or(LeaseError::UnknownAccounting)?;
    let parent_path = path.parent().ok_or(LeaseError::UnknownAccounting)?;
    // Containment is decided once, from the real path, while the tree is still
    // at its recorded names: after a capture every level below is addressed by
    // a private name, and descriptor identity plus the capture verification is
    // what authorizes those levels.
    let canonical = fs::canonicalize(path).map_err(io_error)?;
    let canonical_root = fs::canonicalize(root).map_err(io_error)?;
    if !canonical.starts_with(&canonical_root) || canonical == canonical_root {
        return Err(LeaseError::UnsafePath(
            "deletion is limited to recorded owned slot paths",
        ));
    }
    let parent = open_verified_directory(parent_path)?;
    let name =
        std::ffi::CString::new(name.as_bytes()).map_err(|_| LeaseError::UnknownAccounting)?;
    let mut context = CaptureContext {
        chain: Vec::new(),
        journal,
    };
    remove_tree_owned_at(root, parent.as_fd(), &name, path, &mut context, true)
}

/// Put a captured entry back under its observed name and return the caller's
/// own cause. The journal record is deliberately kept: only a completed
/// deletion or a later replay clears it, so a crash or a failed put-back never
/// loses the only name that can find the object.
fn abort_capture(parent: BorrowedFd<'_>, capture: &Capture, cause: LeaseError) -> LeaseError {
    match put_back(parent, capture) {
        Ok(()) => cause,
        Err(error) => LeaseError::Io(format!(
            "{cause}; the captured entry {} could not be put back under its recorded name ({error}), so both names are kept",
            entry_label(std::ffi::OsStr::from_bytes(capture.private.as_bytes()))
        )),
    }
}

/// Put a captured entry back only while the private name still holds exactly
/// the object this process captured and verified. A name that now holds
/// anything else (a replacement, a link, or something unreadable) is left
/// exactly where it is: an unverified occupant is never renamed into the
/// observed name, and the caller's journal record stays outstanding.
fn put_back_verified(
    parent: BorrowedFd<'_>,
    capture: &Capture,
    observed: &crate::owned_delete::EntryFacts,
    cause: LeaseError,
) -> LeaseError {
    let still_captured = crate::owned_delete::observe_entry(parent, &capture.private)
        .map(|facts| {
            facts.device == observed.device
                && facts.inode == observed.inode
                && facts.file_type() == observed.file_type()
        })
        .unwrap_or(false);
    if still_captured {
        abort_capture(parent, capture, cause)
    } else {
        keep_both_names(capture, cause)
    }
}

/// Refuse without moving anything. The recorded private name no longer holds
/// the object that capture verified, so it is never renamed into the observed
/// name: both names are kept, the unverified object is left exactly where the
/// racer put it, and the caller's journal record stays outstanding (pending).
fn keep_both_names(capture: &Capture, cause: LeaseError) -> LeaseError {
    LeaseError::Io(format!(
        "{cause}; the captured private name {} and its observed name are both kept and nothing was moved",
        entry_label(std::ffi::OsStr::from_bytes(capture.private.as_bytes()))
    ))
}

/// Restores one captured directory's exact mode on **every** exit path —
/// explicit refusals, `?` returns and panics — so a widened mode never outlives
/// the step that widened it. The same mode is in the durable record, which is
/// what a crash restores it from.
struct WidenedMode<'a> {
    dir: BorrowedFd<'a>,
    original: libc::mode_t,
    widened: bool,
}

impl Drop for WidenedMode<'_> {
    fn drop(&mut self) {
        if self.widened {
            // SAFETY: the descriptor is live for the guard's whole scope.
            unsafe { libc::fchmod(self.dir.as_raw_fd(), self.original) };
        }
    }
}

/// One captured entry's private name, as the journal records it. The name is
/// generated ASCII, so this is the exact name and never a lossy conversion.
fn capture_private(capture: &Capture) -> Result<String, LeaseError> {
    crate::owned_delete::journal_name(&capture.private).map_err(LeaseError::Io)
}

/// The durable record of one capture inside the tree, built from the verified
/// observation and the captured private name. Both names are required to
/// round-trip exactly; the walk refuses a name that cannot before it captures.
fn capture_record(
    context: &CaptureContext<'_>,
    capture: &Capture,
    observed: &crate::owned_delete::EntryFacts,
) -> Result<crate::owned_delete::CaptureRecord, LeaseError> {
    Ok(crate::owned_delete::CaptureRecord {
        path: context.chain.clone(),
        private: capture_private(capture)?,
        original: crate::owned_delete::journal_name(&capture.original).map_err(LeaseError::Io)?,
        device: observed.device,
        inode: observed.inode,
        mode: u32::try_from(observed.mode).unwrap_or(u32::MAX),
    })
}

/// One entry, named in the held `parent`, of the tree rooted at `root`.
fn remove_tree_owned_at(
    root: &Path,
    parent: BorrowedFd<'_>,
    name: &std::ffi::CString,
    path: &Path,
    context: &mut CaptureContext<'_>,
    root_level: bool,
) -> Result<(), LeaseError> {
    // A private capture name is never ordinary content: it is either a recorded
    // capture (replay reconciles it) or an unrecorded one, which is kept.
    if !root_level
        && crate::owned_delete::is_capture_name(std::ffi::OsStr::from_bytes(name.as_bytes()))
    {
        return Err(LeaseError::UnsafePath(
            "a private capture name that no record names is kept",
        ));
    }
    // Observe the entry first, then *capture* it under a private name this
    // process just created. Every later check and the removal itself act on
    // that private name, so nothing a racer does to the original name can
    // redirect a delete.
    let observed = observe_entry(parent, name).map_err(io_error)?;
    if observed.file_type() == libc::S_IFLNK {
        if observed.uid != current_uid() {
            return Err(LeaseError::UnsafePath(
                "refusing to delete a foreign symlink",
            ));
        }
    } else if observed.file_type() == libc::S_IFREG {
        if observed.uid != current_uid() {
            return Err(LeaseError::UnsafePath("refusing to delete a foreign file"));
        }
    } else if observed.file_type() == libc::S_IFDIR {
        // Containment was proved once for the whole tree from its recorded
        // path; this level is authorized by the held parent plus the captured
        // identity check below.
        let _ = root;
        if observed.uid != current_uid() {
            return Err(LeaseError::UnsafePath(
                "refusing to delete a foreign directory",
            ));
        }
    } else {
        return Err(LeaseError::UnknownAccounting);
    }
    // A journal records an entry's exact name, so a name that is not UTF-8 is
    // refused **before** it is captured: nothing is renamed, nothing is
    // deleted, the escaped bytes are reported and the deletion stays pending.
    #[cfg(test)]
    let name_cannot_round_trip =
        std::str::from_utf8(name.as_bytes()).is_err() || crate::test_fs::forced_invalid_name();
    #[cfg(not(test))]
    let name_cannot_round_trip = std::str::from_utf8(name.as_bytes()).is_err();
    if name_cannot_round_trip {
        return Err(LeaseError::UnexpectedEntry {
            place: "slot tree entry",
            entry: entry_label(std::ffi::OsStr::from_bytes(name.as_bytes())),
        });
    }
    // The tree root is journaled intent-first: the planned private name, its
    // own identity, the verified parent's identity and its full mode are
    // durable before the rename and before any permission change. A nested
    // entry is recorded durably as `captured` right after its rename, which is
    // the narrow residual: a crash between those two leaves an unrecorded
    // private name that replay keeps and reports, never removes.
    let mut plan = None;
    if root_level && context.has_journal() {
        let planned = plan_root_capture(parent, &observed).map_err(io_error)?;
        if let Err(reason) = context.planned(&planned) {
            return Err(LeaseError::Io(format!(
                "the root capture could not be recorded durably ({reason})"
            )));
        }
        plan = Some(planned);
    }
    #[cfg(test)]
    if root_level {
        crate::test_fs::run_root_capture(crate::test_fs::RootCaptureCrash::Planned);
    }
    #[cfg(test)]
    crate::test_fs::run_before_walk_capture();
    let capture = match plan.as_ref() {
        Some(plan) => capture_planned(parent, name, plan).map_err(io_error)?,
        None => capture_entry(parent, name).map_err(io_error)?,
    };
    #[cfg(test)]
    if root_level {
        crate::test_fs::run_root_capture(crate::test_fs::RootCaptureCrash::Renamed);
    }
    if let Some(plan) = plan.as_ref() {
        if let Err(reason) = context.captured(plan) {
            return Err(abort_capture(
                parent,
                &capture,
                LeaseError::Io(format!(
                    "the root capture could not be recorded durably ({reason})"
                )),
            ));
        }
    }
    if verify_captured(parent, &capture, &observed).is_err() {
        // The capture holds an object this process never verified: a racer took
        // the observed name before the exclusive rename. Record it durably as a
        // foreign capture **before** anything is renamed, keep both names in
        // place, and stay pending — replay never retries past a foreign record
        // and never clears it.
        let found = observe_entry(parent, &capture.private)
            .map(|facts| crate::owned_delete::describe_entry(&facts))
            .unwrap_or_else(|error| format!("unreadable capture ({error})"));
        let foreign = crate::owned_delete::ForeignCapture {
            path: context.chain.clone(),
            private: capture_private(&capture)?,
            original: crate::owned_delete::journal_name(&capture.original)
                .map_err(LeaseError::Io)?,
            expected_device: observed.device,
            expected_inode: observed.inode,
            expected_mode: u32::from(observed.mode),
            found,
            restored: false,
        };
        if let Err(reason) = context.record_foreign(&foreign) {
            return Err(keep_both_names(
                &capture,
                LeaseError::Io(format!(
                    "an unexpected captured object could not be recorded ({reason})"
                )),
            ));
        }
        return Err(keep_both_names(
            &capture,
            LeaseError::UnsafePath("the captured entry is not the observed object"),
        ));
    }
    if observed.file_type() != libc::S_IFDIR {
        // A captured file or link is recorded before it is removed and its
        // removal is recorded before it counts as work done, so a crash is
        // always resolved by replay rather than assumed.
        let record = capture_record(context, &capture, &observed)?;
        if let Err(reason) = context.record_captured(&record) {
            return Err(abort_capture(
                parent,
                &capture,
                LeaseError::Io(format!("a captured entry could not be recorded ({reason})")),
            ));
        }
        // A captured file or link is removed by its private name only; a link's
        // referent is never touched.
        match unlink_captured(parent, &capture) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(LeaseError::Io(
                    "a recorded capture vanished before its removal".to_owned(),
                ))
            }
            Err(error) => return Err(io_error(error)),
        }
        if let Err(reason) = context.record_removed(&record) {
            return Err(LeaseError::Io(format!(
                "a captured entry's removal could not be recorded ({reason})"
            )));
        }
        return Ok(());
    }
    // The record keeps the owner's original mode and is durable **before any
    // permission change** — including the owner-access repair below — so a
    // failed write, a crash or a later refusal never leaves a widened directory
    // without a durable original mode. The **root** is recorded by its plan, not
    // as a nested capture: a nested record is located by the chain of captured
    // private names *below* the root.
    let record = if root_level {
        None
    } else {
        Some(capture_record(context, &capture, &observed)?)
    };
    let capture_name = capture_private(&capture)?;
    if let Some(record) = &record {
        if let Err(reason) = context.record_captured(record) {
            return Err(abort_capture(
                parent,
                &capture,
                LeaseError::Io(format!(
                    "a captured directory could not be recorded ({reason})"
                )),
            ));
        }
    }
    // The owner's exact mode is restored on every refusal after a repair, so a
    // kept directory never stays widened just because a later step refused.
    let mut repaired: Option<std::os::fd::OwnedFd> = None;
    let mut repair_guard: Option<WidenedMode<'_>> = None;
    let child = match open_directory_at(parent, &capture.private) {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            // The repair boundary: a replacement here must be refused, never
            // repaired through its name.
            #[cfg(test)]
            crate::test_fs::run_before_repair();
            let repair = match crate::owned_delete::open_directory_for_repair(
                parent,
                &capture.private,
            ) {
                Ok(repair) => repair,
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    return Err(put_back_verified(
                        parent,
                        &capture,
                        &observed,
                        LeaseError::UnsafePath(
                            "an owned directory that cannot be opened is kept, never chmodded through its name",
                        ),
                    ));
                }
                Err(error) => {
                    return Err(put_back_verified(
                        parent,
                        &capture,
                        &observed,
                        io_error(error),
                    ))
                }
            };
            let identity = match descriptor_identity(repair.as_fd()) {
                Ok(identity) => identity,
                Err(error) => {
                    return Err(put_back_verified(
                        parent,
                        &capture,
                        &observed,
                        io_error(error),
                    ))
                }
            };
            if identity != (observed.device, observed.inode) {
                // The private name now holds a replacement: it is neither
                // chmodded nor renamed.
                return Err(keep_both_names(
                    &capture,
                    LeaseError::UnsafePath(
                        "the verified directory was replaced before its access was repaired",
                    ),
                ));
            }
            if let Err(error) = crate::owned_delete::restore_owner_access(repair.as_fd()) {
                return Err(put_back_verified(
                    parent,
                    &capture,
                    &observed,
                    io_error(error),
                ));
            }
            // The repair widened this entry's own mode. From here every exit
            // path — an error, a `?` or a panic — restores the owner's recorded
            // mode through the **verified repair descriptor**, until the
            // reopened child's identity is established below. Nothing after this
            // point chmods a name or descriptor that has not been verified.
            repaired = Some(repair);
            repair_guard = Some(WidenedMode {
                dir: repaired.as_ref().expect("repair descriptor").as_fd(),
                original: observed.mode_bits(),
                widened: true,
            });
            #[cfg(test)]
            crate::test_fs::run_before_child_reopen();
            match open_directory_at(parent, &capture.private) {
                Ok(child) => child,
                Err(error) => {
                    // The reopen sees a name that is no longer the verified
                    // directory. Only the verified repair descriptor restores
                    // the owner's mode; the object now at the private name is
                    // never renamed, so both names are kept.
                    drop(repair_guard.take());
                    return Err(keep_both_names(&capture, io_error(error)));
                }
            }
        }
        Err(error) => {
            return Err(put_back_verified(
                parent,
                &capture,
                &observed,
                io_error(error),
            ))
        }
    };
    let held = match descriptor_identity(child.as_fd()) {
        Ok(held) => held,
        Err(error) => {
            // An unreadable identity is never restored through and never
            // renamed: the verified repair descriptor (if any) restores the
            // owner's mode and both names are kept.
            drop(repair_guard.take());
            return Err(keep_both_names(&capture, io_error(error)));
        }
    };
    if held != (observed.device, observed.inode) {
        // The reopened name is a replacement: it is never chmodded and never
        // renamed. The verified repair descriptor restores the original's mode.
        drop(repair_guard.take());
        return Err(keep_both_names(
            &capture,
            LeaseError::UnsafePath("the verified directory was replaced before it was opened"),
        ));
    }
    // Identity established: the child guard below restores the same recorded
    // mode on every later exit path, so the repair guard is no longer needed.
    if let Some(guard) = repair_guard.as_mut() {
        guard.widened = false;
    }
    drop(repair_guard);
    drop(repaired);
    let original_mode = observed.mode_bits();
    let widened = original_mode & 0o700 != 0o700;
    if widened {
        // SAFETY: `child` is a live descriptor for the verified directory.
        if unsafe { libc::fchmod(child.as_raw_fd(), original_mode | 0o700) } != 0 {
            return Err(abort_capture(
                parent,
                &capture,
                io_error(std::io::Error::last_os_error()),
            ));
        }
    }
    // From here the widened mode is restored on every exit path, including a
    // `?` return or a panic.
    let _mode = WidenedMode {
        dir: child.as_fd(),
        original: original_mode,
        widened,
    };
    #[cfg(test)]
    if root_level {
        crate::test_fs::run_root_capture(crate::test_fs::RootCaptureCrash::ModeWidened);
    }
    let restore = |child: BorrowedFd<'_>| {
        if widened {
            // SAFETY: the descriptor is live for the call.
            unsafe { libc::fchmod(child.as_raw_fd(), original_mode) };
        }
    };
    // The chain names captured parents *below* the root: the root is the
    // base the journal's replay opens, so its own private name is not in it.
    if !root_level {
        context.chain.push(capture_name);
    }
    let names = match names_of(child.as_fd()) {
        Ok(names) => names,
        Err(error) => {
            if !root_level {
                context.chain.pop();
            }
            restore(child.as_fd());
            return Err(abort_capture(parent, &capture, io_error(error)));
        }
    };
    for entry in names {
        let entry =
            std::ffi::CString::new(entry.as_bytes()).map_err(|_| LeaseError::UnknownAccounting)?;
        let entry_path = path.join(std::ffi::OsStr::from_bytes(entry.as_bytes()));
        if let Err(error) =
            remove_tree_owned_at(root, child.as_fd(), &entry, &entry_path, context, false)
        {
            // Restore the owner's own mode, then leave the captured directory
            // under its original name where the racer has not taken it. The
            // nested records stay: an aborted root keeps their uncertainty.
            context.chain.pop();
            restore(child.as_fd());
            return Err(abort_capture(parent, &capture, error));
        }
        #[cfg(test)]
        if root_level {
            crate::test_fs::run_root_capture(crate::test_fs::RootCaptureCrash::MidDelete);
        }
    }
    // The seam's label is the entry's *observed* name: it is diagnostic and
    // drives the debug-only proof injection, and a private capture name would
    // hide both from the proof. The removal itself uses the private name.
    let label = std::ffi::OsStr::from_bytes(name.to_bytes());
    let outcome = crate::owned_delete::rmdir_retrying_finder_metadata(
        parent,
        &capture.private,
        child.as_fd(),
        label,
        context,
        &mut || Ok::<(), crate::owned_delete::DrainStop>(()),
    );
    if !root_level {
        context.chain.pop();
    }
    // Anything but a positive removal restores the recorded name, so a refused
    // or pending slot is never left under a capture name the ledger cannot
    // name.
    if !matches!(
        outcome,
        crate::owned_delete::Rmdir::Removed | crate::owned_delete::Rmdir::Vanished
    ) {
        restore(child.as_fd());
        let restored = put_back(parent, &capture).is_ok();
        if !restored {
            return Err(LeaseError::Io(
                "a refused slot directory was left under its capture name".to_owned(),
            ));
        }
    }
    match outcome {
        crate::owned_delete::Rmdir::Removed => {
            // The directory's own removal is durable before it counts, and the
            // root's `removed` phase is what lets a replay conclude instead of
            // guessing from two absent names.
            let released = match &record {
                Some(record) => context.record_removed(record),
                None => Ok(()),
            };
            if let Err(reason) = released {
                return Err(LeaseError::Io(format!(
                    "a captured directory's removal could not be recorded ({reason})"
                )));
            }
            if let Some(plan) = plan.as_ref() {
                if let Err(reason) = context.record_root_removed(plan) {
                    return Err(LeaseError::Io(format!(
                        "the captured root's removal could not be recorded ({reason})"
                    )));
                }
            }
            Ok(())
        }
        // A vanished name is never this caller's success: the ledger keeps its
        // pending deletion and a replay decides conservatively.
        crate::owned_delete::Rmdir::Vanished => Err(LeaseError::Io(
            "a recorded capture vanished before its removal could be proved".to_owned(),
        )),
        crate::owned_delete::Rmdir::Failed { error } => Err(io_error(error)),
        crate::owned_delete::Rmdir::Replaced { entry }
        | crate::owned_delete::Rmdir::Refused { entry } => Err(LeaseError::UnexpectedEntry {
            place: "slot directory",
            entry,
        }),
        crate::owned_delete::Rmdir::Unproven { reason } => Err(LeaseError::Io(reason)),
        crate::owned_delete::Rmdir::Mismatch {
            original,
            private,
            restored,
            ..
        } => Err(LeaseError::Io(format!(
            "a captured slot entry ({private}) was not the observed {original}; {}",
            if restored {
                "it was put back and nothing was deleted"
            } else {
                "the original name was taken, so both names are kept"
            }
        ))),
        crate::owned_delete::Rmdir::CaptureFailed { error } => Err(io_error(error)),
        crate::owned_delete::Rmdir::RefusedMany { .. }
        | crate::owned_delete::Rmdir::Exhausted { .. }
        | crate::owned_delete::Rmdir::Stopped { .. }
        | crate::owned_delete::Rmdir::Drained { .. } => Err(LeaseError::UnknownAccounting),
    }
}

/// A real, owned parent directory held by descriptor, with the path's own
/// identity revalidated against the descriptor.
fn open_verified_directory(path: &Path) -> Result<File, LeaseError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.uid() != current_uid()
    {
        return Err(LeaseError::UnsafePath(
            "symlink parent must be an owned directory",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(io_error)?;
    let held = file.metadata().map_err(io_error)?;
    if held.dev() != metadata.dev() || held.ino() != metadata.ino() {
        return Err(LeaseError::UnsafePath(
            "the deletion parent was replaced before it was opened",
        ));
    }
    Ok(file)
}

fn stat_at(parent: BorrowedFd<'_>, name: &std::ffi::CStr) -> Result<libc::stat, LeaseError> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: the descriptor and C string are live for the call; `NOFOLLOW`
    // never resolves a link at the last component.
    let status = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            &mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status != 0 {
        return Err(io_error(std::io::Error::last_os_error()));
    }
    Ok(stat)
}

fn unlink_at(
    parent: BorrowedFd<'_>,
    name: &std::ffi::CStr,
    flags: libc::c_int,
) -> std::io::Result<()> {
    // SAFETY: the descriptor and C string are live for the call.
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), flags) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn stat_device(stat: &libc::stat) -> u64 {
    u64::try_from(stat.st_dev).unwrap_or(u64::MAX)
}

fn stat_ino(stat: &libc::stat) -> u64 {
    u64::try_from(stat.st_ino).unwrap_or(u64::MAX)
}

fn stat_uid(stat: &libc::stat) -> u32 {
    stat.st_uid
}
