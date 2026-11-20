//! The pool's exclusive lock: bounded waits, probes and the trash-sweep test hooks.

use super::*;

pub(super) fn lock_pool(pool: &Path) -> Result<File, LeaseError> {
    lock_pool_inner(pool, true, false)
}

/// Whether a descriptor in this process holds the pool lock. A test seam: a
/// fresh non-blocking exclusive `flock` fails while a holder exists, so a test
/// can assert that a pass ran with the lock released.
#[cfg(test)]
pub(crate) fn pool_lock_is_held(pool: &Path) -> bool {
    let Ok(file) = OpenOptions::new()
        .read(true)
        .write(true)
        .open(pool.join(LOCK_NAME))
    else {
        return false;
    };
    // SAFETY: the descriptor stays open for both calls and owns no lock after
    // the explicit unlock.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return true;
    }
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    false
}

/// Whether the last settle pass on this thread ran every Barge eviction with the
/// pool lock released.
#[cfg(test)]
pub(crate) fn last_settle_held_no_pool_lock() -> bool {
    pending::last_settle_held_no_pool_lock()
}

#[cfg(test)]
pub(crate) fn fail_next_trash_sweep() {
    pending::fail_next_sweep();
}

pub(super) fn lock_pool_inner(pool: &Path, create: bool, notice: bool) -> Result<File, LeaseError> {
    let (deadline, bound) = pool_lock_deadline(Instant::now());
    lock_pool_until(pool, create, notice, deadline, bound)
}

pub(super) fn lock_pool_until(
    pool: &Path,
    create: bool,
    notice: bool,
    deadline: Instant,
    bound: &'static str,
) -> Result<File, LeaseError> {
    let path = pool.join(LOCK_NAME);
    let existing = match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(LeaseError::UnsafePath("lock file must be a regular file"));
            }
            Some(metadata)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => None,
        Err(error) => return Err(io_error(error)),
    };
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    if create {
        options.create(true);
    }
    let file = options.open(&path).map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != current_uid()
        || existing
            .is_some_and(|before| before.dev() != metadata.dev() || before.ino() != metadata.ino())
    {
        return Err(LeaseError::UnsafePath("lock file must be private"));
    }
    wait_for_pool_lock(
        &file,
        notice.then_some(POOL_LOCK_NOTICE),
        deadline,
        bound,
        || match read_ledger(pool).ok().and_then(|ledger| {
            ledger
                .slots
                .into_iter()
                .find(|slot| slot.state == SlotPersistState::Evicting)
        }) {
            Some(slot) => format!(
                "slot {} is recorded as being deleted; its current writer is unknown",
                slot.id
            ),
            None => "holder unknown".to_owned(),
        },
    )?;
    Ok(file)
}

/// The longest a pool-lock wait may notice a holder before it gives up, after
/// the one progress line.
const POOL_LOCK_NOTICE: Duration = Duration::from_secs(5);
/// The bound for a pool-lock wait with no admission budget. Holds on the pool
/// lock are short (eviction moves trees out of the lock), so this is only a
/// backstop against a stale or unreachable holder.
pub(super) const POOL_LOCK_BOUND_SECONDS: u64 = 120;
/// The release's one observation walk happens inside the pool lock, so it is
/// bounded well below [`POOL_LOCK_BOUND_SECONDS`]: a walk that runs out leaves
/// the slot unrecorded, which the next admission charges conservatively, and
/// no other command waits longer than this for the lock.
pub(super) const RELEASE_WALK_MAX_ELAPSED_MS: u64 = 10_000;
const POOL_LOCK_POLL: Duration = Duration::from_millis(10);

/// The latest instant a pool-lock wait started at `now` may last, and what
/// bounds it for the message. A command whose admission window is still open
/// is held to that window; otherwise the wait gets [`POOL_LOCK_BOUND_SECONDS`].
/// An expired window never refuses a wait by itself: a release after a long
/// build lands after its window has closed and must still be able to take the
/// lock. Never unbounded: a stale or unknown holder ends the wait with a
/// refusal naming what it waited on (oxide#325).
pub(super) fn pool_lock_deadline(now: Instant) -> (Instant, &'static str) {
    let default = now + Duration::from_secs(POOL_LOCK_BOUND_SECONDS);
    match crate::lease::admission_deadline() {
        Some(expires) if expires > now && expires < default => {
            (expires, "the admission budget expired")
        }
        _ => (default, "the pool-lock bound was reached"),
    }
}

/// Take the pool lock with a poll that stops at `deadline`. `holder` names
/// what the wait is on; it is read for the one progress line after
/// `notice_after` and again for a refusal.
fn wait_for_pool_lock(
    file: &File,
    notice_after: Option<Duration>,
    deadline: Instant,
    bound: &'static str,
    holder: impl Fn() -> String,
) -> Result<(), LeaseError> {
    let started = Instant::now();
    let mut noticed = false;
    loop {
        // SAFETY: the descriptor stays open through this call and owns the lock on success.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(LeaseError::Io(format!(
                "exclusive pool lock failed: {error}"
            )));
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(LeaseError::Io(format!(
                "gave up waiting for the pool lock after {:.1}s: {bound}; {}",
                now.duration_since(started).as_secs_f64(),
                holder()
            )));
        }
        if !noticed && notice_after.is_some_and(|after| now.duration_since(started) >= after) {
            eprintln!("oxide: waiting for the pool lock ({})", holder());
            noticed = true;
        }
        std::thread::sleep(POOL_LOCK_POLL.min(deadline.saturating_duration_since(now)));
    }
}
