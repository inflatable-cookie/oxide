//! Opt-in per-command admission and execution budgets (oxide#140).
//!
//! One monotonic owner holds the two deadlines for the life of a command.
//! The admission window starts as soon as the budget options have been parsed
//! and covers every preparation step up to a successful primary child start;
//! the execution window arms once, at the observed start, and then covers
//! compilation, Cargo locks, tests, drain, closeout and (for a bounded `run`)
//! the launched program. Neither window is ever re-armed.
//!
//! Expiry is a stop trigger, not a kernel guarantee: the wait loops latch the
//! origin and reuse the owned-group TERM/KILL escalation, then report a finite
//! closeout. The optional event channel carries typed records for a caller
//! that wants machine-readable facts; it never controls a timer.
//!
//! Commands without budget options behave exactly as before: every helper in
//! this module returns "unbudgeted" and the existing blocking/`output()`
//! paths are used unchanged.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::io::RawFd;
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Longest accepted duration: seven days, in milliseconds.
pub const MAX_DURATION_MS: u64 = 604_800_000;
/// Abort a preparation child this long after TERM before trying KILL.
pub const GRACE: Duration = Duration::from_secs(2);
/// Give up on a stopped group this long after KILL: custody stays held.
pub const CLOSE_BOUND: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(10);
/// One event record including its newline.
pub const EVENT_BYTES: usize = 512;
/// Refusal text for a lost event stream, printed at most once.
pub const LOSS_NOTE: &str =
    "oxide: the budget event channel became unusable; typed results are unavailable for this command";

/// The parsed budget options, kept on the invocation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Options {
    pub admission: Option<Duration>,
    pub execution: Option<Duration>,
    pub events: Option<RawFd>,
}

impl Options {
    /// Whether any budget option was given.
    pub fn any(&self) -> bool {
        self.admission.is_some() || self.execution.is_some() || self.events.is_some()
    }
}

const BUDGET_OPTIONS: [&str; 3] = [
    "--admission-budget",
    "--execution-budget",
    "--budget-events",
];

/// Split the reserved budget prefix off `args`, returning the options and the
/// arguments the command keeps. The prefix ends at the first argument that is
/// not one of Oxide's reserved names, so an external tool's own identically
/// named flag after a normal argument is never eaten.
pub fn scan(args: &[OsString]) -> Result<(Options, Vec<OsString>), String> {
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        let text = args[index].to_str().unwrap_or_default();
        let mut matched: Option<(&'static str, Option<&str>)> = None;
        for name in BUDGET_OPTIONS {
            if text == name {
                matched = Some((name, None));
                break;
            }
            if let Some(value) = text.strip_prefix(&format!("{name}=")) {
                matched = Some((name, Some(value)));
                break;
            }
        }
        let Some((name, inline)) = matched else { break };
        let value = match inline {
            Some(value) => value.to_owned(),
            None => match args.get(index + 1).and_then(|value| value.to_str()) {
                Some(value) => value.to_owned(),
                None => return Err(format!("{name} requires a value")),
            },
        };
        index += 1 + usize::from(inline.is_none());
        match name {
            "--admission-budget" => {
                if options.admission.replace(parse_duration(&value)?).is_some() {
                    return Err("--admission-budget may be given only once".to_owned());
                }
            }
            "--execution-budget" => {
                if options.execution.replace(parse_duration(&value)?).is_some() {
                    return Err("--execution-budget may be given only once".to_owned());
                }
            }
            _ => {
                let fd = parse_fd(&value)?;
                if options.events.replace(fd).is_some() {
                    return Err("--budget-events may be given only once".to_owned());
                }
            }
        }
    }
    if options.any() && options.admission.is_none() && options.execution.is_none() {
        return Err(
            "--budget-events needs --admission-budget or --execution-budget: there is nothing to report"
                .to_owned(),
        );
    }
    Ok((options, args[index..].to_vec()))
}

/// Whether `args` begins with any reserved budget name. Used before the
/// hand-off check, which runs before the full parse.
pub fn reserved_prefix(args: &[OsString]) -> bool {
    args.first()
        .and_then(|argument| argument.to_str())
        .is_some_and(|text| {
            BUDGET_OPTIONS
                .iter()
                .any(|name| text == *name || text.starts_with(&format!("{name}=")))
        })
}

/// `^[1-9][0-9]{0,8}(ms|s)$`, at most [`MAX_DURATION_MS`].
pub fn parse_duration(value: &str) -> Result<Duration, String> {
    let (digits, multiplier) = if let Some(digits) = value.strip_suffix("ms") {
        (digits, 1_u64)
    } else if let Some(digits) = value.strip_suffix('s') {
        (digits, 1000)
    } else {
        return Err(format!(
            "--*-budget {value} is not a duration; use milliseconds or seconds, for example 30s or 1500ms"
        ));
    };
    let valid = !digits.is_empty()
        && digits.len() <= 9
        && digits.starts_with(|character: char| ('1'..='9').contains(&character))
        && digits.bytes().all(|byte| byte.is_ascii_digit());
    if !valid {
        return Err(format!(
            "--*-budget {value} is not a duration; use a positive whole number of milliseconds or seconds"
        ));
    }
    let count: u64 = digits
        .parse()
        .map_err(|_| format!("--*-budget {value} is out of range"))?;
    let millis = count
        .checked_mul(multiplier)
        .filter(|millis| *millis <= MAX_DURATION_MS)
        .ok_or_else(|| {
            format!("--*-budget {value} exceeds the seven-day limit of {MAX_DURATION_MS}ms")
        })?;
    Ok(Duration::from_millis(millis))
}

fn parse_fd(value: &str) -> Result<RawFd, String> {
    let fd: i32 = value
        .parse()
        .map_err(|_| format!("--budget-events {value} is not a file descriptor number"))?;
    if !(3..=1023).contains(&fd) {
        return Err(format!(
            "--budget-events {value} is out of range; pass a writable pipe descriptor from 3 to 1023"
        ));
    }
    Ok(fd)
}

/// Where a controlled stop came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Origin {
    Admission,
    Execution,
}

impl Origin {
    /// GNU `timeout` conventions: admission 124, execution 125.
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Admission => 124,
            Self::Execution => 125,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Admission => "admission",
            Self::Execution => "execution",
        }
    }
}

/// The phase a started child belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    PrimaryCargo,
    ExecTool,
    RunProgram,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::PrimaryCargo => "primary_cargo",
            Self::ExecTool => "exec_tool",
            Self::RunProgram => "run_program",
        }
    }
}

/// Typed terminal outcomes of a budgeted command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    AdmissionDeadline,
    ExecutionDeadline,
    ChildExit,
    ChildSignal,
    SpawnFailure,
    UnconfirmedClosure,
    InternalError,
}

impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Self::AdmissionDeadline => "admission_deadline",
            Self::ExecutionDeadline => "execution_deadline",
            Self::ChildExit => "child_exit",
            Self::ChildSignal => "child_signal",
            Self::SpawnFailure => "spawn_failure",
            Self::UnconfirmedClosure => "unconfirmed_closure",
            Self::InternalError => "internal_error",
        }
    }
}

/// Whether the group and drain were observed complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Drain {
    Complete,
    Gapped,
    NotReached,
}

impl Drain {
    fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Gapped => "gapped",
            Self::NotReached => "not_reached",
        }
    }
}

/// What became of the lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lease {
    Released,
    Retired,
    Held,
}

impl Lease {
    fn label(self) -> &'static str {
        match self {
            Self::Released => "released",
            Self::Retired => "retired",
            Self::Held => "held",
        }
    }
}

struct Admission {
    limit: Duration,
    expires: Instant,
}

struct LiveExecution {
    started: Instant,
    /// The stop trigger: `now` for a late start, the armed deadline for an
    /// execution budget, and `None` for an admission-only command that started
    /// inside its window (it has no execution window at all).
    stop: Option<Instant>,
}

struct State {
    started: Instant,
    invocation: String,
    options: Options,
    admission: Option<Admission>,
    execution: Option<LiveExecution>,
    /// Latched controlled-stop origin; set once, never re-armed.
    origin: Option<Origin>,
    /// The admission deadline had already passed when the child started.
    late: bool,
    /// The budget stop has been handed to the wait loops once.
    stop_requested: bool,
    /// The observed start has been recorded once.
    armed: bool,
    events: Option<Events>,
    events_lost: bool,
    note_printed: bool,
}

thread_local! {
    static STATE: std::cell::RefCell<Option<State>> = const { std::cell::RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(Option<&mut State>) -> R) -> R {
    STATE.with(|state| f(state.borrow_mut().as_mut()))
}

/// Start the one clock owner for this command. Must run before any
/// preparation work, including the hand-off check and `run` selection.
pub fn install(options: Options) -> Result<(), String> {
    let events = match options.events {
        Some(fd) => Some(Events::open(fd)?),
        None => None,
    };
    let started = Instant::now();
    let admission = options.admission.map(|limit| Admission {
        limit,
        expires: started + limit,
    });
    STATE.with(|cell| {
        *cell.borrow_mut() = Some(State {
            started,
            invocation: format!("{}-{}", std::process::id(), crate::process::started_ms()),
            options,
            admission,
            execution: None,
            origin: None,
            late: false,
            stop_requested: false,
            armed: false,
            events,
            events_lost: false,
            note_printed: false,
        });
    });
    Ok(())
}

/// Whether an execution window was requested. Only this makes a `run` bounded.
pub fn has_execution() -> bool {
    with(|state| state.is_some_and(|state| state.options.execution.is_some()))
}

/// Milliseconds a caller asked for, for the event records (`null` = unbounded).
fn limits() -> (Option<u64>, Option<u64>) {
    with(|state| {
        state.map_or((None, None), |state| {
            (
                state.options.admission.map(millis),
                state.options.execution.map(millis),
            )
        })
    })
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The admission deadline, if one was given.
pub fn admission_deadline() -> Option<Instant> {
    with(|state| state.and_then(|state| state.admission.as_ref().map(|entry| entry.expires)))
}

/// Whether the admission window has already closed.
pub fn admission_expired() -> bool {
    admission_deadline().is_some_and(|expires| Instant::now() >= expires)
}

/// Clamp an existing inner bound to the admission window that is left.
pub fn clamp(limit: Duration) -> Duration {
    match admission_deadline() {
        Some(expires) => limit.min(expires.saturating_duration_since(Instant::now())),
        None => limit,
    }
}

/// Record the one observed child start. This runs for every budgeted command,
/// including an admission-only one: the start is what the typed event reports,
/// and a late start stops immediately even when no execution budget exists.
/// Nothing here is ever re-armed.
pub fn arm_execution(started: Instant, phase: Phase) {
    with(|state| {
        let Some(state) = state else { return };
        if state.armed {
            return;
        }
        state.armed = true;
        let late = state
            .admission
            .as_ref()
            .is_some_and(|admission| started >= admission.expires);
        state.late = late;
        let stop = match state.options.execution {
            Some(limit) => Some(if late {
                Instant::now()
            } else {
                started + limit
            }),
            // A late start with no execution axis is still a stop: the
            // admission window had already closed.
            None => late.then(Instant::now),
        };
        state.execution = Some(LiveExecution { started, stop });
        if late {
            state.origin = Some(Origin::Admission);
        }
        let admission_limit = state.admission.as_ref().map(|entry| entry.limit);
        let overshoot = state.admission.as_ref().and_then(|admission| {
            started
                .checked_duration_since(admission.expires)
                .map(millis)
        });
        events_started(
            state,
            phase,
            late,
            admission_limit,
            state.options.execution,
            overshoot,
        );
    });
}

fn events_started(
    state: &mut State,
    phase: Phase,
    late: bool,
    admission_limit: Option<Duration>,
    execution_limit: Option<Duration>,
    overshoot_ms: Option<u64>,
) {
    if state.events_lost {
        return;
    }
    let seq = next_seq(state);
    let execution_started = execution_limit.map(|_| 0_u64);
    let record = format!(
        "{{\"v\":1,\"invocation\":\"{}\",\"seq\":{},\"event\":\"admitted_child_started\",\"phase\":\"{}\",\"spawn\":true,\"spawn_late\":{},\"admission_expired_before_start\":{},\"admission_ms\":{},\"execution_ms\":{},\"admission_elapsed_ms\":{},\"execution_started_ms\":{},\"admission_overshoot_ms\":{}}}",
        state.invocation,
        seq,
        phase.label(),
        late,
        late,
        optional(admission_limit.map(millis)),
        optional(execution_limit.map(millis)),
        millis(state.started.elapsed()),
        optional(execution_started),
        optional(overshoot_ms),
    );
    emit(state, &record);
}

fn next_seq(state: &mut State) -> u64 {
    let events = state.events.as_mut();
    match events {
        Some(events) => {
            events.seq += 1;
            events.seq
        }
        None => 0,
    }
}

fn optional(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}

fn emit(state: &mut State, record: &str) {
    if state.events_lost {
        // Sticky: once the stream is lost nothing more is written, so a later
        // terminal record can never land after a lost earlier one.
        return;
    }
    let lost_before = state.events_lost;
    let lost = match state.events.as_mut() {
        Some(events) => !events.write(record),
        None => return,
    };
    if lost {
        state.events_lost = true;
        if !lost_before && !state.note_printed {
            state.note_printed = true;
            eprintln!("{LOSS_NOTE}");
        }
    }
}

/// The one-shot execution stop trigger, or `None` while the window runs or
/// after the stop has been latched.
pub fn take_execution_stop() -> Option<Origin> {
    with(|state| {
        let state = state?;
        if state.stop_requested {
            return None;
        }
        let live = state.execution.as_ref()?;
        let stop = live.stop?;
        if Instant::now() < stop {
            return None;
        }
        state.stop_requested = true;
        let origin = if state.late {
            Origin::Admission
        } else {
            Origin::Execution
        };
        state.origin.get_or_insert(origin);
        // A late start already latched the admission origin in `arm_execution`;
        // the stop request must still reach the wait loops exactly once.
        state.origin
    })
}

/// Latch a controlled-stop origin from outside the clock (a signal-driven
/// closeout keeps its own precedence and does not call this).
pub fn latch(origin: Origin) {
    with(|state| {
        if let Some(state) = state {
            state.origin.get_or_insert(origin);
        }
    });
}

/// The latched origin, if a controlled stop happened.
pub fn origin() -> Option<Origin> {
    with(|state| state.and_then(|state| state.origin))
}

/// Record the program-start phase of a bounded `run`. It reports the running
/// clock and never arms or resets anything: `execution_started_ms` counts from
/// the one observed start, and the admission overshoot is null because no new
/// admission decision happens here.
pub fn program_started() {
    with(|state| {
        let Some(state) = state else { return };
        if state.events_lost {
            return;
        }
        let seq = next_seq(state);
        let admission_limit = state.admission.as_ref().map(|entry| entry.limit);
        let execution_limit = state.options.execution.map(millis);
        let execution_started_ms = state
            .execution
            .as_ref()
            .map(|live| millis(live.started.elapsed()));
        let record = format!(
            "{{\"v\":1,\"invocation\":\"{}\",\"seq\":{},\"event\":\"admitted_child_started\",\"phase\":\"{}\",\"spawn\":true,\"spawn_late\":false,\"admission_expired_before_start\":false,\"admission_ms\":{},\"execution_ms\":{},\"admission_elapsed_ms\":{},\"execution_started_ms\":{},\"admission_overshoot_ms\":null}}",
            state.invocation,
            seq,
            Phase::RunProgram.label(),
            optional(admission_limit.map(millis)),
            optional(execution_limit),
            millis(state.started.elapsed()),
            optional(execution_started_ms),
        );
        emit(state, &record);
    });
}

/// Emit the terminal record. Best effort: never blocks, never decides.
#[allow(clippy::too_many_arguments)]
pub fn finished(
    outcome: Outcome,
    child: Option<ExitStatus>,
    execution_started: Option<Instant>,
    drain: Drain,
    lease: Lease,
    exit_code: u8,
) {
    with(|state| {
        let Some(state) = state else { return };
        if state.events_lost {
            return;
        }
        let signal = child.and_then(|status| status.signal());
        let code = child.and_then(|status| status.code());
        let seq = next_seq(state);
        let record = format!(
            "{{\"v\":1,\"invocation\":\"{}\",\"seq\":{},\"event\":\"finished\",\"outcome\":\"{}\",\"stop_origin\":{},\"child_exit_code\":{},\"child_signal\":{},\"admission_ms\":{},\"execution_ms\":{},\"admission_elapsed_ms\":{},\"execution_elapsed_ms\":{},\"drain\":\"{}\",\"lease\":\"{}\",\"exit_code\":{}}}",
            state.invocation,
            seq,
            outcome.label(),
            state
                .origin
                .map_or_else(|| "null".to_owned(), |origin| format!("\"{}\"", origin.label())),
            optional(code.and_then(|code| u32::try_from(code).ok()).map(u64::from)),
            optional(signal.map(|signal| u64::try_from(signal).unwrap_or(0))),
            optional(state.options.admission.map(millis)),
            optional(state.options.execution.map(millis)),
            millis(state.started.elapsed()),
            optional(
                execution_started
                    .map(|started| millis(Instant::now().saturating_duration_since(started)))
                    .or_else(|| state.execution.as_ref().map(|live| millis(live.started.elapsed()))),
            ),
            drain.label(),
            lease.label(),
            exit_code,
        );
        emit(state, &record);
    });
}

/// The debug-only proof control for a group that never closes.
pub const CLOSURE_HOOK: &str = "OXIDE_TEST_CLOSURE_STUCK";

/// Whether the debug closeout hook asks this run to report an unconfirmed
/// group. Release builds have no hook and always answer `false`.
pub fn closure_stuck() -> bool {
    #[cfg(debug_assertions)]
    {
        std::env::var_os(CLOSURE_HOOK).is_some()
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

/// The debug-only proof control for the in-flight spawn window: a delay placed
/// after the last admission check and before the launch, so the proof can force
/// an observable late start. Release builds have no hook.
pub const SPAWN_DELAY_HOOK: &str = "OXIDE_TEST_SPAWN_DELAY_MS";

/// Milliseconds to wait just before the launch attempt (debug builds only).
pub fn spawn_delay() -> Option<Duration> {
    #[cfg(debug_assertions)]
    {
        let value = std::env::var_os(SPAWN_DELAY_HOOK)?;
        let millis = value.to_str()?.parse::<u64>().ok()?;
        Some(Duration::from_millis(millis))
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

/// A build without the debug hook refuses the variable whenever it is present,
/// rather than silently ignoring a proof control.
pub fn refuse_absent_test_hook() -> Result<(), String> {
    #[cfg(debug_assertions)]
    {
        let _ = (CLOSURE_HOOK, SPAWN_DELAY_HOOK);
        Ok(())
    }
    #[cfg(not(debug_assertions))]
    {
        for hook in [CLOSURE_HOOK, SPAWN_DELAY_HOOK] {
            if std::env::var_os(hook).is_some() {
                return Err(format!(
                    "{hook} is a debug-only proof control; this build has no such hook"
                ));
            }
        }
        Ok(())
    }
}

/// KILL one preparation child and reap it inside a finite bound.
pub fn kill_and_reap(child: &mut std::process::Child) -> bool {
    let _ = child.kill();
    let deadline = Instant::now() + CLOSE_BOUND;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => std::thread::sleep(POLL),
            Err(_) => return false,
        }
    }
    false
}

/// Final exit code for this command. A latched deadline origin wins over the
/// child status it induced (TERM 143, KILL 137 or even 0); a child that ended
/// 124/125 on its own keeps `child_exit` because nothing latched.
pub fn exit_code(child: u8) -> u8 {
    origin().map_or(child, Origin::exit_code)
}

/// The process is being stopped for a budget reason: one bounded human line,
/// printed only when the command is budgeted.
pub fn report_stop(origin: Origin) {
    let (admission, execution) = limits();
    match origin {
        Origin::Admission => eprintln!(
            "oxide: stopped by the admission budget ({}ms) before a usable child start",
            admission.unwrap_or(0)
        ),
        Origin::Execution => eprintln!(
            "oxide: stopped by the execution budget ({}ms)",
            execution.unwrap_or(0)
        ),
    }
}

// ---------------------------------------------------------------------------
// Event channel

/// The caller's pipe, duplicated once.
struct Events {
    fd: RawFd,
    seq: u64,
}

impl Events {
    /// Validate the caller's descriptor before any store effect, then take a
    /// private close-on-exec duplicate. Nothing here mutates shared open-file
    /// description flags or the parent's descriptor table.
    fn open(fd: RawFd) -> Result<Self, String> {
        if !sigpipe_ignored() {
            return Err(
                "refusing --budget-events: this runtime does not ignore SIGPIPE in the parent, so a broken reader would kill oxide; the typed channel needs the pinned 1.97.1 disposition"
                    .to_owned(),
            );
        }
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `stat` is a live, writable struct.
        if unsafe { libc::fstat(fd, &mut stat) } != 0 {
            return Err(format!(
                "--budget-events {fd} is not an open descriptor ({}); pass a writable pipe",
                std::io::Error::last_os_error()
            ));
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFIFO {
            return Err(format!(
                "--budget-events {fd} is not a pipe; only an already-nonblocking writable pipe write end is accepted"
            ));
        }
        // SAFETY: F_GETFL only reads the descriptor's status flags.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(format!("--budget-events {fd} cannot be inspected"));
        }
        let access = flags & libc::O_ACCMODE;
        if access != libc::O_WRONLY && access != libc::O_RDWR {
            return Err(format!(
                "--budget-events {fd} is not writable; pass the write end of a pipe"
            ));
        }
        if flags & libc::O_NONBLOCK == 0 {
            return Err(format!(
                "--budget-events {fd} is not nonblocking; open the pipe with O_NONBLOCK (oxide never changes shared descriptor flags)"
            ));
        }
        // SAFETY: F_GETFD only reads this descriptor.
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
            return Err(format!("--budget-events {fd} cannot be inspected"));
        }
        // SAFETY: F_DUPFD_CLOEXEC returns a new, private descriptor.
        let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        if duplicate < 0 {
            return Err(format!(
                "--budget-events {fd} could not be duplicated: {}",
                std::io::Error::last_os_error()
            ));
        }
        // The inherited descriptor is close-on-exec in this process only, so
        // neither Cargo nor an exec tool can hold the channel open. The flag
        // is per descriptor; the caller's own table is not walked.
        // SAFETY: F_SETFD on a descriptor we were given.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        Ok(Self {
            fd: duplicate,
            seq: 0,
        })
    }

    /// One `write(2)` of one bounded record. Never retries, never waits.
    fn write(&mut self, record: &str) -> bool {
        let mut line = record.as_bytes().to_vec();
        line.push(b'\n');
        if line.len() > EVENT_BYTES {
            return false;
        }
        // SAFETY: `line` is live for the call and `fd` is our private duplicate.
        let written =
            unsafe { libc::write(self.fd, line.as_ptr().cast::<libc::c_void>(), line.len()) };
        written == isize::try_from(line.len()).unwrap_or(-1)
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        // SAFETY: closing our own duplicate.
        unsafe { libc::close(self.fd) };
    }
}

/// Whether this process ignores SIGPIPE, as pinned Rust 1.97.1 `std` does at
/// startup. Read-only query: no disposition is installed or changed.
pub fn sigpipe_ignored() -> bool {
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: a null request with a valid output pointer only reads.
    if unsafe { libc::sigaction(libc::SIGPIPE, std::ptr::null(), &mut action) } != 0 {
        return false;
    }
    action.sa_sigaction == libc::SIG_IGN
}

// ---------------------------------------------------------------------------
// Locking under a deadline

/// Take an exclusive `flock`, honouring the admission window. Unbudgeted
/// commands keep the existing blocking call; budgeted ones poll the existing
/// nonblocking primitive so an expired window never waits forever.
pub fn lock(file: &std::fs::File, what: &str) -> Result<(), String> {
    match admission_deadline() {
        None => crate::store::flock_blocking(file),
        Some(_) => loop {
            if crate::store::flock_nonblocking(file)? {
                return Ok(());
            }
            if admission_expired() {
                return Err(format!(
                    "the admission budget expired while waiting for the {what} lock"
                ));
            }
            std::thread::sleep(POLL);
        },
    }
}

// ---------------------------------------------------------------------------
// Bounded preparation children

/// One finished child, with its captured streams.
#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

const OUTPUT_CAP: u64 = 16 * 1024 * 1024;

/// Run a preparation child bounded by the remaining admission window. Without
/// a budget this is exactly `Command::output()`. With one, the child runs as
/// its own group so a stop reaches exactly what this step started, then TERM,
/// grace, KILL, positive reap, reader settlement and group closure all fit
/// inside the same window.
pub fn output(command: &mut Command) -> std::io::Result<Output> {
    run(command, None)
}

/// [`output`] with bytes fed to the child's standard input.
pub fn output_with_stdin(command: &mut Command, input: Vec<u8>) -> std::io::Result<Output> {
    run(command, Some(input))
}

fn run(command: &mut Command, input: Option<Vec<u8>>) -> std::io::Result<Output> {
    let deadline = admission_deadline();
    if deadline.is_none() {
        if let Some(input) = input {
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = command.spawn()?;
            if let Some(mut stdin) = child.stdin.take() {
                std::thread::spawn(move || {
                    let _ = stdin.write_all(&input);
                });
            }
            let output = child.wait_with_output()?;
            return Ok(Output {
                status: output.status,
                stdout: output.stdout,
                stderr: output.stderr,
            });
        }
        let output = command.output()?;
        return Ok(Output {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        });
    }
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.process_group(0).spawn()?;
    if let Some(bytes) = input {
        if let Some(mut stdin) = child.stdin.take() {
            std::thread::spawn(move || {
                let _ = stdin.write_all(&bytes);
            });
        }
    }
    let id = i32::try_from(child.id()).unwrap_or(0);
    let streams: [(u8, Option<Box<dyn Read + Send>>); 2] = [
        (
            0,
            child
                .stdout
                .take()
                .map(|stream| Box::new(stream) as Box<dyn Read + Send>),
        ),
        (
            1,
            child
                .stderr
                .take()
                .map(|stream| Box::new(stream) as Box<dyn Read + Send>),
        ),
    ];
    let (sender, receiver) = mpsc::channel();
    for (which, stream) in streams {
        let sender = sender.clone();
        match stream {
            Some(stream) => {
                std::thread::spawn(move || {
                    let _ = sender.send((which, read_capped(stream)));
                });
            }
            None => {
                let _ = sender.send((which, Err("the child stream was not captured".to_owned())));
            }
        }
    }
    drop(sender);

    let mut status = None;
    let mut term_at = None;
    let mut kill_at = None;
    let mut gave_up = false;
    while status.is_none() {
        if let Some(status_now) = child.try_wait()? {
            status = Some(status_now);
            break;
        }
        let now = Instant::now();
        if now >= deadline.unwrap() && term_at.is_none() {
            term_at = Some(now);
            signal_group(id, libc::SIGTERM);
        }
        if let Some(term) = term_at {
            if now.duration_since(term) >= GRACE && kill_at.is_none() {
                kill_at = Some(now);
                signal_group(id, libc::SIGKILL);
            }
        }
        if let Some(kill) = kill_at {
            if now.duration_since(kill) >= CLOSE_BOUND {
                gave_up = true;
                break;
            }
        }
        std::thread::sleep(POLL);
    }
    if gave_up {
        signal_group(id, libc::SIGKILL);
        let _ = child.kill();
        return Err(std::io::Error::other(format!(
            "the preparation step did not close inside the admission budget and could not be positively reaped (group {id})"
        )));
    }
    let status = status.expect("a status was observed");
    if let Some(term) = term_at {
        // The child only ended because the admission window closed: report the
        // step as stopped rather than as its (induced) signal status.
        kill_group(id);
        return Err(std::io::Error::other(format!(
            "the preparation step was stopped after {}ms of its admission budget",
            Instant::now().saturating_duration_since(term).as_millis()
        )));
    }
    let mut buffer = [None, None];
    let mut settled = 0;
    let drain_deadline = deadline.unwrap() + CLOSE_BOUND;
    while settled < 2 {
        let left = drain_deadline.saturating_duration_since(Instant::now());
        let wait = if left.is_zero() {
            Duration::from_millis(1)
        } else {
            left
        };
        match receiver.recv_timeout(wait) {
            Ok((which, Ok(bytes))) => {
                buffer[usize::from(which)] = Some(bytes);
                settled += 1;
            }
            Ok((_, Err(_))) => settled += 1,
            Err(_) => {
                return Err(std::io::Error::other(format!(
                    "the preparation step's output did not settle inside the admission budget (group {id})"
                )))
            }
        }
    }
    if !group_empty(id) {
        return Err(std::io::Error::other(format!(
            "the preparation step's process group {id} did not close inside the admission budget"
        )));
    }
    Ok(Output {
        status,
        stdout: buffer[0].take().unwrap_or_default(),
        stderr: buffer[1].take().unwrap_or_default(),
    })
}

fn read_capped(mut stream: impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(bytes),
            Ok(read) => {
                if bytes.len() as u64 + read as u64 > OUTPUT_CAP {
                    return Err("the child's output exceeded its byte cap".to_owned());
                }
                bytes.extend_from_slice(&chunk[..read]);
            }
            Err(error) => return Err(format!("cannot read the child's output: {error}")),
        }
    }
}

fn kill_group(group: i32) {
    signal_group(group, libc::SIGKILL);
}

fn signal_group(group: i32, signal: libc::c_int) {
    if group > 1 {
        // SAFETY: a negative PID addresses the group this step created.
        unsafe {
            libc::kill(-group, signal);
        }
    }
}

fn group_empty(group: i32) -> bool {
    if group <= 1 {
        return false;
    }
    // SAFETY: signal 0 to a negative PID only checks group existence.
    let result = unsafe { libc::kill(-group, 0) };
    result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

/// Test helper: forget the clock (tests in other modules use the same owner).
#[cfg(test)]
pub(crate) fn clear() {
    STATE.with(|cell| *cell.borrow_mut() = None);
}

/// Test helper: the raw duplicate descriptor, for loss tests.
#[cfg(test)]
pub(crate) fn events_fd() -> Option<i32> {
    with(|state| state.and_then(|state| state.events.as_ref().map(|events| events.fd)))
}

/// Test helper: whether the stream is disabled.
#[cfg(test)]
pub(crate) fn events_lost() -> bool {
    with(|state| state.is_some_and(|state| state.events_lost))
}

/// Test helper: the current sequence number.
#[cfg(test)]
pub(crate) fn events_seq() -> u64 {
    with(|state| {
        state
            .and_then(|state| state.events.as_ref().map(|events| events.seq))
            .unwrap_or(0)
    })
}

/// Test helper: mark the stream lost, as a failed write would.
#[cfg(test)]
pub(crate) fn force_loss() {
    with(|state| {
        if let Some(state) = state {
            state.events_lost = true;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::io::AsRawFd;

    fn clear() {
        super::STATE.with(|cell| *cell.borrow_mut() = None);
    }

    fn budgeted() -> bool {
        with(|state| state.is_some_and(|state| state.options.any()))
    }

    fn late_start() -> bool {
        with(|state| state.is_some_and(|state| state.late))
    }

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn guard(options: Options) {
        clear();
        install(options).expect("a clock");
    }

    fn reset() {
        clear();
    }

    #[test]
    fn durations_accept_only_bounded_positive_whole_units() {
        assert_eq!(
            parse_duration("1500ms").unwrap(),
            Duration::from_millis(1500)
        );
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(
            parse_duration("604800s").unwrap(),
            Duration::from_millis(MAX_DURATION_MS)
        );
        for refused in [
            "0ms",
            "0s",
            "1.5s",
            "1m",
            "1h30m",
            "s",
            "ms",
            "-1s",
            "01s",
            "604801s",
            "999999999s",
            "1S",
            "",
        ] {
            assert!(parse_duration(refused).is_err(), "{refused} must refuse");
        }
        // Nine digits of seconds overflow the seven-day cap; nine of ms do not.
        assert!(parse_duration("999999999ms").is_err());
    }

    #[test]
    fn options_are_independently_optional_and_events_need_one_budget() {
        let (options, rest) = scan(&args(&["--admission-budget", "5s", "--", "x"])).unwrap();
        assert_eq!(options.admission, Some(Duration::from_secs(5)));
        assert_eq!(options.execution, None);
        assert_eq!(rest, args(&["--", "x"]));

        let (options, rest) = scan(&args(&["--execution-budget=2s", "check"])).unwrap();
        assert_eq!(options.execution, Some(Duration::from_secs(2)));
        assert_eq!(rest, args(&["check"]));

        assert!(scan(&args(&["--budget-events", "3"])).is_err());
        let (options, rest) = scan(&args(&[
            "--admission-budget",
            "1s",
            "--execution-budget",
            "1s",
            "--budget-events=3",
        ]))
        .unwrap();
        assert_eq!(options.events, Some(3));
        assert!(rest.is_empty());
    }

    #[test]
    fn scan_stops_at_the_first_non_reserved_argument() {
        let (options, rest) =
            scan(&args(&["--profile", "debug", "--admission-budget", "1s"])).unwrap();
        assert!(!options.any());
        assert_eq!(
            rest,
            args(&["--profile", "debug", "--admission-budget", "1s"])
        );

        let (options, rest) = scan(&args(&[
            "--admission-budget",
            "1s",
            "build",
            "--execution-budget",
            "9s",
        ]))
        .unwrap();
        assert_eq!(options.admission, Some(Duration::from_secs(1)));
        assert_eq!(options.execution, None);
        assert_eq!(rest, args(&["build", "--execution-budget", "9s"]));
    }

    #[test]
    fn malformed_and_duplicate_options_refuse() {
        for bad in [
            args(&["--admission-budget"]),
            args(&["--admission-budget="]),
            args(&["--execution-budget", "0s"]),
            args(&["--budget-events", "1"]),
            args(&["--budget-events", "1024"]),
            args(&["--admission-budget", "1s", "--admission-budget", "2s"]),
        ] {
            assert!(scan(&bad).is_err(), "{bad:?} must refuse");
        }
        assert!(reserved_prefix(&args(&["--execution-budget=1s", "x"])));
        assert!(!reserved_prefix(&args(&["check"])));
    }

    #[test]
    fn admission_clamps_and_never_arms_on_its_own() {
        reset();
        assert!(!budgeted());
        guard(Options {
            admission: Some(Duration::from_secs(30)),
            execution: None,
            events: None,
        });
        assert!(budgeted());
        assert!(!admission_expired());
        assert!(clamp(Duration::from_secs(60)) <= Duration::from_secs(30));
        assert_eq!(take_execution_stop(), None);
        assert_eq!(origin(), None);
        reset();
    }

    #[test]
    fn execution_arms_once_and_a_late_start_latches_admission() {
        reset();
        guard(Options {
            admission: Some(Duration::from_millis(1)),
            execution: Some(Duration::from_secs(30)),
            events: None,
        });
        std::thread::sleep(Duration::from_millis(5));
        assert!(admission_expired());
        let started = Instant::now();
        arm_execution(started, Phase::PrimaryCargo);
        assert!(late_start());
        assert_eq!(origin(), Some(Origin::Admission));
        assert_eq!(exit_code(0), 124);
        // The late start must still REQUEST a stop for the wait loops: a
        // latched origin alone never TERMs the group.
        assert_eq!(take_execution_stop(), Some(Origin::Admission));
        assert_eq!(take_execution_stop(), None);
        // A second arm cannot grant a new window.
        arm_execution(Instant::now(), Phase::PrimaryCargo);
        assert_eq!(take_execution_stop(), None);
        reset();
    }

    fn drain_records(reader: &mut std::fs::File) -> Vec<serde_json::Value> {
        let mut text = String::new();
        let mut chunk = [0_u8; 1024];
        for _ in 0..50 {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => text.push_str(&String::from_utf8_lossy(&chunk[..read])),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("{error}"),
            }
        }
        text.lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    #[test]
    fn started_records_carry_real_elapsed_and_nullable_budget_fields() {
        let (mut reader, writer) = pipe(true);
        reset();
        guard(Options {
            admission: None,
            execution: Some(Duration::from_secs(30)),
            events: Some(writer.as_raw_fd()),
        });
        std::thread::sleep(Duration::from_millis(20));
        arm_execution(Instant::now(), Phase::PrimaryCargo);
        let records = drain_records(&mut reader);
        assert_eq!(records.len(), 1);
        let started = &records[0];
        assert_eq!(started["event"], "admitted_child_started");
        assert!(started["admission_ms"].is_null());
        assert_eq!(started["execution_ms"], 30_000);
        // The elapsed admission fact is measured, not the caller's limit.
        let elapsed = started["admission_elapsed_ms"].as_u64().unwrap();
        assert!(elapsed >= 20, "elapsed {elapsed} must include the wait");
        assert!(elapsed < 30_000);
        assert_eq!(started["admission_overshoot_ms"], serde_json::Value::Null);
        assert_eq!(started["spawn_late"], false);
        reset();
    }

    #[test]
    fn program_started_reports_the_running_clock_without_resetting_it() {
        let (mut reader, writer) = pipe(true);
        reset();
        guard(Options {
            admission: Some(Duration::from_secs(30)),
            execution: Some(Duration::from_secs(30)),
            events: Some(writer.as_raw_fd()),
        });
        arm_execution(Instant::now(), Phase::PrimaryCargo);
        std::thread::sleep(Duration::from_millis(20));
        program_started();
        let records = drain_records(&mut reader);
        assert_eq!(records.len(), 2);
        let program = &records[1];
        assert_eq!(program["phase"], "run_program");
        assert_eq!(program["admission_overshoot_ms"], serde_json::Value::Null);
        // The one execution window is unchanged and already running.
        assert_eq!(program["execution_ms"], 30_000);
        assert!(program["execution_started_ms"].as_u64().unwrap() >= 20);
        assert!(program["admission_elapsed_ms"].as_u64().unwrap() >= 20);
        reset();
    }

    #[test]
    fn the_debug_closure_hook_is_absent_unless_set() {
        reset();
        assert!(!closure_stuck());
        assert!(refuse_absent_test_hook().is_ok());
    }

    #[test]
    fn an_admission_only_late_start_still_requests_its_stop() {
        reset();
        guard(Options {
            admission: Some(Duration::from_millis(1)),
            execution: None,
            events: None,
        });
        std::thread::sleep(Duration::from_millis(5));
        arm_execution(Instant::now(), Phase::ExecTool);
        assert!(late_start());
        assert_eq!(origin(), Some(Origin::Admission));
        assert_eq!(take_execution_stop(), Some(Origin::Admission));
        assert_eq!(take_execution_stop(), None);
        assert_eq!(exit_code(0), 124);
        reset();
    }

    #[test]
    fn an_admission_only_start_emits_the_started_record_with_a_null_axis() {
        let (mut reader, writer) = pipe(true);
        reset();
        guard(Options {
            admission: Some(Duration::from_secs(30)),
            execution: None,
            events: Some(writer.as_raw_fd()),
        });
        arm_execution(Instant::now(), Phase::ExecTool);
        let records = drain_records(&mut reader);
        assert_eq!(records.len(), 1, "{records:?}");
        let started = &records[0];
        assert_eq!(started["admission_ms"], 30_000);
        assert_eq!(started["execution_ms"], serde_json::Value::Null);
        assert_eq!(started["execution_started_ms"], serde_json::Value::Null);
        assert_eq!(started["spawn_late"], false);
        // No execution axis means no execution stop can fire.
        assert_eq!(take_execution_stop(), None);
        reset();
    }

    #[test]
    fn a_lost_stream_never_writes_again_or_advances_its_sequence() {
        reset();
        guard(Options {
            admission: Some(Duration::from_secs(1)),
            execution: Some(Duration::from_secs(1)),
            events: None,
        });
        force_loss();
        let before = events_seq();
        arm_execution(Instant::now(), Phase::PrimaryCargo);
        finished(
            Outcome::ChildExit,
            None,
            None,
            Drain::Complete,
            Lease::Released,
            0,
        );
        assert_eq!(
            events_seq(),
            before,
            "a suppressed record must not advance seq"
        );
        reset();
    }

    #[test]
    fn a_normal_start_keeps_the_child_status_unless_the_deadline_fires() {
        reset();
        guard(Options {
            admission: None,
            execution: Some(Duration::from_secs(30)),
            events: None,
        });
        arm_execution(Instant::now(), Phase::PrimaryCargo);
        assert!(!late_start());
        assert_eq!(origin(), None);
        assert_eq!(exit_code(125), 125);
        assert_eq!(take_execution_stop(), None);
        reset();
    }

    #[test]
    fn the_execution_trigger_latches_its_origin_once() {
        reset();
        guard(Options {
            admission: None,
            execution: Some(Duration::from_millis(1)),
            events: None,
        });
        arm_execution(Instant::now(), Phase::PrimaryCargo);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(take_execution_stop(), Some(Origin::Execution));
        assert_eq!(take_execution_stop(), None);
        assert_eq!(exit_code(143), 125);
        reset();
    }

    fn pipe(nonblocking: bool) -> (std::fs::File, std::fs::File) {
        let mut fds = [0_i32; 2];
        // SAFETY: two valid output slots.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        if nonblocking {
            // SAFETY: both descriptors are ours and valid.
            unsafe {
                libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK);
                libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK);
            }
        }
        // SAFETY: close-on-exec on our own descriptors, so a child spawned by
        // a concurrently running test cannot inherit either end of the pipe.
        unsafe {
            libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC);
        }
        use std::os::unix::io::FromRawFd;
        // SAFETY: both descriptors are owned by the returned files.
        unsafe {
            (
                std::fs::File::from_raw_fd(fds[0]),
                std::fs::File::from_raw_fd(fds[1]),
            )
        }
    }

    #[test]
    fn an_event_pipe_must_be_a_nonblocking_writable_fifo() {
        let (reader, writer) = pipe(true);
        reset();
        assert!(install(Options {
            admission: Some(Duration::from_secs(1)),
            execution: Some(Duration::from_secs(1)),
            events: Some(writer.as_raw_fd()),
        })
        .is_ok());
        reset();
        // A read end is refused.
        assert!(install(Options {
            admission: Some(Duration::from_secs(1)),
            execution: Some(Duration::from_secs(1)),
            events: Some(reader.as_raw_fd()),
        })
        .is_err());
        // A blocking pipe is refused (Oxide never sets O_NONBLOCK).
        let (_blocking_reader, blocking_writer) = pipe(false);
        assert!(install(Options {
            admission: Some(Duration::from_secs(1)),
            execution: Some(Duration::from_secs(1)),
            events: Some(blocking_writer.as_raw_fd()),
        })
        .is_err());
        // A regular file is refused.
        let path = std::env::temp_dir().join(format!("oxide-budget-{}", std::process::id()));
        let file = std::fs::File::create(&path).unwrap();
        assert!(install(Options {
            admission: Some(Duration::from_secs(1)),
            execution: Some(Duration::from_secs(1)),
            events: Some(file.as_raw_fd()),
        })
        .is_err());
        let _ = std::fs::remove_file(path);
        reset();
    }

    #[test]
    fn records_are_bounded_single_writes_and_loss_is_sticky() {
        let (mut reader, writer) = pipe(true);
        reset();
        install(Options {
            admission: Some(Duration::from_secs(1)),
            execution: Some(Duration::from_secs(1)),
            events: Some(writer.as_raw_fd()),
        })
        .unwrap();
        arm_execution(Instant::now(), Phase::PrimaryCargo);
        finished(
            Outcome::ChildExit,
            None,
            None,
            Drain::Complete,
            Lease::Released,
            0,
        );
        let mut text = String::new();
        let mut chunk = [0_u8; 1024];
        for _ in 0..50 {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => text.push_str(&String::from_utf8_lossy(&chunk[..read])),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("{error}"),
            }
        }
        assert!(
            text.contains("\"event\":\"admitted_child_started\""),
            "{text}"
        );
        assert!(text.contains("\"event\":\"finished\""), "{text}");
        assert!(text.lines().all(|line| line.len() < EVENT_BYTES));
        assert!(!events_lost());
        reset();
    }

    #[test]
    fn a_broken_reader_disables_the_stream_and_never_panics() {
        let (reader, writer) = pipe(true);
        reset();
        install(Options {
            admission: Some(Duration::from_secs(1)),
            execution: Some(Duration::from_secs(1)),
            events: Some(writer.as_raw_fd()),
        })
        .unwrap();
        let fd = events_fd().unwrap();
        use std::os::unix::fs::MetadataExt;
        let reader_info = reader.metadata().unwrap();
        let writer_info = writer.metadata().unwrap();
        drop(reader);
        arm_execution(Instant::now(), Phase::PrimaryCargo);
        let mut observation = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: one initialized descriptor row; timeout zero never waits.
        let polled = unsafe { libc::poll(&mut observation, 1, 0) };
        // SAFETY: read-only flag observations on the still-owned descriptors.
        let (duplicate_flags, writer_flags) = unsafe {
            (
                libc::fcntl(fd, libc::F_GETFD),
                libc::fcntl(writer.as_raw_fd(), libc::F_GETFD),
            )
        };
        assert!(events_lost(),
            "reader dev/inode={}/{}, writer dev/inode={}/{}, duplicate fd={}, fd_flags={}, writer_flags={}, poll={}, revents={}",
            reader_info.dev(), reader_info.ino(), writer_info.dev(), writer_info.ino(), fd,
            duplicate_flags, writer_flags, polled, observation.revents);
        assert_eq!(std::process::id(), std::process::id());
        reset();
    }

    #[test]
    fn a_full_pipe_loses_later_records_without_blocking() {
        let (_reader, writer) = pipe(true);
        reset();
        install(Options {
            admission: Some(Duration::from_secs(1)),
            execution: Some(Duration::from_secs(1)),
            events: Some(writer.as_raw_fd()),
        })
        .unwrap();
        let fd = events_fd().unwrap();
        // Fill the pipe so the next record cannot be written.
        let filler = vec![b'x'; 4096];
        loop {
            // SAFETY: writing our own duplicate; the pipe is nonblocking.
            let written =
                unsafe { libc::write(fd, filler.as_ptr().cast::<libc::c_void>(), filler.len()) };
            if written < 0 {
                break;
            }
        }
        arm_execution(Instant::now(), Phase::PrimaryCargo);
        assert!(events_lost());
        reset();
    }

    #[test]
    fn term_and_successful_child_status_never_override_a_latched_deadline() {
        reset();
        guard(Options {
            admission: None,
            execution: Some(Duration::from_millis(1)),
            events: None,
        });
        arm_execution(Instant::now(), Phase::PrimaryCargo);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(take_execution_stop(), Some(Origin::Execution));
        assert_eq!(exit_code(143), 125);
        assert_eq!(exit_code(137), 125);
        assert_eq!(exit_code(0), 125);
        reset();
        guard(Options {
            admission: Some(Duration::from_millis(1)),
            execution: None,
            events: None,
        });
        assert_eq!(exit_code(0), 0);
        reset();
    }

    #[test]
    fn bounded_output_stops_a_sleeper_at_the_admission_deadline() {
        reset();
        guard(Options {
            admission: Some(Duration::from_millis(200)),
            execution: None,
            events: None,
        });
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30"]);
        let started = Instant::now();
        let error = output(&mut command).expect_err("the sleeper must be stopped");
        assert!(started.elapsed() < Duration::from_secs(5), "{error}");
        reset();
    }

    #[test]
    fn unbudgeted_output_is_exactly_the_existing_call() {
        reset();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf ready"]);
        let result = output(&mut command).expect("a plain child");
        assert!(result.status.success());
        assert_eq!(result.stdout, b"ready");
    }
}
