//! Exclusive pool: short-lived flock, durable JSON ledger, idle-only eviction.

mod held_eviction;

use super::backing::BackingPaths;
use super::slice::{AdmissionClass, OneUseSlice, RestoreOutcome, SliceReport, VictimPolicy};
use super::{
    canonical_existing_dir, current_uid, ensure_private_dir, io_error, is_finder_metadata,
    is_tolerated_metadata_file, persist_private_file, random_hex, read_private_file,
    unexpected_entry, AccountingCoverage, AccountingScan, LeaseEnvironment, LeaseError, LeaseGrant,
    Leased, LedgerSlot, LedgerView, MeasuredPool, PassiveSnapshot, PoolStatus, Profile,
    RecordedFact, RecordedVolume, ReleaseReceipt, SlotOccupancy, SlotStatus, UnmeasuredSlot,
    ACCOUNTING_MAX_ELAPSED_MS, CAPACITY_POLICY_VERSION, CONFIG_NAME, LEASE_VERSION, LEDGER_NAME,
    LEDGER_VERSION, LEGACY_CAPACITY_POLICY_VERSION, LEGACY_MAX_SLOTS, LOCK_NAME, MANAGED_NAME,
    MAX_HOLDER_BYTES, MAX_LEASE_ID_BYTES, MAX_PATH_BYTES, MAX_SLOTS,
    PREVIOUS_CAPACITY_POLICY_VERSION, RAISED_CAPACITY_POLICY_VERSION, SLOTS_NAME,
};
use super::{BUDGET_BACKEND_DIRECTORY_V1, VOLUME_BACKEND_APFS_V1};
use crate::fs_util::open_directory_at;
use crate::volume::Binding;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
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
    /// The registered removable volume's free-space reserve, separate from
    /// the pool budget. Below reserve, admission tries idle slots until free
    /// space recovers or no eligible root remains. The floor decides refusal.
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

/// The retired holding area's name, still tolerated in an older pool's payload
/// entry set so a pre-adoption pool keeps validating. Nothing is written there.
const HOLDING_NAME: &str = ".oxide-holding";

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
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The Barge label for a new slot root: the project folder and profile
/// squeezed into one label (lowercase letters, digits and `-`, at most 32, no
/// trailing `-`). The root's own random id follows it, so labels repeat
/// freely.
fn barge_label(layout: Option<&SlotLayout>) -> String {
    let profile = layout
        .map(|layout| layout.profile.as_str())
        .unwrap_or("slot");
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

const LEGACY_LEDGER_VERSION: u32 = 1;

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
    /// This row has completed the Barge transition. On a held row it means
    /// the grant took a Barge hold, so recovery can test that hold.
    #[serde(default, skip_serializing_if = "is_false")]
    owned: bool,
    /// Idle rank from this pool's monotonic sequence (oxide#118). Legacy
    /// records keep 0 and sort oldest.
    #[serde(default, skip_serializing_if = "is_zero_rank")]
    rank: u64,
    /// A permanently refused legacy path is retained outside Barge custody.
    #[serde(default, skip_serializing_if = "is_false")]
    adoption_refused: bool,
    /// A logical-byte observation bound to the lease generation. The former
    /// allocated-byte field can still be present in an older ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recorded: Option<serde_json::Value>,
    // Read-tolerant retired deletion journal fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hold_intent: Option<serde_json::Value>,
}

impl SlotRecord {
    fn area_folder(&self) -> Option<&str> {
        self.dir
            .as_deref()?
            .split_once('/')
            .map(|(folder, _)| folder)
    }

    fn is_idle(&self) -> bool {
        self.state == SlotPersistState::Idle && self.lease_id.is_none()
    }

    fn recorded_size(&self) -> Option<RecordedFact> {
        let record = self.recorded.as_ref()?;
        if self.state == SlotPersistState::Evicting
            || record.get("for_lease")?.as_str()? != self.last_lease_id
        {
            return None;
        }
        Some(RecordedFact {
            logical_bytes: record.get("logical_bytes")?.as_u64()?,
            entries: record.get("entries")?.as_u64()?,
            at_ms: record.get("at_ms")?.as_u64()?,
            current: true,
        })
    }
}

/// Only the front door's holder format supplies a PID that can be checked.
/// An opaque or malformed legacy token supplies no evidence of closure.
fn legacy_writer_absent(holder: &str) -> bool {
    let Some((pid, ticket)) = holder
        .strip_prefix("oxide-")
        .and_then(|holder| holder.split_once('-'))
    else {
        return false;
    };
    let canonical_number =
        |text: &str| !text.starts_with('0') && text.bytes().all(|byte| byte.is_ascii_digit());
    if !canonical_number(pid) || !canonical_number(ticket) || ticket.parse::<u64>().is_err() {
        return false;
    }
    let Ok(pid) = pid.parse::<libc::pid_t>() else {
        return false;
    };
    if pid <= 1 {
        return false;
    }
    // SAFETY: signal zero only asks whether this positive PID exists.
    let result = unsafe { libc::kill(pid, 0) };
    result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

fn observed_size(slot: &SlotRecord, path: &Path, deadline: Instant) -> Option<serde_json::Value> {
    let (logical_bytes, entries) = slot_tree_bytes(path, deadline)?;
    let at_ms = std::time::UNIX_EPOCH.elapsed().ok()?.as_millis();
    Some(serde_json::json!({
        "for_lease": slot.last_lease_id,
        "logical_bytes": logical_bytes,
        "entries": entries,
        "at_ms": u64::try_from(at_ms).ok()?,
    }))
}

/// Logical bytes below `path`, read-only, bounded by a deadline: `None` when
/// the walk could not finish or an entry could not be read, so an unknown
/// figure is never reported as a measured one.
fn slot_tree_bytes(path: &Path, deadline: Instant) -> Option<(u64, u64)> {
    #[cfg(test)]
    crate::storage::observe_pool_walk(crate::storage::PoolWalk::Measure(path.to_path_buf()));

    if Instant::now() >= deadline {
        return None;
    }
    let root = fs::symlink_metadata(path).ok()?;
    let parent = fs::symlink_metadata(path.parent()?).ok()?;
    if !root.is_dir() || !parent.is_dir() || parent.dev() != root.dev() {
        return None;
    }
    let device = root.dev();
    let mut directories = vec![path.to_path_buf()];
    let mut total = 0_u64;
    let mut entries = 0_u64;
    while let Some(directory) = directories.pop() {
        if Instant::now() >= deadline {
            return None;
        }
        let metadata = fs::symlink_metadata(&directory).ok()?;
        if !metadata.is_dir() || metadata.dev() != device {
            return None;
        }
        for entry in fs::read_dir(directory).ok()? {
            if Instant::now() >= deadline {
                return None;
            }
            let child = entry.ok()?.path();
            let metadata = fs::symlink_metadata(&child).ok()?;
            if metadata.is_symlink() || metadata.dev() != device {
                continue;
            }
            entries = entries.saturating_add(1);
            if metadata.is_dir() {
                directories.push(child);
            } else {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    (Instant::now() < deadline).then_some((total, entries))
}

/// Coverage for a complete logical observation, or an unknown figure.
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

fn is_false(value: &bool) -> bool {
    !*value
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
    root: Option<String>,
    owned: bool,
}

/// The pool and ledger facts call 1 and call 2 may read.
pub(super) struct SeedPoolView {
    pub(super) store_id: String,
    pub(super) pool_root: PathBuf,
    /// The payload root. Call 1 and the sidecar writers need no payload, so
    /// their seam leaves this `None` and opens no volume; call 2 uses
    /// [`open_seed_writer`] and gets the verified root for both backends.
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
/// nothing. [`open_seed`] opens no payload; [`open_seed_writer`] additionally
/// opens the verified payload call 2 clones from, including a removable
/// `apfs-volume-v1` volume's mount and volume-UID binding (§7, §8.3).
pub(super) struct SeedPoolOpen {
    _lock: File,
    pub(super) view: SeedPoolView,
    storage: Option<crate::storage::PoolStorage>,
}

impl SeedPoolOpen {
    /// Share the donor's Barge hold for every payload read and clone. The
    /// recipient's exclusive hold belongs to the caller's live lease and is
    /// retained through this call; a ledger row alone does not replace it.
    pub(super) fn hold_donor(
        &self,
        donor: &SeedSlotView,
        recipient: &SeedSlotView,
    ) -> Result<Option<barge::Hold>, LeaseError> {
        let storage = self.storage.as_ref().ok_or(LeaseError::BackingMismatch)?;
        if !recipient.owned {
            return Err(LeaseError::UnknownAccounting);
        }
        let recipient_root = recipient
            .root
            .as_deref()
            .ok_or(LeaseError::UnknownAccounting)?;
        let recipient_project = recipient
            .dir
            .as_deref()
            .and_then(|dir| dir.split_once('/'))
            .map(|(folder, _)| folder);
        let recipient_area = storage
            .area_for(recipient_project)
            .map_err(LeaseError::Io)?;
        // A shared probe conflicts only with an exclusive holder. A free or
        // merely shared lock cannot protect this recipient's writes.
        if recipient_area
            .try_hold(recipient_root, barge::Mode::Shared)
            .map_err(|error| LeaseError::Io(error.to_string()))?
            .is_some()
        {
            return Err(LeaseError::UnknownAccounting);
        }
        if !donor.owned {
            return Ok(None);
        }
        let Some(root) = donor.root.as_deref() else {
            return Ok(None);
        };
        let project = donor
            .dir
            .as_deref()
            .and_then(|dir| dir.split_once('/'))
            .map(|(folder, _)| folder);
        storage
            .area_for(project)
            .map_err(LeaseError::Io)?
            .try_hold(root, barge::Mode::Shared)
            .map_err(|error| LeaseError::Io(error.to_string()))
    }

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
    open_seed_inner(root, false, false)
}

/// The same seam, additionally opening the payload call 2 clones from. For a
/// removable `apfs-volume-v1` pool this verifies the recorded mount and volume
/// UUID through [`Binding::open`] and the recorded owner-only payload
/// directory, under the pool lock and before any donor payload is read.
pub(super) fn open_seed_writer(
    root: &Path,
    with_payload: bool,
) -> Result<SeedPoolOpen, LeaseError> {
    open_seed_inner(root, with_payload, true)
}

fn open_seed_inner(
    root: &Path,
    with_payload: bool,
    writer: bool,
) -> Result<SeedPoolOpen, LeaseError> {
    let root = canonical_existing_dir(root, true)?;
    refuse_foreign_pool_entries(&root)?;
    refuse_retired_image_pool(&root)?;
    let lock = lock_pool(&root)?;
    if writer {
        // The front-door admission holder publishes the generation. Recheck
        // it here without upgrading the donor's legacy custody document.
        super::check_control_writer(&root.join(MANAGED_NAME))?;
        if held_eviction::HeldEvictionIntent::read(&root)?.is_some() {
            return Err(LeaseError::UnknownAccounting);
        }
    }
    let config = read_config(&root)?;
    let ledger = read_ledger(&root)?;
    if ledger.store_id != config.store_id || ledger.cap_bytes != config.cap_bytes {
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
        storage: if with_payload {
            pool_storage(&root, &config)?
        } else {
            None
        },
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
                    root: slot.root.clone(),
                    owned: slot.owned,
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
    /// A passive measurement's absolute deadline; a mutating open has none.
    deadline: Option<std::time::Instant>,
    ledger: Ledger,
    /// Admission budget: the recorded one unless the request overrides it.
    budget: u64,
    /// A bounded note the caller prints when admission had to report a policy
    /// decision (for example a fallback or a warning).
    admission_note: Option<String>,
}

/// Entry points for the `oxide` front door. They open a budget-only
/// directory or removable-volume pool; a pool recording the retired APFS
/// sparse-image backend refuses with [`LeaseError::BackingMismatch`].
pub mod managed {
    use super::super::backing::BackingPaths;
    use super::super::{
        canonical_existing_dir, ensure_private_dir, io_error, LeaseError, Leased, LedgerView,
        MeasuredPool, PassiveSnapshot, PoolInfo, PoolStatus, ReleaseReceipt, VolumeDescription,
        ACCOUNTING_MAX_ELAPSED_MS, BUDGET_BACKEND_DIRECTORY_V1, CONFIG_NAME, LEASE_VERSION,
        LEDGER_NAME, LEDGER_VERSION, MAX_SLOTS, PAYLOAD_NAME, SLOTS_NAME, VOLUME_BACKEND_APFS_V1,
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
            version: LEDGER_VERSION,
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
    pub fn drain_holding(
        _pool: &Path,
        _prefer: Option<&str>,
    ) -> Result<crate::lease::HoldingDrain, LeaseError> {
        Ok(crate::lease::HoldingDrain::default())
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
        let opened =
            crate::release_trace::phase("pool open", || Pool::open_for_release(pool, None))?;
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
            version: LEDGER_VERSION,
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

    /// Take a lease, including the root's cooperative hold. The caller keeps
    /// the hold for as long as it uses the root.
    pub fn lease(request: LeaseRequest) -> Result<Leased, LeaseError> {
        lease_measured(request)
    }

    /// [`lease`], with this admission's accounting notes.
    pub fn lease_measured(request: LeaseRequest) -> Result<Leased, LeaseError> {
        Pool::open(&request.pool)?.lease(&request)
    }

    pub fn release(pool: &Path, lease_id: &str) -> Result<ReleaseReceipt, LeaseError> {
        crate::release_trace::phase("pool open", || Pool::open_for_release(pool, Some(lease_id)))?
            .release_or_recover(lease_id)
    }

    pub fn recover(pool: &Path, lease_id: &str) -> Result<ReleaseReceipt, LeaseError> {
        Pool::open_for_release(pool, Some(lease_id))?.release_or_recover(lease_id)
    }

    pub fn status(pool: &Path) -> Result<PoolStatus, LeaseError> {
        Pool::open_passive(
            pool,
            std::time::Instant::now() + Duration::from_millis(ACCOUNTING_MAX_ELAPSED_MS),
        )?
        .accounted(false)
    }

    /// Prepare a pool at the front door before leasing. Only pending legacy
    /// rows enter the one-time adoption transition.
    pub fn prepare_pool(pool: &Path) -> Result<LedgerView, LeaseError> {
        let mut pool = Pool::open(pool)?;
        pool.adopt_legacy_roots()?;
        pool.ledger_view()
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
        if ledger.store_id != config.store_id
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
    let mut leased = managed::lease(request)?;
    if let Some(hold) = leased.hold.take() {
        if let Ok(mut holds) = injected_holds().lock() {
            holds.insert(leased.grant.lease_id.clone(), hold);
        }
    }
    Ok(leased.grant)
}

/// Holds the test helpers took, keyed by lease. A test release or recovery
/// drops the hold first, exactly as the front door drops its `Lease` hold.
#[cfg(test)]
fn injected_holds() -> &'static std::sync::Mutex<std::collections::HashMap<String, barge::Hold>> {
    static HOLDS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, barge::Hold>>,
    > = std::sync::OnceLock::new();
    HOLDS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[cfg(test)]
fn drop_injected_hold(lease_id: &str) {
    let _ = injected_holds()
        .lock()
        .map(|mut holds| holds.remove(lease_id));
}

#[cfg(test)]
pub(super) fn release_injected(pool: &Path, lease_id: &str) -> Result<ReleaseReceipt, LeaseError> {
    drop_injected_hold(lease_id);
    managed::release(pool, lease_id)
}

#[cfg(test)]
pub(super) fn prune_injected(pool: &Path) -> Result<(), LeaseError> {
    Pool::open(pool)?.prune_storage()
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

impl Pool {
    /// Open a budget-only directory or removable-volume pool. Any other
    /// recorded backend, including the retired `apfs-sparse-image-v1`,
    /// refuses before anything under the pool is touched.
    pub(super) fn open(root: &Path) -> Result<Self, LeaseError> {
        Self::open_inner(root, false, None, true)
    }

    fn open_for_release(root: &Path, lease_id: Option<&str>) -> Result<Self, LeaseError> {
        Self::open_inner(root, true, lease_id, true)
    }

    fn open_held_eviction(root: &Path) -> Result<Self, LeaseError> {
        Self::open_inner(root, false, None, false)
    }

    fn open_inner(
        root: &Path,
        trace_release: bool,
        releasing: Option<&str>,
        reconcile: bool,
    ) -> Result<Self, LeaseError> {
        let root = canonical_existing_dir(root, true)?;
        refuse_foreign_pool_entries(&root)?;
        refuse_retired_image_pool(&root)?;
        let lock = if trace_release {
            crate::release_trace::phase("pool lock", || lock_pool_inner(&root, true, true))?
        } else if reconcile {
            lock_pool(&root)?
        } else {
            lock_pool_inner(&root, false, false)?
        };
        let pending = if reconcile {
            held_eviction::HeldEvictionIntent::read(&root)?
        } else {
            None
        };
        // Fence first (oxide#177, ruling 4c9984a1): a managed pool's control
        // state reaches the current fence version before this open can start a
        // recovery journal or finish a recorded deletion. A library fixture
        // has no control state and nothing to fence.
        if reconcile {
            super::fence_managed_control(&root)?;
        } else {
            super::check_control_writer(&root.join(MANAGED_NAME))?;
        }
        let config = read_config(&root)?;
        let paths = BackingPaths::under(&root);
        let backing = match config.backend.as_str() {
            BUDGET_BACKEND_DIRECTORY_V1 => open_budget_payload(&root, &paths, &config)?,
            VOLUME_BACKEND_APFS_V1 => open_volume_payload(&paths, &config)?,
            _ => return Err(LeaseError::BackingMismatch),
        };
        let ledger = read_ledger(&root)?;
        if ledger.store_id != config.store_id || ledger.cap_bytes != config.cap_bytes {
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
            storage: None,
            pending_hold: None,
            deadline: None,
            budget: ledger.cap_bytes,
            ledger,
            admission_note: None,
        };
        if reconcile {
            if let Some(intent) = pending {
                pool.recover_held_eviction(&intent, false)?;
            }
            pool.storage = pool_storage(&pool.root, &pool.config)?;
            pool.recover_abandoned_holds(releasing)?;
            pool.retire_missing_idle_roots()?;
            pool.sweep_trash()?;
        }
        Ok(pool)
    }

    /// Open the same verified backing without the pool lock and without
    /// reconciliation, for [`measure`] only. The recorded payload must still
    /// be the backend's real, verified payload root, so the walks cover what
    /// admission would cover; nothing is created or written.
    fn open_passive(root: &Path, deadline: std::time::Instant) -> Result<Self, LeaseError> {
        #[cfg(test)]
        PASSIVE_OPEN_DELAY.with(|delay| std::thread::sleep(delay.get()));
        measurement_remaining(deadline)?;
        let root = canonical_existing_dir(root, true)?;
        measurement_remaining(deadline)?;
        refuse_foreign_pool_entries_before(&root, Some(deadline))?;
        refuse_retired_image_pool(&root)?;
        measurement_remaining(deadline)?;
        let (config, ledger) = read_consistent_pair_before(&root, Some(deadline))?;
        let paths = BackingPaths::under(&root);
        let backing = match config.backend.as_str() {
            BUDGET_BACKEND_DIRECTORY_V1 => open_budget_payload(&root, &paths, &config)?,
            VOLUME_BACKEND_APFS_V1 => open_volume_payload(&paths, &config)?,
            _ => return Err(LeaseError::BackingMismatch),
        };
        measurement_remaining(deadline)?;
        Ok(Self {
            _lock: None,
            deadline: Some(deadline),
            root,
            config,
            backing,
            storage: None,
            pending_hold: None,
            budget: ledger.cap_bytes,
            ledger,
            admission_note: None,
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

    fn persist(&mut self) -> Result<(), LeaseError> {
        self.ledger.version = LEDGER_VERSION;
        write_ledger(&self.root, &self.ledger)?;
        Ok(())
    }

    /// A revised mutation: install the permanent capacity-policy fence if
    /// this ledger predates it, raise that fence when the slot table now
    /// exceeds the legacy ceiling, validate every stored field against the
    /// raised caps, then replace the ledger atomically. Legacy representation
    /// is never migrated by a read-only observer or by reconciliation alone.
    fn persist_revised(&mut self) -> Result<(), LeaseError> {
        self.validate_revised_ledger()?;
        self.persist()
    }

    fn validate_revised_ledger(&mut self) -> Result<(), LeaseError> {
        self.ensure_capacity_policy()?;
        self.raise_capacity_policy_if_needed();
        validate_ledger_write(&self.ledger)
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
    fn recover_abandoned_holds(&mut self, releasing: Option<&str>) -> Result<(), LeaseError> {
        let Some(storage) = self.storage.clone() else {
            return Ok(());
        };
        let mut changed = false;
        for slot in &mut self.ledger.slots {
            if slot.state != SlotPersistState::Held || releasing == slot.lease_id.as_deref() {
                continue;
            }
            if !slot.owned {
                // A legacy generation never held this lock, so a free lock says
                // nothing about its writer. Keep it held until an explicit
                // recovery settles it.
                continue;
            }
            let Some(root) = slot.root.clone() else {
                continue;
            };
            let project = slot
                .dir
                .as_deref()
                .and_then(|dir| dir.split_once('/'))
                .map(|(folder, _)| folder.to_owned());
            if let Ok(Some(hold)) = storage.try_hold_in(project.as_deref(), &root) {
                drop(hold);
                slot.state = SlotPersistState::Idle;
                slot.lease_id = None;
                slot.recorded = None;
                slot.hold_intent = None;
                slot.owned = true;
                changed = true;
            }
        }
        if changed {
            self.persist()?;
        }
        Ok(())
    }

    /// Finish adoption on the front door's preparation path. A recorded
    /// root is never sent back through Barge's recursive adoption check.
    fn adopt_legacy_roots(&mut self) -> Result<(), LeaseError> {
        let Some(storage) = self.storage.clone() else {
            return Ok(());
        };
        for index in 0..self.ledger.slots.len() {
            let slot = &self.ledger.slots[index];
            if slot.owned
                || slot.adoption_refused
                || !slot.is_idle()
                || !legacy_writer_absent(&slot.holder)
            {
                continue;
            }
            let path = self.slot_path(slot.id)?;
            let folder = slot.dir.clone().unwrap_or_else(|| slot.id.to_string());
            let leaf = folder
                .rsplit('/')
                .next()
                .ok_or(LeaseError::UnknownAccounting)?;
            let id = legacy_root_id(&folder);
            let label = slot.profile.as_str();
            let area = storage
                .area_for(slot.area_folder())
                .map_err(|_| LeaseError::UnknownAccounting)?;
            let adopted = match area.resolve(&id) {
                // The pinned Barge API can unpin by ID but cannot compare
                // pin provenance under its lock. Keep any existing pin.
                Ok(root) => {
                    root.path == path
                        && root.label == label
                        && slot.root.as_deref().is_none_or(|recorded| recorded == id)
                        && fs::symlink_metadata(&path).is_ok_and(|metadata| {
                            metadata.is_dir()
                                && metadata.uid() == current_uid()
                                && metadata.mode() & 0o022 == 0
                        })
                }
                Err(error) if error.kind() == barge::ErrorKind::NotFound => {
                    // A published id is never attached to a replacement path.
                    if slot.root.is_some() {
                        false
                    } else {
                        match storage.adopt_in(slot.area_folder(), leaf, label, &id) {
                            Ok(root) => root.path == path,
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    barge::ErrorKind::Refused | barge::ErrorKind::NotFound
                                ) =>
                            {
                                false
                            }
                            Err(_) => return Err(LeaseError::UnknownAccounting),
                        }
                    }
                }
                Err(_) => return Err(LeaseError::UnknownAccounting),
            };
            if adopted {
                let slot = &mut self.ledger.slots[index];
                slot.root = Some(id);
                slot.owned = true;
            } else {
                self.ledger.slots[index].adoption_refused = true;
            }
            // Publish each transition before visiting another root. If the
            // write failed after adoption, the next preparation resolves the
            // deterministic id above without another recursive adoption.
            self.persist()?;
        }
        Ok(())
    }

    /// A completed Barge rename may precede the workflow ledger write. Only a
    /// positively absent, transitioned Barge root can retire its idle row;
    /// existing or unreadable paths stay recorded and are never re-adopted.
    fn retire_missing_idle_roots(&mut self) -> Result<(), LeaseError> {
        let mut missing = Vec::new();
        for slot in &self.ledger.slots {
            if slot.owned
                && slot.root.is_some()
                && slot.state == SlotPersistState::Idle
                && slot.lease_id.is_none()
                && fs::symlink_metadata(self.slot_path(slot.id)?)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                missing.push(slot.id);
            }
        }
        if !missing.is_empty() {
            self.ledger.slots.retain(|slot| !missing.contains(&slot.id));
            self.persist_revised()?;
        }
        Ok(())
    }

    /// Commit Barge's completed evictions to workflow membership before the
    /// next compatible-root lookup. Barge still owns all deletion decisions.
    #[cfg(test)]
    fn prune_storage(&mut self) -> Result<(), LeaseError> {
        let storage = self.storage.as_ref().ok_or(LeaseError::UnknownAccounting)?;
        let report = storage
            .prune(false)
            .map_err(|_| LeaseError::UnknownAccounting)?;
        let mut retired = Vec::new();
        for slot in &self.ledger.slots {
            if let Some(id) = slot.root.as_deref() {
                let path = self.slot_path(slot.id)?;
                if report
                    .evicted
                    .iter()
                    .any(|root| root.id == id && root.path == path)
                {
                    retired.push(slot.id);
                }
            }
        }
        if !retired.is_empty() {
            self.ledger.slots.retain(|slot| !retired.contains(&slot.id));
            self.persist_revised()?;
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
        if let (Some(storage), Some(reserve)) = (self.storage.as_ref(), request.reserve_bytes) {
            storage
                .set_reserve(reserve)
                .map_err(|_| LeaseError::UnknownAccounting)?;
        }
        let project = canonical_existing_dir(&request.project, false)?;
        let workspace = canonical_existing_dir(&request.workspace, false)?;
        require_holder(&request.holder)?;
        if let Some(existing) = self.find_holder_lease(&request.holder) {
            if existing.state == SlotPersistState::Held {
                // The same holder already owns this generation. A second grant
                // would carry no hold of its own, so it could outlive the
                // original holder and leave the root unprotected; refuse it and
                // let the caller finish its existing lease first.
                return Err(LeaseError::Busy);
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
        if self
            .storage
            .as_ref()
            .is_some_and(crate::storage::PoolStorage::is_removable)
        {
            self.evict_for_reserve(candidate.map(|(id, _)| id))?;
        }
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
        let (grant, unmeasured) = self.allocate(project, workspace, request)?;
        Ok(Leased {
            grant,
            hold: None,
            measured: None,
            admission_note: self.admission_note.take(),
            slice: slice_report,
            unmeasured,
            holding: Vec::new(),
        })
    }

    /// The idle compatible slot this admission would reuse, if any.
    fn idle_compatible(&self, project: &Path, profile: Profile) -> Option<(u64, u64)> {
        self.ledger
            .slots
            .iter()
            .find(|slot| slot.is_idle() && slot.project == project && slot.profile == profile)
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
        let Some(slot) = self.slot(slot_id) else {
            return Err(LeaseError::UnknownAccounting);
        };
        let Some(root) = slot.root.as_deref().filter(|_| slot.owned) else {
            return Err(LeaseError::Busy);
        };
        let storage = self.storage.as_ref().ok_or(LeaseError::UnknownAccounting)?;
        let hold = storage
            .try_hold_in(slot.area_folder(), root)
            .map_err(|_| LeaseError::UnknownAccounting)?
            .ok_or(LeaseError::Busy)?;
        self.pending_hold = Some(hold);
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
    ) -> Result<(LeaseGrant, Vec<UnmeasuredSlot>), LeaseError> {
        if let Some(victim) = request.replace_slot {
            self.retire_idle_slot(victim)?;
        }
        // Retire count victims before measuring any unrecorded survivors.
        if self.ledger.slots.len() >= MAX_SLOTS && !self.evict_one_idle_with(|_| true)? {
            return Err(LeaseError::LedgerExhausted);
        }
        let (charges, unmeasured) = self.admission_charges()?;
        while self
            .ledger
            .slots
            .iter()
            .map(|slot| charges[&slot.id])
            .fold(0, u64::saturating_add)
            >= self.budget
        {
            if !self.evict_one_idle_with(|id| {
                !unmeasured.iter().any(|slot| slot.slot_id == id.to_string())
            })? {
                return Err(LeaseError::Full);
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
            owned: true,
            adoption_refused: false,
            rank: 0,
            recorded: None,
            capture: None,
            hold_intent: None,
        });
        self.ledger.next_slot = slot_id.checked_add(1).ok_or(LeaseError::LedgerExhausted)?;
        self.stamp_rank(slot_id)?;
        if let Err(error) = self.persist_revised() {
            #[cfg(test)]
            eprintln!("DBG persist_revised: {error:?}");
            return Err(error);
        }
        let slot = self.slot(slot_id).ok_or(LeaseError::UnknownAccounting)?;
        if let Err(error) = self.grant_from(slot, false) {
            #[cfg(test)]
            eprintln!("DBG grant_from: {error:?}");
            return Err(error);
        }
        Ok((self.grant_from(slot, false)?, unmeasured))
    }

    fn release_or_recover(&mut self, lease_id: &str) -> Result<ReleaseReceipt, LeaseError> {
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
        let recorded = crate::release_trace::phase("release walk", || {
            let slot = &self.ledger.slots[index];
            self.slot_path(slot.id).ok().and_then(|path| {
                observed_size(
                    slot,
                    &path,
                    Instant::now() + Duration::from_millis(ACCOUNTING_MAX_ELAPSED_MS),
                )
            })
        });
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
    /// Sweep owned trash a crash left behind.
    fn sweep_trash(&self) -> Result<(), LeaseError> {
        if let Some(storage) = self.storage.as_ref() {
            storage
                .sweep_all_trash()
                .map_err(|_| LeaseError::UnknownAccounting)?;
        }
        Ok(())
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

    /// Retire one idle recorded slot: Barge evicts the owned root (refusing a
    /// held or pinned one) and the ledger row is dropped. Nothing is moved
    /// outside Barge and no journal is written.
    fn retire_idle_slot(&mut self, id: u64) -> Result<(), LeaseError> {
        let Some(slot) = self.slot(id) else {
            return Err(LeaseError::Busy);
        };
        if !slot.is_idle() || !slot.owned {
            return Err(LeaseError::Busy);
        }
        let root = slot.root.as_deref().ok_or(LeaseError::UnknownAccounting)?;
        let storage = self.storage.as_ref().ok_or(LeaseError::UnknownAccounting)?;
        storage
            .evict_in(slot.area_folder(), root)
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
        let id =
            slot_id
                .parse::<u64>()
                .ok()
                .filter(|id| *id != 0)
                .ok_or(LeaseError::InvalidInput(
                    "slot ID must be a positive integer",
                ))?;
        self.retire_idle_slot(id)?;
        Ok(Vec::new())
    }

    fn volume_space(&self) -> Result<barge::Space, LeaseError> {
        self.storage
            .as_ref()
            .ok_or(LeaseError::UnknownAccounting)?
            .space()
            .map_err(|_| LeaseError::UnknownAccounting)
    }

    fn evict_for_reserve(&mut self, reuse: Option<u64>) -> Result<(), LeaseError> {
        let mut space = self.volume_space()?;
        // Keep the compatible root the admission is about to claim. Reread
        // actual free space after every completed eviction.
        while space.free_bytes < space.reserve_bytes
            && self.evict_one_idle_with(|id| Some(id) != reuse)?
        {
            space = self.volume_space()?;
        }
        if space.free_bytes < space.floor_bytes {
            return Err(LeaseError::Full);
        }
        Ok(())
    }

    /// Retire the oldest eligible slot. Barge checks the actual hold and pin
    /// of the named root, including a hold inherited by a child.
    fn evict_one_idle_with(&mut self, eligible: impl Fn(u64) -> bool) -> Result<bool, LeaseError> {
        let mut candidates: Vec<(u64, u64)> = self
            .ledger
            .slots
            .iter()
            .filter(|slot| slot.is_idle() && slot.owned && slot.root.is_some() && eligible(slot.id))
            .map(|slot| (slot.rank, slot.id))
            .collect();
        candidates.sort_unstable();
        for (_, id) in candidates {
            match self.retire_idle_slot(id) {
                Ok(()) => return Ok(true),
                Err(LeaseError::Busy) => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(false)
    }

    /// Sample only slots lacking a current observation, once before pressure
    /// decisions. Partial walks get a positive, unpublished estimate and
    /// cannot nominate a byte-pressure victim.
    fn admission_charges(
        &mut self,
    ) -> Result<(BTreeMap<u64, u64>, Vec<UnmeasuredSlot>), LeaseError> {
        let deadline = Instant::now() + Duration::from_millis(ACCOUNTING_MAX_ELAPSED_MS);
        let mut charges = BTreeMap::new();
        let mut missing = Vec::new();
        let mut changed = false;
        for index in 0..self.ledger.slots.len() {
            let slot = &self.ledger.slots[index];
            if slot.recorded_size().is_none()
                && slot.state != SlotPersistState::Evicting
                && Instant::now() < deadline
            {
                let record = self
                    .slot_path(slot.id)
                    .ok()
                    .and_then(|path| observed_size(slot, &path, deadline));
                if let Some(record) = record {
                    self.ledger.slots[index].recorded = Some(record);
                    changed = true;
                }
            }
            let slot = &self.ledger.slots[index];
            if let Some(record) = slot.recorded_size() {
                charges.insert(slot.id, record.logical_bytes);
            } else {
                missing.push((
                    slot.id,
                    UnmeasuredSlot {
                        slot_id: slot.id.to_string(),
                        incarnation: slot.incarnation.to_string(),
                        dir: slot.dir.clone(),
                    },
                ));
            }
        }
        if changed {
            self.persist_revised()?;
        }
        let estimate = charges.values().copied().max().unwrap_or(0).max(1);
        let unmeasured = missing
            .into_iter()
            .map(|(id, slot)| {
                charges.insert(id, estimate);
                slot
            })
            .collect();
        Ok((charges, unmeasured))
    }

    /// Passive status reads recorded sizes. Explicit measurement supplies the
    /// caller's deadline and walks one recorded slot at a time.
    fn accounted(&self, measure: bool) -> Result<PoolStatus, LeaseError> {
        let mut slots = Vec::new();
        let mut incomplete = 0_u64;
        let mut total = 0_u64;
        for slot in &self.ledger.slots {
            let measured = if measure {
                self.deadline.and_then(|deadline| {
                    self.slot_path(slot.id)
                        .ok()
                        .and_then(|path| slot_tree_bytes(&path, deadline))
                        .map(|(bytes, _)| bytes)
                })
            } else {
                slot.recorded_size().map(|record| record.logical_bytes)
            };
            if measured.is_none() {
                incomplete += 1;
            }
            total = total.saturating_add(measured.unwrap_or(0));
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
                logical_bytes: measured,
                unique_allocated_bytes: None,
                accounting: barge_scan(measured),
            });
        }
        let coverage = if incomplete == 0 {
            AccountingCoverage::Complete
        } else {
            AccountingCoverage::Partial
        };
        let full = (incomplete == 0).then_some(total >= self.config.cap_bytes);
        Ok(PoolStatus {
            version: LEASE_VERSION,
            store_id: self.config.store_id.clone(),
            backend: self.config.backend.clone(),
            cap_bytes: self.config.cap_bytes,
            payload_logical_bytes: (incomplete == 0).then_some(total),
            payload_unique_allocated_bytes: None,
            coverage,
            payload_accounting: barge_scan((incomplete == 0).then_some(total)),
            full,
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
    read_consistent_pair_before(root, None)
}

fn read_consistent_pair_before(
    root: &Path,
    deadline: Option<std::time::Instant>,
) -> Result<(PoolConfig, Ledger), LeaseError> {
    for _ in 0..2 {
        if let Some(deadline) = deadline {
            measurement_remaining(deadline)?;
        }
        let config = read_config(root)?;
        if let Some(deadline) = deadline {
            measurement_remaining(deadline)?;
        }
        let ledger = read_ledger(root)?;
        if let Some(deadline) = deadline {
            measurement_remaining(deadline)?;
        }
        if ledger.store_id == config.store_id
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
    let mut recorded_slots = 0;
    let mut total = 0_u64;
    for record in ledger.slots.iter().filter_map(SlotRecord::recorded_size) {
        recorded_slots += 1;
        total = total.saturating_add(record.logical_bytes);
    }
    PassiveSnapshot {
        store_id: config.store_id.clone(),
        backend: config.backend.clone(),
        cap_bytes: config.cap_bytes,
        payload,
        volume,
        recorded_slots,
        unrecorded_slots: ledger.slots.len() - recorded_slots,
        recorded_total: (recorded_slots > 0).then_some(total),
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
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_millis(max_elapsed_ms.min(ACCOUNTING_MAX_ELAPSED_MS));
    let opened = Pool::open_passive(pool, deadline)?;
    let snapshot = opened.recorded();
    let mut status = opened.accounted(true)?;
    let stable = match read_consistent_pair_before(&opened.root, Some(deadline)) {
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
) -> Result<Option<crate::storage::PoolStorage>, LeaseError> {
    crate::storage::PoolStorage::open(&root.join(MANAGED_NAME), &pool_backing(root, config)?)
        .map(Some)
        .map_err(|_| LeaseError::BackingMismatch)
}

fn pool_backing(
    root: &Path,
    config: &PoolConfig,
) -> Result<crate::storage::PoolBacking, LeaseError> {
    use crate::storage::PoolBacking;
    Ok(match config.backend.as_str() {
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
    })
}

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
        recorded: slot.recorded_size(),
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
    if !matches!(ledger.version, LEGACY_LEDGER_VERSION | LEDGER_VERSION) {
        return Err(LeaseError::BackingMismatch);
    }
    validate_ledger_read(&ledger)?;
    Ok(ledger)
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
    let metadata = fs::symlink_metadata(path);
    match metadata {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

fn refuse_foreign_pool_entries(pool: &Path) -> Result<(), LeaseError> {
    refuse_foreign_pool_entries_before(pool, None)
}

fn refuse_foreign_pool_entries_before(
    pool: &Path,
    deadline: Option<std::time::Instant>,
) -> Result<(), LeaseError> {
    if let Some(deadline) = deadline {
        measurement_remaining(deadline)?;
    }
    let device = fs::symlink_metadata(pool).map_err(io_error)?.dev();
    for entry in fs::read_dir(pool).map_err(io_error)? {
        if let Some(deadline) = deadline {
            measurement_remaining(deadline)?;
        }
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

fn measurement_remaining(deadline: std::time::Instant) -> Result<(), LeaseError> {
    if std::time::Instant::now() >= deadline {
        Err(LeaseError::UnknownAccounting)
    } else {
        Ok(())
    }
}

#[cfg(test)]
thread_local! {
    static PASSIVE_OPEN_DELAY: std::cell::Cell<std::time::Duration> = const {
        std::cell::Cell::new(std::time::Duration::ZERO)
    };
}

#[cfg(test)]
pub(super) fn with_passive_open_delay<T>(
    delay: std::time::Duration,
    body: impl FnOnce() -> T,
) -> T {
    struct Restore(std::time::Duration);
    impl Drop for Restore {
        fn drop(&mut self) {
            PASSIVE_OPEN_DELAY.with(|slot| slot.set(self.0));
        }
    }
    let _restore = Restore(PASSIVE_OPEN_DELAY.with(|slot| slot.replace(delay)));
    body()
}

#[cfg(test)]
mod reserve_tests {
    use super::*;
    use crate::lease::tests::{request, Fixture};
    use crate::storage::{count_pool_walks, PoolBacking, PoolStorage, PoolWalk, Storage};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    #[derive(Debug)]
    struct ScriptedSpace {
        volume: barge::FakeOs,
        reads: Mutex<VecDeque<u64>>,
    }

    impl barge::Os for ScriptedSpace {
        fn volume_identity(&self, mount: &Path) -> barge::Result<Option<String>> {
            barge::Os::volume_identity(&self.volume, mount)
        }

        fn free_bytes(&self, _mount: &Path) -> barge::Result<u64> {
            self.reads
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| barge::Error::unknown_state("unexpected free-space read"))
        }
    }

    #[test]
    fn removable_admission_reads_space_and_evicts_only_needed_ledger_roots() {
        const UUID: &str = "01234567-89ab-cdef-0123-456789abcdef";
        const RESERVE: u64 = 4 << 20;
        let fixture = Fixture::new("removable-reserve");
        let pool = fixture.pool();
        let workspace = fixture.dir("workspace");
        let mut opened = Pool::open(&pool).unwrap();
        // Only mount identity and free-space readings are synthetic. The
        // leases, records, locks and evictions use the real fixture tree.
        let os = Arc::new(ScriptedSpace {
            volume: barge::FakeOs::with_volume(pool.clone(), UUID, RESERVE),
            reads: Mutex::new(VecDeque::new()),
        });
        let account = Storage::open_with(
            fixture.dir("account"),
            os.clone(),
            Arc::new(barge::ManualClock::new(1_000)),
        )
        .unwrap();
        let storage = PoolStorage::from_storage(
            account,
            &PoolBacking::volume(&pool, UUID, "payload", 8 << 20, RESERVE, RESERVE / 2),
        )
        .unwrap();
        opened.storage = Some(storage.clone());

        *os.reads.lock().unwrap() = [RESERVE / 2 - 1].into();
        let (refused, walks) = count_pool_walks(|| {
            opened.lease(&request(&pool, &fixture.dir("floor"), &workspace, "floor"))
        });
        assert!(matches!(refused, Err(LeaseError::Full)));
        assert!(walks.is_empty());

        let mut projects = Vec::new();
        let mut roots = Vec::new();
        for i in 0..24 {
            let project = fixture.dir(&format!("project-{i}"));
            *os.reads.lock().unwrap() = [RESERVE + 1].into();
            let leased = opened
                .lease(&request(&pool, &project, &workspace, "first"))
                .unwrap();
            let slot = opened.ledger.slots.last().unwrap();
            roots.push(slot.root.clone().unwrap());
            opened.release_or_recover(&leased.grant.lease_id).unwrap();
            projects.push(project);
        }

        let new_project = fixture.dir("new");
        for (project, reused) in [(&projects[0], true), (&new_project, false)] {
            *os.reads.lock().unwrap() = [RESERVE + 1].into();
            let (leased, walks) =
                count_pool_walks(|| opened.lease(&request(&pool, project, &workspace, "above")));
            let leased = leased.unwrap();
            assert_eq!(leased.grant.reused_slot, reused);
            assert!(walks.is_empty());
            assert!(os.reads.lock().unwrap().is_empty());
            opened.release_or_recover(&leased.grant.lease_id).unwrap();
        }

        storage.pin_in(None, &roots[1], "protected").unwrap();
        let hold = storage.hold(&roots[2]).unwrap();
        // The first free-space reread is still short. The second reaches
        // reserve, so the next eligible root must remain untouched.
        *os.reads.lock().unwrap() = [RESERVE - 2, RESERVE - 1, RESERVE].into();
        let (warm, walks) =
            count_pool_walks(|| opened.lease(&request(&pool, &projects[3], &workspace, "below")));
        let warm = warm.unwrap();
        assert!(warm.grant.reused_slot);
        assert_eq!(
            walks,
            vec![
                PoolWalk::Evict(roots[4].clone()),
                PoolWalk::Evict(roots[5].clone())
            ]
        );
        assert!(os.reads.lock().unwrap().is_empty());
        opened.release_or_recover(&warm.grant.lease_id).unwrap();
        drop(warm);

        *os.reads.lock().unwrap() = [RESERVE - 1, RESERVE].into();
        let (fresh, walks) = count_pool_walks(|| {
            opened.lease(&request(
                &pool,
                &fixture.dir("new-below"),
                &workspace,
                "new-below",
            ))
        });
        assert!(!fresh.unwrap().grant.reused_slot);
        assert_eq!(walks, vec![PoolWalk::Evict(roots[6].clone())]);
        assert!(os.reads.lock().unwrap().is_empty());
        for index in [1, 2, 3, 7] {
            assert!(storage.resolve(&roots[index]).is_ok());
        }
        drop(hold);
    }
}
