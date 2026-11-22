//! The front door's control-state schema and the one parse-and-validate
//! function both its writer (`src/bin/oxide/store.rs`) and the library's
//! fence-first pool open use (oxide#177, Oxide Supervisor ruling f09441af).
//!
//! One schema, one validator: a document the fence accepts is exactly a
//! document the front door's typed load accepts, so the library can never
//! upgrade corrupt control state before reconciliation.

use serde::{Deserialize, Serialize};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_TICKETS: usize = 128;
pub const MAX_CLEANS: usize = 128;
/// Control records: one per slot with the previous 2× headroom over the slot
/// ceiling, so a full 256-slot pool plus slotless and replaced records stays
/// representable (oxide#177).
pub const MAX_RECORDS: usize = 512;
/// Automatic recoveries kept for `oxide status`, newest last.
pub const MAX_RECOVERIES: usize = 16;

pub const MAX_STATE_BYTES: u64 = 32 * 1024 * 1024;
/// Legacy control-state files keep their original read limit until the
/// permanent version-2 fence is actually written.
pub const LEGACY_MAX_STATE_BYTES: u64 = 256 * 1024;

/// Version 8 carries the raised-ceiling fence (oxide#177): the library
/// ledger's `capacity_policy: 4` refuses an older binary, so this control
/// fence must be durable **before** the first locked pool open or ledger
/// write that can record more than the legacy 128 slots or recover a
/// multi-journal state. It is written by the admission gate on the first
/// mutating command.
/// Version 7 fences eviction accounting (oxide#175) before an Evicting record
/// retains its closed size.
/// Version 6 carries the one-use fence. The feature is retired, so nothing
/// writes it any more; a document that already carries `oxide.one-use/1`
/// stays readable as an inert value.
/// Version 5 carries the recorded per-slot accounting fence (oxide#146): the
/// library ledger's `capacity_policy: 2` refuses an older binary's strict
/// parse, so this control fence must be durable *before* the first recorded
/// ledger write, and a newer binary hands off before it reads that ledger.
/// Version 4 (the profile-member owned-outputs fence, oxide#133), version 3
/// (the whole-target registry transaction, oxide#129) and version 2
/// (oxide#128) remain readable until a forward fence upgrades them.
pub const STATE_VERSION: u32 = super::CONTROL_FENCE_VERSION;
/// Version 9 is the whole-tree holding fence (oxide#190): a slot tree is
/// renamed whole into the payload's private holding area under the pool lock
/// and deleted outside it, so a ledger can carry a `holding` row instead of a
/// per-entry deletion journal.
pub const HOLDING_STATE_VERSION: u32 = 9;
/// First schema that records which generation may write.
pub const WRITER_STATE_VERSION: u32 = 10;
/// Advance when an installed successor must exclude earlier writers.
pub const WRITER_GENERATION: u32 = 1;
/// Version 7 is the eviction-accounting fence (oxide#175).
pub const EVICTION_ACCOUNTING_STATE_VERSION: u32 = 7;
/// Version 8 is the raised-ceiling fence (oxide#177).
pub const SLOT_CEILING_STATE_VERSION: u32 = 8;
/// Version 6 carried the one-use fence. A later document may also carry it,
/// so presence requires at least this version and version 6 requires the fence
/// itself; nothing writes the fence any more.
pub const ONE_USE_STATE_VERSION: u32 = 6;
/// Version 5 is the recorded-accounting fence (oxide#146), still written for
/// a pool that has not opted into one-use workspaces or the raised ceiling.
pub const RECORDED_ACCOUNTING_STATE_VERSION: u32 = 5;
/// Version 4 is the owned-outputs fence this version supersedes.
pub const PREVIOUS_STATE_VERSION: u32 = 4;
/// Version 3 is the unfenced-shape successor of version 2: a pool with the
/// whole-target `oxide.outputs/2` fence keeps it so a state-3 binary can still
/// run.
pub const OLDER_STATE_VERSION: u32 = 3;
/// Version 2 (oxide#128) is read with the same strict field bounds.
pub const LEGACY_FIELDS_STATE_VERSION: u32 = 2;
pub const LEGACY_STATE_VERSION: u32 = 1;

/// The recorded-accounting fence value; see [`State::recorded_accounting`].
pub const RECORDED_ACCOUNTING_FENCE: &str = super::CONTROL_RECORDED_ACCOUNTING_FENCE;
/// The one-use fence value; see [`State::one_use_workspaces`]. The feature is
/// retired: nothing writes the fence any more, and an existing document that
/// carries it stays readable as an inert value.
pub const ONE_USE_FENCE: &str = super::CONTROL_ONE_USE_FENCE;
/// The eviction-accounting fence value (oxide#175); see
/// [`State::eviction_accounting`]. A document carrying it is at version 7 or
/// later, and a version-7 or version-8 document always carries it.
pub const EVICTION_ACCOUNTING_FENCE: &str = super::CONTROL_EVICTION_ACCOUNTING_FENCE;
/// The raised-ceiling fence value (oxide#177); see
/// [`State::slot_ceiling`]. A document carrying it is at version 8 or later,
/// and version 8 requires it. The library's fence writer and this store use
/// one value.
pub const SLOT_CEILING_FENCE: &str = super::CONTROL_SLOT_CEILING_FENCE;
/// The whole-tree holding fence value (oxide#190); see [`State::holding`]. A
/// document carrying it is at version 9 or later, and those versions require it.
pub const HOLDING_FENCE: &str = super::CONTROL_HOLDING_FENCE;

/// UTF-8 byte caps for control-state fields (oxide#118).
pub const MAX_IDENTITY_BYTES: usize = 64;
pub const MAX_COMPATIBILITY_BYTES: usize = 64;
pub const MAX_PROFILE_BYTES: usize = 16;
pub const MAX_PATH_TEXT_BYTES: usize = 1024;
pub const MAX_LEASE_HOLDER_BYTES: usize = 128;
/// `exec_label` can carry 160 four-byte characters plus its prefix and
/// ellipsis.
pub const MAX_COMMAND_BYTES: usize = 648;
pub const MAX_STATE_LEASE_ID_BYTES: usize = 160;
pub const MAX_STATE_SLOT_ID_BYTES: usize = 160;
pub const MAX_DIAGNOSTIC_BYTES: usize = 1024;

/// Compact serialized object ceilings, so the record-count bound and the
/// byte bound hold together: 512*40960 + 128*128 + 128*7168 + 16*9216 +
/// 16*8192 + 4096 = 22,188,032 bytes < MAX_STATE_BYTES (oxide#177).
pub const MAX_RECORD_OBJECT_BYTES: usize = 40 * 1024;
pub const MAX_TICKET_OBJECT_BYTES: usize = 128;
pub const MAX_CLEAN_OBJECT_BYTES: usize = 7 * 1024;
pub const MAX_RECOVERY_OBJECT_BYTES: usize = 9 * 1024;
pub const MAX_FLUSH_OBJECT_BYTES: usize = 8 * 1024;
pub const MAX_LOST_OBJECT_BYTES: usize = 8 * 1024;
pub const MAX_HOLDER_OBJECT_BYTES: usize = 5 * 1024;
pub const MAX_STATE_HEADER_BYTES: usize = 4 * 1024;

/// The owned-outputs fence value; see [`State::owned_outputs`].
pub const OWNED_OUTPUTS_FENCE: &str = "oxide.outputs/3";
/// The published whole-target fence: read unchanged (its legacy receipts keep
/// their place and key) and upgraded only by the explicit successor
/// transaction. Version 1 is the released original.
pub const LEGACY_OWNED_OUTPUTS_FENCE: &str = "oxide.outputs/2";
pub const RELEASED_OWNED_OUTPUTS_FENCE: &str = "oxide.outputs/1";

/// A fence this binary accepts as a legitimate predecessor of
/// [`OWNED_OUTPUTS_FENCE`]: the published whole-target `/2` fence and the
/// released `/1` fence it superseded. Every branch that meets a paired
/// removable control accepts exactly this set — plus an absent fence where the
/// control may be new — so a pool fenced by the released binary can never be
/// migrated by one branch and refused as unknown by another (oxide#159).
/// [`Store::load`] is the validation gate for every other value.
pub fn predecessor_owned_outputs_fence(fence: &str) -> bool {
    fence == LEGACY_OWNED_OUTPUTS_FENCE || fence == RELEASED_OWNED_OUTPUTS_FENCE
}

pub const MAX_INIT_TRANSACTION_BYTES: usize = 64;

/// Removable-volume flushes kept for `oxide status`, newest last.
pub const MAX_FLUSHES: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
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
pub struct Record {
    pub id: u64,
    pub identity: String,
    pub compatibility: String,
    pub profile: String,
    /// `None` once `oxide clean` dropped this checkout's affinity.
    pub checkout: Option<PathBuf>,
    pub phase: Phase,
    pub process_group: Option<i32>,
    pub last_used: u64,
    /// Holder of the current command; set while the record is not idle.
    #[serde(default)]
    pub holder: Option<Holder>,
    /// Lease ID once the library granted it.
    #[serde(default)]
    pub lease_id: Option<String>,
    /// Written before the ledger recovery, so the event survives a crash
    /// between that recovery and moving it into the log.
    #[serde(default)]
    pub pending_recovery: Option<Recovery>,
    /// Why a [`Phase::Retired`] slot was retired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired: Option<String>,
    /// Set when the slot was retired because its removable volume vanished
    /// while the record held it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost: Option<Lost>,
    /// Wall clock of the latest use, milliseconds since the epoch, written
    /// with `last_used`. `last_used` orders uses within one pool only; this
    /// lets `status` compare pools. Absent on records last used before it
    /// existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_ms: Option<u64>,
}

impl Record {
    /// Holds a command now: neither idle nor retired.
    pub fn active(&self) -> bool {
        !matches!(self.phase, Phase::Idle | Phase::Retired)
    }
}

/// A slot held when its removable volume vanished (surprise removal). It is
/// never reused. Once the volume returns it is deleted whole only if every
/// writer is positively closed; otherwise it is quarantined: held, bytes
/// counted, never reused or deleted.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lost {
    pub slot_id: String,
    pub at_ms: u64,
    /// Why closure of every writer is not established; `None` when the
    /// holding command saw its own group empty and started nothing Oxide
    /// knows may outlive it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unclosed: Option<String>,
}

impl Lost {
    pub fn describe(&self, closed_by_boot: bool) -> String {
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
pub struct Holder {
    pub pid: u32,
    /// Wall-clock start of the holder process, milliseconds since the epoch.
    pub started_ms: u64,
    pub lease_holder: String,
    /// What `oxide status` names as the holder: `exec <program>` for an
    /// exec command, absent for Cargo commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
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
pub struct Ticket {
    pub id: u64,
    pub pid: u32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub blocked: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// A running `oxide clean`; commands for the same checkout wait while it
/// exists. Its lock (in `waiters/`) is shared with the `cargo clean` child, so
/// it stays held while either lives. The mark is dropped only when the lock is
/// free and the recorded Cargo group, if any, is empty.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clean {
    pub id: u64,
    pub checkout: PathBuf,
    #[serde(default)]
    pub process_group: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub version: u32,
    /// Minimum cooperative writer generation. Passive readers ignore it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "read_writer_generation"
    )]
    pub minimum_writer_generation: Option<u32>,
    pub next_id: u64,
    pub next_use: u64,
    pub tickets: Vec<Ticket>,
    #[serde(default)]
    pub cleans: Vec<Clean>,
    pub records: Vec<Record>,
    #[serde(default)]
    pub recoveries: Vec<Recovery>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flushes: Vec<Flush>,
    /// Forward-only downgrade fence, set before any checkout's first owned
    /// output link is published and never cleared. An Oxide without owned
    /// outputs parses state strictly, so it refuses this pool rather than
    /// building through an owned link without its binding, custody and
    /// clean checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owned_outputs: Option<String>,
    /// Owned-outputs registry initialization (oxide#129). `Initializing` is
    /// durable before either control is fenced; `Ready` is published only
    /// after the registry and its directory are durable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs_init: Option<OutputsInit>,
    /// Forward-only recorded-accounting fence (oxide#146), set durably before
    /// any library write that can record a per-slot observation and never
    /// cleared. The library ledger's `capacity_policy: 2` is unreadable to an
    /// older binary, so this must be durable first: an older binary then hands
    /// off to a receipt-matching successor instead of refusing the pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_accounting: Option<String>,
    /// The retired one-use fence. Nothing writes it any more; the field stays
    /// only so an existing document that carries it keeps parsing, and a
    /// version-6 document still needs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub one_use_workspaces: Option<String>,
    /// Forward-only eviction-accounting fence (oxide#175), set durably
    /// before the first ledger write that retains a closed size across an
    /// eviction and never cleared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eviction_accounting: Option<String>,
    /// Forward-only raised-ceiling fence (oxide#177), set durably by the
    /// admission gate **before** the first locked pool open or library write
    /// that can record more than the legacy 128 slots and never cleared. The
    /// library ledger's `capacity_policy: 4` is unreadable to an older binary,
    /// so an older binary hands off to a receipt-matching successor instead of
    /// refusing a pool this one just raised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_ceiling: Option<String>,
    /// Forward-only whole-tree holding fence (oxide#190), set durably by the
    /// admission gate before the first ledger write that records a holding row
    /// and never cleared. An older binary hands off at version 9 instead of
    /// reading a pool whose eviction route it does not know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holding: Option<String>,
}

fn read_writer_generation<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u32>, D::Error> {
    u32::deserialize(deserializer).map(Some)
}

/// The durable initialization record shared by both pools of a pair.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum OutputsInit {
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
    pub fn transaction(&self) -> &str {
        match self {
            Self::Initializing { transaction, .. } | Self::Ready { transaction, .. } => transaction,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            // A pool with no state document and no fence is the unfenced
            // shape; `save` derives the version that document actually
            // carries, so this value is never a claim about a fence.
            version: LEGACY_FIELDS_STATE_VERSION,
            minimum_writer_generation: None,
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
            one_use_workspaces: None,
            eviction_accounting: None,
            slot_ceiling: None,
            holding: None,
        }
    }
}

impl State {
    pub fn check_writer(&self, generation: u32) -> Result<(), super::LeaseError> {
        if self.minimum_writer_generation.unwrap_or(0) > generation {
            return Err(super::LeaseError::WriterUpdated);
        }
        Ok(())
    }

    pub fn take_id(&mut self) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or("Oxide ID sequence exhausted")?;
        Ok(id)
    }

    pub fn touch(&mut self, index: usize) -> Result<(), String> {
        let use_id = self.next_use;
        self.next_use = use_id
            .checked_add(1)
            .ok_or("Oxide use sequence exhausted")?;
        self.records[index].last_used = use_id;
        self.records[index].last_used_ms = Some(now_ms());
        Ok(())
    }

    pub fn settle_idle(&mut self, index: usize) {
        let record = &mut self.records[index];
        record.phase = Phase::Idle;
        record.process_group = None;
        record.holder = None;
        record.lease_id = None;
        record.pending_recovery = None;
    }

    pub fn log_flush(&mut self, flush: Flush) {
        if self.flushes.len() >= MAX_FLUSHES {
            self.flushes.remove(0);
        }
        self.flushes.push(flush);
    }

    pub fn log_recovery(&mut self, recovery: Recovery) {
        if self.recoveries.len() >= MAX_RECOVERIES {
            self.recoveries.remove(0);
        }
        self.recoveries.push(recovery);
    }
}

/// Validate one control-state document. A legacy version-1 document keeps
/// the legacy read limits (its byte cap is checked by `load`); a version-2
/// document must satisfy every field cap and compact-object ceiling, so the
/// record-count and byte bounds hold together.
pub fn validate_state(state: &State, legacy: bool) -> Result<(), String> {
    if !matches!(
        state.version,
        LEGACY_STATE_VERSION
            | LEGACY_FIELDS_STATE_VERSION
            | OLDER_STATE_VERSION
            | PREVIOUS_STATE_VERSION
            | RECORDED_ACCOUNTING_STATE_VERSION
            | ONE_USE_STATE_VERSION
            | EVICTION_ACCOUNTING_STATE_VERSION
            | SLOT_CEILING_STATE_VERSION
            | HOLDING_STATE_VERSION
            | WRITER_STATE_VERSION
    ) {
        return Err("Oxide state has an unsupported version".to_owned());
    }
    match (state.version, state.minimum_writer_generation) {
        (WRITER_STATE_VERSION, Some(1..)) => {}
        (WRITER_STATE_VERSION, _) | (_, Some(_)) => {
            return Err("Oxide state has an invalid writer generation".to_owned());
        }
        _ => {}
    }
    // A document at or above the recorded-accounting version must carry its
    // fence; a fence value this binary does not know refuses rather than being
    // guessed at.
    match state.recorded_accounting.as_deref() {
        None if state.version >= RECORDED_ACCOUNTING_STATE_VERSION => {
            return Err("Oxide state lacks the recorded_accounting fence".to_owned());
        }
        None => {}
        Some(fence) if fence == RECORDED_ACCOUNTING_FENCE => {}
        Some(_) => return Err("Oxide state has an invalid recorded_accounting fence".to_owned()),
    }
    // The eviction-accounting fence and version 7 are equivalent (oxide#175):
    // a version-7 or version-8 document always carries it, a smaller one never
    // does, and an unknown value refuses.
    match state.eviction_accounting.as_deref() {
        None if state.version >= EVICTION_ACCOUNTING_STATE_VERSION => {
            return Err("Oxide state lacks the eviction accounting fence".to_owned())
        }
        Some(fence) if fence == EVICTION_ACCOUNTING_FENCE => {
            if state.version < EVICTION_ACCOUNTING_STATE_VERSION {
                return Err("Oxide state has an invalid eviction accounting fence".to_owned());
            }
        }
        None => {}
        _ => return Err("Oxide state has an invalid eviction accounting fence".to_owned()),
    }
    // The one-use fence and its numeric version are equivalent at version 6
    // and stay readable as an inert value: the retired one-use feature never
    // wrote one in a shipped build, so an existing document may carry it and
    // nothing writes it any more. An unknown fence value still refuses instead
    // of being guessed at.
    match state.one_use_workspaces.as_deref() {
        None => {
            if state.version == ONE_USE_STATE_VERSION {
                return Err("Oxide state lacks the one_use_workspaces fence".to_owned());
            }
        }
        Some(fence) if fence == ONE_USE_FENCE => {
            if state.version < ONE_USE_STATE_VERSION {
                return Err("Oxide state has one_use_workspaces before version 6".to_owned());
            }
        }
        Some(_) => return Err("Oxide state has an invalid one_use_workspaces fence".to_owned()),
    }
    // The raised-ceiling fence and version 8 are equivalent (oxide#177): a
    // version-8 document always carries it, and a smaller pool never does. An
    // unknown fence value refuses rather than being guessed at.
    match state.slot_ceiling.as_deref() {
        None if state.version >= SLOT_CEILING_STATE_VERSION => {
            return Err("Oxide state lacks the slot_ceiling fence".to_owned());
        }
        None => {}
        Some(SLOT_CEILING_FENCE) if state.version >= SLOT_CEILING_STATE_VERSION => {}
        Some(_) => return Err("Oxide state has an invalid slot_ceiling fence".to_owned()),
    }
    // The whole-tree holding fence is required from version 9 onwards.
    match state.holding.as_deref() {
        None if state.version >= HOLDING_STATE_VERSION => {
            return Err("Oxide state lacks the holding fence".to_owned());
        }
        None => {}
        Some(HOLDING_FENCE) if state.version >= HOLDING_STATE_VERSION => {}
        Some(_) => return Err("Oxide state has an invalid holding fence".to_owned()),
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

pub fn validate_recovery(recovery: &Recovery) -> Result<(), String> {
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

pub fn bounded_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max
}

pub fn path_text(path: &Path) -> bool {
    path.as_os_str().as_bytes().len() <= MAX_PATH_TEXT_BYTES
}

pub fn object_fits<T: Serialize>(value: &T, max: usize, what: &str) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if bytes.len() > max {
        return Err(format!("Oxide {what} exceeds its {max}-byte object limit"));
    }
    Ok(())
}

/// Milliseconds since the epoch, as the front door's `process::now_ms` reads
/// it. Only the record use clock needs it inside this module.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// One fence field's raw presence and type. `Option<String>` folds an
/// explicit JSON `null` into `None`, indistinguishable from an absent key, so
/// this small raw schema keeps the difference: `present` is true whenever the
/// key exists, and `string` only when its value is a JSON string.
#[derive(Default)]
struct RawFence {
    present: bool,
    string: bool,
}

impl<'de> Deserialize<'de> for RawFence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(Self {
            present: true,
            string: value.is_string(),
        })
    }
}

/// Refuse a present fence that is not a JSON string, including explicit
/// `null`. A document this pass cannot read is left to the typed parse, which
/// reports the same malformed input.
fn reject_non_string_fences(bytes: &[u8]) -> Result<(), String> {
    #[derive(Deserialize)]
    struct RawDocument {
        #[serde(default)]
        recorded_accounting: RawFence,
        #[serde(default)]
        one_use_workspaces: RawFence,
        #[serde(default)]
        eviction_accounting: RawFence,
        #[serde(default)]
        slot_ceiling: RawFence,
        #[serde(default)]
        holding: RawFence,
        #[serde(default)]
        owned_outputs: RawFence,
    }
    let Ok(document) = serde_json::from_slice::<RawDocument>(bytes) else {
        return Ok(());
    };
    for (name, field) in [
        ("recorded_accounting", document.recorded_accounting),
        ("one_use_workspaces", document.one_use_workspaces),
        ("eviction_accounting", document.eviction_accounting),
        ("slot_ceiling", document.slot_ceiling),
        ("holding", document.holding),
        ("owned_outputs", document.owned_outputs),
    ] {
        if field.present && !field.string {
            return Err(format!(
                "Oxide state is invalid: a present {name} fence is not a string"
            ));
        }
    }
    Ok(())
}

/// Parse and validate one control document exactly as the front door's
/// `Store::load` does: byte caps, the typed parse (which denies unknown fields
/// and requires the typed shape), the owned-outputs fence rule and
/// [`validate_state`]. The fence uses this same function to decide what it may
/// touch.
pub fn parse_state(bytes: &[u8]) -> Result<State, String> {
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err("Oxide state exceeds its byte limit".to_owned());
    }
    reject_non_string_fences(bytes)?;
    let state: State = serde_json::from_slice(bytes)
        .map_err(|error| format!("Oxide state is invalid: {error}"))?;
    let legacy = match state.version {
        LEGACY_STATE_VERSION => true,
        LEGACY_FIELDS_STATE_VERSION
        | OLDER_STATE_VERSION
        | PREVIOUS_STATE_VERSION
        | RECORDED_ACCOUNTING_STATE_VERSION
        | ONE_USE_STATE_VERSION
        | EVICTION_ACCOUNTING_STATE_VERSION
        | SLOT_CEILING_STATE_VERSION
        | HOLDING_STATE_VERSION
        | WRITER_STATE_VERSION => false,
        _ => {
            return Err(format!(
                "Oxide state has an unsupported version ({})",
                state.version
            ))
        }
    };
    if legacy && bytes.len() as u64 > LEGACY_MAX_STATE_BYTES {
        return Err("Oxide state exceeds its byte limit".to_owned());
    }
    if state.owned_outputs.as_deref().is_some_and(|fence| {
        fence != OWNED_OUTPUTS_FENCE && !predecessor_owned_outputs_fence(fence)
    }) {
        return Err("Oxide state has an invalid owned_outputs fence".to_owned());
    }
    validate_state(&state, legacy)?;
    Ok(state)
}

/// The fence-first upgrade of a document [`parse_state`] already accepted:
/// insert only a genuinely absent forward field and advance the version, then
/// validate the result. A corrupt or unsupported document never reaches this
/// point.
pub fn fenced_state(mut state: State, generation: u32) -> Result<State, String> {
    state
        .check_writer(generation)
        .map_err(|error| error.to_string())?;
    if state.recorded_accounting.is_none() {
        state.recorded_accounting = Some(RECORDED_ACCOUNTING_FENCE.to_owned());
    }
    if state.eviction_accounting.is_none() {
        state.eviction_accounting = Some(EVICTION_ACCOUNTING_FENCE.to_owned());
    }
    if state.slot_ceiling.is_none() {
        state.slot_ceiling = Some(SLOT_CEILING_FENCE.to_owned());
    }
    if state.holding.is_none() {
        state.holding = Some(HOLDING_FENCE.to_owned());
    }
    state.minimum_writer_generation = Some(generation);
    state.version = STATE_VERSION;
    validate_state(&state, false)?;
    Ok(state)
}
