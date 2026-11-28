//! Lease allocation: idle reuse, fresh slot roots, and the admission charges and accounting the reserve reads.

use super::*;

pub(super) fn observed_size(
    slot: &SlotRecord,
    path: &Path,
    deadline: Instant,
) -> Option<serde_json::Value> {
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
    admission_cost::walked(1);
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
pub(super) fn occupancy(slot: &SlotRecord) -> SlotOccupancy {
    match slot.state {
        SlotPersistState::Held => SlotOccupancy::Held,
        SlotPersistState::Idle => SlotOccupancy::Idle,
        SlotPersistState::Evicting => SlotOccupancy::Abandoned,
    }
}

pub(super) fn describe_pins(pins: &BTreeMap<String, usize>, limit: usize) -> String {
    if pins.is_empty() {
        return "none".to_owned();
    }
    let mut reasons: Vec<_> = pins
        .iter()
        .take(limit)
        .map(|(reason, count)| format!("{count} \"{reason}\""))
        .collect();
    if pins.len() > limit {
        reasons.push(format!("{} more reasons", pins.len() - limit));
    }
    reasons.join(", ")
}

#[cfg(test)]
pub(in crate::lease) fn check_ample_reserve(pool: &Path, account: &Path) -> Result<(), LeaseError> {
    use crate::storage::{PoolBacking, PoolStorage, Storage};
    use std::sync::Arc;
    const UUID: &str = "01234567-89ab-cdef-0123-456789abcdef";
    const RESERVE: u64 = 4 << 20;
    let storage = Storage::open_with(
        account.to_path_buf(),
        Arc::new(barge::FakeOs::with_volume(
            pool.to_path_buf(),
            UUID,
            RESERVE + 1,
        )),
        Arc::new(barge::ManualClock::new(1_000)),
    )
    .unwrap();
    let mut opened = Pool::open_observed(pool, Phase::Reserve)?;
    opened.storage = Some(
        PoolStorage::from_storage(
            storage,
            &PoolBacking::volume(pool, UUID, "payload", 8 << 20, RESERVE, RESERVE / 2),
        )
        .unwrap(),
    );
    opened.evict_for_reserve(None)
}

/// Result of probing idle compatible owned slots for a `(project, profile)` pair.
enum IdleCompatible {
    Ready {
        slot_id: u64,
        incarnation: u64,
        hold: Box<barge::Hold>,
    },
    BargeHeld(u64),
    None,
}

impl Pool {
    pub(super) fn lease(&mut self, request: &LeaseRequest) -> Result<Leased, LeaseError> {
        self.pending_hold = None;
        let mut leased = self.lease_inner(request)?;
        leased.hold = self.pending_hold.take();
        if !self.eviction_notes.is_empty() {
            let note = self.eviction_notes.join("; ");
            leased.admission_note = Some(match leased.admission_note.take() {
                Some(existing) => format!("{existing}; {note}"),
                None => note,
            });
        }
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
            admission_cost::in_phase(Phase::Reserve, || storage.set_reserve(reserve))
                .map_err(LeaseError::Barge)?;
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
            if !held.owned {
                return Err(LeaseError::LegacyWriterUnverified(held.id));
            }
            if held.holder != request.holder {
                return Err(LeaseError::Busy);
            }
        }
        let candidate = self.takeable_idle_compatible(&project, request.profile)?;
        if matches!(candidate, IdleCompatible::None) {
            if let Some(legacy) = self.ledger.slots.iter().find(|slot| {
                slot.is_idle()
                    && !slot.owned
                    && !slot.adoption_refused
                    && slot.project == project
                    && slot.profile == request.profile
            }) {
                return Err(if legacy_writer_absent(&legacy.holder) {
                    LeaseError::LegacyAdoptionPending(legacy.id)
                } else {
                    LeaseError::LegacyWriterUnverified(legacy.id)
                });
            }
        }
        let reuse_id = match &candidate {
            IdleCompatible::Ready { slot_id, .. } => Some(*slot_id),
            IdleCompatible::BargeHeld(slot_id) => return Err(LeaseError::BargeHeld(*slot_id)),
            IdleCompatible::None => None,
        };
        if self
            .storage
            .as_ref()
            .is_some_and(crate::storage::PoolStorage::is_removable)
        {
            admission_cost::in_phase(Phase::Reserve, || self.evict_for_reserve(reuse_id))?;
        }
        if let IdleCompatible::Ready {
            slot_id,
            incarnation,
            hold,
        } = candidate
        {
            let grant =
                self.take_idle_with_hold(slot_id, incarnation, *hold, project, workspace, request)?;
            return Ok(Leased {
                grant,
                hold: None,
                measured: None,
                admission_note: None,
                unmeasured: Vec::new(),
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
            unmeasured,
        })
    }

    /// Find an idle compatible owned slot whose Barge hold is not currently held
    /// by another process, skipping any Barge-held slots so a lingering child's
    /// inherited hold does not mask a reusable sibling slot.
    fn takeable_idle_compatible(
        &self,
        project: &Path,
        profile: Profile,
    ) -> Result<IdleCompatible, LeaseError> {
        let mut held_by_area = BTreeMap::new();
        let mut barge_held = None;
        for slot in &self.ledger.slots {
            if !(slot.is_idle() && slot.owned && slot.project == project && slot.profile == profile)
            {
                continue;
            }
            let Some(root) = slot.root.as_deref() else {
                continue;
            };
            let storage = self.storage.as_ref().ok_or(LeaseError::UnknownAccounting)?;
            let folder = slot.area_folder();
            if held_by_area
                .get(&folder)
                .and_then(|ids: &Option<std::collections::BTreeSet<String>>| ids.as_ref())
                .is_some_and(|ids| ids.contains(root))
            {
                barge_held.get_or_insert(slot.id);
                continue;
            }
            match storage
                .try_hold_in(folder, root)
                .map_err(LeaseError::Barge)?
            {
                Some(hold) => {
                    return Ok(IdleCompatible::Ready {
                        slot_id: slot.id,
                        incarnation: slot.incarnation,
                        hold: Box::new(hold),
                    })
                }
                None => {
                    barge_held.get_or_insert(slot.id);
                    // The first refusal gets the spawn retry; use a single-attempt
                    // snapshot to skip other holders rather than waiting per row.
                    if held_by_area.is_empty() {
                        for candidate in &self.ledger.slots {
                            if !(candidate.is_idle()
                                && candidate.owned
                                && candidate.project == project
                                && candidate.profile == profile
                                && candidate.root.is_some())
                            {
                                continue;
                            }
                            let candidate_folder = candidate.area_folder();
                            if !held_by_area.contains_key(&candidate_folder) {
                                let held = storage.held_root_ids_in(candidate_folder).ok();
                                held_by_area.insert(candidate_folder, held);
                            }
                        }
                    }
                }
            }
        }
        Ok(match barge_held {
            Some(slot_id) => IdleCompatible::BargeHeld(slot_id),
            None => IdleCompatible::None,
        })
    }

    fn take_idle_with_hold(
        &mut self,
        slot_id: u64,
        incarnation: u64,
        hold: barge::Hold,
        project: PathBuf,
        workspace: PathBuf,
        request: &LeaseRequest,
    ) -> Result<LeaseGrant, LeaseError> {
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

    pub(super) fn allocate(
        &mut self,
        project: PathBuf,
        workspace: PathBuf,
        request: &LeaseRequest,
    ) -> Result<(LeaseGrant, Vec<UnmeasuredSlot>), LeaseError> {
        if let Some(victim) = request.replace_slot {
            // A committed rename whose trash copy stayed is progress, and the
            // grant reports what it kept instead of dropping the note
            // (oxide#337).
            let kept = self.retire_idle_slot(victim)?;
            self.note_eviction(kept);
        }
        // Retire count victims before measuring any unrecorded survivors.
        if self.ledger.slots.len() >= MAX_SLOTS {
            admission_cost::in_phase(Phase::Eviction, || self.make_slot_room())?;
        }
        let (charges, unmeasured) = self.admission_charges()?;
        // One protection listing decides every victim this loop retires:
        // retiring a root changes nothing about the roots that stay
        // candidates, so a fresh listing per victim re-read the same inventory
        // (oxide#337). It is taken only when the loop first needs it.
        let mut protection: Option<BTreeMap<PathBuf, barge::RootInfo>> = None;
        while self
            .ledger
            .slots
            .iter()
            .map(|slot| charges[&slot.id])
            .fold(0, u64::saturating_add)
            >= self.budget
        {
            match self.evict_one_idle_listed(Phase::Eviction, &mut protection, |id| {
                !unmeasured.iter().any(|slot| slot.slot_id == id.to_string())
            })? {
                Evicted::None => return Err(LeaseError::Full),
                Evicted::Retired => {}
                Evicted::KeptTrash(kept) => self.note_eviction(Some(kept)),
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
        // A new slot is a Barge root: the library mints the id and path,
        // writes the ownership marker and gives us the cooperative hold that
        // keeps the root through the command. Every mutating open has a
        // storage authority; one that does not cannot allocate.
        let storage = self.storage.clone().ok_or(LeaseError::UnknownAccounting)?;
        let slots_dir = self.backing.payload_root().join(SLOTS_NAME);
        let budget = max_barge_label_bytes(&slots_dir);
        let root = storage
            .create(&barge_label(request.layout.as_ref(), budget))
            .map_err(LeaseError::Barge)?;
        // From here on this is a marked Barge root with no ledger row. A
        // failure before the row is durable drops the hold and evicts the
        // fresh id again, so no marked root is ever left behind.
        if let Err(error) = self.prepare_fresh_root(&storage, &root) {
            self.discard_unrecorded_root(&storage, &root.id);
            return Err(error);
        }
        let root_id = Some(root.id.clone());
        let dir = Some(root.folder);
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
            root: root_id.clone(),
            owned: true,
            adoption_refused: false,
            rank: 0,
            recorded: None,
            capture: None,
            hold_intent: None,
        });
        let record = (|| -> Result<(), LeaseError> {
            self.ledger.next_slot = slot_id.checked_add(1).ok_or(LeaseError::LedgerExhausted)?;
            self.stamp_rank(slot_id)?;
            self.persist_revised()
        })();
        if let Err(error) = record {
            // The row never became durable, so the fresh root has none: evict it
            // rather than leave a marked root no ledger names.
            if let (Some(storage), Some(root)) = (self.storage.clone(), root_id.clone()) {
                self.discard_unrecorded_root(&storage, &root);
            }
            return Err(error);
        }
        let slot = self.slot(slot_id).ok_or(LeaseError::UnknownAccounting)?;
        let grant = self.grant_from(slot, false)?;
        Ok((grant, unmeasured))
    }

    /// Finish a fresh root: its build directory exists and this admission holds
    /// it for the life of the command.
    fn prepare_fresh_root(
        &mut self,
        storage: &crate::storage::PoolStorage,
        root: &barge::Root,
    ) -> Result<(), LeaseError> {
        ensure_private_dir(&root.path.join("build"))?;
        let hold = storage.hold(&root.id).map_err(LeaseError::Barge)?;
        self.pending_hold = Some(hold);
        Ok(())
    }

    /// Drop a root this admission created but could not record. The hold this
    /// admission took is released first, and the root is evicted only when the
    /// durable ledger does not name it: a root that may be recorded is kept,
    /// because deleting it would strand a live row.
    fn discard_unrecorded_root(&mut self, storage: &crate::storage::PoolStorage, root: &str) {
        self.pending_hold = None;
        let recorded = match read_ledger(&self.root) {
            Ok(ledger) => ledger
                .slots
                .iter()
                .any(|slot| slot.root.as_deref() == Some(root)),
            Err(_) => true,
        };
        if !recorded {
            // The same pool-area eviction the other paths use, so the admission
            // cost observation counts this root's deletion walk.
            let _ = storage.evict_in(None, root);
        }
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

    /// Sample only idle slots lacking a current observation, once before
    /// pressure decisions. A held row is never sampled: its tree is still being
    /// written, so a walk under this lock would be long and its figure would
    /// describe a build in progress. It is charged the largest current
    /// observation instead, and a partial walk gets the same positive,
    /// unpublished estimate and cannot nominate a byte-pressure victim.
    fn admission_charges(
        &mut self,
    ) -> Result<(BTreeMap<u64, u64>, Vec<UnmeasuredSlot>), LeaseError> {
        let deadline = Instant::now() + Duration::from_millis(ACCOUNTING_MAX_ELAPSED_MS);
        let mut charges = BTreeMap::new();
        let mut missing = Vec::new();
        let mut changed = false;
        for index in 0..self.ledger.slots.len() {
            let slot = &self.ledger.slots[index];
            if slot.is_idle() && slot.recorded_size().is_none() && Instant::now() < deadline {
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
    pub(super) fn accounted(&self, measure: bool) -> Result<PoolStatus, LeaseError> {
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
        })
    }
}
