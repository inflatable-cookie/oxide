//! Exclusive pool: short-lived flock, durable JSON ledger, idle-only eviction.

use super::backing::BackingPaths;
use super::{
    canonical_existing_dir, current_uid, ensure_private_dir, io_error, is_finder_metadata,
    is_tolerated_metadata_file, persist_private_file, random_hex, read_private_file,
    remove_tree_owned, unexpected_entry, AccountingCoverage, AccountingGap, AccountingGapKind,
    AccountingScan, LeaseEnvironment, LeaseError, LeaseGrant, Leased, LedgerSlot, LedgerView,
    MeasuredPool, PassiveSnapshot, PoolStatus, Profile, RecordedVolume, ReleaseReceipt,
    ReuseMeasurement, SlotOccupancy, SlotStatus, ACCOUNTING_MAX_DEPTH, ACCOUNTING_MAX_ELAPSED_MS,
    ACCOUNTING_MAX_ENTRIES, CAPACITY_POLICY_VERSION, CONFIG_NAME, LEASE_VERSION, LEDGER_NAME,
    LOCK_NAME, MAX_HOLDER_BYTES, MAX_LEASE_ID_BYTES, MAX_PATH_BYTES, MAX_SLOTS, SLOTS_NAME,
};
use super::{BUDGET_BACKEND_DIRECTORY_V1, VOLUME_BACKEND_APFS_V1};
use crate::inventory::{inventory_directory, GapKind, InventoryError, InventoryLimits};
use crate::volume::Binding;
use serde::{Deserialize, Serialize};
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
    /// Where a new slot goes. `None` keeps the legacy `slots/<id>` path.
    pub layout: Option<SlotLayout>,
    /// A front-door count replacement, never byte pressure: retire this idle
    /// whole slot as part of this admission, after complete whole-payload
    /// accounting and before the new slot is allocated. The front door
    /// chooses the victim by recorded rank; the library rechecks its
    /// identity, ownership, occupancy and accounting under the pool lock.
    pub replace_slot: Option<u64>,
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
    /// Idle rank from this pool's monotonic sequence (oxide#118). Legacy
    /// records keep 0 and sort oldest.
    #[serde(default, skip_serializing_if = "is_zero_rank")]
    rank: u64,
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
        let opened = Pool::open(pool)?;
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
        Pool::open(pool)?.release_or_recover(lease_id, false)
    }

    pub fn recover(pool: &Path, lease_id: &str) -> Result<ReleaseReceipt, LeaseError> {
        Pool::open(pool)?.release_or_recover(lease_id, true)
    }

    pub fn status(pool: &Path) -> Result<PoolStatus, LeaseError> {
        Pool::open(pool)?.accounted()
    }

    /// Ledger membership without any accounting walk; it refuses exactly
    /// where [`status`] does.
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
        let evicting_slots = ledger
            .slots
            .iter()
            .filter(|slot| slot.state == SlotPersistState::Evicting)
            .count();
        Ok(VolumeDescription {
            store_id: config.store_id,
            cap_bytes: config.cap_bytes,
            mount_point: volume.mount_point,
            volume_uuid: volume.volume_uuid,
            payload_dir: volume.payload_dir,
            evicting_slots,
        })
    }

    /// Retire one recorded idle slot by ID after rechecking its durable
    /// scope, for the per-project slot-cap eviction.
    pub fn evict_idle_slot(pool: &Path, slot_id: &str) -> Result<(), LeaseError> {
        Pool::open(pool)?.evict_idle(slot_id)
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

#[cfg(test)]
pub(super) fn release_injected(pool: &Path, lease_id: &str) -> Result<ReleaseReceipt, LeaseError> {
    managed::release(pool, lease_id)
}

#[cfg(test)]
pub(super) fn recover_injected(pool: &Path, lease_id: &str) -> Result<ReleaseReceipt, LeaseError> {
    managed::recover(pool, lease_id)
}

#[cfg(test)]
pub(super) fn evict_idle_slot_injected(pool: &Path, slot_id: &str) -> Result<(), LeaseError> {
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
    opened.persist()
}

impl Pool {
    /// Open a budget-only directory or removable-volume pool. Any other
    /// recorded backend, including the retired `apfs-sparse-image-v1`,
    /// refuses before anything under the pool is touched.
    fn open(root: &Path) -> Result<Self, LeaseError> {
        let root = canonical_existing_dir(root, true)?;
        refuse_foreign_pool_entries(&root)?;
        refuse_retired_image_pool(&root)?;
        let lock = lock_pool(&root)?;
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
        };
        pool.reconcile_evicting()?;
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
    /// this ledger predates it, validate every stored field against the
    /// raised caps, then replace the ledger atomically. Legacy representation
    /// is never migrated by a read-only observer or by reconciliation alone.
    fn persist_revised(&mut self) -> Result<(), LeaseError> {
        self.ensure_capacity_policy()?;
        validate_ledger_write(&self.ledger)?;
        self.persist()
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
        if self.ledger.capacity_policy != Some(CAPACITY_POLICY_VERSION) {
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
        if self.reconciliation_blocked {
            return Err(LeaseError::UnknownAccounting);
        }
        if let Some(budget) = request.budget_bytes {
            if budget == 0 {
                return Err(LeaseError::InvalidInput("budget must be positive"));
            }
            self.budget = budget;
        }
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
        if let Some((idle, incarnation)) = self
            .ledger
            .slots
            .iter()
            .find(|slot| {
                slot.state == SlotPersistState::Idle
                    && slot.project == project
                    && slot.profile == request.profile
            })
            .map(|slot| (slot.id, slot.incarnation))
        {
            let slot_path =
                self.validate_idle_slot_scope(idle, incarnation, &project, request.profile)?;
            // Compatible reuse needs this slot's own walk complete.
            let _unique = inventory_sizes(&slot_path).require_complete()?;
            let dir = self.slot(idle).and_then(|slot| slot.dir.clone());
            // The idle-rank sequence must have headroom before any grant
            // transition; exhaustion refuses without changing the pool.
            self.ensure_capacity_policy()?;
            self.next_rank()?;
            let grant = self.take_idle(idle, incarnation, project, workspace, request)?;
            return Ok(Leased {
                grant,
                measured: Some(ReuseMeasurement {
                    slot_id: idle.to_string(),
                    incarnation: incarnation.to_string(),
                    unique_allocated_bytes: _unique,
                    dir,
                }),
            });
        }
        self.ensure_capacity_policy()?;
        self.next_rank()?;
        self.allocate_unmeasured(project, workspace, request)
    }

    /// A new slot: nothing measured it.
    fn allocate_unmeasured(
        &mut self,
        project: PathBuf,
        workspace: PathBuf,
        request: &LeaseRequest,
    ) -> Result<Leased, LeaseError> {
        Ok(Leased {
            grant: self.allocate(project, workspace, request)?,
            measured: None,
        })
    }

    fn evict_idle(&mut self, slot_id: &str) -> Result<(), LeaseError> {
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
        self.retire_idle_slot(id, incarnation, &project, profile)
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
        // Every allocated entry counts, including held, idle, retired-backed
        // and unresolved Evicting records.
        if self.ledger.slots.len() >= MAX_SLOTS {
            self.evict_idle_until(MAX_SLOTS - 1)?;
            if self.ledger.slots.len() >= MAX_SLOTS {
                return Err(LeaseError::LedgerExhausted);
            }
        }
        // New-slot admission needs the whole-payload walk complete, before
        // and after any eviction. Below both count ceilings and without byte
        // pressure nothing is evicted.
        if self.payload_accounting().require_complete()? >= self.budget {
            self.evict_idle_until_under_cap()?;
            if self.payload_accounting().require_complete()? >= self.budget {
                return Err(LeaseError::Full);
            }
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
        self.ledger.slots[index].state = SlotPersistState::Idle;
        self.ledger.slots[index].lease_id = None;
        let released = self.ledger.slots[index].id;
        self.stamp_rank(released)?;
        self.persist_revised()?;
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
    fn replace_idle_slot_for_admission(&mut self, id: u64) -> Result<(), LeaseError> {
        if id == 0 {
            return Err(LeaseError::InvalidInput(
                "replacement slot ID must be a positive integer",
            ));
        }
        // The whole-payload walk must be complete before any deletion.
        self.payload_accounting().require_complete()?;
        let Some((incarnation, project, profile)) = self
            .slot(id)
            .filter(|slot| slot.state == SlotPersistState::Idle && slot.lease_id.is_none())
            .map(|slot| (slot.incarnation, slot.project.clone(), slot.profile))
        else {
            return Err(LeaseError::Busy);
        };
        self.retire_idle_slot(id, incarnation, &project, profile)
    }

    fn evict_idle_until(&mut self, max_slots: usize) -> Result<(), LeaseError> {
        // Bound admission to the allocated entries it saw when it began, so
        // concurrent-looking state cannot extend the loop.
        let mut remaining = self.ledger.slots.len();
        while self.ledger.slots.len() > max_slots && remaining > 0 {
            // A count replacement still needs complete accounting first; an
            // incomplete walk refuses instead of deleting blindly.
            self.payload_accounting().require_complete()?;
            let before = self.ledger.slots.len();
            if !self.evict_one_idle()? {
                break;
            }
            if self.ledger.slots.len() >= before {
                return Err(LeaseError::UnknownAccounting);
            }
            remaining -= 1;
        }
        Ok(())
    }

    fn evict_idle_until_under_cap(&mut self) -> Result<(), LeaseError> {
        let mut remaining = self.ledger.slots.len();
        loop {
            // Byte pressure remeasures the whole payload union after each
            // deletion and stops when it is under budget or no safe idle
            // whole slot remains.
            if self.payload_accounting().require_complete()? < self.budget {
                return Ok(());
            }
            if remaining == 0 {
                return Ok(());
            }
            let before = self.ledger.slots.len();
            if !self.evict_one_idle()? {
                return Ok(());
            }
            if self.ledger.slots.len() >= before {
                return Err(LeaseError::UnknownAccounting);
            }
            remaining -= 1;
        }
    }

    /// The least-recently-used eligible idle whole slot: ascending
    /// `(rank, numeric slot id)`. Held, retired-backed and unresolved
    /// `Evicting` records are never candidates. A safety failure on the
    /// selected victim stops admission; it never skips to another slot.
    fn evict_one_idle(&mut self) -> Result<bool, LeaseError> {
        let Some((id, incarnation, project, profile)) = self
            .ledger
            .slots
            .iter()
            .filter(|slot| slot.state == SlotPersistState::Idle && slot.lease_id.is_none())
            .min_by_key(|slot| (slot.rank, slot.id))
            .map(|slot| {
                (
                    slot.id,
                    slot.incarnation,
                    slot.project.clone(),
                    slot.profile,
                )
            })
        else {
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
        // Every durable idle deletion, including the explicit
        // `evict_idle_slot` path, checks the rank sequence before its first
        // transition: an exhausted or missing-headroom sequence refuses
        // without deleting anything.
        self.ensure_capacity_policy()?;
        self.next_rank()?;
        let path = self.validate_idle_slot_scope(id, incarnation, project, profile)?;
        {
            let slot = self.slot_mut(id).ok_or(LeaseError::UnknownAccounting)?;
            slot.state = SlotPersistState::Evicting;
        }
        self.persist_revised()?;
        self.remove_owned_slot(&path)?;
        self.ledger.slots.retain(|slot| slot.id != id);
        self.persist_revised()?;
        Ok(())
    }

    fn validate_idle_slot_scope(
        &self,
        id: u64,
        incarnation: u64,
        project: &Path,
        profile: Profile,
    ) -> Result<PathBuf, LeaseError> {
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
        let slots_device = fs::symlink_metadata(&slots_root).map_err(io_error)?.dev();
        let slot_device = fs::symlink_metadata(&path).map_err(io_error)?.dev();
        if slots_device != slot_device {
            return Err(LeaseError::UnknownAccounting);
        }
        Ok(path)
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
            if self.remove_owned_slot(&path).is_err() {
                self.reconciliation_blocked = true;
                continue;
            }
            self.ledger.slots.retain(|slot| slot.id != id);
            self.persist()?;
        }
        Ok(())
    }

    fn remove_owned_slot(&self, path: &Path) -> Result<(), LeaseError> {
        // A volume that is not this pool's verified root has no slots to
        // remove: the path would resolve elsewhere.
        self.backing.verify_volume()?;
        let slots_root = self.backing.payload_root().join(SLOTS_NAME);
        let slots_root = canonical_existing_dir(&slots_root, true)?;
        require_slot_parent(path, &slots_root)?;
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_error(error)),
            Ok(metadata)
                if metadata.file_type().is_symlink()
                    || !metadata.file_type().is_dir()
                    || metadata.uid() != current_uid()
                    || metadata.permissions().mode() & 0o077 != 0 =>
            {
                return Err(LeaseError::UnknownAccounting);
            }
            Ok(_) => {}
        }
        canonical_existing_dir(path, true)?;
        remove_tree_owned(&slots_root, path)
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
    PassiveSnapshot {
        store_id: config.store_id.clone(),
        backend: config.backend.clone(),
        cap_bytes: config.cap_bytes,
        payload,
        volume,
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
        if !is_slots && !metadata_dirs.contains(&text) {
            return Err(refuse());
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
        if metadata.file_type().is_symlink()
            || !metadata.file_type().is_dir()
            || metadata.dev() != root_device
        {
            return Err(refuse());
        }
        if is_slots {
            if metadata.uid() != current_uid() || metadata.permissions().mode() & 0o077 != 0 {
                return Err(LeaseError::UnsafePath(
                    "slots root must be a private caller-owned directory",
                ));
            }
            found_slots = true;
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
    }
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
    lock_pool_inner(pool, true)
}

fn lock_pool_inner(pool: &Path, create: bool) -> Result<File, LeaseError> {
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
    // SAFETY: `file` is open for the pool lock; flock holds until File drop.
    let status = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if status != 0 {
        return Err(LeaseError::Io("exclusive pool lock failed".into()));
    }
    Ok(file)
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
/// version wrote.
fn validate_ledger_read(ledger: &Ledger) -> Result<(), LeaseError> {
    match (ledger.capacity_policy, ledger.next_rank) {
        (None, None) => {
            if ledger.slots.iter().any(|slot| slot.rank != 0) {
                return Err(LeaseError::UnknownAccounting);
            }
        }
        (Some(policy), Some(next)) => {
            if policy != CAPACITY_POLICY_VERSION || next == 0 {
                return Err(LeaseError::UnknownAccounting);
            }
            if ledger.slots.iter().any(|slot| slot.rank >= next) {
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

fn refuse_foreign_pool_entries(pool: &Path) -> Result<(), LeaseError> {
    let device = fs::symlink_metadata(pool).map_err(io_error)?.dev();
    for entry in fs::read_dir(pool).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        if is_finder_metadata(&name) {
            // Control-directory bytes, like config and ledger: not payload.
            let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
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
        let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
        if metadata.file_type().is_symlink() {
            return Err(LeaseError::UnsafePath("symlinks are refused"));
        }
    }
    Ok(())
}
