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
    evict_idle_slot, flush_volume, init_budget_pool, lease_measured, ledger_view, measure_within,
    recover, release, snapshot, status as pool_status,
};
use oxide_storage::lease::{
    AccountingScan, LeaseError, LeaseRequest, LedgerSlot, LedgerView, PoolStatus,
    Profile as LeaseProfile, ReuseMeasurement, SlotLayout, SlotOccupancy,
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

pub const GIB: u64 = 1024 * 1024 * 1024;
/// Automatic creation refuses below this host free space.
pub const HOST_RESERVE_BYTES: u64 = 20 * GIB;
/// Slots one project identity may keep; a new checkout or a command with no
/// idle compatible slot replaces the least-recently-used eligible idle whole
/// slot of that project (oxide#118).
const PROJECT_SLOT_CAP: usize = 8;
const MAX_TICKETS: usize = 128;
const MAX_CLEANS: usize = 128;
const MAX_RECORDS: usize = 256;
/// Automatic recoveries kept for `oxide status`, newest last.
const MAX_RECOVERIES: usize = 16;
/// Finite control-state file size including the atomic temp.
pub(crate) const MAX_STATE_BYTES: u64 = 16 * 1024 * 1024;
/// Legacy control-state files keep their original read limit until the
/// permanent version-2 fence is actually written.
const LEGACY_MAX_STATE_BYTES: u64 = 256 * 1024;
/// Permanent control-state fence for revised Store mutations (oxide#118).
/// Version 5 carries the recorded per-slot accounting fence (oxide#146): the
/// library ledger's `capacity_policy: 2` refuses an older binary's strict
/// parse, so this control fence must be durable *before* the first recorded
/// ledger write, and a newer binary hands off before it reads that ledger.
/// Version 4 (the profile-member owned-outputs fence, oxide#133), version 3
/// (the whole-target registry transaction, oxide#129) and version 2
/// (oxide#128) remain readable until a forward fence upgrades them.
pub(crate) const STATE_VERSION: u32 = 5;
/// Version 4 is the owned-outputs fence this version supersedes.
const PREVIOUS_STATE_VERSION: u32 = 4;
/// Version 3 is the unfenced-shape successor of version 2: a pool with the
/// whole-target `oxide.outputs/2` fence keeps it so a state-3 binary can still
/// run.
const OLDER_STATE_VERSION: u32 = 3;
/// Version 2 (oxide#128) is read with the same strict field bounds.
const LEGACY_FIELDS_STATE_VERSION: u32 = 2;
const LEGACY_STATE_VERSION: u32 = 1;
/// The recorded-accounting fence value; see [`State::recorded_accounting`].
pub(crate) const RECORDED_ACCOUNTING_FENCE: &str = "oxide.accounting/1";

/// UTF-8 byte caps for control-state fields (oxide#118).
const MAX_IDENTITY_BYTES: usize = 64;
const MAX_COMPATIBILITY_BYTES: usize = 64;
const MAX_PROFILE_BYTES: usize = 16;
const MAX_PATH_TEXT_BYTES: usize = 1024;
const MAX_LEASE_HOLDER_BYTES: usize = 128;
/// `exec_label` can carry 160 four-byte characters plus its prefix and
/// ellipsis.
const MAX_COMMAND_BYTES: usize = 648;
const MAX_STATE_LEASE_ID_BYTES: usize = 160;
const MAX_STATE_SLOT_ID_BYTES: usize = 160;
const MAX_DIAGNOSTIC_BYTES: usize = 1024;

/// Compact serialized object ceilings, so the record-count bound and the
/// byte bound hold together: 256*40960 + 128*128 + 128*7168 + 16*9216 +
/// 16*8192 + 4096 = 11,702,272 bytes < MAX_STATE_BYTES.
const MAX_RECORD_OBJECT_BYTES: usize = 40 * 1024;
const MAX_TICKET_OBJECT_BYTES: usize = 128;
const MAX_CLEAN_OBJECT_BYTES: usize = 7 * 1024;
const MAX_RECOVERY_OBJECT_BYTES: usize = 9 * 1024;
const MAX_FLUSH_OBJECT_BYTES: usize = 8 * 1024;
const MAX_LOST_OBJECT_BYTES: usize = 8 * 1024;
const MAX_HOLDER_OBJECT_BYTES: usize = 5 * 1024;
const MAX_STATE_HEADER_BYTES: usize = 4 * 1024;
/// The owned-outputs fence value; see [`State::owned_outputs`].
pub(crate) const OWNED_OUTPUTS_FENCE: &str = "oxide.outputs/3";
/// The published whole-target fence: read unchanged (its legacy receipts keep
/// their place and key) and upgraded only by the explicit successor
/// transaction. Version 1 is the released original.
const LEGACY_OWNED_OUTPUTS_FENCE: &str = "oxide.outputs/2";
const RELEASED_OWNED_OUTPUTS_FENCE: &str = "oxide.outputs/1";

/// A fence this binary accepts as a legitimate predecessor of
/// [`OWNED_OUTPUTS_FENCE`]: the published whole-target `/2` fence and the
/// released `/1` fence it superseded. Every branch that meets a paired
/// removable control accepts exactly this set — plus an absent fence where the
/// control may be new — so a pool fenced by the released binary can never be
/// migrated by one branch and refused as unknown by another (oxide#159).
/// [`Store::load`] is the validation gate for every other value.
fn predecessor_owned_outputs_fence(fence: &str) -> bool {
    fence == LEGACY_OWNED_OUTPUTS_FENCE || fence == RELEASED_OWNED_OUTPUTS_FENCE
}

const MAX_INIT_TRANSACTION_BYTES: usize = 64;
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
    fn describe(&self, closed_by_boot: bool) -> String {
        match (&self.unclosed, closed_by_boot) {
            (None, _) => {
                "lost with the removable volume; deleted whole when the volume returns".to_owned()
            }
            (Some(why), false) => format!(
                "quarantined: lost with the removable volume and {why}; kept with its bytes, never reused or deleted"
            ),
            (Some(why), true) => format!(
                "lost with the removable volume and {why}; its holder predates this boot session, so it is deleted whole when the volume returns"
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
    /// Forward-only downgrade fence, set before any checkout's first owned
    /// output link is published and never cleared. An Oxide without owned
    /// outputs parses state strictly, so it refuses this pool rather than
    /// building through an owned link without its binding, custody and
    /// clean checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owned_outputs: Option<String>,
    /// Owned-outputs registry initialization (oxide#129). `Initializing` is
    /// durable before either control is fenced; `Ready` is published only
    /// after the registry and its directory are durable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outputs_init: Option<OutputsInit>,
    /// Forward-only recorded-accounting fence (oxide#146), set durably before
    /// any library write that can record a per-slot observation and never
    /// cleared. The library ledger's `capacity_policy: 2` is unreadable to an
    /// older binary, so this must be durable first: an older binary then hands
    /// off to a receipt-matching successor instead of refusing the pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recorded_accounting: Option<String>,
}

/// The durable initialization record shared by both pools of a pair.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum OutputsInit {
    Initializing {
        transaction: String,
        started_ms: u64,
    },
    Ready {
        transaction: String,
        ready_ms: u64,
    },
}

impl OutputsInit {
    fn transaction(&self) -> &str {
        match self {
            Self::Initializing { transaction, .. } | Self::Ready { transaction, .. } => transaction,
        }
    }
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
            owned_outputs: None,
            outputs_init: None,
            recorded_accounting: None,
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
    process::boot_ms()
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

/// How many non-blocking barrier attempts a guarded verb makes before its
/// bounded refusal (design §4.1 step 4). The production budget is
/// [`BARRIER_ATTEMPTS`] shipped polls; the unit suite uses two so a held
/// barrier reaches its bounded refusal quickly.
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
    /// The ledger's own closed observation for this idle slot generation
    /// (oxide#146). It is the admission basis; the oxide#128 sidecar fact above
    /// stays a separate diagnostic and the two are never summed.
    LedgerRecorded {
        allocated_bytes: u64,
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
                allocated_bytes,
                age,
                entries,
            } => {
                let age = match age {
                    RecordedAge::Ago(seconds) => {
                        format!("recorded {} ago at slot release", age_text(*seconds))
                    }
                    RecordedAge::ClockSkew => {
                        "recorded at a time ahead of this clock, so its age is unknown".to_owned()
                    }
                };
                format!(
                    "{} ({age} over {entries} entries; the admission basis, not free space); may have changed since measurement",
                    crate::cargo_home::size(*allocated_bytes)
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

/// A recorded observation's provenance. Only the reuse walk the library
/// already makes during an acquiring command is recorded: nothing fabricates
/// an admission measurement or adds a walk to obtain one.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObservationSource {
    LeaseReuse,
}

impl ObservationSource {
    fn label(self) -> &'static str {
        match self {
            Self::LeaseReuse => "measured by the reuse walk when leased",
        }
    }
}

/// The diagnostic sidecar's schema. It is not part of the pool, ledger,
/// registration or receipt formats and grants no accounting authority.
const OBSERVATIONS_SCHEMA: &str = "oxide.status-observations/1";
const OBSERVATIONS_NAME: &str = "status-observations.json";
const OBSERVATIONS_TEMP: &str = "status-observations.json.tmp";
/// The sidecar's whole-file bound and retained observation count. The count
/// is at least the pool's slot ceiling, so it never lowers real capacity.
const MAX_OBSERVATIONS_BYTES: u64 = 256 * 1024;
const MAX_OBSERVATIONS: usize = 128;

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
    /// when no idle slot has one. This is a recorded admission basis, not free
    /// space, and never includes an oxide#128 sidecar fact.
    pub recorded_total: Option<u64>,
    /// Idle slots whose ledger record is current.
    pub recorded_slots: usize,
    /// Idle slots with no current ledger record: they make `recorded_total` a
    /// stated lower bound.
    pub unrecorded_slots: usize,
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
    crate::prune::authority_read(file.take(MAX_STATE_BYTES + 1), &mut bytes)
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
    crate::prune::authority_read(file.take(MAX_STATE_BYTES + 1), &mut bytes)
        .map_err(|error| error.to_string())?;
    let state: State = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
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

/// [`active_holders`] without clean barriers, for a caller that itself holds
/// this checkout's clean barrier: `begin_clean` admits one barrier per
/// checkout across both pools, so any recorded barrier is the caller's own.
/// Command records are still reported.
pub fn active_records(control: &Path, checkout: &Path) -> Result<Vec<String>, String> {
    holders(control, checkout, false)
}

fn holders(control: &Path, checkout: &Path, cleans: bool) -> Result<Vec<String>, String> {
    let path = control.join("state.json");
    let Some(file) = private_file(&path, false)? else {
        return Ok(Vec::new());
    };
    let mut bytes = Vec::new();
    crate::prune::authority_read(file.take(MAX_STATE_BYTES + 1), &mut bytes)
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
    for clean in state.cleans.iter().filter(|_| cleans) {
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
    /// This slot's size for this view: a live walk, a recorded observation
    /// or an honest not-measured phrase.
    pub size: SlotSize,
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

    /// Open an existing internal pool for `oxide status` without its pool
    /// lock, reconciliation or any write (oxide#128). Refuses a retired,
    /// foreign or unsafe pool exactly as [`Store::open_existing_with`] does.
    pub fn open_recorded(settings: &Settings) -> Result<Option<Self>, String> {
        match config::inspect(&settings.pool) {
            Found::Absent => Ok(None),
            Found::LegacyImage => Err(legacy_refusal()),
            Found::Directory | Found::Unknown => {
                config::refuse_unsafe_pool_path(&settings.pool)?;
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
            if crate::budget::admission_expired() {
                waits.stop(Instant::now());
                self.remove_ticket(ticket.0)?;
                return Ok(Acquired::AdmissionExpired);
            }
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
            state.touch(index)?;
            self.write_origin(held.record, state.records[index].last_used, origin)?;
        }
        self.save(&state)
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
                eprintln!("oxide: deleted slot {} {reported}", slot.slot_id);
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
        let _internal_admission = internal.admission()?;
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
                    let _admission = sibling.admission()?;
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
                                state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
                                sibling.save(&state)?;
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
                            writers_closed(&state, "the paired", own_checkout)?;
                            state.outputs_init = Some(OutputsInit::Ready {
                                transaction: recorded.clone(),
                                ready_ms: process::now_ms(),
                            });
                            state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
                            sibling.save(&state)?;
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
                            writers_closed(&state, "the paired", own_checkout)?;
                            state.outputs_init = Some(OutputsInit::Ready {
                                transaction: recorded.clone(),
                                ready_ms: process::now_ms(),
                            });
                            state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
                            sibling.save(&state)?;
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
            sibling_admission = Some(sibling.admission()?);
            writers_closed(&sibling.load()?, "the paired", own_checkout)?;
        }
        writers_closed(&internal_state, "the internal", own_checkout)?;
        if !fresh && internal_state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
            internal_state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
            internal.save(&internal_state)?;
        }
        if fresh {
            internal_state.outputs_init = Some(OutputsInit::Initializing {
                transaction: transaction.clone(),
                started_ms: process::now_ms(),
            });
            internal_state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
            internal.save(&internal_state)?;
        }
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
                    sibling.save(&state)?;
                }
            }
            if state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
                return Err(
                    "the paired pool does not carry the successor output fence; refusing"
                        .to_owned(),
                );
            }
        }
        create(&transaction, true)?;
        if let Some(OutputsInit::Initializing { .. }) = internal_state.outputs_init {
            internal_state.outputs_init = Some(OutputsInit::Ready {
                transaction: transaction.clone(),
                ready_ms: process::now_ms(),
            });
            internal.save(&internal_state)?;
        }
        if let Some((sibling, _barrier)) = &sibling {
            let _admission = sibling.admission()?;
            let mut state = sibling.load()?;
            if !matches!(
                &state.outputs_init,
                Some(OutputsInit::Ready { transaction: ready, .. }) if ready == &transaction
            ) {
                state.outputs_init = Some(OutputsInit::Ready {
                    transaction: transaction.clone(),
                    ready_ms: process::now_ms(),
                });
                sibling.save(&state)?;
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
            sibling_admission = Some(sibling.admission()?);
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
        internal.save(&internal_state)?;
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
            sibling.save(&state)?;
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

    fn sibling_state(&self) -> Option<State> {
        let sibling = self.sibling.as_ref()?;
        let file = private_file(&sibling.control.join("state.json"), false).ok()??;
        let mut bytes = Vec::new();
        crate::prune::authority_read(file.take(MAX_STATE_BYTES + 1), &mut bytes).ok()?;
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
        let boot = boot_session();
        // Record IDs are per pool, so the caller's own ID is exempt only in
        // this store's state; an equal ID in the other pool is another command.
        for (pool, state, exempt) in [
            ("this", own_state, Some(own)),
            ("the other", other_state, None),
        ] {
            let Some(state) = state else { continue };
            for record in &state.records {
                if record.checkout.as_deref() != Some(checkout) || Some(record.id) == exempt {
                    continue;
                }
                if record.phase == Phase::Retired {
                    if let Some(lost) = record.lost.as_ref() {
                        // A restart closed a quarantined record whose holder
                        // predates this boot session; its slot is released and
                        // deleted whole at the next admission.
                        if let Some(why) = lost
                            .unclosed
                            .as_deref()
                            .filter(|_| !closed_by_boot(record, boot))
                        {
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
    /// `fence_recorded_accounting`, and it takes every lock it needs
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
        // The shipped internal-first order, tried without blocking: a held
        // admission lock (or a live internal clean) is a fact to retry, never a
        // wait while the caller holds the binding mutex. Both admissions are
        // held across the state read and the mark, exactly as the shipped
        // barrier holds them.
        let _sibling = match self.sibling.as_ref().filter(|sibling| sibling.barrier) {
            Some(sibling) => match try_lock_admission(&sibling.control)? {
                Some(lock) => Some(lock),
                None => {
                    return Ok(Cleaning::Retry(
                        "another oxide command holds the internal admission lock".to_owned(),
                    ))
                }
            },
            None => None,
        };
        let Some(_admission) = try_lock_admission(&self.control)? else {
            return Ok(Cleaning::Retry(
                "another oxide command holds this pool's admission lock".to_owned(),
            ));
        };
        // An unreadable state or a probe error is unknown custody, so it
        // refuses with the bounded guarded text instead of a raw error.
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
            match self.sibling_state() {
                Some(other) => {
                    self.barrier_records(&other, checkout, &sibling.control, &mut live, &mut dead)?
                }
                // No state file means no records; an unreadable one is
                // unknown custody, never "clear".
                None if !sibling.control.join("state.json").exists() => {}
                None => {
                    return Err(crate::one_use::barrier_unknown(
                        "the other pool's record state is unreadable",
                    ))
                }
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
                                allocated_bytes: fact.unique_allocated_bytes,
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
        // A project-cap replacement the library performs as part of the
        // admission below; choosing it never deletes anything itself.
        let mut replace: Option<(u64, String)> = None;
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
            // Replace the least-recently-used eligible idle whole slot of
            // this project by ledger rank. A record that is held, retired
            // or uncertain (a live or unattributable lock) is never a
            // candidate; foreign and unmeasurable slots fail the library's
            // own ownership and accounting checks.
            let mut candidates: Vec<(u64, u64, u64, String)> = state
                .records
                .iter()
                .filter(|record| {
                    record.phase == Phase::Idle
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
                .filter(|record| record.identity == key.identity && has_slot(record))
                .count();
            // A specific held compatible slot of any checkout of this
            // repository can become reusable under the existing identity and
            // freshness rules; only such a slot justifies waiting.
            let compatible_busy = state.records.iter().any(|record| {
                record.identity == key.identity
                    && record.compatibility == key.compatibility
                    && matches!(
                        record.phase,
                        Phase::Acquiring | Phase::Spawning | Phase::Running
                    )
            });
            if project_slots >= PROJECT_SLOT_CAP {
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
        let replace_slot =
            match &replace {
                Some((_, slot_id)) => Some(slot_id.parse::<u64>().map_err(|_| {
                    format!("recorded replacement slot ID {slot_id:?} is not numeric")
                })?),
                None => None,
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
        };
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
        let (grant, measured) = match outcome {
            Ok(leased) => {
                if let Some(note) = &leased.admission_note {
                    eprintln!("oxide: warning: {note}");
                }
                (leased.grant, leased.measured)
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
                        Ok(Err(Wait::Capacity))
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
        crate::prune::authority_read(file.take(MAX_ORIGIN_BYTES + 1), &mut bytes).ok()?;
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

    /// The mutating-command gate. Taking it also makes the recorded-accounting
    /// control fence durable, before this command can write a recorded library
    /// ledger (oxide#146): `STATE_VERSION` 5 is the hand-off trigger an older
    /// installed binary needs, because the ledger's `capacity_policy: 2` is
    /// unreadable to it. The write is one-time per pool; a read-only command
    /// never takes this gate and so never writes.
    fn admission(&self) -> Result<File, String> {
        let lock = lock_admission(&self.control)?;
        self.fence_recorded_accounting()?;
        Ok(lock)
    }

    /// Make the version-5 recorded-accounting fence durable. Nothing is
    /// written when it is already set.
    fn fence_recorded_accounting(&self) -> Result<(), String> {
        let mut state = self.load()?;
        if state.recorded_accounting.as_deref() == Some(RECORDED_ACCOUNTING_FENCE) {
            return Ok(());
        }
        state.recorded_accounting = Some(RECORDED_ACCOUNTING_FENCE.to_owned());
        self.save(&state)
    }

    fn load(&self) -> Result<State, String> {
        let Some(file) = private_file(&self.control.join("state.json"), false)? else {
            return Ok(State::default());
        };
        let mut bytes = Vec::new();
        crate::prune::authority_read(file.take(MAX_STATE_BYTES + 1), &mut bytes)
            .map_err(|error| format!("cannot read Oxide state: {error}"))?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            return Err("Oxide state exceeds its byte limit".to_owned());
        }
        let state: State = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Oxide state is invalid: {error}"))?;
        let legacy = match state.version {
            LEGACY_STATE_VERSION => true,
            LEGACY_FIELDS_STATE_VERSION
            | OLDER_STATE_VERSION
            | PREVIOUS_STATE_VERSION
            | STATE_VERSION => false,
            _ => return Err("Oxide state has an unsupported version or exceeds a limit".to_owned()),
        };
        if legacy && bytes.len() as u64 > LEGACY_MAX_STATE_BYTES {
            return Err("Oxide state exceeds its byte limit".to_owned());
        }
        if state.owned_outputs.as_deref().is_some_and(|fence| {
            fence != OWNED_OUTPUTS_FENCE && !predecessor_owned_outputs_fence(fence)
        }) {
            return Err("Oxide state has an unsupported version or exceeds a limit".to_owned());
        }
        validate_state(&state, legacy)?;
        Ok(state)
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
        let successor = fenced.owned_outputs.as_deref() == Some(OWNED_OUTPUTS_FENCE);
        let previous = fenced.outputs_init.is_some()
            || fenced.owned_outputs.as_deref() == Some(LEGACY_OWNED_OUTPUTS_FENCE);
        fenced.version = if recorded {
            // Version 5 is the recorded-accounting fence (oxide#146) and the
            // hand-off trigger that must precede the ledger's policy-2 write.
            STATE_VERSION
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
            .map_err(|error| format!("cannot replace Oxide state: {error}"))
    }
}

/// Validate one control-state document. A legacy version-1 document keeps
/// the legacy read limits (its byte cap is checked by `load`); a version-2
/// document must satisfy every field cap and compact-object ceiling, so the
/// record-count and byte bounds hold together.
fn validate_state(state: &State, legacy: bool) -> Result<(), String> {
    if !matches!(
        state.version,
        LEGACY_STATE_VERSION
            | LEGACY_FIELDS_STATE_VERSION
            | OLDER_STATE_VERSION
            | PREVIOUS_STATE_VERSION
            | STATE_VERSION
    ) {
        return Err("Oxide state has an unsupported version".to_owned());
    }
    // A document at or above the recorded-accounting version must carry its
    // fence; a fence value this binary does not know refuses rather than being
    // guessed at.
    match state.recorded_accounting.as_deref() {
        None => {
            if state.version >= STATE_VERSION {
                return Err("Oxide state has an unsupported version or exceeds a limit".to_owned());
            }
        }
        Some(fence) if fence == RECORDED_ACCOUNTING_FENCE => {}
        Some(_) => {
            return Err("Oxide state has an unsupported version or exceeds a limit".to_owned())
        }
    }
    if state
        .outputs_init
        .as_ref()
        .is_some_and(|init| !bounded_text(init.transaction(), MAX_INIT_TRANSACTION_BYTES))
    {
        return Err("Oxide state has an invalid output-registry transaction".to_owned());
    }
    if state.tickets.len() > MAX_TICKETS
        || state.cleans.len() > MAX_CLEANS
        || state.records.len() > MAX_RECORDS
        || state.recoveries.len() > MAX_RECOVERIES
        || state.flushes.len() > MAX_FLUSHES
    {
        return Err("Oxide state exceeds a collection limit".to_owned());
    }
    if legacy {
        return Ok(());
    }
    if state.next_id == 0 || state.next_use == 0 {
        return Err("Oxide state has an exhausted or invalid ID sequence".to_owned());
    }
    let header = State {
        tickets: Vec::new(),
        cleans: Vec::new(),
        records: Vec::new(),
        recoveries: Vec::new(),
        flushes: Vec::new(),
        outputs_init: None,
        ..state.clone()
    };
    object_fits(&header, MAX_STATE_HEADER_BYTES, "state header")?;
    for ticket in &state.tickets {
        if ticket.id == 0 {
            return Err("Oxide wait ticket has no ID".to_owned());
        }
        object_fits(ticket, MAX_TICKET_OBJECT_BYTES, "wait ticket")?;
    }
    for clean in &state.cleans {
        if clean.id == 0 || !path_text(&clean.checkout) {
            return Err("Oxide clean mark exceeds its field bounds".to_owned());
        }
        object_fits(clean, MAX_CLEAN_OBJECT_BYTES, "clean mark")?;
    }
    for record in &state.records {
        if record.id == 0
            || !bounded_text(&record.identity, MAX_IDENTITY_BYTES)
            || !bounded_text(&record.compatibility, MAX_COMPATIBILITY_BYTES)
            || !bounded_text(&record.profile, MAX_PROFILE_BYTES)
            || record
                .checkout
                .as_ref()
                .is_some_and(|path| !path_text(path))
            || record
                .lease_id
                .as_ref()
                .is_some_and(|value| value.len() > MAX_STATE_LEASE_ID_BYTES)
            || record
                .retired
                .as_ref()
                .is_some_and(|value| value.len() > MAX_DIAGNOSTIC_BYTES)
        {
            return Err("Oxide record exceeds its field bounds".to_owned());
        }
        if let Some(holder) = &record.holder {
            if !bounded_text(&holder.lease_holder, MAX_LEASE_HOLDER_BYTES)
                || holder
                    .command
                    .as_ref()
                    .is_some_and(|value| value.len() > MAX_COMMAND_BYTES)
            {
                return Err("Oxide holder exceeds its field bounds".to_owned());
            }
            object_fits(holder, MAX_HOLDER_OBJECT_BYTES, "holder")?;
        }
        if let Some(lost) = &record.lost {
            if !bounded_text(&lost.slot_id, MAX_STATE_SLOT_ID_BYTES)
                || lost
                    .unclosed
                    .as_ref()
                    .is_some_and(|value| value.len() > MAX_DIAGNOSTIC_BYTES)
            {
                return Err("Oxide lost slot exceeds its field bounds".to_owned());
            }
            object_fits(lost, MAX_LOST_OBJECT_BYTES, "lost slot")?;
        }
        if let Some(recovery) = &record.pending_recovery {
            validate_recovery(recovery)?;
        }
        object_fits(record, MAX_RECORD_OBJECT_BYTES, "record")?;
    }
    for recovery in &state.recoveries {
        validate_recovery(recovery)?;
    }
    for flush in &state.flushes {
        if !bounded_text(&flush.slot_id, MAX_STATE_SLOT_ID_BYTES)
            || flush
                .error
                .as_ref()
                .is_some_and(|value| value.len() > MAX_DIAGNOSTIC_BYTES)
        {
            return Err("Oxide flush record exceeds its field bounds".to_owned());
        }
        object_fits(flush, MAX_FLUSH_OBJECT_BYTES, "flush record")?;
    }
    Ok(())
}

fn validate_recovery(recovery: &Recovery) -> Result<(), String> {
    if !bounded_text(&recovery.slot_id, MAX_STATE_SLOT_ID_BYTES)
        || !bounded_text(&recovery.lease_id, MAX_STATE_LEASE_ID_BYTES)
        || recovery
            .checkout
            .as_ref()
            .is_some_and(|path| !path_text(path))
    {
        return Err("Oxide recovery exceeds its field bounds".to_owned());
    }
    object_fits(recovery, MAX_RECOVERY_OBJECT_BYTES, "recovery")
}

fn bounded_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max
}

fn path_text(path: &Path) -> bool {
    path.as_os_str().as_bytes().len() <= MAX_PATH_TEXT_BYTES
}

fn object_fits<T: Serialize>(value: &T, max: usize, what: &str) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if bytes.len() > max {
        return Err(format!("Oxide {what} exceeds its {max}-byte object limit"));
    }
    Ok(())
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

/// Take the internal pool's existing admission lock for metadata updates and
/// command admission. Only the lock file may be created; the control
/// directory must already exist.
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
fn open_admission_file(control: &Path) -> Result<File, String> {
    let directory = open_control_directory(control)?
        .ok_or_else(|| format!("{} does not exist", control.display()))?;
    let name = CString::new("admission.lock").map_err(|error| error.to_string())?;
    // Create exclusively, then open the existing lock without `O_CREAT`.
    // Repeating `O_CREAT | O_NOFOLLOW` for every contender races with the
    // creator on this platform and can report a spurious `ENOENT`, refusing
    // a valid admission. Both steps keep `O_NOFOLLOW`, so a symlink is never
    // followed, and creation is exclusive so no second file is ever written.
    let mut fd = -1;
    for _ in 0..3 {
        // SAFETY: `name` is a live CString with initialized bytes through its
        // terminator, and `directory` owns a live descriptor until `openat`
        // ends.
        fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
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
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound {
            break;
        }
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
    Ok(file)
}

pub(crate) fn lock_admission(control: &Path) -> Result<File, String> {
    let file = open_admission_file(control)?;
    crate::budget::lock(&file, "admission")?;
    Ok(file)
}

/// A non-blocking admission attempt (one-use S1, design §4.1 step 4): `Some`
/// once the lock is taken, `None` when another command holds it. A guarded
/// verb never blocks on this lock while it holds the checkout binding mutex.
pub(crate) fn try_lock_admission(control: &Path) -> Result<Option<File>, String> {
    let file = open_admission_file(control)?;
    // SAFETY: the descriptor is open for the duration of the call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
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
    let Some(file) = private_file(&path, false)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    crate::prune::authority_read(file.take(MAX_OBSERVATIONS_BYTES + 1), &mut bytes)
        .map_err(|error| format!("cannot read recorded observations: {error}"))?;
    if bytes.len() as u64 > MAX_OBSERVATIONS_BYTES {
        return Err("recorded observations exceed their byte limit".to_owned());
    }
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

/// The existing blocking exclusive `flock`, unchanged for unbudgeted
/// commands.
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
            retired: lost.as_ref().map(|lost| lost.describe(false)),
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
            let bytes = fs::read(control.join("state.json")).unwrap();
            let refused = serde_json::from_slice::<PreFeatureState>(&bytes)
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
        assert_eq!(state.version, STATE_VERSION);
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
                    STATE_VERSION
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
        // Both fixtures already carry the recorded-accounting fence, because a
        // control's own admission publishes it before this path reads either
        // state; that durability step is not part of what a refused migration
        // may change.
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
            error.contains("unsupported version"),
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

    /// A holder of a checkout's clean barrier excludes only barriers when it
    /// rechecks writers: command records are still reported.
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
        assert!(active_records(&internal.control, checkout)
            .unwrap()
            .is_empty());
        let mut running = record(Phase::Running, Some(i32::MAX), Some("v1.a.1.1.1"));
        running.checkout = Some(checkout.to_path_buf());
        state.records.push(running);
        internal.save(&state).unwrap();
        assert_eq!(
            active_records(&internal.control, checkout).unwrap().len(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// The control-state bytes of one pool, for a refusal that must change
    /// nothing (one-use S1: `next_id` included).
    fn state_bytes(store: &Store) -> Vec<u8> {
        fs::read(store.control.join("state.json")).unwrap()
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

    /// An unreadable *sibling* state is unknown custody, not "no records": a
    /// symlinked document cannot be read (`O_NOFOLLOW`), so the guarded barrier
    /// refuses before any mark instead of treating the other pool as clear.
    #[test]
    fn the_guarded_barrier_refuses_an_unreadable_sibling_state() {
        let (root, internal, removable) = paired_controls("guarded-sibling-unknown");
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
        // Non-vacuity: with the document absent the same checkout is clear.
        fs::remove_file(&state).unwrap();
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

    /// Unreadable state is unknown custody: the guarded barrier refuses and
    /// leaves the bytes exactly as they were.
    #[test]
    fn the_guarded_barrier_refuses_unknown_custody() {
        let (root, internal, _removable) = paired_controls("guarded-unknown");
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

    /// The guarded order (design §4.1 step 3): the caller's revalidation runs
    /// before any mark, and its refusal is returned unchanged with nothing
    /// allocated. The shipped `Recovering` mode never consults it.
    #[test]
    fn the_guarded_barrier_revalidates_before_any_mark() {
        let (root, internal, _removable) = paired_controls("guarded-revalidate");
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
            .attempt(ticket, key, Profile::Quick, 16, false, label)
            .unwrap();
        store.remove_ticket(ticket).unwrap();
        outcome
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
        }
    }

    #[test]
    fn worst_escaped_control_state_roundtrips_through_save_and_load() {
        let (root, store, _removable) = paired_controls("control-escaped");
        // The current representation carries both forward fences, so this is
        // the worst valid version-5 document and not an owned-outputs-only one.
        let mut state = State {
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
        assert_eq!(loaded.version, STATE_VERSION);
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
        assert_eq!(stored["version"], serde_json::json!(STATE_VERSION));
        assert_eq!(
            stored["recorded_accounting"],
            serde_json::json!(RECORDED_ACCOUNTING_FENCE)
        );
        store
            .save(&State::default())
            .expect("an unfenced state saves");

        // The *path* truth: `fence_owned_outputs` is a mutating command, so it
        // takes the admission gate, which makes the recorded-accounting fence
        // durable first. That is the ordering the design requires — the
        // control hand-off must precede any incompatible ledger write — and it
        // is why this path ends at version 5 with both fences.
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
        // A repeated recorded fence is idempotent.
        let fenced = stored;
        store
            .fence_recorded_accounting()
            .expect("a repeated fence is a no-op");
        store
            .fence_recorded_accounting()
            .expect("a repeated fence is a no-op");
        let after: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(after, fenced, "a repeated fence rewrote the state");
        // Version 5 without the fence, and an unknown fence, both refuse.
        let mut unfenced = fenced.clone();
        unfenced["recorded_accounting"] = serde_json::Value::Null;
        write_private(&path, &serde_json::to_vec(&unfenced).unwrap());
        let error = store.load().unwrap_err();
        assert!(error.contains("unsupported version"), "{error}");
        let mut unknown = fenced.clone();
        unknown["recorded_accounting"] = serde_json::json!("oxide.accounting/9");
        write_private(&path, &serde_json::to_vec(&unknown).unwrap());
        let error = store.load().unwrap_err();
        assert!(error.contains("unsupported version"), "{error}");
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

    /// Retention keeps at most 128 observations, deterministically choosing
    /// the most recent, so a pool at its slot ceiling still fits.
    #[test]
    fn observation_retention_keeps_the_newest_deterministically() {
        let mut observations: Vec<Observation> = (0..140)
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
        assert_eq!(
            observations.last().map(|item| item.slot_id.as_str()),
            Some("139")
        );
        let mut again: Vec<Observation> = (0..140)
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

        // The worst case the file allows still fits its byte bound: 128
        // observations with every bounded field at its ceiling.
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
}
