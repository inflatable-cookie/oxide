//! The durable ledger: persisted config and ledger I/O, validation, capacity policy and slot ranks.

use super::*;

/// A record's membership fields, as status and the ledger view show them.
pub(super) fn ledger_slot(slot: &SlotRecord) -> LedgerSlot {
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
pub(super) fn require_holder(holder: &str) -> Result<(), LeaseError> {
    super::require_bounded_text(holder, MAX_HOLDER_BYTES)
}

pub(super) fn validate_lease_id(lease_id: &str) -> Result<(), LeaseError> {
    super::require_bounded_text(lease_id, MAX_LEASE_ID_BYTES)
}

pub(super) fn write_config(pool: &Path, config: &PoolConfig) -> Result<(), LeaseError> {
    let bytes = serde_json::to_vec(config).map_err(|error| LeaseError::Io(error.to_string()))?;
    persist_private_file(&pool.join(CONFIG_NAME), &bytes)
}

pub(super) fn write_ledger(pool: &Path, ledger: &Ledger) -> Result<(), LeaseError> {
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
pub(super) fn validate_ledger_write(ledger: &Ledger) -> Result<(), LeaseError> {
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

pub(super) fn read_config(pool: &Path) -> Result<PoolConfig, LeaseError> {
    let bytes = read_private_file(&pool.join(CONFIG_NAME))?;
    serde_json::from_slice(&bytes).map_err(|_| LeaseError::BackingMismatch)
}

pub(super) fn read_ledger(pool: &Path) -> Result<Ledger, LeaseError> {
    let bytes = read_private_file(&pool.join(LEDGER_NAME))?;
    let ledger: Ledger =
        serde_json::from_slice(&bytes).map_err(|_| LeaseError::UnknownAccounting)?;
    if !matches!(ledger.version, LEGACY_LEDGER_VERSION | LEDGER_VERSION) {
        return Err(LeaseError::BackingMismatch);
    }
    validate_ledger_read(&ledger)?;
    Ok(ledger)
}

impl Pool {
    /// Recorded identity and membership, as this open read them.
    pub(super) fn recorded(&self) -> PassiveSnapshot {
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

    pub(super) fn persist(&mut self) -> Result<(), LeaseError> {
        self.ledger.version = LEDGER_VERSION;
        write_ledger(&self.root, &self.ledger)?;
        Ok(())
    }

    /// A revised mutation: install the permanent capacity-policy fence if
    /// this ledger predates it, raise that fence when the slot table now
    /// exceeds the legacy ceiling, validate every stored field against the
    /// raised caps, then replace the ledger atomically. Legacy representation
    /// is never migrated by a read-only observer or by reconciliation alone.
    pub(super) fn persist_revised(&mut self) -> Result<(), LeaseError> {
        self.validate_revised_ledger()?;
        self.persist()
    }

    pub(super) fn validate_revised_ledger(&mut self) -> Result<(), LeaseError> {
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
    pub(super) fn ensure_capacity_policy(&mut self) -> Result<(), LeaseError> {
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
    pub(super) fn next_rank(&self) -> Result<u64, LeaseError> {
        match self.ledger.next_rank {
            Some(next) if next != 0 && next != u64::MAX => Ok(next),
            Some(_) => Err(LeaseError::LedgerExhausted),
            None => Ok(1),
        }
    }

    /// Stamp a new actual grant or a first held-to-idle release/recovery.
    /// Replays never call this.
    pub(super) fn stamp_rank(&mut self, id: u64) -> Result<(), LeaseError> {
        let rank = self.next_rank()?;
        let slot = self.slot_mut(id).ok_or(LeaseError::UnknownAccounting)?;
        slot.rank = rank;
        self.ledger.next_rank = Some(rank + 1);
        Ok(())
    }

    pub(super) fn mint_lease_id(
        &mut self,
        slot_id: u64,
        incarnation: u64,
    ) -> Result<String, LeaseError> {
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

    pub(super) fn find_holder_lease(&self, holder: &str) -> Option<&SlotRecord> {
        self.ledger
            .slots
            .iter()
            .find(|slot| slot.holder == holder && slot.state == SlotPersistState::Held)
    }

    pub(super) fn slot(&self, id: u64) -> Option<&SlotRecord> {
        self.ledger.slots.iter().find(|slot| slot.id == id)
    }

    pub(super) fn slot_mut(&mut self, id: u64) -> Option<&mut SlotRecord> {
        self.ledger.slots.iter_mut().find(|slot| slot.id == id)
    }

    /// Ledger membership without any walk.
    pub(super) fn ledger_view(&self) -> Result<LedgerView, LeaseError> {
        Ok(LedgerView {
            store_id: self.config.store_id.clone(),
            backend: self.config.backend.clone(),
            cap_bytes: self.config.cap_bytes,
            reconciliation_blocked: self.reconciliation_blocked,
            slots: self.ledger.slots.iter().map(ledger_slot).collect(),
        })
    }
    /// A recorded slot's path: its recorded directory, or the legacy
    /// `slots/<id>`.
    pub(super) fn slot_path(&self, id: u64) -> Result<PathBuf, LeaseError> {
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
