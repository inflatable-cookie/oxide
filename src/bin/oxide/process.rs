//! Cargo as leader of its own process group, with Ctrl-C and termination
//! forwarded to the whole group.

use crate::watchdog::Watchdog;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const FORWARDED: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];
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

/// Catch SIGINT/SIGTERM/SIGHUP so Oxide can forward them and release.
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

fn signal_group(group: i32, signal: libc::c_int) {
    if group > 1 {
        // SAFETY: a negative PID addresses the group this command created.
        unsafe {
            libc::kill(-group, signal);
        }
    }
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
            signal_group(group, libc::SIGTERM);
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
}

/// Start `command` as the leader of a new process group.
pub fn spawn_group(command: &mut Command) -> Result<Group, String> {
    start_group(command).map_err(|error| format!("could not start Cargo: {error}"))
}

/// [`spawn_group`] keeping the start error, for `exec`'s exit code.
pub fn start_group(command: &mut Command) -> std::io::Result<Group> {
    let child = command.process_group(0).spawn()?;
    let id = i32::try_from(child.id()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "process ID is out of range",
        )
    })?;
    Ok(Group { child, id })
}

/// Forward any newly received signal to the group.
pub fn forward_pending(group: i32) {
    let signal = PENDING.swap(0, Ordering::SeqCst);
    if signal != 0 && group > 1 {
        // SAFETY: a negative PID addresses the process group this command
        // created; `signal` is one of the forwarded termination signals.
        unsafe {
            libc::kill(-group, signal);
        }
    }
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
        if let Some(status) = group
            .child
            .try_wait()
            .map_err(|error| format!("could not wait for Cargo: {error}"))?
        {
            return Ok(Some(status));
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
        if let Some(status) = group
            .child
            .try_wait()
            .map_err(|error| format!("could not wait for Cargo: {error}"))?
        {
            return Ok(Some(status));
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
        return Ok((start_group(command)?, None));
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
    let group = start_group(command)?;
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
        // SAFETY: SIGTTOU is ignored only around tcsetpgrp on descriptor 0,
        // then the previous disposition is restored.
        unsafe {
            let previous = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            libc::tcsetpgrp(0, group);
            libc::signal(libc::SIGTTOU, previous);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::{self, Options};

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
        let mut group = spawn_group(&mut command).expect("a child");
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

    #[test]
    fn a_live_child_is_stopped_when_the_deadline_is_due() {
        due_execution_clock();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec sleep 30"]);
        let mut group = spawn_group(&mut command).expect("a child");
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
