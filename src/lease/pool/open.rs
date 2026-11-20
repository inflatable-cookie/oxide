//! Opening a pool: mutating and passive opens, payload binding and consistent snapshots.

use super::*;

/// Read the config and ledger a passive observer reports, validating their
/// pair. Each file is replaced atomically, so one read may straddle a
/// concurrent replacement: a mismatched pair is re-read once and a second
/// mismatch refuses as unknown instead of being repaired or waited for.
pub(super) fn read_consistent_pair(root: &Path) -> Result<(PoolConfig, Ledger), LeaseError> {
    read_consistent_pair_before(root, None)
}

/// Passive generation check for a seed status sidecar. No pool lock, recovery
/// or filesystem walk is performed.
pub(in crate::lease) fn seed_record_matches(
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
pub(super) fn recorded_volume(
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
pub(super) fn snapshot_of(
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
pub(super) fn measure_within(pool: &Path, max_elapsed_ms: u64) -> Result<MeasuredPool, LeaseError> {
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

/// Open one pool's Barge storage authority from its own recorded config.
///
/// The account is the pool's control directory, so the literal `oxide/pool`
/// declaration is unambiguous: every pool — removable, internal or a test
/// fixture — owns its own volume and area. A budget pool's volume is the
/// pool directory itself; a removable pool's is the drive at its registered
/// mount point with the recorded identity, so a different volume there is a
/// refusal, never a silent reuse.
pub(super) fn pool_storage(
    root: &Path,
    config: &PoolConfig,
) -> Result<Option<crate::storage::PoolStorage>, LeaseError> {
    crate::storage::PoolStorage::open(&root.join(MANAGED_NAME), &pool_backing(root, config)?)
        .map(Some)
        .map_err(LeaseError::Barge)
}

pub(super) fn pool_backing(
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

/// The budget-only payload is a plain owner-only directory holding `slots`.
/// The budget is not OS-enforced. Image and receipt files refuse, so a legacy
/// APFS image pool is never reinterpreted as a directory.
pub(super) fn open_budget_payload(
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
pub(super) fn open_volume_payload(
    paths: &BackingPaths,
    config: &PoolConfig,
) -> Result<Payload, LeaseError> {
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
    let payload_fd = binding
        .dir_at(binding.fd(), &volume.payload_dir, false)
        .map_err(|_| LeaseError::BackingMismatch)?;
    let root = volume.mount_point.join(&volume.payload_dir);
    let payload = canonical_existing_dir(&root, true).map_err(|_| LeaseError::BackingMismatch)?;
    let by_path = fs::symlink_metadata(&payload).map_err(io_error)?;
    let by_fd = fs::File::from(payload_fd).metadata().map_err(io_error)?;
    if payload != root || by_path.dev() != by_fd.dev() || by_path.ino() != by_fd.ino() {
        return Err(LeaseError::BackingMismatch);
    }
    validate_lease_payload_entries(&payload, &[])?;
    Ok(Payload::Volume {
        binding,
        root: payload,
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

/// The hard floor for an unknown-size admission with nothing left to prune.
///
/// Tom's 2026-10-25 refinement: the registered reserve is a *prune* trigger,
/// not a stop; only this floor is a stop. It is a fixed product constant for
/// oxide#146 — not a registration field, a watchdog setting or a selection
/// change.
const UNKNOWN_REMOVABLE_FLOOR_BYTES: u64 = 10 * 1024 * 1024 * 1024;

pub(super) fn measurement_remaining(deadline: std::time::Instant) -> Result<(), LeaseError> {
    if std::time::Instant::now() >= deadline {
        Err(LeaseError::UnknownAccounting)
    } else {
        Ok(())
    }
}

impl Pool {
    /// Open a budget-only directory or removable-volume pool. Any other
    /// recorded backend, including the retired `apfs-sparse-image-v1`,
    /// refuses before anything under the pool is touched.
    pub(in crate::lease) fn open(root: &Path) -> Result<Self, LeaseError> {
        Self::open_inner(root, false, None, true, None)
    }

    pub(super) fn open_observed(root: &Path, phase: Phase) -> Result<Self, LeaseError> {
        Self::open_inner(root, false, None, true, Some(phase))
    }

    pub(super) fn open_for_release(
        root: &Path,
        lease_id: Option<&str>,
    ) -> Result<Self, LeaseError> {
        Self::open_inner(root, true, lease_id, true, None)
    }

    pub(super) fn open_held_eviction(root: &Path) -> Result<Self, LeaseError> {
        Self::open_inner(root, false, None, false, None)
    }

    pub(super) fn open_inner(
        root: &Path,
        trace_release: bool,
        releasing: Option<&str>,
        reconcile: bool,
        observed_phase: Option<Phase>,
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
        let lock = PoolLock::new(lock, &root, observed_phase);
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
            eviction_notes: Vec::new(),
            reconciliation_blocked: false,
        };
        if reconcile {
            if let Some(intent) = pending {
                // A valid intent this open cannot prove — a substituted parent,
                // another volume, an unreadable root — is quarantined rather
                // than allowed to refuse every lease, release and prepare. Its
                // row keeps exactly the generation it recorded and stays
                // unavailable; a later pass asks again.
                if admission_cost::in_phase(Phase::Eviction, || {
                    pool.recover_held_eviction(&intent, false)
                })
                .is_err()
                {
                    eprintln!(
                        "oxide: warning: a recorded held eviction could not be resolved; its row is kept unavailable"
                    );
                    pool.reconciliation_blocked = true;
                }
            }
            pool.storage = pool_storage(&pool.root, &pool.config)?;
            admission_cost::in_phase(Phase::Accounting, || {
                pool.recover_abandoned_holds(releasing)
            })?;
            admission_cost::in_phase(Phase::Eviction, || {
                pool.retire_missing_idle_roots()?;
                pool.reconcile_pending_evictions()
            })?;
        }
        Ok(pool)
    }

    /// Open the same verified backing without the pool lock and without
    /// reconciliation, for [`measure`] only. The recorded payload must still
    /// be the backend's real, verified payload root, so the walks cover what
    /// admission would cover; nothing is created or written.
    pub(super) fn open_passive(
        root: &Path,
        deadline: std::time::Instant,
    ) -> Result<Self, LeaseError> {
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
            eviction_notes: Vec::new(),
            reconciliation_blocked: false,
        })
    }
}
