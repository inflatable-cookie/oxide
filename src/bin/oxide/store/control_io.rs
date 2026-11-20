//! The control directory's owner-only files, pool creation and admission lock.
//!
//! Every path here refuses a symlink, a foreign owner and a group- or
//! world-readable mode. `admission.lock` is the pool's single writer gate: the
//! holder is the only writer of `state.json`, and a contender either takes it
//! or is refused with the precise reason, never a silent "no lock".

use super::*;
use oxide_storage::lease::managed::{init_budget_pool, snapshot};
use oxide_storage::lease::{check_control_writer, fence_admission_writer, LeaseError};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

/// Caller-available bytes of the filesystem holding the new pool's parent.
/// Debug builds accept a scoped injected reading so the fixture proof reaches
/// the refusal path, capacity diagnostic included, without consuming the
/// host's free space; release builds always sample the filesystem.
pub(super) fn free_bytes_for_creation(parent: &Path) -> Option<u64> {
    #[cfg(debug_assertions)]
    if let Some(injected) = std::env::var("OXIDE_TEST_POOL_FREE_BYTES")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
    {
        return Some(injected);
    }
    statfs(parent).and_then(|sample| {
        let block = u64::try_from(sample.f_bsize).ok()?;
        sample.f_bavail.checked_mul(block)
    })
}

pub(super) fn create_pool(path: &Path, budget: u64) -> Result<(), String> {
    let parent = path.parent().ok_or("the pool path has no parent")?;
    let name = path
        .file_name()
        .ok_or("the pool path has no final component")?;
    create_parents(parent)?;
    let parent = fs::canonicalize(parent).map_err(|error| error.to_string())?;
    let available = free_bytes_for_creation(&parent)
        .ok_or("cannot sample host free space for pool creation")?;
    if available < HOST_RESERVE_BYTES {
        let mut message = format!(
            "refusing to create the pool: host free space {} MiB is below the 20 GiB reserve",
            available / (1024 * 1024)
        );
        // Read-only, bounded macOS figures for the same filesystem, after the
        // refusal is decided: they never change the 20 GiB decision.
        if let Some(note) = crate::capacity::diagnose(&parent) {
            message.push('\n');
            message.push_str(&note);
        }
        return Err(message);
    }
    let pool = parent.join(name);
    let backend = Backend::DirectoryBudget;
    eprintln!(
        "oxide: creating the build pool at {} ({} MB soft budget, {}, {})",
        pool.display(),
        budget / 1_000_000,
        backend.tag(),
        backend.bound()
    );
    match init_budget_pool(&pool, budget) {
        Ok(_) => Ok(()),
        // Only the expected race is tolerated: another command created this
        // pool first, which the library reports as its one fixed refusal. Any
        // other error (a permissions failure, a payload in the way, a full
        // disk) is the caller's to see, not a silent success merely because a
        // `config.json` exists (oxide#337).
        Err(LeaseError::InvalidInput("pool already initialized"))
            if pool.join("config.json").is_file() =>
        {
            Ok(())
        }
        Err(error) => Err(lease_error("could not create the pool", &error)),
    }
}

/// Create missing ancestors owner-only; existing ones are left as they are.
pub(super) fn create_parents(path: &Path) -> Result<(), String> {
    if fs::symlink_metadata(path).is_ok() {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or("the pool path has no existing ancestor")?;
    create_parents(parent)?;
    // The mode is set at creation, never after it: a `create_dir` followed by
    // `set_permissions` leaves the umask-controlled mode readable to others
    // until the second call (oxide#337).
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(format!("cannot create {}: {error}", path.display())),
    }
}

pub(super) fn ensure_private_dir(path: &Path) -> Result<(), String> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("cannot create {}: {error}", path.display())),
    }
    verify_private_dir(path)
}

/// Open an existing owner-only control directory without following its final
/// path component. The descriptor is suitable for relative control-file I/O.
pub(crate) fn open_control_directory(path: &Path) -> Result<Option<File>, String> {
    let named = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    };
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if !named.is_dir() || named.uid() != uid || named.mode() & 0o077 != 0 {
        return Err(format!(
            "{} must be an owner-only directory",
            path.display()
        ));
    }
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let opened = directory
        .metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if !opened.is_dir()
        || opened.uid() != uid
        || opened.mode() & 0o077 != 0
        || opened.dev() != named.dev()
        || opened.ino() != named.ino()
    {
        return Err(format!(
            "{} changed while it was being opened",
            path.display()
        ));
    }
    Ok(Some(directory))
}

pub(super) fn verify_private_dir(path: &Path) -> Result<(), String> {
    if open_control_directory(path)?.is_none() {
        return Err(format!("{} does not exist", path.display()));
    }
    Ok(())
}

/// The pool's admission lock file, created once and opened, but not locked.
pub(super) fn open_admission_file(control: &Path) -> Result<Option<File>, String> {
    let Some(directory) = open_control_directory(control)? else {
        return Ok(None);
    };
    let name = CString::new("admission.lock").map_err(|error| error.to_string())?;
    // Create exclusively, then open the existing lock without `O_CREAT`.
    // Repeating `O_CREAT | O_NOFOLLOW` for every contender races with the
    // creator on this platform and can report a spurious `ENOENT`, refusing
    // a valid admission. Both steps keep `O_NOFOLLOW`, so a symlink is never
    // followed, and creation is exclusive so no second file is ever written.
    let mut fd = -1;
    let mut last = None;
    for _ in 0..3 {
        // SAFETY: `name` is a live CString with initialized bytes through its
        // terminator, and `directory` owns a live descriptor until `openat`
        // ends.
        fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd >= 0 {
            break;
        }
        let create_error = std::io::Error::last_os_error();
        if create_error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(format!(
                "cannot create admission lock in {}: {create_error}",
                control.display()
            ));
        }
        // SAFETY: as above; the existing lock must still be a regular file.
        fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd >= 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        let retry = error.kind() == std::io::ErrorKind::NotFound;
        last = Some(error);
        if !retry {
            break;
        }
    }
    if fd < 0 {
        // A missing lock is absence, and only absence: a symlinked, unreadable
        // or otherwise unusable lock is an error the caller must treat as
        // unknown custody, never as "no lock".
        let error = last.unwrap_or_else(|| std::io::Error::from_raw_os_error(libc::ENOENT));
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(format!(
            "cannot open admission lock in {}: {error}",
            control.display()
        ));
    }
    // SAFETY: `fd` is a newly opened descriptor owned by this function.
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(format!(
            "{} must be an owner-only file",
            control.join("admission.lock").display()
        ));
    }
    Ok(Some(file))
}

/// Take the internal pool's existing admission lock for metadata updates and
/// command admission. Only the lock file may be created; the control
/// directory must already exist.
pub(crate) fn lock_admission(control: &Path) -> Result<File, String> {
    lock_admission_inner(control, None)
}

pub(super) fn prepare_front_door_pool(pool: &Path, context: &str) -> Result<LedgerView, String> {
    let result = prepare_pool(pool);
    if result != Err(LeaseError::WriterFenceBusy) {
        return result.map_err(|error| lease_error(context, &error));
    }
    // The pool-first open has dropped the pool lock. Wait for admission in
    // the normal order, under the command's budget, and reread the state
    // before publishing. Subsequent pool opens see the durable generation.
    let control = pool.join(MANAGED_NAME);
    {
        let _admission = lock_admission(&control)?;
        publish_writer_generation(&control)?;
    }
    prepare_pool(pool).map_err(|error| lease_error(context, &error))
}

pub(crate) fn publish_writer_generation(control: &Path) -> Result<(), String> {
    fence_admission_writer(control)
        .map_err(|error| lease_error("cannot fence the Oxide writer", &error))
}

pub(super) fn check_writer_generation(control: &Path) -> Result<(), String> {
    check_control_writer(control)
        .map_err(|error| lease_error("cannot open the Oxide writer", &error))
}

pub(super) fn lock_admission_inner(
    control: &Path,
    release_pool: Option<&Path>,
) -> Result<File, String> {
    // An absent control directory names itself; every other failure to reach
    // an open descriptor is the precise error [`open_admission_file`] reports,
    // so "no lock" and "cannot tell" are never confused.
    let Some(file) = open_admission_file(control)? else {
        return Err(format!("{} does not exist", control.display()));
    };
    if let Some(pool) = release_pool {
        let wait_started = Instant::now();
        let active_budget =
            crate::budget::admission_deadline().is_some_and(|expires| expires > wait_started);
        wait_release_admission(
            &file,
            Duration::from_secs(5),
            crate::budget::UNBUDGETED_LOCK_BOUND,
            || active_budget,
            crate::budget::admission_expired,
            || {
                let slot = snapshot(pool)
                    .ok()
                    .and_then(|view| view.evicting_slot_ids.into_iter().next());
                eprintln!("{}", admission_wait_message(slot.as_deref()));
            },
        )?;
    } else {
        crate::budget::lock(&file, "admission")?;
    }
    Ok(file)
}

/// A lost slot is recovered to idle and then evicted; the message says so and
/// names the record it was in, instead of calling a held slot deleted.
pub(super) fn idle_slot_evicted_message(slot_id: &str, record: &str) -> String {
    format!("oxide: evicted idle slot {slot_id}; the slot was {record}")
}

pub(super) fn admission_wait_message(slot: Option<&str>) -> String {
    match slot {
        Some(id) => format!("oxide: waiting for the admission lock (slot {id} is recorded as being deleted; its current writer is unknown)"),
        None => "oxide: waiting for the admission lock (holder unknown)".to_owned(),
    }
}

pub(super) fn wait_release_admission(
    file: &File,
    notice_delay: Duration,
    bound: Duration,
    budgeted: impl Fn() -> bool,
    expired: impl Fn() -> bool,
    notice: impl FnOnce(),
) -> Result<(), String> {
    let started = Instant::now();
    let mut notice = Some(notice);
    loop {
        if flock_nonblocking(file)? {
            return Ok(());
        }
        if budgeted() && expired() {
            return Err(
                "the admission budget expired while waiting for the admission lock".to_owned(),
            );
        }
        // Every release wait is capped at `bound` (120 s) even when an
        // admission budget exceeds it, matching `pool_lock_deadline` and
        // `bounded_lock` (oxide#325).
        let waited = started.elapsed();
        if waited >= bound {
            return Err(format!(
                "gave up waiting for the admission lock after {:.1}s (the {:.0}s bound was reached; the notice above names its holder)",
                waited.as_secs_f64(),
                bound.as_secs_f64()
            ));
        }
        if notice.is_some() && waited >= notice_delay {
            if let Some(notice) = notice.take() {
                notice();
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Test-only: record one active record for `checkout` in `store` and take its
/// lock the way a live command holds it. The returned file keeps it held.
#[cfg(test)]
pub fn hold_record_for_tests(store: &Store, checkout: &Path, id: u64) -> File {
    let mut state = store.load().unwrap();
    let mut entry = Record {
        id,
        identity: "identity".to_owned(),
        compatibility: "key".to_owned(),
        profile: "quick".to_owned(),
        checkout: Some(checkout.to_path_buf()),
        phase: Phase::Running,
        process_group: None,
        last_used: 1,
        holder: Some(Holder {
            pid: 41,
            started_ms: 7,
            lease_holder: "oxide-41-3".to_owned(),
            command: None,
        }),
        lease_id: Some("v1.a.1.1.1".to_owned()),
        pending_recovery: None,
        retired: None,
        lost: None,
        last_used_ms: None,
    };
    entry.phase = Phase::Running;
    state.records.push(entry);
    store.save(&state).unwrap();
    let Probe::Free(file) = probe_lock(&store.lock_path(id), true).unwrap() else {
        panic!("a fresh record lock is free");
    };
    file
}

/// Open an owner-only regular control file without following symlinks.
pub(crate) fn private_file(path: &Path, create: bool) -> Result<Option<File>, String> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW);
    if create {
        options.create(true).mode(0o600);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if !create && error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot open {}: {error}", path.display())),
    };
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(format!("{} must be an owner-only file", path.display()));
    }
    Ok(Some(file))
}

/// The bytes of one private control file, read at most `max` bytes. `Ok(None)`
/// means the file is absent. A file longer than `max` is an error, never a
/// truncated read, so a caller cannot parse a prefix of an oversized document.
pub(super) fn bounded_read(path: &Path, max: u64) -> Result<Option<Vec<u8>>, String> {
    let Some(file) = private_file(path, false)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > max {
        return Err(format!("{} exceeds its byte limit", path.display()));
    }
    Ok(Some(bytes))
}

/// One control state document through the shared parser (`parse_state`), the
/// same acceptance test the writer and the owned-outputs fence use. `Ok(None)`
/// is an absent document; every other failure is an error.
pub(super) fn read_state(path: &Path) -> Result<Option<State>, String> {
    bounded_read(path, MAX_STATE_BYTES)?
        .map(|bytes| parse_state(&bytes))
        .transpose()
}

pub(super) enum Probe {
    Free(File),
    Held,
    Missing,
}

/// Try an exclusive lock without blocking.
pub(super) fn probe_lock(path: &Path, create: bool) -> Result<Probe, String> {
    let Some(file) = private_file(path, create)? else {
        return Ok(Probe::Missing);
    };
    // SAFETY: the descriptor is open for the duration of the call.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(Probe::Free(file));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(Probe::Held)
    } else {
        Err(format!("cannot lock {}: {error}", path.display()))
    }
}

/// A blocking exclusive `flock` for test fixtures only: product waits are bounded.
#[cfg(test)]
pub(crate) fn flock_blocking(file: &File) -> Result<(), String> {
    flock(file, libc::LOCK_EX)
}

/// One nonblocking exclusive `flock` attempt; `true` when the lock was taken.
pub(crate) fn flock_nonblocking(file: &File) -> Result<bool, String> {
    // SAFETY: the descriptor is open for the duration of the call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(false)
    } else {
        Err(format!("cannot lock the control file: {error}"))
    }
}

#[cfg(test)]
pub(super) fn flock(file: &File, operation: libc::c_int) -> Result<(), String> {
    loop {
        // SAFETY: the descriptor is open for the duration of the call.
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(format!("cannot take the Oxide admission lock: {error}"));
        }
    }
}

pub(crate) fn statfs(path: &Path) -> Option<libc::statfs> {
    let mut bytes = path.as_os_str().as_bytes().to_vec();
    bytes.push(0);
    let mut sample = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `bytes` is NUL-terminated and `sample` is valid for writes.
    let result = unsafe { libc::statfs(bytes.as_ptr().cast(), sample.as_mut_ptr()) };
    // SAFETY: statfs initialised `sample` when it returned 0.
    (result == 0).then(|| unsafe { sample.assume_init() })
}

pub(super) fn lease_error(context: &str, error: &LeaseError) -> String {
    if *error == LeaseError::WriterUpdated {
        WRITER_UPDATED.set(true);
    }
    format!("{context}: {} ({error})", error.reason())
}

thread_local! {
    // Store callers use String errors and some add context or prefer a budget
    // result. Preserve the typed lock refusal through those front-door paths.
    pub(super) static WRITER_UPDATED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether this command saw a writer-update refusal, consumed by the read.
///
/// `main` asks exactly once, after the command ran, so a refusal is still
/// reported with its own exit code. Taking the flag keeps it from poisoning
/// every later command on this thread, which a plain read did after one
/// `WriterUpdated` (oxide#337).
pub(crate) fn writer_updated() -> bool {
    WRITER_UPDATED.take()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::PathBuf;

    fn fixture(label: &str) -> PathBuf {
        let base = std::env::temp_dir();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let root = base.join(format!(
            "oxide-control-io-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("fixture");
        root
    }

    /// Regression: `create_dir` followed by `set_permissions` left a window in
    /// which a new directory carried the umask-controlled mode. The mode is set
    /// at creation now, so even a fully permissive umask cannot widen it.
    #[test]
    fn new_private_directories_are_owner_only_from_creation() {
        let root = fixture("private-mode");
        // SAFETY: `umask` is process-wide. The adoption suite runs with
        // `--test-threads=1`, and the previous value is restored immediately.
        let previous = unsafe { libc::umask(0) };
        let nested = create_parents(&root.join("a/b/c"));
        let private = ensure_private_dir(&root.join("private"));
        unsafe { libc::umask(previous) };
        nested.expect("the ancestors");
        private.expect("the private directory");
        for path in [
            root.join("a"),
            root.join("a/b"),
            root.join("a/b/c"),
            root.join("private"),
        ] {
            let mode = fs::symlink_metadata(&path).expect("directory").mode() & 0o777;
            assert_eq!(mode, 0o700, "{} has mode {mode:o}", path.display());
        }
        fs::remove_dir_all(&root).expect("remove own fixture");
    }

    #[test]
    fn an_existing_directory_is_left_as_it_is() {
        let root = fixture("existing");
        let existing = root.join("existing");
        fs::create_dir(&existing).expect("fixture directory");
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o755)).expect("mode");
        create_parents(&existing).expect("existing ancestors are accepted");
        ensure_private_dir(&existing).expect_err("a readable directory is not private");
        assert_eq!(
            fs::symlink_metadata(&existing).expect("directory").mode() & 0o777,
            0o755
        );
        fs::remove_dir_all(&root).expect("remove own fixture");
    }
}
