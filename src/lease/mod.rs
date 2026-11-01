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
pub mod control;
mod pool;
pub mod seed;
mod slice;

use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub use pool::{managed, HoldingDrain, LeaseRequest, SlotLayout};
pub use slice::{
    slice_byte_limit, AdmissionClass, MemberPair, MemberSet, OneUseSlice, RestoreOutcome,
    SliceAdmission, SliceReport, VictimPolicy, ONE_USE_MAX_BYTES, ONE_USE_MAX_SLOTS,
};

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
/// it does not raise build concurrency or `max_active` (oxide#118, oxide#177).
pub const MAX_SLOTS: usize = 256;
/// The slot ceiling an older binary understands (oxide#118's 128 slots).
/// A ledger above it must carry [`RAISED_CAPACITY_POLICY_VERSION`] so an
/// older binary refuses it at ledger read instead of reading a slot table it
/// cannot represent (oxide#177).
pub const LEGACY_MAX_SLOTS: usize = 128;
/// Finite ledger file size including the atomic temp. This constant is the
/// **worst-valid byte bound** (oxide#177; Oxide Supervisor ruling 49081d79):
/// the read and write paths refuse a ledger above it, so a larger ledger is
/// corrupt and refused, and any otherwise-valid ledger at or below it is
/// accepted — including one that carries more than one `Evicting` journal,
/// because the reader validates each slot's journal independently and bounds no
/// journal count. No count rule is added and the cap does not grow for
/// multi-journal states.
///
/// The measured states are reported separately: the raised-ceiling
/// representation at `MAX_SLOTS` with every bounded field and numeric field at
/// its maximum is 4,213,726 bytes all-idle — the figure this writer's ordinary
/// eviction path reaches, because since oxide#190 an eviction writes one
/// holding row in two O(1) synced writes and starts no journal — and
/// 5,625,456 bytes with a maximal single `DeletionJournal` on an `Evicting`
/// record that also keeps its closed observation, 2,763,152 bytes below the
/// cap. A journal is now only **legacy input**: an older writer left it, the
/// reader accepts it, and `Pool::open`'s replay rewrites or drops it. No path
/// in this binary creates a `SlotCaptureJournal`, so the journaled figure
/// bounds what an accepted legacy ledger may carry rather than what this
/// writer produces. An earlier write can still have left more than
/// one such journal in a pool, and the reader accepts any otherwise-valid
/// ledger below this cap. A write above the cap is refused, so a durable
/// ledger never exceeds it. The front door's admission gate, and the
/// library's own locked open before reconciliation, make the version-9 control
/// fence (which carries `oxide.holding/1`) durable (oxide#177, fence first), so
/// recovery, holding moves and ledger writes are all preceded by the hand-off
/// fence. A legacy journal
/// is bounded by `MAX_OUTSTANDING_CAPTURES` (96) capture records and
/// `MAX_FOREIGN_CAPTURES` (32) mismatched captures, each with a chain of at
/// most `MAX_CAPTURE_DEPTH` (64) capture private names, and by the enforced
/// journal-name bounds: a name field of at most `PATH_MAX` (1024) bytes in
/// `NAME_MAX` (255)-byte components, escaped sixfold by compact JSON. The
/// writer refuses an over-bound object per object without deleting it, and the
/// reader refuses a ledger that carries one.
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
    /// Capacity could not admit this admission because the slots a partial
    /// walk left unmeasured are charged conservatively and are never evicted on
    /// that charge (oxide#186). A bounded capacity refusal: exit code 3, the
    /// same capacity class as [`LeaseError::Full`], and never
    /// `unknown_accounting`, because no shared authority is unknown. It names
    /// at most [`MAX_REFUSAL_SLOTS`] slots and counts the rest, so one refusal
    /// line stays bounded at the slot ceiling.
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
    /// A one-use admission whose measured slice is still full after restore
    /// (design §6.9): the measured union (or `None` when it could not be
    /// measured) and the byte bound. Exit code 3, beside `Full` and
    /// `unknown_accounting`, so a printed message can never claim a pool cap.
    OneUseBudget {
        measured: Option<u64>,
        limit: u64,
    },
    UnsafePath(&'static str),
    HolderConflict,
    NotFound,
    BackingMismatch,
    LedgerExhausted,
    Unsupported(&'static str),
    Io(String),
}

/// A held eviction's failure class (oxide#196). The durable hold intent is
/// the commit point: a pre-commit failure leaves the pool bytes and identity
/// unchanged, while a post-commit failure names the recorded partial state
/// that the next locked open resumes or rolls back.
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
            Self::Usage(_) | Self::InvalidInput(_) | Self::Io(_) | Self::Unsupported(_) => 1,
            Self::Busy | Self::HolderConflict => 2,
            Self::Full
            | Self::UnknownAccounting
            | Self::UnexpectedEntry { .. }
            | Self::AccountingIncomplete(_)
            | Self::CapacityCharged { .. }
            | Self::OneUseBudget { .. }
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
            Self::CapacityCharged { .. } => "full",
            Self::OneUseBudget { .. } => "one_use_budget",
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
            Self::OneUseBudget { measured, limit } => match measured {
                Some(bytes) => write!(
                    formatter,
                    "one-use workspace slots are full: measured {bytes} of {limit} bytes with no safely evictable idle one-use slot left"
                ),
                None => write!(
                    formatter,
                    "one-use workspace slots are full: the member union could not be measured within the {limit}-byte slice bound and no safely evictable idle one-use slot is left"
                ),
            },
            Self::AccountingIncomplete(scan) => {
                write!(formatter, "owned accounting is incomplete: {scan}")
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
    /// Slot trees already moved into the payload's private holding area and
    /// pending deletion outside the pool lock (oxide#190). They are charged
    /// their recorded bytes and are neither idle nor held slots; a measured
    /// status still catches an unrecorded holding entry in its payload walk.
    #[serde(default)]
    pub holding: Vec<HoldingEntry>,
}

/// One slot tree already renamed into the payload's private holding area and
/// pending deletion outside the pool lock (oxide#190). It is a recorded fact,
/// not a claim that the tree is still there or that a deleter is running.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoldingEntry {
    pub slot_id: String,
    pub incarnation: String,
    /// The entry name below the payload's holding area.
    pub name: String,
    /// The closed observation's unique bytes, charged until the tree is gone.
    pub recorded_bytes: u64,
    /// When the entry was recorded (unix ms).
    pub at_ms: u64,
    /// Whether the holding directory was present when this view read the
    /// area. `None` is "not probed" (a lock-free snapshot never probes).
    pub present: Option<bool>,
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
    /// Held trees pending deletion outside the pool lock (oxide#190).
    pub holding: Vec<HoldingEntry>,
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
    /// Held trees pending deletion outside the pool lock (oxide#190). They are
    /// charged their recorded bytes and are never slots.
    pub holding: Vec<HoldingEntry>,
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
    /// The one-use slice observation restore made before this admission
    /// (design §6.1, §6.3). `None` for a normal warm reuse, which measures
    /// nothing; `Some` for every new-slot admission and every one-use
    /// admission, including one whose slice was empty (`NotNeeded`).
    pub slice: Option<SliceReport>,
    /// Slots this admission could not measure completely and charged
    /// conservatively instead (oxide#186). Empty when every walk completed.
    /// Their charge is not reported: only the incomplete outcome is, so a
    /// caller can record it as the oxide#128 diagnostic fact.
    pub unmeasured: Vec<UnmeasuredSlot>,
    /// The holding entries **this** admission created, in the order it created
    /// them (oxide#190). A capacity or project-cap admission may evict more than
    /// one idle slot, so the list is bounded and may hold several names. The
    /// command that made them publishes exactly these, outside the pool lock, so
    /// its own deletion is identified rather than guessed from a rotation.
    pub holding: Vec<String>,
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
    // oxide#196: an injected failure after the replacement but before the
    // parent-directory sync leaves the new ledger visible while the write
    // errors, so callers must classify the commit boundary conservatively.
    #[cfg(test)]
    if crate::test_fs::take_ledger_dir_sync_failure() {
        return Err(io_error(std::io::Error::other(
            "injected ledger directory sync failure",
        )));
    }
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)
}

fn read_private_file(path: &Path) -> Result<Vec<u8>, LeaseError> {
    read_private_file_bounded(path, MAX_LEDGER_BYTES)
}

fn read_private_file_bounded(path: &Path, max: u64) -> Result<Vec<u8>, LeaseError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != current_uid()
        || metadata.len() > max
    {
        return Err(LeaseError::UnknownAccounting);
    }
    let file = File::open(path).map_err(io_error)?;
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
/// drift.
pub const CONTROL_FENCE_VERSION: u32 = 9;
pub const CONTROL_RECORDED_ACCOUNTING_FENCE: &str = "oxide.accounting/1";
pub const CONTROL_EVICTION_ACCOUNTING_FENCE: &str = "oxide.eviction-accounting/1";
pub const CONTROL_SLOT_CEILING_FENCE: &str = "oxide.slots/256";
pub const CONTROL_ONE_USE_FENCE: &str = "oxide.one-use/1";
/// The whole-tree holding fence (oxide#190). A ledger carrying a `holding` row
/// or a `hold_intent` (the verified identity a rename is bound to), and a pool
/// whose eviction route renames a slot tree into the payload's private holding
/// area, is written only after this front-door fence is durable. An older binary
/// meets version 9 first and hands off to a receipt-matching successor instead
/// of reading a state it cannot represent.
pub const CONTROL_HOLDING_FENCE: &str = "oxide.holding/1";
/// The control version that must carry [`CONTROL_ONE_USE_FENCE`].
const CONTROL_ONE_USE_VERSION: u64 = 6;
/// Raise a front-door control state to the current fence version before a
/// locked pool open reconciles a recorded deletion (oxide#177; fence first).
///
/// The caller already holds the pool lock. An absent control state means the
/// pool has never been used by the front door (`oxide-managed/state.json` is
/// its document), so a library fixture has nothing to fence. A state already
/// at the fence version with every fence is not rewritten. A newer version, an
/// unknown fence value, a present non-string fence, a version/fence
/// combination no writer produces (including version 0, a version-6 document
/// without its one-use fence, and a class-bearing record without that fence)
/// or a non-numeric version refuses: this binary never downgrades or silently
/// repairs a document a successor wrote. Only a field genuinely absent from an
/// older document is inserted. The
/// admission lock is taken
/// non-blocking because the front door's order is admission then pool; if
/// another writer holds it, the open refuses instead of reconciling under an
/// unfenced control. Passive snapshots never call this.
pub(crate) fn fence_managed_control(pool: &Path) -> Result<(), LeaseError> {
    let control = pool.join(MANAGED_NAME);
    let path = control.join("state.json");
    // Only an absent control state means "a library fixture with nothing to
    // fence". Any other metadata failure is a refusal: a state this binary
    // cannot inspect must never let recovery run unfenced.
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(error)),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() > control::MAX_STATE_BYTES
    {
        return Err(LeaseError::UnsafePath(
            "control state must be a private regular file",
        ));
    }
    let bytes = read_private_file_bounded(&path, control::MAX_STATE_BYTES)?;
    // The acceptance test is the front door's own parse-and-validate function:
    // a document this refuses is exactly a document `Store::load` refuses, and
    // a corrupt or unsupported one is never upgraded.
    let state = control::parse_state(&bytes).map_err(|_| LeaseError::UnknownAccounting)?;
    if state.version == control::STATE_VERSION {
        return Ok(());
    }
    let admission = open_control_admission(&control)?;
    // Re-read under the writer lock: another command may have fenced while
    // this one waited for the pool lock.
    let bytes = read_private_file_bounded(&path, control::MAX_STATE_BYTES)?;
    let state = control::parse_state(&bytes).map_err(|_| LeaseError::UnknownAccounting)?;
    if state.version == control::STATE_VERSION {
        drop(admission);
        return Ok(());
    }
    let fenced = control::fenced_state(state).map_err(|_| LeaseError::UnknownAccounting)?;
    let bytes = serde_json::to_vec(&fenced).map_err(|error| LeaseError::Io(error.to_string()))?;
    if bytes.len() as u64 > control::MAX_STATE_BYTES {
        return Err(LeaseError::LedgerExhausted);
    }
    // Prove the bytes the front door will read back are accepted by the same
    // parser before replacing the durable document.
    control::parse_state(&bytes).map_err(|_| LeaseError::UnknownAccounting)?;
    persist_control_state(&path, &bytes)?;
    drop(admission);
    Ok(())
}

fn open_control_admission(control: &Path) -> Result<File, LeaseError> {
    let path = control.join("admission.lock");
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(true)
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
            return Err(LeaseError::UnknownAccounting);
        }
        return Err(io_error(error));
    }
    Ok(file)
}

fn persist_control_state(path: &Path, bytes: &[u8]) -> Result<(), LeaseError> {
    let parent = path
        .parent()
        .ok_or(LeaseError::InvalidInput("control state has no parent"))?;
    let mut temp_name = String::from("state.tmp-");
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
    // oxide#196: an injected failure after the replacement but before the
    // parent-directory sync leaves the new ledger visible while the write
    // errors, so callers must classify the commit boundary conservatively.
    #[cfg(test)]
    if crate::test_fs::take_ledger_dir_sync_failure() {
        return Err(io_error(std::io::Error::other(
            "injected ledger directory sync failure",
        )));
    }
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)
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
