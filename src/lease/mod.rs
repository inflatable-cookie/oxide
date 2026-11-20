//! Same-user leases over durable exclusive slots in a budget-only pool, used
//! by the `oxide` front door.
//!
//! A short-lived OS lock plus an atomic local ledger, no resident daemon,
//! HMAC, issuer, callback service or Cargo invocation. Same-UID callers are
//! trusted cooperators; private paths and safety checks still refuse foreign
//! paths and symlinked roots. Barge owns every deletion: an idle slot's root is
//! evicted by Barge's own marker, hold and pin checks, and the ledger only
//! records the decision. The standalone lease CLI and the APFS sparse-image
//! backend are retired (oxide#102); a pool recording that backend refuses to
//! open. Owning contract: `docs/knowledge/contracts/workflow.md`.

pub mod admission_cost;
mod backing;
#[cfg(test)]
mod tests;

pub mod control;
mod pool;
pub mod seed;

use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub use pool::{managed, LeaseRequest, SlotLayout};

/// Version of the lease JSON, lease receipts and pool config.
pub const LEASE_VERSION: u32 = 1;
/// Version 2 identifies the Barge slot fields previously written as version 1.
pub const LEDGER_VERSION: u32 = 2;
/// Budget-only directory backend for the `oxide` front door: admission and
/// whole-idle-slot eviction against a soft budget, with no OS ceiling.
pub const BUDGET_BACKEND_DIRECTORY_V1: &str = "directory-budget-v1";
/// Removable-volume backend for the `oxide` front door: config, ledger and
/// lock internal, payload in a directory on a registered external APFS
/// volume. Budget-only like the directory backend.
pub const VOLUME_BACKEND_APFS_V1: &str = "apfs-volume-v1";
/// Finite slot count. Exhaustion refuses a new slot. Slot headroom only:
/// it does not raise build concurrency or `max_active` (oxide#118, oxide#177).
pub const MAX_SLOTS: usize = 256;
/// The slot ceiling an older binary understands (oxide#118's 128 slots).
/// A ledger above it must carry [`RAISED_CAPACITY_POLICY_VERSION`] so an
/// older binary refuses it at ledger read instead of reading a slot table it
/// cannot represent (oxide#177).
pub const LEGACY_MAX_SLOTS: usize = 128;
/// Finite ledger file size including the atomic temp. Legacy ledger reads
/// retain bounded inert journal fields; new storage deletion and trash belong
/// exclusively to Barge. No Oxide deletion journal is replayed.
pub const MAX_LEDGER_BYTES: u64 = 8 * 1024 * 1024;
/// Permanent capacity-policy fence in the library ledger (oxide#118). Its
/// presence tells an older binary that this ledger uses ranked idle
/// replacement; the fence is never removed. Version 2 (oxide#146) adds the
/// closed per-slot size record, so a version-2 ledger also refuses an older
/// binary's `deny_unknown_fields` parse. Version 3 retains that closed size
/// across an eviction so older readers refuse the changed state semantics.
/// Version 4
/// (oxide#177) is the raised-ceiling fence: the ledger holds more than
/// [`LEGACY_MAX_SLOTS`] slots, which an older binary cannot represent. It is
/// written forward-only, in the same atomic replacement that first takes the
/// slot count above 128, and an older binary's policy check refuses it before
/// any mutation. A pool that stays at or below 128 slots never carries it, so
/// its ledger stays readable by an older binary. The front door's version-8
/// `oxide.slots/256` control fence is durable before that first policy-4
/// write, so an older installed binary hands off before it opens such a
/// ledger instead of meeting the refusal.
pub const CAPACITY_POLICY_VERSION: u32 = 3;
/// Version 2 recorded only idle sizes and remains readable.
pub const PREVIOUS_CAPACITY_POLICY_VERSION: u32 = 2;
/// The raised-ceiling fence value (oxide#177): more than [`LEGACY_MAX_SLOTS`]
/// slots, written forward-only in the replacement that first records the
/// 129th slot.
pub const RAISED_CAPACITY_POLICY_VERSION: u32 = 4;
/// The version-1 fence (oxide#118) stays readable and is upgraded by the next
/// revised write; a version-1 ledger never carries a record.
pub const LEGACY_CAPACITY_POLICY_VERSION: u32 = 1;
/// Finite holder identity.
pub const MAX_HOLDER_BYTES: usize = 128;
/// Finite canonical path.
pub const MAX_PATH_BYTES: usize = 1024;
/// Finite lease-id text.
pub const MAX_LEASE_ID_BYTES: usize = 160;
/// The most slot IDs a bounded capacity refusal names before it counts the
/// rest (oxide#186). Keeps one refusal line bounded at the 256-slot ceiling.
pub const MAX_REFUSAL_SLOTS: usize = 8;
/// Cooperative inventory bounds for pool accounting (oxide#108), separate
/// from the inventory tool's defaults. Sized for real build caches: the
/// entry bound still admits one 60k-entry Cargo build directory (Farmyard's
/// observed 60,657) and averages about 7,800 entries across a full 256-slot
/// pool; depth covers build-script `out` trees; the deadline allows a cold
/// walk of a USB pool. A walk that hits any bound is incomplete and refuses
/// where complete accounting is required.
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

thread_local! {
    /// The end of this command's admission window, set by the front door on the
    /// thread that runs the command. Waits on the pool lock stop no later.
    static ADMISSION_DEADLINE: std::cell::Cell<Option<std::time::Instant>> =
        const { std::cell::Cell::new(None) };
}

/// Record (or clear) the admission window that bounds this thread's waits on
/// the pool lock (oxide#325). The front door calls this once per command.
pub fn set_admission_deadline(deadline: Option<std::time::Instant>) {
    ADMISSION_DEADLINE.with(|cell| cell.set(deadline));
}

/// The admission window's end on this thread, when the command has one.
pub fn admission_deadline() -> Option<std::time::Instant> {
    ADMISSION_DEADLINE.with(std::cell::Cell::get)
}

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

/// One slot an admission walked but could not measure completely, bound to the
/// exact generation that walk covered (oxide#186).
///
/// It carries no size: the conservative charge stays in the admission's memory
/// and is never persisted. The front door records only the incomplete outcome,
/// as the oxide#128 diagnostic fact that makes default status say
/// `not measured (walk incomplete)` until a later complete walk records the
/// slot normally. Nothing derives a record, an eviction or any authority from
/// it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnmeasuredSlot {
    pub slot_id: String,
    pub incarnation: String,
    /// The ledger's recorded slot directory; `None` is the legacy `<id>`.
    pub dir: Option<String>,
}

/// Static origin of a required accounting walk; never inferred from a scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountingWalk {
    MatchingReuse,
    PoolScope,
    NonTargetSlotRoot,
}

/// Already-held refusal facts. This diagnostic has no serialized form.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountingContext {
    pub walk: AccountingWalk,
    pub removable: bool,
    pub slot: Option<(u64, u64)>,
}

impl std::fmt::Display for AccountingContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let phase = match self.walk {
            AccountingWalk::MatchingReuse => "matching_reuse",
            AccountingWalk::PoolScope => "pool_scope",
            AccountingWalk::NonTargetSlotRoot => "non_target_slot_root",
        };
        let backing = if self.removable {
            "removable"
        } else {
            "directory"
        };
        write!(formatter, " [phase={phase} backing={backing}")?;
        if let Some((id, incarnation)) = self.slot {
            write!(formatter, " slot={id} incarnation={incarnation}")?;
        }
        write!(formatter, "]")
    }
}

/// Why a command refused. Mapped to a small exit set by [`LeaseError::exit_code`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LeaseError {
    InvalidInput(&'static str),
    Busy,
    WriterUpdated,
    Full,
    UnknownAccounting,
    /// A writer cannot read or publish the pool's control state.
    ControlState(String),
    /// A pool-first caller cannot wait on admission without reversing lock order.
    WriterFenceBusy,
    /// An entry the pool's shape does not allow, named by its place and an
    /// escaped name. Same `unknown_accounting` category and exit code.
    UnexpectedEntry {
        place: &'static str,
        entry: String,
    },
    /// A bounded accounting walk that admission, reuse or eviction needed
    /// did not complete. Same reason and exit code as `UnknownAccounting`.
    AccountingIncomplete(Box<AccountingScan>, Option<AccountingContext>),
    /// Capacity could not admit this admission because slots without a current
    /// observation are charged the largest current observation instead of being
    /// walked, and are never evicted on that charge (oxide#186). A bounded
    /// capacity refusal: exit code 3, the same capacity class as
    /// [`LeaseError::Full`], and never `unknown_accounting`, because no shared
    /// authority is unknown. It names at most [`MAX_REFUSAL_SLOTS`] slots and
    /// counts the rest, so one refusal line stays bounded at the slot ceiling.
    CapacityCharged {
        /// The conservative pool figure and its budget for a byte-pressure
        /// refusal, or `None` when the refusal is that a requested idle-slot
        /// count replacement could not be honoured — a count decision, never a
        /// byte one, which has no honest byte figure to print.
        measured: Option<u64>,
        limit: Option<u64>,
        /// Every slot this admission charged conservatively, in slot order.
        unmeasured: Vec<UnmeasuredSlot>,
    },
    UnsafePath(&'static str),
    HolderConflict,
    NotFound,
    BackingMismatch,
    /// A Barge custody operation failed, carrying Barge's own message so the
    /// refusal names the operation and root instead of a bare class. Same
    /// reason and exit code as [`LeaseError::UnknownAccounting`]: this pool's
    /// custody of its roots could not be read or changed.
    Barge(String),
    LedgerExhausted,
    SlotCeiling(String),
    LegacyAdoptionPending(u64),
    LegacyWriterUnverified(u64),
    EvictionIncomplete(String),
    Unsupported(&'static str),
    Io(String),
}

/// A held eviction's failure class. Before Barge commits the root rename,
/// Oxide leaves the control and ledger unchanged and runs no reconciliation.
/// A failure after the rename or an uncertain intent cleanup requires
/// recovery and is reported as post-commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeldEvictionFailure {
    PreCommit(LeaseError),
    PostCommit(String),
}

impl std::fmt::Display for HeldEvictionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PreCommit(error) => error.fmt(formatter),
            Self::PostCommit(reason) => formatter.write_str(reason),
        }
    }
}

impl LeaseError {
    /// 0 is success. 1 usage/invalid/io. 2 busy or holder conflict. 3 full,
    /// unknown accounting or ledger exhaustion. 4 unsafe path or mismatched
    /// backing. 5 unknown lease.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::InvalidInput(_) | Self::Io(_) | Self::Unsupported(_) => 1,
            Self::Busy | Self::HolderConflict => 2,
            Self::WriterUpdated => 76,
            Self::Full
            | Self::UnknownAccounting
            | Self::Barge(_)
            | Self::ControlState(_)
            | Self::WriterFenceBusy
            | Self::UnexpectedEntry { .. }
            | Self::AccountingIncomplete(..)
            | Self::CapacityCharged { .. }
            | Self::LedgerExhausted
            | Self::SlotCeiling(_)
            | Self::LegacyAdoptionPending(_)
            | Self::LegacyWriterUnverified(_)
            | Self::EvictionIncomplete(_) => 3,
            Self::UnsafePath(_) | Self::BackingMismatch => 4,
            Self::NotFound => 5,
        }
    }

    pub fn reason(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid_input",
            Self::Busy => "busy",
            Self::WriterUpdated => "writer_updated",
            Self::Full => "full",
            Self::UnknownAccounting
            | Self::Barge(_)
            | Self::UnexpectedEntry { .. }
            | Self::AccountingIncomplete(..)
            | Self::EvictionIncomplete(_) => "unknown_accounting",
            Self::ControlState(_) => "control_state",
            Self::WriterFenceBusy => "writer_fence_busy",
            Self::CapacityCharged { .. } => "full",
            Self::UnsafePath(_) => "unsafe_path",
            Self::HolderConflict => "holder_conflict",
            Self::NotFound => "not_found",
            Self::BackingMismatch => "backing_mismatch",
            Self::LedgerExhausted | Self::SlotCeiling(_) => "ledger_exhausted",
            Self::LegacyAdoptionPending(_) => "legacy_adoption_pending",
            Self::LegacyWriterUnverified(_) => "legacy_writer_unverified",
            Self::Unsupported(_) => "unsupported",
            Self::Io(_) => "io",
        }
    }
}

impl std::fmt::Display for LeaseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(message) | Self::UnsafePath(message) => {
                formatter.write_str(message)
            }
            Self::Busy => formatter.write_str("compatible slot is held"),
            Self::WriterUpdated => formatter
                .write_str("oxide was updated while this command waited; rerun it"),
            Self::Full => formatter.write_str("pool cap cannot admit a new slot"),
            Self::UnknownAccounting => {
                formatter.write_str("owned accounting is unknown or a foreign entry is present")
            }
            Self::Barge(message) => formatter.write_str(message),
            Self::ControlState(message) => formatter.write_str(message),
            Self::WriterFenceBusy => {
                formatter.write_str("the admission lock is held during the writer generation upgrade")
            }
            Self::UnexpectedEntry { place, entry } => {
                write!(
                    formatter,
                    "the {place} holds an unexpected entry \"{entry}\""
                )
            }
            Self::AccountingIncomplete(scan, context) => {
                write!(formatter, "owned accounting is incomplete: {scan}")?;
                if let Some(context) = context {
                    write!(formatter, "{context}")?;
                }
                Ok(())
            }
            Self::CapacityCharged {
                measured,
                limit,
                unmeasured,
            } => {
                let named: Vec<&str> = unmeasured
                    .iter()
                    .take(MAX_REFUSAL_SLOTS)
                    .map(|slot| slot.slot_id.as_str())
                    .collect();
                match (measured, limit) {
                    (Some(measured), Some(limit)) => write!(
                        formatter,
                        "pool cap cannot admit a new slot: {measured} of {limit} bytes charged; unmeasured slot(s) {}",
                        named.join(", ")
                    )?,
                    _ => write!(
                        formatter,
                        "cannot admit a new slot: unmeasured slot(s) {}",
                        named.join(", ")
                    )?,
                }
                if let Some(more) = unmeasured.len().checked_sub(named.len()).filter(|n| *n > 0) {
                    write!(formatter, " and {more} more")?;
                }
                formatter.write_str(" are kept and never evicted on a partial walk")
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
            Self::LegacyAdoptionPending(slot) => write!(
                formatter,
                "compatible legacy slot {slot} is pending adoption; retry the Oxide command to advance the next batch"
            ),
            Self::LegacyWriterUnverified(slot) => write!(
                formatter,
                "compatible legacy slot {slot} needs its previous writer verified closed before Oxide can use it"
            ),
            Self::Unsupported(message) => formatter.write_str(message),
            Self::Io(message) | Self::SlotCeiling(message) | Self::EvictionIncomplete(message) => formatter.write_str(message),
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
    /// IDs of the pending deletions, in ledger order.
    pub evicting_slot_ids: Vec<String>,
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
/// zero or complete. Its locked open can reconcile recorded evictions and
/// abandoned holds.
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
    /// `None` when no slot carries a current one. This is the recorded
    /// admission basis, never free space; the oxide#128 sidecar facts are a
    /// separate diagnostic source and are never added to it.
    pub recorded_total: Option<u64>,
    /// Slots whose record is current.
    pub recorded_slots: usize,
    /// Slots with no current record: they make `recorded_total` a stated
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

/// A logical-byte observation of one slot generation, recorded by release
/// or admission. A matching generation does not make the observation live
/// or establish writer closure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordedFact {
    pub logical_bytes: u64,
    pub entries: u64,
    pub at_ms: u64,
    /// This record still matches its slot generation.
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
    /// The recorded logical-byte observation, if any.
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

/// The complete target walk a reuse made under the pool lock before taking
/// this generation. Normal warm reuse, new slots and held same-holder replay
/// have no acquisition measurement; release accounting is separate.
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
///
/// The cooperative hold on the slot's root travels with the grant: the caller
/// must keep it for as long as the command uses the root and drop it only
/// after the release is recorded. A child that keeps the inherited descriptor
/// keeps the root held after the launcher's own copy is gone.
#[derive(Debug)]
pub struct Leased {
    pub grant: LeaseGrant,
    /// The Barge hold on the root. `None` for a legacy Oxide slot directory,
    /// which Barge does not own yet.
    pub hold: Option<barge::Hold>,
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
    /// Slots this admission could not measure completely and charged
    /// conservatively instead (oxide#186). Empty when every walk completed.
    /// Their charge is not reported: only the incomplete outcome is, so a
    /// caller can record it as the oxide#128 diagnostic fact.
    pub unmeasured: Vec<UnmeasuredSlot>,
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
pub(crate) use crate::fs_util::{entry_label, is_finder_metadata, is_tolerated_metadata_file};

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

fn require_absolute(path: &Path) -> Result<(), LeaseError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(LeaseError::InvalidInput("paths must be absolute"))
    }
}

fn require_bounded_text(value: &str, max: usize) -> Result<(), LeaseError> {
    if value.is_empty() || value.len() > max {
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
    require_absolute(path)?;
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
    require_absolute(path)?;
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
            // The mode is set at creation: `create_dir` followed by
            // `set_permissions` leaves the umask-controlled mode in place for
            // the window between the two calls (oxide#337).
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(path).map_err(io_error)?;
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

/// Where [`persist_private_atomic`] can be made to fail, so a test can model
/// an interruption: before the rename the temp file stays behind, inert;
/// after it the new bytes are already at their real name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PersistStage {
    BeforeRename,
    AfterRename,
}

/// The fault hook for production writes, which never fails.
fn no_persist_fault(_stage: PersistStage) -> Option<LeaseError> {
    None
}

/// Atomic owner-only replacement of a private file: write a sibling temp
/// named `temp_prefix` plus random hex, fsync it, rename it over `path`, then
/// fsync the directory. A crash leaves at most that inert temp behind.
fn persist_private_atomic(
    path: &Path,
    bytes: &[u8],
    temp_prefix: &str,
    limit: u64,
    fault: fn(PersistStage) -> Option<LeaseError>,
) -> Result<(), LeaseError> {
    if bytes.len() as u64 > limit {
        return Err(LeaseError::LedgerExhausted);
    }
    let parent = path
        .parent()
        .ok_or(LeaseError::InvalidInput("file has no parent"))?;
    let temp_path = parent.join(format!("{temp_prefix}{}", random_hex(8)?));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(&temp_path).map_err(io_error)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temp_path);
        return Err(io_error(error));
    }
    drop(file);
    if let Some(error) = fault(PersistStage::BeforeRename) {
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(io_error(error));
    }
    if let Some(error) = fault(PersistStage::AfterRename) {
        return Err(error);
    }
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)
}

fn persist_private_file(path: &Path, bytes: &[u8]) -> Result<(), LeaseError> {
    persist_private_atomic(
        path,
        bytes,
        "ledger.tmp-",
        MAX_LEDGER_BYTES,
        no_persist_fault,
    )
}

fn read_private_file(path: &Path) -> Result<Vec<u8>, LeaseError> {
    read_private_file_bounded(path, MAX_LEDGER_BYTES)
}

fn read_private_file_bounded(path: &Path, max: u64) -> Result<Vec<u8>, LeaseError> {
    // The entry's own type is judged first, then the descriptor decides every
    // other fact: `O_NOFOLLOW` refuses a replacement that swapped a symlink in
    // between, so the shape checked and the bytes read name one file.
    let entry = fs::symlink_metadata(path).map_err(io_error)?;
    if !entry.file_type().is_file() {
        return Err(LeaseError::UnknownAccounting);
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != current_uid()
        || metadata.len() > max
    {
        return Err(LeaseError::UnknownAccounting);
    }
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > max {
        return Err(LeaseError::LedgerExhausted);
    }
    Ok(bytes)
}

/// The front door's raised-ceiling control fence (oxide#177, Oxide Supervisor
/// ruling 4c9984a1). The library writes only these fields and the version,
/// preserving every other field, so a locked [`pool::Pool::open`] can raise
/// the fence before it reconciles a recorded deletion. `src/bin/oxide/store.rs`
/// owns the same values; its constants alias these so the two writers cannot
/// drift. A version-6 document carries `CONTROL_ONE_USE_FENCE`, which later
/// documents may also carry; a document at [`CONTROL_FENCE_VERSION`] takes
/// every fence.
pub const CONTROL_FENCE_VERSION: u32 = 10;
pub const CONTROL_RECORDED_ACCOUNTING_FENCE: &str = "oxide.accounting/1";
pub const CONTROL_EVICTION_ACCOUNTING_FENCE: &str = "oxide.eviction-accounting/1";
pub const CONTROL_SLOT_CEILING_FENCE: &str = "oxide.slots/256";
pub const CONTROL_ONE_USE_FENCE: &str = "oxide.one-use/1";
/// The whole-tree holding fence (oxide#190), kept as a read-only value. The
/// holding area and its journal are retired: this binary tolerates a `holding`
/// row or a `hold_intent` only as an inert value an older writer left, never
/// acts on either, and deletes slot trees only through Barge's trash. The fence
/// still marks a control state a version-9 writer upgraded, so an older binary
/// meets version 9 first and hands off to a receipt-matching successor instead
/// of reading a state it cannot represent.
pub const CONTROL_HOLDING_FENCE: &str = "oxide.holding/1";
/// Raise a front-door control state to the current fence version before a
/// locked pool open reconciles a recorded deletion (oxide#177; fence first).
///
/// The caller already holds the pool lock. An absent control state means the
/// pool has never been used by the front door (`oxide-managed/state.json` is
/// its document), so a library fixture has nothing to fence. A state already
/// at the fence version with every fence is not rewritten. A newer version, an
/// unknown fence value, a present non-string fence, a version/fence
/// combination no writer produces (including version 0 and a version-6
/// document without its one-use fence)
/// or a non-numeric version refuses: this binary never downgrades or silently
/// repairs a document a successor wrote. Only a field genuinely absent from an
/// older document is inserted. The
/// admission lock is taken
/// non-blocking because the front door's order is admission then pool; if
/// another writer holds it, the open refuses instead of reconciling under an
/// unfenced control. Passive snapshots never call this.
pub(crate) fn fence_managed_control(pool: &Path) -> Result<(), LeaseError> {
    let control = pool.join(MANAGED_NAME);
    let Some(state) = read_writer_state(&control)? else {
        return Ok(());
    };
    if state.minimum_writer_generation == Some(control::WRITER_GENERATION) {
        return Ok(());
    }
    let _admission = open_control_admission(&control, true)?;
    // Re-read after taking both locks. Only the admission holder publishes
    // the minimum, and a newer writer may have published while we waited.
    fence_admission_writer(&control)
}

/// Read control as a prospective writer. The versioned generation header
/// stays recognizable even when a future schema cannot be deserialized.
pub fn read_writer_state(control: &Path) -> Result<Option<control::State>, LeaseError> {
    writer_state_bytes(control)?
        .map(|bytes| control::parse_state(&bytes).map_err(LeaseError::ControlState))
        .transpose()
}

/// Compare generations under a lock without interpreting the owner's other
/// control fields. The caller still validates those fields before writing.
pub fn check_control_writer(control: &Path) -> Result<(), LeaseError> {
    writer_state_bytes(control).map(|_| ())
}

fn writer_state_bytes(control: &Path) -> Result<Option<Vec<u8>>, LeaseError> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(control.join("state.json"))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() || metadata.uid() != current_uid() || metadata.mode() & 0o077 != 0 {
        return Err(LeaseError::UnsafePath(
            "state.json must be a private regular file owned by the current user",
        ));
    }
    let byte_limit = || LeaseError::ControlState("Oxide state exceeds its byte limit".to_owned());
    if metadata.len() > control::MAX_STATE_BYTES {
        return Err(byte_limit());
    }
    let mut bytes = Vec::new();
    file.take(control::MAX_STATE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    #[derive(Deserialize)]
    struct WriterHeader {
        version: u32,
        minimum_writer_generation: u32,
    }
    if bytes.len() as u64 > control::MAX_STATE_BYTES {
        return Err(byte_limit());
    }
    if serde_json::from_slice::<WriterHeader>(&bytes).is_ok_and(|header| {
        header.version >= control::WRITER_STATE_VERSION
            && header.minimum_writer_generation > control::WRITER_GENERATION
    }) {
        return Err(LeaseError::WriterUpdated);
    }
    Ok(Some(bytes))
}

/// With the admission lock held, publish this generation as the first write.
pub fn fence_admission_writer(control: &Path) -> Result<(), LeaseError> {
    let state = read_writer_state(control)?.unwrap_or_default();
    if state.minimum_writer_generation == Some(control::WRITER_GENERATION) {
        return Ok(());
    }
    let fenced = control::fenced_state(state, control::WRITER_GENERATION)
        .map_err(LeaseError::ControlState)?;
    let bytes = serde_json::to_vec(&fenced).map_err(|error| LeaseError::Io(error.to_string()))?;
    if bytes.len() as u64 > control::MAX_STATE_BYTES {
        return Err(LeaseError::ControlState(
            "Oxide state exceeds its byte limit after fencing".to_owned(),
        ));
    }
    // Prove the bytes the front door will read back are accepted by the same
    // parser before replacing the durable document.
    control::parse_state(&bytes).map_err(LeaseError::ControlState)?;
    persist_private_atomic(
        &control.join("state.json"),
        &bytes,
        "state.tmp-",
        control::MAX_STATE_BYTES,
        no_persist_fault,
    )
}

/// Reserve an existing admission lock for a held eviction that needs to
/// raise the fence. The pool lock is already held. Validate again once both
/// locks are held, without publishing the generation or creating a lock.
fn reserve_eviction_fence(control: &Path) -> Result<Option<File>, LeaseError> {
    let Some(state) = read_writer_state(control)? else {
        return Ok(None);
    };
    if state.minimum_writer_generation == Some(control::WRITER_GENERATION) {
        return Ok(None);
    }
    let admission = open_control_admission(control, false)?;
    let state = read_writer_state(control)?.ok_or_else(|| {
        LeaseError::ControlState(
            "state.json disappeared while taking the admission lock".to_owned(),
        )
    })?;
    control::fenced_state(state, control::WRITER_GENERATION).map_err(LeaseError::ControlState)?;
    Ok(Some(admission))
}

fn open_control_admission(control: &Path, create: bool) -> Result<File, LeaseError> {
    let path = control.join("admission.lock");
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(create)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let file = options.open(&path).map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file()
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(LeaseError::UnsafePath(
            "control admission lock must be a private regular file",
        ));
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Err(LeaseError::WriterFenceBusy);
        }
        return Err(io_error(error));
    }
    Ok(file)
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
