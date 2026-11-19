//! Costs observed while the Oxide pool lock is held.

use super::{
    canonical_existing_dir, current_uid, persist_private_file, read_private_file_bounded,
    MANAGED_NAME,
};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Instant;

const HISTORY: &str = "admission-cost.json";
const HISTORY_LEN: usize = 32;
const SLOW_HOLD_MICROS: u64 = 2_000_000;
const HISTORY_BYTES: u64 = 32 * 1024;
const PHASES: [Phase; 4] = [
    Phase::Accounting,
    Phase::Adoption,
    Phase::Eviction,
    Phase::Reserve,
];

#[derive(Clone, Copy, Debug)]
pub(crate) enum Phase {
    Accounting,
    Adoption,
    Eviction,
    Reserve,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Accounting => "accounting",
            Self::Adoption => "adoption",
            Self::Eviction => "eviction",
            Self::Reserve => "reserve",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
struct Cost {
    micros: u64,
    walked: usize,
    listed: usize,
    opaque_listing: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
pub(super) struct Sample {
    micros: u64,
    phases: [Cost; PHASES.len()],
}

impl Sample {
    fn walked(&self) -> usize {
        self.phases
            .iter()
            .fold(0_usize, |total, cost| total.saturating_add(cost.walked))
    }

    fn listed(&self) -> usize {
        self.phases
            .iter()
            .fold(0_usize, |total, cost| total.saturating_add(cost.listed))
    }

    fn opaque_listing(&self) -> bool {
        self.phases.iter().any(|cost| cost.opaque_listing)
    }

    fn describe(&self) -> String {
        let mut phases = Vec::new();
        for phase in PHASES {
            let cost = self.phases[phase as usize];
            if (self.micros > SLOW_HOLD_MICROS && cost.micros != 0)
                || cost.walked != 0
                || cost.listed != 0
                || cost.opaque_listing
            {
                phases.push(format!(
                    "{} {:.1} ms, byte walks {}, inventory entries {}{}",
                    phase.name(),
                    cost.micros as f64 / 1000.0,
                    cost.walked,
                    cost.listed,
                    if cost.opaque_listing {
                        ", Barge listing count unavailable"
                    } else {
                        ""
                    }
                ));
            }
        }
        format!(
            "pool lock {:.1} ms ({})",
            self.micros as f64 / 1000.0,
            phases.join("; ")
        )
    }
}

struct Trace {
    started: Instant,
    changed: Instant,
    phase: Phase,
    sample: Sample,
}

impl Trace {
    fn switch(&mut self, phase: Phase) -> Phase {
        let now = Instant::now();
        self.sample.phases[self.phase as usize].micros += micros(now.duration_since(self.changed));
        self.changed = now;
        std::mem::replace(&mut self.phase, phase)
    }
}

fn micros(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

thread_local! {
    static TRACE: RefCell<Option<Trace>> = const { RefCell::new(None) };
    static SUMMARY: Cell<Option<[Option<Sample>; PHASES.len() * 2]>> = const { Cell::new(None) };
}

/// Collect notable holds for this CLI command, including preparation and retries.
pub fn start_summary() {
    SUMMARY.set(Some([None; PHASES.len() * 2]));
}

/// Consume this command's notable holds. Each phase keeps its strongest sample.
pub fn take_summary() -> Option<String> {
    let samples = SUMMARY.take()?;
    let descriptions: Vec<_> = samples
        .into_iter()
        .flatten()
        .map(|sample| sample.describe())
        .collect();
    (!descriptions.is_empty()).then(|| descriptions.join(" / "))
}

pub(super) struct PoolLock {
    file: Option<File>,
    control: Option<PathBuf>,
}

impl PoolLock {
    pub(super) fn new(file: File, pool: &Path, phase: Option<Phase>) -> Self {
        let started = Instant::now();
        let control = phase.map(|phase| {
            TRACE.set(Some(Trace {
                started,
                changed: started,
                phase,
                sample: Sample::default(),
            }));
            pool.join(MANAGED_NAME)
        });
        Self {
            file: Some(file),
            control,
        }
    }
}

impl Drop for PoolLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let Some(control) = self.control.as_deref() else {
            return;
        };
        let Some(mut trace) = TRACE.take() else {
            return;
        };
        trace.switch(trace.phase);
        trace.sample.micros = micros(trace.started.elapsed());
        let sample = trace.sample;
        if sample.micros > SLOW_HOLD_MICROS || sample.walked() > 1 || sample.opaque_listing() {
            let phase = PHASES
                .into_iter()
                .max_by_key(|phase| {
                    let cost = sample.phases[*phase as usize];
                    if sample.micros > SLOW_HOLD_MICROS {
                        (cost.micros, 0, cost.opaque_listing)
                    } else {
                        (0, cost.walked, cost.opaque_listing)
                    }
                })
                .unwrap_or(trace.phase);
            if let Some(mut summary) = SUMMARY.get() {
                let current =
                    &mut summary[phase as usize * 2 + usize::from(sample.opaque_listing())];
                let weight = |s: &Sample| {
                    (
                        s.micros > SLOW_HOLD_MICROS,
                        s.walked(),
                        s.opaque_listing(),
                        s.micros,
                    )
                };
                if current
                    .as_ref()
                    .is_none_or(|old| weight(&sample) > weight(old))
                {
                    *current = Some(sample);
                }
                SUMMARY.set(Some(summary));
            }
        }
        if let Err(error) = record(control, sample) {
            eprintln!(
                "oxide: admission cost could not be recorded ({error}): {}",
                sample.describe()
            );
        }
    }
}

pub(crate) fn in_phase<T>(phase: Phase, action: impl FnOnce() -> T) -> T {
    struct Restore(Phase);
    impl Drop for Restore {
        fn drop(&mut self) {
            TRACE.with_borrow_mut(|trace| {
                if let Some(trace) = trace {
                    trace.switch(self.0);
                }
            });
        }
    }
    let _restore =
        TRACE.with_borrow_mut(|trace| trace.as_mut().map(|trace| Restore(trace.switch(phase))));
    action()
}

fn observe(action: impl FnOnce(&mut Cost)) {
    TRACE.with_borrow_mut(|trace| {
        if let Some(trace) = trace {
            action(&mut trace.sample.phases[trace.phase as usize]);
        }
    });
}

pub(crate) fn walked(roots: usize) {
    observe(|cost| cost.walked = cost.walked.saturating_add(roots));
}

pub(crate) fn listed(roots: usize) {
    observe(|cost| cost.listed = cost.listed.saturating_add(roots));
}

pub(crate) fn opaque_listing() {
    observe(|cost| cost.opaque_listing = true);
}

fn read(control: &Path) -> Result<Vec<Sample>, String> {
    let path = control.join(HISTORY);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
        Ok(_) => {}
    }
    let bytes =
        read_private_file_bounded(&path, HISTORY_BYTES).map_err(|error| error.to_string())?;
    let history: Vec<Sample> = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if history.len() > HISTORY_LEN {
        return Err("admission history exceeds its sample limit".into());
    }
    Ok(history)
}

fn record(control: &Path, sample: Sample) -> Result<(), String> {
    // An open that refused before creating control state has no safe place
    // for a receipt. Do not create a control directory on that path.
    canonical_existing_dir(control, true).map_err(|error| error.to_string())?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(control.join("admission-cost.lock"))
        .map_err(|error| error.to_string())?;
    let metadata = lock.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.uid() != current_uid() || metadata.mode() & 0o077 != 0 {
        return Err("admission cost lock is not private".into());
    }
    // SAFETY: the open descriptor stays live until the durable write ends.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut history = read(control)?;
    if history.len() == HISTORY_LEN {
        history.remove(0);
    }
    history.push(sample);
    let bytes = serde_json::to_vec(&history).map_err(|error| error.to_string())?;
    persist_private_file(&control.join(HISTORY), &bytes).map_err(|error| error.to_string())
}

/// Describe recent maxima using recorded data only.
pub fn recorded(pool: &Path) -> Result<Option<String>, String> {
    let history = read(&pool.join(MANAGED_NAME))?;
    let Some(slowest) = history.iter().max_by_key(|sample| sample.micros) else {
        return Ok(None);
    };
    let walked = history.iter().map(Sample::walked).max().unwrap_or(0);
    let listed = history.iter().map(Sample::listed).max().unwrap_or(0);
    let opaque = history
        .iter()
        .filter(|sample| sample.opaque_listing())
        .count();
    let slow_phase = PHASES
        .into_iter()
        .max_by_key(|phase| slowest.phases[*phase as usize].micros)
        .unwrap_or(Phase::Accounting);
    Ok(Some(format!(
        "recent admission maxima ({} holds): lock {:.1} ms ({}), byte walks {walked}, explicit inventory entries {listed}{}",
        history.len(),
        slowest.micros as f64 / 1000.0,
        slow_phase.name(),
        if opaque == 0 { String::new() } else { format!(", Barge listing count unavailable for {opaque} holds") }
    )))
}

#[cfg(test)]
pub(super) fn latest(pool: &Path) -> (u64, usize, usize, bool) {
    let history = read(&pool.join(MANAGED_NAME)).unwrap();
    let sample = history.last().unwrap();
    (
        sample.micros,
        sample.walked(),
        sample.listed(),
        sample.opaque_listing(),
    )
}
