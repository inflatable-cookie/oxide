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
//! and the group is empty; see [`recovery::settle`]. Standalone leases have no
//! record and are never recovered here.
//!
//! The module is split by concern: [`admission`] leases a slot and accounts
//! for the wait, [`release`] hands one back, [`recovery`] settles crashed and
//! removable-volume leftovers, [`seeding`] drives cold-slot seeding,
//! [`barrier`] holds the clean and checkout barriers, [`status`] reads the
//! pool for `oxide status`, [`outputs`] runs the owned-outputs
//! initialization, and [`control_io`] owns the control directory's private
//! files, pool creation and admission lock.

mod admission;
mod barrier;
mod control_io;
mod outputs;
mod recovery;
mod release;
mod relocation;
mod seeding;
mod status;

#[cfg(test)]
mod launch_hold_tests;
#[cfg(test)]
mod prune_layout_v2_tests;
#[cfg(all(test, target_os = "macos"))]
mod scratch_internal_tests;
#[cfg(all(test, target_os = "macos"))]
mod seed_layout_v2_tests;
#[cfg(test)]
pub(crate) mod tests;

pub(crate) use oxide_storage::lease::control::*;

pub(crate) use admission::{Acquired, Lease, WaitAccounting, PROJECT_SLOT_CAP};
pub(crate) use barrier::publication_guard;
pub(crate) use barrier::{CheckoutCustody, CleanGuard, Cleaning};
#[cfg(test)]
pub(crate) use control_io::hold_record_for_tests;
pub(crate) use control_io::{
    atomic_write_private, flock_nonblocking, lock_admission, lock_existing_admission,
    open_control_directory, private_file, publish_writer_generation, statfs, writer_updated,
};
pub(crate) use recovery::Return;
pub(crate) use release::ReleaseDisposition;
pub(crate) use seeding::SeedRun;
pub(crate) use status::{
    active_holders, recorded_last_use, recorded_lost, Accounting, LastUse, Report, SlotView,
};
// The front door's status tests read recorded sizes and labels directly.
#[cfg(test)]
pub(crate) use status::{
    age_text, slot_label, ObservationSource, RecordedAge, SlotSize, INCOMPLETE_WALK,
};

use crate::config::{self, Backup, Facts, Found, Settings};
use crate::watchdog::Watchdog;
use oxide_storage::lease::managed::{ledger_view, prepare_pool, snapshot};
use oxide_storage::lease::{
    read_writer_state, LedgerView, PoolStatus, BUDGET_BACKEND_DIRECTORY_V1, MANAGED_NAME,
    VOLUME_BACKEND_APFS_V1,
};

use self::control_io::{
    check_writer_generation, create_pool, ensure_private_dir, free_bytes_for_creation, lease_error,
    lock_admission_inner, prepare_front_door_pool, read_control_state_if_present, read_state,
    verify_private_dir,
};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

pub const GIB: u64 = 1024 * 1024 * 1024;

/// Automatic creation refuses below this host free space.
pub const HOST_RESERVE_BYTES: u64 = 20 * GIB;

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

impl Store {
    /// Open the pool, creating it on first use above the host reserve. A
    /// macOS pool is excluded from Time Machine before any build writes into
    /// it; an unverified exclusion refuses before Cargo starts. An absent pool
    /// named by `OXIDE_POOL` refuses instead of being created silently unless
    /// `OXIDE_POOL_CREATE=1` or the path is inside the caller's task scratch
    /// root (oxide#182); the config file and the built-in default are
    /// unchanged.
    ///
    pub fn open_or_create() -> Result<Self, String> {
        let settings = settings()?;
        let path = settings.pool.clone();
        let source = settings.pool_source;
        // A pasted `oxide status` annotation is not part of a path, so it
        // refuses for every pool state: an absent path and an existing
        // ordinary directory both get the hint instead of the generic open
        // error (oxide#182).
        config::pool_annotation_refused(&path, source)?;
        let scratch = crate::scratch_io::evaluate(&settings, false).map_err(|error| {
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
                    crate::scratch_io::create(&settings, uuid, admission_budget(&settings))?;
                } else {
                    create_pool(&path, admission_budget(&settings))?;
                }
            }
            Found::LegacyImage => return Err(legacy_refusal()),
            Found::Directory | Found::Unknown => {}
        }
        if let Some(uuid) = &scratch {
            crate::scratch_io::repair(&settings, uuid)?;
        }
        let store = match Self::open_at(&path, &settings) {
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
        // A declared scratch root is judged before the pool is inspected, so
        // its refusal holds even for an absent pool. That judgement is reused
        // for the repair below instead of being evaluated a second time.
        let declared = if std::env::var_os("OXIDE_SCRATCH_ROOT").is_some() {
            Some(crate::scratch_io::evaluate(settings, false)?)
        } else {
            None
        };
        match config::inspect(&settings.pool) {
            Found::Absent => Ok(None),
            Found::LegacyImage => Err(legacy_refusal()),
            Found::Directory | Found::Unknown => {
                let scratch = match declared {
                    Some(scratch) => scratch,
                    None => crate::scratch_io::evaluate(settings, false)?,
                };
                if let Some(uuid) = scratch {
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

    /// The registered removable volume's free-space reserve, when this store is
    /// the removable pool that registration describes.
    ///
    /// The registration lives in the **internal** control directory, which is
    /// this removable store's owner control. Reading `control()` here found
    /// nothing on a removable pool (its own control holds no registration) and
    /// made an internal pool's fallback admission carry the removable volume's
    /// reserve (oxide#337).
    fn admission_reserve(&self) -> Option<u64> {
        if self.backend != Backend::ApfsVolume {
            return None;
        }
        crate::removable::read_registration(self.owner_control())
            .ok()
            .flatten()
            .map(|registration| registration.reserve_bytes)
    }

    /// The host-wide limit resolved for this command.
    pub fn max_active(&self) -> usize {
        self.max_active
    }

    /// Ledger membership with no accounting walk, for selection, affinity,
    /// waits and recovery.
    pub fn ledger(&self) -> Result<LedgerView, String> {
        ledger_view(&self.pool).map_err(|error| lease_error("cannot read the pool", &error))
    }

    /// The last flush of this pool, for the summary line.
    pub fn last_flush(&self) -> Option<Flush> {
        self.load().ok()?.flushes.pop()
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
        // A dangling control directory must not read as "no records", so the
        // helper inspects the directory itself before reading its state.
        read_control_state_if_present(&sibling.control)
            .map_err(|error| format!("the other pool's record state is unreadable: {error}"))
    }

    /// The number of independent active permits in the other pool. An unreadable
    /// state is an error, so the caller refuses rather than admitting over it.
    fn sibling_active(&self) -> Result<usize, String> {
        match (self.sibling.as_ref(), self.checked_sibling_state()?) {
            (Some(sibling), Some(state)) => {
                admission::host_permit_count(&sibling.control, &state.records)
            }
            _ => Ok(0),
        }
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

    fn forget_record(&self, state: &mut State, id: u64) {
        state.records.retain(|record| record.id != id);
        let _ = fs::remove_dir(self.affinity_dir(id));
        let _ = fs::remove_file(self.lock_path(id));
        let _ = fs::remove_file(admission::permit_path(&self.control, id));
        let _ = fs::remove_file(self.origin_path(id));
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

    fn ticket_blocked_path(&self, id: u64) -> PathBuf {
        self.control.join("waiters").join(format!("{id}.blocked"))
    }

    pub(super) fn is_ticket_blocked(&self, id: u64) -> Result<bool, String> {
        private_file(&self.ticket_blocked_path(id), false).map(|file| file.is_some())
    }

    pub(super) fn set_ticket_blocked(&self, id: u64, blocked: bool) -> Result<(), String> {
        let path = self.ticket_blocked_path(id);
        if blocked {
            private_file(&path, true)?;
            Ok(())
        } else {
            match fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(format!(
                    "cannot clear waiter state {}: {error}",
                    path.display()
                )),
                _ => Ok(()),
            }
        }
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
        atomic_write_private(&self.control, "state.json", &bytes)
            .map_err(|error| format!("cannot write Oxide state: {error}"))
    }
}
