//! `oxide status --launch-check`: how much slower a newly written executable
//! launches than a re-run of the same file, on macOS.
//!
//! Advisory only. It copies this `oxide` into a private temporary directory
//! and runs the copies in the hidden probe mode, which exits before any other
//! work. It starts no Cargo, needs no pool, and reads or changes no setting.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
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

/// True when this process is a launch-check probe: exactly the hidden
/// argument and nothing else.
pub fn is_probe() -> bool {
    let mut args = std::env::args_os().skip(1);
    args.next().as_deref() == Some(OsStr::new(PROBE)) && args.next().is_none()
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
        Cleanup::Unfinished => format!(
            "cleanup did not finish within the limit; {} may remain",
            outcome.dir.display()
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

/// Deadlines for one check, plus delays the tests inject.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    /// The check returns by then, whatever is still blocked.
    pub limit: Duration,
    /// Samples must complete by then; no new work starts after it.
    pub work: Duration,
    pub delays: Delays,
}

/// Pauses before a copy's data, inside a start and inside each removal.
/// Zero outside tests.
#[derive(Clone, Copy, Debug, Default)]
pub struct Delays {
    pub copy: Duration,
    pub start: Duration,
    pub remove: Duration,
}

impl Plan {
    pub const STANDARD: Self = Self {
        limit: DEADLINE,
        work: WORK_DEADLINE,
        delays: Delays {
            copy: Duration::ZERO,
            start: Duration::ZERO,
            remove: Duration::ZERO,
        },
    };
}

fn pause(delay: Duration) {
    if !delay.is_zero() {
        std::thread::sleep(delay);
    }
}

fn limit_reached(limit: Duration) -> String {
    format!("the {} ms limit was reached", limit.as_millis())
}

const STOPPED: &str = "stopped at the limit";

/// What the measuring thread owns and cleanup must undo. Every change is
/// made under the lock, so cleanup sees each created file and an unreaped
/// child, and nothing new is created once it has started.
#[derive(Default)]
struct Owned {
    stopped: bool,
    dir: Option<PathBuf>,
    files: Vec<PathBuf>,
    child: Option<Child>,
}

#[derive(Default)]
struct Check {
    owned: Mutex<Owned>,
    /// Mirrors `Owned::stopped` for the unlocked copy loop.
    stopped: AtomicBool,
}

impl Check {
    fn lock(&self) -> MutexGuard<'_, Owned> {
        self.owned.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One step under the lock, refused once cleanup has started.
    fn step<T>(&self, action: impl FnOnce(&mut Owned) -> Result<T, String>) -> Result<T, String> {
        let mut owned = self.lock();
        if owned.stopped {
            return Err(STOPPED.to_owned());
        }
        action(&mut owned)
    }

    /// A new owner-only executable copy of `source`, written in chunks.
    fn copy(&self, source: &Path, path: &Path, plan: &Plan) -> Result<(), String> {
        let failed = |error: std::io::Error| format!("could not copy oxide: {error}");
        let mut output = self.step(|owned| {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o700)
                .open(path)
                .map_err(failed)?;
            owned.files.push(path.to_owned());
            Ok(file)
        })?;
        pause(plan.delays.copy);
        let mut input = std::fs::File::open(source).map_err(failed)?;
        let mut buffer = vec![0_u8; COPY_CHUNK];
        let mut written = 0_u64;
        loop {
            if self.stopped.load(Ordering::SeqCst) {
                return Err(STOPPED.to_owned());
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
    /// successfully by `deadline`; otherwise the unreaped child is left
    /// for cleanup to kill and reap.
    fn launch(&self, program: &Path, plan: &Plan, deadline: Instant) -> Result<u64, String> {
        let started = self.step(|owned| {
            pause(plan.delays.start);
            let started = Instant::now();
            let child = Command::new(program)
                .arg(PROBE)
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|error| format!("could not start a probe: {error}"))?;
            owned.child = Some(child);
            Ok(started)
        })?;
        loop {
            let exited = self.step(|owned| {
                let child = owned.child.as_mut().ok_or("the probe was not recorded")?;
                match child.try_wait() {
                    Ok(Some(status)) => {
                        owned.child = None;
                        Ok(Some((status, Instant::now())))
                    }
                    Ok(None) => Ok(None),
                    Err(error) => Err(format!("could not wait for a probe: {error}")),
                }
            })?;
            if let Some((status, ended)) = exited {
                if ended > deadline {
                    return Err(limit_reached(plan.limit));
                }
                if !status.success() {
                    return Err(format!("a probe exited with {status}"));
                }
                let elapsed = ended.duration_since(started);
                return Ok(u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX));
            }
            if Instant::now() >= deadline {
                return Err(limit_reached(plan.limit));
            }
            std::thread::sleep(POLL);
        }
    }

    fn remove(&self, path: &Path, plan: &Plan) -> Result<(), String> {
        self.step(|owned| {
            pause(plan.delays.remove);
            std::fs::remove_file(path)
                .map_err(|error| format!("could not remove {}: {error}", path.display()))?;
            owned.files.retain(|file| file != path);
            Ok(())
        })
    }

    /// Stop new work, kill and reap an unreaped probe (only this check's
    /// own child), then remove every created file and the directory.
    /// Returns what is left behind.
    fn clean(&self, plan: &Plan) -> Vec<String> {
        let mut owned = self.lock();
        owned.stopped = true;
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(mut child) = owned.child.take() {
            // Not yet reaped, so the PID is still this child's.
            let _ = child.kill();
            let _ = child.wait();
        }
        let mut left = Vec::new();
        for file in std::mem::take(&mut owned.files) {
            pause(plan.delays.remove);
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => left.push(file.display().to_string()),
            }
        }
        if let Some(dir) = owned.dir.take() {
            if std::fs::remove_dir(&dir).is_err() && dir.exists() {
                left.push(dir.display().to_string());
            }
        }
        left
    }
}

/// A sample or the end of the measuring thread.
enum Event {
    Warmup(u64),
    Same(u64),
    Fresh(u64),
    Done(Result<(), String>),
}

/// Warm-up and same-file runs on one copy, then one fresh copy per run.
/// At most one copy exists at a time.
fn measure(
    check: &Check,
    source: &Path,
    dir: &Path,
    plan: &Plan,
    deadline: Instant,
    events: &mpsc::Sender<Event>,
) -> Result<(), String> {
    check.step(|owned| {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(dir)
            .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
        owned.dir = Some(dir.to_owned());
        Ok(())
    })?;
    let same = dir.join("same");
    check.copy(source, &same, plan)?;
    let _ = events.send(Event::Warmup(check.launch(&same, plan, deadline)?));
    for _ in 0..RUNS {
        let _ = events.send(Event::Same(check.launch(&same, plan, deadline)?));
    }
    check.remove(&same, plan)?;
    for run in 0..RUNS {
        let fresh = dir.join(format!("fresh-{run}"));
        check.copy(source, &fresh, plan)?;
        let _ = events.send(Event::Fresh(check.launch(&fresh, plan, deadline)?));
        check.remove(&fresh, plan)?;
    }
    Ok(())
}

/// How cleanup ended.
#[derive(Debug, Eq, PartialEq)]
pub enum Cleanup {
    Removed,
    Left(Vec<String>),
    /// Still blocked at the limit; the directory may remain.
    Unfinished,
}

/// What one check observed.
pub struct Outcome {
    pub samples: Samples,
    pub failure: Option<String>,
    pub cleanup: Cleanup,
    pub elapsed: Duration,
    /// The private directory the check used.
    pub dir: PathBuf,
}

/// A fresh private directory name under the temporary directory.
fn scratch_dir() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    std::env::temp_dir().join(format!(
        "oxide-launch-check-{}-{nanos}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ))
}

/// Measure on a separate thread and clean up on another, so a blocked copy,
/// start, wait or removal cannot hold the check past `plan.limit`. A thread
/// still blocked then is abandoned and ends with the process; the report
/// names what may remain.
pub fn run_measure(source: &Path, plan: Plan) -> Outcome {
    let started = Instant::now();
    let limit = started + plan.limit;
    let deadline = started + plan.work;
    let dir = scratch_dir();
    let check = Arc::new(Check::default());
    let (events, received) = mpsc::channel();
    {
        let check = Arc::clone(&check);
        let source = source.to_owned();
        let dir = dir.clone();
        std::thread::spawn(move || {
            let result = measure(&check, &source, &dir, &plan, deadline, &events);
            let _ = events.send(Event::Done(result));
        });
    }
    let mut samples = Samples::default();
    let mut failure = loop {
        let wait = deadline.saturating_duration_since(Instant::now());
        match received.recv_timeout(wait) {
            Ok(Event::Warmup(micros)) => samples.warmup = Some(micros),
            Ok(Event::Same(micros)) => samples.same.push(micros),
            Ok(Event::Fresh(micros)) => samples.fresh.push(micros),
            Ok(Event::Done(result)) => break result.err(),
            Err(mpsc::RecvTimeoutError::Timeout) => break Some(limit_reached(plan.limit)),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Some("the measurement stopped unexpectedly".to_owned())
            }
        }
    };
    let (cleaned, outcome) = mpsc::channel();
    {
        let check = Arc::clone(&check);
        std::thread::spawn(move || {
            let _ = cleaned.send(check.clean(&plan));
        });
    }
    let cleanup = match outcome.recv_timeout(limit.saturating_duration_since(Instant::now())) {
        Ok(left) if left.is_empty() => Cleanup::Removed,
        Ok(left) => Cleanup::Left(left),
        Err(_) => {
            failure.get_or_insert_with(|| limit_reached(plan.limit));
            Cleanup::Unfinished
        }
    };
    let elapsed = started.elapsed();
    if elapsed > plan.limit {
        failure.get_or_insert_with(|| limit_reached(plan.limit));
    }
    Outcome {
        samples,
        failure,
        cleanup,
        elapsed,
        dir,
    }
}

/// `oxide status --launch-check`.
pub fn run_check() -> Result<u8, String> {
    if !cfg!(target_os = "macos") {
        println!("launch check: not applicable (it measures a macOS first-launch cost)");
        return Ok(0);
    }
    let source =
        std::env::current_exe().map_err(|error| format!("could not locate oxide: {error}"))?;
    let size = std::fs::metadata(&source)
        .map_err(|error| format!("could not read {}: {error}", source.display()))?
        .len();
    if size > MAX_COPY_BYTES {
        return Err(format!(
            "launch check refused: oxide is {} MiB, over the {} MiB copy limit",
            size / crate::MIB,
            MAX_COPY_BYTES / crate::MIB
        ));
    }
    let outcome = run_measure(&source, Plan::STANDARD);
    let app = launching_app(&ancestry());
    print!("{}", report(&outcome, DEADLINE, &app));
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
            dir: PathBuf::from("/t/oxide-launch-check-1"),
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
            Cleanup::Unfinished,
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
            "; cleanup did not finish within the limit; /t/oxide-launch-check-1 may remain\n"
        ));
    }

    /// A private directory for test sources, and a short plan.
    fn source(label: &str, script: &str) -> PathBuf {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(format!("launch-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("launch scratch");
        let path = dir.join("probe");
        std::fs::write(&path, script).expect("probe script");
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .expect("probe mode");
        path
    }

    const LIMIT: Duration = Duration::from_millis(600);
    /// Scheduling slack above the limit on a loaded host.
    const SLACK: Duration = Duration::from_millis(400);

    fn plan(delays: Delays) -> Plan {
        Plan {
            limit: LIMIT,
            work: Duration::from_millis(500),
            delays,
        }
    }

    /// Run a check and assert its bound.
    fn bounded(label: &str, script: &str, plan: Plan) -> Outcome {
        let probe = source(label, script);
        let started = Instant::now();
        let outcome = run_measure(&probe, plan);
        let wall = started.elapsed();
        assert!(
            wall < plan.limit + SLACK,
            "{label}: returned after {wall:?}"
        );
        assert!(outcome.elapsed <= wall);
        outcome
    }

    fn eventually_gone(dir: &Path) {
        let until = Instant::now() + Duration::from_secs(5);
        while dir.exists() {
            assert!(Instant::now() < until, "left behind: {}", dir.display());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A new script file pays the same first-launch cost as a binary, so
    /// a complete check needs the standard limit.
    const QUICK: &str = "#!/bin/sh\nexit 0\n";

    #[test]
    fn an_ordinary_check_completes_and_cleans_up() {
        let outcome = bounded("ordinary", QUICK, Plan::STANDARD);
        assert_eq!(outcome.failure, None);
        assert!(
            outcome.samples.measured().is_some(),
            "{:?}",
            outcome.samples
        );
        assert_eq!(outcome.cleanup, Cleanup::Removed);
        assert!(
            !outcome.dir.exists(),
            "left behind: {}",
            outcome.dir.display()
        );
    }

    #[test]
    fn a_probe_past_the_deadline_is_killed_and_never_a_sample() {
        let outcome = bounded(
            "slow",
            "#!/bin/sh\nexec sleep 30\n",
            plan(Delays::default()),
        );
        assert_eq!(
            outcome.failure.as_deref(),
            Some("the 600 ms limit was reached")
        );
        assert_eq!(outcome.samples.warmup, None);
        assert_eq!(outcome.cleanup, Cleanup::Removed);
        assert!(
            !outcome.dir.exists(),
            "left behind: {}",
            outcome.dir.display()
        );
    }

    #[test]
    fn a_slow_copy_cannot_hold_the_check() {
        let delays = Delays {
            copy: Duration::from_secs(2),
            ..Delays::default()
        };
        let outcome = bounded("copy", QUICK, plan(delays));
        assert_eq!(
            outcome.failure.as_deref(),
            Some("the 600 ms limit was reached")
        );
        assert_eq!(outcome.samples.measured(), None);
        // The copy pauses outside the lock, so cleanup ran in time.
        assert_eq!(outcome.cleanup, Cleanup::Removed);
        assert!(
            !outcome.dir.exists(),
            "left behind: {}",
            outcome.dir.display()
        );
        // The resumed copy stops without creating anything.
        std::thread::sleep(Duration::from_millis(2_100));
        assert!(
            !outcome.dir.exists(),
            "recreated: {}",
            outcome.dir.display()
        );
    }

    #[test]
    fn a_slow_start_or_removal_reports_what_may_remain() {
        for (label, delays) in [
            (
                "start",
                Delays {
                    start: Duration::from_secs(1),
                    ..Delays::default()
                },
            ),
            (
                "remove",
                Delays {
                    remove: Duration::from_millis(700),
                    ..Delays::default()
                },
            ),
        ] {
            let outcome = bounded(label, QUICK, plan(delays));
            assert_eq!(
                outcome.failure.as_deref(),
                Some("the 600 ms limit was reached"),
                "{label}"
            );
            assert_eq!(outcome.samples.measured(), None, "{label}");
            assert_eq!(outcome.cleanup, Cleanup::Unfinished, "{label}");
            // In this test process the abandoned threads finish; cleanup
            // then kills any started probe and removes everything.
            eventually_gone(&outcome.dir);
        }
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
