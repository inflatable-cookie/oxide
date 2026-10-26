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
/// Captured entries are renamed to a private name carrying 128 random bits.
pub const CAPTURE_TAG_BYTES: usize = 16;
/// Every captured name carries this prefix, so a leftover capture is
/// recognisable in a directory listing.
pub const CAPTURE_PREFIX: &str = ".oxide-capture-";
/// Tolerated Finder metadata is small; a captured regular file larger than
/// this is not treated as metadata and is put back.
pub const FINDER_METADATA_MAX_BYTES: u64 = 4 * 1024 * 1024;

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

/// Why a caller stopped a drain between two verified entries: its own bounded
/// check (volume, reserve, deadline, journal) failed. The caller keeps the
/// reason, so its refusal or pending intent is unchanged by the seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DrainStop {
    Stopped(String),
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
    /// A captured entry was not the observed object. It was either put back
    /// under its original name or, when a racer had already taken that name,
    /// left under the private name. Both names are reported; nothing was
    /// deleted.
    Mismatch {
        original: String,
        private: String,
        restored: bool,
        kind: &'static str,
    },
    /// A capture rename failed, so nothing was removed and the caller stops
    /// pending.
    CaptureFailed { error: io::Error },
    /// The caller's between-entries check stopped the drain. The entries it
    /// unlinked before stopping stay deleted; every remaining entry, and the
    /// directory itself, are left exactly as they were found.
    Stopped { reason: String },
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
    between: &mut dyn FnMut() -> Result<(), DrainStop>,
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
    match drain_finder_metadata(parent, name, dir, device, between) {
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
    between: &mut dyn FnMut() -> Result<(), DrainStop>,
) -> Rmdir {
    let mut attempt = 0;
    let mut drained = 0;
    loop {
        attempt += 1;
        if attempt > 1 {
            std::thread::sleep(FINDER_RACE_PAUSE);
        }
        match rmdir_draining_finder_metadata(parent, name, dir, label, between) {
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
    size: u64,
    /// 1-based position in this round's verified list, for the test seams.
    ordinal: u32,
}

/// One entry moved to an unguessable private name inside the same directory.
///
/// The object is captured *before* it is verified, so verification and removal
/// both act on a name the deleting process itself just created: a same-user
/// racer can no longer substitute anything between a check and a removal,
/// because the name being removed is not one the racer knows.
#[derive(Debug)]
pub struct Capture {
    /// The private name now holding the object.
    pub private: CString,
    /// The name the object was observed under.
    pub original: CString,
}

/// Facts about one directory entry, read with `fstatat` and `AT_SYMLINK_NOFOLLOW`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EntryFacts {
    pub device: u64,
    pub inode: u64,
    pub mode: libc::mode_t,
    pub uid: u32,
    pub size: u64,
}

impl EntryFacts {
    pub fn is_regular(&self) -> bool {
        self.mode == libc::S_IFREG
    }

    pub fn is_directory(&self) -> bool {
        self.mode == libc::S_IFDIR
    }

    pub fn is_symlink(&self) -> bool {
        self.mode == libc::S_IFLNK
    }

    /// The permission bits of the observed entry.
    pub fn mode_bits(&self) -> libc::mode_t {
        self.mode & 0o7777
    }
}

/// One exclusive rename inside a single held directory: never across
/// directories, never replacing an existing name.
fn rename_exclusive(dir: BorrowedFd<'_>, from: &CStr, to: &CStr) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: the descriptor and both C strings are live for the call.
        let status = unsafe {
            libc::renameatx_np(
                dir.as_raw_fd(),
                from.as_ptr(),
                dir.as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        if status == 0 {
            return Ok(());
        }
        return Err(io::Error::last_os_error());
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: the descriptor and both C strings are live for the call.
        let status = unsafe {
            libc::renameat2(
                dir.as_raw_fd(),
                from.as_ptr(),
                dir.as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if status == 0 {
            return Ok(());
        }
        return Err(io::Error::last_os_error());
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (dir, from, to);
        Err(io::Error::from_raw_os_error(libc::ENOTSUP))
    }
}

fn random_tag() -> io::Result<String> {
    let mut value = vec![0_u8; CAPTURE_TAG_BYTES];
    fs::File::open("/dev/urandom")
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut value))
        .map_err(|_| io::Error::other("cannot read a random capture tag"))?;
    use std::fmt::Write as _;
    let mut tag = String::with_capacity(CAPTURE_TAG_BYTES * 2);
    for byte in value {
        let _ = write!(&mut tag, "{byte:02x}");
    }
    Ok(tag)
}

/// Capture `name` under a fresh private name in the same directory. Retries a
/// name collision a bounded number of times; any other rename error is the
/// caller's stop.
pub fn capture_entry(dir: BorrowedFd<'_>, name: &CStr) -> io::Result<Capture> {
    for _ in 0..8 {
        let private = CString::new(format!("{CAPTURE_PREFIX}{}", random_tag()?))
            .map_err(|_| io::Error::other("capture name"))?;
        match rename_exclusive(dir, name, &private) {
            Ok(()) => {
                return Ok(Capture {
                    private,
                    original: name.to_owned(),
                })
            }
            Err(error) if error.raw_os_error() == Some(libc::EEXIST) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::from_raw_os_error(libc::EEXIST))
}

/// `fstatat` facts for one name in a held directory, never following a link.
pub fn observe_entry(dir: BorrowedFd<'_>, name: &CStr) -> io::Result<EntryFacts> {
    let stat = stat_at(dir, name)?;
    Ok(EntryFacts {
        device: stat_device(&stat),
        inode: stat_ino(&stat),
        mode: stat_mode(&stat),
        uid: stat_uid(&stat),
        size: u64::try_from(stat.st_size).unwrap_or(u64::MAX),
    })
}

/// The captured object must be the object that was observed: same device, same
/// inode and same type. Anything else is a mismatch and is never removed.
pub fn verify_captured(
    dir: BorrowedFd<'_>,
    capture: &Capture,
    observed: &EntryFacts,
) -> Result<EntryFacts, &'static str> {
    let captured = observe_entry(dir, &capture.private).map_err(|_| "unreadable capture")?;
    if captured.device != observed.device {
        return Err("captured object is on another device");
    }
    if captured.inode != observed.inode {
        return Err("captured object is not the observed entry");
    }
    if captured.mode != observed.mode {
        return Err("captured object changed type");
    }
    Ok(captured)
}

/// Put a captured object back under its original name, exclusively. A racer
/// that took that name again makes this fail; the caller then leaves the
/// private name in place and reports both.
pub fn put_back(dir: BorrowedFd<'_>, capture: &Capture) -> io::Result<()> {
    rename_exclusive(dir, &capture.private, &capture.original)
}

/// Remove the captured regular file or link by its private name only.
pub fn unlink_captured(dir: BorrowedFd<'_>, capture: &Capture) -> io::Result<()> {
    // SAFETY: the descriptor and the private C string are live for the call.
    if unsafe { libc::unlinkat(dir.as_raw_fd(), capture.private.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Remove the captured (already emptied) directory by its private name only.
pub fn remove_captured_directory(dir: BorrowedFd<'_>, capture: &Capture) -> io::Result<()> {
    // SAFETY: the descriptor and the private C string are live for the call.
    if unsafe {
        libc::unlinkat(
            dir.as_raw_fd(),
            capture.private.as_ptr(),
            libc::AT_REMOVEDIR,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn drain_finder_metadata(
    parent: BorrowedFd<'_>,
    name: &CStr,
    dir: BorrowedFd<'_>,
    device: u64,
    between: &mut dyn FnMut() -> Result<(), DrainStop>,
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
                size: identity.2,
                ordinal: u32::try_from(verified.len() + 1).unwrap_or(u32::MAX),
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
        // per round: a name rebound while the drain runs stops it.
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
        // Capture first: from here on the object lives under a private name
        // this process just created, so nothing a racer does to the original
        // name can redirect the removal.
        #[cfg(test)]
        crate::test_fs::run_before_capture(entry.ordinal);
        let capture = match capture_entry(dir, &entry.name) {
            Ok(capture) => capture,
            Err(error) => return Err(Rmdir::CaptureFailed { error }),
        };
        let observed = EntryFacts {
            device: entry.device,
            inode: entry.inode,
            mode: libc::S_IFREG,
            uid: caller_uid(),
            size: entry.size,
        };
        // Verify the *captured* object against the observation, never the name
        // a racer could still be holding.
        let mismatch = match verify_captured(dir, &capture, &observed) {
            Ok(facts)
                if facts.is_regular()
                    && facts.device == device
                    && facts.uid == caller_uid()
                    && facts.size <= FINDER_METADATA_MAX_BYTES =>
            {
                None
            }
            Ok(_) => Some("the captured entry is not tolerated Finder metadata"),
            Err(reason) => Some(reason),
        };
        if let Some(kind) = mismatch {
            let restored = put_back(dir, &capture).is_ok();
            return Err(Rmdir::Mismatch {
                original: entry_label(OsStr::from_bytes(entry.name.to_bytes())),
                private: entry_label(OsStr::from_bytes(capture.private.to_bytes())),
                restored,
                kind,
            });
        }
        // Parent binding rechecked after the capture.
        match held_name(parent, name, dir) {
            Ok(HeldName::Held) => {}
            Ok(HeldName::Vanished) => {
                let _ = put_back(dir, &capture);
                return Err(Rmdir::Vanished);
            }
            Ok(HeldName::Different) => {
                let _ = put_back(dir, &capture);
                return Err(Rmdir::Replaced {
                    entry: entry_label(OsStr::from_bytes(name.to_bytes())),
                });
            }
            Err(error) => {
                let _ = put_back(dir, &capture);
                return Err(Rmdir::Failed { error });
            }
        }
        #[cfg(test)]
        crate::test_fs::run_before_unlink();
        // Only the private name this process created is removed.
        let status = unsafe { libc::unlinkat(dir.as_raw_fd(), capture.private.as_ptr(), 0) };
        if status == 0 {
            entries += 1;
            // The caller's own bounded check runs after *every* metadata unlink,
            // never once per batch: a reserve, volume or deadline failure stops
            // the drain here with the remaining entries untouched.
            if let Err(DrainStop::Stopped(reason)) = between() {
                return Err(Rmdir::Stopped { reason });
            }
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
) -> io::Result<Option<(u64, u64, u64)>> {
    let stat = stat_at(dir, name)?;
    if !is_finder_metadata(OsStr::from_bytes(name.to_bytes()))
        || stat_mode(&stat) != libc::S_IFREG
        || stat_device(&stat) != device
        || stat_uid(&stat) != caller_uid()
    {
        return Ok(None);
    }
    Ok(Some((
        stat_device(&stat),
        stat_ino(&stat),
        u64::try_from(stat.st_size).unwrap_or(u64::MAX),
    )))
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
