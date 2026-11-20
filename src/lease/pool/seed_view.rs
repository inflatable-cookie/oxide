//! Read-only seed pool views: opened for seeding without taking a lease.

use super::*;

/// One ledger slot as the seed custody calls read it: an immutable snapshot
/// taken under the pool lock. Seeding never writes ledger state.
pub(in crate::lease) struct SeedSlotView {
    pub(in crate::lease) id: u64,
    pub(in crate::lease) incarnation: u64,
    pub(in crate::lease) lease_id: Option<String>,
    pub(in crate::lease) last_lease_id: String,
    pub(in crate::lease) rank: u64,
    pub(in crate::lease) dir: Option<String>,
    /// The recorded project: for a front-door slot this is the pool's
    /// `oxide-managed/affinity/<record id>` directory, which is what binds a
    /// slot to its front-door record.
    pub(in crate::lease) project: PathBuf,
    pub(in crate::lease) held: bool,
    pub(in crate::lease) idle: bool,
    root: Option<String>,
    owned: bool,
}

/// The pool and ledger facts call 1 and call 2 may read.
pub(in crate::lease) struct SeedPoolView {
    pub(in crate::lease) store_id: String,
    pub(in crate::lease) pool_root: PathBuf,
    /// The payload root. Call 1 and the sidecar writers need no payload, so
    /// their seam leaves this `None` and opens no volume; call 2 uses
    /// [`open_seed_writer`] and gets the verified root for both backends.
    pub(in crate::lease) payload_root: Option<PathBuf>,
    pub(in crate::lease) slots: Vec<SeedSlotView>,
}

impl SeedPoolView {
    pub(in crate::lease) fn slot(&self, id: &str) -> Option<&SeedSlotView> {
        self.slots.iter().find(|slot| slot.id.to_string() == id)
    }
}

/// The seeding custody seam (`oxide#137` §6.3 calls 1 and 2 and
/// `publish_seed_catalog`): the pool lock held for the whole transaction plus
/// the immutable facts those calls read. It takes no reconciliation and writes
/// nothing. [`open_seed`] opens no payload; [`open_seed_writer`] additionally
/// opens the verified payload call 2 clones from, including a removable
/// `apfs-volume-v1` volume's mount and volume-UID binding (§7, §8.3).
pub(in crate::lease) struct SeedPoolOpen {
    _lock: File,
    pub(in crate::lease) view: SeedPoolView,
    storage: Option<crate::storage::PoolStorage>,
}

impl SeedPoolOpen {
    /// Share the donor's Barge hold for every payload read and clone. The
    /// recipient's exclusive hold belongs to the caller's live lease and is
    /// retained through this call; a ledger row alone does not replace it.
    pub(in crate::lease) fn hold_donor(
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
    pub(in crate::lease) fn donor_unchanged(
        &self,
        donor: &SeedSlotView,
    ) -> Result<bool, LeaseError> {
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
    pub(in crate::lease) fn recipient_held(
        &self,
        recipient: &SeedSlotView,
    ) -> Result<bool, LeaseError> {
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
pub(in crate::lease) fn open_seed(root: &Path) -> Result<SeedPoolOpen, LeaseError> {
    open_seed_inner(root, false, false)
}

/// The same seam, additionally opening the payload call 2 clones from. For a
/// removable `apfs-volume-v1` pool this verifies the recorded mount and volume
/// UUID through [`Binding::open`] and the recorded owner-only payload
/// directory, under the pool lock and before any donor payload is read.
pub(in crate::lease) fn open_seed_writer(
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
