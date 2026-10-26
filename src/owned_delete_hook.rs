//! Debug-only deterministic race injection for the owned-tree deletion seam
//! (oxide#155).
//!
//! The race this feature bounds is "Finder created metadata after the caller's
//! listing and before the removal". A proof cannot schedule Finder, so a debug
//! build can create exactly one new entry in the directory being removed, at
//! that boundary and nowhere else. It is absent from release builds, it only
//! arms inside the shared proof supervisor's own layout
//! ([`crate::proof::evidence`]), and every firing is appended to
//! `finder-race-fired` in the supervisor's evidence directory so a proof can
//! show the injection really happened.
//!
//! `OXIDE_TEST_FINDER_RACE` holds one or more comma-separated
//! `<label-pattern>:<kind>[:repeat]` specs. A label is the leaf name of the
//! directory being removed. A pattern is `*`, a `prefix*`, or an exact name.
//! Kinds:
//!
//! - `metadata`: tolerated Finder metadata (`.DS_Store` and `._member`);
//! - `foreign`: one regular file with an ordinary name;
//! - `link`: a `.DS_Store`-named symbolic link to `.` (the directory itself);
//! - `directory`: a `.DS_Store`-named directory;
//! - `repeat`: tolerated Finder metadata on every attempt, so the fixed bound
//!   is exercised. `repeat` without the suffix fires on every round.
//!
//! Unit tests reach the same seam through `crate::test_fs`, which arms one
//! firing in-process instead of through the environment.

use std::ffi::OsStr;
use std::io::Write;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::ffi::OsStrExt;
use std::sync::Mutex;

use super::entry_label;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Metadata,
    Foreign,
    Link,
    Directory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Once,
    Every,
}

/// Create this round's injected entry, if the seam is armed for `label`.
pub(super) fn race(dir: BorrowedFd<'_>, label: &OsStr) {
    #[cfg(test)]
    if let Some(kind) = crate::test_fs::race_firing(label).and_then(parse_kind) {
        // The in-process arm owns its own fire count, so one test never
        // exhausts another's.
        create(dir, kind);
        record(label, kind);
        return;
    }
    let Some((kind, mode)) = planned(label) else {
        return;
    };
    if mode == Mode::Once && !first_firing(label, kind) {
        return;
    }
    create(dir, kind);
    record(label, kind);
}

fn planned(label: &OsStr) -> Option<(Kind, Mode)> {
    let specs = std::env::var("OXIDE_TEST_FINDER_RACE").ok()?;
    // The supervisor's own layout must be armed: this hook exists only for a
    // proof run, never for an ordinary debug command.
    crate::proof::evidence()?;
    for spec in specs.split(',') {
        let mut parts = spec.split(':');
        let pattern = parts.next()?;
        let kind = parse_kind(parts.next()?)?;
        let mode = match parts.next() {
            Some("repeat") => Mode::Every,
            Some(_) => return None,
            None => Mode::Once,
        };
        if parts.next().is_some() {
            return None;
        }
        if matches(pattern, label) {
            return Some((kind, mode));
        }
    }
    None
}

fn parse_kind(text: &str) -> Option<Kind> {
    match text.trim() {
        "metadata" | "repeat" => Some(Kind::Metadata),
        "foreign" => Some(Kind::Foreign),
        "link" => Some(Kind::Link),
        "directory" => Some(Kind::Directory),
        _ => None,
    }
}

fn matches(pattern: &str, label: &OsStr) -> bool {
    if pattern == "*" {
        return true;
    }
    let label = String::from_utf8_lossy(label.as_bytes());
    match pattern.strip_suffix('*') {
        Some(prefix) => label.starts_with(prefix),
        None => label == pattern,
    }
}

/// The first firing for one label and kind, so a `Once` spec creates its entry
/// exactly once even when the caller retries the removal. A second directory
/// with the same leaf name (a nested `debug` directory, say) keeps its own
/// first firing.
fn first_firing(label: &OsStr, kind: Kind) -> bool {
    static FIRED: Mutex<Option<Vec<(Vec<u8>, Kind)>>> = Mutex::new(None);
    let key = (label.as_bytes().to_vec(), kind);
    let Ok(mut fired) = FIRED.lock() else {
        return false;
    };
    let fired = fired.get_or_insert_with(Vec::new);
    if fired.contains(&key) {
        return false;
    }
    fired.push(key);
    true
}

fn create(dir: BorrowedFd<'_>, kind: Kind) {
    match kind {
        Kind::Metadata => {
            write_file(dir, ".DS_Store", 6_148);
            write_file(dir, "._member", 4_096);
        }
        Kind::Foreign => write_file(dir, "foreign.txt", 16),
        Kind::Link => {
            let target = c".";
            let name = c".DS_Store";
            // SAFETY: `dir` and both C strings are live for the call; the link
            // is created inside the directory only if it is absent.
            unsafe { libc::symlinkat(target.as_ptr(), dir.as_raw_fd(), name.as_ptr()) };
        }
        Kind::Directory => {
            let name = c".DS_Store";
            // SAFETY: `dir` and the C string are live for the call.
            unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) };
        }
    }
}

fn write_file(dir: BorrowedFd<'_>, name: &str, bytes: usize) {
    let Ok(name) = std::ffi::CString::new(name) else {
        return;
    };
    // SAFETY: `dir` and the C string are live for the call.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return;
    }
    // SAFETY: `fd` is a newly owned descriptor; the payload is ownership-free.
    let owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
    let payload = vec![b'd'; bytes];
    // `File::from` takes the descriptor; the write is best effort.
    let mut file = std::fs::File::from(owned);
    let _ = file.write_all(&payload);
}

fn record(label: &OsStr, kind: Kind) {
    let Some(evidence) = crate::proof::evidence() else {
        return;
    };
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(evidence.join("finder-race-fired"))
        .and_then(|mut file| writeln!(file, "{kind:?} {}", entry_label(label)));
}

use std::os::fd::FromRawFd as _;
