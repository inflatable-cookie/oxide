//! Read-only writer checks under admission, followed by origin invalidation.

use super::control_io::{probe_lock, Probe};
use super::{Phase, Store};
use oxide_storage::lease::{managed::snapshot, SlotOccupancy};

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
}
