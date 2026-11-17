//! Exclusive pool: short-lived flock, durable JSON ledger, idle-only eviction.

use super::backing::BackingPaths;
use super::slice::{
    AdmissionClass, MemberPair, OneUseSlice, RestoreOutcome, SliceAdmission, SliceReport,
    VictimPolicy,
};
// Only the admission-policy tests build a `MemberSet` here; release code reaches
// one through `LeaseRequest`.
#[cfg(test)]
use super::slice::MemberSet;
use super::{
    canonical_existing_dir, current_uid, ensure_private_dir, io_error, is_finder_metadata,
    is_tolerated_metadata_file, open_verified_directory, persist_private_file, random_hex,
    read_private_file, unexpected_entry, unlink_at, AccountingContext, AccountingCoverage,
    AccountingGap, AccountingGapKind, AccountingScan, AccountingWalk, HeldEvictionFailure,
    LeaseEnvironment, LeaseError, LeaseGrant, Leased, LedgerSlot, LedgerView, MeasuredPool,
    PassiveSnapshot, PoolStatus, Profile, RecordedFact, RecordedVolume, ReleaseReceipt,
    ReuseMeasurement, SlotOccupancy, SlotStatus, UnmeasuredSlot, ACCOUNTING_MAX_DEPTH,
    ACCOUNTING_MAX_ELAPSED_MS, ACCOUNTING_MAX_ENTRIES, CAPACITY_POLICY_VERSION, CONFIG_NAME,
    LEASE_VERSION, LEDGER_NAME, LEGACY_CAPACITY_POLICY_VERSION, LEGACY_MAX_SLOTS, LOCK_NAME,
    MANAGED_NAME, MAX_HOLDER_BYTES, MAX_LEASE_ID_BYTES, MAX_PATH_BYTES, MAX_SLOTS,
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
use barge::Hold;
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

/// A stable Barge root id for an existing Oxide slot folder: a hex digest of
/// the recorded folder, so the same folder always adopts to the same identity
/// and a repeat adoption is idempotent.
fn legacy_root_id(folder: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(folder.as_bytes());
    digest[..8].iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The Barge label for a new slot root: the project folder and profile
/// squeezed into one label (lowercase letters, digits and `-`, at most 32, no
/// trailing `-`). The root's own random id follows it, so labels repeat
/// freely.
fn barge_label(layout: Option<&SlotLayout>) -> String {
    let profile = layout.map(|layout| layout.profile.as_str()).unwrap_or("slot");
    let folder = layout
        .and_then(|layout| layout.folders.first())
        .map(String::as_str);
    let mut label = match folder {
        Some(folder) if !folder.is_empty() => format!("{folder}-{profile}"),
        _ => profile.to_owned(),
    };
    label = label
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    label.truncate(32);
    while label.ends_with('-') || label.is_empty() {
        if label.is_empty() {
            label.push_str("slot");
            break;
        }
        label.pop();
    }
    label
}

/// A recorded slot directory: exactly `<folder>/<profile>-<id>` with plain
/// names. Anything else refuses the ledger.
fn split_slot_dir(dir: &str, id: u64) -> Option<(&str, &str)> {
    match dir.split_once('/') {
        Some((folder, leaf)) => {
            let profile = leaf.strip_suffix(&format!("-{id}"))?;
            (valid_slot_name(folder) && valid_slot_name(leaf) && !profile.is_empty())
                .then_some((folder, leaf))
        }
        // A Barge root folder is one level and carries the library's own id,
        // not the numeric slot id: `<label>-<16 hex>`.
        None => (valid_slot_name(dir) && !dir.starts_with('.')).then_some(("", dir)),
    }
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
    /// Read-tolerant legacy key from the retired holding journal. This
    /// version never writes or acts on it; an older writer's rows are kept
    /// as inert values so the ledger still parses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    holding: Vec<serde_json::Value>,
    slots: Vec<SlotRecord>,
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
    /// The Barge root id this slot lives in (oxide#314). Absent for a legacy
    /// Oxide slot directory, which Barge does not own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root: Option<String>,
    /// Idle rank from this pool's monotonic sequence (oxide#118). Legacy
    /// records keep 0 and sort oldest.
    #[serde(default, skip_serializing_if = "is_zero_rank")]
    rank: u64,
    /// Read-tolerant legacy keys from the retired size observation and
    /// deletion journal. This version never writes or acts on them; older
    /// values stay inert so the ledger still parses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recorded: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hold_intent: Option<serde_json::Value>,
}





/// Most captures one deletion may have recorded and not yet removed. A

/// Bytes a path holds, Barge's accounting rule: no symlink followed, no
/// nested filesystem crossed.
fn barge_tree_bytes(path: &Path) -> Option<u64> {
    fn walk(path: &Path, device: u64, total: &mut u64) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let child = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&child) else {
                continue;
            };
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                use std::os::unix::fs::MetadataExt;
                if metadata.dev() == device {
                    walk(&child, device, total);
                }
            } else {
                *total = total.saturating_add(metadata.len());
            }
        }
    }
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_dir() {
        return Some(metadata.len());
    }
    let device = metadata.dev();
    let mut total = 0;
    walk(path, device, &mut total);
    Some(total)
}

/// A complete scan for a figure Barge's own accounting produced.
fn barge_scan(bytes: Option<u64>) -> AccountingScan {
    AccountingScan {
        coverage: if bytes.is_some() {
            AccountingCoverage::Complete
        } else {
            AccountingCoverage::Unknown
        },
        gaps: Vec::new(),
        vanished: 0,
        entries_visited: u64::from(bytes.is_some()),
        elapsed_ms: 0,
        max_entries: 0,
        max_depth: 0,
        max_elapsed_ms: 0,
    }
}

/// The occupancy a ledger row reports.
fn occupancy(slot: &SlotRecord) -> SlotOccupancy {
    match slot.state {
        SlotPersistState::Held => SlotOccupancy::Held,
        SlotPersistState::Idle => SlotOccupancy::Idle,
        SlotPersistState::Evicting => SlotOccupancy::Abandoned,
    }
}

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
}

pub(super) struct Pool {
    /// The exclusive pool lock, held for the life of a mutating open. A
    /// passive measurement holds none.
    _lock: Option<File>,
    root: PathBuf,
    config: PoolConfig,
    backing: Payload,
    /// The pool's Barge area. `None` for a passive measurement, which creates
    /// and writes nothing.
    storage: Option<crate::storage::PoolStorage>,
    /// The hold this admission took, handed to the caller with the grant so it
    /// lives as long as the command.
    pending_hold: Option<barge::Hold>,
    ledger: Ledger,
    /// Admission budget: the recorded one unless the request overrides it.
    budget: u64,
    /// The caller's registered reserve for a removable pool, when it owns the
    /// fact. Barge's reserve policy decides when it prunes; this is reported.
    reserve: Option<u64>,
    /// A bounded note the caller prints when admission had to report a policy
    /// decision (for example a fallback or a warning).
    admission_note: Option<String>,
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
        refuse_foreign_pool_entries, refuse_retired_image_pool, snapshot_of,
        verify_scratch_initializer, write_config, write_ledger, Ledger, Payload, Pool, PoolConfig,
        SlotPersistState, VolumeConfig,
    };
    use crate::lease::HeldEvictionFailure;
    use crate::volume::Binding;
use barge::Hold;
    use std::fs::File;
    use std::os::fd::{AsFd, AsRawFd};
    use std::os::unix::fs::MetadataExt;
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

    /// Retire one recorded idle slot by ID.
    pub fn evict_idle_slot(pool: &Path, slot_id: &str) -> Result<Vec<String>, LeaseError> {
        Pool::open(pool)?.evict_idle(slot_id)
    }

    /// Retire one held slot by ID and its exact lease.
    pub fn evict_held_slot(
        pool: &Path,
        slot_id: &str,
        lease_id: &str,
    ) -> Result<Vec<String>, HeldEvictionFailure> {
        Pool::open_held_eviction(pool)
            .map_err(HeldEvictionFailure::PreCommit)?
            .evict_held(slot_id, lease_id)
    }

    /// No holding entries exist in this engine: eviction is Barge's.
    pub fn holding_entry_name(
        _store_id: &str,
        _slot_id: &str,
        _incarnation: &str,
        _lease_id: &str,
    ) -> Option<String> {
        None
    }

    /// A capability issued only by exclusive creation of the scratch leaf.
    pub struct ScratchLeaf {
        path: std::path::PathBuf,
        file: std::fs::File,
    }

    /// Create one owner-only scratch leaf for a scratch pool.
    pub fn create_scratch_leaf(pool: &Path) -> Result<ScratchLeaf, LeaseError> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(pool)
            .map_err(io_error)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(pool)
            .map_err(io_error)?;
        Ok(ScratchLeaf {
            path: pool.to_path_buf(),
            file,
        })
    }

    impl ScratchLeaf {
        /// Compensate only while the path still names the held leaf.
        pub fn remove_if_owned(&self, binding: &[u8]) -> Result<(), LeaseError> {
            verify_scratch_initializer(&self.path, &self.file, binding, false)?;
            let current = std::fs::symlink_metadata(&self.path).map_err(io_error)?;
            let held = self.file.metadata().map_err(io_error)?;
            use std::os::unix::fs::MetadataExt;
            if !current.is_dir()
                || current.file_type().is_symlink()
                || current.dev() != held.dev()
                || current.ino() != held.ino()
            {
                return Err(LeaseError::InvalidInput(
                    "scratch-incomplete: leaf identity changed",
                ));
            }
            let control = self.path.join("oxide-managed");
            let names = std::fs::read_dir(&control)
                .map_err(io_error)?
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(io_error)?;
            if names.len() != 1 || names[0] != "scratch.json" {
                return Err(LeaseError::InvalidInput(
                    "scratch-incomplete: unexpected entries",
                ));
            }
            std::fs::remove_file(control.join("scratch.json")).map_err(io_error)?;
            std::fs::remove_dir(&control).map_err(io_error)?;
            let current = std::fs::symlink_metadata(&self.path).map_err(io_error)?;
            if current.dev() != held.dev() || current.ino() != held.ino() {
                return Err(LeaseError::InvalidInput(
                    "scratch-incomplete: leaf identity changed",
                ));
            }
            std::fs::remove_dir(&self.path).map_err(io_error)
        }
    }

    /// Initialize only a leaf exclusively created and prepared by this caller.
    pub fn init_scratch_pool(
        leaf: &ScratchLeaf,
        binding: &[u8],
        budget_bytes: u64,
    ) -> Result<PoolInfo, LeaseError> {
        let pool = leaf.path.as_path();
        if budget_bytes == 0 {
            return Err(LeaseError::InvalidInput("budget must be positive"));
        }
        verify_scratch_initializer(pool, &leaf.file, binding, false)?;
        let _lock = lock_pool(pool)?;
        verify_scratch_initializer(pool, &leaf.file, binding, true)?;
        init_budget_pool_locked(pool, budget_bytes)
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
        init_budget_pool_locked(&pool, budget_bytes)
    }



    fn init_budget_pool_locked(pool: &Path, budget_bytes: u64) -> Result<PoolInfo, LeaseError> {
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
        #[cfg(test)]
        crate::tape::pool_row(
            "lease:ledger-view",
            pool,
            crate::tape::TapeOutcome::Facts {
                facts: "entered".to_owned(),
            },
        );
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
pub(super) fn evict_held_slot_injected(
    pool: &Path,
    slot_id: &str,
    lease_id: &str,
) -> Result<Vec<String>, HeldEvictionFailure> {
    managed::evict_held_slot(pool, slot_id, lease_id)
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

impl Pool {
    /// Open a budget-only directory or removable-volume pool. Any other
    /// recorded backend, including the retired `apfs-sparse-image-v1`,
    /// refuses before anything under the pool is touched.
    pub(super) fn open(root: &Path) -> Result<Self, LeaseError> {
        Self::open_inner(root, false, true, true)
    }

    fn open_for_release(root: &Path) -> Result<Self, LeaseError> {
        Self::open_inner(root, true, true, true)
    }

    /// The held eviction's locked open (oxide#196): the pool lock is taken, but
    /// no control fence is written and nothing is reconciled before the
    /// caller's held-row validation and its durable hold intent. A pre-commit
    /// failure therefore leaves the pool bytes and identity unchanged. A pool
    /// with pending recorded reconciliation refuses in the caller.
    fn open_held_eviction(root: &Path) -> Result<Self, LeaseError> {
        Self::open_inner(root, false, false, false)
    }

    fn open_inner(
        root: &Path,
        trace_release: bool,
        reconcile: bool,
        fence_control: bool,
    ) -> Result<Self, LeaseError> {
        let root = canonical_existing_dir(root, true)?;
        refuse_foreign_pool_entries(&root)?;
        refuse_retired_image_pool(&root)?;
        let lock = if trace_release {
            crate::release_trace::phase("pool lock", || lock_pool_inner(&root, true, true))?
        } else {
            lock_pool(&root)?
        };
        #[cfg(test)]
        crate::tape::lock_row(
            "pool:lock",
            &root.join("lock"),
            crate::tape::TapeOutcome::Facts {
                facts: "locked".to_owned(),
            },
        );
        // Debug proof observation (oxide#190): an eviction's reported interval
        // starts at this lock acquisition, so it covers every decision this lock
        // hold makes instead of only the rename transaction.
        #[cfg(debug_assertions)]
        let lock_taken = Some(Instant::now());
        // Fence first (oxide#177, ruling 4c9984a1): a managed pool's control
        // state reaches the current fence version before this open can start a
        // recovery journal or finish a recorded deletion. A library fixture
        // has no control state and nothing to fence.
        if fence_control {
            super::fence_managed_control(&root)?;
        }
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
        let storage = Some(pool_storage(&root, &config)?);
        #[cfg(test)]
        let tape_open_root = root.display().to_string();
        let mut pool = Self {
            _lock: Some(lock),
            root,
            config,
            backing,
            storage,
            pending_hold: None,
            budget: ledger.cap_bytes,
            ledger,
            reserve: None,
            admission_note: None,
            #[cfg(debug_assertions)]
            lock_taken,
        };
        // A Barge-owned slot's hold is the authority: a held row whose root
        // can be held again has no live holder, so it returns to idle before
        // any admission reads the ledger. Existing Oxide roots are adopted in
        // place first, keeping their path and bytes.
        pool.adopt_legacy_roots()?;
        pool.recover_abandoned_holds()?;
        pool.sweep_trash()?;
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
        let _ = max_elapsed_ms;
        Ok(Self {
            _lock: None,
            root,
            config,
            backing,
            storage: None,
            pending_hold: None,
            budget: ledger.cap_bytes,
            ledger,
            reserve: None,
            admission_note: None,
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
        write_ledger(&self.root, &self.ledger)?;
        Ok(())
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

    /// A held row whose Barge root can be held again has no live holder: the
    /// process that held it is gone and the kernel released its lock. Mark it
    /// idle so the next compatible command reuses it. A row something still
    /// holds, and a root whose lock cannot be tested, are kept as they are.
    ///
    /// This is the whole crash-recovery rule for a Barge-owned slot: the flock
    /// is the authority, not a process or descriptor census.
    fn recover_abandoned_holds(&mut self) -> Result<(), LeaseError> {
        let Some(storage) = self.storage.clone() else {
            return Ok(());
        };
        let mut changed = false;
        for slot in &mut self.ledger.slots {
            if slot.state != SlotPersistState::Held {
                continue;
            }
            let Some(root) = slot.root.clone() else {
                continue;
            };
            if let Ok(Some(hold)) = storage.try_hold(&root) {
                drop(hold);
                slot.state = SlotPersistState::Idle;
                slot.lease_id = None;
                slot.holder.clear();
                slot.recorded = None;
                slot.hold_intent = None;
                changed = true;
            }
        }
        if changed {
            self.persist()?;
        }
        Ok(())
    }

    /// Adopt every existing Oxide slot directory as a Barge-owned root,
    /// keeping its path and bytes. The library writes its own marker and moves
    /// nothing; a row whose directory has gone, and an unmarked stray at a
    /// retired folder, are left alone.
    fn adopt_legacy_roots(&mut self) -> Result<(), LeaseError> {
        let Some(storage) = self.storage.clone() else {
            return Ok(());
        };
        let mut changed = false;
        for slot in &mut self.ledger.slots {
            if slot.root.is_some() {
                continue;
            }
            let dir = slot.dir.clone().unwrap_or_else(|| slot.id.to_string());
            let (project_folder, leaf) = match dir.split_once('/') {
                Some((folder, leaf)) => (Some(folder), leaf.to_owned()),
                None => (None, dir.clone()),
            };
            let label = slot.profile.as_str().to_owned();
            let id = legacy_root_id(&dir);
            if let Ok(root) = storage.adopt_in(project_folder.as_deref(), &leaf, &label, &id) {
                slot.root = Some(root.id);
                changed = true;
            }
        }
        if changed {
            self.persist()?;
        }
        Ok(())
    }

    fn lease(&mut self, request: &LeaseRequest) -> Result<Leased, LeaseError> {
        self.pending_hold = None;
        let mut leased = self.lease_inner(request)?;
        leased.hold = self.pending_hold.take();
        Ok(leased)
    }

    fn lease_inner(&mut self, request: &LeaseRequest) -> Result<Leased, LeaseError> {
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
                    hold: None,
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
        let candidate = self.idle_compatible(&project, request.profile);
        // One-use workspaces are dormant; their bounded slice report is still
        // returned so a caller sees the outcome it supplied.
        let slice_report = match request.slice.as_ref() {
            Some(slice) => {
                if !slice.members.is_bounded() || slice.max_slots == 0 || slice.max_bytes == 0 {
                    return Err(LeaseError::InvalidInput(
                        "the one-use slice bounds are not usable",
                    ));
                }
                Some(self.restore(slice, candidate)?)
            }
            None => None,
        };
        if let Some((idle, incarnation)) = candidate {
            let grant = self.take_idle(idle, incarnation, project, workspace, request)?;
            return Ok(Leased {
                grant,
                hold: None,
                measured: None,
                admission_note: None,
                slice: slice_report,
                unmeasured: Vec::new(),
                holding: Vec::new(),
            });
        }
        self.ensure_capacity_policy()?;
        self.next_rank()?;
        let grant = self.allocate(project, workspace, request)?;
        Ok(Leased {
            grant,
            hold: None,
            measured: None,
            admission_note: self.admission_note.take(),
            slice: slice_report,
            unmeasured: Vec::new(),
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

    fn take_idle(
        &mut self,
        slot_id: u64,
        incarnation: u64,
        project: PathBuf,
        workspace: PathBuf,
        request: &LeaseRequest,
    ) -> Result<LeaseGrant, LeaseError> {
        // A Barge-owned slot is idle exactly when its exclusive hold can be
        // taken. `try_hold` never waits, so a live holder — including a child
        // that kept the inherited descriptor — makes this row unusable now.
        if let (Some(storage), Some(root)) = (
            self.storage.clone(),
            self.slot(slot_id).and_then(|slot| slot.root.clone()),
        ) {
            match storage
                .try_hold(&root)
                .map_err(|_| LeaseError::UnknownAccounting)?
            {
                Some(hold) => self.pending_hold = Some(hold),
                None => return Err(LeaseError::Busy),
            }
        }
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
    ) -> Result<LeaseGrant, LeaseError> {
        if let Some(victim) = request.replace_slot {
            self.replace_idle_slot_for_admission(victim)?;
        }
        // Slot headroom only: the pool ceiling is replaced, never exceeded.
        if self.ledger.slots.len() >= MAX_SLOTS {
            self.evict_idle_until(MAX_SLOTS - 1)?;
            if self.ledger.slots.len() >= MAX_SLOTS {
                return Err(LeaseError::LedgerExhausted);
            }
        }
        // Byte pressure and space policy are Barge's: the area's own bytes
        // against the pool's soft cap, and (on a removable pool) the volume's
        // reserve and floor. One idle unpinned slot is retired per attempt.
        if let Some(storage) = self.storage.clone() {
            let used = storage
                .total_bytes()
                .map_err(|_| LeaseError::UnknownAccounting)?;
            if used >= self.budget {
                if !self.evict_one_idle_with()? {
                    return Err(LeaseError::Full);
                }
                let used = storage
                    .total_bytes()
                    .map_err(|_| LeaseError::UnknownAccounting)?;
                if used >= self.budget {
                    return Err(LeaseError::Full);
                }
            }
            if self.is_removable() {
                if storage.prune(false).is_err() {
                    return Err(LeaseError::UnknownAccounting);
                }
                if let Ok(space) = storage.space() {
                    if space.floor_bytes > 0 && space.free_bytes < space.floor_bytes {
                        return Err(LeaseError::Full);
                    }
                }
            }
        }
        let slot_id = self.ledger.next_slot;
        if slot_id == 0 {
            return Err(LeaseError::UnknownAccounting);
        }
        let incarnation = 1_u64;
        let lease_id = self.mint_lease_id(slot_id, incarnation)?;
        if let Some(layout) = &request.layout {
            if !valid_slot_name(&layout.profile) {
                return Err(LeaseError::InvalidInput("invalid slot profile name"));
            }
        }
        // A new slot is a Barge root when this pool has a storage authority:
        // the library mints the id and path, writes the ownership marker and
        // gives us the cooperative hold that keeps the root through the
        // command. A pool opened without storage (a library fixture with a
        // fake volume identity) keeps the pre-adoption directory path.
        let mut root_id = None;
        let dir = match self.storage.clone() {
            Some(storage) => {
                let root = storage
                    .create(&barge_label(request.layout.as_ref()))
                    .map_err(|_| LeaseError::UnknownAccounting)?;
                ensure_private_dir(&root.path.join("build"))?;
                self.pending_hold = Some(
                    storage
                        .hold(&root.id)
                        .map_err(|_| LeaseError::UnknownAccounting)?,
                );
                root_id = Some(root.id.clone());
                Some(root.folder_name())
            }
            None => {
                let dir = match &request.layout {
                    Some(layout) => {
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
                    None => None,
                };
                let slot_path = self.slot_path_in(slot_id, dir.as_deref())?;
                match &self.backing {
                    Payload::Volume { binding, fd, .. } => {
                        let create =
                            |dir: std::os::fd::BorrowedFd<'_>, name: &str, new: bool| {
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
                dir
            }
        };
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
            root: root_id,
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
        // Barge's own accounting is the only size source; release writes no
        // separate observation.
        self.ledger.slots[index].state = SlotPersistState::Idle;
        self.ledger.slots[index].lease_id = None;
        self.ledger.slots[index].recorded = None;
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

    /// Whether this pool's roots live on a removable volume.
    fn is_removable(&self) -> bool {
        matches!(self.backing, Payload::Volume { .. })
    }

    /// Sweep owned trash a crash left behind.
    fn sweep_trash(&self) -> Result<(), LeaseError> {
        if let Some(storage) = self.storage.as_ref() {
            storage
                .sweep_all_trash()
                .map_err(|_| LeaseError::UnknownAccounting)?;
        }
        Ok(())
    }

    /// The pool's own bytes, as Barge accounts its area.
    fn pressure(&mut self) -> Result<u64, LeaseError> {
        match self.storage.as_ref() {
            Some(storage) => storage
                .total_bytes()
                .map_err(|_| LeaseError::UnknownAccounting),
            None => Ok(0),
        }
    }

    /// Retire one idle recorded slot: Barge evicts the owned root (refusing a
    /// held or pinned one) and the ledger row is dropped. Nothing is moved
    /// outside Barge and no journal is written.
    fn retire_idle_slot(&mut self, id: u64) -> Result<(), LeaseError> {
        let Some(slot) = self.slot(id) else {
            return Err(LeaseError::Busy);
        };
        if slot.state != SlotPersistState::Idle || slot.lease_id.is_some() {
            return Err(LeaseError::Busy);
        }
        let root = slot.root.clone().ok_or(LeaseError::UnknownAccounting)?;
        let project = slot.dir.as_deref().and_then(|dir| dir.split_once('/')).map(|(f, _)| f.to_owned());
        let storage = self.storage.as_ref().ok_or(LeaseError::UnknownAccounting)?;
        storage
            .evict_in(project.as_deref(), &root)
            .map_err(|error| {
                if error.contains("held") || error.contains("pinned") {
                    LeaseError::Busy
                } else {
                    LeaseError::UnknownAccounting
                }
            })?;
        self.ledger.slots.retain(|slot| slot.id != id);
        self.persist_revised()
    }

    /// Retire one idle slot named by its numeric id.
    fn evict_idle(&mut self, slot_id: &str) -> Result<Vec<String>, LeaseError> {
        let id = slot_id
            .parse::<u64>()
            .ok()
            .filter(|id| *id != 0)
            .ok_or(LeaseError::InvalidInput("slot ID must be a positive integer"))?;
        self.retire_idle_slot(id)?;
        Ok(Vec::new())
    }

    /// Retire one held slot by its exact lease.
    fn evict_held(
        &mut self,
        slot_id: &str,
        lease_id: &str,
    ) -> Result<Vec<String>, HeldEvictionFailure> {
        let pre = HeldEvictionFailure::PreCommit;
        let id = slot_id
            .parse::<u64>()
            .ok()
            .filter(|id| *id != 0)
            .ok_or(LeaseError::InvalidInput("slot ID must be a positive integer"))
            .map_err(pre)?;
        let Some(slot) = self.slot(id) else {
            return Err(pre(LeaseError::Busy));
        };
        if slot.state != SlotPersistState::Held || slot.lease_id.as_deref() != Some(lease_id) {
            return Err(pre(LeaseError::Busy));
        }
        let root = slot.root.clone().ok_or(pre(LeaseError::UnknownAccounting))?;
        let project = slot.dir.as_deref().and_then(|dir| dir.split_once('/')).map(|(f, _)| f.to_owned());
        let storage = self.storage.as_ref().ok_or(pre(LeaseError::UnknownAccounting))?;
        storage
            .evict_in(project.as_deref(), &root)
            .map_err(|_| pre(LeaseError::UnknownAccounting))?;
        self.ledger.slots.retain(|slot| slot.id != id);
        self.persist_revised().map_err(pre)?;
        Ok(Vec::new())
    }

    /// Retire idle slots until at most `max` remain.
    fn evict_idle_until(&mut self, max: usize) -> Result<(), LeaseError> {
        while self.ledger.slots.len() > max {
            if !self.evict_one_idle_with()? {
                break;
            }
        }
        Ok(())
    }

    /// Retire the oldest idle slot, if any. A held or pinned slot is refused
    /// by Barge and skipped, never forced.
    fn evict_one_idle_with(&mut self) -> Result<bool, LeaseError> {
        let victim = self
            .ledger
            .slots
            .iter()
            .filter(|slot| slot.state == SlotPersistState::Idle && slot.lease_id.is_none())
            .min_by_key(|slot| (slot.rank, slot.id))
            .map(|slot| slot.id);
        match victim {
            Some(id) => match self.retire_idle_slot(id) {
                Ok(()) => Ok(true),
                Err(LeaseError::Busy) => Ok(false),
                Err(error) => Err(error),
            },
            None => Ok(false),
        }
    }

    /// The front door's count replacement: retire this idle slot.
    fn replace_idle_slot_for_admission(&mut self, id: u64) -> Result<(), LeaseError> {
        self.retire_idle_slot(id)
    }

    /// A bounded one-use restore observation. The dormant feature's slice
    /// carries its own member set; with no walk in this engine the outcome is
    /// "not needed", which every caller treats as no measurement.
    fn restore(
        &mut self,
        slice: &OneUseSlice,
        _candidate: Option<(u64, u64)>,
    ) -> Result<SliceReport, LeaseError> {
        Ok(SliceReport {
            outcome: RestoreOutcome::NotNeeded,
            members: 0,
            protected: 0,
            unverified: 0,
            max_slots: slice.max_slots,
            max_bytes: slice.max_bytes,
        })
    }

    /// No slot is charged conservatively in this engine.
    fn unmeasured_slots(&mut self) -> Vec<UnmeasuredSlot> {
        Vec::new()
    }

    fn unmeasured_charge_exhausts_budget(&self) -> bool {
        false
    }

    fn capacity_refusal(&self, _measured: u64) -> LeaseError {
        LeaseError::Full
    }

    fn count_selection_refusal(&self) -> LeaseError {
        LeaseError::LedgerExhausted
    }

    /// The status document: the ledger's membership plus Barge's own per-root
    /// accounting. There is no separate inventory walk.
    fn accounted(&self) -> Result<PoolStatus, LeaseError> {
        let storage = self.storage.as_ref().ok_or(LeaseError::UnknownAccounting)?;
        let roots = storage
            .all_roots()
            .map_err(|_| LeaseError::UnknownAccounting)?;
        let mut by_id = BTreeMap::new();
        for (_project, info) in &roots {
            by_id.insert(info.id.clone(), info.bytes);
        }
        let mut slots = Vec::new();
        for slot in &self.ledger.slots {
            let bytes = slot
                .root
                .as_ref()
                .and_then(|root| by_id.get(root).copied())
                .or_else(|| {
                    self.slot_path(slot.id)
                        .ok()
                        .and_then(|path| barge_tree_bytes(&path))
                });
            slots.push(SlotStatus {
                slot_id: slot.id.to_string(),
                incarnation: slot.incarnation.to_string(),
                state: occupancy(slot),
                project: slot.project.clone(),
                workspace: slot.workspace.clone(),
                profile: slot.profile,
                holder: (!slot.holder.is_empty()).then(|| slot.holder.clone()),
                lease_id: slot.lease_id.clone(),
                dir: slot.dir.clone(),
                rank: slot.rank,
                logical_bytes: bytes,
                unique_allocated_bytes: bytes,
                accounting: barge_scan(bytes),
            });
        }
        let total = storage
            .total_bytes()
            .map_err(|_| LeaseError::UnknownAccounting)?;
        Ok(PoolStatus {
            version: LEASE_VERSION,
            store_id: self.config.store_id.clone(),
            backend: self.config.backend.clone(),
            cap_bytes: self.config.cap_bytes,
            payload_logical_bytes: Some(total),
            payload_unique_allocated_bytes: Some(total),
            coverage: AccountingCoverage::Complete,
            payload_accounting: barge_scan(Some(total)),
            full: Some(total >= self.config.cap_bytes),
            slots,
            holding: Vec::new(),
        })
    }

    /// Ledger membership without any walk.
    fn ledger_view(&self) -> Result<LedgerView, LeaseError> {
        Ok(LedgerView {
            store_id: self.config.store_id.clone(),
            backend: self.config.backend.clone(),
            cap_bytes: self.config.cap_bytes,
            reconciliation_blocked: false,
            slots: self.ledger.slots.iter().map(ledger_slot).collect(),
            holding: Vec::new(),
        })
    }

    fn holding_entries(&self) -> Vec<super::HoldingEntry> {
        Vec::new()
    }

    fn holding_bytes(&self) -> u64 {
        0
    }

    fn holding_name_for(&self, _id: u64, _incarnation: u64, _lease: &str) -> Option<String> {
        None
    }

    fn holding_device(&self) -> u64 {
        0
    }

    fn open_holding_area(&self, _create: bool) -> Result<Option<OwnedFd>, LeaseError> {
        Ok(None)
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
    let idle = ledger
        .slots
        .iter()
        .filter(|slot| slot.state == SlotPersistState::Idle && slot.lease_id.is_none())
        .count();
    PassiveSnapshot {
        store_id: config.store_id.clone(),
        backend: config.backend.clone(),
        cap_bytes: config.cap_bytes,
        payload,
        volume,
        recorded_slots: 0,
        unrecorded_slots: idle,
        recorded_total: None,
        holding: Vec::new(),
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
        && snapshot.holding.is_empty()
        && snapshot
            .slots
            .iter()
            .zip(ledger.slots.iter())
            .all(|(shown, stored)| {
                shown.slot_id == stored.id.to_string()
                    && shown.incarnation == stored.incarnation.to_string()
                    && shown.dir == stored.dir
                    && shown.state == occupancy(stored)
            })
}

/// The budget-only payload is a plain owner-only directory holding `slots`.
/// The budget is not OS-enforced. Image and receipt files refuse, so a legacy
/// APFS image pool is never reinterpreted as a directory.
/// Open one pool's Barge storage authority from its own recorded config.
///
/// The account is the pool's control directory, so the literal `oxide/pool`
/// declaration is unambiguous: every pool — removable, internal, one-use or a
/// test fixture — owns its own volume and area. A budget pool's volume is the
/// pool directory itself; a removable pool's is the drive at its registered
/// mount point with the recorded identity, so a different volume there is a
/// refusal, never a silent reuse.
fn pool_storage(
    root: &Path,
    config: &PoolConfig,
) -> Result<crate::storage::PoolStorage, LeaseError> {
    use crate::storage::PoolBacking;
    let control = root.join(MANAGED_NAME);
    let backing = match config.backend.as_str() {
        BUDGET_BACKEND_DIRECTORY_V1 => PoolBacking::budget(root, config.cap_bytes),
        VOLUME_BACKEND_APFS_V1 => {
            let volume = config.volume.as_ref().ok_or(LeaseError::BackingMismatch)?;
            PoolBacking::volume(
                &volume.mount_point,
                &volume.volume_uuid,
                &volume.payload_dir,
                config.cap_bytes,
                0,
                UNKNOWN_REMOVABLE_FLOOR_BYTES,
            )
        }
        _ => return Err(LeaseError::BackingMismatch),
    };
    crate::storage::PoolStorage::open(&control, &backing).map_err(|_| LeaseError::BackingMismatch)
}

fn open_budget_payload(
    root: &Path,
    paths: &BackingPaths,
    config: &PoolConfig,
) -> Result<Payload, LeaseError> {
    if config.version != LEASE_VERSION || config.cap_bytes == 0 || config.image_bytes != 0 {
        return Err(LeaseError::BackingMismatch);
    }
    #[cfg(not(test))]
    if paths.image.symlink_metadata().is_ok() || paths.receipt.symlink_metadata().is_ok() {
        return Err(LeaseError::BackingMismatch);
    }
    #[cfg(test)]
    if observe_pool_metadata("pool:budget:image", &paths.image).is_ok()
        || observe_pool_metadata("pool:budget:receipt", &paths.receipt).is_ok()
    {
        return Err(LeaseError::BackingMismatch);
    }
    let payload =
        canonical_existing_dir(&paths.mount, true).map_err(|_| LeaseError::BackingMismatch)?;
    #[cfg(not(test))]
    let root_device = fs::symlink_metadata(root).map_err(io_error)?.dev();
    #[cfg(test)]
    let root_device = observe_pool_metadata("pool:budget:root", root)
        .map_err(io_error)?
        .dev();
    #[cfg(not(test))]
    let payload_device = fs::symlink_metadata(&payload).map_err(io_error)?.dev();
    #[cfg(test)]
    let payload_device = observe_pool_metadata("pool:budget:payload", &payload)
        .map_err(io_error)?
        .dev();
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
    #[cfg(not(test))]
    if paths.image.symlink_metadata().is_ok()
        || paths.receipt.symlink_metadata().is_ok()
        || paths.mount.symlink_metadata().is_ok()
    {
        return Err(LeaseError::BackingMismatch);
    }
    #[cfg(test)]
    if observe_pool_metadata("pool:volume:image", &paths.image).is_ok()
        || observe_pool_metadata("pool:volume:receipt", &paths.receipt).is_ok()
        || observe_pool_metadata("pool:volume:internal-payload", &paths.mount).is_ok()
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
    #[cfg(not(test))]
    let by_path = fs::symlink_metadata(&payload).map_err(io_error)?;
    #[cfg(test)]
    let by_path = observe_pool_metadata("pool:volume:payload-path", &payload).map_err(io_error)?;
    #[cfg(not(test))]
    let by_fd = fs::File::from(fd.try_clone().map_err(io_error)?)
        .metadata()
        .map_err(io_error)?;
    #[cfg(test)]
    let by_fd = {
        let result = fs::File::from(fd.try_clone().map_err(io_error)?).metadata();
        let saved_errno = crate::tape::observer_errno();
        match &result {
            Ok(metadata) => crate::tape::read_row_dev(
                "pool:volume:payload-held",
                &payload,
                metadata.dev(),
                metadata.ino(),
                crate::tape::TapeOutcome::Opened {
                    dev: metadata.dev(),
                    ino: metadata.ino(),
                },
                None,
            ),
            Err(error) => crate::tape::read_row(
                "pool:volume:payload-held",
                &payload,
                crate::tape::TapeOutcome::Refusal {
                    reason: error.to_string(),
                },
                None,
            ),
        }
        crate::tape::restore_observer_errno(saved_errno);
        result.map_err(io_error)?
    };
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

fn recorded_type(record: &CaptureRecord) -> libc::mode_t {
    (record.mode & u32::from(libc::S_IFMT)) as libc::mode_t
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
    #[cfg(not(test))]
    let root_device = fs::symlink_metadata(slots_root).map_err(io_error)?.dev();
    #[cfg(test)]
    let root_device = observe_pool_metadata("pool:slot-parent:root", slots_root)
        .map_err(io_error)?
        .dev();
    #[cfg(not(test))]
    if fs::symlink_metadata(parent).map_err(io_error)?.dev() != root_device {
        return Err(LeaseError::UnknownAccounting);
    }
    #[cfg(test)]
    if observe_pool_metadata("pool:slot-parent:parent", parent)
        .map_err(io_error)?
        .dev()
        != root_device
    {
        return Err(LeaseError::UnknownAccounting);
    }
    Ok(())
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

/// The hard floor for an unknown-size admission with nothing left to prune.
///
/// Tom's 2026-10-25 refinement: the registered reserve is a *prune* trigger,
/// not a stop; only this floor is a stop. It is a fixed product constant for
/// oxide#146 — not a registration field, a watchdog setting or a selection
/// change.
const UNKNOWN_REMOVABLE_FLOOR_BYTES: u64 = 10 * 1024 * 1024 * 1024;



/// A record's membership fields, as status and the ledger view show them.
fn ledger_slot(slot: &SlotRecord) -> LedgerSlot {
    LedgerSlot {
        slot_id: slot.id.to_string(),
        incarnation: slot.incarnation.to_string(),
        state: occupancy(slot),
        project: slot.project.clone(),
        workspace: slot.workspace.clone(),
        profile: slot.profile,
        holder: (slot.state == SlotPersistState::Held).then(|| slot.holder.clone()),
        lease_id: slot.lease_id.clone(),
        dir: slot.dir.clone(),
        rank: slot.rank,
        recorded: None,
    }
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

/// Wall-clock milliseconds. A recorded observation's age, never a validity
/// input: validity is the slot's generation.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
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

/// Check the creator's held leaf and the complete prepared shape under the init lock.
fn verify_scratch_initializer(
    pool: &Path,
    leaf: &File,
    expected: &[u8],
    locked: bool,
) -> Result<(), LeaseError> {
    use std::io::Read;
    use std::os::fd::FromRawFd;
    let incomplete =
        || LeaseError::InvalidInput("scratch-incomplete: prepared pool requires operator cleanup");
    if expected.len() > 16384 {
        return Err(incomplete());
    }
    let current = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(pool)
        .map_err(|_| incomplete())?;
    let held_meta = leaf.metadata().map_err(|_| incomplete())?;
    let meta = current.metadata().map_err(|_| incomplete())?;
    if !meta.is_dir()
        || meta.uid() != current_uid()
        || meta.mode() & 0o7777 != 0o700
        || (meta.dev(), meta.ino()) != (held_meta.dev(), held_meta.ino())
    {
        return Err(incomplete());
    }
    let list = |path: &Path| -> Result<Vec<String>, LeaseError> {
        std::fs::read_dir(path)
            .map_err(|_| incomplete())?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .map_err(|_| incomplete())
            })
            .collect()
    };
    let names = list(pool)?;
    let has_managed = names.iter().any(|n| n == "oxide-managed");
    let has_lock = names.iter().any(|n| n == "lock");
    if !has_managed || names.len() != 1 + usize::from(has_lock) || (locked && !has_lock) {
        return Err(incomplete());
    }
    let managed_names = list(&pool.join("oxide-managed"))?;
    if managed_names.len() != 1 || managed_names[0] != "scratch.json" {
        return Err(incomplete());
    }
    let managed = open_directory_at(current.as_fd(), c"oxide-managed").map_err(|_| incomplete())?;
    let raw = unsafe {
        libc::openat(
            managed.as_raw_fd(),
            c"scratch.json".as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(incomplete());
    }
    let file = unsafe { File::from_raw_fd(raw) };
    let binding_meta = file.metadata().map_err(|_| incomplete())?;
    if !binding_meta.is_file() || binding_meta.uid() != current_uid() || binding_meta.len() > 16384
    {
        return Err(incomplete());
    }
    let mut bytes = Vec::new();
    file.take(16385)
        .read_to_end(&mut bytes)
        .map_err(|_| incomplete())?;
    if bytes != expected {
        return Err(incomplete());
    }
    Ok(())
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
    #[cfg(not(test))]
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
    #[cfg(test)]
    let existing = {
        let result = fs::symlink_metadata(&path);
        let saved_errno = crate::tape::observer_errno();
        match &result {
            Ok(metadata) => crate::tape::read_row_dev(
                "pool:lock:metadata-before",
                &path,
                metadata.dev(),
                metadata.ino(),
                crate::tape::TapeOutcome::Opened {
                    dev: metadata.dev(),
                    ino: metadata.ino(),
                },
                None,
            ),
            Err(error) => crate::tape::read_row(
                "pool:lock:metadata-before",
                &path,
                if error.kind() == std::io::ErrorKind::NotFound {
                    crate::tape::TapeOutcome::Absent
                } else {
                    crate::tape::TapeOutcome::Refusal {
                        reason: error.to_string(),
                    }
                },
                None,
            ),
        }
        crate::tape::restore_observer_errno(saved_errno);
        match result {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(LeaseError::UnsafePath("lock file must be a regular file"));
                }
                Some(metadata)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => None,
            Err(error) => return Err(io_error(error)),
        }
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
    #[cfg(not(test))]
    let file = options.open(&path).map_err(io_error)?;
    #[cfg(test)]
    let file = {
        let result = options.open(&path);
        if create {
            crate::tape::effect_row("attempt@pool-lock-open", &path);
        }
        result.map_err(io_error)?
    };
    #[cfg(not(test))]
    let metadata = file.metadata().map_err(io_error)?;
    #[cfg(test)]
    let metadata = {
        let result = file.metadata();
        let saved_errno = crate::tape::observer_errno();
        match &result {
            Ok(metadata) => crate::tape::read_row_dev(
                "pool:lock:metadata-held",
                &path,
                metadata.dev(),
                metadata.ino(),
                crate::tape::TapeOutcome::Opened {
                    dev: metadata.dev(),
                    ino: metadata.ino(),
                },
                None,
            ),
            Err(error) => crate::tape::read_row(
                "pool:lock:metadata-held",
                &path,
                crate::tape::TapeOutcome::Refusal {
                    reason: error.to_string(),
                },
                None,
            ),
        }
        crate::tape::restore_observer_errno(saved_errno);
        result.map_err(io_error)?
    };
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
    #[cfg(test)]
    if status != 0 {
        let saved_errno = crate::tape::observer_errno();
        let error = std::io::Error::last_os_error();
        crate::tape::lock_row(
            "pool:lock:failure",
            &path,
            crate::tape::TapeOutcome::Refusal {
                reason: error.to_string(),
            },
        );
        crate::tape::restore_observer_errno(saved_errno);
    }
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
    persist_private_file(&pool.join(LEDGER_NAME), &bytes)?;
    Ok(())
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
#[cfg(test)]
fn observe_pool_metadata(site: &str, path: &Path) -> std::io::Result<fs::Metadata> {
    let result = fs::symlink_metadata(path);
    let saved_errno = crate::tape::observer_errno();
    match &result {
        Ok(metadata) => crate::tape::read_row_dev(
            site,
            path,
            metadata.dev(),
            metadata.ino(),
            crate::tape::TapeOutcome::Opened {
                dev: metadata.dev(),
                ino: metadata.ino(),
            },
            None,
        ),
        Err(error) => crate::tape::read_row(
            site,
            path,
            if error.kind() == std::io::ErrorKind::NotFound {
                crate::tape::TapeOutcome::Absent
            } else {
                crate::tape::TapeOutcome::Refusal {
                    reason: error.to_string(),
                }
            },
            None,
        ),
    }
    crate::tape::restore_observer_errno(saved_errno);
    result
}

fn refuse_retired_image_pool(pool: &Path) -> Result<(), LeaseError> {
    let paths = BackingPaths::under(pool);
    #[cfg(not(test))]
    if paths.image.symlink_metadata().is_ok() || paths.receipt.symlink_metadata().is_ok() {
        return Err(LeaseError::BackingMismatch);
    }
    #[cfg(test)]
    if observe_pool_metadata("pool:retired:image", &paths.image).is_ok()
        || observe_pool_metadata("pool:retired:receipt", &paths.receipt).is_ok()
    {
        return Err(LeaseError::BackingMismatch);
    }
    #[cfg(not(test))]
    if pool.join(CONFIG_NAME).symlink_metadata().is_err() {
        return Ok(());
    }
    #[cfg(test)]
    if observe_pool_metadata("pool:retired:config", &pool.join(CONFIG_NAME)).is_err() {
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
    #[cfg(test)]
    {
        let saved_errno = crate::tape::observer_errno();
        match &metadata {
            Ok(metadata) => crate::tape::read_row_dev(
                "pool:listed-entry",
                path,
                metadata.dev(),
                metadata.ino(),
                crate::tape::TapeOutcome::Opened {
                    dev: metadata.dev(),
                    ino: metadata.ino(),
                },
                None,
            ),
            Err(error) => crate::tape::read_row(
                "pool:listed-entry",
                path,
                if error.kind() == std::io::ErrorKind::NotFound {
                    crate::tape::TapeOutcome::Absent
                } else {
                    crate::tape::TapeOutcome::Refusal {
                        reason: error.to_string(),
                    }
                },
                None,
            ),
        }
        crate::tape::restore_observer_errno(saved_errno);
    }
    match metadata {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

/// Test-only observer for the same `read_dir` and iterator used by the
/// production loop below. The real directory handle and error flow are kept;
/// no later pathname lookup is used to invent a parent identity.
#[cfg(test)]
mod pool_directory_tape {
    use std::path::{Path, PathBuf};

    pub(super) fn read_dir_at(path: &Path, site: &'static str) -> std::io::Result<Entries> {
        let result = std::fs::read_dir(path);
        let saved_errno = crate::tape::observer_errno();
        let outcome = match &result {
            Ok(_) => crate::tape::TapeOutcome::Facts {
                facts: "directory opened; identity unproven".to_owned(),
            },
            Err(error) => crate::tape::TapeOutcome::Refusal {
                reason: error.to_string(),
            },
        };
        crate::tape::read_row(&format!("{site}:read-dir-open"), path, outcome, None);
        let wrapped = result.map(|inner| Entries {
            inner,
            path: path.to_path_buf(),
            site,
        });
        crate::tape::restore_observer_errno(saved_errno);
        wrapped
    }

    pub(super) struct Entries {
        inner: std::fs::ReadDir,
        path: PathBuf,
        site: &'static str,
    }

    impl Iterator for Entries {
        type Item = std::io::Result<std::fs::DirEntry>;

        fn next(&mut self) -> Option<Self::Item> {
            let result = self.inner.next();
            let saved_errno = crate::tape::observer_errno();
            match &result {
                Some(Ok(entry)) => crate::tape::component_without_parent(
                    &format!("{}:name", self.site),
                    &entry.file_name(),
                    crate::tape::TapeOutcome::Facts {
                        facts: "enumerated".to_owned(),
                    },
                ),
                Some(Err(error)) => crate::tape::read_row(
                    &format!("{}:iteration", self.site),
                    &self.path,
                    crate::tape::TapeOutcome::Refusal {
                        reason: error.to_string(),
                    },
                    None,
                ),
                None => crate::tape::read_row(
                    &format!("{}:end", self.site),
                    &self.path,
                    crate::tape::TapeOutcome::Facts {
                        facts: "end of directory".to_owned(),
                    },
                    None,
                ),
            }
            crate::tape::restore_observer_errno(saved_errno);
            result
        }
    }
}

fn refuse_foreign_pool_entries(pool: &Path) -> Result<(), LeaseError> {
    #[cfg(not(test))]
    let device = fs::symlink_metadata(pool).map_err(io_error)?.dev();
    #[cfg(test)]
    let device = observe_pool_metadata("pool:entries:root", pool)
        .map_err(io_error)?
        .dev();
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
        // A test fixture's fake volume identity owns no Barge account; its
        // slots keep the pre-adoption directory path.
        storage: None,
        pending_hold: None,
        budget,
        ledger,
        reserve,
        admission_note: None,
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
        root: None,
        rank,
        recorded: None,
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

