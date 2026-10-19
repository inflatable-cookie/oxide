//! `oxide status --launch-check`: how much slower a newly written executable
//! launches than a re-run of the same file, on macOS.
//!
//! Advisory only. It copies this `oxide` into a private temporary directory
//! and runs the copies in the hidden probe mode, which exits before any other
//! work. It starts no Cargo, needs no pool, and reads or changes no setting.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
pub fn report(
    samples: &Samples,
    failure: Option<&str>,
    app: &App,
    elapsed: Duration,
    cleanup: &str,
) -> String {
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
    let _ = writeln!(
        text,
        "  took {} ms of a {} ms limit; {cleanup}",
        elapsed.as_millis(),
        DEADLINE.as_millis()
    );
    let measured = match (failure, samples.measured()) {
        (None, Some(measured)) => measured,
        (failure, _) => {
            let reason = failure.unwrap_or("incomplete samples");
            // The first launches of a newly built oxide can themselves take
            // longer than the limit; later copies of the same file are faster.
            let hint = if reason == limit_reached() {
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

/// Private scratch: every created file and the directory are removed on drop.
struct Scratch {
    dir: PathBuf,
    files: Vec<PathBuf>,
}

impl Scratch {
    fn create() -> Result<Self, String> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let dir =
            std::env::temp_dir().join(format!("oxide-launch-check-{}-{nanos}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
        Ok(Self {
            dir,
            files: Vec::new(),
        })
    }

    /// A new copy of `source`, owner-only and executable.
    fn copy(&mut self, source: &Path, name: &str) -> Result<PathBuf, String> {
        let path = self.dir.join(name);
        self.files.push(path.clone());
        std::fs::copy(source, &path)
            .and_then(|_| std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)))
            .map_err(|error| format!("could not copy oxide: {error}"))?;
        Ok(path)
    }

    fn remove(&mut self, path: &Path) -> Result<(), String> {
        std::fs::remove_file(path)
            .map_err(|error| format!("could not remove {}: {error}", path.display()))?;
        self.files.retain(|file| file != path);
        Ok(())
    }

    /// Remove everything; the description names anything left behind.
    fn cleanup(mut self) -> String {
        let left = self.remove_all();
        if left.is_empty() {
            "temporary copies removed".to_owned()
        } else {
            format!("could not remove {}", left.join(", "))
        }
    }

    fn remove_all(&mut self) -> Vec<String> {
        let mut left = Vec::new();
        for file in std::mem::take(&mut self.files) {
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => left.push(file.display().to_string()),
            }
        }
        if std::fs::remove_dir(&self.dir).is_err() && self.dir.exists() {
            left.push(self.dir.display().to_string());
        }
        left
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.files.is_empty() || self.dir.exists() {
            let _ = self.remove_all();
        }
    }
}

/// Time one probe launch. Past the deadline the child is killed and reaped.
fn time_launch(program: &Path, deadline: Instant) -> Result<u64, String> {
    let started = Instant::now();
    let mut child = Command::new(program)
        .arg(PROBE)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not start a probe: {error}"))?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let elapsed = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
                return if status.success() {
                    Ok(elapsed)
                } else {
                    Err(format!("a probe exited with {status}"))
                };
            }
            Ok(None) if Instant::now() >= deadline => {
                // Not yet reaped, so the PID is still this child's.
                let _ = child.kill();
                let _ = child.wait();
                return Err(limit_reached());
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

fn limit_reached() -> String {
    format!("the {} ms limit was reached", DEADLINE.as_millis())
}

fn check_deadline(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        return Err(limit_reached());
    }
    Ok(())
}

/// Warm-up and same-file runs on one copy, then one fresh copy per run.
/// At most one copy exists at a time.
fn measure(
    scratch: &mut Scratch,
    source: &Path,
    samples: &mut Samples,
    deadline: Instant,
) -> Result<(), String> {
    let same = scratch.copy(source, "same")?;
    samples.warmup = Some(time_launch(&same, deadline)?);
    for _ in 0..RUNS {
        check_deadline(deadline)?;
        samples.same.push(time_launch(&same, deadline)?);
    }
    scratch.remove(&same)?;
    for run in 0..RUNS {
        check_deadline(deadline)?;
        let fresh = scratch.copy(source, &format!("fresh-{run}"))?;
        samples.fresh.push(time_launch(&fresh, deadline)?);
        scratch.remove(&fresh)?;
    }
    Ok(())
}

/// `oxide status --launch-check`.
pub fn run_check() -> Result<u8, String> {
    if !cfg!(target_os = "macos") {
        println!("launch check: not applicable (it measures a macOS first-launch cost)");
        return Ok(0);
    }
    let started = Instant::now();
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
    let mut samples = Samples::default();
    let mut scratch = Scratch::create()?;
    let failure = measure(&mut scratch, &source, &mut samples, started + WORK_DEADLINE).err();
    let cleanup = scratch.cleanup();
    let elapsed = started.elapsed();
    let app = launching_app(&ancestry());
    print!(
        "{}",
        report(&samples, failure.as_deref(), &app, elapsed, &cleanup)
    );
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

    #[test]
    fn report_keeps_raw_samples_and_states_the_result() {
        let elapsed = Duration::from_millis(900);
        let small = report(
            &full([2_300; 5], [2_500; 5]),
            None,
            &App::None,
            elapsed,
            "temporary copies removed",
        );
        assert!(small.contains("same file: warm-up 120.0 ms; runs 2.3 ms, 2.3 ms"));
        assert!(small.contains("medians: same file 2.3 ms, fresh copy 2.5 ms, difference 0.2 ms"));
        assert!(small.contains("result: no material penalty"));
        assert!(
            !small.contains("Developer Tools"),
            "no advice below the threshold"
        );
        let negative = report(
            &full([3_000; 5], [2_000; 5]),
            None,
            &App::None,
            elapsed,
            "x",
        );
        assert!(negative.contains("difference -1.0 ms"));
        let failed = full([2_000; 5], [100_000; 5]);
        let failed = report(
            &failed,
            Some("a probe exited with exit status: 1"),
            &App::None,
            elapsed,
            "x",
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
        let partial = report(
            &partial,
            Some("the 3000 ms limit was reached"),
            &App::None,
            elapsed,
            "x",
        );
        assert!(partial.contains("fresh copies: runs 100.0 ms, 100.0 ms\n"));
        assert!(partial.contains(
            "inconclusive (the 3000 ms limit was reached); no median reported; run it again"
        ));
        assert!(
            !failed.contains("run it again"),
            "the hint is for the limit only"
        );
        let penalty = report(
            &full([2_300; 5], [101_600; 5]),
            None,
            &App::None,
            elapsed,
            "x",
        );
        assert!(penalty.contains(
            "result: penalty 99.3 ms per newly written executable, above the 30 ms threshold"
        ));
        assert!(penalty.contains("took 900 ms of a 3000 ms limit; x"));
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
