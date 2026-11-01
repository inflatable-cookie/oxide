//! Bounded same-uid descriptor census for `oxide pool release-quarantined`
//! (oxide#196).
//!
//! macOS has no unprivileged primitive that proves "no process holds this
//! subtree open", so the check is an explicitly bounded *same-uid descriptor
//! and working-directory observation*: every process this account can own is
//! read, and every open vnode descriptor and current working directory is
//! compared with the slot tree by canonical path. A hold found is a refusal.
//!
//! The observation follows the writer-custody rules Barge established, because
//! the platform answers a very few reads in ways that are not "closed":
//!
//! - a process whose descriptor *listing* answers `EPERM` is re-observed by
//!   kernel identity (pid, start time and effective uid) with a short bounded
//!   backoff; only a positive fact settles it — a later listing, the kernel
//!   reporting the pid gone, a changed start identity or a changed effective
//!   uid. The same identity still answering `EPERM` is UNKNOWN and refuses;
//! - a vnode descriptor whose path and vnode facts both answer `EPERM` is the
//!   platform's own blind spot, never "closed". It is counted and reported,
//!   and does not refuse, only when the process's own `csops(CS_OPS_STATUS)`
//!   reports `CS_PLATFORM_BINARY` and its kernel start identity is positively
//!   the same before and after that status read. Every other hidden descriptor
//!   is UNKNOWN and refuses, and every readable descriptor of the same process
//!   is still checked (an in-slot one refuses). A working directory whose read
//!   answers `EPERM` is likewise hidden and always UNKNOWN/refuses;
//! - a descriptor whose two reads both answer `ENOENT` is a dead handle (a
//!   revoked or reclaimed vnode cannot do I/O): it is counted, not refused,
//!   after the same identity is re-observed and both reads answer `ENOENT`
//!   again;
//! - a descriptor the platform reports closed is never treated as a hold;
//! - a process of another effective uid is outside this cooperative check: the
//!   slot tree is the account's own `0700` directory, so another account
//!   cannot open a path inside it;
//! - the whole scan is bounded by a process count, a descriptor count, a
//!   per-process descriptor count and a wall-clock limit. Reaching any bound
//!   is UNKNOWN and refuses.
//!
//! `lsof` is never used: it is external, unbounded and its silence is not a
//! kernel guarantee.

use std::path::{Path, PathBuf};

/// A recorded holder pid that the release must find positively gone (or,
/// when the kernel's own table says the pid now names another account's
/// process, positively different). A same-account live pid is unidentifiable
/// from the record's userspace wall-clock sample and refuses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Holder {
    pub pid: u32,
}

/// What a complete census observed, for the release log line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Summary {
    pub processes: usize,
    pub descriptors: usize,
    /// Descriptors hidden by the platform on verified platform binaries and
    /// counted as the OS blind spot.
    pub hidden: usize,
    /// Descriptors whose vnode is revoked or reclaimed (a dead handle).
    pub dead: usize,
    /// What the recorded holder is now: positively gone, positively another
    /// account's process, or absent.
    pub holder: String,
}

/// A positive writer check for one slot tree. `Ok` changes nothing; `Err` is
/// the refusal and nothing has been released.
#[cfg(target_os = "macos")]
pub fn verify(slot: &Path, holder: Option<&Holder>) -> Result<Summary, String> {
    let root = std::fs::canonicalize(slot).map_err(|error| {
        format!(
            "the slot tree {} cannot be resolved ({error}); nothing was released",
            slot.display()
        )
    })?;
    let metadata = std::fs::symlink_metadata(&root).map_err(|error| {
        format!(
            "the slot tree {} cannot be inspected ({error}); nothing was released",
            root.display()
        )
    })?;
    if !metadata.is_dir() {
        return Err(format!(
            "the slot tree {} is not a directory; nothing was released",
            root.display()
        ));
    }
    let mut roots = vec![root.clone()];
    let recorded = slot.to_path_buf();
    if recorded != root {
        roots.push(recorded);
    }
    let scan = collect(&roots);
    let holder = holder_state(holder);
    verdict(&root, scan, holder)
}

/// The descriptor census is a macOS observation (the removable backend is
/// macOS-only); elsewhere the check is UNKNOWN and the release refuses.
#[cfg(not(target_os = "macos"))]
pub fn verify(slot: &Path, holder: Option<&Holder>) -> Result<Summary, String> {
    let _ = (slot, holder);
    Err("the descriptor census is available only on macOS; nothing was released".to_owned())
}

/// One complete or failed observation.
#[derive(Default)]
struct Scan {
    processes: usize,
    descriptors: usize,
    hidden: usize,
    dead: usize,
    hits: Vec<Hit>,
}

struct Hit {
    pid: i32,
    command: String,
    what: String,
}

/// The decision layer: a scan that could not complete, a positive hold, or an
/// unidentifiable live holder all refuse. Kept separate from the platform
/// reads so a stated unreadable observation is exercised by the unit suite.
fn verdict(
    root: &Path,
    scan: Result<Scan, String>,
    holder: Result<String, String>,
) -> Result<Summary, String> {
    let scan = scan.map_err(|reason| {
        format!("the descriptor census is unknown ({reason}); nothing was released")
    })?;
    let holder = holder?;
    if let Some(first) = scan.hits.first() {
        let more = scan.hits.len().saturating_sub(1);
        let suffix = if more == 0 {
            String::new()
        } else {
            format!(" (and {more} more)")
        };
        return Err(format!(
            "process {} ({}) {} inside {}; nothing was released{suffix}",
            first.pid,
            first.command,
            first.what,
            root.display()
        ));
    }
    Ok(Summary {
        processes: scan.processes,
        descriptors: scan.descriptors,
        hidden: scan.hidden,
        dead: scan.dead,
        holder,
    })
}

/// Whether the recorded holder is positively gone or positively a different
/// process. A same-account live pid is unidentifiable: the record's start is
/// a userspace wall-clock sample, not the kernel's process identity, so a
/// two-second tolerance would not distinguish the holder from a reused pid.
/// Only two positive facts pass: the kernel reports no such pid, or the pid
/// now names another account's process (which the recorded holder, an Oxide
/// process of this account, cannot be).
#[cfg(target_os = "macos")]
fn holder_state(holder: Option<&Holder>) -> Result<String, String> {
    use platform::{current_uid, process_identity};

    let Some(holder) = holder else {
        return Ok("no recorded holder".to_owned());
    };
    let pid =
        i32::try_from(holder.pid).map_err(|_| "the recorded holder pid is invalid".to_owned())?;
    let observed = process_identity(pid)?.map(|identity| (identity.uid, identity.command));
    holder_verdict(holder.pid, observed, current_uid())
}

/// The holder decision over one observed kernel identity (or its absence):
/// a gone pid and another account's process are the only positives; a
/// same-account live pid is unidentifiable and refuses. Kept separate so the
/// decision is exercised without relying on which host pids this account may
/// inspect.
fn holder_verdict(
    pid: u32,
    observed: Option<(u32, String)>,
    self_uid: u32,
) -> Result<String, String> {
    let Some((uid, command)) = observed else {
        return Ok(format!("recorded holder pid {pid} is gone"));
    };
    if uid != self_uid {
        return Ok(format!(
            "recorded holder pid {pid} now names another account's process"
        ));
    }
    Err(format!(
        "the recorded holder pid {pid} ({command}) is still running as this account's process; the record's wall-clock start cannot positively distinguish a reused pid, so the holder is unidentifiable and nothing was released"
    ))
}

#[cfg(not(target_os = "macos"))]
fn holder_state(holder: Option<&Holder>) -> Result<String, String> {
    let _ = holder;
    Err("the holder identity is available only on macOS; nothing was released".to_owned())
}

// --- macOS reads -----------------------------------------------------------

#[cfg(target_os = "macos")]
mod platform {
    use super::{Hit, Scan};
    use std::ffi::OsStr;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// The most same-uid processes one scan observes before it is bounded.
    const MAX_PROCESSES: usize = 4_096;
    /// The most descriptors one scan observes before it is bounded.
    const MAX_DESCRIPTORS: usize = 262_144;
    /// The most descriptors one process may hold before the scan is bounded.
    /// The platform's own per-process open-file limit is below this, so
    /// reaching it means the observation cannot be complete.
    const MAX_DESCRIPTORS_PER_PROCESS: usize = 65_536;
    /// The most time one scan may take before it is bounded.
    const SCAN_TIMEOUT: Duration = Duration::from_secs(15);
    /// Backoff before each re-observation of a process whose descriptor
    /// listing answered `EPERM`, and of a dead-handle candidate.
    const RETRY_BACKOFFS: [Duration; 3] = [
        Duration::from_millis(25),
        Duration::from_millis(75),
        Duration::from_millis(100),
    ];
    /// Paired re-reads of a descriptor whose two reads disagreed, including
    /// the first observation.
    const PAIR_RETRY_ATTEMPTS: usize = 3;
    const PAIR_RETRY_BACKOFF: Duration = Duration::from_millis(10);
    /// The most holds one refusal lists; the rest are counted.
    const MAX_HITS: usize = 8;

    // `sys/proc_info.h` flavours and `sys/codesign.h` flags, all stable ABI.
    const PROC_UID_ONLY: u32 = 4;
    const PROC_PIDLISTFDS: libc::c_int = 1;
    const PROC_PIDFDVNODEINFO: libc::c_int = 1;
    const PROC_PIDFDVNODEPATHINFO: libc::c_int = 2;
    const PROC_PIDTBSDINFO: libc::c_int = 3;
    const PROC_PIDVNODEPATHINFO: libc::c_int = 9;
    /// `CS_OPS_STATUS` from `sys/codesign.h`: the process's code-signing flags.
    const CS_OPS_STATUS: libc::c_uint = 0;
    /// `CS_PLATFORM_BINARY`: the flag Apple's own platform binaries carry. It
    /// is the only fact that explains a hidden descriptor path (Tom e528a712).
    pub(super) const CS_PLATFORM_BINARY: u32 = 0x0400_0000;
    const PROX_FDTYPE_VNODE: u32 = 1;

    // Sizes and field offsets read from the installed SDK's headers and
    // verified with `offsetof`: `struct proc_bsdinfo` (136),
    // `struct proc_vnodepathinfo` (2352, cwd path at 152),
    // `struct vnode_fdinfowithpath` (1200, path at 176),
    // `struct vnode_fdinfo` (176) and `struct proc_fdinfo` (8).
    const BSDINFO_SIZE: usize = 136;
    const BSDINFO_PID: usize = 12;
    const BSDINFO_UID: usize = 20;
    const BSDINFO_COMMAND: usize = 40;
    const BSDINFO_NAME: usize = 56;
    const BSDINFO_START_SEC: usize = 120;
    const BSDINFO_START_USEC: usize = 128;
    const VNODEPATHINFO_SIZE: usize = 2352;
    const VNODEPATHINFO_CDIR_PATH: usize = 152;
    const VNODE_FDINFO_SIZE: usize = 176;
    const VNODE_FDINFO_WITH_PATH_SIZE: usize = 1200;
    const VNODE_FDINFO_PATH: usize = 176;
    const PROC_FDINFO_SIZE: usize = 8;
    const MAX_PATH_BYTES: usize = 1024;

    #[link(name = "proc")]
    extern "C" {
        fn csops(
            pid: libc::c_int,
            ops: libc::c_uint,
            useraddr: *mut libc::c_void,
            usersize: libc::size_t,
        ) -> libc::c_int;
    }

    extern "C" {
        fn proc_listpids(
            kind: u32,
            typeinfo: u32,
            buffer: *mut libc::c_void,
            size: libc::c_int,
        ) -> libc::c_int;
        fn proc_pidinfo(
            pid: libc::c_int,
            flavor: libc::c_int,
            arg: u64,
            buffer: *mut libc::c_void,
            size: libc::c_int,
        ) -> libc::c_int;
        fn proc_pidfdinfo(
            pid: libc::c_int,
            descriptor: libc::c_int,
            flavor: libc::c_int,
            buffer: *mut libc::c_void,
            size: libc::c_int,
        ) -> libc::c_int;
    }

    #[derive(Clone)]
    pub(super) struct Identity {
        pub(super) pid: i32,
        pub(super) uid: u32,
        /// `pbi_start_tvsec * 1_000_000 + pbi_start_tvusec`: the kernel's
        /// start identity, never a PID-age inference.
        pub(super) start: u64,
        pub(super) command: String,
    }

    enum Listed {
        Descriptors(Vec<(i32, u32)>),
        Gone,
        Foreign,
    }

    /// The one decision for a descriptor read that returned no bytes. Zero
    /// bytes with errno zero is the positive empty listing; zero with `ESRCH`
    /// is a gone process; zero with `EPERM` is the intermittent answer the
    /// bounded identity re-observation settles; every other errno is
    /// UNKNOWN and refuses. It is never silently an empty descriptor list.
    pub(super) enum ZeroListing {
        Empty,
        Gone,
        Retry,
        Refuse(String),
    }

    pub(super) fn classify_zero_listing(pid: i32, errno: i32) -> ZeroListing {
        match errno {
            0 => ZeroListing::Empty,
            libc::ESRCH => ZeroListing::Gone,
            libc::EPERM => ZeroListing::Retry,
            _ => ZeroListing::Refuse(format!(
                "pid {pid}'s descriptor listing returned no bytes with errno {errno}; the census is unknown"
            )),
        }
    }

    #[derive(Debug)]
    pub(super) enum Cwd {
        Path(PathBuf),
        None,
    }

    #[derive(Debug)]
    pub(super) enum Observed {
        Path(PathBuf),
        Hidden,
        Dead,
        Closed,
    }

    /// The production decision for one descriptor's two reads.
    pub(super) enum Pair {
        Path(PathBuf),
        /// Both reads are platform-hidden: a blind-spot candidate that only a
        /// verified platform binary with an unchanged identity may keep.
        Opaque,
        DeadCandidate,
        Closed,
        Retry,
    }

    /// One kernel path field: a resolved path, an empty name (the vnode has no
    /// reachable name), or a field this build cannot read.
    enum PathField {
        Path(PathBuf),
        Empty,
        Malformed,
    }

    pub(super) fn current_uid() -> u32 {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() }
    }

    /// The census over the canonical slot root.
    pub(super) fn collect(roots: &[PathBuf]) -> Result<Scan, String> {
        let started = Instant::now();
        let uid = current_uid();
        let pids = scan_pids(uid)?;
        let mut scan = Scan::default();
        for pid in pids {
            if started.elapsed() >= SCAN_TIMEOUT {
                return Err("the descriptor scan exceeded its wall-clock bound".to_owned());
            }
            let Some(identity) = process_identity(pid)? else {
                continue;
            };
            if identity.uid != uid {
                // The pid now names another account's process: outside this
                // cooperative same-uid check, and unable to open the slot.
                continue;
            }
            scan.processes = scan.processes.saturating_add(1);
            if scan.processes > MAX_PROCESSES {
                return Err(format!(
                    "the descriptor scan met more than {MAX_PROCESSES} same-uid processes"
                ));
            }
            match working_directory(&identity, started)? {
                Cwd::Path(path) => {
                    if roots.iter().any(|root| path.starts_with(root)) {
                        hit(
                            &mut scan,
                            &identity,
                            format!("has its working directory at {}", path.display()),
                        );
                    }
                }
                Cwd::None => {}
            }
            let descriptors = match list_descriptors(&identity, uid, started)? {
                Listed::Descriptors(descriptors) => descriptors,
                Listed::Gone => continue,
                Listed::Foreign => continue,
            };
            scan.descriptors = scan.descriptors.saturating_add(descriptors.len());
            if scan.descriptors > MAX_DESCRIPTORS {
                return Err(format!(
                    "the descriptor scan met more than {MAX_DESCRIPTORS} descriptors"
                ));
            }
            if descriptors.len() >= MAX_DESCRIPTORS_PER_PROCESS {
                return Err(format!(
                    "pid {} holds {MAX_DESCRIPTORS_PER_PROCESS} or more descriptors",
                    identity.pid
                ));
            }
            for (descriptor, kind) in descriptors {
                if started.elapsed() >= SCAN_TIMEOUT {
                    return Err("the descriptor scan exceeded its wall-clock bound".to_owned());
                }
                if kind != PROX_FDTYPE_VNODE {
                    continue;
                }
                match observe_descriptor(&identity, descriptor, started)? {
                    Observed::Path(path) => {
                        if roots.iter().any(|root| path.starts_with(root)) {
                            hit(
                                &mut scan,
                                &identity,
                                format!("holds {} open", path.display()),
                            );
                        }
                    }
                    Observed::Hidden => scan.hidden = scan.hidden.saturating_add(1),
                    Observed::Dead => scan.dead = scan.dead.saturating_add(1),
                    Observed::Closed => {}
                }
            }
        }
        Ok(scan)
    }

    fn hit(scan: &mut Scan, identity: &Identity, what: String) {
        if scan.hits.len() < MAX_HITS {
            scan.hits.push(Hit {
                pid: identity.pid,
                command: identity.command.clone(),
                what,
            });
        }
    }

    /// The process table one scan observes: the kernel's same-uid table, or
    /// (in the unit suite only) a stated table whose real descriptor and
    /// working-directory reads are still this build's. The production path
    /// states nothing.
    fn scan_pids(uid: u32) -> Result<Vec<i32>, String> {
        #[cfg(test)]
        {
            if let Some(pids) = STATED_PROCESSES.with(|stated| stated.borrow().clone()) {
                return Ok(pids);
            }
        }
        same_uid_pids(uid)
    }

    #[cfg(test)]
    thread_local! {
        static STATED_PROCESSES: std::cell::RefCell<Option<Vec<i32>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// Test-only: run one census over a stated pid set. The real kernel
    /// identity, descriptor, path and working-directory reads still run for
    /// those pids, so a fixture writer is observed exactly as production
    /// would observe it; only the table membership is stated. The full table
    /// is restored even when the closure panics.
    #[cfg(test)]
    pub(super) fn with_stated_processes<T>(pids: Vec<i32>, run: impl FnOnce() -> T) -> T {
        struct Clear;
        impl Drop for Clear {
            fn drop(&mut self) {
                STATED_PROCESSES.with(|stated| *stated.borrow_mut() = None);
            }
        }
        STATED_PROCESSES.with(|stated| *stated.borrow_mut() = Some(pids));
        let clear = Clear;
        let result = run();
        drop(clear);
        result
    }

    /// The kernel's same-uid process list, retried when the table grew
    /// between its size query and its read.
    fn same_uid_pids(uid: u32) -> Result<Vec<i32>, String> {
        let mut extra = 128usize;
        for _ in 0..3 {
            set_errno(0);
            // SAFETY: a null buffer with size 0 asks the kernel for the byte
            // count it needs; no memory is written.
            let needed = unsafe { proc_listpids(PROC_UID_ONLY, uid, std::ptr::null_mut(), 0) };
            if needed <= 0 {
                return Err(format!(
                    "the same-uid process table could not be sized ({})",
                    io::Error::last_os_error()
                ));
            }
            let count = needed as usize / std::mem::size_of::<i32>();
            if count > MAX_PROCESSES {
                return Err(format!(
                    "the same-uid process table exceeds {MAX_PROCESSES} processes"
                ));
            }
            let mut buffer = vec![0i32; count + extra];
            let bytes = buffer
                .len()
                .checked_mul(std::mem::size_of::<i32>())
                .and_then(|bytes| libc::c_int::try_from(bytes).ok())
                .ok_or("the process table buffer is too large")?;
            set_errno(0);
            // SAFETY: `buffer` is writable for `bytes` bytes and outlives the
            // call.
            let written =
                unsafe { proc_listpids(PROC_UID_ONLY, uid, buffer.as_mut_ptr().cast(), bytes) };
            if written <= 0 {
                return Err(format!(
                    "the same-uid process table could not be read ({})",
                    io::Error::last_os_error()
                ));
            }
            let written = written as usize;
            if written == buffer.len() * std::mem::size_of::<i32>() {
                // The table filled the whole buffer: it grew between the two
                // calls. One larger retry is a positive answer; more is a
                // table that will not stand still.
                extra = extra.saturating_mul(2);
                continue;
            }
            let entries = written / std::mem::size_of::<i32>();
            let mut pids: Vec<i32> = buffer[..entries]
                .iter()
                .copied()
                .filter(|pid| *pid > 0)
                .collect();
            pids.sort_unstable();
            pids.dedup();
            if pids.len() > MAX_PROCESSES {
                return Err(format!(
                    "the same-uid process table exceeds {MAX_PROCESSES} processes"
                ));
            }
            return Ok(pids);
        }
        Err("the same-uid process table kept changing during the scan".to_owned())
    }

    pub(super) fn process_identity(pid: i32) -> Result<Option<Identity>, String> {
        let mut buffer = [0u8; BSDINFO_SIZE];
        set_errno(0);
        // SAFETY: `buffer` is writable for exactly BSDINFO_SIZE bytes and no
        // new value is supplied.
        let read = unsafe {
            proc_pidinfo(
                pid,
                PROC_PIDTBSDINFO,
                0,
                buffer.as_mut_ptr().cast(),
                BSDINFO_SIZE as libc::c_int,
            )
        };
        if read != BSDINFO_SIZE as libc::c_int {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(None);
            }
            return Err(format!(
                "the kernel identity of pid {pid} is unreadable ({error})"
            ));
        }
        let pid_in_record = u32_at(&buffer, BSDINFO_PID)
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|found| *found == pid)
            .ok_or_else(|| format!("pid {pid} reported a different pid"))?;
        let uid = u32_at(&buffer, BSDINFO_UID)
            .ok_or_else(|| format!("pid {pid} reported no effective uid"))?;
        let seconds = u64_at(&buffer, BSDINFO_START_SEC)
            .ok_or_else(|| format!("pid {pid} reported no start time"))?;
        let micros = u64_at(&buffer, BSDINFO_START_USEC)
            .ok_or_else(|| format!("pid {pid} reported no start time"))?;
        if micros >= 1_000_000 {
            return Err(format!("pid {pid} reported an unusable start time"));
        }
        let command = text_at(&buffer, BSDINFO_NAME, 32)
            .filter(|text| !text.is_empty())
            .or_else(|| text_at(&buffer, BSDINFO_COMMAND, 16))
            .unwrap_or_else(|| format!("pid {pid_in_record}"));
        Ok(Some(Identity {
            pid,
            uid,
            start: seconds
                .checked_mul(1_000_000)
                .and_then(|start| start.checked_add(micros))
                .ok_or_else(|| format!("pid {pid} reported an unusable start time"))?,
            command,
        }))
    }

    /// One same-uid process's current working directory, settling the
    /// platform's hidden and dead answers the same way the descriptor scan
    /// does.
    fn working_directory(identity: &Identity, started: Instant) -> Result<Cwd, String> {
        if started.elapsed() >= SCAN_TIMEOUT {
            return Err("the descriptor scan exceeded its wall-clock bound".to_owned());
        }
        let mut buffer = vec![0u8; VNODEPATHINFO_SIZE];
        set_errno(0);
        // SAFETY: `buffer` is writable for its whole length and outlives the
        // call.
        let read = unsafe {
            proc_pidinfo(
                identity.pid,
                PROC_PIDVNODEPATHINFO,
                0,
                buffer.as_mut_ptr().cast(),
                VNODEPATHINFO_SIZE as libc::c_int,
            )
        };
        if read == VNODEPATHINFO_SIZE as libc::c_int {
            return match path_field(&buffer, VNODEPATHINFO_CDIR_PATH) {
                PathField::Path(path) => Ok(Cwd::Path(path)),
                // A removed or unreachable working directory cannot be a
                // path inside the slot tree, which still exists.
                PathField::Empty => Ok(Cwd::None),
                PathField::Malformed => Err(format!(
                    "pid {}'s working-directory path is unusable; the census is unknown",
                    identity.pid
                )),
            };
        }
        cwd_unreadable(identity.pid, &io::Error::last_os_error())
    }

    /// The decision for an unreadable working-directory read. A gone or
    /// removed directory (`ESRCH`, `ENOENT`) cannot be a path inside the slot
    /// tree; **every other answer, including a platform-hidden `EPERM`, is
    /// UNKNOWN and refuses** rather than being counted as no hold.
    pub(super) fn cwd_unreadable(pid: i32, error: &io::Error) -> Result<Cwd, String> {
        match error.raw_os_error() {
            Some(libc::ESRCH | libc::ENOENT) => Ok(Cwd::None),
            _ => Err(format!(
                "pid {pid}'s working directory is unreadable ({error}); the census is unknown"
            )),
        }
    }

    /// One process's descriptor listing, settling a transient `EPERM` by
    /// bounded identity re-observation. Only a positive fact settles it.
    fn list_descriptors(identity: &Identity, uid: u32, started: Instant) -> Result<Listed, String> {
        let mut current = identity.clone();
        for (attempt, backoff) in RETRY_BACKOFFS.iter().enumerate() {
            if attempt > 0 {
                std::thread::sleep(*backoff);
                if started.elapsed() >= SCAN_TIMEOUT {
                    return Err("the descriptor scan exceeded its wall-clock bound".to_owned());
                }
                match process_identity(current.pid)? {
                    None => return Ok(Listed::Gone),
                    Some(next) if next.uid != uid => return Ok(Listed::Foreign),
                    Some(next) => current = next,
                }
            }
            set_errno(0);
            // SAFETY: a null buffer with size 0 asks the kernel for the byte
            // count it needs; no memory is written.
            let sized =
                unsafe { proc_pidinfo(current.pid, PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
            let sizing_error = io::Error::last_os_error();
            let sizing_errno = sizing_error.raw_os_error().unwrap_or(0);
            if sized < 0 {
                let error = sizing_error;
                match error.raw_os_error() {
                    Some(libc::ESRCH) => return Ok(Listed::Gone),
                    Some(libc::EPERM) => continue,
                    _ => {
                        return Err(format!(
                            "pid {}'s descriptor listing is unreadable ({error}); the census is unknown",
                            current.pid
                        ))
                    }
                }
            }
            let needed = sized as usize;
            if needed == 0 {
                match classify_zero_listing(current.pid, sizing_errno) {
                    ZeroListing::Empty => return Ok(Listed::Descriptors(Vec::new())),
                    ZeroListing::Gone => return Ok(Listed::Gone),
                    ZeroListing::Retry => continue,
                    ZeroListing::Refuse(reason) => return Err(reason),
                }
            }
            if needed / PROC_FDINFO_SIZE >= MAX_DESCRIPTORS_PER_PROCESS {
                return Err(format!(
                    "pid {} holds {MAX_DESCRIPTORS_PER_PROCESS} or more descriptors",
                    current.pid
                ));
            }
            // Slack for a descriptor opened between the size query and the
            // read; a listing that fills it is retried, then bounded.
            let mut buffer = vec![0u8; needed + 64 * PROC_FDINFO_SIZE];
            let mut retry_outer = false;
            for _ in 0..2 {
                set_errno(0);
                // SAFETY: `buffer` is writable for its whole length and
                // outlives the call.
                let written = unsafe {
                    proc_pidinfo(
                        current.pid,
                        PROC_PIDLISTFDS,
                        0,
                        buffer.as_mut_ptr().cast(),
                        buffer.len() as libc::c_int,
                    )
                };
                let written_errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
                if written < 0 {
                    let error = io::Error::last_os_error();
                    match error.raw_os_error() {
                        Some(libc::ESRCH) => return Ok(Listed::Gone),
                        Some(libc::EPERM) => {
                            retry_outer = true;
                            break;
                        }
                        _ => {
                            return Err(format!(
                                "pid {}'s descriptor listing is unreadable ({error}); the census is unknown",
                                current.pid
                            ))
                        }
                    }
                }
                let written = written as usize;
                if written == 0 {
                    match classify_zero_listing(current.pid, written_errno) {
                        ZeroListing::Empty => return Ok(Listed::Descriptors(Vec::new())),
                        ZeroListing::Gone => return Ok(Listed::Gone),
                        ZeroListing::Retry => {
                            retry_outer = true;
                            break;
                        }
                        ZeroListing::Refuse(reason) => return Err(reason),
                    }
                }
                if written == buffer.len() {
                    buffer.resize(written + 256 * PROC_FDINFO_SIZE, 0);
                    continue;
                }
                if written % PROC_FDINFO_SIZE != 0 {
                    return Err(format!(
                        "pid {}'s descriptor listing is not a whole entry array; the census is unknown",
                        current.pid
                    ));
                }
                let entries = written / PROC_FDINFO_SIZE;
                let mut descriptors = Vec::with_capacity(entries);
                for entry in 0..entries {
                    let offset = entry * PROC_FDINFO_SIZE;
                    let descriptor =
                        i32_at(&buffer[offset..], 0).ok_or("a descriptor entry is truncated")?;
                    let kind =
                        u32_at(&buffer[offset..], 4).ok_or("a descriptor entry is truncated")?;
                    if descriptor >= 0 {
                        descriptors.push((descriptor, kind));
                    }
                }
                return Ok(Listed::Descriptors(descriptors));
            }
            if retry_outer {
                continue;
            }
            return Err(format!(
                "pid {}'s descriptor listing kept growing during the scan",
                current.pid
            ));
        }
        Err(format!(
            "pid {}'s descriptor listing stayed unreadable (EPERM) after its bounded re-observations; the census is unknown",
            identity.pid
        ))
    }

    /// One vnode descriptor, with the bounded paired re-read of a disagreeing
    /// observation and the dead-handle re-observation.
    fn observe_descriptor(
        identity: &Identity,
        descriptor: i32,
        started: Instant,
    ) -> Result<Observed, String> {
        for attempt in 0..PAIR_RETRY_ATTEMPTS {
            if attempt > 0 {
                if started.elapsed().saturating_add(PAIR_RETRY_BACKOFF) >= SCAN_TIMEOUT {
                    return Err("the descriptor scan exceeded its wall-clock bound".to_owned());
                }
                std::thread::sleep(PAIR_RETRY_BACKOFF);
                if started.elapsed() >= SCAN_TIMEOUT {
                    return Err("the descriptor scan exceeded its wall-clock bound".to_owned());
                }
                match process_identity(identity.pid)? {
                    None => return Ok(Observed::Closed),
                    Some(next) if next.uid != current_uid() => return Ok(Observed::Closed),
                    Some(next) if next.start != identity.start => {
                        // A different process now has the pid: this
                        // observation is about a process that is gone.
                        return Ok(Observed::Closed);
                    }
                    Some(_) => {}
                }
            }
            let (facts, path) = read_pair(identity.pid, descriptor);
            match classify_pair(identity.pid, descriptor, &facts, &path) {
                Pair::Path(path) => return Ok(Observed::Path(path)),
                Pair::DeadCandidate => return dead_handle(identity, descriptor, started),
                Pair::Closed => return Ok(Observed::Closed),
                Pair::Opaque => {
                    let verified = verified_platform_binary(identity)?;
                    return opaque_verdict(identity, descriptor, verified);
                }
                Pair::Retry => {
                    // A mixed pair is not an answer: the descriptor may have
                    // closed between the two flavors. Re-read it as a pair.
                    if attempt + 1 == PAIR_RETRY_ATTEMPTS {
                        return Err(format!(
                            "pid {}'s descriptor {descriptor} did not read as one observation ({}, {}); the census is unknown",
                            identity.pid,
                            describe(&facts),
                            describe(&path)
                        ));
                    }
                }
            }
        }
        Err("the descriptor scan could not settle a descriptor".to_owned())
    }

    /// Both reads answering `ENOENT` is a question: a revoked or reclaimed
    /// vnode cannot do I/O, but only a second observation of the same process
    /// establishes it.
    fn dead_handle(
        identity: &Identity,
        descriptor: i32,
        started: Instant,
    ) -> Result<Observed, String> {
        std::thread::sleep(RETRY_BACKOFFS[0]);
        if started.elapsed() >= SCAN_TIMEOUT {
            return Err("the descriptor scan exceeded its wall-clock bound".to_owned());
        }
        match process_identity(identity.pid)? {
            None => return Ok(Observed::Closed),
            Some(next) if next.start != identity.start || next.uid != current_uid() => {
                return Ok(Observed::Closed)
            }
            Some(_) => {}
        }
        let (facts, path) = read_pair(identity.pid, descriptor);
        match classify_pair(identity.pid, descriptor, &facts, &path) {
            Pair::Path(path) => Ok(Observed::Path(path)),
            // Both-flavour `ENOENT` (or a second empty name) is the positive
            // dead-handle fact; a hidden second read is UNKNOWN and refuses.
            Pair::DeadCandidate => Ok(Observed::Dead),
            Pair::Closed => Ok(Observed::Closed),
            Pair::Opaque => {
                let verified = verified_platform_binary(identity)?;
                opaque_verdict(identity, descriptor, verified)
            }
            Pair::Retry => Err(format!(
                "pid {}'s dead-handle descriptor {descriptor} was not readable on its second observation ({}, {}); the census is unknown",
                identity.pid,
                describe(&facts),
                describe(&path)
            )),
        }
    }

    /// The one production decision for a descriptor's two reads. A readable
    /// path is a positive observation whatever the other flavour says, so it
    /// is checked for containment and never discarded in favour of a closed
    /// read. Both-flavour `EPERM` is a hidden path: the census cannot say what
    /// the process holds, so it is UNKNOWN and refuses. Both-flavour `ENOENT`
    /// is a dead-handle candidate; only both reads reporting the descriptor
    /// closed mean it is no longer held; any other mixed pair is re-read.
    pub(super) fn classify_pair(
        _pid: i32,
        _descriptor: i32,
        facts: &Result<(), io::Error>,
        path: &Result<Option<PathBuf>, io::Error>,
    ) -> Pair {
        match (facts, path) {
            // A readable path is a live observation, whatever the vnode-facts
            // read says. Dropping it for a closed sibling read would let a
            // handle reopened inside the slot pass as closed.
            (_, Ok(Some(path))) => Pair::Path(path.clone()),
            (Ok(()), Ok(None)) => Pair::DeadCandidate,
            (Err(left), Err(right))
                if left.raw_os_error() == Some(libc::EPERM)
                    && right.raw_os_error() == Some(libc::EPERM) =>
            {
                Pair::Opaque
            }
            (Err(left), Err(right))
                if left.raw_os_error() == Some(libc::ENOENT)
                    && right.raw_os_error() == Some(libc::ENOENT) =>
            {
                Pair::DeadCandidate
            }
            (Err(left), Err(right)) if closed(left) && closed(right) => Pair::Closed,
            _ => Pair::Retry,
        }
    }

    /// The caller's decision for a platform-hidden descriptor: a verified
    /// platform binary's opaque descriptor is a counted blind spot, every other
    /// non-platform answer refuses.
    pub(super) fn opaque_verdict(
        identity: &Identity,
        descriptor: i32,
        verified: bool,
    ) -> Result<Observed, String> {
        if verified {
            Ok(Observed::Hidden)
        } else {
            Err(format!(
                "pid {}'s descriptor {descriptor} is hidden by the platform and the process is not a verified platform binary; the census is unknown",
                identity.pid
            ))
        }
    }

    /// Whether a platform-hidden descriptor belongs to a verified platform
    /// binary with the same kernel start identity around the status read. Any
    /// failed status read, disappeared/changed identity or non-platform answer
    /// is a refusal, never a blind spot.
    pub(super) fn verified_platform_binary(identity: &Identity) -> Result<bool, String> {
        let mut status: u32 = 0;
        set_errno(0);
        // SAFETY: `status` is writable for exactly four bytes and outlives the
        // call.
        let read = unsafe {
            csops(
                identity.pid,
                CS_OPS_STATUS,
                (&mut status as *mut u32).cast(),
                std::mem::size_of::<u32>(),
            )
        };
        let status = if read == 0 {
            Ok(status)
        } else {
            Err(format!(
                "the code-signing status of pid {} is unreadable ({}); the census is unknown",
                identity.pid,
                io::Error::last_os_error()
            ))
        };
        let after =
            process_identity(identity.pid).map(|option| option.map(|next| (next.start, next.uid)));
        platform_status_verdict(identity, status, after)
    }

    /// The pure platform-binary decision: a readable platform flag with the
    /// same start identity and uid before and after the status read. Any other
    /// answer refuses; the caller then treats the hidden descriptor as UNKNOWN.
    pub(super) fn platform_status_verdict(
        identity: &Identity,
        status: Result<u32, String>,
        after: Result<Option<(u64, u32)>, String>,
    ) -> Result<bool, String> {
        let status = status?;
        let Some((start, uid)) = after? else {
            return Err(format!(
                "pid {}'s kernel identity disappeared around its code-signing read; the census is unknown",
                identity.pid
            ));
        };
        if start != identity.start || uid != identity.uid {
            return Err(format!(
                "pid {}'s kernel identity changed around its code-signing read; the census is unknown",
                identity.pid
            ));
        }
        Ok(status & CS_PLATFORM_BINARY != 0)
    }

    fn read_pair(
        pid: i32,
        descriptor: i32,
    ) -> (Result<(), io::Error>, Result<Option<PathBuf>, io::Error>) {
        let mut facts = vec![0u8; VNODE_FDINFO_SIZE];
        set_errno(0);
        // SAFETY: `facts` is writable for its whole length and outlives the
        // call.
        let read = unsafe {
            proc_pidfdinfo(
                pid,
                descriptor,
                PROC_PIDFDVNODEINFO,
                facts.as_mut_ptr().cast(),
                VNODE_FDINFO_SIZE as libc::c_int,
            )
        };
        let facts = if read == VNODE_FDINFO_SIZE as libc::c_int {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        };
        let mut with_path = vec![0u8; VNODE_FDINFO_WITH_PATH_SIZE];
        set_errno(0);
        // SAFETY: `with_path` is writable for its whole length and outlives
        // the call.
        let read = unsafe {
            proc_pidfdinfo(
                pid,
                descriptor,
                PROC_PIDFDVNODEPATHINFO,
                with_path.as_mut_ptr().cast(),
                VNODE_FDINFO_WITH_PATH_SIZE as libc::c_int,
            )
        };
        let path = if read == VNODE_FDINFO_WITH_PATH_SIZE as libc::c_int {
            match path_field(&with_path, VNODE_FDINFO_PATH) {
                PathField::Path(path) => Ok(Some(path)),
                // A vnode with no reachable name (an unlinked file) cannot
                // change the slot tree; it is a dead handle, not a path.
                PathField::Empty => Ok(None),
                PathField::Malformed => Err(io::Error::from_raw_os_error(libc::EINVAL)),
            }
        } else {
            Err(io::Error::last_os_error())
        };
        (facts, path)
    }

    fn closed(error: &io::Error) -> bool {
        matches!(error.raw_os_error(), Some(libc::EBADF | libc::ESRCH))
    }

    fn describe<T>(result: &Result<T, io::Error>) -> String {
        match result {
            Ok(_) => "ok".to_owned(),
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => "ENOENT".to_owned(),
            Err(error) if error.raw_os_error() == Some(libc::EPERM) => "EPERM".to_owned(),
            Err(error) => format!("{error}"),
        }
    }

    fn set_errno(value: libc::c_int) {
        // SAFETY: `__error` returns this thread's errno pointer, which is
        // writable for the thread's lifetime.
        unsafe {
            *libc::__error() = value;
        }
    }

    fn u32_at(buffer: &[u8], offset: usize) -> Option<u32> {
        let bytes = buffer.get(offset..offset + 4)?;
        Some(u32::from_ne_bytes(bytes.try_into().ok()?))
    }

    fn i32_at(buffer: &[u8], offset: usize) -> Option<i32> {
        let bytes = buffer.get(offset..offset + 4)?;
        Some(i32::from_ne_bytes(bytes.try_into().ok()?))
    }

    fn u64_at(buffer: &[u8], offset: usize) -> Option<u64> {
        let bytes = buffer.get(offset..offset + 8)?;
        Some(u64::from_ne_bytes(bytes.try_into().ok()?))
    }

    fn text_at(buffer: &[u8], offset: usize, length: usize) -> Option<String> {
        let bytes = buffer.get(offset..offset + length)?;
        let end = bytes.iter().position(|byte| *byte == 0)?;
        Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }

    fn path_field(buffer: &[u8], offset: usize) -> PathField {
        let Some(bytes) = buffer.get(offset..offset + MAX_PATH_BYTES) else {
            return PathField::Malformed;
        };
        let Some(end) = bytes.iter().position(|byte| *byte == 0) else {
            return PathField::Malformed;
        };
        if end == 0 {
            return PathField::Empty;
        }
        PathField::Path(PathBuf::from(OsStr::from_bytes(&bytes[..end])))
    }
}

#[cfg(target_os = "macos")]
use platform::collect;

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    /// One fixture directory that never outlives its test.
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let root = base.join(format!(
                "oxide-census-{label}-{}-{}",
                std::process::id(),
                crate::process::now_ms()
            ));
            std::fs::create_dir(&root).expect("census fixture root");
            Self {
                root: std::fs::canonicalize(root).expect("canonical fixture root"),
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.root.join(name)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// A same-uid child that holds one file open (or one working directory)
    /// until it is told to stop, always reaped even when an assertion fails.
    struct Writer {
        child: Option<Child>,
        stop: PathBuf,
    }

    impl Writer {
        fn start(script: &str, arguments: &[&Path], ready: &Path) -> Self {
            let mut command = Command::new("/bin/sh");
            command
                .arg("-c")
                .arg(script)
                .arg("sh")
                .args(arguments)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let stop = arguments
                .last()
                .map(|argument| argument.to_path_buf())
                .expect("a stop path");
            let child = command.spawn().expect("a writer child");
            let fixture = Self {
                child: Some(child),
                stop,
            };
            let deadline = Instant::now() + Duration::from_secs(10);
            while !ready.exists() {
                assert!(
                    Instant::now() < deadline,
                    "the writer child never published {}",
                    ready.display()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            fixture
        }

        fn pid(&self) -> u32 {
            self.child.as_ref().expect("a live writer child").id()
        }

        /// Stop the writer and wait for it to be gone, so its descriptors are
        /// closed before the caller observes again.
        fn stop(mut self) {
            std::fs::write(&self.stop, b"stop").expect("write the stop file");
            let child = self.child.take().expect("a live writer child");
            self.wait_gone(child);
        }

        fn wait_gone(&self, mut child: Child) {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }
                }
            }
        }
    }

    impl Drop for Writer {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = std::fs::write(&self.stop, b"stop");
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => return,
                        Ok(None) if Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        _ => {
                            let _ = child.kill();
                            let _ = child.wait();
                            return;
                        }
                    }
                }
            }
        }
    }

    const HOLD_SCRIPT: &str =
        "exec 3<\"$1\" || exit 9; : > \"$2\"; while [ ! -e \"$3\" ]; do sleep 0.02; done";
    const CWD_SCRIPT: &str =
        "cd \"$1\" || exit 9; : > \"$2\"; while [ ! -e \"$3\" ]; do sleep 0.02; done";

    /// A hold found is a refusal naming the process and the path.
    #[test]
    fn an_open_handle_inside_the_slot_refuses() {
        let fixture = Fixture::new("open");
        let slot = fixture.path("slot");
        std::fs::create_dir(&slot).expect("slot");
        let held = slot.join("writer.txt");
        std::fs::write(&held, b"writer").expect("held file");
        let ready = fixture.path("ready");
        let stop = fixture.path("stop");
        let writer = Writer::start(HOLD_SCRIPT, &[&held, &ready, &stop], &ready);
        let pid = writer.pid();
        let error = platform::with_stated_processes(vec![pid as i32], || verify(&slot, None))
            .expect_err("an open handle must refuse");
        assert!(error.contains("holds"), "{error}");
        assert!(error.contains(&held.display().to_string()), "{error}");
        assert!(error.contains("nothing was released"), "{error}");
        writer.stop();
    }

    /// A working directory inside the slot is a refusal of its own.
    #[test]
    fn a_working_directory_inside_the_slot_refuses() {
        let fixture = Fixture::new("cwd");
        let slot = fixture.path("slot");
        let nested = slot.join("nested");
        std::fs::create_dir_all(&nested).expect("nested slot directory");
        let ready = fixture.path("ready");
        let stop = fixture.path("stop");
        let writer = Writer::start(CWD_SCRIPT, &[&nested, &ready, &stop], &ready);
        let pid = writer.pid();
        let error = platform::with_stated_processes(vec![pid as i32], || verify(&slot, None))
            .expect_err("a working directory must refuse");
        assert!(error.contains("working directory"), "{error}");
        assert!(error.contains(&nested.display().to_string()), "{error}");
        writer.stop();
    }

    /// A sibling path that merely shares a name prefix is not inside the
    /// slot: the comparison is component-wise.
    #[test]
    fn a_prefixed_sibling_is_not_inside_the_slot() {
        let fixture = Fixture::new("prefix");
        let slot = fixture.path("slot");
        std::fs::create_dir(&slot).expect("slot");
        let sibling = fixture.path("slot-extra");
        std::fs::create_dir(&sibling).expect("sibling");
        let held = sibling.join("writer.txt");
        std::fs::write(&held, b"writer").expect("held file");
        let ready = fixture.path("ready");
        let stop = fixture.path("stop");
        let writer = Writer::start(HOLD_SCRIPT, &[&held, &ready, &stop], &ready);
        // One clean observation also shows the census counted the processes
        // and descriptors it read.
        let pid = writer.pid();
        let summary = platform::with_stated_processes(vec![pid as i32], || verify(&slot, None))
            .expect("a prefixed sibling is not the slot");
        assert!(summary.processes > 0, "{summary:?}");
        assert!(summary.descriptors > 0, "{summary:?}");
        assert_eq!(summary.holder, "no recorded holder");
        writer.stop();
    }

    /// A census that could not complete is UNKNOWN and refuses, never
    /// "closed"; a positive hold refuses too. The decision layer is exercised
    /// directly so a stated unreadable observation needs no fault injection.
    #[test]
    fn an_unknown_census_refuses() {
        let fixture = Fixture::new("unknown");
        let slot = fixture.path("slot");
        std::fs::create_dir(&slot).expect("slot");
        let error = verdict(
            &slot,
            Err("a stated unreadable descriptor".to_owned()),
            Ok("no recorded holder".to_owned()),
        )
        .expect_err("an incomplete census must refuse");
        assert!(error.contains("unknown"), "{error}");
        assert!(error.contains("a stated unreadable descriptor"), "{error}");
        let mut scan = Scan::default();
        scan.hits.push(Hit {
            pid: 7,
            command: "writer".to_owned(),
            what: "holds /slot/file open".to_owned(),
        });
        let error = verdict(&slot, Ok(scan), Ok("no recorded holder".to_owned()))
            .expect_err("a hold must refuse");
        assert!(error.contains("process 7"), "{error}");
    }

    /// A same-account live holder is unidentifiable and refuses (the record's
    /// wall-clock sample is not a kernel identity); a gone pid or another
    /// account's process is a positive fact and passes.
    #[test]
    fn a_live_holder_refuses_and_only_positive_identities_pass() {
        let fixture = Fixture::new("holder");
        let slot = fixture.path("slot");
        std::fs::create_dir(&slot).expect("slot");
        let pid = std::process::id();
        let live = Holder { pid };
        let error = holder_state(Some(&live)).expect_err("a live holder must refuse");
        assert!(error.contains("still running"), "{error}");
        assert!(error.contains("unidentifiable"), "{error}");
        // The whole check refuses the live holder too. The stated table keeps
        // the real descriptor reads deterministic.
        let error =
            platform::with_stated_processes(vec![pid as i32], || verify(&slot, Some(&live)))
                .expect_err("a live holder must refuse");
        assert!(error.contains("still running"), "{error}");
        // A pid above the platform's range: the kernel reports it gone.
        let gone =
            holder_state(Some(&Holder { pid: 4_000_000 })).expect("an impossible pid is gone");
        assert!(gone.contains("is gone"), "{gone}");
        // The decision over a stated kernel identity: another account's
        // process is the other positive; a same-account process is not.
        let other = holder_verdict(7, Some((501, "launchd".to_owned())), 501 + 1)
            .expect("another account's process is positively different");
        assert!(other.contains("another account"), "{other}");
        let unidentifiable = holder_verdict(7, Some((501, "oxide".to_owned())), 501)
            .expect_err("a same-account process is unidentifiable");
        assert!(
            unidentifiable.contains("unidentifiable"),
            "{unidentifiable}"
        );
        assert_eq!(
            holder_verdict(7, None, 501).unwrap(),
            "recorded holder pid 7 is gone"
        );
        assert_eq!(holder_state(None).expect("no holder"), "no recorded holder");
    }

    /// A platform-hidden working directory is UNKNOWN and refuses; a gone or
    /// removed one is not a path in the slot. The production decision is
    /// exercised directly so a hidden read needs no fault injection.
    #[test]
    fn a_hidden_working_directory_refuses() {
        let hidden = platform::cwd_unreadable(7, &std::io::Error::from_raw_os_error(libc::EPERM))
            .expect_err("a hidden working directory must refuse");
        assert!(hidden.contains("census is unknown"), "{hidden}");
        assert!(hidden.contains("pid 7"), "{hidden}");
        let other = platform::cwd_unreadable(7, &std::io::Error::from_raw_os_error(libc::EACCES))
            .expect_err("any other unreadable working directory must refuse");
        assert!(other.contains("census is unknown"), "{other}");
        for gone in [libc::ESRCH, libc::ENOENT] {
            assert!(matches!(
                platform::cwd_unreadable(7, &std::io::Error::from_raw_os_error(gone)),
                Ok(platform::Cwd::None)
            ));
        }
    }

    /// A zero-byte descriptor read is only an empty listing when the kernel
    /// reports no error: `ESRCH` is gone, `EPERM` is the bounded retry, and
    /// every other errno is UNKNOWN and refuses.
    #[test]
    fn a_zero_descriptor_listing_is_never_silently_empty() {
        assert!(matches!(
            platform::classify_zero_listing(7, 0),
            platform::ZeroListing::Empty
        ));
        assert!(matches!(
            platform::classify_zero_listing(7, libc::ESRCH),
            platform::ZeroListing::Gone
        ));
        assert!(matches!(
            platform::classify_zero_listing(7, libc::EPERM),
            platform::ZeroListing::Retry
        ));
        match platform::classify_zero_listing(7, libc::EACCES) {
            platform::ZeroListing::Refuse(reason) => {
                assert!(reason.contains("census is unknown"), "{reason}");
                assert!(reason.contains("errno"), "{reason}");
            }
            _ => panic!("a zero listing with an unreadable errno was not refused"),
        }
    }

    /// A vnode descriptor whose path and vnode facts are both hidden is only
    /// a blind-spot candidate; the platform-binary decision separately accepts
    /// it or refuses. Only a readable path, a closed descriptor or both-flavour
    /// `ENOENT` is a direct answer.
    #[test]
    fn a_hidden_descriptor_is_only_a_blind_spot_candidate() {
        use std::io::Error;
        assert!(matches!(
            platform::classify_pair(
                9,
                4,
                &Err(Error::from_raw_os_error(libc::EPERM)),
                &Err(Error::from_raw_os_error(libc::EPERM)),
            ),
            platform::Pair::Opaque
        ));
        // A readable path is checked for containment even when the other
        // flavour is hidden; it is never a blind spot.
        match platform::classify_pair(
            9,
            4,
            &Err(Error::from_raw_os_error(libc::EPERM)),
            &Ok(Some(PathBuf::from("/slot/file"))),
        ) {
            platform::Pair::Path(path) => assert_eq!(path, PathBuf::from("/slot/file")),
            _ => panic!("a readable path was not classified as a path"),
        }
        assert!(matches!(
            platform::classify_pair(
                9,
                4,
                &Err(Error::from_raw_os_error(libc::ENOENT)),
                &Err(Error::from_raw_os_error(libc::ENOENT)),
            ),
            platform::Pair::DeadCandidate
        ));
        // A closed facts read paired with a readable empty name is a mixed
        // pair, not a closed descriptor.
        assert!(matches!(
            platform::classify_pair(9, 4, &Err(Error::from_raw_os_error(libc::EBADF)), &Ok(None),),
            platform::Pair::Retry
        ));
    }

    /// A mixed closed/readable observation must never be closed: a live
    /// handle reopened inside the slot has to reach the containment check, and
    /// an ambiguous pair has to be re-read (and refused if it does not settle).
    #[test]
    fn a_mixed_closed_and_readable_observation_is_never_closed() {
        use std::io::Error;
        match platform::classify_pair(
            9,
            4,
            &Err(Error::from_raw_os_error(libc::EBADF)),
            &Ok(Some(PathBuf::from("/slot/file"))),
        ) {
            platform::Pair::Path(path) => assert_eq!(path, PathBuf::from("/slot/file")),
            _ => panic!("a readable in-slot path was discarded"),
        }
        assert!(matches!(
            platform::classify_pair(9, 4, &Ok(()), &Err(Error::from_raw_os_error(libc::EBADF))),
            platform::Pair::Retry
        ));
        assert!(matches!(
            platform::classify_pair(
                9,
                4,
                &Err(Error::from_raw_os_error(libc::EBADF)),
                &Err(Error::from_raw_os_error(libc::EBADF)),
            ),
            platform::Pair::Closed
        ));
    }

    /// The verified-platform blind spot is tolerated only for a readable
    /// `CS_PLATFORM_BINARY` status with the same kernel start and uid before
    /// and after the read; every other answer refuses.
    #[test]
    fn a_verified_platform_blind_spot_is_tolerated_and_everything_else_refuses() {
        let identity = platform::Identity {
            pid: 7,
            uid: 501,
            start: 1_000,
            command: "platform".to_owned(),
        };
        let status = platform::CS_PLATFORM_BINARY;
        assert!(
            platform::platform_status_verdict(&identity, Ok(status), Ok(Some((1_000, 501))))
                .expect("a verified platform binary")
        );
        assert!(
            !platform::platform_status_verdict(&identity, Ok(0), Ok(Some((1_000, 501))))
                .expect("a non-platform answer is a refusal at the caller")
        );
        for (label, status, after) in [
            (
                "unreadable status",
                Err("code-signing read failed; the census is unknown".to_owned()),
                Ok(Some((1_000, 501))),
            ),
            ("disappeared", Ok(status), Ok(None)),
            ("changed", Ok(status), Ok(Some((2_000, 501)))),
            ("changed uid", Ok(status), Ok(Some((1_000, 502)))),
            (
                "unreadable identity",
                Ok(status),
                Err("identity read failed; the census is unknown".to_owned()),
            ),
        ] {
            let error = platform::platform_status_verdict(&identity, status, after)
                .expect_err(&format!("{label} must refuse"));
            assert!(error.contains("unknown"), "{label}: {error}");
        }
    }

    /// A platform-hidden descriptor of a non-platform process is a refusal;
    /// only a verified platform binary's opaque descriptor is tolerated.
    #[test]
    fn a_non_platform_hidden_descriptor_refuses() {
        let identity = platform::Identity {
            pid: 7,
            uid: 501,
            start: 1_000,
            command: "user".to_owned(),
        };
        let error = platform::opaque_verdict(&identity, 4, false)
            .expect_err("a non-platform hidden descriptor must refuse");
        assert!(error.contains("not a verified platform binary"), "{error}");
        assert!(error.contains("census is unknown"), "{error}");
        assert!(platform::opaque_verdict(&identity, 4, true).is_ok());
    }

    /// A locally built test binary is not an Apple platform binary: the
    /// blind-spot exemption cannot be claimed by this process.
    #[test]
    fn this_test_binary_is_not_a_platform_binary() {
        let own = platform::process_identity(std::process::id() as i32)
            .expect("identity read")
            .expect("this process exists");
        assert_eq!(
            platform::verified_platform_binary(&own).expect("code-signing status"),
            false
        );
    }
}
