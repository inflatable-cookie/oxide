//! `oxide status --launch-check`: how much slower a newly written executable
//! launches than a re-run of the same file, on macOS.
//!
//! Advisory only. A hidden measuring helper, leading its own process group,
//! copies this `oxide` into a private temporary directory and runs the
//! copies in the hidden probe mode, which exits before any other work. The
//! check kills that group and removes the directory before it returns. It
//! starts no Cargo, needs no pool, and reads or changes no setting.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The hidden probe argument. `main` checks it before anything else.
pub const PROBE: &str = "__oxide-launch-probe";
/// Total budget: copies, process starts, waits and cleanup.
const DEADLINE: Duration = Duration::from_secs(3);
/// New work stops here, so cleanup still fits in the deadline.
const WORK_DEADLINE: Duration = Duration::from_millis(2_750);
/// A difference above this is reported as a material penalty.
pub const THRESHOLD_MS: u64 = 30;
/// One copy exists at a time; a larger binary is refused.
const MAX_COPY_BYTES: u64 = 64 * 1024 * 1024;
const COPY_CHUNK: usize = 1024 * 1024;
const RUNS: usize = 5;
const POLL: Duration = Duration::from_micros(250);
const MAX_ANCESTORS: usize = 64;

/// Bounded debug-only phase observation for the launch lifecycle regression.
/// It neither changes a deadline nor permits an effect. The empty environment
/// of helper/probes stays empty; these records describe the outer CLI only.
pub fn record_timing(phase: &str) {
    #[cfg(all(debug_assertions, target_os = "macos"))]
    {
        let Some(path) = std::env::var_os("OXIDE_TEST_LAUNCH_TIMING_LOG") else {
            return;
        };
        if oxide_storage::proof::evidence().is_none() {
            return;
        }
        let path = PathBuf::from(path);
        let Some(parent) = path.parent().and_then(|p| std::fs::canonicalize(p).ok()) else {
            return;
        };
        let Some(root) =
            std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").and_then(|p| std::fs::canonicalize(p).ok())
        else {
            return;
        };
        if !parent.starts_with(&root) || parent == root || path.parent() != Some(parent.as_path()) {
            return;
        }
        // SAFETY: the writable timespec is valid; the shared cross-process
        // uptime clock is observation only, never the product's time owner.
        // SAFETY: an all-zero timespec is valid plain data.
        let mut clock: libc::timespec = unsafe { std::mem::zeroed() };
        if unsafe { libc::clock_gettime(libc::CLOCK_UPTIME_RAW, &mut clock) } != 0 {
            return;
        }
        let Ok(mut file) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        else {
            return;
        };
        use std::os::unix::fs::MetadataExt;
        let Ok(facts) = file.metadata() else {
            return;
        };
        // SAFETY: geteuid has no preconditions.
        let owner = unsafe { libc::geteuid() };
        if !facts.is_file() || facts.nlink() != 1 || facts.uid() != owner || facts.len() > 8192 {
            return;
        }
        let nanos = i128::from(clock.tv_sec) * 1_000_000_000 + i128::from(clock.tv_nsec);
        let _ = writeln!(file, "{} {} {}", std::process::id(), nanos, phase);
    }
    #[cfg(not(all(debug_assertions, target_os = "macos")))]
    let _ = phase;
}

/// Raw timings in microseconds.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Samples {
    pub warmup: Option<u64>,
    pub same: Vec<u64>,
    pub fresh: Vec<u64>,
}

/// A complete set of samples, summarised.
#[derive(Debug, Eq, PartialEq)]
pub struct Measured {
    pub same_median: u64,
    pub fresh_median: u64,
}

impl Measured {
    /// Fresh-copy median minus same-file median; negative when fresh copies
    /// were faster.
    pub fn difference(&self) -> i64 {
        signed(self.fresh_median) - signed(self.same_median)
    }

    pub fn material(&self) -> bool {
        self.difference() > signed(THRESHOLD_MS * 1000)
    }
}

fn signed(micros: u64) -> i64 {
    i64::try_from(micros).unwrap_or(i64::MAX)
}

/// The middle of an odd-sized set; `None` for an incomplete one.
pub fn median(values: &[u64]) -> Option<u64> {
    if values.len() != RUNS {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.get(RUNS / 2).copied()
}

impl Samples {
    /// Medians only when every planned sample exists.
    pub fn measured(&self) -> Option<Measured> {
        self.warmup?;
        Some(Measured {
            same_median: median(&self.same)?,
            fresh_median: median(&self.fresh)?,
        })
    }
}

/// The app bundle the process ancestry suggests, best effort.
#[derive(Debug, Eq, PartialEq)]
pub enum App {
    One(PathBuf),
    None,
    Several(Vec<PathBuf>),
}

/// The outermost `.app` bundle containing `path`, if any.
pub fn outermost_app(path: &Path) -> Option<PathBuf> {
    let mut bundle = PathBuf::new();
    for component in path.components() {
        bundle.push(component);
        if bundle.extension() == Some(OsStr::new("app")) && path != bundle {
            return Some(bundle);
        }
    }
    None
}

/// The distinct bundles in an ancestry, nearest first.
pub fn launching_app(executables: &[PathBuf]) -> App {
    let mut apps: Vec<PathBuf> = Vec::new();
    for app in executables.iter().filter_map(|path| outermost_app(path)) {
        if !apps.contains(&app) {
            apps.push(app);
        }
    }
    match apps.len() {
        0 => App::None,
        1 => App::One(apps.remove(0)),
        _ => App::Several(apps),
    }
}

fn ms(micros: u64) -> String {
    format!("{}.{} ms", micros / 1000, micros % 1000 / 100)
}

fn signed_ms(micros: i64) -> String {
    let sign = if micros < 0 { "-" } else { "" };
    format!("{sign}{}", ms(micros.unsigned_abs()))
}

fn runs(values: &[u64]) -> String {
    let list: Vec<String> = values.iter().map(|&value| ms(value)).collect();
    if list.is_empty() {
        "none".to_owned()
    } else {
        list.join(", ")
    }
}

/// The full report: raw samples, then either the result or why there is none.
pub fn report(outcome: &Outcome, limit: Duration, app: &App) -> String {
    let samples = &outcome.samples;
    let mut text = String::from(
        "launch check: a new executable's first launch against a re-run of the same file\n",
    );
    let warmup = samples.warmup.map_or_else(|| "none".to_owned(), ms);
    let _ = writeln!(
        text,
        "  same file: warm-up {warmup}; runs {}",
        runs(&samples.same)
    );
    let _ = writeln!(text, "  fresh copies: runs {}", runs(&samples.fresh));
    let cleanup = match &outcome.cleanup {
        Cleanup::Removed => "temporary copies removed".to_owned(),
        Cleanup::Left(left) => format!("could not remove {}", left.join(", ")),
        Cleanup::Unsettled { group, left } if left.is_empty() => format!(
            "process group {group} not confirmed closed within the limit; temporary copies removed"
        ),
        Cleanup::Unsettled { group, left } => format!(
            "process group {group} not confirmed closed within the limit; could not remove {}",
            left.join(", ")
        ),
    };
    let _ = writeln!(
        text,
        "  took {} ms of a {} ms limit; {cleanup}",
        outcome.elapsed.as_millis(),
        limit.as_millis()
    );
    let measured = match (outcome.failure.as_deref(), samples.measured()) {
        (None, Some(measured)) => measured,
        (failure, _) => {
            let reason = failure.unwrap_or("incomplete samples");
            // The first launches of a newly built oxide can themselves take
            // longer than the limit; later copies of the same file are faster.
            let hint = if reason == limit_reached(limit) {
                "; run it again, since a newly built oxide's first launches can take longer"
            } else {
                ""
            };
            let _ = writeln!(
                text,
                "  result: inconclusive ({reason}); no median reported{hint}"
            );
            return text;
        }
    };
    let _ = writeln!(
        text,
        "  medians: same file {}, fresh copy {}, difference {}",
        ms(measured.same_median),
        ms(measured.fresh_median),
        signed_ms(measured.difference())
    );
    if !measured.material() {
        let _ = writeln!(
            text,
            "  result: no material penalty (difference at most {THRESHOLD_MS} ms); nothing to do"
        );
        return text;
    }
    let _ = writeln!(
        text,
        "  result: penalty {} per newly written executable, above the {THRESHOLD_MS} ms threshold",
        signed_ms(measured.difference())
    );
    text.push_str(&advice(app));
    text
}

/// What to try when the penalty is material.
pub fn advice(app: &App) -> String {
    let mut text = String::from(
        "  cause: inferred, not measured; it matches macOS's first-launch assessment of new executables (XProtect/Gatekeeper)\n",
    );
    let target = match app {
        App::One(path) => {
            let name = path.file_name().unwrap_or(path.as_os_str());
            let _ = writeln!(
                text,
                "  launching app (best effort, from this process's ancestry): {} ({})",
                name.to_string_lossy(),
                path.display()
            );
            "that app"
        }
        App::None => {
            text.push_str("  launching app: unknown (no .app bundle in this process's ancestry)\n");
            "the app that starts your terminal or agent"
        }
        App::Several(paths) => {
            let names: Vec<String> = paths
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            let _ = writeln!(
                text,
                "  launching app: unknown (the ancestry holds several apps: {})",
                names.join(", ")
            );
            "the app that starts your terminal or agent"
        }
    };
    let _ = writeln!(
        text,
        "  to try: add {target} under System Settings → Privacy & Security → Developer Tools; the app may need a restart before it takes effect"
    );
    text.push_str(
        "  Oxide has not checked or changed any setting, and listing an app may not remove the penalty on this host\n",
    );
    text.push_str("  never disable Gatekeeper globally\n");
    text
}

/// The hidden measuring mode, `__oxide-launch-measure <dir> <work ms>
/// <copy ms> <start ms> <remove ms> <source>`, started only by the check.
pub const MEASURE: &str = "__oxide-launch-measure";
/// The measuring helper's own deadline error; the check reports its limit.
const PAST_DEADLINE: &str = "past the deadline";

/// Hidden modes, checked first in `main`: the probe exits at once and the
/// measuring helper runs and exits. `None` for every other command.
pub fn hidden() -> Option<u8> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next()?;
    if first == PROBE {
        return args.next().is_none().then_some(0);
    }
    if first == MEASURE {
        return Some(helper(&args.collect::<Vec<_>>()));
    }
    None
}

/// Pauses inside the helper, for the integration tests: before a copy's
/// data, after a probe starts and before a removal. Zero otherwise.
#[derive(Clone, Copy, Debug, Default)]
pub struct Delays {
    pub copy: Duration,
    pub start: Duration,
    pub remove: Duration,
}

/// Limits and inputs for one check.
#[derive(Clone, Debug)]
pub struct Plan {
    /// The check returns by then, cleanup included.
    pub limit: Duration,
    /// Samples must complete by then.
    pub work: Duration,
    pub delays: Delays,
    /// What is copied and launched: this `oxide`, unless a test names a
    /// script.
    pub source: PathBuf,
}

impl Plan {
    /// The standard plan. `OXIDE_TEST_LAUNCH_LIMIT_MS`,
    /// `OXIDE_TEST_LAUNCH_DELAY` (`copy:<ms>`, `start:<ms>` or
    /// `remove:<ms>`) and `OXIDE_TEST_LAUNCH_SOURCE` exist for the
    /// integration tests.
    fn from_environment(oxide: PathBuf) -> Result<Self, String> {
        let mut plan = Self {
            limit: DEADLINE,
            work: WORK_DEADLINE,
            delays: Delays::default(),
            source: oxide,
        };
        let millis = |text: &str| text.parse::<u64>().ok().map(Duration::from_millis);
        if let Ok(text) = std::env::var("OXIDE_TEST_LAUNCH_LIMIT_MS") {
            plan.limit = millis(&text).ok_or("OXIDE_TEST_LAUNCH_LIMIT_MS must be milliseconds")?;
            plan.work = plan.limit - plan.limit / 12;
        }
        if let Ok(text) = std::env::var("OXIDE_TEST_LAUNCH_DELAY") {
            let invalid = "OXIDE_TEST_LAUNCH_DELAY must be copy:<ms>, start:<ms> or remove:<ms>";
            let (step, value) = text.split_once(':').ok_or(invalid)?;
            let delay = millis(value).ok_or(invalid)?;
            match step {
                "copy" => plan.delays.copy = delay,
                "start" => plan.delays.start = delay,
                "remove" => plan.delays.remove = delay,
                _ => return Err(invalid.to_owned()),
            }
        }
        if let Some(source) = std::env::var_os("OXIDE_TEST_LAUNCH_SOURCE") {
            plan.source = PathBuf::from(source);
        }
        Ok(plan)
    }
}

fn pause(delay: Duration) {
    if !delay.is_zero() {
        std::thread::sleep(delay);
    }
}

fn limit_reached(limit: Duration) -> String {
    format!("the {} ms limit was reached", limit.as_millis())
}

/// Every name the helper creates in its directory.
fn copy_names() -> impl Iterator<Item = String> {
    std::iter::once("same".to_owned()).chain((0..RUNS).map(|run| format!("fresh-{run}")))
}

/// The measuring helper: samples on stdout, then `done` or `error <reason>`.
/// It removes its own copies and directory before it exits, so an
/// interrupted check leaves nothing either.
fn helper(args: &[std::ffi::OsString]) -> u8 {
    let [dir, work, copy, start, remove, source] = args else {
        return 2;
    };
    let millis = |value: &std::ffi::OsString| {
        value
            .to_str()
            .and_then(|text| text.parse::<u64>().ok())
            .map(Duration::from_millis)
    };
    let (Some(work), Some(copy), Some(start), Some(remove)) =
        (millis(work), millis(copy), millis(start), millis(remove))
    else {
        return 2;
    };
    let deadline = Instant::now() + work;
    let delays = Delays {
        copy,
        start,
        remove,
    };
    let dir = Path::new(dir);
    let mut out = std::io::stdout().lock();
    let result = measure(&mut out, Path::new(source), dir, delays, deadline);
    for name in copy_names() {
        let _ = std::fs::remove_file(dir.join(name));
    }
    let _ = std::fs::remove_dir(dir);
    let _ = match result {
        Ok(()) => writeln!(out, "done"),
        Err(reason) => writeln!(out, "error {reason}"),
    };
    let _ = out.flush();
    0
}

fn emit(out: &mut impl Write, kind: &str, micros: u64) {
    let _ = writeln!(out, "{kind} {micros}");
    let _ = out.flush();
}

/// Warm-up and same-file runs on one copy, then one fresh copy per run.
/// At most one copy exists at a time.
fn measure(
    out: &mut impl Write,
    source: &Path,
    dir: &Path,
    delays: Delays,
    deadline: Instant,
) -> Result<(), String> {
    let same = dir.join("same");
    copy(source, &same, delays, deadline)?;
    emit(out, "warmup", launch(&same, delays, deadline)?);
    for _ in 0..RUNS {
        emit(out, "same", launch(&same, delays, deadline)?);
    }
    remove(&same, delays)?;
    for run in 0..RUNS {
        let fresh = dir.join(format!("fresh-{run}"));
        copy(source, &fresh, delays, deadline)?;
        emit(out, "fresh", launch(&fresh, delays, deadline)?);
        remove(&fresh, delays)?;
    }
    Ok(())
}

/// A new owner-only executable copy of `source`, written in chunks.
fn copy(source: &Path, path: &Path, delays: Delays, deadline: Instant) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("could not copy oxide: {error}");
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(path)
        .map_err(failed)?;
    pause(delays.copy);
    let mut input = std::fs::File::open(source).map_err(failed)?;
    let mut buffer = vec![0_u8; COPY_CHUNK];
    let mut written = 0_u64;
    loop {
        if Instant::now() >= deadline {
            return Err(PAST_DEADLINE.to_owned());
        }
        let read = input.read(&mut buffer).map_err(failed)?;
        if read == 0 {
            return Ok(());
        }
        written += read as u64;
        if written > MAX_COPY_BYTES {
            return Err(format!(
                "oxide grew past the {} MiB copy limit",
                MAX_COPY_BYTES / crate::MIB
            ));
        }
        output.write_all(&buffer[..read]).map_err(failed)?;
    }
}

/// Time one probe launch. A sample counts only if the probe exited
/// successfully by `deadline`; past it the probe is killed and reaped.
fn launch(program: &Path, delays: Delays, deadline: Instant) -> Result<u64, String> {
    let started = Instant::now();
    let mut child = Command::new(program)
        .arg(PROBE)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not start a probe: {error}"))?;
    pause(delays.start);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let ended = Instant::now();
                if ended > deadline {
                    return Err(PAST_DEADLINE.to_owned());
                }
                if !status.success() {
                    return Err(format!("a probe exited with {status}"));
                }
                let elapsed = ended.duration_since(started);
                return Ok(u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX));
            }
            Ok(None) if Instant::now() >= deadline => {
                // Not yet reaped, so the PID is still this child's.
                let _ = child.kill();
                let _ = child.wait();
                return Err(PAST_DEADLINE.to_owned());
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("could not wait for a probe: {error}"));
            }
        }
    }
}

fn remove(path: &Path, delays: Delays) -> Result<(), String> {
    pause(delays.remove);
    std::fs::remove_file(path)
        .map_err(|error| format!("could not remove {}: {error}", path.display()))
}

/// How cleanup ended.
#[derive(Debug, Eq, PartialEq)]
pub enum Cleanup {
    Removed,
    Left(Vec<String>),
    /// The helper's process group was not confirmed closed by the limit.
    Unsettled {
        group: i32,
        left: Vec<String>,
    },
}

/// What one check observed.
pub struct Outcome {
    pub samples: Samples,
    pub failure: Option<String>,
    pub cleanup: Cleanup,
    pub elapsed: Duration,
}

/// A fresh private directory name under the temporary directory.
fn scratch_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    std::env::temp_dir().join(format!("oxide-launch-check-{}-{nanos}", std::process::id()))
}

/// Remove the helper's known copies and the directory; returns what is left.
fn sweep(dir: &Path) -> Vec<String> {
    let mut left = Vec::new();
    for name in copy_names() {
        let path = dir.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => left.push(path.display().to_string()),
        }
    }
    match std::fs::remove_dir(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => left.push(dir.display().to_string()),
    }
    left
}

/// Run the measuring helper as the leader of its own process group and
/// collect its samples until the work deadline. Then kill the whole group
/// (the helper and any probe, even one still starting), reap the helper,
/// confirm the group is empty and remove the directory, all by the limit.
/// Nothing this check started outlives it unless the kernel holds a killed
/// process past the limit, which the report names.
pub fn run_measure(plan: &Plan, oxide: &Path) -> Outcome {
    record_timing("measure-entry");
    let started = Instant::now();
    let limit = started + plan.limit;
    let work = started + plan.work;
    let dir = scratch_dir();
    let mut samples = Samples::default();
    let finish = |samples, failure: Option<String>, cleanup| {
        let elapsed = started.elapsed();
        let failure = failure.or_else(|| (elapsed > plan.limit).then(|| limit_reached(plan.limit)));
        Outcome {
            samples,
            failure,
            cleanup,
            elapsed,
        }
    };
    if let Err(error) = std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        let failure = format!("could not create {}: {error}", dir.display());
        return finish(samples, Some(failure), Cleanup::Removed);
    }
    let millis = |duration: Duration| duration.as_millis().to_string();
    record_timing("helper-spawn-before");
    let spawned = Command::new(oxide)
        .arg(MEASURE)
        .arg(&dir)
        .arg(millis(work.saturating_duration_since(Instant::now())))
        .arg(millis(plan.delays.copy))
        .arg(millis(plan.delays.start))
        .arg(millis(plan.delays.remove))
        .arg(&plan.source)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            let failure = format!("could not start the measurement: {error}");
            let left = sweep(&dir);
            let cleanup = if left.is_empty() {
                Cleanup::Removed
            } else {
                Cleanup::Left(left)
            };
            return finish(samples, Some(failure), cleanup);
        }
    };
    record_timing("helper-spawn-after");
    let group = i32::try_from(child.id()).unwrap_or(0);
    record_group_for_tests(group);
    let (lines, received) = mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        // Probes have null streams, so the pipe closes with the helper.
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if lines.send(line).is_err() {
                    break;
                }
            }
        });
    }
    let failure = loop {
        match received.recv_timeout(work.saturating_duration_since(Instant::now())) {
            Ok(line) => match line.split_once(' ').unwrap_or((line.as_str(), "")) {
                ("done", "") => break None,
                ("error", PAST_DEADLINE) => break Some(limit_reached(plan.limit)),
                ("error", reason) => break Some(reason.to_owned()),
                (kind, micros) => match (kind, micros.parse::<u64>()) {
                    ("warmup", Ok(micros)) => samples.warmup = Some(micros),
                    ("same", Ok(micros)) => samples.same.push(micros),
                    ("fresh", Ok(micros)) => samples.fresh.push(micros),
                    _ => break Some("the measurement printed an unexpected line".to_owned()),
                },
            },
            Err(mpsc::RecvTimeoutError::Timeout) => break Some(limit_reached(plan.limit)),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Some("the measurement ended early".to_owned())
            }
        }
    };
    record_timing("collection-ended");
    if group > 1 {
        // SAFETY: the helper is not yet reaped, so this group ID is still
        // its own group: the helper and the probes it started.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
    let reaped = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if Instant::now() < limit => std::thread::sleep(POLL),
            _ => break false,
        }
    };
    record_timing("helper-reap-ended");
    let closed = reaped
        && loop {
            if crate::process::group_empty(group) {
                break true;
            }
            if Instant::now() >= limit {
                break false;
            }
            std::thread::sleep(POLL);
        };
    record_timing("group-check-ended");
    let left = sweep(&dir);
    record_timing("sweep-ended");
    let cleanup = match (closed, left.is_empty()) {
        (true, true) => Cleanup::Removed,
        (true, false) => Cleanup::Left(left),
        (false, _) => Cleanup::Unsettled { group, left },
    };
    let failure = failure.or_else(|| (!closed).then(|| limit_reached(plan.limit)));
    finish(samples, failure, cleanup)
}

/// Debug builds only: `OXIDE_TEST_LAUNCH_GROUP_LOG` names a file, absent
/// beforehand, that receives the measuring helper's process-group ID once it
/// is spawned, so the integration tests can check that group is gone after
/// the CLI exits. Observation only: a write failure is ignored here (the
/// test fails on the missing record) and nothing else changes.
fn record_group_for_tests(group: i32) {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("OXIDE_TEST_LAUNCH_GROUP_LOG") {
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .and_then(|mut file| writeln!(file, "{group}"));
    }
    #[cfg(not(debug_assertions))]
    let _ = group;
}

/// Only a successful material measurement can print ancestry-based advice.
/// Do not add an unused, potentially slow lookup after deadline/cleanup when
/// report() will return its inconclusive or no-penalty text first.
fn app_for_advice(outcome: &Outcome, ancestors: impl FnOnce() -> Vec<PathBuf>) -> App {
    if outcome.failure.is_none()
        && outcome
            .samples
            .measured()
            .is_some_and(|value| value.material())
    {
        launching_app(&ancestors())
    } else {
        App::None
    }
}

/// `oxide status --launch-check`.
pub fn run_check() -> Result<u8, String> {
    record_timing("check-entry");
    if !cfg!(target_os = "macos") {
        println!("launch check: not applicable (it measures a macOS first-launch cost)");
        return Ok(0);
    }
    let oxide =
        std::env::current_exe().map_err(|error| format!("could not locate oxide: {error}"))?;
    let plan = Plan::from_environment(oxide.clone())?;
    let size = std::fs::metadata(&plan.source)
        .map_err(|error| format!("could not read {}: {error}", plan.source.display()))?
        .len();
    if size > MAX_COPY_BYTES {
        return Err(format!(
            "launch check refused: oxide is {} MiB, over the {} MiB copy limit",
            size / crate::MIB,
            MAX_COPY_BYTES / crate::MIB
        ));
    }
    let outcome = run_measure(&plan, &oxide);
    record_timing("measure-returned");
    let app = app_for_advice(&outcome, ancestry);
    record_timing("advice-app-ended");
    print!("{}", report(&outcome, plan.limit, &app));
    record_timing("report-ended");
    Ok(0)
}

/// Executable paths of this process's ancestors, nearest first, as far as
/// the same user can read them.
#[cfg(target_os = "macos")]
fn ancestry() -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut paths = Vec::new();
    // SAFETY: getppid has no preconditions.
    let mut pid = unsafe { libc::getppid() };
    for _ in 0..MAX_ANCESTORS {
        if pid <= 1 {
            break;
        }
        let mut buffer = vec![0_u8; 4096];
        // SAFETY: the buffer is writable for the length passed.
        let length = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), 4096) };
        if let Ok(length) = usize::try_from(length) {
            if length > 0 {
                buffer.truncate(length);
                paths.push(PathBuf::from(OsStr::from_bytes(&buffer)));
            }
        }
        // SAFETY: an all-zero proc_bsdinfo is a valid plain-data value.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).unwrap_or(0);
        // SAFETY: the buffer is one proc_bsdinfo of the size passed.
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                std::ptr::addr_of_mut!(info).cast(),
                size,
            )
        };
        if read != size {
            break;
        }
        pid = i32::try_from(info.pbi_ppid).unwrap_or(0);
    }
    paths
}

#[cfg(not(target_os = "macos"))]
fn ancestry() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full(same: [u64; 5], fresh: [u64; 5]) -> Samples {
        Samples {
            warmup: Some(120_000),
            same: same.to_vec(),
            fresh: fresh.to_vec(),
        }
    }

    #[test]
    fn medians_need_every_sample() {
        assert_eq!(median(&[5, 1, 4, 2, 3]), Some(3));
        assert_eq!(median(&[1, 2, 3, 4]), None);
        let mut samples = full([1; 5], [2; 5]);
        assert!(samples.measured().is_some());
        samples.warmup = None;
        assert_eq!(samples.measured(), None, "no warm-up, no median");
        let short = Samples {
            warmup: Some(1),
            same: vec![1; 5],
            fresh: vec![2; 3],
        };
        assert_eq!(short.measured(), None);
    }

    #[test]
    fn difference_is_signed_and_threshold_is_strict() {
        let faster = full([3_000; 5], [2_000; 5]).measured().unwrap();
        assert_eq!(faster.difference(), -1_000);
        assert!(!faster.material());
        let at = full([2_000; 5], [32_000; 5]).measured().unwrap();
        assert_eq!(at.difference(), 30_000);
        assert!(!at.material(), "exactly the threshold is not material");
        let above = full([2_000; 5], [32_001; 5]).measured().unwrap();
        assert!(above.material());
        // A single slow outlier does not move the median.
        let outlier = full([2_000, 2_100, 2_200, 2_300, 290_000], [2_400; 5]);
        assert!(!outlier.measured().unwrap().material());
    }

    #[test]
    fn ancestry_is_unused_after_failed_incomplete_or_non_material_measurement() {
        let outcome = |samples, failure| Outcome {
            samples,
            failure,
            cleanup: Cleanup::Removed,
            elapsed: Duration::from_millis(600),
        };
        let unused = || panic!("ancestry lookup must not run without printable advice");
        let failed = outcome(
            full([1; 5], [100_000; 5]),
            Some(limit_reached(Duration::from_millis(600))),
        );
        assert_eq!(app_for_advice(&failed, unused), App::None);
        assert!(report(&failed, Duration::from_millis(600), &App::None).contains("inconclusive"));
        let incomplete = outcome(Samples::default(), None);
        assert_eq!(app_for_advice(&incomplete, unused), App::None);
        let non_material = outcome(full([2_000; 5], [2_001; 5]), None);
        assert_eq!(app_for_advice(&non_material, unused), App::None);
        let at_threshold = outcome(full([2_000; 5], [32_000; 5]), None);
        assert_eq!(app_for_advice(&at_threshold, unused), App::None);
        let called = std::cell::Cell::new(0);
        let material = outcome(full([2_000; 5], [32_001; 5]), None);
        let app = app_for_advice(&material, || {
            called.set(called.get() + 1);
            vec![PathBuf::from(
                "/Applications/Fixture.app/Contents/MacOS/Fixture",
            )]
        });
        assert_eq!(called.get(), 1);
        assert_eq!(app, App::One(PathBuf::from("/Applications/Fixture.app")));
        assert!(report(&material, DEADLINE, &app).contains("launching app (best effort"));
    }

    #[test]
    fn ancestry_names_the_outermost_bundle() {
        assert_eq!(
            outermost_app(Path::new(
                "/Applications/Code.app/Contents/Frameworks/Code Helper.app/Contents/MacOS/Code Helper"
            )),
            Some(PathBuf::from("/Applications/Code.app"))
        );
        assert_eq!(outermost_app(Path::new("/bin/zsh")), None);
        assert_eq!(
            outermost_app(Path::new("/Applications/Odd.app")),
            None,
            "a bundle is a directory"
        );
        let terminal =
            PathBuf::from("/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal");
        assert_eq!(
            launching_app(&[
                PathBuf::from("/bin/zsh"),
                PathBuf::from("/usr/bin/login"),
                terminal
            ]),
            App::One(PathBuf::from("/System/Applications/Utilities/Terminal.app"))
        );
        assert_eq!(launching_app(&[PathBuf::from("/bin/zsh")]), App::None);
        assert_eq!(
            launching_app(&[
                PathBuf::from("/Applications/A.app/Contents/MacOS/A"),
                PathBuf::from("/Applications/B.app/Contents/MacOS/B"),
                PathBuf::from("/Applications/A.app/Contents/MacOS/A"),
            ]),
            App::Several(vec![
                PathBuf::from("/Applications/A.app"),
                PathBuf::from("/Applications/B.app")
            ])
        );
    }

    fn render(samples: Samples, failure: Option<&str>, cleanup: Cleanup) -> String {
        let outcome = Outcome {
            samples,
            failure: failure.map(str::to_owned),
            cleanup,
            elapsed: Duration::from_millis(900),
        };
        report(&outcome, DEADLINE, &App::None)
    }

    #[test]
    fn report_keeps_raw_samples_and_states_the_result() {
        let small = render(full([2_300; 5], [2_500; 5]), None, Cleanup::Removed);
        assert!(small.contains("same file: warm-up 120.0 ms; runs 2.3 ms, 2.3 ms"));
        assert!(small.contains("medians: same file 2.3 ms, fresh copy 2.5 ms, difference 0.2 ms"));
        assert!(small.contains("result: no material penalty"));
        assert!(
            !small.contains("Developer Tools"),
            "no advice below the threshold"
        );
        let negative = render(full([3_000; 5], [2_000; 5]), None, Cleanup::Removed);
        assert!(negative.contains("difference -1.0 ms"));
        let failed = render(
            full([2_000; 5], [100_000; 5]),
            Some("a probe exited with exit status: 1"),
            Cleanup::Left(vec!["/t/x".to_owned()]),
        );
        assert!(failed.contains(
            "result: inconclusive (a probe exited with exit status: 1); no median reported"
        ));
        assert!(!failed.contains("medians:"));
        let partial = Samples {
            warmup: Some(1),
            same: vec![1; 5],
            fresh: vec![100_000; 2],
        };
        let partial = render(
            partial,
            Some("the 3000 ms limit was reached"),
            Cleanup::Unsettled {
                group: 41,
                left: Vec::new(),
            },
        );
        assert!(partial.contains("fresh copies: runs 100.0 ms, 100.0 ms\n"));
        assert!(partial.contains(
            "inconclusive (the 3000 ms limit was reached); no median reported; run it again"
        ));
        assert!(
            !failed.contains("run it again"),
            "the hint is for the limit only"
        );
        let penalty = render(full([2_300; 5], [101_600; 5]), None, Cleanup::Removed);
        assert!(penalty.contains(
            "result: penalty 99.3 ms per newly written executable, above the 30 ms threshold"
        ));
        assert!(penalty.contains("took 900 ms of a 3000 ms limit; temporary copies removed"));
        assert!(failed.contains("; could not remove /t/x\n"));
        assert!(partial.contains(
            "; process group 41 not confirmed closed within the limit; temporary copies removed\n"
        ));
    }

    #[test]
    fn advice_points_to_developer_tools_and_never_claims_a_fix() {
        let named = advice(&App::One(PathBuf::from("/Applications/Paseo.app")));
        assert!(named.contains("cause: inferred, not measured"));
        assert!(named.contains("launching app (best effort, from this process's ancestry): Paseo.app (/Applications/Paseo.app)"));
        assert!(named
            .contains("add that app under System Settings → Privacy & Security → Developer Tools"));
        assert!(named.contains("may need a restart"));
        assert!(named.contains("has not checked or changed any setting"));
        assert!(named.contains("never disable Gatekeeper globally"));
        let unknown = advice(&App::None);
        assert!(unknown.contains("launching app: unknown (no .app bundle"));
        assert!(unknown.contains("add the app that starts your terminal or agent under"));
        let several = advice(&App::Several(vec![
            PathBuf::from("/A.app"),
            PathBuf::from("/B.app"),
        ]));
        assert!(several.contains("several apps: /A.app, /B.app"));
        for text in [&named, &unknown, &several] {
            assert!(!text.contains("spctl") && !text.contains("fixes"), "{text}");
        }
    }
}
