//! Descriptor-relative deletion of a verified owned tree (oxide#155).
//!
//! Two product paths delete a whole tree they have positively verified: a
//! whole-slot eviction and its `Evicting` reconciliation in [`crate::lease`],
//! and the outputs-prune quarantine pass. Both list a directory and then
//! remove it, and on a volume someone has browsed Finder can create
//! `.DS_Store` or an AppleDouble `._*` sidecar in between, so the removal
//! fails `ENOTEMPTY`.
//!
//! The recovery here is deliberately narrow:
//!
//! - only on `ENOTEMPTY`, after the caller's own listing succeeded, and only
//!   through the caller's held, positively verified descriptors: the parent
//!   directory, the child's name in it, and the verified child itself. Nothing
//!   is reopened or rebound by a path, so a name replaced during the race is
//!   never mistaken for the verified directory;
//! - the name is revalidated against the held descriptor at every effect
//!   boundary — before the removal, again before the drain, and again before
//!   each entry's unlink. A replacement is reported, never removed and never
//!   drained;
//! - only entries that are already tolerated Finder metadata by the oxide#105
//!   rule ([`is_finder_metadata`] and a no-follow regular file, on the
//!   directory's device, owned by the caller — never a link, a directory,
//!   another device, another owner or a near-miss name). Each is verified by
//!   `fstatat`, its `(device, inode)` identity is kept, and it is verified
//!   again immediately before its own unlink;
//! - only within one fixed finite bound: [`FINDER_RACE_ATTEMPTS`] removal
//!   attempts with [`FINDER_RACE_PAUSE`] between them, and at most
//!   [`FINDER_RACE_MAX_NAMES`] names in one re-list.
//!
//! Any other new entry, any entry whose identity cannot be verified, and an
//! exhausted bound are reported to the caller, which keeps its own refusal or
//! its pending deletion record. Nothing here deletes arbitrary new content,
//! widens ownership, follows a link, removes a foreign directory or turns an
//! unfinished deletion into a success.
//!
//! Both callers keep their own authority boundaries and their own journals:
//! the lease records `Evicting` in its durable ledger before the deletion and
//! only clears the record after a positive removal, and the prune journals
//! `deleting`/`marker_last`/`deleted` around its own bounded pass.

use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::time::Duration;

#[cfg(debug_assertions)]
#[path = "owned_delete_hook.rs"]
mod hook;

/// Release builds carry no injection at all: the seam behaves exactly as
/// production, with no hook module, no environment read and no extra code.
#[cfg(not(debug_assertions))]
mod hook {
    use std::ffi::OsStr;
    use std::os::fd::BorrowedFd;

    pub(super) fn race(_dir: BorrowedFd<'_>, _label: &OsStr) {}
}

#[cfg(test)]
#[path = "owned_delete_tests.rs"]
mod tests;

/// Total removal attempts one held directory may take: the first attempt plus
/// the re-listed retries. Exhaustion is a refusal or a pending intent, never a
/// success.
pub const FINDER_RACE_ATTEMPTS: u32 = 3;
/// Short pause between attempts, so a writer that is mid-create is not polled
/// in a tight loop. It is part of the fixed bound, not a wait for Finder.
pub const FINDER_RACE_PAUSE: Duration = Duration::from_millis(10);
/// Most names one re-list may see and unlink. Recreated Finder metadata is one
/// or two entries; a directory that gained more content than this is not this
/// race and is refused.
pub const FINDER_RACE_MAX_NAMES: usize = 64;

/// Longest entry name, in bytes, a diagnostic prints before eliding.
const MAX_ENTRY_LABEL_BYTES: usize = 64;

/// The descriptor-only open flag that reaches a directory without requiring
/// read permission: `O_SEARCH` on Darwin (it includes `O_DIRECTORY`), `O_PATH`
/// on Linux. Both never follow a link with `O_NOFOLLOW`, so a repaired entry is
/// always the entry itself. `fchmod` is supported on the Darwin descriptor; on
/// Linux an `O_PATH` descriptor cannot be chmodded, so the repair returns that
/// error and the caller keeps its refusal.
#[cfg(target_os = "macos")]
const SEARCH_ONLY: libc::c_int = libc::O_SEARCH;
#[cfg(not(target_os = "macos"))]
const SEARCH_ONLY: libc::c_int = libc::O_PATH;

/// Finder metadata a verified tree tolerates: exactly `.DS_Store` (Finder's
/// Desktop Services store; Apple's `com.apple.desktopservices`
/// `DSDontWriteNetworkStores` / `DSDontWriteUSBStores` preferences govern where
/// Finder writes it) and AppleDouble sidecars `._<name>` with a nonempty
/// `<name>` (dot_clean(1): "Merge ._* files with corresponding native files").
/// The name alone grants nothing: callers still require an owned regular file
/// on the tree's device, and its bytes stay in accounting.
pub fn is_finder_metadata(name: &OsStr) -> bool {
    let name = name.as_bytes();
    name == b".DS_Store" || (name.len() > 2 && name.starts_with(b"._"))
}

/// Whether a no-follow `metadata` is a Finder metadata file the tree may hold:
/// a regular caller-owned file on `device`.
pub fn is_tolerated_metadata_file(metadata: &fs::Metadata, device: u64) -> bool {
    metadata.file_type().is_file() && metadata.dev() == device && metadata.uid() == caller_uid()
}

fn caller_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// An entry name safe to print: printable ASCII except `"` and `\` as is,
/// every other byte (control, non-ASCII, non-UTF-8) as `\xNN`, at most
/// [`MAX_ENTRY_LABEL_BYTES`] input bytes followed by `...` when longer.
pub(crate) fn entry_label(name: &OsStr) -> String {
    use std::fmt::Write as _;
    let bytes = name.as_bytes();
    let mut label = String::new();
    for &byte in bytes.iter().take(MAX_ENTRY_LABEL_BYTES) {
        if byte == b' ' || (byte.is_ascii_graphic() && byte != b'"' && byte != b'\\') {
            label.push(char::from(byte));
        } else {
            let _ = write!(label, "\\x{byte:02x}");
        }
    }
    if bytes.len() > MAX_ENTRY_LABEL_BYTES {
        label.push_str("...");
    }
    label
}

/// One bounded removal round of a held, verified directory.
#[derive(Debug)]
pub enum Rmdir {
    /// The directory is removed.
    Removed,
    /// `ENOTEMPTY`, and every newly listed entry was verified tolerated Finder
    /// metadata, which has been unlinked. The caller may retry the same
    /// removal against its own remaining bound.
    Drained { entries: u64 },
    /// The fixed bound ran out with the directory still not empty. Reported
    /// only by [`rmdir_retrying_finder_metadata`]; a caller that owns its own
    /// bound reports exhaustion from its own attempt count.
    Exhausted { entries: u64 },
    /// The name does not exist any more. What that means is the caller's
    /// decision: a directory already gone inside a bounded pass is finished
    /// work, while a journaled quarantine leaf that is missing without
    /// positive deletion proof is never completion.
    Vanished,
    /// The name no longer names the held, verified directory (another inode,
    /// another device or another kind of entry). Nothing was removed and
    /// nothing was drained.
    Replaced { entry: String },
    /// `ENOTEMPTY` and the directory holds a newly observed entry that is not
    /// tolerated Finder metadata, or whose identity could not be verified or
    /// changed before its own unlink. Its escaped name is reported.
    Refused { entry: String },
    /// `ENOTEMPTY` and one re-list saw more than [`FINDER_RACE_MAX_NAMES`]
    /// names: more new content than this race explains. Nothing was deleted.
    RefusedMany { entries: u64 },
    /// The removal failed for any other reason.
    Failed { error: io::Error },
}

/// Remove `name` — a directory named in `parent` — and, when the removal races
/// Finder metadata, drain only verified tolerated metadata from the held
/// `dir` and report that a retry is allowed. One round: the caller owns the
/// bound and its own journal.
///
/// `label` is the directory's own leaf name. It is used for diagnostics and
/// for the debug-only proof hook; it never authorizes an effect.
pub fn rmdir_draining_finder_metadata(
    parent: BorrowedFd<'_>,
    name: &CStr,
    dir: BorrowedFd<'_>,
    label: &OsStr,
) -> Rmdir {
    hook::race(dir, label);
    // The name must still name exactly the held, verified directory before any
    // effect is attempted, so a replacement is never removed.
    match held_name(parent, name, dir) {
        Ok(HeldName::Held) => {}
        Ok(HeldName::Vanished) => return Rmdir::Vanished,
        Ok(HeldName::Different) => {
            return Rmdir::Replaced {
                entry: entry_label(label),
            }
        }
        Err(error) => return Rmdir::Failed { error },
    }
    // SAFETY: `parent` and the C string are live for the call; the flag asks
    // for the directory `name` itself and never for its contents.
    let status = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) };
    if status == 0 {
        return Rmdir::Removed;
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::NotFound {
        return Rmdir::Vanished;
    }
    if error.raw_os_error() != Some(libc::ENOTEMPTY) {
        return Rmdir::Failed { error };
    }
    // Revalidate at the drain boundary: the entries about to be unlinked must
    // belong to the held directory, not to a similar name.
    match held_name(parent, name, dir) {
        Ok(HeldName::Held) => {}
        Ok(HeldName::Vanished) => return Rmdir::Vanished,
        Ok(HeldName::Different) => {
            return Rmdir::Replaced {
                entry: entry_label(label),
            }
        }
        Err(error) => return Rmdir::Failed { error },
    }
    let device = match directory_device(dir) {
        Ok(device) => device,
        Err(error) => return Rmdir::Failed { error },
    };
    match drain_finder_metadata(parent, name, dir, device) {
        Ok(entries) => Rmdir::Drained { entries },
        Err(outcome) => outcome,
    }
}

/// Remove a held, verified directory through [`rmdir_draining_finder_metadata`]
/// within the fixed bound, pausing between attempts.
pub fn rmdir_retrying_finder_metadata(
    parent: BorrowedFd<'_>,
    name: &CStr,
    dir: BorrowedFd<'_>,
    label: &OsStr,
) -> Rmdir {
    let mut attempt = 0;
    let mut drained = 0;
    loop {
        attempt += 1;
        if attempt > 1 {
            std::thread::sleep(FINDER_RACE_PAUSE);
        }
        match rmdir_draining_finder_metadata(parent, name, dir, label) {
            Rmdir::Drained { entries } => {
                drained += entries;
                if attempt >= FINDER_RACE_ATTEMPTS {
                    return Rmdir::Exhausted { entries: drained };
                }
            }
            other => return other,
        }
    }
}

/// Whether a held directory holds nothing but tolerated Finder metadata. It
/// deletes nothing. A directory that holds more than [`FINDER_RACE_MAX_NAMES`]
/// names, or a name that cannot be inspected, is an error.
pub fn only_finder_metadata(dir: BorrowedFd<'_>) -> io::Result<bool> {
    let device = directory_device(dir)?;
    let names = match list_names(dir, FINDER_RACE_MAX_NAMES)? {
        Names::Listed(names) => names,
        Names::TooMany(count) => {
            return Err(io::Error::other(format!(
                "more than {FINDER_RACE_MAX_NAMES} entries ({count})"
            )));
        }
    };
    for name in names {
        match metadata_identity(dir, &name, device) {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}

/// One verified tolerated-metadata entry: its name and the exact
/// `(device, inode)` identity that must still hold immediately before its
/// unlink.
struct VerifiedEntry {
    name: CString,
    device: u64,
    inode: u64,
}

fn drain_finder_metadata(
    parent: BorrowedFd<'_>,
    name: &CStr,
    dir: BorrowedFd<'_>,
    device: u64,
) -> Result<u64, Rmdir> {
    // Revalidate the directory binding one last time before anything in it is
    // deleted, and again before every single unlink below: the entries must
    // belong to the held directory the caller verified, not to a name that now
    // points somewhere else.
    match held_name(parent, name, dir) {
        Ok(HeldName::Held) => {}
        Ok(HeldName::Vanished) => return Err(Rmdir::Vanished),
        Ok(HeldName::Different) => {
            return Err(Rmdir::Replaced {
                entry: entry_label(OsStr::from_bytes(name.to_bytes())),
            })
        }
        Err(error) => return Err(Rmdir::Failed { error }),
    }
    let mut names = match list_names(dir, FINDER_RACE_MAX_NAMES) {
        Ok(Names::Listed(names)) => names,
        Ok(Names::TooMany(entries)) => return Err(Rmdir::RefusedMany { entries }),
        Err(error) => return Err(Rmdir::Failed { error }),
    };
    // Deterministic order: the first refusal names the same entry whatever the
    // filesystem's directory order is.
    names.sort();
    // Verify every newly observed name before the first unlink, remembering
    // the exact identity each one had. A name that is not tolerated metadata
    // refuses the whole round with nothing deleted.
    let mut verified = Vec::with_capacity(names.len());
    for name in names {
        match metadata_identity(dir, &name, device) {
            Ok(Some(identity)) => verified.push(VerifiedEntry {
                name,
                device: identity.0,
                inode: identity.1,
            }),
            Ok(None) => {
                return Err(Rmdir::Refused {
                    entry: entry_label(OsStr::from_bytes(name.to_bytes())),
                })
            }
            // The writer's own entry vanished before we looked: not our
            // deletion and not this round's work.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(Rmdir::Failed { error }),
        }
    }
    let mut entries = 0;
    for entry in &verified {
        // The directory binding is revalidated for every entry, not only once
        // per round: a name rebound while the drain runs stops it before the
        // next unlink.
        match held_name(parent, name, dir) {
            Ok(HeldName::Held) => {}
            Ok(HeldName::Vanished) => return Err(Rmdir::Vanished),
            Ok(HeldName::Different) => {
                return Err(Rmdir::Replaced {
                    entry: entry_label(OsStr::from_bytes(name.to_bytes())),
                })
            }
            Err(error) => return Err(Rmdir::Failed { error }),
        }
        // Revalidate the exact entry immediately before its unlink: a
        // metadata-named replacement (a link, a directory, another device or
        // another inode) is refused and left alone.
        match metadata_identity(dir, &entry.name, device) {
            Ok(Some((device, inode))) if device == entry.device && inode == entry.inode => {}
            Ok(Some(_)) | Ok(None) => {
                return Err(Rmdir::Refused {
                    entry: entry_label(OsStr::from_bytes(entry.name.to_bytes())),
                })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(Rmdir::Failed { error }),
        }
        #[cfg(test)]
        crate::test_fs::run_before_unlink();
        // SAFETY: `dir` and the C string are live for the call. This unlinks
        // the name itself; a symlink is never followed by `unlinkat`.
        let status = unsafe { libc::unlinkat(dir.as_raw_fd(), entry.name.as_ptr(), 0) };
        if status == 0 {
            entries += 1;
            continue;
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound {
            // It vanished again between the revalidation and the unlink.
            continue;
        }
        return Err(Rmdir::Failed { error });
    }
    Ok(entries)
}

/// The identity of one entry of `dir` when it is tolerated Finder metadata: a
/// no-follow regular file, on this directory's device, owned by the caller,
/// with a tolerated name. `Ok(None)` means the name exists but is not
/// tolerated; `NotFound` means it is gone.
fn metadata_identity(
    dir: BorrowedFd<'_>,
    name: &CStr,
    device: u64,
) -> io::Result<Option<(u64, u64)>> {
    let stat = stat_at(dir, name)?;
    if !is_finder_metadata(OsStr::from_bytes(name.to_bytes()))
        || stat_mode(&stat) != libc::S_IFREG
        || stat_device(&stat) != device
        || stat_uid(&stat) != caller_uid()
    {
        return Ok(None);
    }
    Ok(Some((stat_device(&stat), stat_ino(&stat))))
}

/// What one name in a held parent currently denotes, relative to the held
/// child descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HeldName {
    /// The name is that directory: same kind, device and inode.
    Held,
    /// The name is taken, but by something else.
    Different,
    /// The name is free.
    Vanished,
}

fn held_name(parent: BorrowedFd<'_>, name: &CStr, dir: BorrowedFd<'_>) -> io::Result<HeldName> {
    let (device, inode) = descriptor_identity(dir)?;
    match stat_at(parent, name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(HeldName::Vanished),
        Err(error) => Err(error),
        Ok(stat) => Ok(
            if stat_mode(&stat) == libc::S_IFDIR
                && stat_device(&stat) == device
                && stat_ino(&stat) == inode
            {
                HeldName::Held
            } else {
                HeldName::Different
            },
        ),
    }
}

enum Names {
    Listed(Vec<CString>),
    TooMany(u64),
}

fn list_names(dir: BorrowedFd<'_>, max: usize) -> io::Result<Names> {
    // A fresh open of the held directory, not `dup`: `dup` shares the
    // descriptor's read offset, so a second listing of the same directory
    // would start at its end and see nothing. This listing may run again after
    // a drain, and `only_finder_metadata` may follow another walk.
    let name = c".";
    // SAFETY: `dir` and the C string are live for the call.
    let fresh = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fresh < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fresh` is a live descriptor; `fdopendir` owns it now.
    let stream = unsafe { libc::fdopendir(fresh) };
    if stream.is_null() {
        let error = io::Error::last_os_error();
        // SAFETY: the failed `fdopendir` left `fresh` owned here.
        unsafe { libc::close(fresh) };
        return Err(error);
    }
    let mut names = Vec::new();
    let mut seen = 0_u64;
    let mut too_many = None;
    loop {
        // SAFETY: `stream` is a live directory stream.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        // SAFETY: `d_name` is a live NUL-terminated array for this entry.
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        seen += 1;
        if seen as usize > max {
            too_many = Some(seen);
            break;
        }
        names.push(name.to_owned());
    }
    // SAFETY: `stream` is live and closed exactly once.
    unsafe { libc::closedir(stream) };
    Ok(match too_many {
        Some(count) => Names::TooMany(count),
        None => Names::Listed(names),
    })
}

fn stat_at(dir: BorrowedFd<'_>, name: &CStr) -> io::Result<libc::stat> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `dir` and the C string are live for the call; `NOFOLLOW` never
    // resolves a link at the last component.
    let status = unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            name.as_ptr(),
            &mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(stat)
}

fn directory_device(dir: BorrowedFd<'_>) -> io::Result<u64> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `dir` is live for the call.
    if unsafe { libc::fstat(dir.as_raw_fd(), &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if stat_mode(&stat) != libc::S_IFDIR {
        return Err(io::Error::other(
            "a held deletion target is not a directory",
        ));
    }
    Ok(stat_device(&stat))
}

fn stat_mode(stat: &libc::stat) -> libc::mode_t {
    stat.st_mode & libc::S_IFMT
}

fn stat_device(stat: &libc::stat) -> u64 {
    u64::try_from(stat.st_dev).unwrap_or(u64::MAX)
}

fn stat_ino(stat: &libc::stat) -> u64 {
    u64::try_from(stat.st_ino).unwrap_or(u64::MAX)
}

fn stat_uid(stat: &libc::stat) -> u32 {
    // `uid_t` is `u32` on every supported target.
    stat.st_uid
}

/// Open `name` in `parent` as a directory, never through a link. Used by the
/// callers' own recursion so every level stays descriptor-relative.
pub fn open_directory_at(parent: BorrowedFd<'_>, name: &CStr) -> io::Result<OwnedFd> {
    // SAFETY: `parent` and the C string are live for the call.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a newly owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Open `name` in `parent` as a directory *without* requiring read permission
/// and without following a link (`O_SEARCH`, which is execute/search only).
/// This is how an owned directory the owner left unreadable is reached at all:
/// the descriptor names that entry itself, so nothing below ever repairs
/// permissions through a path or a link.
pub fn open_directory_for_repair(parent: BorrowedFd<'_>, name: &CStr) -> io::Result<OwnedFd> {
    // SAFETY: `parent` and the C string are live for the call.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            SEARCH_ONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a newly owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Restore the owner's own access through the already-held descriptor of that
/// verified directory. Only this entry's own mode gains `0o700`, and a link is
/// never involved: the descriptor cannot be a link and its identity has been
/// revalidated by the caller. Where the platform refuses `fchmod` on a
/// search-only descriptor, this returns that error and the caller keeps its
/// refusal instead of repairing anything.
pub fn restore_owner_access(dir: BorrowedFd<'_>) -> io::Result<()> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `dir` is live for the call.
    if unsafe { libc::fstat(dir.as_raw_fd(), &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if stat_mode(&stat) != libc::S_IFDIR {
        return Err(io::Error::other(
            "a repaired directory descriptor is not a directory",
        ));
    }
    let mode = (stat.st_mode & 0o7777) | 0o700;
    // SAFETY: `dir` is a live descriptor for the verified directory.
    if unsafe { libc::fchmod(dir.as_raw_fd(), mode) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Every name a held directory lists, in directory order, as a caller's own
/// recursion needs it. No cap: the caller already bounded its own walk, and
/// this only enumerates a directory it holds by descriptor. The Finder-race
/// recovery uses its own capped listing.
pub fn names_of(dir: BorrowedFd<'_>) -> io::Result<Vec<OsString>> {
    match list_names(dir, usize::MAX)? {
        Names::Listed(names) => Ok(names
            .into_iter()
            .map(|name| OsString::from_vec(name.into_bytes()))
            .collect()),
        Names::TooMany(count) => Err(io::Error::other(format!(
            "directory entry count {count} exceeded its bound"
        ))),
    }
}

/// The `(device, inode)` identity of a held descriptor.
pub fn descriptor_identity(fd: BorrowedFd<'_>) -> io::Result<(u64, u64)> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `fd` is live for the call.
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((
        stat_device(&stat),
        u64::try_from(stat.st_ino).unwrap_or(u64::MAX),
    ))
}
