//! Leasing a slot: the wait ticket, the attempt, the wait accounting and the
//! record and origin bookkeeping around them.
//!
//! One attempt takes the pool's admission lock, settles earlier leftovers,
//! picks the record and slot this command may use, and asks the library for
//! the lease. Everything a refusal needs to say is decided here; the caller
//! ([`Store::acquire`]) only polls, accounts the wait and ends its ticket.

use super::*;
use crate::cli::Profile;
use crate::freshness::{Built, Prior};
use crate::identity::{self, SlotKey};
use crate::process;
use crate::session_broker::{
    BorrowWait, JoinedSession, PermitController, PermitDenial, PermitGrant,
};
use oxide_storage::lease::managed::lease_measured;
use oxide_storage::lease::{
    LeaseError, LeaseRequest, LedgerSlot, LedgerView, Profile as LeaseProfile, ReuseMeasurement,
    SlotLayout, SlotOccupancy,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use super::control_io::{
    bounded_read, ensure_private_dir, flock_nonblocking, lease_error, open_admission_file,
    probe_lock, Probe,
};
use super::status::{Observation, ObservationSource};

/// Slots one project identity may keep; a new checkout or a command with no
/// idle compatible slot replaces the least-recently-used eligible idle whole
/// slot of that project (oxide#118, raised to twelve by oxide#177).
pub(crate) const PROJECT_SLOT_CAP: usize = 12;

/// Largest origin document read; the origin is a small JSON record.
pub(super) const MAX_ORIGIN_BYTES: u64 = 8 * 1024 * 1024;

pub(super) const ORIGIN_VERSION: u32 = 1;

pub(super) const WAIT_POLL: Duration = Duration::from_millis(200);

/// The compile origin a record's slot was left with. `mark` is the record's
/// use counter when the origin was written; leasing re-stamps it and Cargo's
/// start advances the counter, so a mismatch means Cargo may have run
/// unrecorded. `origin: None` is an
/// untrusted slot. An untrusted slot is never reused: the next command evicts
/// it and builds cold.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OriginFile {
    version: u32,
    mark: u64,
    origin: Option<Built>,
}

/// The exclusive admission reason returned by one attempt.
#[derive(Debug)]
pub(super) enum Wait {
    Queue(usize),
    HostLimit(usize),
    Checkout,
    Clean,
    /// Test-only: a project cap is a replacement or a refusal, never a wait
    /// that production admission reports (see [`ProjectCap`]).
    #[cfg(test)]
    ProjectCap,
    Uncertain,
    BargeHeld(String),
    Capacity(Option<String>),
}

impl Wait {
    pub(super) fn category(&self) -> WaitCategory {
        match self {
            Self::Queue(_) => WaitCategory::Queue,
            Self::HostLimit(_) => WaitCategory::HostLimit,
            Self::Checkout => WaitCategory::Checkout,
            Self::Clean => WaitCategory::Clean,
            #[cfg(test)]
            Self::ProjectCap => WaitCategory::ProjectCap,
            Self::Uncertain => WaitCategory::Uncertain,
            Self::BargeHeld(_) => WaitCategory::BargeHeld,
            Self::Capacity(_) => WaitCategory::Capacity,
        }
    }

    pub(super) fn describe(&self, max_active: usize) -> String {
        match self {
            Self::Queue(position) => {
                format!("earlier unblocked oxide commands (position {position})")
            }
            Self::HostLimit(active) => {
                format!("a build slot ({active} of {max_active} host-wide in use)")
            }
            Self::Checkout => "another oxide command in this checkout".to_owned(),
            Self::Clean => "oxide clean in this checkout".to_owned(),
            #[cfg(test)]
            Self::ProjectCap => {
                format!("an idle slot of this project (cap {PROJECT_SLOT_CAP} slots)")
            }
            Self::Uncertain => {
                "a slot held by an interrupted command whose Cargo may still run".to_owned()
            }
            Self::BargeHeld(slot_id) => {
                format!("slot {slot_id}'s Barge hold (still held by another process)")
            }
            Self::Capacity(None) => "pool capacity held by running builds".to_owned(),
            Self::Capacity(Some(diagnosis)) => format!("pool capacity ({diagnosis})"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WaitCategory {
    Queue,
    HostLimit,
    Checkout,
    Clean,
    ProjectCap,
    Uncertain,
    BargeHeld,
    Capacity,
}

impl WaitCategory {
    pub(super) const ALL: [Self; 8] = [
        Self::Queue,
        Self::HostLimit,
        Self::Checkout,
        Self::Clean,
        Self::ProjectCap,
        Self::Uncertain,
        Self::BargeHeld,
        Self::Capacity,
    ];

    pub(super) fn index(self) -> usize {
        match self {
            Self::Queue => 0,
            Self::HostLimit => 1,
            Self::Checkout => 2,
            Self::Clean => 3,
            Self::ProjectCap => 4,
            Self::Uncertain => 5,
            Self::BargeHeld => 6,
            Self::Capacity => 7,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::HostLimit => "host limit",
            Self::Checkout => "checkout",
            Self::Clean => "clean",
            Self::ProjectCap => "project cap",
            Self::Uncertain => "uncertain slot",
            Self::BargeHeld => "barge hold",
            Self::Capacity => "capacity",
        }
    }
}

/// Measured monotonic waiting observed by one Oxide command.
#[derive(Debug)]
pub(crate) struct WaitAccounting {
    totals: [Duration; WaitCategory::ALL.len()],
    total: Duration,
    active: Option<(WaitCategory, Instant)>,
    observed: bool,
    observed_categories: [bool; WaitCategory::ALL.len()],
    duration_available: bool,
    last_announced: Option<WaitCategory>,
    pub(super) reason_lines: usize,
    pub(super) suppression_announced: bool,
    pub(super) ceiling_announced: bool,
}

impl Default for WaitAccounting {
    fn default() -> Self {
        Self {
            totals: [Duration::ZERO; WaitCategory::ALL.len()],
            total: Duration::ZERO,
            active: None,
            observed: false,
            observed_categories: [false; WaitCategory::ALL.len()],
            duration_available: true,
            last_announced: None,
            reason_lines: 0,
            suppression_announced: false,
            ceiling_announced: false,
        }
    }
}

impl WaitAccounting {
    pub(super) fn observe(&mut self, wait: &Wait, max_active: usize, now: Instant) {
        let reason = wait.describe(max_active);
        self.observe_with_reason(wait, now, &reason);
    }

    pub(super) fn observe_with_reason(&mut self, wait: &Wait, now: Instant, reason: &str) {
        let category = wait.category();
        self.observed = true;
        self.account_active_until(now);
        self.observed_categories[category.index()] = true;
        self.active = Some((category, now));

        // The first ceiling diagnosis matters even after a generic capacity
        // wait or after the ordinary wait notices reached their limit.
        if matches!(wait, Wait::Capacity(Some(_))) && !self.ceiling_announced {
            self.ceiling_announced = true;
            self.last_announced = Some(category);
            eprintln!("oxide: waiting for {reason}; Ctrl-C cancels");
            return;
        }
        if self.last_announced == Some(category) || self.suppression_announced {
            return;
        }
        if self.reason_lines < 8 {
            eprintln!("oxide: waiting for {}; Ctrl-C cancels", reason);
            self.reason_lines += 1;
            self.last_announced = Some(category);
        } else {
            eprintln!("oxide: waiting reasons changed; (further changes not shown)");
            self.suppression_announced = true;
        }
    }

    /// End a measured interval at a grant, refusal, cancellation or error.
    pub(crate) fn stop(&mut self, now: Instant) {
        self.account_active_until(now);
    }

    pub(super) fn account_active_until(&mut self, now: Instant) {
        let Some((category, started)) = self.active.take() else {
            return;
        };
        let Some(elapsed) = now.checked_duration_since(started) else {
            self.duration_available = false;
            return;
        };
        if !self.duration_available {
            return;
        }
        let Some(category_total) = self.totals[category.index()].checked_add(elapsed) else {
            self.duration_available = false;
            return;
        };
        let Some(total) = self.total.checked_add(elapsed) else {
            self.duration_available = false;
            return;
        };
        self.totals[category.index()] = category_total;
        self.total = total;
    }

    pub(super) fn completion_line(&self, ending: &str) -> Option<String> {
        if !self.observed {
            return None;
        }
        if !self.duration_available {
            return Some(format!("oxide: wait duration unavailable; {ending}"));
        }
        let Some(total) = format_duration(self.total) else {
            return Some(format!("oxide: wait duration unavailable; {ending}"));
        };
        let Some(categories) = WaitCategory::ALL
            .into_iter()
            .filter(|category| self.observed_categories[category.index()])
            .map(|category| {
                format_duration(self.totals[category.index()])
                    .map(|duration| format!("{} {duration}", category.label()))
            })
            .collect::<Option<Vec<_>>>()
        else {
            return Some(format!("oxide: wait duration unavailable; {ending}"));
        };
        let categories = categories.join(", ");
        Some(format!("oxide: waited {total} ({categories}); {ending}"))
    }

    pub(crate) fn print_completion(&self, ending: &str) {
        if let Some(line) = self.completion_line(ending) {
            eprintln!("{line}");
        }
    }

    /// Time behind other tickets or the active-build limit, excluding time
    /// spent retrying admission and taking the pool locks.
    pub(crate) fn admission_queue_wait(&self) -> Option<Duration> {
        self.duration_available.then_some(())?;
        self.totals[WaitCategory::Queue.index()]
            .checked_add(self.totals[WaitCategory::HostLimit.index()])
    }

    pub(crate) fn summary_suffix(&self) -> Option<String> {
        if !self.observed {
            return None;
        }
        if !self.duration_available {
            return Some(", wait duration unavailable".to_owned());
        }
        Some(format_duration(self.total).map_or_else(
            || ", wait duration unavailable".to_owned(),
            |duration| format!(", waited {duration}"),
        ))
    }
}

pub(super) fn format_duration(duration: Duration) -> Option<String> {
    let tenths = duration.as_nanos().checked_add(50_000_000)? / 100_000_000;
    Some(format!("{}.{:01}s", tenths / 10, tenths % 10))
}

/// One held slot for the life of a command. The record lock is the liveness
/// proof other commands check before recovering.
pub struct Lease {
    pub slot_id: String,
    pub reused: bool,
    pub build_dir: PathBuf,
    /// The pool's recorded store identity and this slot's incarnation: the
    /// durable binding of anything the command owns inside the slot.
    pub store_id: String,
    pub incarnation: String,
    /// The command's owned child temporary directory (oxide#136), removed at
    /// release after positive writer closure.
    pub scratch: Option<crate::child_temp::Owned>,
    /// The slot's history for the freshness gate.
    pub prior: Prior,
    /// Checkout whose idle slot this command took; restored if the gate
    /// refuses.
    pub shared_from: Option<PathBuf>,
    /// The library's reuse walk of this slot generation when it was leased;
    /// `None` for a new slot or a held replay.
    pub measured: Option<ReuseMeasurement>,
    pub(super) lease_id: String,
    pub(super) record: u64,
    _lock: File,
    // Keep the inherited permit marker locked while this process owns the
    // slot. A crash closes it, making any unsettled record count again.
    _inherited_permit: Option<File>,
    yielded_permit: Option<File>,
    /// The cooperative hold on the slot's Barge root, kept for as long as the
    /// command uses it. A child that kept the inherited descriptor keeps the
    /// root held after this lease is dropped.
    pub(super) _root_hold: Option<barge::Hold>,
}

impl Lease {
    /// This command's own slot record, for the checkout barrier that excludes
    /// every other command in the same checkout.
    pub fn record_id(&self) -> u64 {
        self.record
    }

    /// The library's lease identity, recorded in its ledger for this grant.
    pub fn lease_id(&self) -> &str {
        &self.lease_id
    }

    /// Whether this lease currently carries its exclusive Barge root hold.
    pub(crate) fn has_exclusive_hold(&self) -> bool {
        self._root_hold.is_some()
    }

    #[cfg(test)]
    pub(crate) fn drop_hold_for_tests(&mut self) {
        self._root_hold.take();
    }
}

pub enum Acquired {
    Lease(Lease),
    Interrupted(i32),
    /// The admission window closed while this command was waiting (oxide#140).
    AdmissionExpired,
}

/// What one attempt selected from the pool before the library is asked for
/// the lease. Everything here is a preference: the library rechecks the
/// record's identity, ownership, occupancy and accounting under the pool
/// lock.
pub(super) struct SlotChoice {
    /// The record this command continues on, when the state already holds one.
    existing: Option<usize>,
    /// The record lock taken for that record.
    lock: Option<File>,
    /// A record of this checkout exists but is busy.
    busy: bool,
    /// A ledger-idle slot of this checkout was skipped because its Barge hold
    /// is still held by another process.
    barge_held: Option<String>,
    /// The checkout an adopted shared slot came from, restored on refusal.
    shared_from: Option<PathBuf>,
    /// The adopted shared slot's trusted origin.
    shared_origin: Option<Built>,
    /// The idle project-cap victim the library may replace.
    replace: Option<(u64, String)>,
    /// The reason this attempt must wait instead of leasing.
    wait: Option<Wait>,
}

pub(super) struct OwnSlotProbe {
    existing: Option<usize>,
    lock: Option<File>,
    busy: bool,
    barge_held_slot: Option<String>,
}

/// Lazy cache of Barge-held slot IDs for one `choose_slot` pass, fetched at
/// most once and only when an idle owned slot's Barge hold must be checked.
pub(super) struct BargeHolds<'a> {
    pool: &'a Path,
    held: Option<BTreeSet<String>>,
}

impl<'a> BargeHolds<'a> {
    fn new(pool: &'a Path) -> Self {
        Self { pool, held: None }
    }

    fn is_held(&mut self, slot_id: &str) -> Result<bool, String> {
        if self.held.is_none() {
            let held = oxide_storage::lease::managed::barge_held_slots(self.pool)
                .map_err(|error| lease_error("cannot inspect pool slot holds", &error))?;
            self.held = Some(held);
        }
        Ok(self.held.as_ref().is_some_and(|set| set.contains(slot_id)))
    }
}

/// What the project cap allows a record that owns no slot of this project.
pub(super) enum ProjectCap {
    /// The project holds fewer slots than its cap: no replacement is needed.
    Room,
    /// The library may replace this idle whole slot (record ID, slot ID).
    Replace(u64, String),
    /// Nothing may be replaced safely now: wait for this reason.
    Wait(Wait),
}

/// The record an attempt claimed, its lock, the trusted origin read before the
/// use counter advanced and the lease holder string both the record and the
/// library request carry.
pub(super) struct ClaimedRecord {
    record: u64,
    lock: File,
    recorded: Option<Built>,
    lease_holder: String,
}

pub(super) fn permit_path(control: &std::path::Path, record: u64) -> PathBuf {
    control
        .join("locks")
        .join(format!("{record}.inherited-permit"))
}

pub(super) fn exec_permit_path(control: &Path, record: u64, marker: &str) -> PathBuf {
    control
        .join("locks")
        .join(format!("{record}.exec-{marker}"))
}

#[derive(Clone, Copy)]
pub(crate) enum ExecPermit {
    Yielded,
    Borrowed,
}

pub(super) fn exec_permit(control: &Path, record: u64) -> Result<Option<ExecPermit>, String> {
    if !matches!(
        probe_lock(&exec_permit_path(control, record, "yielded"), false)?,
        Probe::Held
    ) {
        return Ok(None);
    }
    let borrowed = matches!(
        probe_lock(&exec_permit_path(control, record, "borrowed"), false)?,
        Probe::Held
    );
    Ok(Some(if borrowed {
        ExecPermit::Borrowed
    } else {
        ExecPermit::Yielded
    }))
}

pub(super) fn host_permit_count(
    control: &std::path::Path,
    records: &[Record],
) -> Result<usize, String> {
    let mut count = 0;
    for record in records.iter().filter(|record| record.active()) {
        match exec_permit(control, record.id)? {
            Some(ExecPermit::Yielded) => {}
            Some(ExecPermit::Borrowed) => count += 1,
            None => {
                if !matches!(
                    probe_lock(&permit_path(control, record.id), false)?,
                    Probe::Held
                ) {
                    count += 1;
                }
            }
        }
    }
    Ok(count)
}

/// Counts once in host admission while any nested build has an open grant.
/// The weak reference never extends the lifetime of a grant.
pub(crate) struct ExecPermits {
    store: Store,
    record: u64,
    active: Mutex<Weak<File>>,
}

struct BorrowTicket<'a> {
    store: &'a Store,
    ticket: Option<(u64, File)>,
}

impl Drop for BorrowTicket<'_> {
    fn drop(&mut self) {
        if self.ticket.take().is_some() {
            // Dropping the lock cancels immediately. Reap now if admission
            // is free, or let the next admission do it under that same lock.
            let _ = self.store.reap_borrow_tickets();
        }
    }
}

impl PermitController for ExecPermits {
    fn borrow(&self, _: u64, wait: &BorrowWait<'_>) -> Result<PermitGrant, PermitDenial> {
        let mut ticket = BorrowTicket {
            store: &self.store,
            ticket: None,
        };
        loop {
            wait.check()?;
            match self.try_grant(&mut ticket, wait) {
                Ok(Some(permit)) => return Ok(Box::new(permit)),
                Ok(None) => wait.wait()?,
                Err(error) => {
                    eprintln!("oxide: cannot admit an exec build permit: {error}");
                    return Err(PermitDenial::Unavailable);
                }
            }
        }
    }
}

impl ExecPermits {
    fn try_grant(
        &self,
        ticket: &mut BorrowTicket<'_>,
        wait: &BorrowWait<'_>,
    ) -> Result<Option<Arc<File>>, String> {
        // A live grant can share immediately, even during another admission.
        if let Some(active) = self.active_permit()? {
            return Ok(Some(active));
        }
        let Some(_locks) = self.store.try_permit_admission()? else {
            return Ok(None);
        };
        // No other thread can create a new permit while admission is held.
        if let Some(active) = self.active_permit()? {
            return Ok(Some(active));
        }

        let mut state = self.store.load()?;
        let waits_before = state.tickets.len() + state.cleans.len();
        self.store.drop_dead_tickets(&mut state)?;
        let reaped = state.tickets.len() + state.cleans.len() != waits_before;
        let enqueued = ticket.ticket.is_none();
        if enqueued {
            self.store.check_waiter_pool()?;
            ticket.ticket = Some(self.store.enqueue_waiter(&mut state, false)?);
        }
        if enqueued || reaped {
            self.store.save(&state)?;
        }
        let (id, _) = ticket
            .ticket
            .as_ref()
            .ok_or("missing exec build permit ticket")?;
        if self.store.queue_position(&state, *id)?.is_some() {
            return Ok(None);
        }
        // A build that died while we waited may need normal slot recovery
        // before it stops counting. Unknown writers still count.
        if self
            .store
            .host_limit(&state, self.store.max_active)?
            .is_some()
        {
            let active_before = state
                .records
                .iter()
                .filter(|record| record.active())
                .count();
            oxide_storage::lease::set_admission_deadline(wait.deadline());
            let recovered = self.store.recover_settled(&mut state);
            oxide_storage::lease::set_admission_deadline(None);
            recovered?;
            if state
                .records
                .iter()
                .filter(|record| record.active())
                .count()
                != active_before
            {
                self.store.save(&state)?;
            }
            if self
                .store
                .host_limit(&state, self.store.max_active)?
                .is_some()
            {
                return Ok(None);
            }
        }
        if wait.check().is_err() {
            return Ok(None);
        }
        let Probe::Free(marker) = probe_lock(
            &exec_permit_path(&self.store.control, self.record, "borrowed"),
            true,
        )?
        else {
            return Err("the exec build permit marker is unexpectedly held".to_owned());
        };
        let permit = Arc::new(marker);
        self.store.remove_waiter(&mut state, *id)?;
        ticket.ticket.take();
        *self
            .active
            .lock()
            .map_err(|_| "exec build permit state is poisoned")? = Arc::downgrade(&permit);
        Ok(Some(permit))
    }

    fn active_permit(&self) -> Result<Option<Arc<File>>, String> {
        Ok(self
            .active
            .lock()
            .map_err(|_| "exec build permit state is poisoned")?
            .upgrade())
    }
}

impl Store {
    /// The exec tool has started and its group is recorded. Its slot and
    /// writer custody stay held while the separate host permit is yielded.
    pub(crate) fn yield_exec_permit(&self, lease: &mut Lease) -> Result<(), String> {
        let Probe::Free(marker) = probe_lock(
            &exec_permit_path(&self.control, lease.record, "yielded"),
            true,
        )?
        else {
            return Err("the exec yield marker is unexpectedly held".to_owned());
        };
        lease.yielded_permit = Some(marker);
        Ok(())
    }

    pub(crate) fn exec_permits(&self, lease: &Lease) -> ExecPermits {
        ExecPermits {
            store: self.clone(),
            record: lease.record,
            active: Mutex::new(Weak::new()),
        }
    }

    /// Use the slot admission gates in the same order. A pending borrower
    /// must be free to notice disconnects even while another writer holds them.
    fn try_permit_admission(&self) -> Result<Option<(Option<File>, File)>, String> {
        fn try_lock(control: &Path) -> Result<Option<File>, String> {
            let file = open_admission_file(control)?
                .ok_or_else(|| format!("{} does not exist", control.display()))?;
            Ok(flock_nonblocking(&file)?.then_some(file))
        }

        let internal = if let Some(sibling) = self.sibling.as_ref().filter(|s| s.barrier) {
            let Some(internal) = try_lock(&sibling.control)? else {
                return Ok(None);
            };
            Some(internal)
        } else {
            None
        };
        let Some(admission) = try_lock(&self.control)? else {
            return Ok(None);
        };
        self.fence_admission_state()?;
        Ok(Some((internal, admission)))
    }

    fn reap_borrow_tickets(&self) -> Result<(), String> {
        if let Some(_locks) = self.try_permit_admission()? {
            let mut state = self.load()?;
            self.drop_dead_tickets(&mut state)?;
            self.save(&state)?;
        }
        Ok(())
    }

    /// Lease this checkout's slot, waiting in FIFO order. Ctrl-C while waiting
    /// cancels without touching any slot.
    /// `share` allows taking another checkout's idle compatible slot.
    /// `label` names the holder in `oxide status` (`exec <program>`).
    pub(crate) fn acquire(
        &self,
        key: &SlotKey,
        profile: Profile,
        share: bool,
        label: Option<&str>,
        waits: &mut WaitAccounting,
        joined: Option<&JoinedSession>,
    ) -> Result<Acquired, String> {
        let max_active = self.max_active;
        let independent_limit = joined.is_none().then_some(max_active);
        let ticket = self.register_waiter(joined.is_some())?;
        loop {
            // End the polling interval before doing any admission work. A
            // slow attempt that finally grants must count as work, even
            // when the preceding attempt found the queue or host limit full.
            waits.stop(Instant::now());
            if crate::budget::admission_expired() {
                self.remove_ticket(ticket.0)?;
                return Ok(Acquired::AdmissionExpired);
            }
            if let Some(signal) = process::received_signal() {
                self.remove_ticket(ticket.0)?;
                return Ok(Acquired::Interrupted(signal));
            }
            let attempt = joined
                .map_or(Ok(()), JoinedSession::check_live)
                .and_then(|()| {
                    self.attempt(ticket.0, key, profile, independent_limit, share, label)
                });
            match attempt {
                // A failed attempt still ends this command's wait: the ticket
                // must not outlive it, or every later command queues behind a
                // dead ticket (oxide#337).
                Err(error) => {
                    self.remove_ticket(ticket.0)?;
                    return Err(error);
                }
                Ok(Ok(lease)) => {
                    self.remove_ticket(ticket.0)?;
                    if crate::budget::admission_expired() {
                        // The window closed while this attempt ran: the lease
                        // goes straight back and no child starts.
                        let origin = lease.prior.unchanged();
                        self.release(lease, origin)?;
                        return Ok(Acquired::AdmissionExpired);
                    }
                    return Ok(Acquired::Lease(lease));
                }
                Ok(Err(wait)) => {
                    waits.observe(&wait, max_active, Instant::now());
                    std::thread::sleep(crate::budget::clamp(WAIT_POLL));
                }
            }
        }
    }

    /// Record that Cargo is about to start. A crash from here until
    /// [`Store::mark_running`] leaves the slot held for explicit recovery:
    /// Cargo's group may exist but is not recorded. Advancing the use counter
    /// untrusts the slot's origin until release records a new one; the gate
    /// has already refreshed this checkout's differing files, so a retry here
    /// is safe under Cargo's own checks.
    pub fn mark_spawning(&self, lease: &Lease) -> Result<(), String> {
        let _admission = self.admission()?;
        let mut state = self.load()?;
        let index = state
            .records
            .iter()
            .position(|record| record.id == lease.record)
            .ok_or("Oxide slot record disappeared")?;
        state.records[index].phase = Phase::Spawning;
        state.records[index].process_group = None;
        state.touch(index)?;
        self.save(&state)
    }

    pub fn mark_running(&self, lease: &Lease, group: i32) -> Result<(), String> {
        self.update_record(lease.record, |record| {
            record.phase = Phase::Running;
            record.process_group = Some(group);
        })
    }

    /// One admission attempt inside this command's wait ticket: report the
    /// reason it must wait, or take the slot and return its lease.
    pub(super) fn attempt(
        &self,
        ticket: u64,
        key: &SlotKey,
        profile: Profile,
        independent_limit: Option<usize>,
        share: bool,
        label: Option<&str>,
    ) -> Result<Result<Lease, Wait>, String> {
        // Internal admission first (removable store only), then this pool's.
        let (_barrier, cleaning) = self.sibling_barrier(&key.checkout)?;
        let _admission = self.admission()?;
        let mut state = self.load()?;
        self.drop_dead_tickets(&mut state)?;
        if let Some(wait) =
            self.waiting_reason(&mut state, ticket, key, independent_limit, cleaning)?
        {
            self.save(&state)?;
            return Ok(Err(wait));
        }
        let view = self.ledger()?;
        let mut choice = self.choose_slot(&mut state, key, share, &view)?;
        if let Some(wait) = choice.wait.take() {
            self.set_ticket_blocked(ticket, true)?;
            self.save(&state)?;
            return Ok(Err(wait));
        }
        let claimed = self.claim_record(&mut state, key, profile, ticket, label, &mut choice)?;
        let inherited_permit = if independent_limit.is_none() {
            let Probe::Free(marker) =
                probe_lock(&permit_path(&self.control, claimed.record), true)?
            else {
                return Err("the record's inherited permit marker is unexpectedly held".to_owned());
            };
            Some(marker)
        } else {
            None
        };
        self.save(&state)?;
        let folders = self.slot_folders(&state, key, &view);
        let request =
            self.lease_request(key, profile, &claimed, folders, choice.replace.as_ref())?;
        let outcome = lease_measured(request);
        // The library deletes the requested victim only after complete
        // accounting and rank headroom, and it may still refuse afterwards.
        // Drop that record's metadata only when the ledger no longer holds
        // the slot; an untouched victim keeps its record.
        if let Some((victim_record, victim_slot)) = choice.replace.clone() {
            let view = self.ledger()?;
            if !view.slots.iter().any(|slot| slot.slot_id == victim_slot) {
                eprintln!(
                    "oxide: evicted idle slot {victim_slot} (project slot cap {PROJECT_SLOT_CAP})"
                );
                self.forget_record(&mut state, victim_record);
            }
        }
        let index = state
            .records
            .iter()
            .position(|entry| entry.id == claimed.record)
            .ok_or("Oxide slot record disappeared")?;
        let (grant, measured, unmeasured, root_hold) = match outcome {
            Ok(leased) => {
                if let Some(note) = &leased.admission_note {
                    eprintln!("oxide: warning: {note}");
                }
                let root_hold = leased.hold;
                // The child launch must keep the slot's cooperative hold: the
                // descriptor is made inheritable before Cargo is spawned, so a
                // child that outlives this process still holds the root.
                if let Some(hold) = root_hold.as_ref() {
                    hold.set_inheritable(true).map_err(|error| {
                        format!("cannot make the slot hold inheritable: {error}")
                    })?;
                }
                (leased.grant, leased.measured, leased.unmeasured, root_hold)
            }
            Err(error) => return self.record_refusal(&mut state, index, ticket, &choice, error),
        };
        state.records[index].lease_id = Some(grant.lease_id.clone());
        self.save(&state)?;
        // The library's reuse walk is the only complete measurement an
        // acquiring command already made. Record it as a diagnostic bound to
        // exactly this slot generation; a failure never changes admission,
        // ownership or exit status, and no read path ever writes it.
        if let Some(measured) = measured.as_ref() {
            let _ = self.record_observation(Observation {
                store_id: grant.store_id.clone(),
                slot_id: measured.slot_id.clone(),
                incarnation: measured.incarnation.clone(),
                dir: measured.dir.clone(),
                measured_at_ms: process::now_ms(),
                allocated_bytes: measured.unique_allocated_bytes,
                source: ObservationSource::LeaseReuse,
                lease_id: Some(grant.lease_id.clone()),
            });
        }
        // A slot this admission could not measure completely is kept and
        // charged conservatively; record that incomplete outcome so default
        // status says `not measured (walk incomplete)` for it (oxide#186).
        self.record_incomplete_walks(Some(&grant.store_id), &unmeasured);
        let prior = match (grant.reused_slot, claimed.recorded) {
            (false, _) => Prior::Fresh,
            (true, Some(origin)) => Prior::Known(origin),
            (true, None) => Prior::Unknown,
        };
        Ok(Ok(Lease {
            slot_id: grant.slot_id,
            reused: grant.reused_slot,
            build_dir: grant.env.cargo_build_build_dir,
            store_id: grant.store_id,
            incarnation: grant.incarnation,
            scratch: None,
            prior,
            shared_from: choice.shared_from.take(),
            measured,
            lease_id: grant.lease_id,
            record: claimed.record,
            _lock: claimed.lock,
            _inherited_permit: inherited_permit,
            yielded_permit: None,
            _root_hold: root_hold,
        }))
    }

    /// The reason this attempt must wait, after dead tickets are dropped and
    /// earlier leftovers settled. `Ok(None)` continues with the selection.
    ///
    /// Strict FIFO ordering is preserved among unblocked waiters competing for
    /// `HostLimit`, while tickets blocked only on a per-checkout or per-slot
    /// condition do not head-of-line block unrelated checkouts when
    /// `active < max_active`.
    pub(super) fn waiting_reason(
        &self,
        state: &mut State,
        ticket: u64,
        key: &SlotKey,
        independent_limit: Option<usize>,
        cleaning: bool,
    ) -> Result<Option<Wait>, String> {
        if independent_limit.is_some() {
            if let Some(wait) = self.queue_position(state, ticket)? {
                return Ok(Some(wait));
            }
        } else if !state.tickets.iter().any(|entry| entry.id == ticket) {
            return Err("Oxide wait ticket was lost".to_owned());
        }
        if cleaning
            || state
                .cleans
                .iter()
                .any(|clean| clean.checkout == key.checkout)
        {
            self.set_ticket_blocked(ticket, true)?;
            return Ok(Some(Wait::Clean));
        }
        self.recover_settled(state)?;
        if self.backend == Backend::ApfsVolume {
            self.discard_lost(state)?;
        }
        if let Some(limit) = independent_limit {
            if let Some(wait) = self.host_limit(state, limit)? {
                return Ok(Some(wait));
            }
            self.set_ticket_blocked(ticket, false)?;
        }
        Ok(None)
    }

    fn queue_position(&self, state: &State, ticket: u64) -> Result<Option<Wait>, String> {
        let mut unblocked_ahead = 0;
        for entry in &state.tickets {
            if entry.id == ticket {
                return Ok((unblocked_ahead > 0).then_some(Wait::Queue(unblocked_ahead + 1)));
            }
            if !self.is_ticket_blocked(entry.id)? {
                unblocked_ahead += 1;
            }
        }
        Err("Oxide wait ticket was lost".to_owned())
    }

    fn host_limit(&self, state: &State, limit: usize) -> Result<Option<Wait>, String> {
        let active = host_permit_count(&self.control, &state.records)? + self.sibling_active()?;
        Ok((active >= limit).then_some(Wait::HostLimit(active)))
    }

    /// Choose the record and slot this attempt uses: this checkout's own idle
    /// record, another checkout's shareable idle slot, or a new record. A
    /// project at its slot cap may offer one idle whole slot to replace.
    pub(super) fn choose_slot(
        &self,
        state: &mut State,
        key: &SlotKey,
        share: bool,
        view: &LedgerView,
    ) -> Result<SlotChoice, String> {
        self.forget_stale_records(state, key, view);
        let mut barge_holds = BargeHolds::new(&self.pool);
        let own = self.find_own_slot(state, key, view, &mut barge_holds)?;
        let mut existing = own.existing;
        let mut lock = own.lock;
        let busy = own.busy;
        let barge_held = own.barge_held_slot;
        let mut shared_from = None;
        let mut shared_origin = None;
        // A project-cap replacement the library performs as part of the
        // admission below; choosing it never deletes anything itself.
        let mut replace: Option<(u64, String)> = None;
        let shared = if share
            && !existing.is_some_and(|index| self.record_has_slot(&state.records[index], view))
        {
            self.shared_candidate(state, key, view, &mut barge_holds)?
        } else {
            None
        };
        if let Some((id, file, origin)) = shared {
            // This checkout's slotless record is replaced by the taken one.
            if let Some(own) = existing.map(|index| state.records[index].id) {
                self.forget_record(state, own);
            }
            let index = state
                .records
                .iter()
                .position(|record| record.id == id)
                .ok_or("the shared slot record disappeared")?;
            shared_from = state.records[index].checkout.replace(key.checkout.clone());
            shared_origin = Some(origin);
            lock = Some(file);
            existing = Some(index);
        } else if !existing.is_some_and(|index| self.record_has_slot(&state.records[index], view)) {
            match self.project_cap_replacement(
                state,
                key,
                view,
                existing,
                busy,
                barge_held.as_deref(),
                &mut barge_holds,
            ) {
                Ok(ProjectCap::Room) => {}
                Ok(ProjectCap::Replace(id, slot_id)) => replace = Some((id, slot_id)),
                Ok(ProjectCap::Wait(wait)) => {
                    return Ok(SlotChoice {
                        existing,
                        lock,
                        busy,
                        barge_held,
                        shared_from,
                        shared_origin,
                        replace,
                        wait: Some(wait),
                    });
                }
                // The refusal names what a safety gate forbids; the state
                // was already cleaned of stale records above, so keep it.
                Err(error) => {
                    self.save(state)?;
                    return Err(error);
                }
            }
        }
        Ok(SlotChoice {
            existing,
            lock,
            busy,
            barge_held,
            shared_from,
            shared_origin,
            replace,
            wait: None,
        })
    }

    /// The ledger slot owned by this record's private project, if any.
    pub(super) fn record_slot<'a>(
        &self,
        record: &Record,
        view: &'a LedgerView,
    ) -> Option<&'a LedgerSlot> {
        let project = self.affinity_dir(record.id);
        view.slots.iter().find(|slot| slot.project == project)
    }

    /// Whether the ledger holds a slot for this record's private project.
    pub(super) fn record_has_slot(&self, record: &Record, view: &LedgerView) -> bool {
        self.record_slot(record, view).is_some()
    }

    /// Decide what this project's slot cap allows a record that owns no slot
    /// of it. A record that is held, retired, Barge-held or uncertain (a live
    /// or unattributable lock) is never a candidate for replacement.
    pub(super) fn project_cap_replacement(
        &self,
        state: &State,
        key: &SlotKey,
        view: &LedgerView,
        existing: Option<usize>,
        busy: bool,
        own_barge_held: Option<&str>,
        barge_holds: &mut BargeHolds<'_>,
    ) -> Result<ProjectCap, String> {
        let project_slots = state
            .records
            .iter()
            .filter(|record| record.identity == key.identity && self.record_has_slot(record, view))
            .count();
        if project_slots < PROJECT_SLOT_CAP {
            return Ok(ProjectCap::Room);
        }
        // Replace the least-recently-used eligible idle whole slot of this
        // project by ledger rank; foreign and unmeasurable slots fail the
        // library's own ownership and accounting checks.
        let mut candidates: Vec<(u64, u64, u64, String)> = state
            .records
            .iter()
            .filter(|record| {
                record.phase == Phase::Idle
                    && record.identity == key.identity
                    && self.record_has_slot(record, view)
            })
            .filter_map(|record| {
                let project = self.affinity_dir(record.id);
                view.slots
                    .iter()
                    .find(|slot| slot.project == project && slot.state == SlotOccupancy::Idle)
                    .map(|slot| {
                        (
                            slot.rank,
                            slot.slot_id.parse::<u64>().unwrap_or(u64::MAX),
                            record.id,
                            slot.slot_id.clone(),
                        )
                    })
            })
            .collect();
        // A specific held compatible slot of any checkout of this repository
        // can become reusable under the existing identity and freshness
        // rules; only such a slot justifies waiting.
        let compatible_busy = state.records.iter().any(|record| {
            record.identity == key.identity
                && record.compatibility == key.compatibility
                && matches!(
                    record.phase,
                    Phase::Acquiring | Phase::Spawning | Phase::Running
                )
        });
        let chosen = existing.map(|index| state.records[index].id);
        let mut barge_held_victim: Option<String> = None;
        candidates.sort();
        for (_, _, id, slot_id) in candidates {
            if Some(id) == chosen {
                continue;
            }
            // Only an idle record whose lock and Barge hold are both free can
            // be replaced; the library rechecks its identity, ownership,
            // occupancy and complete accounting under the pool lock.
            let Probe::Free(_victim) = probe_lock(&self.lock_path(id), false)? else {
                continue;
            };
            if barge_holds.is_held(&slot_id)? {
                barge_held_victim.get_or_insert(slot_id);
                continue;
            }
            return Ok(ProjectCap::Replace(id, slot_id));
        }
        // Wait only for a specific held compatible slot of this checkout, or for
        // a Barge-held slot whose hold is kept by another process. With no such
        // slot and no safely evictable idle one, refuse honestly instead of
        // promising a deletion that a safety gate forbids.
        if busy || compatible_busy {
            return Ok(ProjectCap::Wait(Wait::Checkout));
        }
        if let Some(slot_id) = own_barge_held.map(str::to_owned).or(barge_held_victim) {
            return Ok(ProjectCap::Wait(Wait::BargeHeld(slot_id)));
        }
        Err(format!(
            "the project already holds {PROJECT_SLOT_CAP} slots and none is a safely evictable idle whole slot; no compatible slot can be reused. Wait for one to become idle or free one with `oxide clean` in another checkout, then retry"
        ))
    }

    /// Take this attempt's record lock, create the record when this checkout
    /// has none, and record the command as acquiring. The trusted origin is
    /// read before the use counter advances.
    pub(super) fn claim_record(
        &self,
        state: &mut State,
        key: &SlotKey,
        profile: Profile,
        ticket: u64,
        label: Option<&str>,
        choice: &mut SlotChoice,
    ) -> Result<ClaimedRecord, String> {
        let (index, lock) = match choice.existing {
            Some(index) => {
                let lock = choice
                    .lock
                    .take()
                    .ok_or("Oxide record lock was not taken")?;
                (index, lock)
            }
            None => {
                if state.records.len() >= MAX_RECORDS {
                    return Err("Oxide control state reached its record limit".to_owned());
                }
                let id = state.take_id()?;
                ensure_private_dir(&self.affinity_dir(id))?;
                let Probe::Free(file) = probe_lock(&self.lock_path(id), true)? else {
                    return Err("a new Oxide record lock is unexpectedly held".to_owned());
                };
                state.records.push(Record {
                    id,
                    identity: key.identity.clone(),
                    compatibility: key.compatibility.clone(),
                    profile: profile.label().to_owned(),
                    checkout: Some(key.checkout.clone()),
                    phase: Phase::Idle,
                    process_group: None,
                    last_used: 0,
                    holder: None,
                    lease_id: None,
                    pending_recovery: None,
                    retired: None,
                    lost: None,
                    last_used_ms: None,
                });
                (state.records.len() - 1, file)
            }
        };
        let record = state.records[index].id;
        // Read before this command advances the use counter.
        let recorded = match choice.shared_origin.take() {
            Some(origin) => Some(origin),
            None => self.trusted_origin(&state.records[index]),
        };
        let lease_holder = format!("oxide-{}-{ticket}", std::process::id());
        state.records[index].phase = Phase::Acquiring;
        state.records[index].process_group = None;
        state.records[index].lease_id = None;
        state.records[index].holder = Some(Holder {
            pid: std::process::id(),
            started_ms: process::started_ms(),
            lease_holder: lease_holder.clone(),
            command: label.map(str::to_owned),
        });
        state.touch(index)?;
        // Keep a trusted origin trusted until Cargo starts: written before
        // the state that advances the counter, so a crash before Cargo (even
        // mid-gate, after this record moved to this checkout) leaves the
        // gate to run again on retry.
        if let Some(origin) = &recorded {
            self.write_origin(record, state.records[index].last_used, Some(origin.clone()))?;
        }
        Ok(ClaimedRecord {
            record,
            lock,
            recorded,
            lease_holder,
        })
    }

    /// The library request for this record: its affinity project, the slot
    /// layout this slot joins, the pool's budget and volume reserve, and the
    /// victim a project-cap replacement may delete.
    pub(super) fn lease_request(
        &self,
        key: &SlotKey,
        profile: Profile,
        claimed: &ClaimedRecord,
        folders: Vec<String>,
        replace: Option<&(u64, String)>,
    ) -> Result<LeaseRequest, String> {
        let replace_slot =
            match replace {
                Some((_, slot_id)) => Some(slot_id.parse::<u64>().map_err(|_| {
                    format!("recorded replacement slot ID {slot_id:?} is not numeric")
                })?),
                None => None,
            };
        let lease_profile = match profile {
            Profile::Debug => LeaseProfile::FullDebug,
            Profile::Quick | Profile::Release => LeaseProfile::Dev,
        };
        Ok(LeaseRequest {
            pool: self.pool.clone(),
            project: self.affinity_dir(claimed.record),
            workspace: key.checkout.clone(),
            profile: lease_profile,
            holder: claimed.lease_holder.clone(),
            budget_bytes: self.budget,
            // The registered removable volume's free-space reserve, when this
            // pool is the removable one. It is a reserve/prune signal for an
            // unknown-size admission, never a pool-budget input.
            reserve_bytes: self.admission_reserve(),
            layout: Some(SlotLayout {
                folders,
                profile: profile.label().to_owned(),
            }),
            replace_slot,
        })
    }

    /// Record a refused lease: the record goes back to idle (and an adopted
    /// shared slot back to its checkout), and the refusal maps to the wait
    /// this command observes. A capacity refusal that charged an incomplete
    /// walk also records that fact, and never changes the refusal itself.
    pub(super) fn record_refusal(
        &self,
        state: &mut State,
        index: usize,
        ticket: u64,
        choice: &SlotChoice,
        error: LeaseError,
    ) -> Result<Result<Lease, Wait>, String> {
        state.settle_idle(index);
        if let Some(shared_from) = choice.shared_from.clone() {
            state.records[index].checkout = Some(shared_from);
        }
        let wait = match error {
            LeaseError::BargeHeld(slot_id) => Wait::BargeHeld(slot_id.to_string()),
            LeaseError::Busy => Wait::Uncertain,
            // Wait only for a specific held compatible slot of this
            // checkout. Nothing promises that an unrelated build's
            // completion frees a count or byte ceiling.
            LeaseError::Full | LeaseError::LedgerExhausted | LeaseError::SlotCeiling(_) => {
                if choice.busy {
                    match error {
                        LeaseError::SlotCeiling(diagnosis) => Wait::Capacity(Some(diagnosis)),
                        _ => Wait::Capacity(None),
                    }
                } else if let Some(slot_id) = &choice.barge_held {
                    Wait::BargeHeld(slot_id.clone())
                } else {
                    self.save(state)?;
                    return Err(lease_error("cannot lease a slot", &error));
                }
            }
            // The refusal names the slots a partial walk charged
            // conservatively and keeps; record the incomplete outcome so
            // status shows `not measured (walk incomplete)` for them too
            // (oxide#186). Diagnostic only: nothing here changes the refusal,
            // its reason or its exit code.
            LeaseError::CapacityCharged { ref unmeasured, .. } => {
                self.record_incomplete_walks(None, unmeasured);
                self.save(state)?;
                return Err(lease_error("cannot lease a slot", &error));
            }
            error => {
                self.save(state)?;
                return Err(lease_error("cannot lease a slot", &error));
            }
        };
        self.set_ticket_blocked(ticket, true)?;
        self.save(state)?;
        Ok(Err(wait))
    }

    pub(super) fn find_own_slot(
        &self,
        state: &State,
        key: &SlotKey,
        view: &LedgerView,
        barge_holds: &mut BargeHolds<'_>,
    ) -> Result<OwnSlotProbe, String> {
        let matches = |record: &Record| {
            record.phase != Phase::Retired
                && record.checkout.as_deref() == Some(key.checkout.as_path())
                && record.compatibility == key.compatibility
        };
        // Second-slot rule: this checkout may hold several records of one
        // key. Take an idle one whose lock and Barge hold are both free, one
        // with a slot first, then the most recently used. A busy one (a live
        // command, an interrupted one whose group may still run, a lock still
        // held, or an idle-in-ledger slot whose Barge hold is still held by
        // another process) is never reused, released, evicted or relabelled
        // here; the command takes another idle compatible slot or a new one
        // through the normal admission below, and waits only when neither
        // exists.
        let mut own: Vec<usize> = (0..state.records.len())
            .filter(|&index| matches(&state.records[index]))
            .collect();
        own.sort_by_key(|&index| {
            let record = &state.records[index];
            std::cmp::Reverse((self.record_has_slot(record, view), record.last_used))
        });
        let mut existing = None;
        let mut lock = None;
        let mut busy = false;
        let mut barge_held_slot: Option<String> = None;
        for index in own {
            if state.records[index].phase != Phase::Idle {
                busy = true;
                continue;
            }
            match probe_lock(&self.lock_path(state.records[index].id), true)? {
                Probe::Free(file) => {
                    if let Some(slot) = self.record_slot(&state.records[index], view) {
                        if barge_holds.is_held(&slot.slot_id)? {
                            barge_held_slot.get_or_insert_with(|| slot.slot_id.clone());
                            continue;
                        }
                    }
                    existing = Some(index);
                    lock = Some(file);
                    break;
                }
                Probe::Held | Probe::Missing => busy = true,
            }
        }
        Ok(OwnSlotProbe {
            existing,
            lock,
            busy,
            barge_held_slot,
        })
    }

    /// Forget idle records that no longer own a slot or match this key.
    pub(super) fn forget_stale_records(&self, state: &mut State, key: &SlotKey, view: &LedgerView) {
        let same_key = |record: &Record| {
            record.phase != Phase::Retired
                && record.checkout.as_deref() == Some(key.checkout.as_path())
                && record.compatibility == key.compatibility
        };
        let stale: Vec<u64> = state
            .records
            .iter()
            .filter(|record| {
                record.phase == Phase::Idle
                    && !self.record_has_slot(record, view)
                    && !same_key(record)
            })
            .map(|record| record.id)
            .collect();
        for id in stale {
            self.forget_record(state, id);
        }
    }

    /// Choose a folder already used by this repository, or derive a new one.
    pub(super) fn slot_folders(
        &self,
        state: &State,
        key: &SlotKey,
        view: &LedgerView,
    ) -> Vec<String> {
        // A new slot joins the folder this repository already uses, which
        // needs no Git call; otherwise its label comes from the checkout.
        let same_repository: Vec<PathBuf> = state
            .records
            .iter()
            .filter(|other| other.identity == key.identity)
            .map(|other| self.affinity_dir(other.id))
            .collect();
        let known = view
            .slots
            .iter()
            .filter(|slot| same_repository.contains(&slot.project))
            .find_map(|slot| {
                slot.dir
                    .as_deref()?
                    .split_once('/')
                    .map(|(folder, _)| folder)
            });
        match known {
            Some(folder) => vec![folder.to_owned()],
            None => identity::slot_folders(&identity::project_label(&key.checkout), &key.identity),
        }
    }

    /// The most recently used idle slot of the same repository and key whose
    /// last checkout is another live affinity and whose origin is trusted.
    /// Detached records (after `oxide clean`) are never shared.
    pub(super) fn shared_candidate(
        &self,
        state: &State,
        key: &SlotKey,
        view: &LedgerView,
        barge_holds: &mut BargeHolds<'_>,
    ) -> Result<Option<(u64, File, Built)>, String> {
        let mut candidates: Vec<(&Record, &str)> = state
            .records
            .iter()
            .filter(|record| {
                record.phase == Phase::Idle
                    && record.identity == key.identity
                    && record.compatibility == key.compatibility
                    && record
                        .checkout
                        .as_ref()
                        .is_some_and(|checkout| *checkout != key.checkout)
            })
            .filter_map(|record| {
                let project = self.affinity_dir(record.id);
                view.slots
                    .iter()
                    .find(|slot| slot.project == project && slot.state == SlotOccupancy::Idle)
                    .map(|slot| (record, slot.slot_id.as_str()))
            })
            .collect();
        candidates.sort_by_key(|(record, _)| std::cmp::Reverse(record.last_used));
        for (record, slot_id) in candidates {
            let Some(origin @ Built::Content(_)) = self.trusted_origin(record) else {
                continue;
            };
            if barge_holds.is_held(slot_id)? {
                continue;
            }
            if let Probe::Free(file) = probe_lock(&self.lock_path(record.id), true)? {
                return Ok(Some((record.id, file, origin)));
            }
        }
        Ok(None)
    }

    /// The record's origin if the last command on it recorded one and no
    /// command ran since.
    pub(super) fn trusted_origin(&self, record: &Record) -> Option<Built> {
        let bytes = bounded_read(&self.origin_path(record.id), MAX_ORIGIN_BYTES).ok()??;
        let stored: OriginFile = serde_json::from_slice(&bytes).ok()?;
        (stored.version == ORIGIN_VERSION && stored.mark == record.last_used)
            .then_some(stored.origin)
            .flatten()
    }

    pub(super) fn write_origin(
        &self,
        id: u64,
        mark: u64,
        origin: Option<Built>,
    ) -> Result<(), String> {
        ensure_private_dir(&self.control.join("origins"))?;
        let bytes = serde_json::to_vec(&OriginFile {
            version: ORIGIN_VERSION,
            mark,
            origin,
        })
        .map_err(|error| error.to_string())?;
        let path = self.origin_path(id);
        if bytes.len() as u64 > MAX_ORIGIN_BYTES {
            // Too large to record: leave the slot untrusted.
            return match fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    Err(format!("cannot clear a slot origin: {error}"))
                }
                _ => Ok(()),
            };
        }
        atomic_write_private(&self.control.join("origins"), &format!("{id}.json"), &bytes)
            .map_err(|error| format!("cannot write a slot origin: {error}"))
    }

    #[cfg(test)]
    pub(super) fn register_ticket(&self) -> Result<(u64, File), String> {
        self.register_waiter(false)
    }

    fn register_waiter(&self, already_admitted: bool) -> Result<(u64, File), String> {
        let _admission = self.admission()?;
        self.check_waiter_pool()?;
        let mut state = self.load()?;
        self.drop_dead_tickets(&mut state)?;
        let ticket = self.enqueue_waiter(&mut state, already_admitted)?;
        self.save(&state)?;
        Ok(ticket)
    }

    fn check_waiter_pool(&self) -> Result<(), String> {
        if self.backend == super::Backend::ApfsVolume && self.sibling.is_some() {
            let registration = crate::removable::read_registration(self.owner_control())?
                .ok_or("the selected removable pool is no longer registered")?;
            if registration.mount_point.join(registration.payload_dir()) != self.payload {
                return Err(
                    "the removable pool moved before admission; retry the command".to_owned(),
                );
            }
        }
        Ok(())
    }

    fn enqueue_waiter(
        &self,
        state: &mut State,
        already_admitted: bool,
    ) -> Result<(u64, File), String> {
        if state.tickets.len() >= MAX_TICKETS {
            return Err(format!(
                "the Oxide wait queue is full ({MAX_TICKETS} commands)"
            ));
        }
        let id = state.take_id()?;
        let Probe::Free(file) = probe_lock(&self.ticket_path(id), true)? else {
            return Err("a new Oxide wait ticket is unexpectedly held".to_owned());
        };
        // A joiner may wait for storage, but it does not queue for a host permit.
        self.set_ticket_blocked(id, already_admitted)?;
        state.tickets.push(Ticket {
            id,
            pid: std::process::id(),
        });
        Ok((id, file))
    }

    pub(super) fn remove_ticket(&self, id: u64) -> Result<(), String> {
        let _admission = self.admission()?;
        let mut state = self.load()?;
        self.remove_waiter(&mut state, id)
    }

    fn remove_waiter(&self, state: &mut State, id: u64) -> Result<(), String> {
        state.tickets.retain(|ticket| ticket.id != id);
        self.save(state)?;
        self.set_ticket_blocked(id, false)?;
        match fs::remove_file(self.ticket_path(id)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        }
    }

    /// A ticket or clean mark whose lock is free belongs to a command that ended.
    pub(super) fn drop_dead_tickets(&self, state: &mut State) -> Result<(), String> {
        let mut live = Vec::with_capacity(state.tickets.len());
        for ticket in state.tickets.drain(..) {
            let path = self.ticket_path(ticket.id);
            match probe_lock(&path, false)? {
                Probe::Held => live.push(ticket),
                Probe::Free(_) => {
                    let _ = fs::remove_file(path);
                    let _ = self.set_ticket_blocked(ticket.id, false);
                }
                Probe::Missing => {
                    let _ = self.set_ticket_blocked(ticket.id, false);
                }
            }
        }
        state.tickets = live;
        let mut live = Vec::with_capacity(state.cleans.len());
        for clean in state.cleans.drain(..) {
            let path = self.ticket_path(clean.id);
            match probe_lock(&path, false)? {
                Probe::Held => live.push(clean),
                Probe::Free(_)
                    if clean
                        .process_group
                        .is_some_and(|group| !process::group_empty(group)) =>
                {
                    live.push(clean)
                }
                Probe::Free(_) => {
                    let _ = fs::remove_file(path);
                }
                Probe::Missing => {}
            }
        }
        state.cleans = live;
        Ok(())
    }
}
