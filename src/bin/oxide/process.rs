//! Cargo as leader of its own process group, with Ctrl-C and termination
//! forwarded to the whole group.
//!
//! A group Oxide runs in the background never reads this terminal: only a
//! group it hands the terminal's foreground to may, and a stop of that group
//! (Ctrl-Z) hands the terminal back and stops this command with it, as a shell
//! does for a stopped job.

use crate::watchdog::Watchdog;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const FORWARDED: [libc::c_int; 4] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];
/// The budget stop reuses the space watchdog's escalation allowances.
const STOP_GRACE: Duration = crate::budget::GRACE;
const STOP_CLOSE_BOUND: Duration = crate::budget::CLOSE_BOUND;
const POLL: Duration = Duration::from_millis(20);
const LINGER_NOTICE: Duration = Duration::from_secs(2);

/// Signal received but not yet forwarded.
static PENDING: AtomicI32 = AtomicI32::new(0);
/// Last signal received, kept for the exit status.
static RECEIVED: AtomicI32 = AtomicI32::new(0);
/// Wall-clock start of this process, set first thing in `main`.
static STARTED_MS: OnceLock<u64> = OnceLock::new();

/// Milliseconds since the epoch; 0 if the clock is before it.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// This process's start time, recorded with its PID as a lease holder.
pub fn started_ms() -> u64 {
    *STARTED_MS.get_or_init(now_ms)
}

/// The current boot session's start, milliseconds since the epoch, read from
/// the kernel (`kern.boottime`). `None` when the platform has no reading, the
/// call fails or the value is unusable, so a closure decision that needs it
/// stays unknown. Later clock adjustments move the kernel's boot time with
/// the clock, so it is never a stale instant from boot.
#[cfg(target_os = "macos")]
pub fn boot_ms() -> Option<u64> {
    let name = b"kern.boottime\0";
    let mut value: libc::timeval = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of_val(&value);
    // SAFETY: `name` is NUL-terminated, `value` and `length` are writable, and
    // no new value is supplied to sysctl.
    let result = unsafe {
        libc::sysctlbyname(
            name.as_ptr().cast(),
            (&mut value as *mut libc::timeval).cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 || length != std::mem::size_of_val(&value) {
        return None;
    }
    if value.tv_sec <= 0 || !(0..1_000_000).contains(&value.tv_usec) {
        return None;
    }
    let seconds = u64::try_from(value.tv_sec).ok()?;
    let micros = u64::try_from(value.tv_usec).ok()?;
    seconds
        .checked_mul(1_000)?
        .checked_add(micros / 1_000)
        .filter(|ms| *ms > 0)
}

/// The boot session is unavailable off macOS (compile-only target).
#[cfg(not(target_os = "macos"))]
pub fn boot_ms() -> Option<u64> {
    None
}

extern "C" fn record_signal(signal: libc::c_int) {
    PENDING.store(signal, Ordering::SeqCst);
    RECEIVED.store(signal, Ordering::SeqCst);
}

/// Catch SIGINT/SIGTERM/SIGHUP/SIGQUIT so Oxide can forward them and release.
pub fn install_signal_handlers() -> Result<(), String> {
    for signal in FORWARDED {
        // SAFETY: `record_signal` is an `extern "C"` process-lifetime function
        // that only stores into atomics, which is async-signal-safe.
        let previous =
            unsafe { libc::signal(signal, record_signal as *const () as libc::sighandler_t) };
        if previous == libc::SIG_ERR {
            return Err(format!(
                "could not install a signal handler: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

/// The last signal Oxide received, if any.
pub fn received_signal() -> Option<i32> {
    match RECEIVED.load(Ordering::SeqCst) {
        0 => None,
        signal => Some(signal),
    }
}

/// Exit code for a command that Oxide stopped on a signal before Cargo ran.
pub fn signal_exit_code(signal: i32) -> u8 {
    128_u8.saturating_add(u8::try_from(signal).unwrap_or(0))
}

/// Signal one process group. Nothing is sent to a group id that could name
/// this process (`<= 1`).
pub(crate) fn signal_group(group: i32, signal: libc::c_int) {
    if group > 1 {
        // SAFETY: a negative PID addresses the group this command created.
        unsafe {
            libc::kill(-group, signal);
        }
    }
}

/// Ask a group to terminate. TERM alone is not acted on by a group stopped by
/// job control until it is continued, so CONT always follows TERM.
pub fn terminate_group(group: i32) {
    signal_group(group, libc::SIGTERM);
    signal_group(group, libc::SIGCONT);
}

/// The budget stop trigger, polled by every wait loop. It is independent of
/// the space/volume watchdog: a budgeted stop happens even when no watchdog
/// exists (a `DirectoryBudget` run-program wait, for example).
#[derive(Default)]
pub struct Stop {
    term_at: Option<Instant>,
    kill_at: Option<Instant>,
}

impl Stop {
    pub fn new() -> Self {
        Self::default()
    }

    /// Latch and escalate a budget stop when its deadline has arrived. Returns
    /// `true` once the group has outlived the finite closeout allowance after
    /// KILL: the caller then keeps custody instead of claiming closure.
    pub fn poll(&mut self, group: i32) -> bool {
        let now = Instant::now();
        if self.term_at.is_none() {
            if crate::budget::take_execution_stop().is_none() {
                return false;
            }
            self.term_at = Some(now);
            terminate_group(group);
            return false;
        }
        if self.kill_at.is_none() && now.duration_since(self.term_at.unwrap()) >= STOP_GRACE {
            self.kill_at = Some(now);
            signal_group(group, libc::SIGKILL);
            return false;
        }
        self.kill_at
            .is_some_and(|kill| now.duration_since(kill) >= STOP_CLOSE_BOUND)
    }
}

/// A started Cargo process and the group it leads.
pub struct Group {
    pub child: Child,
    pub id: i32,
    /// This group owns the terminal's foreground while it runs, so its stops
    /// are this command's business.
    foreground: bool,
    /// The leader's status once a direct `waitpid` reaped it. `child` knows
    /// nothing of that reap, so it must never be waited on or signalled again:
    /// its PID may be recycled by then.
    reaped: Option<ExitStatus>,
}

/// Start `command` as the leader of a new process group.
pub fn spawn_group(command: &mut Command) -> Result<Group, String> {
    start_group(command).map_err(|error| format!("could not start Cargo: {error}"))
}

/// [`spawn_group`] keeping the start error, for `exec`'s exit code.
pub fn start_group(command: &mut Command) -> std::io::Result<Group> {
    start_group_with(command, false)
}

fn start_group_with(command: &mut Command, owns_terminal: bool) -> std::io::Result<Group> {
    if detaches_stdin(owns_terminal, stdin_is_terminal()) {
        // A background group must not read this terminal: a read there raises
        // SIGTTIN and stops the job, which is a hang when nobody owns the
        // terminal to carry on. It reads end-of-file instead.
        command.stdin(Stdio::null());
    }
    let child = command.process_group(0).spawn()?;
    let id = i32::try_from(child.id()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "process ID is out of range",
        )
    })?;
    Ok(Group {
        child,
        id,
        foreground: owns_terminal,
        reaped: None,
    })
}

/// Whether a group must read end-of-file instead of the terminal: only a group
/// that is not given the terminal's foreground, and only when standard input is
/// that terminal.
fn detaches_stdin(owns_terminal: bool, stdin_is_terminal: bool) -> bool {
    !owns_terminal && stdin_is_terminal
}

fn stdin_is_terminal() -> bool {
    // SAFETY: isatty only inspects descriptor 0.
    unsafe { libc::isatty(0) == 1 }
}

/// Forward any newly received signal to the group, then continue it: a member
/// stopped by a job-control signal must run again to act on what it was sent.
pub fn forward_pending(group: i32) {
    let signal = PENDING.swap(0, Ordering::SeqCst);
    if signal == 0 || group <= 1 {
        return;
    }
    signal_group(group, signal);
    signal_group(group, libc::SIGCONT);
}

/// What one non-blocking wait on the leader found.
enum Harvest {
    Running,
    Exited(ExitStatus),
    /// Stopped by a job-control signal, and not yet reaped.
    Stopped,
}

/// Harvest the leader without blocking or losing it.
///
/// A background group is harvested with `try_wait`: its stops are its own
/// business. A group that owns the terminal is harvested with `WUNTRACED`, so a
/// Ctrl-Z is observed instead of looking like a child that never exits.
fn harvest(group: &mut Group) -> Result<Harvest, String> {
    if let Some(status) = group.reaped {
        return Ok(Harvest::Exited(status));
    }
    if !group.foreground {
        return group
            .child
            .try_wait()
            .map(|status| status.map_or(Harvest::Running, Harvest::Exited))
            .map_err(|error| format!("could not wait for Cargo: {error}"));
    }
    let mut status = 0;
    // SAFETY: `status` is writable and `group.id` is the child this command
    // started, which nothing else in this process waits on.
    let waited = unsafe { libc::waitpid(group.id, &mut status, libc::WNOHANG | libc::WUNTRACED) };
    match waited {
        0 => Ok(Harvest::Running),
        -1 => Err(format!(
            "could not wait for Cargo: {}",
            std::io::Error::last_os_error()
        )),
        _ if libc::WIFSTOPPED(status) => Ok(Harvest::Stopped),
        _ if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) => {
            let status = ExitStatus::from_raw(status);
            group.reaped = Some(status);
            Ok(Harvest::Exited(status))
        }
        _ => Ok(Harvest::Running),
    }
}

/// Hand the terminal's foreground to `group`.
fn hand_terminal_to(group: i32) {
    // SAFETY: SIGTTOU is ignored only around tcsetpgrp on descriptor 0, then
    // the previous disposition is restored.
    unsafe {
        let previous = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
        libc::tcsetpgrp(0, group);
        libc::signal(libc::SIGTTOU, previous);
    }
}

/// A foreground group was stopped, as a shell's job is on Ctrl-Z: hand the
/// terminal back, stop with the job, and — once this command is resumed — give
/// the job the terminal again and continue it. Without this the caller's shell
/// never learns the job stopped and the terminal stays owned by a group that
/// cannot use it.
fn suspend_with_stopped_group(group: i32) {
    // SAFETY: getpgrp has no preconditions.
    hand_terminal_to(unsafe { libc::getpgrp() });
    // SAFETY: SIGSTOP cannot be caught or ignored, so the stop always lands;
    // whatever started this command resumes it with SIGCONT.
    unsafe {
        libc::raise(libc::SIGSTOP);
    }
    // Resumed: a `fg` gave this command the terminal back; a `bg` did not, and
    // then the shell keeps it and only the group continues.
    // SAFETY: tcgetpgrp and getpgrp only inspect this process and descriptor 0.
    if unsafe { libc::tcgetpgrp(0) } == unsafe { libc::getpgrp() } {
        hand_terminal_to(group);
    }
    signal_group(group, libc::SIGCONT);
}

/// Wait for the leader, forwarding signals.
pub fn wait_leader(group: &mut Group) -> Result<ExitStatus, String> {
    match wait_leader_budgeted(group, &mut Stop::new())? {
        Some(status) => Ok(status),
        None => Err("the command was stopped before its leader exited".to_owned()),
    }
}

/// [`wait_leader`] with a budget stop: `None` means the stop could not confirm
/// group closure inside its finite closeout allowance.
///
/// Each iteration harvests the leader first, then forwards a caller signal,
/// then evaluates the deadline. A child that already exited in this poll is a
/// natural completion, so a due deadline must not latch a controlled stop over
/// it (freeze A2 item 6). The deadline still applies to whatever the exit
/// leaves behind in the group, which the closeout loops observe.
pub fn wait_leader_budgeted(
    group: &mut Group,
    stop: &mut Stop,
) -> Result<Option<ExitStatus>, String> {
    loop {
        match harvest(group)? {
            Harvest::Exited(status) => return Ok(Some(status)),
            Harvest::Stopped => suspend_with_stopped_group(group.id),
            Harvest::Running => {}
        }
        forward_pending(group.id);
        if stop.poll(group.id) {
            return Ok(None);
        }
        std::thread::sleep(POLL);
    }
}

/// Wait for the leader while the watchdog samples. `None`: the watchdog
/// sent KILL and the leader still had not exited within its bound.
pub fn wait_leader_watched(
    group: &mut Group,
    watchdog: &mut Watchdog,
    stop: &mut Stop,
) -> Result<Option<ExitStatus>, String> {
    loop {
        // Same order as the unwatched wait: harvest, forward, then deadline.
        match harvest(group)? {
            Harvest::Exited(status) => return Ok(Some(status)),
            Harvest::Stopped => suspend_with_stopped_group(group.id),
            Harvest::Running => {}
        }
        forward_pending(group.id);
        if stop.poll(group.id) {
            return Ok(None);
        }
        watchdog.poll(group.id);
        if watchdog.gave_up() {
            return Ok(None);
        }
        std::thread::sleep(POLL);
    }
}

/// Wait until the group is empty while the watchdog samples. Returns when
/// it is observed empty, or `None` once a stopped group outlives the
/// watchdog's bound after KILL.
pub fn wait_group_empty_watched(
    group: i32,
    watchdog: &mut Watchdog,
    stop: &mut Stop,
) -> Option<Instant> {
    let started = Instant::now();
    let mut noticed = false;
    loop {
        if group_empty(group) {
            return Some(Instant::now());
        }
        forward_pending(group);
        if stop.poll(group) {
            return None;
        }
        watchdog.poll(group);
        if watchdog.gave_up() {
            return None;
        }
        if !noticed && !watchdog.stopped() && started.elapsed() >= LINGER_NOTICE {
            eprintln!("oxide: waiting for processes left in Cargo's process group {group} to exit");
            noticed = true;
        }
        std::thread::sleep(POLL);
    }
}

/// Wait until no process remains in the group, forwarding signals. A
/// descendant that escaped with `setsid` is not observed.
pub fn wait_group_empty(group: i32) {
    wait_group_empty_budgeted(group, &mut Stop::new());
}

/// [`wait_group_empty`] with a budget stop; `None` means the group outlived
/// the finite closeout allowance after KILL.
pub fn wait_group_empty_budgeted(group: i32, stop: &mut Stop) -> Option<Instant> {
    let started = Instant::now();
    let mut noticed = false;
    while !group_empty(group) {
        forward_pending(group);
        if stop.poll(group) {
            return None;
        }
        if !noticed && started.elapsed() >= LINGER_NOTICE {
            eprintln!("oxide: waiting for processes left in Cargo's process group {group} to exit");
            noticed = true;
        }
        std::thread::sleep(POLL);
    }
    Some(Instant::now())
}

/// Stop a group Oxide started but could not record as running. TERM and CONT
/// go first, KILL follows once the stop grace has passed, and the wait is
/// bounded: it returns `true` only when the group was observed empty. `false`
/// leaves custody with the caller, so nothing here blocks on a group that
/// survives KILL.
pub fn stop_unrecorded(group: &mut Group) -> bool {
    terminate_group(group.id);
    let started = Instant::now();
    let mut killed: Option<Instant> = None;
    loop {
        // Reaps the leader once it has exited; a stop is observed, not waited on.
        let _ = harvest(group);
        if group_empty(group.id) {
            return true;
        }
        match killed {
            None if started.elapsed() >= STOP_GRACE => {
                signal_group(group.id, libc::SIGKILL);
                killed = Some(Instant::now());
            }
            Some(at) if at.elapsed() >= STOP_CLOSE_BOUND => return false,
            _ => {}
        }
        std::thread::sleep(POLL);
    }
}

/// True only when the kernel reports no process in the group.
pub fn group_empty(group: i32) -> bool {
    if group <= 1 {
        return false;
    }
    // SAFETY: signal 0 to a negative PID only checks group existence.
    let result = unsafe { libc::kill(-group, 0) };
    result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

/// Start `command` as the leader of a new process group that owns the
/// terminal's foreground while it runs, as a shell starts a job: the program
/// reads the terminal and gets Ctrl-C itself. Without a terminal in the
/// foreground this is [`start_group`]. Drop the returned guard after the group
/// closes to take the foreground back.
pub fn start_foreground_group(
    command: &mut Command,
) -> std::io::Result<(Group, Option<Foreground>)> {
    // SAFETY: isatty, tcgetpgrp and getpgrp only inspect descriptor 0 and
    // this process.
    let foreground = unsafe { libc::isatty(0) == 1 && libc::tcgetpgrp(0) == libc::getpgrp() };
    if !foreground {
        return Ok((start_group_with(command, false)?, None));
    }
    // SAFETY: the closure runs in the child between fork and exec and only
    // calls async-signal-safe functions: the child takes the foreground for
    // its own new group (ignoring SIGTTOU for that call) before exec.
    unsafe {
        command.pre_exec(|| {
            let previous = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            libc::tcsetpgrp(0, libc::getpid());
            libc::signal(libc::SIGTTOU, previous);
            Ok(())
        });
    }
    // SAFETY: getpgrp has no preconditions.
    let previous = unsafe { libc::getpgrp() };
    let group = start_group_with(command, true)?;
    let guard = Foreground { previous };
    // The parent hands the foreground over too, so whichever runs first wins
    // the race and the program never reads the terminal from the background.
    guard.give(group.id);
    Ok((group, Some(guard)))
}

/// The terminal's foreground, handed to a program group; restored on drop.
pub struct Foreground {
    previous: i32,
}

impl Foreground {
    fn give(&self, group: i32) {
        hand_terminal_to(group);
    }
}

impl Drop for Foreground {
    fn drop(&mut self) {
        self.give(self.previous);
    }
}

/// Cargo's exit code, or 128 + signal when the leader died on a signal.
pub fn exit_code(status: ExitStatus) -> u8 {
    match status.code() {
        Some(code) => u8::try_from(code).unwrap_or(1),
        None => signal_exit_code(status.signal().unwrap_or(0)),
    }
}

/// Test-only owner of a group a test spawned. Every exit path, including a
/// panic unwinding through a failing assertion, KILLs whatever is still in the
/// group, then waits up to `OWNED_CLOSE` for the leader to be reaped and the
/// group to empty. A survivor is printed to stderr; it is never reported as
/// closed. Without the owner a failed test leaves a member such as `sleep`
/// behind, reparented to launchd and outside the runner's closure.
#[cfg(test)]
pub struct OwnedGroup(Group);

#[cfg(test)]
const OWNED_CLOSE: Duration = Duration::from_secs(5);

#[cfg(test)]
impl OwnedGroup {
    pub fn spawn(command: &mut Command) -> Self {
        Self(spawn_group(command).expect("a test group"))
    }
}

#[cfg(test)]
impl std::ops::Deref for OwnedGroup {
    type Target = Group;

    fn deref(&self) -> &Group {
        &self.0
    }
}

#[cfg(test)]
impl std::ops::DerefMut for OwnedGroup {
    fn deref_mut(&mut self) -> &mut Group {
        &mut self.0
    }
}

#[cfg(test)]
impl Drop for OwnedGroup {
    fn drop(&mut self) {
        let id = self.0.id;
        if group_empty(id) {
            // Nothing left to signal. Collect the leader if it has exited but
            // was not yet reaped; this never blocks on a live child.
            if self.0.reaped.is_none() {
                let _ = self.0.child.try_wait();
            }
            return;
        }
        signal_group(id, libc::SIGKILL);
        // One bound covers both the leader's reap and the group's emptiness.
        // A survivor is reported, never treated as closed.
        let started = Instant::now();
        let mut reaped = false;
        loop {
            reaped =
                reaped || self.0.reaped.is_some() || matches!(self.0.child.try_wait(), Ok(Some(_)));
            if (reaped && group_empty(id)) || started.elapsed() >= OWNED_CLOSE {
                break;
            }
            std::thread::sleep(POLL);
        }
        if !(reaped && group_empty(id)) {
            eprintln!(
                "oxide test: process group {id} did not close after KILL (leader reaped: {reaped})"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::{self, Options};

    #[test]
    fn a_panic_through_an_owned_group_closes_it() {
        // `sh` ignores TERM and leaves `sleep` as a member, the shape the
        // watchdog tests spawn. The panic stands for a failing assertion.
        let mut pid = 0;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let group =
                OwnedGroup::spawn(Command::new("/bin/sh").args(["-c", "trap '' TERM; sleep 30"]));
            pid = group.id;
            std::thread::sleep(Duration::from_millis(50));
            panic!("a failing assertion while the group is alive");
        }));
        assert!(outcome.is_err(), "the panic must reach the caller");
        assert!(pid > 1, "the group was spawned");
        assert!(group_empty(pid), "the owned group {pid} outlived its panic");
    }

    fn due_execution_clock() {
        budget::clear();
        budget::install(Options {
            admission: None,
            execution: Some(Duration::from_millis(1)),
            events: None,
        })
        .expect("a budget clock");
        budget::arm_execution(Instant::now(), budget::Phase::PrimaryCargo);
        std::thread::sleep(Duration::from_millis(10));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_boot_session_start_is_read_from_the_kernel() {
        let boot = boot_ms().expect("kern.boottime on macOS");
        let now = now_ms();
        assert!(boot > 0);
        assert!(boot <= now + 1_000, "boot {boot} ms is after now {now} ms");
        // The reading tracks the wall clock the rest of Oxide records, so the
        // age it reports is the uptime's order of magnitude, not a stale 1970
        // or a future instant.
        assert!(
            now - boot < 400 * 24 * 60 * 60 * 1_000,
            "boot {boot} ms is implausibly old for now {now} ms"
        );
    }

    #[test]
    fn a_child_that_already_exited_wins_over_a_due_deadline() {
        due_execution_clock();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 0"]);
        let mut group = OwnedGroup::spawn(&mut command);
        // Let the child exit so the harvest below is deterministic.
        std::thread::sleep(Duration::from_millis(100));
        let status = wait_leader_budgeted(&mut group, &mut Stop::new())
            .expect("a status")
            .expect("the natural exit must be returned, not a stop");
        assert_eq!(status.code(), Some(0));
        assert_eq!(
            budget::origin(),
            None,
            "a natural exit must not latch a stop"
        );
        budget::clear();
    }

    fn scratch(label: &str) -> std::path::PathBuf {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(format!("process-{label}-{:x}", std::process::id()));
        std::fs::create_dir(&dir).expect("scratch dir");
        dir
    }

    /// A job-control stop is observed, not mistaken for a child that never
    /// exits, and the leader is left for the ordinary reap. Counterfactual:
    /// harvesting with `try_wait` alone (no WUNTRACED) reports nothing until
    /// the child is continued, which is the terminal hang Ctrl-Z caused.
    #[test]
    fn a_stopped_foreground_leader_is_observed_without_being_reaped() {
        let mut group = OwnedGroup::spawn(Command::new("/bin/sh").args(["-c", "kill -STOP $$"]));
        // As `start_foreground_group` marks a group that owns the terminal.
        group.foreground = true;
        let deadline = Instant::now() + Duration::from_secs(5);
        let stopped = loop {
            match harvest(&mut group).expect("a wait") {
                Harvest::Stopped => break true,
                Harvest::Exited(_) => break false,
                Harvest::Running if Instant::now() >= deadline => break false,
                Harvest::Running => std::thread::sleep(POLL),
            }
        };
        assert!(stopped, "the stopped leader was never observed");
        signal_group(group.id, libc::SIGCONT);
        let status = group
            .child
            .wait()
            .expect("the leader is still this process's child to reap");
        assert!(status.success(), "the resumed leader exited {status}");
    }

    /// Forwarding a signal to a stopped group continues it, so the group acts
    /// on the signal instead of staying stopped forever.
    #[test]
    fn a_forwarded_signal_continues_a_stopped_group() {
        let dir = scratch("forward-cont");
        let report = dir.join("term");
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            &format!(
                "trap 'echo term > {}; exit 0' TERM; while :; do sleep 0.05; done",
                report.display()
            ),
        ]);
        let mut group = OwnedGroup::spawn(&mut command);
        group.foreground = true;
        // Let the shell install its trap, then stop the whole group.
        std::thread::sleep(Duration::from_millis(200));
        signal_group(group.id, libc::SIGSTOP);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match harvest(&mut group).expect("a wait") {
                Harvest::Stopped => break,
                Harvest::Exited(_) => panic!("the group exited before it was stopped"),
                Harvest::Running if Instant::now() >= deadline => {
                    panic!("the group did not stop")
                }
                Harvest::Running => std::thread::sleep(POLL),
            }
        }
        PENDING.store(libc::SIGTERM, Ordering::SeqCst);
        forward_pending(group.id);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !report.exists() && Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        assert!(
            report.exists(),
            "the stopped group never acted on the forwarded signal"
        );
        let _ = group.child.wait();
        std::fs::remove_dir_all(&dir).expect("remove scratch");
    }

    /// Only a group that is not given the terminal, and only when standard
    /// input is that terminal, reads end-of-file instead.
    #[test]
    fn only_a_background_group_on_a_terminal_detaches_its_stdin() {
        assert!(detaches_stdin(false, true));
        assert!(!detaches_stdin(true, true));
        assert!(!detaches_stdin(false, false));
        assert!(!detaches_stdin(true, false));
    }

    /// Regression: a leader reaped by the foreground harvest was waited on again
    /// through `waitpid`, which fails with ECHILD (or, once its PID is recycled,
    /// reaps an unrelated child). The recorded status must answer instead.
    #[test]
    fn a_foreground_leader_reaped_by_harvest_is_not_waited_for_again() {
        let mut group = OwnedGroup::spawn(Command::new("/bin/sh").args(["-c", "exit 3"]));
        group.foreground = true;
        let deadline = Instant::now() + Duration::from_secs(5);
        let first = loop {
            match harvest(&mut group).expect("a wait") {
                Harvest::Exited(status) => break status,
                Harvest::Stopped => panic!("the leader was not stopped"),
                Harvest::Running if Instant::now() >= deadline => panic!("no exit"),
                Harvest::Running => std::thread::sleep(POLL),
            }
        };
        assert_eq!(first.code(), Some(3));
        match harvest(&mut group) {
            Ok(Harvest::Exited(again)) => assert_eq!(again, first),
            Ok(_) => panic!("a second harvest reported a different state"),
            Err(error) => panic!("a second harvest waited on the reaped leader: {error}"),
        }
    }

    /// Regression: TERM alone does not end a group stopped by job control; the
    /// group must be continued with CONT for TERM to land.
    #[test]
    fn terminate_group_continues_a_stopped_group_so_term_lands() {
        let mut group = OwnedGroup::spawn(Command::new("/bin/sh").args(["-c", "sleep 30"]));
        std::thread::sleep(Duration::from_millis(100));
        signal_group(group.id, libc::SIGSTOP);
        std::thread::sleep(Duration::from_millis(100));
        terminate_group(group.id);
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Ok(Some(status)) = group.child.try_wait() {
                break status;
            }
            assert!(Instant::now() < deadline, "the stopped group ignored TERM");
            std::thread::sleep(POLL);
        };
        assert!(!status.success(), "the group exited cleanly: {status}");
    }

    /// Regression: an unrecorded group that ignores TERM was waited on without
    /// bound. It must be KILLed after the grace and confirmed closed.
    #[test]
    fn an_unrecorded_group_that_ignores_term_is_killed_and_confirmed_closed() {
        let mut group = OwnedGroup::spawn(
            Command::new("/bin/sh").args(["-c", "trap '' TERM; while :; do sleep 0.05; done"]),
        );
        std::thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        assert!(
            stop_unrecorded(&mut group),
            "the group was not confirmed closed"
        );
        assert!(started.elapsed() < Duration::from_secs(20));
        assert!(group_empty(group.id));
    }

    #[test]
    fn a_live_child_is_stopped_when_the_deadline_is_due() {
        due_execution_clock();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec sleep 30"]);
        let mut group = OwnedGroup::spawn(&mut command);
        let started = Instant::now();
        let status = wait_leader_budgeted(&mut group, &mut Stop::new()).expect("a result");
        // The stop is a trigger: the leader may be reaped as a signal status
        // once TERM lands, and the closeout bound only matters if it survives.
        assert_eq!(budget::origin(), Some(budget::Origin::Execution));
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the deadline did not stop the child"
        );
        let _ = status;
        let _ = group.child.wait();
        budget::clear();
    }
}
