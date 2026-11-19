//! Bounded, read-only capacity diagnostics for a capacity refusal (oxide#130).
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md` (Capacity
//! diagnostics). After a pool-creation reserve refusal, or after a
//! host-reserve stop when every writer in Cargo's group has closed, Oxide may
//! ask macOS for its own purgeable/reclaimable estimates. The probe is a
//! separate process of the same executing artifact, inside one 2-second budget
//! that also covers a bounded KILL, its positive reap and the pipe drain, and
//! with 8 KiB of combined output.
//!
//! The figures are informational. They overlap each other, they are not
//! guaranteed free space, nothing is purged, and no decision reads them: the
//! 20 GiB raw start/abort decision is unchanged. An unsupported platform, a
//! timeout, an error, an oversized or malformed answer and an absent, negative
//! or wrongly typed value are all `unknown`. Nothing here runs on a watchdog
//! sample, on plain `oxide status` or during polling.

use crate::process;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The hidden probe child, dispatched before any other work in `main`.
pub const PROBE: &str = "__oxide-capacity-probe";
/// One budget for the whole probe: the child's answer, a bounded KILL, its
/// positive reap and the pipe drain all fit inside this window.
const LIMIT: Duration = Duration::from_secs(2);
/// Held back from [`LIMIT`] so a KILL and its positive reap cannot overrun it.
const TERMINATION_RESERVE: Duration = Duration::from_millis(250);
/// Combined stdout and stderr bound.
const OUTPUT_CAP: usize = 8 * 1024;
/// How often the wait loop observes the child.
const POLL: Duration = Duration::from_millis(10);

/// The child. Prints one typed JSON answer, or an explicit error class.
pub fn hidden() -> Option<u8> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != PROBE {
        return None;
    }
    let Some(path) = args.next().map(PathBuf::from) else {
        eprintln!("oxide: {PROBE} needs one absolute path");
        return Some(2);
    };
    if args.next().is_some() || !path.is_absolute() {
        eprintln!("oxide: {PROBE} takes one absolute path");
        return Some(2);
    }
    let answer = answer(&path);
    println!("{}", answer.to_json());
    Some(0)
}

/// One probe answer. `None` means the volume did not report that figure.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Answer {
    important: Option<u64>,
    opportunistic: Option<u64>,
    error: Option<&'static str>,
}

impl Answer {
    fn unknown(error: &'static str) -> Self {
        Self {
            important: None,
            opportunistic: None,
            error: Some(error),
        }
    }

    fn to_json(&self) -> String {
        let number = |value: Option<u64>| match value {
            Some(bytes) => bytes.to_string(),
            None => "null".to_owned(),
        };
        let error = match self.error {
            Some(error) => format!("\"{error}\""),
            None => "null".to_owned(),
        };
        format!(
            "{{\"important_usage_bytes\":{},\"opportunistic_usage_bytes\":{},\"error\":{error}}}",
            number(self.important),
            number(self.opportunistic)
        )
    }
}

fn answer(path: &Path) -> Answer {
    answer_with(path, injected_value().as_deref())
}

/// Debug builds read the injection; release builds never do.
fn injected_value() -> Option<String> {
    #[cfg(debug_assertions)]
    {
        std::env::var("OXIDE_TEST_CAPACITY_INJECT").ok()
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

fn answer_with(path: &Path, injected: Option<&str>) -> Answer {
    // Release builds never read the injection; the parameter stays in the
    // signature so one call shape serves both profiles.
    #[cfg(not(debug_assertions))]
    let _ = injected;
    #[cfg(debug_assertions)]
    if let Some(injected) = injected {
        if let Some(answer) = injected_answer(injected) {
            return answer;
        }
    }
    #[cfg(target_os = "macos")]
    {
        match macos::available(path) {
            Ok((important, opportunistic)) => Answer {
                important,
                opportunistic,
                error: None,
            },
            Err(class) => Answer::unknown(class),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Answer::unknown("unsupported-platform")
    }
}

/// Debug-only injection for the fixture proof, so a diagnostic format, an
/// error class, a hang and an oversized answer are provable without moving
/// the host's free space. Release builds ignore it.
#[cfg(debug_assertions)]
fn injected_answer(value: &str) -> Option<Answer> {
    let number = |token: &str| match token {
        "none" | "unknown" | "" => None,
        _ => token.parse::<u64>().ok(),
    };
    if let Some(rest) = value.strip_prefix("ok:") {
        let (important, opportunistic) = rest.split_once(':')?;
        return Some(Answer {
            important: number(important),
            opportunistic: number(opportunistic),
            error: None,
        });
    }
    if let Some(class) = value.strip_prefix("error:") {
        return Some(Answer::unknown(match class {
            "no-volume" => "no-volume",
            "unsupported" => "unsupported-property",
            _ => "probe-error",
        }));
    }
    match value {
        // Neither answer arrives: the parent must kill and reap the child.
        "hang" => {
            std::thread::sleep(Duration::from_secs(60));
            Some(Answer::unknown("injected-hang-finished"))
        }
        // More than the combined bound.
        "flood" => {
            let block = vec![b'x'; OUTPUT_CAP + 1];
            let mut stdout = std::io::stdout();
            let _ = std::io::Write::write_all(&mut stdout, &block);
            let _ = std::io::Write::flush(&mut stdout);
            Some(Answer::unknown("injected-flood-finished"))
        }
        _ => Some(Answer::unknown("injected-unrecognised")),
    }
}

/// One read-only diagnostic line for `path`'s filesystem, or `None` where the
/// platform has no probe (Linux keeps its raw refusal text).
pub fn diagnose(path: &Path) -> Option<String> {
    // Linux and every other platform keep their raw refusal text exactly.
    if !cfg!(target_os = "macos") {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let mut command = Command::new(exe);
    command.arg(PROBE).arg(path);
    Some(diagnose_command(&mut command))
}

/// Run one probe command under the parent's bounds. `path` only reaches the
/// real child; the command is a parameter so the bounds themselves are unit
/// testable with fixed scripts.
fn diagnose_command(command: &mut Command) -> String {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // One absolute budget, taken before the spawn, so the child's answer, a
    // KILL, its positive reap and the pipe drain all share it. Nothing below
    // adds a window of its own; each phase is given what is left.
    let deadline = Instant::now() + LIMIT;
    let answer_by = deadline - TERMINATION_RESERVE;
    // The probe is its own process group, so a bounded stop reaches exactly
    // what this run started and nothing else.
    let mut group = match process::start_group(command) {
        Ok(group) => group,
        Err(error) => return unknown_line(&format!("the read-only probe did not start ({error})")),
    };
    let Some(stdout) = group.child.stdout.take() else {
        kill_and_reap(&mut group, deadline);
        return unknown_line("the probe's output could not be read");
    };
    let Some(stderr) = group.child.stderr.take() else {
        kill_and_reap(&mut group, deadline);
        return unknown_line("the probe's output could not be read");
    };
    let (sender, answers) = std::sync::mpsc::channel();
    let stdout_sender = sender.clone();
    std::thread::spawn(move || {
        let (bytes, overflow) = read_bounded(stdout);
        let _ = stdout_sender.send((true, bytes, overflow));
    });
    let stderr_sender = sender;
    std::thread::spawn(move || {
        let (bytes, overflow) = read_bounded(stderr);
        let _ = stderr_sender.send((false, bytes, overflow));
    });
    let mut status = None;
    while Instant::now() < answer_by {
        match group.child.try_wait() {
            Ok(Some(exit)) => {
                status = Some(exit);
                break;
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(error) => {
                kill_and_reap(&mut group, deadline);
                return unknown_line(&format!("the probe could not be observed ({error})"));
            }
        }
    }
    if status.is_none() {
        let reaped = kill_and_reap(&mut group, deadline);
        return unknown_line(if reaped {
            "the probe did not answer within 2 s; it was killed and reaped inside that bound"
        } else {
            "the probe did not answer within 2 s and was not positively reaped inside that bound"
        });
    }
    // The leader is reaped. Both pipes normally reach end-of-file at once; a
    // remaining writer is read only with what is left of the same budget.
    let mut pipes = Vec::new();
    while pipes.len() < 2 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return unknown_line("the probe's output did not close inside its 2 s bound");
        }
        match answers.recv_timeout(left) {
            Ok(answer) => pipes.push(answer),
            Err(_) => return unknown_line("the probe's output did not close inside its 2 s bound"),
        }
    }
    let overflow = pipes.iter().any(|(_, _, overflow)| *overflow);
    let total: usize = pipes.iter().map(|(_, bytes, _)| bytes.len()).sum();
    if overflow || total > OUTPUT_CAP {
        return unknown_line("the probe's combined output exceeded its 8 KiB bound");
    }
    if !status.is_some_and(|status| status.success()) {
        return unknown_line("the probe failed");
    }
    let stdout = pipes
        .into_iter()
        .find_map(|(standard, bytes, _)| standard.then_some(bytes))
        .unwrap_or_default();
    match parse(&stdout) {
        Ok(answer) => answer_line(&answer),
        Err(class) => unknown_line(class),
    }
}

fn unknown_line(reason: &str) -> String {
    format!("capacity diagnostics: unknown ({reason}); the raw reserve decision is unchanged")
}

/// KILL the probe's group and positively reap its leader, using only what is
/// left of the caller's [`LIMIT`] budget. Returns whether the leader was
/// reaped; a false answer is reported as unknown, never as a clean stop.
fn kill_and_reap(group: &mut process::Group, deadline: Instant) -> bool {
    if group.id > 1 {
        // SAFETY: a negative PID addresses the process group this run
        // created; the probe is its only member.
        unsafe {
            libc::kill(-group.id, libc::SIGKILL);
        }
    }
    while Instant::now() < deadline {
        match group.child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => std::thread::sleep(POLL),
            Err(_) => return false,
        }
    }
    false
}

/// Read one pipe up to the combined bound, reporting an overflow instead of
/// buffering without limit.
fn read_bounded(mut pipe: impl Read) -> (Vec<u8>, bool) {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return (bytes, false),
            Ok(read) => {
                if bytes.len() + read > OUTPUT_CAP {
                    return (bytes, true);
                }
                bytes.extend_from_slice(&buffer[..read]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return (bytes, true),
        }
    }
}

/// A typed answer: absent, negative, malformed or wrongly typed values become
/// unknown, and an explicit error class is kept as itself.
fn parse(bytes: &[u8]) -> Result<Answer, &'static str> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "the probe printed no valid answer")?;
    let Some(object) = value.as_object() else {
        return Err("the probe printed no answer object");
    };
    if let Some(class) = object.get("error").and_then(serde_json::Value::as_str) {
        return Err(match class {
            "no-volume" => "the volume reported no capacity facts",
            "unsupported-property" => "this volume does not report the property",
            "unsupported-platform" => "this platform has no probe",
            _ => "the probe reported an error",
        });
    }
    let bytes_at = |name: &str| -> Result<Option<u64>, &'static str> {
        match object.get(name) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(value) => value
                .as_u64()
                .map(Some)
                .ok_or("a capacity figure was not a non-negative integer"),
        }
    };
    Ok(Answer {
        important: bytes_at("important_usage_bytes")?,
        opportunistic: bytes_at("opportunistic_usage_bytes")?,
        error: None,
    })
}

fn answer_line(answer: &Answer) -> String {
    let figure = |value: Option<u64>| match value {
        Some(bytes) => format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0)),
        None => "unknown".to_owned(),
    };
    format!(
        "capacity diagnostics: macOS volume resource values (CoreFoundation, read-only) — important use {}, opportunistic use {}; purgeable and reclaimable: unknown (no supported public API); these overlapping estimates are not guaranteed free space, nothing was purged, and the raw reserve decision is unchanged",
        figure(answer.important),
        figure(answer.opportunistic)
    )
}

#[cfg(target_os = "macos")]
mod macos {
    //! The smallest CoreFoundation reader for two public URL resource keys.
    //! `CFURLCopyResourcePropertyForKey` compares the key's string value, so
    //! the documented key names below are the public keys; no private
    //! selector, `NSFileManager` proxy or detached thread is involved.
    //!
    //! The framework is opened by name at probe time rather than linked into
    //! `oxide`. Linking CoreFoundation makes the dynamic loader set
    //! `__CF_USER_TEXT_ENCODING` in every `oxide` process, which would change
    //! the caller's environment for Cargo, `exec`'d tools and the exact
    //! passthrough commands; loading it only in the probe child leaves every
    //! other invocation's environment untouched.

    use std::ffi::{c_char, c_int, c_void, CString};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    type Ref = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    const POSIX_PATH_STYLE: i32 = 0;
    const SINT64: i32 = 4;
    const IMPORTANT: &str = "NSURLVolumeAvailableCapacityForImportantUsageKey";
    const OPPORTUNISTIC: &str = "NSURLVolumeAvailableCapacityForOpportunisticUsageKey";
    const FRAMEWORK: &str = "/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation";
    const RTLD_NOW: c_int = 0x2;
    const RTLD_LOCAL: c_int = 0x4;

    extern "C" {
        fn dlopen(image: *const c_char, mode: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    type CreateString = unsafe extern "C" fn(Ref, *const u8, isize, u32, u8) -> Ref;
    type CreateUrl = unsafe extern "C" fn(Ref, Ref, i32, u8) -> Ref;
    type CopyProperty = unsafe extern "C" fn(Ref, Ref, *mut Ref, *mut Ref) -> u8;
    type TypeId = unsafe extern "C" fn(Ref) -> usize;
    type TypeIdQuery = unsafe extern "C" fn() -> usize;
    type NumberValue = unsafe extern "C" fn(Ref, i32, *mut c_void) -> u8;
    type Release = unsafe extern "C" fn(Ref);

    /// The public CoreFoundation entry points this probe uses.
    struct Api {
        create_string: CreateString,
        create_url: CreateUrl,
        copy_property: CopyProperty,
        type_id: TypeId,
        number_type_id: TypeIdQuery,
        number_value: NumberValue,
        release: Release,
    }

    impl Api {
        fn load() -> Result<Self, &'static str> {
            let image = CString::new(FRAMEWORK).map_err(|_| "probe-error")?;
            // SAFETY: the path is NUL-terminated and names a system framework
            // read-only; the handle is intentionally kept for the process's
            // remaining lifetime (this probe child exits immediately).
            let handle = unsafe { dlopen(image.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
            if handle.is_null() {
                return Err("probe-error");
            }
            let symbol = |name: &'static str| -> Result<*mut c_void, &'static str> {
                let name_text = CString::new(name).map_err(|_| "probe-error")?;
                // SAFETY: the handle came from dlopen and the name is
                // NUL-terminated. A missing symbol is a null, never UB.
                let found = unsafe { dlsym(handle, name_text.as_ptr()) };
                if found.is_null() {
                    Err("unsupported-property")
                } else {
                    Ok(found)
                }
            };
            // SAFETY: each symbol is cast to the exact signature CoreFoundation
            // declares for it; dlsym returned a non-null address for that name.
            Ok(Self {
                create_string: unsafe { std::mem::transmute(symbol("CFStringCreateWithBytes")?) },
                create_url: unsafe {
                    std::mem::transmute(symbol("CFURLCreateWithFileSystemPath")?)
                },
                copy_property: unsafe {
                    std::mem::transmute(symbol("CFURLCopyResourcePropertyForKey")?)
                },
                type_id: unsafe { std::mem::transmute(symbol("CFGetTypeID")?) },
                number_type_id: unsafe { std::mem::transmute(symbol("CFNumberGetTypeID")?) },
                number_value: unsafe { std::mem::transmute(symbol("CFNumberGetValue")?) },
                release: unsafe { std::mem::transmute(symbol("CFRelease")?) },
            })
        }
    }

    /// One owned CoreFoundation object, released once.
    struct Owned {
        value: Ref,
        release: Release,
    }

    impl Owned {
        fn new(value: Ref, release: Release) -> Self {
            Self { value, release }
        }
    }

    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.value.is_null() {
                // SAFETY: `value` is a live object this module created or
                // copied, and dropping consumes it exactly once.
                unsafe { (self.release)(self.value) };
            }
        }
    }

    fn string(api: &Api, text: &str) -> Result<Owned, &'static str> {
        let length = isize::try_from(text.len()).map_err(|_| "unsupported-property")?;
        // SAFETY: the bytes are valid for `length` and CF copies them.
        let value =
            unsafe { (api.create_string)(std::ptr::null(), text.as_ptr(), length, UTF8, 0) };
        if value.is_null() {
            return Err("probe-error");
        }
        Ok(Owned::new(value, api.release))
    }

    /// `(important, opportunistic)` caller-available bytes, each absent when
    /// the volume does not report it.
    pub fn available(path: &Path) -> Result<(Option<u64>, Option<u64>), &'static str> {
        let api = Api::load()?;
        let text = std::str::from_utf8(path.as_os_str().as_bytes()).map_err(|_| "no-volume")?;
        let path = string(&api, text)?;
        // SAFETY: the path string is live and CF copies or retains it.
        let url = unsafe { (api.create_url)(std::ptr::null(), path.value, POSIX_PATH_STYLE, 0) };
        if url.is_null() {
            return Err("no-volume");
        }
        let url = Owned::new(url, api.release);
        Ok((
            property(&api, url.value, IMPORTANT)?,
            property(&api, url.value, OPPORTUNISTIC)?,
        ))
    }

    fn property(api: &Api, url: Ref, name: &'static str) -> Result<Option<u64>, &'static str> {
        let key = string(api, name)?;
        let mut value: Ref = std::ptr::null();
        let mut error: Ref = std::ptr::null();
        // SAFETY: `url` and the key are live; CF writes at most one owned
        // object into `value`, or one into `error`.
        let found = unsafe { (api.copy_property)(url, key.value, &mut value, &mut error) };
        let error = Owned::new(error, api.release);
        let value = Owned::new(value, api.release);
        if found == 0 {
            let _ = error;
            return Ok(None);
        }
        if value.value.is_null() {
            return Ok(None);
        }
        // SAFETY: both type IDs are pure queries, and the value is live.
        if unsafe { (api.type_id)(value.value) } != unsafe { (api.number_type_id)() } {
            return Ok(None);
        }
        let mut number: i64 = 0;
        // SAFETY: the value is a CFNumber and `number` is valid storage for an
        // SInt64.
        let read = unsafe {
            (api.number_value)(value.value, SINT64, std::ptr::addr_of_mut!(number).cast())
        };
        if read == 0 || number < 0 {
            return Ok(None);
        }
        Ok(u64::try_from(number).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn scratch() -> PathBuf {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = base.join(format!("oxide-capacity-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&path).expect("capacity scratch");
        path
    }

    #[test]
    fn typed_answers_and_unknowns_round_trip() {
        let complete = Answer {
            important: Some(12 * 1024 * 1024 * 1024),
            opportunistic: Some(24 * 1024 * 1024 * 1024),
            error: None,
        };
        assert_eq!(parse(complete.to_json().as_bytes()), Ok(complete.clone()));
        let line = answer_line(&complete);
        assert!(line.contains("important use 12.0 GiB"), "{line}");
        assert!(line.contains("opportunistic use 24.0 GiB"), "{line}");
        assert!(
            line.contains("not guaranteed free space")
                && line.contains("nothing was purged")
                && line.contains("purgeable and reclaimable: unknown")
                && line.contains("raw reserve decision is unchanged"),
            "{line}"
        );
        let partial = Answer {
            important: None,
            opportunistic: Some(1024),
            error: None,
        };
        assert_eq!(parse(partial.to_json().as_bytes()), Ok(partial.clone()));
        assert!(answer_line(&partial).contains("important use unknown"));
        // Absent fields, nulls, negative, fractional and stringly values are
        // unknown, never a number.
        for bytes in [
            "{}",
            "{\"important_usage_bytes\":null,\"opportunistic_usage_bytes\":null}",
            "{\"important_usage_bytes\":-1}",
            "{\"important_usage_bytes\":1.5}",
            "{\"important_usage_bytes\":\"12\"}",
            "[]",
            "not json",
        ] {
            match parse(bytes.as_bytes()) {
                Ok(answer) => {
                    assert_eq!(answer.important, None, "{bytes}");
                    assert_eq!(answer.opportunistic, None, "{bytes}");
                }
                Err(class) => assert!(
                    matches!(
                        class,
                        "a capacity figure was not a non-negative integer"
                            | "the probe printed no valid answer"
                            | "the probe printed no answer object"
                    ),
                    "{bytes}: {class}"
                ),
            }
        }
        for (class, expected) in [
            ("no-volume", "the volume reported no capacity facts"),
            (
                "unsupported-property",
                "this volume does not report the property",
            ),
            ("unsupported-platform", "this platform has no probe"),
            ("probe-error", "the probe reported an error"),
        ] {
            assert_eq!(
                parse(Answer::unknown(class).to_json().as_bytes()),
                Err(expected)
            );
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    fn injected_answers_drive_each_diagnostic_case() {
        // The injection is read in the child and passed in here, so the
        // formatting and the unknown cases are unit-testable without a
        // separate process or a mutated environment.
        let path = scratch();
        assert_eq!(
            answer_with(&path, Some("ok:1073741824:2147483648")),
            Answer {
                important: Some(1024 * 1024 * 1024),
                opportunistic: Some(2 * 1024 * 1024 * 1024),
                error: None
            }
        );
        assert_eq!(
            answer_with(&path, Some("ok:none:5")),
            Answer {
                important: None,
                opportunistic: Some(5),
                error: None
            }
        );
        assert_eq!(
            answer_with(&path, Some("error:unsupported")).error,
            Some("unsupported-property")
        );
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    /// Positive control on the real reader: this build's own CoreFoundation
    /// path answers without an error, or reports the volume does not report
    /// the property. It never panics and never invents a number.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_real_reader_answers_for_an_owned_directory() {
        let path = scratch();
        let answer = answer_with(&path, None);
        assert!(
            answer.error.is_none() || answer.error == Some("unsupported-property"),
            "{answer:?}"
        );
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn a_bounded_reader_stops_at_the_combined_cap() {
        let short = std::io::Cursor::new(vec![b'a'; 100]);
        let (bytes, overflow) = read_bounded(short);
        assert_eq!(bytes.len(), 100);
        assert!(!overflow);
        let long = std::io::Cursor::new(vec![b'a'; OUTPUT_CAP + 1]);
        let (_, overflow) = read_bounded(long);
        assert!(overflow);
    }

    fn script(source: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(source);
        command
    }

    /// The parent's bounds: a hang is killed and reaped inside the 2-second
    /// budget, a flood is capped, a typed answer is rendered, and an explicit
    /// class stays unknown. The elapsed checks are the contract: the wait, the
    /// KILL, its reap and the drain all fit inside one [`LIMIT`].
    #[test]
    fn the_probe_is_bounded_and_reported() {
        let started = Instant::now();
        let line = diagnose_command(&mut script("sleep 60"));
        let timeout_elapsed = started.elapsed();
        assert!(
            line.contains("did not answer within 2 s")
                && line.contains("killed and reaped inside that bound")
                && line.contains("the raw reserve decision is unchanged"),
            "{line}"
        );
        assert!(
            timeout_elapsed <= LIMIT + Duration::from_millis(150),
            "the timeout path took {timeout_elapsed:?}, past its {LIMIT:?} budget"
        );
        // An answer that arrives just inside the answer window still renders,
        // and the drain that follows stays inside the same budget.
        let started = Instant::now();
        let line = diagnose_command(&mut script(
            "sleep 1.4; printf '{\"important_usage_bytes\":1073741824,\"opportunistic_usage_bytes\":null}'",
        ));
        let answer_elapsed = started.elapsed();
        assert!(line.contains("important use 1.0 GiB"), "{line}");
        assert!(
            answer_elapsed <= LIMIT + Duration::from_millis(150),
            "the slow-answer path took {answer_elapsed:?}, past its {LIMIT:?} budget"
        );
        let line = diagnose_command(&mut script("printf '%020000d' 0"));
        assert!(line.contains("output exceeded its 8 KiB bound"), "{line}");
        let line = diagnose_command(&mut script(
            "printf '{\"important_usage_bytes\":1073741824,\"opportunistic_usage_bytes\":null}'",
        ));
        assert!(
            line.contains("important use 1.0 GiB") && line.contains("opportunistic use unknown"),
            "{line}"
        );
        let line = diagnose_command(&mut script(
            "printf '{\"important_usage_bytes\":null,\"opportunistic_usage_bytes\":null,\"error\":\"no-volume\"}'",
        ));
        assert!(
            line.contains("the volume reported no capacity facts"),
            "{line}"
        );
        let line = diagnose_command(&mut script("exit 3"));
        assert!(line.contains("the probe failed"), "{line}");
        let line = diagnose_command(&mut script("printf 'not json'"));
        assert!(line.contains("no valid answer"), "{line}");
    }
}
