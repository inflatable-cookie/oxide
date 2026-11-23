//! Front-door entry points: budget-only and removable-volume pools for the `oxide` command.

use super::super::backing::BackingPaths;
use super::super::{
    canonical_existing_dir, ensure_private_dir, io_error, LeaseError, Leased, LedgerView,
    MeasuredPool, PassiveSnapshot, PoolInfo, PoolStatus, ReleaseReceipt, VolumeDescription,
    ACCOUNTING_MAX_ELAPSED_MS, BUDGET_BACKEND_DIRECTORY_V1, CONFIG_NAME, LEASE_VERSION,
    LEDGER_NAME, LEDGER_VERSION, MANAGED_NAME, MAX_SLOTS, PAYLOAD_NAME, SLOTS_NAME,
    VOLUME_BACKEND_APFS_V1,
};
use super::LeaseRequest;
use super::{describe_pins, Phase};
use super::{
    lock_pool, measure_within as measure_walks, open_budget_payload, prepare_new_pool, random_hex,
    read_config, read_consistent_pair, read_ledger, recorded_volume, refuse_foreign_pool_entries,
    refuse_retired_image_pool, snapshot_of, verify_scratch_initializer, write_config, write_ledger,
    Ledger, Payload, Pool, PoolConfig, SlotPersistState, VolumeConfig,
};
use crate::lease::HeldEvictionFailure;
use crate::volume::Binding;
use std::os::fd::AsFd;
use std::path::Path;
use std::time::Duration;

/// The removable pool's configuration held against concurrent lease writers
/// during an area move. The area and its bytes are moved by Barge.
#[derive(Debug)]
pub struct PayloadRelocation {
    pool: std::path::PathBuf,
    config: PoolConfig,
    ledger: Ledger,
    _lock: std::fs::File,
}

/// The path and ownership evidence for a slot in the locked removable ledger.
#[derive(Debug)]
pub struct QuietSlot<'a> {
    pub id: u64,
    pub folder: std::borrow::Cow<'a, str>,
    pub project: &'a Path,
    pub workspace: &'a Path,
    pub root: Option<&'a str>,
}

impl PayloadRelocation {
    /// Lock the ledger and verify that the registered layout is still current
    /// and no lease or pending eviction can be displaced.
    pub fn begin(pool: &Path, recorded: &super::super::RecordedVolume) -> Result<Self, LeaseError> {
        let pool = canonical_existing_dir(pool, true)?;
        refuse_foreign_pool_entries(&pool)?;
        let lock = super::lock_pool_inner(&pool, false, false)?;
        let (config, ledger) = read_consistent_pair(&pool)?;
        if config.backend != VOLUME_BACKEND_APFS_V1
            || recorded_volume(&BackingPaths::under(&pool), &config)? != *recorded
        {
            return Err(LeaseError::BackingMismatch);
        }
        if super::held_eviction::HeldEvictionIntent::read(&pool)?.is_some()
            || !ledger.holding.is_empty()
            || ledger.slots.iter().any(|slot| !slot.is_idle())
        {
            return Err(LeaseError::InvalidInput(
                "the removable ledger has a held slot or unfinished eviction",
            ));
        }
        Ok(Self {
            pool,
            config,
            ledger,
            _lock: lock,
        })
    }

    /// Publish the moved payload's relative path. All other config fields
    /// and the ledger stay as they were.
    pub fn publish(&self, payload_dir: &str) -> Result<(), LeaseError> {
        crate::volume::relative_components(payload_dir).map_err(LeaseError::Io)?;
        let mut config = self.config.clone();
        config
            .volume
            .as_mut()
            .ok_or(LeaseError::BackingMismatch)?
            .payload_dir = payload_dir.to_owned();
        write_config(&self.pool, &config)
    }

    /// Inspect the slots without reopening or reconciling the locked ledger.
    pub fn slots(&self) -> impl Iterator<Item = QuietSlot<'_>> {
        self.ledger.slots.iter().map(|slot| QuietSlot {
            id: slot.id,
            folder: slot.dir.as_deref().map_or_else(
                || std::borrow::Cow::Owned(slot.id.to_string()),
                std::borrow::Cow::Borrowed,
            ),
            project: &slot.project,
            workspace: &slot.workspace,
            root: slot.root.as_deref(),
        })
    }

    /// Validate the ledger replacement before maintenance starts deleting.
    pub fn check_legacy_retirement(
        &self,
        ids: &std::collections::BTreeSet<u64>,
    ) -> Result<(), LeaseError> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut ledger = self.ledger.clone();
        ledger.slots.retain(|slot| !ids.contains(&slot.id));
        super::ledger::validate_ledger_write(&ledger)
    }

    /// Forget a nested legacy slot after its caller has removed the payload.
    pub fn forget_legacy_slot(&mut self, id: u64) -> Result<(), LeaseError> {
        let mut ledger = self.ledger.clone();
        let index = ledger
            .slots
            .iter()
            .position(|slot| slot.id == id && slot.area_folder().is_some())
            .ok_or(LeaseError::InvalidInput("no such nested legacy slot"))?;
        ledger.slots.remove(index);
        super::ledger::validate_ledger_write(&ledger)?;
        write_ledger(&self.pool, &ledger)?;
        self.ledger = ledger;
        Ok(())
    }
}

pub use super::super::seed::{
    held_seed_attempt, publish_seed_catalog, read_seed_last, seed, seed_candidates, seed_monitored,
    write_seed_attempt, write_seed_last, write_seed_sources, SeedAbandon, SeedAttempt,
    SeedAttemptState, SeedBucket, SeedCandidate, SeedCandidates, SeedCatalog, SeedDonor, SeedFile,
    SeedFileKind, SeedJoin, SeedLast, SeedObservation, SeedOutcome, SeedPackage, SeedProducer,
    SeedReceipt, SeedRecipient, SeedRejectReason, SeedRejection, SeedRequest, SeedRole, SeedSkip,
    SeedSkipReason, SeedSourceKind, SeedSourceRoot, SeedUnit, SeedUnitRef, SeedWalkGap,
    SeedWalkLimits, ATTEMPT_SCHEMA,
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
        .dir_path_at(payload_dir, true)
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

/// Evict every idle slot whose eviction this pool decided under its lock,
/// and report each trash deletion that did not finish. Barge owns the
/// eviction: its marker, hold and pin checks still decide. Call this with
/// no pool or front-door admission lock held, so a large tree's deletion
/// never blocks another admission.
pub fn settle_pending_evictions(pool: &Path) -> Result<Vec<String>, LeaseError> {
    super::pending::settle(pool)
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
    let opened = crate::release_trace::phase("pool open", || Pool::open_for_release(pool, None))?;
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
    Pool::open_observed(&request.pool, Phase::Accounting)?.lease(&request)
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

/// Pin reasons for known slot paths, without a lock or root inventory.
pub fn pinned_roots(pool: &Path) -> Result<String, LeaseError> {
    let opened = Pool::open_passive(
        pool,
        std::time::Instant::now() + Duration::from_millis(ACCOUNTING_MAX_ELAPSED_MS),
    )?;
    let mut reasons = std::collections::BTreeMap::new();
    let mut unreadable = 0;
    for slot in &opened.ledger.slots {
        let Some(id) = slot.root.as_deref().filter(|_| slot.owned) else {
            continue;
        };
        match crate::storage::slot_pin_reason(&opened.slot_path(slot.id)?, slot.area_folder(), id) {
            Ok(Some(reason)) => *reasons.entry(reason).or_insert(0) += 1,
            Ok(None) => {}
            Err(()) => unreadable += 1,
        }
    }
    let mut line = format!(
        "pinned roots by reason: {}",
        describe_pins(&reasons, usize::MAX)
    );
    if unreadable != 0 {
        line.push_str(&format!(" ({unreadable} unreadable)"));
    }
    Ok(line)
}

/// Slot IDs of Barge-owned ledger slots whose Barge hold is currently held by
/// another process, read without taking the pool lock, reconciling or walking
/// slot trees.
pub fn barge_held_slots(pool: &Path) -> Result<std::collections::BTreeSet<String>, LeaseError> {
    use std::collections::BTreeSet;
    let opened = Pool::open_passive(
        pool,
        std::time::Instant::now() + Duration::from_millis(ACCOUNTING_MAX_ELAPSED_MS),
    )?;
    let folders: BTreeSet<Option<&str>> = opened
        .ledger
        .slots
        .iter()
        .filter(|slot| slot.owned && slot.root.is_some())
        .map(|slot| slot.area_folder())
        .collect();
    if folders.is_empty() {
        return Ok(BTreeSet::new());
    }
    let storage = crate::storage::Storage::open_read_only(&opened.root.join(MANAGED_NAME))
        .map_err(LeaseError::Barge)?;
    let backing = super::pool_backing(&opened.root, &opened.config)?;
    let mut held_roots = BTreeSet::new();
    for folder in folders {
        let area = backing
            .existing_pool_area(&storage, folder)
            .map_err(LeaseError::Barge)?;
        let listing = area
            .list_without_bytes()
            .map_err(|error| LeaseError::Barge(error.to_string()))?;
        for root in listing.roots {
            if root.held {
                held_roots.insert(root.id);
            }
        }
    }
    Ok(opened
        .ledger
        .slots
        .iter()
        .filter(|slot| {
            slot.owned
                && slot
                    .root
                    .as_deref()
                    .is_some_and(|root_id| held_roots.contains(root_id))
        })
        .map(|slot| slot.id.to_string())
        .collect())
}

/// Prepare a pool at the front door before leasing. Only pending legacy
/// rows enter the one-time adoption transition.
pub fn prepare_pool(pool: &Path) -> Result<LedgerView, LeaseError> {
    let mut pool = Pool::open_observed(pool, Phase::Adoption)?;
    pool.adopt_legacy_roots()?;
    pool.ledger_view()
}

/// Ledger membership without accounting walks; it takes the pool lock
/// and reconciles recorded evictions before returning.
pub fn ledger_view(pool: &Path) -> Result<LedgerView, LeaseError> {
    Pool::open_observed(pool, Phase::Accounting)?.ledger_view()
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
