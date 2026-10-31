//! Exclusive pool: short-lived flock, durable JSON ledger, idle-only eviction.

use super::backing::BackingPaths;
use super::slice::{
    AdmissionClass, MemberPair, MemberSet, OneUseSlice, RestoreOutcome, SliceAdmission,
    SliceReport, VictimPolicy,
};
use super::{
    canonical_existing_dir, current_uid, ensure_private_dir, io_error, is_finder_metadata,
    is_tolerated_metadata_file, open_verified_directory, persist_private_file, random_hex,
    read_private_file, unexpected_entry, unlink_at, AccountingCoverage, AccountingGap,
    AccountingGapKind, AccountingScan, LeaseEnvironment, LeaseError, LeaseGrant, Leased,
    LedgerSlot, LedgerView, MeasuredPool, PassiveSnapshot, PoolStatus, Profile, RecordedFact,
    RecordedVolume, ReleaseReceipt, ReuseMeasurement, SlotOccupancy, SlotStatus, UnmeasuredSlot,
    ACCOUNTING_MAX_DEPTH, ACCOUNTING_MAX_ELAPSED_MS, ACCOUNTING_MAX_ENTRIES,
    CAPACITY_POLICY_VERSION, CONFIG_NAME, LEASE_VERSION, LEDGER_NAME,
    LEGACY_CAPACITY_POLICY_VERSION, LEGACY_MAX_SLOTS, LOCK_NAME, MAX_HOLDER_BYTES,
    MAX_LEASE_ID_BYTES, MAX_PATH_BYTES, MAX_REFUSAL_SLOTS, MAX_SLOTS,
    PREVIOUS_CAPACITY_POLICY_VERSION, RAISED_CAPACITY_POLICY_VERSION, SLOTS_NAME,
};
use super::{BUDGET_BACKEND_DIRECTORY_V1, VOLUME_BACKEND_APFS_V1};
use crate::inventory::{
    inventory_directory, union_directory, GapKind, InventoryError, InventoryLimits,
};
use crate::owned_delete::{
    descriptor_identity, open_directory_at, put_back, reconcile_root_capture,
    restore_captured_mode, restore_root_capture, Capture, CaptureRecord, ForeignCapture,
    RootCapturePhase, RootCapturePlan, RootCaptureState,
};
use crate::volume::Binding;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Where a refused entry sits, for [`LeaseError::UnexpectedEntry`].
const POOL_PLACE: &str = "pool directory";
const PAYLOAD_PLACE: &str = "payload directory";

#[derive(Clone, Debug)]
pub struct LeaseRequest {
    pub pool: PathBuf,
    pub project: PathBuf,
    pub workspace: PathBuf,
    pub profile: Profile,
    pub holder: String,
    /// The soft budget for this admission. `None` uses the budget recorded
    /// at creation. It drives admission and whole-idle-slot eviction only;
    /// nothing enforces it while a build writes.
    pub budget_bytes: Option<u64>,
    /// The registered removable volume's free-space reserve, when the caller
    /// owns that fact. It is a **reserve** signal, never a pool-budget input:
    /// an unknown pool size admits on positively measured free space above
    /// this reserve, and free space at or below it prunes eligible idle whole
    /// slots until it recovers, with a named hard floor deciding when nothing
    /// is left to prune.
    pub reserve_bytes: Option<u64>,
    /// Where a new slot goes. `None` keeps the legacy `slots/<id>` path.
    pub layout: Option<SlotLayout>,
    /// A front-door **count** replacement, never byte pressure: retire this
    /// idle whole slot as part of this admission, before the new slot is
    /// allocated. Count pressure is authorised separately from the recorded
    /// byte figure — a complete pool figure, a removable pool's bound-only
    /// unknown, or an internal pool's conservative charge for a slot whose
    /// walk stopped short (oxide#186), all allow it — while a recorded
    /// byte-pressure admission takes
    /// the ordinary LRU path instead. The front door chooses the victim by
    /// recorded rank; the library rechecks its identity, ownership, occupancy
    /// and accounting under the pool lock.
    pub replace_slot: Option<u64>,
    /// This admission's workspace class (design §6.1). `Normal` for every
    /// production command while the one-use feature is dormant; a one-use
    /// admission is reachable from tests and, later, from the activated proof
    /// slice only.
    pub class: AdmissionClass,
    /// Which victims every pressure path of this admission may select
    /// (design §6.7). [`VictimPolicy::Any`] is the default and leaves a
    /// library caller's behaviour exactly as shipped.
    pub victims: VictimPolicy,
    /// The bounded member observation `restore` runs before admission
    /// (design §6.3): `Some` for every new-slot admission and every one-use
    /// admission, `None` where no measurement is allowed (normal warm reuse).
    pub slice: Option<OneUseSlice>,
}

/// Presentation path of a new slot: `slots/<folder>/<profile>-<id>`. It is
/// recorded in the ledger at creation and never changes; the slot key,
/// identity and ledger semantics do not depend on it.
#[derive(Clone, Debug, Default)]
pub struct SlotLayout {
    /// Project folder names, preferred first. The first one that holds no
    /// other repository's slot and no unknown entry is used; when none
    /// qualifies the slot keeps the legacy path.
    pub folders: Vec<String>,
    /// Ledger projects of the same repository: a folder whose slots all
    /// belong to these is this repository's folder.
    pub same_repository: Vec<PathBuf>,
    /// Leaf prefix, the Oxide profile label.
    pub profile: String,
}

/// Longest project folder or leaf name.
const MAX_SLOT_NAME_BYTES: usize = 80;

/// The private holding area at the payload root (oxide#190): a slot tree is
/// renamed whole into it under the pool lock and deleted outside it.
const HOLDING_NAME: &str = ".oxide-holding";
/// Longest holding entry name. It carries the store ID, the slot, its
/// incarnation and the generation's lease ID, all bounded.
const MAX_HOLDING_NAME_BYTES: usize = 200;
/// Most slot trees one pool may hold pending deletion. An eviction at this
/// cap refuses and deletes nothing, so the ledger bound stays derived.
const MAX_HOLDING_ENTRIES: usize = 64;
/// Depth a holding deletion descends to. A slot tree deeper than this keeps
/// the entry charged and reported instead of being half-deleted.
const HOLDING_MAX_DEPTH: usize = 64;

/// One plain holding entry name: the characters the derived name can carry.
/// It starts with the store ID (hex), never `.`, so the entry can never be
/// `.`, `..` or Finder metadata.
fn holding_name_is_bounded(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_HOLDING_NAME_BYTES
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// One plain directory name: lowercase ASCII letters, digits, `-`, `_` and
/// `.`, not starting with `.` (so never `.`, `..`, hidden or Finder
/// metadata).
fn valid_slot_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SLOT_NAME_BYTES
        && !name.starts_with('.')
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

/// A recorded slot directory: exactly `<folder>/<profile>-<id>` with plain
/// names. Anything else refuses the ledger.
fn split_slot_dir(dir: &str, id: u64) -> Option<(&str, &str)> {
    let (folder, leaf) = dir.split_once('/')?;
    let profile = leaf.strip_suffix(&format!("-{id}"))?;
    (valid_slot_name(folder) && valid_slot_name(leaf) && !profile.is_empty())
        .then_some((folder, leaf))
}

/// The folder a new slot of `layout` goes into, or `None` for the legacy
/// path. A folder already holding a slot of another project (or of a
/// project not known to share this repository) is a display collision and
/// is skipped; so is an existing folder with no recorded slot that is not
/// empty apart from Finder metadata (`occupied`). Nothing existing is moved
/// or adopted.
fn choose_folder(
    slots: &[SlotRecord],
    layout: &SlotLayout,
    occupied: impl Fn(&str) -> Result<bool, LeaseError>,
) -> Result<Option<String>, LeaseError> {
    for folder in &layout.folders {
        if !valid_slot_name(folder) {
            return Err(LeaseError::InvalidInput("invalid slot project folder"));
        }
        let mut members = slots.iter().filter(|slot| {
            slot.dir
                .as_deref()
                .and_then(|dir| dir.split_once('/'))
                .is_some_and(|(name, _)| name == folder)
        });
        let first = members.next();
        let usable = match first {
            Some(first) => std::iter::once(first)
                .chain(members)
                .all(|slot| layout.same_repository.contains(&slot.project)),
            None => !occupied(folder)?,
        };
        if usable {
            return Ok(Some(folder.clone()));
        }
    }
    Ok(None)
}

/// One member pair verified against this pool's ledger under the pool lock
/// (design §6.2). The pair is kept exactly as the front door named it so set
/// membership stays a literal comparison with the ledger row.
#[derive(Clone, Debug)]
struct VerifiedMember {
    id: u64,
    incarnation: u64,
    pair: MemberPair,
}

/// Which victim set a pressure path selects (design §6.7).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum VictimPlanKind {
    #[default]
    Any,
    Prefer,
    Only,
}

/// The admission's resolved victim plan: the preferred (evictable) members
/// that verified against the ledger, and the verified member pairs a normal
/// `Prefer` fallback must never select.
#[derive(Clone, Debug, Default)]
struct VictimPlan {
    kind: VictimPlanKind,
    preferred: Vec<VerifiedMember>,
    /// Every verified accounting member pair, used to subtract one-use slots
    /// from the normal fallback (design §6.7).
    excluded: Vec<MemberPair>,
}

impl VictimPlan {
    /// Today's behaviour, for a library caller that passes no policy and for
    /// the shipped explicit-idle-deletion paths.
    fn any() -> Self {
        Self::default()
    }

    /// Whether this plan names the ledger row as a preferred victim.
    fn prefers(&self, slot: &SlotRecord) -> bool {
        self.prefers_id(slot.id, slot.incarnation)
    }

    fn prefers_id(&self, id: u64, incarnation: u64) -> bool {
        self.preferred
            .iter()
            .any(|member| member.id == id && member.incarnation == incarnation)
    }

    /// Whether this slot is a verified one-use member that the normal fallback
    /// must leave alone.
    fn excludes(&self, slot: &SlotRecord) -> bool {
        self.excluded.iter().any(|pair| {
            pair.slot_id == slot.id.to_string() && pair.incarnation == slot.incarnation.to_string()
        })
    }
}

/// Why a member union could not be measured (design §6.4, §6.8).
#[derive(Debug)]
enum UnionGap {
    /// The bounded union walk stopped partial or timed out; its bytes are not
    /// an exact figure.
    Incomplete(AccountingScan),
    /// A member root is not the real directory the ledger claims, or the walk
    /// refused before producing a report. Unknown custody, never zero.
    Unknown(String),
}

fn union_gap_reason(gap: &UnionGap) -> String {
    match gap {
        UnionGap::Incomplete(scan) => scan.to_string(),
        UnionGap::Unknown(reason) => reason.clone(),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PoolConfig {
    version: u32,
    backend: String,
    store_id: String,
    cap_bytes: u64,
    image_bytes: u64,
    /// The removable volume of an `apfs-volume-v1` pool. Absent for every
    /// other backend, so their config bytes are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volume: Option<VolumeConfig>,
}

/// Where an `apfs-volume-v1` pool keeps its payload: `payload_dir` directly
/// under the mount root of the volume with this UUID. Config, ledger and
/// lock stay in the internal pool directory.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct VolumeConfig {
    mount_point: PathBuf,
    volume_uuid: String,
    payload_dir: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    version: u32,
    store_id: String,
    cap_bytes: u64,
    next_slot: u64,
    next_lease: u64,
    /// Permanent revised-capacity fence (oxide#118). Absent means the
    /// legacy representation; once written it is never removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capacity_policy: Option<u32>,
    /// The pool's monotonic idle-rank sequence. Legacy ledgers keep no
    /// sequence and every record's rank is 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_rank: Option<u64>,
    /// Slot trees moved whole into the payload's private holding area and
    /// pending deletion outside the pool lock (oxide#190). Absent means the
    /// pre-#190 representation, so a pool that never evicted keeps the exact
    /// bytes an older binary wrote.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    holding: Vec<HoldingRecord>,
    slots: Vec<SlotRecord>,
}

/// One slot tree already renamed into the payload's private holding area
/// (oxide#190). The row is the whole durable record of the pending deletion:
/// there is no per-entry journal, because the tree is Oxide's own private
/// directory. It is charged its recorded bytes until the tree is gone and
/// counts toward neither the slot ceiling nor the project cap.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct HoldingRecord {
    /// The entry name below the holding area, derived from the slot record's
    /// own durable facts so a crash between the rename and this row is
    /// adoptable by the next open without a second sync.
    name: String,
    slot: u64,
    incarnation: u64,
    /// The directory identity the rename observed and the deleter consumes.
    device: u64,
    inode: u64,
    /// The closed observation's unique bytes, charged until the tree is gone.
    bytes: u64,
    /// When the row was recorded (unix ms), for `oxide status`.
    at_ms: u64,
    /// When a pass last concluded with this row still in place (unix ms): its
    /// deletion refused something, or the entry could not be opened. It orders
    /// the takeover rotation, so an undeletable row cannot starve a later
    /// deletable one even while every admission is capacity-blocked and the
    /// grant/release-derived rotation stands still.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attempted_ms: Option<u64>,
}

impl HoldingRecord {
    fn is_valid(&self) -> bool {
        holding_name_is_bounded(&self.name)
            && self.slot != 0
            && self.incarnation != 0
            && self.bytes != 0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SlotRecord {
    id: u64,
    incarnation: u64,
    state: SlotPersistState,
    project: PathBuf,
    workspace: PathBuf,
    profile: Profile,
    holder: String,
    lease_id: Option<String>,
    last_lease_id: String,
    /// Slot directory below `slots`, fixed at creation; absent for the
    /// legacy `<id>` path, so a legacy ledger reads and writes unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dir: Option<String>,
    /// Idle rank from this pool's monotonic sequence (oxide#118). Legacy
    /// records keep 0 and sort oldest.
    #[serde(default, skip_serializing_if = "is_zero_rank")]
    rank: u64,
    /// The closed size observation of the generation that last went idle
    /// (oxide#146). Written only by the atomic `Held -> Idle` replacement and
    /// cleared when held again, and retained while eviction is pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recorded: Option<RecordedSize>,
    /// The journaled captures of this recorded deletion (oxide#155): the root
    /// plan, every outstanding nested capture and every capture that was not
    /// the observed object. Only an `Evicting` record may carry one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture: Option<DeletionJournal>,
    /// The verified rename a legacy pending deletion is about to perform
    /// (oxide#190): the holding entry name and the root identity the
    /// completeness preflight itself verified. Only an `Evicting` record may
    /// carry one, and it is durable **before** the rename, so a recovery pass
    /// can bind an interrupted migration's entry to the root the preflight
    /// verified instead of adopting a directory that merely has the right name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hold_intent: Option<HoldingIntent>,
}

/// One durable rename intent for a legacy pending deletion (oxide#190). It is
/// the identity binding the legacy migration needs: the hot idle-eviction path
/// keeps its single durable write after the rename (the brief's rule), while
/// this rare one-time path can afford one write before it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct HoldingIntent {
    /// The holding entry name the rename uses, exactly as derived.
    name: String,
    device: u64,
    inode: u64,
}

impl HoldingIntent {
    fn is_valid(&self) -> bool {
        holding_name_is_bounded(&self.name) && self.device != 0 && self.inode != 0
    }
}

/// One closed size observation. It is a measurement at a closing transition,
/// not proof that the slot's bytes are immutable, that its writer closed or
/// that a volume is closed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RecordedSize {
    /// The `last_lease_id` of the generation this figure measured. Every
    /// `Held` transition mints a new one, so a leftover record can never
    /// match a later generation.
    for_lease: String,
    unique_bytes: u64,
    logical_bytes: u64,
    entries: u64,
    at_ms: u64,
}

/// The durable journal of one recorded whole-tree deletion (oxide#155).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeletionJournal {
    /// The tree root: planned before its exclusive rename, captured after it,
    /// and removed only once its own removal completed.
    pub(super) root: RootCapturePlan,
    /// Captures recorded and not yet recorded removed, deepest last.
    pub(super) captures: Vec<CaptureRecord>,
    /// Captures that were not the observed object. Replay never removes a name
    /// these name, and the deletion never completes while one exists.
    pub(super) foreign: Vec<ForeignCapture>,
}

impl DeletionJournal {
    fn is_valid(&self) -> bool {
        self.root.is_valid()
            && self.captures.len() <= MAX_OUTSTANDING_CAPTURES
            && self.captures.iter().all(CaptureRecord::is_valid)
            && self.foreign.len() <= MAX_FOREIGN_CAPTURES
            && self.foreign.iter().all(ForeignCapture::is_valid)
    }
}

/// Most captures one deletion may have recorded and not yet removed. A
/// whole-tree recursion holds at most one capture per level plus the entry it
/// is finishing, and a deeper tree is refused before it is captured.
const MAX_OUTSTANDING_CAPTURES: usize = 96;
/// Most mismatched captures one deletion may record. A refusal already keeps
/// the deletion pending; the cap only bounds the ledger.
const MAX_FOREIGN_CAPTURES: usize = 32;

fn is_zero_rank(rank: &u64) -> bool {
    *rank == 0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SlotPersistState {
    Held,
    Idle,
    Evicting,
}

/// One ledger slot as the seed custody calls read it: an immutable snapshot
/// taken under the pool lock. Seeding never writes ledger state.
pub(super) struct SeedSlotView {
    pub(super) id: u64,
    pub(super) incarnation: u64,
    pub(super) lease_id: Option<String>,
    pub(super) last_lease_id: String,
    pub(super) rank: u64,
    pub(super) dir: Option<String>,
    /// The recorded project: for a front-door slot this is the pool's
    /// `oxide-managed/affinity/<record id>` directory, which is what binds a
    /// slot to its front-door record.
    pub(super) project: PathBuf,
    pub(super) held: bool,
    pub(super) idle: bool,
}

/// The pool and ledger facts call 1 and call 2 may read.
pub(super) struct SeedPoolView {
    pub(super) store_id: String,
    /// The recorded backend: `directory-budget-v1` or `apfs-volume-v1`.
    pub(super) backend: String,
    pub(super) pool_root: PathBuf,
    /// The payload root. Call 1 and the sidecar writers need no payload, so
    /// their seam leaves this `None` and opens no volume; call 2 uses
    /// [`open_seed_payload`] and gets the verified root for both backends.
    pub(super) payload_root: Option<PathBuf>,
    pub(super) slots: Vec<SeedSlotView>,
}

impl SeedPoolView {
    pub(super) fn slot(&self, id: &str) -> Option<&SeedSlotView> {
        self.slots.iter().find(|slot| slot.id.to_string() == id)
    }
}

/// The seeding custody seam (`oxide#137` §6.3 calls 1 and 2 and
/// `publish_seed_catalog`): the pool lock held for the whole transaction plus
/// the immutable facts those calls read. It takes no reconciliation and writes
/// nothing. [`open_seed`] opens no payload; [`open_seed_payload`] additionally
/// opens the verified payload call 2 clones from, including a removable
/// `apfs-volume-v1` volume's mount and volume-UID binding (§7, §8.3).
pub(super) struct SeedPoolOpen {
    _lock: File,
    pub(super) view: SeedPoolView,
}

impl SeedPoolOpen {
    /// Re-read the ledger from disk and require this donor's generation to be
    /// unchanged: exactly `Idle`, without a lease, with the planned lease id as
    /// its most recent grant. The final recheck the pin exists for.
    pub(super) fn donor_unchanged(&self, donor: &SeedSlotView) -> Result<bool, LeaseError> {
        let ledger = read_ledger(&self.view.pool_root)?;
        Ok(ledger.slots.iter().any(|slot| {
            slot.id == donor.id
                && slot.incarnation == donor.incarnation
                && slot.state == SlotPersistState::Idle
                && slot.lease_id.is_none()
                && slot.last_lease_id == donor.last_lease_id
        }))
    }

    /// Re-read the ledger and require the recipient still held by this lease.
    pub(super) fn recipient_held(&self, recipient: &SeedSlotView) -> Result<bool, LeaseError> {
        let ledger = read_ledger(&self.view.pool_root)?;
        Ok(ledger.slots.iter().any(|slot| {
            slot.id == recipient.id
                && slot.incarnation == recipient.incarnation
                && slot.state == SlotPersistState::Held
                && slot.lease_id.as_deref() == recipient.lease_id.as_deref()
        }))
    }
}

/// Open the seeding seam: the pool lock alone, no reconciliation, no ledger or
/// payload mutation. A recorded `Evicting` slot is read as it stands and never
/// finished here. No payload is opened: the sidecar writers and call 1 read
/// control state only.
pub(super) fn open_seed(root: &Path) -> Result<SeedPoolOpen, LeaseError> {
    open_seed_inner(root, false)
}

/// The same seam, additionally opening the payload call 2 clones from. For a
/// removable `apfs-volume-v1` pool this verifies the recorded mount and volume
/// UUID through [`Binding::open`] and the recorded owner-only payload
/// directory, under the pool lock and before any donor payload is read.
pub(super) fn open_seed_payload(root: &Path) -> Result<SeedPoolOpen, LeaseError> {
    open_seed_inner(root, true)
}

fn open_seed_inner(root: &Path, with_payload: bool) -> Result<SeedPoolOpen, LeaseError> {
    let root = canonical_existing_dir(root, true)?;
    refuse_foreign_pool_entries(&root)?;
    refuse_retired_image_pool(&root)?;
    let lock = lock_pool(&root)?;
    let config = read_config(&root)?;
    let ledger = read_ledger(&root)?;
    if ledger.version != LEASE_VERSION
        || ledger.store_id != config.store_id
        || ledger.cap_bytes != config.cap_bytes
    {
        return Err(LeaseError::BackingMismatch);
    }
    if ledger.slots.len() > MAX_SLOTS {
        return Err(LeaseError::LedgerExhausted);
    }
    for slot in &ledger.slots {
        if let Some(dir) = &slot.dir {
            split_slot_dir(dir, slot.id).ok_or(LeaseError::UnknownAccounting)?;
        }
    }
    let paths = BackingPaths::under(&root);
    let payload_root: Option<PathBuf> = match config.backend.as_str() {
        BUDGET_BACKEND_DIRECTORY_V1 => Some(
            open_budget_payload(&root, &paths, &config)?
                .payload_root()
                .to_path_buf(),
        ),
        // Call 2 requires the verified mount and volume-UID binding (§7): the
        // payload is opened through the same strict check a mutating pool open
        // uses, so a removed, remounted or substituted volume refuses before
        // any donor payload is read. The sidecar seam opens nothing.
        VOLUME_BACKEND_APFS_V1 if with_payload => Some(
            open_volume_payload(&paths, &config)?
                .payload_root()
                .to_path_buf(),
        ),
        VOLUME_BACKEND_APFS_V1 => None,
        _ => return Err(LeaseError::BackingMismatch),
    };
    Ok(SeedPoolOpen {
        _lock: lock,
        view: SeedPoolView {
            store_id: config.store_id.clone(),
            backend: config.backend.clone(),
            pool_root: root,
            payload_root,
            slots: ledger
                .slots
                .iter()
                .map(|slot| SeedSlotView {
                    id: slot.id,
                    incarnation: slot.incarnation,
                    lease_id: slot.lease_id.clone(),
                    last_lease_id: slot.last_lease_id.clone(),
                    rank: slot.rank,
                    dir: slot.dir.clone(),
                    project: slot.project.clone(),
                    held: slot.state == SlotPersistState::Held,
                    idle: slot.state == SlotPersistState::Idle,
                })
                .collect(),
        },
    })
}

/// Where slots live. Neither payload is an OS-enforced bound.
enum Payload {
    Budget(PathBuf),
    /// A directory on a verified removable volume, held open. Slot
    /// directories are created through `fd`, never by path.
    Volume {
        binding: Binding,
        root: PathBuf,
        fd: OwnedFd,
    },
}

impl Payload {
    fn payload_root(&self) -> &Path {
        match self {
            Self::Budget(root) | Self::Volume { root, .. } => root,
        }
    }

    /// Recheck a removable volume before Oxide changes anything on it.
    fn verify_volume(&self) -> Result<(), LeaseError> {
        match self {
            Self::Volume { binding, .. } => binding
                .verify()
                .map(|_| ())
                .map_err(|_| LeaseError::BackingMismatch),
            Self::Budget(_) => Ok(()),
        }
    }
}

pub(super) struct Pool {
    /// The exclusive pool lock, held for the life of a mutating open. A
    /// passive measurement holds none.
    _lock: Option<File>,
    root: PathBuf,
    config: PoolConfig,
    backing: Payload,
    ledger: Ledger,
    /// Admission budget: the recorded one unless the request overrides it.
    budget: u64,
    /// Bounds this open's accounting walks use.
    limits: InventoryLimits,
    /// The shared deadline for every walk of one measurement, if any. Each
    /// walk's remaining elapsed bound is recomputed from it, so the payload
    /// and every slot walk draw on one cumulative budget instead of each
    /// starting a fresh one.
    deadline: Option<Instant>,
    reconciliation_blocked: bool,
    /// A recorded journal names a root that is at neither its original nor its
    /// private name: the deletion cannot be classified, so no new lease is
    /// admitted (oxide#175, oxide#177). An unjournaled pending root or a
    /// journaled root that is still present is charged conservatively instead
    /// and does not block unrelated admissions.
    reconciliation_unresolved: bool,
    /// Why this open admitted a new slot without a complete walk, if it did.
    admission_note: Option<String>,
    /// Slots this admission could not measure completely, with the
    /// conservative charge it used instead (oxide#186), keyed by slot ID. The
    /// charge is kept for the rest of this admission, so a repeated pool figure
    /// reuses it rather than walking the slot again, and no eviction may select
    /// the slot. It is cleared at the start of every admission: a later one
    /// measures the slot again and may evict it normally.
    unmeasured: BTreeMap<u64, u64>,
    /// When this admission's first slot walk started. Every slot walk in one
    /// admission shares ONE elapsed budget (the pressure limits' elapsed
    /// bound), so many slow slots cost one bound, not one bound each; once it
    /// is spent, a further slot is charged conservatively without a walk.
    walk_started: Option<std::time::Instant>,
    /// The caller's registered free-space reserve for a removable payload.
    /// Read only by the unknown-size admission route; never a cap input.
    reserve: Option<u64>,
    /// The holding entries this admission created, in creation order
    /// (oxide#190). Reported to the caller so the command publishes exactly the
    /// trees it evicted instead of selecting one by rotation.
    created_holding: Vec<String>,
    /// Debug proof observation (oxide#190): the instant this mutating open took
    /// the pool lock. An eviction's recorded figure is the elapsed interval from
    /// here to its row write, so it measures the **whole eviction-deciding
    /// pool-lock interval** — fence, ledger read, reconciliation, admission
    /// bookkeeping, the byte-pressure decision and the recorded-total walks that
    /// feed it, victim selection, the intent write, the identity-verified rename
    /// and the row write — and not only the rename transaction. A command that
    /// evicts more than once under one lock records a figure that also covers
    /// the evictions before it. Absent from release builds.
    #[cfg(debug_assertions)]
    lock_taken: Option<Instant>,
}

/// Entry points for the `oxide` front door. They open a budget-only
/// directory or removable-volume pool; a pool recording the retired APFS
/// sparse-image backend refuses with [`LeaseError::BackingMismatch`].
pub mod managed {
    use super::super::backing::BackingPaths;
    use super::super::{
        canonical_existing_dir, ensure_private_dir, io_error, LeaseError, LeaseGrant, Leased,
        LedgerView, MeasuredPool, PassiveSnapshot, PoolInfo, PoolStatus, ReleaseReceipt,
        VolumeDescription, ACCOUNTING_MAX_ELAPSED_MS, BUDGET_BACKEND_DIRECTORY_V1, CONFIG_NAME,
        LEASE_VERSION, LEDGER_NAME, MAX_SLOTS, PAYLOAD_NAME, SLOTS_NAME, VOLUME_BACKEND_APFS_V1,
    };
    use super::LeaseRequest;
    use super::{
        lock_pool, measure_within as measure_walks, open_budget_payload, prepare_new_pool,
        random_hex, read_config, read_consistent_pair, read_ledger, recorded_volume,
        refuse_foreign_pool_entries, refuse_retired_image_pool, snapshot_of, write_config,
        write_ledger, Ledger, Payload, Pool, PoolConfig, SlotPersistState, VolumeConfig,
    };
    use crate::volume::Binding;
    use std::os::fd::AsFd;
    use std::path::Path;
    use std::time::Duration;

    pub use super::super::seed::{
        held_seed_attempt, publish_seed_catalog, read_seed_last, seed, seed_candidates,
        seed_monitored, write_seed_attempt, write_seed_last, write_seed_sources, SeedAbandon,
        SeedAttempt, SeedAttemptState, SeedBucket, SeedCandidate, SeedCandidates, SeedCatalog,
        SeedDonor, SeedFile, SeedFileKind, SeedJoin, SeedLast, SeedObservation, SeedOutcome,
        SeedPackage, SeedProducer, SeedReceipt, SeedRecipient, SeedRejectReason, SeedRejection,
        SeedRequest, SeedRole, SeedSkip, SeedSkipReason, SeedSourceKind, SeedSourceRoot, SeedUnit,
        SeedUnitRef, SeedWalkGap, SeedWalkLimits, ATTEMPT_SCHEMA,
    };

    /// Create an `apfs-volume-v1` pool: config, ledger and lock in `pool`
    /// (internal), payload in a new empty owner-only `payload_dir` on the
    /// verified volume, created through the volume's descriptor. The budget
    /// is an admission figure only.
    pub fn init_volume_pool(
        pool: &Path,
        binding: &Binding,
        payload_dir: &str,
        budget_bytes: u64,
    ) -> Result<PoolInfo, LeaseError> {
        if budget_bytes == 0 {
            return Err(LeaseError::InvalidInput("budget must be positive"));
        }
        let pool = prepare_new_pool(pool)?;
        let _lock = lock_pool(&pool)?;
        if pool.join(CONFIG_NAME).symlink_metadata().is_ok() {
            return Err(LeaseError::InvalidInput("pool already initialized"));
        }
        let payload = binding
            .dir_at(binding.fd(), payload_dir, true)
            .map_err(LeaseError::Io)?;
        let root = binding.mount_point().join(payload_dir);
        let entries = std::fs::read_dir(&root)
            .map_err(|error| LeaseError::Io(error.to_string()))?
            .count();
        if entries != 0 {
            return Err(LeaseError::InvalidInput(
                "the volume's payload directory is not empty",
            ));
        }
        binding
            .dir_at(payload.as_fd(), SLOTS_NAME, true)
            .map_err(LeaseError::Io)?;
        let store_id = random_hex(16)?;
        let config = PoolConfig {
            version: LEASE_VERSION,
            backend: VOLUME_BACKEND_APFS_V1.to_owned(),
            store_id: store_id.clone(),
            cap_bytes: budget_bytes,
            image_bytes: 0,
            volume: Some(VolumeConfig {
                mount_point: binding.mount_point().to_path_buf(),
                volume_uuid: binding.uuid().to_owned(),
                payload_dir: payload_dir.to_owned(),
            }),
        };
        let ledger = Ledger {
            version: LEASE_VERSION,
            store_id: store_id.clone(),
            cap_bytes: budget_bytes,
            next_slot: 1,
            next_lease: 1,
            capacity_policy: None,
            next_rank: None,
            holding: Vec::new(),
            slots: Vec::new(),
        };
        write_config(&pool, &config)?;
        write_ledger(&pool, &ledger)?;
        Ok(PoolInfo {
            version: LEASE_VERSION,
            store_id,
            backend: config.backend,
            cap_bytes: budget_bytes,
            verified_capacity_bytes: 0,
        })
    }

    /// Flush an `apfs-volume-v1` pool's volume with the pool lock held and
    /// return the time the flush took. Any doubt about the volume is an
    /// error.
    pub fn flush_volume(pool: &Path) -> Result<Duration, LeaseError> {
        let opened = crate::release_trace::phase("pool open", || Pool::open_for_release(pool))?;
        match &opened.backing {
            Payload::Volume { binding, .. } => binding.flush().map_err(LeaseError::Io),
            Payload::Budget(_) => Err(LeaseError::Unsupported(
                "the pool is not on a removable volume",
            )),
        }
    }

    /// Create a budget-only directory pool. The budget is recorded once and
    /// drives admission and whole-idle-slot eviction; nothing enforces it
    /// while a build writes.
    pub fn init_budget_pool(pool: &Path, budget_bytes: u64) -> Result<PoolInfo, LeaseError> {
        if budget_bytes == 0 {
            return Err(LeaseError::InvalidInput("budget must be positive"));
        }
        let pool = prepare_new_pool(pool)?;
        let _lock = lock_pool(&pool)?;
        if pool.join(CONFIG_NAME).symlink_metadata().is_ok() {
            return Err(LeaseError::InvalidInput("pool already initialized"));
        }
        let payload = pool.join(PAYLOAD_NAME);
        ensure_private_dir(&payload)?;
        ensure_private_dir(&payload.join(SLOTS_NAME))?;
        let store_id = random_hex(16)?;
        let config = PoolConfig {
            version: LEASE_VERSION,
            backend: BUDGET_BACKEND_DIRECTORY_V1.to_owned(),
            store_id: store_id.clone(),
            cap_bytes: budget_bytes,
            image_bytes: 0,
            volume: None,
        };
        let ledger = Ledger {
            version: LEASE_VERSION,
            store_id: store_id.clone(),
            cap_bytes: budget_bytes,
            next_slot: 1,
            next_lease: 1,
            capacity_policy: None,
            next_rank: None,
            holding: Vec::new(),
            slots: Vec::new(),
        };
        write_config(&pool, &config)?;
        write_ledger(&pool, &ledger)?;
        Ok(PoolInfo {
            version: LEASE_VERSION,
            store_id,
            backend: config.backend,
            cap_bytes: budget_bytes,
            verified_capacity_bytes: 0,
        })
    }

    pub fn lease(request: LeaseRequest) -> Result<LeaseGrant, LeaseError> {
        lease_measured(request).map(|leased| leased.grant)
    }

    /// [`lease`], with the reuse walk's measurement of the slot it took.
    pub fn lease_measured(request: LeaseRequest) -> Result<Leased, LeaseError> {
        Pool::open(&request.pool)?.lease(&request)
    }

    pub fn release(pool: &Path, lease_id: &str) -> Result<ReleaseReceipt, LeaseError> {
        crate::release_trace::phase("pool open", || Pool::open_for_release(pool))?
            .release_or_recover(lease_id, false)
    }

    pub fn recover(pool: &Path, lease_id: &str) -> Result<ReleaseReceipt, LeaseError> {
        Pool::open(pool)?.release_or_recover(lease_id, true)
    }

    pub fn status(pool: &Path) -> Result<PoolStatus, LeaseError> {
        Pool::open(pool)?.accounted()
    }

    /// Ledger membership without accounting walks; it takes the pool lock
    /// and reconciles recorded evictions before returning.
    pub fn ledger_view(pool: &Path) -> Result<LedgerView, LeaseError> {
        Pool::open(pool)?.ledger_view()
    }

    /// Recorded identity and ledger membership of a pool, read with no lock,
    /// no reconciliation, no creation and no write (oxide#128). A directory
    /// pool's recorded payload root must still be the private, owner-only
    /// directory the backend records; a removable pool's recorded volume is
    /// reported as recorded and never probed, so an absent volume still
    /// yields its ledger.
    ///
    /// The config and ledger are read atomically each, so one snapshot may
    /// straddle a concurrent replacement. A mismatched pair is re-read once;
    /// a second mismatch is [`LeaseError::BackingMismatch`] and is never
    /// repaired or waited for.
    pub fn snapshot(pool: &Path) -> Result<PassiveSnapshot, LeaseError> {
        let root = canonical_existing_dir(pool, true)?;
        refuse_foreign_pool_entries(&root)?;
        refuse_retired_image_pool(&root)?;
        let (config, ledger) = read_consistent_pair(&root)?;
        let paths = BackingPaths::under(&root);
        let (payload, volume) = match config.backend.as_str() {
            BUDGET_BACKEND_DIRECTORY_V1 => (
                open_budget_payload(&root, &paths, &config)?
                    .payload_root()
                    .to_path_buf(),
                None,
            ),
            VOLUME_BACKEND_APFS_V1 => {
                let volume = recorded_volume(&paths, &config)?;
                let payload = volume.mount_point.join(&volume.payload_dir);
                (payload, Some(volume))
            }
            _ => return Err(LeaseError::BackingMismatch),
        };
        Ok(snapshot_of(&config, &ledger, payload, volume))
    }

    /// The bounded walker result of a pool, with no lock, no reconciliation
    /// and no write (oxide#128). A recorded `Evicting` slot is walked as it
    /// stands and makes coverage unknown, because this measurement never
    /// finishes its deletion. Uses the existing entry, depth and elapsed
    /// bounds.
    pub fn measure(pool: &Path) -> Result<MeasuredPool, LeaseError> {
        measure_walks(pool, ACCOUNTING_MAX_ELAPSED_MS)
    }

    /// [`measure`] with a caller's remaining elapsed bound, clamped to the
    /// existing accounting deadline. One `oxide status --measure` gives each
    /// pool its share of one cumulative budget; inside a pool the payload and
    /// every slot walk draw on that same deadline rather than each starting
    /// a fresh bound.
    pub fn measure_within(pool: &Path, max_elapsed_ms: u64) -> Result<MeasuredPool, LeaseError> {
        measure_walks(pool, max_elapsed_ms)
    }

    /// Read-only description of an `apfs-volume-v1` pool from its config and
    /// ledger. It takes no lock, creates nothing, writes nothing and runs no
    /// reconciliation, so a recorded deletion is reported as it stands. Every
    /// write is an atomic rename, so a read sees the files before or after a
    /// write. A missing config or ledger is [`LeaseError::NotFound`]; any
    /// other refusal is the same shape check [`Pool::open`] makes.
    pub fn describe_volume(pool: &Path) -> Result<VolumeDescription, LeaseError> {
        let root = canonical_existing_dir(pool, true)?;
        for name in [CONFIG_NAME, LEDGER_NAME] {
            let path = root.join(name);
            #[cfg(test)]
            let metadata = crate::test_fs::symlink_metadata(&path);
            #[cfg(not(test))]
            let metadata = path.symlink_metadata();
            match metadata {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(LeaseError::NotFound);
                }
                Err(error) => return Err(io_error(error)),
                Ok(_) => {}
            }
        }
        refuse_foreign_pool_entries(&root)?;
        let paths = BackingPaths::under(&root);
        if paths.image.symlink_metadata().is_ok()
            || paths.receipt.symlink_metadata().is_ok()
            || paths.mount.symlink_metadata().is_ok()
        {
            return Err(LeaseError::BackingMismatch);
        }
        let config = read_config(&root)?;
        if config.version != LEASE_VERSION
            || config.backend != VOLUME_BACKEND_APFS_V1
            || config.cap_bytes == 0
            || config.image_bytes != 0
        {
            return Err(LeaseError::BackingMismatch);
        }
        let volume = config.volume.ok_or(LeaseError::BackingMismatch)?;
        let ledger = read_ledger(&root)?;
        if ledger.version != LEASE_VERSION
            || ledger.store_id != config.store_id
            || ledger.cap_bytes != config.cap_bytes
            || ledger.slots.len() > MAX_SLOTS
        {
            return Err(LeaseError::BackingMismatch);
        }
        let evicting_slot_ids: Vec<String> = ledger
            .slots
            .iter()
            .filter(|slot| slot.state == SlotPersistState::Evicting)
            .map(|slot| slot.id.to_string())
            .collect();
        let evicting_slots = evicting_slot_ids.len();
        Ok(VolumeDescription {
            store_id: config.store_id,
            cap_bytes: config.cap_bytes,
            mount_point: volume.mount_point,
            volume_uuid: volume.volume_uuid,
            payload_dir: volume.payload_dir,
            evicting_slots,
            evicting_slot_ids,
        })
    }

    /// Retire one recorded idle slot by ID after rechecking its durable
    /// scope, for the per-project slot-cap eviction. The slot tree is moved
    /// whole into the payload's private holding area under this lock and
    /// deleted outside it (oxide#190), so the lock is held for one rename and
    /// one durable write.
    pub fn evict_idle_slot(pool: &Path, slot_id: &str) -> Result<Vec<String>, LeaseError> {
        Pool::open(pool)?.evict_idle(slot_id)
    }

    /// The holding entry name one generation derives, from facts a caller
    /// already holds (`store_id`, slot, incarnation and the generation's lease
    /// ID). A command that evicted a slot by ID therefore names the entry it
    /// created instead of selecting one by rotation.
    pub fn holding_entry_name(
        store_id: &str,
        slot_id: &str,
        incarnation: &str,
        lease_id: &str,
    ) -> Option<String> {
        let slot: u64 = slot_id.parse().ok()?;
        let incarnation: u64 = incarnation.parse().ok()?;
        super::derived_holding_name(store_id, slot, incarnation, lease_id)
    }

    /// Delete at most one pending holding entry outside the pool lock
    /// (oxide#190). `prefer` names the entry this command just created.
    pub fn drain_holding(
        pool: &Path,
        prefer: Option<&str>,
    ) -> Result<super::HoldingDrain, LeaseError> {
        super::drain_holding(pool, prefer)
    }
}

// Test entry points: the library tests drive a budget-only directory pool.
#[cfg(test)]
pub(super) fn create_injected(pool: &Path, cap_bytes: u64) -> Result<super::PoolInfo, LeaseError> {
    managed::init_budget_pool(pool, cap_bytes)
}

#[cfg(test)]
pub(super) fn lease_injected(request: LeaseRequest) -> Result<LeaseGrant, LeaseError> {
    managed::lease(request)
}

/// Test-only: [`lease_injected`] with the holding entries that admission
/// created (oxide#190), so a test can assert the publishing contract.
#[cfg(test)]
pub(super) fn lease_holding_injected(
    request: LeaseRequest,
) -> Result<(LeaseGrant, Vec<String>), LeaseError> {
    managed::lease_measured(request).map(|leased| (leased.grant, leased.holding))
}

#[cfg(test)]
pub(super) fn release_injected(pool: &Path, lease_id: &str) -> Result<ReleaseReceipt, LeaseError> {
    managed::release(pool, lease_id)
}

#[cfg(test)]
pub(super) fn recover_injected(pool: &Path, lease_id: &str) -> Result<ReleaseReceipt, LeaseError> {
    managed::recover(pool, lease_id)
}

#[cfg(test)]
pub(super) fn evict_idle_slot_injected(
    pool: &Path,
    slot_id: &str,
) -> Result<Vec<String>, LeaseError> {
    managed::evict_idle_slot(pool, slot_id)
}

#[cfg(test)]
pub(super) fn status_injected(pool: &Path) -> Result<PoolStatus, LeaseError> {
    managed::status(pool)
}

#[cfg(test)]
pub(super) fn snapshot_injected(pool: &Path) -> Result<super::PassiveSnapshot, LeaseError> {
    managed::snapshot(pool)
}

#[cfg(test)]
pub(super) fn lock_injected(pool: &Path) -> Result<File, LeaseError> {
    lock_pool(pool)
}

/// A ledger whose store identity no longer matches its config, to prove a
/// passive snapshot refuses a mismatched pair instead of repairing it.
#[cfg(test)]
pub(super) fn mismatched_store_id_injected(pool: &Path) -> Result<(), LeaseError> {
    let mut ledger = read_ledger(pool)?;
    ledger.store_id = "0".repeat(32);
    write_ledger(pool, &ledger)
}

#[cfg(test)]
pub(super) fn measure_injected(pool: &Path) -> Result<super::MeasuredPool, LeaseError> {
    managed::measure(pool)
}

/// Test-only: record a removable `apfs-volume-v1` backend with a deliberately
/// absent mount point, so a seeding call can be shown to gate the removable
/// path from the backend alone without opening or probing any volume.
#[cfg(test)]
pub(super) fn rewrite_removable_backend_injected(
    pool: &Path,
    mount_point: &Path,
) -> Result<(), LeaseError> {
    let mut config = read_config(pool)?;
    config.backend = VOLUME_BACKEND_APFS_V1.to_owned();
    config.volume = Some(VolumeConfig {
        mount_point: mount_point.to_path_buf(),
        volume_uuid: "1".repeat(36),
        payload_dir: "payload".to_owned(),
    });
    write_config(pool, &config)
}

/// Test-only: give every idle slot a different `last_lease_id`, so a seed that
/// already holds the pool lock can be shown to re-check the donor's recorded
/// generation instead of trusting its own snapshot.
#[cfg(test)]
pub(super) fn tamper_idle_generations_injected(pool: &Path) -> Result<(), LeaseError> {
    let mut ledger = read_ledger(pool)?;
    for slot in ledger.slots.iter_mut() {
        if slot.state == SlotPersistState::Idle && slot.lease_id.is_none() {
            slot.last_lease_id = "0".repeat(32);
        }
    }
    write_ledger(pool, &ledger)
}

/// Test-only: one slot's ledger state as the ledger file records it, read
/// without opening (and therefore without reconciling) the pool.
#[cfg(test)]
pub(super) fn slot_state_injected(pool: &Path, slot_id: &str) -> Option<String> {
    let ledger = read_ledger(pool).ok()?;
    let id = slot_id.parse::<u64>().ok()?;
    ledger
        .slots
        .iter()
        .find(|slot| slot.id == id)
        .map(|slot| match slot.state {
            SlotPersistState::Held => "held".to_owned(),
            SlotPersistState::Idle => "idle".to_owned(),
            SlotPersistState::Evicting => "evicting".to_owned(),
        })
}

/// Test-only: open a passive measurement, wait, then walk, returning each
/// scan's recorded elapsed bound so the shared deadline can be observed.
#[cfg(test)]
pub(super) fn measure_after_delay_injected(
    pool: &Path,
    bound_ms: u64,
    delay_ms: u64,
) -> Result<Vec<u64>, LeaseError> {
    let opened = Pool::open_passive(pool, bound_ms)?;
    std::thread::sleep(Duration::from_millis(delay_ms));
    let status = opened.accounted()?;
    let mut bounds = vec![status.payload_accounting.max_elapsed_ms];
    bounds.extend(
        status
            .slots
            .iter()
            .map(|slot| slot.accounting.max_elapsed_ms),
    );
    Ok(bounds)
}

#[cfg(test)]
pub(super) fn mark_evicting_for_test(pool: &Path, slot_id: u64) -> Result<(), LeaseError> {
    let mut opened = Pool::open(pool)?;
    let slot = opened
        .slot_mut(slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    if slot.state != SlotPersistState::Idle || slot.lease_id.is_some() {
        return Err(LeaseError::UnknownAccounting);
    }
    slot.state = SlotPersistState::Evicting;
    slot.capture = None;
    opened.persist()
}

/// The journaled root capture recorded for one slot, read straight from the
/// ledger so a test can inspect a modelled crash state without opening (and
/// therefore reconciling) the pool.
/// Write a modelled crash state's journal straight into the ledger, without
/// opening (and therefore reconciling) the pool.
#[cfg(test)]
pub(super) fn write_capture_record_injected(
    pool: &Path,
    slot_id: u64,
    journal: super::pool::DeletionJournal,
) -> Result<(), LeaseError> {
    let mut ledger = read_ledger(pool)?;
    let slot = ledger
        .slots
        .iter_mut()
        .find(|slot| slot.id == slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    if slot.state != SlotPersistState::Evicting {
        return Err(LeaseError::UnknownAccounting);
    }
    slot.capture = Some(journal);
    write_ledger(pool, &ledger)
}

/// Test-only: the holding entry name one generation derives from its own
/// durable facts, read straight from the ledger so a test can model the crash
/// between the rename and the row without opening (and therefore reconciling)
/// the pool.
#[cfg(test)]
pub(super) fn holding_name_for_slot_injected(pool: &Path, slot_id: u64) -> Option<String> {
    let ledger = read_ledger(pool).ok()?;
    let slot = ledger.slots.iter().find(|slot| slot.id == slot_id)?;
    derived_holding_name(
        &ledger.store_id,
        slot.id,
        slot.incarnation,
        &slot.last_lease_id,
    )
}

/// Test-only: record the durable hold intent of one legacy pending deletion
/// straight into the ledger, so a test can model the migration's
/// rename-before-row interruption without opening (and reconciling) the pool.
#[cfg(test)]
pub(super) fn write_hold_intent_injected(
    pool: &Path,
    slot_id: u64,
    device: u64,
    inode: u64,
) -> Result<(), LeaseError> {
    let mut ledger = read_ledger(pool)?;
    let store_id = ledger.store_id.clone();
    let slot = ledger
        .slots
        .iter_mut()
        .find(|slot| slot.id == slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    if slot.state != SlotPersistState::Evicting {
        return Err(LeaseError::UnknownAccounting);
    }
    let name = derived_holding_name(&store_id, slot.id, slot.incarnation, &slot.last_lease_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    slot.hold_intent = Some(HoldingIntent {
        name,
        device,
        inode,
    });
    write_ledger(pool, &ledger)
}

/// Test-only: record the durable rename intent of one idle slot straight into
/// the ledger, so a test can model the interruption windows around an eviction's
/// rename without opening (and reconciling) the pool.
#[cfg(test)]
pub(super) fn hold_intent_for_test(
    pool: &Path,
    slot_id: u64,
    device: u64,
    inode: u64,
) -> Result<(), LeaseError> {
    let mut ledger = read_ledger(pool)?;
    let store_id = ledger.store_id.clone();
    let slot = ledger
        .slots
        .iter_mut()
        .find(|slot| slot.id == slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    if slot.state != SlotPersistState::Idle {
        return Err(LeaseError::UnknownAccounting);
    }
    let name = derived_holding_name(&store_id, slot.id, slot.incarnation, &slot.last_lease_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    slot.hold_intent = Some(HoldingIntent {
        name,
        device,
        inode,
    });
    write_ledger(pool, &ledger)
}

/// Test-only: move one slot tree into the holding area with an explicit
/// expected root identity, without reconciliation, so a test can prove the
/// rename boundary refuses a replaced root. Writes nothing durable beyond the
/// rename; the caller inspects the pool itself.
#[cfg(test)]
pub(super) fn hold_expecting_injected(
    pool: &Path,
    slot_id: u64,
    expected: Option<(u64, u64)>,
) -> Result<String, LeaseError> {
    let mut opened = Pool::open_passive(pool, ACCOUNTING_MAX_ELAPSED_MS)?;
    let slot = opened
        .slot(slot_id)
        .cloned()
        .ok_or(LeaseError::UnknownAccounting)?;
    let path = opened.slot_path_in(slot.id, slot.dir.as_deref())?;
    opened
        .hold_slot_tree(slot.id, &path, expected)
        .map(|row| row.name)
}

#[cfg(test)]
pub(super) fn capture_record_injected(pool: &Path, slot_id: u64) -> Option<DeletionJournal> {
    read_ledger(pool)
        .ok()?
        .slots
        .iter()
        .find(|slot| slot.id == slot_id)
        .and_then(|slot| slot.capture.clone())
}

#[cfg(test)]
pub(super) fn pending_charge_injected(
    pool: &Path,
    slot_id: u64,
) -> Result<(u64, u64, u64), LeaseError> {
    let mut opened = Pool::open(pool)?;
    let index = opened
        .ledger
        .slots
        .iter()
        .position(|slot| slot.id == slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    let largest = opened.largest_recorded_slot();
    Ok((opened.slot_figure(index, &mut Vec::new())?, largest, 0))
}

/// Test-only: one admission plus the conservative charges it used for slots a
/// partial walk left unmeasured (oxide#186), in slot order. Nothing is
/// persisted: this reads the admission's own in-memory figure.
#[cfg(test)]
pub(super) fn lease_with_charges_injected(
    request: LeaseRequest,
) -> Result<(Leased, Vec<(u64, u64)>), LeaseError> {
    let mut opened = Pool::open(&request.pool)?;
    let leased = opened.lease(&request)?;
    let charges = opened
        .unmeasured
        .iter()
        .map(|(id, charge)| (*id, *charge))
        .collect();
    Ok((leased, charges))
}

impl Pool {
    /// Open a budget-only directory or removable-volume pool. Any other
    /// recorded backend, including the retired `apfs-sparse-image-v1`,
    /// refuses before anything under the pool is touched.
    pub(super) fn open(root: &Path) -> Result<Self, LeaseError> {
        Self::open_inner(root, false)
    }

    fn open_for_release(root: &Path) -> Result<Self, LeaseError> {
        Self::open_inner(root, true)
    }

    fn open_inner(root: &Path, trace_release: bool) -> Result<Self, LeaseError> {
        let root = canonical_existing_dir(root, true)?;
        refuse_foreign_pool_entries(&root)?;
        refuse_retired_image_pool(&root)?;
        let lock = if trace_release {
            crate::release_trace::phase("pool lock", || lock_pool_inner(&root, true, true))?
        } else {
            lock_pool(&root)?
        };
        // Debug proof observation (oxide#190): an eviction's reported interval
        // starts at this lock acquisition, so it covers every decision this lock
        // hold makes instead of only the rename transaction.
        #[cfg(debug_assertions)]
        let lock_taken = Some(Instant::now());
        // Fence first (oxide#177, ruling 4c9984a1): a managed pool's control
        // state reaches the current fence version before this open can start a
        // recovery journal or finish a recorded deletion. A library fixture
        // has no control state and nothing to fence.
        super::fence_managed_control(&root)?;
        let config = read_config(&root)?;
        let paths = BackingPaths::under(&root);
        let backing = match config.backend.as_str() {
            BUDGET_BACKEND_DIRECTORY_V1 => open_budget_payload(&root, &paths, &config)?,
            VOLUME_BACKEND_APFS_V1 => open_volume_payload(&paths, &config)?,
            _ => return Err(LeaseError::BackingMismatch),
        };
        let ledger = read_ledger(&root)?;
        if ledger.version != LEASE_VERSION
            || ledger.store_id != config.store_id
            || ledger.cap_bytes != config.cap_bytes
        {
            return Err(LeaseError::BackingMismatch);
        }
        if ledger.slots.len() > MAX_SLOTS {
            return Err(LeaseError::LedgerExhausted);
        }
        for slot in &ledger.slots {
            if let Some(dir) = &slot.dir {
                split_slot_dir(dir, slot.id).ok_or(LeaseError::UnknownAccounting)?;
            }
        }
        let mut pool = Self {
            _lock: Some(lock),
            root,
            config,
            backing,
            budget: ledger.cap_bytes,
            ledger,
            limits: accounting_limits(),
            deadline: None,
            reconciliation_blocked: false,
            reconciliation_unresolved: false,
            admission_note: None,
            unmeasured: BTreeMap::new(),
            walk_started: None,
            reserve: None,
            created_holding: Vec::new(),
            #[cfg(debug_assertions)]
            lock_taken,
        };
        // Holding rows and the trees they name are reconciled first, in the
        // same locked open: an entry a crash left before its row is adopted
        // (including one interrupted between a legacy migration's rename and
        // its row), and a row whose tree is positively gone is finished.
        // Nothing here deletes. Running this before `reconcile_evicting` is
        // what makes the migration window recoverable: the interrupted
        // `Evicting` row is adopted here, so the legacy replay never meets a
        // root that its own rename already moved.
        if trace_release {
            crate::release_trace::phase("holding reconciliation", || pool.reconcile_holding())?;
            crate::release_trace::phase("pool reconciliation", || pool.reconcile_evicting())?;
        } else {
            pool.reconcile_holding()?;
            pool.reconcile_evicting()?;
        }
        Ok(pool)
    }

    /// Open the same verified backing without the pool lock and without
    /// reconciliation, for [`measure`] only. The recorded payload must still
    /// be the backend's real, verified payload root, so the walks cover what
    /// admission would cover; nothing is created or written.
    fn open_passive(root: &Path, max_elapsed_ms: u64) -> Result<Self, LeaseError> {
        let root = canonical_existing_dir(root, true)?;
        refuse_foreign_pool_entries(&root)?;
        refuse_retired_image_pool(&root)?;
        let (config, ledger) = read_consistent_pair(&root)?;
        let paths = BackingPaths::under(&root);
        let backing = match config.backend.as_str() {
            BUDGET_BACKEND_DIRECTORY_V1 => open_budget_payload(&root, &paths, &config)?,
            VOLUME_BACKEND_APFS_V1 => open_volume_payload(&paths, &config)?,
            _ => return Err(LeaseError::BackingMismatch),
        };
        let bound = max_elapsed_ms.clamp(1, ACCOUNTING_MAX_ELAPSED_MS);
        let mut limits = accounting_limits();
        limits.max_elapsed_ms = bound;
        let pending = ledger
            .slots
            .iter()
            .any(|slot| slot.state == SlotPersistState::Evicting);
        Ok(Self {
            _lock: None,
            root,
            config,
            backing,
            budget: ledger.cap_bytes,
            ledger,
            limits,
            deadline: Some(Instant::now() + Duration::from_millis(bound)),
            // Coverage for a measurement made without reconciliation is
            // unknown while a recorded deletion is pending.
            reconciliation_blocked: pending,
            reconciliation_unresolved: false,
            admission_note: None,
            unmeasured: BTreeMap::new(),
            walk_started: None,
            reserve: None,
            created_holding: Vec::new(),
            #[cfg(debug_assertions)]
            lock_taken: None,
        })
    }

    /// Recorded identity and membership, as this open read them.
    fn recorded(&self) -> PassiveSnapshot {
        let volume = match &self.config.volume {
            Some(volume) => Some(RecordedVolume {
                mount_point: volume.mount_point.clone(),
                volume_uuid: volume.volume_uuid.clone(),
                payload_dir: volume.payload_dir.clone(),
            }),
            None => None,
        };
        snapshot_of(
            &self.config,
            &self.ledger,
            self.backing.payload_root().to_path_buf(),
            volume,
        )
    }

    fn persist(&self) -> Result<(), LeaseError> {
        write_ledger(&self.root, &self.ledger)
    }

    /// A revised mutation: install the permanent capacity-policy fence if
    /// this ledger predates it, raise that fence when the slot table now
    /// exceeds the legacy ceiling, validate every stored field against the
    /// raised caps, then replace the ledger atomically. Legacy representation
    /// is never migrated by a read-only observer or by reconciliation alone.
    fn persist_revised(&mut self) -> Result<(), LeaseError> {
        self.ensure_capacity_policy()?;
        self.raise_capacity_policy_if_needed();
        validate_ledger_write(&self.ledger)?;
        self.persist()
    }

    /// The raised-ceiling fence (oxide#177) is written forward-only, in the
    /// same atomic replacement that first records more than
    /// [`LEGACY_MAX_SLOTS`] slots, so an older binary refuses the ledger
    /// before it can act on a slot table it cannot represent. Nothing is
    /// written below that count: a pool at or under 128 slots keeps a fence
    /// an older binary still reads.
    fn raise_capacity_policy_if_needed(&mut self) {
        if self.ledger.slots.len() > LEGACY_MAX_SLOTS {
            self.ledger.capacity_policy = Some(RAISED_CAPACITY_POLICY_VERSION);
        }
    }

    /// Add the permanent capacity-policy fence to an in-memory ledger that
    /// predates it. The next revised write persists it atomically with the
    /// mutation that needed it.
    fn ensure_capacity_policy(&mut self) -> Result<(), LeaseError> {
        if self.ledger.capacity_policy.is_none() {
            let next = self
                .ledger
                .slots
                .iter()
                .map(|slot| slot.rank)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(LeaseError::LedgerExhausted)?;
            self.ledger.capacity_policy = Some(CAPACITY_POLICY_VERSION);
            self.ledger.next_rank = Some(next.max(1));
        }
        // A version-1 fence (oxide#118) stays readable and is upgraded by this
        // revised write, atomically with whatever needed it. The step is
        // one-way: an older binary refuses a version-2 ledger before any
        // mutation, which is why the front door writes its control fence first.
        if self.ledger.capacity_policy == Some(LEGACY_CAPACITY_POLICY_VERSION)
            || self.ledger.capacity_policy == Some(PREVIOUS_CAPACITY_POLICY_VERSION)
        {
            self.ledger.capacity_policy = Some(CAPACITY_POLICY_VERSION);
        }
        if !matches!(
            self.ledger.capacity_policy,
            Some(CAPACITY_POLICY_VERSION) | Some(RAISED_CAPACITY_POLICY_VERSION)
        ) {
            return Err(LeaseError::UnknownAccounting);
        }
        Ok(())
    }

    /// The next idle rank. Checked before any deletion or transition; a
    /// sequence with no headroom refuses instead of wrapping or rebasing.
    fn next_rank(&self) -> Result<u64, LeaseError> {
        match self.ledger.next_rank {
            Some(next) if next != 0 && next != u64::MAX => Ok(next),
            Some(_) => Err(LeaseError::LedgerExhausted),
            None => Ok(1),
        }
    }

    /// Stamp a new actual grant or a first held-to-idle release/recovery.
    /// Replays never call this.
    fn stamp_rank(&mut self, id: u64) -> Result<(), LeaseError> {
        let rank = self.next_rank()?;
        let slot = self.slot_mut(id).ok_or(LeaseError::UnknownAccounting)?;
        slot.rank = rank;
        self.ledger.next_rank = Some(rank + 1);
        Ok(())
    }

    fn lease(&mut self, request: &LeaseRequest) -> Result<Leased, LeaseError> {
        self.created_holding.clear();
        let mut leased = self.lease_inner(request)?;
        leased.holding = std::mem::take(&mut self.created_holding);
        Ok(leased)
    }

    fn lease_inner(&mut self, request: &LeaseRequest) -> Result<Leased, LeaseError> {
        // Per-admission accounting state. A slot a partial walk leaves
        // unmeasured is charged conservatively only for this admission
        // (oxide#186), so the next admission measures it again and may evict
        // it normally.
        self.unmeasured.clear();
        self.walk_started = None;
        // A journaled deletion whose recorded root is at neither of its names
        // leaves the ledger uncertain: no new lease is admitted until it is
        // resolved (oxide#175 capture recovery, oxide#177 single-journal
        // guard). Other blocked recoveries are charged, not refused.
        if self.reconciliation_unresolved {
            return Err(LeaseError::UnknownAccounting);
        }
        if let Some(budget) = request.budget_bytes {
            if budget == 0 {
                return Err(LeaseError::InvalidInput("budget must be positive"));
            }
            self.budget = budget;
        }
        self.reserve = request.reserve_bytes;
        let project = canonical_existing_dir(&request.project, false)?;
        let workspace = canonical_existing_dir(&request.workspace, false)?;
        require_holder(&request.holder)?;
        if let Some(existing) = self.find_holder_lease(&request.holder) {
            if existing.project == project
                && existing.workspace == workspace
                && existing.profile == request.profile
                && existing.state == SlotPersistState::Held
            {
                return Ok(Leased {
                    grant: self.grant_from(existing, true)?,
                    measured: None,
                    admission_note: None,
                    slice: None,
                    unmeasured: Vec::new(),
                    holding: Vec::new(),
                });
            }
            if existing.state == SlotPersistState::Held {
                return Err(LeaseError::HolderConflict);
            }
        }
        if let Some(held) = self
            .ledger
            .slots
            .iter()
            .find(|slot| {
                slot.state == SlotPersistState::Held
                    && slot.project == project
                    && slot.profile == request.profile
            })
            .cloned()
        {
            if held.holder != request.holder {
                return Err(LeaseError::Busy);
            }
        }
        // The membership and victim policy are resolved against the ledger
        // under the caller's admission lock, before any restore decision.
        let plan = self.resolve_victims(request)?;
        let candidate = self.idle_compatible(&project, request.profile);
        // A one-use admission always carries its slice (design §6.1) and its
        // restore runs before either branch (§6.3). The reuse candidate must
        // be a verified member: a non-member idle slot is refused, never
        // relabelled (§6.6).
        if request.class == AdmissionClass::OneUse {
            let slice = request.slice.as_ref().ok_or(LeaseError::InvalidInput(
                "a one-use admission requires its bounded slice",
            ))?;
            if !slice.members.is_bounded() || slice.max_slots == 0 || slice.max_bytes == 0 {
                return Err(LeaseError::InvalidInput(
                    "the one-use slice bounds are not usable",
                ));
            }
            if let Some((idle, incarnation)) = candidate {
                let pair = MemberPair::new(idle.to_string(), incarnation.to_string());
                if !slice.members.contains(&pair) {
                    return Err(LeaseError::UnknownAccounting);
                }
            }
            let report = self.restore(slice, &plan, request.class, candidate)?;
            match &report.outcome {
                RestoreOutcome::LowerBound { unknown, .. } if *unknown > 0 => {
                    return Err(LeaseError::UnknownAccounting)
                }
                RestoreOutcome::Unmeasured { .. } => {
                    return Err(LeaseError::OneUseBudget {
                        measured: None,
                        limit: slice.max_bytes,
                    })
                }
                RestoreOutcome::Full { bytes, .. } => {
                    return Err(LeaseError::OneUseBudget {
                        measured: Some(*bytes),
                        limit: slice.max_bytes,
                    })
                }
                _ => {}
            }
            if let Some((idle, incarnation)) = candidate {
                return self
                    .reuse_idle(idle, incarnation, project, workspace, request)
                    .map(|leased| Leased {
                        slice: Some(report),
                        ..leased
                    });
            }
            self.ensure_capacity_policy()?;
            self.next_rank()?;
            let grant = self.allocate(project, workspace, request, &plan)?;
            return Ok(Leased {
                grant,
                measured: None,
                admission_note: self.admission_note.take(),
                slice: Some(report),
                unmeasured: self.unmeasured_slots(),
                holding: Vec::new(),
            });
        }
        if let Some((idle, incarnation)) = candidate {
            // A normal warm reuse measures nothing (design §6.3): the #110
            // target-only walk below is its only measurement.
            return self
                .reuse_idle(idle, incarnation, project, workspace, request)
                .map(|leased| Leased {
                    slice: None,
                    ..leased
                });
        }
        // Restore runs before every new-slot admission, normal included
        // (design §6.3). An empty member set costs no walk and no write.
        let slice_report = match request.slice.as_ref() {
            Some(slice) => {
                if !slice.members.is_bounded() || slice.max_slots == 0 || slice.max_bytes == 0 {
                    return Err(LeaseError::InvalidInput(
                        "the one-use slice bounds are not usable",
                    ));
                }
                Some(self.restore(slice, &plan, request.class, None)?)
            }
            None => None,
        };
        self.ensure_capacity_policy()?;
        self.next_rank()?;
        let grant = self.allocate(project, workspace, request, &plan)?;
        Ok(Leased {
            grant,
            measured: None,
            admission_note: self.admission_note.take(),
            slice: slice_report,
            unmeasured: self.unmeasured_slots(),
            holding: Vec::new(),
        })
    }

    /// The idle compatible slot this admission would reuse, if any.
    fn idle_compatible(&self, project: &Path, profile: Profile) -> Option<(u64, u64)> {
        self.ledger
            .slots
            .iter()
            .find(|slot| {
                slot.state == SlotPersistState::Idle
                    && slot.project == project
                    && slot.profile == profile
            })
            .map(|slot| (slot.id, slot.incarnation))
    }

    /// Take an idle compatible slot after validating its own scope.
    fn reuse_idle(
        &mut self,
        idle: u64,
        incarnation: u64,
        project: PathBuf,
        workspace: PathBuf,
        request: &LeaseRequest,
    ) -> Result<Leased, LeaseError> {
        let (slot_path, _identity) =
            self.validate_idle_slot_scope(idle, incarnation, &project, request.profile)?;
        // Compatible reuse needs this slot's own walk complete.
        let _unique = inventory_sizes(&slot_path).require_complete()?;
        let dir = self.slot(idle).and_then(|slot| slot.dir.clone());
        // The idle-rank sequence must have headroom before any grant
        // transition; exhaustion refuses without changing the pool.
        self.ensure_capacity_policy()?;
        self.next_rank()?;
        let grant = self.take_idle(idle, incarnation, project, workspace, request)?;
        Ok(Leased {
            grant,
            measured: Some(ReuseMeasurement {
                slot_id: idle.to_string(),
                incarnation: incarnation.to_string(),
                unique_allocated_bytes: _unique,
                dir,
            }),
            admission_note: None,
            slice: None,
            unmeasured: Vec::new(),
            holding: Vec::new(),
        })
    }

    /// Resolve one admission's victim policy against this pool's ledger
    /// (design §6.2, §6.7). Every pair is looked up under the pool lock; one
    /// that does not verify is dropped from the preferred set, never replaced
    /// by another slot. A preferred pair outside the accounting set is a
    /// caller error and refuses before any victim decision.
    fn resolve_victims(&self, request: &LeaseRequest) -> Result<VictimPlan, LeaseError> {
        let kind = match &request.victims {
            VictimPolicy::Any => return Ok(VictimPlan::any()),
            VictimPolicy::Prefer { .. } => VictimPlanKind::Prefer,
            VictimPolicy::Only { .. } => VictimPlanKind::Only,
        };
        let preferred = request.victims.preferred();
        if !preferred.is_bounded() {
            return Err(LeaseError::InvalidInput(
                "the evictable member set exceeds the ledger's slot bound",
            ));
        }
        let excluded: Vec<MemberPair> = match request.slice.as_ref() {
            Some(slice) => {
                if !slice.members.is_bounded() {
                    return Err(LeaseError::InvalidInput(
                        "the verified member set exceeds the ledger's slot bound",
                    ));
                }
                for pair in preferred.iter() {
                    if !slice.members.contains(pair) {
                        return Err(LeaseError::InvalidInput(
                            "an evictable member is not in the verified member set",
                        ));
                    }
                }
                slice.members.iter().cloned().collect()
            }
            None => {
                if kind != VictimPlanKind::Any {
                    return Err(LeaseError::InvalidInput(
                        "a class-scoped victim policy requires its slice",
                    ));
                }
                Vec::new()
            }
        };
        let preferred = preferred
            .iter()
            .filter_map(|pair| self.verify_member(pair))
            .collect();
        Ok(VictimPlan {
            kind,
            preferred,
            excluded,
        })
    }

    /// Verify one `(slot_id, incarnation)` pair against the ledger (§6.2).
    fn verify_member(&self, pair: &MemberPair) -> Option<VerifiedMember> {
        let slot = self.ledger.slots.iter().find(|slot| {
            slot.id.to_string() == pair.slot_id && slot.incarnation.to_string() == pair.incarnation
        })?;
        Some(VerifiedMember {
            id: slot.id,
            incarnation: slot.incarnation,
            pair: pair.clone(),
        })
    }

    /// Restore the one-use slice before an admission (design §6.3). Runs
    /// before every new-slot admission and every one-use admission, and never
    /// on a normal warm reuse. It reclaims eligible idle members oldest
    /// `(rank, slot_id)` first until both thresholds hold, the union is within
    /// its byte bound, or no eligible victim is left. The reuse candidate is
    /// never a victim; its bytes still count. A deletion always goes through
    /// [`Pool::retire_idle_slot`], and a safety failure stops the decision —
    /// it never falls through to a less-checked victim.
    ///
    /// A pool with class-unknown rows is reported as a lower bound and
    /// reclaims nothing: its bound cannot be proven, so no deletion may be
    /// spent on it (§5.6, §6.9). A partial or unusable union walk is reported
    /// `Unmeasured`, never as zero and never as an exact overage.
    fn restore(
        &mut self,
        slice: &OneUseSlice,
        plan: &VictimPlan,
        class: AdmissionClass,
        reuse: Option<(u64, u64)>,
    ) -> Result<SliceReport, LeaseError> {
        let mut members: Vec<VerifiedMember> = Vec::new();
        let mut unverified = 0usize;
        for pair in slice.members.iter() {
            match self.verify_member(pair) {
                Some(member) if !members.iter().any(|taken| taken.id == member.id) => {
                    members.push(member);
                }
                _ => unverified += 1,
            }
        }
        if members.is_empty() {
            let outcome = if slice.unknown > 0 {
                RestoreOutcome::LowerBound {
                    bytes: 0,
                    unknown: slice.unknown,
                }
            } else {
                RestoreOutcome::NotNeeded
            };
            return Ok(Self::slice_report(slice, outcome, 0, 0, unverified));
        }
        let mut bytes = match self.member_union(&members) {
            Ok(bytes) => bytes,
            Err(gap) => {
                let outcome = RestoreOutcome::Unmeasured {
                    reason: union_gap_reason(&gap),
                };
                let protected = self.protected_members(&members, plan, reuse);
                return Ok(Self::slice_report(
                    slice,
                    outcome,
                    members.len(),
                    protected,
                    unverified,
                ));
            }
        };
        if slice.unknown > 0 {
            let outcome = RestoreOutcome::LowerBound {
                bytes,
                unknown: slice.unknown,
            };
            let protected = self.protected_members(&members, plan, reuse);
            return Ok(Self::slice_report(
                slice,
                outcome,
                members.len(),
                protected,
                unverified,
            ));
        }
        // A new slot needs one free place; a warm reuse adds none. A caller
        // that declared a new slot is held to the stricter threshold even if
        // the reuse branch is taken.
        let new_slot_needed = reuse.is_none() || slice.kind == SliceAdmission::NewSlot;
        loop {
            let threshold = members.len() > slice.max_slots
                || (new_slot_needed && members.len() >= slice.max_slots)
                || bytes > slice.max_bytes;
            if !threshold {
                break;
            }
            let Some((id, incarnation)) = self.pick_restore_victim(&members, plan, reuse) else {
                break;
            };
            let Some(index) = self.ledger.slots.iter().position(|slot| {
                slot.id == id
                    && slot.incarnation == incarnation
                    && slot.state == SlotPersistState::Idle
                    && slot.lease_id.is_none()
            }) else {
                break;
            };
            let project = self.ledger.slots[index].project.clone();
            let profile = self.ledger.slots[index].profile;
            self.retire_idle_slot(id, incarnation, &project, profile)?;
            members.retain(|member| member.id != id);
            bytes = match self.member_union(&members) {
                Ok(bytes) => bytes,
                Err(gap) => {
                    let outcome = RestoreOutcome::Unmeasured {
                        reason: union_gap_reason(&gap),
                    };
                    let protected = self.protected_members(&members, plan, reuse);
                    return Ok(Self::slice_report(
                        slice,
                        outcome,
                        members.len(),
                        protected,
                        unverified,
                    ));
                }
            };
        }
        let still_over = members.len() > slice.max_slots
            || (new_slot_needed && members.len() >= slice.max_slots)
            || bytes > slice.max_bytes;
        let protected = self.protected_members(&members, plan, reuse);
        let outcome = if still_over {
            match class {
                // A one-use admission refuses with a dedicated bounded error
                // (§6.9); a normal one continues with its measured overage
                // (§6.8).
                AdmissionClass::OneUse => RestoreOutcome::Full {
                    bytes,
                    held: protected,
                },
                AdmissionClass::Normal => RestoreOutcome::Over { bytes },
            }
        } else {
            RestoreOutcome::Within { bytes }
        };
        Ok(Self::slice_report(
            slice,
            outcome,
            members.len(),
            protected,
            unverified,
        ))
    }

    fn slice_report(
        slice: &OneUseSlice,
        outcome: RestoreOutcome,
        members: usize,
        protected: usize,
        unverified: usize,
    ) -> SliceReport {
        SliceReport {
            outcome,
            members,
            protected,
            unverified,
            max_slots: slice.max_slots,
            max_bytes: slice.max_bytes,
        }
    }

    /// Verified members that cannot be a victim for this admission: not in the
    /// front door's evictable set, or the reuse candidate itself.
    fn protected_members(
        &self,
        members: &[VerifiedMember],
        plan: &VictimPlan,
        reuse: Option<(u64, u64)>,
    ) -> usize {
        members
            .iter()
            .filter(|member| {
                !plan.prefers_id(member.id, member.incarnation)
                    || Some((member.id, member.incarnation)) == reuse
            })
            .count()
    }

    /// The oldest eligible idle member that may be a victim, never the reuse
    /// candidate (design §6.3).
    fn pick_restore_victim(
        &self,
        members: &[VerifiedMember],
        plan: &VictimPlan,
        reuse: Option<(u64, u64)>,
    ) -> Option<(u64, u64)> {
        members
            .iter()
            .filter(|member| plan.prefers_id(member.id, member.incarnation))
            .filter(|member| Some((member.id, member.incarnation)) != reuse)
            .filter_map(|member| {
                self.slot(member.id)
                    .filter(|slot| slot.state == SlotPersistState::Idle && slot.lease_id.is_none())
                    .map(|slot| (slot.rank, slot.id, slot.incarnation))
            })
            .min_by_key(|(rank, id, _)| (*rank, *id))
            .map(|(_, id, incarnation)| (id, incarnation))
    }

    /// The unique allocated bytes of the member slots as one union (design
    /// §6.4). One bounded walk over every member root with one shared
    /// `(device, inode)` set, so a cross-member hard link is counted once.
    fn member_union(&self, members: &[VerifiedMember]) -> Result<u64, UnionGap> {
        let mut roots = Vec::with_capacity(members.len());
        for member in members {
            match self.slot_root(member.id) {
                Err(error) => {
                    return Err(UnionGap::Unknown(format!(
                        "member slot {} has no readable ledger row: {error}",
                        member.id
                    )))
                }
                Ok((_, None)) => continue,
                Ok((path, Some(metadata))) if metadata.file_type().is_dir() => roots.push(path),
                Ok((_, Some(_))) => {
                    return Err(UnionGap::Unknown(format!(
                        "member slot {} root is not a real directory",
                        member.id
                    )))
                }
            }
        }
        if roots.is_empty() {
            return Ok(0);
        }
        let limits = self.pressure_limits();
        match union_directory(&roots, limits) {
            Ok(report) if report.coverage.complete => Ok(report.allocated_bytes),
            Ok(report) => {
                let gaps = report
                    .coverage
                    .gaps
                    .iter()
                    .map(|gap| AccountingGap {
                        kind: gap_kind(gap.kind),
                        occurrences: gap.occurrences,
                    })
                    .collect();
                Err(UnionGap::Incomplete(scan_with(
                    limits,
                    AccountingCoverage::Partial,
                    gaps,
                    report.entries_visited,
                    report.elapsed_ms,
                    report.vanished,
                )))
            }
            Err(error) => Err(UnionGap::Unknown(error.to_string())),
        }
    }

    fn evict_idle(&mut self, slot_id: &str) -> Result<Vec<String>, LeaseError> {
        let id =
            slot_id
                .parse::<u64>()
                .ok()
                .filter(|id| *id != 0)
                .ok_or(LeaseError::InvalidInput(
                    "slot ID must be a positive integer",
                ))?;
        let Some((incarnation, project, profile)) = self
            .slot(id)
            .filter(|slot| slot.state == SlotPersistState::Idle && slot.lease_id.is_none())
            .map(|slot| (slot.incarnation, slot.project.clone(), slot.profile))
        else {
            return Err(LeaseError::Busy);
        };
        self.retire_idle_slot(id, incarnation, &project, profile)?;
        Ok(std::mem::take(&mut self.created_holding))
    }

    fn take_idle(
        &mut self,
        slot_id: u64,
        incarnation: u64,
        project: PathBuf,
        workspace: PathBuf,
        request: &LeaseRequest,
    ) -> Result<LeaseGrant, LeaseError> {
        let lease_id = self.mint_lease_id(slot_id, incarnation)?;
        {
            let slot = self
                .slot_mut(slot_id)
                .ok_or(LeaseError::UnknownAccounting)?;
            if slot.state != SlotPersistState::Idle {
                return Err(LeaseError::Busy);
            }
            slot.state = SlotPersistState::Held;
            slot.project = project;
            slot.workspace = workspace;
            slot.holder = request.holder.clone();
            slot.lease_id = Some(lease_id.clone());
            slot.last_lease_id = lease_id;
            // A held slot never borrows or keeps an idle observation, and it
            // never keeps a rename intent from its idle generation.
            slot.recorded = None;
            slot.hold_intent = None;
        }
        self.stamp_rank(slot_id)?;
        self.persist_revised()?;
        let slot = self.slot(slot_id).ok_or(LeaseError::UnknownAccounting)?;
        self.grant_from(slot, true)
    }

    fn allocate(
        &mut self,
        project: PathBuf,
        workspace: PathBuf,
        request: &LeaseRequest,
        plan: &VictimPlan,
    ) -> Result<LeaseGrant, LeaseError> {
        if let Some(victim) = request.replace_slot {
            self.replace_idle_slot_for_admission(victim, plan)?;
        }
        // Slot headroom only: the pool ceiling is replaced, never exceeded.
        // Every allocated entry counts, including held, idle, retired-backed
        // and unresolved Evicting records.
        if self.ledger.slots.len() >= MAX_SLOTS {
            self.evict_idle_until(MAX_SLOTS - 1, plan)?;
            if self.ledger.slots.len() >= MAX_SLOTS {
                // A slot this admission could not measure is kept, so the only
                // idle slot may have been excluded from the victim set and the
                // ceiling cannot be met (oxide#186). That is a bounded capacity
                // refusal naming the slot, never the unnamed count refusal;
                // with every slot measured the shipped refusal stands.
                return Err(self.count_selection_refusal());
            }
        }
        // New-slot admission compares the pool's own recorded bytes with its
        // soft cap, before and after any eviction. Below both count ceilings
        // and without byte pressure nothing is evicted. When every slot's
        // record is current the figure costs O(slots) metadata work and no
        // inventory walk at all; only a missing or invalid record is walked,
        // once, bounded.
        //
        // A removable pool whose pool figure is unknown for bound gaps only
        // takes the reserve route instead: no byte-cap comparison at all, but
        // it may prune eligible idle whole slots while free space is at or
        // below the reserve or below the hard floor, then admit or refuse.
        match self.pressure() {
            Ok(used) => {
                if used >= self.budget {
                    // A conservative charge that already exhausts the budget
                    // cannot be helped by evicting anything, so nothing is
                    // deleted (oxide#186).
                    if self.unmeasured_charge_exhausts_budget() {
                        return Err(self.capacity_refusal(used));
                    }
                    // **One** bounded victim per attempt. A held tree keeps its
                    // bytes charged until it is positively removed, so the
                    // figure cannot fall inside this locked attempt: looping
                    // here would retire every eligible idle slot, including warm
                    // ones that the next decision may not need. The caller
                    // publishes that one tree outside the lock, and the next
                    // attempt re-evaluates from scratch.
                    if !self.evict_one_idle_with(plan)? {
                        return Err(self.capacity_refusal(used));
                    }
                    let used = self.pressure()?;
                    if used >= self.budget {
                        return Err(self.capacity_refusal(used));
                    }
                }
            }
            Err(LeaseError::AccountingIncomplete(scan)) if self.is_removable() => {
                self.admit_unknown_removable(&scan, plan)?;
            }
            Err(error) => return Err(error),
        }
        let slot_id = self.ledger.next_slot;
        if slot_id == 0 {
            return Err(LeaseError::UnknownAccounting);
        }
        let incarnation = 1_u64;
        let lease_id = self.mint_lease_id(slot_id, incarnation)?;
        let dir = match &request.layout {
            Some(layout) if valid_slot_name(&layout.profile) => {
                let same_repository: Vec<PathBuf> = layout
                    .same_repository
                    .iter()
                    .filter_map(|project| fs::canonicalize(project).ok())
                    .collect();
                let layout = SlotLayout {
                    same_repository,
                    ..layout.clone()
                };
                let slots_root = self.backing.payload_root().join(SLOTS_NAME);
                choose_folder(&self.ledger.slots, &layout, |folder| {
                    folder_occupied(&slots_root.join(folder))
                })?
                .map(|folder| format!("{folder}/{}-{slot_id}", layout.profile))
            }
            Some(_) => return Err(LeaseError::InvalidInput("invalid slot profile name")),
            None => None,
        };
        let slot_path = self.slot_path_in(slot_id, dir.as_deref())?;
        match &self.backing {
            Payload::Volume { binding, fd, .. } => {
                let create = |dir: std::os::fd::BorrowedFd<'_>, name: &str, new: bool| {
                    binding
                        .dir_at(dir, name, new)
                        .map_err(|_| LeaseError::BackingMismatch)
                };
                let slots = create(fd.as_fd(), SLOTS_NAME, false)?;
                let slot = match dir.as_deref().and_then(|dir| dir.split_once('/')) {
                    Some((folder, leaf)) => {
                        let folder = create(slots.as_fd(), folder, true)?;
                        create(folder.as_fd(), leaf, true)?
                    }
                    None => create(slots.as_fd(), &slot_id.to_string(), true)?,
                };
                create(slot.as_fd(), "build", true)?;
            }
            Payload::Budget(_) => {
                if let Some(folder) = slot_path.parent() {
                    ensure_private_dir(folder)?;
                }
                ensure_private_dir(&slot_path)?;
                ensure_private_dir(&slot_path.join("build"))?;
            }
        }
        self.ledger.slots.push(SlotRecord {
            id: slot_id,
            incarnation,
            state: SlotPersistState::Held,
            project,
            workspace,
            profile: request.profile,
            holder: request.holder.clone(),
            lease_id: Some(lease_id.clone()),
            last_lease_id: lease_id,
            dir,
            rank: 0,
            recorded: None,
            capture: None,
            hold_intent: None,
        });
        self.ledger.next_slot = slot_id.checked_add(1).ok_or(LeaseError::LedgerExhausted)?;
        self.stamp_rank(slot_id)?;
        self.persist_revised()?;
        let slot = self.slot(slot_id).ok_or(LeaseError::UnknownAccounting)?;
        self.grant_from(slot, false)
    }

    fn release_or_recover(
        &mut self,
        lease_id: &str,
        _recover: bool,
    ) -> Result<ReleaseReceipt, LeaseError> {
        validate_lease_id(lease_id)?;
        let Some(index) = self.ledger.slots.iter().position(|slot| {
            slot.last_lease_id == lease_id || slot.lease_id.as_deref() == Some(lease_id)
        }) else {
            return Err(LeaseError::NotFound);
        };
        let slot = &self.ledger.slots[index];
        if slot.last_lease_id != lease_id {
            return Err(LeaseError::NotFound);
        }
        if slot.state == SlotPersistState::Idle && slot.lease_id.is_none() {
            return Ok(ReleaseReceipt {
                version: LEASE_VERSION,
                lease_id: lease_id.to_owned(),
                state: SlotOccupancy::Idle,
                replay: true,
            });
        }
        if slot.state == SlotPersistState::Evicting {
            return Err(LeaseError::UnknownAccounting);
        }
        // A real release is a ranked transition: the fence and the rank
        // sequence must be available before anything changes. A replay above
        // returns without writing, so read-only use never migrates.
        self.ensure_capacity_policy()?;
        self.next_rank()?;
        // After the front door's writer closure, owned-scratch removal and
        // removable flush: one bounded walk of this slot while its metadata is
        // warm. A failure leaves the slot honestly unrecorded and never fails
        // an otherwise successful release.
        let recorded =
            crate::release_trace::phase("release walk", || self.measure_for_release(index));
        self.ledger.slots[index].state = SlotPersistState::Idle;
        self.ledger.slots[index].lease_id = None;
        self.ledger.slots[index].recorded = recorded;
        let released = self.ledger.slots[index].id;
        self.stamp_rank(released)?;
        crate::release_trace::phase("ledger write", || self.persist_revised())?;
        Ok(ReleaseReceipt {
            version: LEASE_VERSION,
            lease_id: lease_id.to_owned(),
            state: SlotOccupancy::Idle,
            replay: false,
        })
    }

    /// Observational: walks the payload and each slot with the same bounds
    /// as admission, and reports each walk's coverage, causes and time.
    fn accounted(&self) -> Result<PoolStatus, LeaseError> {
        let accounted = self.payload_accounting();
        let mut coverage = if self.reconciliation_blocked {
            AccountingCoverage::Unknown
        } else {
            accounted.scan.coverage
        };
        let mut slots = Vec::new();
        for slot in &self.ledger.slots {
            let sizes = self.slot_accounting(slot.id)?;
            match sizes.scan.coverage {
                AccountingCoverage::Complete => {}
                AccountingCoverage::Partial if coverage == AccountingCoverage::Complete => {
                    coverage = AccountingCoverage::Partial;
                }
                AccountingCoverage::Partial | AccountingCoverage::Unknown => {
                    coverage = AccountingCoverage::Unknown;
                }
            }
            let member = ledger_slot(slot);
            slots.push(SlotStatus {
                slot_id: member.slot_id,
                incarnation: member.incarnation,
                state: member.state,
                project: member.project,
                workspace: member.workspace,
                profile: member.profile,
                holder: member.holder,
                lease_id: member.lease_id,
                dir: member.dir,
                rank: member.rank,
                logical_bytes: sizes.logical,
                unique_allocated_bytes: sizes.unique,
                accounting: sizes.scan,
            });
        }
        let full = match coverage {
            AccountingCoverage::Complete => {
                Some(accounted.unique.unwrap_or(0) >= self.config.cap_bytes)
            }
            _ => None,
        };
        Ok(PoolStatus {
            version: LEASE_VERSION,
            store_id: self.config.store_id.clone(),
            backend: self.config.backend.clone(),
            cap_bytes: self.config.cap_bytes,
            payload_logical_bytes: accounted.logical,
            payload_unique_allocated_bytes: accounted.unique,
            coverage,
            payload_accounting: accounted.scan,
            full,
            slots,
            holding: self.holding_entries(),
        })
    }

    /// Status's opening and per-record refusals without its walks: a zero
    /// slot ID and a slot-root lookup error other than `NotFound` refuse in
    /// the same ledger order. A missing root, held or idle, stays a member.
    fn ledger_view(&self) -> Result<LedgerView, LeaseError> {
        let mut slots = Vec::new();
        for slot in &self.ledger.slots {
            self.slot_root(slot.id)?;
            slots.push(ledger_slot(slot));
        }
        Ok(LedgerView {
            store_id: self.config.store_id.clone(),
            backend: self.config.backend.clone(),
            cap_bytes: self.config.cap_bytes,
            reconciliation_blocked: self.reconciliation_blocked,
            slots,
            holding: self.holding_entries(),
        })
    }

    fn grant_from(&self, slot: &SlotRecord, reused_slot: bool) -> Result<LeaseGrant, LeaseError> {
        let lease_id = slot.lease_id.clone().ok_or(LeaseError::UnknownAccounting)?;
        let build = self.slot_path(slot.id)?.join("build");
        Ok(LeaseGrant {
            version: LEASE_VERSION,
            lease_id,
            store_id: self.config.store_id.clone(),
            slot_id: slot.id.to_string(),
            incarnation: slot.incarnation.to_string(),
            project: slot.project.clone(),
            workspace: slot.workspace.clone(),
            profile: slot.profile,
            holder: slot.holder.clone(),
            state: SlotOccupancy::Held,
            reused_slot,
            env: LeaseEnvironment {
                cargo_build_build_dir: build,
                cargo_target_dir: slot.workspace.join("target"),
            },
        })
    }

    fn mint_lease_id(&mut self, slot_id: u64, incarnation: u64) -> Result<String, LeaseError> {
        let seq = self.ledger.next_lease;
        if seq == 0 {
            return Err(LeaseError::UnknownAccounting);
        }
        let lease_id = format!(
            "v{}.{}.{}.{}.{}",
            LEASE_VERSION, self.config.store_id, slot_id, incarnation, seq
        );
        if lease_id.len() > MAX_LEASE_ID_BYTES {
            return Err(LeaseError::LedgerExhausted);
        }
        if self.ledger.slots.iter().any(|slot| {
            slot.last_lease_id == lease_id || slot.lease_id.as_deref() == Some(lease_id.as_str())
        }) {
            return Err(LeaseError::LedgerExhausted);
        }
        self.ledger.next_lease = seq.checked_add(1).ok_or(LeaseError::LedgerExhausted)?;
        Ok(lease_id)
    }

    fn find_holder_lease(&self, holder: &str) -> Option<&SlotRecord> {
        self.ledger
            .slots
            .iter()
            .find(|slot| slot.holder == holder && slot.state == SlotPersistState::Held)
    }

    fn slot(&self, id: u64) -> Option<&SlotRecord> {
        self.ledger.slots.iter().find(|slot| slot.id == id)
    }

    fn slot_mut(&mut self, id: u64) -> Option<&mut SlotRecord> {
        self.ledger.slots.iter_mut().find(|slot| slot.id == id)
    }

    /// A recorded slot's path: its recorded directory, or the legacy
    /// `slots/<id>`.
    fn slot_path(&self, id: u64) -> Result<PathBuf, LeaseError> {
        let dir = self.slot(id).and_then(|slot| slot.dir.clone());
        self.slot_path_in(id, dir.as_deref())
    }

    fn slot_path_in(&self, id: u64, dir: Option<&str>) -> Result<PathBuf, LeaseError> {
        if id == 0 {
            return Err(LeaseError::UnknownAccounting);
        }
        let slots = self.backing.payload_root().join(SLOTS_NAME);
        match dir {
            None => Ok(slots.join(id.to_string())),
            Some(dir) => {
                let (folder, leaf) =
                    split_slot_dir(dir, id).ok_or(LeaseError::UnknownAccounting)?;
                Ok(slots.join(folder).join(leaf))
            }
        }
    }

    /// A count replacement requested by the front door. Complete accounting
    /// is required before the deletion, and the rank sequence was already
    /// checked before this admission changed anything. A victim that is no
    /// longer idle refuses as busy without touching it.
    fn replace_idle_slot_for_admission(
        &mut self,
        id: u64,
        plan: &VictimPlan,
    ) -> Result<(), LeaseError> {
        if id == 0 {
            return Err(LeaseError::InvalidInput(
                "replacement slot ID must be a positive integer",
            ));
        }
        // A named project-cap replacement is a normal-slot decision and must
        // never reach a verified one-use member (design §6.7).
        if self
            .slot(id)
            .is_some_and(|slot| plan.excludes(slot) || plan.prefers(slot))
        {
            return Err(LeaseError::UnknownAccounting);
        }
        // A count replacement is count pressure, not a byte-size decision: it
        // needs a complete pool figure, or a removable pool whose figure is
        // unknown for bound gaps only. An internal or mixed incompleteness
        // still refuses before anything is deleted.
        self.pressure_known_or_bounded_removable()?;
        // The named slot is a candidate like any other: a partial walk proves
        // nothing about its bytes, so it is kept even when it is the slot this
        // replacement names (oxide#186). Substituting an unrelated slot would
        // delete storage the caller did not choose, and admitting without the
        // replacement would break the project cap the caller asked to keep, so
        // only this admission refuses, naming the slot. The value decides, not
        // the name.
        if self.unmeasured.contains_key(&id) {
            return Err(self.count_selection_refusal());
        }
        let Some((incarnation, project, profile)) = self
            .slot(id)
            .filter(|slot| slot.state == SlotPersistState::Idle && slot.lease_id.is_none())
            .map(|slot| (slot.incarnation, slot.project.clone(), slot.profile))
        else {
            return Err(LeaseError::Busy);
        };
        self.retire_idle_slot(id, incarnation, &project, profile)
    }

    fn evict_idle_until(&mut self, max_slots: usize, plan: &VictimPlan) -> Result<(), LeaseError> {
        // Bound admission to the allocated entries it saw when it began, so
        // concurrent-looking state cannot extend the loop.
        let mut remaining = self.ledger.slots.len();
        while self.ledger.slots.len() > max_slots && remaining > 0 {
            // Count pressure only: a complete pool figure, or a removable
            // pool whose figure is unknown for bound gaps only. It never
            // authorises a byte-pressure deletion.
            self.pressure_known_or_bounded_removable()?;
            let before = self.ledger.slots.len();
            if !self.evict_one_idle_with(plan)? {
                break;
            }
            if self.ledger.slots.len() >= before {
                return Err(LeaseError::UnknownAccounting);
            }
            remaining -= 1;
        }
        Ok(())
    }

    /// New-slot admission for a removable pool whose pool accounting could
    /// not finish inside its bounds.
    ///
    /// Pool size is **unknown**, so this route never compares bytes against
    /// the pool cap and never uses the volume's total usage: it acts on
    /// positively measured free space, the caller's registered reserve and the
    /// named hard floor. Above the reserve (and at or above the floor) it
    /// admits with no eviction; at or below the reserve, or below the floor
    /// while anything is left to prune, it retires one eligible idle whole
    /// slot in the existing LRU order and re-reads; once nothing is left to
    /// prune, free space at or above the floor admits with a warning and below
    /// it refuses with the original incomplete scan. Everything else on the
    /// volume — caches, scratch, another tool's Desktop — is irrelevant here,
    /// which is why the decision takes no "volume used" figure at all.
    fn admit_unknown_removable(
        &mut self,
        scan: &AccountingScan,
        plan: &VictimPlan,
    ) -> Result<(), LeaseError> {
        // A bounded scan may fall back to the removable volume's reserve,
        // but a pending eviction still consumes its conservative charge. If
        // that charge alone exhausts the budget, unknown other bytes cannot
        // make room for an admission.
        if bound_gaps_only(scan) {
            let mut pending = 0_u64;
            let evicting: Vec<usize> = self
                .ledger
                .slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| slot.state == SlotPersistState::Evicting)
                .map(|(index, _)| index)
                .collect();
            for index in evicting {
                pending = pending.saturating_add(self.slot_figure(index, &mut Vec::new())?);
            }
            if pending >= self.budget {
                return Err(LeaseError::Full);
            }
        }
        let mut idle_available = true;
        loop {
            let available = self.volume_available_bytes()?;
            match unknown_removable_step(
                scan,
                available,
                self.reserve,
                UNKNOWN_REMOVABLE_FLOOR_BYTES,
                idle_available,
            )? {
                UnknownRemovableStep::Admit(note) | UnknownRemovableStep::AdmitAtFloor(note) => {
                    self.admission_note = Some(note);
                    return Ok(());
                }
                UnknownRemovableStep::Evict => {
                    // The reserve is a prune trigger: retire one eligible idle
                    // whole slot and re-read free space. When no eligible slot
                    // is left, the floor decides.
                    idle_available = self.evict_one_idle_with(plan)?;
                    if !idle_available {
                        continue;
                    }
                }
                UnknownRemovableStep::Refuse(scan) => {
                    return Err(LeaseError::AccountingIncomplete(Box::new(scan)))
                }
            }
        }
    }

    fn is_removable(&self) -> bool {
        matches!(self.backing, Payload::Volume { .. })
    }

    /// The bounds an admission's own walks use. A removable pool keeps the
    /// `VOLUME_WALK_MS` bound on the walks a pool figure needs, so a payload
    /// too large to measure in 5 s reaches the unknown-size reserve route
    /// instead of timing out mid-measurement; an internal directory pool keeps
    /// the full accounting bound.
    fn pressure_limits(&self) -> InventoryLimits {
        let mut limits = self.walk_limits();
        if self.is_removable() {
            limits.max_elapsed_ms = limits.max_elapsed_ms.min(VOLUME_WALK_MS);
        }
        limits
    }

    /// The pool's byte-pressure figure: attributable pool bytes only.
    ///
    /// Both backings use the recorded sum — `pool_scope()` plus one figure
    /// per ledger slot, where a held slot is always walked fresh, a current
    /// idle record is trusted, and a missing or invalid record is walked once
    /// and published (or, on an internal pool, charged conservatively when
    /// that walk stops short: oxide#186). A shared removable volume's other
    /// occupants (caches,
    /// scratch, another tool's files) are **not** this pool's usage and can
    /// never push it over its cap, so they are not part of this figure; the
    /// volume's free space is a reserve signal, handled separately.
    fn pressure(&mut self) -> Result<u64, LeaseError> {
        // Every recorded holding row is charged its bytes until it is
        // positively removed: an eviction moves bytes, it never frees them
        // before the tree is gone (oxide#190). A command that needs room must
        // finish a pending deletion outside the lock and retry, never decide as
        // if the bytes were already free.
        self.recorded_total()
    }

    /// Count-limit preparation: a count replacement is count pressure, never a
    /// byte-size decision, so it may proceed when the pool figure is complete
    /// **or** when a removable pool's pool figure is genuinely unknown for
    /// bound gaps only. An internal pool, a mixed or non-bound gap set, or any
    /// other accounting failure still refuses before anything is deleted.
    ///
    /// A slot charged conservatively for a partial walk (oxide#186) is not a
    /// refusal here either: count pressure only ever selects a verified idle
    /// whole slot, and an unmeasured slot is never a candidate.
    fn pressure_known_or_bounded_removable(&mut self) -> Result<(), LeaseError> {
        match self.pressure() {
            Ok(_) => Ok(()),
            Err(LeaseError::AccountingIncomplete(scan)) if self.is_removable() => {
                if bound_gaps_only(&scan) {
                    Ok(())
                } else {
                    Err(LeaseError::AccountingIncomplete(scan))
                }
            }
            Err(error) => Err(error),
        }
    }

    /// The recorded sum over the payload: the shallow scope pass plus every
    /// slot's figure. It walks no payload or slot inventory when every record
    /// is current, and only the exact slots that need one otherwise.
    ///
    /// Completed walks are published in **one** revised write before the
    /// caller decides anything, so progress survives a refusal. A slot whose
    /// own walk is partial at admission is not something the whole pool can
    /// fail on: an idle slot on an internal pool is charged conservatively
    /// into [`Pool::unmeasured`] (oxide#186) and published nowhere, so its
    /// number is never zero, never free space and never a record.
    fn recorded_total(&mut self) -> Result<u64, LeaseError> {
        // Held trees pending deletion are charged their recorded bytes until
        // they are gone (oxide#190). They are not slots, so they count toward
        // neither the slot ceiling nor the project cap.
        let mut total = self
            .pool_scope()
            .require_complete()?
            .saturating_add(self.holding_bytes());
        let mut pending: Vec<(u64, RecordedSize)> = Vec::new();
        for index in 0..self.ledger.slots.len() {
            let figure = self.slot_figure(index, &mut pending)?;
            total = total.saturating_add(figure);
        }
        if !pending.is_empty() {
            for (id, record) in pending {
                if let Some(slot) = self.slot_mut(id) {
                    if slot.state == SlotPersistState::Idle && slot.lease_id.is_none() {
                        slot.recorded = Some(record);
                    }
                }
            }
            self.persist_revised()?;
        }
        Ok(total)
    }

    /// One slot's contribution. The index is re-read after the walk, and the
    /// completion recheck refuses to publish a figure for a generation that
    /// changed while it was measured.
    ///
    /// An **idle** slot on an internal pool whose walk stopped partial with an
    /// observed figure is charged conservatively instead of refusing: max(the
    /// bytes observed before it stopped, its current-generation record if any,
    /// the largest recorded slot), at least one byte and never the whole
    /// budget. It is kept, marked unmeasured for this admission and never
    /// evicted on that charge. Everything else keeps its refusal: the slot a
    /// reuse would take (its own walk is a separate requirement), a held
    /// slot's live figure, a root that could not even be opened, a removable
    /// pool (whose documented unknown-size reserve route owns its bounded
    /// walks) and every shared ledger, backing or fence failure.
    fn slot_figure(
        &mut self,
        index: usize,
        pending: &mut Vec<(u64, RecordedSize)>,
    ) -> Result<u64, LeaseError> {
        let Some(slot) = self.ledger.slots.get(index) else {
            return Err(LeaseError::UnknownAccounting);
        };
        if slot.state == SlotPersistState::Evicting {
            // These pools have no per-slot byte ceiling. The pathname-based
            // inventory does not pin the root, so it supplies no observation
            // for an isolated eviction. Use the largest recorded slot instead.
            let largest = self.largest_recorded_slot();
            let recorded = slot
                .recorded
                .as_ref()
                .filter(|record| record.for_lease == slot.last_lease_id)
                .map_or(0, |record| record.unique_bytes);
            return Ok(largest.max(recorded));
        }
        let id = slot.id;
        let last_lease_id = slot.last_lease_id.clone();
        let idle = slot.state == SlotPersistState::Idle && slot.lease_id.is_none();
        let current = idle
            && slot
                .recorded
                .as_ref()
                .is_some_and(|record| record.for_lease == last_lease_id);
        let recorded = slot
            .recorded
            .as_ref()
            .filter(|record| record.for_lease == last_lease_id)
            .map_or(0, |record| record.unique_bytes);
        // A slot this admission already charged keeps that figure: the walk is
        // not repeated, so one slow slot costs one bounded walk, not one per
        // pressure decision.
        let cached = self.unmeasured.get(&id).copied();
        let Some(path) = self.recorded_slot_root(id)? else {
            // A missing or non-directory recorded root has no observation. The
            // scope pass counts it exactly as the whole-payload walk counted
            // it, so this slot contributes nothing on top.
            return Ok(0);
        };
        if current {
            return Ok(recorded);
        }
        if let Some(charge) = cached {
            return Ok(charge);
        }
        let mut limits = self.pressure_limits();
        let started = *self
            .walk_started
            .get_or_insert_with(std::time::Instant::now);
        let spent = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let left = limits.max_elapsed_ms.saturating_sub(spent);
        if left == 0 {
            // This admission's shared walk budget is spent: charge the slot
            // without walking it (oxide#186's conservative charge, observed 0).
            let charge = self.conservative_charge(recorded, 0);
            self.unmeasured.insert(id, charge);
            return Ok(charge);
        }
        limits.max_elapsed_ms = left;
        let sizes = self.inventory_slot_with(&path, limits)?;
        let Some(observed) = sizes.unique else {
            // The walk could not open or classify the slot root itself. That is
            // a root-identity failure, never a size this admission may charge.
            return Err(LeaseError::AccountingIncomplete(Box::new(sizes.scan)));
        };
        if sizes.scan.coverage != AccountingCoverage::Complete {
            // A partial walk of another slot, idle or held, on either backend,
            // is charged conservatively instead of refusing this admission
            // (oxide#186, extended to held and removable slots, Tom
            // 2026-10-31). One slow slot never blocks an unrelated build, and
            // an unmeasured slot is never an eviction candidate.
            let charge = self.conservative_charge(recorded, observed);
            self.unmeasured.insert(id, charge);
            return Ok(charge);
        }
        if idle {
            pending.push((
                id,
                RecordedSize {
                    for_lease: last_lease_id,
                    unique_bytes: observed,
                    logical_bytes: sizes.logical.unwrap_or(0),
                    entries: sizes.scan.entries_visited,
                    at_ms: now_ms(),
                },
            ));
        }
        Ok(observed)
    }

    /// The largest closed per-slot observation in the ledger, or 0 when none is
    /// recorded. These budget-only pools have no per-slot byte ceiling, so the
    /// largest peer observation is the conservative stand-in for a slot whose
    /// own figure is unknown (oxide#175, oxide#186).
    fn largest_recorded_slot(&self) -> u64 {
        self.ledger
            .slots
            .iter()
            .filter_map(|slot| slot.recorded.as_ref())
            .map(|record| record.unique_bytes)
            .max()
            .unwrap_or(0)
    }

    /// The conservative charge one unmeasured slot carries for this admission:
    /// the maximum of the bytes its own walk observed before it stopped, its
    /// last recorded size for the same generation, and the largest recorded
    /// slot in the pool. It is never zero, so an unmeasured slot can never be
    /// read as free space, and never the pool's whole budget, so one slow slot
    /// cannot refuse every unrelated admission by itself.
    fn conservative_charge(&self, recorded: u64, observed: u64) -> u64 {
        observed
            .max(recorded)
            .max(self.largest_recorded_slot())
            .max(UNMEASURED_MIN_CHARGE_BYTES)
    }

    /// The capacity refusal after eviction could not bring the pool under
    /// budget. With no unmeasured slot it is the shipped [`LeaseError::Full`];
    /// otherwise a bounded capacity reason names the slots a partial walk
    /// charged conservatively, at most [`MAX_REFUSAL_SLOTS`] IDs and a count.
    fn capacity_refusal(&self, measured: u64) -> LeaseError {
        if self.unmeasured.is_empty() {
            return LeaseError::Full;
        }
        LeaseError::CapacityCharged {
            measured: Some(measured),
            limit: Some(self.budget),
            unmeasured: self.unmeasured_slots(),
        }
    }

    /// The refusal when a **count** decision cannot be honoured because the
    /// slot it needs is one a partial walk left unmeasured: the pool-wide slot
    /// ceiling with no verified victim left, or a named project-cap
    /// replacement whose victim cannot be deleted (oxide#186). It is a bounded
    /// capacity reason naming the slot and carries no byte figures, because a
    /// count decision has none; the slot is kept and nothing is allocated.
    /// [`LeaseError::LedgerExhausted`] stands when every slot was measured.
    fn count_selection_refusal(&self) -> LeaseError {
        if self.unmeasured.is_empty() {
            return LeaseError::LedgerExhausted;
        }
        LeaseError::CapacityCharged {
            measured: None,
            limit: None,
            unmeasured: self.unmeasured_slots(),
        }
    }

    /// The slots this admission charged conservatively, bound to the exact
    /// generation the incomplete walk covered, in slot order. The charge is
    /// deliberately not reported: a caller may record only the incomplete
    /// outcome, never a size (oxide#186).
    fn unmeasured_slots(&self) -> Vec<UnmeasuredSlot> {
        self.unmeasured
            .keys()
            .filter_map(|id| self.slot(*id))
            .map(|slot| UnmeasuredSlot {
                slot_id: slot.id.to_string(),
                incarnation: slot.incarnation.to_string(),
                dir: slot.dir.clone(),
            })
            .collect()
    }

    /// Whether one conservative charge alone already exhausts the budget. No
    /// eviction can then bring the figure under it, so admission refuses before
    /// deleting a verified idle slot for nothing.
    fn unmeasured_charge_exhausts_budget(&self) -> bool {
        self.unmeasured
            .values()
            .any(|charge| *charge >= self.budget)
    }

    /// The shallow scope pass over the payload root (oxide#146). It counts
    /// every regular file exactly once by hard-stopping at each recorded slot
    /// root, which owns its own subtree's figures. Out-of-slot metadata,
    /// strays and orphan regions are counted here and attribute to no slot.
    fn pool_scope(&self) -> Sizes {
        let stops = match self.scope_stops() {
            Ok(stops) => stops,
            Err(LeaseError::UnknownAccounting) => {
                return Sizes {
                    logical: None,
                    unique: None,
                    scan: root_failure_with(
                        self.pressure_limits(),
                        AccountingGapKind::UnsupportedMetadata,
                        0,
                    ),
                }
            }
            Err(_) => {
                return Sizes {
                    logical: None,
                    unique: None,
                    scan: root_failure_with(
                        self.pressure_limits(),
                        AccountingGapKind::RootUnreadable,
                        0,
                    ),
                }
            }
        };
        let mut isolated = Vec::new();
        // A recorded holding entry is charged its own figure below and is
        // never counted twice by the scope pass. An entry the ledger does not
        // name is deliberately **not** isolated: the scope pass counts it, so
        // unknown bytes are charged rather than free.
        for row in &self.ledger.holding {
            isolated.push(PathBuf::from(HOLDING_NAME).join(&row.name));
        }
        for slot in &self.ledger.slots {
            if slot.state == SlotPersistState::Evicting {
                let Ok(relative) = self.slot_relative(slot) else {
                    return Sizes {
                        logical: None,
                        unique: None,
                        scan: root_failure_with(
                            self.pressure_limits(),
                            AccountingGapKind::UnsupportedMetadata,
                            0,
                        ),
                    };
                };
                isolated.push(relative.clone());
                if let Some(journal) = &slot.capture {
                    let mut private = relative;
                    private.set_file_name(&journal.root.private);
                    isolated.push(private);
                }
            }
        }
        scope_sizes_with(
            self.backing.payload_root(),
            &stops,
            &isolated,
            self.pressure_limits(),
        )
    }

    /// The stop predicate: every recorded slot root that is still a real
    /// directory, as a path relative to the payload root. Membership comes
    /// from the **ledger**, not from record validity, so an idle slot is never
    /// walked twice; a root that is missing or another kind is not stopped, so
    /// the scope pass counts it exactly as the whole-payload walk counts it.
    fn scope_stops(&self) -> Result<Vec<PathBuf>, LeaseError> {
        let mut stops = Vec::with_capacity(self.ledger.slots.len());
        for slot in &self.ledger.slots {
            let relative = self.slot_relative(slot)?;
            if slot.state != SlotPersistState::Evicting {
                if self.recorded_slot_root(slot.id)?.is_some() {
                    stops.push(relative);
                }
                continue;
            }
            // Pending names use the separate isolation list in pool_scope.
        }
        Ok(stops)
    }

    /// A slot's recorded directory relative to the payload root: `slots/<id>`
    /// (legacy) or `slots/<folder>/<profile>-<id>`.
    fn slot_relative(&self, slot: &SlotRecord) -> Result<PathBuf, LeaseError> {
        let mut relative = PathBuf::from(SLOTS_NAME);
        match slot.dir.as_deref() {
            None => relative.push(slot.id.to_string()),
            Some(dir) => {
                let (folder, leaf) =
                    split_slot_dir(dir, slot.id).ok_or(LeaseError::UnknownAccounting)?;
                relative.push(folder);
                relative.push(leaf);
            }
        }
        Ok(relative)
    }

    /// A slot's recorded root, verified as a real directory (no follow).
    /// `None` when the root is missing or another kind.
    fn recorded_slot_root(&self, id: u64) -> Result<Option<PathBuf>, LeaseError> {
        let (path, metadata) = self.slot_root(id)?;
        Ok(metadata.filter(std::fs::Metadata::is_dir).map(|_| path))
    }

    /// One slot's own bounded walk, the same call `slot_accounting` makes.
    fn inventory_slot(&self, path: &Path) -> Result<Sizes, LeaseError> {
        Ok(inventory_sizes_with(path, 0, self.walk_limits()))
    }

    /// One slot's bounded walk under explicit bounds.
    fn inventory_slot_with(
        &self,
        path: &Path,
        limits: InventoryLimits,
    ) -> Result<Sizes, LeaseError> {
        Ok(inventory_sizes_with(path, 0, limits))
    }

    /// One bounded walk of the slot this transition closes, after the front
    /// door's positive writer closure, owned-scratch removal and removable
    /// flush, and before the `Held -> Idle` replacement. `None` for any doubt:
    /// the transition still succeeds and the slot is honestly unrecorded.
    fn measure_for_release(&self, index: usize) -> Option<RecordedSize> {
        let slot = self.ledger.slots.get(index)?;
        if slot.state != SlotPersistState::Held || slot.lease_id.is_none() {
            return None;
        }
        let id = slot.id;
        let incarnation = slot.incarnation;
        let last_lease_id = slot.last_lease_id.clone();
        let path = self.recorded_slot_root(id).ok()??;
        let sizes = self.inventory_slot(&path).ok()?;
        let unique = sizes.require_complete().ok()?;
        // Conditional publication: the generation this figure measured must
        // still be the one about to become idle.
        let slot = self.ledger.slots.get(index)?;
        if slot.state != SlotPersistState::Held
            || slot.id != id
            || slot.incarnation != incarnation
            || slot.last_lease_id != last_lease_id
        {
            return None;
        }
        Some(RecordedSize {
            for_lease: last_lease_id,
            unique_bytes: unique,
            logical_bytes: sizes.logical.unwrap_or(0),
            entries: sizes.scan.entries_visited,
            at_ms: now_ms(),
        })
    }

    /// The bound removable volume's free bytes, when they are positively
    /// known. This is the reserve signal only; it never becomes pool usage.
    /// A volume that cannot be verified is a backing mismatch, not a refusal
    /// to measure some other number.
    fn volume_available_bytes(&self) -> Result<Option<u64>, LeaseError> {
        let Payload::Volume { binding, .. } = &self.backing else {
            return Err(LeaseError::UnknownAccounting);
        };
        let facts = binding.verify().map_err(|_| LeaseError::BackingMismatch)?;
        Ok(facts.available_bytes)
    }

    /// One eligible idle victim for this admission's policy (design §6.7),
    /// oldest `(rank, numeric slot id)` first. Held, retired-backed and
    /// unresolved `Evicting` records are never candidates. A safety failure on
    /// the selected victim stops admission; it never skips to another slot.
    ///
    /// `Any` is today's behaviour. `Only` selects from the evictable member set
    /// alone, so a normal slot and a class-unknown orphan are impossible
    /// victims. `Prefer` selects an evictable member first and otherwise the
    /// ordinary order with **every** verified one-use pair removed.
    fn evict_one_idle_with(&mut self, plan: &VictimPlan) -> Result<bool, LeaseError> {
        // A slot this admission could not measure completely is never a victim:
        // its conservative charge is a capacity figure, not custody, and a
        // partial walk proves nothing about its bytes (oxide#186).
        let unmeasured = &self.unmeasured;
        let eligible = |slot: &SlotRecord| {
            slot.state == SlotPersistState::Idle
                && slot.lease_id.is_none()
                && !unmeasured.contains_key(&slot.id)
        };
        let oldest = |predicate: &dyn Fn(&SlotRecord) -> bool| {
            self.ledger
                .slots
                .iter()
                .filter(|slot| eligible(slot) && predicate(slot))
                .min_by_key(|slot| (slot.rank, slot.id))
                .map(|slot| {
                    (
                        slot.id,
                        slot.incarnation,
                        slot.project.clone(),
                        slot.profile,
                    )
                })
        };
        let victim = match plan.kind {
            VictimPlanKind::Any => oldest(&|_| true),
            VictimPlanKind::Only => oldest(&|slot| plan.prefers(slot)),
            VictimPlanKind::Prefer => {
                oldest(&|slot| plan.prefers(slot)).or_else(|| oldest(&|slot| !plan.excludes(slot)))
            }
        };
        let Some((id, incarnation, project, profile)) = victim else {
            return Ok(false);
        };
        self.retire_idle_slot(id, incarnation, &project, profile)?;
        Ok(true)
    }

    fn retire_idle_slot(
        &mut self,
        id: u64,
        incarnation: u64,
        project: &Path,
        profile: Profile,
    ) -> Result<(), LeaseError> {
        // Debug proof observation (oxide#190): the figure an eviction reports is
        // the **whole eviction-deciding pool-lock interval** — measured from this
        // open's lock acquisition (`lock_taken`) to this function's row write, so
        // it covers the byte-pressure decision and the recorded-total walks that
        // feed it, the victim selection, and then the rank policy and victim
        // re-checks, the intent write, the identity-verified rename and the row
        // write. It is not an inner fragment, and not only the rename
        // transaction; a command that evicts more than once under one lock
        // records a figure that also covers the evictions before it. Absent from
        // release builds and armed only inside the supervisor's own scratch
        // layout.
        #[cfg(debug_assertions)]
        let lock_started = self.lock_taken.unwrap_or_else(Instant::now);
        // A legacy #155 journal must still be resolved before anything moves
        // (oxide#177). Since oxide#190 no path starts a journal, so this guard is
        // about legacy input an older writer left: it covers every entry point,
        // including the explicit `evict_idle_slot` that does not pass through
        // `lease()`'s blocked-reconciliation refusal, so a holding move never
        // races a journal the replay still has to resolve. The pending record and
        // its journal are kept, and a later open retries reconciliation.
        if self.reconciliation_blocked
            || self.ledger.slots.iter().any(|slot| slot.capture.is_some())
        {
            return Err(LeaseError::UnknownAccounting);
        }
        // Every durable idle deletion, including the explicit
        // `evict_idle_slot` path, checks the rank sequence before its first
        // transition: an exhausted or missing-headroom sequence refuses
        // without deleting anything.
        self.ensure_capacity_policy()?;
        self.next_rank()?;
        let (path, identity) = self.validate_idle_slot_scope(id, incarnation, project, profile)?;
        // The slot tree is Oxide's own private directory, so it needs no
        // per-entry journal: one rename moves it whole into the payload's
        // holding area on the same device and one durable write records the
        // holding row while dropping the slot row (oxide#190). The tree is
        // deleted outside this lock by the command that evicted it or by a later
        // one.
        //
        // The root identity this scope verified is bound **durably before** the
        // rename, on this ordinary path as well as the legacy one. It costs one
        // extra synced write per eviction and no per-entry work at all, and it is
        // what makes a crash between the rename and the row write recoverable by
        // identity rather than by a name: the next locked open finds the
        // generation's row, its intent and the tree under the derived name, and
        // `reconcile_holding` adopts it only because the entry is the very object
        // the intent names.
        self.record_hold_intent(id, identity)?;
        let row = self.hold_slot_tree(id, &path, Some(identity))?;
        self.created_holding.push(row.name.clone());
        self.ledger.slots.retain(|slot| slot.id != id);
        self.ledger.holding.push(row);
        self.persist_revised()?;
        #[cfg(debug_assertions)]
        record_eviction_lock_ms(&self.root, id, lock_started.elapsed().as_millis());
        Ok(())
    }

    fn validate_idle_slot_scope(
        &self,
        id: u64,
        incarnation: u64,
        project: &Path,
        profile: Profile,
    ) -> Result<(PathBuf, (u64, u64)), LeaseError> {
        let slot = self.slot(id).ok_or(LeaseError::UnknownAccounting)?;
        if slot.id != id
            || slot.incarnation != incarnation
            || slot.state != SlotPersistState::Idle
            || slot.lease_id.is_some()
            || slot.project.as_path() != project
            || slot.profile != profile
        {
            return Err(LeaseError::UnknownAccounting);
        }

        let path = self.slot_path(id)?;
        let slots_root = self.backing.payload_root().join(SLOTS_NAME);
        let slots_root = canonical_existing_dir(&slots_root, true)?;
        require_slot_parent(&path, &slots_root)?;
        canonical_existing_dir(&path, true)?;
        let slots_root_metadata = fs::symlink_metadata(&slots_root).map_err(io_error)?;
        let slot_metadata = fs::symlink_metadata(&path).map_err(io_error)?;
        if slot_metadata.file_type().is_symlink()
            || !slot_metadata.file_type().is_dir()
            || slot_metadata.uid() != current_uid()
            || slot_metadata.permissions().mode() & 0o077 != 0
            || slots_root_metadata.dev() != slot_metadata.dev()
        {
            return Err(LeaseError::UnknownAccounting);
        }
        Ok((path, (slot_metadata.dev(), slot_metadata.ino())))
    }

    fn reconcile_evicting(&mut self) -> Result<(), LeaseError> {
        let evicting: Vec<u64> = self
            .ledger
            .slots
            .iter()
            .filter(|slot| slot.state == SlotPersistState::Evicting)
            .map(|slot| slot.id)
            .collect();
        for id in evicting {
            let path = self.slot_path(id)?;
            let removed = self
                .slot(id)
                .and_then(|slot| slot.capture.as_ref())
                .is_some_and(|journal| journal.root.phase == RootCapturePhase::Removed);
            // A legacy deletion's tree must pass the #155 completeness
            // preflight before it is moved: an uncertain, partially
            // inventoried or replaced root keeps its record and blocks the
            // next admission instead of being held and deleted.
            let ready = if removed {
                None
            } else if let Some(root) = self.pending_root_verified(id, &path) {
                Some(root)
            } else {
                if self.recorded_root_is_untracked(id, &path) {
                    self.reconciliation_unresolved = true;
                }
                self.reconciliation_blocked = true;
                continue;
            };
            // A journaled root capture is reconciled first: the recorded path
            // may be absent because the tree is under its private name, and
            // deleting by the recorded path alone would conclude a deletion
            // that never finished while the captured bytes stayed behind.
            match self.reconcile_slot_capture(id, &path) {
                Ok(CaptureDisposition::Resume) => {}
                Ok(CaptureDisposition::Concluded) => {
                    self.ledger.slots.retain(|slot| slot.id != id);
                    self.persist()?;
                    continue;
                }
                Ok(CaptureDisposition::Blocked) | Err(_) => {
                    self.reconciliation_blocked = true;
                    continue;
                }
            }
            // The journal is resolved and the root is back at its recorded
            // path, so the pending deletion continues as a whole-tree move
            // into the holding area (oxide#190): no new per-entry journal is
            // started, and the tree is deleted outside the pool lock.
            let expected = ready
                .as_ref()
                .and_then(|root| descriptor_identity(root.as_fd()).ok());
            if ready.is_some() && expected.is_none() {
                self.reconciliation_blocked = true;
                continue;
            }
            // The identity this deletion is about to move is durable **before**
            // the rename, so a recovery pass can bind an interrupted migration
            // to the root the preflight verified instead of adopting whatever
            // directory happens to carry the derived name.
            let Some(identity) = expected else {
                self.reconciliation_blocked = true;
                continue;
            };
            if self.record_hold_intent(id, identity).is_err() {
                self.reconciliation_blocked = true;
                continue;
            }
            let held = self.hold_slot_tree(id, &path, Some(identity));
            drop(ready);
            let row = match held {
                Ok(row) => row,
                Err(_) => {
                    self.reconciliation_blocked = true;
                    continue;
                }
            };
            self.ledger.slots.retain(|slot| slot.id != id);
            self.ledger.holding.push(row);
            if self.persist_revised().is_err() {
                self.reconciliation_blocked = true;
                continue;
            }
        }
        Ok(())
    }

    /// A journaled pending root that is at neither its original nor its
    /// private recorded name cannot be reconciled; it stays pending and no new
    /// lease is admitted. A root that is still present (a collision or an
    /// unjournaled pending tree) is charged conservatively instead.
    fn recorded_root_is_untracked(&self, id: u64, path: &Path) -> bool {
        let Some(journal) = self.slot(id).and_then(|slot| slot.capture.as_ref()) else {
            return false;
        };
        let private = path
            .parent()
            .map(|parent| parent.join(&journal.root.private));
        fs::symlink_metadata(path).is_err()
            && private.is_none_or(|private| fs::symlink_metadata(private).is_err())
    }

    /// The pending root one legacy deletion is about to move, verified in O(1)
    /// under the pool lock (oxide#190).
    ///
    /// This used to walk the whole pending tree recursively — a preflight #155
    /// needed because its deletion captured and unlinked entry by entry. That
    /// walk ran while `Pool::open` held the pool lock, so one large legacy
    /// pending tree could stall every admission on the host. A whole-tree rename
    /// needs no inventory: the root is opened without following a link, its
    /// identity, owner, mode and device are verified (against the journal when
    /// one exists), and the tree is moved whole. Per-object refusals belong to
    /// the outside-lock deletion, which keeps anything it cannot delete. An
    /// absent, replaced or unclassifiable root still refuses, so an uncertain
    /// record is never reinterpreted.
    fn pending_root_verified(&self, id: u64, path: &Path) -> Option<OwnedFd> {
        let slot = self.slot(id)?;
        if self.checked_slot_scope(path).is_err() {
            return None;
        }
        let parent = super::open_verified_directory(path.parent()?).ok()?;
        let original = std::ffi::CString::new(path.file_name()?.as_bytes()).ok()?;
        let candidates = if let Some(journal) = &slot.capture {
            match reconcile_root_capture(parent.as_fd(), &original, &journal.root) {
                RootCaptureState::Original => vec![original],
                RootCaptureState::Private => {
                    vec![std::ffi::CString::new(journal.root.private.as_bytes()).ok()?]
                }
                _ => return None,
            }
        } else {
            vec![original]
        };
        candidates.into_iter().find_map(|candidate| {
            let facts = crate::owned_delete::observe_entry(parent.as_fd(), &candidate).ok()?;
            if !facts.is_directory() || facts.uid != current_uid() || facts.mode_bits() & 0o077 != 0
            {
                return None;
            }
            if let Some(journal) = &slot.capture {
                if facts.device != journal.root.device || facts.inode != journal.root.inode {
                    return None;
                }
            }
            let (root, mut mode_guard) = if let Some(journal) = &slot.capture {
                let (root, guard) = crate::owned_delete::open_verified_step(
                    parent.as_fd(),
                    &candidate,
                    journal.root.device,
                    journal.root.inode,
                    journal.root.mode,
                )
                .ok()?;
                (root, Some(guard))
            } else {
                (open_directory_at(parent.as_fd(), &candidate).ok()?, None)
            };
            if descriptor_identity(root.as_fd()).ok() != Some((facts.device, facts.inode)) {
                return None;
            }
            if let Some(guard) = mode_guard.as_mut() {
                guard.restore().ok()?;
            }
            crate::owned_delete::observe_entry(parent.as_fd(), &candidate)
                .ok()
                .filter(|after| {
                    after.device == facts.device
                        && after.inode == facts.inode
                        && after.is_directory()
                })
                .map(|_| root)
        })
    }

    /// Reconcile one recorded deletion's journal, deciding only from the
    /// records and the live names. The deletion is complete **only** when the
    /// record says its root was removed; every other state is either rolled
    /// back and retried, or kept pending.
    fn reconcile_slot_capture(
        &mut self,
        id: u64,
        path: &Path,
    ) -> Result<CaptureDisposition, LeaseError> {
        let Some(journal) = self.slot(id).and_then(|slot| slot.capture.clone()) else {
            return Ok(CaptureDisposition::Resume);
        };
        self.checked_slot_scope(path)?;
        let parent_path = path.parent().ok_or(LeaseError::UnknownAccounting)?;
        let parent = super::open_verified_directory(parent_path)?;
        let leaf = path.file_name().ok_or(LeaseError::UnknownAccounting)?;
        let name =
            std::ffi::CString::new(leaf.as_bytes()).map_err(|_| LeaseError::UnknownAccounting)?;
        // A durably recorded root removal is the only positive completion
        // proof. Two absent names without it may be a move, never completion.
        if journal.root.phase == RootCapturePhase::Removed {
            self.record_slot_capture(id, None).map_err(LeaseError::Io)?;
            return Ok(CaptureDisposition::Concluded);
        }
        // A recorded mismatch keeps the deletion pending: replay never removes
        // a name one of those records names.
        let foreign = !journal.foreign.is_empty();
        match reconcile_root_capture(parent.as_fd(), &name, &journal.root) {
            RootCaptureState::Private => {
                // The root is under its recorded private name, so the chain is
                // resolved through that name.
                let private = std::ffi::CString::new(journal.root.private.as_str())
                    .map_err(|_| LeaseError::UnknownAccounting)?;
                let root_facts = crate::owned_delete::observe_entry(parent.as_fd(), &private)
                    .map_err(io_error)?;
                let private_root = root_facts.uid == current_uid()
                    && root_facts.mode & 0o077 == 0
                    && journal.root.mode & 0o077 == 0;
                let concluded =
                    match rollback_captures(parent.as_fd(), &private, &journal, private_root) {
                        Ok(concluded) => concluded,
                        Err(_) => return Ok(CaptureDisposition::Blocked),
                    };
                if !concluded.is_empty() {
                    let mut updated = journal.clone();
                    updated
                        .captures
                        .retain(|record| !concluded.contains(record));
                    self.record_slot_capture(id, Some(updated))
                        .map_err(LeaseError::Io)?;
                }
                if restore_root_capture(parent.as_fd(), &name, &journal.root).is_err() {
                    return Ok(CaptureDisposition::Blocked);
                }
            }
            RootCaptureState::Original => {
                if rollback_captures(parent.as_fd(), &name, &journal, false).is_err() {
                    return Ok(CaptureDisposition::Blocked);
                }
            }
            RootCaptureState::Absent | RootCaptureState::Uncertain { .. } => {
                return Ok(CaptureDisposition::Blocked)
            }
        }
        if foreign {
            return Ok(CaptureDisposition::Blocked);
        }
        self.record_slot_capture(id, None).map_err(LeaseError::Io)?;
        Ok(CaptureDisposition::Resume)
    }

    /// Write one slot's deletion journal durably. The ledger replacement is
    /// atomic and synced, so a crash after this returns leaves exactly this
    /// journal.
    fn record_slot_capture(
        &mut self,
        id: u64,
        capture: Option<DeletionJournal>,
    ) -> Result<(), String> {
        if let Some(journal) = &capture {
            if !journal.is_valid() {
                return Err("the capture journal is not one a writer produces".to_owned());
            }
        }
        {
            let slot = self.slot_mut(id).ok_or("the recorded slot disappeared")?;
            if slot.state != SlotPersistState::Evicting || slot.lease_id.is_some() {
                return Err("a capture journal belongs to a recorded deletion".to_owned());
            }
            if slot.capture == capture {
                return Ok(());
            }
            slot.capture = capture;
        }
        self.persist().map_err(|error| error.to_string())
    }

    /// The recorded slot's path is inside this pool's verified `slots` root.
    fn checked_slot_scope(&self, path: &Path) -> Result<PathBuf, LeaseError> {
        // A volume that is not this pool's verified root has no slots to
        // remove: the path would resolve elsewhere.
        self.backing.verify_volume()?;
        let slots_root = self.backing.payload_root().join(SLOTS_NAME);
        let slots_root = canonical_existing_dir(&slots_root, true)?;
        require_slot_parent(path, &slots_root)?;
        Ok(slots_root)
    }

    /// The bounds the next walk may use: this open's limits, with the
    /// elapsed bound cut to what is left of the measurement's shared
    /// deadline. A measurement's payload and every slot walk therefore share
    /// one cumulative budget; past the deadline a walk keeps a 1 ms bound so
    /// it still stops and reports an elapsed-deadline gap.
    fn walk_limits(&self) -> InventoryLimits {
        let mut limits = self.limits;
        if let Some(deadline) = self.deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            let left_ms = u64::try_from(left.as_millis()).unwrap_or(u64::MAX).max(1);
            limits.max_elapsed_ms = limits.max_elapsed_ms.min(left_ms);
        }
        limits
    }

    /// The payload root's whole tree: `slots` and any tolerated Finder
    /// metadata beside it, one `(device, inode)` union. Two extra levels
    /// (`slots` and a project folder) keep slot contents at the depth bound
    /// they had under `slots`.
    fn payload_accounting(&self) -> Sizes {
        inventory_sizes_with(self.backing.payload_root(), 2, self.walk_limits())
    }

    /// A slot's path and no-follow root metadata, `None` when missing. A
    /// zero ID and any other lookup error refuse; status and the ledger view
    /// share these checks.
    fn slot_root(&self, id: u64) -> Result<(PathBuf, Option<fs::Metadata>), LeaseError> {
        let path = self.slot_path(id)?;
        #[cfg(test)]
        let metadata = crate::test_fs::symlink_metadata(&path);
        #[cfg(not(test))]
        let metadata = fs::symlink_metadata(&path);
        match metadata {
            Ok(metadata) => Ok((path, Some(metadata))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((path, None)),
            Err(error) => Err(io_error(error)),
        }
    }

    fn slot_accounting(&self, id: u64) -> Result<Sizes, LeaseError> {
        let limits = self.walk_limits();
        match self.slot_root(id)? {
            (_, None) => Ok(Sizes {
                logical: Some(0),
                unique: Some(0),
                scan: scan_with(limits, AccountingCoverage::Complete, Vec::new(), 0, 0, 0),
            }),
            (_, Some(metadata)) if metadata.file_type().is_symlink() => Ok(Sizes {
                logical: None,
                unique: None,
                scan: root_failure_with(limits, AccountingGapKind::RootSymlink, 0),
            }),
            (path, Some(_)) => Ok(inventory_sizes_with(&path, 0, limits)),
        }
    }
}

/// Read the config and ledger a passive observer reports, validating their
/// pair. Each file is replaced atomically, so one read may straddle a
/// concurrent replacement: a mismatched pair is re-read once and a second
/// mismatch refuses as unknown instead of being repaired or waited for.
/// Passive generation check for a seed status sidecar. No pool lock,
/// recovery or filesystem walk is performed.
pub(super) fn seed_record_matches(
    root: &Path,
    store_id: &str,
    slot_id: &str,
    incarnation: &str,
    lease_id: &str,
) -> bool {
    let Ok(root) = canonical_existing_dir(root, true) else {
        return false;
    };
    let Ok((config, ledger)) = read_consistent_pair(&root) else {
        return false;
    };
    config.store_id == store_id
        && ledger.slots.iter().any(|slot| {
            slot.id.to_string() == slot_id
                && slot.incarnation.to_string() == incarnation
                && slot.last_lease_id == lease_id
                && matches!(slot.state, SlotPersistState::Idle | SlotPersistState::Held)
                && (slot.state != SlotPersistState::Held
                    || slot.lease_id.as_deref() == Some(lease_id))
        })
}

fn read_consistent_pair(root: &Path) -> Result<(PoolConfig, Ledger), LeaseError> {
    for _ in 0..2 {
        let config = read_config(root)?;
        let ledger = read_ledger(root)?;
        if ledger.version == LEASE_VERSION
            && ledger.store_id == config.store_id
            && ledger.cap_bytes == config.cap_bytes
            && ledger.slots.len() <= MAX_SLOTS
        {
            return Ok((config, ledger));
        }
    }
    Err(LeaseError::BackingMismatch)
}

/// The volume an `apfs-volume-v1` config records, without opening it. The
/// recorded payload must still name one plain directory below that mount.
fn recorded_volume(
    paths: &BackingPaths,
    config: &PoolConfig,
) -> Result<RecordedVolume, LeaseError> {
    let Some(volume) = config.volume.as_ref() else {
        return Err(LeaseError::BackingMismatch);
    };
    if config.version != LEASE_VERSION || config.cap_bytes == 0 || config.image_bytes != 0 {
        return Err(LeaseError::BackingMismatch);
    }
    if paths.image.symlink_metadata().is_ok()
        || paths.receipt.symlink_metadata().is_ok()
        || paths.mount.symlink_metadata().is_ok()
    {
        return Err(LeaseError::BackingMismatch);
    }
    let name = volume.payload_dir.as_str();
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(LeaseError::BackingMismatch);
    }
    Ok(RecordedVolume {
        mount_point: volume.mount_point.clone(),
        volume_uuid: volume.volume_uuid.clone(),
        payload_dir: volume.payload_dir.clone(),
    })
}

/// The recorded snapshot of a validated config/ledger pair.
fn snapshot_of(
    config: &PoolConfig,
    ledger: &Ledger,
    payload: PathBuf,
    volume: Option<RecordedVolume>,
) -> PassiveSnapshot {
    let recorded: Vec<&RecordedSize> = ledger
        .slots
        .iter()
        .filter(|slot| slot.state == SlotPersistState::Idle && slot.lease_id.is_none())
        .filter_map(|slot| {
            slot.recorded
                .as_ref()
                .filter(|record| record.for_lease == slot.last_lease_id)
        })
        .collect();
    let idle = ledger
        .slots
        .iter()
        .filter(|slot| slot.state == SlotPersistState::Idle && slot.lease_id.is_none())
        .count();
    let recorded_total = if recorded.is_empty() {
        None
    } else {
        Some(
            recorded
                .iter()
                .fold(0u64, |sum, record| sum.saturating_add(record.unique_bytes)),
        )
    };
    PassiveSnapshot {
        store_id: config.store_id.clone(),
        backend: config.backend.clone(),
        cap_bytes: config.cap_bytes,
        payload,
        volume,
        recorded_slots: recorded.len(),
        unrecorded_slots: idle.saturating_sub(recorded.len()),
        recorded_total,
        holding: ledger
            .holding
            .iter()
            .map(|row| holding_entry(row, None))
            .collect(),
        slots: ledger.slots.iter().map(ledger_slot).collect(),
        evicting_slot_ids: ledger
            .slots
            .iter()
            .filter(|slot| slot.state == SlotPersistState::Evicting)
            .map(|slot| slot.id.to_string())
            .collect(),
    }
}

/// The walks of one passive measurement. The caller's remaining elapsed
/// bound becomes one deadline that the payload walk and every slot walk
/// recompute against, so the whole measurement stays within it. No lock,
/// reconciliation, creation or write.
///
/// A bounded reread afterwards keeps the pairing honest: a concurrent grant,
/// release or eviction between the read and the walks makes coverage unknown
/// rather than pairing a walk with a different slot generation. Nothing is
/// repaired or waited for.
fn measure_within(pool: &Path, max_elapsed_ms: u64) -> Result<MeasuredPool, LeaseError> {
    let opened = Pool::open_passive(pool, max_elapsed_ms)?;
    let snapshot = opened.recorded();
    let mut status = opened.accounted()?;
    let stable = match read_consistent_pair(&opened.root) {
        Ok((config, ledger)) => same_generations(&snapshot, &config, &ledger),
        Err(_) => false,
    };
    if !stable {
        status.coverage = AccountingCoverage::Unknown;
        status.full = None;
    }
    Ok(MeasuredPool { snapshot, status })
}

/// Whether a reread still names exactly the generations this measurement
/// read: same store, backend, cap, and the same slot IDs, incarnations,
/// recorded directories and occupancy.
fn same_generations(snapshot: &PassiveSnapshot, config: &PoolConfig, ledger: &Ledger) -> bool {
    snapshot.store_id == config.store_id
        && snapshot.backend == config.backend
        && snapshot.cap_bytes == config.cap_bytes
        && snapshot.slots.len() == ledger.slots.len()
        && snapshot.holding.len() == ledger.holding.len()
        && snapshot
            .holding
            .iter()
            .zip(ledger.holding.iter())
            .all(|(shown, stored)| {
                shown.name == stored.name && shown.incarnation == stored.incarnation.to_string()
            })
        && snapshot
            .slots
            .iter()
            .zip(ledger.slots.iter())
            .all(|(shown, stored)| {
                shown.slot_id == stored.id.to_string()
                    && shown.incarnation == stored.incarnation.to_string()
                    && shown.dir == stored.dir
                    && shown.state == occupancy(stored.state)
            })
}

/// The budget-only payload is a plain owner-only directory holding `slots`.
/// The budget is not OS-enforced. Image and receipt files refuse, so a legacy
/// APFS image pool is never reinterpreted as a directory.
fn open_budget_payload(
    root: &Path,
    paths: &BackingPaths,
    config: &PoolConfig,
) -> Result<Payload, LeaseError> {
    if config.version != LEASE_VERSION || config.cap_bytes == 0 || config.image_bytes != 0 {
        return Err(LeaseError::BackingMismatch);
    }
    if paths.image.symlink_metadata().is_ok() || paths.receipt.symlink_metadata().is_ok() {
        return Err(LeaseError::BackingMismatch);
    }
    let payload =
        canonical_existing_dir(&paths.mount, true).map_err(|_| LeaseError::BackingMismatch)?;
    let root_device = fs::symlink_metadata(root).map_err(io_error)?.dev();
    let payload_device = fs::symlink_metadata(&payload).map_err(io_error)?.dev();
    if payload != paths.mount || payload_device != root_device {
        return Err(LeaseError::BackingMismatch);
    }
    validate_lease_payload_entries(&payload, &[])?;
    Ok(Payload::Budget(payload))
}

/// The removable payload: the recorded directory on the mount root of the
/// recorded volume, opened through the verified root's descriptor and
/// matching its path. Image, receipt or an internal `payload` refuse.
fn open_volume_payload(paths: &BackingPaths, config: &PoolConfig) -> Result<Payload, LeaseError> {
    let Some(volume) = config.volume.as_ref() else {
        return Err(LeaseError::BackingMismatch);
    };
    if config.version != LEASE_VERSION || config.cap_bytes == 0 || config.image_bytes != 0 {
        return Err(LeaseError::BackingMismatch);
    }
    if paths.image.symlink_metadata().is_ok()
        || paths.receipt.symlink_metadata().is_ok()
        || paths.mount.symlink_metadata().is_ok()
    {
        return Err(LeaseError::BackingMismatch);
    }
    let binding = Binding::open(&volume.mount_point, Some(&volume.volume_uuid))
        .map_err(|_| LeaseError::BackingMismatch)?;
    let fd = binding
        .dir_at(binding.fd(), &volume.payload_dir, false)
        .map_err(|_| LeaseError::BackingMismatch)?;
    let root = volume.mount_point.join(&volume.payload_dir);
    let payload = canonical_existing_dir(&root, true).map_err(|_| LeaseError::BackingMismatch)?;
    let by_path = fs::symlink_metadata(&payload).map_err(io_error)?;
    let by_fd = fs::File::from(fd.try_clone().map_err(io_error)?)
        .metadata()
        .map_err(io_error)?;
    if payload != root || by_path.dev() != by_fd.dev() || by_path.ino() != by_fd.ino() {
        return Err(LeaseError::BackingMismatch);
    }
    validate_lease_payload_entries(&payload, &[])?;
    Ok(Payload::Volume {
        binding,
        root: payload,
        fd,
    })
}

fn validate_lease_payload_entries(
    payload_root: &Path,
    metadata_dirs: &[&str],
) -> Result<(), LeaseError> {
    let root_metadata = fs::symlink_metadata(payload_root).map_err(io_error)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.file_type().is_dir() {
        return Err(LeaseError::UnknownAccounting);
    }
    let root_device = root_metadata.dev();
    let mut found_slots = false;
    for entry in fs::read_dir(payload_root).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        let refuse = || unexpected_entry(PAYLOAD_PLACE, &name);
        if is_finder_metadata(&name) {
            // Counted by payload accounting, which walks the payload root.
            let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
            if !is_tolerated_metadata_file(&metadata, root_device) {
                return Err(refuse());
            }
            continue;
        }
        let text = name.to_str().ok_or_else(refuse)?;
        let is_slots = text == SLOTS_NAME;
        // The private holding area (oxide#190) is a second owner-only control
        // directory beside `slots`, on the payload's own device. Its own
        // entries are judged when they are recorded and deleted, never here.
        let is_holding = text == HOLDING_NAME;
        if !is_slots && !is_holding && !metadata_dirs.contains(&text) {
            return Err(refuse());
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
        if metadata.file_type().is_symlink()
            || !metadata.file_type().is_dir()
            || metadata.dev() != root_device
        {
            return Err(refuse());
        }
        if is_slots || is_holding {
            if metadata.uid() != current_uid() || metadata.permissions().mode() & 0o077 != 0 {
                return Err(LeaseError::UnsafePath(
                    "slots root must be a private caller-owned directory",
                ));
            }
            found_slots |= is_slots;
        }
    }
    if !found_slots {
        return Err(LeaseError::UnknownAccounting);
    }
    canonical_existing_dir(&payload_root.join(SLOTS_NAME), true)?;
    Ok(())
}

/// Deletion is limited to recorded owned slot paths: directly under the
/// slots root (legacy), or under one project folder that is a real
/// owner-only directory of the caller on the slots root's device.
/// A recorded private/original name as a C string.
fn recorded_name(name: &str) -> Result<std::ffi::CString, String> {
    std::ffi::CString::new(name).map_err(|_| "a recorded capture name holds a NUL".to_owned())
}

fn recorded_type(record: &CaptureRecord) -> libc::mode_t {
    (record.mode & u32::from(libc::S_IFMT)) as libc::mode_t
}

/// Whether `name` in `base` still holds exactly `(device, inode)`, with the
/// caller's refusal text when it does not. Used to stop a step whose name was
/// rebound after its descriptor was opened.
fn verify_step_name(
    base: BorrowedFd<'_>,
    name: &std::ffi::CStr,
    device: u64,
    inode: u64,
    refusal: &str,
) -> Result<(), String> {
    match descriptor_identity_of_name(base, name) {
        Ok(identity) if identity == (device, inode) => Ok(()),
        Ok(_) => Err(refusal.to_owned()),
        Err(error) => Err(format!("{refusal} ({error})")),
    }
}

/// The `(device, inode)` of one name in a held parent, never following a link.
fn descriptor_identity_of_name(
    base: BorrowedFd<'_>,
    name: &std::ffi::CStr,
) -> Result<(u64, u64), String> {
    let facts = crate::owned_delete::observe_entry(base, name)
        .map_err(|error| format!("the name's identity cannot be read ({error})"))?;
    Ok((facts.device, facts.inode))
}

/// One recorded capture must still be the object the record describes.
fn verify_recorded(
    base: BorrowedFd<'_>,
    private: &std::ffi::CStr,
    record: &CaptureRecord,
) -> Result<(), String> {
    let facts = crate::owned_delete::observe_entry(base, private)
        .map_err(|error| format!("a recorded capture cannot be read ({error})"))?;
    if facts.device != record.device
        || facts.inode != record.inode
        || facts.file_type() != recorded_type(record)
    {
        return Err("a recorded capture is not the object it recorded".to_owned());
    }
    Ok(())
}

/// Roll back every recorded capture inside one held tree root: each recorded
/// object is put back under its observed name, deepest first, so every put-back
/// sees a live parent. A captured directory's own recorded mode is restored
/// before it is put back, and never before its contents are resolved (a
/// search-only mode would make that rollback impossible).
fn rollback_captures(
    parent: BorrowedFd<'_>,
    root_name: &std::ffi::CStr,
    journal: &DeletionJournal,
    private_root: bool,
) -> Result<Vec<CaptureRecord>, String> {
    // The captured root may carry the owner's search-only mode. One descriptor,
    // opened without following a link and verified against the record, carries
    // both the rollback and the mode restoration, so a rebound name can never
    // redirect the chmod.
    let (root, mut widened) = crate::owned_delete::open_verified_step(
        parent,
        root_name,
        journal.root.device,
        journal.root.inode,
        journal.root.mode,
    )?;
    let mut concluded = Vec::new();
    let outcome = (|| {
        verify_step_name(
            parent,
            root_name,
            journal.root.device,
            journal.root.inode,
            "the held tree root is not the recorded root",
        )?;
        let mut resolved = 0;
        restore_captures_in(
            root.as_fd(),
            &[],
            &journal.captures,
            &mut resolved,
            private_root,
            &mut concluded,
        )?;
        if resolved != journal.captures.len() {
            // A record the walk could not locate is a representation this replay
            // cannot resolve: fail closed rather than completing past it.
            return Err(format!(
                "the journal records {} captures but the rollback resolved {resolved}",
                journal.captures.len()
            ));
        }
        Ok(())
    })();
    // The root stays under its private name here (its put-back is later), so its
    // recorded mode is restored now, and on any error before this point too.
    outcome.and(widened.restore())?;
    Ok(concluded)
}

fn restore_captures_in(
    base: BorrowedFd<'_>,
    chain: &[String],
    records: &[CaptureRecord],
    resolved: &mut usize,
    private_root: bool,
    concluded: &mut Vec<CaptureRecord>,
) -> Result<(), String> {
    let here: Vec<&CaptureRecord> = records
        .iter()
        .filter(|record| record.path == chain)
        .collect();
    for record in here.iter().filter(|record| record.is_directory()) {
        let private = recorded_name(&record.private)?;
        if crate::owned_delete::observe_entry(base, &private).is_err() {
            // The capture was already put back before the crash. The observed
            // name must hold exactly the recorded object — and its own recorded
            // children are then resolved *through* that verified directory, so
            // a put-back parent never strands its child captures.
            restore_recorded(base, record)?;
            *resolved += 1;
            let mut inner = chain.to_vec();
            inner.push(record.private.clone());
            if !records.iter().any(|child| child.path == inner) {
                // No recorded child needs descent, so the restored parent is
                // not opened at all.
                continue;
            }
            let original = recorded_name(&record.original)?;
            // The restored parent may carry the owner's search-only mode. One
            // descriptor, opened without following a link and verified against
            // the record, carries both the descent and the mode restoration, so
            // a name rebound after the widening can never redirect a chmod and
            // the original still gets its recorded mode back.
            let (step, mut widened) = crate::owned_delete::open_verified_step(
                base,
                &original,
                record.device,
                record.inode,
                record.mode,
            )?;
            #[cfg(test)]
            crate::test_fs::run_after_restored_parent_widen();
            let stepped = (|| {
                // The name must still hold the recorded object before anything
                // is read through the held descriptor; a rebind stops the step
                // without modifying the replacement.
                verify_recorded(base, &original, record)?;
                restore_captures_in(
                    step.as_fd(),
                    &inner,
                    records,
                    resolved,
                    private_root,
                    concluded,
                )
            })();
            match stepped {
                Ok(()) => {
                    widened.restore()?;
                }
                Err(reason) => {
                    return match widened.restore() {
                        Ok(()) => Err(reason),
                        Err(restore_reason) => Err(format!(
                            "{reason}; the restored directory's recorded mode could not be restored ({restore_reason})"
                        )),
                    }
                }
            }
            continue;
        }
        verify_recorded(base, &private, record)?;
        // Read access for this replay step comes from one verified descriptor,
        // and the recorded mode is restored through that same descriptor on
        // every exit path (including a panic).
        let (step, mut widened) = crate::owned_delete::open_verified_step(
            base,
            &private,
            record.device,
            record.inode,
            record.mode,
        )?;
        let stepped = (|| {
            let mut inner = chain.to_vec();
            inner.push(record.private.clone());
            restore_captures_in(
                step.as_fd(),
                &inner,
                records,
                resolved,
                private_root,
                concluded,
            )?;
            restore_recorded(base, record)?;
            Ok(())
        })();
        match stepped {
            Ok(()) => *resolved += 1,
            Err(reason) => {
                return match widened.restore() {
                    Ok(()) => Err(reason),
                    Err(restore_reason) => Err(format!(
                        "{reason}; the captured directory's recorded mode could not be restored ({restore_reason})"
                    )),
                }
            }
        }
    }
    for record in here.iter().filter(|record| !record.is_directory()) {
        // A journaled regular file may have been unlinked just before its
        // record was dropped. Only the verified private root and its verified
        // ancestor chain give this absence that meaning; errors other than
        // NotFound, links, and names in a restored root remain uncertain.
        let absent = |name: &str| -> Result<bool, String> {
            match crate::owned_delete::observe_entry(base, &recorded_name(name)?) {
                Ok(_) => Ok(false),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
                Err(error) => Err(format!("a recorded capture cannot be inspected ({error})")),
            }
        };
        if private_root
            && recorded_type(record) == libc::S_IFREG
            && absent(&record.private)?
            && absent(&record.original)?
        {
            concluded.push((*record).clone());
            *resolved += 1;
            continue;
        }
        restore_recorded(base, record)?;
        *resolved += 1;
    }
    Ok(())
}

/// Put one recorded capture back under its observed name, restoring a captured
/// directory's recorded mode first. Nothing is ever unlinked here.
///
/// A capture whose private name is already gone but whose **observed** name
/// holds exactly the recorded object was put back before the crash
/// (`abort_capture` does that whenever a later step refuses): that is released,
/// not treated as uncertainty. Any other absence is uncertain.
fn restore_recorded(base: BorrowedFd<'_>, record: &CaptureRecord) -> Result<(), String> {
    let private = recorded_name(&record.private)?;
    if crate::owned_delete::observe_entry(base, &private).is_err() {
        let original = recorded_name(&record.original)?;
        return match crate::owned_delete::observe_entry(base, &original) {
            Ok(facts)
                if facts.device == record.device
                    && facts.inode == record.inode
                    && facts.file_type() == recorded_type(record) =>
            {
                Ok(())
            }
            _ => {
                Err("a recorded capture is neither under its private name nor put back".to_owned())
            }
        };
    }
    verify_recorded(base, &private, record)?;
    if record.is_directory() {
        restore_captured_mode(base, &private, record.device, record.inode, record.mode)?;
    }
    let original = recorded_name(&record.original)?;
    put_back(base, &Capture { private, original })
        .map_err(|error| format!("a recorded capture cannot be put back ({error})"))
}

fn require_slot_parent(path: &Path, slots_root: &Path) -> Result<(), LeaseError> {
    let refuse = || LeaseError::UnsafePath("deletion is limited to recorded owned slot paths");
    let parent = path.parent().ok_or_else(refuse)?;
    if parent == slots_root {
        return Ok(());
    }
    if parent.parent() != Some(slots_root) {
        return Err(refuse());
    }
    let folder = canonical_existing_dir(parent, true)?;
    if folder != parent {
        return Err(refuse());
    }
    let root_device = fs::symlink_metadata(slots_root).map_err(io_error)?.dev();
    if fs::symlink_metadata(parent).map_err(io_error)?.dev() != root_device {
        return Err(LeaseError::UnknownAccounting);
    }
    Ok(())
}

/// What reconciling one recorded deletion's journal settled.
enum CaptureDisposition {
    /// The recorded path may be deleted now.
    Resume,
    /// The recorded root's own removal completed: nothing of the slot remains.
    Concluded,
    /// The names, the records and the objects do not agree well enough to act.
    /// Every record and object is kept and the pool keeps its refusal.
    Blocked,
}

/// Whether a project folder with no recorded slot holds anything: missing
/// is free, an empty owned directory (Finder metadata aside) is free, and
/// anything else is occupied.
fn folder_occupied(folder: &Path) -> Result<bool, LeaseError> {
    let metadata = match fs::symlink_metadata(folder) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_error(error)),
        Ok(metadata) => metadata,
    };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.uid() != current_uid()
    {
        return Ok(true);
    }
    for entry in fs::read_dir(folder).map_err(io_error)? {
        if !is_finder_metadata(&entry.map_err(io_error)?.file_name()) {
            return Ok(true);
        }
    }
    Ok(false)
}

struct Sizes {
    logical: Option<u64>,
    unique: Option<u64>,
    scan: AccountingScan,
}

impl Sizes {
    /// Unique allocated bytes of a complete walk. A partial, late or
    /// unknown walk refuses with its scan; its bytes are never used.
    fn require_complete(&self) -> Result<u64, LeaseError> {
        match (self.scan.coverage, self.unique) {
            (AccountingCoverage::Complete, Some(unique)) => Ok(unique),
            _ => Err(LeaseError::AccountingIncomplete(Box::new(
                self.scan.clone(),
            ))),
        }
    }
}

/// The smallest charge one unmeasured slot carries (oxide#186). A partial walk
/// is never free space, so the charge is positive even when the slot holds
/// nothing observable yet and no peer has a record; it distinguishes "not
/// measured" from "measured empty".
const UNMEASURED_MIN_CHARGE_BYTES: u64 = 1;

/// A removable payload walk longer than this is cut short, so a pool too large
/// to measure reaches the unknown-size reserve/floor route instead of timing
/// out mid-walk: a 127-slot USB pool cannot be walked within the 60 s bound.
const VOLUME_WALK_MS: u64 = 5_000;

/// Is this scan incomplete for the walk's own bounds only?
///
/// `ElapsedDeadline` and `EntryLimit` are the two gaps that say "the pool is
/// bigger than the walk could see", which is the only incomplete shape that
/// may take the reserve route. A complete scan, an empty gap list, or any
/// unreadable / missing / unsupported / mount-boundary gap is a real
/// accounting failure and refuses unchanged.
fn bound_gaps_only(scan: &AccountingScan) -> bool {
    scan.coverage == AccountingCoverage::Partial
        && !scan.gaps.is_empty()
        && scan.gaps.iter().all(|gap| {
            matches!(
                gap.kind,
                AccountingGapKind::ElapsedDeadline | AccountingGapKind::EntryLimit
            )
        })
}

/// The hard floor for an unknown-size admission with nothing left to prune.
///
/// Tom's 2026-10-25 refinement: the registered reserve is a *prune* trigger,
/// not a stop; only this floor is a stop. It is a fixed product constant for
/// oxide#146 — not a registration field, a watchdog setting or a selection
/// change.
const UNKNOWN_REMOVABLE_FLOOR_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// What to do about a removable pool whose own size is unknown.
#[derive(Clone, Debug, Eq, PartialEq)]
enum UnknownRemovableStep {
    /// Free space is above the reserve: admit, evict nothing.
    Admit(String),
    /// Free space is at or below the reserve and some idle whole slot may be
    /// pruned: retire the least-recently-used eligible one, then re-read.
    Evict,
    /// Free space is below the reserve, nothing is left to prune, and it is at
    /// or above the hard floor: admit with a warning and no further eviction.
    AdmitAtFloor(String),
    /// Free space is below the reserve, nothing is left to prune, and it is
    /// below the hard floor: refuse with the original incomplete scan.
    Refuse(AccountingScan),
}

/// Decide one step of a removable pool's unknown-size admission.
///
/// The pool figure is genuinely unknown here, so it is never treated as zero,
/// never compared with the pool cap, and never replaced by the volume's total
/// usage: only positively measured free space, the operator's reserve and the
/// fixed hard floor participate. `available` and `reserve` must both be known;
/// otherwise the honest answer is the original incomplete scan.
fn unknown_removable_step(
    scan: &AccountingScan,
    available: Option<u64>,
    reserve: Option<u64>,
    floor: u64,
    idle_available: bool,
) -> Result<UnknownRemovableStep, LeaseError> {
    if !bound_gaps_only(scan) {
        return Err(LeaseError::AccountingIncomplete(Box::new(scan.clone())));
    }
    let (Some(available), Some(reserve)) = (available, reserve) else {
        return Err(LeaseError::AccountingIncomplete(Box::new(scan.clone())));
    };
    // Recovering space comes first: while any eligible idle whole slot
    // remains, free space at or below the reserve — and any free space below
    // the hard floor, including when a legacy reserve sits lower than the
    // floor — prunes one slot and re-reads. The floor decides only once
    // nothing is left to prune.
    if available > reserve && available >= floor {
        return Ok(UnknownRemovableStep::Admit(format!(
            "pool size unknown (the walk stopped at its bound after {} entries); admitted on {} free above the {} reserve; nothing was evicted and the pool cap is unverified",
            scan.entries_visited,
            gigabytes(available),
            gigabytes(reserve)
        )));
    }
    if idle_available {
        return Ok(UnknownRemovableStep::Evict);
    }
    if available >= floor {
        return Ok(UnknownRemovableStep::AdmitAtFloor(format!(
            "pool size unknown (the walk stopped at its bound after {} entries); {} free is at or below the {} reserve, at or above the {} hard floor, and no idle slot is left to prune, so this admission proceeds with a warning; nothing was evicted and the pool cap is unverified",
            scan.entries_visited,
            gigabytes(available),
            gigabytes(reserve),
            gigabytes(floor)
        )));
    }
    Ok(UnknownRemovableStep::Refuse(scan.clone()))
}

/// Binary gibibytes with one place, for admission notes.
///
/// The reserve and the hard floor are both binary quantities
/// (`10 * 1024^3`), so their notes must not reprint them as decimal GB.
fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

/// One recorded holding row, as status and the ledger view show it.
fn holding_entry(record: &HoldingRecord, present: Option<bool>) -> super::HoldingEntry {
    super::HoldingEntry {
        slot_id: record.slot.to_string(),
        incarnation: record.incarnation.to_string(),
        name: record.name.clone(),
        recorded_bytes: record.bytes,
        at_ms: record.at_ms,
        present,
    }
}

/// One pending deletion claimed for the caller: the holding entry's own
/// descriptor with an exclusive, non-blocking custody lock on it, taken under
/// a brief pool lock and held while the tree is deleted outside it
/// (oxide#190).
pub(super) struct HoldingClaim {
    name: String,
    tree: OwnedFd,
    /// The holding area's own descriptor, for removing the entry directory
    /// itself once its contents are gone.
    area: OwnedFd,
    identity: (u64, u64),
    trusted: bool,
}

/// What one deletion pass did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HoldingDrain {
    /// The entry this pass claimed, if any.
    pub claimed: Option<String>,
    /// Whether the holding directory is gone, so its record was dropped.
    pub gone: bool,
    /// Entries removed from the tree by this pass.
    pub entries: u64,
    /// Logical bytes the removed entries held.
    pub bytes: u64,
    /// Bounded diagnostics for entries this pass refused to delete. An
    /// undelible item stays in the holding area, still charged, and never
    /// blocks admission or another entry.
    pub refused: Vec<String>,
}

impl HoldingDrain {
    fn nothing() -> Self {
        Self::default()
    }
}

/// The bounds one async deletion uses. They match the accounting bounds: a
/// holding tree is one slot's payload, and a pass that reaches them leaves the
/// entry whole and charged for the next command.
const HOLDING_DELETE_MAX_ENTRIES: u64 = 2_000_000;
const HOLDING_DELETE_MAX_ELAPSED_MS: u64 = 120_000;
/// Most names one directory listing batch holds, so a pass keeps bounded
/// memory even for a directory with a huge number of entries.
const HOLDING_LIST_BATCH: usize = 4096;
/// Most per-object refusals one pass reports.
const HOLDING_MAX_REFUSALS: usize = 8;

impl Pool {
    fn holding_root(&self) -> PathBuf {
        self.backing.payload_root().join(HOLDING_NAME)
    }

    /// The bytes every recorded holding entry is charged until it is gone.
    fn holding_bytes(&self) -> u64 {
        self.ledger
            .holding
            .iter()
            .fold(0u64, |sum, row| sum.saturating_add(row.bytes))
    }

    /// The recorded holding entries, each with a single presence check of its
    /// own directory (an `lstat`, never a walk).
    fn holding_entries(&self) -> Vec<super::HoldingEntry> {
        let root = self.holding_root();
        self.ledger
            .holding
            .iter()
            .map(|row| holding_entry(row, holding_presence(&root.join(&row.name))))
            .collect()
    }

    /// The holding entry name this generation derives: store, slot,
    /// incarnation and the generation's own lease ID. Every part is durable
    /// ledger state, so a crash between the rename and the one durable write
    /// leaves an entry the next open can adopt without a second sync.
    fn holding_name_for(&self, id: u64, incarnation: u64, last_lease_id: &str) -> Option<String> {
        derived_holding_name(&self.ledger.store_id, id, incarnation, last_lease_id)
    }

    /// The private holding area's descriptor, created owner-only on first use.
    /// It must be a real directory of the caller on the payload's own device:
    /// the rename is same-device by construction, so a different device is a
    /// refusal, never a copy.
    fn open_holding_area(&self, create: bool) -> Result<Option<OwnedFd>, LeaseError> {
        let root = self.holding_root();
        let payload = fs::symlink_metadata(self.backing.payload_root()).map_err(io_error)?;
        if create {
            match fs::symlink_metadata(&root) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                        return Err(LeaseError::UnsafePath(
                            "the holding area must be a private owned directory",
                        ));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let mut builder = fs::DirBuilder::new();
                    builder.mode(0o700);
                    match builder.create(&root) {
                        Ok(()) => {}
                        // Another command created it between the probe and the
                        // create; its own ownership is checked below.
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(io_error(error)),
                    }
                }
                Err(error) => return Err(io_error(error)),
            }
        }
        let held = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&root)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
        };
        let metadata = held.metadata().map_err(io_error)?;
        if !metadata.is_dir()
            || metadata.uid() != current_uid()
            || metadata.permissions().mode() & 0o077 != 0
            || metadata.dev() != payload.dev()
        {
            return Err(LeaseError::UnsafePath(
                "the holding area must be a private owned directory",
            ));
        }
        Ok(Some(OwnedFd::from(held)))
    }

    /// Move one verified slot tree whole into the holding area and return its
    /// record. The caller writes the record and drops the slot row in one
    /// durable write. Nothing here walks the tree: one identity-verified rename.
    ///
    /// `expected` is the root identity the caller's own preflight verified (the
    /// idle-scope check, or a legacy deletion's completeness preflight). The
    /// root is opened by name without following a link, its descriptor identity
    /// is compared against `expected` **before** the rename, and the moved
    /// entry is compared against the same identity afterwards. A root replaced
    /// in the window between the preflight and the rename therefore refuses:
    /// nothing is held and nothing is later deleted.
    fn hold_slot_tree(
        &mut self,
        id: u64,
        path: &Path,
        expected: Option<(u64, u64)>,
    ) -> Result<HoldingRecord, LeaseError> {
        if self.ledger.holding.len() >= MAX_HOLDING_ENTRIES {
            return Err(LeaseError::UnknownAccounting);
        }
        let slot = self.slot(id).ok_or(LeaseError::UnknownAccounting)?;
        let incarnation = slot.incarnation;
        let name = self
            .holding_name_for(id, incarnation, &slot.last_lease_id)
            .ok_or(LeaseError::UnknownAccounting)?;
        // The closed observation this generation holds, or the pool's largest
        // recorded slot when it has none: the charge is never zero.
        let recorded = slot
            .recorded
            .as_ref()
            .filter(|record| record.for_lease == slot.last_lease_id)
            .map(|record| record.unique_bytes);
        let bytes = recorded
            .unwrap_or_else(|| self.largest_recorded_slot())
            .max(UNMEASURED_MIN_CHARGE_BYTES);
        // The slot root must still be the real, owned, owner-only directory
        // the ledger records, on the slots root's own device.
        let slots_root = self.backing.payload_root().join(SLOTS_NAME);
        let slots_root = canonical_existing_dir(&slots_root, true)?;
        require_slot_parent(path, &slots_root)?;
        let parent_path = path.parent().ok_or(LeaseError::UnknownAccounting)?;
        let parent = open_verified_directory(parent_path)?;
        let source = std::ffi::CString::new(
            path.file_name()
                .ok_or(LeaseError::UnknownAccounting)?
                .as_bytes(),
        )
        .map_err(|_| LeaseError::UnknownAccounting)?;
        let tree =
            crate::owned_delete::open_directory_at(parent.as_fd(), &source).map_err(io_error)?;
        let identity = descriptor_identity(tree.as_fd()).map_err(io_error)?;
        let observed =
            crate::owned_delete::observe_entry(parent.as_fd(), &source).map_err(io_error)?;
        if !observed.is_directory()
            || observed.uid != current_uid()
            || observed.mode_bits() & 0o077 != 0
            || (observed.device, observed.inode) != identity
        {
            return Err(LeaseError::UnknownAccounting);
        }
        if expected.is_some_and(|expected| expected != identity) {
            return Err(LeaseError::UnsafePath(
                "the verified slot root was replaced before it was moved",
            ));
        }
        let holding = self
            .open_holding_area(true)?
            .ok_or(LeaseError::UnknownAccounting)?;
        let holding_metadata = fs::symlink_metadata(self.holding_root()).map_err(io_error)?;
        if holding_metadata.dev() != identity.0 {
            return Err(LeaseError::UnsafePath(
                "the holding area is not on the slot's own device",
            ));
        }
        let target =
            std::ffi::CString::new(name.as_bytes()).map_err(|_| LeaseError::UnknownAccounting)?;
        // Nothing may exist under the derived name: a leftover from an earlier
        // attempt is adopted by the recovery pass, never overwritten here.
        match crate::owned_delete::observe_entry(holding.as_fd(), &target) {
            Ok(_) => return Err(LeaseError::UnknownAccounting),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
        #[cfg(test)]
        crate::test_fs::run_before_holding_rename();
        // One rename moves the whole tree; nothing is read, copied or
        // journaled, and the name is the only path the deleter needs.
        // SAFETY: both descriptors and both C strings are live for the call.
        let renamed = unsafe {
            libc::renameat(
                parent.as_raw_fd(),
                source.as_ptr(),
                holding.as_raw_fd(),
                target.as_ptr(),
            )
        };
        if renamed != 0 {
            return Err(io_error(std::io::Error::last_os_error()));
        }
        // The moved entry must be the very object whose descriptor was
        // verified. A swap in the rename window is detected here, and the
        // foreign tree that took its place is **contained**: it is moved to a
        // name no generation can ever derive, so no later recovery pass can
        // adopt it, charge it as a slot's tree or delete it. The payload walk
        // keeps charging those bytes, and this command refuses.
        let moved =
            crate::owned_delete::observe_entry(holding.as_fd(), &target).map_err(io_error)?;
        if (moved.device, moved.inode) != identity {
            let contained = std::ffi::CString::new(unverified_holding_name(&name))
                .map_err(|_| LeaseError::UnknownAccounting)?;
            // SAFETY: both names are inside the held holding-area descriptor.
            unsafe {
                libc::renameat(
                    holding.as_raw_fd(),
                    target.as_ptr(),
                    holding.as_raw_fd(),
                    contained.as_ptr(),
                );
            }
            return Err(LeaseError::UnsafePath(
                "the held tree is not the slot that was verified",
            ));
        }
        #[cfg(test)]
        crate::test_fs::run_after_holding_rename();
        Ok(HoldingRecord {
            name,
            slot: id,
            incarnation,
            device: identity.0,
            inode: identity.1,
            bytes,
            at_ms: now_ms(),
            attempted_ms: None,
        })
    }

    /// Bind a deletion's rename durably **before** it happens: the holding entry
    /// name and the root identity the caller's own scope or completeness
    /// preflight verified. Both rename sources use it — the ordinary idle
    /// eviction and the legacy pending deletion — so recovery adopts an
    /// interrupted rename only when the entry is that exact object and never
    /// because a directory merely carries the right name.
    fn record_hold_intent(&mut self, id: u64, expected: (u64, u64)) -> Result<(), LeaseError> {
        let slot = self.slot(id).ok_or(LeaseError::UnknownAccounting)?;
        if !matches!(
            slot.state,
            SlotPersistState::Idle | SlotPersistState::Evicting
        ) {
            return Err(LeaseError::UnknownAccounting);
        }
        let name = self
            .holding_name_for(slot.id, slot.incarnation, &slot.last_lease_id)
            .ok_or(LeaseError::UnknownAccounting)?;
        let intent = HoldingIntent {
            name,
            device: expected.0,
            inode: expected.1,
        };
        let slot = self.slot_mut(id).ok_or(LeaseError::UnknownAccounting)?;
        if slot.hold_intent.as_ref() == Some(&intent) {
            return Ok(());
        }
        slot.hold_intent = Some(intent);
        self.persist_revised()
    }

    /// The holding entry name a legacy pending deletion already bound durably,
    /// when it matches this generation's own derivation.
    fn recorded_hold_intent<'a>(&self, slot: &'a SlotRecord) -> Option<&'a HoldingIntent> {
        let intent = slot.hold_intent.as_ref()?;
        let derived = derived_holding_name(
            &self.ledger.store_id,
            slot.id,
            slot.incarnation,
            &slot.last_lease_id,
        )?;
        (intent.name == derived && intent.is_valid()).then_some(intent)
    }

    /// Adopt a holding entry a crash left before its row was durable, and
    /// finish a recorded row whose tree is positively gone.
    ///
    /// Two interruptions land here. The idle-eviction rename writes its row in
    /// the following durable replacement, and a **legacy** deletion migrated by
    /// `reconcile_evicting` renames the resolved root and then writes the row:
    /// in both windows the slot row (idle or `Evicting`) is still recorded, its
    /// root is gone, and the tree sits under the name that generation derives
    /// from its own durable facts. Adoption is therefore not limited to idle
    /// rows: a legacy `Evicting` row interrupted after its migration rename is
    /// adopted exactly as an idle one, so the window is recoverable in one
    /// write instead of staying pending forever. A tree that is still at its
    /// recorded root never adopts anything.
    ///
    /// Only a positively classified `NotFound` means a recorded entry is gone.
    /// Any other `lstat` error (`EACCES`, `EIO`, an over-bound name) is unknown,
    /// not absence: the row and its charge are kept, so a tree that is still
    /// there is never reported as deleted and never stops being accounted.
    fn reconcile_holding(&mut self) -> Result<(), LeaseError> {
        let mut changed = false;
        // An intent whose root is still at its recorded path names a rename that
        // never happened (the command refused or crashed before it). It is
        // stale: the generation is an ordinary slot again, so the intent is
        // cleared before anything else, and this needs no holding area.
        let stale: Vec<u64> = self
            .ledger
            .slots
            .iter()
            .filter(|slot| slot.hold_intent.is_some())
            .filter(|slot| {
                self.slot_path_in(slot.id, slot.dir.as_deref())
                    .ok()
                    .is_some_and(|path| holding_presence(&path) == Some(true))
            })
            .map(|slot| slot.id)
            .collect();
        for id in stale {
            if let Some(slot) = self.slot_mut(id) {
                slot.hold_intent = None;
                changed = true;
            }
        }
        let Some(holding) = self.open_holding_area(false)? else {
            // The area probe read the payload's own metadata, so the payload is
            // present and the holding area is **positively** absent: no tree can
            // live there, and this open holds the pool lock, so no eviction can
            // be creating it right now. A recorded row is therefore finished,
            // not unknown — a row that kept its charge because its directory
            // vanished would otherwise be charged forever. (An absent removable
            // volume never reaches here: the open that got this far already
            // resolved the payload.)
            if !self.ledger.holding.is_empty() {
                self.ledger.holding.clear();
                changed = true;
            }
            if changed {
                self.persist_revised()?;
            }
            return Ok(());
        };
        let root = self.holding_root();
        let mut gone: Vec<String> = Vec::new();
        for row in &self.ledger.holding {
            match holding_presence(&root.join(&row.name)) {
                Some(true) | None => {}
                Some(false) => gone.push(row.name.clone()),
            }
        }
        if !gone.is_empty() {
            self.ledger.holding.retain(|row| !gone.contains(&row.name));
            changed = true;
        }
        // The generation's own derived name, its recorded bytes and the entry's
        // real identity are what an adoption records. Anything else (a
        // stranger, a non-directory, a foreign owner, a group-writable entry)
        // is left untouched and stays counted by the payload walk.
        let mut adopted: Vec<(u64, HoldingRecord)> = Vec::new();
        for slot in &self.ledger.slots {
            // Only the two windows that rename a tree exist: an idle slot's
            // eviction and a legacy pending deletion's migration. A held row's
            // tree is this command's live build directory; it is never moved
            // into the holding area, so a name that happens to match its
            // generation must not be adopted as one (see the match below).
            let path = self.slot_path_in(slot.id, slot.dir.as_deref())?;
            match holding_presence(&path) {
                // The root is still there: nothing was renamed for this
                // generation, so there is nothing to adopt.
                Some(true) | None => continue,
                Some(false) => {}
            }
            let Some(name) = self.holding_name_for(slot.id, slot.incarnation, &slot.last_lease_id)
            else {
                continue;
            };
            if self.ledger.holding.iter().any(|row| row.name == name) {
                continue;
            }
            // A generation is adopted only against the root identity its own
            // durable intent recorded before the rename. Without that bind
            // nothing is adopted: a directory that merely carries the derived
            // name — a swapped root whose containment failed, an entry an
            // observation error could not classify, or a stranger — is kept,
            // charged by the payload walk and left to a replay, never deleted on
            // a name alone.
            let bound = match slot.state {
                SlotPersistState::Idle | SlotPersistState::Evicting => {
                    match self.recorded_hold_intent(slot) {
                        Some(intent) => (intent.device, intent.inode),
                        None => continue,
                    }
                }
                SlotPersistState::Held => continue,
            };
            let Ok(target) = std::ffi::CString::new(name.as_bytes()) else {
                continue;
            };
            let Ok(observed) = crate::owned_delete::observe_entry(holding.as_fd(), &target) else {
                continue;
            };
            if !observed.is_directory()
                || observed.uid != current_uid()
                || observed.mode_bits() & 0o077 != 0
                || observed.device != self.holding_device()
                || bound != (observed.device, observed.inode)
            {
                continue;
            }
            let bytes = slot
                .recorded
                .as_ref()
                .filter(|record| record.for_lease == slot.last_lease_id)
                .map_or_else(
                    || self.largest_recorded_slot(),
                    |record| record.unique_bytes,
                )
                .max(UNMEASURED_MIN_CHARGE_BYTES);
            adopted.push((
                slot.id,
                HoldingRecord {
                    name,
                    slot: slot.id,
                    incarnation: slot.incarnation,
                    device: observed.device,
                    inode: observed.inode,
                    bytes,
                    at_ms: now_ms(),
                    attempted_ms: None,
                },
            ));
        }
        for (id, row) in adopted {
            self.ledger.slots.retain(|slot| slot.id != id);
            // A record adopted after an interrupted legacy migration no longer
            // names any per-entry journal: the tree is deleted whole.
            self.ledger.holding.push(row);
            changed = true;
        }
        if changed {
            self.persist_revised()?;
        }
        Ok(())
    }

    /// The payload's holding area device, or 0 when it cannot be read. An
    /// adoption refuses an entry on another device; the holding area is on the
    /// payload's own device by construction, so this only ever narrows.
    fn holding_device(&self) -> u64 {
        fs::symlink_metadata(self.holding_root())
            .map(|metadata| metadata.dev())
            .unwrap_or(0)
    }
}

/// The holding entry name one generation derives from its own durable ledger
/// facts: store, slot, incarnation and the generation's lease ID. One function
/// so the writer, the recovery adoption and the tests cannot drift.
fn derived_holding_name(
    store_id: &str,
    id: u64,
    incarnation: u64,
    last_lease_id: &str,
) -> Option<String> {
    let nonce: String = last_lease_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    let name = format!("{store_id}-{id}-{incarnation}-{nonce}");
    holding_name_is_bounded(&name).then_some(name)
}

/// The containment name for a rename whose moved object is not the verified
/// root. It starts with `.`, which `holding_name_is_bounded` refuses for a
/// derived name, so no recovery pass can ever derive it. Nothing ever adopts or
/// deletes a name that is not a recorded row.
fn unverified_holding_name(name: &str) -> String {
    format!(".unverified-{name}")
}

/// Whether one path is positively present. `Some(true)` is an observed entry,
/// `Some(false)` is a positively classified `NotFound`, and `None` is a
/// failure that says nothing about absence — an unreadable parent, an I/O
/// error or an over-bound name. Only `Some(false)` may conclude that a tree is
/// gone.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn holding_presence(path: &Path) -> Option<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Some(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

/// A record's membership fields, as status and the ledger view show them.
fn ledger_slot(slot: &SlotRecord) -> LedgerSlot {
    LedgerSlot {
        slot_id: slot.id.to_string(),
        incarnation: slot.incarnation.to_string(),
        state: occupancy(slot.state),
        project: slot.project.clone(),
        workspace: slot.workspace.clone(),
        profile: slot.profile,
        holder: (slot.state == SlotPersistState::Held).then(|| slot.holder.clone()),
        lease_id: slot.lease_id.clone(),
        dir: slot.dir.clone(),
        rank: slot.rank,
        recorded: slot.recorded.as_ref().map(|record| RecordedFact {
            unique_allocated_bytes: record.unique_bytes,
            logical_bytes: record.logical_bytes,
            entries: record.entries,
            at_ms: record.at_ms,
            current: slot.state == SlotPersistState::Idle
                && slot.lease_id.is_none()
                && record.for_lease == slot.last_lease_id,
        }),
    }
}

/// Debug-only: append one eviction's pool-lock interval to
/// `OXIDE_TEST_HOLDING_LOCK_LOG` (oxide#190). The duration is measured from the
/// open's lock acquisition to the eviction's row write, so it covers the whole
/// eviction-deciding lock hold — admission bookkeeping, the byte-pressure
/// decision and its walks, victim selection, the intent write, the rename and
/// the row write — rather than only the rename transaction.
/// Armed only when the path is inside the supervisor's own
/// `OXIDE_TASK_SCRATCH_ROOT`, so a release binary
/// has no such control and an ordinary developer run writes nothing.
#[cfg(debug_assertions)]
fn record_eviction_lock_ms(pool: &Path, slot: u64, elapsed_ms: u128) {
    use std::io::Write as _;
    let (Some(path), Some(scratch)) = (
        std::env::var_os("OXIDE_TEST_HOLDING_LOCK_LOG"),
        std::env::var_os("OXIDE_TASK_SCRATCH_ROOT"),
    ) else {
        return;
    };
    let path = PathBuf::from(path);
    if !path.is_absolute() || !path.starts_with(PathBuf::from(&scratch)) {
        return;
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let _ = writeln!(
        file,
        "{} {elapsed_ms}",
        pool.join(slot.to_string()).display()
    );
}

/// Clear `errno` before a `readdir` call, so `NULL` can be told from EOF.
fn clear_errno() {
    // SAFETY: the platform's errno location is valid for the thread.
    unsafe {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            *libc::__error() = 0;
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            *libc::__errno_location() = 0;
        }
    }
}

/// The thread's current `errno`.
fn read_errno() -> i32 {
    // SAFETY: the platform's errno location is valid for the thread.
    unsafe {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            *libc::__error()
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            *libc::__errno_location()
        }
    }
}

/// Up to `batch` entry names of one held directory descriptor. Reading stops
/// at the batch bound, and the next pass lists the directory again, so a
/// directory with a huge number of entries costs bounded memory.
fn holding_names(dir: BorrowedFd<'_>, batch: usize) -> Result<Vec<std::ffi::CString>, LeaseError> {
    // A fresh *open* of the same directory, never a `dup`: a duplicate shares
    // this directory's file offset, so the next batch would resume at the end
    // and report an empty directory while refused entries were still in it.
    // `.` is opened without following a link like every other level.
    let reopened = {
        let dot = c".";
        // SAFETY: `dir` and the C string are live for the call.
        unsafe {
            libc::openat(
                dir.as_raw_fd(),
                dot.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        }
    };
    if reopened < 0 {
        return Err(io_error(std::io::Error::last_os_error()));
    }
    // SAFETY: `reopened` is a freshly owned descriptor of a directory.
    let stream = unsafe { libc::fdopendir(reopened) };
    if stream.is_null() {
        // SAFETY: `fdopendir` failed, so the descriptor is still ours.
        unsafe { libc::close(reopened) };
        return Err(io_error(std::io::Error::last_os_error()));
    }
    let mut names = Vec::new();
    loop {
        clear_errno();
        // SAFETY: `stream` is live for the loop and is closed below.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            let code = read_errno();
            // SAFETY: `stream` was returned by `fdopendir` and is closed once.
            unsafe { libc::closedir(stream) };
            if code != 0 {
                return Err(io_error(std::io::Error::from_raw_os_error(code)));
            }
            return Ok(names);
        }
        // SAFETY: `readdir` returned a live dirent for this stream.
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        names.push(name.to_owned());
        if names.len() >= batch {
            // SAFETY: `stream` was returned by `fdopendir` and is closed once.
            unsafe { libc::closedir(stream) };
            return Ok(names);
        }
    }
}

/// Record one per-object refusal, bounded.
fn holding_refuse(stats: &mut HoldingDrain, name: &std::ffi::CStr, reason: &str) {
    if stats.refused.len() >= HOLDING_MAX_REFUSALS {
        return;
    }
    stats.refused.push(format!(
        "{} ({reason})",
        String::from_utf8_lossy(name.to_bytes())
    ));
}

/// Delete every child of one held directory, descriptor-relative and without
/// following a link: a symlink is unlinked, a directory is entered only when
/// it is on the same device, and an entry this traversal cannot classify is
/// kept and reported. Returns whether the directory is now empty.
fn delete_holding_children(
    dir: BorrowedFd<'_>,
    device: u64,
    depth: usize,
    deadline: Instant,
    stats: &mut HoldingDrain,
) -> Result<bool, LeaseError> {
    loop {
        if stats.entries >= HOLDING_DELETE_MAX_ENTRIES || Instant::now() >= deadline {
            return Ok(false);
        }
        let names = holding_names(dir, HOLDING_LIST_BATCH)?;
        if names.is_empty() {
            return Ok(true);
        }
        let mut removed = 0usize;
        for name in names {
            if stats.entries >= HOLDING_DELETE_MAX_ENTRIES || Instant::now() >= deadline {
                return Ok(false);
            }
            stats.entries += 1;
            let facts = match crate::owned_delete::observe_entry(dir, &name) {
                Ok(facts) => facts,
                // Gone already: an earlier pass or a peer removed it.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    holding_refuse(stats, &name, "unreadable entry");
                    continue;
                }
            };
            // A private capture name is #155 journal state, never ordinary
            // content: the retirement of the journal means no record names one
            // any more, so an unrecorded one is *uncertain* and is kept exactly
            // where it is. The tree stays held and charged; the old recursive,
            // in-lock preflight used to refuse the migration for the same
            // reason, and this is the same rule applied outside the lock.
            if crate::owned_delete::is_capture_name(std::ffi::OsStr::from_bytes(name.to_bytes())) {
                holding_refuse(stats, &name, "an unrecorded private capture");
                continue;
            }
            match facts.file_type() {
                libc::S_IFDIR => {
                    if facts.device != device {
                        holding_refuse(stats, &name, "another device");
                        continue;
                    }
                    if facts.uid != current_uid() {
                        holding_refuse(stats, &name, "another owner");
                        continue;
                    }
                    if depth + 1 >= HOLDING_MAX_DEPTH {
                        holding_refuse(stats, &name, "at the depth bound");
                        continue;
                    }
                    // An owned directory the owner left read-only or without
                    // its read bit (a test or build scratch tree) is entered
                    // through a search-only descriptor when needed and its
                    // owner access is restored: the whole tree is being
                    // deleted, so no mode needs putting back. Only this entry's
                    // own mode changes, through a descriptor that cannot be a
                    // link.
                    let child = match crate::owned_delete::open_directory_at(dir, &name) {
                        Ok(child) => child,
                        Err(_) => {
                            match crate::owned_delete::open_directory_for_repair(dir, &name) {
                                Ok(child) => child,
                                Err(_) => {
                                    holding_refuse(stats, &name, "unreadable directory");
                                    continue;
                                }
                            }
                        }
                    };
                    if crate::owned_delete::restore_owner_access(child.as_fd()).is_err() {
                        holding_refuse(stats, &name, "unwritable directory");
                        continue;
                    }
                    if !delete_holding_children(child.as_fd(), device, depth + 1, deadline, stats)?
                    {
                        holding_refuse(stats, &name, "not empty");
                        continue;
                    }
                    match unlink_at(dir, &name, libc::AT_REMOVEDIR) {
                        Ok(()) => removed += 1,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(_) => {
                            holding_refuse(stats, &name, "not empty");
                            continue;
                        }
                    }
                }
                libc::S_IFREG | libc::S_IFLNK => {
                    if facts.file_type() == libc::S_IFREG && facts.uid != current_uid() {
                        holding_refuse(stats, &name, "another owner");
                        continue;
                    }
                    match unlink_at(dir, &name, 0) {
                        Ok(()) => {
                            removed += 1;
                            stats.bytes = stats.bytes.saturating_add(facts.size);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(_) => holding_refuse(stats, &name, "undeletable"),
                    }
                }
                _ => holding_refuse(stats, &name, "an unknown entry type"),
            }
        }
        // Nothing this batch could remove: the rest is refused and stays, so
        // this directory is not empty now and the entry stays charged.
        if removed == 0 {
            return Ok(false);
        }
    }
}

/// Test-only seam: run the traversal on one root with an injected device, so
/// the "another device" refusal can be proved without a second mounted volume.
/// Nothing else is injected: the same bounded traversal, the same refusals and
/// the same device comparison run.
#[cfg(test)]
pub(super) fn delete_holding_children_for_test(
    dir: BorrowedFd<'_>,
    device: u64,
) -> Result<(bool, HoldingDrain), LeaseError> {
    let mut stats = HoldingDrain::default();
    let deadline = Instant::now() + Duration::from_millis(HOLDING_DELETE_MAX_ELAPSED_MS);
    let emptied = delete_holding_children(dir, device, 0, deadline, &mut stats)?;
    Ok((emptied, stats))
}

/// What one claim attempt produced.
enum ClaimOutcome {
    /// This pass has custody of the entry and must delete it.
    Claimed(Box<HoldingClaim>),
    /// Nothing to do: no entry, or every candidate is another deleter's.
    Idle,
    /// A recorded entry could not even be opened for a reason other than
    /// `NotFound`. It stays in the holding area, stays charged and is reported;
    /// this pass deletes nothing else, exactly as for a refused item.
    Unreadable { name: String, reason: String },
}

/// Claim at most one recorded holding entry under a brief pool lock, taking a
/// non-blocking custody lock on the entry's own descriptor. The lock is held
/// for the whole deletion, so a second command skips an entry another process
/// is already deleting instead of interleaving with it.
fn claim_holding(pool: &Path, prefer: Option<&str>) -> Result<ClaimOutcome, LeaseError> {
    let opened = Pool::open(pool)?;
    let Some(area) = opened.open_holding_area(false)? else {
        return Ok(ClaimOutcome::Idle);
    };
    // The pass's own entry first, then a rotating start. The rotation uses the
    // pool's own monotonic idle-rank sequence, which every grant and release
    // advances, so an entry whose deletion refuses a foreign item never keeps
    // another entry from being deleted by a later command. Nothing extra is
    // recorded for it.
    let mut order: Vec<usize> = (0..opened.ledger.holding.len()).collect();
    if let Some(name) = prefer {
        if let Some(index) = opened
            .ledger
            .holding
            .iter()
            .position(|row| row.name == name)
        {
            order.retain(|candidate| *candidate != index);
            order.insert(0, index);
        }
    }
    // Takeover order is least-recently-attempted first, then oldest row, so a
    // row whose deletion refuses is retried only after the others have had their
    // turn. It does not depend on any grant or release: progress must not stall
    // just because every admission is capacity-blocked.
    if prefer.is_none() {
        order.sort_by_key(|index| {
            opened
                .ledger
                .holding
                .get(*index)
                .map(|row| (row.attempted_ms.unwrap_or(0), row.at_ms, row.name.clone()))
                .unwrap_or((0, 0, String::new()))
        });
    }
    let mut claimed = None;
    let mut unreadable = None;
    for index in order {
        let Some(record) = opened.ledger.holding.get(index).cloned() else {
            continue;
        };
        let Ok(target) = std::ffi::CString::new(record.name.as_bytes()) else {
            continue;
        };
        let tree = match crate::owned_delete::open_directory_at(area.as_fd(), &target) {
            Ok(tree) => tree,
            // A positively classified miss is a row a later reconciliation
            // finishes; anything else is unknown, so the entry stays charged.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                if unreadable.is_none() {
                    unreadable = Some((record.name, format!("unreadable entry ({error})")));
                }
                continue;
            }
        };
        // SAFETY: `tree` is a live descriptor for the duration of the deletion.
        if unsafe { libc::flock(tree.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            // Another command holds custody of this entry right now.
            continue;
        }
        claimed = Some((record, tree));
        break;
    }
    let Some((record, tree)) = claimed else {
        return Ok(match unreadable {
            Some((name, reason)) => ClaimOutcome::Unreadable { name, reason },
            None => ClaimOutcome::Idle,
        });
    };
    let target = std::ffi::CString::new(record.name.as_bytes())
        .map_err(|_| LeaseError::UnknownAccounting)?;
    let facts = crate::owned_delete::observe_entry(area.as_fd(), &target).map_err(io_error)?;
    let identity = descriptor_identity(tree.as_fd()).map_err(io_error)?;
    let trusted = identity == (record.device, record.inode)
        && facts.uid == current_uid()
        && facts.mode_bits() & 0o077 == 0
        && facts.device == record.device
        && facts.is_directory();
    Ok(ClaimOutcome::Claimed(Box::new(HoldingClaim {
        name: record.name,
        tree,
        area,
        identity,
        trusted,
    })))
}

/// Delete one claimed holding tree outside the pool lock, then drop its record
/// under a brief lock once the entry is gone.
fn drain_claimed(pool: &Path, claim: &HoldingClaim) -> Result<HoldingDrain, LeaseError> {
    let mut drain = HoldingDrain {
        claimed: Some(claim.name.clone()),
        ..HoldingDrain::default()
    };
    if !claim.trusted {
        drain
            .refused
            .push(format!("{} (not the recorded tree)", claim.name));
        return Ok(drain);
    }
    let deadline = Instant::now() + Duration::from_millis(HOLDING_DELETE_MAX_ELAPSED_MS);
    // The tree is being deleted, so no mode needs putting back: an owned slot
    // root the owner left read-only or without its write bit (a test or build
    // scratch tree) gains its owner access through the descriptor this claim
    // already holds, addressed by no path and never through a link. Without it
    // the root's own children cannot be unlinked and the entry would stay
    // charged forever.
    if crate::owned_delete::restore_owner_access(claim.tree.as_fd()).is_err() {
        drain
            .refused
            .push(format!("{} (unwritable tree)", claim.name));
        return Ok(drain);
    }
    let emptied = delete_holding_children(
        claim.tree.as_fd(),
        claim.identity.0,
        0,
        deadline,
        &mut drain,
    )?;
    if emptied {
        let target = std::ffi::CString::new(claim.name.as_bytes())
            .map_err(|_| LeaseError::UnknownAccounting)?;
        // Remove exactly the directory this claim locked: the name must still
        // be the same object, or a racer has taken it and nothing is removed.
        match crate::owned_delete::observe_entry(claim.area.as_fd(), &target) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => drain.gone = true,
            Ok(facts) if (facts.device, facts.inode) == claim.identity => {
                match unlink_at(claim.area.as_fd(), &target, libc::AT_REMOVEDIR) {
                    Ok(()) => drain.gone = true,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => drain.gone = true,
                    Err(_) => {
                        holding_refuse(&mut drain, &target, "the entry is not empty");
                    }
                }
            }
            Ok(_) => holding_refuse(&mut drain, &target, "the entry was replaced"),
            Err(_) => holding_refuse(&mut drain, &target, "the entry is unreadable"),
        }
    }
    if drain.gone {
        // One brief locked write drops the row: the bytes are charged until
        // this point and never after.
        let mut opened = Pool::open(pool)?;
        if opened
            .ledger
            .holding
            .iter()
            .any(|row| row.name == claim.name)
        {
            opened.ledger.holding.retain(|row| row.name != claim.name);
            opened.persist_revised()?;
        }
    }
    Ok(drain)
}

/// Delete at most one pending holding entry outside the pool lock (oxide#190).
///
/// The claim and the conclusion each take the pool lock briefly; the deletion
/// itself walks the tree with held descriptors and no lock, so admission and
/// release are never held behind a deletion. `prefer` names the entry this
/// command just created, so its own eviction is what it finishes.
pub(super) fn drain_holding(pool: &Path, prefer: Option<&str>) -> Result<HoldingDrain, LeaseError> {
    match claim_holding(pool, prefer)? {
        ClaimOutcome::Claimed(claim) => {
            let drain = drain_claimed(pool, &claim)?;
            if !drain.gone {
                // The row stayed: record that this pass reached it, so the next
                // takeover pass prefers another row and one undeletable tree
                // cannot starve the rest.
                mark_holding_attempted(pool, &claim.name)?;
            }
            Ok(drain)
        }
        ClaimOutcome::Idle => Ok(HoldingDrain::nothing()),
        ClaimOutcome::Unreadable { name, reason } => {
            mark_holding_attempted(pool, &name)?;
            Ok(HoldingDrain {
                claimed: Some(name),
                refused: vec![reason],
                ..HoldingDrain::default()
            })
        }
    }
}

/// One O(1) write records that a pass reached this row without finishing it.
fn mark_holding_attempted(pool: &Path, name: &str) -> Result<(), LeaseError> {
    let mut opened = Pool::open(pool)?;
    let stamp = now_ms();
    let Some(row) = opened
        .ledger
        .holding
        .iter_mut()
        .find(|row| row.name == name)
    else {
        return Ok(());
    };
    if row.attempted_ms == Some(stamp) {
        return Ok(());
    }
    row.attempted_ms = Some(stamp);
    opened.persist_revised()
}

fn occupancy(state: SlotPersistState) -> SlotOccupancy {
    match state {
        SlotPersistState::Held => SlotOccupancy::Held,
        SlotPersistState::Idle => SlotOccupancy::Idle,
        SlotPersistState::Evicting => SlotOccupancy::Abandoned,
    }
}

#[cfg(test)]
thread_local! {
    static TEST_LIMITS: std::cell::Cell<Option<InventoryLimits>> =
        const { std::cell::Cell::new(None) };
}

/// Run `body` with these accounting bounds on this thread (tests only).
#[cfg(test)]
pub(super) fn with_accounting_limits<T>(
    max_entries: u64,
    max_depth: usize,
    max_elapsed_ms: u64,
    body: impl FnOnce() -> T,
) -> T {
    let limits = InventoryLimits {
        max_entries,
        max_depth,
        max_output_bytes: 64,
        max_elapsed_ms,
    };
    TEST_LIMITS.with(|cell| cell.set(Some(limits)));
    let result = body();
    TEST_LIMITS.with(|cell| cell.set(None));
    result
}

fn accounting_limits() -> InventoryLimits {
    #[cfg(test)]
    if let Some(limits) = TEST_LIMITS.with(std::cell::Cell::get) {
        return limits;
    }
    InventoryLimits {
        max_entries: ACCOUNTING_MAX_ENTRIES,
        max_depth: ACCOUNTING_MAX_DEPTH,
        max_output_bytes: 64,
        max_elapsed_ms: ACCOUNTING_MAX_ELAPSED_MS,
    }
}

fn scan_with(
    limits: InventoryLimits,
    coverage: AccountingCoverage,
    gaps: Vec<AccountingGap>,
    entries_visited: u64,
    elapsed_ms: u64,
    vanished: u64,
) -> AccountingScan {
    AccountingScan {
        coverage,
        gaps,
        vanished,
        entries_visited,
        elapsed_ms,
        max_entries: limits.max_entries,
        max_depth: limits.max_depth,
        max_elapsed_ms: limits.max_elapsed_ms,
    }
}

fn root_failure_with(
    limits: InventoryLimits,
    kind: AccountingGapKind,
    elapsed_ms: u64,
) -> AccountingScan {
    let gap = AccountingGap {
        kind,
        occurrences: 1,
    };
    scan_with(
        limits,
        AccountingCoverage::Unknown,
        vec![gap],
        0,
        elapsed_ms,
        0,
    )
}

fn gap_kind(kind: GapKind) -> AccountingGapKind {
    match kind {
        GapKind::EntryLimit => AccountingGapKind::EntryLimit,
        GapKind::DepthLimit => AccountingGapKind::DepthLimit,
        GapKind::ElapsedDeadline => AccountingGapKind::ElapsedDeadline,
        GapKind::MountBoundary => AccountingGapKind::MountBoundary,
        GapKind::UnreadableEntry => AccountingGapKind::UnreadableEntry,
        GapKind::MissingMetadata => AccountingGapKind::MissingMetadata,
        // Accounting renders nothing, so an output gap means unusable metadata.
        GapKind::UnsupportedMetadata | GapKind::OutputLimit => {
            AccountingGapKind::UnsupportedMetadata
        }
    }
}

/// One bounded walk with the pool accounting bounds. Bytes of an incomplete
/// walk are a lower bound for status only.
fn inventory_sizes(path: &Path) -> Sizes {
    inventory_sizes_deeper(path, 0)
}

/// [`inventory_sizes`] for the shallow pool scope pass: the same bounds and
/// the same gap mapping, over a walk that hard-stops at the recorded slot
/// roots it is given. It is a different entry point from the payload walk, so
/// a scope pass never registers as an inventory walk of the payload.
fn scope_sizes_with(
    root: &Path,
    stops: &[PathBuf],
    isolated: &[PathBuf],
    base: InventoryLimits,
) -> Sizes {
    let started = std::time::Instant::now();
    match crate::inventory::scope_directory_isolating(root, stops, isolated, base) {
        Ok(report) => {
            let coverage = if report.coverage.complete {
                AccountingCoverage::Complete
            } else {
                AccountingCoverage::Partial
            };
            let gaps = report
                .coverage
                .gaps
                .iter()
                .map(|gap| AccountingGap {
                    kind: gap_kind(gap.kind),
                    occurrences: gap.occurrences,
                })
                .collect();
            Sizes {
                logical: Some(report.total.logical_bytes_by_name),
                unique: Some(report.total.allocated_bytes),
                scan: scan_with(
                    base,
                    coverage,
                    gaps,
                    report.entries_visited,
                    report.elapsed_ms,
                    report.vanished,
                ),
            }
        }
        Err(error) => {
            let kind = match error {
                InventoryError::RootMissing(_) => AccountingGapKind::RootMissing,
                InventoryError::RootNotDirectory(_) => AccountingGapKind::RootNotDirectory,
                InventoryError::RootUnreadable { .. } | InventoryError::InvalidLimits(_) => {
                    AccountingGapKind::RootUnreadable
                }
            };
            let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            Sizes {
                logical: None,
                unique: None,
                scan: root_failure_with(base, kind, elapsed),
            }
        }
    }
}

/// Wall-clock milliseconds. A recorded observation's age, never a validity
/// input: validity is the slot's generation.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// [`inventory_sizes`] with the depth bound raised by `extra_depth`, for a
/// root that sits that many levels above the slots; the scan reports the
/// bounds actually used.
fn inventory_sizes_deeper(path: &Path, extra_depth: usize) -> Sizes {
    inventory_sizes_with(path, extra_depth, accounting_limits())
}

/// [`inventory_sizes_deeper`] with explicit bounds, so one measurement can
/// share a cumulative deadline across every walk it makes.
fn inventory_sizes_with(path: &Path, extra_depth: usize, base: InventoryLimits) -> Sizes {
    let started = std::time::Instant::now();
    let mut limits = base;
    limits.max_depth = limits.max_depth.saturating_add(extra_depth);
    match inventory_directory(path, limits) {
        Ok(report) => {
            let coverage = if report.coverage.complete {
                AccountingCoverage::Complete
            } else {
                AccountingCoverage::Partial
            };
            let gaps = report
                .coverage
                .gaps
                .iter()
                .map(|gap| AccountingGap {
                    kind: gap_kind(gap.kind),
                    occurrences: gap.occurrences,
                })
                .collect();
            Sizes {
                logical: Some(report.total.logical_bytes_by_name),
                unique: Some(report.total.allocated_bytes),
                scan: scan_with(
                    limits,
                    coverage,
                    gaps,
                    report.entries_visited,
                    report.elapsed_ms,
                    report.vanished,
                ),
            }
        }
        Err(error) => {
            let kind = match error {
                InventoryError::RootMissing(_) => AccountingGapKind::RootMissing,
                InventoryError::RootNotDirectory(_) => AccountingGapKind::RootNotDirectory,
                InventoryError::RootUnreadable { .. } | InventoryError::InvalidLimits(_) => {
                    AccountingGapKind::RootUnreadable
                }
            };
            let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            Sizes {
                logical: None,
                unique: None,
                scan: root_failure_with(limits, kind, elapsed),
            }
        }
    }
}

fn require_holder(holder: &str) -> Result<(), LeaseError> {
    super::require_bounded_text(holder, MAX_HOLDER_BYTES, "holder")
}

fn validate_lease_id(lease_id: &str) -> Result<(), LeaseError> {
    super::require_bounded_text(lease_id, MAX_LEASE_ID_BYTES, "lease")
}

fn prepare_new_pool(pool: &Path) -> Result<PathBuf, LeaseError> {
    super::require_absolute(pool, "pool")?;
    if pool.as_os_str().as_bytes().len() > MAX_PATH_BYTES {
        return Err(LeaseError::InvalidInput("path is too long"));
    }
    match fs::symlink_metadata(pool) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(LeaseError::UnsafePath("symlinks are refused"));
            }
            if !metadata.file_type().is_dir() {
                return Err(LeaseError::InvalidInput("pool must be a directory"));
            }
            if metadata.uid() != current_uid() {
                return Err(LeaseError::UnsafePath(
                    "pool must be an owner-only directory",
                ));
            }
            refuse_occupied_init_directory(pool)?;
            if metadata.permissions().mode() & 0o077 != 0 {
                fs::set_permissions(pool, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let name = pool
                .file_name()
                .ok_or(LeaseError::InvalidInput("pool has no name"))?;
            let parent = pool
                .parent()
                .ok_or(LeaseError::InvalidInput("pool has no parent"))?;
            let parent = canonical_existing_dir(parent, false)?;
            let created = parent.join(name);
            fs::create_dir(&created).map_err(io_error)?;
            fs::set_permissions(&created, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(io_error)?;
            return canonical_existing_dir(&created, true);
        }
        Err(error) => return Err(io_error(error)),
    }
    canonical_existing_dir(pool, true)
}

/// Empty, or only the CLI's leftover `lock` from a failed init.
fn refuse_occupied_init_directory(pool: &Path) -> Result<(), LeaseError> {
    let mut leftover_lock = false;
    for entry in fs::read_dir(pool).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        if leftover_lock || name != LOCK_NAME {
            return Err(LeaseError::InvalidInput(
                "init requires an empty pool directory",
            ));
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
        if metadata.file_type().is_symlink() {
            return Err(LeaseError::UnsafePath("symlinks are refused"));
        }
        if !metadata.file_type().is_file() || metadata.uid() != current_uid() {
            return Err(LeaseError::InvalidInput(
                "init requires an empty pool directory",
            ));
        }
        leftover_lock = true;
    }
    Ok(())
}

fn lock_pool(pool: &Path) -> Result<File, LeaseError> {
    lock_pool_inner(pool, true, false)
}

fn lock_pool_inner(pool: &Path, create: bool, notice: bool) -> Result<File, LeaseError> {
    let path = pool.join(LOCK_NAME);
    let existing = match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(LeaseError::UnsafePath("lock file must be a regular file"));
            }
            Some(metadata)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => None,
        Err(error) => return Err(io_error(error)),
    };
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    if create {
        options.create(true);
    }
    let file = options.open(&path).map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != current_uid()
        || existing
            .is_some_and(|before| before.dev() != metadata.dev() || before.ino() != metadata.ino())
    {
        return Err(LeaseError::UnsafePath("lock file must be private"));
    }
    if notice {
        wait_before_pool_notice(&file, Duration::from_secs(5), || {
            let slot = read_ledger(pool).ok().and_then(|ledger| {
                ledger
                    .slots
                    .into_iter()
                    .find(|slot| slot.state == SlotPersistState::Evicting)
            });
            match slot {
                Some(slot) => eprintln!("oxide: waiting for the pool lock (slot {} is recorded as being deleted; its current writer is unknown)", slot.id),
                None => eprintln!("oxide: waiting for the pool lock (holder unknown)"),
            }
        })?;
        return Ok(file);
    }
    // SAFETY: `file` is open for the pool lock; flock holds until File drop.
    let status = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if status != 0 {
        return Err(LeaseError::Io("exclusive pool lock failed".into()));
    }
    Ok(file)
}

// Poll only until the one notice, then return to the ordinary blocking flock.
fn wait_before_pool_notice(
    file: &File,
    delay: Duration,
    notice: impl FnOnce(),
) -> Result<(), LeaseError> {
    let started = Instant::now();
    loop {
        // SAFETY: the descriptor stays open through this call and until release.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(LeaseError::Io(format!(
                "exclusive pool lock failed: {error}"
            )));
        }
        if started.elapsed() >= delay {
            notice();
            // SAFETY: the descriptor stays open and owns the lock on success.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(LeaseError::Io("exclusive pool lock failed".into()));
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100).min(delay.saturating_sub(started.elapsed())));
    }
}

fn write_config(pool: &Path, config: &PoolConfig) -> Result<(), LeaseError> {
    let bytes = serde_json::to_vec(config).map_err(|error| LeaseError::Io(error.to_string()))?;
    persist_private_file(&pool.join(CONFIG_NAME), &bytes)
}

fn write_ledger(pool: &Path, ledger: &Ledger) -> Result<(), LeaseError> {
    let bytes = serde_json::to_vec(ledger).map_err(|error| LeaseError::Io(error.to_string()))?;
    persist_private_file(&pool.join(LEDGER_NAME), &bytes)
}

/// Read-side shape checks: identities, the recorded slot path and the
/// capacity-policy/rank fields. Legacy ledgers have no policy fence and rank
/// 0 everywhere; a rank without its fence is not a representation this
/// version wrote. A slot count above the legacy ceiling always carries the
/// raised-ceiling fence (oxide#177), so an older binary refuses it.
fn validate_ledger_read(ledger: &Ledger) -> Result<(), LeaseError> {
    if ledger.slots.len() > MAX_SLOTS {
        return Err(LeaseError::LedgerExhausted);
    }
    // Held trees pending deletion (oxide#190): bounded rows of the exact shape
    // this writer produces, and never the same entry name twice, so one
    // holding directory is never charged or deleted twice.
    if ledger.holding.len() > MAX_HOLDING_ENTRIES
        || !ledger.holding.iter().all(HoldingRecord::is_valid)
        || ledger.holding.iter().enumerate().any(|(index, row)| {
            ledger.holding[..index]
                .iter()
                .any(|other| other.name == row.name)
        })
    {
        return Err(LeaseError::UnknownAccounting);
    }
    if ledger.slots.len() > LEGACY_MAX_SLOTS
        && ledger.capacity_policy != Some(RAISED_CAPACITY_POLICY_VERSION)
    {
        return Err(LeaseError::UnknownAccounting);
    }
    match (ledger.capacity_policy, ledger.next_rank) {
        (None, None) => {
            if ledger.slots.iter().any(|slot| slot.rank != 0) {
                return Err(LeaseError::UnknownAccounting);
            }
            if ledger.slots.iter().any(|slot| slot.recorded.is_some()) {
                return Err(LeaseError::UnknownAccounting);
            }
        }
        (Some(policy), Some(next)) => {
            if !matches!(
                policy,
                LEGACY_CAPACITY_POLICY_VERSION
                    | PREVIOUS_CAPACITY_POLICY_VERSION
                    | CAPACITY_POLICY_VERSION
                    | RAISED_CAPACITY_POLICY_VERSION
            ) || next == 0
            {
                return Err(LeaseError::UnknownAccounting);
            }
            if ledger.slots.iter().any(|slot| slot.rank >= next) {
                return Err(LeaseError::UnknownAccounting);
            }
            // Only the version-2 and later representations write records: a
            // version-1 ledger holding one is a state no writer can produce.
            if policy == LEGACY_CAPACITY_POLICY_VERSION
                && ledger.slots.iter().any(|slot| slot.recorded.is_some())
            {
                return Err(LeaseError::UnknownAccounting);
            }
        }
        (None, Some(_)) | (Some(_), None) => return Err(LeaseError::UnknownAccounting),
    }
    for slot in &ledger.slots {
        if slot.id == 0 || slot.incarnation == 0 {
            return Err(LeaseError::UnknownAccounting);
        }
        if let Some(dir) = &slot.dir {
            split_slot_dir(dir, slot.id).ok_or(LeaseError::UnknownAccounting)?;
        }
        if slot.recorded.is_some()
            && (slot.state == SlotPersistState::Held
                || (slot.state == SlotPersistState::Evicting
                    && !matches!(
                        ledger.capacity_policy,
                        Some(CAPACITY_POLICY_VERSION) | Some(RAISED_CAPACITY_POLICY_VERSION)
                    )))
        {
            return Err(LeaseError::UnknownAccounting);
        }
        // A capture record belongs to a recorded deletion and nowhere else,
        // and only in the exact shape the seam writes.
        if slot.capture.is_some()
            && (slot.state != SlotPersistState::Evicting
                || !slot.capture.as_ref().is_some_and(DeletionJournal::is_valid))
        {
            return Err(LeaseError::UnknownAccounting);
        }
        // A hold intent names a rename a deletion is about to perform and the
        // root identity it verified. Only the two states that can be a rename
        // source may carry one: an idle slot waiting for its eviction's write,
        // and a recorded legacy deletion. A held row can never carry it.
        if slot.hold_intent.is_some()
            && (!matches!(
                slot.state,
                SlotPersistState::Idle | SlotPersistState::Evicting
            ) || !slot
                .hold_intent
                .as_ref()
                .is_some_and(HoldingIntent::is_valid))
        {
            return Err(LeaseError::UnknownAccounting);
        }
    }
    Ok(())
}

/// Write-side checks: everything `validate_ledger_read` checks, plus every
/// stored field against the raised bounds. An oversized legacy field
/// refuses the revised write with the original file untouched; nothing is
/// truncated or dropped.
fn validate_ledger_write(ledger: &Ledger) -> Result<(), LeaseError> {
    validate_ledger_read(ledger)?;
    if ledger.slots.len() > MAX_SLOTS {
        return Err(LeaseError::LedgerExhausted);
    }
    // A holding row's name must be the derivation of a recorded generation, so
    // a row can never name a directory this writer would not have created.
    if ledger.holding.iter().any(|row| {
        row.slot >= ledger.next_slot || row.incarnation == 0 || row.device == 0 || row.inode == 0
    }) {
        return Err(LeaseError::UnknownAccounting);
    }
    if ledger.next_slot == 0 || ledger.next_lease == 0 {
        return Err(LeaseError::UnknownAccounting);
    }
    if ledger.next_rank.is_some_and(|next| next == 0) {
        return Err(LeaseError::UnknownAccounting);
    }
    for slot in &ledger.slots {
        if slot.project.as_os_str().as_bytes().len() > MAX_PATH_BYTES
            || !slot.project.is_absolute()
            || slot.workspace.as_os_str().as_bytes().len() > MAX_PATH_BYTES
            || !slot.workspace.is_absolute()
            || slot.holder.is_empty()
            || slot.holder.len() > MAX_HOLDER_BYTES
            || slot.last_lease_id.is_empty()
            || slot.last_lease_id.len() > MAX_LEASE_ID_BYTES
            || slot.recorded.as_ref().is_some_and(|record| {
                record.for_lease.is_empty() || record.for_lease.len() > MAX_LEASE_ID_BYTES
            })
            || slot
                .lease_id
                .as_ref()
                .is_some_and(|lease| lease.is_empty() || lease.len() > MAX_LEASE_ID_BYTES)
        {
            return Err(LeaseError::UnknownAccounting);
        }
    }
    Ok(())
}

fn read_config(pool: &Path) -> Result<PoolConfig, LeaseError> {
    let bytes = read_private_file(&pool.join(CONFIG_NAME))?;
    serde_json::from_slice(&bytes).map_err(|_| LeaseError::BackingMismatch)
}

fn read_ledger(pool: &Path) -> Result<Ledger, LeaseError> {
    let bytes = read_private_file(&pool.join(LEDGER_NAME))?;
    let ledger: Ledger =
        serde_json::from_slice(&bytes).map_err(|_| LeaseError::UnknownAccounting)?;
    validate_ledger_read(&ledger)?;
    Ok(ledger)
}

#[cfg(test)]
pub(super) fn write_ledger_bytes_injected(
    pool: &Path,
    bytes: &[u8],
) -> Result<Vec<u8>, LeaseError> {
    let ledger: Ledger =
        serde_json::from_slice(bytes).map_err(|_| LeaseError::UnknownAccounting)?;
    validate_ledger_write(&ledger)?;
    write_ledger(pool, &ledger)?;
    let stored = read_ledger(pool)?;
    serde_json::to_vec(&stored).map_err(|error| LeaseError::Io(error.to_string()))
}

/// Read-only, before the pool lock is created or taken: a pool holding the
/// retired image or receipt, or whose config records any backend but the
/// two current ones, refuses untouched. A missing config is left to the
/// locked read, which waits for a concurrent creation.
fn refuse_retired_image_pool(pool: &Path) -> Result<(), LeaseError> {
    let paths = BackingPaths::under(pool);
    if paths.image.symlink_metadata().is_ok() || paths.receipt.symlink_metadata().is_ok() {
        return Err(LeaseError::BackingMismatch);
    }
    if pool.join(CONFIG_NAME).symlink_metadata().is_err() {
        return Ok(());
    }
    let config = read_config(pool)?;
    if config.backend == BUDGET_BACKEND_DIRECTORY_V1 || config.backend == VOLUME_BACKEND_APFS_V1 {
        Ok(())
    } else {
        Err(LeaseError::BackingMismatch)
    }
}

/// `lstat` of a pool control entry that the listing just named. `NotFound`
/// means the entry was renamed away or removed in between: a ledger writer
/// renames its `ledger.tmp-*` onto `ledger.json`, and Finder removes its
/// metadata files. The caller skips such an entry. Every other error refuses.
fn listed_control_entry(path: &Path) -> Result<Option<fs::Metadata>, LeaseError> {
    #[cfg(test)]
    let metadata = crate::test_fs::symlink_metadata(path);
    #[cfg(not(test))]
    let metadata = fs::symlink_metadata(path);
    match metadata {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

fn refuse_foreign_pool_entries(pool: &Path) -> Result<(), LeaseError> {
    let device = fs::symlink_metadata(pool).map_err(io_error)?.dev();
    for entry in fs::read_dir(pool).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        if is_finder_metadata(&name) {
            // Control-directory bytes, like config and ledger: not payload.
            let Some(metadata) = listed_control_entry(&entry.path())? else {
                continue;
            };
            if !is_tolerated_metadata_file(&metadata, device) {
                return Err(unexpected_entry(POOL_PLACE, &name));
            }
            continue;
        }
        let text = name.to_string_lossy();
        let allowed = matches!(
            text.as_ref(),
            super::CONFIG_NAME
                | super::LEDGER_NAME
                | super::LOCK_NAME
                | super::IMAGE_NAME
                | super::RECEIPT_NAME
                | super::PAYLOAD_NAME
                | super::MANAGED_NAME
        ) || text.starts_with("ledger.tmp-");
        if !allowed {
            return Err(unexpected_entry(POOL_PLACE, &name));
        }
        let Some(metadata) = listed_control_entry(&entry.path())? else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            return Err(LeaseError::UnsafePath("symlinks are refused"));
        }
    }
    Ok(())
}

/// Build a removable-backed `Pool` over an owned fixture payload directory.
///
/// Test-only. The backing is `Payload::Volume` (so every production branch for
/// a removable pool is taken) with a descriptor over the fixture directory, and
/// the verification result comes from [`crate::volume::verified_facts`]: while
/// that adapter is installed `Binding::verify()` returns those modelled facts
/// and therefore skips `observe()`, the mount-root rule, the device comparison
/// and the mount-point path re-check. Nothing here is a public or environment
/// interface, and it is never native verification of a real volume.
#[cfg(test)]
pub(super) fn volume_pool_for_test(
    pool_root: &Path,
    payload: &Path,
    budget: u64,
    reserve: Option<u64>,
) -> Result<Pool, LeaseError> {
    let store_id = "b".repeat(32);
    let binding = crate::volume::binding_for_test(payload, &"1".repeat(36))
        .map_err(|_| LeaseError::BackingMismatch)?;
    let fd = binding
        .fd()
        .try_clone_to_owned()
        .map_err(|error| LeaseError::Io(error.to_string()))?;
    let config = PoolConfig {
        version: LEASE_VERSION,
        backend: VOLUME_BACKEND_APFS_V1.to_owned(),
        store_id: store_id.clone(),
        cap_bytes: budget,
        image_bytes: 0,
        volume: Some(VolumeConfig {
            mount_point: payload.to_path_buf(),
            volume_uuid: "1".repeat(36),
            payload_dir: "payload".to_owned(),
        }),
    };
    let ledger = Ledger {
        version: LEASE_VERSION,
        store_id,
        cap_bytes: budget,
        next_slot: 1,
        next_lease: 1,
        capacity_policy: None,
        next_rank: None,
        holding: Vec::new(),
        slots: Vec::new(),
    };
    Ok(Pool {
        _lock: None,
        root: pool_root.to_path_buf(),
        config,
        backing: Payload::Volume {
            binding,
            root: payload.to_path_buf(),
            fd,
        },
        budget,
        ledger,
        limits: accounting_limits(),
        deadline: None,
        reconciliation_blocked: false,
        reconciliation_unresolved: false,
        admission_note: None,
        unmeasured: BTreeMap::new(),
        walk_started: None,
        reserve,
        created_holding: Vec::new(),
        #[cfg(debug_assertions)]
        lock_taken: None,
    })
}

/// Test-only: one revised write that fills the whole slot table with **held**
/// slots, so an admission at the ceiling has no evictable idle victim. Returns
/// the last slot's ID and lease ID, which a caller can release to create the
/// only idle slot (oxide#186 ceiling fixtures). No payload is written.
#[cfg(test)]
pub(super) fn fill_held_slot_table_for_test(pool: &Path) -> Result<(u64, String), LeaseError> {
    let mut opened = Pool::open(pool)?;
    for id in 1..=MAX_SLOTS as u64 {
        add_idle_slot_for_test(&mut opened, id, id, 0, 0)?;
        let slot = opened.slot_mut(id).ok_or(LeaseError::UnknownAccounting)?;
        slot.state = SlotPersistState::Held;
        slot.holder = format!("holder-{id}");
        slot.lease_id = Some(slot.last_lease_id.clone());
        slot.recorded = None;
    }
    opened.persist_revised()?;
    let last = opened
        .ledger
        .slots
        .last()
        .map(|slot| (slot.id, slot.last_lease_id.clone()))
        .ok_or(LeaseError::UnknownAccounting)?;
    Ok(last)
}

/// Test-only: give this pool an idle whole slot with a closed observation.
#[cfg(test)]
pub(super) fn add_idle_slot_for_test(
    pool: &mut Pool,
    id: u64,
    rank: u64,
    recorded_bytes: u64,
    payload_files: usize,
) -> Result<PathBuf, LeaseError> {
    let slot_path = pool.slot_path_in(id, None)?;
    fs::create_dir_all(slot_path.join("build")).map_err(io_error)?;
    // The product's slots are owner-only; a fixture slot must be too, or the
    // idle-scope and holding checks correctly refuse it.
    for path in [slot_path.join("build"), slot_path.clone()] {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    }
    for index in 0..payload_files {
        fs::write(
            slot_path.join("build").join(format!("blob-{index}")),
            vec![b'x'; 64 * 1024],
        )
        .map_err(io_error)?;
    }
    let last_lease_id = format!("v1.{}.{id}.1.{id}", pool.config.store_id);
    pool.ledger.slots.push(SlotRecord {
        id,
        incarnation: 1,
        state: SlotPersistState::Idle,
        project: PathBuf::from("/fixture/project"),
        workspace: PathBuf::from("/fixture/workspace"),
        profile: Profile::Dev,
        holder: "fixture".to_owned(),
        lease_id: None,
        last_lease_id: last_lease_id.clone(),
        dir: None,
        rank,
        recorded: Some(RecordedSize {
            for_lease: last_lease_id,
            unique_bytes: recorded_bytes,
            logical_bytes: recorded_bytes,
            entries: payload_files as u64,
            at_ms: 1,
        }),
        capture: None,
        hold_intent: None,
    });
    pool.ledger.next_slot = id + 1;
    pool.ledger.next_rank = Some(rank + 1);
    Ok(slot_path)
}

/// Test-only: the pool's slot IDs in ledger order.
#[cfg(test)]
pub(super) fn slot_ids_for_test(pool: &Pool) -> Vec<u64> {
    pool.ledger.slots.iter().map(|slot| slot.id).collect()
}

/// Test-only: mark a slot held, for victim-safety checks.
#[cfg(test)]
pub(super) fn hold_slot_for_test(pool: &mut Pool, id: u64, holder: &str) {
    let lease = format!("v1.{}.{id}.1.{}", pool.config.store_id, id + 100);
    if let Some(slot) = pool.slot_mut(id) {
        slot.state = SlotPersistState::Held;
        slot.lease_id = Some(lease);
        slot.holder = holder.to_owned();
        // The product clears the observation in the same write that records
        // `Held`; a held slot never carries an idle record.
        slot.recorded = None;
    }
}

/// Test-only: drop one idle slot's closed observation, producing the
/// legacy-shaped state a pool that never released under this version has.
#[cfg(test)]
pub(super) fn drop_record_for_test(pool: &Path, slot_id: u64) -> Result<(), LeaseError> {
    let mut opened = Pool::open(pool)?;
    let slot = opened
        .slot_mut(slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    slot.recorded = None;
    opened.persist()
}

/// Test-only: leave a record whose generation fence no longer matches, with an
/// absurd figure so a borrowed one would be visible.
#[cfg(test)]
pub(super) fn with_stale_record_for_test(pool: &Path, slot_id: u64) -> Result<(), LeaseError> {
    let mut opened = Pool::open(pool)?;
    let slot = opened
        .slot_mut(slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    let record = slot
        .recorded
        .as_mut()
        .ok_or(LeaseError::UnknownAccounting)?;
    record.for_lease = "v1.stale.0.0.0".to_owned();
    record.unique_bytes = record.unique_bytes.saturating_add(9 * 1024 * 1024 * 1024);
    opened.persist()
}

#[cfg(test)]
mod unknown_removable_tests {
    use super::*;

    fn scan(gaps: &[AccountingGapKind], coverage: AccountingCoverage) -> AccountingScan {
        AccountingScan {
            coverage,
            gaps: gaps
                .iter()
                .map(|kind| AccountingGap {
                    kind: *kind,
                    occurrences: 1,
                })
                .collect(),
            vanished: 0,
            entries_visited: 318_620,
            elapsed_ms: 60_005,
            max_entries: 2_000_000,
            max_depth: 66,
            max_elapsed_ms: 60_000,
        }
    }

    fn bounds(gaps: &[AccountingGapKind]) -> AccountingScan {
        scan(gaps, AccountingCoverage::Partial)
    }

    const RESERVE: u64 = 20 * 1024 * 1024 * 1024;
    const FLOOR: u64 = 10 * 1024 * 1024 * 1024;

    #[test]
    fn free_above_the_reserve_admits_without_eviction_and_without_a_cap_claim() {
        for gaps in [
            &[AccountingGapKind::ElapsedDeadline][..],
            &[AccountingGapKind::EntryLimit][..],
            &[
                AccountingGapKind::EntryLimit,
                AccountingGapKind::ElapsedDeadline,
            ][..],
        ] {
            let step = unknown_removable_step(
                &bounds(gaps),
                Some(RESERVE + 1),
                Some(RESERVE),
                FLOOR,
                true,
            )
            .expect("bounds-only gaps qualify");
            let UnknownRemovableStep::Admit(note) = step else {
                panic!("free space above the reserve must admit: {step:?}");
            };
            assert!(note.contains("318620 entries"), "{note}");
            assert!(note.contains("nothing was evicted"), "{note}");
            assert!(note.contains("cap is unverified"), "{note}");
            // No volume-usage input exists, so no foreign total can appear.
            assert!(!note.contains("used"), "{note}");
        }
    }

    #[test]
    fn at_or_below_the_reserve_prunes_an_idle_slot_first() {
        // Equality at the reserve prunes; so does any free space between the
        // reserve and the floor, while an eligible idle slot remains.
        for available in [RESERVE, RESERVE - 1, FLOOR] {
            let step = unknown_removable_step(
                &bounds(&[AccountingGapKind::ElapsedDeadline]),
                Some(available),
                Some(RESERVE),
                FLOOR,
                true,
            )
            .expect("bounds-only gaps qualify");
            assert_eq!(step, UnknownRemovableStep::Evict, "available {available}");
        }
        // Below the hard floor the pool still reclaims first: an eligible idle
        // slot is pruned and free space is re-read before anything decides.
        for available in [FLOOR - 1, 0] {
            let step = unknown_removable_step(
                &bounds(&[AccountingGapKind::ElapsedDeadline]),
                Some(available),
                Some(RESERVE),
                FLOOR,
                true,
            )
            .expect("bounds-only gaps qualify");
            assert_eq!(step, UnknownRemovableStep::Evict, "available {available}");
            // With nothing left to prune the same free space refuses.
            let step = unknown_removable_step(
                &bounds(&[AccountingGapKind::ElapsedDeadline]),
                Some(available),
                Some(RESERVE),
                FLOOR,
                false,
            )
            .expect("bounds-only gaps qualify");
            assert!(
                matches!(step, UnknownRemovableStep::Refuse(_)),
                "available {available} with no idle left"
            );
        }
    }

    #[test]
    fn the_hard_floor_wins_over_a_lower_legacy_reserve() {
        let original = bounds(&[AccountingGapKind::EntryLimit]);
        // 5 GiB free, a legacy 2 GiB reserve: free is above the reserve yet
        // below the floor, so nothing admits merely because of the reserve.
        // With an eligible idle slot the pool reclaims first...
        let step = unknown_removable_step(
            &original,
            Some(5 * 1024 * 1024 * 1024),
            Some(2 * 1024 * 1024 * 1024),
            FLOOR,
            true,
        )
        .expect("bounds-only gaps qualify");
        assert_eq!(step, UnknownRemovableStep::Evict);
        // ...and with nothing left to prune it refuses rather than admitting.
        let step = unknown_removable_step(
            &original,
            Some(5 * 1024 * 1024 * 1024),
            Some(2 * 1024 * 1024 * 1024),
            FLOOR,
            false,
        )
        .expect("bounds-only gaps qualify");
        match step {
            UnknownRemovableStep::Refuse(scan) => assert_eq!(scan, original),
            other => panic!("the floor must win over a lower reserve: {other:?}"),
        }
        // Equality at the floor admits with a warning when nothing is left.
        let step = unknown_removable_step(&original, Some(FLOOR), Some(RESERVE), FLOOR, false)
            .expect("bounds-only gaps qualify");
        assert!(matches!(step, UnknownRemovableStep::AdmitAtFloor(_)));
    }

    #[test]
    fn unknown_facts_never_become_zero_and_never_admit() {
        let original = bounds(&[AccountingGapKind::EntryLimit]);
        for (available, reserve) in [
            (None, Some(RESERVE)),
            (Some(RESERVE + 1), None),
            (None, None),
        ] {
            assert!(matches!(
                unknown_removable_step(&original, available, reserve, FLOOR, true),
                Err(LeaseError::AccountingIncomplete(_))
            ));
        }
    }

    #[test]
    fn only_bound_gaps_qualify() {
        for refusing in [
            scan(&[], AccountingCoverage::Complete),
            scan(&[], AccountingCoverage::Partial),
            scan(&[], AccountingCoverage::Unknown),
            bounds(&[AccountingGapKind::UnreadableEntry]),
            bounds(&[AccountingGapKind::MountBoundary]),
            bounds(&[AccountingGapKind::MissingMetadata]),
            bounds(&[
                AccountingGapKind::EntryLimit,
                AccountingGapKind::UnreadableEntry,
            ]),
            bounds(&[AccountingGapKind::RootMissing]),
        ] {
            assert!(!bound_gaps_only(&refusing), "{refusing:?}");
            assert!(matches!(
                unknown_removable_step(&refusing, Some(RESERVE + 1), Some(RESERVE), FLOOR, true),
                Err(LeaseError::AccountingIncomplete(_))
            ));
        }
        assert!(bound_gaps_only(&bounds(&[AccountingGapKind::EntryLimit])));
    }
}

/// Production-path exercises of a removable-backed pool.
///
/// These build a real `Pool` over a tiny owned fixture payload
/// ([`volume_pool_for_test`]) with verified-volume facts supplied by
/// [`crate::volume::verified_facts`], then call the same
/// admission/count/runtime methods production uses. The fake boundary is the
/// whole verification result: with the adapter installed, `Binding::verify()`
/// returns those modelled facts and skips the structural checks it performs in
/// production (`observe()`, the mount-root rule, the device comparison and the
/// mount-point path re-check). Nothing here is evidence that a real volume was
/// verified or that a disk image was used.
#[cfg(test)]
mod removable_pool_path_tests {
    use super::*;
    use crate::volume::verified_facts::{self, VolumeFacts};
    use std::os::unix::fs::PermissionsExt;

    const GIB: u64 = 1024 * 1024 * 1024;
    const CAP: u64 = 64 * 1024 * 1024;
    const RESERVE: u64 = 20 * GIB;
    const FLOOR: u64 = 10 * GIB;

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let parent = base.join("removable-path");
            fs::create_dir_all(&parent).expect("fixture parent");
            let root = parent.join(format!("{label}-{:x}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir(&root).expect("fixture root");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private root");
            Self { root }
        }

        fn pool(&self, reserve: Option<u64>) -> Pool {
            let pool_root = self.root.join("pool");
            let payload = self.root.join("payload");
            for path in [&pool_root, &payload, &payload.join(SLOTS_NAME)] {
                fs::create_dir_all(path).expect("fixture directory");
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                    .expect("private directory");
            }
            volume_pool_for_test(&pool_root, &payload, CAP, reserve).expect("removable pool")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn release_pool_wait_notices_once_only_when_contended() {
        let fixture = Fixture::new("release-wait-notice");
        let path = fixture.root.join("lock");
        let holder = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&path)
            .unwrap();
        let waiter = OpenOptions::new().write(true).open(&path).unwrap();
        // SAFETY: both descriptors remain open and refer to this fixture's lock.
        assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
        let mut calls = 0;
        wait_before_pool_notice(&waiter, Duration::ZERO, || {
            calls += 1;
            // SAFETY: holder owns the fixture lock, and remains open.
            assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_UN) }, 0);
        })
        .unwrap();
        assert_eq!(calls, 1);
        // SAFETY: waiter owns the lock after the wait.
        assert_eq!(unsafe { libc::flock(waiter.as_raw_fd(), libc::LOCK_UN) }, 0);
        wait_before_pool_notice(&waiter, Duration::ZERO, || {
            panic!("uncontended lock noticed")
        })
        .unwrap();
    }

    fn bounds_scan() -> AccountingScan {
        AccountingScan {
            coverage: AccountingCoverage::Partial,
            gaps: vec![AccountingGap {
                kind: AccountingGapKind::EntryLimit,
                occurrences: 1,
            }],
            vanished: 0,
            entries_visited: 12,
            elapsed_ms: 5,
            max_entries: 1,
            max_depth: 66,
            max_elapsed_ms: 60_000,
        }
    }

    fn facts(available: u64) -> VolumeFacts {
        VolumeFacts {
            capacity_bytes: Some(1024 * GIB),
            available_bytes: Some(available),
            verify_error: None,
            available_sequence: Vec::new(),
            verify_error_after: None,
            reads: 0,
        }
    }

    /// A volume whose free space changes between readings, as after a prune.
    fn recovering(readings: &[u64]) -> VolumeFacts {
        VolumeFacts {
            available_sequence: readings.iter().map(|bytes| Some(*bytes)).collect(),
            ..facts(FLOOR)
        }
    }

    fn idle_dir(pool: &mut Pool, id: u64, rank: u64) -> PathBuf {
        let path = add_idle_slot_for_test(pool, id, rank, 1024, 1).expect("idle slot");
        for directory in [path.clone(), path.join("build")] {
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .expect("private slot directory");
        }
        path
    }

    /// The removable reserve route passes the admission's policy to the same
    /// selector, so a one-use admission can never prune a normal slot and the
    /// route's own behaviour is unchanged (design §6.7, §10.2).
    #[test]
    fn the_reserve_route_selects_only_the_admission_policy() {
        let fixture = Fixture::new("reserve-policy");
        let mut pool = fixture.pool(Some(RESERVE));
        let member_slot = idle_dir(&mut pool, 1, 1);
        let normal_slot = idle_dir(&mut pool, 2, 2);
        let member = pool
            .verify_member(&MemberPair::new("1", "1"))
            .expect("the member verifies");
        let only = VictimPlan {
            kind: VictimPlanKind::Only,
            preferred: vec![member.clone()],
            excluded: vec![member.pair],
        };
        // Free space at the floor with a reserve above it: the route prunes
        // while it has an eligible idle slot, then admits at the floor.
        verified_facts::with(recovering(&[FLOOR, FLOOR, FLOOR]), || {
            pool.admit_unknown_removable(&bounds_scan(), &only)
                .expect("admits at the floor");
        });
        assert!(!member_slot.exists(), "the evictable member survived");
        assert_eq!(
            slot_ids_for_test(&pool),
            vec![2],
            "Only selected a normal slot on the reserve route"
        );
        assert!(normal_slot.exists());

        // The non-vacuous control: `Any` takes both idle slots, so the survival
        // above is the policy's doing.
        let fixture = Fixture::new("reserve-policy-any");
        let mut pool = fixture.pool(Some(RESERVE));
        idle_dir(&mut pool, 1, 1);
        idle_dir(&mut pool, 2, 2);
        verified_facts::with(recovering(&[FLOOR, FLOOR, FLOOR]), || {
            pool.admit_unknown_removable(&bounds_scan(), &VictimPlan::any())
                .expect("admits at the floor");
        });
        assert!(slot_ids_for_test(&pool).is_empty());
    }

    #[test]
    fn foreign_volume_usage_never_evicts_a_pool_below_its_cap() {
        let fixture = Fixture::new("foreign");
        let mut pool = fixture.pool(Some(RESERVE));
        let idle = idle_dir(&mut pool, 1, 1);
        let record = pool.slot(1).and_then(|slot| slot.recorded.clone());
        // 1014 GiB of unrelated data on the volume, pool bytes far below cap.
        let facts = facts(10 * GIB);
        verified_facts::with(facts, || {
            let used = pool.recorded_total().expect("complete pool figure");
            assert!(used < CAP, "pool figure {used} is not below the cap");
            // The frozen rule: the cap comparison never sees the volume total.
            // Restoring `max(recorded, volume_used)` would make this evict.
            // Below the cap the byte-pressure branch takes no victim at all:
            // the decision is the comparison, and nothing below it runs.
            assert!(
                used < pool.budget,
                "foreign volume bytes were charged into the pool figure"
            );
            assert!(
                pool.slot(1).is_some(),
                "a pool below its cap evicted an idle slot for foreign volume bytes"
            );
            assert_eq!(pool.slot(1).and_then(|slot| slot.recorded.clone()), record);
            assert!(
                idle.join("build").join("blob-0").is_file(),
                "payload removed"
            );
        });
    }

    #[test]
    fn pool_bytes_over_the_cap_evict_the_lru_idle_slot_and_never_a_held_one() {
        let fixture = Fixture::new("over-cap");
        let mut pool = fixture.pool(Some(RESERVE));
        let older = idle_dir(&mut pool, 1, 1);
        let newer = idle_dir(&mut pool, 2, 2);
        let newest = idle_dir(&mut pool, 3, 3);
        // A held slot with the oldest rank: if it were eligible it would be
        // the first victim, so its survival proves the held exclusion.
        idle_dir(&mut pool, 4, 0);
        hold_slot_for_test(&mut pool, 4, "writer");
        for slot in pool.ledger.slots.iter_mut() {
            if let Some(record) = slot.recorded.as_mut() {
                record.unique_bytes = CAP / 3; // pool bytes now at the cap
            }
        }
        // One eviction decision: a held tree's bytes stay charged until they
        // are positively removed (oxide#190), so the pool-wide loop would keep
        // retiring idle slots. This asserts the victim order of one decision.
        verified_facts::with(facts(10 * GIB), || {
            assert!(
                pool.evict_one_idle_with(&VictimPlan::any())
                    .expect("known-byte eviction"),
                "no victim was selected"
            );
        });
        assert!(!older.exists(), "the rank-LRU idle slot was not evicted");
        assert!(
            newer.exists() && newest.exists(),
            "a newer idle slot was evicted first"
        );
        assert!(pool.slot(4).is_some(), "a held slot was evicted");
    }

    #[test]
    fn the_unknown_route_still_charges_a_pending_eviction() {
        let fixture = Fixture::new("pending-reserve-charge");
        let mut pool = fixture.pool(Some(RESERVE));
        let path = idle_dir(&mut pool, 1, 1);
        let slot = pool.slot_mut(1).expect("slot");
        slot.state = SlotPersistState::Evicting;
        slot.recorded.as_mut().expect("record").unique_bytes = CAP;
        verified_facts::with(facts(RESERVE + GIB), || {
            assert!(matches!(
                pool.admit_unknown_removable(&bounds_scan(), &VictimPlan::any()),
                Err(LeaseError::Full)
            ));
        });
        assert!(path.exists());
        pool.slot_mut(1).expect("slot").recorded = None;
        verified_facts::with(facts(RESERVE + GIB), || {
            pool.admit_unknown_removable(&bounds_scan(), &VictimPlan::any())
                .expect("an unrecorded pending root is not a whole-budget charge");
        });
        assert!(path.exists());
    }

    #[test]
    fn the_unknown_route_prunes_one_idle_slot_then_admits_above_the_reserve() {
        let fixture = Fixture::new("prune-one");
        let mut pool = fixture.pool(Some(RESERVE));
        let victim = idle_dir(&mut pool, 1, 1);
        let mut reads = 0u64;
        verified_facts::with(recovering(&[FLOOR - GIB, RESERVE + GIB]), || {
            // Below the floor with an eligible idle slot: prune it, re-read
            // free space, and admit once it is above the reserve.
            let outcome = pool.admit_unknown_removable(&bounds_scan(), &VictimPlan::any());
            reads = verified_facts::reads();
            outcome.expect("the recovered admission succeeds");
        });
        assert!(
            reads >= 2,
            "free space was not re-read after the prune ({reads})"
        );
        assert!(!victim.exists(), "the eligible idle slot was not pruned");
        assert!(
            slot_ids_for_test(&pool).is_empty(),
            "a second victim was taken"
        );
        let note = pool.admission_note.clone().unwrap_or_default();
        assert!(note.contains("cap is unverified"), "{note}");
        assert!(note.contains("GiB"), "the note must use binary GiB: {note}");
    }

    #[test]
    fn the_unknown_route_refuses_when_nothing_is_left_and_free_is_below_the_floor() {
        let fixture = Fixture::new("exhausted");
        let mut pool = fixture.pool(Some(RESERVE));
        verified_facts::with(facts(FLOOR - GIB), || {
            let scan = bounds_scan();
            match pool.admit_unknown_removable(&scan, &VictimPlan::any()) {
                Err(LeaseError::AccountingIncomplete(original)) => {
                    assert_eq!(*original, scan, "the original scan was not preserved")
                }
                other => panic!("below the floor with nothing to prune must refuse: {other:?}"),
            }
        });
    }

    #[test]
    fn the_unknown_route_admits_with_a_warning_between_the_floor_and_the_reserve() {
        let fixture = Fixture::new("warning");
        let mut pool = fixture.pool(Some(RESERVE));
        idle_dir(&mut pool, 1, 1);
        verified_facts::with(facts(FLOOR), || {
            pool.admit_unknown_removable(&bounds_scan(), &VictimPlan::any())
                .expect("at the floor with nothing left admits");
        });
        let note = pool.admission_note.clone().unwrap_or_default();
        assert!(note.contains("warning"), "{note}");
        assert!(
            note.contains("10.0 GiB"),
            "the floor must print as binary GiB: {note}"
        );
    }

    #[test]
    fn warning_admit_after_exhausting_every_victim_reports_what_it_deleted() {
        let fixture = Fixture::new("warning-victims");
        let mut pool = fixture.pool(Some(RESERVE));
        let first = idle_dir(&mut pool, 1, 1);
        let second = idle_dir(&mut pool, 2, 2);
        // Two victims cannot recover the reserve; the third reading reaches
        // the floor, so the route warns and admits with nothing left.
        verified_facts::with(recovering(&[FLOOR - GIB, FLOOR - GIB, FLOOR]), || {
            pool.admit_unknown_removable(&bounds_scan(), &VictimPlan::any())
                .expect("at the floor with nothing left admits");
        });
        assert!(!first.exists(), "the first LRU victim was not retired");
        assert!(!second.exists(), "the second LRU victim was not retired");
        assert!(
            slot_ids_for_test(&pool).is_empty(),
            "a victim survived the loop"
        );
        let note = pool.admission_note.clone().unwrap_or_default();
        assert!(note.contains("warning"), "{note}");
        assert!(note.contains("no idle slot is left to prune"), "{note}");
        assert!(
            note.contains("GiB"),
            "the figures must be binary GiB: {note}"
        );
    }

    #[test]
    fn the_global_count_cap_proceeds_on_a_removable_bound_only_unknown() {
        let fixture = Fixture::new("global-count");
        let mut pool = fixture.pool(Some(RESERVE));
        let victims = [
            idle_dir(&mut pool, 1, 1),
            idle_dir(&mut pool, 2, 2),
            idle_dir(&mut pool, 3, 3),
        ];
        pool.limits = InventoryLimits {
            max_entries: 1,
            ..accounting_limits()
        };
        verified_facts::with(facts(10 * GIB), || {
            // The pool ceiling path: three slots down to one is count pressure.
            pool.evict_idle_until(1, &VictimPlan::any())
                .expect("bounds-only removable count pressure is allowed");
        });
        assert_eq!(
            slot_ids_for_test(&pool),
            vec![3],
            "the LRU victims were not retired"
        );
        assert!(
            !victims[0].exists() && !victims[1].exists(),
            "victims still on disk"
        );
        assert!(victims[2].exists(), "the newest slot was deleted");
    }

    #[test]
    fn internal_incompleteness_refuses_before_deleting_anything() {
        let fixture = Fixture::new("internal-refuses");
        let pool_root = fixture.root.join("budget-pool");
        fs::create_dir_all(&pool_root).expect("budget pool root");
        fs::set_permissions(&pool_root, fs::Permissions::from_mode(0o700))
            .expect("private pool root");
        create_injected(&pool_root, 64 * 1024 * 1024).expect("budget pool");
        let project = fixture.root.join("project");
        let workspace = fixture.root.join("workspace");
        for path in [&project, &workspace] {
            fs::create_dir_all(path).expect("fixture directory");
        }
        let mut request = LeaseRequest {
            pool: pool_root.clone(),
            project,
            workspace,
            profile: Profile::Dev,
            holder: "fixture".to_owned(),
            budget_bytes: None,
            reserve_bytes: None,
            layout: None,
            replace_slot: None,
            class: AdmissionClass::Normal,
            victims: VictimPolicy::Any,
            slice: None,
        };
        let idle = lease_injected(request.clone()).expect("first slot");
        release_injected(&pool_root, &idle.lease_id).expect("idle slot");
        let slot_path = pool_root
            .join("payload")
            .join(SLOTS_NAME)
            .join(&idle.slot_id);
        // A count replacement on the internal pool must refuse before deletion
        // when its own figure is incomplete.
        request.replace_slot = Some(idle.slot_id.parse().expect("slot id"));
        with_accounting_limits(1, 64, 60_000, || {
            let refused = lease_injected(request.clone());
            assert!(
                matches!(refused, Err(LeaseError::AccountingIncomplete(_))),
                "an internal incompleteness must refuse: {refused:?}"
            );
        });
        assert!(slot_path.exists(), "an internal refusal deleted its victim");
    }

    #[test]
    fn allocate_admits_on_pool_bytes_and_the_volume_total_would_not() {
        let fixture = Fixture::new("allocate-path");
        let mut pool = fixture.pool(Some(RESERVE));
        let idle = idle_dir(&mut pool, 1, 1);
        let project = fixture.root.join("project");
        let workspace = fixture.root.join("workspace");
        for path in [&project, &workspace] {
            fs::create_dir_all(path).expect("fixture directory");
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .expect("private directory");
        }
        let request = LeaseRequest {
            pool: fixture.root.join("pool"),
            project,
            workspace,
            profile: Profile::Dev,
            holder: "fixture".to_owned(),
            budget_bytes: None,
            reserve_bytes: Some(RESERVE),
            layout: None,
            replace_slot: None,
            class: AdmissionClass::Normal,
            victims: VictimPolicy::Any,
            slice: None,
        };
        // 1014 GiB of unrelated volume usage against a 64 MiB pool cap.
        verified_facts::with(facts(10 * GIB), || {
            let recorded = pool.recorded_total().expect("complete pool figure");
            let volume_used = 1024 * GIB - 10 * GIB;
            assert!(recorded < CAP, "the pool is not below its cap: {recorded}");
            // Executable counterfactual: the retired rule would have taken the
            // larger figure, which is far above the cap, and evicted.
            assert!(
                recorded.max(volume_used) >= CAP,
                "the counterfactual cannot distinguish the two rules"
            );
            let granted = pool.lease(&request).expect("the real admission succeeds");
            assert!(!granted.grant.reused_slot, "a new slot was expected");
        });
        assert!(
            idle.exists(),
            "a pool below its cap evicted an idle slot for foreign volume bytes"
        );
        assert!(pool.slot(1).is_some(), "the idle slot left the ledger");
        assert!(
            pool.slot(1)
                .and_then(|slot| slot.recorded.as_ref())
                .is_some(),
            "the idle record was dropped"
        );
    }

    #[test]
    fn a_failed_verification_after_a_prune_stops_the_route() {
        let fixture = Fixture::new("verify-fail");
        let mut pool = fixture.pool(Some(RESERVE));
        idle_dir(&mut pool, 1, 1);
        let mut read = 0u64;
        let mut broken = recovering(&[FLOOR - GIB, FLOOR - GIB]);
        broken.verify_error_after = Some(1);
        verified_facts::with(broken, || {
            let outcome = pool.admit_unknown_removable(&bounds_scan(), &VictimPlan::any());
            read = verified_facts::reads();
            assert!(
                matches!(outcome, Err(LeaseError::BackingMismatch)),
                "a failed verification after a prune must stop: {outcome:?}"
            );
        });
        assert!(read >= 1);
    }

    #[test]
    fn count_replacement_uses_count_pressure_only() {
        let fixture = Fixture::new("count");
        let mut pool = fixture.pool(Some(RESERVE));
        let victim = idle_dir(&mut pool, 1, 1);
        idle_dir(&mut pool, 2, 2);
        hold_slot_for_test(&mut pool, 2, "writer");
        // A tiny entry bound makes the pool figure genuinely unknown for a
        // bound gap, which is the only shape the count gate may allow.
        pool.limits = InventoryLimits {
            max_entries: 1,
            ..accounting_limits()
        };
        with_accounting_limits(1, 64, 60_000, || {
            verified_facts::with(facts(10 * GIB), || {
                pool.pressure_known_or_bounded_removable()
                    .expect("bounds-only removable count pressure is allowed");
                pool.replace_idle_slot_for_admission(1, &VictimPlan::any())
                    .expect("the count replacement proceeds");
            });
        });
        assert!(
            !victim.exists(),
            "the count replacement did not retire its victim"
        );
        assert!(pool.slot(2).is_some(), "the held slot was touched");
        // The same incomplete figure must not authorise a byte-pressure
        // deletion on the known-usage path.
        with_accounting_limits(1, 64, 60_000, || {
            // The byte-pressure figure is unknown, so no victim may be chosen
            // from it: the one-victim decision refuses with the incomplete scan.
            let outcome = pool.pressure();
            assert!(
                matches!(outcome, Err(LeaseError::AccountingIncomplete(_))),
                "byte pressure on an unknown removable figure must refuse: {outcome:?}"
            );
        });
        assert!(
            pool.slot(2).is_some(),
            "an unknown figure deleted a held slot"
        );
    }
}

/// The recorded-accounting fence and record representation (oxide#146), tested
/// on the real validators rather than through a decision mock.
#[cfg(test)]
mod recorded_ledger_tests {
    use super::{validate_ledger_read, Ledger, RecordedSize, SlotPersistState};
    use crate::lease::LeaseError;
    use crate::lease::{
        Profile, CAPACITY_POLICY_VERSION, LEASE_VERSION, LEGACY_CAPACITY_POLICY_VERSION,
        LEGACY_MAX_SLOTS, RAISED_CAPACITY_POLICY_VERSION,
    };
    use std::path::PathBuf;

    fn record(for_lease: &str) -> Option<RecordedSize> {
        Some(RecordedSize {
            for_lease: for_lease.to_owned(),
            unique_bytes: 4096,
            logical_bytes: 4096,
            entries: 1,
            at_ms: 1,
        })
    }

    fn ledger(policy: Option<u32>, rank: u64, state: SlotPersistState, recorded: bool) -> Ledger {
        Ledger {
            version: LEASE_VERSION,
            store_id: "a".repeat(32),
            cap_bytes: 1024 * 1024,
            next_slot: 2,
            next_lease: 2,
            capacity_policy: policy,
            next_rank: Some(rank + 1),
            holding: Vec::new(),
            slots: vec![super::SlotRecord {
                id: 1,
                incarnation: 1,
                state,
                project: PathBuf::from("/p"),
                workspace: PathBuf::from("/w"),
                profile: Profile::Dev,
                holder: "h".to_owned(),
                lease_id: None,
                last_lease_id: "v1.a.1.1.1".to_owned(),
                dir: None,
                rank,
                recorded: recorded.then(|| record("v1.a.1.1.1").expect("record")),
                capture: None,
                hold_intent: None,
            }],
        }
    }

    #[test]
    fn the_fence_value_and_record_placement_are_enforced() {
        // Version 1 without a record reads; with one it is a state no writer
        // produces and refuses.
        assert!(validate_ledger_read(&ledger(
            Some(LEGACY_CAPACITY_POLICY_VERSION),
            1,
            SlotPersistState::Idle,
            false
        ))
        .is_ok());
        assert!(validate_ledger_read(&ledger(
            Some(LEGACY_CAPACITY_POLICY_VERSION),
            1,
            SlotPersistState::Idle,
            true
        ))
        .is_err());
        // Version 2 carries a closed record on an idle or evicting slot.
        assert!(validate_ledger_read(&ledger(
            Some(CAPACITY_POLICY_VERSION),
            1,
            SlotPersistState::Idle,
            true
        ))
        .is_ok());
        assert!(validate_ledger_read(&ledger(
            Some(CAPACITY_POLICY_VERSION),
            1,
            SlotPersistState::Held,
            true
        ))
        .is_err());
        assert!(validate_ledger_read(&ledger(
            Some(CAPACITY_POLICY_VERSION),
            1,
            SlotPersistState::Evicting,
            true
        ))
        .is_ok());
        // The raised-ceiling fence (oxide#177, policy 4) carries the same
        // record shape, including the version-3 retained record on an
        // eviction, and stays readable when a pool shrinks back below the
        // legacy ceiling.
        assert!(validate_ledger_read(&ledger(
            Some(RAISED_CAPACITY_POLICY_VERSION),
            1,
            SlotPersistState::Idle,
            true
        ))
        .is_ok());
        assert!(validate_ledger_read(&ledger(
            Some(RAISED_CAPACITY_POLICY_VERSION),
            1,
            SlotPersistState::Evicting,
            true
        ))
        .is_ok());
        // An unknown fence value refuses, and a legacy ledger holding a record
        // refuses too.
        assert!(validate_ledger_read(&ledger(Some(5), 1, SlotPersistState::Idle, false)).is_err());
        assert!(validate_ledger_read(&ledger(None, 0, SlotPersistState::Idle, true)).is_err());
    }

    /// A slot table above the legacy ceiling always carries the raised fence
    /// (oxide#177). Without it the ledger is a state no writer produces, so a
    /// reader that would not recognise the fence cannot silently accept a
    /// 256-slot table.
    #[test]
    fn a_slot_table_above_the_legacy_ceiling_requires_the_raised_fence() {
        let mut fenced = ledger(
            Some(RAISED_CAPACITY_POLICY_VERSION),
            1,
            SlotPersistState::Idle,
            false,
        );
        fenced.slots = (1..=LEGACY_MAX_SLOTS as u64 + 1)
            .map(|id| {
                let mut slot = fenced.slots[0].clone();
                slot.id = id;
                slot.rank = id;
                slot.last_lease_id = format!("v1.a.{id}.1.1");
                slot
            })
            .collect();
        fenced.next_slot = LEGACY_MAX_SLOTS as u64 + 2;
        fenced.next_rank = Some(LEGACY_MAX_SLOTS as u64 + 2);
        assert!(validate_ledger_read(&fenced).is_ok());

        // The same table under the version-2 fence is refused on read, so an
        // older binary's stricter check is never the only guard.
        let mut unfenced = fenced.clone();
        unfenced.capacity_policy = Some(CAPACITY_POLICY_VERSION);
        assert_eq!(
            validate_ledger_read(&unfenced).unwrap_err(),
            crate::lease::LeaseError::UnknownAccounting
        );
        // Exactly the legacy ceiling still reads under the version-2 fence.
        let mut at_ceiling = unfenced;
        at_ceiling.slots.pop();
        assert!(validate_ledger_read(&at_ceiling).is_ok());
    }

    /// Journal name fields are bounded for the reader as well as the writer
    /// (oxide#177): a name over `PATH_MAX`, a component over `NAME_MAX`, a
    /// capture private name longer than its writer form and an over-long
    /// mismatched-capture description all refuse, and the writer refuses such
    /// an object per object without deleting it.
    #[test]
    fn journaled_names_are_bounded_on_read() {
        use crate::owned_delete::{
            CaptureRecord, ForeignCapture, RootCapturePhase, RootCapturePlan,
        };

        let private = format!("{}{}", crate::owned_delete::CAPTURE_PREFIX, "a".repeat(32));
        let root = RootCapturePlan {
            private: private.clone(),
            phase: RootCapturePhase::Captured,
            device: 1,
            inode: 2,
            parent_device: 1,
            parent_inode: 3,
            mode: 0o040777,
        };
        assert!(root.is_valid());
        let capture = |original: String| CaptureRecord {
            path: vec![private.clone()],
            private: private.clone(),
            original,
            device: 1,
            inode: 2,
            mode: 0o100777,
        };
        let foreign = |original: String, found: String| ForeignCapture {
            path: vec![private.clone()],
            private: private.clone(),
            original,
            expected_device: 1,
            expected_inode: 2,
            expected_mode: 0o100777,
            found,
            restored: false,
        };
        let journal = |captures, foreign| super::DeletionJournal {
            root: root.clone(),
            captures,
            foreign,
        };
        let mut accepted = ledger(None, 0, SlotPersistState::Evicting, false);
        accepted.capacity_policy = Some(CAPACITY_POLICY_VERSION);
        accepted.next_rank = Some(1);
        accepted.slots[0].capture = Some(journal(
            vec![capture("observed name".to_owned())],
            vec![foreign(
                "observed name".to_owned(),
                "regular file".to_owned(),
            )],
        ));
        assert!(validate_ledger_read(&accepted).is_ok());

        // PATH_MAX bytes with one component over NAME_MAX; more than PATH_MAX
        // in NAME_MAX components; a component one byte over NAME_MAX.
        let over_name = "x".repeat(1024);
        let over_path = vec!["a".repeat(255); 5].join("/");
        let over_component = format!("{}/{}", "a".repeat(256), "b");
        assert!(over_path.len() > 1024 && over_component.len() <= 1024);
        for original in [over_name.clone(), over_path.clone(), over_component.clone()] {
            assert!(!capture(original).is_valid());
        }
        for bad in [
            foreign(over_name.clone(), "regular file".to_owned()),
            foreign("observed name".to_owned(), over_name.clone()),
            foreign("observed name".to_owned(), over_path.clone()),
        ] {
            assert!(!bad.is_valid());
        }
        let mut long_private = capture("observed name".to_owned());
        long_private.private = format!("{}{}", crate::owned_delete::CAPTURE_PREFIX, "a".repeat(48));
        assert!(!long_private.is_valid());
        let mut long_foreign_private =
            foreign("observed name".to_owned(), "regular file".to_owned());
        long_foreign_private.private =
            format!("{}{}", crate::owned_delete::CAPTURE_PREFIX, "a".repeat(48));
        assert!(!long_foreign_private.is_valid());

        // The whole journal refuses on read with any over-bound member.
        let mut refused = ledger(None, 0, SlotPersistState::Evicting, false);
        refused.capacity_policy = Some(CAPACITY_POLICY_VERSION);
        refused.next_rank = Some(1);
        refused.slots[0].capture = Some(journal(vec![capture(over_name)], Vec::new()));
        assert!(validate_ledger_read(&refused).is_err());
    }

    /// A root-capture record belongs to a recorded deletion and nowhere else,
    /// and only in the shape the seam writes: a ledger holding any other
    /// placement or a near-miss name is a state no writer produces and refuses
    /// rather than being acted on.
    #[test]
    fn a_capture_record_belongs_only_to_a_recorded_deletion() {
        let plan = |private: &str, mode: u32| crate::owned_delete::RootCapturePlan {
            private: private.to_owned(),
            phase: crate::owned_delete::RootCapturePhase::Captured,
            device: 1,
            inode: 2,
            parent_device: 1,
            parent_inode: 3,
            mode,
        };
        let mut evicting = ledger(None, 0, SlotPersistState::Evicting, false);
        evicting.capacity_policy = Some(CAPACITY_POLICY_VERSION);
        evicting.next_rank = Some(1);
        let written = plan(
            &format!("{}{}", crate::owned_delete::CAPTURE_PREFIX, "0".repeat(32)),
            0o040500,
        );
        assert!(written.is_valid(), "the recorded shape is a writer's shape");
        let journal = |root: crate::owned_delete::RootCapturePlan| super::DeletionJournal {
            root,
            captures: Vec::new(),
            foreign: Vec::new(),
        };
        evicting.slots[0].capture = Some(journal(written.clone()));
        assert!(validate_ledger_read(&evicting).is_ok());

        // The same record on an idle or held slot refuses.
        for state in [SlotPersistState::Idle, SlotPersistState::Held] {
            let mut other = ledger(None, 0, state, false);
            other.capacity_policy = Some(CAPACITY_POLICY_VERSION);
            other.next_rank = Some(1);
            other.slots[0].capture = Some(journal(written.clone()));
            assert!(validate_ledger_read(&other).is_err());
        }
        // A near-miss name, a missing random tag and a non-directory mode all
        // refuse.
        for bad in [
            plan(".DS_Store", 0o040500),
            plan(crate::owned_delete::CAPTURE_PREFIX, 0o040500),
            plan(
                &format!("{}{}", crate::owned_delete::CAPTURE_PREFIX, "0".repeat(32)),
                0o100500,
            ),
        ] {
            let mut refused = ledger(None, 0, SlotPersistState::Evicting, false);
            refused.capacity_policy = Some(CAPACITY_POLICY_VERSION);
            refused.next_rank = Some(1);
            refused.slots[0].capture = Some(journal(bad));
            assert!(validate_ledger_read(&refused).is_err());
        }
    }
}

/// Production-path exercises of the one-use slice protocol
/// (`one-use-workspaces` §6.2-§6.9). These build a real budget-only pool over
/// its own tiny fixture payload and drive the same `restore`, selection and
/// admission methods production uses. The feature is dormant: every one-use
/// request here comes from a unit test, never from a production command.
#[cfg(test)]
mod one_use_slice_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct SliceFixture {
        root: PathBuf,
    }

    impl SliceFixture {
        fn new(label: &str) -> Self {
            let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let parent = base.join("one-use-slice");
            fs::create_dir_all(&parent).expect("fixture parent");
            let root = parent.join(format!("{label}-{:x}-{}", std::process::id(), now_ms()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir(&root).expect("fixture root");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .expect("private fixture root");
            Self { root }
        }

        fn pool_root(&self, name: &str) -> PathBuf {
            let path = self.root.join(name);
            fs::create_dir(&path).expect("pool root");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .expect("private pool root");
            path
        }

        fn pool(&self, name: &str, cap: u64) -> (PathBuf, Pool) {
            let root = self.pool_root(name);
            create_injected(&root, cap).expect("budget pool");
            let pool = Pool::open(&root).expect("open budget pool");
            (root, pool)
        }

        fn dir(&self, name: &str) -> PathBuf {
            let path = self.root.join(name);
            fs::create_dir(&path).expect("fixture dir");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .expect("private fixture dir");
            path
        }
    }

    impl Drop for SliceFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn idle(pool: &mut Pool, id: u64, rank: u64, files: usize) -> PathBuf {
        let path = add_idle_slot_for_test(pool, id, rank, 1024, files).expect("idle slot");
        for directory in [path.clone(), path.join("build")] {
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .expect("private slot directory");
        }
        path
    }

    fn pair(id: u64) -> MemberPair {
        MemberPair::new(id.to_string(), "1")
    }

    fn members(pool: &Pool, ids: &[u64]) -> Vec<VerifiedMember> {
        ids.iter()
            .map(|id| pool.verify_member(&pair(*id)).expect("verified member"))
            .collect()
    }

    fn plan(kind: VictimPlanKind, preferred: &[u64], excluded: &[u64]) -> VictimPlan {
        VictimPlan {
            kind,
            preferred: preferred
                .iter()
                .map(|id| VerifiedMember {
                    id: *id,
                    incarnation: 1,
                    pair: pair(*id),
                })
                .collect(),
            excluded: excluded.iter().map(|id| pair(*id)).collect(),
        }
    }

    fn slice(ids: &[u64], kind: SliceAdmission, max_slots: usize, max_bytes: u64) -> OneUseSlice {
        OneUseSlice {
            members: MemberSet::new(ids.iter().map(|id| pair(*id))),
            max_slots,
            max_bytes,
            kind,
            unknown: 0,
        }
    }

    fn ids(pool: &Pool) -> Vec<u64> {
        slot_ids_for_test(pool)
    }

    #[allow(clippy::too_many_arguments)]
    fn request(
        pool: &Path,
        project: &Path,
        workspace: &Path,
        holder: &str,
        class: AdmissionClass,
        victims: VictimPolicy,
        slice: Option<OneUseSlice>,
    ) -> LeaseRequest {
        LeaseRequest {
            pool: pool.to_path_buf(),
            project: project.to_path_buf(),
            workspace: workspace.to_path_buf(),
            profile: Profile::Dev,
            holder: holder.to_owned(),
            budget_bytes: None,
            reserve_bytes: None,
            layout: None,
            replace_slot: None,
            class,
            victims,
            slice,
        }
    }

    fn preferred(ids: &[u64]) -> VictimPolicy {
        VictimPolicy::Only {
            preferred: MemberSet::new(ids.iter().map(|id| pair(*id))),
        }
    }

    /// A new one-use slot at the member ceiling reclaims the oldest idle
    /// evictable member and then admits (§6.3, §6.5, §6.9).
    #[test]
    fn a_new_one_use_slot_reclaims_the_oldest_member_and_admits() {
        let fixture = SliceFixture::new("threshold");
        let (root, mut pool) = fixture.pool("pool", 1 << 30);
        for id in 1..=16 {
            idle(&mut pool, id, id, 0);
        }
        let project = fixture.dir("project");
        let workspace = fixture.dir("workspace");
        let report = pool
            .restore(
                &slice(
                    &(1..=16).collect::<Vec<_>>(),
                    SliceAdmission::NewSlot,
                    16,
                    1 << 30,
                ),
                &plan(VictimPlanKind::Only, &(1..=16).collect::<Vec<_>>(), &[]),
                AdmissionClass::OneUse,
                None,
            )
            .expect("restore");
        assert_eq!(report.members, 15, "restore did not reclaim one place");
        assert!(matches!(report.outcome, RestoreOutcome::Within { .. }));
        assert_eq!(ids(&pool), (2..=16).collect::<Vec<_>>());
        assert_eq!(report.protected, 0);

        // The same threshold through a real admission: one reclaimed place is
        // used by the new slot.
        let leased = pool
            .lease(&request(
                &root,
                &project,
                &workspace,
                "slice-new",
                AdmissionClass::OneUse,
                preferred(&(2..=16).collect::<Vec<_>>()),
                Some(slice(
                    &(2..=16).collect::<Vec<_>>(),
                    SliceAdmission::NewSlot,
                    16,
                    1 << 30,
                )),
            ))
            .expect("the new one-use slot admits");
        assert!(!leased.grant.reused_slot);
        let report = leased.slice.expect("a one-use admission always reports");
        assert_eq!(report.members, 15);
        assert_eq!(ids(&pool).len(), 16);
    }

    /// A warm reuse at the ceiling adds no slot, so it admits with all 16
    /// members while the bytes fit (§6.1, §6.6).
    #[test]
    fn a_warm_reuse_at_the_member_ceiling_admits_without_eviction() {
        let fixture = SliceFixture::new("warm");
        let (_root, mut pool) = fixture.pool("pool", 1 << 30);
        for id in 1..=16 {
            idle(&mut pool, id, id, 0);
        }
        let report = pool
            .restore(
                &slice(
                    &(1..=16).collect::<Vec<_>>(),
                    SliceAdmission::WarmReuse,
                    16,
                    1 << 30,
                ),
                &plan(VictimPlanKind::Only, &(1..=16).collect::<Vec<_>>(), &[]),
                AdmissionClass::OneUse,
                Some((1, 1)),
            )
            .expect("restore");
        assert_eq!(report.members, 16);
        assert_eq!(report.protected, 1, "the reuse candidate is not a victim");
        assert!(matches!(report.outcome, RestoreOutcome::Within { .. }));
        assert_eq!(ids(&pool), (1..=16).collect::<Vec<_>>());
    }

    /// A warm reuse whose only remaining eligible victim is the reuse
    /// candidate itself is full, and the candidate keeps its bytes (§6.3).
    #[test]
    fn a_warm_reuse_walled_in_by_its_own_candidate_is_full() {
        let fixture = SliceFixture::new("reuse-full");
        let (_root, mut pool) = fixture.pool("pool", 1 << 30);
        idle(&mut pool, 1, 1, 1);
        let report = pool
            .restore(
                &slice(&[1], SliceAdmission::WarmReuse, 16, 1),
                &plan(VictimPlanKind::Only, &[1], &[]),
                AdmissionClass::OneUse,
                Some((1, 1)),
            )
            .expect("restore");
        match report.outcome {
            RestoreOutcome::Full { bytes, held } => {
                assert!(bytes > 1);
                assert_eq!(held, 1);
            }
            other => panic!("expected Full, got {other:?}"),
        }
        assert_eq!(ids(&pool), vec![1], "the reuse candidate was deleted");
    }

    /// A protected member is counted and never a victim, even when it is the
    /// only member left over the bound (§5.5.2, §6.7).
    #[test]
    fn a_protected_member_is_counted_and_never_a_victim() {
        let fixture = SliceFixture::new("protected");
        let (_root, mut pool) = fixture.pool("pool", 1 << 30);
        idle(&mut pool, 1, 1, 1);
        idle(&mut pool, 2, 2, 1);
        let report = pool
            .restore(
                &slice(&[1, 2], SliceAdmission::NewSlot, 16, 1),
                &plan(VictimPlanKind::Only, &[2], &[1, 2]),
                AdmissionClass::OneUse,
                None,
            )
            .expect("restore");
        match report.outcome {
            RestoreOutcome::Full { held, .. } => assert_eq!(held, 1),
            other => panic!("expected Full, got {other:?}"),
        }
        assert_eq!(
            ids(&pool),
            vec![1],
            "the evictable member was not reclaimed"
        );
    }

    /// The normal `Prefer` fallback subtracts every verified one-use pair, so
    /// it can never pick a one-use member — protected or not (§6.7).
    #[test]
    fn the_normal_fallback_never_selects_a_one_use_member() {
        let fixture = SliceFixture::new("fallback");
        let (_root, mut pool) = fixture.pool("pool", 1 << 30);
        idle(&mut pool, 1, 1, 0);
        idle(&mut pool, 2, 2, 0);
        let prefer = plan(VictimPlanKind::Prefer, &[], &[1]);
        assert!(pool.evict_one_idle_with(&prefer).expect("victim"));
        assert_eq!(ids(&pool), vec![1], "the fallback took a one-use member");
        assert!(
            !pool.evict_one_idle_with(&prefer).expect("no victim"),
            "the fallback fell through to a one-use member"
        );
    }

    /// `Only` selects an evictable member and never a normal slot (§6.7).
    #[test]
    fn only_never_selects_a_normal_slot() {
        let fixture = SliceFixture::new("only");
        let (_root, mut pool) = fixture.pool("pool", 1 << 30);
        idle(&mut pool, 1, 1, 0);
        idle(&mut pool, 2, 2, 0);
        let only = plan(VictimPlanKind::Only, &[1], &[1]);
        assert!(pool.evict_one_idle_with(&only).expect("victim"));
        assert_eq!(
            ids(&pool),
            vec![2],
            "Only did not take the evictable member"
        );
        assert!(
            !pool.evict_one_idle_with(&only).expect("no victim"),
            "Only fell through to a normal slot"
        );
    }

    /// A cross-member hard link is counted once by the union walk (§6.4).
    #[test]
    fn the_union_counts_a_cross_member_hard_link_once() {
        let fixture = SliceFixture::new("union");
        let (_root, mut pool) = fixture.pool("pool", 1 << 30);
        let first = idle(&mut pool, 1, 1, 1);
        let second = idle(&mut pool, 2, 2, 0);
        fs::hard_link(
            first.join("build").join("blob-0"),
            second.join("build").join("linked"),
        )
        .expect("hard link");
        fs::write(second.join("build").join("unique"), vec![b'y'; 32 * 1024]).expect("unique file");
        let one = pool.member_union(&members(&pool, &[1])).expect("slot 1");
        let two = pool.member_union(&members(&pool, &[2])).expect("slot 2");
        let union = pool.member_union(&members(&pool, &[1, 2])).expect("union");
        assert!(one > 0 && two > 0, "the fixture files are empty");
        assert_eq!(union, two, "the union is not the second root's total");
        assert!(
            one + two > union,
            "summing independent walks did not overcount the hard link"
        );
    }

    /// Class-unknown rows make the slice a lower bound and restore reclaims
    /// nothing; a one-use admission refuses (§5.6, §6.9).
    #[test]
    fn unknown_rows_are_a_lower_bound_and_never_reclaimed() {
        let fixture = SliceFixture::new("unknown");
        let (root, mut pool) = fixture.pool("pool", 1 << 30);
        idle(&mut pool, 1, 1, 1);
        let mut slice = slice(&[1], SliceAdmission::NewSlot, 16, 1);
        slice.unknown = 2;
        let report = pool
            .restore(
                &slice,
                &plan(VictimPlanKind::Only, &[1], &[1]),
                AdmissionClass::OneUse,
                None,
            )
            .expect("restore");
        match report.outcome {
            RestoreOutcome::LowerBound { unknown, .. } => assert_eq!(unknown, 2),
            other => panic!("expected a lower bound, got {other:?}"),
        }
        assert_eq!(ids(&pool), vec![1], "an unprovable bound deleted a member");

        let project = fixture.dir("project");
        let workspace = fixture.dir("workspace");
        let refused = pool.lease(&request(
            &root,
            &project,
            &workspace,
            "slice-unknown",
            AdmissionClass::OneUse,
            preferred(&[1]),
            Some(slice),
        ));
        assert!(
            matches!(refused, Err(LeaseError::UnknownAccounting)),
            "a one-use admission admitted beside class-unknown rows: {refused:?}"
        );
        assert_eq!(ids(&pool), vec![1]);
    }

    /// An accounting pair that does not verify is dropped and reported, never
    /// replaced by another slot (§6.2).
    #[test]
    fn an_unverified_pair_is_dropped_and_never_replaced() {
        let fixture = SliceFixture::new("unverified");
        let (_root, mut pool) = fixture.pool("pool", 1 << 30);
        idle(&mut pool, 1, 1, 0);
        let mut request = slice(&[1], SliceAdmission::NewSlot, 16, 1 << 30);
        request.members = MemberSet::new([pair(1), MemberPair::new("99", "1")]);
        let report = pool
            .restore(
                &request,
                &plan(VictimPlanKind::Only, &[1], &[1]),
                AdmissionClass::Normal,
                None,
            )
            .expect("restore");
        assert_eq!(report.members, 1);
        assert_eq!(report.unverified, 1);
        assert!(matches!(report.outcome, RestoreOutcome::Within { .. }));
        assert_eq!(ids(&pool), vec![1]);
    }

    /// A one-use new-slot admission with only protected members refuses with
    /// the dedicated `one_use_budget` error and deletes nothing (§6.9).
    #[test]
    fn a_full_one_use_slice_refuses_with_one_use_budget() {
        let fixture = SliceFixture::new("one-use-budget");
        let (root, mut pool) = fixture.pool("pool", 1 << 30);
        let slot = idle(&mut pool, 1, 1, 1);
        let project = fixture.dir("project");
        let workspace = fixture.dir("workspace");
        let refused = pool.lease(&request(
            &root,
            &project,
            &workspace,
            "slice-full",
            AdmissionClass::OneUse,
            preferred(&[]),
            Some(slice(&[1], SliceAdmission::NewSlot, 16, 1)),
        ));
        match refused {
            Err(error @ LeaseError::OneUseBudget { measured, limit }) => {
                assert!(measured.is_some_and(|bytes| bytes > limit));
                assert_eq!(limit, 1);
                assert_eq!(error.reason(), "one_use_budget");
                assert_eq!(error.exit_code(), 3);
                assert!(!error.to_string().contains("pool cap"), "{error}");
            }
            other => panic!("expected one_use_budget, got {other:?}"),
        }
        assert!(slot.exists(), "a refusal deleted the protected member");
        assert_eq!(ids(&pool), vec![1]);
    }

    /// A one-use warm reuse must land on a verified member: a non-member idle
    /// slot is refused, never relabelled (§6.6).
    #[test]
    fn a_one_use_warm_reuse_of_a_non_member_is_refused() {
        let fixture = SliceFixture::new("reuse-member");
        let (root, mut pool) = fixture.pool("pool", 1 << 30);
        let slot = idle(&mut pool, 1, 1, 0);
        // The idle slot matches the request's project and profile, so the
        // library would reuse it; it is deliberately absent from the member
        // set.
        let project = fixture.dir("project");
        if let Some(record) = pool.slot_mut(1) {
            record.project = project.clone();
        }
        let workspace = fixture.dir("workspace");
        let refused = pool.lease(&request(
            &root,
            &project,
            &workspace,
            "slice-reuse",
            AdmissionClass::OneUse,
            preferred(&[]),
            Some(slice(&[], SliceAdmission::WarmReuse, 16, 1 << 30)),
        ));
        assert!(
            matches!(refused, Err(LeaseError::UnknownAccounting)),
            "a non-member warm reuse was not refused: {refused:?}"
        );
        assert_eq!(ids(&pool), vec![1]);
        assert!(slot.exists());
    }

    /// Dormant byte identity: a normal admission that carries today's request
    /// shape and one that carries the new empty-member shape produce the same
    /// ledger bytes, grant and slot tree. The two pools are byte-identical
    /// copies made before either admission, so the store ID and lease IDs
    /// match and no field can differ by accident.
    #[test]
    fn a_dormant_normal_admission_is_byte_identical_to_the_shipped_shape() {
        fn copy_tree(from: &Path, to: &Path) {
            fs::create_dir_all(to).expect("copy dir");
            let metadata = fs::symlink_metadata(from).expect("source metadata");
            fs::set_permissions(to, metadata.permissions()).expect("copy mode");
            for entry in fs::read_dir(from).expect("read source") {
                let entry = entry.expect("entry");
                let target = to.join(entry.file_name());
                let kind = entry.file_type().expect("entry type");
                if kind.is_dir() {
                    copy_tree(&entry.path(), &target);
                } else {
                    fs::copy(entry.path(), &target).expect("copy file");
                }
            }
        }

        let fixture = SliceFixture::new("dormant");
        let source = fixture.pool_root("source");
        create_injected(&source, 1 << 30).expect("source pool");
        let shipped = fixture.root.join("shipped");
        let candidate = fixture.root.join("candidate");
        copy_tree(&source, &shipped);
        copy_tree(&source, &candidate);
        let project = fixture.dir("project");
        let workspace = fixture.dir("workspace");

        let mut shipped_pool = Pool::open(&shipped).expect("shipped pool");
        let shipped_grant = shipped_pool
            .lease(&request(
                &shipped,
                &project,
                &workspace,
                "dormant",
                AdmissionClass::Normal,
                VictimPolicy::Any,
                None,
            ))
            .expect("shipped-shape admission");
        let mut candidate_pool = Pool::open(&candidate).expect("candidate pool");
        let candidate_grant = candidate_pool
            .lease(&request(
                &candidate,
                &project,
                &workspace,
                "dormant",
                AdmissionClass::Normal,
                VictimPolicy::Prefer {
                    preferred: MemberSet::empty(),
                },
                Some(OneUseSlice::bounded(
                    MemberSet::empty(),
                    SliceAdmission::NewSlot,
                    0,
                    1 << 30,
                )),
            ))
            .expect("empty-member admission");
        assert_eq!(
            fs::read(shipped.join(LEDGER_NAME)).expect("shipped ledger"),
            fs::read(candidate.join(LEDGER_NAME)).expect("candidate ledger"),
            "the dormant admission changed the ledger bytes"
        );
        // The pool root itself differs between the two copies; strip it so the
        // comparison is about the admission, not the fixture path.
        let shape = |grant: &LeaseGrant, root: &Path| {
            (
                grant.slot_id.clone(),
                grant.incarnation.clone(),
                grant.reused_slot,
                grant
                    .env
                    .cargo_build_build_dir
                    .strip_prefix(root)
                    .map(Path::to_path_buf),
                grant.env.cargo_target_dir.clone(),
            )
        };
        assert_eq!(
            shape(&shipped_grant.grant, &shipped),
            shape(&candidate_grant.grant, &candidate)
        );
        assert!(
            shipped_grant.slice.is_none(),
            "a shipped shape has no slice"
        );
        assert_eq!(
            candidate_grant.slice.as_ref().map(|report| &report.outcome),
            Some(&RestoreOutcome::NotNeeded),
            "an empty member set must cost no walk and no write"
        );
    }
}
