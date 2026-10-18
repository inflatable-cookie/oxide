//! Oxide's private control state beside the lease ledger.
//!
//! The ledger, its JSON and the standalone CLI stay unchanged. Oxide leases
//! through the library with one owner-only project directory per affinity
//! record (checkout + compatibility key), so the library's "same project,
//! same profile" reuse is checkout-affine by construction. The control
//! directory `<pool>/oxide-managed` holds the records, FIFO wait tickets, the
//! admission lock and one `flock` per record held for the life of a command.

use crate::cli::Profile;
use crate::identity::SlotKey;
use crate::process;
use oxide_storage::lease::{
    evict_idle_slot, init_pool, lease, recover, release, status as pool_status, LeaseError,
    LeaseRequest, PoolStatus, Profile as LeaseProfile, SlotOccupancy, MANAGED_NAME,
};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

const GIB: u64 = 1024 * 1024 * 1024;
/// Cap of an automatically created pool; the sparse image is the same size.
const DEFAULT_CAP_BYTES: u64 = 24 * GIB;
/// Automatic creation refuses below this host free space.
const HOST_RESERVE_BYTES: u64 = 20 * GIB;
const DEFAULT_MAX_ACTIVE: usize = 2;
const MAX_ACTIVE_LIMIT: usize = 32;
/// Slots one project identity may keep; a new checkout at the cap evicts the
/// least recently used idle one.
const PROJECT_SLOT_CAP: usize = 2;
const MAX_TICKETS: usize = 128;
const MAX_RECORDS: usize = 64;
const MAX_STATE_BYTES: u64 = 256 * 1024;
const STATE_VERSION: u32 = 1;
const WAIT_POLL: Duration = Duration::from_millis(200);

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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ticket {
    id: u64,
    pid: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    next_id: u64,
    next_use: u64,
    tickets: Vec<Ticket>,
    records: Vec<Record>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            next_id: 1,
            next_use: 1,
            tickets: Vec::new(),
            records: Vec::new(),
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
}

/// Why a command is waiting. Only the first reason is printed.
enum Wait {
    Queue(usize),
    HostLimit(usize),
    Checkout,
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
    lease_id: String,
    record: u64,
    _lock: File,
}

pub enum Acquired {
    Lease(Lease),
    Interrupted(i32),
}

/// Pool and slot view for `oxide status`.
pub struct Report {
    pub pool: PoolStatus,
    pub waiting: usize,
    pub slots: Vec<SlotView>,
}

pub struct SlotView {
    pub slot_id: String,
    pub state: SlotOccupancy,
    pub profile: Option<String>,
    pub checkout: Option<PathBuf>,
    pub managed: bool,
    /// Held with no live Oxide owner. The next command recovers it unless
    /// Cargo may still run; then `oxide-storage-lease recover` is explicit.
    pub interrupted: bool,
    pub bytes: Option<u64>,
}

pub struct Store {
    pub pool: PathBuf,
    control: PathBuf,
}

/// `OXIDE_POOL`, else `~/Library/Caches/oxide/pool`.
pub fn pool_path() -> Result<PathBuf, String> {
    let configured = match std::env::var_os("OXIDE_POOL").filter(|value| !value.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => {
            let home = std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .ok_or("HOME is unset; set OXIDE_POOL to choose the pool")?;
            Path::new(&home).join("Library/Caches/oxide/pool")
        }
    };
    if configured.is_absolute() {
        Ok(configured)
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(configured))
            .map_err(|error| format!("cannot resolve OXIDE_POOL: {error}"))
    }
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
    /// Open the pool, creating it on first use above the host reserve.
    pub fn open_or_create() -> Result<Self, String> {
        let path = pool_path()?;
        if fs::symlink_metadata(&path).is_err() {
            create_pool(&path)?;
        }
        let store = Self::open_at(&path)?;
        store.ensure_control()?;
        Ok(store)
    }

    /// Open an existing pool. Never creates one.
    pub fn open_existing() -> Result<Option<Self>, String> {
        let path = pool_path()?;
        if fs::symlink_metadata(&path).is_err() {
            return Ok(None);
        }
        Self::open_at(&path).map(Some)
    }

    fn open_at(path: &Path) -> Result<Self, String> {
        let pool =
            fs::canonicalize(path).map_err(|error| format!("cannot open the pool: {error}"))?;
        pool_status(&pool).map_err(|error| lease_error("cannot open the pool", &error))?;
        let control = pool.join(MANAGED_NAME);
        Ok(Self { pool, control })
    }

    pub fn status(&self) -> Result<PoolStatus, String> {
        pool_status(&self.pool).map_err(|error| lease_error("cannot read the pool", &error))
    }

    /// Lease this checkout's slot, waiting in FIFO order. Ctrl-C while waiting
    /// cancels without touching any slot.
    pub fn acquire(&self, key: &SlotKey, profile: Profile) -> Result<Acquired, String> {
        let max_active = max_active()?;
        let ticket = self.register_ticket()?;
        let mut announced = false;
        loop {
            if let Some(signal) = process::received_signal() {
                self.remove_ticket(ticket.0)?;
                return Ok(Acquired::Interrupted(signal));
            }
            match self.attempt(ticket.0, key, profile, max_active)? {
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
    /// [`Store::mark_running`] leaves the slot held for explicit recovery.
    pub fn mark_spawning(&self, lease: &Lease) -> Result<(), String> {
        self.update_record(lease.record, |record| {
            record.phase = Phase::Spawning;
            record.process_group = None;
        })
    }

    pub fn mark_running(&self, lease: &Lease, group: i32) -> Result<(), String> {
        self.update_record(lease.record, |record| {
            record.phase = Phase::Running;
            record.process_group = Some(group);
        })
    }

    /// Release after every writer in the group has exited.
    pub fn release(&self, held: Lease) -> Result<(), String> {
        let _admission = self.admission()?;
        release(&self.pool, &held.lease_id)
            .map_err(|error| lease_error("could not release the slot", &error))?;
        let mut state = self.load()?;
        if let Some(index) = state
            .records
            .iter()
            .position(|record| record.id == held.record)
        {
            state.records[index].phase = Phase::Idle;
            state.records[index].process_group = None;
            state.touch(index)?;
        }
        self.save(&state)
    }

    /// Drop this checkout's slot affinity. Slots stay intact.
    pub fn detach_checkout(&self, checkout: &Path) -> Result<usize, String> {
        if !self.control.exists() {
            return Ok(0);
        }
        let _admission = self.admission()?;
        let mut state = self.load()?;
        let mut detached = 0;
        for record in &mut state.records {
            if record.checkout.as_deref() == Some(checkout) {
                record.checkout = None;
                detached += 1;
            }
        }
        if detached > 0 {
            self.save(&state)?;
        }
        Ok(detached)
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
                    interrupted: record.is_some_and(|record| {
                        record.phase != Phase::Idle
                            && slot.state == SlotOccupancy::Held
                            && !matches!(
                                probe_lock(&self.lock_path(record.id), false),
                                Ok(Probe::Held)
                            )
                    }),
                    bytes: slot.unique_allocated_bytes,
                }
            })
            .collect();
        Ok(Report {
            pool,
            waiting: state.tickets.len(),
            slots,
        })
    }

    /// Used and total bytes of the mounted payload volume.
    pub fn payload_use(&self) -> Option<(u64, u64)> {
        let sample = statfs(&self.pool.join("payload"))?;
        let block = u64::try_from(sample.f_bsize).ok()?;
        let total = sample.f_blocks.checked_mul(block)?;
        let free = sample.f_bfree.checked_mul(block)?;
        Some((total.saturating_sub(free), total))
    }

    fn attempt(
        &self,
        ticket: u64,
        key: &SlotKey,
        profile: Profile,
        max_active: usize,
    ) -> Result<Result<Lease, Wait>, String> {
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
        self.recover_settled(&mut state)?;
        let active = state
            .records
            .iter()
            .filter(|record| record.phase != Phase::Idle)
            .count();
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
            record.checkout.as_deref() == Some(key.checkout.as_path())
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
        if !existing.is_some_and(|index| has_slot(&state.records[index])) {
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
                });
                state.records.len() - 1
            }
        };
        let lock = lock.ok_or("Oxide record lock was not taken")?;
        let record = state.records[index].id;
        state.records[index].phase = Phase::Acquiring;
        state.touch(index)?;
        self.save(&state)?;

        let request = LeaseRequest {
            pool: self.pool.clone(),
            project: self.affinity_dir(record),
            workspace: key.checkout.clone(),
            profile: match profile {
                Profile::Debug => LeaseProfile::FullDebug,
                Profile::Quick | Profile::Release => LeaseProfile::Dev,
            },
            holder: format!("oxide-{}-{ticket}", std::process::id()),
        };
        let outcome = lease(request);
        let grant = match outcome {
            Ok(grant) => grant,
            Err(error) => {
                state.records[index].phase = Phase::Idle;
                self.save(&state)?;
                return match error {
                    LeaseError::Busy => Ok(Err(Wait::Uncertain)),
                    LeaseError::Full if active > 0 => Ok(Err(Wait::Capacity)),
                    error => Err(lease_error("cannot lease a slot", &error)),
                };
            }
        };
        Ok(Ok(Lease {
            slot_id: grant.slot_id,
            reused: grant.reused_slot,
            build_dir: grant.env.cargo_build_build_dir,
            lease_id: grant.lease_id,
            record,
            _lock: lock,
        }))
    }

    /// Own-command crash recovery: a non-idle record whose lock is free has no
    /// live Oxide owner. Its held lease is recovered when no writer can
    /// remain: Cargo never started, or its recorded group is empty. A record
    /// that crashed while spawning stays held for explicit recovery.
    fn recover_settled(&self, state: &mut State) -> Result<(), String> {
        let mut status: Option<PoolStatus> = None;
        for index in 0..state.records.len() {
            let record = &state.records[index];
            if record.phase == Phase::Idle {
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
            let settled = match (record.phase, held) {
                (_, None) | (Phase::Acquiring, Some(_)) => true,
                (Phase::Running, Some(_)) => record.process_group.is_some_and(process::group_empty),
                (Phase::Spawning | Phase::Idle, Some(_)) => false,
            };
            if !settled {
                continue;
            }
            if let Some(slot) = held {
                let lease_id = slot
                    .lease_id
                    .as_deref()
                    .ok_or("held slot has no lease ID")?;
                recover(&self.pool, lease_id)
                    .map_err(|error| lease_error("cannot recover a settled slot", &error))?;
                eprintln!(
                    "oxide: recovered slot {} after an interrupted oxide command",
                    slot.slot_id
                );
                status = None;
            }
            state.records[index].phase = Phase::Idle;
            state.records[index].process_group = None;
        }
        Ok(())
    }

    fn forget_record(&self, state: &mut State, id: u64) {
        state.records.retain(|record| record.id != id);
        let _ = fs::remove_dir(self.affinity_dir(id));
        let _ = fs::remove_file(self.lock_path(id));
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

    /// A ticket whose lock is free belongs to a command that ended.
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
            || state.records.len() > MAX_RECORDS
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

fn create_pool(path: &Path) -> Result<(), String> {
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
    eprintln!(
        "oxide: creating the build pool at {} ({} GiB cap)",
        pool.display(),
        DEFAULT_CAP_BYTES / GIB
    );
    match init_pool(&pool, DEFAULT_CAP_BYTES, DEFAULT_CAP_BYTES) {
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

fn statfs(path: &Path) -> Option<libc::statfs> {
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
