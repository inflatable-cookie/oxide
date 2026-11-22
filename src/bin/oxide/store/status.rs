//! Reading the pool for `oxide status`: the recorded facts, the bounded walks,
//! the diagnostic observation sidecar and the custody view.
//!
//! A recorded fact never reads as a current size and an absent or unreadable
//! one never reads as zero. Recorded mode takes no pool lock, reconciles
//! nothing and writes nothing.

use super::*;
use crate::process;
use oxide_storage::lease::managed::{measure_within, snapshot};
use oxide_storage::lease::{AccountingScan, LedgerSlot, SlotOccupancy, UnmeasuredSlot};
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::control_io::{
    atomic_write_private, bounded_read, lease_error, probe_lock, read_control_state_if_present,
    Probe,
};
use super::recovery::{boot_session, closed_by_boot, return_path};

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
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::LeaseReuse => "measured by the reuse walk when leased",
            Self::AdmissionWalkIncomplete => "walked by an admission without completing",
        }
    }
}

/// The diagnostic sidecar's schema. It is not part of the pool, ledger,
/// registration or receipt formats and grants no accounting authority.
pub(super) const OBSERVATIONS_SCHEMA: &str = "oxide.status-observations/1";

/// One slot's size phrase when the last walk of its generation stopped partial
/// (oxide#186). It never carries a figure.
pub(crate) const INCOMPLETE_WALK: &str = "not measured (walk incomplete)";

pub(super) const OBSERVATIONS_NAME: &str = "status-observations.json";

/// The sidecar's whole-file bound and retained observation count. The count
/// is at least the pool's slot ceiling, so it never lowers real capacity
/// (256 facts at the 256-slot ceiling, oxide#177).
pub(super) const MAX_OBSERVATIONS_BYTES: u64 = 512 * 1024;

pub(super) const MAX_OBSERVATIONS: usize = 256;

/// One recorded size fact, bound to exactly one slot generation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub(super) store_id: String,
    pub(super) slot_id: String,
    pub(super) incarnation: String,
    /// The ledger's recorded slot directory; `None` is the legacy `<id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) dir: Option<String>,
    pub(super) measured_at_ms: u64,
    /// The measured unique allocated bytes. For
    /// [`ObservationSource::AdmissionWalkIncomplete`] this is written as 0 and
    /// read by nothing: an incomplete walk has no size.
    pub(super) allocated_bytes: u64,
    pub(super) source: ObservationSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) lease_id: Option<String>,
}

impl Observation {
    pub(super) fn matches(&self, store_id: &str, slot: &LedgerSlot) -> bool {
        self.store_id == store_id
            && self.slot_id == slot.slot_id
            && self.incarnation == slot.incarnation
            && self.dir == slot.dir
    }

    pub(super) fn validate(&self) -> Result<(), String> {
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
pub(super) struct Observations {
    pub(super) schema: String,
    pub(super) observations: Vec<Observation>,
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
}

/// An existing lease-ticket record shown by `oxide status`.
#[derive(Debug)]
pub(crate) struct Waiter {
    pub(crate) pid: u32,
    pub(crate) age: Option<WaitAge>,
    pub(crate) blocked: bool,
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

pub(super) fn holders(
    control: &Path,
    checkout: &Path,
    cleans: bool,
) -> Result<Vec<String>, String> {
    let Some(state) = read_state(&control.join("state.json"))? else {
        return Ok(Vec::new());
    };
    Ok(holder_lines(&state, checkout, cleans))
}

/// One parsed state's holder lines for `checkout`: active records and, when
/// `cleans`, recorded clean barriers. Shared with the fresh per-row custody
/// witness (oxide#235) so both routes report the exact same texts.
pub(super) fn holder_lines(state: &State, checkout: &Path, cleans: bool) -> Vec<String> {
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
    /// True when Barge reports that this slot's root hold is currently held by
    /// another process (for example, an orphaned child on a ledger-idle slot).
    pub barge_held: bool,
    /// This slot's size for this view: a live walk, a recorded observation
    /// or an honest not-measured phrase.
    pub size: SlotSize,
}

pub struct Interrupted {
    pub holder_pid: Option<u32>,
    pub path: Return,
}

/// The optional diagnostic sidecar, read without following a link. `None`
/// means no observations were ever recorded; any other failure is returned
/// so a caller reports "not measured" instead of falling back to a scan.
pub(super) fn read_observations(control: &Path) -> Result<Option<Observations>, String> {
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
pub(super) fn recorded_size(
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
pub(super) fn retain_newest(observations: &mut Vec<Observation>) {
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

/// Approximate age from the existing ticket file's metadata only.
pub(super) fn ticket_age(path: &Path, now: SystemTime) -> Option<WaitAge> {
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

pub(super) fn wait_age_from_times(
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

impl Store {
    /// Record that an admission's walk of each of these slot generations
    /// stopped partial, so default status reports `not measured (walk
    /// incomplete)` for them (oxide#186). No size is written and nothing is
    /// derived from the fact. A refusal has no grant, so it passes `None` and
    /// the store identity comes from the pool's own recorded snapshot.
    /// Diagnostic only: every failure is ignored, and admission, ownership and
    /// exit status never change.
    pub(super) fn record_incomplete_walks(&self, store_id: Option<&str>, slots: &[UnmeasuredSlot]) {
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
    pub(super) fn record_observation(&self, observation: Observation) -> Result<(), String> {
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
        atomic_write_private(&self.control, OBSERVATIONS_NAME, &bytes)
            .map_err(|error| format!("cannot write recorded observations: {error}"))
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
        let (state, state_error) = match read_control_state_if_present(&self.control) {
            Ok(state) => (state.unwrap_or_default(), None),
            Err(error) => (State::default(), Some(error)),
        };
        let observations = if accounting == Accounting::Measured {
            Ok(None)
        } else {
            read_observations(&self.control)
        };
        let barge_held = if snapshot
            .slots
            .iter()
            .any(|slot| slot.state == SlotOccupancy::Idle)
        {
            oxide_storage::lease::managed::barge_held_slots(&self.pool).unwrap_or_default()
        } else {
            std::collections::BTreeSet::new()
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
                    barge_held: slot.state == SlotOccupancy::Idle
                        && record.is_some()
                        && barge_held.contains(&slot.slot_id),
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
                blocked: self.is_ticket_blocked(ticket.id).unwrap_or(false),
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
}
