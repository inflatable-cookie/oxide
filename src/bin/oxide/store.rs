//! Oxide's private control state beside the lease ledger.
//!
//! The ledger, its JSON and the standalone CLI stay unchanged. Oxide leases
//! through the library with one owner-only project directory per affinity
//! record (checkout + compatibility key), so the library's "same project,
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
use oxide_storage::lease::managed::{
    evict_idle_slot, flush_volume, init_budget_pool, lease_measured, ledger_view, recover, release,
    status as pool_status,
};
use oxide_storage::lease::{
    LeaseError, LeaseRequest, LedgerSlot, LedgerView, PoolStatus, Profile as LeaseProfile,
    ReuseMeasurement, SlotLayout, SlotOccupancy, SlotStatus, BUDGET_BACKEND_DIRECTORY_V1,
    MANAGED_NAME, VOLUME_BACKEND_APFS_V1,
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

pub const GIB: u64 = 1024 * 1024 * 1024;
/// Automatic creation refuses below this host free space.
pub const HOST_RESERVE_BYTES: u64 = 20 * GIB;
/// Slots one project identity may keep; a new checkout at the cap evicts the
/// least recently used idle one.
const PROJECT_SLOT_CAP: usize = 2;
const MAX_TICKETS: usize = 128;
const MAX_RECORDS: usize = 64;
/// Automatic recoveries kept for `oxide status`, newest last.
const MAX_RECOVERIES: usize = 16;
const MAX_STATE_BYTES: u64 = 256 * 1024;
const STATE_VERSION: u32 = 1;
const MAX_ORIGIN_BYTES: u64 = 8 * 1024 * 1024;
const ORIGIN_VERSION: u32 = 1;
const WAIT_POLL: Duration = Duration::from_millis(200);
/// Removable-volume flushes kept for `oxide status`, newest last.
const MAX_FLUSHES: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Idle,
    /// A lease may exist; no Cargo has started.
    Acquiring,
    /// Cargo may have started; its group is not recorded yet.
    Spawning,
    /// Cargo runs as leader of `process_group`.
    Running,
    /// A removable-volume slot whose flush before release failed or is
    /// unknown. Its lease stays held: never reused, recovered or deleted
    /// here.
    Retired,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    id: u64,
    identity: String,
    compatibility: String,
    profile: String,
    /// `None` once `oxide clean` dropped this checkout's affinity.
    checkout: Option<PathBuf>,
    phase: Phase,
    process_group: Option<i32>,
    last_used: u64,
    /// Holder of the current command; set while the record is not idle.
    #[serde(default)]
    holder: Option<Holder>,
    /// Lease ID once the library granted it.
    #[serde(default)]
    lease_id: Option<String>,
    /// Written before the ledger recovery, so the event survives a crash
    /// between that recovery and moving it into the log.
    #[serde(default)]
    pending_recovery: Option<Recovery>,
    /// Why a [`Phase::Retired`] slot was retired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retired: Option<String>,
    /// Set when the slot was retired because its removable volume vanished
    /// while the record held it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lost: Option<Lost>,
    /// Wall clock of the latest use, milliseconds since the epoch, written
    /// with `last_used`. `last_used` orders uses within one pool only; this
    /// lets `status` compare pools. Absent on records last used before it
    /// existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_used_ms: Option<u64>,
}

impl Record {
    /// Holds a command now: neither idle nor retired.
    fn active(&self) -> bool {
        !matches!(self.phase, Phase::Idle | Phase::Retired)
    }
}

/// A slot held when its removable volume vanished (surprise removal). It is
/// never reused. Once the volume returns it is deleted whole only if every
/// writer is positively closed; otherwise it is quarantined: held, bytes
/// counted, never reused or deleted.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Lost {
    slot_id: String,
    at_ms: u64,
    /// Why closure of every writer is not established; `None` when the
    /// holding command saw its own group empty and started nothing Oxide
    /// knows may outlive it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unclosed: Option<String>,
}

impl Lost {
    fn describe(&self) -> String {
        match &self.unclosed {
            None => "lost with the removable volume; deleted whole when the volume returns".to_owned(),
            Some(why) => format!(
                "quarantined: lost with the removable volume and {why}; kept with its bytes, never reused or deleted"
            ),
        }
    }
}

/// The `oxide` process holding a record, and the lease holder string it
/// passed to the library.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Holder {
    pid: u32,
    /// Wall-clock start of the holder process, milliseconds since the epoch.
    started_ms: u64,
    lease_holder: String,
    /// What `oxide status` names as the holder: `exec <program>` for an
    /// exec command, absent for Cargo commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<String>,
}

/// A slot's last recorded build. `mark` is the record's use counter when it
/// was written; leasing re-stamps it and Cargo's start advances the counter,
/// so a mismatch means Cargo may have run unrecorded. `origin: None` is an
/// untrusted slot. An untrusted slot is never reused: the next command evicts
/// it and builds cold.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginFile {
    version: u32,
    mark: u64,
    origin: Option<Built>,
}

/// One automatic recovery, kept for `oxide status`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recovery {
    pub slot_id: String,
    pub lease_id: String,
    pub checkout: Option<PathBuf>,
    pub holder_pid: u32,
    pub holder_started_ms: u64,
    /// `None` when Cargo never started.
    pub process_group: Option<i32>,
    pub recovered_ms: u64,
    pub recovered_by: u32,
}

/// One flush of a removable-volume pool before a slot went idle.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flush {
    pub slot_id: String,
    pub at_ms: u64,
    /// `None` when the flush failed and the slot was retired.
    pub micros: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The flush failed because the volume was gone (surprise removal).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lost: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ticket {
    id: u64,
    pid: u32,
}

/// A running `oxide clean`; commands for the same checkout wait while it
/// exists. Its lock (in `waiters/`) is shared with the `cargo clean` child, so
/// it stays held while either lives. The mark is dropped only when the lock is
/// free and the recorded Cargo group, if any, is empty.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Clean {
    id: u64,
    checkout: PathBuf,
    #[serde(default)]
    process_group: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    next_id: u64,
    next_use: u64,
    tickets: Vec<Ticket>,
    #[serde(default)]
    cleans: Vec<Clean>,
    records: Vec<Record>,
    #[serde(default)]
    recoveries: Vec<Recovery>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    flushes: Vec<Flush>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            next_id: 1,
            next_use: 1,
            tickets: Vec::new(),
            cleans: Vec::new(),
            records: Vec::new(),
            recoveries: Vec::new(),
            flushes: Vec::new(),
        }
    }
}

impl State {
    fn take_id(&mut self) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or("Oxide ID sequence exhausted")?;
        Ok(id)
    }

    fn touch(&mut self, index: usize) -> Result<(), String> {
        let use_id = self.next_use;
        self.next_use = use_id
            .checked_add(1)
            .ok_or("Oxide use sequence exhausted")?;
        self.records[index].last_used = use_id;
        self.records[index].last_used_ms = Some(process::now_ms());
        Ok(())
    }

    fn settle_idle(&mut self, index: usize) {
        let record = &mut self.records[index];
        record.phase = Phase::Idle;
        record.process_group = None;
        record.holder = None;
        record.lease_id = None;
        record.pending_recovery = None;
    }

    fn log_flush(&mut self, flush: Flush) {
        if self.flushes.len() >= MAX_FLUSHES {
            self.flushes.remove(0);
        }
        self.flushes.push(flush);
    }

    fn log_recovery(&mut self, recovery: Recovery) {
        if self.recoveries.len() >= MAX_RECOVERIES {
            self.recoveries.remove(0);
        }
        self.recoveries.push(recovery);
    }
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
/// The pool slot whose ledger project is `project`.
fn slots_view<'a>(pool: &'a PoolStatus, project: &Path) -> Option<&'a SlotStatus> {
    pool.slots.iter().find(|slot| slot.project == project)
}

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
enum Wait {
    Queue(usize),
    HostLimit(usize),
    Checkout,
    Clean,
    ProjectCap,
    Uncertain,
    Capacity,
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
            Self::Capacity => WaitCategory::Capacity,
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
            Self::Capacity => "pool capacity held by running builds".to_owned(),
        }
    }
}

/// One held slot for the life of a command. The record lock is the liveness
/// proof other commands check before recovering.
pub struct Lease {
    pub slot_id: String,
    pub reused: bool,
    pub build_dir: PathBuf,
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
}

impl Lease {
    /// This command's own slot record, for the checkout barrier that excludes
    /// every other command in the same checkout.
    pub fn record_id(&self) -> u64 {
        self.record
    }
}

pub enum Acquired {
    Lease(Lease),
    Interrupted(i32),
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

/// Pool and slot view for `oxide status`.
pub struct Report {
    pub pool: PoolStatus,
    pub flushes: Vec<Flush>,
    pub waiting: usize,
    pub(crate) waiters: Vec<Waiter>,
    pub cleaning: Vec<PathBuf>,
    pub slots: Vec<SlotView>,
    pub recoveries: Vec<Recovery>,
    /// This pool's most recently used slot by its use counter.
    pub last_use: Option<LastUse>,
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
    let path = control.join("state.json");
    let file = private_file(&path, false)?.ok_or("no Oxide control state")?;
    let mut bytes = Vec::new();
    file.take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let state: State = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
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
    let path = control.join("state.json");
    let Some(file) = private_file(&path, false)? else {
        return Ok(Vec::new());
    };
    let mut bytes = Vec::new();
    file.take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let state: State = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    Ok(state
        .records
        .iter()
        .filter_map(|record| {
            let lost = record.lost.as_ref()?;
            Some((
                lost.slot_id.clone(),
                record.checkout.clone(),
                lost.describe(),
            ))
        })
        .collect())
}

/// Cooperative holders of one checkout, read-only: active records and clean
/// barriers, newest state only. Used by `oxide outputs plan`; an empty list
/// means no cooperative Oxide command holds the checkout right now, never
/// that no external writer exists.
pub fn active_holders(control: &Path, checkout: &Path) -> Result<Vec<String>, String> {
    let path = control.join("state.json");
    let Some(file) = private_file(&path, false)? else {
        return Ok(Vec::new());
    };
    let mut bytes = Vec::new();
    file.take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read Oxide state: {error}"))?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err("Oxide state exceeds its byte limit".to_owned());
    }
    let state: State = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Oxide state is invalid: {error}"))?;
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
    for clean in &state.cleans {
        if clean.checkout == checkout {
            holders.push("oxide clean barrier".to_owned());
        }
    }
    Ok(holders)
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
    config::settings(&Facts::from_env())
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

impl Store {
    /// Open the pool, creating it on first use above the host reserve. A
    /// macOS pool is excluded from Time Machine before any build writes into
    /// it; an unverified exclusion refuses before Cargo starts.
    pub fn open_or_create() -> Result<Self, String> {
        let settings = settings()?;
        let path = settings.pool.clone();
        config::refuse_unsafe_pool_path(&path)?;
        match config::inspect(&path) {
            Found::Absent => create_pool(&path, admission_budget(&settings))?,
            Found::LegacyImage => return Err(legacy_refusal()),
            Found::Directory | Found::Unknown => {}
        }
        let store = Self::open_at(&path, &settings)?;
        if !config::has_sticky_exclusion(&store.pool) {
            match config::exclude_from_backup(&store.pool) {
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
            if let Err(error) = config::remember_pool(&Facts::from_env(), &store.pool) {
                eprintln!("oxide: could not record the pool path for status: {error}");
            }
        }
        store.ensure_control()?;
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
        let view = ledger_view(&pool)
            .map_err(|error| lease_error("cannot open the removable pool", &error))?;
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

    /// Open an existing pool. Never creates one; a retired image pool
    /// refuses without being touched.
    pub fn open_existing() -> Result<Option<Self>, String> {
        let settings = settings()?;
        Self::open_existing_with(&settings)
    }

    /// Open an existing internal pool using settings already resolved for
    /// this command.
    pub fn open_existing_with(settings: &Settings) -> Result<Option<Self>, String> {
        match config::inspect(&settings.pool) {
            Found::Absent => Ok(None),
            Found::LegacyImage => Err(legacy_refusal()),
            Found::Directory | Found::Unknown => {
                config::refuse_unsafe_pool_path(&settings.pool)?;
                Self::open_at(&settings.pool, settings).map(Some)
            }
        }
    }

    /// The existing internal pool's private control directory, without
    /// opening its ledger or reconciling any pool state.
    pub fn existing_control() -> Result<Option<PathBuf>, String> {
        let settings = settings()?;
        config::refuse_unsafe_pool_path(&settings.pool)?;
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

    fn open_at(path: &Path, settings: &Settings) -> Result<Self, String> {
        let pool =
            fs::canonicalize(path).map_err(|error| format!("cannot open the pool: {error}"))?;
        let view =
            ledger_view(&pool).map_err(|error| lease_error("cannot open the pool", &error))?;
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

    /// Full accounting: the whole-payload walk and one walk per slot, for
    /// `oxide status` and readiness only.
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
    pub fn acquire(
        &self,
        key: &SlotKey,
        profile: Profile,
        share: bool,
        label: Option<&str>,
        waits: &mut WaitAccounting,
    ) -> Result<Acquired, String> {
        let max_active = self.max_active;
        let ticket = self.register_ticket()?;
        loop {
            if let Some(signal) = process::received_signal() {
                waits.stop(Instant::now());
                self.remove_ticket(ticket.0)?;
                return Ok(Acquired::Interrupted(signal));
            }
            let attempt = self.attempt(ticket.0, key, profile, max_active, share, label);
            match attempt {
                Err(error) => {
                    waits.stop(Instant::now());
                    return Err(error);
                }
                Ok(Ok(lease)) => {
                    waits.stop(Instant::now());
                    self.remove_ticket(ticket.0)?;
                    return Ok(Acquired::Lease(lease));
                }
                Ok(Err(wait)) => {
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
        let _admission = self.admission()?;
        if !self.flush_before_idle(&held.slot_id, held.record)? {
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
            state.touch(index)?;
            self.write_origin(held.record, state.records[index].last_used, origin)?;
        }
        self.save(&state)
    }

    /// The gate refused a slot taken from another checkout before Cargo ran:
    /// release it and give it back, origin unchanged.
    pub fn unshare(&self, held: Lease) -> Result<(), String> {
        let _admission = self.admission()?;
        if !self.flush_before_idle(&held.slot_id, held.record)? {
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
                            found.describe()
                        );
                        entry.retired = Some(found.describe());
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
                found.describe()
            );
            entry.phase = Phase::Retired;
            entry.retired = Some(found.describe());
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
    /// outlive it. Runs when the volume is selected again, under the
    /// admission lock. Anything else stays quarantined.
    fn discard_lost(&self, state: &mut State) -> Result<(), String> {
        let candidates: Vec<u64> = state
            .records
            .iter()
            .filter(|record| {
                record.phase == Phase::Retired
                    && record
                        .lost
                        .as_ref()
                        .is_some_and(|lost| lost.unclosed.is_none())
            })
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
            if record
                .process_group
                .is_some_and(|group| !process::group_empty(group))
            {
                continue;
            }
            let project = self.affinity_dir(id);
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
                eprintln!(
                    "oxide: deleted slot {} held when the removable volume was lost ({})",
                    slot.slot_id,
                    record.process_group.map_or_else(
                        || "Cargo never started".to_owned(),
                        |group| format!("Cargo group {group} empty")
                    )
                );
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

    fn sibling_state(&self) -> Option<State> {
        let sibling = self.sibling.as_ref()?;
        let file = private_file(&sibling.control.join("state.json"), false).ok()??;
        let mut bytes = Vec::new();
        file.take(MAX_STATE_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        serde_json::from_slice::<State>(&bytes).ok()
    }

    fn sibling_active(&self) -> usize {
        self.sibling_state().map_or(0, |state| {
            state
                .records
                .iter()
                .filter(|record| record.active())
                .count()
        })
    }

    /// Whether the other pool holds an active command for `checkout`
    /// (internal clean checks the removable pool's commands).
    fn sibling_busy(&self, checkout: &Path) -> bool {
        self.sibling_state().is_some_and(|state| {
            state
                .records
                .iter()
                .any(|record| record.checkout.as_deref() == Some(checkout) && record.active())
        })
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
        flock(&file, libc::LOCK_EX)?;
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
        // A sibling with no state file has no records; an unreadable one is
        // unknown custody, never "clear".
        let other_state = match &self.sibling {
            None => None,
            Some(sibling) => match self.sibling_state() {
                Some(state) => Some(state),
                None if !sibling.control.join("state.json").exists() => None,
                None => {
                    return Ok(CheckoutCustody::Unknown(
                        "the other pool's record state is unreadable".to_owned(),
                    ))
                }
            },
        };
        let mut writers = false;
        for (pool, state) in [("this", own_state), ("the other", other_state)] {
            let Some(state) = state else { continue };
            for record in &state.records {
                if record.checkout.as_deref() != Some(checkout) || record.id == own {
                    continue;
                }
                if record.phase == Phase::Retired {
                    if let Some(lost) = record.lost.as_ref() {
                        if let Some(why) = lost.unclosed.as_deref() {
                            return Ok(CheckoutCustody::Unknown(format!(
                                "slot {} in the {pool} pool is lost with writers that were never confirmed closed ({why})",
                                lost.slot_id
                            )));
                        }
                    }
                    continue;
                }
                if matches!(record.phase, Phase::Spawning | Phase::Running) {
                    writers = true;
                }
            }
        }
        Ok(if writers {
            CheckoutCustody::Writers
        } else {
            CheckoutCustody::Clear
        })
    }

    /// The sibling pool's admission lock, when this store is the removable
    /// one. Always internal first, as slot attempts and cleans do.
    fn sibling_admission(&self) -> Result<Option<File>, String> {
        let Some(sibling) = self.sibling.as_ref().filter(|sibling| sibling.barrier) else {
            return Ok(None);
        };
        let file = private_file(&sibling.control.join("admission.lock"), true)?
            .ok_or("the internal admission lock is missing")?;
        flock(&file, libc::LOCK_EX)?;
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
        flock(&file, libc::LOCK_EX)?;
        let cleaning = match self.sibling_state() {
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
    pub fn discard(&self, held: Lease) -> Result<(), String> {
        let _admission = self.admission()?;
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
    /// cancels.
    pub fn begin_clean(
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
                    || self.sibling_busy(checkout);
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

    pub fn report(&self) -> Result<Report, String> {
        let pool = self.status()?;
        let state = if self.control.exists() {
            self.load()?
        } else {
            State::default()
        };
        let slots = pool
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
                            path: return_path(record, &LedgerSlot::from(slot)),
                        }),
                    retired: record.and_then(|record| record.retired.clone()),
                    holder: record
                        .filter(|_| slot.state == SlotOccupancy::Held)
                        .and_then(|record| record.holder.as_ref())
                        .and_then(|holder| holder.command.clone()),
                }
            })
            .collect::<Vec<SlotView>>();
        let last_use = state
            .records
            .iter()
            .filter(|record| record.last_used > 0)
            .filter_map(|record| {
                let project = self.affinity_dir(record.id);
                let slot = slots_view(&pool, &project)?;
                Some((record, slot))
            })
            .max_by_key(|(record, _)| record.last_used)
            .map(|(record, slot)| LastUse {
                slot_id: Some(slot.slot_id.clone()),
                label: slot_label(slot.dir.as_deref(), record.checkout.as_deref()),
                checkout: record.checkout.clone(),
                at_ms: record.last_used_ms,
            });
        let now = SystemTime::now();
        let waiters = state
            .tickets
            .iter()
            .map(|ticket| Waiter {
                pid: ticket.pid,
                age: ticket_age(&self.ticket_path(ticket.id), now),
            })
            .collect();
        Ok(Report {
            last_use,
            pool,
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
        })
    }

    fn attempt(
        &self,
        ticket: u64,
        key: &SlotKey,
        profile: Profile,
        max_active: usize,
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
            + self.sibling_active();
        if active >= max_active {
            self.save(&state)?;
            return Ok(Err(Wait::HostLimit(active)));
        }

        let view = self.ledger()?;
        let has_slot = |record: &Record| {
            let project = self.affinity_dir(record.id);
            view.slots.iter().any(|slot| slot.project == project)
        };
        let matches = |record: &Record| {
            record.phase != Phase::Retired
                && record.checkout.as_deref() == Some(key.checkout.as_path())
                && record.compatibility == key.compatibility
        };
        let stale: Vec<u64> = state
            .records
            .iter()
            .filter(|record| record.phase == Phase::Idle && !has_slot(record) && !matches(record))
            .map(|record| record.id)
            .collect();
        for id in stale {
            self.forget_record(&mut state, id);
        }

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
        let mut shared_from = None;
        let mut shared_origin = None;
        let shared = if share && !existing.is_some_and(|index| has_slot(&state.records[index])) {
            self.shared_candidate(&state, key, &view)?
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
            let mut candidates: Vec<(u64, u64, String)> = state
                .records
                .iter()
                .filter(|record| record.identity == key.identity && has_slot(record))
                .filter_map(|record| {
                    let project = self.affinity_dir(record.id);
                    view.slots
                        .iter()
                        .find(|slot| slot.project == project && slot.state == SlotOccupancy::Idle)
                        .map(|slot| (record.last_used, record.id, slot.slot_id.clone()))
                })
                .collect();
            let project_slots = state
                .records
                .iter()
                .filter(|record| record.phase != Phase::Retired)
                .filter(|record| record.identity == key.identity && has_slot(record))
                .count();
            if project_slots >= PROJECT_SLOT_CAP {
                let chosen = existing.map(|index| state.records[index].id);
                candidates.sort();
                let mut evicted = false;
                for (_, id, slot_id) in candidates {
                    if Some(id) == chosen {
                        continue;
                    }
                    let Probe::Free(_victim) = probe_lock(&self.lock_path(id), false)? else {
                        continue;
                    };
                    evict_idle_slot(&self.pool, &slot_id)
                        .map_err(|error| lease_error("cannot evict an idle slot", &error))?;
                    eprintln!(
                        "oxide: evicted idle slot {slot_id} (project slot cap {PROJECT_SLOT_CAP})"
                    );
                    self.forget_record(&mut state, id);
                    evicted = true;
                    break;
                }
                if !evicted {
                    self.save(&state)?;
                    return Ok(Err(if busy {
                        Wait::Checkout
                    } else {
                        Wait::ProjectCap
                    }));
                }
                // Eviction forgot only an idle record whose lock was free,
                // never the chosen one; find it again by ID.
                existing =
                    chosen.and_then(|id| state.records.iter().position(|record| record.id == id));
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
        let request = LeaseRequest {
            pool: self.pool.clone(),
            project: self.affinity_dir(record),
            workspace: key.checkout.clone(),
            profile: match profile {
                Profile::Debug => LeaseProfile::FullDebug,
                Profile::Quick | Profile::Release => LeaseProfile::Dev,
            },
            holder: lease_holder,
            budget_bytes: self.budget,
            layout: Some(SlotLayout {
                folders,
                same_repository,
                profile: profile.label().to_owned(),
            }),
        };
        let outcome = lease_measured(request);
        let (grant, measured) = match outcome {
            Ok(leased) => (leased.grant, leased.measured),
            Err(error) => {
                state.settle_idle(index);
                if shared_from.is_some() {
                    state.records[index].checkout = shared_from;
                }
                self.save(&state)?;
                return match error {
                    LeaseError::Busy => Ok(Err(Wait::Uncertain)),
                    LeaseError::Full if active > 0 => Ok(Err(Wait::Capacity)),
                    error => Err(lease_error("cannot lease a slot", &error)),
                };
            }
        };
        state.records[index].lease_id = Some(grant.lease_id.clone());
        self.save(&state)?;
        let prior = match (grant.reused_slot, recorded) {
            (false, _) => Prior::Fresh,
            (true, Some(origin)) => Prior::Known(origin),
            (true, None) => Prior::Unknown,
        };
        Ok(Ok(Lease {
            slot_id: grant.slot_id,
            reused: grant.reused_slot,
            build_dir: grant.env.cargo_build_build_dir,
            prior,
            shared_from,
            measured,
            lease_id: grant.lease_id,
            record,
            _lock: lock,
        }))
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
    fn shared_candidate(
        &self,
        state: &State,
        key: &SlotKey,
        view: &LedgerView,
    ) -> Result<Option<(u64, File, Built)>, String> {
        let mut candidates: Vec<&Record> = state
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
        let file = private_file(&self.origin_path(record.id), false).ok()??;
        let mut bytes = Vec::new();
        file.take(MAX_ORIGIN_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > MAX_ORIGIN_BYTES {
            return None;
        }
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

    fn admission(&self) -> Result<File, String> {
        lock_admission(&self.control)
    }

    fn load(&self) -> Result<State, String> {
        let Some(file) = private_file(&self.control.join("state.json"), false)? else {
            return Ok(State::default());
        };
        let mut bytes = Vec::new();
        file.take(MAX_STATE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read Oxide state: {error}"))?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            return Err("Oxide state exceeds its byte limit".to_owned());
        }
        let state: State = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Oxide state is invalid: {error}"))?;
        if state.version != STATE_VERSION
            || state.tickets.len() > MAX_TICKETS
            || state.cleans.len() > MAX_TICKETS
            || state.records.len() > MAX_RECORDS
            || state.recoveries.len() > MAX_RECOVERIES
        {
            return Err("Oxide state has an unsupported version or exceeds a limit".to_owned());
        }
        Ok(state)
    }

    fn save(&self, state: &State) -> Result<(), String> {
        let bytes = serde_json::to_vec(state).map_err(|error| error.to_string())?;
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
            .map_err(|error| format!("cannot replace Oxide state: {error}"))
    }
}

fn create_pool(path: &Path, budget: u64) -> Result<(), String> {
    let parent = path.parent().ok_or("the pool path has no parent")?;
    let name = path
        .file_name()
        .ok_or("the pool path has no final component")?;
    create_parents(parent)?;
    let parent = fs::canonicalize(parent).map_err(|error| error.to_string())?;
    let available = statfs(&parent)
        .and_then(|sample| {
            let block = u64::try_from(sample.f_bsize).ok()?;
            sample.f_bavail.checked_mul(block)
        })
        .ok_or("cannot sample host free space for pool creation")?;
    if available < HOST_RESERVE_BYTES {
        return Err(format!(
            "refusing to create the pool: host free space {} MiB is below the 20 GiB reserve",
            available / (1024 * 1024)
        ));
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

/// Take the internal pool's existing admission lock for metadata updates and
/// command admission. Only the lock file may be created; the control
/// directory must already exist.
pub(crate) fn lock_admission(control: &Path) -> Result<File, String> {
    let directory = open_control_directory(control)?
        .ok_or_else(|| format!("{} does not exist", control.display()))?;
    let name = CString::new("admission.lock").map_err(|error| error.to_string())?;
    // Oxide never removes a control directory, so an `ENOENT` for this create
    // in the verified directory is transient: a concurrent create of the same
    // lock was observed to report it on macOS APFS (oxide#117 fixture). Retry
    // a bounded number of times on the same verified descriptor; a directory
    // that is really gone still fails after the attempts.
    let mut fd = -1;
    for attempt in 0..3 {
        // SAFETY: `name` is a live CString with initialized bytes through its
        // terminator, and `directory` owns a live descriptor for this call.
        fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd >= 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::NotFound || attempt == 2 {
            return Err(format!(
                "cannot open admission lock in {}: {error}",
                control.display()
            ));
        }
        std::thread::yield_now();
    }
    if fd < 0 {
        return Err(format!(
            "cannot open admission lock in {}: the create did not succeed",
            control.display()
        ));
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
    flock(&file, libc::LOCK_EX)?;
    Ok(file)
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

fn lease_error(context: &str, error: &LeaseError) -> String {
    format!("{context}: {} ({error})", error.reason())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        waits.observe(&Wait::Capacity, 2, start + Duration::from_secs(3));
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
            Wait::Capacity,
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
            retired: lost.as_ref().map(Lost::describe),
            lost,
            last_used_ms: Some(1),
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
        assert!(internal.sibling_busy(checkout));
        assert!(!internal.sibling_busy(other));
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
            removable.attempt(ticket, &key, Profile::Quick, 2, false, None),
            Ok(Err(Wait::HostLimit(2)))
        ));
        removable.remove_ticket(ticket).unwrap();

        // An internal admission sees two active removable records.
        internal.save(&State::default()).unwrap();
        removable.save(&active).unwrap();
        let (ticket, _file) = internal.register_ticket().unwrap();
        assert!(matches!(
            internal.attempt(ticket, &key, Profile::Quick, 2, false, None),
            Ok(Err(Wait::HostLimit(2)))
        ));
        internal.remove_ticket(ticket).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    fn lease(store: &Store, key: &SlotKey, label: Option<&str>) -> Result<Lease, Wait> {
        let (ticket, _file) = store.register_ticket().unwrap();
        let outcome = store
            .attempt(ticket, key, Profile::Quick, 8, false, label)
            .unwrap();
        store.remove_ticket(ticket).unwrap();
        outcome
    }

    /// Second-slot rule: a busy slot of this checkout's key sends a further
    /// command to another idle compatible slot or a new one; a slot whose
    /// holder died while its group lives is never reused; with no safe
    /// candidate under the project cap the command waits.
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
            max_active: 2,
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
        let Ok(first) = lease(&store, &key, Some("exec tool")) else {
            panic!("the first command waited");
        };
        // The holder's group is this test's own, which is alive.
        // SAFETY: getpgrp has no preconditions.
        let group = unsafe { libc::getpgrp() };
        store.mark_spawning(&first).unwrap();
        store.mark_running(&first, group).unwrap();
        let Ok(second) = lease(&store, &key, None) else {
            panic!("a further command waited behind a busy slot");
        };
        assert_ne!(second.slot_id, first.slot_id);
        let report = store.report().unwrap();
        let shown = |slot: &str| {
            report
                .slots
                .iter()
                .find(|view| view.slot_id == slot)
                .and_then(|view| view.holder.clone())
        };
        assert_eq!(shown(&first.slot_id).as_deref(), Some("exec tool"));
        assert_eq!(shown(&second.slot_id), None);
        // Both slots of the project are held: the cap leaves no safe slot.
        assert!(matches!(lease(&store, &key, None), Err(Wait::Checkout)));
        // The first holder dies while its group lives: its slot stays held
        // and is never reused; the released second slot is taken instead.
        let first_slot = first.slot_id.clone();
        drop(first);
        let second_slot = second.slot_id.clone();
        store.release(second, None).unwrap();
        let Ok(third) = lease(&store, &key, None) else {
            panic!("an idle compatible slot was not taken");
        };
        assert_eq!(third.slot_id, second_slot);
        let view = store.ledger().unwrap();
        let held = view
            .slots
            .iter()
            .find(|slot| slot.slot_id == first_slot)
            .unwrap();
        assert_eq!(held.state, SlotOccupancy::Held);
        assert!(matches!(lease(&store, &key, None), Err(Wait::Checkout)));
        store.release(third, None).unwrap();
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
            .describe()
            .contains("deleted whole"));
        lost.lost.as_mut().unwrap().unclosed = Some("why".to_owned());
        assert!(lost
            .lost
            .as_ref()
            .unwrap()
            .describe()
            .starts_with("quarantined"));
        let encoded = serde_json::to_string(&lost).unwrap();
        let decoded: Record = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.lost.unwrap().unclosed.as_deref(), Some("why"));
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
}
