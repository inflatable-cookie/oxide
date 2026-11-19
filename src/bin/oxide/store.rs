//! Oxide's private control state beside the lease ledger.
//!
//! The lease ledger and its JSON live in `oxide_storage`, which this module
//! does not change. Oxide leases through the library with one owner-only
//! project directory per affinity record (checkout + compatibility key), so the library's "same project,
//! same profile" reuse is checkout-affine by construction. The control
//! directory `<pool>/oxide-managed` holds the records, FIFO wait tickets, the
//! admission lock and one `flock` per record held for the life of a command.
//!
//! Cross-checkout reuse: a checkout with no slot of its own may take an
//! idle compatible slot last used by another checkout of the same
//! repository when that slot's recorded compile origin is trusted; the
//! freshness gate then runs before Cargo. The origin lives in
//! `origins/<record>.json`, stamped with the record's use counter, so a
//! command that crashed or an older Oxide build leaves it untrusted.
//!
//! Own-command crash recovery: a record that is not idle names the holder
//! (PID, start time, lease holder and lease ID) and Cargo's process group.
//! Another command recovers its held lease only when the record lock is free
//! and the group is empty; see [`settle`]. Standalone leases have no record
//! and are never recovered here.

use crate::cli::Profile;
use crate::config::{self, Backup, Facts, Found, Settings};
use crate::freshness::{Built, Prior};
use crate::identity::{self, SlotKey};
use crate::process;
use crate::watchdog::Watchdog;
pub(crate) use oxide_storage::lease::control::*;
use oxide_storage::lease::managed::{
    evict_idle_slot, flush_volume, init_budget_pool, lease_measured, ledger_view, measure_within,
    prepare_pool, recover, release, snapshot, status as pool_status,
};
use oxide_storage::lease::{
    check_control_writer, fence_admission_writer, read_writer_state, AccountingScan,
    AdmissionClass, LeaseError, LeaseRequest, LedgerSlot, LedgerView, MemberPair, MemberSet,
    OneUseSlice, PoolStatus, Profile as LeaseProfile, RestoreOutcome, ReuseMeasurement,
    SliceAdmission, SliceReport, SlotLayout, SlotOccupancy, UnmeasuredSlot, VictimPolicy,
    BUDGET_BACKEND_DIRECTORY_V1, MANAGED_NAME, VOLUME_BACKEND_APFS_V1,
};
use oxide_storage::volume::Binding;
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

#[cfg(test)]
thread_local! {
    static EVICTION_REJECTION: std::cell::RefCell<Vec<(String, &'static str)>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub const GIB: u64 = 1024 * 1024 * 1024;
/// Automatic creation refuses below this host free space.
pub const HOST_RESERVE_BYTES: u64 = 20 * GIB;
/// Slots one project identity may keep; a new checkout or a command with no
/// idle compatible slot replaces the least-recently-used eligible idle whole
/// slot of that project (oxide#118, raised to twelve by oxide#177).
pub(crate) const PROJECT_SLOT_CAP: usize = 12;
/// Largest origin document read; the origin is a small JSON record.
const MAX_ORIGIN_BYTES: u64 = 8 * 1024 * 1024;
const ORIGIN_VERSION: u32 = 1;
const WAIT_POLL: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceAdmission<'a> {
    pub class: WorkspaceClass,
    /// The bounded classification reason, recorded only for a `one_use`
    /// record (§5.1).
    pub reason: Option<&'a str>,
}

impl<'a> WorkspaceAdmission<'a> {
    /// The pre-feature admission: a normal record, with neither class field on
    /// disk.
    pub const NORMAL: Self = Self {
        class: WorkspaceClass::Normal,
        reason: None,
    };

    /// A one-use admission with the bounded reason the record keeps.
    pub(crate) fn one_use(reason: &'a str) -> Self {
        Self {
            class: WorkspaceClass::OneUse,
            reason: Some(reason),
        }
    }
}

/// The compile origin a record's slot was left with. `mark` is the record's
/// use counter when the origin was written; leasing re-stamps it and Cargo's
/// start advances the counter, so a mismatch means Cargo may have run
/// unrecorded. `origin: None` is an
/// untrusted slot. An untrusted slot is never reused: the next command evicts
/// it and builds cold.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginFile {
    version: u32,
    mark: u64,
    origin: Option<Built>,
}

/// What a command may do with a non-idle record whose lock it could take,
/// so no Oxide process holds it.
#[derive(Debug, Eq, PartialEq)]
enum Settle {
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
fn return_path(record: &Record, slot: &LedgerSlot) -> Return {
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
fn unclosed_writers(record: &Record) -> Option<String> {
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
const BOOT_MARGIN_MS: u64 = 60_000;

/// A debug-only proof control naming the boot time a closure decision uses.
/// A native proof cannot reboot the host to test a previous boot session, so
/// the qualified shared supervisor's own layout may override the reading.
/// Release builds have no hook, and outside that layout the variable is
/// ignored. A present value defines the reading even when it is unusable: an
/// unparseable or zero value is an unknown boot time, never a fall back to
/// the real one.
#[cfg(debug_assertions)]
const BOOT_HOOK: &str = "OXIDE_TEST_BOOT_MS";

/// The qualified shared supervisor's own layout: a canonical
/// `OXIDE_TASK_SCRATCH_ROOT` at `<root>/scratch`, its sibling `evidence`
/// directory and an `OXIDE_PROOF_DEADLINE_NS`, under one `oxide-proof.*`
/// root.
#[cfg(debug_assertions)]
fn supervisor_layout() -> bool {
    let canonical = |name: &str| {
        let path = PathBuf::from(std::env::var_os(name)?);
        let resolved = path.canonicalize().ok()?;
        (resolved == path && resolved.is_dir()).then_some(resolved)
    };
    let (Some(scratch), Some(evidence)) = (
        canonical("OXIDE_TASK_SCRATCH_ROOT"),
        canonical("OXIDE_PROOF_EVIDENCE"),
    ) else {
        return false;
    };
    supervisor_layout_paths(
        &scratch,
        &evidence,
        std::env::var("OXIDE_PROOF_DEADLINE_NS").ok().as_deref(),
    )
}

/// The layout rule over already-canonical paths, so the seam's scope is
/// testable without touching the process environment.
#[cfg(debug_assertions)]
fn supervisor_layout_paths(scratch: &Path, evidence: &Path, deadline: Option<&str>) -> bool {
    let Some(root) = scratch.parent().filter(|root| {
        scratch.file_name().is_some_and(|name| name == "scratch")
            && root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("oxide-proof."))
    }) else {
        return false;
    };
    evidence == root.join("evidence")
        && deadline.is_some_and(|deadline| deadline.parse::<u64>().is_ok())
}

/// Parse a boot-time override: `None` when there is none, `Some(None)` when
/// one is present but unusable.
#[cfg(debug_assertions)]
fn boot_override(value: Option<&str>) -> Option<Option<u64>> {
    Some(value?.parse::<u64>().ok().filter(|ms| *ms > 0))
}

/// The boot session's start for a closure decision, or `None` when unknown.
/// Unknown is the safe answer: it never closes a slot.
fn boot_session() -> Option<u64> {
    #[cfg(debug_assertions)]
    if supervisor_layout() {
        let value = std::env::var_os(BOOT_HOOK).map(|value| value.to_string_lossy().into_owned());
        if let Some(injected) = boot_override(value.as_deref()) {
            return injected;
        }
    }
    let boot = process::boot_ms();
    boot
}

/// Whether the record's holder started before the current boot session, so a
/// restart ended every process that could still write through its slot. An
/// unknown boot time, a missing or zero start and a start inside the safety
/// margin are not closure; elapsed time, PID absence and device
/// disappearance never are either.
fn started_before_boot(record: &Record, boot_ms: Option<u64>) -> bool {
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
fn lost_writers_closed(record: &Record, boot_ms: Option<u64>) -> bool {
    match record.lost.as_ref() {
        Some(lost) if lost.unclosed.is_none() => true,
        Some(_) => started_before_boot(record, boot_ms),
        None => false,
    }
}

/// Whether a quarantined lost record is closed by the boot session alone: its
/// recorded reason still stands, but a restart ended its writers.
fn closed_by_boot(record: &Record, boot_ms: Option<u64>) -> bool {
    record
        .lost
        .as_ref()
        .is_some_and(|lost| lost.unclosed.is_some())
        && started_before_boot(record, boot_ms)
}

/// The control-record half of the one-use evictable checklist (design §5.5.1
/// items 1, 2 and 5): phase exactly `Idle`, no holder, lease, pending recovery,
/// retirement or loss marker, and no recorded process group. A recorded group
/// means a writer may still exist, so it is protected.
fn idle_member_record(record: &Record) -> bool {
    record.phase == Phase::Idle
        && record.holder.is_none()
        && record.lease_id.is_none()
        && record.pending_recovery.is_none()
        && record.retired.is_none()
        && record.lost.is_none()
        && record.process_group.is_none()
}

/// The own-command recovery rule. `held` is the held slot on the record's
/// private project, if any. `group_empty` is `killpg(pgid, 0)` reporting
/// `ESRCH`; a surviving member or a reused group keeps the slot held.
fn settle(record: &Record, held: Option<&LedgerSlot>, group_empty: impl Fn(i32) -> bool) -> Settle {
    let Some(slot) = held else {
        return Settle::Reset;
    };
    match return_path(record, slot) {
        Return::NeverStarted => Settle::Recover,
        Return::GroupEmpty(group) if group_empty(group) => Settle::Recover,
        Return::GroupEmpty(_) | Return::GroupUnrecorded | Return::NotOwn => Settle::Keep,
    }
}

/// The exclusive admission reason returned by one attempt.
#[derive(Debug)]
enum Wait {
    Queue(usize),
    HostLimit(usize),
    Checkout,
    Clean,
    #[allow(dead_code)]
    ProjectCap,
    Uncertain,
    Capacity(Option<String>),
}

impl Wait {
    fn category(&self) -> WaitCategory {
        match self {
            Self::Queue(_) => WaitCategory::Queue,
            Self::HostLimit(_) => WaitCategory::HostLimit,
            Self::Checkout => WaitCategory::Checkout,
            Self::Clean => WaitCategory::Clean,
            Self::ProjectCap => WaitCategory::ProjectCap,
            Self::Uncertain => WaitCategory::Uncertain,
            Self::Capacity(_) => WaitCategory::Capacity,
        }
    }

    fn describe(&self, max_active: usize) -> String {
        match self {
            Self::Queue(position) => format!("earlier oxide commands (position {position})"),
            Self::HostLimit(active) => {
                format!("a build slot ({active} of {max_active} host-wide in use)")
            }
            Self::Checkout => "another oxide command in this checkout".to_owned(),
            Self::Clean => "oxide clean in this checkout".to_owned(),
            Self::ProjectCap => {
                format!("an idle slot of this project (cap {PROJECT_SLOT_CAP} slots)")
            }
            Self::Uncertain => {
                "a slot held by an interrupted command whose Cargo may still run".to_owned()
            }
            Self::Capacity(None) => "pool capacity held by running builds".to_owned(),
            Self::Capacity(Some(diagnosis)) => format!("pool capacity ({diagnosis})"),
        }
    }
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
    lease_id: String,
    record: u64,
    _lock: File,
    /// The cooperative hold on the slot's Barge root, kept for as long as the
    /// command uses it. A child that kept the inherited descriptor keeps the
    /// root held after this lease is dropped.
    _root_hold: Option<barge::Hold>,
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
}

pub enum Acquired {
    Lease(Lease),
    Interrupted(i32),
    /// The admission window closed while this command was waiting (oxide#140).
    AdmissionExpired,
}

/// Exclusive clean of one checkout: no command for it holds or takes a slot
/// until [`Store::finish_clean`].
pub struct CleanGuard {
    id: u64,
    lock: File,
}

impl CleanGuard {
    /// Let the next spawned child inherit the clean lock, so the barrier
    /// holds until that child and everything sharing the descriptor ends.
    pub fn share_with_children(&self) -> Result<(), String> {
        // SAFETY: the descriptor is open; F_SETFD with 0 only clears
        // FD_CLOEXEC on it.
        if unsafe { libc::fcntl(self.lock.as_raw_fd(), libc::F_SETFD, 0) } == 0 {
            Ok(())
        } else {
            Err(format!(
                "cannot share the clean lock with Cargo: {}",
                std::io::Error::last_os_error()
            ))
        }
    }
}

pub enum Cleaning {
    Guard(CleanGuard),
    Interrupted(i32),
    /// A guarded verb's `Barrier` attempt found no progress possible: a dead
    /// busy fact (a free or missing lock with no live group) refuses here.
    /// Nothing was written (one-use S1, design §4.1 step 5).
    Busy(String),
    /// A guarded verb's `Barrier` attempt found a *live* holder or a lock held
    /// elsewhere. The caller releases the binding mutex, waits on the shipped
    /// poll and tries again; it never blocks inside this attempt, so the
    /// shipped lock order cannot deadlock with it (design §4.1 step 4).
    Retry(String),
}

/// How one checkout's clean barrier is acquired (one-use S1, design §4.1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanMode {
    /// The shipped order: recover dead tickets and settled leases, then mark
    /// the checkout and wait as long as it has activity. Every caller that
    /// existed before one-use keeps this mode unchanged.
    Recovering,
    /// Guarded verbs (`oxide outputs detach`, `oxide outputs
    /// cancel-reservation`): recover nothing, refuse dead or unknown custody
    /// **before** any mark, id or `waiters/` file exists, wait only for a
    /// classified *live* holder, and give up after a bounded attempt budget.
    /// Nothing here mutates control state on the refusal path, so a guarded
    /// verb's "nothing changed" claim holds, `next_id` included.
    Barrier,
}

/// The classified busy facts of one checkout for [`CleanMode::Barrier`]: a
/// live holder can progress, a dead fact never can without the shipped
/// recovery, and anything unreadable is unknown custody.
enum BarrierState {
    Clear,
    Live(String),
    Dead(String),
}

/// The production number of non-blocking barrier attempts (design §4.1 step 4).
const BARRIER_ATTEMPTS: usize = 25;

/// How many non-blocking barrier attempts a guarded verb makes before its
/// bounded refusal (design §4.1 step 4). The caller of
/// [`Store::begin_clean_barrier`] owns the budget and the poll between
/// attempts. The production budget is [`BARRIER_ATTEMPTS`] shipped polls; the
/// unit suite uses two so a held barrier reaches its bounded refusal quickly.
pub(crate) fn barrier_attempts() -> usize {
    if cfg!(test) {
        2
    } else {
        BARRIER_ATTEMPTS
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WaitCategory {
    Queue,
    HostLimit,
    Checkout,
    Clean,
    ProjectCap,
    Uncertain,
    Capacity,
}

impl WaitCategory {
    const ALL: [Self; 7] = [
        Self::Queue,
        Self::HostLimit,
        Self::Checkout,
        Self::Clean,
        Self::ProjectCap,
        Self::Uncertain,
        Self::Capacity,
    ];

    fn index(self) -> usize {
        match self {
            Self::Queue => 0,
            Self::HostLimit => 1,
            Self::Checkout => 2,
            Self::Clean => 3,
            Self::ProjectCap => 4,
            Self::Uncertain => 5,
            Self::Capacity => 6,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::HostLimit => "host limit",
            Self::Checkout => "checkout",
            Self::Clean => "clean",
            Self::ProjectCap => "project cap",
            Self::Uncertain => "uncertain slot",
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
    reason_lines: usize,
    suppression_announced: bool,
    ceiling_announced: bool,
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
    fn observe(&mut self, wait: &Wait, max_active: usize, now: Instant) {
        let reason = wait.describe(max_active);
        self.observe_with_reason(wait, now, &reason);
    }

    fn observe_with_reason(&mut self, wait: &Wait, now: Instant, reason: &str) {
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

    fn account_active_until(&mut self, now: Instant) {
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

    fn completion_line(&self, ending: &str) -> Option<String> {
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

fn format_duration(duration: Duration) -> Option<String> {
    let tenths = duration.as_nanos().checked_add(50_000_000)? / 100_000_000;
    Some(format!("{}.{:01}s", tenths / 10, tenths % 10))
}

/// Which accounting one `oxide status` view shows (oxide#128).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Accounting {
    /// Recorded ledger, control state and auxiliary observations only: no
    /// pool lock, no reconciliation, no walk and no write.
    Recorded,
    /// The existing bounded walks, made without the pool lock, without
    /// reconciliation and without writing.
    Measured,
}

/// How one slot's size is known for this view. Only [`SlotSize::Measured`]
/// is a current walk; a recorded observation is a historical fact bound to
/// one slot generation and may have changed since it was taken.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SlotSize {
    Measured {
        logical: Option<u64>,
        unique: Option<u64>,
        scan: AccountingScan,
    },
    Recorded {
        allocated_bytes: u64,
        age: RecordedAge,
        source: ObservationSource,
        lease_id: Option<String>,
    },
    /// The ledger's logical-byte observation for this slot generation.
    LedgerRecorded {
        logical_bytes: u64,
        age: RecordedAge,
        entries: u64,
    },
    NotMeasured(String),
}

/// How old a recorded observation is by the wall clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordedAge {
    /// Seconds before this view's clock reading.
    Ago(u64),
    /// The recorded time is ahead of this clock; age unknown.
    ClockSkew,
}

impl SlotSize {
    /// One honest size phrase for a slot line. A recorded fact never reads
    /// as a current size, and an absent or unreadable one never reads as
    /// zero.
    pub fn describe(&self) -> String {
        match self {
            Self::Measured {
                logical, unique, ..
            } => match unique {
                Some(bytes) => format!(
                    "{} allocated, {} logical (measured now)",
                    crate::cargo_home::size(*bytes),
                    logical.map_or_else(|| "unknown".to_owned(), crate::cargo_home::size)
                ),
                None => "size unknown (measured now)".to_owned(),
            },
            Self::Recorded {
                allocated_bytes,
                age,
                source,
                lease_id,
            } => {
                let age = match age {
                    RecordedAge::Ago(seconds) => format!("recorded {} ago", age_text(*seconds)),
                    RecordedAge::ClockSkew => {
                        "recorded at a time ahead of this clock, so its age is unknown".to_owned()
                    }
                };
                let lease = lease_id
                    .as_deref()
                    .map_or_else(String::new, |lease| format!(" by lease {lease}"));
                format!(
                    "{} ({age}; {}{lease}); may have changed since measurement",
                    crate::cargo_home::size(*allocated_bytes),
                    source.label()
                )
            }
            Self::LedgerRecorded {
                logical_bytes,
                age,
                entries,
            } => {
                let age = match age {
                    RecordedAge::Ago(seconds) => {
                        format!("recorded {} ago", age_text(*seconds))
                    }
                    RecordedAge::ClockSkew => {
                        "recorded at a time ahead of this clock, so its age is unknown".to_owned()
                    }
                };
                format!(
                    "{} logical ({age} over {entries} entries); may have changed since measurement",
                    crate::cargo_home::size(*logical_bytes)
                )
            }
            Self::NotMeasured(reason) => reason.clone(),
        }
    }
}

/// Compact wall-clock age: seconds, minutes, hours or days.
pub(crate) fn age_text(seconds: u64) -> String {
    match seconds {
        0..=119 => format!("{seconds}s"),
        120..=7199 => format!("{}m", seconds / 60),
        7200..=172_799 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// A recorded observation's provenance. Only walks the library already makes
/// during an acquiring command are recorded: nothing fabricates an admission
/// measurement or adds a walk to obtain one.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObservationSource {
    /// The reuse walk measured this slot generation completely.
    LeaseReuse,
    /// An admission walked this slot generation and stopped partial, so it was
    /// charged conservatively and kept (oxide#186). The fact carries **no**
    /// size: `allocated_bytes` is written as 0 and is never read for this
    /// source, which reads as `not measured (walk incomplete)`. A later
    /// complete walk of the same generation records it normally, either as a
    /// ledger record at release or as a reuse fact.
    AdmissionWalkIncomplete,
}

impl ObservationSource {
    fn label(self) -> &'static str {
        match self {
            Self::LeaseReuse => "measured by the reuse walk when leased",
            Self::AdmissionWalkIncomplete => "walked by an admission without completing",
        }
    }
}

/// The diagnostic sidecar's schema. It is not part of the pool, ledger,
/// registration or receipt formats and grants no accounting authority.
const OBSERVATIONS_SCHEMA: &str = "oxide.status-observations/1";
/// One slot's size phrase when the last walk of its generation stopped partial
/// (oxide#186). It never carries a figure.
pub(crate) const INCOMPLETE_WALK: &str = "not measured (walk incomplete)";
const OBSERVATIONS_NAME: &str = "status-observations.json";
const OBSERVATIONS_TEMP: &str = "status-observations.json.tmp";
/// The sidecar's whole-file bound and retained observation count. The count
/// is at least the pool's slot ceiling, so it never lowers real capacity
/// (256 facts at the 256-slot ceiling, oxide#177).
const MAX_OBSERVATIONS_BYTES: u64 = 512 * 1024;
const MAX_OBSERVATIONS: usize = 256;

/// One recorded size fact, bound to exactly one slot generation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    store_id: String,
    slot_id: String,
    incarnation: String,
    /// The ledger's recorded slot directory; `None` is the legacy `<id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dir: Option<String>,
    measured_at_ms: u64,
    /// The measured unique allocated bytes. For
    /// [`ObservationSource::AdmissionWalkIncomplete`] this is written as 0 and
    /// read by nothing: an incomplete walk has no size.
    allocated_bytes: u64,
    source: ObservationSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease_id: Option<String>,
}

impl Observation {
    fn matches(&self, store_id: &str, slot: &LedgerSlot) -> bool {
        self.store_id == store_id
            && self.slot_id == slot.slot_id
            && self.incarnation == slot.incarnation
            && self.dir == slot.dir
    }

    fn validate(&self) -> Result<(), String> {
        let bounded = |value: &str, max: usize| !value.is_empty() && value.len() <= max;
        if !bounded(&self.store_id, 64)
            || !bounded(&self.slot_id, 160)
            || !bounded(&self.incarnation, 64)
            || self.dir.as_deref().is_some_and(|dir| dir.len() > 200)
            || self
                .lease_id
                .as_deref()
                .is_some_and(|lease| lease.len() > 160)
        {
            return Err("a recorded observation has an out-of-range field".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Observations {
    schema: String,
    observations: Vec<Observation>,
}

impl Default for Observations {
    fn default() -> Self {
        Self {
            schema: OBSERVATIONS_SCHEMA.to_owned(),
            observations: Vec::new(),
        }
    }
}

/// Pool and slot view for `oxide status`.
pub struct Report {
    /// The walks, only in measured mode.
    pub measured: Option<PoolStatus>,
    /// The pool's recorded cap, always.
    pub cap_bytes: u64,
    /// Slots recorded `Evicting`: a deletion is pending reconciliation, and
    /// this view neither reports them as idle nor tries to finish them.
    pub pending_reconciliation: Vec<String>,
    pub flushes: Vec<Flush>,
    pub waiting: usize,
    pub(crate) waiters: Vec<Waiter>,
    pub cleaning: Vec<PathBuf>,
    pub slots: Vec<SlotView>,
    pub recoveries: Vec<Recovery>,
    /// This pool's most recently used slot by its use counter.
    pub last_use: Option<LastUse>,
    /// Why the control state could not be read. Slots, flushes, waits and
    /// holders then read as unknown, never as empty.
    pub state_error: Option<String>,
    /// The sum of the ledger's current per-slot records (oxide#146), or `None`
    /// when no slot has one. This is a recorded admission basis, not free
    /// space, and never includes an oxide#128 sidecar fact.
    pub recorded_total: Option<u64>,
    /// Idle slots whose ledger record is current.
    pub recorded_slots: usize,
    /// Slots with no current ledger record: they make `recorded_total` a
    /// stated lower bound.
    pub unrecorded_slots: usize,
    /// The bounded class facts of this pool's ledger (design §5.5, §5.6).
    pub class: ClassMembership,
}

/// One bounded ledger row of a one-use member or an unattributed row, carried
/// as the `(slot_id, incarnation)` pair design §5.5 encodes so the library can
/// verify it against the ledger before any use.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(dead_code)] // The pairs are consumed by the S3 admission protocol.
pub struct ClassMember {
    pub slot_id: String,
    pub incarnation: String,
}

/// The bounded class-scoped view of one pool's ledger (design §5.5, §5.6).
///
/// `members` and `unknown` are disjoint, and together with the verified normal
/// slots they account for every ledger row exactly once: a row is a member when
/// its owning record is `one_use`, normal when that record is `normal`, and
/// **unknown** when no record owns it. An unknown row is never a member, never
/// an eviction candidate and never free quota; a pool holding one can only ever
/// report its slice as a lower bound.
///
/// This view is an observation, never an authorisation: nothing here is a
/// writer-closure proof, and a dormant build writes no one-use record at all,
/// so `status` sees an empty view for every pool that exists today.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ClassMembership {
    pub members: Vec<ClassMember>,
    pub unknown: Vec<ClassMember>,
}

/// An existing lease-ticket record shown by `oxide status`.
#[derive(Debug)]
pub(crate) struct Waiter {
    pub(crate) pid: u32,
    pub(crate) age: Option<WaitAge>,
}

#[derive(Debug)]
pub(crate) struct WaitAge {
    pub(crate) seconds: u64,
    pub(crate) source: WaitAgeSource,
}

#[derive(Debug)]
pub(crate) enum WaitAgeSource {
    Created,
    Modified,
}

impl WaitAgeSource {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Created => "file creation time",
            Self::Modified => "file modification time fallback",
        }
    }
}

/// The latest recorded use in one pool, from its control state only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LastUse {
    /// `None` when the pool's ledger was not read.
    pub slot_id: Option<String>,
    pub label: Option<String>,
    pub checkout: Option<PathBuf>,
    /// Wall clock of that use; `None` for a use recorded before Oxide kept it.
    pub at_ms: Option<u64>,
}

/// Status label of a slot: its recorded folder, else its checkout's
/// directory name.
pub fn slot_label(dir: Option<&str>, checkout: Option<&Path>) -> Option<String> {
    match dir.and_then(|dir| dir.split_once('/')) {
        Some((folder, _)) => Some(folder.to_owned()),
        None => checkout
            .and_then(Path::file_name)
            .map(|name| crate::identity::sanitize_label(&name.to_string_lossy())),
    }
}

/// The latest use recorded in a pool's control state, read-only and with
/// no ledger or volume access, for a pool status does not open. A missing
/// or unreadable state is `Err`, never "never used".
pub fn recorded_last_use(control: &Path) -> Result<Option<LastUse>, String> {
    let state = read_state(&control.join("state.json"))?.ok_or("no Oxide control state")?;
    Ok(state
        .records
        .iter()
        .filter(|record| record.last_used > 0)
        .max_by_key(|record| record.last_used)
        .map(|record| LastUse {
            slot_id: None,
            label: slot_label(None, record.checkout.as_deref()),
            checkout: record.checkout.clone(),
            at_ms: record.last_used_ms,
        }))
}

/// Slots lost with the removable volume, from its control state read-only
/// and without the volume: slot ID, checkout and what happens to it.
pub fn recorded_lost(control: &Path) -> Result<Vec<(String, Option<PathBuf>, String)>, String> {
    let Some(state) = read_state(&control.join("state.json"))? else {
        return Ok(Vec::new());
    };
    let boot = boot_session();
    Ok(state
        .records
        .iter()
        .filter_map(|record| {
            let lost = record.lost.as_ref()?;
            Some((
                lost.slot_id.clone(),
                record.checkout.clone(),
                lost.describe(closed_by_boot(record, boot)),
            ))
        })
        .collect())
}

/// Cooperative holders of one checkout, read-only: active records and clean
/// barriers, newest state only. Used by `oxide outputs plan`; an empty list
/// means no cooperative Oxide command holds the checkout right now, never
/// that no external writer exists.
pub fn active_holders(control: &Path, checkout: &Path) -> Result<Vec<String>, String> {
    holders(control, checkout, true)
}

fn holders(control: &Path, checkout: &Path, cleans: bool) -> Result<Vec<String>, String> {
    let Some(state) = read_state(&control.join("state.json"))? else {
        return Ok(Vec::new());
    };
    Ok(holder_lines(&state, checkout, cleans))
}

/// One parsed state's holder lines for `checkout`: active records and, when
/// `cleans`, recorded clean barriers. Shared with the fresh per-row custody
/// witness (oxide#235) so both routes report the exact same texts.
fn holder_lines(state: &State, checkout: &Path, cleans: bool) -> Vec<String> {
    let mut holders = Vec::new();
    for record in &state.records {
        if record.checkout.as_deref() != Some(checkout) || !record.active() {
            continue;
        }
        let name = record
            .holder
            .as_ref()
            .and_then(|holder| holder.command.as_deref())
            .unwrap_or("command");
        let pid = record
            .holder
            .as_ref()
            .map_or_else(|| "unknown".to_owned(), |holder| holder.pid.to_string());
        holders.push(format!("oxide pid {pid}: {name}"));
    }
    for clean in state.cleans.iter().filter(|_| cleans) {
        if clean.checkout == checkout {
            holders.push("oxide clean barrier".to_owned());
        }
    }
    holders
}

/// Cooperative custody for one checkout across parsed pool states: an active
/// record in either pool counts as a writer, and a retired slot whose lost
/// writers were never positively closed — and whose holder predates no boot
/// boundary — is unknown custody, not activity. Shared by
/// [`Store::checkout_custody`] and the fresh per-row custody witness
/// (oxide#235) so both classify records identically. The pool labels appear
/// verbatim in unknown-custody reasons.
fn custody_verdict(
    pools: &[(Option<&State>, &str, Option<u64>)],
    checkout: &Path,
    boot: Option<u64>,
) -> CheckoutCustody {
    let mut writers = false;
    // Record IDs are per pool, so the caller's own ID is exempt only in
    // its own pool's state; an equal ID in the other pool is another command.
    for (state, pool, exempt) in pools {
        let Some(state) = state else { continue };
        for record in &state.records {
            if record.checkout.as_deref() != Some(checkout) || Some(record.id) == *exempt {
                continue;
            }
            if record.phase == Phase::Retired {
                if let Some(lost) = record.lost.as_ref() {
                    // A restart closed a quarantined record whose holder
                    // predates this boot session; its slot is released and
                    // deleted whole at the next admission.
                    #[cfg(not(test))]
                    let unclosed = lost
                        .unclosed
                        .as_deref()
                        .filter(|_| !closed_by_boot(record, boot));
                    #[cfg(test)]
                    let unclosed = lost.unclosed.as_deref().filter(|_| {
                        let closed = closed_by_boot(record, boot);
                        !closed
                    });
                    if let Some(why) = unclosed {
                        return CheckoutCustody::Unknown(format!(
                            "slot {} in the {pool} pool is lost with writers that were never confirmed closed ({why})",
                            lost.slot_id
                        ));
                    }
                }
                continue;
            }
            if matches!(record.phase, Phase::Spawning | Phase::Running) {
                writers = true;
            }
        }
    }
    if writers {
        CheckoutCustody::Writers
    } else {
        CheckoutCustody::Clear
    }
}

/// What a checkout's cooperative custody allows; see
/// [`Store::checkout_custody`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckoutCustody {
    /// No command may be writing and no unknown writer custody remains.
    Clear,
    /// A command in this checkout may be running Cargo now.
    Writers,
    /// A lost slot's writers were never positively closed. Nothing may be
    /// relinked or cleaned through this checkout until that is resolved.
    Unknown(String),
}

pub struct SlotView {
    pub slot_id: String,
    /// Slot directory below `slots`; `None` is the legacy `<id>`.
    pub dir: Option<String>,
    /// Project label shown by status: the folder of a labelled slot, else
    /// the checkout directory name, else `None`.
    pub label: Option<String>,
    pub state: SlotOccupancy,
    pub profile: Option<String>,
    pub checkout: Option<PathBuf>,
    pub managed: bool,
    /// Held with no live Oxide owner. The next command recovers it once
    /// Cargo's group is empty; if the group was never recorded,
    /// `oxide pool recover-interrupted <slot>` is explicit.
    pub interrupted: Option<Interrupted>,
    /// Why a removable-volume slot was retired and stays held.
    pub retired: Option<String>,
    /// The holding command's label (`exec <program>`), while held.
    pub holder: Option<String>,
    /// This slot's size for this view: a live walk, a recorded observation
    /// or an honest not-measured phrase.
    pub size: SlotSize,
    /// The owning record's workspace class (design §5.1, §10.5); `None` when
    /// no record owns this row, which is a class-unknown slot (§5.6).
    pub class: Option<WorkspaceClass>,
    /// The bounded reason the owning record was classified one-use.
    pub class_reason: Option<String>,
}

pub struct Interrupted {
    pub holder_pid: Option<u32>,
    pub path: Return,
}

pub struct Store {
    pub pool: PathBuf,
    pub backend: Backend,
    /// The slot payload root: `<pool>/payload`, or the directory on the
    /// removable volume.
    pub payload: PathBuf,
    control: PathBuf,
    /// The soft budget for this command's admissions: the effective setting
    /// for the internal pool, `None` (the recorded budget) for a removable
    /// pool.
    budget: Option<u64>,
    /// The budget the pool recorded at creation, read when it opened.
    recorded_budget: u64,
    /// The command's resolved host-wide active-build limit.
    max_active: usize,
    /// The other pool's control directory when a removable pool is
    /// registered: the host-wide limit counts both pools and a checkout's
    /// clean barrier spans both.
    sibling: Option<Sibling>,
}

/// The other pool of a registered pair.
struct Sibling {
    control: PathBuf,
    /// Set on the removable store: its slot attempts also hold the internal
    /// pool's admission lock (always internal first), which internal
    /// `oxide clean` holds while it checks and marks. That makes a clean in
    /// either pool and a removable build for the same checkout exclusive.
    barrier: bool,
}

/// The pool's recorded backend. Neither has a ceiling of its own; the
/// retired `apfs-sparse-image-v1` backend refuses to open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    /// Plain directory: admission and idle eviction against a soft budget,
    /// plus the best-effort host-reserve watchdog. No ceiling.
    DirectoryBudget,
    /// A directory on a registered removable APFS volume: admission against
    /// a recorded budget, volume reserve checked when a build starts, flush
    /// before a slot goes idle. No ceiling of its own.
    ApfsVolume,
}

/// The retired image backend's recorded tag (oxide#102).
const LEGACY_IMAGE_BACKEND: &str = "apfs-sparse-image-v1";

impl Backend {
    pub fn tag(self) -> &'static str {
        match self {
            Self::DirectoryBudget => BUDGET_BACKEND_DIRECTORY_V1,
            Self::ApfsVolume => VOLUME_BACKEND_APFS_V1,
        }
    }

    /// What bounds the pool, as `status` and every summary line say it.
    pub fn bound(self) -> &'static str {
        match self {
            Self::DirectoryBudget => {
                "budget-only: soft budget checked at admission plus best-effort reserve watchdog, no hard ceiling"
            }
            Self::ApfsVolume => {
                "budget-only on a removable volume: admission estimate, volume reserve checked when a build starts, no hard ceiling"
            }
        }
    }

    fn recorded(tag: &str) -> Result<Self, String> {
        match tag {
            BUDGET_BACKEND_DIRECTORY_V1 => Ok(Self::DirectoryBudget),
            VOLUME_BACKEND_APFS_V1 => Ok(Self::ApfsVolume),
            LEGACY_IMAGE_BACKEND => Err(legacy_refusal()),
            other => Err(format!("the pool records an unknown backend {other:?}")),
        }
    }
}

fn legacy_refusal() -> String {
    "the pool is a retired apfs-sparse-image-v1 image pool; Oxide no longer attaches, writes or removes it. Choose another pool path (unset OXIDE_POOL or change [pool] path); the old pool is retired separately".to_owned()
}

/// `OXIDE_POOL_BACKEND` is retired with the image backend: unset or
/// `directory-budget` only.
fn refuse_retired_backend_request() -> Result<(), String> {
    match std::env::var_os("OXIDE_POOL_BACKEND") {
        None => Ok(()),
        Some(value) if value == "directory-budget" => Ok(()),
        Some(_) => Err(
            "OXIDE_POOL_BACKEND is retired: the apfs-sparse-image backend is gone and every internal pool is directory-budget".to_owned(),
        ),
    }
}

/// A size flag of `oxide pool register-removable`, with `OXIDE_POOL_CAP`'s
/// units. The removable backend keeps its own range check.
pub fn parse_cap(value: &str, _backend: Backend) -> Result<u64, String> {
    config::parse_size(value).map_err(|error| format!("OXIDE_POOL_CAP={value} {error}"))
}

/// The effective settings: environment, then config file, then defaults.
pub fn settings() -> Result<Settings, String> {
    refuse_retired_backend_request()?;
    let settings = config::settings(&Facts::from_env())?;
    static NOTICED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if let Some(notice) = &settings.home_notice {
        if !NOTICED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!("oxide: {notice}");
        }
    }
    Ok(settings)
}

/// The soft budget for the next admission. Debug builds accept
/// `OXIDE_TEST_BUDGET_BYTES` above everything else, so fixture proofs can
/// exercise admission on a tiny budget.
pub fn admission_budget(settings: &Settings) -> u64 {
    #[cfg(debug_assertions)]
    if let Some(bytes) = std::env::var("OXIDE_TEST_BUDGET_BYTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|bytes| *bytes > 0)
    {
        return bytes;
    }
    settings.budget
}

/// A source-verification failure inside the admission lifetime. A failure
/// before the first clone skips the seed and the command builds cold; only an
/// unknown Git closure refuses, because its custody is unknown (§6.5, §13).
pub(crate) enum SourceFailure {
    Skip(&'static str),
    Refuse(String),
}

pub(crate) fn source_failure(reason: &'static str) -> SourceFailure {
    if reason == "git source verification closure unknown" {
        SourceFailure::Refuse(reason.to_owned())
    } else {
        SourceFailure::Skip(reason)
    }
}

/// The post-Cargo source may stand in for a pre-Cargo entry only when every
/// recorded identity fact is unchanged. A newly appearing source has no
/// pre-Cargo entry and is accepted after the release-time unchanged-since check.
fn source_identity_matches(
    previous: &crate::seed_plan::VerifiedSource,
    post: &crate::seed_plan::VerifiedSource,
) -> bool {
    previous.package == post.package
        && previous.kind == post.kind
        && previous.root == post.root
        && previous.verification_root == post.verification_root
        && previous.tree_digest == post.tree_digest
        && previous.proc_macro == post.proc_macro
}

/// Outcome of one pre-child seed attempt. An abandoned recipient must be
/// released untrusted and the command admitted normally again.
pub(crate) enum SeedRun {
    Skipped(&'static str),
    Seeded(
        /// The transaction's own receipt. The command logs the `last`
        /// document and ignores this one; the seed-outcome regression reads it.
        #[allow(dead_code)]
        oxide_storage::lease::managed::SeedReceipt,
        oxide_storage::lease::managed::SeedLast,
    ),
    Abandoned,
}

impl Store {
    /// Verify the locked sources inside the admission lifetime before making
    /// a plan. Source verification reads Cargo home, never the donor payload.
    #[allow(dead_code)]
    pub(crate) fn seed_verified(
        &self,
        held: &Lease,
        producer: &oxide_storage::lease::managed::SeedProducer,
        lock: &crate::seed_lock::Analysis,
        cargo_home: &Path,
        cache: &mut oxide_storage::seed_source::SourceCache,
    ) -> Result<SeedRun, String> {
        self.seed(held, producer, lock, |candidates| {
            let mut pass = crate::seed_sources::Pass::new(Duration::from_secs(3));
            let offered: Vec<_> = lock
                .eligible
                .iter()
                .filter(|entry| {
                    candidates.iter().any(|candidate| {
                        candidate.catalog.packages.iter().any(|package| {
                            package.name == entry.name
                                && package.version == entry.version
                                && Some(package.source.as_str()) == entry.source.as_deref()
                                && package.checksum == entry.checksum
                        })
                    })
                })
                .cloned()
                .collect();
            let git_hints = pass
                .discover_git_hints(cargo_home, &offered)
                .unwrap_or_default();
            pass.locked(cargo_home, &offered, &git_hints, cache, process::now_ms())
                .map_err(source_failure)
        })
    }

    /// Build and publish a catalog for a successful, still-held producing
    /// generation. The caller verifies source trees on both sides of Cargo and
    /// supplies the producing start; all catalog work is bounded and the
    /// library rechecks the generation before its atomic sidecar write.
    #[allow(clippy::too_many_arguments, dead_code)]
    pub(crate) fn publish_seed_catalog(
        &self,
        held: &Lease,
        producer: oxide_storage::lease::managed::SeedProducer,
        lock: &crate::seed_lock::Analysis,
        before: &[crate::seed_plan::VerifiedSource],
        after: &[crate::seed_plan::VerifiedSource],
        started: SystemTime,
        started_ms: u64,
        successful: bool,
        origin: Option<&Built>,
        profile: &str,
        source_cache: &oxide_storage::seed_source::SourceCache,
    ) -> Result<(), String> {
        use oxide_storage::lease::managed::{
            held_seed_attempt, publish_seed_catalog, SeedAttemptState, SeedCatalog, SeedPackage,
            SeedRecipient, SeedSourceRoot,
        };
        if !successful || origin.is_none() || held.reused || !matches!(held.prior, Prior::Fresh) {
            return Ok(());
        }
        let recipient = SeedRecipient {
            slot_id: held.slot_id.clone(),
            incarnation: held.incarnation.clone(),
            lease_id: held.lease_id.clone(),
        };
        let _sibling = self.sibling_admission()?;
        let _admission = self.admission()?;
        let state = self.load()?;
        let record = state
            .records
            .iter()
            .find(|record| record.id == held.record)
            .filter(|record| record.lease_id.as_deref() == Some(held.lease_id.as_str()))
            .ok_or("the producing record is no longer held")?;
        let attempt = held_seed_attempt(&self.pool, &recipient)
            .map_err(|error| lease_error("cannot read seed attempt", &error))?;
        if !matches!(attempt, SeedAttemptState::None | SeedAttemptState::Skipped) {
            return Ok(());
        }
        let Some(mut classifier) = crate::seed_catalog::Producer::new(&held.build_dir) else {
            return Ok(());
        };
        let rejected = classifier.rejections(profile, &lock.rejected);
        let mut pass = crate::seed_sources::Pass::new(Duration::from_secs(4));
        let mut packages = Vec::new();
        // The catalog is built from the POST-Cargo verified sources. A first
        // build extracts a registry or Git source only while Cargo runs, so a
        // pre-Cargo pass alone can never catalogue it (§4.1/§4.4: the closed
        // producing epoch is the release-time evidence). An entry that also
        // appeared before Cargo must still have the same kind, roots, digest
        // and macro flag; a changed source is skipped, never catalogued. A
        // newly appearing source is accepted only after the same
        // unchanged-since check below.
        for post in after {
            if !lock.eligible.iter().any(|entry| entry == &post.package) {
                continue;
            }
            if let Some(previous) = before
                .iter()
                .find(|previous| previous.package == post.package)
            {
                if !source_identity_matches(previous, post) {
                    continue;
                }
            }
            if !pass.unchanged_since(&post.verification_root, post.kind, started) {
                continue;
            }
            let Some(units) = classifier.units(profile, post) else {
                return Ok(());
            };
            if units.is_empty() {
                continue;
            }
            let Some(source_name) = post.package.source.clone() else {
                continue;
            };
            packages.push(SeedPackage {
                name: post.package.name.clone(),
                target: post.package.name.replace('-', "_"),
                version: post.package.version.clone(),
                source: source_name,
                checksum: post.package.checksum.clone(),
                unique: true,
                leaf: true,
                source_root: SeedSourceRoot {
                    kind: post.kind,
                    digest: post.tree_digest.clone(),
                },
                units,
            });
        }
        if packages.is_empty() {
            return Ok(());
        }
        let catalog = SeedCatalog {
            schema: oxide_storage::lease::seed::SEED_CATALOG_SCHEMA.into(),
            store_id: held.store_id.clone(),
            slot_id: held.slot_id.clone(),
            incarnation: held.incarnation.clone(),
            lease_id: held.lease_id.clone(),
            record_id: held.record,
            mark: record.last_used,
            started_at_ms: started_ms,
            finished_at_ms: process::now_ms(),
            attempt,
            producer,
            packages,
            rejected,
        };
        publish_seed_catalog(&self.pool, &catalog)
            .map_err(|error| lease_error("cannot publish seed catalog", &error))?;
        // A cache miss is safe: this sidecar is only a performance hint and
        // never a trust root. Keep a malformed or over-cap cache untouched.
        let _ =
            oxide_storage::lease::managed::write_seed_sources(&self.pool, &recipient, source_cache);
        Ok(())
    }

    /// One admission lifetime spans the catalog read, effect-free plan,
    /// reserve gate, durable intent and pinned library transaction. `held`
    /// already owns the outermost recipient record lock.
    pub(crate) fn seed(
        &self,
        held: &Lease,
        producer: &oxide_storage::lease::managed::SeedProducer,
        lock: &crate::seed_lock::Analysis,
        verify_sources: impl FnOnce(
            &[oxide_storage::lease::managed::SeedCandidate],
        )
            -> Result<Vec<crate::seed_plan::VerifiedSource>, SourceFailure>,
    ) -> Result<SeedRun, String> {
        self.seed_with(
            held,
            producer,
            lock,
            verify_sources,
            |pool, request, monitor| {
                oxide_storage::lease::managed::seed_monitored(pool, request, monitor)
            },
        )
    }

    fn seed_with(
        &self,
        held: &Lease,
        producer: &oxide_storage::lease::managed::SeedProducer,
        lock: &crate::seed_lock::Analysis,
        verify_sources: impl FnOnce(
            &[oxide_storage::lease::managed::SeedCandidate],
        )
            -> Result<Vec<crate::seed_plan::VerifiedSource>, SourceFailure>,
        transaction: impl FnOnce(
            &Path,
            &oxide_storage::lease::managed::SeedRequest,
            &mut dyn FnMut() -> bool,
        )
            -> Result<oxide_storage::lease::managed::SeedOutcome, LeaseError>,
    ) -> Result<SeedRun, String> {
        use oxide_storage::lease::managed::{
            seed_candidates, write_seed_attempt, write_seed_last, SeedAttempt, SeedAttemptState,
            SeedBucket, SeedLast, SeedOutcome, SeedRecipient, ATTEMPT_SCHEMA,
        };
        if held.reused || !matches!(held.prior, Prior::Fresh) {
            return Ok(SeedRun::Skipped("recipient not fresh"));
        }
        if lock.eligible.is_empty() {
            return Ok(SeedRun::Skipped("no eligible locked packages"));
        }
        let recipient = SeedRecipient {
            slot_id: held.slot_id.clone(),
            incarnation: held.incarnation.clone(),
            lease_id: held.lease_id.clone(),
        };
        let _sibling = self.sibling_admission()?;
        let _admission = self.admission()?;
        let state = self.load()?;
        let record = state
            .records
            .iter()
            .find(|record| record.id == held.record)
            .filter(|record| record.lease_id.as_deref() == Some(held.lease_id.as_str()))
            .ok_or("the recipient record is no longer held")?;
        let candidates = seed_candidates(&self.pool, &recipient, 8)
            .map_err(|error| lease_error("cannot inspect seed candidates", &error))?;
        if candidates.candidates.is_empty() {
            return Ok(SeedRun::Skipped("no eligible donor"));
        }
        let sources = match verify_sources(&candidates.candidates) {
            Ok(sources) => sources,
            Err(SourceFailure::Skip(reason)) => return Ok(SeedRun::Skipped(reason)),
            Err(SourceFailure::Refuse(reason)) => return Err(reason),
        };
        let Some(request) = crate::seed_plan::select(
            &recipient,
            producer,
            &lock.eligible,
            &sources,
            &candidates.candidates,
        ) else {
            return Ok(SeedRun::Skipped("no eligible donor"));
        };
        // Debug-only proof pause, ordered between call 1 (the candidate read)
        // and call 2 (the donor pin). Absent from release builds and armed only
        // under the shared proof supervisor's own layout, so a competing
        // command can lease or evict the selected donor while this one waits.
        #[cfg(debug_assertions)]
        seed_proof_pause("between-calls");
        let rejected = candidates
            .candidates
            .iter()
            .find(|candidate| candidate.slot_id == request.donor.slot_id)
            .ok_or("the selected donor disappeared")?
            .catalog
            .rejected
            .clone();
        // The seed point's start gate (§8.3). The internal pool samples the
        // host reserve that will later stop its Cargo; a removable pool
        // re-verifies its mount/volume-UID binding and the registration's
        // reserve (the existing admission rule) and then monitors the binding.
        // A command with nothing to seed never reaches here.
        let (watchdog, trigger) = match self.backend {
            Backend::DirectoryBudget => {
                let watchdog = Watchdog::for_pool(&self.payload);
                let trigger = HOST_RESERVE_BYTES + crate::watchdog::MARGIN_BYTES;
                let first = watchdog.seed_sample();
                if first.is_none_or(|bytes| bytes < trigger) {
                    return Err(format!(
                        "seed reserve stop: available space {first:?}, trigger {trigger} bytes"
                    ));
                }
                (watchdog, Some(trigger))
            }
            Backend::ApfsVolume => {
                let mount = self.seed_removable_gate()?;
                (Watchdog::for_volume(&mount), None)
            }
        };
        let mut minimum: Option<u64> = None;
        let mut attempt = SeedAttempt {
            schema: ATTEMPT_SCHEMA.to_owned(),
            store_id: held.store_id.clone(),
            slot_id: held.slot_id.clone(),
            incarnation: held.incarnation.clone(),
            lease_id: held.lease_id.clone(),
            state: SeedAttemptState::Intent,
            donor_slot: None,
            units: None,
            files: None,
            logical_bytes: None,
            catalog_digest: None,
            observation: None,
        };
        write_seed_attempt(&self.pool, &attempt)
            .map_err(|error| lease_error("cannot record seed intent", &error))?;
        self.write_origin(held.record, record.last_used, None)?;
        let mut next_sample = Instant::now() + crate::watchdog::INTERVAL;
        let mut steps = 0usize;
        let mut stopped = false;
        let mut admission_stopped = false;
        let mut monitor = || {
            if crate::budget::admission_expired() {
                admission_stopped = true;
                return true;
            }
            steps += 1;
            if steps < 64 && Instant::now() < next_sample {
                return false;
            }
            steps = 0;
            next_sample = Instant::now() + crate::watchdog::INTERVAL;
            match (trigger, watchdog.seed_sample()) {
                (Some(trigger), Some(bytes)) if bytes >= trigger => {
                    minimum = Some(minimum.map_or(bytes, |low| low.min(bytes)));
                    false
                }
                (Some(_), Some(bytes)) => {
                    minimum = Some(minimum.unwrap_or(bytes).min(bytes));
                    stopped = true;
                    true
                }
                // A removable pool has no space trigger: only a failed
                // binding (a removal or remount) stops the seed.
                (None, Some(_)) => false,
                (Some(_), None) | (None, None) => {
                    stopped = true;
                    true
                }
            }
        };
        let outcome = transaction(&self.pool, &request, &mut monitor);
        drop(monitor);
        match outcome {
            Ok(SeedOutcome::Skipped(skip)) => {
                attempt.state = SeedAttemptState::Skipped;
                write_seed_attempt(&self.pool, &attempt)
                    .map_err(|error| lease_error("cannot record seed skip", &error))?;
                Ok(SeedRun::Skipped(skip.reason.as_str()))
            }
            Ok(SeedOutcome::Abandoned(_)) => {
                attempt.state = SeedAttemptState::Reset;
                write_seed_attempt(&self.pool, &attempt)
                    .map_err(|error| lease_error("cannot record abandoned seed", &error))?;
                Ok(SeedRun::Abandoned)
            }
            Ok(SeedOutcome::Seeded(receipt)) => {
                attempt.state = SeedAttemptState::Seeded;
                attempt.donor_slot = Some(receipt.donor_slot.clone());
                attempt.units = Some(receipt.units);
                attempt.files = Some(receipt.files);
                attempt.logical_bytes = Some(receipt.logical_bytes);
                attempt.catalog_digest = Some(receipt.catalog_digest.clone());
                attempt.observation = Some(receipt.observation.clone());
                write_seed_attempt(&self.pool, &attempt)
                    .map_err(|error| lease_error("cannot record completed seed", &error))?;
                let mut buckets = std::collections::BTreeMap::new();
                let mut unknown = 0u32;
                for rejection in rejected {
                    if let Some(count) = rejection.units {
                        let current: &mut u32 = buckets.entry(rejection.reason).or_default();
                        *current = current
                            .checked_add(count)
                            .ok_or("seed rejection count overflow")?;
                    } else {
                        unknown = unknown
                            .checked_add(1)
                            .ok_or("seed rejection count overflow")?;
                    }
                }
                let last = SeedLast {
                    schema: "oxide.cold-seed/1".into(),
                    store_id: held.store_id.clone(),
                    slot_id: held.slot_id.clone(),
                    incarnation: held.incarnation.clone(),
                    lease_id: held.lease_id.clone(),
                    donor_slot: receipt.donor_slot.clone(),
                    units: receipt.units,
                    files: receipt.files,
                    logical_bytes: receipt.logical_bytes,
                    elapsed_ms: receipt.elapsed_ms,
                    when_ms: process::now_ms(),
                    buckets: buckets
                        .into_iter()
                        .filter(|(_, units)| *units > 0)
                        .map(|(reason, units)| SeedBucket { reason, units })
                        .collect(),
                    unknown,
                };
                write_seed_last(&self.pool, &last)
                    .map_err(|error| lease_error("cannot record last seed", &error))?;
                Ok(SeedRun::Seeded(receipt, last))
            }
            Err(error) => {
                attempt.state = SeedAttemptState::Reset;
                write_seed_attempt(&self.pool, &attempt)
                    .map_err(|error| lease_error("cannot record stopped seed", &error))?;
                if stopped {
                    let minimum =
                        minimum.map_or_else(|| "unknown".to_owned(), |bytes| bytes.to_string());
                    return match trigger {
                        Some(trigger) => Err(format!(
                            "seed reserve stop: minimum {minimum} bytes, trigger {trigger} bytes"
                        )),
                        None => Err(format!(
                            "seed volume stop: the removable volume under {} is gone or was remounted",
                            self.payload.display()
                        )),
                    };
                }
                if admission_stopped {
                    return Err("seed admission deadline expired".to_owned());
                }
                Err(lease_error("cannot seed slot", &error))
            }
        }
    }

    /// The seed's removable start gate: the registration read from this
    /// removable pool's paired internal control, re-verified for its
    /// mount/volume-UID binding and its free-space reserve (§8.3).
    fn seed_removable_gate(&self) -> Result<PathBuf, String> {
        let sibling = self
            .sibling
            .as_ref()
            .ok_or("the removable pool has no paired internal control")?;
        let registration = crate::removable::read_registration(&sibling.control)?
            .ok_or("the removable pool has no registration")?;
        crate::removable::verified_registration(&self.control, &registration)?;
        Ok(registration.mount_point)
    }

    /// Open the pool, creating it on first use above the host reserve. A
    /// macOS pool is excluded from Time Machine before any build writes into
    /// it; an unverified exclusion refuses before Cargo starts. An absent pool
    /// named by `OXIDE_POOL` refuses instead of being created silently unless
    /// `OXIDE_POOL_CREATE=1` or the path is inside the caller's task scratch
    /// root (oxide#182); the config file and the built-in default are
    /// unchanged.
    ///
    /// The debug-only `handoff_trace::phase` calls partition the guarded
    /// creation path for the installed-handoff proof; each wraps exactly one
    /// existing step and changes no statement, order, result or branch.
    pub fn open_or_create() -> Result<Self, String> {
        let settings = settings()?;
        let path = settings.pool.clone();
        let source = settings.pool_source;
        // A pasted `oxide status` annotation is not part of a path, so it
        // refuses for every pool state: an absent path and an existing
        // ordinary directory both get the hint instead of the generic open
        // error (oxide#182).
        config::pool_annotation_refused(&path, source)?;
        let scratch = crate::handoff_trace::phase("scratch-evaluate", || {
            crate::scratch_io::evaluate(&settings, false)
        })
        .map_err(|error| {
            if matches!(source, config::Source::Environment("OXIDE_POOL")) {
                format!("OXIDE_POOL names {}: {error}", path.display())
            } else {
                error
            }
        })?;
        match config::inspect(&path) {
            Found::Absent => {
                config::pool_creation_allowed(
                    &path,
                    source,
                    std::env::var_os("OXIDE_POOL_CREATE").as_deref(),
                    std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").as_deref(),
                )?;
                if let Some(uuid) = &scratch {
                    let parent = path.parent().ok_or("the pool path has no parent")?;
                    let available = free_bytes_for_creation(parent)
                        .ok_or("cannot sample host free space for pool creation")?;
                    if available < HOST_RESERVE_BYTES {
                        return Err("refusing to create the pool: host free space is below the 20 GiB reserve".into());
                    }
                    crate::handoff_trace::phase("scratch-create", || {
                        crate::scratch_io::create(&settings, uuid, admission_budget(&settings))
                    })?;
                } else {
                    create_pool(&path, admission_budget(&settings))?;
                }
            }
            Found::LegacyImage => return Err(legacy_refusal()),
            Found::Directory | Found::Unknown => {}
        }
        if let Some(uuid) = &scratch {
            crate::handoff_trace::phase("scratch-repair", || {
                crate::scratch_io::repair(&settings, uuid)
            })?;
        }
        let store =
            match crate::handoff_trace::phase("store-open", || Self::open_at(&path, &settings)) {
                Ok(store) => store,
                // An explicit path that exists but holds no recognizable Oxide
                // pool is still this command's pool: name it in the refusal
                // (oxide#182), so a typo or a non-pool directory is actionable.
                Err(error)
                    if matches!(
                        settings.pool_source,
                        config::Source::Environment("OXIDE_POOL")
                    ) =>
                {
                    return Err(format!("OXIDE_POOL names {}: {error}", path.display()))
                }
                Err(error) => return Err(error),
            };
        if !crate::handoff_trace::phase("sticky-exclusion", || {
            config::has_sticky_exclusion(&store.pool)
        }) {
            match crate::handoff_trace::phase("backup-exclude", || {
                config::exclude_from_backup(&store.pool)
            }) {
                Backup::Excluded | Backup::NotApplicable => {}
                Backup::NotExcluded(reason) => {
                    return Err(format!(
                        "refusing to build: the pool {} is not excluded from Time Machine ({reason}); Oxide excludes its own pool with `tmutil addexclusion` and builds only once `tmutil isexcluded` confirms it",
                        store.pool.display()
                    ))
                }
            }
        }
        if !matches!(settings.pool_source, config::Source::Environment(_)) {
            if let Err(error) = crate::handoff_trace::phase("remember-pool", || {
                config::remember_pool(&Facts::from_env(), &store.pool)
            }) {
                eprintln!("oxide: could not record the pool path for status: {error}");
            }
        }
        crate::handoff_trace::phase("ensure-control", || store.ensure_control())?;
        Ok(store)
    }

    /// Open the registered removable pool whose config and ledger live in
    /// `root` (internal) and whose payload is `payload` on the volume. The
    /// library verifies the volume on every open. The pool-creation
    /// environment does not apply to it.
    pub fn open_removable(
        root: &Path,
        payload: PathBuf,
        max_active: usize,
    ) -> Result<Self, String> {
        let pool = fs::canonicalize(root)
            .map_err(|error| format!("cannot open the removable pool: {error}"))?;
        let view = prepare_front_door_pool(&pool, "cannot open the removable pool")?;
        if Backend::recorded(&view.backend)? != Backend::ApfsVolume {
            return Err("the removable pool records another backend".to_owned());
        }
        let store = Self {
            control: pool.join(MANAGED_NAME),
            pool,
            backend: Backend::ApfsVolume,
            payload,
            budget: None,
            recorded_budget: view.cap_bytes,
            max_active,
            sibling: None,
        };
        store.ensure_control()?;
        Ok(store)
    }

    /// The private control directory, which holds the removable
    /// registration and pool of an internal pool.
    pub fn control(&self) -> &Path {
        &self.control
    }

    /// The internal pool's control directory. Owned-output receipts live
    /// there for every checkout, including a command whose slots come from
    /// the removable pool, so one checkout never has two receipts.
    pub fn owner_control(&self) -> &Path {
        match &self.sibling {
            Some(sibling) if sibling.barrier => &sibling.control,
            _ => &self.control,
        }
    }

    /// Open an existing pool for guarded recovery, repairing scratch identity.
    /// Never creates a pool; a retired image pool refuses without being touched.
    pub fn open_existing() -> Result<Option<Self>, String> {
        let settings = settings()?;
        Self::open_existing_mode(&settings, true)
    }

    /// Open an existing internal pool using settings already resolved for
    /// this command.
    pub fn open_existing_with(settings: &Settings) -> Result<Option<Self>, String> {
        Self::open_existing_mode(settings, false)
    }

    /// A guarded mutating operation repairs an absent scratch record.
    pub fn open_existing_for_clean(settings: &Settings) -> Result<Option<Self>, String> {
        Self::open_existing_mode(settings, true)
    }

    fn open_existing_mode(settings: &Settings, repair: bool) -> Result<Option<Self>, String> {
        if std::env::var_os("OXIDE_SCRATCH_ROOT").is_some() {
            crate::scratch_io::evaluate(settings, false)?;
        }
        match config::inspect(&settings.pool) {
            Found::Absent => Ok(None),
            Found::LegacyImage => Err(legacy_refusal()),
            Found::Directory | Found::Unknown => {
                if let Some(uuid) = crate::scratch_io::evaluate(settings, false)? {
                    if repair {
                        crate::scratch_io::repair(settings, &uuid)?;
                    }
                }
                Self::open_at(&settings.pool, settings).map(Some)
            }
        }
    }

    /// The existing internal pool's private control directory, without
    /// opening its ledger or reconciling any pool state.
    pub fn existing_control() -> Result<Option<PathBuf>, String> {
        let settings = settings()?;
        crate::scratch_io::evaluate(&settings, false)?;
        match config::inspect(&settings.pool) {
            Found::Absent => Ok(None),
            Found::LegacyImage => Err(legacy_refusal()),
            Found::Unknown => Err(format!(
                "cannot open the existing pool at {}: unrecognized pool",
                settings.pool.display()
            )),
            Found::Directory => {
                let pool = fs::canonicalize(&settings.pool)
                    .map_err(|error| format!("cannot resolve the existing pool: {error}"))?;
                let control = pool.join(MANAGED_NAME);
                verify_private_dir(&control)?;
                Ok(Some(control))
            }
        }
    }

    /// Open an existing internal pool for `oxide status` without its pool
    /// lock, reconciliation or any write (oxide#128). Refuses a retired,
    /// foreign or unsafe pool exactly as [`Store::open_existing_with`] does.
    pub fn open_recorded(settings: &Settings) -> Result<Option<Self>, String> {
        let scratch = crate::scratch_io::evaluate(settings, true)?;
        match config::inspect(&settings.pool) {
            Found::Absent => Ok(None),
            Found::LegacyImage => Err(legacy_refusal()),
            Found::Directory | Found::Unknown => {
                if scratch.is_none() {
                    config::refuse_unsafe_pool_path(&settings.pool)?;
                }
                Self::open_recorded_at(&settings.pool, settings).map(Some)
            }
        }
    }

    fn open_recorded_at(path: &Path, settings: &Settings) -> Result<Self, String> {
        let pool =
            fs::canonicalize(path).map_err(|error| format!("cannot open the pool: {error}"))?;
        let snapshot =
            snapshot(&pool).map_err(|error| lease_error("cannot open the pool", &error))?;
        let backend = Backend::recorded(&snapshot.backend)?;
        if backend != Backend::DirectoryBudget {
            return Err(format!(
                "the pool at {} records {}, not an internal pool",
                pool.display(),
                backend.tag()
            ));
        }
        let control = pool.join(MANAGED_NAME);
        Ok(Self {
            payload: snapshot.payload,
            pool,
            backend,
            control,
            budget: Some(admission_budget(settings)),
            recorded_budget: snapshot.cap_bytes,
            max_active: settings.active_build_limit.value,
            sibling: None,
        })
    }

    /// Open a recorded removable pool (its config, ledger and control live
    /// under the internal pool's control directory) without probing the
    /// volume. `root` is that pool directory.
    pub fn open_recorded_removable(root: &Path, max_active: usize) -> Result<Self, String> {
        let snapshot = snapshot(root)
            .map_err(|error| lease_error("cannot read the removable pool", &error))?;
        if Backend::recorded(&snapshot.backend)? != Backend::ApfsVolume {
            return Err(format!(
                "the removable pool at {} does not record a removable backend",
                root.display()
            ));
        }
        Ok(Self {
            payload: snapshot.payload,
            pool: root.to_path_buf(),
            backend: Backend::ApfsVolume,
            control: root.join(MANAGED_NAME),
            budget: None,
            recorded_budget: snapshot.cap_bytes,
            max_active,
            sibling: None,
        })
    }

    fn open_at(path: &Path, settings: &Settings) -> Result<Self, String> {
        let pool =
            fs::canonicalize(path).map_err(|error| format!("cannot open the pool: {error}"))?;
        let view = prepare_front_door_pool(&pool, "cannot open the pool")?;
        let backend = Backend::recorded(&view.backend)?;
        if backend != Backend::DirectoryBudget {
            return Err(format!(
                "the pool at {} records {}, not an internal pool",
                pool.display(),
                backend.tag()
            ));
        }
        let control = pool.join(MANAGED_NAME);
        Ok(Self {
            payload: pool.join("payload"),
            pool,
            backend,
            control,
            budget: Some(admission_budget(settings)),
            recorded_budget: view.cap_bytes,
            max_active: settings.active_build_limit.value,
            sibling: None,
        })
    }

    /// This command's soft admission budget, or the recorded one.
    pub fn budget(&self, recorded: u64) -> u64 {
        self.budget.unwrap_or(recorded)
    }

    /// The best-effort host-reserve watchdog for a supervised build. The
    /// internal pool has one; a removable pool checks its volume reserve
    /// when a build starts.
    pub fn watchdog(&self) -> Option<Watchdog> {
        match self.backend {
            Backend::ApfsVolume => self.payload.parent().map(Watchdog::for_volume),
            Backend::DirectoryBudget => Some(Watchdog::for_pool(&self.payload)),
        }
    }

    /// This command's soft admission budget.
    pub fn command_budget(&self) -> u64 {
        self.budget(self.recorded_budget)
    }

    /// The host-wide limit resolved for this command.
    pub fn max_active(&self) -> usize {
        self.max_active
    }

    /// Passive status using recorded slot observations.
    #[allow(dead_code)]
    pub fn status(&self) -> Result<PoolStatus, String> {
        pool_status(&self.pool).map_err(|error| lease_error("cannot read the pool", &error))
    }

    /// Ledger membership with no accounting walk, for selection, affinity,
    /// waits and recovery. It refuses where [`Store::status`] does.
    pub fn ledger(&self) -> Result<LedgerView, String> {
        ledger_view(&self.pool).map_err(|error| lease_error("cannot read the pool", &error))
    }

    /// Lease this checkout's slot, waiting in FIFO order. Ctrl-C while waiting
    /// cancels without touching any slot.
    /// `share` allows taking another checkout's idle compatible slot.
    /// `label` names the holder in `oxide status` (`exec <program>`).
    /// `admission` is this command's workspace class (design §5.1): `Normal`
    /// for every production command in this slice.
    pub fn acquire(
        &self,
        key: &SlotKey,
        profile: Profile,
        admission: WorkspaceAdmission<'_>,
        share: bool,
        label: Option<&str>,
        waits: &mut WaitAccounting,
    ) -> Result<Acquired, String> {
        let max_active = self.max_active;
        let admission_started = Instant::now();
        let admission_command = crate::admission::command_token();
        let mut reported_waiting = false;
        let ticket = self.register_ticket()?;
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
            let attempt = self.attempt(ticket.0, key, profile, max_active, admission, share, label);
            match attempt {
                Err(error) => return Err(error),
                Ok(Ok(lease)) => {
                    self.remove_ticket(ticket.0)?;
                    if crate::budget::admission_expired() {
                        // The window closed while this attempt ran: the lease
                        // goes straight back and no child starts.
                        let origin = lease.prior.unchanged();
                        self.release(lease, origin)?;
                        return Ok(Acquired::AdmissionExpired);
                    }
                    // Report this command's own admission wait to a supervising
                    // proof, best effort: unarmed or unwritable means the wait is
                    // simply not reported (oxide#192).
                    crate::admission::report_wait(&admission_command, admission_started.elapsed());
                    return Ok(Acquired::Lease(lease));
                }
                Ok(Err(wait)) => {
                    // One bounded report at the start of the wait, so a wait that
                    // never ends before a supervising proof stops is still
                    // classified; it grants nothing (oxide#192).
                    if !reported_waiting {
                        reported_waiting = true;
                        crate::admission::report_waiting(
                            &admission_command,
                            wait.category().label(),
                        );
                    }
                    waits.observe(&wait, max_active, Instant::now());
                    std::thread::sleep(WAIT_POLL);
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

    /// Release after every writer in the group has exited, recording the
    /// slot's compile origin (`None`: untrusted from now on).
    pub fn release(&self, held: Lease, origin: Option<Built>) -> Result<(), String> {
        let _admission =
            oxide_storage::release_trace::phase("admission lock", || self.release_admission())?;
        if !oxide_storage::release_trace::phase("temporary directory removal", || {
            self.discard_scratch(&held)
        }) || !oxide_storage::release_trace::phase("removable flush", || {
            self.flush_before_idle(&held.slot_id, held.record)
        })? {
            return Ok(());
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
    fn discard_scratch(&self, held: &Lease) -> bool {
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

    /// A removable-volume slot is flushed to the device before it can go
    /// idle; any other backend returns `true` at once. A failed or unknown
    /// flush retires the slot: the record becomes [`Phase::Retired`], the
    /// lease stays held, and `false` tells the caller not to release. This is
    /// a durability step after writer closure, not a closure proof. When the
    /// volume itself is gone the retirement also records [`Lost`], so the
    /// slot is deleted or quarantined once the volume returns. The caller
    /// holds the admission lock.
    fn flush_before_idle(&self, slot_id: &str, record: u64) -> Result<bool, String> {
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
    fn volume_present(&self) -> bool {
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
    fn discard_lost(&self, state: &mut State) -> Result<(), String> {
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

    /// The last flush of this pool, for the summary line.
    pub fn last_flush(&self) -> Option<Flush> {
        self.load().ok()?.flushes.pop()
    }

    /// Publish the owned-outputs downgrade fence in this pool's state and in
    /// its paired pool's (internal first), creating either state when absent
    /// so a pre-feature Oxide cannot start from a fresh unfenced one. Test
    /// fixtures only: production initialization is [`Store::initialize_outputs`].
    #[cfg(test)]
    pub fn fence_owned_outputs(&self) -> Result<(), String> {
        let mut controls = vec![self.owner_control().to_path_buf()];
        controls.extend(self.sibling.as_ref().map(|sibling| sibling.control.clone()));
        for control in controls {
            let store = Store {
                pool: self.pool.clone(),
                backend: self.backend,
                payload: self.payload.clone(),
                control,
                budget: self.budget,
                recorded_budget: self.recorded_budget,
                max_active: self.max_active,
                sibling: None,
            };
            store.ensure_control()?;
            let _admission = store.admission()?;
            let mut state = store.load()?;
            if state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
                if state
                    .owned_outputs
                    .as_deref()
                    .is_some_and(|fence| !predecessor_owned_outputs_fence(fence))
                {
                    return Err(format!(
                        "Oxide state in {} carries an unknown owned-outputs fence; refusing",
                        store.control.display()
                    ));
                }
                if state.outputs_init.is_some() {
                    return Err(format!(
                        "Oxide state in {} already records an output-registry transaction but not the successor fence; refusing",
                        store.control.display()
                    ));
                }
                state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
                store.save(&state)?;
            }
        }
        Ok(())
    }

    /// The recorded owned-outputs registry transaction, when initialization
    /// has begun. Read without a lock; callers that mutate recheck it under
    /// admission.
    pub fn outputs_initialization(&self) -> Result<Option<OutputsInit>, String> {
        #[cfg(test)]
        {
            let state = self.load()?;
            Ok(state.outputs_init)
        }
        #[cfg(not(test))]
        Ok(self.load()?.outputs_init)
    }

    /// Run the owned-outputs initialization protocol exactly once
    /// (oxide#129). The internal control is durable first
    /// (`initializing` + the successor `oxide.outputs/3` fence), then the
    /// paired removable control, then `create(transaction)` creates and syncs
    /// the registry,
    /// then both controls record `ready`. A crash resumes only the recorded
    /// transaction; an unknown or contradicting record refuses before any
    /// registry effect.
    ///
    /// Initialization (and resuming an `initializing` record) first requires
    /// positive pool-wide writer closure under both admission locks, held
    /// together: no active or held command record and no lost slot with
    /// unclosed writers in either pool, and no clean barrier except the
    /// caller's own `own_checkout`. An older binary's in-flight command is a
    /// record in the same state, so it must close before the fence is
    /// published.
    pub fn initialize_outputs(
        &self,
        transaction: &str,
        own_checkout: Option<&Path>,
        create: &dyn Fn(&str, bool) -> Result<(), String>,
    ) -> Result<bool, String> {
        if !bounded_text(transaction, MAX_INIT_TRANSACTION_BYTES) {
            return Err("the output-registry transaction identity is unusable".to_owned());
        }
        let internal_control = self.owner_control().to_path_buf();
        let internal = self.view(&internal_control);
        internal.ensure_control()?;
        let _internal_admission = internal.control_admission()?;
        let mut internal_state = internal.load()?;
        let mut fresh = false;
        let transaction = match internal_state.outputs_init.clone() {
            Some(OutputsInit::Ready {
                transaction: recorded,
                ..
            }) => {
                if internal_state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
                    // The published whole-target fence is upgraded to the
                    // successor only with positive writer closure across both
                    // pools, internal fence first (oxide#133).
                    self.upgrade_outputs_fence(own_checkout, &recorded)?;
                    internal_state = internal.load()?;
                    if internal_state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
                        return Err(
                            "the internal control records a ready output registry without the successor fence; refusing"
                                .to_owned(),
                        );
                    }
                }
                // A new or reappearing paired control is fenced and marked
                // ready under the exact recorded transaction before any
                // effect; a different or unknown record refuses.
                let sibling = self
                    .sibling
                    .as_ref()
                    .filter(|sibling| sibling.control != internal_control)
                    .map(|sibling| (self.view(&sibling.control), sibling.barrier));
                if let Some((sibling, _barrier)) = &sibling {
                    sibling.ensure_control()?;
                    let _admission = sibling.control_admission()?;
                    let mut state = sibling.load()?;
                    match &state.outputs_init {
                        Some(OutputsInit::Ready { transaction, .. })
                            if transaction == &recorded =>
                        {
                            if state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
                                if state
                                    .owned_outputs
                                    .as_deref()
                                    .is_some_and(|fence| !predecessor_owned_outputs_fence(fence))
                                {
                                    return Err(
                                            "the paired pool carries an unknown owned-outputs fence; refusing"
                                                .to_owned(),
                                        );
                                }
                                // The internal control may already carry the
                                // successor fence (a crash between the two
                                // saves), so this paired upgrade is the only
                                // remaining closure check: it must hold here
                                // too, not only in the first upgrade.
                                writers_closed(&state, "the paired", own_checkout)?;
                                state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
                                sibling.save_for_writer(&state)?;
                            }
                        }
                        Some(OutputsInit::Ready { .. }) => return Err(
                            "the paired pool records a different ready output registry; refusing"
                                .to_owned(),
                        ),
                        Some(OutputsInit::Initializing { transaction, .. }) => {
                            if transaction != &recorded {
                                return Err(format!(
                                    "the paired pool records initializer transaction {transaction}, not the recorded {recorded}; refusing"
                                ));
                            }
                            fence_paired(sibling, &mut state, &recorded, own_checkout)?;
                        }
                        None => {
                            // Parity with the `Ready` arm above: the released
                            // `/1` fence is a legitimate predecessor here too,
                            // not an unknown value (oxide#159).
                            if state
                                .owned_outputs
                                .as_deref()
                                .is_some_and(|fence| !predecessor_owned_outputs_fence(fence))
                            {
                                return Err(
                                    "the paired pool carries an unknown owned-outputs fence; refusing"
                                        .to_owned(),
                                );
                            }
                            // A new or reappearing control is fenced only
                            // with its own writers positively closed.
                            fence_paired(sibling, &mut state, &recorded, own_checkout)?;
                        }
                    }
                    if state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
                        return Err(
                            "the paired pool does not carry the successor output fence; refusing"
                                .to_owned(),
                        );
                    }
                }
                // The registry must already exist: this path is validation
                // only and must never create or rebuild it.
                create(&recorded, false)?;
                return Ok(true);
            }
            Some(OutputsInit::Initializing { transaction, .. }) => transaction,
            None => {
                fresh = true;
                transaction.to_owned()
            }
        };
        // Both admission locks are held together (internal first) from the
        // closure check through both `initializing` records.
        let sibling = self
            .sibling
            .as_ref()
            .filter(|sibling| sibling.control != internal_control)
            .map(|sibling| (self.view(&sibling.control), sibling.barrier));
        let mut sibling_admission = None;
        if let Some((sibling, _barrier)) = &sibling {
            sibling.ensure_control()?;
            sibling_admission = Some(sibling.control_admission()?);
            writers_closed(&sibling.load()?, "the paired", own_checkout)?;
        }
        writers_closed(&internal_state, "the internal", own_checkout)?;
        if !fresh && internal_state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
            internal_state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
            internal.save_for_writer(&internal_state)?;
        }
        if fresh {
            internal_state.outputs_init = Some(OutputsInit::Initializing {
                transaction: transaction.clone(),
                started_ms: process::now_ms(),
            });
            internal_state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
            internal.save_for_writer(&internal_state)?;
        }
        internal.fence_admission_state()?;
        // Internal-first: the paired removable control is fenced only after
        // the internal record is durable.
        if let Some((sibling, _barrier)) = &sibling {
            let _admission = sibling_admission.take();
            let mut state = sibling.load()?;
            match &state.outputs_init {
                Some(OutputsInit::Ready { transaction: ready, .. }) if ready == &transaction => {}
                Some(OutputsInit::Ready { .. }) => {
                    return Err(
                        "the paired pool records a different ready output registry; refusing"
                            .to_owned(),
                    )
                }
                Some(OutputsInit::Initializing { transaction: other, .. })
                    if other != &transaction =>
                {
                    return Err(format!(
                        "the paired pool records initializer transaction {other}, not {transaction}; refusing"
                    ))
                }
                Some(OutputsInit::Initializing { .. }) => {}
                None => {
                    state.outputs_init = Some(OutputsInit::Initializing {
                        transaction: transaction.clone(),
                        started_ms: process::now_ms(),
                    });
                    state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
                    sibling.save_for_writer(&state)?;
                }
            }
            if state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
                return Err(
                    "the paired pool does not carry the successor output fence; refusing"
                        .to_owned(),
                );
            }
            sibling.fence_admission_state()?;
        }
        create(&transaction, true)?;
        if let Some(OutputsInit::Initializing { .. }) = internal_state.outputs_init {
            internal_state.outputs_init = Some(OutputsInit::Ready {
                transaction: transaction.clone(),
                ready_ms: process::now_ms(),
            });
            internal.save_for_writer(&internal_state)?;
        }
        if let Some((sibling, _barrier)) = &sibling {
            let _admission = sibling.control_admission()?;
            let mut state = sibling.load()?;
            if !matches!(
                &state.outputs_init,
                Some(OutputsInit::Ready { transaction: ready, .. }) if ready == &transaction
            ) {
                state.outputs_init = Some(OutputsInit::Ready {
                    transaction: transaction.clone(),
                    ready_ms: process::now_ms(),
                });
                sibling.save_for_writer(&state)?;
            }
        }
        Ok(true)
    }

    /// Upgrade the published whole-target fence to the successor, internal
    /// control first (oxide#133). The caller holds the internal admission and
    /// is inside its writer-closure window; this takes the paired pool's
    /// admission so both are held together, and a missing, unreadable or
    /// contradicting authority refuses and changes nothing.
    fn upgrade_outputs_fence(
        &self,
        own_checkout: Option<&Path>,
        transaction: &str,
    ) -> Result<(), String> {
        let internal_control = self.owner_control().to_path_buf();
        let internal = self.view(&internal_control);
        let mut internal_state = internal.load()?;
        if internal_state.owned_outputs.as_deref() == Some(OWNED_OUTPUTS_FENCE) {
            return Ok(());
        }
        match internal_state.owned_outputs.as_deref() {
            Some(fence) if predecessor_owned_outputs_fence(fence) => {}
            None => {
                return Err(
                    "the internal control records a ready output registry without an owned-outputs fence; refusing"
                        .to_owned(),
                )
            }
            Some(_) => {
                return Err(
                    "the internal control carries an unknown owned-outputs fence; refusing"
                        .to_owned(),
                )
            }
        }
        if !matches!(
            &internal_state.outputs_init,
            Some(OutputsInit::Ready { transaction: ready, .. }) if ready == transaction
        ) {
            return Err(
                "the recorded output-registry transaction changed while upgrading the fence; refusing"
                    .to_owned(),
            );
        }
        writers_closed(&internal_state, "the internal", own_checkout)?;
        let sibling = self
            .sibling
            .as_ref()
            .filter(|sibling| sibling.control != internal_control)
            .map(|sibling| (self.view(&sibling.control), sibling.barrier));
        let mut sibling_admission = None;
        if let Some((sibling, _barrier)) = &sibling {
            sibling.ensure_control()?;
            sibling_admission = Some(sibling.control_admission()?);
            let state = sibling.load()?;
            writers_closed(&state, "the paired", own_checkout)?;
            match state.outputs_init.as_ref() {
                Some(OutputsInit::Ready {
                    transaction: ready, ..
                }) if ready == transaction => {}
                Some(_) => {
                    return Err(
                        "the paired pool records a different output-registry transaction; refusing"
                            .to_owned(),
                    )
                }
                None => match state.owned_outputs.as_deref() {
                    None => {}
                    Some(fence) if predecessor_owned_outputs_fence(fence) => {}
                    Some(_) => {
                        return Err(
                            "the paired pool carries an unknown owned-outputs fence; refusing"
                                .to_owned(),
                        )
                    }
                },
            }
        }
        internal_state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
        internal.save_for_writer(&internal_state)?;
        if let Some((sibling, _barrier)) = &sibling {
            let _admission = sibling_admission.take();
            let mut state = sibling.load()?;
            state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
            if state.outputs_init.is_none() {
                state.outputs_init = Some(OutputsInit::Ready {
                    transaction: transaction.to_owned(),
                    ready_ms: process::now_ms(),
                });
            }
            sibling.save_for_writer(&state)?;
        }
        Ok(())
    }

    fn view(&self, control: &Path) -> Store {
        Store {
            pool: self.pool.clone(),
            backend: self.backend,
            payload: self.payload.clone(),
            control: control.to_path_buf(),
            budget: self.budget,
            recorded_budget: self.recorded_budget,
            max_active: self.max_active,
            sibling: None,
        }
    }

    /// A store over `root` for other modules' unit tests.
    #[cfg(test)]
    pub fn for_tests(root: &Path) -> Store {
        fs::create_dir_all(root).unwrap();
        let store = Store {
            pool: root.to_path_buf(),
            backend: Backend::DirectoryBudget,
            payload: root.join("payload"),
            control: root.join(MANAGED_NAME),
            budget: None,
            recorded_budget: 1,
            max_active: 2,
            sibling: None,
        };
        store.ensure_control().unwrap();
        store
    }

    /// Test-only: an independent handle to the same pool, so a test can hand
    /// one to a verb that takes the pool by value and still observe the pool.
    #[cfg(test)]
    pub fn handle_for_tests(&self) -> Store {
        Store {
            pool: self.pool.clone(),
            backend: self.backend,
            payload: self.payload.clone(),
            control: self.control.clone(),
            budget: self.budget,
            recorded_budget: self.recorded_budget,
            max_active: self.max_active,
            sibling: self.sibling.as_ref().map(|sibling| Sibling {
                control: sibling.control.clone(),
                barrier: sibling.barrier,
            }),
        }
    }

    /// Record a lost slot whose writers were never confirmed closed, for
    /// other modules' unit tests of unknown custody.
    #[cfg(test)]
    pub fn quarantine_for_tests(&self, checkout: &Path) {
        let mut state = self.load().unwrap();
        let id = state.take_id().unwrap();
        let lost = Lost {
            slot_id: "1".to_owned(),
            at_ms: 1,
            unclosed: Some("a test writer was never confirmed closed".to_owned()),
        };
        state.records.push(Record {
            id,
            identity: "identity".to_owned(),
            compatibility: "compatibility".to_owned(),
            profile: "quick".to_owned(),
            checkout: Some(checkout.to_path_buf()),
            phase: Phase::Retired,
            process_group: None,
            last_used: 1,
            holder: None,
            lease_id: None,
            pending_recovery: None,
            retired: Some(lost.describe(false)),
            lost: Some(lost),
            last_used_ms: Some(1),
            class: WorkspaceClass::Normal,
            class_reason: None,
        });
        self.save(&state).unwrap();
    }

    /// Pair this store with the other pool's control directory. `barrier`
    /// is set for the removable store (see [`Sibling`]). The other pool's
    /// state is replaced atomically and read without its own lock; for the
    /// host-wide limit, a command starting in the other pool at the same
    /// moment may briefly exceed it by one.
    pub fn with_sibling(mut self, control: &Path, barrier: bool) -> Self {
        self.sibling = Some(Sibling {
            control: control.to_path_buf(),
            barrier,
        });
        self
    }

    /// The other pool's state with its failures distinguished from absence.
    /// `Ok(None)` means there is no state document at all (so no records);
    /// every other problem — a symlinked or unreadable document, an
    /// inaccessible ancestor, an invalid document — is an error. Callers map
    /// that error to unknown custody (or refuse), never to clear. `exists()` is
    /// never used for this, because it maps every metadata error to `false`.
    fn checked_sibling_state(&self) -> Result<Option<State>, String> {
        let Some(sibling) = self.sibling.as_ref() else {
            return Ok(None);
        };
        // The control directory itself is inspected first: a genuinely absent
        // one means the other pool has no state at all, while a symlinked,
        // dangling, foreign or unreadable one is unknown custody. Reading
        // `state.json` alone would map a dangling control directory to ENOENT
        // and so to "no records" — exactly the direction this must not take.
        match fs::symlink_metadata(&sibling.control) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "the other pool's control directory cannot be inspected: {error}"
                ))
            }
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err("the other pool's control path is not a real directory".to_owned())
            }
            Ok(_) => {}
        }
        read_state(&sibling.control.join("state.json"))
            .map_err(|error| format!("the other pool's record state is unreadable: {error}"))
    }

    /// The number of active command records in the other pool. An unreadable
    /// state is an error, so the caller refuses rather than admitting over it.
    fn sibling_active(&self) -> Result<usize, String> {
        Ok(self.checked_sibling_state()?.map_or(0, |state| {
            state
                .records
                .iter()
                .filter(|record| record.active())
                .count()
        }))
    }

    /// Whether the other pool holds an active command for `checkout`
    /// (internal clean checks the removable pool's commands). An unreadable
    /// state is an error, never "not busy".
    fn sibling_busy(&self, checkout: &Path) -> Result<bool, String> {
        Ok(self.checked_sibling_state()?.is_some_and(|state| {
            state
                .records
                .iter()
                .any(|record| record.checkout.as_deref() == Some(checkout) && record.active())
        }))
    }

    /// A per-checkout binding mutex: not state-dependent, so two commands can
    /// never wait on each other through it. It serialises only the short
    /// window in which a command validates or replaces its checkout's owned
    /// output link and starts Cargo. The file lives in the internal pool's
    /// control directory, so both pools share it.
    pub fn lock_binding(&self, checkout: &Path) -> Result<File, String> {
        let directory = self.owner_control().join("bindings");
        ensure_private_dir(&directory)?;
        let id = identity::digest(&checkout.to_string_lossy());
        let file = private_file(&directory.join(format!("{id}.lock")), true)?
            .ok_or("the checkout binding lock is missing")?;
        crate::budget::lock(&file, "checkout binding")?;
        Ok(file)
    }

    /// One checkout's cooperative custody for an output transition or a clean.
    /// A command that may be writing counts while it is spawning or running; a
    /// record that has not started Cargo cannot write through the link and
    /// binds again after a transition. A retired slot whose lost writers were
    /// never positively closed is unknown custody, not activity: it blocks
    /// every relink and clean until it is resolved.
    pub fn checkout_custody(&self, checkout: &Path, own: u64) -> Result<CheckoutCustody, String> {
        let _sibling = self.sibling_admission()?;
        let _admission = self.admission()?;
        let own_state = if self.control.exists() {
            Some(self.load()?)
        } else {
            None
        };
        // A sibling with no state file has no records; any failure to read or
        // parse one is unknown custody, never "clear".
        let other_state = match self.checked_sibling_state() {
            Ok(state) => state,
            Err(reason) => return Ok(CheckoutCustody::Unknown(reason)),
        };
        let boot = boot_session();
        let pools = [
            (own_state.as_ref(), "this", Some(own)),
            (other_state.as_ref(), "the other", None),
        ];
        Ok(custody_verdict(&pools, checkout, boot))
    }

    /// The sibling pool's admission lock, when this store is the removable
    /// one. Always internal first, as slot attempts and cleans do.
    fn sibling_admission(&self) -> Result<Option<File>, String> {
        let Some(sibling) = self.sibling.as_ref().filter(|sibling| sibling.barrier) else {
            return Ok(None);
        };
        let file = private_file(&sibling.control.join("admission.lock"), true)?
            .ok_or("the internal admission lock is missing")?;
        crate::budget::lock(&file, "internal admission")?;
        Ok(Some(file))
    }

    /// The internal pool's admission lock, for the removable store's slot
    /// attempts, plus whether a live internal clean covers `checkout`. A
    /// clean mark counts while its lock is held or its Cargo group lives.
    fn sibling_barrier(&self, checkout: &Path) -> Result<(Option<File>, bool), String> {
        let Some(sibling) = self.sibling.as_ref().filter(|sibling| sibling.barrier) else {
            return Ok((None, false));
        };
        let file = private_file(&sibling.control.join("admission.lock"), true)?
            .ok_or("the internal admission lock is missing")?;
        crate::budget::lock(&file, "internal admission")?;
        let cleaning = match self.checked_sibling_state()? {
            None => false,
            Some(state) => {
                let mut live = false;
                for clean in state
                    .cleans
                    .iter()
                    .filter(|clean| clean.checkout == checkout)
                {
                    let path = sibling
                        .control
                        .join("waiters")
                        .join(format!("{}.lock", clean.id));
                    live |= match probe_lock(&path, false)? {
                        Probe::Held => true,
                        Probe::Free(_) => clean
                            .process_group
                            .is_some_and(|group| !process::group_empty(group)),
                        Probe::Missing => false,
                    };
                }
                live
            }
        };
        Ok((Some(file), cleaning))
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

    /// Wait until no live or unsettled command for this checkout holds a
    /// record, then mark the checkout as cleaning. Ctrl-C while waiting
    /// cancels. This is [`CleanMode::Recovering`], the shipped order, and
    /// every pre-one-use caller keeps it byte for byte.
    pub fn begin_clean(
        &self,
        checkout: &Path,
        waits: &mut WaitAccounting,
    ) -> Result<Cleaning, String> {
        self.begin_clean_recovering(checkout, waits)
    }

    /// One `Barrier`-mode attempt (one-use S1, design §4.1 steps 3-5).
    ///
    /// `revalidate` runs **under the caller's checkout binding mutex and
    /// before any mark exists**; its error is the guard's own refusal and is
    /// returned unchanged with nothing written. This path never calls
    /// `drop_dead_tickets`, `recover_settled`, `ensure_control` or
    /// the writer fence, and it takes every lock it needs
    /// **non-blocking**: a refusal or a retry writes no state, allocates no
    /// id and creates no `waiters/` file, and holding the binding mutex across
    /// this call can never deadlock with a shipped verb that holds the barrier
    /// and then wants the binding mutex.
    ///
    /// The busy facts are classified rather than acted on:
    ///
    /// - **dead** (a free or missing waiter/record lock with no live group) —
    ///   [`Cleaning::Busy`] with the bounded non-progress diagnostic. A dead
    ///   fact refuses even beside a live one: it cannot clear itself without
    ///   the shipped recovery, so no amount of waiting makes progress.
    /// - **live** (a held waiter or record lock, or a recorded non-empty
    ///   group) — [`Cleaning::Retry`], because the holder can progress.
    /// - **unknown** (a probe or state read error, or an unreadable sibling
    ///   state) — a refusal; unknown custody is protected.
    ///
    /// The caller owns the bounded attempt budget and the poll between
    /// attempts (`crate::store::barrier_attempts`).
    pub fn begin_clean_barrier<F>(
        &self,
        checkout: &Path,
        waits: &mut WaitAccounting,
        revalidate: F,
    ) -> Result<Cleaning, String>
    where
        F: FnMut() -> Result<(), String>,
    {
        self.begin_clean_with(checkout, waits, CleanMode::Barrier, revalidate)
    }

    /// The one barrier seam (one-use S1, design §4.1): `Recovering` is the
    /// shipped behaviour for every existing caller and ignores `revalidate`;
    /// `Barrier` is the guarded verbs' non-recovering, classified,
    /// non-blocking attempt. Keeping one entry point makes the mode choice
    /// explicit at the call site instead of implicit in which method is named.
    pub fn begin_clean_with<F>(
        &self,
        checkout: &Path,
        waits: &mut WaitAccounting,
        mode: CleanMode,
        mut revalidate: F,
    ) -> Result<Cleaning, String>
    where
        F: FnMut() -> Result<(), String>,
    {
        if mode == CleanMode::Recovering {
            return self.begin_clean_recovering(checkout, waits);
        }
        // This path creates no control tree: with no pool state there is no
        // barrier to take, and creating one would be an effect before the
        // guarded verb's own transition. `is_dir()` would map an unreadable
        // directory to "absent", so the metadata is inspected instead: only
        // `NotFound` is absence, anything else is unknown custody.
        match fs::symlink_metadata(&self.control) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(crate::one_use::barrier_unknown(
                    "this pool has no control state to take a barrier over",
                ))
            }
            Err(error) => {
                return Err(crate::one_use::barrier_unknown(&format!(
                    "the pool's control directory cannot be inspected: {error}"
                )))
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(crate::one_use::barrier_unknown(
                    "the pool's control path is not a directory",
                ))
            }
            Ok(_) => {}
        }
        if let Some(signal) = process::received_signal() {
            waits.stop(Instant::now());
            return Ok(Cleaning::Interrupted(signal));
        }
        revalidate()?;
        // Phase 1 — the refusal decision, with **nothing created**. The
        // admission locks are only *probed*: a guarded verb never creates one,
        // so no refusal can leave a new `admission.lock` behind (design §4.1
        // step 4 allows only the checkout binding mutex file). A pool with no
        // admission lock has never had a controller enter its critical section;
        // the verb cannot create one, so it retries.
        let sibling = self.sibling.as_ref().filter(|sibling| sibling.barrier);
        if let Some(sibling) = sibling {
            match probe_admission(&sibling.control)
                .map_err(|error| crate::one_use::barrier_unknown(&error))?
            {
                AdmissionProbe::Free => {}
                AdmissionProbe::Held => {
                    return Ok(Cleaning::Retry(
                        "another oxide command holds the internal admission lock".to_owned(),
                    ))
                }
                AdmissionProbe::Absent => {
                    return Ok(Cleaning::Retry(
                        "the internal pool has no admission lock to take a barrier under"
                            .to_owned(),
                    ))
                }
            }
        }
        match probe_admission(&self.control)
            .map_err(|error| crate::one_use::barrier_unknown(&error))?
        {
            AdmissionProbe::Free => {}
            AdmissionProbe::Held => {
                return Ok(Cleaning::Retry(
                    "another oxide command holds this pool's admission lock".to_owned(),
                ))
            }
            AdmissionProbe::Absent => {
                return Ok(Cleaning::Retry(
                    "this pool has no admission lock to take a barrier under".to_owned(),
                ))
            }
        }
        match self.barrier_decision(checkout)? {
            BarrierState::Dead(what) => {
                return Ok(Cleaning::Busy(crate::one_use::barrier_dead(&what)))
            }
            BarrierState::Live(reason) => return Ok(Cleaning::Retry(reason)),
            BarrierState::Clear => {}
        }
        // Phase 2 — take the **existing** admission locks (opened, never
        // created) and decide again under them. The caller's revalidation runs
        // once more here, still under the checkout binding mutex, so a
        // transition that appeared between the two decisions is refused before
        // a mark exists. Neither decision can create anything, so a non-clear
        // second answer leaves the pool's files exactly as they were.
        let _sibling = match sibling {
            Some(sibling) => match acquire_existing_admission(&sibling.control)
                .map_err(|error| crate::one_use::barrier_unknown(&error))?
            {
                Some(lock) => Some(lock),
                None => {
                    return Ok(Cleaning::Retry(
                        "another oxide command holds the internal admission lock".to_owned(),
                    ))
                }
            },
            None => None,
        };
        let Some(_admission) = acquire_existing_admission(&self.control)
            .map_err(|error| crate::one_use::barrier_unknown(&error))?
        else {
            return Ok(Cleaning::Retry(
                "another oxide command holds this pool's admission lock".to_owned(),
            ));
        };
        revalidate()?;
        let mut state = self
            .load()
            .map_err(|error| crate::one_use::barrier_unknown(&error))?;
        match self
            .barrier_state(&state, checkout)
            .map_err(|error| crate::one_use::barrier_unknown(&error))?
        {
            BarrierState::Dead(what) => Ok(Cleaning::Busy(crate::one_use::barrier_dead(&what))),
            BarrierState::Live(reason) => Ok(Cleaning::Retry(reason)),
            BarrierState::Clear => {
                if state.cleans.len() >= MAX_TICKETS {
                    return Err(format!("too many concurrent oxide cleans ({MAX_TICKETS})"));
                }
                if state.minimum_writer_generation != Some(WRITER_GENERATION) {
                    publish_writer_generation(&self.control)?;
                    state = self.load()?;
                }
                let id = state.take_id()?;
                let Probe::Free(lock) = probe_lock(&self.ticket_path(id), true)? else {
                    return Err("a new Oxide clean lock is unexpectedly held".to_owned());
                };
                state.cleans.push(Clean {
                    id,
                    checkout: checkout.to_path_buf(),
                    process_group: None,
                });
                self.save(&state)?;
                waits.stop(Instant::now());
                waits.print_completion("clean barrier acquired");
                Ok(Cleaning::Guard(CleanGuard { id, lock }))
            }
        }
    }

    /// The guarded barrier's decision for one checkout, read without taking any
    /// lock: the state document plus the classified busy facts. Unknown custody
    /// (an unreadable state or a probe error) refuses with the bounded guarded
    /// text rather than a raw error.
    fn barrier_decision(&self, checkout: &Path) -> Result<BarrierState, String> {
        let state = self
            .load()
            .map_err(|error| crate::one_use::barrier_unknown(&error))?;
        self.barrier_state(&state, checkout)
            .map_err(|error| crate::one_use::barrier_unknown(&error))
    }

    /// The busy classification of one checkout for [`CleanMode::Barrier`]
    /// (design §4.1 step 5). Any **dead** fact outranks a live one: a free
    /// waiter or record lock never clears itself without the shipped recovery,
    /// so waiting for a live holder could not make progress.
    fn barrier_state(&self, state: &State, checkout: &Path) -> Result<BarrierState, String> {
        let mut live: Option<String> = None;
        let mut dead: Option<String> = None;
        for clean in state
            .cleans
            .iter()
            .filter(|clean| clean.checkout == checkout)
        {
            match probe_lock(&self.ticket_path(clean.id), false)? {
                Probe::Held => {
                    live.get_or_insert_with(|| {
                        format!("a clean of this checkout (mark {}) is running", clean.id)
                    });
                }
                Probe::Free(_) | Probe::Missing => {
                    if clean
                        .process_group
                        .is_some_and(|group| !process::group_empty(group))
                    {
                        live.get_or_insert_with(|| {
                            format!(
                                "a clean of this checkout (mark {}) still has a live recorded group",
                                clean.id
                            )
                        });
                    } else {
                        dead.get_or_insert_with(|| {
                            format!(
                                "the clean mark {} for this checkout has a free lock and no live group",
                                clean.id
                            )
                        });
                    }
                }
            }
        }
        self.barrier_records(state, checkout, &self.control, &mut live, &mut dead)?;
        if let Some(sibling) = self.sibling.as_ref() {
            // The sibling's state is read with its failures kept apart from
            // absence: a missing document means no records, while any other
            // failure is unknown custody, never "clear".
            match self.checked_sibling_state()? {
                Some(other) => {
                    self.barrier_records(&other, checkout, &sibling.control, &mut live, &mut dead)?
                }
                None => {}
            }
        }
        // A dead fact outranks a live one: waiting for a holder that can
        // progress would still end on the dead fact, which never clears itself
        // without the shipped recovery, so the guarded verb refuses now.
        Ok(match (dead, live) {
            (Some(what), _) => BarrierState::Dead(what),
            (None, Some(reason)) => BarrierState::Live(reason),
            (None, None) => BarrierState::Clear,
        })
    }

    /// The records of one pool that hold `checkout`, classified by whether
    /// their lock is still held.
    fn barrier_records(
        &self,
        state: &State,
        checkout: &Path,
        control: &Path,
        live: &mut Option<String>,
        dead: &mut Option<String>,
    ) -> Result<(), String> {
        for record in state
            .records
            .iter()
            .filter(|record| record.checkout.as_deref() == Some(checkout) && record.active())
        {
            let path = control.join("locks").join(format!("{}.lock", record.id));
            match probe_lock(&path, false)? {
                Probe::Held => {
                    live.get_or_insert_with(|| {
                        format!(
                            "a command (record {}) is active in this checkout",
                            record.id
                        )
                    });
                }
                Probe::Free(_) | Probe::Missing => {
                    // A free lock is not by itself a dead fact: the design's
                    // dead row also requires *no live group*. A recorded group
                    // that still exists may still write through this slot, so
                    // it is a live fact the verb retries, and the shipped
                    // recovery reclaims the record once that group closes.
                    match record.process_group {
                        Some(group) if !process::group_empty(group) => {
                            live.get_or_insert_with(|| {
                                format!(
                                    "the record {} for this checkout has a free lock but its recorded group {group} is still alive",
                                    record.id
                                )
                            });
                        }
                        _ => {
                            dead.get_or_insert_with(|| {
                                format!(
                                    "the record {} for this checkout has a free lock and no live group",
                                    record.id
                                )
                            });
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn begin_clean_recovering(
        &self,
        checkout: &Path,
        waits: &mut WaitAccounting,
    ) -> Result<Cleaning, String> {
        self.ensure_control()?;
        loop {
            if let Some(signal) = process::received_signal() {
                waits.stop(Instant::now());
                return Ok(Cleaning::Interrupted(signal));
            }
            let attempt = (|| {
                // A removable store's barrier covers both pools, so it takes
                // the internal admission lock first (the same order slot
                // attempts and cleans use) and checks the sibling's live state
                // under it.
                let _sibling = self.sibling_admission()?;
                let _admission = self.admission()?;
                let mut state = self.load()?;
                self.drop_dead_tickets(&mut state)?;
                self.recover_settled(&mut state)?;
                let busy = state.cleans.iter().any(|clean| clean.checkout == checkout)
                    || state.records.iter().any(|record| {
                        record.checkout.as_deref() == Some(checkout) && record.active()
                    })
                    || self.sibling_busy(checkout)?;
                if !busy {
                    if state.cleans.len() >= MAX_TICKETS {
                        return Err(format!("too many concurrent oxide cleans ({MAX_TICKETS})"));
                    }
                    let id = state.take_id()?;
                    let Probe::Free(lock) = probe_lock(&self.ticket_path(id), true)? else {
                        return Err("a new Oxide clean lock is unexpectedly held".to_owned());
                    };
                    state.cleans.push(Clean {
                        id,
                        checkout: checkout.to_path_buf(),
                        process_group: None,
                    });
                    self.save(&state)?;
                    return Ok(Some(CleanGuard { id, lock }));
                }
                self.save(&state)?;
                Ok(None)
            })();
            match attempt {
                Err(error) => {
                    waits.stop(Instant::now());
                    return Err(error);
                }
                Ok(Some(guard)) => {
                    waits.stop(Instant::now());
                    waits.print_completion("clean barrier acquired");
                    return Ok(Cleaning::Guard(guard));
                }
                Ok(None) => {
                    waits.observe_with_reason(
                        &Wait::Clean,
                        Instant::now(),
                        "active oxide commands in this checkout",
                    );
                    std::thread::sleep(WAIT_POLL);
                }
            }
        }
    }

    /// Record the `cargo clean` process group for crash checks.
    pub fn clean_running(&self, guard: &CleanGuard, group: i32) -> Result<(), String> {
        let _admission = self.admission()?;
        let mut state = self.load()?;
        if let Some(clean) = state.cleans.iter_mut().find(|clean| clean.id == guard.id) {
            clean.process_group = Some(group);
        }
        self.save(&state)
    }

    /// Drop this checkout's slot affinity and end the clean. Slots stay intact.
    pub fn finish_clean(&self, guard: CleanGuard, checkout: &Path) -> Result<usize, String> {
        let _admission = self.admission()?;
        let mut state = self.load()?;
        let mut detached = 0;
        for record in &mut state.records {
            if record.checkout.as_deref() == Some(checkout)
                && matches!(record.phase, Phase::Idle | Phase::Retired)
            {
                record.checkout = None;
                detached += 1;
            }
        }
        state.cleans.retain(|clean| clean.id != guard.id);
        self.save(&state)?;
        let _ = fs::remove_file(self.ticket_path(guard.id));
        Ok(detached)
    }

    /// End a refused or failed clean without touching affinity.
    pub fn abandon_clean(&self, guard: CleanGuard) -> Result<(), String> {
        let _admission = self.admission()?;
        let mut state = self.load()?;
        state.cleans.retain(|clean| clean.id != guard.id);
        self.save(&state)?;
        let _ = fs::remove_file(self.ticket_path(guard.id));
        Ok(())
    }

    /// The recorded (or measured) view of this pool for `oxide status`.
    /// Recorded mode takes no pool lock, reconciles nothing, walks nothing
    /// and writes nothing: it reads the ledger, the control state and the
    /// optional diagnostic sidecar. Measured mode reads the same recorded
    /// facts plus the existing bounded walks, giving `max_elapsed_ms` as this
    /// pool's share of one cumulative budget, and still reconciles nothing.
    pub fn report(&self, accounting: Accounting, max_elapsed_ms: u64) -> Result<Report, String> {
        let (snapshot, measured) = match accounting {
            Accounting::Recorded => (
                snapshot(&self.pool)
                    .map_err(|error| lease_error("cannot read the pool", &error))?,
                None,
            ),
            Accounting::Measured => {
                let walked = measure_within(&self.pool, max_elapsed_ms)
                    .map_err(|error| lease_error("cannot measure the pool", &error))?;
                (walked.snapshot, Some(walked.status))
            }
        };
        let (state, state_error) = if self.control.exists() {
            match self.load() {
                Ok(state) => (state, None),
                Err(error) => (State::default(), Some(error)),
            }
        } else {
            (State::default(), None)
        };
        let observations = if accounting == Accounting::Measured {
            Ok(None)
        } else {
            read_observations(&self.control)
        };
        let now = process::now_ms();
        let boot = boot_session();
        // The class view is derived from the state and ledger rows this
        // report already read (design §5.5, §5.6); it reads nothing else and
        // writes nothing.
        let class = self.class_membership(&state, snapshot.slots.as_slice());
        let slots = snapshot
            .slots
            .iter()
            .map(|slot| {
                let record = state
                    .records
                    .iter()
                    .find(|record| slot.project == self.affinity_dir(record.id));
                SlotView {
                    slot_id: slot.slot_id.clone(),
                    dir: slot.dir.clone(),
                    label: slot_label(
                        slot.dir.as_deref(),
                        record.and_then(|record| record.checkout.as_deref()),
                    ),
                    state: slot.state,
                    class: record.map(|record| record.class),
                    class_reason: record.and_then(|record| record.class_reason.clone()),
                    profile: record.map(|record| record.profile.clone()),
                    checkout: record.and_then(|record| record.checkout.clone()),
                    managed: record.is_some(),
                    interrupted: record
                        .filter(|record| {
                            record.active()
                                && slot.state == SlotOccupancy::Held
                                && !matches!(
                                    probe_lock(&self.lock_path(record.id), false),
                                    Ok(Probe::Held)
                                )
                        })
                        .map(|record| Interrupted {
                            holder_pid: record.holder.as_ref().map(|holder| holder.pid),
                            path: return_path(record, slot),
                        }),
                    // A quarantined record whose holder predates this boot
                    // session is reported as closed by the restart, not as
                    // kept forever.
                    retired: record.and_then(|record| {
                        if closed_by_boot(record, boot) {
                            record.lost.as_ref().map(|lost| lost.describe(true))
                        } else {
                            record.retired.clone()
                        }
                    }),
                    holder: record
                        .filter(|_| slot.state == SlotOccupancy::Held)
                        .and_then(|record| record.holder.as_ref())
                        .and_then(|holder| holder.command.clone()),
                    size: match &measured {
                        Some(status) => status
                            .slots
                            .iter()
                            .find(|pooled| pooled.slot_id == slot.slot_id)
                            .map_or_else(
                                || {
                                    SlotSize::NotMeasured(
                                        "size unknown (this slot was not in the measured ledger)"
                                            .to_owned(),
                                    )
                                },
                                |pooled| SlotSize::Measured {
                                    logical: pooled.logical_bytes,
                                    unique: pooled.unique_allocated_bytes,
                                    scan: pooled.accounting.clone(),
                                },
                            ),
                        // A current ledger record is the admission basis, so it is
                        // reported first and labelled as such; the oxide#128 sidecar
                        // stays a separate diagnostic fact.
                        None => match slot.recorded.filter(|fact| fact.current) {
                            Some(fact) => SlotSize::LedgerRecorded {
                                logical_bytes: fact.logical_bytes,
                                age: if fact.at_ms > now {
                                    RecordedAge::ClockSkew
                                } else {
                                    RecordedAge::Ago((now - fact.at_ms) / 1000)
                                },
                                entries: fact.entries,
                            },
                            None => recorded_size(&observations, &snapshot.store_id, slot, now),
                        },
                    },
                }
            })
            .collect::<Vec<SlotView>>();
        let last_use = state
            .records
            .iter()
            .filter(|record| record.last_used > 0)
            .filter_map(|record| {
                let project = self.affinity_dir(record.id);
                let slot = snapshot.slots.iter().find(|slot| slot.project == project)?;
                Some((record, slot))
            })
            .max_by_key(|(record, _)| record.last_used)
            .map(|(record, slot)| LastUse {
                slot_id: Some(slot.slot_id.clone()),
                label: slot_label(slot.dir.as_deref(), record.checkout.as_deref()),
                checkout: record.checkout.clone(),
                at_ms: record.last_used_ms,
            });
        let now_time = SystemTime::now();
        let waiters = state
            .tickets
            .iter()
            .map(|ticket| Waiter {
                pid: ticket.pid,
                age: ticket_age(&self.ticket_path(ticket.id), now_time),
            })
            .collect();
        Ok(Report {
            measured,
            cap_bytes: snapshot.cap_bytes,
            pending_reconciliation: snapshot.evicting_slot_ids,
            last_use,
            flushes: state.flushes,
            waiting: state.tickets.len(),
            waiters,
            cleaning: state
                .cleans
                .iter()
                .map(|clean| clean.checkout.clone())
                .collect(),
            slots,
            recoveries: state.recoveries,
            state_error,
            recorded_total: snapshot.recorded_total,
            recorded_slots: snapshot.recorded_slots,
            unrecorded_slots: snapshot.unrecorded_slots,
            class,
        })
    }

    /// The bounded class-scoped view of one pool's ledger (design §5.5, §5.6),
    /// derived from control state and ledger rows the caller already read.
    ///
    /// `members` is the ledger row of every `one_use` record and `unknown` every
    /// row no record owns; that is what makes a slice figure honest: it is
    /// either the verified member union with no unknown row, or the same union
    /// labelled a lower bound beside the unattributed count. Both sets are
    /// bounded by the ledger's own ceiling: a record owns at most one row (its
    /// affinity directory is unique) and the library refuses a ledger above
    /// `MAX_SLOTS` rows, so nothing here is truncated.
    fn class_membership(&self, state: &State, slots: &[LedgerSlot]) -> ClassMembership {
        let pair = |slot: &LedgerSlot| ClassMember {
            slot_id: slot.slot_id.clone(),
            incarnation: slot.incarnation.clone(),
        };
        let mut members = Vec::new();
        for record in state
            .records
            .iter()
            .filter(|record| record.class == WorkspaceClass::OneUse)
        {
            let project = self.affinity_dir(record.id);
            members.extend(
                slots
                    .iter()
                    .filter(|slot| slot.project == project)
                    .map(pair),
            );
        }
        let unknown = slots
            .iter()
            .filter(|slot| {
                !state
                    .records
                    .iter()
                    .any(|record| slot.project == self.affinity_dir(record.id))
            })
            .map(pair)
            .collect();
        ClassMembership { members, unknown }
    }

    /// The member pairs as the library's bounded set.
    fn member_set(members: &[ClassMember]) -> MemberSet {
        MemberSet::new(
            members
                .iter()
                .map(|member| MemberPair::new(&member.slot_id, &member.incarnation)),
        )
    }

    /// The **evictable** subset of this pool's one-use members (design §5.5.1,
    /// §5.5.2): the only one-use victim source the library is allowed to use.
    ///
    /// Every fact below is read here, from the control state this admission
    /// already holds under its own admission lock, plus one non-blocking probe
    /// of the member's own record lock. `Store::checkout_custody` is
    /// deliberately **not** called: it takes this store's admission lock again,
    /// and on macOS a second `flock` from the same process blocks rather than
    /// recursing. Cross-pool facts are used for nothing: a one-use slot lives
    /// in exactly one pool, so only a writer of that pool can write its tree
    /// (design §5.5.2).
    ///
    /// A member failing any clause is **protected**: still counted by the
    /// slice, never a victim. A donor pin is a held record lock (the donor is
    /// pinned by taking its record lock before the pool lock), so the lock probe
    /// refuses it here without a separate fact.
    fn evictable_members(
        &self,
        state: &State,
        slots: &[LedgerSlot],
        members: &[ClassMember],
        own: u64,
    ) -> Vec<ClassMember> {
        let boot = boot_session();
        let mut evictable = Vec::with_capacity(members.len());
        #[cfg(test)]
        EVICTION_REJECTION.with(|value| value.borrow_mut().clear());
        for pair in members {
            macro_rules! reject {
                ($reason:literal) => {{
                    #[cfg(test)]
                    EVICTION_REJECTION
                        .with(|value| value.borrow_mut().push((pair.slot_id.clone(), $reason)));
                    continue;
                }};
            }
            let Some(slot) = slots
                .iter()
                .find(|slot| slot.slot_id == pair.slot_id && slot.incarnation == pair.incarnation)
            else {
                reject!("ledger membership");
            };
            // A slot's class is a function of exactly one record's immutable
            // field, matched by its own affinity directory (§5.2).
            let Some(record) = state.records.iter().find(|record| {
                record.class == WorkspaceClass::OneUse
                    && slot.project == self.affinity_dir(record.id)
            }) else {
                reject!("record mapping");
            };
            if !idle_member_record(record) {
                reject!("idle record");
            }
            // A free, existing record lock: `Missing` is not free (item 3).
            match probe_lock(&self.lock_path(record.id), false) {
                Ok(Probe::Free(_probe)) => {}
                Ok(Probe::Held) => reject!("lock held"),
                Ok(Probe::Missing) => reject!("lock missing"),
                Err(_) => reject!("lock error"),
            }
            if self.other_record_active(state, record, own, boot) {
                reject!("other active record");
            }
            if slot.state != SlotOccupancy::Idle || slot.lease_id.is_some() {
                reject!("slot occupied or leased");
            }
            let Some(checkout) = record.checkout.as_deref() else {
                reject!("missing checkout");
            };
            if state.cleans.iter().any(|clean| clean.checkout == checkout) {
                reject!("clean mark");
            }
            evictable.push(pair.clone());
        }
        evictable
    }

    /// No other record **in this pool** for the member's checkout is active,
    /// and no retired record's unclosed writers are open for this boot session.
    /// The caller's own record is excluded exactly as `checkout_custody`'s
    /// `own` argument does.
    fn other_record_active(
        &self,
        state: &State,
        member: &Record,
        own: u64,
        boot: Option<u64>,
    ) -> bool {
        let Some(checkout) = member.checkout.as_deref() else {
            return true;
        };
        state
            .records
            .iter()
            .filter(|other| other.id != own)
            .filter(|other| other.checkout.as_deref() == Some(checkout))
            .any(|other| {
                other.active()
                    || (other.phase == Phase::Retired
                        && other
                            .lost
                            .as_ref()
                            .is_some_and(|lost| lost.unclosed.is_some())
                        && !closed_by_boot(other, boot))
            })
    }

    /// Whether a full one-use slice has a holder that can finish (design §6.9):
    /// a record lock held by a live process, an `Acquiring`/`Spawning`/`Running`
    /// phase, and no open unclosed-writer marker. A control-uncertain member is
    /// never progress-capable, so it is never a reason to wait.
    fn progress_capable_member(&self, state: &State, own: u64) -> bool {
        let boot = boot_session();
        state
            .records
            .iter()
            .filter(|record| record.class == WorkspaceClass::OneUse && record.id != own)
            .filter(|record| {
                matches!(
                    record.phase,
                    Phase::Acquiring | Phase::Spawning | Phase::Running
                )
            })
            .filter(|record| {
                !record
                    .lost
                    .as_ref()
                    .is_some_and(|lost| lost.unclosed.is_some())
                    || closed_by_boot(record, boot)
            })
            .any(|record| {
                matches!(
                    probe_lock(&self.lock_path(record.id), false),
                    Ok(Probe::Held)
                )
            })
    }

    /// The bounded visibility line of §10.5 for one slice observation. A normal
    /// warm reuse has no observation and prints nothing; a normal new-slot
    /// admission prints only when a member or a class-unknown row exists, so a
    /// dormant pool's output stays byte-identical.
    fn report_slice(class: WorkspaceClass, report: &SliceReport) {
        if class == WorkspaceClass::Normal && report.members == 0 {
            return;
        }
        match &report.outcome {
            RestoreOutcome::NotNeeded => {}
            RestoreOutcome::Within { bytes } => eprintln!(
                "oxide: one-use slots: {} of {}, {bytes} of {} bytes",
                report.members, report.max_slots, report.max_bytes
            ),
            RestoreOutcome::Over { bytes } => eprintln!(
                "oxide: one-use slots: over by {} bytes: measured {bytes} of {}, {} protected, {} unmeasured",
                bytes.saturating_sub(report.max_bytes),
                report.max_bytes,
                report.protected,
                report.unverified
            ),
            RestoreOutcome::Full { bytes, held } => eprintln!(
                "oxide: one-use slots: full: measured {bytes} of {} bytes, {held} protected of {} members",
                report.max_bytes, report.members
            ),
            RestoreOutcome::LowerBound { bytes, unknown } => eprintln!(
                "oxide: one-use slots: lower bound, {unknown} class-unknown ledger rows: measured {bytes} of {} bytes",
                report.max_bytes
            ),
            RestoreOutcome::Unmeasured { reason } => {
                eprintln!("oxide: one-use slots: unmeasured ({reason})")
            }
        }
    }

    fn attempt(
        &self,
        ticket: u64,
        key: &SlotKey,
        profile: Profile,
        max_active: usize,
        admission: WorkspaceAdmission<'_>,
        share: bool,
        label: Option<&str>,
    ) -> Result<Result<Lease, Wait>, String> {
        // Internal admission first (removable store only), then this pool's.
        let (_barrier, cleaning) = self.sibling_barrier(&key.checkout)?;
        let _admission = self.admission()?;
        let mut state = self.load()?;
        self.drop_dead_tickets(&mut state)?;
        let position = state
            .tickets
            .iter()
            .position(|entry| entry.id == ticket)
            .ok_or("Oxide wait ticket was lost")?;
        if position != 0 {
            self.save(&state)?;
            return Ok(Err(Wait::Queue(position + 1)));
        }
        if cleaning
            || state
                .cleans
                .iter()
                .any(|clean| clean.checkout == key.checkout)
        {
            self.save(&state)?;
            return Ok(Err(Wait::Clean));
        }
        self.recover_settled(&mut state)?;
        if self.backend == Backend::ApfsVolume {
            self.discard_lost(&mut state)?;
        }
        let active = state
            .records
            .iter()
            .filter(|record| record.active())
            .count()
            + self.sibling_active()?;
        if active >= max_active {
            self.save(&state)?;
            return Ok(Err(Wait::HostLimit(active)));
        }

        let view = self.ledger()?;
        let has_slot = |record: &Record| {
            let project = self.affinity_dir(record.id);
            view.slots.iter().any(|slot| slot.project == project)
        };
        self.forget_stale_records(&mut state, key, &view);

        let (mut existing, mut lock, busy) =
            self.find_own_slot(&state, key, admission.class, &view)?;
        let mut shared_from = None;
        let mut shared_origin = None;
        // A project-cap replacement the library performs as part of the
        // admission below; choosing it never deletes anything itself.
        let mut replace: Option<(u64, String)> = None;
        let shared = if share && !existing.is_some_and(|index| has_slot(&state.records[index])) {
            self.shared_candidate(&state, key, admission.class, &view)?
        } else {
            None
        };
        if let Some((id, file, origin)) = shared {
            // This checkout's slotless record is replaced by the taken one.
            if let Some(own) = existing.map(|index| state.records[index].id) {
                self.forget_record(&mut state, own);
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
        } else if !existing.is_some_and(|index| has_slot(&state.records[index])) {
            // Replace the least-recently-used eligible idle whole slot of
            // this project by ledger rank. A record that is held, retired
            // or uncertain (a live or unattributable lock) is never a
            // candidate; foreign and unmeasurable slots fail the library's
            // own ownership and accounting checks.
            // The twelve-slot project cap counts **normal** slots only
            // (design §5.3): a one-use slot never consumes one of a
            // repository's twelve normal slots, and deleting one can never
            // relieve that cap.
            let mut candidates: Vec<(u64, u64, u64, String)> = state
                .records
                .iter()
                .filter(|record| {
                    record.class == WorkspaceClass::Normal
                        && record.phase == Phase::Idle
                        && record.identity == key.identity
                        && has_slot(record)
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
            let project_slots = state
                .records
                .iter()
                .filter(|record| {
                    record.class == WorkspaceClass::Normal
                        && record.identity == key.identity
                        && has_slot(record)
                })
                .count();
            // A specific held compatible slot of any checkout of this
            // repository can become reusable under the existing identity and
            // freshness rules; only such a slot justifies waiting. It must be
            // of this command's class too, because neither the second-slot rule
            // nor sharing takes a record across classes (§5.2, §5.4).
            let compatible_busy = state.records.iter().any(|record| {
                record.class == admission.class
                    && record.identity == key.identity
                    && record.compatibility == key.compatibility
                    && matches!(
                        record.phase,
                        Phase::Acquiring | Phase::Spawning | Phase::Running
                    )
            });
            // The cap is a **normal**-slot cap and only a normal admission
            // enters it (§5.3): a one-use slot never consumes one of a
            // repository's twelve normal slots, deleting one can never
            // relieve that cap (the candidate list below holds no one-use
            // record), and
            // a temporary clone of the same repository therefore no longer
            // competes with its normal checkouts for it. A one-use admission's
            // slot count is bounded by the slice (slice S3) and by `MAX_SLOTS`.
            if admission.class == WorkspaceClass::Normal && project_slots >= PROJECT_SLOT_CAP {
                let chosen = existing.map(|index| state.records[index].id);
                candidates.sort();
                for (_, _, id, slot_id) in candidates {
                    if Some(id) == chosen {
                        continue;
                    }
                    // Only an idle record whose lock is free can be replaced;
                    // the library rechecks its identity, ownership, occupancy
                    // and complete accounting under the pool lock.
                    let Probe::Free(_victim) = probe_lock(&self.lock_path(id), false)? else {
                        continue;
                    };
                    replace = Some((id, slot_id));
                    break;
                }
                if replace.is_none() {
                    self.save(&state)?;
                    // Wait only for a specific held compatible slot of this
                    // checkout. With no such slot and no safely evictable
                    // idle one, refuse honestly instead of promising a
                    // deletion that a safety gate forbids.
                    if busy || compatible_busy {
                        return Ok(Err(Wait::Checkout));
                    }
                    return Err(format!(
                        "the project already holds {PROJECT_SLOT_CAP} slots and none is a safely evictable idle whole slot; no compatible slot can be reused. Wait for one to become idle or free one with `oxide clean` in another checkout, then retry"
                    ));
                }
            }
        }

        let index = match existing {
            Some(index) => index,
            None => {
                if state.records.len() >= MAX_RECORDS {
                    return Err("Oxide control state reached its record limit".to_owned());
                }
                let id = state.take_id()?;
                ensure_private_dir(&self.affinity_dir(id))?;
                let Probe::Free(file) = probe_lock(&self.lock_path(id), true)? else {
                    return Err("a new Oxide record lock is unexpectedly held".to_owned());
                };
                lock = Some(file);
                // The one-use fence is durable before the first document that
                // carries a class-bearing record (design §5.1 step 1). A
                // normal admission writes neither, so a dormant build leaves
                // the pool's numeric version exactly as it found it.
                if admission.class == WorkspaceClass::OneUse {
                    self.fence_one_use_workspaces(&mut state)?;
                }
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
                    class: admission.class,
                    class_reason: admission.reason.map(bounded_class_reason),
                });
                state.records.len() - 1
            }
        };
        let lock = lock.ok_or("Oxide record lock was not taken")?;
        let record = state.records[index].id;
        // Read before this command advances the use counter.
        let recorded = match shared_origin {
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
        self.save(&state)?;

        let (same_repository, folders) = self.slot_folders(&state, key, &view);
        let replace_slot =
            match &replace {
                Some((_, slot_id)) => Some(slot_id.parse::<u64>().map_err(|_| {
                    format!("recorded replacement slot ID {slot_id:?} is not numeric")
                })?),
                None => None,
            };
        // The workspace class and the bounded slice observation (design §6.1,
        // §6.3). Production is dormant: `admission.class` is `Normal` for every
        // command while `one_use::is_active()` is false, so no member set is
        // ever nonempty in a production pool and no record carries a class.
        let class = admission.class;
        let lease_profile = match profile {
            Profile::Debug => LeaseProfile::FullDebug,
            Profile::Quick | Profile::Release => LeaseProfile::Dev,
        };
        let membership = self.class_membership(&state, &view.slots);
        let unknown_rows = membership.unknown.len();
        let evictable = if membership.members.is_empty() {
            Vec::new()
        } else {
            self.evictable_members(&state, &view.slots, &membership.members, record)
        };
        let affinity = self.affinity_dir(record);
        let reuse_candidate = view.slots.iter().any(|slot| {
            slot.project == affinity
                && slot.profile == lease_profile
                && slot.state == SlotOccupancy::Idle
                && slot.lease_id.is_none()
        });
        // Every admission carries the observation. The library performs no
        // restore on a normal warm reuse (it is the branch, not this
        // prediction, that decides), so a normal warm reuse still measures
        // nothing while a new slot — normal or one-use — restores.
        let slice = Some(OneUseSlice::bounded(
            Self::member_set(&membership.members),
            if reuse_candidate {
                SliceAdmission::WarmReuse
            } else {
                SliceAdmission::NewSlot
            },
            membership.unknown.len(),
            self.command_budget(),
        ));
        let preferred = Self::member_set(&evictable);
        let victims = match class {
            WorkspaceClass::Normal => VictimPolicy::Prefer { preferred },
            WorkspaceClass::OneUse => VictimPolicy::Only { preferred },
        };
        let request = LeaseRequest {
            pool: self.pool.clone(),
            project: affinity,
            workspace: key.checkout.clone(),
            profile: lease_profile,
            holder: lease_holder,
            budget_bytes: self.budget,
            // The registered removable volume's free-space reserve, when this
            // pool is the removable one. It is a reserve/prune signal for an
            // unknown-size admission, never a pool-budget input.
            reserve_bytes: crate::removable::registration(self)
                .ok()
                .flatten()
                .map(|registration| registration.reserve_bytes),
            layout: Some(SlotLayout {
                folders,
                same_repository,
                profile: profile.label().to_owned(),
            }),
            replace_slot,
            class: match class {
                WorkspaceClass::Normal => AdmissionClass::Normal,
                WorkspaceClass::OneUse => AdmissionClass::OneUse,
            },
            victims,
            slice,
        };
        let root_hold: Option<barge::Hold>;
        let outcome = lease_measured(request);
        // The library deletes the requested victim only after complete
        // accounting and rank headroom, and it may still refuse afterwards.
        // Drop that record's metadata only when the ledger no longer holds
        // the slot; an untouched victim keeps its record.
        if let Some((victim_record, victim_slot)) = replace {
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
            .position(|entry| entry.id == record)
            .ok_or("Oxide slot record disappeared")?;
        let (grant, measured, unmeasured) = match outcome {
            Ok(leased) => {
                if let Some(note) = &leased.admission_note {
                    eprintln!("oxide: warning: {note}");
                }
                if let Some(report) = &leased.slice {
                    Self::report_slice(class, report);
                }
                root_hold = leased.hold;
                // The child launch must keep the slot's cooperative hold: the
                // descriptor is made inheritable before Cargo is spawned, so a
                // child that outlives this process still holds the root.
                if let Some(hold) = root_hold.as_ref() {
                    hold.set_inheritable(true).map_err(|error| {
                        format!("cannot make the slot hold inheritable: {error}")
                    })?;
                }
                (leased.grant, leased.measured, leased.unmeasured)
            }
            Err(error) => {
                state.settle_idle(index);
                if shared_from.is_some() {
                    state.records[index].checkout = shared_from;
                }
                self.save(&state)?;
                return match error {
                    LeaseError::Busy => Ok(Err(Wait::Uncertain)),
                    // Wait only for a specific held compatible slot of this
                    // checkout. Nothing promises that an unrelated build's
                    // completion frees a count or byte ceiling.
                    LeaseError::Full | LeaseError::LedgerExhausted if busy => {
                        Ok(Err(Wait::Capacity(None)))
                    }
                    LeaseError::SlotCeiling(diagnosis) if busy => {
                        Ok(Err(Wait::Capacity(Some(diagnosis))))
                    }
                    // A full one-use slice waits only for a holder that can
                    // finish (design §6.9); a control-uncertain member is
                    // protected, never a reason to wait.
                    LeaseError::OneUseBudget { .. }
                        if class == WorkspaceClass::OneUse
                            && self.progress_capable_member(&state, record) =>
                    {
                        Ok(Err(Wait::Uncertain))
                    }
                    // Class-unknown ledger rows make the slice bound
                    // unprovable: refuse with the shipped safety class and
                    // name the unattributed rows (§5.6).
                    LeaseError::UnknownAccounting
                        if class == WorkspaceClass::OneUse && unknown_rows > 0 =>
                    {
                        Err(format!(
                            "one-use workspace admission refused: {unknown_rows} of this pool's ledger slots have no owning record, so the one-use slice cannot be bounded; nothing was evicted"
                        ))
                    }
                    // The refusal names the slots a partial walk charged
                    // conservatively and keeps; record the incomplete outcome
                    // so status shows `not measured (walk incomplete)` for
                    // them too (oxide#186). Diagnostic only: nothing here
                    // changes the refusal, its reason or its exit code.
                    LeaseError::CapacityCharged {
                        ref unmeasured, ..
                    } => {
                        self.record_incomplete_walks(None, unmeasured);
                        Err(lease_error("cannot lease a slot", &error))
                    }
                    error => Err(lease_error("cannot lease a slot", &error)),
                };
            }
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
        let prior = match (grant.reused_slot, recorded) {
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
            shared_from,
            measured,
            lease_id: grant.lease_id,
            record,
            _lock: lock,
            _root_hold: root_hold,
        }))
    }

    fn find_own_slot(
        &self,
        state: &State,
        key: &SlotKey,
        class: WorkspaceClass,
        view: &LedgerView,
    ) -> Result<(Option<usize>, Option<File>, bool), String> {
        let has_slot = |record: &Record| {
            let project = self.affinity_dir(record.id);
            view.slots.iter().any(|slot| slot.project == project)
        };
        let matches = |record: &Record| {
            record.phase != Phase::Retired
                && record.checkout.as_deref() == Some(key.checkout.as_path())
                && record.compatibility == key.compatibility
                // Class is part of record identity (design §5.2): an existing
                // record of the other class is never reused or relabelled.
                && record.class == class
        };
        // Second-slot rule: this checkout may hold several records of one
        // key. Take an idle one whose lock is free, one with a slot first,
        // then the most recently used. A busy one (a live command, an
        // interrupted one whose group may still run, or a lock still held)
        // is never reused, released, evicted or relabelled here; the
        // command takes another idle compatible slot or a new one through
        // the normal admission below, and waits only when neither exists.
        let mut own: Vec<usize> = (0..state.records.len())
            .filter(|&index| matches(&state.records[index]))
            .collect();
        own.sort_by_key(|&index| {
            let record = &state.records[index];
            std::cmp::Reverse((has_slot(record), record.last_used))
        });
        let mut existing = None;
        let mut lock = None;
        let mut busy = false;
        for index in own {
            if state.records[index].phase != Phase::Idle {
                busy = true;
                continue;
            }
            match probe_lock(&self.lock_path(state.records[index].id), true)? {
                Probe::Free(file) => {
                    existing = Some(index);
                    lock = Some(file);
                    break;
                }
                Probe::Held | Probe::Missing => busy = true,
            }
        }
        Ok((existing, lock, busy))
    }

    /// Forget idle records that no longer own a slot or match this key.
    fn forget_stale_records(&self, state: &mut State, key: &SlotKey, view: &LedgerView) {
        let has_slot = |record: &Record| {
            let project = self.affinity_dir(record.id);
            view.slots.iter().any(|slot| slot.project == project)
        };
        // The stale sweep is about **this key**, not this class (design §5.2,
        // §5.7): an idle slotless record of the other class is still a live
        // record of this checkout and keeps it, so the one-use command takes a
        // distinct record instead of deleting the normal or legacy one.
        let same_key = |record: &Record| {
            record.phase != Phase::Retired
                && record.checkout.as_deref() == Some(key.checkout.as_path())
                && record.compatibility == key.compatibility
        };
        let stale: Vec<u64> = state
            .records
            .iter()
            .filter(|record| record.phase == Phase::Idle && !has_slot(record) && !same_key(record))
            .map(|record| record.id)
            .collect();
        for id in stale {
            self.forget_record(state, id);
        }
    }

    /// Choose a folder already used by this repository, or derive a new one.
    fn slot_folders(
        &self,
        state: &State,
        key: &SlotKey,
        view: &LedgerView,
    ) -> (Vec<PathBuf>, Vec<String>) {
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
        let folders = match known {
            Some(folder) => vec![folder.to_owned()],
            None => identity::slot_folders(&identity::project_label(&key.checkout), &key.identity),
        };
        (same_repository, folders)
    }

    /// Own-command crash recovery: a non-idle record whose lock is free has
    /// no live Oxide owner. [`settle`] decides whether its held lease can be
    /// recovered; each recovery is logged for `oxide status`.
    fn recover_settled(&self, state: &mut State) -> Result<(), String> {
        let mut view: Option<LedgerView> = None;
        for index in 0..state.records.len() {
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
                    let record_id = state.records[index].id;
                    let flushed = self.flush_before_idle(&recovery.slot_id, record_id)?;
                    // The flush saved its receipt (and any retirement);
                    // continue from that state so it is not overwritten.
                    *state = self.load()?;
                    if !flushed {
                        continue;
                    }
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
        recover(&self.pool, &recovery.lease_id)
            .map_err(|error| lease_error("cannot recover the slot", &error))?;
        state.records[index].pending_recovery = None;
        state.log_recovery(recovery);
        state.settle_idle(index);
        self.save(&state)
    }

    fn forget_record(&self, state: &mut State, id: u64) {
        state.records.retain(|record| record.id != id);
        let _ = fs::remove_dir(self.affinity_dir(id));
        let _ = fs::remove_file(self.lock_path(id));
        let _ = fs::remove_file(self.origin_path(id));
    }

    /// The most recently used idle slot of the same repository and key whose
    /// last checkout is another live affinity and whose origin is trusted.
    /// Detached records (after `oxide clean`) are never shared.
    ///
    /// Sharing is class-matched (design §5.4): a normal command never borrows a
    /// one-use slot, whose build may be about to be deleted, and a one-use
    /// command never borrows or relabels a normal slot.
    fn shared_candidate(
        &self,
        state: &State,
        key: &SlotKey,
        class: WorkspaceClass,
        view: &LedgerView,
    ) -> Result<Option<(u64, File, Built)>, String> {
        let mut candidates: Vec<&Record> = state
            .records
            .iter()
            .filter(|record| {
                record.class == class
                    && record.phase == Phase::Idle
                    && record.identity == key.identity
                    && record.compatibility == key.compatibility
                    && record
                        .checkout
                        .as_ref()
                        .is_some_and(|checkout| *checkout != key.checkout)
            })
            .filter(|record| {
                let project = self.affinity_dir(record.id);
                view.slots
                    .iter()
                    .any(|slot| slot.project == project && slot.state == SlotOccupancy::Idle)
            })
            .collect();
        candidates.sort_by_key(|record| std::cmp::Reverse(record.last_used));
        for record in candidates {
            let Some(origin @ Built::Content(_)) = self.trusted_origin(record) else {
                continue;
            };
            if let Probe::Free(file) = probe_lock(&self.lock_path(record.id), true)? {
                return Ok(Some((record.id, file, origin)));
            }
        }
        Ok(None)
    }

    /// The record's origin if the last command on it recorded one and no
    /// command ran since.
    fn trusted_origin(&self, record: &Record) -> Option<Built> {
        let bytes = bounded_read(&self.origin_path(record.id), MAX_ORIGIN_BYTES).ok()??;
        let stored: OriginFile = serde_json::from_slice(&bytes).ok()?;
        (stored.version == ORIGIN_VERSION && stored.mark == record.last_used)
            .then_some(stored.origin)
            .flatten()
    }

    fn write_origin(&self, id: u64, mark: u64, origin: Option<Built>) -> Result<(), String> {
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
        let temp = path.with_extension("json.tmp");
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)
            .map_err(|error| format!("cannot write a slot origin: {error}"))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| fs::rename(&temp, &path))
            .map_err(|error| format!("cannot write a slot origin: {error}"))
    }

    fn update_record(&self, id: u64, change: impl FnOnce(&mut Record)) -> Result<(), String> {
        let _admission = self.admission()?;
        let mut state = self.load()?;
        let record = state
            .records
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or("Oxide slot record disappeared")?;
        change(record);
        self.save(&state)
    }

    fn register_ticket(&self) -> Result<(u64, File), String> {
        let _admission = self.admission()?;
        let mut state = self.load()?;
        self.drop_dead_tickets(&mut state)?;
        if state.tickets.len() >= MAX_TICKETS {
            return Err(format!(
                "the Oxide wait queue is full ({MAX_TICKETS} commands)"
            ));
        }
        let id = state.take_id()?;
        let Probe::Free(file) = probe_lock(&self.ticket_path(id), true)? else {
            return Err("a new Oxide wait ticket is unexpectedly held".to_owned());
        };
        state.tickets.push(Ticket {
            id,
            pid: std::process::id(),
        });
        self.save(&state)?;
        Ok((id, file))
    }

    fn remove_ticket(&self, id: u64) -> Result<(), String> {
        let _admission = self.admission()?;
        let mut state = self.load()?;
        state.tickets.retain(|ticket| ticket.id != id);
        self.save(&state)?;
        match fs::remove_file(self.ticket_path(id)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        }
    }

    /// A ticket or clean mark whose lock is free belongs to a command that ended.
    fn drop_dead_tickets(&self, state: &mut State) -> Result<(), String> {
        let mut live = Vec::with_capacity(state.tickets.len());
        for ticket in state.tickets.drain(..) {
            let path = self.ticket_path(ticket.id);
            match probe_lock(&path, false)? {
                Probe::Held => live.push(ticket),
                Probe::Free(_) => {
                    let _ = fs::remove_file(path);
                }
                Probe::Missing => {}
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

    fn ensure_control(&self) -> Result<(), String> {
        for directory in ["", "affinity", "locks", "waiters", "bindings"] {
            ensure_private_dir(&self.control.join(directory))?;
        }
        Ok(())
    }

    fn affinity_dir(&self, id: u64) -> PathBuf {
        self.control.join("affinity").join(id.to_string())
    }

    fn origin_path(&self, id: u64) -> PathBuf {
        self.control.join("origins").join(format!("{id}.json"))
    }

    fn lock_path(&self, id: u64) -> PathBuf {
        self.control.join("locks").join(format!("{id}.lock"))
    }

    fn ticket_path(&self, id: u64) -> PathBuf {
        self.control.join("waiters").join(format!("{id}.lock"))
    }

    /// Take admission, then publish the generation before the first
    /// protected write. Release takes the same gate after Cargo has closed.
    fn admission(&self) -> Result<File, String> {
        let lock = lock_admission(&self.control)?;
        self.fence_admission_state()?;
        Ok(lock)
    }

    fn release_admission(&self) -> Result<File, String> {
        let lock = lock_admission_inner(&self.control, Some(&self.pool))?;
        self.fence_admission_state()?;
        Ok(lock)
    }

    /// Output initialization decides writer closure before publishing
    /// anything, including a generation bump.
    fn control_admission(&self) -> Result<File, String> {
        let lock = lock_admission(&self.control)?;
        check_writer_generation(&self.control)?;
        Ok(lock)
    }

    /// Publish an output transaction and any necessary writer generation in
    /// the same control replacement, with admission held.
    fn save_for_writer(&self, state: &State) -> Result<(), String> {
        if state.minimum_writer_generation == Some(WRITER_GENERATION) {
            return self.save(state);
        }
        self.save(&fenced_state(state.clone(), WRITER_GENERATION)?)
    }

    fn fence_admission_state(&self) -> Result<(), String> {
        let state = read_writer_state(&self.control)
            .map_err(|error| lease_error("cannot write control state", &error))?
            .unwrap_or_default();
        if state.minimum_writer_generation == Some(WRITER_GENERATION) {
            return Ok(());
        }
        self.save(&fenced_state(state, WRITER_GENERATION)?)
    }

    /// Make the one-use fence durable **before** the first document that
    /// carries a class-bearing record (design §5.1 step 1, §10.1).
    ///
    /// `state` is the caller's already-loaded state under this pool's
    /// admission lock. The fence is written as its own atomic save first, so a
    /// crash between the two leaves a fenced pool with no class-bearing record
    /// — readable by this binary and refused by an older one, which is the
    /// safe direction. Nothing is written for a normal-only pool, so a dormant
    /// build never advances an existing pool's numeric version.
    fn fence_one_use_workspaces(&self, state: &mut State) -> Result<(), String> {
        if state.one_use_workspaces.as_deref() == Some(ONE_USE_FENCE) {
            return Ok(());
        }
        let mut fenced = state.clone();
        fenced.one_use_workspaces = Some(ONE_USE_FENCE.to_owned());
        self.save(&fenced)?;
        state.one_use_workspaces = Some(ONE_USE_FENCE.to_owned());
        Ok(())
    }

    fn load(&self) -> Result<State, String> {
        // Every reader goes through `read_state`, which uses the shared parser
        // (`oxide_storage::lease::control::parse_state`) the fence also uses, so
        // no reader or writer can disagree about a valid control document.
        Ok(read_state(&self.control.join("state.json"))?.unwrap_or_default())
    }

    fn save(&self, state: &State) -> Result<(), String> {
        // The numeric version advances only with the owned-outputs forward
        // fence or its initialization record. An ordinary mutation of a pool
        // that has not opted into owned outputs stays on the previous
        // numeric version, so an older binary is not refused merely because a
        // build ran (oxide#129). A legacy state is validated against the new
        // fields and object caps first; an oversized legacy field refuses
        // with the original file untouched.
        let mut fenced = state.clone();
        // Only the successor `oxide.outputs/3` fence advances the numeric
        // version to 4. The published `/2` fence keeps version 3 so a state-3
        // binary still runs a pool it understands, and the released `/1` fence
        // advances nothing.
        let recorded = fenced.recorded_accounting.is_some();
        let one_use = fenced.one_use_workspaces.is_some();
        let slot_ceiling = fenced.slot_ceiling.is_some();
        let holding = fenced.holding.is_some();
        let successor = fenced.owned_outputs.as_deref() == Some(OWNED_OUTPUTS_FENCE);
        let previous = fenced.outputs_init.is_some()
            || fenced.owned_outputs.as_deref() == Some(LEGACY_OWNED_OUTPUTS_FENCE);
        fenced.version = if fenced.minimum_writer_generation.is_some() {
            WRITER_STATE_VERSION
        } else if holding {
            // Version 9 is the whole-tree holding fence (oxide#190) and the
            // hand-off trigger that must precede any ledger write recording a
            // holding row.
            HOLDING_STATE_VERSION
        } else if slot_ceiling {
            // Version 8 is the raised-ceiling fence (oxide#177) and the
            // hand-off trigger that must precede the ledger's policy-4 write.
            SLOT_CEILING_STATE_VERSION
        } else if fenced.eviction_accounting.is_some() {
            // Version 7 is the eviction-accounting fence (oxide#175) and the
            // hand-off trigger that must precede the retained-record write.
            EVICTION_ACCOUNTING_STATE_VERSION
        } else if one_use {
            ONE_USE_STATE_VERSION
        } else if recorded {
            // Version 5 is the recorded-accounting fence (oxide#146) and the
            // hand-off trigger that must precede the ledger's policy-2 write.
            RECORDED_ACCOUNTING_STATE_VERSION
        } else if successor {
            PREVIOUS_STATE_VERSION
        } else if previous {
            OLDER_STATE_VERSION
        } else {
            LEGACY_FIELDS_STATE_VERSION
        };
        validate_state(&fenced, false)?;
        let bytes = serde_json::to_vec(&fenced).map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            return Err("Oxide state reached its byte limit".to_owned());
        }
        // Only the admission-lock holder writes, so one temp name suffices.
        let temp = self.control.join("state.json.tmp");
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)
            .map_err(|error| format!("cannot write Oxide state: {error}"))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("cannot write Oxide state: {error}"))?;
        fs::rename(&temp, self.control.join("state.json"))
            .and_then(|()| File::open(&self.control)?.sync_all())
            .map_err(|error| format!("cannot replace Oxide state: {error}"))?;
        Ok(())
    }
}

/// Caller-available bytes of the filesystem holding the new pool's parent.
/// Debug builds accept a scoped injected reading so the fixture proof reaches
/// the refusal path, capacity diagnostic included, without consuming the
/// host's free space; release builds always sample the filesystem.
fn free_bytes_for_creation(parent: &Path) -> Option<u64> {
    #[cfg(debug_assertions)]
    if let Some(injected) = std::env::var("OXIDE_TEST_POOL_FREE_BYTES")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
    {
        return Some(injected);
    }
    statfs(parent).and_then(|sample| {
        let block = u64::try_from(sample.f_bsize).ok()?;
        sample.f_bavail.checked_mul(block)
    })
}

fn create_pool(path: &Path, budget: u64) -> Result<(), String> {
    let parent = path.parent().ok_or("the pool path has no parent")?;
    let name = path
        .file_name()
        .ok_or("the pool path has no final component")?;
    create_parents(parent)?;
    let parent = fs::canonicalize(parent).map_err(|error| error.to_string())?;
    let available = free_bytes_for_creation(&parent)
        .ok_or("cannot sample host free space for pool creation")?;
    if available < HOST_RESERVE_BYTES {
        let mut message = format!(
            "refusing to create the pool: host free space {} MiB is below the 20 GiB reserve",
            available / (1024 * 1024)
        );
        // Read-only, bounded macOS figures for the same filesystem, after the
        // refusal is decided: they never change the 20 GiB decision.
        if let Some(note) = crate::capacity::diagnose(&parent) {
            message.push('\n');
            message.push_str(&note);
        }
        return Err(message);
    }
    let pool = parent.join(name);
    let backend = Backend::DirectoryBudget;
    eprintln!(
        "oxide: creating the build pool at {} ({} MB soft budget, {}, {})",
        pool.display(),
        budget / 1_000_000,
        backend.tag(),
        backend.bound()
    );
    match init_budget_pool(&pool, budget) {
        Ok(_) => Ok(()),
        // Another command created it first.
        Err(_) if pool.join("config.json").is_file() => Ok(()),
        Err(error) => Err(lease_error("could not create the pool", &error)),
    }
}

/// Create missing ancestors owner-only; existing ones are left as they are.
fn create_parents(path: &Path) -> Result<(), String> {
    if fs::symlink_metadata(path).is_ok() {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or("the pool path has no existing ancestor")?;
    create_parents(parent)?;
    match fs::create_dir(path) {
        Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(format!("cannot create {}: {error}", path.display())),
    }
}

fn ensure_private_dir(path: &Path) -> Result<(), String> {
    match fs::create_dir(path) {
        Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("cannot create {}: {error}", path.display())),
    }
    verify_private_dir(path)
}

/// Open an existing owner-only control directory without following its final
/// path component. The descriptor is suitable for relative control-file I/O.
pub(crate) fn open_control_directory(path: &Path) -> Result<Option<File>, String> {
    let named = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    };
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if !named.is_dir() || named.uid() != uid || named.mode() & 0o077 != 0 {
        return Err(format!(
            "{} must be an owner-only directory",
            path.display()
        ));
    }
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let opened = directory
        .metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if !opened.is_dir()
        || opened.uid() != uid
        || opened.mode() & 0o077 != 0
        || opened.dev() != named.dev()
        || opened.ino() != named.ino()
    {
        return Err(format!(
            "{} changed while it was being opened",
            path.display()
        ));
    }
    Ok(Some(directory))
}

fn verify_private_dir(path: &Path) -> Result<(), String> {
    if open_control_directory(path)?.is_none() {
        return Err(format!("{} does not exist", path.display()));
    }
    Ok(())
}

/// The internal pool's admission lock, made reentrant for this thread so an
/// owned-output publication can update its registry and its receipt inside one
/// critical section without taking the same lock twice (oxide#129). Nested
/// acquisition only succeeds on the thread that already holds it.
pub(crate) struct PublicationGuard {
    control: PathBuf,
    nested: bool,
    _file: Option<File>,
}

thread_local! {
    static HELD_PUBLICATION: std::cell::RefCell<Vec<PathBuf>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

pub(crate) fn publication_guard(control: &Path) -> Result<PublicationGuard, String> {
    let already = HELD_PUBLICATION.with(|held| held.borrow().iter().any(|path| path == control));
    if already {
        return Ok(PublicationGuard {
            control: control.to_path_buf(),
            nested: true,
            _file: None,
        });
    }
    let file = lock_admission(control)?;
    publish_writer_generation(control)?;
    HELD_PUBLICATION.with(|held| held.borrow_mut().push(control.to_path_buf()));
    Ok(PublicationGuard {
        control: control.to_path_buf(),
        nested: false,
        _file: Some(file),
    })
}

impl Drop for PublicationGuard {
    fn drop(&mut self) {
        if self.nested {
            return;
        }
        HELD_PUBLICATION.with(|held| {
            let mut held = held.borrow_mut();
            if let Some(index) = held.iter().rposition(|path| path == &self.control) {
                held.remove(index);
            }
        });
    }
}

/// Record `transaction` as the paired control's ready registry and publish the
/// successor fence, after that control's writers are positively closed. The
/// caller holds the paired pool's admission lock and has already checked any
/// predecessor fence it accepted.
fn fence_paired(
    sibling: &Store,
    state: &mut State,
    transaction: &str,
    own_checkout: Option<&Path>,
) -> Result<(), String> {
    writers_closed(state, "the paired", own_checkout)?;
    state.outputs_init = Some(OutputsInit::Ready {
        transaction: transaction.to_owned(),
        ready_ms: process::now_ms(),
    });
    state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
    sibling.save_for_writer(state)
}

/// Positive writer closure in one pool's state for owned-outputs
/// initialization (see [`Store::initialize_outputs`]). Anything that is not
/// positively idle, closed or the caller's own barrier refuses.
fn writers_closed(state: &State, pool: &str, own_checkout: Option<&Path>) -> Result<(), String> {
    let boot = boot_session();
    for record in &state.records {
        if record.active() {
            return Err(format!(
                "{pool} pool holds an active command record (record {}); the shared output format is initialized only after every command in both pools closes",
                record.id
            ));
        }
        if closed_by_boot(record, boot) {
            // A restart ended the quarantined writers; the slot is released
            // and deleted whole at the next admission.
            continue;
        }
        if let Some(why) = record
            .lost
            .as_ref()
            .and_then(|lost| lost.unclosed.as_deref())
        {
            return Err(format!(
                "{pool} pool has a lost slot whose writers were never confirmed closed ({why}); the shared output format is not initialized until that is resolved"
            ));
        }
    }
    if let Some(clean) = state
        .cleans
        .iter()
        .find(|clean| Some(clean.checkout.as_path()) != own_checkout)
    {
        return Err(format!(
            "{pool} pool holds a clean barrier for {}; the shared output format is initialized only after it closes",
            clean.checkout.display()
        ));
    }
    Ok(())
}

/// The pool's admission lock file, opened (and created once) but not locked.
fn open_admission_file(control: &Path, create: bool) -> Result<Option<File>, String> {
    let Some(directory) = open_control_directory(control)? else {
        return Ok(None);
    };
    let name = CString::new("admission.lock").map_err(|error| error.to_string())?;
    // Create exclusively, then open the existing lock without `O_CREAT`.
    // Repeating `O_CREAT | O_NOFOLLOW` for every contender races with the
    // creator on this platform and can report a spurious `ENOENT`, refusing
    // a valid admission. Both steps keep `O_NOFOLLOW`, so a symlink is never
    // followed, and creation is exclusive so no second file is ever written.
    let mut fd = -1;
    let mut last = None;
    for _ in 0..3 {
        if create {
            // SAFETY: `name` is a live CString with initialized bytes through
            // its terminator, and `directory` owns a live descriptor until
            // `openat` ends.
            fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDWR
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if fd >= 0 {
                break;
            }
            let create_error = std::io::Error::last_os_error();
            if create_error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(format!(
                    "cannot create admission lock in {}: {create_error}",
                    control.display()
                ));
            }
        }
        // SAFETY: as above; the existing lock must still be a regular file.
        fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd >= 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        let retry = error.kind() == std::io::ErrorKind::NotFound;
        last = Some(error);
        if !retry {
            break;
        }
    }
    if fd < 0 {
        // A missing lock is absence, and only absence: a symlinked, unreadable
        // or otherwise unusable lock is an error the caller must treat as
        // unknown custody, never as "no lock". A guarded verb may not create
        // the lock, so `create == false` must distinguish the two precisely.
        let error = last.unwrap_or_else(|| std::io::Error::from_raw_os_error(libc::ENOENT));
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(if create {
            // Pre-S1 wording, kept verbatim for the shared dormant path.
            format!(
                "cannot open admission lock in {}: the create did not succeed",
                control.display()
            )
        } else {
            // The guarded probe's precise wording: a symlinked, unreadable or
            // otherwise unusable lock is not absence.
            format!(
                "cannot open admission lock in {}: {error}",
                control.display()
            )
        });
    }
    // SAFETY: `fd` is a newly opened descriptor owned by this function.
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(format!(
            "{} must be an owner-only file",
            control.join("admission.lock").display()
        ));
    }
    Ok(Some(file))
}

/// Take the internal pool's existing admission lock for metadata updates and
/// command admission. Only the lock file may be created; the control
/// directory must already exist.
pub(crate) fn lock_admission(control: &Path) -> Result<File, String> {
    lock_admission_inner(control, None)
}

fn prepare_front_door_pool(pool: &Path, context: &str) -> Result<LedgerView, String> {
    let result = prepare_pool(pool);
    if result != Err(LeaseError::WriterFenceBusy) {
        return result.map_err(|error| lease_error(context, &error));
    }
    // The pool-first open has dropped the pool lock. Wait for admission in
    // the normal order, under the command's budget, and reread the state
    // before publishing. Subsequent pool opens see the durable generation.
    let control = pool.join(MANAGED_NAME);
    {
        let _admission = lock_admission(&control)?;
        publish_writer_generation(&control)?;
    }
    prepare_pool(pool).map_err(|error| lease_error(context, &error))
}

pub(crate) fn publish_writer_generation(control: &Path) -> Result<(), String> {
    fence_admission_writer(control)
        .map_err(|error| lease_error("cannot fence the Oxide writer", &error))
}

fn check_writer_generation(control: &Path) -> Result<(), String> {
    check_control_writer(control)
        .map_err(|error| lease_error("cannot open the Oxide writer", &error))
}

fn lock_admission_inner(control: &Path, release_pool: Option<&Path>) -> Result<File, String> {
    // The shipped messages are part of the pre-S1 behaviour this slice must
    // keep byte-identical while it is dormant, so they are produced here
    // exactly as they were: an absent control directory names itself, and any
    // other failure to end with an open descriptor says the create did not
    // succeed. The guarded probe path (create == false) keeps the precise
    // wording it needs to tell "no lock" from "cannot tell".
    let Some(file) = open_admission_file(control, true)? else {
        return if open_control_directory(control)?.is_none() {
            Err(format!("{} does not exist", control.display()))
        } else {
            Err(format!(
                "cannot open admission lock in {}: the create did not succeed",
                control.display()
            ))
        };
    };
    if let Some(pool) = release_pool {
        wait_release_admission(
            &file,
            Duration::from_secs(5),
            crate::budget::UNBUDGETED_LOCK_BOUND,
            || crate::budget::admission_deadline().is_some(),
            crate::budget::admission_expired,
            || {
                let slot = snapshot(pool)
                    .ok()
                    .and_then(|view| view.evicting_slot_ids.into_iter().next());
                eprintln!("{}", admission_wait_message(slot.as_deref()));
            },
        )?;
    } else {
        crate::budget::lock(&file, "admission")?;
    }
    Ok(file)
}

/// A lost slot is recovered to idle and then evicted; the message says so and
/// names the record it was in, instead of calling a held slot deleted.
fn idle_slot_evicted_message(slot_id: &str, record: &str) -> String {
    format!("oxide: evicted idle slot {slot_id}; the slot was {record}")
}

fn admission_wait_message(slot: Option<&str>) -> String {
    match slot {
        Some(id) => format!("oxide: waiting for the admission lock (slot {id} is recorded as being deleted; its current writer is unknown)"),
        None => "oxide: waiting for the admission lock (holder unknown)".to_owned(),
    }
}

fn wait_release_admission(
    file: &File,
    notice_delay: Duration,
    bound: Duration,
    budgeted: impl Fn() -> bool,
    expired: impl Fn() -> bool,
    notice: impl FnOnce(),
) -> Result<(), String> {
    let started = Instant::now();
    let mut notice = Some(notice);
    loop {
        if flock_nonblocking(file)? {
            return Ok(());
        }
        if budgeted() && expired() {
            return Err(
                "the admission budget expired while waiting for the admission lock".to_owned(),
            );
        }
        // Without an admission budget the wait still ends: a holder that
        // outlives the bound is a refusal, never an unbounded flock (oxide#325).
        let waited = started.elapsed();
        if !budgeted() && waited >= bound {
            return Err(format!(
                "gave up waiting for the admission lock after {:.1}s (the {:.0}s bound was reached; the notice above names its holder)",
                waited.as_secs_f64(),
                bound.as_secs_f64()
            ));
        }
        if notice.is_some() && waited >= notice_delay {
            if let Some(notice) = notice.take() {
                notice();
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// One non-blocking admission look, **without creating anything**: `Held` when
/// another command holds the lock, `Free` when it exists and is unlocked, and
/// `Absent` when the pool has no admission lock at all (so no other command has
/// ever entered a critical section on it). This is what lets a guarded verb
/// decide a refusal with nothing written (design §4.1 step 4).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AdmissionProbe {
    Held,
    Free,
    Absent,
}

pub(crate) fn probe_admission(control: &Path) -> Result<AdmissionProbe, String> {
    let Some(file) = open_admission_file(control, false)? else {
        return Ok(AdmissionProbe::Absent);
    };
    // SAFETY: the descriptor is open for the duration of the call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        // Released when `file` drops: this is a look, not an acquisition.
        return Ok(AdmissionProbe::Free);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(AdmissionProbe::Held)
    } else {
        Err(format!("cannot probe the Oxide admission lock: {error}"))
    }
}

/// A non-blocking acquisition of an **existing** admission lock (one-use S1,
/// design §4.1 step 4): `Some` once the lock is taken, `None` when it is held or
/// has disappeared. It never creates the file, because a guarded verb may only
/// touch the checkout binding mutex on its refusal paths.
fn acquire_existing_admission(control: &Path) -> Result<Option<File>, String> {
    let Some(file) = open_admission_file(control, false)? else {
        return Ok(None);
    };
    // SAFETY: the descriptor is open for the duration of the call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        check_writer_generation(control)?;
        return Ok(Some(file));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(None)
    } else if error.kind() == std::io::ErrorKind::Interrupted {
        Ok(None)
    } else {
        Err(format!("cannot take the Oxide admission lock: {error}"))
    }
}

/// Test-only: record one active record for `checkout` in `store` and take its
/// lock the way a live command holds it. The returned file keeps it held.
#[cfg(test)]
pub fn hold_record_for_tests(store: &Store, checkout: &Path, id: u64) -> File {
    let mut state = store.load().unwrap();
    let mut entry = Record {
        id,
        identity: "identity".to_owned(),
        compatibility: "key".to_owned(),
        profile: "quick".to_owned(),
        checkout: Some(checkout.to_path_buf()),
        phase: Phase::Running,
        process_group: None,
        last_used: 1,
        holder: Some(Holder {
            pid: 41,
            started_ms: 7,
            lease_holder: "oxide-41-3".to_owned(),
            command: None,
        }),
        lease_id: Some("v1.a.1.1.1".to_owned()),
        pending_recovery: None,
        retired: None,
        lost: None,
        last_used_ms: None,
        class: WorkspaceClass::Normal,
        class_reason: None,
    };
    entry.phase = Phase::Running;
    state.records.push(entry);
    store.save(&state).unwrap();
    let Probe::Free(file) = probe_lock(&store.lock_path(id), true).unwrap() else {
        panic!("a fresh record lock is free");
    };
    file
}

/// The optional diagnostic sidecar, read without following a link. `None`
/// means no observations were ever recorded; any other failure is returned
/// so a caller reports "not measured" instead of falling back to a scan.
fn read_observations(control: &Path) -> Result<Option<Observations>, String> {
    let path = control.join(OBSERVATIONS_NAME);
    let Some(bytes) = bounded_read(&path, MAX_OBSERVATIONS_BYTES)
        .map_err(|error| format!("cannot read recorded observations: {error}"))?
    else {
        return Ok(None);
    };
    let document: Observations = serde_json::from_slice(&bytes)
        .map_err(|error| format!("recorded observations are invalid: {error}"))?;
    if document.schema != OBSERVATIONS_SCHEMA {
        return Err(format!(
            "recorded observations use schema {:?}, not {OBSERVATIONS_SCHEMA}",
            document.schema
        ));
    }
    if document.observations.len() > MAX_OBSERVATIONS {
        return Err("recorded observations exceed their retention bound".to_owned());
    }
    for observation in &document.observations {
        observation.validate()?;
    }
    Ok(Some(document))
}

/// A slot's size phrase from recorded observations, matched to exactly this
/// ledger record: store identity, slot ID, incarnation and recorded
/// directory. A different generation is a different slot and reads as not
/// measured.
fn recorded_size(
    observations: &Result<Option<Observations>, String>,
    store_id: &str,
    slot: &LedgerSlot,
    now_ms: u64,
) -> SlotSize {
    let document = match observations {
        Err(error) => {
            return SlotSize::NotMeasured(format!(
                "not measured (recorded observations unreadable: {error}); use `oxide status --measure`"
            ))
        }
        Ok(None) => {
            return SlotSize::NotMeasured(
                "not measured (no recorded observation); use `oxide status --measure`".to_owned(),
            )
        }
        Ok(Some(document)) => document,
    };
    match document
        .observations
        .iter()
        .find(|observation| observation.matches(store_id, slot))
    {
        // The last walk of this generation stopped partial: say so and never
        // report a size (oxide#186). A current ledger record supersedes this
        // fact before it is reached.
        Some(observation) if observation.source == ObservationSource::AdmissionWalkIncomplete => {
            SlotSize::NotMeasured(INCOMPLETE_WALK.to_owned())
        }
        Some(observation) => SlotSize::Recorded {
            allocated_bytes: observation.allocated_bytes,
            age: if observation.measured_at_ms > now_ms {
                RecordedAge::ClockSkew
            } else {
                RecordedAge::Ago((now_ms - observation.measured_at_ms) / 1000)
            },
            source: observation.source,
            lease_id: observation.lease_id.clone(),
        },
        None => SlotSize::NotMeasured(
            "not measured (no recorded observation for this slot generation); use `oxide status --measure`"
                .to_owned(),
        ),
    }
}

/// Retain the most recent observations, at most [`MAX_OBSERVATIONS`], in a
/// deterministic order. A deterministic tie-break keeps repeated writes of
/// the same set byte-identical.
fn retain_newest(observations: &mut Vec<Observation>) {
    observations.sort_by(|left, right| {
        (
            left.measured_at_ms,
            left.store_id.as_str(),
            left.slot_id.as_str(),
            left.incarnation.as_str(),
            left.dir.as_deref().unwrap_or(""),
        )
            .cmp(&(
                right.measured_at_ms,
                right.store_id.as_str(),
                right.slot_id.as_str(),
                right.incarnation.as_str(),
                right.dir.as_deref().unwrap_or(""),
            ))
    });
    if observations.len() > MAX_OBSERVATIONS {
        let excess = observations.len() - MAX_OBSERVATIONS;
        observations.drain(..excess);
    }
}

/// Read the recorded observations, if any, for one pool's status view.
/// Public so the front door's read path can be exercised without a pool.
#[cfg(test)]
pub(crate) fn read_observations_for_tests(
    control: &Path,
) -> Result<Option<Vec<Observation>>, String> {
    Ok(read_observations(control)?.map(|document| document.observations))
}

impl Store {
    /// Record that an admission's walk of each of these slot generations
    /// stopped partial, so default status reports `not measured (walk
    /// incomplete)` for them (oxide#186). No size is written and nothing is
    /// derived from the fact. A refusal has no grant, so it passes `None` and
    /// the store identity comes from the pool's own recorded snapshot.
    /// Diagnostic only: every failure is ignored, and admission, ownership and
    /// exit status never change.
    fn record_incomplete_walks(&self, store_id: Option<&str>, slots: &[UnmeasuredSlot]) {
        if slots.is_empty() {
            return;
        }
        let store_id = match store_id {
            Some(store_id) => store_id.to_owned(),
            None => match snapshot(&self.pool) {
                Ok(recorded) => recorded.store_id.clone(),
                Err(_) => return,
            },
        };
        for slot in slots {
            let _ = self.record_observation(Observation {
                store_id: store_id.clone(),
                slot_id: slot.slot_id.clone(),
                incarnation: slot.incarnation.clone(),
                dir: slot.dir.clone(),
                measured_at_ms: process::now_ms(),
                allocated_bytes: 0,
                source: ObservationSource::AdmissionWalkIncomplete,
                lease_id: None,
            });
        }
    }

    /// Persist one complete measurement as a diagnostic observation. Never
    /// called by a read: only an acquiring command that already made the
    /// reuse walk records it. Failure is diagnostic only, so callers ignore
    /// it and admission, ownership and exit status never change.
    fn record_observation(&self, observation: Observation) -> Result<(), String> {
        if !self.control.is_dir() {
            return Err("the control directory is absent".to_owned());
        }
        let mut document = match read_observations(&self.control)? {
            Some(document) => document,
            None => Observations::default(),
        };
        document.observations.retain(|existing| {
            !(existing.store_id == observation.store_id
                && existing.slot_id == observation.slot_id
                && existing.incarnation == observation.incarnation
                && existing.dir == observation.dir)
        });
        document.observations.push(observation);
        retain_newest(&mut document.observations);
        let mut bytes = serde_json::to_vec(&document).map_err(|error| error.to_string())?;
        while bytes.len() as u64 > MAX_OBSERVATIONS_BYTES && !document.observations.is_empty() {
            document.observations.remove(0);
            bytes = serde_json::to_vec(&document).map_err(|error| error.to_string())?;
        }
        if bytes.len() as u64 > MAX_OBSERVATIONS_BYTES {
            return Err("recorded observations would exceed their byte limit".to_owned());
        }
        let temp = self.control.join(OBSERVATIONS_TEMP);
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)
            .map_err(|error| format!("cannot write recorded observations: {error}"))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("cannot write recorded observations: {error}"))?;
        drop(file);
        fs::rename(&temp, self.control.join(OBSERVATIONS_NAME))
            .and_then(|()| File::open(&self.control)?.sync_all())
            .map_err(|error| format!("cannot replace recorded observations: {error}"))
    }
}

/// Test-only: write a default control state, so a fixture has a durable
/// `state.json` (with `next_id`) to compare against.
#[cfg(test)]
pub fn save_default_state_for_tests(store: &Store) {
    store.save(&State::default()).unwrap();
}

/// Test-only: an active record for `checkout` whose lock file exists but is
/// free, so the guarded barrier classifies it as a *dead* fact.
#[cfg(test)]
pub fn dead_record_for_tests(store: &Store, checkout: &Path, id: u64) {
    let held = hold_record_for_tests(store, checkout, id);
    drop(held);
}

/// Test-only: a clean mark for `checkout` with no live group and no waiter
/// lock, so the guarded barrier classifies it as a dead fact.
#[cfg(test)]
pub fn dead_clean_mark_for_tests(store: &Store, checkout: &Path, id: u64) {
    let mut state = store.load().unwrap();
    state.cleans.push(Clean {
        id,
        checkout: checkout.to_path_buf(),
        process_group: None,
    });
    store.save(&state).unwrap();
}

/// Open an owner-only regular control file without following symlinks.
pub(crate) fn private_file(path: &Path, create: bool) -> Result<Option<File>, String> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW);
    if create {
        options.create(true).mode(0o600);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if !create && error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot open {}: {error}", path.display())),
    };
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(format!("{} must be an owner-only file", path.display()));
    }
    Ok(Some(file))
}

/// The bytes of one private control file, read at most `max` bytes. `Ok(None)`
/// means the file is absent. A file longer than `max` is an error, never a
/// truncated read, so a caller cannot parse a prefix of an oversized document.
fn bounded_read(path: &Path, max: u64) -> Result<Option<Vec<u8>>, String> {
    let Some(file) = private_file(path, false)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > max {
        return Err(format!("{} exceeds its byte limit", path.display()));
    }
    Ok(Some(bytes))
}

/// One control state document through the shared parser (`parse_state`), the
/// same acceptance test the writer and the owned-outputs fence use. `Ok(None)`
/// is an absent document; every other failure is an error.
fn read_state(path: &Path) -> Result<Option<State>, String> {
    bounded_read(path, MAX_STATE_BYTES)?
        .map(|bytes| parse_state(&bytes))
        .transpose()
}

/// Approximate age from the existing ticket file's metadata only.
fn ticket_age(path: &Path, now: SystemTime) -> Option<WaitAge> {
    let metadata = fs::symlink_metadata(path).ok()?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return None;
    }
    let created = metadata.created().ok();
    let modified = if created.is_none() {
        metadata.modified().ok()
    } else {
        None
    };
    wait_age_from_times(created, modified, now)
}

fn wait_age_from_times(
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
    now: SystemTime,
) -> Option<WaitAge> {
    let (timestamp, source) = match created {
        Some(timestamp) => (timestamp, WaitAgeSource::Created),
        None => (modified?, WaitAgeSource::Modified),
    };
    let seconds = now.duration_since(timestamp).ok()?.as_secs();
    Some(WaitAge { seconds, source })
}

enum Probe {
    Free(File),
    Held,
    Missing,
}

/// Try an exclusive lock without blocking.
fn probe_lock(path: &Path, create: bool) -> Result<Probe, String> {
    let Some(file) = private_file(path, create)? else {
        return Ok(Probe::Missing);
    };
    // SAFETY: the descriptor is open for the duration of the call.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(Probe::Free(file));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(Probe::Held)
    } else {
        Err(format!("cannot lock {}: {error}", path.display()))
    }
}

/// A blocking exclusive `flock` for test fixtures only: product waits are bounded.
#[cfg(test)]
pub(crate) fn flock_blocking(file: &File) -> Result<(), String> {
    flock(file, libc::LOCK_EX)
}

/// One nonblocking exclusive `flock` attempt; `true` when the lock was taken.
pub(crate) fn flock_nonblocking(file: &File) -> Result<bool, String> {
    // SAFETY: the descriptor is open for the duration of the call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(false)
    } else {
        Err(format!("cannot lock the control file: {error}"))
    }
}

#[cfg(test)]
fn flock(file: &File, operation: libc::c_int) -> Result<(), String> {
    loop {
        // SAFETY: the descriptor is open for the duration of the call.
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(format!("cannot take the Oxide admission lock: {error}"));
        }
    }
}

pub(crate) fn statfs(path: &Path) -> Option<libc::statfs> {
    let mut bytes = path.as_os_str().as_bytes().to_vec();
    bytes.push(0);
    let mut sample = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `bytes` is NUL-terminated and `sample` is valid for writes.
    let result = unsafe { libc::statfs(bytes.as_ptr().cast(), sample.as_mut_ptr()) };
    // SAFETY: statfs initialised `sample` when it returned 0.
    (result == 0).then(|| unsafe { sample.assume_init() })
}

/// Debug-only proof pause (absent from release builds): under the shared proof
/// supervisor's layout, `OXIDE_TEST_SEED_PAUSE=<point>:<gate>` writes
/// `<gate>.<point>-<pid>` and waits (at most 120 s) for `<gate>`.
#[cfg(debug_assertions)]
fn seed_proof_pause(point: &str) {
    if oxide_storage::proof::evidence().is_none() {
        return;
    }
    let Some(gate) = std::env::var("OXIDE_TEST_SEED_PAUSE")
        .ok()
        .and_then(|value| value.strip_prefix(&format!("{point}:")).map(str::to_owned))
    else {
        return;
    };
    let marker = format!("{gate}.{point}-{}", std::process::id());
    let staged = format!("{gate}.staging-{point}-{}", std::process::id());
    if std::fs::write(&staged, b"paused").is_ok() {
        let _ = std::fs::rename(&staged, &marker);
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    while !Path::new(&gate).exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn lease_error(context: &str, error: &LeaseError) -> String {
    if *error == LeaseError::WriterUpdated {
        WRITER_UPDATED.set(true);
    }
    format!("{context}: {} ({error})", error.reason())
}

thread_local! {
    // Store callers use String errors and some add context or prefer a budget
    // result. Preserve the typed lock refusal through those front-door paths.
    static WRITER_UPDATED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn writer_updated() -> bool {
    WRITER_UPDATED.get()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn release_admission_notice_and_budget_are_bounded_and_truthful() {
        let (root, _store, _key) = class_fixture("release-admission-notice");
        let path = root.join("notice-lock");
        let holder = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let waiter = OpenOptions::new().write(true).open(&path).unwrap();
        flock_blocking(&holder).unwrap();
        let calls = std::cell::Cell::new(0);
        wait_release_admission(
            &waiter,
            Duration::ZERO,
            Duration::from_secs(120),
            || false,
            || false,
            || {
                calls.set(calls.get() + 1);
                // SAFETY: holder owns this fixture lock.
                assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_UN) }, 0);
            },
        )
        .unwrap();
        assert_eq!(calls.get(), 1);
        // SAFETY: waiter owns this fixture lock.
        assert_eq!(unsafe { libc::flock(waiter.as_raw_fd(), libc::LOCK_UN) }, 0);
        wait_release_admission(
            &waiter,
            Duration::from_secs(5),
            Duration::from_secs(120),
            || false,
            || false,
            || panic!("uncontended notice"),
        )
        .unwrap();
        // SAFETY: waiter owns this fixture lock.
        assert_eq!(unsafe { libc::flock(waiter.as_raw_fd(), libc::LOCK_UN) }, 0);
        flock_blocking(&holder).unwrap();
        assert_eq!(
            wait_release_admission(
                &waiter,
                Duration::ZERO,
                Duration::from_secs(120),
                || true,
                || true,
                || panic!("expired notice")
            )
            .unwrap_err(),
            "the admission budget expired while waiting for the admission lock"
        );
        // An unbudgeted wait on a held lock ends at its bound, naming what it waited on.
        let started = Instant::now();
        let refusal = wait_release_admission(
            &waiter,
            Duration::ZERO,
            Duration::from_millis(60),
            || false,
            || false,
            || {},
        )
        .unwrap_err();
        assert!(
            refusal.starts_with("gave up waiting for the admission lock after "),
            "{refusal}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(
            idle_slot_evicted_message("3", "held when the removable volume was lost (Cargo group 4 empty)"),
            "oxide: evicted idle slot 3; the slot was held when the removable volume was lost (Cargo group 4 empty)"
        );
        assert_eq!(admission_wait_message(Some("17")), "oxide: waiting for the admission lock (slot 17 is recorded as being deleted; its current writer is unknown)");
        assert_eq!(
            admission_wait_message(None),
            "oxide: waiting for the admission lock (holder unknown)"
        );
        drop(holder);
        drop(waiter);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn release_admission_wait_is_bounded_when_the_holder_never_lets_go() {
        let (root, _store, _key) = class_fixture("release-admission-bound");
        let path = root.join("bound-lock");
        let holder = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let waiter = OpenOptions::new().write(true).open(&path).unwrap();
        flock_blocking(&holder).unwrap();
        let started = Instant::now();
        let refusal = wait_release_admission(
            &waiter,
            Duration::from_millis(10),
            Duration::from_millis(80),
            || false,
            || false,
            || {},
        )
        .unwrap_err();
        assert!(refusal.contains("the admission lock"), "{refusal}");
        assert!(started.elapsed() < Duration::from_secs(10));
        drop(holder);
        drop(waiter);
        fs::remove_dir_all(root).unwrap();
    }

    /// A ledger as an earlier Oxide wrote it reads back, keeps its stored
    /// digests and serializes to the same bytes (oxide#103 sha2 0.11.0).
    #[test]
    fn frozen_ledger_reads_back_unchanged() {
        let frozen = concat!(
            r#"{"version":1,"next_id":2,"next_use":4,"tickets":[],"cleans":[],"records":[{"#,
            r#""id":1,"identity":"4344f9285b31bb4b9e695c75b8af04a5f4a97a99705393e8d0126c94832eec4f","#,
            r#""compatibility":"12d9fc45c5c84a2e41a4e96d4918b08d421b41fd92114cbecd1eecb4b0a16936","#,
            r#""profile":"quick","checkout":"/Users/Zoë/work space/oxide","phase":"idle","#,
            r#""process_group":null,"last_used":3,"holder":null,"lease_id":null,"#,
            r#""pending_recovery":null,"last_used_ms":1760000000000}],"recoveries":[]}"#
        );
        let state: State = serde_json::from_str(frozen).expect("frozen ledger");
        let record = &state.records[0];
        assert_eq!(
            record.identity,
            crate::identity::digest("git:https://github.com/inflatable-cookie/oxide:")
        );
        assert_eq!(
            record.compatibility,
            "12d9fc45c5c84a2e41a4e96d4918b08d421b41fd92114cbecd1eecb4b0a16936"
        );
        assert_eq!(serde_json::to_string(&state).expect("serialize"), frozen);
    }

    #[test]
    fn wait_accounting_partitions_observed_intervals_and_pauses_between_grants() {
        let start = Instant::now();
        let mut waits = WaitAccounting::default();
        waits.observe(&Wait::Queue(1), 2, start);
        waits.observe(&Wait::Queue(2), 2, start + Duration::from_millis(200));
        waits.stop(start + Duration::from_millis(300));

        // Freshness work between lease attempts is outside measured waiting.
        waits.observe(&Wait::HostLimit(2), 2, start + Duration::from_secs(10));
        waits.stop(start + Duration::from_millis(11_100));
        assert_eq!(
            waits.admission_queue_wait(),
            Some(Duration::from_millis(1_400))
        );

        assert_eq!(
            waits.completion_line("slot ready").as_deref(),
            Some("oxide: waited 1.4s (queue 0.3s, host limit 1.1s); slot ready")
        );
        assert_eq!(waits.summary_suffix().as_deref(), Some(", waited 1.4s"));
    }

    #[test]
    fn wait_accounting_reports_unavailable_after_a_backward_clock() {
        let start = Instant::now();
        let mut waits = WaitAccounting::default();
        waits.observe(&Wait::Queue(1), 2, start);
        waits.stop(start + Duration::from_secs(1));
        waits.observe(&Wait::Capacity(None), 2, start + Duration::from_secs(3));
        waits.stop(start + Duration::from_secs(2));

        assert_eq!(
            waits.completion_line("slot ready").as_deref(),
            Some("oxide: wait duration unavailable; slot ready")
        );
        assert_eq!(
            waits.summary_suffix().as_deref(),
            Some(", wait duration unavailable")
        );
    }

    #[test]
    fn wait_accounting_closes_intervals_on_cancellation_and_error() {
        let start = Instant::now();
        for ending in ["cancelled", "refused"] {
            let mut waits = WaitAccounting::default();
            waits.observe(&Wait::Clean, 2, start);
            waits.stop(start + Duration::from_millis(250));
            let expected = format!("oxide: waited 0.3s (clean 0.3s); {ending}");
            assert_eq!(
                waits.completion_line(ending).as_deref(),
                Some(expected.as_str())
            );
        }
    }

    #[test]
    fn wait_accounting_bounds_reason_announcements_and_omits_unobserved_waits() {
        let mut waits = WaitAccounting::default();
        assert_eq!(waits.summary_suffix(), None);
        let start = Instant::now();
        let reasons = [
            Wait::Queue(1),
            Wait::HostLimit(2),
            Wait::Checkout,
            Wait::Clean,
            Wait::ProjectCap,
            Wait::Uncertain,
            Wait::Capacity(None),
        ];
        for reason in reasons {
            waits.observe(&reason, 2, start);
        }
        waits.observe(&Wait::Queue(2), 2, start + Duration::from_millis(7));
        assert_eq!(waits.reason_lines, 8);
        waits.observe(&Wait::HostLimit(1), 2, start + Duration::from_millis(8));
        assert_eq!(waits.reason_lines, 8);
        assert!(waits.suppression_announced);
        waits.observe(&Wait::Checkout, 2, start + Duration::from_millis(9));
        assert_eq!(waits.reason_lines, 8);
        assert!(waits.suppression_announced);
    }

    #[test]
    fn wait_age_prefers_creation_and_uses_modification_only_when_unavailable() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let created = now - Duration::from_secs(10);
        let modified = now - Duration::from_secs(20);
        let age = wait_age_from_times(Some(created), Some(modified), now).unwrap();
        assert_eq!(age.seconds, 10);
        assert!(matches!(age.source, WaitAgeSource::Created));

        let age = wait_age_from_times(None, Some(modified), now).unwrap();
        assert_eq!(age.seconds, 20);
        assert!(matches!(age.source, WaitAgeSource::Modified));
        assert!(
            wait_age_from_times(Some(now + Duration::from_secs(1)), Some(modified), now).is_none()
        );
        assert!(wait_age_from_times(None, None, now).is_none());
    }

    #[test]
    fn wait_ticket_age_requires_private_regular_nonsymlink_metadata() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let (root, _, _) = paired_controls("wait-ticket-age");
        let ticket = root.join("ticket.lock");
        fs::write(&ticket, b"").unwrap();
        fs::set_permissions(&ticket, fs::Permissions::from_mode(0o600)).unwrap();
        let age = ticket_age(&ticket, SystemTime::now()).unwrap();
        assert!(age.seconds <= 1);

        fs::set_permissions(&ticket, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(ticket_age(&ticket, SystemTime::now()).is_none());
        fs::set_permissions(&ticket, fs::Permissions::from_mode(0o600)).unwrap();
        let link = root.join("ticket-link.lock");
        symlink(&ticket, &link).unwrap();
        assert!(ticket_age(&link, SystemTime::now()).is_none());
        assert!(ticket_age(&root.join("missing.lock"), SystemTime::now()).is_none());
        fs::remove_dir_all(root).unwrap();
    }

    fn paired_controls(label: &str) -> (PathBuf, Store, Store) {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "oxide-{label}-{}-{}",
            std::process::id(),
            process::now_ms()
        ));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let store = |name: &str| Store {
            pool: root.join(name),
            backend: Backend::DirectoryBudget,
            payload: root.join(name).join("payload"),
            control: root.join(name).join(MANAGED_NAME),
            budget: None,
            recorded_budget: 1,
            max_active: 2,
            sibling: None,
        };
        let (internal, removable) = (store("internal"), store("removable"));
        for side in [&internal, &removable] {
            fs::create_dir_all(&side.pool).unwrap();
            side.ensure_control().unwrap();
        }
        let internal_control = internal.control.clone();
        let removable_control = removable.control.clone();
        (
            root,
            internal.with_sibling(&removable_control, false),
            removable.with_sibling(&internal_control, true),
        )
    }

    /// Output receipts are keyed in the internal pool's control directory,
    /// whichever pool a command's slots come from.
    #[test]
    fn owned_output_receipts_live_in_the_internal_control_directory() {
        let (root, internal, removable) = paired_controls("owner-control");
        assert_eq!(internal.owner_control(), internal.control());
        assert_eq!(removable.owner_control(), internal.control());
        fs::remove_dir_all(root).unwrap();
    }

    fn custody_record(id: u64, checkout: &Path, phase: Phase, lost: Option<Lost>) -> Record {
        Record {
            id,
            identity: "identity".to_owned(),
            compatibility: "compatibility".to_owned(),
            profile: "quick".to_owned(),
            checkout: Some(checkout.to_path_buf()),
            phase,
            process_group: None,
            last_used: 1,
            holder: None,
            lease_id: None,
            pending_recovery: None,
            retired: lost.as_ref().map(|lost| lost.describe(false)),
            lost,
            last_used_ms: Some(1),
            class: WorkspaceClass::Normal,
            class_reason: None,
        }
    }

    fn state_with(records: Vec<Record>) -> State {
        State {
            records,
            ..State::default()
        }
    }

    /// Only a possibly-writing command or an unresolved lost writer blocks a
    /// relink or clean; a retired slot that closed positively does not, and a
    /// caller never blocks on its own record.
    #[test]
    fn checkout_custody_separates_writers_unknown_and_retired_storage() {
        let (root, internal, removable) = paired_controls("custody");
        let checkout = Path::new("/checkout/custody");
        internal
            .save(&state_with(vec![custody_record(
                1,
                checkout,
                Phase::Running,
                None,
            )]))
            .unwrap();
        assert_eq!(
            internal.checkout_custody(checkout, 0).unwrap(),
            CheckoutCustody::Writers
        );
        assert_eq!(
            internal.checkout_custody(checkout, 1).unwrap(),
            CheckoutCustody::Clear
        );

        let unknown = Lost {
            slot_id: "3".to_owned(),
            at_ms: 2,
            unclosed: Some("exec may have started processes outside its group".to_owned()),
        };
        removable
            .save(&state_with(vec![custody_record(
                2,
                checkout,
                Phase::Retired,
                Some(unknown),
            )]))
            .unwrap();
        internal.save(&State::default()).unwrap();
        match internal.checkout_custody(checkout, 0).unwrap() {
            CheckoutCustody::Unknown(reason) => {
                assert!(reason.contains("never confirmed closed"), "{reason}")
            }
            other => panic!("expected unknown custody, got {other:?}"),
        }

        // A retired slot whose writers closed positively is not unknown.
        removable
            .save(&state_with(vec![custody_record(
                3,
                checkout,
                Phase::Retired,
                Some(Lost {
                    slot_id: "4".to_owned(),
                    at_ms: 3,
                    unclosed: None,
                }),
            )]))
            .unwrap();
        assert_eq!(
            internal.checkout_custody(checkout, 0).unwrap(),
            CheckoutCustody::Clear
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Record IDs are per pool: the caller's own ID exempts only its record in
    /// its own pool, never an equal ID in the other pool, in either direction
    /// and for running and quarantined records alike.
    #[test]
    fn checkout_custody_exempts_own_id_only_in_its_own_pool() {
        let (root, internal, removable) = paired_controls("custody-ids");
        let checkout = Path::new("/checkout/custody-ids");
        let id_seven = |phase| state_with(vec![custody_record(7, checkout, phase, None)]);

        internal.save(&id_seven(Phase::Acquiring)).unwrap();
        removable.save(&id_seven(Phase::Running)).unwrap();
        assert_eq!(
            internal.checkout_custody(checkout, 7).unwrap(),
            CheckoutCustody::Writers
        );
        assert_eq!(
            removable.checkout_custody(checkout, 7).unwrap(),
            CheckoutCustody::Clear
        );

        removable.save(&id_seven(Phase::Acquiring)).unwrap();
        internal.save(&id_seven(Phase::Spawning)).unwrap();
        assert_eq!(
            removable.checkout_custody(checkout, 7).unwrap(),
            CheckoutCustody::Writers
        );

        let quarantined = Lost {
            slot_id: "9".to_owned(),
            at_ms: 4,
            unclosed: Some("an allowed compiler wrapper may still run".to_owned()),
        };
        internal
            .save(&state_with(vec![custody_record(
                7,
                checkout,
                Phase::Retired,
                Some(quarantined),
            )]))
            .unwrap();
        match removable.checkout_custody(checkout, 7).unwrap() {
            CheckoutCustody::Unknown(reason) => {
                assert!(reason.contains("the other pool"), "{reason}")
            }
            other => panic!("expected unknown custody, got {other:?}"),
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// The fence is written to both pools' state (created when absent), a
    /// state parser without the field (as every pre-feature Oxide has)
    /// refuses it, the current one keeps loading it, and an unknown fence
    /// value refuses.
    #[test]
    fn the_owned_outputs_fence_refuses_a_pre_feature_parser() {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct PreFeatureState {
            version: u32,
            next_id: u64,
            next_use: u64,
            tickets: Vec<serde_json::Value>,
            #[serde(default)]
            cleans: Vec<serde_json::Value>,
            records: Vec<serde_json::Value>,
            #[serde(default)]
            recoveries: Vec<serde_json::Value>,
            #[serde(default)]
            flushes: Vec<serde_json::Value>,
        }
        let (root, internal, removable) = paired_controls("fence");
        let before = fs::read(internal.control.join("state.json")).ok();
        if let Some(bytes) = &before {
            assert!(serde_json::from_slice::<PreFeatureState>(bytes).is_ok());
        }
        let _ = fs::remove_file(removable.control.join("state.json"));
        internal.fence_owned_outputs().unwrap();
        internal.fence_owned_outputs().unwrap();
        for control in [&internal.control, &removable.control] {
            let text = fs::read_to_string(control.join("state.json")).unwrap();
            assert!(serde_json::from_str::<PreFeatureState>(&text).is_err());
            // Isolate the owned-outputs check from the earlier writer field.
            let writer = format!("\"minimum_writer_generation\":{WRITER_GENERATION},");
            let owned_only = text.replacen(&writer, "", 1);
            let refused = serde_json::from_str::<PreFeatureState>(&owned_only)
                .err()
                .expect("a pre-feature parser accepted the fenced state");
            assert!(refused.to_string().contains("owned_outputs"), "{refused}");
        }
        assert_eq!(
            internal.load().unwrap().owned_outputs.as_deref(),
            Some(OWNED_OUTPUTS_FENCE)
        );
        let mut state = internal.load().unwrap();
        state.owned_outputs = Some("oxide.outputs/9".to_owned());
        internal.save(&state).unwrap();
        assert!(internal.load().is_err());
        fs::remove_dir_all(root).unwrap();
    }

    /// The numeric state version advances only with the owned-outputs fence
    /// or its initialization record (oxide#129). An ordinary mutation must
    /// not refuse older binaries that have not opted into owned outputs.
    #[test]
    fn the_numeric_state_version_advances_only_with_the_output_fence() {
        let (root, internal, _removable) = paired_controls("version-gate");
        let state = internal.load().unwrap();
        internal.save(&state).unwrap();
        let doc: serde_json::Value =
            serde_json::from_slice(&fs::read(internal.control.join("state.json")).unwrap())
                .unwrap();
        assert_eq!(
            doc["version"].as_u64(),
            Some(u64::from(LEGACY_FIELDS_STATE_VERSION)),
            "an ordinary unfenced save advanced the numeric state version"
        );
        // Neither released fence is consent to a successor numeric format.
        let released = State {
            owned_outputs: Some(RELEASED_OWNED_OUTPUTS_FENCE.to_owned()),
            ..State::default()
        };
        internal.save(&released).unwrap();
        let doc: serde_json::Value =
            serde_json::from_slice(&fs::read(internal.control.join("state.json")).unwrap())
                .unwrap();
        assert_eq!(
            doc["version"].as_u64(),
            Some(u64::from(LEGACY_FIELDS_STATE_VERSION)),
            "a released /1 fence advanced the numeric state version"
        );
        // The published whole-target fence keeps version 3, so a state-3
        // binary still runs a pool it understands.
        let published = State {
            owned_outputs: Some(LEGACY_OWNED_OUTPUTS_FENCE.to_owned()),
            outputs_init: Some(OutputsInit::Ready {
                transaction: "b".repeat(32),
                ready_ms: 1,
            }),
            ..State::default()
        };
        internal.save(&published).unwrap();
        let doc: serde_json::Value =
            serde_json::from_slice(&fs::read(internal.control.join("state.json")).unwrap())
                .unwrap();
        assert_eq!(
            doc["version"].as_u64(),
            Some(u64::from(OLDER_STATE_VERSION)),
            "the published /2 fence left its numeric version"
        );
        // A pool that has not opted into owned outputs initializes from a
        // fresh state below.
        internal.save(&State::default()).unwrap();
        let transaction = "a".repeat(32);
        let created = std::cell::Cell::new(0);
        internal
            .initialize_outputs(&transaction, None, &|_, may_create| {
                assert!(may_create);
                created.set(created.get() + 1);
                Ok(())
            })
            .unwrap();
        assert_eq!(created.get(), 1);
        let state = internal.load().unwrap();
        assert_eq!(state.version, WRITER_STATE_VERSION);
        assert_eq!(state.minimum_writer_generation, Some(WRITER_GENERATION));
        assert!(matches!(
            state.outputs_init,
            Some(OutputsInit::Ready { .. })
        ));
        assert_eq!(state.owned_outputs.as_deref(), Some(OWNED_OUTPUTS_FENCE));
        // A ready pool validates the existing registry without creating it.
        internal
            .initialize_outputs(&transaction, None, &|_, may_create| {
                assert!(!may_create);
                Ok(())
            })
            .unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    /// Owned-outputs initialization requires positive writer closure in both
    /// pools under both admission locks: an active record (an older binary's
    /// in-flight command included), a lost slot with unclosed writers or
    /// another checkout's clean barrier refuses with nothing written, while
    /// the initializing checkout's own barrier is allowed.
    #[test]
    fn output_initialization_requires_closed_writers_in_both_pools() {
        let (root, internal, removable) = paired_controls("init-closure");
        let own = Path::new("/own");
        let transaction = "b".repeat(32);
        let refuses = |internal: &Store, label: &str| {
            let created = std::cell::Cell::new(false);
            let error = internal
                .initialize_outputs(&transaction, Some(own), &|_, _| {
                    created.set(true);
                    Ok(())
                })
                .unwrap_err();
            assert!(!created.get(), "{label}: the registry was created");
            assert!(internal.load().unwrap().outputs_init.is_none(), "{label}");
            assert!(internal.load().unwrap().owned_outputs.is_none(), "{label}");
            assert!(removable.load().unwrap().outputs_init.is_none(), "{label}");
            error
        };
        let mut active = State::default();
        active
            .records
            .push(record(Phase::Running, Some(i32::MAX), Some("v1.a.1.1.1")));
        removable.save(&active).unwrap();
        assert!(refuses(&internal, "active removable record").contains("active command record"));
        removable.save(&State::default()).unwrap();
        let mut lost = State::default();
        let mut retired = record(Phase::Retired, None, None);
        // A holder of this boot session: the restart fact never closes it.
        retired.holder.as_mut().unwrap().started_ms = process::now_ms();
        retired.lost = Some(Lost {
            slot_id: "1".to_owned(),
            at_ms: 1,
            unclosed: Some("never confirmed".to_owned()),
        });
        lost.records.push(retired);
        internal.save(&lost).unwrap();
        assert!(refuses(&internal, "unclosed lost slot").contains("never confirmed closed"));
        let mut other_clean = State::default();
        other_clean.cleans.push(Clean {
            id: 1,
            checkout: PathBuf::from("/other"),
            process_group: None,
        });
        internal.save(&other_clean).unwrap();
        assert!(refuses(&internal, "other clean barrier").contains("/other"));
        let mut own_clean = State::default();
        own_clean.cleans.push(Clean {
            id: 1,
            checkout: own.to_path_buf(),
            process_group: None,
        });
        internal.save(&own_clean).unwrap();
        removable.save(&own_clean).unwrap();
        internal
            .initialize_outputs(&transaction, Some(own), &|_, may_create| {
                assert!(may_create);
                Ok(())
            })
            .unwrap();
        for side in [&internal, &removable] {
            let state = side.load().unwrap();
            assert!(matches!(
                state.outputs_init,
                Some(OutputsInit::Ready { .. })
            ));
            assert_eq!(state.owned_outputs.as_deref(), Some(OWNED_OUTPUTS_FENCE));
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// Owned output format initialization accepts a quarantined lost slot in
    /// either pool when a restart ended its writers, and still refuses the
    /// same record when its holder started in this boot session.
    #[cfg(target_os = "macos")]
    #[test]
    fn output_initialization_accepts_a_lost_slot_from_an_earlier_boot() {
        let own = Path::new("/own");
        let transaction = "e".repeat(32);
        let quarantined = |started_ms: u64| {
            let mut entry = record(Phase::Retired, None, None);
            entry.holder.as_mut().unwrap().started_ms = started_ms;
            entry.holder.as_mut().unwrap().command = Some("exec tool".to_owned());
            entry.lost = Some(Lost {
                slot_id: "4".to_owned(),
                at_ms: 1,
                unclosed: Some("exec tool may have started processes outside its group".to_owned()),
            });
            entry
        };
        for label in ["internal", "paired"] {
            let (root, internal, removable) = paired_controls(&format!("init-boot-{label}"));
            let (side, other) = if label == "internal" {
                (&internal, &removable)
            } else {
                (&removable, &internal)
            };
            // This boot session's holder: unchanged refusal, in either pool.
            other.save(&State::default()).unwrap();
            side.save(&state_with(vec![quarantined(process::now_ms())]))
                .unwrap();
            let created = std::cell::Cell::new(false);
            let error = internal
                .initialize_outputs(&transaction, Some(own), &|_, _| {
                    created.set(true);
                    Ok(())
                })
                .unwrap_err();
            assert!(error.contains("never confirmed closed"), "{label}: {error}");
            assert!(!created.get(), "{label}: the registry was created");
            // A holder from an earlier boot session: the restart closed it,
            // so initialization proceeds and reads both pools the same way.
            side.save(&state_with(vec![quarantined(1)])).unwrap();
            internal
                .initialize_outputs(&transaction, Some(own), &|_, may_create| {
                    assert!(may_create);
                    Ok(())
                })
                .unwrap();
            for pool in [&internal, &removable] {
                let state = pool.load().unwrap();
                assert!(matches!(
                    state.outputs_init,
                    Some(OutputsInit::Ready { .. })
                ));
                assert_eq!(state.owned_outputs.as_deref(), Some(OWNED_OUTPUTS_FENCE));
            }
            // The record is still present until the next admission deletes
            // it whole; initialization never deletes a slot itself.
            assert_eq!(side.load().unwrap().records.len(), 1, "{label}");
            fs::remove_dir_all(root).unwrap();
        }
    }

    /// A quarantined lost slot whose holder started before this boot session
    /// is released and deleted whole through the existing path. A group
    /// number that is live now does not keep it: it belongs to no writer of an
    /// earlier session. The same record with a current-session holder is kept
    /// (the counterfactual).
    #[cfg(target_os = "macos")]
    #[test]
    fn a_lost_slot_from_an_earlier_boot_is_released_and_deleted_whole() {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "oxide-lost-boot-{}-{}",
            std::process::id(),
            process::now_ms()
        ));
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let pool = root.join("pool");
        init_budget_pool(&pool, 1 << 30).unwrap();
        let store = Store {
            control: pool.join(MANAGED_NAME),
            payload: pool.join("payload"),
            pool: pool.clone(),
            backend: Backend::DirectoryBudget,
            budget: Some(1 << 30),
            recorded_budget: 1 << 30,
            max_active: 16,
            sibling: None,
        };
        store.ensure_control().unwrap();
        let checkout = root.join("checkout");
        fs::create_dir(&checkout).unwrap();
        let key = SlotKey {
            checkout,
            identity: "identity".to_owned(),
            compatibility: "quick-key".to_owned(),
        };
        let leased = lease(&store, &key, None).expect("a slot");
        let record_id = leased.record_id();
        let slot_id = leased.slot_id.clone();
        let slot_dir = leased.build_dir.parent().expect("slot dir").to_path_buf();
        drop(leased);
        // SAFETY: getpgrp has no preconditions and this test process is in
        // its group, so the recorded group is genuinely not empty.
        let live_group = unsafe { libc::getpgrp() };
        let quarantine = |started_ms: u64| {
            let mut state = store.load().unwrap();
            let entry = state
                .records
                .iter_mut()
                .find(|entry| entry.id == record_id)
                .unwrap();
            entry.phase = Phase::Retired;
            entry.process_group = Some(live_group);
            entry.holder.as_mut().unwrap().started_ms = started_ms;
            entry.lost = Some(Lost {
                slot_id: slot_id.clone(),
                at_ms: 1,
                unclosed: Some("exec tool may have started processes outside its group".to_owned()),
            });
            store.save(&state).unwrap();
        };
        // Counterfactual: this session's holder is never closed by the boot
        // fact, whatever its recorded group is.
        quarantine(process::now_ms());
        let mut state = store.load().unwrap();
        store.discard_lost(&mut state).unwrap();
        assert!(slot_dir.is_dir(), "a current-session lost slot was deleted");
        assert_eq!(
            store.load().unwrap().records.len(),
            1,
            "a current-session lost record was forgotten"
        );
        // A holder from an earlier boot session: deleted whole, with its
        // record, despite the live group number.
        quarantine(1);
        let mut state = store.load().unwrap();
        store.discard_lost(&mut state).unwrap();
        // The decision is committed under the pool lock; Barge's eviction and
        // deletion are the settle's work, with that lock released.
        oxide_storage::lease::managed::settle_pending_evictions(&store.pool).unwrap();
        assert!(!slot_dir.exists(), "the boot-closed slot was not deleted");
        assert!(
            store.load().unwrap().records.is_empty(),
            "the boot-closed record was not forgotten"
        );
        assert!(
            store.ledger().unwrap().slots.is_empty(),
            "the boot-closed slot is still in the ledger"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// A paired removable control fenced by the released `oxide.outputs/1`
    /// binary or by the published `/2` fence carries no registry transaction
    /// on a host that last ran the released binary. Both predecessors migrate
    /// under the internal control's exact recorded transaction; an unknown
    /// fence and an open writer still refuse with both controls unchanged
    /// (oxide#159).
    #[test]
    fn a_paired_pool_fenced_by_a_predecessor_migrates_under_the_recorded_transaction() {
        let transaction = "c".repeat(32);
        // One definition decides the accepted predecessor set, so no branch
        // can narrow it back to `/2`-only.
        assert!(predecessor_owned_outputs_fence(LEGACY_OWNED_OUTPUTS_FENCE));
        assert!(predecessor_owned_outputs_fence(
            RELEASED_OWNED_OUTPUTS_FENCE
        ));
        assert!(!predecessor_owned_outputs_fence(OWNED_OUTPUTS_FENCE));
        assert!(!predecessor_owned_outputs_fence("oxide.outputs/9"));
        // Counterfactual: this is the paired `None` arm's condition before
        // oxide#159, which accepted only an absent or `/2` fence. It refuses
        // exactly the `/1` input migrated below, so a revert fails there too.
        let refused_before =
            |fence: Option<&str>| fence.is_some_and(|fence| fence != LEGACY_OWNED_OUTPUTS_FENCE);
        assert!(refused_before(Some(RELEASED_OWNED_OUTPUTS_FENCE)));
        assert!(!refused_before(Some(LEGACY_OWNED_OUTPUTS_FENCE)));
        assert!(!refused_before(None));

        // The released host state records both fences at numeric version 5;
        // the published `/2` predecessor keeps version 3.
        for (label, fence, recorded_accounting) in [
            (
                "paired-released",
                RELEASED_OWNED_OUTPUTS_FENCE,
                Some(RECORDED_ACCOUNTING_FENCE),
            ),
            ("paired-published", LEGACY_OWNED_OUTPUTS_FENCE, None),
        ] {
            let (root, internal, removable) = paired_controls(label);
            internal
                .save(&State {
                    owned_outputs: Some(OWNED_OUTPUTS_FENCE.to_owned()),
                    outputs_init: Some(OutputsInit::Ready {
                        transaction: transaction.clone(),
                        ready_ms: 1,
                    }),
                    recorded_accounting: recorded_accounting.map(str::to_owned),
                    ..State::default()
                })
                .unwrap();
            removable
                .save(&State {
                    owned_outputs: Some(fence.to_owned()),
                    recorded_accounting: recorded_accounting.map(str::to_owned),
                    ..State::default()
                })
                .unwrap();
            let before = removable.load().unwrap();
            assert_eq!(before.owned_outputs.as_deref(), Some(fence), "{label}");
            assert!(before.outputs_init.is_none(), "{label}");
            assert_eq!(
                before.version,
                if recorded_accounting.is_some() {
                    RECORDED_ACCOUNTING_STATE_VERSION
                } else {
                    OLDER_STATE_VERSION
                },
                "{label}: unexpected fixture state version"
            );

            let created = std::cell::Cell::new(0);
            let outcome = internal
                .initialize_outputs(&transaction, None, &|recorded, may_create| {
                    assert!(!may_create, "{label}: a ready control created a registry");
                    assert_eq!(recorded, transaction, "{label}");
                    created.set(created.get() + 1);
                    Ok(())
                })
                .unwrap();
            assert!(outcome, "{label}");
            assert_eq!(created.get(), 1, "{label}");

            let internal_state = internal.load().unwrap();
            assert_eq!(
                internal_state.owned_outputs.as_deref(),
                Some(OWNED_OUTPUTS_FENCE),
                "{label}"
            );
            let paired_state = removable.load().unwrap();
            assert_eq!(
                paired_state.owned_outputs.as_deref(),
                Some(OWNED_OUTPUTS_FENCE),
                "{label}: the paired fence was not upgraded"
            );
            for (side, state) in [("internal", internal_state), ("paired", paired_state)] {
                match &state.outputs_init {
                    Some(OutputsInit::Ready {
                        transaction: recorded,
                        ..
                    }) => assert_eq!(
                        recorded, &transaction,
                        "{label}: the {side} control lost the recorded transaction"
                    ),
                    other => panic!("{label}: the {side} control is not ready: {other:?}"),
                }
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    /// An unknown paired fence refuses at the shared state parser before any
    /// branch acts, and a released `/1` fence with an open writer still
    /// refuses through writer closure: neither case changes either control
    /// (oxide#159).
    #[test]
    fn an_unknown_paired_fence_and_an_open_writer_still_refuse() {
        let transaction = "d".repeat(32);
        // Both fixtures model older writable controls. A refused migration
        // leaves both checks and both documents unchanged.
        let ready_internal = |internal: &Store| {
            internal
                .save(&State {
                    owned_outputs: Some(OWNED_OUTPUTS_FENCE.to_owned()),
                    outputs_init: Some(OutputsInit::Ready {
                        transaction: transaction.clone(),
                        ready_ms: 1,
                    }),
                    recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
                    ..State::default()
                })
                .unwrap();
        };
        let snapshot = |side: &Store| fs::read(side.control.join("state.json")).unwrap();
        let refuses = |internal: &Store, removable: &Store, label: &str| {
            let (internal_before, paired_before) = (snapshot(internal), snapshot(removable));
            let error = internal
                .initialize_outputs(&transaction, None, &|_, _| -> Result<(), String> {
                    panic!("{label}: a refusing control created a registry")
                })
                .unwrap_err();
            assert_eq!(snapshot(internal), internal_before, "{label}");
            assert_eq!(snapshot(removable), paired_before, "{label}");
            error
        };

        let (root, internal, removable) = paired_controls("paired-unknown-fence");
        ready_internal(&internal);
        // `Store::load` is the single validation gate for every fence value
        // outside the accepted set, so this refuses before any branch can
        // read it as an empty record.
        removable
            .save(&State {
                owned_outputs: Some("oxide.outputs/9".to_owned()),
                recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
                ..State::default()
            })
            .unwrap();
        let error = refuses(&internal, &removable, "unknown paired fence");
        assert!(
            error.contains("invalid owned_outputs fence"),
            "unknown paired fence: {error}"
        );
        fs::remove_dir_all(root).unwrap();

        let (root, internal, removable) = paired_controls("paired-released-writer");
        ready_internal(&internal);
        let mut active = State {
            owned_outputs: Some(RELEASED_OWNED_OUTPUTS_FENCE.to_owned()),
            recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
            ..State::default()
        };
        active
            .records
            .push(record(Phase::Running, Some(i32::MAX), Some("v1.a.1.1.1")));
        removable.save(&active).unwrap();
        let error = refuses(&internal, &removable, "released fence with an open writer");
        assert!(
            error.contains("the paired pool holds an active command record"),
            "released fence with an open writer: {error}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// The crash window of the paired fence upgrade: the internal control
    /// already carries the successor fence while the paired control is still
    /// `Ready` for the same transaction under the predecessor fence. The
    /// upgrade must refuse while a paired command still holds a record.
    #[test]
    fn the_paired_ready_upgrade_refuses_an_open_writer() {
        let transaction = "e".repeat(32);
        let (root, internal, removable) = paired_controls("paired-ready-upgrade");
        internal
            .save(&State {
                owned_outputs: Some(OWNED_OUTPUTS_FENCE.to_owned()),
                outputs_init: Some(OutputsInit::Ready {
                    transaction: transaction.clone(),
                    ready_ms: 1,
                }),
                recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
                ..State::default()
            })
            .unwrap();
        let mut paired = State {
            owned_outputs: Some(LEGACY_OWNED_OUTPUTS_FENCE.to_owned()),
            outputs_init: Some(OutputsInit::Ready {
                transaction: transaction.clone(),
                ready_ms: 1,
            }),
            recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
            ..State::default()
        };
        paired
            .records
            .push(record(Phase::Running, Some(i32::MAX), Some("v1.a.1.1.1")));
        removable.save(&paired).unwrap();
        let paired_before = fs::read(removable.control.join("state.json")).unwrap();
        let error = internal
            .initialize_outputs(&transaction, None, &|_, _| -> Result<(), String> {
                panic!("a refusing control created a registry")
            })
            .unwrap_err();
        assert!(
            error.contains("the paired pool holds an active command record"),
            "{error}"
        );
        assert_eq!(
            fs::read(removable.control.join("state.json")).unwrap(),
            paired_before,
            "the refused upgrade left the paired control unchanged"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// An unreadable sibling control directory is unknown custody, never
    /// clear: the document looks absent through it, so the verdict must
    /// refuse instead of reading that as no records.
    #[test]
    fn checkout_custody_refuses_an_unreadable_sibling_state() {
        // SAFETY: geteuid has no preconditions; root reads through mode 000.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let (root, internal, removable) = paired_controls("custody-sibling-unreadable");
        removable.save(&State::default()).unwrap();
        fs::set_permissions(&removable.control, fs::Permissions::from_mode(0o000)).unwrap();
        let custody = internal.checkout_custody(Path::new("/checkout"), 0);
        fs::set_permissions(&removable.control, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(custody, Ok(CheckoutCustody::Unknown(_))),
            "{custody:?}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// A sibling document the shared parser refuses is unknown custody even
    /// when plain serde would accept it: the fence, the writer and every
    /// reader must agree on what a valid control document is.
    #[test]
    fn checkout_custody_refuses_a_sibling_state_the_parser_rejects() {
        let (root, internal, removable) = paired_controls("custody-sibling-invalid");
        let invalid = State {
            owned_outputs: Some("oxide.outputs/9".to_owned()),
            ..State::default()
        };
        write_private(
            &removable.control.join("state.json"),
            &serde_json::to_vec(&invalid).unwrap(),
        );
        let custody = internal
            .checkout_custody(Path::new("/checkout"), 0)
            .unwrap();
        assert!(
            matches!(custody, CheckoutCustody::Unknown(_)),
            "{custody:?}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// The bounded reader refuses a file over its limit rather than returning
    /// a truncated prefix, and an absent file is `None`, not an error.
    #[test]
    fn bounded_read_enforces_its_limit_and_reports_absence() {
        let (root, internal, _removable) = paired_controls("bounded-read");
        let path = internal.control.join("bounded.json");
        assert_eq!(bounded_read(&path, 4).unwrap(), None);
        write_private(&path, b"0123456789");
        let error = bounded_read(&path, 4).unwrap_err();
        assert!(error.contains("byte limit"), "{error}");
        assert_eq!(
            bounded_read(&path, 10).unwrap(),
            Some(b"0123456789".to_vec())
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// A holder of a checkout's clean barrier excludes only barriers when it
    /// rechecks writers: command records are still reported. The prune
    /// custody routes take the records-only shape through the fresh witness's
    /// `cleans` split; this pins the split at the shared formatter.
    #[test]
    fn active_records_omit_barriers_but_keep_commands() {
        let (root, internal, _removable) = paired_controls("active-records");
        let checkout = Path::new("/checkout");
        let mut state = State::default();
        state.cleans.push(Clean {
            id: 1,
            checkout: checkout.to_path_buf(),
            process_group: None,
        });
        internal.save(&state).unwrap();
        assert_eq!(
            active_holders(&internal.control, checkout).unwrap(),
            vec!["oxide clean barrier".to_owned()]
        );
        assert!(holders(&internal.control, checkout, false)
            .unwrap()
            .is_empty());
        let mut running = record(Phase::Running, Some(i32::MAX), Some("v1.a.1.1.1"));
        running.checkout = Some(checkout.to_path_buf());
        state.records.push(running);
        internal.save(&state).unwrap();
        assert_eq!(
            holders(&internal.control, checkout, false).unwrap().len(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// The shared `lock_admission` messages are the pre-S1 texts verbatim, so a
    /// dormant build reports exactly what it reported before this slice (gate
    /// `dormant_byte_identity`). The guarded probe keeps its own, precise
    /// wording; this test pins the shipped pair.
    #[test]
    fn the_shipped_admission_messages_are_unchanged() {
        let (root, internal, _removable) = paired_controls("shipped-admission-text");
        // An absent control directory names itself, exactly as before.
        let absent = Path::new("/nonexistent-oxide-control");
        let refused = lock_admission(absent).unwrap_err();
        assert_eq!(refused, "/nonexistent-oxide-control does not exist");
        // Any other failure to reach an open descriptor is reported with the
        // shipped "create did not succeed" text, not the OS error.
        let admission = internal.control.join("admission.lock");
        let elsewhere = root.join("elsewhere.lock");
        fs::write(&elsewhere, b"").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &admission).unwrap();
        let refused = lock_admission(&internal.control).unwrap_err();
        assert_eq!(
            refused,
            format!(
                "cannot open admission lock in {}: the create did not succeed",
                internal.control.display()
            )
        );
        // The guarded probe on the same path is precise instead, and never
        // reports absence for an unusable lock.
        let probed = probe_admission(&internal.control).unwrap_err();
        assert!(probed.contains("cannot open admission lock in"), "{probed}");
        assert!(!probed.contains("create did not succeed"), "{probed}");
        fs::remove_file(&admission).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    /// The control-state bytes of one pool, for a refusal that must change
    /// nothing (one-use S1: `next_id` included).
    fn state_bytes(store: &Store) -> Vec<u8> {
        fs::read(store.control.join("state.json")).unwrap()
    }

    /// The admission lock every pool that has ever run a mutating command
    /// carries: created and released, so a guarded barrier can take it without
    /// creating it.
    fn lock_pool(store: &Store) {
        drop(lock_admission(&store.control).unwrap());
        assert!(store.control.join("admission.lock").is_file());
    }

    /// A record whose lock file exists but is free: its holder ended without
    /// recovering it, so it is a *dead* record.
    fn dead_record(store: &Store, checkout: &Path, id: u64) {
        let mut state = store.load().unwrap();
        let mut entry = record(Phase::Running, None, Some("v1.a.1.1.1"));
        entry.id = id;
        entry.checkout = Some(checkout.to_path_buf());
        state.records.push(entry);
        store.save(&state).unwrap();
        // Create the lock file and drop it unlocked.
        let Probe::Free(file) = probe_lock(&store.lock_path(id), true).unwrap() else {
            panic!("a fresh record lock is free");
        };
        drop(file);
    }

    /// Hold one record's lock for the duration of the guard, the way a live
    /// command holds it.
    fn hold_record(store: &Store, checkout: &Path, id: u64) -> File {
        let mut state = store.load().unwrap();
        let mut entry = record(Phase::Running, None, Some("v1.a.1.1.1"));
        entry.id = id;
        entry.checkout = Some(checkout.to_path_buf());
        state.records.push(entry);
        store.save(&state).unwrap();
        let Probe::Free(file) = probe_lock(&store.lock_path(id), true).unwrap() else {
            panic!("a fresh record lock is free");
        };
        file
    }

    /// Barrier mode (`CleanMode::Barrier`) classifies a dead record as
    /// non-progress and refuses before allocating an id, a mark or a waiter
    /// lock: the state bytes, `next_id` included, are unchanged.
    #[test]
    fn the_guarded_barrier_refuses_a_dead_record_before_any_mark() {
        let (root, internal, _removable) = paired_controls("guarded-dead-record");
        lock_pool(&internal);
        let checkout = Path::new("/checkout");
        dead_record(&internal, checkout, 7);
        let before = state_bytes(&internal);
        let refused =
            match internal.begin_clean_barrier(checkout, &mut WaitAccounting::default(), || Ok(()))
            {
                Ok(Cleaning::Busy(reason)) => reason,
                _ => panic!("a dead record refuses the guarded barrier"),
            };
        assert!(refused.contains("clean barrier refused"), "{refused}");
        assert!(refused.contains("free lock"), "{refused}");
        assert_eq!(state_bytes(&internal), before);
        assert!(!internal.ticket_path(8).exists());
        // The dead lease is still present: nothing recovered it.
        let state = internal.load().unwrap();
        assert_eq!(state.records.len(), 1);
        assert_eq!(state.cleans.len(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    /// A dead clean mark is the same refusal: a free waiter lock with no live
    /// group never clears itself, so the guarded barrier refuses instead of
    /// waiting.
    #[test]
    fn the_guarded_barrier_refuses_a_dead_clean_mark() {
        let (root, internal, _removable) = paired_controls("guarded-dead-mark");
        lock_pool(&internal);
        let checkout = Path::new("/checkout");
        let mut state = State::default();
        state.cleans.push(Clean {
            id: 3,
            checkout: checkout.to_path_buf(),
            process_group: None,
        });
        internal.save(&state).unwrap();
        let before = state_bytes(&internal);
        let refused =
            match internal.begin_clean_barrier(checkout, &mut WaitAccounting::default(), || Ok(()))
            {
                Ok(Cleaning::Busy(reason)) => reason,
                _ => panic!("a dead clean mark refuses the guarded barrier"),
            };
        assert!(refused.contains("clean barrier refused"), "{refused}");
        assert!(refused.contains("clean mark 3"), "{refused}");
        assert_eq!(state_bytes(&internal), before);
        fs::remove_dir_all(root).unwrap();
    }

    /// A live holder is a *retry* fact (the holder can progress), and once it
    /// is gone the same record is a dead fact that refuses. Neither attempt
    /// writes anything.
    #[test]
    fn the_guarded_barrier_retries_a_live_holder_and_refuses_a_dead_one() {
        let (root, internal, _removable) = paired_controls("guarded-live-holder");
        lock_pool(&internal);
        let checkout = Path::new("/checkout");
        let held = hold_record(&internal, checkout, 5);
        let before = state_bytes(&internal);
        match internal.begin_clean_barrier(checkout, &mut WaitAccounting::default(), || Ok(())) {
            Ok(Cleaning::Retry(reason)) => assert!(reason.contains("record 5"), "{reason}"),
            _ => panic!("a live holder is retried, never marked"),
        }
        assert_eq!(state_bytes(&internal), before);
        assert!(internal.load().unwrap().cleans.is_empty());
        drop(held);
        // With the holder gone the same checkout is classified dead, so it
        // refuses: only an ordinary (recovering) command recovers it.
        let refused =
            match internal.begin_clean_barrier(checkout, &mut WaitAccounting::default(), || Ok(()))
            {
                Ok(Cleaning::Busy(reason)) => reason,
                _ => panic!("a dead record refuses the guarded barrier"),
            };
        assert!(refused.contains("clean barrier refused"), "{refused}");
        assert_eq!(state_bytes(&internal), before);
        fs::remove_dir_all(root).unwrap();
    }

    /// Any dead fact refuses even beside a live one (design §4.1 step 5): a
    /// dead record never clears itself without the shipped recovery, so
    /// waiting for the live holder could not make progress.
    #[test]
    fn a_dead_fact_outranks_a_live_holder() {
        let (root, internal, _removable) = paired_controls("guarded-dead-and-live");
        lock_pool(&internal);
        let checkout = Path::new("/checkout");
        let held = hold_record(&internal, checkout, 5);
        let mut state = internal.load().unwrap();
        state.cleans.push(Clean {
            id: 9,
            checkout: checkout.to_path_buf(),
            process_group: None,
        });
        internal.save(&state).unwrap();
        // A free waiter lock with no live group is dead.
        let before = state_bytes(&internal);
        match internal.begin_clean_barrier(checkout, &mut WaitAccounting::default(), || Ok(())) {
            Ok(Cleaning::Busy(reason)) => {
                assert!(reason.contains("clean mark 9"), "{reason}");
                assert!(reason.contains("clean barrier refused"), "{reason}");
            }
            _ => panic!("a dead clean mark refuses despite the live record"),
        }
        assert_eq!(state_bytes(&internal), before);
        drop(held);
        fs::remove_dir_all(root).unwrap();
    }

    /// A free record lock with a recorded live group is a **live** fact, not a
    /// dead one (design §4.1 step 5: the dead row also requires *no live
    /// group*). The group may still write through the slot, so the verb retries;
    /// with no group it is dead and refuses.
    #[test]
    fn a_record_with_a_dead_lock_and_a_live_group_is_retried() {
        let (root, internal, _removable) = paired_controls("guarded-free-lock-live-group");
        lock_pool(&internal);
        let checkout = Path::new("/checkout");
        // SAFETY: getpgid has no preconditions; the test process's own group is
        // alive for the duration of the test.
        let group = unsafe { libc::getpgid(0) };
        assert!(group > 1, "the test process has a live group");
        let mut state = State::default();
        let mut entry = record(Phase::Running, Some(group), Some("v1.a.1.1.1"));
        entry.id = 7;
        entry.checkout = Some(checkout.to_path_buf());
        state.records.push(entry);
        internal.save(&state).unwrap();
        // The holder ended: its lock file is free.
        let Probe::Free(file) = probe_lock(&internal.lock_path(7), true).unwrap() else {
            panic!("a fresh record lock is free");
        };
        drop(file);
        let before = state_bytes(&internal);
        match internal.begin_clean_barrier(checkout, &mut WaitAccounting::default(), || Ok(())) {
            Ok(Cleaning::Retry(reason)) => {
                assert!(reason.contains("record 7"), "{reason}");
                assert!(reason.contains("still alive"), "{reason}");
            }
            _ => panic!("a recorded live group is retried, never called dead"),
        }
        assert_eq!(state_bytes(&internal), before);
        // A missing lock file with the same live group is the same live fact.
        fs::remove_file(internal.lock_path(7)).unwrap();
        match internal.begin_clean_barrier(checkout, &mut WaitAccounting::default(), || Ok(())) {
            Ok(Cleaning::Retry(reason)) => assert!(reason.contains("still alive"), "{reason}"),
            _ => panic!("a missing lock with a live group is retried"),
        }
        // Non-vacuity: the same record with no recorded group is dead, so the
        // live group is what changed the classification.
        let mut state = internal.load().unwrap();
        state.records[0].process_group = None;
        internal.save(&state).unwrap();
        let before = state_bytes(&internal);
        match internal.begin_clean_barrier(checkout, &mut WaitAccounting::default(), || Ok(())) {
            Ok(Cleaning::Busy(reason)) => assert!(reason.contains("no live group"), "{reason}"),
            _ => panic!("a free lock with no group is dead"),
        }
        assert_eq!(state_bytes(&internal), before);
        fs::remove_dir_all(root).unwrap();
    }

    /// A held admission lock is a fact to retry, never a lock to block on:
    /// that is what makes it safe for a guarded verb to hold the checkout
    /// binding mutex across the attempt (the shipped order takes the barrier
    /// and then the binding mutex, so blocking here could deadlock).
    #[test]
    fn the_guarded_barrier_never_blocks_on_a_held_admission_lock() {
        let (root, internal, _removable) = paired_controls("guarded-admission");
        internal.save(&State::default()).unwrap();
        let held = lock_admission(&internal.control).unwrap();
        let before = state_bytes(&internal);
        match internal.begin_clean_barrier(
            Path::new("/checkout"),
            &mut WaitAccounting::default(),
            || Ok(()),
        ) {
            Ok(Cleaning::Retry(reason)) => assert!(reason.contains("admission"), "{reason}"),
            _ => panic!("a held admission lock is retried, never blocked on"),
        }
        assert_eq!(state_bytes(&internal), before);
        assert!(internal.load().unwrap().cleans.is_empty());
        drop(held);
        fs::remove_dir_all(root).unwrap();
    }

    /// Unreadable state is unknown custody: the guarded barrier refuses and
    /// leaves the bytes exactly as they were.
    #[test]
    fn the_guarded_barrier_refuses_unknown_custody() {
        let (root, internal, _removable) = paired_controls("guarded-unknown");
        lock_pool(&internal);
        internal.save(&State::default()).unwrap();
        fs::write(internal.control.join("state.json"), b"{not json").unwrap();
        let refused = match internal.begin_clean_barrier(
            Path::new("/checkout"),
            &mut WaitAccounting::default(),
            || Ok(()),
        ) {
            Err(reason) => reason,
            Ok(_) => panic!("unreadable state refuses the guarded barrier"),
        };
        assert!(refused.contains("clean barrier refused"), "{refused}");
        assert_eq!(
            fs::read(internal.control.join("state.json")).unwrap(),
            b"{not json"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// An unreadable *sibling* state is unknown custody, not "no records": a
    /// symlinked document cannot be read (`O_NOFOLLOW`), so the guarded barrier
    /// refuses before any mark instead of treating the other pool as clear.
    #[test]
    fn the_guarded_barrier_refuses_an_unreadable_sibling_state() {
        let (root, internal, removable) = paired_controls("guarded-sibling-unknown");
        lock_pool(&internal);
        removable.save(&State::default()).unwrap();
        let state = removable.control.join("state.json");
        let elsewhere = root.join("elsewhere.json");
        fs::write(&elsewhere, b"{}").unwrap();
        fs::remove_file(&state).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &state).unwrap();
        let refused = match internal.begin_clean_barrier(
            Path::new("/checkout"),
            &mut WaitAccounting::default(),
            || Ok(()),
        ) {
            Err(reason) => reason,
            Ok(_) => panic!("an unreadable sibling state refuses the guarded barrier"),
        };
        assert!(refused.contains("clean barrier refused"), "{refused}");
        assert!(refused.contains("other pool"), "{refused}");
        assert!(internal.load().unwrap().cleans.is_empty());
        // A **dangling** symlink is the same unknown custody, not absence.
        fs::remove_file(&state).unwrap();
        std::os::unix::fs::symlink(root.join("missing.json"), &state).unwrap();
        let refused = match internal.begin_clean_barrier(
            Path::new("/checkout"),
            &mut WaitAccounting::default(),
            || Ok(()),
        ) {
            Err(reason) => reason,
            Ok(_) => panic!("a dangling sibling state refuses the guarded barrier"),
        };
        assert!(refused.contains("clean barrier refused"), "{refused}");
        fs::remove_file(&state).unwrap();
        // An unreadable sibling control directory is unknown custody too.
        // SAFETY: geteuid has no preconditions; root reads through mode 000.
        if unsafe { libc::geteuid() } != 0 {
            fs::set_permissions(&removable.control, fs::Permissions::from_mode(0o000)).unwrap();
            let refused = match internal.begin_clean_barrier(
                Path::new("/checkout"),
                &mut WaitAccounting::default(),
                || Ok(()),
            ) {
                Err(reason) => reason,
                Ok(_) => panic!("an unreadable sibling control directory refuses"),
            };
            fs::set_permissions(&removable.control, fs::Permissions::from_mode(0o700)).unwrap();
            assert!(refused.contains("clean barrier refused"), "{refused}");
        }
        // Non-vacuity: with the document absent the same checkout is clear.
        match internal.begin_clean_barrier(
            Path::new("/checkout"),
            &mut WaitAccounting::default(),
            || Ok(()),
        ) {
            Ok(Cleaning::Guard(guard)) => internal.abandon_clean(guard).unwrap(),
            _ => panic!("an absent sibling state is clear"),
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// A **dangling sibling control directory** is unknown custody, not
    /// absence: reading `state.json` through it would report ENOENT and let the
    /// guarded barrier mark the checkout clear.
    #[test]
    fn the_guarded_barrier_refuses_a_dangling_sibling_control_directory() {
        let (root, internal, removable) = paired_controls("guarded-sibling-dangling");
        lock_pool(&internal);
        internal.save(&State::default()).unwrap();
        // Replace the sibling control directory with a symlink to a missing
        // directory: every open through it reports ENOENT.
        let control = removable.control.clone();
        fs::remove_dir_all(&control).unwrap();
        std::os::unix::fs::symlink(root.join("missing-control"), &control).unwrap();
        let refused = match internal.begin_clean_barrier(
            Path::new("/checkout"),
            &mut WaitAccounting::default(),
            || Ok(()),
        ) {
            Err(reason) => reason,
            Ok(_) => panic!("a dangling sibling control directory refuses"),
        };
        assert!(refused.contains("clean barrier refused"), "{refused}");
        assert!(refused.contains("other pool"), "{refused}");
        assert!(internal.load().unwrap().cleans.is_empty());
        // The same control path sitting there as a regular file is refused too.
        fs::remove_file(&control).unwrap();
        fs::write(&control, b"not a directory").unwrap();
        let refused = match internal.begin_clean_barrier(
            Path::new("/checkout"),
            &mut WaitAccounting::default(),
            || Ok(()),
        ) {
            Err(reason) => reason,
            Ok(_) => panic!("a non-directory sibling control path refuses"),
        };
        assert!(refused.contains("clean barrier refused"), "{refused}");
        fs::remove_file(&control).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    /// A symlinked (or otherwise unusable) admission lock is unknown custody,
    /// not "absent": the probe must refuse rather than report the pool as
    /// lock-free and retry.
    #[test]
    fn the_guarded_barrier_refuses_an_unusable_admission_lock() {
        let (root, internal, _removable) = paired_controls("guarded-bad-admission");
        internal.save(&State::default()).unwrap();
        let admission = internal.control.join("admission.lock");
        let elsewhere = root.join("elsewhere.lock");
        fs::write(&elsewhere, b"").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &admission).unwrap();
        let probed = probe_admission(&internal.control);
        assert!(probed.is_err(), "{probed:?}");
        let refused = match internal.begin_clean_barrier(
            Path::new("/checkout"),
            &mut WaitAccounting::default(),
            || Ok(()),
        ) {
            Err(reason) => reason,
            Ok(_) => panic!("an unusable admission lock refuses"),
        };
        assert!(refused.contains("clean barrier refused"), "{refused}");
        assert!(internal.load().unwrap().cleans.is_empty());
        assert!(
            fs::symlink_metadata(&admission)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the symlink is untouched"
        );
        fs::remove_file(&admission).unwrap();
        // Non-vacuity: with the symlink gone the lock is genuinely absent, and
        // absence is a retry that creates nothing.
        match internal.begin_clean_barrier(
            Path::new("/checkout"),
            &mut WaitAccounting::default(),
            || Ok(()),
        ) {
            Ok(Cleaning::Retry(reason)) => assert!(reason.contains("admission lock"), "{reason}"),
            _ => panic!("an absent admission lock is a retry"),
        }
        assert!(!admission.exists());
        fs::remove_dir_all(root).unwrap();
    }

    /// The guarded order (design §4.1 step 3): the caller's revalidation runs
    /// before any mark, and its refusal is returned unchanged with nothing
    /// allocated. The shipped `Recovering` mode never consults it.
    #[test]
    fn the_guarded_barrier_revalidates_before_any_mark() {
        let (root, internal, _removable) = paired_controls("guarded-revalidate");
        lock_pool(&internal);
        internal.save(&State::default()).unwrap();
        let before = state_bytes(&internal);
        let mut calls = 0;
        let refused = match internal.begin_clean_barrier(
            Path::new("/checkout"),
            &mut WaitAccounting::default(),
            || {
                calls += 1;
                Err("prepared output transition unresolved".to_owned())
            },
        ) {
            Err(reason) => reason,
            Ok(_) => panic!("the caller's revalidation refusal is returned"),
        };
        assert_eq!(refused, "prepared output transition unresolved");
        assert_eq!(calls, 1, "revalidated exactly once before refusing");
        assert_eq!(state_bytes(&internal), before);
        assert!(internal.load().unwrap().cleans.is_empty());

        // Recovering mode ignores the revalidation entirely and takes the
        // mark: the shipped order is unchanged.
        let mut calls = 0;
        let guard = match internal
            .begin_clean_with(
                Path::new("/checkout"),
                &mut WaitAccounting::default(),
                CleanMode::Recovering,
                || {
                    calls += 1;
                    Err("must not be consulted".to_owned())
                },
            )
            .unwrap()
        {
            Cleaning::Guard(guard) => guard,
            _ => panic!("a recovering barrier of an idle checkout acquires"),
        };
        assert_eq!(calls, 0);
        internal.abandon_clean(guard).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    /// The guarded barrier takes its mark on a clear checkout in `Barrier`
    /// mode too: the refusal path is the only one that writes nothing.
    #[test]
    fn the_guarded_barrier_marks_a_clear_checkout() {
        let (root, internal, _removable) = paired_controls("guarded-clear");
        lock_pool(&internal);
        internal.save(&State::default()).unwrap();
        let guard = match internal
            .begin_clean_barrier(
                Path::new("/checkout"),
                &mut WaitAccounting::default(),
                || Ok(()),
            )
            .unwrap()
        {
            Cleaning::Guard(guard) => guard,
            _ => panic!("a clear checkout is marked"),
        };
        let state = internal.load().unwrap();
        assert_eq!(state.cleans.len(), 1);
        assert_eq!(state.next_id, 2, "exactly one id for the proceeding verb");
        internal.abandon_clean(guard).unwrap();
        assert!(internal.load().unwrap().cleans.is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    /// A pool with no admission lock is a retry, and the guarded barrier never
    /// creates the lock: that is what makes "a refusal leaves only the checkout
    /// binding mutex" structural rather than a promise.
    #[test]
    fn the_guarded_barrier_creates_no_admission_lock() {
        let (root, internal, _removable) = paired_controls("guarded-no-admission");
        internal.save(&State::default()).unwrap();
        let admission = internal.control.join("admission.lock");
        assert!(!admission.exists(), "the fixture starts without the lock");
        for checkout in ["/checkout", "/other"] {
            match internal.begin_clean_barrier(
                Path::new(checkout),
                &mut WaitAccounting::default(),
                || Ok(()),
            ) {
                Ok(Cleaning::Retry(reason)) => {
                    assert!(reason.contains("admission lock"), "{reason}")
                }
                _ => panic!("a pool without an admission lock is retried"),
            }
            assert!(!admission.exists(), "the guarded barrier created the lock");
        }
        // Non-vacuity: the shipped recovering barrier does create it, exactly
        // as it always has.
        let guard = match internal
            .begin_clean(Path::new("/checkout"), &mut WaitAccounting::default())
            .unwrap()
        {
            Cleaning::Guard(guard) => guard,
            _ => panic!("the recovering barrier of an idle checkout acquires"),
        };
        assert!(admission.exists(), "the shipped barrier creates the lock");
        internal.abandon_clean(guard).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    /// The checkout clean barrier spans both pools: an internal clean waits
    /// for the checkout's active removable command, and a removable slot
    /// attempt sees a live internal clean under the internal admission lock.
    #[test]
    fn clean_barrier_spans_both_pools() {
        let (root, internal, removable) = paired_controls("clean-barrier");
        let checkout = Path::new("/checkout");
        let other = Path::new("/other");
        let mut state = State::default();
        state
            .records
            .push(record(Phase::Running, Some(i32::MAX), Some("v1.a.1.1.1")));
        removable.save(&state).unwrap();
        assert!(internal.sibling_busy(checkout).unwrap());
        assert!(!internal.sibling_busy(other).unwrap());
        let Cleaning::Guard(guard) = internal
            .begin_clean(other, &mut WaitAccounting::default())
            .unwrap()
        else {
            panic!("clean of an idle checkout was interrupted");
        };
        let (lock, cleaning) = removable.sibling_barrier(other).unwrap();
        assert!(lock.is_some() && cleaning);
        drop(lock);
        assert!(!removable.sibling_barrier(checkout).unwrap().1);
        internal.abandon_clean(guard).unwrap();
        assert!(!removable.sibling_barrier(other).unwrap().1);
        // The internal store itself never takes the other pool's lock.
        assert!(internal.sibling_barrier(other).unwrap().0.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn host_limit_counts_internal_and_removable_activity_in_both_routes() {
        let (root, internal, removable) = paired_controls("host-limit");
        let key = SlotKey {
            checkout: PathBuf::from("/checkout"),
            identity: "identity".to_owned(),
            compatibility: "quick-key".to_owned(),
        };
        // SAFETY: getpgrp has no preconditions.
        let group = unsafe { libc::getpgrp() };
        let mut active = State::default();
        active.records = vec![
            record(Phase::Running, Some(group), Some("v1.a.1.1.1")),
            record(Phase::Running, Some(group), Some("v1.a.2.1.2")),
        ];

        // A removable admission sees two active internal records.
        internal.save(&active).unwrap();
        let (ticket, _file) = removable.register_ticket().unwrap();
        assert!(matches!(
            removable.attempt(
                ticket,
                &key,
                Profile::Quick,
                2,
                WorkspaceAdmission::NORMAL,
                false,
                None
            ),
            Ok(Err(Wait::HostLimit(2)))
        ));
        removable.remove_ticket(ticket).unwrap();

        // An internal admission sees two active removable records.
        internal.save(&State::default()).unwrap();
        removable.save(&active).unwrap();
        let (ticket, _file) = internal.register_ticket().unwrap();
        assert!(matches!(
            internal.attempt(
                ticket,
                &key,
                Profile::Quick,
                2,
                WorkspaceAdmission::NORMAL,
                false,
                None
            ),
            Ok(Err(Wait::HostLimit(2)))
        ));
        internal.remove_ticket(ticket).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    fn lease(store: &Store, key: &SlotKey, label: Option<&str>) -> Result<Lease, Wait> {
        // Another test's `Command` spawn briefly duplicates every open
        // descriptor in the forked child before `exec` closes the
        // close-on-exec ones, so this fixture's admission lock can read as
        // busy for a few milliseconds. Only that transient uncertain outcome
        // is retried; every other wait is returned as before.
        for _ in 0..200 {
            let (ticket, _file) = store.register_ticket().unwrap();
            let outcome = store
                .attempt(
                    ticket,
                    key,
                    Profile::Quick,
                    16,
                    WorkspaceAdmission::NORMAL,
                    false,
                    label,
                )
                .unwrap();
            store.remove_ticket(ticket).unwrap();
            match outcome {
                Err(Wait::Uncertain) => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                other => return other,
            }
        }
        Err(Wait::Uncertain)
    }

    pub(crate) fn seed_fixture_lease(store: &Store, key: &SlotKey) -> Lease {
        lease(store, key, None).expect("fixture lease")
    }

    pub(crate) fn seed_fixture() -> (PathBuf, Store, SlotKey) {
        use std::ffi::{CStr, CString, OsStr};
        use std::os::unix::ffi::OsStrExt;
        let template = std::env::temp_dir().join("oxide-seed-front.XXXXXX");
        let mut name = CString::new(template.as_os_str().as_bytes())
            .unwrap()
            .into_bytes_with_nul();
        // SAFETY: name is writable, NUL-terminated and ends in XXXXXX.
        assert!(!unsafe { libc::mkdtemp(name.as_mut_ptr().cast()) }.is_null());
        let name = CStr::from_bytes_until_nul(&name).unwrap();
        let root = fs::canonicalize(PathBuf::from(OsStr::from_bytes(name.to_bytes()))).unwrap();
        let pool = root.join("pool");
        init_budget_pool(&pool, 1 << 30).unwrap();
        let store = Store {
            control: pool.join(MANAGED_NAME),
            payload: pool.join("payload"),
            pool: pool.clone(),
            backend: Backend::DirectoryBudget,
            budget: Some(1 << 30),
            recorded_budget: 1 << 30,
            max_active: 16,
            sibling: None,
        };
        store.ensure_control().unwrap();
        let checkout = root.join("checkout");
        fs::create_dir(&checkout).unwrap();
        let key = SlotKey {
            checkout,
            identity: "identity".into(),
            compatibility: "quick-key".into(),
        };
        (root, store, key)
    }

    fn seed_test_producer() -> oxide_storage::lease::managed::SeedProducer {
        oxide_storage::lease::managed::SeedProducer {
            cargo_version: "cargo".into(),
            rustc_digest: "digest".into(),
            rustc: "rustc".into(),
            target: "target".into(),
            recipe: "quick".into(),
            backend: "backend".into(),
            adapter: "adapter".into(),
            context_digest: "context".into(),
            config_files: vec![],
            flags_digest: "flags".into(),
            cargo_home: "/cargo".into(),
            command_family: "check".into(),
            command_class: "check".into(),
        }
    }

    #[test]
    fn seed_with_no_catalog_does_not_verify_sources_or_write_sidecars() {
        let (root, store, key) = seed_fixture();
        let held = lease(&store, &key, None).expect("fresh lease");
        let lock = format!("version = 4\n[[package]]\nname = 'leaf'\nversion = '1'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n", "a".repeat(64));
        let lock = crate::seed_lock::analyze(lock.as_bytes()).unwrap();
        let producer = seed_test_producer();
        let before = fs::read(store.control.join("state.json")).unwrap();
        let result = store.seed(&held, &producer, &lock, |_| {
            panic!("source verification ran")
        });
        assert!(matches!(result, Ok(SeedRun::Skipped("no eligible donor"))));
        assert_eq!(fs::read(store.control.join("state.json")).unwrap(), before);
        assert!(!store.control.join("seeds").exists());
        store.release(held, None).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_refused_catalog_after_a_successful_build_keeps_the_built_origin() {
        // A catalog the library refuses (here: the producing record is no
        // longer ours) must leave the successful generation's trusted origin
        // and slot intact; the caller ignores the refusal (§4.4, §9.1).
        let (root, store, donor_key) = seed_fixture();
        let mut donor = seed_fixture_lease(&store, &donor_key);
        let record_id = donor.record_id();
        let origin = Built::Affine(donor_key.checkout.clone());
        let lock = format!("version = 4\n[[package]]\nname = 'cfg-if'\nversion = '1.0.0'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n", "a".repeat(64));
        let lock = crate::seed_lock::analyze(lock.as_bytes()).unwrap();
        let cache = oxide_storage::seed_source::SourceCache::new(&donor.store_id).unwrap();
        let lease_id = std::mem::replace(&mut donor.lease_id, "lost-producer".into());
        let refused = store.publish_seed_catalog(
            &donor,
            seed_test_producer(),
            &lock,
            &[],
            &[],
            SystemTime::now(),
            process::now_ms(),
            true,
            Some(&origin),
            "debug",
            &cache,
        );
        assert!(refused.is_err());
        donor.lease_id = lease_id;
        assert!(!store
            .control
            .join("seeds")
            .join(format!("{}.catalog.json", donor.slot_id))
            .exists());
        store.release(donor, Some(origin)).unwrap();
        let state = store.load().unwrap();
        let record = state
            .records
            .iter()
            .find(|record| record.id == record_id)
            .unwrap();
        assert!(matches!(
            store.trusted_origin(record),
            Some(Built::Affine(path)) if path == donor_key.checkout
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn post_cargo_source_identity_rejects_a_changed_source_and_accepts_new_ones() {
        use crate::seed_plan::VerifiedSource;
        let lock = format!("version = 4\n[[package]]\nname = 'cfg-if'\nversion = '1.0.0'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n", "a".repeat(64));
        let package = crate::seed_lock::eligible(lock.as_bytes())
            .unwrap()
            .remove(0);
        let source = VerifiedSource {
            package: package.clone(),
            tree_digest: "b".repeat(64),
            kind: oxide_storage::lease::managed::SeedSourceKind::Registry,
            root: "/source".into(),
            verification_root: "/source".into(),
            proc_macro: false,
        };
        assert!(source_identity_matches(&source, &source));
        let mut changed = source.clone();
        changed.tree_digest = "c".repeat(64);
        assert!(!source_identity_matches(&source, &changed));
        let mut changed = source.clone();
        changed.root = "/other".into();
        assert!(!source_identity_matches(&source, &changed));
        let mut changed = source.clone();
        changed.proc_macro = true;
        assert!(!source_identity_matches(&source, &changed));
        let mut changed = source.clone();
        changed.kind = oxide_storage::lease::managed::SeedSourceKind::Git;
        assert!(!source_identity_matches(&source, &changed));
    }

    #[test]
    fn seed_with_an_empty_allowlist_skips_without_side_effects() {
        let (root, store, key) = seed_fixture();
        let held = lease(&store, &key, None).expect("fresh lease");
        let lock = crate::seed_lock::analyze(b"version = 4\n").unwrap();
        let before = fs::read(store.control.join("state.json")).unwrap();
        let producer = seed_test_producer();
        let result = store.seed(&held, &producer, &lock, |_| {
            panic!("source verification ran")
        });
        assert!(matches!(
            result,
            Ok(SeedRun::Skipped("no eligible locked packages"))
        ));
        assert_eq!(fs::read(store.control.join("state.json")).unwrap(), before);
        assert!(!store.control.join("seeds").exists());
        store.release(held, None).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_removable_store_without_its_registration_refuses_its_start_gate() {
        // §8.3: the removable start gate re-verifies the registration's
        // mount/volume-UID binding and reserve inside the admission lifetime.
        // A removable store with no paired internal control cannot prove that,
        // so its gate refuses before the attempt record or untrusted origin.
        let (root, mut store, _key) = seed_fixture();
        store.backend = Backend::ApfsVolume;
        let refused = store.seed_removable_gate();
        assert!(matches!(
            refused,
            Err(ref reason) if reason.contains("no paired internal control")
        ));
        assert!(!store.control.join("seeds").exists());
        fs::remove_dir_all(root).unwrap();
    }

    /// A released donor with a verified catalog that offers cfg-if.
    fn front_door_donor() -> (
        PathBuf,
        Store,
        SlotKey,
        crate::seed_lock::Analysis,
        oxide_storage::lease::managed::SeedProducer,
    ) {
        use oxide_storage::lease::managed::{SeedCatalog, SeedSourceKind};
        let (root, store, donor_key) = seed_fixture();
        let donor = lease(&store, &donor_key, None).expect("donor lease");
        let source_root = root.join("source");
        fs::create_dir_all(source_root.join("src")).unwrap();
        let input = source_root.join("src/lib.rs");
        fs::write(&input, "source").unwrap();
        let lock = format!("version = 4\n[[package]]\nname = 'cfg-if'\nversion = '1.0.0'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n", "a".repeat(64));
        let lock = crate::seed_lock::analyze(lock.as_bytes()).unwrap();
        let source = crate::seed_plan::VerifiedSource {
            package: lock.eligible[0].clone(),
            tree_digest: "b".repeat(64),
            kind: SeedSourceKind::Registry,
            root: source_root.clone(),
            verification_root: source_root,
            proc_macro: false,
        };
        let fingerprint = donor
            .build_dir
            .join("debug/.fingerprint/cfg-if-aaaaaaaaaaaaaaaa");
        fs::create_dir_all(&fingerprint).unwrap();
        for name in ["lib-cfg_if", "lib-cfg_if.json", "dep-lib-cfg_if"] {
            fs::write(fingerprint.join(name), "fingerprint").unwrap();
        }
        let deps = donor.build_dir.join("debug/deps");
        fs::create_dir_all(&deps).unwrap();
        let dep = deps.join("cfg_if-aaaaaaaaaaaaaaaa.d");
        let artifact = deps.join("libcfg_if-aaaaaaaaaaaaaaaa.rmeta");
        fs::write(&artifact, "artifact").unwrap();
        fs::write(
            &dep,
            format!(
                "{}: {}\n{}: {}\n{}:\n",
                dep.display(),
                input.display(),
                artifact.display(),
                input.display(),
                input.display()
            ),
        )
        .unwrap();
        let mut producer = seed_test_producer();
        producer.rustc_digest = "c".repeat(64);
        producer.context_digest = "d".repeat(64);
        producer.flags_digest = "e".repeat(64);
        let origin = Built::Affine(donor_key.checkout.clone());
        let cache = oxide_storage::seed_source::SourceCache::new(&donor.store_id).unwrap();
        store
            .publish_seed_catalog(
                &donor,
                producer.clone(),
                &lock,
                &[source.clone()],
                &[source.clone()],
                SystemTime::now(),
                process::now_ms(),
                true,
                Some(&origin),
                "debug",
                &cache,
            )
            .unwrap();
        let catalog_path = store
            .control
            .join("seeds")
            .join(format!("{}.catalog.json", donor.slot_id));
        let catalog = SeedCatalog::parse(&fs::read(&catalog_path).unwrap()).unwrap();
        assert_eq!(catalog.packages.len(), 1);
        assert_eq!(catalog.packages[0].units.len(), 1);
        assert_eq!(catalog.packages[0].units[0].files.len(), 5);
        assert!(store.control.join("seeds/sources.json").exists());
        store.release(donor, Some(origin)).unwrap();
        (root, store, donor_key, lock, producer)
    }

    #[test]
    fn a_source_failure_before_the_first_clone_skips_and_only_unknown_closure_refuses() {
        assert!(matches!(
            source_failure("injected source failure"),
            SourceFailure::Skip("injected source failure")
        ));
        assert!(matches!(
            source_failure("git source verification closure unknown"),
            SourceFailure::Refuse(reason) if reason == "git source verification closure unknown"
        ));
        let (root, store, _donor_key, lock, producer) = front_door_donor();
        let recipient_checkout = root.join("recipient");
        fs::create_dir(&recipient_checkout).unwrap();
        let recipient_key = SlotKey {
            checkout: recipient_checkout,
            identity: "recipient".into(),
            compatibility: "quick-key".into(),
        };
        let recipient = lease(&store, &recipient_key, None).expect("recipient lease");
        let before = fs::read(store.control.join("state.json")).unwrap();
        // A skip before the first clone writes no attempt and changes no state:
        // the command builds cold in the same empty slot.
        let skipped = store
            .seed(&recipient, &producer, &lock, |_| {
                Err(SourceFailure::Skip("injected source failure"))
            })
            .unwrap();
        assert!(matches!(
            skipped,
            SeedRun::Skipped("injected source failure")
        ));
        assert_eq!(fs::read(store.control.join("state.json")).unwrap(), before);
        assert!(!store
            .control
            .join("seeds")
            .join(format!("{}.attempt.json", recipient.slot_id))
            .exists());
        // Only an unknown Git closure refuses, and it still changes nothing.
        let refused = store.seed(&recipient, &producer, &lock, |_| {
            Err(SourceFailure::Refuse(
                "git source verification closure unknown".into(),
            ))
        });
        assert_eq!(
            refused.err(),
            Some("git source verification closure unknown".to_owned())
        );
        assert_eq!(fs::read(store.control.join("state.json")).unwrap(), before);
        store.release(recipient, None).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    /// Second-slot rule: a busy slot of this checkout's key sends a further
    /// command to another idle compatible slot or a new one; a slot whose
    /// holder died while its group lives is never reused; with no safely
    /// evictable idle slot under the project cap, a command waits only for a
    /// specific held compatible slot.
    #[test]
    fn a_busy_slot_sends_a_further_command_elsewhere() {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "oxide-second-slot-{}-{}",
            std::process::id(),
            process::now_ms()
        ));
        fs::create_dir_all(&root).unwrap();
        let pool = fs::canonicalize(&root).unwrap().join("pool");
        init_budget_pool(&pool, 1 << 30).unwrap();
        let store = Store {
            control: pool.join(MANAGED_NAME),
            payload: pool.join("payload"),
            pool: pool.clone(),
            backend: Backend::DirectoryBudget,
            budget: Some(1 << 30),
            recorded_budget: 1 << 30,
            max_active: 16,
            sibling: None,
        };
        store.ensure_control().unwrap();
        let checkout = fs::canonicalize(&root).unwrap().join("checkout");
        fs::create_dir(&checkout).unwrap();
        let key = SlotKey {
            checkout,
            identity: "identity".to_owned(),
            compatibility: "quick-key".to_owned(),
        };
        let first = lease(&store, &key, Some("exec tool")).expect("the first command leased");
        // The holder's group is this test's own, which is alive.
        // SAFETY: getpgrp has no preconditions.
        let group = unsafe { libc::getpgrp() };
        store.mark_spawning(&first).unwrap();
        store.mark_running(&first, group).unwrap();
        let second = lease(&store, &key, None).expect("a further command leased");
        assert_ne!(second.slot_id, first.slot_id);
        let report = store.report(Accounting::Recorded, 1).unwrap();
        let shown = |slot: &str| {
            report
                .slots
                .iter()
                .find(|view| view.slot_id == slot)
                .and_then(|view| view.holder.clone())
        };
        assert_eq!(shown(&first.slot_id).as_deref(), Some("exec tool"));
        assert_eq!(shown(&second.slot_id), None);

        // Fill the project to its cap with held records; each further lease
        // takes a new slot because the earlier ones are not idle.
        let mut held = vec![first];
        held.push(second);
        while held.len() < PROJECT_SLOT_CAP {
            held.push(lease(&store, &key, None).expect("a slot below the project cap"));
        }
        // Every project slot is held: a further command waits for a specific
        // held compatible slot, and no slot is evicted.
        assert!(matches!(lease(&store, &key, None), Err(Wait::Checkout)));

        // Release the newest held slot: it is the specific slot that can
        // become reusable, and the waiting rule takes it instead of a new one.
        let released = held.pop().expect("newest lease");
        let released_slot = released.slot_id.clone();
        store.release(released, None).unwrap();
        let reused = lease(&store, &key, None).expect("the released slot is taken");
        assert_eq!(reused.slot_id, released_slot);

        // The first holder's group is still alive: its slot stays held and is
        // never reused while the cap admits nothing else.
        let view = store.ledger().unwrap();
        let held_first = view
            .slots
            .iter()
            .find(|slot| slot.slot_id == held[0].slot_id)
            .unwrap();
        assert_eq!(held_first.state, SlotOccupancy::Held);
        assert!(matches!(lease(&store, &key, None), Err(Wait::Checkout)));
        store.release(reused, None).unwrap();
        for lease in held {
            drop(lease);
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// A run program that uses an owned temporary directory is still that
    /// directory's writer. The lease is the fence: while the holder's group
    /// is alive a competing command takes another slot and never reconciles
    /// the live directory, and only the holder's own release — which the
    /// product performs after positive group closure — removes it.
    #[test]
    fn a_live_program_keeps_its_owned_temporary_directory_from_a_competing_lease() {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "oxide-live-scratch-{}-{}",
            std::process::id(),
            process::now_ms()
        ));
        fs::create_dir_all(&root).unwrap();
        let pool = fs::canonicalize(&root).unwrap().join("pool");
        init_budget_pool(&pool, 1 << 30).unwrap();
        let store = Store {
            control: pool.join(MANAGED_NAME),
            payload: pool.join("payload"),
            pool: pool.clone(),
            backend: Backend::DirectoryBudget,
            budget: Some(1 << 30),
            recorded_budget: 1 << 30,
            max_active: 16,
            sibling: None,
        };
        store.ensure_control().unwrap();
        let checkout = fs::canonicalize(&root).unwrap().join("checkout");
        fs::create_dir(&checkout).unwrap();
        let key = SlotKey {
            checkout,
            identity: "identity".to_owned(),
            compatibility: "quick-key".to_owned(),
        };
        let mut live = lease(&store, &key, Some("exec run program")).expect("the run leased");
        let store_id = store.ledger().unwrap().store_id.clone();
        let slot_dir = live.build_dir.parent().expect("slot dir").to_path_buf();
        let scratch = crate::child_temp::choose_with(
            None,
            &store.payload,
            &slot_dir,
            crate::child_temp::Identity {
                store_id,
                slot_id: live.slot_id.clone(),
                incarnation: live.incarnation.clone(),
                lease_id: live.lease_id().to_owned(),
            },
            4096,
        )
        .expect("the run's owned directory");
        let owned = scratch.owned().expect("owned directory");
        assert!(owned.path().is_dir());
        fs::write(owned.path().join("program-marker"), b"live writer").unwrap();
        live.scratch = Some(owned.clone());
        // The program's own group is alive for the whole competing attempt.
        // SAFETY: getpgrp has no preconditions.
        let group = unsafe { libc::getpgrp() };
        store.mark_spawning(&live).unwrap();
        store.mark_running(&live, group).unwrap();

        // A competing command in the same checkout takes another slot and
        // leaves the live directory alone: it never reconciles, deletes or
        // re-creates it.
        let competing = lease(&store, &key, None).expect("a competing command leased");
        assert_ne!(competing.slot_id, live.slot_id);
        assert!(
            owned.path().is_dir(),
            "a competing lease removed the live program's directory"
        );
        assert_eq!(
            fs::read(owned.path().join("program-marker")).unwrap(),
            b"live writer"
        );
        // The held slot is not evictable by the project cap either.
        let view = store.ledger().unwrap();
        let held = view
            .slots
            .iter()
            .find(|slot| slot.slot_id == live.slot_id)
            .expect("the live slot is a ledger member");
        assert_eq!(held.state, SlotOccupancy::Held);
        store.release(competing, None).unwrap();
        assert!(owned.path().is_dir(), "releasing another slot touched it");

        // Only this holder's release removes it, which the product performs
        // after its own group is observed empty.
        store.release(live, None).unwrap();
        assert!(
            !owned.path().exists(),
            "the released holder kept its owned directory"
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn record(phase: Phase, group: Option<i32>, lease_id: Option<&str>) -> Record {
        Record {
            id: 1,
            identity: "identity".to_owned(),
            compatibility: "key".to_owned(),
            profile: "quick".to_owned(),
            checkout: Some(PathBuf::from("/checkout")),
            phase,
            process_group: group,
            last_used: 1,
            holder: Some(Holder {
                pid: 41,
                started_ms: 7,
                lease_holder: "oxide-41-3".to_owned(),
                command: None,
            }),
            lease_id: lease_id.map(str::to_owned),
            pending_recovery: None,
            retired: None,
            lost: None,
            last_used_ms: None,
            class: WorkspaceClass::Normal,
            class_reason: None,
        }
    }

    #[test]
    fn lost_volume_closure_needs_the_own_closed_group() {
        let mut own = record(Phase::Running, Some(77), Some("lease"));
        own.holder.as_mut().unwrap().pid = std::process::id();
        let bypassed = crate::wrapper::Policy::from_env().bypasses()
            && [
                "RUSTC_WORKSPACE_WRAPPER",
                "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
            ]
            .iter()
            .all(|name| std::env::var_os(name).is_none_or(|value| value.is_empty()));
        if bypassed {
            assert_eq!(unclosed_writers(&own), None);
        }
        // Another process's record, an exec tool or no holder: not closed.
        let other = record(Phase::Running, Some(77), Some("lease"));
        assert!(unclosed_writers(&other).unwrap().contains("oxide pid 41"));
        let mut exec = own.clone();
        exec.holder.as_mut().unwrap().command = Some("exec tool".to_owned());
        assert!(unclosed_writers(&exec).unwrap().contains("exec tool"));
        // Cargo never started: no writer.
        let acquiring = record(Phase::Acquiring, None, Some("lease"));
        assert_eq!(unclosed_writers(&acquiring), None);
    }

    #[test]
    fn a_lost_slot_is_never_active_or_reused() {
        let mut lost = record(Phase::Retired, Some(77), Some("lease"));
        lost.lost = Some(Lost {
            slot_id: "3".to_owned(),
            at_ms: 1,
            unclosed: None,
        });
        assert!(!lost.active());
        assert!(lost
            .lost
            .as_ref()
            .unwrap()
            .describe(false)
            .contains("deleted whole"));
        lost.lost.as_mut().unwrap().unclosed = Some("why".to_owned());
        assert!(lost
            .lost
            .as_ref()
            .unwrap()
            .describe(false)
            .starts_with("quarantined"));
        let encoded = serde_json::to_string(&lost).unwrap();
        let decoded: Record = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.lost.unwrap().unclosed.as_deref(), Some("why"));
    }

    /// A quarantined lost record with a holder start, for the boot-session
    /// closure table.
    fn quarantined_at(started_ms: u64) -> Record {
        let mut entry = record(Phase::Retired, None, Some("lease"));
        entry.holder.as_mut().unwrap().started_ms = started_ms;
        entry.holder.as_mut().unwrap().command = Some("exec tool".to_owned());
        entry.lost = Some(Lost {
            slot_id: "5".to_owned(),
            at_ms: 1,
            unclosed: Some("exec tool may have started processes outside its group".to_owned()),
        });
        entry
    }

    /// A restart ends every process: a quarantined holder that started before
    /// the current boot session is closed, one that started inside the safety
    /// margin or during this session is not.
    #[test]
    fn only_a_holder_from_an_earlier_boot_session_is_closed_by_the_restart() {
        let boot = 1_900_000_000_000_u64;
        let earlier = quarantined_at(boot - BOOT_MARGIN_MS - 1);
        assert!(lost_writers_closed(&earlier, Some(boot)));
        assert!(closed_by_boot(&earlier, Some(boot)));
        // The same record is still quoted and kept when the boot fact is
        // absent (the counterfactual: without the check nothing closes it).
        assert!(!lost_writers_closed(&earlier, None));
        assert!(!closed_by_boot(&earlier, None));
        for started in [boot - BOOT_MARGIN_MS, boot - 1, boot, boot + 1] {
            let near = quarantined_at(started);
            assert!(!lost_writers_closed(&near, Some(boot)), "started {started}");
            assert!(!closed_by_boot(&near, Some(boot)), "started {started}");
        }
        // A record that already closed positively is never reported as
        // closed by the boot session, whatever the boot time says.
        let mut positive = quarantined_at(0);
        positive.lost.as_mut().unwrap().unclosed = None;
        assert!(lost_writers_closed(&positive, None));
        assert!(!closed_by_boot(&positive, Some(boot)));
    }

    /// An unknown boot time, a missing holder and a zero start are never
    /// closure, and elapsed time is not a fact this reads.
    #[test]
    fn an_unknown_boot_time_or_a_missing_start_never_closes_a_lost_slot() {
        let boot = 1_900_000_000_000_u64;
        assert!(!lost_writers_closed(
            &quarantined_at(boot - BOOT_MARGIN_MS - 1),
            None
        ));
        assert!(!lost_writers_closed(&quarantined_at(0), Some(boot)));
        let mut missing = quarantined_at(boot - BOOT_MARGIN_MS - 1);
        missing.holder = None;
        assert!(!lost_writers_closed(&missing, Some(boot)));
        assert!(!closed_by_boot(&missing, Some(boot)));
        // A record without a lost entry is not a lost slot at all.
        assert!(!lost_writers_closed(
            &record(Phase::Idle, None, None),
            Some(boot)
        ));
    }

    /// The boot override works only inside the shared supervisor's own
    /// scratch/evidence/deadline layout; anything else leaves the real
    /// reading in force.
    #[cfg(debug_assertions)]
    #[test]
    fn the_boot_override_needs_the_supervisors_own_layout() {
        let root = Path::new("/scratch/oxide-proof.unit");
        let scratch = root.join("scratch");
        let evidence = root.join("evidence");
        assert!(supervisor_layout_paths(
            &scratch,
            &evidence,
            Some("1900000000000")
        ));
        for (label, scratch, evidence, deadline) in [
            (
                "evidence elsewhere",
                scratch.clone(),
                root.join("other"),
                Some("1"),
            ),
            (
                "nested evidence",
                scratch.clone(),
                scratch.join("evidence"),
                Some("1"),
            ),
            ("absent deadline", scratch.clone(), evidence.clone(), None),
            (
                "unparseable deadline",
                scratch.clone(),
                evidence.clone(),
                Some("soon"),
            ),
            (
                "not the supervisor leaf",
                root.to_path_buf(),
                evidence.clone(),
                Some("1"),
            ),
            (
                "not an oxide-proof root",
                Path::new("/scratch/other/scratch").to_path_buf(),
                Path::new("/scratch/other/evidence").to_path_buf(),
                Some("1"),
            ),
        ] {
            assert!(
                !supervisor_layout_paths(&scratch, &evidence, deadline),
                "{label}"
            );
        }
    }

    /// The debug proof control defines the reading only when it is present,
    /// and a present but unusable value is unknown rather than the real boot.
    #[cfg(debug_assertions)]
    #[test]
    fn a_present_boot_override_is_read_or_unknown_but_never_ignored() {
        assert_eq!(boot_override(None), None);
        assert_eq!(
            boot_override(Some("1900000000000")),
            Some(Some(1_900_000_000_000))
        );
        for unusable in ["unreadable", "0", "", "1900000000000 "] {
            assert_eq!(boot_override(Some(unusable)), Some(None), "{unusable:?}");
        }
    }

    fn held(holder: &str, lease_id: &str) -> LedgerSlot {
        LedgerSlot {
            slot_id: "1".to_owned(),
            incarnation: "1".to_owned(),
            state: SlotOccupancy::Held,
            project: PathBuf::from("/pool/oxide-managed/affinity/1"),
            workspace: PathBuf::from("/checkout"),
            profile: LeaseProfile::Dev,
            holder: Some(holder.to_owned()),
            lease_id: Some(lease_id.to_owned()),
            dir: None,
            rank: 0,
            recorded: None,
        }
    }

    #[test]
    fn recovers_only_an_empty_recorded_group() {
        let slot = held("oxide-41-3", "v1.a.1.1.1");
        let running = record(Phase::Running, Some(900), Some("v1.a.1.1.1"));
        assert_eq!(settle(&running, Some(&slot), |_| true), Settle::Recover);
        // A surviving member, or a reused or unreadable group ID, reports
        // anything but ESRCH.
        assert_eq!(settle(&running, Some(&slot), |_| false), Settle::Keep);
        let unrecorded = record(Phase::Spawning, None, Some("v1.a.1.1.1"));
        assert_eq!(settle(&unrecorded, Some(&slot), |_| true), Settle::Keep);
        let no_group = record(Phase::Running, None, Some("v1.a.1.1.1"));
        assert_eq!(settle(&no_group, Some(&slot), |_| true), Settle::Keep);
    }

    #[test]
    fn cargo_that_never_started_leaves_no_group() {
        let slot = held("oxide-41-3", "v1.a.1.1.1");
        let granted = record(Phase::Acquiring, None, Some("v1.a.1.1.1"));
        assert_eq!(settle(&granted, Some(&slot), |_| false), Settle::Recover);
        // Died between the ledger commit and saving the lease ID: the unique
        // holder string identifies the lease.
        let unsaved = record(Phase::Acquiring, None, None);
        assert_eq!(settle(&unsaved, Some(&slot), |_| false), Settle::Recover);
        assert_eq!(settle(&unsaved, None, |_| false), Settle::Reset);
    }

    #[test]
    fn never_recovers_a_lease_the_record_did_not_take() {
        let running = record(Phase::Running, Some(900), Some("v1.a.1.1.1"));
        let other = held("nucleus-task", "v1.a.1.2.5");
        assert_eq!(settle(&running, Some(&other), |_| true), Settle::Keep);
        let unsaved = record(Phase::Acquiring, None, None);
        assert_eq!(settle(&unsaved, Some(&other), |_| true), Settle::Keep);
        let mut anonymous = record(Phase::Running, Some(900), None);
        anonymous.holder = None;
        let slot = held("oxide-41-3", "v1.a.1.1.1");
        assert_eq!(settle(&anonymous, Some(&slot), |_| true), Settle::Keep);
    }

    #[test]
    fn state_without_holder_or_recoveries_still_loads() {
        let earlier = r#"{"version":1,"next_id":3,"next_use":2,"tickets":[],"records":[
            {"id":1,"identity":"i","compatibility":"k","profile":"quick",
             "checkout":"/checkout","phase":"running","process_group":900,"last_used":1}]}"#;
        let state: State = serde_json::from_str(earlier).expect("earlier state parses");
        assert!(state.recoveries.is_empty() && state.cleans.is_empty());
        let record = &state.records[0];
        assert!(record.holder.is_none() && record.lease_id.is_none());
        // Without a holder or lease ID the held lease cannot be proven its own.
        let slot = held("oxide-41-3", "v1.a.1.1.1");
        assert_eq!(settle(record, Some(&slot), |_| true), Settle::Keep);
        // Status must not promise automatic recovery for it.
        assert_eq!(return_path(record, &slot), Return::NotOwn);
    }

    #[test]
    fn status_path_matches_the_rule() {
        let slot = held("oxide-41-3", "v1.a.1.1.1");
        let running = record(Phase::Running, Some(900), Some("v1.a.1.1.1"));
        assert_eq!(return_path(&running, &slot), Return::GroupEmpty(900));
        let spawning = record(Phase::Spawning, None, Some("v1.a.1.1.1"));
        assert_eq!(return_path(&spawning, &slot), Return::GroupUnrecorded);
        let acquiring = record(Phase::Acquiring, None, None);
        assert_eq!(return_path(&acquiring, &slot), Return::NeverStarted);
        let other = held("nucleus-task", "v1.a.1.2.5");
        assert_eq!(return_path(&running, &other), Return::NotOwn);
    }

    #[test]
    fn pending_recovery_survives_a_crash_after_the_ledger_change() {
        let mut state = State::default();
        let mut crashed = record(Phase::Running, Some(900), Some("v1.a.1.1.1"));
        crashed.pending_recovery = Some(Recovery {
            slot_id: "1".to_owned(),
            lease_id: "v1.a.1.1.1".to_owned(),
            checkout: None,
            holder_pid: 41,
            holder_started_ms: 7,
            process_group: Some(900),
            recovered_ms: 9,
            recovered_by: 42,
        });
        let bytes = serde_json::to_vec(&crashed).expect("serialise");
        state
            .records
            .push(serde_json::from_slice(&bytes).expect("pending reloads"));
        // The ledger recovery committed, so the slot is no longer held.
        assert_eq!(settle(&state.records[0], None, |_| false), Settle::Reset);
        let pending = state.records[0].pending_recovery.take().expect("pending");
        state.log_recovery(pending);
        state.settle_idle(0);
        assert_eq!(state.recoveries.len(), 1);
        assert!(state.records[0].pending_recovery.is_none());
    }

    #[test]
    fn pool_cap_parses_decimal_and_binary_units() {
        let budget = Backend::DirectoryBudget;
        assert_eq!(parse_cap("100GB", budget), Ok(100_000_000_000));
        assert_eq!(parse_cap("1TiB", budget), Ok(1024 * GIB));
        assert_eq!(parse_cap("1024KiB", budget), Ok(1024 * 1024));
        assert_eq!(parse_cap("12MB", budget), Ok(12_000_000));
        assert_eq!(parse_cap("1048577", budget), Ok(1_048_577));
        for invalid in [
            "",
            "MiB",
            "1.5GiB",
            "-1",
            "64 MiB",
            "99999999999999999999",
            "17TiB",
            "0",
        ] {
            let error = parse_cap(invalid, budget).unwrap_err();
            assert!(error.contains("OXIDE_POOL_CAP"), "{error}");
        }
    }

    #[test]
    fn the_retired_image_backend_refuses_by_name() {
        let error = Backend::recorded("apfs-sparse-image-v1").unwrap_err();
        assert!(error.contains("retired"), "{error}");
        assert_eq!(
            Backend::recorded("directory-budget-v1"),
            Ok(Backend::DirectoryBudget)
        );
    }

    #[test]
    fn recovery_log_keeps_the_newest() {
        let mut state = State::default();
        for index in 0..MAX_RECOVERIES + 3 {
            state.log_recovery(Recovery {
                slot_id: index.to_string(),
                lease_id: format!("v1.a.{index}.1.1"),
                checkout: None,
                holder_pid: 41,
                holder_started_ms: 7,
                process_group: Some(900),
                recovered_ms: 9,
                recovered_by: 42,
            });
        }
        assert_eq!(state.recoveries.len(), MAX_RECOVERIES);
        assert_eq!(state.recoveries[0].slot_id, "3");
    }

    fn worst_record(id: u64) -> Record {
        Record {
            id,
            identity: "i".repeat(MAX_IDENTITY_BYTES),
            compatibility: "c".repeat(MAX_COMPATIBILITY_BYTES),
            profile: "p".repeat(MAX_PROFILE_BYTES),
            checkout: Some(PathBuf::from(format!(
                "/{}",
                "x".repeat(MAX_PATH_TEXT_BYTES - 1)
            ))),
            phase: Phase::Retired,
            process_group: Some(i32::MAX),
            last_used: u64::MAX,
            holder: Some(Holder {
                pid: u32::MAX,
                started_ms: u64::MAX,
                lease_holder: "h".repeat(MAX_LEASE_HOLDER_BYTES),
                command: Some("c".repeat(MAX_COMMAND_BYTES)),
            }),
            lease_id: Some("l".repeat(MAX_STATE_LEASE_ID_BYTES)),
            pending_recovery: Some(Recovery {
                slot_id: "s".repeat(MAX_STATE_SLOT_ID_BYTES),
                lease_id: "l".repeat(MAX_STATE_LEASE_ID_BYTES),
                checkout: Some(PathBuf::from(format!(
                    "/{}",
                    "x".repeat(MAX_PATH_TEXT_BYTES - 1)
                ))),
                holder_pid: 1,
                holder_started_ms: 2,
                process_group: Some(3),
                recovered_ms: 4,
                recovered_by: 5,
            }),
            retired: Some("r".repeat(MAX_DIAGNOSTIC_BYTES)),
            lost: Some(Lost {
                slot_id: "s".repeat(MAX_STATE_SLOT_ID_BYTES),
                at_ms: u64::MAX,
                unclosed: Some("u".repeat(MAX_DIAGNOSTIC_BYTES)),
            }),
            last_used_ms: Some(u64::MAX),
            class: WorkspaceClass::Normal,
            class_reason: None,
        }
    }

    fn escaped(value: usize) -> String {
        "\u{1}".repeat(value)
    }

    fn escaped_path(bytes: usize) -> PathBuf {
        PathBuf::from(format!("/{}", escaped(bytes - 1)))
    }

    fn escaped_record(id: u64) -> Record {
        Record {
            id,
            identity: escaped(MAX_IDENTITY_BYTES),
            compatibility: escaped(MAX_COMPATIBILITY_BYTES),
            profile: escaped(MAX_PROFILE_BYTES),
            checkout: Some(escaped_path(MAX_PATH_TEXT_BYTES)),
            phase: Phase::Retired,
            process_group: Some(i32::MAX),
            last_used: u64::MAX,
            holder: Some(Holder {
                pid: u32::MAX,
                started_ms: u64::MAX,
                lease_holder: escaped(MAX_LEASE_HOLDER_BYTES),
                command: Some(escaped(MAX_COMMAND_BYTES)),
            }),
            lease_id: Some(escaped(MAX_STATE_LEASE_ID_BYTES)),
            pending_recovery: Some(Recovery {
                slot_id: escaped(MAX_STATE_SLOT_ID_BYTES),
                lease_id: escaped(MAX_STATE_LEASE_ID_BYTES),
                checkout: Some(escaped_path(MAX_PATH_TEXT_BYTES)),
                holder_pid: 1,
                holder_started_ms: 2,
                process_group: Some(3),
                recovered_ms: 4,
                recovered_by: 5,
            }),
            retired: Some(escaped(MAX_DIAGNOSTIC_BYTES)),
            lost: Some(Lost {
                slot_id: escaped(MAX_STATE_SLOT_ID_BYTES),
                at_ms: u64::MAX,
                unclosed: Some(escaped(MAX_DIAGNOSTIC_BYTES)),
            }),
            last_used_ms: Some(u64::MAX),
            class: WorkspaceClass::Normal,
            class_reason: None,
        }
    }

    #[test]
    fn worst_escaped_control_state_roundtrips_through_save_and_load() {
        let (root, store, _removable) = paired_controls("control-escaped");
        // The current representation carries both forward fences, so this is
        // the worst valid version-5 document and not an owned-outputs-only one.
        let mut state = State {
            version: RECORDED_ACCOUNTING_STATE_VERSION,
            owned_outputs: Some(OWNED_OUTPUTS_FENCE.to_owned()),
            recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
            ..State::default()
        };
        for id in 1..=MAX_RECORDS as u64 {
            state.records.push(escaped_record(id));
        }
        for id in 1..=MAX_TICKETS as u64 {
            state.tickets.push(Ticket { id, pid: u32::MAX });
        }
        for id in 1..=MAX_CLEANS as u64 {
            state.cleans.push(Clean {
                id,
                checkout: escaped_path(MAX_PATH_TEXT_BYTES),
                process_group: Some(i32::MAX),
            });
        }
        for _ in 0..MAX_RECOVERIES {
            state.recoveries.push(Recovery {
                slot_id: escaped(MAX_STATE_SLOT_ID_BYTES),
                lease_id: escaped(MAX_STATE_LEASE_ID_BYTES),
                checkout: Some(escaped_path(MAX_PATH_TEXT_BYTES)),
                holder_pid: u32::MAX,
                holder_started_ms: u64::MAX,
                process_group: Some(i32::MAX),
                recovered_ms: u64::MAX,
                recovered_by: u32::MAX,
            });
            state.flushes.push(Flush {
                slot_id: escaped(MAX_STATE_SLOT_ID_BYTES),
                at_ms: u64::MAX,
                micros: None,
                error: Some(escaped(MAX_DIAGNOSTIC_BYTES)),
                lost: true,
            });
        }
        let before = serde_json::to_vec(&state).expect("serialize escaped worst valid state");
        assert!(
            before.len() as u64 <= MAX_STATE_BYTES,
            "escaped worst valid state {} exceeds {MAX_STATE_BYTES}",
            before.len()
        );
        assert!(
            before.len() > 4_000_000,
            "escaped fixture is not substantial: {}",
            before.len()
        );
        assert!(before.windows(6).any(|window| window == b"\\u0001"));

        store
            .save(&state)
            .expect("the escaped worst valid state saves");
        let loaded = store.load().expect("the escaped worst valid state loads");
        assert_eq!(loaded.version, RECORDED_ACCOUNTING_STATE_VERSION);
        assert_eq!(
            serde_json::to_vec(&loaded).expect("reserialize loaded"),
            before,
            "the escaped worst valid state did not round-trip byte for byte"
        );
        assert_eq!(loaded.records.len(), MAX_RECORDS);
        assert_eq!(loaded.records[0].checkout, state.records[0].checkout);
        assert_eq!(loaded.flushes.len(), MAX_FLUSHES);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worst_valid_control_state_fits_its_object_and_byte_caps() {
        // The current representation, so the bound covers the version-5 fence.
        let mut state = State {
            version: RECORDED_ACCOUNTING_STATE_VERSION,
            recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
            ..State::default()
        };
        for id in 1..=MAX_RECORDS as u64 {
            state.records.push(worst_record(id));
        }
        for id in 1..=MAX_TICKETS as u64 {
            state.tickets.push(Ticket { id, pid: u32::MAX });
        }
        for id in 1..=MAX_CLEANS as u64 {
            state.cleans.push(Clean {
                id,
                checkout: PathBuf::from(format!("/{}", "x".repeat(MAX_PATH_TEXT_BYTES - 1))),
                process_group: Some(i32::MAX),
            });
        }
        for index in 0..MAX_RECOVERIES {
            state.recoveries.push(Recovery {
                slot_id: "s".repeat(MAX_STATE_SLOT_ID_BYTES),
                lease_id: "l".repeat(MAX_STATE_LEASE_ID_BYTES),
                checkout: None,
                holder_pid: u32::MAX,
                holder_started_ms: u64::MAX,
                process_group: Some(i32::MAX),
                recovered_ms: u64::MAX,
                recovered_by: u32::MAX,
            });
            state.flushes.push(Flush {
                slot_id: "s".repeat(MAX_STATE_SLOT_ID_BYTES),
                at_ms: u64::MAX,
                micros: Some(u64::MAX),
                error: Some("e".repeat(MAX_DIAGNOSTIC_BYTES)),
                lost: index % 2 == 0,
            });
        }
        state.next_id = u64::MAX;
        state.next_use = u64::MAX;
        validate_state(&state, false).expect("the worst valid control state is accepted");
        let bytes = serde_json::to_vec(&state).expect("serialize");
        assert!(
            bytes.len() as u64 <= MAX_STATE_BYTES,
            "worst valid control state {} exceeds {MAX_STATE_BYTES}",
            bytes.len()
        );
        assert!(
            bytes.len() > 1_000_000,
            "fixture is not substantial: {}",
            bytes.len()
        );
        let object = serde_json::to_vec(&worst_record(1)).expect("record");
        assert!(object.len() <= MAX_RECORD_OBJECT_BYTES);
        // The class-bearing shape is the largest record this state can carry:
        // a `one_use` record with a full-length reason still fits the record
        // object ceiling, and a full record set of them still fits the file.
        let one_use = Record {
            class: WorkspaceClass::OneUse,
            class_reason: Some(escaped(MAX_DIAGNOSTIC_BYTES)),
            ..worst_record(1)
        };
        let object = serde_json::to_vec(&one_use).expect("one-use record");
        assert!(
            object.len() <= MAX_RECORD_OBJECT_BYTES,
            "a one-use record is {} bytes",
            object.len()
        );
        assert!(
            object.len() * MAX_RECORDS <= MAX_STATE_BYTES as usize,
            "a full set of one-use records exceeds the state bound"
        );
    }

    #[test]
    fn legacy_control_reads_keep_their_limits_and_save_keeps_the_previous_version_until_the_fence()
    {
        let (root, store, _removable) = paired_controls("control-fence");
        let mut legacy = State {
            version: LEGACY_STATE_VERSION,
            ..State::default()
        };
        legacy.records.push(Record {
            checkout: Some(PathBuf::from(format!(
                "/{}",
                "x".repeat(MAX_PATH_TEXT_BYTES + 64)
            ))),
            ..worst_record(1)
        });
        let bytes = serde_json::to_vec(&legacy).expect("legacy state");
        let path = store.control.join("state.json");
        write_private(&path, &bytes);
        let loaded = store.load().expect("legacy oversized field still reads");
        assert_eq!(loaded.version, LEGACY_STATE_VERSION);
        // A revised mutation validates every legacy field against the raised
        // caps and refuses with the original file untouched.
        let error = store.save(&loaded).unwrap_err();
        assert!(error.contains("field bounds"), "{error}");
        assert_eq!(
            fs::read(&path).unwrap(),
            bytes,
            "the original state was rewritten"
        );
        // Within the caps, an ordinary save stays on the previous numeric
        // version so older binaries are not refused before the owned-outputs
        // fence opts the pool in.
        let mut bounded = legacy.clone();
        bounded.records[0].checkout = Some(PathBuf::from("/checkout"));
        store.save(&bounded).expect("bounded legacy state saves");
        let stored: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            stored["version"],
            serde_json::json!(LEGACY_FIELDS_STATE_VERSION)
        );
        assert!(stored.get("owned_outputs").is_none());
        // The version *derivation*, proved on `save` itself so it cannot
        // depend on which caller reached it: an owned-outputs successor alone
        // is version 4, and only the recorded-accounting fence (oxide#146)
        // makes it 5.
        let owned_only = State {
            owned_outputs: Some(OWNED_OUTPUTS_FENCE.to_owned()),
            ..State::default()
        };
        store
            .save(&owned_only)
            .expect("an owned-outputs-only state saves");
        let stored: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored["version"], serde_json::json!(PREVIOUS_STATE_VERSION));
        assert_eq!(
            stored["owned_outputs"],
            serde_json::json!(OWNED_OUTPUTS_FENCE)
        );
        assert!(stored.get("recorded_accounting").is_none());
        let both = State {
            owned_outputs: Some(OWNED_OUTPUTS_FENCE.to_owned()),
            recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
            ..State::default()
        };
        store.save(&both).expect("a doubly fenced state saves");
        let stored: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            stored["version"],
            serde_json::json!(RECORDED_ACCOUNTING_STATE_VERSION)
        );
        assert_eq!(
            stored["recorded_accounting"],
            serde_json::json!(RECORDED_ACCOUNTING_FENCE)
        );
        store
            .save(&State::default())
            .expect("an unfenced state saves");

        // The *path* truth: `fence_owned_outputs` is a mutating command, so it
        // takes the admission gate, which makes the control fence durable
        // first. That is the ordering the design requires — the control
        // hand-off must precede any incompatible ledger write or locked pool
        // open — and it is why this path ends at version 8 with both fences.
        store
            .fence_owned_outputs()
            .expect("the forward fence saves");
        let stored: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored["version"], serde_json::json!(STATE_VERSION));
        assert_eq!(
            stored["owned_outputs"],
            serde_json::json!(OWNED_OUTPUTS_FENCE)
        );
        assert_eq!(
            stored["recorded_accounting"],
            serde_json::json!(RECORDED_ACCOUNTING_FENCE)
        );
        // A repeated admission fence is idempotent.
        let fenced = stored;
        store
            .fence_admission_state()
            .expect("a repeated fence is a no-op");
        store
            .fence_admission_state()
            .expect("a repeated fence is a no-op");
        let after: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(after, fenced, "a repeated fence rewrote the state");
        // Genuine absence at the current version refuses, and an explicit JSON null is a
        // present non-string field that the shared parser refuses too.
        let mut unfenced = fenced.clone();
        unfenced
            .as_object_mut()
            .expect("object")
            .remove("recorded_accounting");
        write_private(&path, &serde_json::to_vec(&unfenced).unwrap());
        let error = store.load().unwrap_err();
        assert!(
            error.contains("lacks the recorded_accounting fence"),
            "{error}"
        );
        let mut nulled = fenced.clone();
        nulled["recorded_accounting"] = serde_json::Value::Null;
        write_private(&path, &serde_json::to_vec(&nulled).unwrap());
        let error = store.load().unwrap_err();
        assert!(error.contains("not a string"), "{error}");
        let mut unknown = fenced.clone();
        unknown["recorded_accounting"] = serde_json::json!("oxide.accounting/9");
        write_private(&path, &serde_json::to_vec(&unknown).unwrap());
        let error = store.load().unwrap_err();
        assert!(
            error.contains("invalid recorded_accounting fence"),
            "{error}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_control_reads_keep_the_legacy_byte_limit() {
        let (root, store, _removable) = paired_controls("control-legacy-bytes");
        let mut legacy = State {
            version: LEGACY_STATE_VERSION,
            ..State::default()
        };
        legacy.records.push(Record {
            retired: Some("r".repeat(LEGACY_MAX_STATE_BYTES as usize)),
            ..worst_record(1)
        });
        let bytes = serde_json::to_vec(&legacy).expect("legacy state");
        assert!(bytes.len() as u64 > LEGACY_MAX_STATE_BYTES);
        write_private(&store.control.join("state.json"), &bytes);
        let error = store.load().unwrap_err();
        assert!(error.contains("byte limit"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    fn write_private(path: &Path, bytes: &[u8]) {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true).mode(0o600);
        let mut file = options.open(path).unwrap();
        file.write_all(bytes).unwrap();
    }

    fn ledger_slot(id: &str, incarnation: &str, dir: Option<&str>) -> LedgerSlot {
        LedgerSlot {
            slot_id: id.to_owned(),
            incarnation: incarnation.to_owned(),
            state: SlotOccupancy::Idle,
            project: PathBuf::from("/pool/oxide-managed/affinity/1"),
            workspace: PathBuf::from("/work/oxide"),
            profile: LeaseProfile::Dev,
            holder: None,
            lease_id: None,
            dir: dir.map(str::to_owned),
            rank: 0,
            recorded: None,
        }
    }

    fn observation(slot: &LedgerSlot, at_ms: u64) -> Observation {
        Observation {
            store_id: "a".repeat(32),
            slot_id: slot.slot_id.clone(),
            incarnation: slot.incarnation.clone(),
            dir: slot.dir.clone(),
            measured_at_ms: at_ms,
            allocated_bytes: 3 * 1024 * 1024,
            source: ObservationSource::LeaseReuse,
            lease_id: Some("v1.a.1.1.1".to_owned()),
        }
    }

    /// A slot whose last walk stopped partial is reported as
    /// `not measured (walk incomplete)` and never with a figure; a later
    /// complete walk of the same generation replaces that fact (oxide#186).
    #[test]
    fn an_incomplete_admission_walk_reads_as_not_measured() {
        let slot = ledger_slot("1", "7", Some("oxide-abcd/quick-1"));
        let incomplete = Observation {
            store_id: "a".repeat(32),
            slot_id: slot.slot_id.clone(),
            incarnation: slot.incarnation.clone(),
            dir: slot.dir.clone(),
            measured_at_ms: 10_000,
            allocated_bytes: 0,
            source: ObservationSource::AdmissionWalkIncomplete,
            lease_id: None,
        };
        let document = Observations {
            schema: OBSERVATIONS_SCHEMA.to_owned(),
            observations: vec![incomplete],
        };
        let recorded = recorded_size(&Ok(Some(document.clone())), &"a".repeat(32), &slot, 15_000);
        assert_eq!(recorded, SlotSize::NotMeasured(INCOMPLETE_WALK.to_owned()));
        assert_eq!(recorded.describe(), "not measured (walk incomplete)");
        assert!(
            !recorded.describe().contains("bytes") && !recorded.describe().contains('B'),
            "an incomplete walk claimed a size"
        );
        // Another generation, directory or store is not this slot's fact.
        for other in [
            ledger_slot("1", "8", Some("oxide-abcd/quick-1")),
            ledger_slot("1", "7", Some("oxide-abcd/quick-2")),
            ledger_slot("2", "7", Some("oxide-abcd/quick-1")),
        ] {
            assert!(
                matches!(
                    recorded_size(&Ok(Some(document.clone())), &"a".repeat(32), &other, 15_000),
                    SlotSize::NotMeasured(ref reason)
                        if reason.starts_with("not measured (no recorded observation")
                ),
                "another generation borrowed the incomplete fact"
            );
        }
    }

    /// The incomplete fact is written from the admission's own report, bound to
    /// the walked generation, and a later reuse measurement of the same
    /// generation replaces it (oxide#186).
    #[test]
    fn recording_an_incomplete_walk_says_not_measured_until_a_complete_walk() {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "oxide-incomplete-walk-{}-{}",
            std::process::id(),
            process::now_ms()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store {
            control: root.clone(),
            payload: root.join("payload"),
            pool: root.clone(),
            backend: Backend::DirectoryBudget,
            budget: None,
            recorded_budget: 1 << 30,
            max_active: 1,
            sibling: None,
        };
        let slot = ledger_slot("1", "1", Some("oxide-abcd/quick-1"));
        let store_id = "a".repeat(32);
        store.record_incomplete_walks(
            Some(&store_id),
            &[UnmeasuredSlot {
                slot_id: slot.slot_id.clone(),
                incarnation: slot.incarnation.clone(),
                dir: slot.dir.clone(),
            }],
        );
        let document = read_observations_for_tests(&root).unwrap().unwrap();
        assert_eq!(document.len(), 1);
        assert_eq!(
            document[0].source,
            ObservationSource::AdmissionWalkIncomplete
        );
        assert_eq!(document[0].allocated_bytes, 0, "a walk wrote a size");
        assert_eq!(document[0].slot_id, slot.slot_id);
        assert_eq!(document[0].incarnation, slot.incarnation);
        assert_eq!(document[0].dir, slot.dir);
        assert_eq!(document[0].lease_id, None);
        let recorded = recorded_size(&read_observations(&root), &store_id, &slot, 5);
        assert_eq!(recorded.describe(), "not measured (walk incomplete)");
        // A complete walk of the same generation records it normally.
        store
            .record_observation(observation(&slot, 20))
            .expect("record the complete walk");
        let recorded = recorded_size(&read_observations(&root), &store_id, &slot, 25);
        assert!(
            matches!(recorded, SlotSize::Recorded { .. }),
            "a complete walk did not supersede the incomplete fact: {recorded:?}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// A recorded fact is bound to exactly one slot generation: a different
    /// incarnation or recorded directory is not a size, and a future clock
    /// is an unknown age rather than a fresh observation.
    #[test]
    fn recorded_sizes_bind_to_one_slot_generation() {
        let slot = ledger_slot("1", "7", Some("oxide-abcd/quick-1"));
        let document = Observations {
            schema: OBSERVATIONS_SCHEMA.to_owned(),
            observations: vec![observation(&slot, 10_000)],
        };
        let recorded = recorded_size(&Ok(Some(document)), &"a".repeat(32), &slot, 15_000);
        assert_eq!(
            recorded,
            SlotSize::Recorded {
                allocated_bytes: 3 * 1024 * 1024,
                age: RecordedAge::Ago(5),
                source: ObservationSource::LeaseReuse,
                lease_id: Some("v1.a.1.1.1".to_owned()),
            }
        );
        // A different incarnation, directory or store identity is a
        // different slot: not measured, never a stale size.
        for other in [
            ledger_slot("1", "8", Some("oxide-abcd/quick-1")),
            ledger_slot("1", "7", Some("oxide-abcd/quick-2")),
            ledger_slot("1", "7", None),
            ledger_slot("2", "7", Some("oxide-abcd/quick-1")),
        ] {
            assert!(
                matches!(
                    recorded_size(
                        &Ok(Some(Observations {
                            schema: OBSERVATIONS_SCHEMA.to_owned(),
                            observations: vec![observation(&slot, 10_000)],
                        })),
                        &"a".repeat(32),
                        &other,
                        15_000
                    ),
                    SlotSize::NotMeasured(_)
                ),
                "a different slot generation must not inherit a recorded size"
            );
        }
        assert!(matches!(
            recorded_size(&Ok(None), &"a".repeat(32), &slot, 15_000),
            SlotSize::NotMeasured(_)
        ));
        assert!(matches!(
            recorded_size(&Err("corrupt".to_owned()), &"a".repeat(32), &slot, 15_000),
            SlotSize::NotMeasured(_)
        ));
        let skew = recorded_size(
            &Ok(Some(Observations {
                schema: OBSERVATIONS_SCHEMA.to_owned(),
                observations: vec![observation(&slot, 20_000)],
            })),
            &"a".repeat(32),
            &slot,
            15_000,
        );
        assert!(matches!(
            skew,
            SlotSize::Recorded {
                age: RecordedAge::ClockSkew,
                ..
            }
        ));
    }

    /// Retention keeps at most [`MAX_OBSERVATIONS`] observations,
    /// deterministically choosing the most recent, so a pool at its slot
    /// ceiling still fits.
    #[test]
    fn observation_retention_keeps_the_newest_deterministically() {
        let count = MAX_OBSERVATIONS + 12;
        let mut observations: Vec<Observation> = (0..count)
            .map(|index| {
                let slot = ledger_slot(&format!("{index}"), "1", None);
                observation(&slot, 1_000 + index as u64)
            })
            .collect();
        retain_newest(&mut observations);
        assert_eq!(observations.len(), MAX_OBSERVATIONS);
        assert_eq!(
            observations.first().map(|item| item.slot_id.as_str()),
            Some("12")
        );
        let newest = (count - 1).to_string();
        assert_eq!(
            observations.last().map(|item| item.slot_id.as_str()),
            Some(newest.as_str())
        );
        let mut again: Vec<Observation> = (0..count)
            .rev()
            .map(|index| {
                let slot = ledger_slot(&format!("{index}"), "1", None);
                observation(&slot, 1_000 + index as u64)
            })
            .collect();
        retain_newest(&mut again);
        assert_eq!(observations, again, "retention is order independent");
        assert!(MAX_OBSERVATIONS >= oxide_storage::lease::MAX_SLOTS);
    }

    /// The sidecar is read strictly: unknown schema, a foreign or unreadable
    /// document and an over-retention file all refuse, which the view reports
    /// as not measured instead of scanning.
    #[test]
    fn observations_read_strictly_and_never_scan_as_fallback() {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "oxide-observations-{}-{}",
            std::process::id(),
            process::now_ms()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join(OBSERVATIONS_NAME);
        assert!(read_observations(&root).unwrap().is_none());

        let slot = ledger_slot("1", "1", None);
        let good = Observations {
            schema: OBSERVATIONS_SCHEMA.to_owned(),
            observations: vec![observation(&slot, 5)],
        };
        write_private(&path, &serde_json::to_vec(&good).unwrap());
        assert_eq!(
            read_observations_for_tests(&root)
                .unwrap()
                .map(|items| items.len()),
            Some(1)
        );

        write_private(
            &path,
            br#"{"schema":"oxide.status-observations/2","observations":[]}"#,
        );
        let error = read_observations(&root).unwrap_err();
        assert!(error.contains("schema"), "{error}");

        write_private(&path, b"not json");
        assert!(read_observations(&root).unwrap_err().contains("invalid"));

        let over = Observations {
            schema: OBSERVATIONS_SCHEMA.to_owned(),
            observations: (0..=MAX_OBSERVATIONS)
                .map(|index| {
                    let slot = ledger_slot(&format!("{index}"), "1", None);
                    observation(&slot, index as u64)
                })
                .collect(),
        };
        write_private(&path, &serde_json::to_vec(&over).unwrap());
        assert!(read_observations(&root)
            .unwrap_err()
            .contains("retention bound"));

        // A symlink or a world-readable file is foreign, never followed or
        // trusted.
        fs::remove_file(&path).unwrap();
        let target = root.join("elsewhere.json");
        write_private(&target, &serde_json::to_vec(&good).unwrap());
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(read_observations(&root).is_err());
        fs::remove_file(&path).unwrap();
        write_private(&path, &serde_json::to_vec(&good).unwrap());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_observations(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    /// Recording replaces one slot generation's fact, keeps the newest
    /// bounded set, and never rewrites a document it cannot read.
    #[test]
    fn recording_observations_is_bounded_and_never_repairs_a_foreign_file() {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "oxide-observation-write-{}-{}",
            std::process::id(),
            process::now_ms()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store {
            control: root.clone(),
            payload: root.join("payload"),
            pool: root.clone(),
            backend: Backend::DirectoryBudget,
            budget: None,
            recorded_budget: 1 << 30,
            max_active: 1,
            sibling: None,
        };
        let slot = ledger_slot("1", "1", Some("oxide-abcd/quick-1"));
        store
            .record_observation(observation(&slot, 10))
            .expect("record");
        let path = root.join(OBSERVATIONS_NAME);
        let first = fs::read(&path).unwrap();
        assert_eq!(
            read_observations_for_tests(&root)
                .unwrap()
                .map(|items| items.len()),
            Some(1)
        );
        // The same generation is replaced, not duplicated, and the write is
        // byte-stable for the same fact.
        store
            .record_observation(observation(&slot, 10))
            .expect("record");
        assert_eq!(fs::read(&path).unwrap(), first);
        let newer = observation(&slot, 20);
        store.record_observation(newer).expect("record");
        let document = read_observations_for_tests(&root).unwrap().unwrap();
        assert_eq!(document.len(), 1);
        assert_eq!(document[0].measured_at_ms, 20);

        // A file this version cannot read is left exactly as it is.
        write_private(&path, b"foreign");
        let foreign = fs::read(&path).unwrap();
        assert!(store.record_observation(observation(&slot, 30)).is_err());
        assert_eq!(fs::read(&path).unwrap(), foreign);

        // The worst case the file allows still fits its byte bound: every
        // retained observation with every bounded field at its ceiling.
        let mut document = Observations {
            schema: OBSERVATIONS_SCHEMA.to_owned(),
            observations: Vec::new(),
        };
        for index in 0..MAX_OBSERVATIONS {
            document.observations.push(Observation {
                store_id: "a".repeat(64),
                slot_id: format!("{:0>160}", index),
                incarnation: "1".repeat(64),
                dir: Some("d".repeat(200)),
                measured_at_ms: 9_999_999_999_999,
                allocated_bytes: u64::MAX,
                source: ObservationSource::LeaseReuse,
                lease_id: Some("l".repeat(160)),
            });
        }
        let bytes = serde_json::to_vec(&document).unwrap();
        assert!(
            bytes.len() as u64 <= MAX_OBSERVATIONS_BYTES,
            "{} > {}",
            bytes.len(),
            MAX_OBSERVATIONS_BYTES
        );
        fs::remove_dir_all(root).unwrap();
    }

    // ---------------------------------------------------------------------
    // One-use workspaces, slice S2 (design §5.1, §5.3, §5.4, §5.6, §5.7).
    //
    // Everything here is reachable from unit tests only: the feature is
    // dormant, so no production command can classify a workspace one-use and
    // no production path writes a class-bearing record. These regressions
    // therefore drive the seam directly (`WorkspaceAdmission::one_use`) and
    // assert both the class behaviour and the dormant byte identity.
    // ---------------------------------------------------------------------

    /// The pre-feature record shape, written exactly as a version-5 reader
    /// sees it: `deny_unknown_fields`, field for field, with neither class
    /// field. A normal document must round-trip through this and come back
    /// byte-identical, which is what makes the omission rule of §5.1 provable.
    mod v5 {
        use std::path::PathBuf;

        #[derive(serde::Serialize, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        pub(super) struct Record {
            pub id: u64,
            pub identity: String,
            pub compatibility: String,
            pub profile: String,
            pub checkout: Option<PathBuf>,
            pub phase: String,
            pub process_group: Option<i32>,
            pub last_used: u64,
            #[serde(default)]
            pub holder: Option<serde_json::Value>,
            #[serde(default)]
            pub lease_id: Option<String>,
            #[serde(default)]
            pub pending_recovery: Option<serde_json::Value>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub retired: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub lost: Option<serde_json::Value>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub last_used_ms: Option<u64>,
        }

        #[derive(serde::Serialize, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        pub(super) struct State {
            pub version: u32,
            pub next_id: u64,
            pub next_use: u64,
            pub tickets: Vec<serde_json::Value>,
            pub cleans: Vec<serde_json::Value>,
            pub records: Vec<Record>,
            pub recoveries: Vec<serde_json::Value>,
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            pub flushes: Vec<serde_json::Value>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub owned_outputs: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub outputs_init: Option<serde_json::Value>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub recorded_accounting: Option<String>,
        }

        /// Parse the document as a version-5 reader would, then re-serialize
        /// it; an absent class field survives exactly.
        pub(super) fn roundtrip(bytes: &[u8]) -> Vec<u8> {
            let state: State = serde_json::from_slice(bytes).expect("a v5 reader accepts it");
            serde_json::to_vec(&state).expect("serialize")
        }
    }

    /// Write a raw control document the way the store does: owner-only, so
    /// the private-file checks in `load` are exercised rather than bypassed.
    fn write_raw_state(store: &Store, bytes: &[u8]) {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let path = store.control.join("state.json");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(bytes).unwrap();
    }

    fn class_fixture(label: &str) -> (PathBuf, Store, SlotKey) {
        class_fixture_with_budget(label, 1 << 30)
    }

    fn class_fixture_with_budget(label: &str, budget: u64) -> (PathBuf, Store, SlotKey) {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "oxide-class-{label}-{}-{}",
            std::process::id(),
            process::now_ms()
        ));
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let pool = root.join("pool");
        init_budget_pool(&pool, budget).unwrap();
        let store = Store {
            control: pool.join(MANAGED_NAME),
            payload: pool.join("payload"),
            pool: pool.clone(),
            backend: Backend::DirectoryBudget,
            budget: Some(budget),
            recorded_budget: budget,
            max_active: 16,
            sibling: None,
        };
        store.ensure_control().unwrap();
        let checkout = root.join("checkout");
        fs::create_dir(&checkout).unwrap();
        let key = SlotKey {
            checkout,
            identity: "identity".to_owned(),
            compatibility: "quick-key".to_owned(),
        };
        (root, store, key)
    }

    #[test]
    fn a_legacy_match_past_the_first_batch_refuses_and_the_next_command_reuses_it() {
        let (root, store, key) = class_fixture("pending-legacy-adoption");
        let lease = lease(&store, &key, None).unwrap();
        let slot_id = lease.slot_id.clone();
        let affinity = store.affinity_dir(lease.record);
        store.release(lease, None).unwrap();

        let ledger_path = store.pool.join("ledger.json");
        let mut ledger: serde_json::Value =
            serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
        let store_id = ledger["store_id"].as_str().unwrap().to_owned();
        oxide_storage::lease::managed::evict_idle_slot(&store.pool, &slot_id).unwrap();

        let slots: Vec<_> = (1..=9)
            .map(|id| {
                let path = store.payload.join("slots").join(id.to_string());
                fs::create_dir_all(path.join("build")).unwrap();
                fs::write(path.join("build/artifact"), b"legacy").unwrap();
                let project = if id == 9 { &affinity } else { &key.checkout };
                serde_json::json!({
                    "id": id, "incarnation": 1, "state": "idle",
                    "project": project, "workspace": key.checkout, "profile": "dev",
                    "holder": format!("oxide-{}-1", libc::pid_t::MAX),
                    "lease_id": null, "last_lease_id": format!("v1.{store_id}.{id}.1.1"),
                    "rank": id,
                })
            })
            .collect();
        ledger["slots"] = slots.into();
        ledger["next_slot"] = 10.into();
        ledger["next_rank"] = 10.into();
        fs::write(&ledger_path, serde_json::to_vec(&ledger).unwrap()).unwrap();

        prepare_pool(&store.pool).unwrap();
        let path = store.payload.join("slots/9");
        assert!(!path.join(oxide_storage::storage::ROOT_MARKER).exists());
        let (ticket, _lock) = store.register_ticket().unwrap();
        let attempt = store.attempt(
            ticket,
            &key,
            Profile::Quick,
            16,
            WorkspaceAdmission::NORMAL,
            false,
            None,
        );
        let refusal = attempt
            .err()
            .expect("pending adoption finishes this attempt without entering the wait loop");
        assert!(
            refusal.contains("compatible legacy slot 9 is pending adoption"),
            "{refusal}"
        );
        assert!(refusal.contains("retry the Oxide command"), "{refusal}");
        assert!(!path.join(oxide_storage::storage::ROOT_MARKER).exists());
        store.remove_ticket(ticket).unwrap();

        prepare_pool(&store.pool).unwrap();
        let reused = match lease_as(&store, &key, WorkspaceAdmission::NORMAL, false) {
            Ok(reused) => reused,
            Err(wait) => panic!("the next command reuses the adopted row: {wait:?}"),
        };
        assert!(reused.reused);
        assert_eq!(reused.slot_id, "9");
        assert_eq!(
            fs::read(reused.build_dir.join("artifact")).unwrap(),
            b"legacy"
        );
        store.release(reused, None).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_ceiling_wait_keeps_the_held_and_pinned_diagnosis() {
        use oxide_storage::lease::{MAX_SLOTS, RAISED_CAPACITY_POLICY_VERSION};
        use oxide_storage::storage::{PoolBacking, PoolStorage};

        let (root, store, key) = class_fixture("pinned-ceiling-wait");
        let held = lease(&store, &key, None).unwrap();
        let storage = PoolStorage::open(
            &store.control,
            &PoolBacking::budget(&store.pool, store.recorded_budget),
        )
        .unwrap();
        let ledger_path = store.pool.join("ledger.json");
        let mut ledger: serde_json::Value =
            serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
        let store_id = ledger["store_id"].as_str().unwrap().to_owned();
        let slots = ledger["slots"].as_array_mut().unwrap();
        for id in 2..=MAX_SLOTS {
            let pinned = storage.create("pinned").unwrap();
            storage.pin_in(None, &pinned.id, "keep evidence").unwrap();
            slots.push(serde_json::json!({
                "id": id, "incarnation": 1, "state": "idle",
                "project": key.checkout, "workspace": key.checkout, "profile": "dev",
                "holder": format!("oxide-{}-{id}", std::process::id()), "lease_id": null, "last_lease_id": format!("v1.{store_id}.{id}.1.1"),
                "rank": id, "root": pinned.id, "dir": pinned.folder, "owned": true,
            }));
        }
        ledger["next_slot"] = (MAX_SLOTS + 1).into();
        ledger["next_rank"] = (MAX_SLOTS + 1).into();
        ledger["capacity_policy"] = RAISED_CAPACITY_POLICY_VERSION.into();
        fs::write(&ledger_path, serde_json::to_vec(&ledger).unwrap()).unwrap();

        let wait = match lease_as(&store, &key, WorkspaceAdmission::NORMAL, false) {
            Err(wait @ Wait::Capacity(Some(_))) => wait,
            Err(other) => panic!("expected the detailed ceiling wait, got {other:?}"),
            Ok(_) => panic!("admitted past the ceiling"),
        };
        let message = wait.describe(16);
        assert!(message.contains("1 held, 255 pinned"), "{message}");
        assert!(message.contains(r#"255 "keep evidence""#), "{message}");
        let mut waits = WaitAccounting::default();
        waits.observe(&Wait::Capacity(None), 16, Instant::now());
        waits.suppression_announced = true;
        waits.observe(&wait, 16, Instant::now());
        assert!(
            waits.ceiling_announced,
            "the first diagnosis bypasses generic wait suppression"
        );

        let held_slot = held.slot_id.clone();
        store.release(held, None).unwrap();
        let reused = match lease_as(&store, &key, WorkspaceAdmission::NORMAL, false) {
            Ok(reused) => reused,
            Err(wait) => panic!("the released slot makes progress at the ceiling: {wait:?}"),
        };
        assert!(reused.reused);
        assert_eq!(reused.slot_id, held_slot);
        store.release(reused, None).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    fn lease_as(
        store: &Store,
        key: &SlotKey,
        admission: WorkspaceAdmission<'_>,
        share: bool,
    ) -> Result<Lease, Wait> {
        let (ticket, _file) = store.register_ticket().unwrap();
        let outcome = store
            .attempt(ticket, key, Profile::Quick, 16, admission, share, None)
            .unwrap();
        store.remove_ticket(ticket).unwrap();
        outcome
    }

    /// The checkout's clean barrier still refuses while this checkout holds a
    /// lease and proceeds once it is released.
    /// The checkout's clean barrier still refuses while this checkout holds a
    /// lease (non-blocking Barrier mode) and proceeds once it is released.
    #[test]
    fn a_clean_barrier_refuses_a_held_lease_and_proceeds_after_release() {
        let (_root, store, key) = class_fixture("clean-barrier");
        let held = lease(&store, &key, None).unwrap();
        let mut waits = WaitAccounting::default();
        let refused = store
            .begin_clean_barrier(&key.checkout, &mut waits, || Ok(()))
            .unwrap();
        assert!(
            matches!(
                refused,
                Cleaning::Retry(_) | Cleaning::Busy(_) | Cleaning::Interrupted(_)
            ),
            "clean must refuse while the checkout holds a lease"
        );
        store.release(held, None).unwrap();
        match store
            .begin_clean_barrier(&key.checkout, &mut waits, || Ok(()))
            .unwrap()
        {
            Cleaning::Guard(guard) => store.abandon_clean(guard).unwrap(),
            _ => panic!("expected a clean guard after release"),
        }
    }

    #[test]
    fn warm_reuse_has_no_measurement_sidecar() {
        let (root, store, key) = class_fixture("warm-reuse-size");
        let first = lease(&store, &key, None).unwrap();
        let slot = first.slot_id.clone();
        store.release(first, None).unwrap();
        assert!(read_observations(&store.control).unwrap().is_none());
        let warm = lease(&store, &key, None).unwrap();
        assert_eq!(warm.slot_id, slot);
        assert!(warm.reused);
        assert!(warm.measured.is_none());
        assert_eq!(
            crate::leased_size(warm.measured.as_ref(), warm.reused),
            "slot size unknown"
        );
        assert!(read_observations(&store.control).unwrap().is_none());
        let ledger = store.ledger().unwrap();
        assert!(ledger.slots.iter().any(|row| row.slot_id == slot
            && row.state == SlotOccupancy::Held
            && row.recorded.is_none()));
        store.release(warm, None).unwrap();
        fs::remove_dir_all(root).unwrap();
        if std::env::var_os("OXIDE_WARM_REUSE_FIXTURE").is_some() {
            let evidence =
                PathBuf::from(std::env::var_os("OXIDE_PROOF_EVIDENCE").expect("proof evidence"));
            fs::write(
                evidence.join("warm-reuse-frontdoor.json"),
                b"{\"passed\":true}\n",
            )
            .unwrap();
        }
    }

    fn class_state_bytes(store: &Store) -> Vec<u8> {
        fs::read(store.control.join("state.json")).unwrap()
    }

    /// The presence-aware decode of §5.1 over the six inputs §16.1 names, plus
    /// both explicit default values. `#[serde(default)]` alone cannot tell
    /// `"class":"normal"` from an absent field, so this pass is what refuses a
    /// sub-6 document that carries either key with **any** value.
    #[test]
    fn a_sub_six_document_carrying_a_class_field_is_refused() {
        let (root, store, _key) = class_fixture("presence");
        let document = |version: u32, fence: bool, record_fields: &str| {
            let fence = if fence {
                ",\"one_use_workspaces\":\"oxide.one-use/1\""
            } else {
                ""
            };
            format!(
                "{{\"version\":{version},\"next_id\":1,\"next_use\":1,\"tickets\":[],\"cleans\":[],\"records\":[{{\"id\":1,\"identity\":\"identity\",\"compatibility\":\"quick-key\",\"profile\":\"quick\",\"checkout\":\"/checkout\",\"phase\":\"idle\",\"process_group\":null,\"last_used\":1,\"holder\":null,\"lease_id\":null,\"pending_recovery\":null,\"last_used_ms\":1{record_fields}}}],\"recoveries\":[],\"recorded_accounting\":\"oxide.accounting/1\"{fence}}}"
            )
        };
        let write = |bytes: String| write_raw_state(&store, bytes.as_bytes());
        let accept = |label: &str| match store.load() {
            Ok(state) => {
                assert_eq!(state.records.len(), 1, "{label}");
                state
            }
            Err(error) => panic!("{label}: {error}"),
        };
        let refuse = |label: &str, expected: &str| {
            let error = store
                .load()
                .err()
                .unwrap_or_else(|| panic!("{label}: accepted"));
            assert!(error.contains(expected), "{label}: {error}");
        };

        write(document(5, false, ""));
        let absent = accept("v5 absent");
        assert_eq!(absent.version, RECORDED_ACCOUNTING_STATE_VERSION);
        assert_eq!(absent.records[0].class, WorkspaceClass::Normal);

        write(document(5, false, ",\"class\":\"normal\""));
        refuse("v5 with an explicit normal class", "class field");
        write(document(5, false, ",\"class_reason\":null"));
        refuse("v5 with an explicit null reason", "class field");
        write(document(5, false, ",\"class\":\"one_use\""));
        refuse("v5 with a one-use class", "class field");

        write(document(6, true, ",\"class\":\"one_use\""));
        let fenced = accept("v6 with the fence and a class");
        assert_eq!(fenced.records[0].class, WorkspaceClass::OneUse);
        assert_eq!(fenced.version, ONE_USE_STATE_VERSION);

        write(document(6, false, ""));
        refuse("v6 without the fence", "lacks the one_use_workspaces fence");
        let version_seven = |fields: &str| {
            document(7, false, fields).replace(
                "\"recorded_accounting\":\"oxide.accounting/1\"",
                "\"recorded_accounting\":\"oxide.accounting/1\",\"eviction_accounting\":\"oxide.eviction-accounting/1\"",
            )
        };
        write(version_seven(""));
        accept("v7 normal with eviction fence");
        write(version_seven(",\"class\":\"normal\""));
        refuse("v7 explicit class without one-use fence", "class field");
        write(version_seven(",\"class_reason\":null"));
        refuse(
            "v7 explicit null reason without one-use fence",
            "class field",
        );
        // Both version/fence directions the design names: a fence below the
        // one-use version refuses, and an unknown fence value refuses.
        write(document(5, true, ""));
        refuse(
            "the one-use fence at version 5",
            "one_use_workspaces before version 6",
        );
        write(document(6, true, "").replace("oxide.one-use/1", "oxide.one-use/9"));
        refuse(
            "an unknown one-use fence value",
            "invalid one_use_workspaces fence",
        );

        // Non-vacuity of the pass: the same document with the class key
        // removed loads, so the refusal above is the field, not the shape.
        write(document(5, false, ""));
        accept("the control document without either field");
        fs::remove_dir_all(root).unwrap();
    }

    /// Fence first (oxide#177; Supervisor ruling 4c9984a1): the first mutating
    /// command makes version 8 durable at the admission gate, before any locked
    /// pool open or ledger write, for every pool — no slot-count or ledger-size
    /// condition. An already-version-8 control is not rewritten, and a later
    /// ordinary save keeps the version.
    #[test]
    fn an_admission_fences_the_control_state_to_version_eight() {
        let (root, store, _key) = class_fixture("fence-first-gate");
        store.admission().unwrap();
        let state = store.load().unwrap();
        assert_eq!(state.version, STATE_VERSION);
        assert_eq!(state.slot_ceiling.as_deref(), Some(SLOT_CEILING_FENCE));
        assert_eq!(
            state.recorded_accounting.as_deref(),
            Some(RECORDED_ACCOUNTING_FENCE)
        );

        // A second gate finds version 8 already durable: no write at all.
        let bytes = fs::read(store.control.join("state.json")).unwrap();
        store.admission().unwrap();
        assert_eq!(
            fs::read(store.control.join("state.json")).unwrap(),
            bytes,
            "an already-version-8 control was rewritten"
        );

        // Forward-only: a later ordinary save keeps the version.
        store.save(&state).unwrap();
        let again = store.load().unwrap();
        assert_eq!(again.version, STATE_VERSION);
        assert_eq!(again.slot_ceiling.as_deref(), Some(SLOT_CEILING_FENCE));
        fs::remove_dir_all(root).unwrap();
    }

    /// A pool with one idle slot made `Evicting` without a journal, its
    /// recorded slot root renamed aside so the recovery preflight finds it
    /// absent: the next locked open keeps the record and starts no journal,
    /// and renaming it back lets the following open journal and conclude. The
    /// fixture carries a version-5 control state (the recorded-accounting fence
    /// without the raised fence). Nothing in it is unreadable, so the shared
    /// supervisor's scratch measurement can always walk it. Returns the fixture
    /// root, the store, the key, the recorded slot root and the hidden path.
    fn pending_deletion_fixture(label: &str) -> (PathBuf, Store, SlotKey, PathBuf, PathBuf) {
        let (root, store, key) = class_fixture(label);
        let held = lease(&store, &key, None).expect("a slot");
        let build = held.build_dir.clone();
        store.release(held, None).expect("idle slot");

        // Make the recorded slot Evicting with no journal.
        let ledger_path = store.pool.join("ledger.json");
        let mut ledger: serde_json::Value =
            serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
        let slot = ledger["slots"]
            .as_array_mut()
            .expect("slots")
            .iter_mut()
            .next()
            .expect("one recorded slot");
        slot["state"] = serde_json::json!("evicting");
        slot["lease_id"] = serde_json::json!(null);
        slot.as_object_mut().expect("object").remove("recorded");
        fs::write(&ledger_path, serde_json::to_vec(&ledger).unwrap()).unwrap();

        // Move the recorded root aside: the preflight then finds it absent,
        // which keeps the `Evicting` record and starts no journal.
        let slot_root = build.parent().expect("the slot root").to_path_buf();
        let hidden = slot_root.with_file_name(format!(
            "{}.pending",
            slot_root
                .file_name()
                .expect("the slot leaf")
                .to_string_lossy()
        ));
        fs::rename(&slot_root, &hidden).expect("hide the pending slot root");

        downgrade_control_to_recorded_accounting(&store);
        (root, store, key, slot_root, hidden)
    }

    /// Rewrite a pool's control state as version 5 with only the
    /// recorded-accounting fence, the shape a predecessor leaves behind.
    fn downgrade_control_to_recorded_accounting(store: &Store) {
        let state_path = store.control.join("state.json");
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
        state["version"] = serde_json::json!(RECORDED_ACCOUNTING_STATE_VERSION);
        state["recorded_accounting"] = serde_json::json!(RECORDED_ACCOUNTING_FENCE);
        for field in [
            "slot_ceiling",
            "eviction_accounting",
            "one_use_workspaces",
            "holding",
        ] {
            state.as_object_mut().expect("object").remove(field);
        }
        write_raw_state(store, &serde_json::to_vec(&state).unwrap());
    }

    /// A pool with one idle slot made `Evicting` (no journal) whose root is
    /// ready for the recovery preflight, and a version-5 control state.
    /// Returns the fixture root, the store, the key and the ledger path.
    fn ready_pending_deletion_fixture(label: &str) -> (PathBuf, Store, SlotKey, PathBuf) {
        let (root, store, key) = class_fixture(label);
        let held = lease(&store, &key, None).expect("a slot");
        store.release(held, None).expect("idle slot");
        let ledger_path = store.pool.join("ledger.json");
        let mut ledger: serde_json::Value =
            serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
        let slot = ledger["slots"]
            .as_array_mut()
            .expect("slots")
            .iter_mut()
            .next()
            .expect("one recorded slot");
        slot["state"] = serde_json::json!("evicting");
        slot["lease_id"] = serde_json::json!(null);
        fs::write(&ledger_path, serde_json::to_vec(&ledger).unwrap()).unwrap();
        downgrade_control_to_recorded_accounting(&store);
        (root, store, key, ledger_path)
    }
    /// A fence-write failure in the real entry refuses before the recovery
    /// journal or any ledger write: the bytes stay identical (oxide#177).
    #[test]
    fn a_failed_real_open_fence_leaves_the_ledger_byte_identical() {
        let (root, store, _key, ledger_path) = ready_pending_deletion_fixture("open-fence-fails");
        let ledger_bytes = fs::read(&ledger_path).unwrap();
        let mode = fs::metadata(&store.control).unwrap().permissions().mode() & 0o777;
        fs::set_permissions(&store.control, fs::Permissions::from_mode(0o500)).unwrap();
        let result = Store::open_removable(&store.pool, store.payload.clone(), 8);
        assert!(result.is_err(), "a failed fence did not refuse the open");
        assert_eq!(
            fs::read(&ledger_path).unwrap(),
            ledger_bytes,
            "the ledger changed although the fence failed"
        );
        let after: serde_json::Value =
            serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
        let pending = after["slots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|slot| slot["id"] == 1)
            .expect("the pending record was dropped");
        assert_eq!(pending["state"], "evicting");
        assert!(
            pending.get("capture").is_none(),
            "a journal started although the fence failed"
        );
        fs::set_permissions(&store.control, fs::Permissions::from_mode(mode)).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    /// A failed fence write refuses at the gate with the ledger and journals
    /// byte-identical: the fence is the command's first durable effect.
    #[test]
    fn a_failed_fence_write_leaves_the_ledger_and_journals_byte_identical() {
        let (root, store, _key, _slot_root, hidden) =
            pending_deletion_fixture("fence-write-failure");
        let ledger_path = store.pool.join("ledger.json");
        let state_path = store.control.join("state.json");
        let ledger_bytes = fs::read(&ledger_path).unwrap();
        let state_bytes = fs::read(&state_path).unwrap();
        // Force the atomic state write to fail: its temp path is a directory.
        let temp = store.control.join("state.json.tmp");
        if temp.exists() {
            fs::remove_file(&temp).unwrap();
        }
        fs::create_dir(&temp).unwrap();

        let outcome = store.admission();
        assert!(outcome.is_err(), "a failed fence write did not refuse");
        assert_eq!(
            fs::read(&ledger_path).unwrap(),
            ledger_bytes,
            "a failed fence write changed the ledger"
        );
        assert_eq!(
            fs::read(&state_path).unwrap(),
            state_bytes,
            "a failed fence write changed the control state"
        );
        assert!(
            hidden.is_dir(),
            "a failed fence write touched the pending slot"
        );
        let ledger: serde_json::Value = serde_json::from_slice(&ledger_bytes).unwrap();
        assert!(
            ledger["slots"][0].get("capture").is_none(),
            "a failed fence write started a recovery journal"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// A version-8 control is newer than the one-use and eviction-accounting
    /// fences a predecessor knows, so a binary that only reads up to version 7
    /// refuses it before reading the ledger; this binary refuses an unknown
    /// version by the same rule (oxide#122, oxide#175, oxide#177).
    #[test]
    fn a_version_eight_control_is_newer_than_a_predecessor_knows() {
        assert!(
            STATE_VERSION > EVICTION_ACCOUNTING_STATE_VERSION
                && STATE_VERSION > ONE_USE_STATE_VERSION,
            "a predecessor that knows version 7 or 6 must see the current version as newer"
        );
        let (root, store, _key) = class_fixture("v7-predecessor");
        store.admission().unwrap();
        let bytes = fs::read(store.control.join("state.json")).unwrap();
        let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(document["version"], serde_json::json!(STATE_VERSION));
        assert_eq!(
            document["slot_ceiling"],
            serde_json::json!(SLOT_CEILING_FENCE)
        );
        assert_eq!(document["holding"], serde_json::json!(HOLDING_FENCE));

        // A predecessor knows at most version 7, so it refuses this real
        // version-8 document before reading the ledger. The model is local
        // because the test build has no older binary; it mirrors the one rule
        // such a reader applies (oxide#122).
        assert!(
            predecessor_accepts_control(&bytes).is_err(),
            "a predecessor accepted a current-version control"
        );
        // This binary applies the same rule to a version it does not know.
        let mut newer = document;
        newer["version"] = serde_json::json!(STATE_VERSION + 1);
        write_raw_state(&store, &serde_json::to_vec(&newer).unwrap());
        let error = store.load().unwrap_err();
        assert!(error.contains("unsupported version"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    /// Stand-in for an installed predecessor's strict control reader: it
    /// accepts the versions it knows and refuses anything newer before reading
    /// the ledger (oxide#122).
    fn predecessor_accepts_control(bytes: &[u8]) -> Result<(), String> {
        let document: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|error| format!("unparseable control state: {error}"))?;
        let version = document
            .get("version")
            .and_then(|version| version.as_u64())
            .ok_or("the control state has no numeric version")?;
        if version > u64::from(EVICTION_ACCOUNTING_STATE_VERSION) {
            return Err("Oxide state has an unsupported version".to_owned());
        }
        Ok(())
    }

    /// The fence's parser and the front door's `Store::load` are the same
    /// shared function (`oxide_storage::lease::control::parse_state`), so they
    /// must agree on every document the fence is asked to inspect.
    #[test]
    fn the_fence_parser_and_store_load_agree_on_control_documents() {
        use serde_json::json;
        let (root, store, key) = class_fixture("control-parity");
        let held = lease(&store, &key, None).expect("a slot");
        store.release(held, None).expect("idle slot");
        let path = store.control.join("state.json");
        let valid: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        // Each invalid case is a complete document with only the named field
        // wrong, so it cannot be rejected for a missing scalar first.
        let complete_v8 = json!({
            "version": 8,
            "next_id": 1,
            "next_use": 1,
            "tickets": [],
            "records": [],
            "cleans": [],
            "recorded_accounting": "oxide.accounting/1",
            "eviction_accounting": "oxide.eviction-accounting/1",
            "slot_ceiling": "oxide.slots/256",
        });
        let mut malformed = complete_v8.clone();
        malformed["records"] = json!(42);
        let unsupported = json!({
            "version": 0,
            "next_id": 1,
            "next_use": 1,
            "tickets": [],
            "records": [],
            "cleans": [],
        });
        let missing_one_use = json!({
            "version": 6,
            "next_id": 1,
            "next_use": 1,
            "tickets": [],
            "records": [],
            "cleans": [],
            "recorded_accounting": "oxide.accounting/1",
        });
        let mut non_string = complete_v8.clone();
        non_string["slot_ceiling"] = json!(42);
        let mut null_fence = complete_v8.clone();
        null_fence["slot_ceiling"] = serde_json::Value::Null;
        for (label, document) in [
            ("valid", valid),
            ("malformed records", malformed),
            ("unsupported version", unsupported),
            ("version 6 without one use", missing_one_use),
            ("non-string ceiling", non_string),
            ("null ceiling", null_fence),
        ] {
            let bytes = serde_json::to_vec(&document).unwrap();
            fs::write(&path, &bytes).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            let shared = oxide_storage::lease::control::parse_state(&bytes);
            let front_door = store.load();
            assert_eq!(
                shared.is_ok(),
                front_door.is_ok(),
                "{label}: the fence and Store::load disagree ({shared:?} vs {front_door:?})"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// A version-8 document always carries the raised-ceiling fence and a
    /// smaller document never does; an unknown fence value and a version-8
    /// document without the fence are states no writer produces. Version 8
    /// carries the eviction-accounting fence too and may carry the one-use
    /// fence, so a one-use pool at the raised ceiling keeps all three.
    /// Version 9 adds the whole-tree holding fence (oxide#190) on the same
    /// terms: a version-9 document always carries it, and a smaller pool never
    /// does, so an older binary hands off before it reads a holding row.
    #[test]
    fn the_raised_slot_ceiling_fence_and_version_are_equivalent() {
        let mut state = State::default();
        state.recorded_accounting = Some(RECORDED_ACCOUNTING_FENCE.to_owned());
        state.eviction_accounting = Some(EVICTION_ACCOUNTING_FENCE.to_owned());

        state.version = HOLDING_STATE_VERSION;
        assert!(
            validate_state(&state, false).is_err(),
            "the current version without the raised fence was accepted"
        );
        state.slot_ceiling = Some(SLOT_CEILING_FENCE.to_owned());
        assert!(
            validate_state(&state, false).is_err(),
            "the current version without the holding fence was accepted"
        );
        state.holding = Some(HOLDING_FENCE.to_owned());
        assert!(validate_state(&state, false).is_ok());

        // Version 8 is still exactly the raised-ceiling shape: it carries that
        // fence and neither the holding fence nor a holding row.
        state.version = SLOT_CEILING_STATE_VERSION;
        state.holding = None;
        assert!(validate_state(&state, false).is_ok());
        assert!(
            validate_state(
                &State {
                    holding: Some(HOLDING_FENCE.to_owned()),
                    ..state.clone()
                },
                false
            )
            .is_err(),
            "the holding fence at version 8 was accepted"
        );
        state.holding = Some(HOLDING_FENCE.to_owned());
        state.version = HOLDING_STATE_VERSION;
        state.slot_ceiling = None;
        assert!(
            validate_state(&state, false).is_err(),
            "the holding version without the raised fence was accepted"
        );
        state.slot_ceiling = Some(SLOT_CEILING_FENCE.to_owned());

        // Version 7 is the eviction-accounting shape without the raised fence.
        state.version = EVICTION_ACCOUNTING_STATE_VERSION;
        state.slot_ceiling = None;
        state.holding = None;
        assert!(validate_state(&state, false).is_ok());
        assert!(
            validate_state(
                &State {
                    slot_ceiling: Some(SLOT_CEILING_FENCE.to_owned()),
                    ..state.clone()
                },
                false
            )
            .is_err(),
            "the raised fence at version 7 was accepted"
        );

        // Version 6 requires the one-use fence and carries neither later one.
        state.version = ONE_USE_STATE_VERSION;
        state.eviction_accounting = None;
        assert!(
            validate_state(&state, false).is_err(),
            "version 6 without the one-use fence was accepted"
        );
        state.one_use_workspaces = Some(ONE_USE_FENCE.to_owned());
        assert!(
            validate_state(&state, false).is_ok(),
            "the one-use version must stay readable without the raised fence"
        );
        state.one_use_workspaces = None;

        // An unknown raised-ceiling fence, or a version-8 document without
        // it, is a state no writer produces.
        state.version = HOLDING_STATE_VERSION;
        state.eviction_accounting = Some(EVICTION_ACCOUNTING_FENCE.to_owned());
        state.slot_ceiling = Some("oxide.slots/128".to_owned());
        assert!(
            validate_state(&state, false).is_err(),
            "an unknown raised-ceiling fence was accepted"
        );

        state.slot_ceiling = Some(SLOT_CEILING_FENCE.to_owned());
        state.holding = Some(HOLDING_FENCE.to_owned());
        state.one_use_workspaces = Some(ONE_USE_FENCE.to_owned());
        assert!(
            validate_state(&state, false).is_ok(),
            "a one-use pool at the raised ceiling must keep every fence"
        );
    }

    /// The serializer omission rule of §5.1. A fresh normal record and a
    /// rewritten normal-only state print **neither** class field, a version-5
    /// reader's strict parse accepts the bytes and re-serializes them
    /// identically, and a `one_use` record always carries both the class and
    /// the durable fence at version 6.
    #[test]
    fn a_normal_record_writes_neither_class_field_and_a_one_use_record_always_fences() {
        let (root, store, key) = class_fixture("omission");
        // A rewritten normal-only state.
        store.save(&State::default()).unwrap();
        let bytes = class_state_bytes(&store);
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(!text.contains("\"class\""), "{text}");
        assert!(!text.contains("class_reason"), "{text}");
        assert_eq!(
            v5::roundtrip(&bytes),
            bytes,
            "a v5 reader changed the bytes"
        );

        // A freshly created normal record, written by a full lease cycle: the
        // admission gate's raised-ceiling fence puts the pool at version 8 and
        // no class field appears anywhere. A version-5 reader refuses the
        // document by version, which is the hand-off an older binary takes.
        let held = lease_as(&store, &key, WorkspaceAdmission::NORMAL, false).expect("lease");
        store.release(held, None).unwrap();
        let bytes = class_state_bytes(&store);
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(!text.contains("\"class\""), "{text}");
        assert!(!text.contains("class_reason"), "{text}");
        assert!(
            text.contains(&format!("\"version\":{STATE_VERSION}")),
            "a normal admission did not carry the current fenced version: {text}"
        );
        assert_eq!(
            store.load().unwrap().one_use_workspaces,
            None,
            "a normal admission wrote the one-use fence"
        );
        assert!(
            serde_json::from_slice::<v5::State>(&bytes).is_err(),
            "a version-5 reader accepted a current-version document"
        );

        // A one-use record carries the class, its reason and the fence; the
        // document stays at version 8 because it also carries the
        // raised-ceiling fence, and a class-bearing record requires at least
        // version 6.
        let held = lease_as(
            &store,
            &key,
            WorkspaceAdmission::one_use("a temporary workspace"),
            false,
        )
        .expect("one-use lease");
        let slot = held.slot_id.clone();
        store.release(held, None).unwrap();
        let state = store.load().unwrap();
        // The admission gate's raised-ceiling fence puts the document at
        // version 8; a class-bearing record still requires at least version 6.
        assert_eq!(state.version, STATE_VERSION);
        assert_eq!(state.one_use_workspaces.as_deref(), Some(ONE_USE_FENCE));
        let record = state
            .records
            .iter()
            .find(|record| record.class == WorkspaceClass::OneUse)
            .expect("the one-use record");
        assert_eq!(
            record.class_reason.as_deref(),
            Some("a temporary workspace")
        );
        assert!(
            serde_json::from_slice::<v5::State>(&class_state_bytes(&store)).is_err(),
            "a version-5 reader accepted a class-bearing record"
        );
        let report = store.report(Accounting::Recorded, 1).unwrap();
        let view = report
            .slots
            .iter()
            .find(|view| view.slot_id == slot)
            .expect("the one-use slot");
        assert_eq!(view.class, Some(WorkspaceClass::OneUse));
        assert_eq!(view.class_reason.as_deref(), Some("a temporary workspace"));

        // A reason longer than the diagnostic bound is truncated, never a
        // state error and never invented text.
        let long = "r".repeat(MAX_DIAGNOSTIC_BYTES * 2);
        assert_eq!(bounded_class_reason(&long).len(), MAX_DIAGNOSTIC_BYTES);
        assert_eq!(bounded_class_reason("short"), "short");
        fs::remove_dir_all(root).unwrap();
    }

    /// The fence is written only when a class-bearing record is written, and
    /// it is durable on its own first: a crash between the two saves leaves a
    /// fenced pool with no class-bearing record, which every reader accepts.
    #[test]
    fn the_one_use_fence_is_written_only_with_a_class_bearing_record() {
        let (root, store, _key) = class_fixture("fence-order");
        // Every real mutating command takes the admission gate first, which
        // makes the recorded-accounting fence durable; a version-6 document
        // carries both fences, so the fixture starts from the same state.
        let mut state = State::default();
        state.recorded_accounting = Some(RECORDED_ACCOUNTING_FENCE.to_owned());
        store.save(&state).unwrap();
        let mut state = store.load().unwrap();
        store.fence_one_use_workspaces(&mut state).unwrap();
        let fenced = store.load().unwrap();
        assert_eq!(fenced.version, ONE_USE_STATE_VERSION);
        assert_eq!(fenced.one_use_workspaces.as_deref(), Some(ONE_USE_FENCE));
        assert!(
            fenced
                .records
                .iter()
                .all(|record| record.class == WorkspaceClass::Normal),
            "the fence-only document carries no class-bearing record"
        );
        // Idempotent, and it never writes a second time.
        store.fence_one_use_workspaces(&mut state).unwrap();
        let again = store.load().unwrap();
        assert_eq!(again.version, fenced.version);
        assert_eq!(again.one_use_workspaces, fenced.one_use_workspaces);
        assert_eq!(again.records.len(), fenced.records.len());

        // A class-bearing record without the fence is refused, so the ordering
        // above is load-bearing rather than decorative.
        let mut unfenced = State::default();
        unfenced.recorded_accounting = Some(RECORDED_ACCOUNTING_FENCE.to_owned());
        unfenced.records.push(Record {
            class: WorkspaceClass::OneUse,
            class_reason: Some("reason".to_owned()),
            ..record(Phase::Idle, None, None)
        });
        let error = store.save(&unfenced).unwrap_err();
        assert!(error.contains("without the one-use fence"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    /// A refused count replacement never becomes a thirteenth project slot.
    /// At `PROJECT_SLOT_CAP` the front door asks the library to retire one
    /// named idle slot; when the library refuses that candidate (here because
    /// its recorded root no longer resolves) the command fails with the
    /// bounded reason and the pool keeps exactly its capped slot count. The
    /// library-side refusal for a slot whose own accounting walk was partial
    /// (`a_named_count_replacement_refuses_rather_than_exceeding_the_project_
    /// cap`) reaches the same front-door path.
    #[test]
    fn a_refused_count_replacement_never_adds_a_thirteenth_project_slot() {
        let (root, store, key) = class_fixture("cap-refused-replacement");
        let mut normal = Vec::new();
        while normal.len() < PROJECT_SLOT_CAP {
            normal.push(lease_as(&store, &key, WorkspaceAdmission::NORMAL, false).expect("normal"));
        }
        // Free one record and its lock so it becomes the cap's replacement
        // candidate, then make the recorded slot root unresolvable.
        let victim = normal.pop().expect("a released slot");
        let victim_slot = victim.slot_id.clone();
        store.release(victim, None).unwrap();
        let view = store.ledger().unwrap();
        let dir = view
            .slots
            .iter()
            .find(|slot| slot.slot_id == victim_slot)
            .and_then(|slot| slot.dir.clone())
            .expect("the victim's recorded directory");
        let slot_root = store.payload.join("slots").join(&dir);
        fs::remove_dir_all(&slot_root).unwrap();
        fs::write(&slot_root, b"not a directory").unwrap();

        let (ticket, _file) = store.register_ticket().unwrap();
        let outcome = store.attempt(
            ticket,
            &key,
            Profile::Quick,
            16,
            WorkspaceAdmission::NORMAL,
            false,
            None,
        );
        store.remove_ticket(ticket).unwrap();
        let error = match outcome {
            Err(error) => error,
            Ok(_) => panic!("a refused replacement admitted or waited instead"),
        };
        assert!(
            error.contains("cannot lease a slot"),
            "the refusal is not the bounded capacity shape: {error}"
        );
        let view = store.ledger().unwrap();
        assert_eq!(
            view.slots.len(),
            PROJECT_SLOT_CAP,
            "the refused replacement still added a slot"
        );
        assert!(view.slots.iter().any(|slot| slot.slot_id == victim_slot));
        for held in normal {
            drop(held);
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// The twelve-slot project cap counts **normal** slots only (§5.3): a
    /// one-use admission is never blocked by it, and a one-use slot never
    /// relieves it — the cap path can only evict a normal slot, and it refuses
    /// honestly when none is safe.
    #[test]
    fn the_project_cap_counts_normal_slots_only() {
        let (root, store, key) = class_fixture("cap");
        let mut normal = Vec::new();
        while normal.len() < PROJECT_SLOT_CAP {
            normal.push(lease_as(&store, &key, WorkspaceAdmission::NORMAL, false).expect("normal"));
        }
        let state = store.load().unwrap();
        assert_eq!(
            state
                .records
                .iter()
                .filter(|record| record.class == WorkspaceClass::Normal)
                .count(),
            PROJECT_SLOT_CAP
        );
        // A normal command is at the cap: it waits for a held compatible slot.
        assert!(matches!(
            lease_as(&store, &key, WorkspaceAdmission::NORMAL, false),
            Err(Wait::Checkout)
        ));
        // A one-use command is not blocked by twelve normal slots of this
        // repository; it takes its own record and its own slot.
        let one_use = lease_as(
            &store,
            &key,
            WorkspaceAdmission::one_use("a temporary workspace"),
            false,
        )
        .expect("a one-use admission is not capped by normal slots");
        let one_use_slot = one_use.slot_id.clone();
        let state = store.load().unwrap();
        assert_eq!(
            state
                .records
                .iter()
                .filter(|record| record.class == WorkspaceClass::OneUse)
                .count(),
            1
        );
        // A one-use slot never relieves the normal cap: releasing it leaves a
        // normal admission at the cap, and the one-use slot stays untouched.
        store.release(one_use, None).unwrap();
        assert!(matches!(
            lease_as(&store, &key, WorkspaceAdmission::NORMAL, false),
            Err(Wait::Checkout)
        ));
        let view = store.ledger().unwrap();
        assert!(
            view.slots.iter().any(|slot| slot.slot_id == one_use_slot),
            "a normal admission evicted the one-use slot"
        );
        for held in normal {
            drop(held);
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// Sharing is class-matched (§5.4): another checkout's idle compatible slot
    /// is taken only when the class matches. A normal command never borrows a
    /// one-use slot, a one-use command never borrows or relabels a normal slot,
    /// and same-class sharing between two one-use checkouts keeps today's gate,
    /// origin and custodian rules.
    #[test]
    fn sharing_is_class_matched() {
        let (root, store, key) = class_fixture("sharing");
        let other_checkout = key
            .checkout
            .parent()
            .expect("fixture root")
            .join("other-checkout");
        fs::create_dir(&other_checkout).unwrap();
        let other = SlotKey {
            checkout: other_checkout,
            identity: key.identity.clone(),
            compatibility: key.compatibility.clone(),
        };
        // The same trusted content origin for both records: only the class can
        // decide the share, so the case is never vacuous.
        let origin = Built::Content(crate::freshness::Origin {
            checkout: key.checkout.clone(),
            content: crate::freshness::Content::default(),
            content_version: 0,
            anchor_ns: 1,
            uncertain: std::collections::BTreeSet::new(),
        });
        let settle = |store: &Store, held: Lease| {
            let record = held.record_id();
            store.release(held, Some(origin.clone())).unwrap();
            record
        };

        // A one-use slot of the first checkout, idle with a trusted origin.
        let one_use = lease_as(
            &store,
            &key,
            WorkspaceAdmission::one_use("a temporary workspace"),
            false,
        )
        .expect("one-use lease");
        let one_use_slot = one_use.slot_id.clone();
        let one_use_record = settle(&store, one_use);

        // A normal command of the other checkout does not take it: the idle
        // compatible slot, its origin and its lock are all eligible, so only
        // the class can refuse the share. It takes its own normal slot.
        let normal =
            lease_as(&store, &other, WorkspaceAdmission::NORMAL, true).expect("normal admission");
        assert!(
            normal.shared_from.is_none(),
            "a normal command borrowed a one-use slot"
        );
        assert_ne!(normal.slot_id, one_use_slot);
        let normal_record = settle(&store, normal);

        let state = store.load().unwrap();
        let class_of = |id: u64| {
            state
                .records
                .iter()
                .find(|record| record.id == id)
                .map(|record| record.class)
        };
        assert_eq!(class_of(one_use_record), Some(WorkspaceClass::OneUse));
        assert_eq!(class_of(normal_record), Some(WorkspaceClass::Normal));
        let pair_of = |slot: &str| {
            store
                .ledger()
                .unwrap()
                .slots
                .iter()
                .find(|row| row.slot_id == slot)
                .map(|row| row.incarnation.clone())
        };
        let one_use_pair = pair_of(&one_use_slot).expect("the one-use row");

        // Same-class sharing between two one-use checkouts is allowed and
        // relabels nothing: the record keeps its class, the ledger row keeps
        // its pair, and only the checkout affinity moves, as it always has.
        let shared = lease_as(
            &store,
            &other,
            WorkspaceAdmission::one_use("a temporary workspace"),
            true,
        )
        .expect("one-use sharing");
        assert_eq!(shared.slot_id, one_use_slot, "a one-use slot is shareable");
        assert_eq!(shared.shared_from.as_deref(), Some(key.checkout.as_path()));
        store.release(shared, None).unwrap();
        assert_eq!(pair_of(&one_use_slot), Some(one_use_pair));

        let state = store.load().unwrap();
        let record = state
            .records
            .iter()
            .find(|record| record.id == one_use_record)
            .expect("the one-use record");
        assert_eq!(record.class, WorkspaceClass::OneUse);
        let membership = store.class_membership(&state, &store.ledger().unwrap().slots);
        assert!(membership
            .members
            .iter()
            .any(|member| member.slot_id == one_use_slot));
        assert!(membership.unknown.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    /// A pre-feature control document: one idle record of `key`'s checkout and
    /// compatibility, written with neither class field, exactly as every binary
    /// before this feature wrote it. It has no slot yet, and its affinity
    /// directory exists because a real record always created one.
    fn write_slotless_legacy_state(store: &Store, key: &SlotKey) {
        let legacy = format!(
            concat!(
                "{{\"version\":5,\"next_id\":2,\"next_use\":2,\"tickets\":[],\"cleans\":[],",
                "\"records\":[{{\"id\":1,\"identity\":\"identity\",",
                "\"compatibility\":\"quick-key\",\"profile\":\"quick\",",
                "\"checkout\":\"{}\",\"phase\":\"idle\",\"process_group\":null,",
                "\"last_used\":1,\"holder\":null,\"lease_id\":null,",
                "\"pending_recovery\":null,\"last_used_ms\":1}}],\"recoveries\":[],",
                "\"recorded_accounting\":\"oxide.accounting/1\"}}"
            ),
            key.checkout.display()
        );
        write_raw_state(store, legacy.as_bytes());
        fs::create_dir(store.affinity_dir(1)).unwrap();
        fs::set_permissions(
            store.affinity_dir(1),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
    }

    /// The stale sweep is class-agnostic (design §5.2, §5.7): an idle slotless
    /// record of **this** checkout and compatibility is a live record, so a
    /// one-use admission takes a distinct record instead of forgetting the
    /// normal or legacy one. The kept record is not junk either: the next
    /// normal command reuses it and its slot.
    #[test]
    fn a_one_use_admission_keeps_a_slotless_legacy_record_of_the_same_key() {
        let (root, store, key) = class_fixture("slotless-legacy");
        write_slotless_legacy_state(&store, &key);

        let one_use = lease_as(
            &store,
            &key,
            WorkspaceAdmission::one_use("a temporary workspace"),
            false,
        )
        .expect("one-use lease");
        assert_ne!(one_use.record_id(), 1);
        store.release(one_use, None).unwrap();

        let state = store.load().unwrap();
        let legacy = state
            .records
            .iter()
            .find(|record| record.id == 1)
            .expect("the slotless legacy record was forgotten");
        assert_eq!(legacy.class, WorkspaceClass::Normal);
        assert_eq!(legacy.class_reason, None);
        assert!(
            store.affinity_dir(1).is_dir(),
            "the legacy record's affinity directory was removed with the record"
        );

        // The kept record is still this key's reusable record, so the next
        // normal command takes it rather than leaving it behind.
        let normal =
            lease_as(&store, &key, WorkspaceAdmission::NORMAL, false).expect("normal lease");
        assert_eq!(normal.record_id(), 1);
        store.release(normal, None).unwrap();
        let slots = store.ledger().unwrap().slots;
        assert_eq!(slots.len(), 2, "each class keeps its own slot");
        let state = store.load().unwrap();
        assert_eq!(
            state
                .records
                .iter()
                .filter(|record| record.id == 1)
                .map(|record| record.class)
                .collect::<Vec<_>>(),
            vec![WorkspaceClass::Normal]
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Legacy records (§5.7): a record written before this feature has no
    /// class and reads as **protected normal**. Its next one-use command in a
    /// temporary workspace classifies `one_use`, which differs from the
    /// record's class, so it takes a distinct record and the legacy record
    /// keeps its class and its slot under today's rules.
    #[test]
    fn a_legacy_record_reads_as_protected_normal() {
        let (root, store, key) = class_fixture("legacy");
        write_slotless_legacy_state(&store, &key);
        let state = store.load().unwrap();
        assert_eq!(state.records[0].class, WorkspaceClass::Normal);
        assert_eq!(state.records[0].class_reason, None);
        assert_eq!(state.one_use_workspaces, None);

        // A legacy record that owns a real slot keeps it: the one-use command
        // takes a distinct record and never relabels, reuses or deletes the
        // normal one.
        let normal = lease_as(&store, &key, WorkspaceAdmission::NORMAL, false).expect("normal");
        let normal_slot = normal.slot_id.clone();
        store.release(normal, None).unwrap();
        let one_use = lease_as(
            &store,
            &key,
            WorkspaceAdmission::one_use("a temporary workspace"),
            false,
        )
        .expect("one-use lease");
        assert_ne!(one_use.slot_id, normal_slot);
        store.release(one_use, None).unwrap();
        let state = store.load().unwrap();
        let legacy = state
            .records
            .iter()
            .find(|record| record.id == 1)
            .expect("the legacy record");
        assert_eq!(legacy.class, WorkspaceClass::Normal);
        assert_eq!(legacy.class_reason, None);
        let one_use = state
            .records
            .iter()
            .find(|record| record.class == WorkspaceClass::OneUse)
            .expect("the one-use record");
        assert_ne!(one_use.id, 1, "the legacy record was relabelled");
        assert_eq!(
            one_use.class_reason.as_deref(),
            Some("a temporary workspace")
        );
        let slots = store.ledger().unwrap().slots;
        assert!(
            slots.iter().any(|slot| slot.slot_id == normal_slot),
            "the legacy record lost its slot"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Membership and class-unknown derivation (§5.5, §5.6): member rows come
    /// from one-use records, an unowned ledger row is **unknown** and never a
    /// member, and the two sets account for every one-use-or-unknown row once.
    #[test]
    fn class_membership_separates_one_use_rows_from_class_unknown_rows() {
        let (root, store, key) = class_fixture("membership");
        let normal = lease_as(&store, &key, WorkspaceAdmission::NORMAL, false).expect("normal");
        let normal_record = normal.record_id();
        store.release(normal, None).unwrap();
        let one_use = lease_as(
            &store,
            &key,
            WorkspaceAdmission::one_use("a temporary workspace"),
            false,
        )
        .expect("one-use lease");
        let one_use_slot = one_use.slot_id.clone();
        store.release(one_use, None).unwrap();

        let state = store.load().unwrap();
        let slots = store.ledger().unwrap().slots;
        let membership = store.class_membership(&state, &slots);
        assert_eq!(membership.members.len(), 1);
        assert_eq!(membership.members[0].slot_id, one_use_slot);
        assert!(membership.unknown.is_empty());

        // Forgetting the normal record leaves its ledger row with no owning
        // record: that row is class-unknown, never a member and never quota.
        let mut state = store.load().unwrap();
        store.forget_record(&mut state, normal_record);
        let slots = store.ledger().unwrap().slots;
        let membership = store.class_membership(&state, &slots);
        assert_eq!(membership.members.len(), 1);
        assert_eq!(membership.unknown.len(), 1);
        assert_ne!(membership.unknown[0].slot_id, one_use_slot);
        // Together they account for every ledger row exactly once.
        assert_eq!(
            membership.members.len() + membership.unknown.len(),
            slots.len()
        );
        fs::remove_dir_all(root).unwrap();
    }

    // ---------------------------------------------------------------------
    // One-use workspaces, slice S3: the evictable split and its reader
    // (design §5.5.1, §5.5.2, §6.9). Unit tests only: production is dormant.
    // ---------------------------------------------------------------------

    /// The evictable checklist protects every doubt: a held record lock, an
    /// active phase, a recorded group, a clean mark and a missing lock are all
    /// protected, and only a positively idle, unlocked, clean `Idle` member is
    /// evictable.
    #[test]
    fn the_evictable_split_protects_every_control_uncertainty() {
        check_evictable_split("evictable");
    }

    // A record lock is an `flock`, and `flock` belongs to the **open file
    // description**: `fork` copies the descriptor table, not the locking
    // state, so a child forked by a *parallel* test while this test holds a
    // probe keeps the same description (and therefore the lock) after this
    // test closes its own descriptor, until that child `exec`s and the
    // descriptor's `O_CLOEXEC` bit closes it. `evictable_members` correctly
    // refuses a lock it observes held (design §5.5.1: a held lock is a donor
    // pin or a writer), so a free observation must wait out that inherited
    // window rather than assume the release is instantaneous. Both bounds
    // below are on the *observation*: an inherited description closes in
    // milliseconds once the child runs, so a lock still held at the deadline
    // is a real failure, not a scheduling artefact.
    const FREE_PROBE_DEADLINE: Duration = Duration::from_secs(3);
    const FREE_PROBE_STEP: Duration = Duration::from_millis(5);

    /// Observe a free record lock, tolerating only an inherited description
    /// (`Probe::Held`); a missing file or an error is reported as it is.
    fn free_probe(path: &Path, create: bool) -> Probe {
        let deadline = Instant::now() + FREE_PROBE_DEADLINE;
        loop {
            match probe_lock(path, create) {
                Ok(Probe::Held) if Instant::now() < deadline => {
                    std::thread::sleep(FREE_PROBE_STEP);
                }
                Ok(probe) => return probe,
                Err(error) => panic!("cannot probe {}: {error}", path.display()),
            }
        }
    }

    /// Observe the eviction decision until it settles on an evictable member
    /// or the deadline expires; the caller asserts on the result so the
    /// product's own rejection reasons are in the failure. A member that never
    /// becomes evictable is a real failure.
    fn settled_evictable_members(
        store: &Store,
        state: &State,
        slots: &[LedgerSlot],
        members: &[ClassMember],
        own: u64,
    ) -> Vec<ClassMember> {
        let deadline = Instant::now() + FREE_PROBE_DEADLINE;
        loop {
            let evictable = store.evictable_members(state, slots, members, own);
            if !evictable.is_empty() || Instant::now() >= deadline {
                return evictable;
            }
            std::thread::sleep(FREE_PROBE_STEP);
        }
    }

    /// The mechanism above, made deterministic: a second descriptor from
    /// `try_clone` shares this test's open file description exactly as a
    /// `fork`ed child does, so the lock survives the original descriptor's
    /// close. The product refuses that member (a held lock is protected); once
    /// the shared description closes, the bounded observer sees it evictable.
    /// This is the flaky-suite signature (`("1", "lock held")`) reproduced
    /// on purpose in a controlled window.
    #[test]
    fn the_eviction_lock_probe_settles_after_an_inherited_description_closes() {
        let (root, store, key) = class_fixture("evictable-inherited");
        let held = lease_as(
            &store,
            &key,
            WorkspaceAdmission::one_use("a temporary workspace"),
            false,
        )
        .expect("one-use lease");
        let record_id = held.record_id();
        store.release(held, None).unwrap();
        let state = store.load().unwrap();
        let slots = store.ledger().unwrap().slots;
        let membership = store.class_membership(&state, &slots);
        assert_eq!(membership.members.len(), 1);
        let pair = membership.members[0].clone();

        let path = store.lock_path(record_id);
        let probe = match free_probe(&path, true) {
            Probe::Free(probe) => probe,
            Probe::Held => panic!("the record lock is held with no writer"),
            Probe::Missing => panic!("the record lock is missing"),
        };
        // What `fork` gives a child: another descriptor of the same open file
        // description, so the lock outlives this descriptor.
        let inherited = probe.try_clone().expect("inherit the open description");
        drop(probe);
        assert!(
            store
                .evictable_members(&state, &slots, &membership.members, 0)
                .is_empty(),
            "a lock held through an inherited description was evictable"
        );
        assert_eq!(
            EVICTION_REJECTION.with(|value| value.borrow().clone()),
            vec![(pair.slot_id.clone(), "lock held")],
            "the product refused the member for a reason other than its held lock"
        );

        drop(inherited);
        assert_eq!(
            settled_evictable_members(&store, &state, &slots, &membership.members, 0),
            vec![pair],
            "the decision did not settle after the inherited description closed"
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn check_evictable_split(label: &str) {
        let (root, store, key) = class_fixture(label);
        let held = lease_as(
            &store,
            &key,
            WorkspaceAdmission::one_use("a temporary workspace"),
            false,
        )
        .expect("one-use lease");
        let record_id = held.record_id();
        store.release(held, None).unwrap();
        let state = store.load().unwrap();
        let slots = store.ledger().unwrap().slots;
        let membership = store.class_membership(&state, &slots);
        assert_eq!(membership.members.len(), 1);
        let pair = membership.members[0].clone();
        let evictable = settled_evictable_members(&store, &state, &slots, &membership.members, 0);
        assert_eq!(evictable, vec![pair.clone()], "a closed idle member");
        // The caller's own record is excluded exactly as `checkout_custody`
        // does; here the member is that record and it is still idle.
        assert_eq!(
            settled_evictable_members(&store, &state, &slots, &membership.members, record_id),
            vec![pair.clone()],
            "actual eviction decision rejected: {:?}",
            EVICTION_REJECTION.with(|value| value.borrow().clone())
        );

        // 1/2: an active phase or a recorded holder is protected.
        for mutate in [
            |record: &mut Record| record.phase = Phase::Spawning,
            |record: &mut Record| {
                record.phase = Phase::Idle;
                record.holder = Some(Holder {
                    pid: 41,
                    started_ms: 7,
                    lease_holder: "oxide-41-3".to_owned(),
                    command: None,
                });
            },
        ] {
            let mut state = store.load().unwrap();
            let index = state
                .records
                .iter()
                .position(|record| record.id == record_id)
                .unwrap();
            mutate(&mut state.records[index]);
            let evictable = store.evictable_members(&state, &slots, &membership.members, 0);
            assert!(evictable.is_empty(), "an active member was evictable");
        }

        // 5: a recorded process group means a writer may still exist.
        let mut state = store.load().unwrap();
        let index = state
            .records
            .iter()
            .position(|record| record.id == record_id)
            .unwrap();
        state.records[index].process_group = Some(41);
        assert!(store
            .evictable_members(&state, &slots, &membership.members, 0)
            .is_empty());

        // 3: a lock probe that is free is evictable; a held or missing lock is
        // not. The donor pin is a held record lock, so this covers §8.2.
        let mut state = store.load().unwrap();
        state.records[index].process_group = None;
        store.save(&state).unwrap();
        let path = store.lock_path(record_id);
        // A parallel test's `fork` may briefly hold an inherited description
        // of the lease's record lock after `release` closed its own.
        let held_probe = free_probe(&path, true);
        assert!(
            matches!(&held_probe, Probe::Free(_)),
            "the record lock is not free after release"
        );
        assert!(
            store
                .evictable_members(&state, &slots, &membership.members, 0)
                .is_empty(),
            "a held record lock was evictable"
        );
        drop(held_probe);
        std::thread::yield_now();
        let member = &state.records[index];
        let slot = slots
            .iter()
            .find(|slot| slot.slot_id == pair.slot_id && slot.incarnation == pair.incarnation)
            .expect("member remains in the ledger");
        assert_eq!(
            slot.project,
            store.affinity_dir(member.id),
            "member mapping"
        );
        assert!(idle_member_record(member), "member record is not idle");
        match free_probe(&path, false) {
            Probe::Free(probe) => drop(probe),
            Probe::Held => panic!("member lock remains held after dropping the probe"),
            Probe::Missing => panic!("member lock is missing after dropping the probe"),
        }
        assert!(
            !store.other_record_active(&state, member, 0, boot_session()),
            "another record has active or unclosed writers"
        );
        assert!(
            slot.state == SlotOccupancy::Idle && slot.lease_id.is_none(),
            "ledger slot is occupied or leased"
        );
        let checkout = member.checkout.as_deref().expect("member checkout missing");
        assert!(
            !state.cleans.iter().any(|clean| clean.checkout == checkout),
            "member checkout has a clean mark"
        );
        assert_eq!(
            settled_evictable_members(&store, &state, &slots, &membership.members, 0),
            vec![pair.clone()],
            "actual eviction decision rejected: {:?}",
            EVICTION_REJECTION.with(|value| value.borrow().clone())
        );
        fs::remove_file(&path).unwrap();
        assert!(
            store
                .evictable_members(&state, &slots, &membership.members, 0)
                .is_empty(),
            "a missing record lock is not free"
        );
        let _ = probe_lock(&path, true).expect("lock");

        // 7: a recorded clean for the member's checkout is protected.
        let mut state = store.load().unwrap();
        state.cleans.push(Clean {
            id: 1,
            checkout: key.checkout.clone(),
            process_group: None,
        });
        store.save(&state).unwrap();
        assert!(
            store
                .evictable_members(&state, &slots, &membership.members, 0)
                .is_empty(),
            "a clean mark was evictable"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn evictable_split_fifty_yielded_probe_drops() {
        for iteration in 0..50 {
            eprintln!("eviction probe diagnostic iteration {iteration}");
            check_evictable_split(&format!("evictable-diagnostic-{iteration}"));
        }
    }

    /// The evictable reader reads no sibling-pool state at all (§5.5.2): with
    /// an active record in the other pool and a blocking barrier configured,
    /// the result is identical and no admission file is opened there.
    #[test]
    fn the_evictable_reader_ignores_the_other_pool() {
        let (root, store, key) = class_fixture("evictable-sibling");
        let held = lease_as(
            &store,
            &key,
            WorkspaceAdmission::one_use("a temporary workspace"),
            false,
        )
        .expect("one-use lease");
        store.release(held, None).unwrap();
        let state = store.load().unwrap();
        let slots = store.ledger().unwrap().slots;
        let membership = store.class_membership(&state, &slots);
        let expected = store.evictable_members(&state, &slots, &membership.members, 0);
        assert_eq!(expected.len(), 1, "the fixture member is not evictable");

        // The other pool holds an active record for the same checkout, which a
        // cross-pool premise would have protected the member against.
        let sibling = Store::for_tests(&root.join("sibling-pool"));
        let mut other = State::default();
        let mut active = record(Phase::Running, Some(42), None);
        active.checkout = Some(key.checkout.clone());
        other.records.push(active);
        sibling.save(&other).unwrap();
        let sibling_control = sibling.control.clone();
        let with_sibling = store
            .handle_for_tests()
            .with_sibling(&sibling_control, true);
        let evictable = with_sibling.evictable_members(&state, &slots, &membership.members, 0);
        assert_eq!(evictable, expected, "the reader read cross-pool state");
        assert!(
            !sibling_control.join("admission.lock").exists(),
            "the evictable reader opened the other pool's admission lock"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Only a live, active, custody-clear member is progress-capable (§6.9);
    /// an idle record, an unheld active record and unknown custody are not.
    #[test]
    fn a_progress_capable_member_is_a_live_active_record() {
        let (root, store, key) = class_fixture("progress");
        let held = lease_as(
            &store,
            &key,
            WorkspaceAdmission::one_use("a temporary workspace"),
            false,
        )
        .expect("one-use lease");
        let record_id = held.record_id();
        // The command's own record is excluded; another ID sees the holder.
        assert!(store.progress_capable_member(&store.load().unwrap(), 0));
        assert!(
            !store.progress_capable_member(&store.load().unwrap(), record_id),
            "the caller's own record was progress-capable"
        );
        store.release(held, None).unwrap();
        assert!(
            !store.progress_capable_member(&store.load().unwrap(), 0),
            "an idle record was progress-capable"
        );

        // An active phase without the live lock is not progress-capable: its
        // writer custody is unknown, so it is protected and never a reason to
        // wait.
        let mut state = store.load().unwrap();
        let index = state
            .records
            .iter()
            .position(|record| record.id == record_id)
            .unwrap();
        state.records[index].phase = Phase::Spawning;
        store.save(&state).unwrap();
        assert!(!store.progress_capable_member(&state, 0));

        // A live lock with an active phase is progress-capable, and an open
        // unclosed-writer marker makes it unknown custody again.
        let probe = probe_lock(&store.lock_path(record_id), true).expect("lock");
        assert!(matches!(&probe, Probe::Free(_)));
        assert!(store.progress_capable_member(&state, 0));
        drop(probe);
        let mut state = store.load().unwrap();
        state.records[index].lost = Some(Lost {
            slot_id: "1".to_owned(),
            at_ms: 1,
            unclosed: Some("a test writer was never confirmed closed".to_owned()),
        });
        store.save(&state).unwrap();
        let _ = probe_lock(&store.lock_path(record_id), true).expect("lock");
        assert!(!store.progress_capable_member(&state, 0));
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(test, target_os = "macos"))]
mod scratch_internal_tests {
    use super::*;
    use std::ffi::{CStr, CString, OsStr};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::process::Command;

    fn fixture() -> PathBuf {
        let named = std::env::var_os("OXIDE_TEST_INTERNAL_ROOT")
            .expect("OXIDE_TEST_INTERNAL_ROOT is required");
        let root = fs::canonicalize(&named).expect("internal root");
        assert_eq!(Path::new(&named), root);
        let supervised =
            fs::canonicalize(std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").expect("supervised root"))
                .expect("supervised root");
        assert!(
            root.starts_with(supervised),
            "internal root must be supervised"
        );
        let metadata = fs::metadata(&root).unwrap();
        assert!(
            metadata.is_dir()
                && metadata.uid() == unsafe { libc::getuid() }
                && metadata.mode() & 0o077 == 0
        );
        let mut name = CString::new(root.join("scratch-unit.XXXXXX").as_os_str().as_bytes())
            .unwrap()
            .into_bytes_with_nul();
        assert!(!unsafe { libc::mkdtemp(name.as_mut_ptr().cast()) }.is_null());
        PathBuf::from(OsStr::from_bytes(
            CStr::from_bytes_until_nul(&name).unwrap().to_bytes(),
        ))
    }

    fn isolated(test: &str, root: &Path) {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("OXIDE_S3_UNIT_CHILD", "1")
            .env(
                "OXIDE_TEST_INTERNAL_ROOT",
                std::env::var_os("OXIDE_TEST_INTERNAL_ROOT").unwrap(),
            )
            .env("HOME", root.join("home"))
            .env("OXIDE_TEST_HOME_FROM_ENV", "1")
            .env("OXIDE_CONFIG", root.join("config.toml"))
            .env("OXIDE_POOL", root.join("pool"))
            .env("OXIDE_POOL_CAP", "64MiB")
            .env("OXIDE_POOL_CREATE", "1")
            .env("OXIDE_SCRATCH_ROOT", root)
            .env("OXIDE_S3_ROOT", root)
            .env("TMPDIR", root)
            .env("TMP", root)
            .env("TEMP", root)
            .status()
            .expect("unit child");
        assert!(status.success(), "isolated unit child failed");
    }

    #[test]
    fn scratch_declaration_is_inert_on_boot_volume() {
        if std::env::var_os("OXIDE_S3_UNIT_CHILD").is_none() {
            let root = fixture();
            isolated(
                "store::scratch_internal_tests::scratch_declaration_is_inert_on_boot_volume",
                &root,
            );
            fs::remove_dir_all(root).unwrap();
            return;
        }
        let _internal = crate::config::internal_fixture_scope();
        let root = PathBuf::from(std::env::var_os("OXIDE_S3_ROOT").unwrap());
        fs::create_dir(root.join("home")).unwrap();
        let declared = root.join("pool");
        let control = root.join("control-pool");
        assert!(!declared.exists() && !control.exists());
        let first = Store::open_or_create().expect("declared internal pool");
        assert_eq!(first.pool, declared);
        assert!(!declared.join("oxide-managed/scratch.json").exists());
        assert!(!root
            .join("home/Library/Application Support/oxide/state/scratch-pools")
            .exists());
        // This child process is isolated from the unit runner's environment.
        unsafe {
            std::env::remove_var("OXIDE_SCRATCH_ROOT");
            std::env::set_var("OXIDE_POOL", &control);
        }
        let second = Store::open_or_create().expect("ordinary internal pool");
        assert_eq!(second.pool, control);
        assert!(!control.join("oxide-managed/scratch.json").exists());
        assert!(!root
            .join("home/Library/Application Support/oxide/state/scratch-pools")
            .exists());
        let mut first_config: serde_json::Value =
            serde_json::from_slice(&fs::read(declared.join("config.json")).unwrap()).unwrap();
        let mut second_config: serde_json::Value =
            serde_json::from_slice(&fs::read(control.join("config.json")).unwrap()).unwrap();
        let left = first_config.as_object_mut().unwrap();
        let right = second_config.as_object_mut().unwrap();
        let expected: std::collections::BTreeSet<&str> =
            ["version", "backend", "store_id", "cap_bytes", "image_bytes"]
                .into_iter()
                .collect();
        assert_eq!(
            left.keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            expected
        );
        assert_eq!(
            right
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            expected
        );
        // Only store_id is generated per initialization; a new field must be assessed above.
        let left_id = left
            .remove("store_id")
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        let right_id = right
            .remove("store_id")
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        for id in [&left_id, &right_id] {
            assert!(oxide_storage::seed_source::SourceCache::new(id).is_ok());
            assert!(
                id.len() == 32
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            );
        }
        assert_ne!(left_id, right_id);
        assert_eq!(left, right);
        fn entries(root: &Path, path: &Path, id: &str, result: &mut Vec<String>) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace(id, "<store-id>");
                result.push(relative);
                if entry.file_type().unwrap().is_dir() {
                    entries(root, &entry.path(), id, result);
                }
            }
        }
        let mut left_entries = Vec::new();
        let mut right_entries = Vec::new();
        entries(&declared, &declared, &left_id, &mut left_entries);
        entries(&control, &control, &right_id, &mut right_entries);
        left_entries.sort();
        right_entries.sort();
        assert_eq!(left_entries, right_entries);
    }

    #[test]
    fn internal_git_refusal_is_unchanged_by_scratch_declaration() {
        if std::env::var_os("OXIDE_S3_UNIT_CHILD").is_none() {
            let root = fixture();
            isolated("store::scratch_internal_tests::internal_git_refusal_is_unchanged_by_scratch_declaration", &root);
            fs::remove_dir_all(root).unwrap();
            return;
        }
        let _internal = crate::config::internal_fixture_scope();
        let root = PathBuf::from(std::env::var_os("OXIDE_S3_ROOT").unwrap());
        fs::create_dir(root.join(".git")).unwrap();
        let settings = super::settings().unwrap();
        let with = crate::scratch_io::evaluate(&settings, false).unwrap_err();
        unsafe { std::env::remove_var("OXIDE_SCRATCH_ROOT") };
        let without = crate::scratch_io::evaluate(&settings, false).unwrap_err();
        assert_eq!(with, without);
        assert!(with.contains("Git work tree"));
        assert!(!settings.pool.exists());
    }
}

/// The real child launch keeps the lease's root hold. The test runs in its own
/// test process: a spawned child inherits every inheritable descriptor of the
/// process, so running it beside other fixtures would make their lock probes
/// observe this child's copies.
#[cfg(test)]
mod launch_hold_tests {
    use super::*;

    const NAME: &str =
        "store::launch_hold_tests::the_real_launch_keeps_the_lease_hold_until_the_child_exits";

    #[test]
    fn the_real_launch_keeps_the_lease_hold_until_the_child_exits() {
        if std::env::var_os("OXIDE_LAUNCH_HOLD_CHILD").is_none() {
            let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let root = base.join(format!("oxide-launch-hold-{}", std::process::id()));
            fs::create_dir_all(&root).unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env_clear()
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("OXIDE_LAUNCH_HOLD_CHILD", "1")
                .env("OXIDE_LAUNCH_ROOT", &root)
                .env("TMPDIR", &root)
                .env("TMP", &root)
                .env("TEMP", &root)
                .status()
                .expect("isolated child");
            let _ = fs::remove_dir_all(&root);
            assert!(status.success(), "isolated launch-hold child failed");
            return;
        }
        let root = PathBuf::from(std::env::var_os("OXIDE_LAUNCH_ROOT").unwrap());
        let pool = root.join("pool");
        oxide_storage::lease::managed::init_budget_pool(&pool, 8 << 20).unwrap();
        let store = Store {
            pool: pool.clone(),
            control: pool.join(oxide_storage::lease::MANAGED_NAME),
            payload: pool.join("payload"),
            backend: Backend::DirectoryBudget,
            budget: Some(8 << 20),
            recorded_budget: 8 << 20,
            max_active: 16,
            sibling: None,
        };
        store.ensure_control().unwrap();
        let checkout = root.join("checkout");
        fs::create_dir(&checkout).unwrap();
        let key = SlotKey {
            checkout,
            identity: "identity".to_owned(),
            compatibility: "quick-key".to_owned(),
        };
        let (ticket, _file) = store.register_ticket().unwrap();
        let outcome = store
            .attempt(
                ticket,
                &key,
                Profile::Quick,
                16,
                WorkspaceAdmission::NORMAL,
                false,
                None,
            )
            .unwrap();
        store.remove_ticket(ticket).unwrap();
        let lease = match outcome {
            Ok(leased) => leased,
            Err(wait) => panic!("the first command leased: {wait:?}"),
        };
        // The real launch route: the store made the hold inheritable in
        // `attempt`, so this child keeps the root held across exec.
        let mut command = std::process::Command::new("/bin/sleep");
        command.arg("0.6");
        let mut group = crate::process::spawn_group(&mut command).expect("launch");
        // The launcher drops its own copy; only the child's inherited
        // descriptor holds the root now.
        drop(lease);
        let backing = oxide_storage::storage::PoolBacking::budget(&pool, 8 << 20);
        let area = oxide_storage::storage::PoolStorage::open(
            &pool.join(oxide_storage::lease::MANAGED_NAME),
            &backing,
        )
        .unwrap();
        let roots = area.all_roots().unwrap();
        let (_, info) = roots.first().expect("one slot root").clone();
        assert!(
            area.try_hold_in(None, &info.id).unwrap().is_none(),
            "the child keeps the inherited hold"
        );
        assert!(
            area.evict_any(&info.id).is_err(),
            "a competing eviction refuses while the child holds the root"
        );
        // A separately pinned fixture is protected independently of the test.
        let pinned = area.create("pinned").unwrap();
        let pinned_area = area.area();
        pinned_area.pin(&pinned.id, "selected output").unwrap();
        assert!(
            area.evict_any(&pinned.id).is_err(),
            "the pin keeps the root"
        );
        assert!(group.child.wait().unwrap().success(), "the child exited");
        assert!(
            area.try_hold_in(None, &info.id).unwrap().is_some(),
            "the root is eligible once the child exits"
        );
        area.evict_any(&info.id)
            .expect("evicted after the child exits");
        assert!(area.evict_any(&pinned.id).is_err(), "still pinned");
        pinned_area.unpin(&pinned.id).unwrap();
        area.evict_any(&pinned.id).expect("evicted after unpin");
    }
}
