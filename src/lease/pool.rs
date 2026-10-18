//! Exclusive pool: short-lived flock, durable JSON ledger, idle-only eviction.

use super::backing::{self, BackingPaths, CapacitySample};
use super::{
    canonical_existing_dir, current_uid, ensure_private_dir, io_error, persist_private_file,
    random_hex, read_private_file, remove_tree_owned, AccountingCoverage, CapacityObservations,
    CapacityStatus, LeaseEnvironment, LeaseError, LeaseGrant, PoolInfo, PoolStatus, Profile,
    ReleaseReceipt, SlotOccupancy, SlotStatus, ACCOUNTING_MAX_DEPTH, ACCOUNTING_MAX_ELAPSED_MS,
    ACCOUNTING_MAX_ENTRIES, CAPACITY_STATUS_NOTES, CAPACITY_STATUS_VERSION, CONFIG_NAME,
    LEASE_VERSION, LEDGER_NAME, LOCK_NAME, MAX_HOLDER_BYTES, MAX_LEASE_ID_BYTES, MAX_PATH_BYTES,
    MAX_SLOTS, SLOTS_NAME,
};
#[cfg(test)]
use super::{ByteObservation, ObservationUnknownReason};
use crate::inventory::{inventory_directory, InventoryLimits};
use crate::{BoundedBacking, BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

pub(super) struct InitRequest {
    pub pool: PathBuf,
    pub cap_bytes: u64,
    pub image_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct LeaseRequest {
    pub pool: PathBuf,
    pub project: PathBuf,
    pub workspace: PathBuf,
    pub profile: Profile,
    pub holder: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PoolConfig {
    version: u32,
    backend: String,
    store_id: String,
    cap_bytes: u64,
    image_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    version: u32,
    store_id: String,
    cap_bytes: u64,
    next_slot: u64,
    next_lease: u64,
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SlotPersistState {
    Held,
    Idle,
    Evicting,
}

pub(super) struct Pool {
    _lock: File,
    root: PathBuf,
    config: PoolConfig,
    backing: BoundedBacking,
    ledger: Ledger,
    reconciliation_blocked: bool,
}

pub fn init(request: InitRequest) -> Result<PoolInfo, LeaseError> {
    if request.cap_bytes == 0 {
        return Err(LeaseError::InvalidInput("cap-bytes must be positive"));
    }
    // Checked before any host effect: the declared cap must equal the image
    // size so the volume size is the enforced ceiling. A smaller cap on a
    // larger volume is not OS-enforced and is refused.
    if request.image_bytes != request.cap_bytes {
        return Err(LeaseError::InvalidInput(
            "cap-bytes must equal image-bytes so the volume size is the enforced ceiling",
        ));
    }
    refuse_unsupported_init_platform()?;
    let pool = prepare_new_pool(&request.pool)?;
    let _lock = lock_pool(&pool)?;
    if pool.join(CONFIG_NAME).symlink_metadata().is_ok() {
        return Err(LeaseError::InvalidInput("pool already initialized"));
    }
    let paths = BackingPaths::under(&pool);
    let backing = backing::provision(&paths, request.cap_bytes, request.image_bytes)?;
    ensure_private_dir(&backing.payload_root().join(SLOTS_NAME))?;
    validate_lease_payload_entries(backing.payload_root())?;
    let store_id = random_hex(16)?;
    let config = PoolConfig {
        version: LEASE_VERSION,
        backend: BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1.to_owned(),
        store_id: store_id.clone(),
        cap_bytes: request.cap_bytes,
        image_bytes: request.image_bytes,
    };
    let ledger = Ledger {
        version: LEASE_VERSION,
        store_id: store_id.clone(),
        cap_bytes: request.cap_bytes,
        next_slot: 1,
        next_lease: 1,
        slots: Vec::new(),
    };
    write_config(&pool, &config)?;
    write_ledger(&pool, &ledger)?;
    Ok(PoolInfo {
        version: LEASE_VERSION,
        store_id,
        backend: config.backend,
        cap_bytes: request.cap_bytes,
        verified_capacity_bytes: backing.descriptor().verified_capacity_bytes,
    })
}

pub fn lease(request: LeaseRequest) -> Result<LeaseGrant, LeaseError> {
    let mut pool = Pool::open(&request.pool)?;
    pool.lease(&request)
}

pub fn release(pool: &Path, lease_id: &str) -> Result<ReleaseReceipt, LeaseError> {
    let mut opened = Pool::open(pool)?;
    opened.release_or_recover(lease_id, false)
}

pub fn recover(pool: &Path, lease_id: &str) -> Result<ReleaseReceipt, LeaseError> {
    let mut opened = Pool::open(pool)?;
    opened.release_or_recover(lease_id, true)
}

pub fn status(pool: &Path) -> Result<PoolStatus, LeaseError> {
    let opened = Pool::open(pool)?;
    opened.status()
}

pub fn capacity_status(pool: &Path) -> Result<CapacityStatus, LeaseError> {
    let (opened, sample) = Pool::open_observational(pool, false)?;
    opened.capacity_status(sample)
}

/// Retire one recorded idle slot by ID after rechecking its durable scope.
/// The `oxide` front door uses this for its per-project slot-cap eviction;
/// the standalone lease CLI does not expose it.
pub fn evict_idle_slot(pool: &Path, slot_id: &str) -> Result<(), LeaseError> {
    Pool::open(pool)?.evict_idle(slot_id)
}

#[cfg(test)]
pub(super) fn create_injected(pool: &Path, cap_bytes: u64) -> Result<PoolInfo, LeaseError> {
    let pool = prepare_new_pool(pool)?;
    let _lock = lock_pool(&pool)?;
    let mount = pool.join(super::PAYLOAD_NAME);
    let backing = backing::inject_directory(&mount, cap_bytes)?;
    ensure_private_dir(&backing.payload_root().join(SLOTS_NAME))?;
    let store_id = random_hex(16)?;
    let config = PoolConfig {
        version: LEASE_VERSION,
        backend: BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1.to_owned(),
        store_id: store_id.clone(),
        cap_bytes,
        image_bytes: cap_bytes,
    };
    let ledger = Ledger {
        version: LEASE_VERSION,
        store_id: store_id.clone(),
        cap_bytes,
        next_slot: 1,
        next_lease: 1,
        slots: Vec::new(),
    };
    write_config(&pool, &config)?;
    write_ledger(&pool, &ledger)?;
    Ok(PoolInfo {
        version: LEASE_VERSION,
        store_id,
        backend: config.backend,
        cap_bytes,
        verified_capacity_bytes: backing.descriptor().verified_capacity_bytes,
    })
}

#[cfg(test)]
pub(super) fn lease_injected(request: LeaseRequest) -> Result<LeaseGrant, LeaseError> {
    let mut opened = Pool::open_with(&request.pool, true)?;
    opened.lease(&request)
}

#[cfg(test)]
pub(super) fn release_injected(pool: &Path, lease_id: &str) -> Result<ReleaseReceipt, LeaseError> {
    let mut opened = Pool::open_with(pool, true)?;
    opened.release_or_recover(lease_id, false)
}

#[cfg(test)]
pub(super) fn recover_injected(pool: &Path, lease_id: &str) -> Result<ReleaseReceipt, LeaseError> {
    let mut opened = Pool::open_with(pool, true)?;
    opened.release_or_recover(lease_id, true)
}

#[cfg(test)]
pub(super) fn evict_idle_slot_injected(pool: &Path, slot_id: &str) -> Result<(), LeaseError> {
    Pool::open_with(pool, true)?.evict_idle(slot_id)
}

#[cfg(test)]
pub(super) fn status_injected(pool: &Path) -> Result<PoolStatus, LeaseError> {
    Pool::open_with(pool, true)?.status()
}

#[cfg(test)]
pub(super) fn capacity_status_injected(
    pool: &Path,
    sample: CapacitySample,
) -> Result<CapacityStatus, LeaseError> {
    let (opened, _) = Pool::open_observational(pool, true)?;
    opened.capacity_status(sample)
}

#[cfg(test)]
pub(super) fn mark_evicting_for_test(pool: &Path, slot_id: u64) -> Result<(), LeaseError> {
    let mut opened = Pool::open_with(pool, true)?;
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
    fn open(root: &Path) -> Result<Self, LeaseError> {
        Self::open_inner(root, false)
    }

    #[cfg(test)]
    fn open_with(root: &Path, injected: bool) -> Result<Self, LeaseError> {
        Self::open_inner(root, injected)
    }

    fn open_observational(
        root: &Path,
        injected: bool,
    ) -> Result<(Self, CapacitySample), LeaseError> {
        // Capacity status preserves Evicting records; only ordinary opens reconcile them.
        let root = canonical_existing_dir(root, true)?;
        refuse_foreign_pool_entries(&root)?;
        let lock = lock_pool_existing(&root)?;
        let config = read_config(&root)?;
        if config.version != LEASE_VERSION
            || config.backend != BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1
            || config.cap_bytes == 0
            || config.cap_bytes != config.image_bytes
        {
            return Err(LeaseError::BackingMismatch);
        }
        let paths = BackingPaths::under(&root);
        let (backing, sample) = if injected {
            #[cfg(test)]
            {
                let backing = backing::inject_directory(&paths.mount, config.cap_bytes)?;
                let unknown = ByteObservation::unknown(ObservationUnknownReason::Unsupported);
                (
                    backing,
                    CapacitySample {
                        payload_free_bytes: unknown,
                        payload_available_bytes: unknown,
                        image_logical_bytes: unknown,
                        image_allocated_bytes: unknown,
                        host_available_bytes: unknown,
                    },
                )
            }
            #[cfg(not(test))]
            {
                let _ = (&paths, &config);
                return Err(LeaseError::BackingMismatch);
            }
        } else {
            backing::open_observational(&paths, config.cap_bytes, config.image_bytes)?
        };
        if backing.descriptor().payload_root != paths.mount {
            return Err(LeaseError::BackingMismatch);
        }
        validate_lease_payload_entries(backing.payload_root())?;
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
        let reconciliation_blocked = ledger
            .slots
            .iter()
            .any(|slot| slot.state == SlotPersistState::Evicting);
        Ok((
            Self {
                _lock: lock,
                root,
                config,
                backing,
                ledger,
                reconciliation_blocked,
            },
            sample,
        ))
    }

    fn open_inner(root: &Path, injected: bool) -> Result<Self, LeaseError> {
        let root = canonical_existing_dir(root, true)?;
        refuse_foreign_pool_entries(&root)?;
        let lock = lock_pool(&root)?;
        let config = read_config(&root)?;
        if config.version != LEASE_VERSION
            || config.backend != BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1
            || config.cap_bytes == 0
            || config.cap_bytes != config.image_bytes
        {
            return Err(LeaseError::BackingMismatch);
        }
        let paths = BackingPaths::under(&root);
        let backing = open_backing(&paths, &config, injected)?;
        if backing.descriptor().payload_root != paths.mount {
            return Err(LeaseError::BackingMismatch);
        }
        validate_lease_payload_entries(backing.payload_root())?;
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
        let mut pool = Self {
            _lock: lock,
            root,
            config,
            backing,
            ledger,
            reconciliation_blocked: false,
        };
        pool.reconcile_evicting()?;
        Ok(pool)
    }

    fn persist(&self) -> Result<(), LeaseError> {
        write_ledger(&self.root, &self.ledger)
    }

    fn lease(&mut self, request: &LeaseRequest) -> Result<LeaseGrant, LeaseError> {
        if self.reconciliation_blocked {
            return Err(LeaseError::UnknownAccounting);
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
                return self.grant_from(existing, true);
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
            let sizes = inventory_sizes(&slot_path)?;
            if sizes.coverage != AccountingCoverage::Complete {
                return Err(LeaseError::UnknownAccounting);
            }
            let unique = sizes.unique.ok_or(LeaseError::UnknownAccounting)?;
            if should_recycle_slot(unique, self.config.cap_bytes) {
                self.retire_idle_slot(idle, incarnation, &project, request.profile)?;
                return self.allocate(project, workspace, request);
            }
            return self.take_idle(idle, incarnation, project, workspace, request);
        }
        self.allocate(project, workspace, request)
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
        self.persist()?;
        let slot = self.slot(slot_id).ok_or(LeaseError::UnknownAccounting)?;
        self.grant_from(slot, true)
    }

    fn allocate(
        &mut self,
        project: PathBuf,
        workspace: PathBuf,
        request: &LeaseRequest,
    ) -> Result<LeaseGrant, LeaseError> {
        if self.ledger.slots.len() >= MAX_SLOTS {
            self.evict_idle_until(self.ledger.slots.len().saturating_sub(1))?;
            if self.ledger.slots.len() >= MAX_SLOTS {
                return Err(LeaseError::LedgerExhausted);
            }
        }
        let accounted = self.payload_accounting()?;
        if accounted.coverage != AccountingCoverage::Complete {
            return Err(LeaseError::UnknownAccounting);
        }
        if accounted
            .unique
            .is_some_and(|bytes| bytes >= self.config.cap_bytes)
        {
            self.evict_idle_until_under_cap()?;
            let accounted = self.payload_accounting()?;
            if accounted.coverage != AccountingCoverage::Complete {
                return Err(LeaseError::UnknownAccounting);
            }
            if accounted
                .unique
                .is_some_and(|bytes| bytes >= self.config.cap_bytes)
            {
                return Err(LeaseError::Full);
            }
        }
        let slot_id = self.ledger.next_slot;
        if slot_id == 0 {
            return Err(LeaseError::UnknownAccounting);
        }
        let incarnation = 1_u64;
        let lease_id = self.mint_lease_id(slot_id, incarnation)?;
        let slot_path = self.slot_path(slot_id)?;
        ensure_private_dir(&slot_path)?;
        ensure_private_dir(&slot_path.join("build"))?;
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
        });
        self.ledger.next_slot = slot_id.checked_add(1).ok_or(LeaseError::LedgerExhausted)?;
        self.persist()?;
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
        self.ledger.slots[index].state = SlotPersistState::Idle;
        self.ledger.slots[index].lease_id = None;
        self.persist()?;
        Ok(ReleaseReceipt {
            version: LEASE_VERSION,
            lease_id: lease_id.to_owned(),
            state: SlotOccupancy::Idle,
            replay: false,
        })
    }

    fn status(&self) -> Result<PoolStatus, LeaseError> {
        let accounted = self.payload_accounting()?;
        let mut coverage = if self.reconciliation_blocked {
            AccountingCoverage::Unknown
        } else {
            accounted.coverage
        };
        let mut slots = Vec::new();
        for slot in &self.ledger.slots {
            let sizes = self.slot_accounting(slot.id)?;
            match sizes.coverage {
                AccountingCoverage::Complete => {}
                AccountingCoverage::Partial if coverage == AccountingCoverage::Complete => {
                    coverage = AccountingCoverage::Partial;
                }
                AccountingCoverage::Partial | AccountingCoverage::Unknown => {
                    coverage = AccountingCoverage::Unknown;
                }
            }
            slots.push(SlotStatus {
                slot_id: slot.id.to_string(),
                incarnation: slot.incarnation.to_string(),
                state: occupancy(slot.state),
                project: slot.project.clone(),
                workspace: slot.workspace.clone(),
                profile: slot.profile,
                holder: if slot.state == SlotPersistState::Held {
                    Some(slot.holder.clone())
                } else {
                    None
                },
                lease_id: slot.lease_id.clone(),
                logical_bytes: sizes.logical,
                unique_allocated_bytes: sizes.unique,
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
            full,
            slots,
        })
    }

    fn capacity_status(&self, sample: CapacitySample) -> Result<CapacityStatus, LeaseError> {
        let status = self.status()?;
        let capacity = CapacityObservations::new(
            self.config.cap_bytes,
            self.backing.descriptor().verified_capacity_bytes,
            sample.payload_free_bytes,
            sample.payload_available_bytes,
            sample.image_logical_bytes,
            sample.image_allocated_bytes,
            sample.host_available_bytes,
        );
        Ok(CapacityStatus {
            version: CAPACITY_STATUS_VERSION,
            status,
            capacity,
            notes: CAPACITY_STATUS_NOTES,
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

    fn slot_path(&self, id: u64) -> Result<PathBuf, LeaseError> {
        if id == 0 {
            return Err(LeaseError::UnknownAccounting);
        }
        Ok(self
            .backing
            .payload_root()
            .join(SLOTS_NAME)
            .join(id.to_string()))
    }

    fn evict_idle_until(&mut self, max_slots: usize) -> Result<(), LeaseError> {
        while self.ledger.slots.len() > max_slots {
            if !self.evict_one_idle()? {
                break;
            }
        }
        Ok(())
    }

    fn evict_idle_until_under_cap(&mut self) -> Result<(), LeaseError> {
        loop {
            let accounted = self.payload_accounting()?;
            if accounted.coverage != AccountingCoverage::Complete {
                return Err(LeaseError::UnknownAccounting);
            }
            if accounted.unique.unwrap_or(0) < self.config.cap_bytes {
                return Ok(());
            }
            if !self.evict_one_idle()? {
                return Ok(());
            }
        }
    }

    fn evict_one_idle(&mut self) -> Result<bool, LeaseError> {
        let Some((id, incarnation, project, profile)) = self
            .ledger
            .slots
            .iter()
            .find(|slot| slot.state == SlotPersistState::Idle && slot.lease_id.is_none())
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
        let path = self.validate_idle_slot_scope(id, incarnation, project, profile)?;
        {
            let slot = self.slot_mut(id).ok_or(LeaseError::UnknownAccounting)?;
            slot.state = SlotPersistState::Evicting;
        }
        self.persist()?;
        self.remove_owned_slot(&path)?;
        self.ledger.slots.retain(|slot| slot.id != id);
        self.persist()?;
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
        if path.parent() != Some(slots_root.as_path()) {
            return Err(LeaseError::UnsafePath(
                "deletion is limited to recorded owned slot paths",
            ));
        }
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
        let slots_root = self.backing.payload_root().join(SLOTS_NAME);
        let slots_root = canonical_existing_dir(&slots_root, true)?;
        if path.parent() != Some(slots_root.as_path()) {
            return Err(LeaseError::UnsafePath(
                "deletion is limited to recorded owned slot paths",
            ));
        }
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

    fn payload_accounting(&self) -> Result<Sizes, LeaseError> {
        let slots = self.backing.payload_root().join(SLOTS_NAME);
        inventory_sizes(&slots)
    }

    fn slot_accounting(&self, id: u64) -> Result<Sizes, LeaseError> {
        let path = self.slot_path(id)?;
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Sizes {
                    logical: Some(0),
                    unique: Some(0),
                    coverage: AccountingCoverage::Complete,
                });
            }
            Err(error) => return Err(io_error(error)),
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Ok(Sizes {
                    logical: None,
                    unique: None,
                    coverage: AccountingCoverage::Unknown,
                });
            }
            Ok(_) => {}
        }
        inventory_sizes(&path)
    }
}

const LEASE_METADATA_DIRS: [&str; 4] = [
    ".fseventsd",
    ".Spotlight-V100",
    ".Trashes",
    ".TemporaryItems",
];

fn validate_lease_payload_entries(payload_root: &Path) -> Result<(), LeaseError> {
    let root_metadata = fs::symlink_metadata(payload_root).map_err(io_error)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.file_type().is_dir() {
        return Err(LeaseError::UnknownAccounting);
    }
    let root_device = root_metadata.dev();
    let mut found_slots = false;
    for entry in fs::read_dir(payload_root).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        let name = name.to_str().ok_or(LeaseError::UnknownAccounting)?;
        let is_slots = name == SLOTS_NAME;
        if !is_slots && !LEASE_METADATA_DIRS.contains(&name) {
            return Err(LeaseError::UnknownAccounting);
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
        if metadata.file_type().is_symlink()
            || !metadata.file_type().is_dir()
            || metadata.dev() != root_device
        {
            return Err(LeaseError::UnknownAccounting);
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

struct Sizes {
    logical: Option<u64>,
    unique: Option<u64>,
    coverage: AccountingCoverage,
}

fn occupancy(state: SlotPersistState) -> SlotOccupancy {
    match state {
        SlotPersistState::Held => SlotOccupancy::Held,
        SlotPersistState::Idle => SlotOccupancy::Idle,
        SlotPersistState::Evicting => SlotOccupancy::Abandoned,
    }
}

fn inventory_sizes(path: &Path) -> Result<Sizes, LeaseError> {
    let limits = InventoryLimits {
        max_entries: ACCOUNTING_MAX_ENTRIES,
        max_depth: ACCOUNTING_MAX_DEPTH,
        max_output_bytes: 64,
        max_elapsed_ms: ACCOUNTING_MAX_ELAPSED_MS,
    };
    match inventory_directory(path, limits) {
        Ok(report) if report.coverage.complete => Ok(Sizes {
            logical: Some(report.total.logical_bytes_by_name),
            unique: Some(report.total.allocated_bytes),
            coverage: AccountingCoverage::Complete,
        }),
        Ok(report) => Ok(Sizes {
            logical: Some(report.total.logical_bytes_by_name),
            unique: Some(report.total.allocated_bytes),
            coverage: AccountingCoverage::Partial,
        }),
        Err(_) => Ok(Sizes {
            logical: None,
            unique: None,
            coverage: AccountingCoverage::Unknown,
        }),
    }
}

pub(super) fn recycle_threshold(cap_bytes: u64) -> u64 {
    cap_bytes / 2 + cap_bytes % 2
}

pub(super) fn should_recycle_slot(allocated_bytes: u64, cap_bytes: u64) -> bool {
    allocated_bytes >= recycle_threshold(cap_bytes)
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

fn refuse_unsupported_init_platform() -> Result<(), LeaseError> {
    #[cfg(not(target_os = "macos"))]
    {
        return Err(LeaseError::Unsupported(
            "bounded APFS payload backing is macOS-only",
        ));
    }
    #[cfg(target_os = "macos")]
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
    lock_pool_inner(pool, true)
}

fn lock_pool_existing(pool: &Path) -> Result<File, LeaseError> {
    lock_pool_inner(pool, false)
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

fn read_config(pool: &Path) -> Result<PoolConfig, LeaseError> {
    let bytes = read_private_file(&pool.join(CONFIG_NAME))?;
    serde_json::from_slice(&bytes).map_err(|_| LeaseError::BackingMismatch)
}

fn read_ledger(pool: &Path) -> Result<Ledger, LeaseError> {
    let bytes = read_private_file(&pool.join(LEDGER_NAME))?;
    serde_json::from_slice(&bytes).map_err(|_| LeaseError::UnknownAccounting)
}

fn open_backing(
    paths: &BackingPaths,
    config: &PoolConfig,
    injected: bool,
) -> Result<BoundedBacking, LeaseError> {
    #[cfg(test)]
    if injected {
        return backing::inject_directory(&paths.mount, config.cap_bytes);
    }
    let _ = injected;
    backing::open(paths, config.cap_bytes, config.image_bytes)
}

fn refuse_foreign_pool_entries(pool: &Path) -> Result<(), LeaseError> {
    for entry in fs::read_dir(pool).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let allowed = matches!(
            name.as_ref(),
            super::CONFIG_NAME
                | super::LEDGER_NAME
                | super::LOCK_NAME
                | super::IMAGE_NAME
                | super::RECEIPT_NAME
                | super::PAYLOAD_NAME
                | super::MANAGED_NAME
        ) || name.starts_with("ledger.tmp-");
        if !allowed {
            return Err(LeaseError::UnknownAccounting);
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
        if metadata.file_type().is_symlink() {
            return Err(LeaseError::UnsafePath("symlinks are refused"));
        }
    }
    Ok(())
}
