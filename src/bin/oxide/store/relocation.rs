//! Read-only writer checks under admission, followed by origin invalidation.

use super::control_io::{probe_lock, Probe};
use super::{Phase, Store};
use oxide_storage::lease::{managed::snapshot, SlotOccupancy};
use std::fs::File;
use std::path::Path;

impl Store {
    /// The caller holds this pool's admission lock. Refuse unknown build
    /// custody as well as a live writer or waiter.
    pub(crate) fn require_quiet_for_relocation(&self) -> Result<(), String> {
        super::check_writer_generation(self.control())?;
        let state = self.load()?;
        for record in &state.records {
            if record.phase != Phase::Idle
                || matches!(probe_lock(&self.lock_path(record.id), false)?, Probe::Held)
            {
                return Err(format!(
                    "build record {} is active or its custody is unknown",
                    record.id
                ));
            }
        }
        for ticket in &state.tickets {
            if matches!(
                probe_lock(&self.ticket_path(ticket.id), false)?,
                Probe::Held
            ) {
                return Err(format!("waiter ticket {} is held", ticket.id));
            }
        }
        for clean in &state.cleans {
            if matches!(probe_lock(&self.ticket_path(clean.id), false)?, Probe::Held)
                || clean
                    .process_group
                    .is_some_and(|group| !crate::process::group_empty(group))
            {
                return Err(format!(
                    "checkout clean is in progress at {}",
                    clean.checkout.display()
                ));
            }
        }
        let view = snapshot(&self.pool).map_err(|error| error.to_string())?;
        if !view.evicting_slot_ids.is_empty()
            || view
                .slots
                .iter()
                .any(|slot| slot.state != SlotOccupancy::Idle)
        {
            return Err(format!(
                "{} has a held slot or unfinished eviction",
                self.pool.display()
            ));
        }
        Ok(())
    }

    /// Origins describe builds in this pool's old absolute build directory.
    /// An origin already untrusted needs no write.
    pub(crate) fn invalidate_relocated_origins(&self) -> Result<usize, String> {
        let mut count = 0;
        for record in &self.load()?.records {
            if self.trusted_origin(record).is_some() {
                self.write_origin(record.id, record.last_used, None)?;
                count += 1;
            }
        }
        Ok(count)
    }

    /// Resolve a legacy slot's store record by its recorded affinity path.
    /// The library slot ID and front-door record ID are separate sequences.
    pub(crate) fn lock_legacy_slot_record(
        &self,
        project: &Path,
        workspace: &Path,
    ) -> Result<Option<(u64, Option<File>)>, String> {
        let Some(id) = project
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<u64>().ok())
            .filter(|&id| project == self.affinity_dir(id))
        else {
            return Ok(None);
        };
        let lock = match probe_lock(&self.lock_path(id), false)? {
            Probe::Free(lock) => Some(lock),
            Probe::Missing => None,
            Probe::Held => return Err(format!("legacy slot record {id} is held")),
        };
        let state = self.load()?;
        if let Some(record) = state.records.iter().find(|record| record.id == id) {
            if record
                .checkout
                .as_deref()
                .is_some_and(|checkout| checkout != workspace)
            {
                return Err(format!(
                    "legacy slot record {id} names a different checkout"
                ));
            }
            return Ok(Some((id, lock)));
        }
        // A lock can outlive its state row and must still be checked above.
        Ok(None)
    }

    /// Forget the idle affinity while admission and its record lock are held.
    /// The ledger is retired last, so a failure here can be retried.
    pub(crate) fn forget_legacy_slot_record(&self, id: u64) -> Result<(), String> {
        let mut state = self.load()?;
        if !state.records.iter().any(|record| record.id == id) {
            return Ok(());
        }
        state.records.retain(|record| record.id != id);
        self.save_for_writer(&state)?;
        self.forget_record(&mut state, id);
        Ok(())
    }
}
