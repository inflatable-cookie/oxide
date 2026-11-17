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
//!
//! A tree **root** capture is journaled intent-first by its caller: before the
//! exclusive rename (and before any permission change) the caller durably
//! records the planned private name, the root's `(device, inode)`, the
//! verified parent's identity and the root's full mode, and after the rename it
//! records the captured phase. A crash then leaves two names and an explicit
//! record, and recovery reconciles them conservatively instead of finding the
//! recorded path absent and concluding a deletion that never finished.

use serde::{Deserialize, Serialize};
use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
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
/// Most names one re-list may see and unlink. Recreated Finder metadata is one
/// or two entries; a directory that gained more content than this is not this
/// race and is refused.
pub const FINDER_RACE_MAX_NAMES: usize = 64;

/// Deepest captured-parent chain one record may name. A tree deeper than this
/// is refused before it is captured, so the journal and its replay stay bounded.
pub const MAX_CAPTURE_DEPTH: usize = 64;

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

/// One entry name as the journal records it: the **exact** UTF-8 name, never a
/// lossy conversion. This platform's own pool and output names are valid UTF-8,
/// so a name that is not is refused before any capture rather than recorded
/// lossily; the error carries the escaped bytes and the caller stays pending.
pub fn journal_name(name: &CStr) -> Result<String, String> {
    std::str::from_utf8(name.to_bytes())
        .map(str::to_owned)
        .map_err(|_| {
            format!(
                "an entry name is not valid UTF-8 ({})",
                entry_label(OsStr::from_bytes(name.to_bytes()))
            )
        })
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
    /// The drain could not durably prove one captured entry's removal — its
    /// journal write failed, or the captured private name vanished before that
    /// write. Nothing was assumed: the caller keeps its pending intent.
    Unproven { reason: String },
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
    context: &mut CaptureContext<'_>,
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
    match drain_finder_metadata(parent, name, dir, device, context, between) {
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
    context: &mut CaptureContext<'_>,
    between: &mut dyn FnMut() -> Result<(), DrainStop>,
) -> Rmdir {
    let mut attempt = 0;
    let mut drained = 0;
    loop {
        attempt += 1;
        if attempt > 1 {
            std::thread::sleep(FINDER_RACE_PAUSE);
        }
        match rmdir_draining_finder_metadata(parent, name, dir, label, context, between) {
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
    #[cfg(test)]
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
    /// The full `st_mode`: file type *and* permission bits, so a caller can
    /// restore exactly the mode it changed.
    pub mode: libc::mode_t,
    pub uid: u32,
    pub size: u64,
}

impl EntryFacts {
    /// The file type alone (`S_IFMT` mask).
    pub fn file_type(&self) -> libc::mode_t {
        self.mode & libc::S_IFMT
    }

    pub fn is_regular(&self) -> bool {
        self.file_type() == libc::S_IFREG
    }

    pub fn is_directory(&self) -> bool {
        self.file_type() == libc::S_IFDIR
    }

    pub fn is_symlink(&self) -> bool {
        self.file_type() == libc::S_IFLNK
    }

    /// The permission bits of the observed entry.
    pub fn mode_bits(&self) -> libc::mode_t {
        self.mode & 0o7777
    }
}

/// How far one journaled root capture durably got.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootCapturePhase {
    /// The record is durable and the exclusive rename has not been recorded as
    /// done. A crash here may still leave either name holding the root, so
    /// recovery decides from both names and their identities.
    Planned,
    /// The exclusive rename to the recorded private name is durable.
    Captured,
    /// The captured root's own removal completed and was durably recorded. Only
    /// this phase proves the recorded deletion finished; its absence is never
    /// treated as completion.
    Removed,
}

/// The caller's durable record of one tree-root capture (oxide#155).
///
/// It is written before the rename and before any permission change, so a
/// crash between any two steps leaves the planned private name, the root's own
/// identity, the parent it must be found in and the mode it must be restored to
/// recoverable. It never authorizes an effect on its own: every step below
/// revalidates the identities against live descriptors.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootCapturePlan {
    /// The private name the root is exclusively renamed to.
    pub private: String,
    pub phase: RootCapturePhase,
    /// The root's `(device, inode)` as observed before the rename.
    pub device: u64,
    pub inode: u64,
    /// The verified parent directory's `(device, inode)`.
    pub parent_device: u64,
    pub parent_inode: u64,
    /// The root's full `st_mode` before any permission change, so recovery can
    /// put back exactly the mode the owner had.
    pub mode: u32,
}

impl RootCapturePlan {
    /// The root's permission bits, as a value that can be handed back to
    /// `fchmod`.
    pub fn mode_bits(&self) -> libc::mode_t {
        (self.mode & 0o7777) as libc::mode_t
    }

    /// Whether this record is one a writer can produce. A ledger or journal
    /// holding anything else is refused, never acted on.
    pub fn is_valid(&self) -> bool {
        let private = self.private.as_bytes();
        let type_bits = self.mode & u32::from(libc::S_IFMT);
        private.len() > CAPTURE_PREFIX.len()
            && private.len() <= CAPTURE_PREFIX.len() + 2 * CAPTURE_TAG_BYTES
            && private.starts_with(CAPTURE_PREFIX.as_bytes())
            && private[CAPTURE_PREFIX.len()..]
                .iter()
                .all(u8::is_ascii_hexdigit)
            && self.device != 0
            && self.inode != 0
            && self.parent_device != 0
            && self.parent_inode != 0
            && type_bits == u32::from(libc::S_IFDIR)
            && self.mode & !(u32::from(libc::S_IFMT) | 0o7777) == 0
    }
}

/// The longest full path a journaled name field may carry (`PATH_MAX`).
const MAX_JOURNAL_PATH_BYTES: usize = 1024;
/// The longest single name component a journaled name field may carry
/// (`NAME_MAX`).
const MAX_JOURNAL_NAME_BYTES: usize = 255;
/// Whether a journaled name is within the enforced path and name bounds: at
/// most `PATH_MAX` bytes in total and at most `NAME_MAX` bytes in every
/// `/`-separated component. The writer refuses a record that fails this, and a
/// reader refuses a ledger that carries one, so the ledger's journal bound
/// stays finite (oxide#177).
fn bounded_journal_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_JOURNAL_PATH_BYTES
        && value
            .split('/')
            .all(|part| !part.is_empty() && part.len() <= MAX_JOURNAL_NAME_BYTES)
}

/// One outstanding capture below the root: a verified object moved to a private
/// name inside the tree, recorded durably before it is removed so a replay can
/// find it again (oxide#155).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureRecord {
    /// The private names of the captured directories between the root and this
    /// entry's parent, outermost first. Each one is itself an outstanding
    /// capture, so replay resolves and verifies the whole chain. Empty for a
    /// direct child of the captured root.
    pub path: Vec<String>,
    /// The private name this entry was moved to in its parent.
    pub private: String,
    /// The name it was observed under, for diagnostics and the put-back.
    pub original: String,
    /// The identity and full mode the captured object was verified with.
    pub device: u64,
    pub inode: u64,
    pub mode: u32,
}

impl CaptureRecord {
    pub fn mode_bits(&self) -> libc::mode_t {
        (self.mode & 0o7777) as libc::mode_t
    }

    pub fn is_directory(&self) -> bool {
        self.mode & u32::from(libc::S_IFMT) == u32::from(libc::S_IFDIR)
    }

    /// A record only this module's writer produces.
    pub fn is_valid(&self) -> bool {
        let private = self.private.as_bytes();
        let type_bits = self.mode & u32::from(libc::S_IFMT);
        private.len() > CAPTURE_PREFIX.len()
            && private.len() <= CAPTURE_PREFIX.len() + 2 * CAPTURE_TAG_BYTES
            && private.starts_with(CAPTURE_PREFIX.as_bytes())
            && private[CAPTURE_PREFIX.len()..]
                .iter()
                .all(u8::is_ascii_hexdigit)
            && bounded_journal_name(&self.original)
            && self.path.len() <= MAX_CAPTURE_DEPTH
            && self.path.iter().all(|name| {
                name.len() > CAPTURE_PREFIX.len()
                    && name.len() <= CAPTURE_PREFIX.len() + 2 * CAPTURE_TAG_BYTES
                    && name.starts_with(CAPTURE_PREFIX)
                    && name[CAPTURE_PREFIX.len()..]
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
            })
            && self.device != 0
            && self.inode != 0
            && (type_bits == u32::from(libc::S_IFDIR)
                || type_bits == u32::from(libc::S_IFREG)
                || type_bits == u32::from(libc::S_IFLNK))
            && self.mode & !(u32::from(libc::S_IFMT) | 0o7777) == 0
    }
}

/// One capture that was **not** the observed object, durably recorded before
/// any put-back attempt. Replay never removes a name one of these names, and
/// the recorded deletion never completes while one exists.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForeignCapture {
    pub path: Vec<String>,
    pub private: String,
    pub original: String,
    /// What the observation expected.
    pub expected_device: u64,
    pub expected_inode: u64,
    pub expected_mode: u32,
    /// What the private name actually held, as a short no-follow description.
    pub found: String,
    /// Whether the exclusive put-back to `original` succeeded.
    pub restored: bool,
}

impl ForeignCapture {
    pub fn is_valid(&self) -> bool {
        let private = self.private.as_bytes();
        private.len() > CAPTURE_PREFIX.len()
            && private.len() <= CAPTURE_PREFIX.len() + 2 * CAPTURE_TAG_BYTES
            && private.starts_with(CAPTURE_PREFIX.as_bytes())
            && private[CAPTURE_PREFIX.len()..]
                .iter()
                .all(u8::is_ascii_hexdigit)
            && bounded_journal_name(&self.original)
            && bounded_journal_name(&self.found)
            && self.path.len() <= MAX_CAPTURE_DEPTH
            && self.path.iter().all(|name| {
                name.len() > CAPTURE_PREFIX.len()
                    && name.len() <= CAPTURE_PREFIX.len() + 2 * CAPTURE_TAG_BYTES
                    && name.starts_with(CAPTURE_PREFIX)
                    && name[CAPTURE_PREFIX.len()..]
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
            })
    }
}

/// Whether `name` is one of the private capture names this module creates. A
/// deletion never treats such a name as ordinary content: it is either a
/// recorded capture (replay reconciles it) or an unrecorded one, which is
/// reported and kept, never removed.
pub fn is_capture_name(name: &OsStr) -> bool {
    let bytes = name.as_bytes();
    bytes.len() > CAPTURE_PREFIX.len()
        && bytes.starts_with(CAPTURE_PREFIX.as_bytes())
        && bytes[CAPTURE_PREFIX.len()..]
            .iter()
            .all(u8::is_ascii_hexdigit)
}

/// Where one directory sits inside a journaled whole-tree deletion (oxide#155):
/// the chain of captured private names from the root down to it, and the
/// caller's durable journal. A caller with no journal of its own (the prune
/// quarantine pass) leaves `journal` empty and is documented as a narrower
/// caller whose captured residue is never consumed.
pub struct CaptureContext<'a> {
    pub chain: Vec<String>,
    pub journal: Option<&'a mut dyn RootCaptureJournal>,
}

impl CaptureContext<'_> {
    /// Durably record one captured entry before it is removed.
    pub fn record_captured(&mut self, record: &CaptureRecord) -> Result<(), String> {
        match self.journal.as_mut() {
            Some(journal) => (**journal).record_captured(record),
            None => Ok(()),
        }
    }

    /// Durably record one captured entry's own removal.
    pub fn record_removed(&mut self, record: &CaptureRecord) -> Result<(), String> {
        match self.journal.as_mut() {
            Some(journal) => (**journal).record_removed(record),
            None => Ok(()),
        }
    }

    /// Durably record a capture that was not the observed object, before any
    /// put-back attempt.
    pub fn record_foreign(&mut self, capture: &ForeignCapture) -> Result<(), String> {
        match self.journal.as_mut() {
            Some(journal) => (**journal).record_foreign(capture),
            None => Ok(()),
        }
    }

    pub fn record_root_removed(&mut self, plan: &RootCapturePlan) -> Result<(), String> {
        match self.journal.as_mut() {
            Some(journal) => (**journal).record_root_removed(plan),
            None => Ok(()),
        }
    }

    pub fn planned(&mut self, plan: &RootCapturePlan) -> Result<(), String> {
        match self.journal.as_mut() {
            Some(journal) => (**journal).planned(plan),
            None => Ok(()),
        }
    }

    pub fn captured(&mut self, plan: &RootCapturePlan) -> Result<(), String> {
        match self.journal.as_mut() {
            Some(journal) => (**journal).captured(plan),
            None => Ok(()),
        }
    }

    pub fn has_journal(&self) -> bool {
        self.journal.is_some()
    }
}

/// A short no-follow description of one observed entry, for a foreign-capture
/// record. It never authorizes an effect.
pub fn describe_entry(facts: &EntryFacts) -> String {
    let class = if facts.is_directory() {
        "directory"
    } else if facts.is_regular() {
        "regular file"
    } else if facts.is_symlink() {
        "symbolic link"
    } else {
        "other object"
    };
    format!(
        "{class} device {} inode {} mode {:o} owner {}",
        facts.device, facts.inode, facts.mode, facts.uid
    )
}

/// Arms the exact recorded-mode restoration of one step directory **through the
/// verified descriptor that was widened**, so no later rebind of a name can
/// redirect the chmod and a failure after the widening still restores the
/// owner's mode on every exit path, including a panic.
pub struct StepModeGuard {
    dir: OwnedFd,
    mode: u32,
    armed: bool,
}

impl StepModeGuard {
    /// Restore the recorded mode now through the held descriptor and report a
    /// failure, so the caller stays pending instead of keeping it widened.
    pub fn restore(&mut self) -> Result<(), String> {
        if !self.armed {
            return Ok(());
        }
        self.armed = false;
        restore_mode_through(self.dir.as_fd(), self.mode)
    }
}

impl Drop for StepModeGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = restore_mode_through(self.dir.as_fd(), self.mode);
        }
    }
}

fn restore_mode_through(dir: BorrowedFd<'_>, mode: u32) -> Result<(), String> {
    let bits = (mode & 0o7777) as libc::mode_t;
    // SAFETY: `dir` is a live descriptor for the verified directory.
    #[cfg(not(test))]
    if unsafe { libc::fchmod(dir.as_raw_fd(), bits) } != 0 {
        return Err(format!(
            "the step directory's recorded mode cannot be restored ({})",
            io::Error::last_os_error()
        ));
    }
    #[cfg(test)]
    {
        let status = unsafe { libc::fchmod(dir.as_raw_fd(), bits) };
        if status != 0 {
            return Err(format!(
                "the step directory's recorded mode cannot be restored ({})",
                io::Error::last_os_error()
            ));
        }
    }
    if unsafe { libc::fsync(dir.as_raw_fd()) } != 0 {
        return Err(format!(
            "the restored mode cannot be synced ({})",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// Open one directory for **one replay step**: open it without following a
/// link, verify the descriptor against the record, arm the recorded-mode
/// restoration **immediately** after the widening chmod, and only then reopen
/// it through `.` on that descriptor. The returned guard owns the verified
/// descriptor, so the mode is restored through it on every exit path; the
/// caller never restores by name.
pub fn open_verified_step(
    parent: BorrowedFd<'_>,
    name: &CStr,
    device: u64,
    inode: u64,
    mode: u32,
) -> Result<(OwnedFd, StepModeGuard), String> {
    let held = open_directory_for_repair(parent, name)
        .map_err(|error| format!("the directory cannot be opened for the step ({error})"))?;
    match descriptor_identity(held.as_fd()) {
        Ok(identity) if identity == (device, inode) => {}
        Ok(_) => return Err("the directory was replaced before the step".to_owned()),
        Err(error) => return Err(format!("the directory's identity cannot be read ({error})")),
    }
    let bits = ((mode & 0o7777) | 0o700) as libc::mode_t;
    // SAFETY: `held` is a live descriptor for the verified directory.
    #[cfg(not(test))]
    if unsafe { libc::fchmod(held.as_raw_fd(), bits) } != 0 {
        return Err(format!(
            "the directory cannot be given access for the step ({})",
            io::Error::last_os_error()
        ));
    }
    #[cfg(test)]
    {
        let status = unsafe { libc::fchmod(held.as_raw_fd(), bits) };
        if status != 0 {
            return Err(format!(
                "the directory cannot be given access for the step ({})",
                io::Error::last_os_error()
            ));
        }
    }
    // Armed from this instant, before anything else can fail.
    let mut guard = StepModeGuard {
        dir: held,
        mode,
        armed: true,
    };
    #[cfg(test)]
    crate::test_fs::run_after_step_chmod();
    let fault = {
        #[cfg(test)]
        {
            crate::test_fs::injected_step_open_failure()
        }
        #[cfg(not(test))]
        {
            false
        }
    };
    // Reopen through the held descriptor, never through the name.
    let step = c".";
    // SAFETY: `guard.dir` and the C string are live for the call.
    let fd = if fault {
        -1
    } else {
        unsafe {
            libc::openat(
                guard.dir.as_raw_fd(),
                step.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        }
    };
    if fd < 0 {
        let reason = if fault {
            "the verified directory cannot be read for the step (injected)".to_owned()
        } else {
            format!(
                "the verified directory cannot be read for the step ({})",
                io::Error::last_os_error()
            )
        };
        return match guard.restore() {
            Ok(()) => Err(reason),
            Err(restore_reason) => Err(format!("{reason}; {restore_reason}")),
        };
    }
    // SAFETY: `fd` is a newly owned descriptor.
    Ok((unsafe { OwnedFd::from_raw_fd(fd) }, guard))
}

/// Restore one captured directory's exact recorded mode through a search-only,
/// no-follow descriptor of that very entry, after revalidating its identity.
pub fn restore_captured_mode(
    parent: BorrowedFd<'_>,
    name: &CStr,
    device: u64,
    inode: u64,
    mode: u32,
) -> Result<(), String> {
    let held = open_directory_for_repair(parent, name)
        .map_err(|error| format!("the captured directory cannot be opened ({error})"))?;
    match descriptor_identity(held.as_fd()) {
        Ok(identity) if identity == (device, inode) => {}
        Ok(_) => {
            return Err(
                "the captured directory was replaced before its mode was restored".to_owned(),
            )
        }
        Err(error) => {
            return Err(format!(
                "the captured directory's identity cannot be read ({error})"
            ))
        }
    }
    let bits = (mode & 0o7777) as libc::mode_t;
    // SAFETY: `held` is a live descriptor for the verified directory.
    #[cfg(not(test))]
    if unsafe { libc::fchmod(held.as_raw_fd(), bits) } != 0 {
        return Err(format!(
            "the captured directory's recorded mode cannot be restored ({})",
            io::Error::last_os_error()
        ));
    }
    #[cfg(test)]
    {
        let status = unsafe { libc::fchmod(held.as_raw_fd(), bits) };
        if status != 0 {
            return Err(format!(
                "the captured directory's recorded mode cannot be restored ({})",
                io::Error::last_os_error()
            ));
        }
    }
    if unsafe { libc::fsync(held.as_raw_fd()) } != 0 {
        return Err(format!(
            "the restored mode cannot be synced ({})",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// A caller's durable journal for the capture of one tree root. The seam calls
/// `planned` after it has generated the private name and read the identities
/// but before the exclusive rename, and `captured` with that same plan after the
/// rename and its directory sync, so the caller records the captured phase
/// itself. An error from either stops the caller: nothing else is attempted and
/// no record is cleared.
pub trait RootCaptureJournal {
    fn planned(&mut self, plan: &RootCapturePlan) -> Result<(), String>;
    fn captured(&mut self, plan: &RootCapturePlan) -> Result<(), String>;
    /// Durably record one captured entry before it is removed.
    fn record_captured(&mut self, record: &CaptureRecord) -> Result<(), String>;
    /// Durably record that one captured entry's own removal completed.
    fn record_removed(&mut self, record: &CaptureRecord) -> Result<(), String>;
    /// Durably record a capture that was not the observed object, before any
    /// put-back attempt.
    fn record_foreign(&mut self, capture: &ForeignCapture) -> Result<(), String>;
    /// Durably record that the captured root itself was removed.
    fn record_root_removed(&mut self, plan: &RootCapturePlan) -> Result<(), String>;
}

/// What the two names of one journaled root capture hold right now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RootCaptureState {
    /// The original name holds the recorded root and the private name is free:
    /// no capture is in effect.
    Original,
    /// The private name holds the recorded root and the original name is free:
    /// the capture is in effect.
    Private,
    /// Neither name exists.
    Absent,
    /// The parent is not the recorded parent, a name holds another object, or
    /// both names hold something. Nothing may be assumed and nothing may be
    /// removed.
    Uncertain { reason: String },
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

/// One fresh private capture name. 128 random bits make it unguessable and a
/// collision practically impossible; `capture_named` still handles `EEXIST`.
fn capture_name() -> io::Result<String> {
    Ok(format!("{CAPTURE_PREFIX}{}", random_tag()?))
}

/// Capture `name` under a fresh private name in the same directory. Retries a
/// name collision a bounded number of times; any other rename error is the
/// caller's stop.
pub fn capture_entry(dir: BorrowedFd<'_>, name: &CStr) -> io::Result<Capture> {
    for _ in 0..8 {
        match capture_named(dir, name, &capture_name()?) {
            Ok(capture) => return Ok(capture),
            Err(error) if error.raw_os_error() == Some(libc::EEXIST) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::from_raw_os_error(libc::EEXIST))
}

/// Capture `name` under one already-planned private name. This is the form a
/// caller uses when the name must be durable before the rename: the plan names
/// it, so recovery can look for exactly these two names.
pub fn capture_named(dir: BorrowedFd<'_>, name: &CStr, private: &str) -> io::Result<Capture> {
    let private =
        CString::new(private).map_err(|_| io::Error::other("capture name holds a NUL"))?;
    rename_exclusive(dir, name, &private)?;
    Ok(Capture {
        private,
        original: name.to_owned(),
    })
}

/// Plan the capture of one verified tree root: its own no-follow facts, the
/// held parent's identity, and a fresh private name. Reads only; the caller
/// must journal the returned plan durably before [`capture_planned`].
pub fn plan_root_capture(
    parent: BorrowedFd<'_>,
    observed: &EntryFacts,
) -> io::Result<RootCapturePlan> {
    let (parent_device, parent_inode) = descriptor_identity(parent)?;
    Ok(RootCapturePlan {
        private: capture_name()?,
        phase: RootCapturePhase::Planned,
        device: observed.device,
        inode: observed.inode,
        parent_device,
        parent_inode,
        mode: u32::try_from(observed.mode).unwrap_or(u32::MAX),
    })
}

/// Move a verified root to its planned private name and make that rename
/// durable, so the caller can record the captured phase afterwards. A failed
/// rename is reported and moved nothing.
pub fn capture_planned(
    parent: BorrowedFd<'_>,
    name: &CStr,
    plan: &RootCapturePlan,
) -> io::Result<Capture> {
    let capture = capture_named(parent, name, &plan.private)?;
    sync_directory(parent)?;
    Ok(capture)
}

/// Compare the two names of one journaled root capture against the recorded
/// identities. Reads only; nothing is renamed, created or removed.
pub fn reconcile_root_capture(
    parent: BorrowedFd<'_>,
    name: &CStr,
    plan: &RootCapturePlan,
) -> RootCaptureState {
    if !plan.is_valid() {
        return RootCaptureState::Uncertain {
            reason: "the recorded capture plan is not one a writer produces".to_owned(),
        };
    }
    match descriptor_identity(parent) {
        Ok(identity) if identity == (plan.parent_device, plan.parent_inode) => {}
        Ok(_) => {
            return RootCaptureState::Uncertain {
                reason: "the recorded parent directory is not the parent this run holds".to_owned(),
            }
        }
        Err(error) => {
            return RootCaptureState::Uncertain {
                reason: format!("the parent's identity cannot be read ({error})"),
            }
        }
    }
    let original = match name_holds(parent, name, plan) {
        Ok(state) => state,
        Err(reason) => return RootCaptureState::Uncertain { reason },
    };
    let private = match CString::new(plan.private.as_str()) {
        Err(_) => {
            return RootCaptureState::Uncertain {
                reason: "the recorded private name holds a NUL".to_owned(),
            }
        }
        Ok(private) => match name_holds(parent, &private, plan) {
            Ok(state) => state,
            Err(reason) => return RootCaptureState::Uncertain { reason },
        },
    };
    match (original, private) {
        (Some(true), Some(false)) => RootCaptureState::Original,
        (Some(false), Some(true)) => RootCaptureState::Private,
        (Some(false), Some(false)) => RootCaptureState::Absent,
        (Some(true), Some(true)) => RootCaptureState::Uncertain {
            reason: "both the original and the private name hold the recorded root".to_owned(),
        },
        _ => RootCaptureState::Uncertain {
            reason: "a name holds an object that is not the recorded root".to_owned(),
        },
    }
}

/// Whether one name holds the recorded root (`Some(true)`), is free
/// (`Some(false)`) or holds another object (`None`).
fn name_holds(
    parent: BorrowedFd<'_>,
    name: &CStr,
    plan: &RootCapturePlan,
) -> Result<Option<bool>, String> {
    match stat_at(parent, name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Some(false)),
        Err(error) => Err(format!("a capture name cannot be read ({error})")),
        Ok(stat) => Ok(
            if stat_mode(&stat) == libc::S_IFDIR
                && stat_device(&stat) == plan.device
                && stat_ino(&stat) == plan.inode
            {
                Some(true)
            } else {
                None
            },
        ),
    }
}

/// Put a captured root back under its recorded name, in exactly the mode the
/// record kept. The mode is restored while the object is still under the
/// private name, so the recorded name never briefly holds a widened mode, and
/// only then is the exclusive rename done. Every step revalidates an identity;
/// a failure leaves both names as they were and the caller keeps its record.
pub fn restore_root_capture(
    parent: BorrowedFd<'_>,
    name: &CStr,
    plan: &RootCapturePlan,
) -> Result<(), String> {
    if !plan.is_valid() {
        return Err("the recorded capture plan is not one a writer produces".to_owned());
    }
    match descriptor_identity(parent) {
        Ok(identity) if identity == (plan.parent_device, plan.parent_inode) => {}
        Ok(_) => {
            return Err("the recorded parent directory is not the parent this run holds".to_owned())
        }
        Err(error) => return Err(format!("the parent's identity cannot be read ({error})")),
    }
    let private = CString::new(plan.private.as_str())
        .map_err(|_| "the recorded private name holds a NUL".to_owned())?;
    if name_holds(parent, &private, plan)? != Some(true) {
        return Err("the private name does not hold the recorded root".to_owned());
    }
    if name_holds(parent, name, plan)? != Some(false) {
        return Err("the recorded name is not free, so the root cannot be put back".to_owned());
    }
    // The mode is restored through a search-only, no-follow descriptor of the
    // captured entry itself, so nothing is chmodded through a name a racer
    // could redirect.
    {
        let held = open_directory_for_repair(parent, &private)
            .map_err(|error| format!("the captured root cannot be opened ({error})"))?;
        match descriptor_identity(held.as_fd()) {
            Ok(identity) if identity == (plan.device, plan.inode) => {}
            Ok(_) => {
                return Err(
                    "the captured root was replaced before its mode was restored".to_owned(),
                )
            }
            Err(error) => {
                return Err(format!(
                    "the captured root's identity cannot be read ({error})"
                ))
            }
        }
        // SAFETY: `held` is a live descriptor for the captured directory.
        #[cfg(not(test))]
        if unsafe { libc::fchmod(held.as_raw_fd(), plan.mode_bits()) } != 0 {
            return Err(format!(
                "the captured root's recorded mode cannot be restored ({})",
                io::Error::last_os_error()
            ));
        }
        #[cfg(test)]
        {
            let status = unsafe { libc::fchmod(held.as_raw_fd(), plan.mode_bits()) };
            if status != 0 {
                return Err(format!(
                    "the captured root's recorded mode cannot be restored ({})",
                    io::Error::last_os_error()
                ));
            }
        }
        if unsafe { libc::fsync(held.as_raw_fd()) } != 0 {
            return Err(format!(
                "the restored mode cannot be synced ({})",
                io::Error::last_os_error()
            ));
        }
    }
    #[cfg(not(test))]
    rename_exclusive(parent, &private, name)
        .map_err(|error| format!("the captured root cannot be put back ({error})"))?;
    #[cfg(test)]
    {
        let result = rename_exclusive(parent, &private, name);
        result.map_err(|error| format!("the captured root cannot be put back ({error})"))?;
    }
    sync_directory(parent).map_err(|error| format!("the put-back cannot be synced ({error})"))?;
    match name_holds(parent, name, plan)? {
        Some(true) => Ok(()),
        _ => Err("the recorded name does not hold the recorded root after its put-back".to_owned()),
    }
}

/// Make a directory rename durable before it is recorded.
fn sync_directory(dir: BorrowedFd<'_>) -> io::Result<()> {
    // SAFETY: `dir` is a live descriptor for the call.
    if unsafe { libc::fsync(dir.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `fstatat` facts for one name in a held directory, never following a link.
pub fn observe_entry(dir: BorrowedFd<'_>, name: &CStr) -> io::Result<EntryFacts> {
    let stat = stat_at(dir, name)?;
    Ok(EntryFacts {
        device: stat_device(&stat),
        inode: stat_ino(&stat),
        mode: stat.st_mode,
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
    if captured.file_type() != observed.file_type() {
        return Err("captured object changed type");
    }
    Ok(captured)
}

/// Put a captured object back under its original name, exclusively. A racer
/// that took that name again makes this fail; the caller then leaves the
/// private name in place and reports both.
pub fn put_back(dir: BorrowedFd<'_>, capture: &Capture) -> io::Result<()> {
    #[cfg(not(test))]
    {
        rename_exclusive(dir, &capture.private, &capture.original)
    }
    #[cfg(test)]
    {
        let result = rename_exclusive(dir, &capture.private, &capture.original);
        result
    }
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
    context: &mut CaptureContext<'_>,
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
                #[cfg(test)]
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
        // A journal records the exact name, so a name that cannot round-trip
        // is refused *before* it is captured: nothing is renamed or removed.
        let original = match journal_name(&entry.name) {
            Ok(name) => name,
            Err(reason) => {
                return Err(Rmdir::Unproven {
                    reason: format!("{reason}; nothing was captured or removed"),
                })
            }
        };
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
                if facts.is_regular() && facts.device == device && facts.uid == caller_uid() =>
            {
                None
            }
            Ok(_) => Some("the captured entry is not tolerated Finder metadata"),
            Err(reason) => Some(reason),
        };
        if let Some(kind) = mismatch {
            // Durably record the mismatch *before* the put-back: the private
            // name must never hold an object no record names, and replay never
            // removes a name recorded here.
            let found = observe_entry(dir, &capture.private)
                .map(|facts| describe_entry(&facts))
                .unwrap_or_else(|error| format!("unreadable capture ({error})"));
            let mut foreign = ForeignCapture {
                path: context.chain.clone(),
                private: journal_name(&capture.private)
                    .map_err(|reason| Rmdir::Unproven { reason })?,
                original,
                expected_device: entry.device,
                expected_inode: entry.inode,
                expected_mode: u32::from(libc::S_IFREG),
                found,
                restored: false,
            };
            if !foreign.is_valid() {
                // An over-bound name is refused per object and nothing is
                // deleted: the capture is put back and the deletion stays
                // pending (oxide#177).
                let restored = put_back(dir, &capture).is_ok();
                return Err(Rmdir::Unproven {
                    reason: format!(
                        "a mismatched capture is outside the journal's bounded names; put back: {restored}"
                    ),
                });
            }
            if let Err(reason) = context.record_foreign(&foreign) {
                // Without the record the put-back is still the conservative
                // action; the caller stops pending either way.
                let restored = put_back(dir, &capture).is_ok();
                return Err(Rmdir::Unproven {
                    reason: format!(
                        "a mismatched capture could not be recorded ({reason}); put back: {restored}"
                    ),
                });
            }
            let restored = put_back(dir, &capture).is_ok();
            if restored {
                foreign.restored = true;
                let _ = context.record_foreign(&foreign);
            }
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
        // Durable record before the effect: replay can then find this capture
        // by name and identity, and never treats the name as ordinary content.
        let record = CaptureRecord {
            path: context.chain.clone(),
            private: journal_name(&capture.private).map_err(|reason| Rmdir::Unproven { reason })?,
            original: journal_name(&entry.name).map_err(|reason| Rmdir::Unproven { reason })?,
            device: entry.device,
            inode: entry.inode,
            mode: u32::from(libc::S_IFREG),
        };
        if !record.is_valid() {
            // Refuse per object before recording or deleting anything: the
            // capture goes back and the deletion stays pending (oxide#177).
            let _ = put_back(dir, &capture);
            return Err(Rmdir::Unproven {
                reason: "the captured entry is outside the journal's bounded names".to_owned(),
            });
        }
        if let Err(reason) = context.record_captured(&record) {
            let _ = put_back(dir, &capture);
            return Err(Rmdir::Unproven {
                reason: format!("the captured entry could not be recorded ({reason})"),
            });
        }
        // Only the private name this process created is removed.
        let status = unsafe { libc::unlinkat(dir.as_raw_fd(), capture.private.as_ptr(), 0) };
        if status == 0 {
            // Durable proof of this capture's removal before anything else
            // counts it as work done.
            if let Err(reason) = context.record_removed(&record) {
                return Err(Rmdir::Unproven {
                    reason: format!(
                        "the captured entry's removal could not be recorded ({reason})"
                    ),
                });
            }
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
            // The captured private name vanished between its record and its
            // removal. That is never this drain's success: the caller keeps
            // its pending intent and replay decides conservatively.
            return Err(Rmdir::Unproven {
                reason: "a recorded capture vanished before its removal".to_owned(),
            });
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

/// This thread's `errno` location, where the platform exposes one.
///
/// `readdir` reports both the end of a directory and a failure by returning
/// null, and only `errno` tells them apart. A platform whose `errno` this crate
/// cannot read cannot prove a listing ended, so it is reported as unprovable
/// rather than as an empty directory.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn errno_slot() -> Option<*mut libc::c_int> {
    // SAFETY: `__errno_location` returns this thread's live errno location.
    Some(unsafe { libc::__errno_location() })
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn errno_slot() -> Option<*mut libc::c_int> {
    // SAFETY: `__error` returns this thread's live errno location.
    Some(unsafe { libc::__error() })
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios"
)))]
fn errno_slot() -> Option<*mut libc::c_int> {
    None
}

fn set_errno(code: libc::c_int) {
    if let Some(slot) = errno_slot() {
        // SAFETY: `slot` is this thread's live errno location.
        unsafe { *slot = code };
    }
}

/// Clear this thread's `errno` before a `readdir` call, so a null return can be
/// classified by [`enumeration_error`].
pub fn clear_errno() {
    set_errno(0);
}

/// The failure a null `readdir` return reported, or `None` when the directory
/// really ended. Call it immediately after the null return, with `errno`
/// cleared beforehand by [`clear_errno`].
///
/// A listing this platform cannot classify is an unproven enumeration, never an
/// end of directory: a partial list is never handed to a caller that would act
/// on it.
pub fn enumeration_error() -> Option<io::Error> {
    if errno_slot().is_none() {
        return Some(io::Error::from_raw_os_error(libc::ENOTSUP));
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(0) => None,
        Some(_) => Some(error),
        None => Some(error),
    }
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
        // `readdir` reports the end of the directory and a failure with the same
        // null return, so `errno` is cleared before each call and read after a
        // null one: a listing that stopped early is an error, never a shorter
        // directory, and no caller ever acts on a partial list.
        clear_errno();
        #[cfg(test)]
        let entry = match crate::test_fs::faulted_readdir(seen) {
            // The injected failure takes the same classification path as a real
            // one: `errno` carries the code and the call reports a null return.
            Some(code) => {
                set_errno(code);
                std::ptr::null_mut()
            }
            None => {
                // SAFETY: `stream` is a live directory stream.
                unsafe { libc::readdir(stream) }
            }
        };
        #[cfg(not(test))]
        // SAFETY: `stream` is a live directory stream.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            if let Some(error) = enumeration_error() {
                // SAFETY: `stream` is live and closed exactly once.
                unsafe { libc::closedir(stream) };
                return Err(error);
            }
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
    #[cfg(test)]
    {
        use std::os::unix::ffi::OsStrExt;
        let outcome = if status == 0 {
        } else {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::NotFound {
            } else {
            }
        };
    }
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
    #[cfg(not(test))]
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    #[cfg(test)]
    {
        let status = unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) };
        let outcome = if status == 0 {
        } else {
        };
        if status != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((
        stat_device(&stat),
        u64::try_from(stat.st_ino).unwrap_or(u64::MAX),
    ))
}
