//! Recovery and cleanup: abandoned holds, legacy adoption, idle eviction and the release path.

use super::*;

/// Only the front door's holder format supplies a PID that can be checked.
/// An opaque or malformed legacy token supplies no evidence of closure.
pub(super) fn legacy_writer_absent(holder: &str) -> bool {
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

pub(super) fn require_deleted(evicted: barge::area::Eviction) -> Result<(), LeaseError> {
    if evicted.deleted {
        Ok(())
    } else {
        Err(LeaseError::EvictionIncomplete(format!(
            "slot root {} moved to trash and could not be deleted: {}",
            evicted.id,
            evicted.kept_reason.as_deref().unwrap_or("reason unknown")
        )))
    }
}

/// Whether a listing identifies `path` as `root` and neither held nor pinned.
fn require_unprotected(
    protection: &BTreeMap<PathBuf, barge::RootInfo>,
    path: &Path,
    root: &str,
) -> Result<(), LeaseError> {
    match protection.get(path) {
        Some(info) if info.id == root && !info.held && !info.pinned => Ok(()),
        _ => Err(LeaseError::Busy),
    }
}

/// Whether one hand-off record names exactly this idle row, incarnation and
/// root. The record is committed before the row is dropped, so a match means
/// the record outlived a ledger write that never landed. Only an idle row is
/// matched: that is the only state a hand-off record is written for, and a held
/// generation is never retired on a record's word.
fn pending_names_slot(record: &pending::PendingEviction, slot: &SlotRecord) -> bool {
    slot.is_idle()
        && slot.id == record.slot_id
        && slot.incarnation == record.incarnation
        && slot.root.as_deref() == Some(record.root_id.as_str())
}

/// Resolve one legacy root's Barge identity. Barge's own error is returned so
/// the caller can still tell `NotFound` from an unreadable account.
fn resolve_legacy_root(area: &barge::Area, id: &str) -> Result<barge::Root, barge::Error> {
    #[cfg(test)]
    if FAIL_ADOPTION.with(|flag| flag.replace(false)) {
        return Err(barge::Error::unknown_state(
            "injected Barge failure while resolving a legacy root",
        ));
    }
    area.resolve(id)
}

impl Pool {
    /// A held row whose Barge root can be held again has no live holder: the
    /// process that held it is gone and the kernel released its lock. Mark it
    /// idle so the next compatible command reuses it. A row something still
    /// holds, and a root whose lock cannot be tested, are kept as they are.
    ///
    /// This is the whole crash-recovery rule for a Barge-owned slot: the flock
    /// is the authority, not a process or descriptor census.
    pub(super) fn recover_abandoned_holds(
        &mut self,
        releasing: Option<&str>,
    ) -> Result<(), LeaseError> {
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
            // The probe is one non-blocking `flock`, never a byte walk: it
            // must not be reported as one (oxide#337).
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

    /// Advance a fixed batch on the front door's preparation path. Each
    /// finished row is durable before the next row is attempted.
    pub(super) fn adopt_legacy_roots(&mut self) -> Result<(), LeaseError> {
        let Some(storage) = self.storage.clone() else {
            return Ok(());
        };
        let mut remaining = LEGACY_ADOPTION_BATCH;
        for index in 0..self.ledger.slots.len() {
            let slot = &self.ledger.slots[index];
            if slot.owned
                || slot.adoption_refused
                || !slot.is_idle()
                || !legacy_writer_absent(&slot.holder)
            {
                continue;
            }
            if remaining == 0 {
                break;
            }
            remaining -= 1;
            let path = self.slot_path(slot.id)?;
            let folder = slot.dir.clone().unwrap_or_else(|| slot.id.to_string());
            let leaf = folder
                .rsplit('/')
                .next()
                .ok_or(LeaseError::UnknownAccounting)?;
            let id = legacy_root_id(&folder);
            let label = slot.profile.as_str();
            let area = storage.area_for(slot.area_folder()).map_err(|error| {
                LeaseError::Barge(format!(
                    "cannot declare the area of legacy slot {}: {error}",
                    slot.id
                ))
            })?;
            admission_cost::opaque_listing();
            let adopted = match resolve_legacy_root(&area, &id) {
                // The pinned Barge API can unpin by ID but cannot compare
                // pin provenance under its lock. Keep any existing pin.
                Ok(root) => Ok(root.path == path
                    && root.label == label
                    && slot.root.as_deref().is_none_or(|recorded| recorded == id)
                    && fs::symlink_metadata(&path).is_ok_and(|metadata| {
                        metadata.is_dir()
                            && metadata.uid() == current_uid()
                            && metadata.mode() & 0o022 == 0
                    })),
                Err(error) if error.kind() == barge::ErrorKind::NotFound => {
                    // A published id is never attached to a replacement path.
                    if slot.root.is_some() {
                        Ok(false)
                    } else {
                        match storage.adopt_in(slot.area_folder(), leaf, label, &id) {
                            Ok(root) => Ok(root.path == path),
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    barge::ErrorKind::Refused | barge::ErrorKind::NotFound
                                ) =>
                            {
                                Ok(false)
                            }
                            Err(error) => Err(LeaseError::Barge(format!(
                                "cannot adopt legacy slot {} at {}: {error}",
                                slot.id,
                                path.display()
                            ))),
                        }
                    }
                }
                Err(error) => Err(LeaseError::Barge(format!(
                    "cannot resolve legacy slot {} root {id}: {error}",
                    slot.id
                ))),
            };
            match adopted {
                Ok(true) => {
                    let slot = &mut self.ledger.slots[index];
                    slot.root = Some(id);
                    slot.owned = true;
                }
                Ok(false) => self.ledger.slots[index].adoption_refused = true,
                // A Barge error must not refuse every lease, release and
                // prepare. Keep this row out of service — it is never granted,
                // evicted or adopted again — and report the open as blocked.
                Err(error) => {
                    eprintln!(
                        "oxide: warning: legacy slot {} is kept out of service: {error}",
                        self.ledger.slots[index].id
                    );
                    self.ledger.slots[index].adoption_refused = true;
                    self.reconciliation_blocked = true;
                }
            }
            // Publish each transition before visiting another root. If the
            // write failed after adoption, the next preparation resolves the
            // deterministic id above without another adoption.
            self.persist()?;
        }
        Ok(())
    }

    /// A completed Barge rename may precede the workflow ledger write. Only a
    /// positively absent, transitioned Barge root can retire its idle row;
    /// existing or unreadable paths stay recorded and are never re-adopted.
    pub(super) fn retire_missing_idle_roots(&mut self) -> Result<(), LeaseError> {
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

    /// Roll forward a deferred idle eviction whose ledger write failed.
    ///
    /// The hand-off record is durable before the row is dropped, so a row that
    /// is still recorded was never carried past that write: settling would
    /// otherwise evict a root an admission could still grant, or leave a slot
    /// recorded idle with no root. Dropping the row under this lock restores
    /// the invariant that no record names a live row. If the save fails the
    /// record is removed instead, so the surviving row and its root stay
    /// consistent.
    pub(super) fn reconcile_pending_evictions(&mut self) -> Result<(), LeaseError> {
        let (records, quarantined) = pending::read_quarantining(&self.root)?;
        if quarantined {
            // One unreadable hand-off list used to refuse every command. Its
            // roots are left exactly as recorded, so no deletion decision is
            // invented from it.
            eprintln!(
                "oxide: warning: an unreadable idle-eviction record was quarantined; its roots are kept as recorded"
            );
            self.reconciliation_blocked = true;
        }
        let matched: Vec<pending::PendingEviction> = records
            .into_iter()
            .filter(|record| {
                self.ledger
                    .slots
                    .iter()
                    .any(|slot| pending_names_slot(record, slot))
            })
            .collect();
        if matched.is_empty() {
            // A decided idle eviction whose root is already gone finished its
            // Barge rename; its record is only bookkeeping now.
            return pending::drop_completed(&self.root);
        }
        self.ledger.slots.retain(|slot| {
            !matched
                .iter()
                .any(|record| pending_names_slot(record, slot))
        });
        match self.persist_revised() {
            Ok(()) => pending::drop_completed(&self.root),
            Err(error) => {
                // The row is still durable, so the record must not stay: a
                // settle would otherwise evict a root the ledger still names.
                pending::remove_settled(&self.root, &matched)?;
                Err(error)
            }
        }
    }

    /// Commit Barge's completed evictions to workflow membership before the
    /// next compatible-root lookup. Barge still owns all deletion decisions.
    #[cfg(test)]
    pub(super) fn prune_storage(&mut self) -> Result<(), LeaseError> {
        let storage = self.storage.as_ref().ok_or(LeaseError::UnknownAccounting)?;
        let report = storage.prune(false).map_err(LeaseError::Barge)?;
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

    pub(super) fn release_or_recover(
        &mut self,
        lease_id: &str,
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
        let recorded = crate::release_trace::phase("release walk", || {
            let slot = &self.ledger.slots[index];
            self.slot_path(slot.id).ok().and_then(|path| {
                observed_size(
                    slot,
                    &path,
                    Instant::now() + Duration::from_millis(RELEASE_WALK_MAX_ELAPSED_MS),
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

    /// Retire one idle recorded slot: the decision is committed durably and
    /// the ledger row is dropped. Barge evicts the owned root later, with the
    /// pool lock released, and its own checks still decide (oxide#324). The
    /// returned message is trash a committed inline rename left in place.
    pub(super) fn retire_idle_slot(&mut self, id: u64) -> Result<Option<String>, LeaseError> {
        self.retire_idle_slot_in_phase(id, Phase::Eviction)
    }

    fn retire_idle_slot_in_phase(
        &mut self,
        id: u64,
        phase: Phase,
    ) -> Result<Option<String>, LeaseError> {
        // Free-space pressure must observe the bytes a deletion returns before
        // it can decide again, so its eviction stays one inline call. Every
        // other route (the slot ceiling, the byte budget, an explicit
        // `oxide clean`) hands the deletion to a settle pass.
        let defer = phase != Phase::Reserve;
        admission_cost::in_phase(phase, || self.retire_idle_slot_known(id, None, defer))
    }

    /// Retire one idle row, using a protection listing the caller already paid
    /// for when it has one. The listing is a bytes-free observation; Barge's
    /// own eviction rechecks the same facts when it runs. The returned message
    /// is trash a committed rename left behind: progress, not a refusal.
    fn retire_idle_slot_known(
        &mut self,
        id: u64,
        protection: Option<&BTreeMap<PathBuf, barge::RootInfo>>,
        defer: bool,
    ) -> Result<Option<String>, LeaseError> {
        let Some(slot) = self.slot(id) else {
            return Err(LeaseError::Busy);
        };
        if !slot.is_idle() || !slot.owned {
            return Err(LeaseError::Busy);
        }
        let root = slot
            .root
            .as_deref()
            .ok_or(LeaseError::UnknownAccounting)?
            .to_owned();
        let path = self.slot_path(id)?;
        let area_folder = slot.area_folder().map(str::to_owned);
        let recorded = slot.recorded_size();
        let record = pending::PendingEviction {
            store_id: self.config.store_id.clone(),
            slot_id: id,
            incarnation: slot.incarnation,
            root_id: root.clone(),
            area_folder: area_folder.clone(),
            path: path.clone(),
            bytes: recorded.map_or(0, |fact| fact.logical_bytes),
            bytes_approximate: recorded.is_none(),
        };
        if !defer {
            return self.retire_inline(id, &root, area_folder);
        }
        // Barge refuses a held or pinned root only when its eviction runs, and
        // that now runs after this lock is released. Decide the same fact here
        // with a bytes-free listing, so a protected candidate is still skipped
        // for the next one instead of being committed to a record that can
        // never finish.
        match protection {
            Some(protection) => require_unprotected(protection, &path, &root)?,
            None => self.require_evictable(&path, &root)?,
        }
        match pending::push(&self.root, &self.config.store_id, record.clone()) {
            Ok(()) => {
                // The hand-off record is durable. Dropping the row now is the
                // commit: the root is no longer in the ledger, so nothing can
                // grant or reuse it while Barge's eviction waits for a free
                // pool lock.
                let index = self
                    .ledger
                    .slots
                    .iter()
                    .position(|slot| slot.id == id)
                    .ok_or(LeaseError::Busy)?;
                let removed = self.ledger.slots.remove(index);
                match self.persist_revised() {
                    Ok(()) => Ok(None),
                    Err(error) => {
                        // The row is still durable, so the hand-off record must
                        // not survive: a settle pass running beside this
                        // command would otherwise evict a root an admission can
                        // still grant (oxide#337), exactly as
                        // `reconcile_pending_evictions` keeps that invariant.
                        self.ledger.slots.insert(index, removed);
                        pending::remove_settled(&self.root, &[record])?;
                        Err(error)
                    }
                }
            }
            // A full hand-off list falls back to the inline eviction rather
            // than growing the record file without bound.
            Err(LeaseError::LedgerExhausted) => self.retire_inline(id, &root, area_folder),
            Err(error) => Err(error),
        }
    }

    /// The inline eviction: Barge renames and deletes inside this pool lock.
    /// Free-space pressure needs the returned bytes before it can decide again,
    /// and a full hand-off list falls back to it rather than grow without
    /// bound.
    ///
    /// A committed rename is progress even when the trash copy could not be
    /// deleted: the row is gone, so the caller re-reads free space and reports
    /// the kept tree instead of failing and draining the next idle slot.
    fn retire_inline(
        &mut self,
        id: u64,
        root: &str,
        area_folder: Option<String>,
    ) -> Result<Option<String>, LeaseError> {
        let storage = self.storage.as_ref().ok_or(LeaseError::UnknownAccounting)?;
        let evicted = storage
            .evict_in(area_folder.as_deref(), root)
            .map_err(|error| {
                if crate::storage::eviction_protected(&error) {
                    LeaseError::Busy
                } else {
                    LeaseError::EvictionIncomplete(format!(
                        "cannot evict slot root {root}: {error}"
                    ))
                }
            })?;
        self.ledger.slots.retain(|slot| slot.id != id);
        self.persist_revised()?;
        match require_deleted(evicted) {
            Ok(()) => Ok(None),
            Err(kept) => Ok(Some(kept.to_string())),
        }
    }

    /// One bytes-free inventory of the pool's roots, keyed by path.
    fn protection_listing(&self) -> Result<BTreeMap<PathBuf, barge::RootInfo>, LeaseError> {
        let storage = self.storage.as_ref().ok_or(LeaseError::UnknownAccounting)?;
        Ok(storage
            .list_protection()
            .map_err(LeaseError::EvictionIncomplete)?
            .into_iter()
            .map(|info| (info.path.clone(), info))
            .collect())
    }

    /// A candidate Barge would refuse right now: held by another writer, or
    /// pinned. A root this listing cannot identify is not a candidate.
    fn require_evictable(&self, path: &Path, root: &str) -> Result<(), LeaseError> {
        let protection = self.protection_listing()?;
        require_unprotected(&protection, path, root)
    }

    /// Retire one idle slot named by its numeric id. The returned messages are
    /// the trash a done eviction could not delete, so `oxide clean` reports
    /// what it kept instead of claiming the bytes came back.
    pub(super) fn evict_idle(&mut self, slot_id: &str) -> Result<Vec<String>, LeaseError> {
        let id =
            slot_id
                .parse::<u64>()
                .ok()
                .filter(|id| *id != 0)
                .ok_or(LeaseError::InvalidInput(
                    "slot ID must be a positive integer",
                ))?;
        let kept = self.retire_idle_slot(id)?;
        Ok(kept.into_iter().collect())
    }

    fn volume_space(&self) -> Result<barge::Space, LeaseError> {
        self.storage
            .as_ref()
            .ok_or(LeaseError::UnknownAccounting)?
            .space()
            .map_err(LeaseError::Barge)
    }

    pub(super) fn evict_for_reserve(&mut self, reuse: Option<u64>) -> Result<(), LeaseError> {
        let mut space = self.volume_space()?;
        // Keep the compatible root the admission is about to claim. Reread
        // actual free space after every completed eviction, including one whose
        // trash copy stayed: the row is gone and the rename is Barge's, so the
        // next decision is made on real space instead of the same failure.
        while space.free_bytes < space.reserve_bytes {
            match self.evict_one_idle_with(Phase::Reserve, |id| Some(id) != reuse)? {
                Evicted::None => break,
                Evicted::Retired => {}
                Evicted::KeptTrash(kept) => self.note_eviction(Some(kept)),
            }
            space = self.volume_space()?;
        }
        if space.free_bytes < space.floor_bytes {
            return Err(LeaseError::Full);
        }
        Ok(())
    }

    /// Keep a report from a committed eviction that left its trash copy. It is
    /// attached to the grant's admission note, never printed as a refusal.
    pub(super) fn note_eviction(&mut self, kept: Option<String>) {
        if let Some(kept) = kept {
            self.eviction_notes.push(kept);
        }
    }

    pub(super) fn make_slot_room(&mut self) -> Result<(), LeaseError> {
        let candidates = self.ranked_idle(|_| true);
        // One bytes-free listing decides every candidate: a held or pinned
        // root is skipped for the next one before any decision is committed.
        let protection = self.protection_listing()?;
        for (_, id) in &candidates {
            match self.retire_idle_slot_known(*id, Some(&protection), true) {
                Ok(kept) => {
                    self.note_eviction(kept);
                    return Ok(());
                }
                Err(LeaseError::Busy) => {}
                Err(error) => return Err(error),
            }
        }

        let mut held = 0;
        let mut pinned = 0;
        let mut unavailable = 0;
        let mut pending = 0;
        let mut pins = BTreeMap::new();
        for slot in &self.ledger.slots {
            let path = self.slot_path(slot.id)?;
            let info = protection
                .get(&path)
                .filter(|info| slot.root.as_deref() == Some(&info.id));
            let is_held =
                slot.state == SlotPersistState::Held || info.is_some_and(|info| info.held);
            let is_pinned = info.is_some_and(|info| info.pinned);
            held += usize::from(is_held);
            pinned += usize::from(is_pinned);
            unavailable += usize::from(!is_held && !is_pinned);
            pending += usize::from(
                !slot.owned
                    && !slot.adoption_refused
                    && slot.is_idle()
                    && legacy_writer_absent(&slot.holder),
            );
            if let Some(info) = info.filter(|info| info.pinned) {
                let reason = crate::storage::slot_pin_reason(&path, slot.area_folder(), &info.id)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "unknown".to_owned());
                *pins.entry(reason).or_insert(0) += 1;
            }
        }

        let reasons = describe_pins(&pins, 8);
        Err(LeaseError::SlotCeiling(format!(
            "{MAX_SLOTS}-slot ceiling: {held} held, {pinned} pinned, {unavailable} other unavailable ({pending} pending legacy adoption); pin reasons {reasons}"
        )))
    }

    /// Retire the oldest eligible slot. Barge checks the actual hold and pin
    /// of the named root, including a hold inherited by a child. `None` means
    /// no candidate was evictable; a kept-trash message means one was retired
    /// and the caller may decide again.
    fn evict_one_idle_with(
        &mut self,
        phase: Phase,
        eligible: impl Fn(u64) -> bool,
    ) -> Result<Evicted, LeaseError> {
        let mut protection = None;
        self.evict_one_idle_listed(phase, &mut protection, eligible)
    }

    /// [`Self::evict_one_idle_with`] over a protection-listing cache the caller
    /// owns, so a loop that retires several victims lists protection once
    /// (oxide#337). The listing is still taken only when a candidate exists.
    pub(super) fn evict_one_idle_listed(
        &mut self,
        phase: Phase,
        protection: &mut Option<BTreeMap<PathBuf, barge::RootInfo>>,
        eligible: impl Fn(u64) -> bool,
    ) -> Result<Evicted, LeaseError> {
        let candidates = self.ranked_idle(eligible);
        if candidates.is_empty() {
            return Ok(Evicted::None);
        }
        let defer = phase != Phase::Reserve;
        if defer && protection.is_none() {
            *protection = Some(self.protection_listing()?);
        }
        for (_, id) in candidates {
            match admission_cost::in_phase(phase, || {
                self.retire_idle_slot_known(id, protection.as_ref(), defer)
            }) {
                Ok(None) => return Ok(Evicted::Retired),
                Ok(Some(kept)) => return Ok(Evicted::KeptTrash(kept)),
                Err(LeaseError::Busy) => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(Evicted::None)
    }

    fn ranked_idle(&self, eligible: impl Fn(u64) -> bool) -> Vec<(u64, u64)> {
        let mut candidates: Vec<(u64, u64)> = self
            .ledger
            .slots
            .iter()
            .filter(|slot| slot.is_idle() && slot.owned && slot.root.is_some() && eligible(slot.id))
            .map(|slot| (slot.rank, slot.id))
            .collect();
        candidates.sort_unstable();
        candidates
    }
}
