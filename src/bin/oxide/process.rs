//! Cargo as leader of its own process group, with Ctrl-C and termination
//! forwarded to the whole group.

use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

const FORWARDED: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];
const POLL: Duration = Duration::from_millis(20);
const LINGER_NOTICE: Duration = Duration::from_secs(2);

/// Signal received but not yet forwarded.
static PENDING: AtomicI32 = AtomicI32::new(0);
/// Last signal received, kept for the exit status.
static RECEIVED: AtomicI32 = AtomicI32::new(0);

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

/// A started Cargo process and the group it leads.
pub struct Group {
    pub child: Child,
    pub id: i32,
}

/// Start `command` as the leader of a new process group.
pub fn spawn_group(command: &mut Command) -> Result<Group, String> {
    let child = command
        .process_group(0)
        .spawn()
        .map_err(|error| format!("could not start Cargo: {error}"))?;
    let id = i32::try_from(child.id()).map_err(|_| "Cargo process ID is out of range")?;
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
    loop {
        forward_pending(group.id);
        if let Some(status) = group
            .child
            .try_wait()
            .map_err(|error| format!("could not wait for Cargo: {error}"))?
        {
            return Ok(status);
        }
        std::thread::sleep(POLL);
    }
}

/// Wait until no process remains in the group, forwarding signals. A
/// descendant that escaped with `setsid` is not observed.
pub fn wait_group_empty(group: i32) {
    let started = Instant::now();
    let mut noticed = false;
    while !group_empty(group) {
        forward_pending(group);
        if !noticed && started.elapsed() >= LINGER_NOTICE {
            eprintln!("oxide: waiting for processes left in Cargo's process group {group} to exit");
            noticed = true;
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

/// Cargo's exit code, or 128 + signal when the leader died on a signal.
pub fn exit_code(status: ExitStatus) -> u8 {
    match status.code() {
        Some(code) => u8::try_from(code).unwrap_or(1),
        None => signal_exit_code(status.signal().unwrap_or(0)),
    }
}
