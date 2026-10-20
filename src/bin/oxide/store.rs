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
use crate::identity::SlotKey;
use crate::process;
use crate::watchdog::Watchdog;
use oxide_storage::lease::managed::{
    evict_idle_slot, flush_volume, init_budget_pool, lease, recover, release, status as pool_status,
};
use oxide_storage::lease::{
    LeaseError, LeaseRequest, PoolStatus, Profile as LeaseProfile, SlotOccupancy, SlotStatus,
    BUDGET_BACKEND_DIRECTORY_V1, MANAGED_NAME, VOLUME_BACKEND_APFS_V1,
};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const GIB: u64 = 1024 * 1024 * 1024;
/// Automatic creation refuses below this host free space.
pub const HOST_RESERVE_BYTES: u64 = 20 * GIB;
const DEFAULT_MAX_ACTIVE: usize = 2;
const MAX_ACTIVE_LIMIT: usize = 32;
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
}

impl Record {
    /// Holds a command now: neither idle nor retired.
    fn active(&self) -> bool {
        !matches!(self.phase, Phase::Idle | Phase::Retired)
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
fn return_path(record: &Record, slot: &SlotStatus) -> Return {
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

/// The own-command recovery rule. `held` is the held slot on the record's
/// private project, if any. `group_empty` is `killpg(pgid, 0)` reporting
/// `ESRCH`; a surviving member or a reused group keeps the slot held.
fn settle(record: &Record, held: Option<&SlotStatus>, group_empty: impl Fn(i32) -> bool) -> Settle {
    let Some(slot) = held else {
        return Settle::Reset;
    };
    match return_path(record, slot) {
        Return::NeverStarted => Settle::Recover,
        Return::GroupEmpty(group) if group_empty(group) => Settle::Recover,
        Return::GroupEmpty(_) | Return::GroupUnrecorded | Return::NotOwn => Settle::Keep,
    }
}

/// Why a command is waiting. Only the first reason is printed.
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
    lease_id: String,
    record: u64,
    _lock: File,
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

/// Pool and slot view for `oxide status`.
pub struct Report {
    pub pool: PoolStatus,
    pub flushes: Vec<Flush>,
    pub waiting: usize,
    pub cleaning: Vec<PathBuf>,
    pub slots: Vec<SlotView>,
    pub recoveries: Vec<Recovery>,
}

pub struct SlotView {
    pub slot_id: String,
    pub state: SlotOccupancy,
    pub profile: Option<String>,
    pub checkout: Option<PathBuf>,
    pub managed: bool,
    /// Held with no live Oxide owner. The next command recovers it once
    /// Cargo's group is empty; if the group was never recorded,
    /// `oxide-storage-lease recover` is explicit.
    pub interrupted: Option<Interrupted>,
    /// Why a removable-volume slot was retired and stays held.
    pub retired: Option<String>,
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

/// The internal pool path from the effective settings.
pub fn pool_path() -> Result<PathBuf, String> {
    settings().map(|settings| settings.pool)
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

/// `OXIDE_MAX_ACTIVE`, else 2.
pub fn max_active() -> Result<usize, String> {
    let Some(value) = std::env::var_os("OXIDE_MAX_ACTIVE") else {
        return Ok(DEFAULT_MAX_ACTIVE);
    };
    value
        .to_str()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=MAX_ACTIVE_LIMIT).contains(value))
        .ok_or_else(|| format!("OXIDE_MAX_ACTIVE must be an integer from 1 to {MAX_ACTIVE_LIMIT}"))
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
    pub fn open_removable(root: &Path, payload: PathBuf) -> Result<Self, String> {
        let pool = fs::canonicalize(root)
            .map_err(|error| format!("cannot open the removable pool: {error}"))?;
        let status = pool_status(&pool)
            .map_err(|error| lease_error("cannot open the removable pool", &error))?;
        if Backend::recorded(&status.backend)? != Backend::ApfsVolume {
            return Err("the removable pool records another backend".to_owned());
        }
        let store = Self {
            control: pool.join(MANAGED_NAME),
            pool,
            backend: Backend::ApfsVolume,
            payload,
            budget: None,
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

    /// Open an existing pool. Never creates one; a retired image pool
    /// refuses without being touched.
    pub fn open_existing() -> Result<Option<Self>, String> {
        let settings = settings()?;
        match config::inspect(&settings.pool) {
            Found::Absent => Ok(None),
            Found::LegacyImage => Err(legacy_refusal()),
            Found::Directory | Found::Unknown => {
                config::refuse_unsafe_pool_path(&settings.pool)?;
                Self::open_at(&settings.pool, &settings).map(Some)
            }
        }
    }

    fn open_at(path: &Path, settings: &Settings) -> Result<Self, String> {
        let pool =
            fs::canonicalize(path).map_err(|error| format!("cannot open the pool: {error}"))?;
        let status =
            pool_status(&pool).map_err(|error| lease_error("cannot open the pool", &error))?;
        let backend = Backend::recorded(&status.backend)?;
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
            Backend::ApfsVolume => None,
            Backend::DirectoryBudget => Some(Watchdog::for_pool(&self.payload)),
        }
    }

    pub fn status(&self) -> Result<PoolStatus, String> {
        pool_status(&self.pool).map_err(|error| lease_error("cannot read the pool", &error))
    }

    /// Lease this checkout's slot, waiting in FIFO order. Ctrl-C while waiting
    /// cancels without touching any slot.
    /// `share` allows taking another checkout's idle compatible slot.
    pub fn acquire(
        &self,
        key: &SlotKey,
        profile: Profile,
        share: bool,
    ) -> Result<Acquired, String> {
        let max_active = max_active()?;
        let ticket = self.register_ticket()?;
        let mut announced = false;
        loop {
            if let Some(signal) = process::received_signal() {
                self.remove_ticket(ticket.0)?;
                return Ok(Acquired::Interrupted(signal));
            }
            match self.attempt(ticket.0, key, profile, max_active, share)? {
                Ok(lease) => {
                    self.remove_ticket(ticket.0)?;
                    return Ok(Acquired::Lease(lease));
                }
                Err(wait) => {
                    if !announced {
                        eprintln!(
                            "oxide: waiting for {}; Ctrl-C cancels",
                            wait.describe(max_active)
                        );
                        announced = true;
                    }
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
    /// a durability step after writer closure, not a closure proof. The
    /// caller holds the admission lock.
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
                });
                true
            }
            Err(error) => {
                let reason = format!("flush before release failed: {error}");
                eprintln!(
                    "oxide: removable volume flush failed for slot {slot_id} ({error}); the slot is retired and stays held, never reused"
                );
                state.log_flush(Flush {
                    slot_id: slot_id.to_owned(),
                    at_ms: process::now_ms(),
                    micros: None,
                    error: Some(error.to_string()),
                });
                if let Some(entry) = state.records.iter_mut().find(|entry| entry.id == record) {
                    entry.phase = Phase::Retired;
                    entry.process_group = None;
                    entry.retired = Some(reason);
                }
                false
            }
        };
        self.save(&state)?;
        Ok(flushed)
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
    pub fn begin_clean(&self, checkout: &Path) -> Result<Cleaning, String> {
        self.ensure_control()?;
        let mut announced = false;
        loop {
            if let Some(signal) = process::received_signal() {
                return Ok(Cleaning::Interrupted(signal));
            }
            {
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
                    return Ok(Cleaning::Guard(CleanGuard { id, lock }));
                }
                self.save(&state)?;
            }
            if !announced {
                eprintln!(
                    "oxide: waiting for active oxide commands in this checkout; Ctrl-C cancels"
                );
                announced = true;
            }
            std::thread::sleep(WAIT_POLL);
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
                            path: return_path(record, slot),
                        }),
                    retired: record.and_then(|record| record.retired.clone()),
                }
            })
            .collect();
        Ok(Report {
            pool,
            flushes: state.flushes,
            waiting: state.tickets.len(),
            cleaning: state
                .cleans
                .iter()
                .map(|clean| clean.checkout.clone())
                .collect(),
            slots,
            recoveries: state.recoveries,
        })
    }

    /// Slot allocation and this command's soft budget.
    pub fn payload_use(&self) -> Option<(u64, u64)> {
        let status = self.status().ok()?;
        Some((
            status.payload_unique_allocated_bytes?,
            self.budget(status.cap_bytes),
        ))
    }

    fn attempt(
        &self,
        ticket: u64,
        key: &SlotKey,
        profile: Profile,
        max_active: usize,
        share: bool,
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

        let status = self.status()?;
        let has_slot = |record: &Record| {
            let project = self.affinity_dir(record.id);
            status.slots.iter().any(|slot| slot.project == project)
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

        let mut existing = state.records.iter().position(|record| matches(record));
        let mut lock = None;
        if let Some(index) = existing {
            if state.records[index].phase != Phase::Idle {
                self.save(&state)?;
                return Ok(Err(Wait::Checkout));
            }
            match probe_lock(&self.lock_path(state.records[index].id), true)? {
                Probe::Free(file) => lock = Some(file),
                Probe::Held | Probe::Missing => {
                    self.save(&state)?;
                    return Ok(Err(Wait::Checkout));
                }
            }
        }
        let mut shared_from = None;
        let mut shared_origin = None;
        let shared = if share && !existing.is_some_and(|index| has_slot(&state.records[index])) {
            self.shared_candidate(&state, key, &status)?
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
                    status
                        .slots
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
                candidates.sort();
                let mut evicted = false;
                for (_, id, slot_id) in candidates {
                    if Some(id) == existing.map(|index| state.records[index].id) {
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
                    return Ok(Err(Wait::ProjectCap));
                }
                existing = state.records.iter().position(|record| matches(record));
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
        };
        let outcome = lease(request);
        let grant = match outcome {
            Ok(grant) => grant,
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
            lease_id: grant.lease_id,
            record,
            _lock: lock,
        }))
    }

    /// Own-command crash recovery: a non-idle record whose lock is free has
    /// no live Oxide owner. [`settle`] decides whether its held lease can be
    /// recovered; each recovery is logged for `oxide status`.
    fn recover_settled(&self, state: &mut State) -> Result<(), String> {
        let mut status: Option<PoolStatus> = None;
        for index in 0..state.records.len() {
            let record = &state.records[index];
            if !record.active() {
                continue;
            }
            let Probe::Free(_owner) = probe_lock(&self.lock_path(record.id), true)? else {
                continue;
            };
            if status.is_none() {
                status = Some(self.status()?);
            }
            let project = self.affinity_dir(record.id);
            let held = status.as_ref().and_then(|status| {
                status
                    .slots
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
                    status = None;
                    continue;
                }
            }
            state.settle_idle(index);
        }
        Ok(())
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
        status: &PoolStatus,
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
                status
                    .slots
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
        for directory in ["", "affinity", "locks", "waiters"] {
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
        let file = private_file(&self.control.join("admission.lock"), true)?
            .ok_or("admission lock is missing")?;
        flock(&file, libc::LOCK_EX)?;
        Ok(file)
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
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(format!(
            "{} must be an owner-only directory",
            path.display()
        ));
    }
    Ok(())
}

/// Open an owner-only regular control file without following symlinks.
fn private_file(path: &Path, create: bool) -> Result<Option<File>, String> {
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

    fn paired_controls() -> (PathBuf, Store, Store) {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "oxide-barrier-{}-{}",
            std::process::id(),
            process::now_ms()
        ));
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let store = |name: &str| Store {
            pool: root.join(name),
            backend: Backend::DirectoryBudget,
            payload: root.join(name).join("payload"),
            control: root.join(name).join(MANAGED_NAME),
            budget: None,
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

    /// The checkout clean barrier spans both pools: an internal clean waits
    /// for the checkout's active removable command, and a removable slot
    /// attempt sees a live internal clean under the internal admission lock.
    #[test]
    fn clean_barrier_spans_both_pools() {
        let (root, internal, removable) = paired_controls();
        let checkout = Path::new("/checkout");
        let other = Path::new("/other");
        let mut state = State::default();
        state
            .records
            .push(record(Phase::Running, Some(i32::MAX), Some("v1.a.1.1.1")));
        removable.save(&state).unwrap();
        assert!(internal.sibling_busy(checkout));
        assert!(!internal.sibling_busy(other));
        let Cleaning::Guard(guard) = internal.begin_clean(other).unwrap() else {
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
            }),
            lease_id: lease_id.map(str::to_owned),
            pending_recovery: None,
            retired: None,
        }
    }

    fn held(holder: &str, lease_id: &str) -> SlotStatus {
        SlotStatus {
            slot_id: "1".to_owned(),
            incarnation: "1".to_owned(),
            state: SlotOccupancy::Held,
            project: PathBuf::from("/pool/oxide-managed/affinity/1"),
            workspace: PathBuf::from("/checkout"),
            profile: LeaseProfile::Dev,
            holder: Some(holder.to_owned()),
            lease_id: Some(lease_id.to_owned()),
            logical_bytes: None,
            unique_allocated_bytes: None,
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
