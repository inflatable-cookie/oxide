//! Handing a slot back: the ordinary release, the refused-share restore, the
//! untrusted discard and the owned child temporary directory's removal.
//!
//! Every path here runs after the caller saw its own writer group close, and
//! every one keeps the slot held when a later step cannot confirm closure.

use super::*;
use crate::freshness::Built;
use oxide_storage::lease::managed::{evict_idle_slot, release};

use super::control_io::lease_error;

/// What one release call did with the slot.
///
/// `Ok` alone is not enough for a caller's terminal report: a release may be
/// deliberately deferred and leave the lease held.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleaseDisposition {
    /// The slot was handed back and its record marked idle.
    Released,
    /// The release was deliberately deferred (the owned child temporary
    /// directory could not be removed, or a removable flush could not
    /// complete). The lease stays held for a later command to recover.
    Deferred,
}

impl Store {
    /// Release after every writer in the group has exited, recording the
    /// slot's compile origin (`None`: untrusted from now on).
    pub fn release(
        &self,
        held: Lease,
        origin: Option<Built>,
    ) -> Result<ReleaseDisposition, String> {
        let _admission =
            oxide_storage::release_trace::phase("admission lock", || self.release_admission())?;
        if !oxide_storage::release_trace::phase("temporary directory removal", || {
            self.discard_scratch(&held)
        }) || !oxide_storage::release_trace::phase("removable flush", || {
            self.flush_before_idle(&held.slot_id, held.record)
        })? {
            return Ok(ReleaseDisposition::Deferred);
        }
        release(&self.pool, &held.lease_id)
            .map_err(|error| lease_error("could not release the slot", &error))?;
        oxide_storage::release_trace::phase("origin and control write", || {
            let mut state = self.load()?;
            if let Some(index) = state
                .records
                .iter()
                .position(|record| record.id == held.record)
            {
                state.settle_idle(index);
                state.touch(index)?;
                self.write_origin(held.record, state.records[index].last_used, origin)?;
            }
            self.save(&state)
        })
        .map(|()| ReleaseDisposition::Released)
    }

    /// The gate refused a slot taken from another checkout before Cargo ran:
    /// release it and give it back, origin unchanged.
    pub fn unshare(&self, held: Lease) -> Result<(), String> {
        let _admission = self.admission()?;
        if !self.discard_scratch(&held) || !self.flush_before_idle(&held.slot_id, held.record)? {
            return Ok(());
        }
        release(&self.pool, &held.lease_id)
            .map_err(|error| lease_error("could not release the slot", &error))?;
        let mut state = self.load()?;
        if let Some(index) = state
            .records
            .iter()
            .position(|record| record.id == held.record)
        {
            state.settle_idle(index);
            state.records[index].checkout = held.shared_from.clone();
            let origin = held.prior.unchanged();
            self.write_origin(held.record, state.records[index].last_used, origin)?;
        }
        self.save(&state)
    }

    /// Remove the command's owned child temporary directory (oxide#136)
    /// before the slot can go idle. The caller has already seen its own
    /// process group empty; a failure is reported and never fails an
    /// otherwise successful command, and the slot's own accounting and
    /// deletion are unchanged, so a kept leftover is never counted twice.
    /// `false` means the owned directory could not be removed (another
    /// incarnation's, a foreign entry, an unreadable tree): the slot is not
    /// marked idle then, exactly as for an unconfirmed group closure, and a
    /// later command recovers it once its closure and ownership rules pass.
    pub(super) fn discard_scratch(&self, held: &Lease) -> bool {
        let Some(scratch) = held.scratch.as_ref() else {
            return true;
        };
        match crate::child_temp::discard(scratch) {
            Ok(()) => true,
            Err(reason) => {
                eprintln!(
                    "oxide: {reason}; slot {} stays held, and a later oxide command recovers it once its closure and ownership rules pass",
                    held.slot_id
                );
                false
            }
        }
    }

    /// The gate cannot vouch for this checkout's own slot (no trusted record,
    /// or its history cannot be read): release it before Cargo and evict it
    /// whole, so the command builds cold instead of reusing it unchecked.
    pub fn discard(&self, mut held: Lease) -> Result<(), String> {
        let _admission = self.admission()?;
        // Barge refuses to evict a root that is still held. This command is
        // discarding its own generation, so its cooperative hold is dropped
        // before the eviction; any child that kept the inherited descriptor
        // still keeps the root held, and the eviction then refuses as it should.
        drop(held._root_hold.take());
        // Even an evicted removable slot is flushed first: a failed flush
        // retires it held instead of releasing and deleting it.
        if !self.flush_before_idle(&held.slot_id, held.record)? {
            return Ok(());
        }
        release(&self.pool, &held.lease_id)
            .map_err(|error| lease_error("could not release the slot", &error))?;
        evict_idle_slot(&self.pool, &held.slot_id)
            .map_err(|error| lease_error("cannot evict an untrusted slot", &error))?;
        let mut state = self.load()?;
        self.forget_record(&mut state, held.record);
        self.save(&state)
    }
}
