//! Settling what a dead or removable command left behind.
//!
//! The own-command recovery rule ([`settle`]) and the boot-session closure
//! facts live here, with the removable-volume flush, quarantine and lost-slot
//! deletion they feed. Nothing here deletes a slot whose writers are not
//! positively closed.

use super::control_io::{idle_slot_evicted_message, lease_error, probe_lock, Probe};
use super::*;
use crate::process;
use oxide_storage::lease::managed::{evict_idle_slot, flush_volume, recover};
use oxide_storage::lease::{LedgerSlot, LedgerView, SlotOccupancy};
use oxide_storage::volume::Binding;

/// What a command may do with a non-idle record whose lock it could take,
/// so no Oxide process holds it.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum Settle {
    /// Its slot is not held (never leased, or released before the record
    /// was updated): mark the record idle.
    Reset,
    /// Recover its held lease: Cargo never started, or its recorded process
    /// group is empty.
    Recover,
    /// Keep the slot held: Cargo may still run, or the held lease is not the
    /// one this record took.
    Keep,
}

/// How a held lease left by a dead command can return to the pool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Return {
    /// Cargo never started: the next command recovers it.
    NeverStarted,
    /// The next command recovers it once this process group is empty.
    GroupEmpty(i32),
    /// Cargo's group was never recorded: only an explicit recover.
    GroupUnrecorded,
    /// The held lease is not provably the record's own (for example a record
    /// from an earlier build without a holder): only an explicit recover.
    NotOwn,
}

/// The held lease must be the record's own: its lease ID, or before that was
/// saved, its unique lease holder string.
pub(super) fn return_path(record: &Record, slot: &LedgerSlot) -> Return {
    let own = match (&record.lease_id, &record.holder) {
        (Some(lease_id), _) => slot.lease_id.as_ref() == Some(lease_id),
        (None, Some(holder)) => slot.holder.as_ref() == Some(&holder.lease_holder),
        (None, None) => false,
    };
    match (own, record.phase, record.process_group) {
        (false, _, _) => Return::NotOwn,
        (true, Phase::Acquiring, _) => Return::NeverStarted,
        (true, Phase::Running, Some(group)) => Return::GroupEmpty(group),
        (true, Phase::Running | Phase::Spawning | Phase::Idle | Phase::Retired, _) => {
            Return::GroupUnrecorded
        }
    }
}

/// Why the writers of a record retired by a lost volume cannot be taken as
/// positively closed, from this process's view; `None` when they can. The
/// holder waited for its own group to be empty before releasing; a record of
/// another process, an exec tool (which may start processes outside its
/// group) or a rustc wrapper Cargo was allowed to run (a server may outlive
/// the group) is not closed. A record whose Cargo never started has no
/// writer.
pub(super) fn unclosed_writers(record: &Record) -> Option<String> {
    if record.phase == Phase::Acquiring {
        return None;
    }
    let holder = record.holder.as_ref()?;
    if holder.pid != std::process::id() {
        return Some(format!("its writers belong to oxide pid {}", holder.pid));
    }
    if let Some(command) = &holder.command {
        return Some(format!(
            "{command} may have started processes outside its group"
        ));
    }
    if !crate::wrapper::Policy::from_env().bypasses() {
        return Some("an inherited rustc wrapper was allowed and may outlive the build".to_owned());
    }
    let workspace_wrapper = [
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    workspace_wrapper
        .then(|| "a workspace rustc wrapper was set and may outlive the build".to_owned())
}

/// The safety margin a recorded holder start must precede the boot session
/// by. A restart ends every process, but the boot instant and a holder start
/// are wall-clock milliseconds from the same host, and macOS sets the wall
/// clock from the RTC or network during early boot. The kernel's boot time
/// (`kern.boottime`) then moves with later clock adjustments, so a holder
/// from the current session never falls behind it; 60 s absorbs the
/// early-boot clock set and millisecond truncation while leaving any start
/// within the last minute before boot unknown.
pub(super) const BOOT_MARGIN_MS: u64 = 60_000;

/// The boot session's start for a closure decision, or `None` when unknown.
/// Unknown is the safe answer: it never closes a slot.
pub(super) fn boot_session() -> Option<u64> {
    process::boot_ms()
}

/// Whether the record's holder started before the current boot session, so a
/// restart ended every process that could still write through its slot. An
/// unknown boot time, a missing or zero start and a start inside the safety
/// margin are not closure; elapsed time, PID absence and device
/// disappearance never are either.
pub(super) fn started_before_boot(record: &Record, boot_ms: Option<u64>) -> bool {
    let Some(boot) = boot_ms else {
        return false;
    };
    let Some(holder) = record
        .holder
        .as_ref()
        .filter(|holder| holder.started_ms > 0)
    else {
        return false;
    };
    holder
        .started_ms
        .checked_add(BOOT_MARGIN_MS)
        .is_some_and(|started| started < boot)
}

/// Whether a lost record's writers are positively closed: the holding command
/// saw its own group empty, or the current boot session began after its
/// holder started. [`unclosed_writers`] records the first reason at loss;
/// this is the one later fact that can close it.
pub(super) fn lost_writers_closed(record: &Record, boot_ms: Option<u64>) -> bool {
    match record.lost.as_ref() {
        Some(lost) if lost.unclosed.is_none() => true,
        Some(_) => started_before_boot(record, boot_ms),
        None => false,
    }
}

/// Whether a quarantined lost record is closed by the boot session alone: its
/// recorded reason still stands, but a restart ended its writers.
pub(super) fn closed_by_boot(record: &Record, boot_ms: Option<u64>) -> bool {
    record
        .lost
        .as_ref()
        .is_some_and(|lost| lost.unclosed.is_some())
        && started_before_boot(record, boot_ms)
}

/// The own-command recovery rule. `held` is the held slot on the record's
/// private project, if any. `group_empty` is `killpg(pgid, 0)` reporting
/// `ESRCH`; a surviving member or a reused group keeps the slot held.
pub(super) fn settle(
    record: &Record,
    held: Option<&LedgerSlot>,
    group_empty: impl Fn(i32) -> bool,
) -> Settle {
    let Some(slot) = held else {
        return Settle::Reset;
    };
    match return_path(record, slot) {
        Return::NeverStarted => Settle::Recover,
        Return::GroupEmpty(group) if group_empty(group) => Settle::Recover,
        Return::GroupEmpty(_) | Return::GroupUnrecorded | Return::NotOwn => Settle::Keep,
    }
}

impl Store {
    /// A removable-volume slot is flushed to the device before it can go
    /// idle; any other backend returns `true` at once. A failed or unknown
    /// flush retires the slot: the record becomes [`Phase::Retired`], the
    /// lease stays held, and `false` tells the caller not to release. This is
    /// a durability step after writer closure, not a closure proof. When the
    /// volume itself is gone the retirement also records [`Lost`], so the
    /// slot is deleted or quarantined once the volume returns. The caller
    /// holds the admission lock.
    pub(super) fn flush_before_idle(&self, slot_id: &str, record: u64) -> Result<bool, String> {
        if self.backend != Backend::ApfsVolume {
            return Ok(true);
        }
        let outcome = flush_volume(&self.pool);
        let mut state = self.load()?;
        let flushed = match &outcome {
            Ok(elapsed) => {
                state.log_flush(Flush {
                    slot_id: slot_id.to_owned(),
                    at_ms: process::now_ms(),
                    micros: Some(u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)),
                    error: None,
                    lost: false,
                });
                true
            }
            Err(error) => {
                let lost = !self.volume_present();
                state.log_flush(Flush {
                    slot_id: slot_id.to_owned(),
                    at_ms: process::now_ms(),
                    micros: None,
                    error: Some(error.to_string()),
                    lost,
                });
                if let Some(entry) = state.records.iter_mut().find(|entry| entry.id == record) {
                    if lost {
                        let unclosed = unclosed_writers(entry);
                        let found = Lost {
                            slot_id: slot_id.to_owned(),
                            at_ms: process::now_ms(),
                            unclosed,
                        };
                        eprintln!(
                            "oxide: removable volume gone; slot {slot_id} is {}",
                            found.describe(false)
                        );
                        entry.retired = Some(found.describe(false));
                        entry.lost = Some(found);
                    } else {
                        eprintln!(
                            "oxide: removable volume flush failed for slot {slot_id} ({error}); the slot is retired and stays held, never reused"
                        );
                        entry.process_group = None;
                        entry.retired = Some(format!("flush before release failed: {error}"));
                    }
                    entry.phase = Phase::Retired;
                }
                false
            }
        };
        self.save(&state)?;
        Ok(flushed)
    }

    /// The removal stop could not confirm the group closed: quarantine the
    /// slot as lost, held and never reused or deleted.
    pub fn quarantine_lost(&self, held: &Lease, why: &str) -> Result<(), String> {
        let _admission = self.admission()?;
        let mut state = self.load()?;
        if let Some(entry) = state
            .records
            .iter_mut()
            .find(|entry| entry.id == held.record)
        {
            let found = Lost {
                slot_id: held.slot_id.clone(),
                at_ms: process::now_ms(),
                unclosed: Some(why.to_owned()),
            };
            eprintln!(
                "oxide: removable volume gone; slot {} is {}",
                held.slot_id,
                found.describe(false)
            );
            entry.phase = Phase::Retired;
            entry.retired = Some(found.describe(false));
            entry.lost = Some(found);
        }
        self.save(&state)
    }

    /// Whether the removable pool's mount root still opens as a mount root.
    /// A failed flush with the mount gone is a surprise removal.
    pub(super) fn volume_present(&self) -> bool {
        self.payload
            .parent()
            .is_some_and(|mount| Binding::open(mount, None).is_ok())
    }

    /// Delete whole every slot lost with the removable volume whose writers
    /// are positively closed: no live Oxide holds its record, the recorded
    /// group (if Cargo started) is still empty and nothing known could
    /// outlive it. A quarantined record whose holder started before this boot
    /// session is closed too: a restart ended every process, and its recorded
    /// group number says nothing about the current session. Runs when the
    /// volume is selected again, under the admission lock. Anything else
    /// stays quarantined.
    pub(super) fn discard_lost(&self, state: &mut State) -> Result<(), String> {
        let boot = boot_session();
        let candidates: Vec<u64> = state
            .records
            .iter()
            .filter(|record| record.phase == Phase::Retired && lost_writers_closed(record, boot))
            .map(|record| record.id)
            .collect();
        if candidates.is_empty() {
            return Ok(());
        }
        let view = self.ledger()?;
        for id in candidates {
            let Some(record) = state.records.iter().find(|record| record.id == id) else {
                continue;
            };
            if !matches!(probe_lock(&self.lock_path(id), true)?, Probe::Free(_)) {
                continue;
            }
            // A group recorded in an earlier boot session cannot still be
            // this slot's writer: the number may belong to an unrelated
            // process now, so only a group of the current session is checked.
            let boot_closed = closed_by_boot(record, boot);
            if !boot_closed
                && record
                    .process_group
                    .is_some_and(|group| !process::group_empty(group))
            {
                continue;
            }
            let project = self.affinity_dir(id);
            let reported = if boot_closed {
                let elapsed = boot
                    .zip(record.holder.as_ref())
                    .map_or(0, |(boot, holder)| {
                        boot.saturating_sub(holder.started_ms) / 1_000
                    });
                format!(
                    "lost before this boot (holder started {elapsed} s before boot; a restart ends every process)"
                )
            } else {
                record.process_group.map_or_else(
                    || "held when the removable volume was lost (Cargo never started)".to_owned(),
                    |group| {
                        format!(
                            "held when the removable volume was lost (Cargo group {group} empty)"
                        )
                    },
                )
            };
            if let Some(slot) = view.slots.iter().find(|slot| slot.project == project) {
                if slot.state == SlotOccupancy::Held {
                    let lease_id = slot
                        .lease_id
                        .as_deref()
                        .ok_or("held slot has no lease ID")?;
                    recover(&self.pool, lease_id)
                        .map_err(|error| lease_error("cannot release a lost slot", &error))?;
                }
                evict_idle_slot(&self.pool, &slot.slot_id)
                    .map_err(|error| lease_error("cannot delete a lost slot", &error))?;
                eprintln!("{}", idle_slot_evicted_message(&slot.slot_id, &reported));
            }
            self.forget_record(state, id);
            self.save(state)?;
        }
        Ok(())
    }

    /// Own-command crash recovery: a non-idle record whose lock is free has
    /// no live Oxide owner. [`settle`] decides whether its held lease can be
    /// recovered; each recovery is logged for `oxide status`.
    pub(super) fn recover_settled(&self, state: &mut State) -> Result<(), String> {
        self.recover_settled_with(state, Self::flush_before_idle)
    }

    /// [`Store::recover_settled`] with the removable-volume flush supplied, so a
    /// test can make the flush reload a state that moved or dropped records.
    /// Candidates are named by record ID, never by position: the flush reloads
    /// `state`, so any index taken before it can be stale.
    pub(super) fn recover_settled_with(
        &self,
        state: &mut State,
        flush: impl Fn(&Store, &str, u64) -> Result<bool, String>,
    ) -> Result<(), String> {
        let mut view: Option<LedgerView> = None;
        let candidates: Vec<u64> = state
            .records
            .iter()
            .filter(|record| record.active())
            .map(|record| record.id)
            .collect();
        for id in candidates {
            let Some(index) = state.records.iter().position(|entry| entry.id == id) else {
                continue;
            };
            let record = &state.records[index];
            if !record.active() {
                continue;
            }
            let Probe::Free(_owner) = probe_lock(&self.lock_path(record.id), true)? else {
                continue;
            };
            if view.is_none() {
                view = Some(self.ledger()?);
            }
            let project = self.affinity_dir(record.id);
            let held = view.as_ref().and_then(|view| {
                view.slots
                    .iter()
                    .find(|slot| slot.project == project && slot.state == SlotOccupancy::Held)
            });
            match settle(record, held, process::group_empty) {
                Settle::Keep => continue,
                Settle::Reset => {
                    // A crash after the ledger recovery but before the log
                    // update left the event pending; the slot is no longer
                    // held, so the recovery took effect.
                    if let Some(recovery) = state.records[index].pending_recovery.take() {
                        state.log_recovery(recovery);
                    }
                }
                Settle::Recover => {
                    let slot = held.ok_or("a recovered record has no held slot")?;
                    let lease_id = slot.lease_id.clone().ok_or("held slot has no lease ID")?;
                    let holder = record
                        .holder
                        .as_ref()
                        .ok_or("a recovered record has no holder")?;
                    let recovery = Recovery {
                        slot_id: slot.slot_id.clone(),
                        lease_id,
                        checkout: record.checkout.clone(),
                        holder_pid: holder.pid,
                        holder_started_ms: holder.started_ms,
                        process_group: record.process_group,
                        recovered_ms: process::now_ms(),
                        recovered_by: std::process::id(),
                    };
                    // Write ahead: callers hold the admission lock, and the
                    // event is durable before the ledger changes.
                    state.records[index].pending_recovery = Some(recovery.clone());
                    self.save(state)?;
                    let flushed = flush(self, &recovery.slot_id, id)?;
                    // The flush saved its receipt (and any retirement);
                    // continue from that state so it is not overwritten.
                    *state = self.load()?;
                    // The reload is authoritative: it may have moved or dropped
                    // records (a failed flush retires one), so no index taken
                    // before it is trusted. This record is re-located by ID for
                    // both the flushed and the unflushed path (oxide#337, oxide#356).
                    if !flushed {
                        continue;
                    }
                    let Some(index) = state.records.iter().position(|entry| entry.id == id) else {
                        continue;
                    };
                    recover(&self.pool, &recovery.lease_id)
                        .map_err(|error| lease_error("cannot recover a settled slot", &error))?;
                    eprintln!(
                        "oxide: recovered slot {} after oxide pid {} died ({})",
                        recovery.slot_id,
                        recovery.holder_pid,
                        match recovery.process_group {
                            Some(group) => format!("Cargo group {group} empty"),
                            None => "Cargo never started".to_owned(),
                        }
                    );
                    state.log_recovery(recovery);
                    state.settle_idle(index);
                    self.save(state)?;
                    view = None;
                    continue;
                }
            }
            state.settle_idle(index);
        }
        Ok(())
    }

    /// Explicit release of an interrupted slot that automatic recovery keeps
    /// held: Cargo's group was never recorded, or the lease is not provably
    /// the record's. It replaces the retired standalone CLI's `recover`.
    /// The caller asserts nothing still writes in the slot; Oxide still
    /// refuses while any live command holds the record or the slot is
    /// retired, and logs the release like an automatic recovery.
    pub fn recover_interrupted(&self, slot_id: &str) -> Result<(), String> {
        let _admission = self.admission()?;
        let mut state = self.load()?;
        let view = self.ledger()?;
        let slot = view
            .slots
            .iter()
            .find(|slot| slot.slot_id == slot_id && slot.state == SlotOccupancy::Held)
            .ok_or_else(|| format!("slot {slot_id} is not held in {}", self.pool.display()))?;
        let lease_id = slot
            .lease_id
            .clone()
            .ok_or("the held slot has no lease ID")?;
        let index = state
            .records
            .iter()
            .position(|record| record.active() && self.affinity_dir(record.id) == slot.project)
            .ok_or_else(|| format!("slot {slot_id} has no interrupted oxide command record"))?;
        let record = &state.records[index];
        if record.retired.is_some() {
            return Err(format!(
                "slot {slot_id} is retired after a failed flush and stays held"
            ));
        }
        let Probe::Free(_owner) = probe_lock(&self.lock_path(record.id), true)? else {
            return Err(format!("a live oxide command holds slot {slot_id}"));
        };
        if matches!(
            return_path(record, slot),
            Return::NeverStarted | Return::GroupEmpty(_)
        ) {
            return Err(format!(
                "slot {slot_id} is recovered automatically by the next command once its Cargo group is empty"
            ));
        }
        let recovery = Recovery {
            slot_id: slot.slot_id.clone(),
            lease_id,
            checkout: record.checkout.clone(),
            holder_pid: record.holder.as_ref().map_or(0, |holder| holder.pid),
            holder_started_ms: record.holder.as_ref().map_or(0, |holder| holder.started_ms),
            process_group: record.process_group,
            recovered_ms: process::now_ms(),
            recovered_by: std::process::id(),
        };
        state.records[index].pending_recovery = Some(recovery.clone());
        self.save(&state)?;
        let record_id = state.records[index].id;
        if !self.flush_before_idle(&recovery.slot_id, record_id)? {
            return Err(format!(
                "slot {slot_id} stays held: the removable volume flush failed"
            ));
        }
        let mut state = self.load()?;
        // The pre-flush index is not an identity across the reload (the flush
        // and its receipt write may have moved or dropped records), so
        // re-locate this record by ID (oxide#337).
        let index = state
            .records
            .iter()
            .position(|record| record.id == record_id)
            .ok_or_else(|| format!("slot {slot_id}'s record vanished while it was recovered"))?;
        recover(&self.pool, &recovery.lease_id)
            .map_err(|error| lease_error("cannot recover the slot", &error))?;
        state.records[index].pending_recovery = None;
        state.log_recovery(recovery);
        state.settle_idle(index);
        self.save(&state)
    }
}
