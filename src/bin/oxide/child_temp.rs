//! Where a leased command's children put temporary files (oxide#136).
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md#child-temporary-files`.
//! One rule decides the whole child tree's temporary directory:
//!
//! - an inherited `TMPDIR` that canonically resolves to an existing,
//!   user-writable directory on the same device as the selected pool is
//!   retained for every child as that **canonical** directory, and nothing of
//!   Oxide's is created or cleaned;
//! - otherwise the command gets one Oxide-owned directory inside the held
//!   slot (`<slot>/scratch`), created and removed under the exclusive lease,
//!   after positive writer closure. If that directory cannot be prepared the
//!   command refuses before spawning: it never falls back to the machine
//!   default this decision rejected.
//!
//! `TMPDIR`, `TMP` and `TEMP` are all set to the chosen value on every child
//! Oxide launches for a leased command, so descendants (test and `run`
//! programs, build scripts, tools under `oxide exec`) inherit it and no
//! conflicting alias survives. Oxide never writes management data into, or
//! removes, an inherited directory: it is not Oxide's, and the owning tool
//! keeps its own retention rules. Effect-free commands (plain passthrough,
//! probes, `status`) create nothing.
//!
//! The owned directory is bound to pool identity, slot and incarnation by a
//! marker file written before any writer starts, so a later lease of the same
//! incarnation may reconcile it and no other shape is ever deleted. The
//! marker says whose directory it is, never that a writer closed: the
//! exclusive lease is the fence, so a competing command can only run after
//! the previous holder released (which the product does only after positive
//! group closure), and a `run` program using an owned directory keeps that
//! lease until its own group is empty. It lives
//! inside the slot, so the existing whole-slot accounting counts it once and
//! whole-slot eviction, crash recovery and lost-volume retirement remove or
//! keep it with the slot they already own.
//!
//! Path length is a hard limit, not a preference: keep the injected path
//! short enough for a Unix socket. A longer derived path is reported and not
//! used rather than claimed to fit every configured pool.

use crate::wrapper;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::PathBuf;

/// The Oxide-owned directory inside the held slot.
const SCRATCH_NAME: &str = "scratch";
/// Ownership marker inside that directory, written before any writer starts.
const OWNER_NAME: &str = "owner";
const OWNER_VERSION: &str = "oxide.scratch/1";
const MAX_OWNER_BYTES: u64 = 1024;
/// Variables a child's temporary directory is published through.
pub(crate) const VARIABLES: [&str; 3] = ["TMPDIR", "TMP", "TEMP"];
/// Longest owned scratch path Oxide injects. macOS `sun_path` is 104 bytes
/// including the terminating NUL; a Unix socket needs a `/` and a short name
/// inside the directory, so 91 + 1 + 11 + NUL is the whole budget.
pub(crate) const MAX_SCRATCH_PATH_BYTES: usize = 91;

/// The durable binding of one owned scratch directory: the pool's recorded
/// store identity, the slot number and its incarnation. The lease that
/// created it is recorded for evidence only; the binding decides ownership
/// because no two commands ever hold one slot at the same time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Identity {
    pub store_id: String,
    pub slot_id: String,
    pub incarnation: String,
    pub lease_id: String,
}

impl Identity {
    fn binding(&self) -> (&str, &str, &str) {
        (&self.store_id, &self.slot_id, &self.incarnation)
    }
}

/// One owned per-command temporary directory.
#[derive(Clone, Debug)]
pub(crate) struct Owned {
    path: PathBuf,
    /// The device the directory was created on; removal refuses another.
    dev: u64,
    identity: Identity,
}

impl Owned {
    #[cfg(test)]
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// What a leased command's children get. Either answer is published on all
/// three alias variables, so a child can never inherit a value this decision
/// rejected.
#[derive(Clone, Debug)]
pub(crate) enum Chosen {
    /// Retained inherited directory, resolved canonically: children always
    /// get the one directory this decision qualified, never the raw text
    /// (which a child could resolve against a different working directory).
    /// Oxide owns nothing in it.
    Inherited(PathBuf),
    /// Oxide's own per-command directory inside the held slot.
    Oxide(Owned),
}

impl Chosen {
    /// The bounded, escaped summary and status fragment.
    pub(crate) fn note(&self) -> String {
        match self {
            Self::Inherited(path) => format!(
                "temp: inherited {}",
                wrapper::shown(&path.to_string_lossy())
            ),
            Self::Oxide(owned) => format!(
                "temp: oxide {}",
                wrapper::shown(&owned.path.to_string_lossy())
            ),
        }
    }

    /// The owned directory, when this command owns one.
    pub(crate) fn owned(&self) -> Option<Owned> {
        match self {
            Self::Oxide(owned) => Some(owned.clone()),
            Self::Inherited(_) => None,
        }
    }

    /// Publish the choice on one child's environment: `TMPDIR`, `TMP` and
    /// `TEMP` all name the chosen directory, so a conflicting alias never
    /// survives to the child.
    pub(crate) fn apply(&self, command: &mut std::process::Command) {
        let value: &OsStr = match self {
            Self::Inherited(path) => path.as_os_str(),
            Self::Oxide(owned) => owned.path.as_os_str(),
        };
        for name in VARIABLES {
            command.env(name, value);
        }
    }
}

/// Choose the child temporary directory for one leased command, or refuse.
///
/// `inherited` is the caller's `TMPDIR`, `pool` the selected pool's payload
/// root (its device is the pool's) and `slot` the held slot directory. There
/// is no third answer on purpose: a command whose children can be given
/// neither qualified directory refuses before it spawns, rather than quietly
/// leaving them on the machine default this decision rejected.
pub(crate) fn choose(
    inherited: Option<OsString>,
    pool: &std::path::Path,
    slot: &std::path::Path,
    identity: Identity,
) -> Result<Chosen, String> {
    choose_with(inherited, pool, slot, identity, MAX_SCRATCH_PATH_BYTES)
}

/// [`choose`] with an explicit path bound. The bound is a parameter so a
/// unit test's fixture root length cannot silently change which branch runs;
/// every product call uses [`MAX_SCRATCH_PATH_BYTES`].
pub(crate) fn choose_with(
    inherited: Option<OsString>,
    pool: &std::path::Path,
    slot: &std::path::Path,
    identity: Identity,
    bound: usize,
) -> Result<Chosen, String> {
    let pool_dev = fs::metadata(pool)
        .map_err(|error| format!("cannot read the selected pool {}: {error}", pool.display()))?
        .dev();
    if let Some(value) = inherited {
        if let Some(canonical) = inheritable(&value, pool_dev) {
            return Ok(Chosen::Inherited(canonical));
        }
    }
    prepare(slot, pool_dev, &identity, bound).map(Chosen::Oxide)
}

/// The inherited directory to keep, resolved canonically, when it is an
/// existing directory this user can write on the pool's own device. The
/// canonical form is what children are given, so a relative or symlinked
/// `TMPDIR` can never be re-resolved against a child's own working
/// directory. No prefix comparison, no mount mutation.
pub(crate) fn inheritable(value: &OsStr, pool_dev: u64) -> Option<PathBuf> {
    if value.is_empty() {
        return None;
    }
    let canonical = fs::canonicalize(value).ok()?;
    let meta = fs::metadata(&canonical).ok()?;
    (meta.is_dir() && meta.dev() == pool_dev && writable(&canonical)).then_some(canonical)
}
fn writable(path: &std::path::Path) -> bool {
    let Ok(text) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `text` is a valid NUL-terminated path and the call only asks
    // the kernel about access.
    unsafe { libc::faccessat(libc::AT_FDCWD, text.as_ptr(), libc::W_OK, libc::AT_EACCESS) == 0 }
}

fn reported(path: &std::path::Path) -> String {
    wrapper::shown(&path.to_string_lossy())
}

/// Create (or reconcile this incarnation's leftover) owned scratch inside the
/// held slot. Every failure is a refusal to redirect, never a deletion of
/// something Oxide cannot positively identify.
fn prepare(
    slot: &std::path::Path,
    pool_dev: u64,
    identity: &Identity,
    bound: usize,
) -> Result<Owned, String> {
    let slot_meta = fs::symlink_metadata(slot)
        .map_err(|error| format!("cannot read the held slot {}: {error}", reported(slot)))?;
    if !slot_meta.is_dir() || slot_meta.file_type().is_symlink() {
        return Err(format!(
            "the held slot {} is not a real directory",
            reported(slot)
        ));
    }
    if slot_meta.uid() != effective_uid() {
        return Err(format!(
            "the held slot {} is not owned by this user",
            reported(slot)
        ));
    }
    if slot_meta.dev() != pool_dev {
        return Err(format!(
            "the held slot {} is not on the selected pool's device",
            reported(slot)
        ));
    }
    let path = slot.join(SCRATCH_NAME);
    let bytes = path.as_os_str().as_bytes().len();
    if bytes > bound {
        return Err(format!(
            "the per-command temporary directory {} is {bytes} bytes, over the {bound}-byte Unix-socket bound",
            reported(&path)
        ));
    }
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "cannot read the per-command temporary directory {}: {error}",
                reported(&path)
            ))
        }
        Ok(_) => {
            // A directory inside the slot this lease now owns can only be a
            // previous command's leftover: reconcile it when, and only when,
            // its marker binds it to this same slot incarnation.
            remove(&path, pool_dev, identity)?;
        }
    }
    // The directory and its marker are built under a staging name, then renamed
    // into place: a crash can never leave the scratch name without a marker,
    // which would refuse every later command in this slot.
    let staging = slot.join(format!("{SCRATCH_NAME}.staging-{}", std::process::id()));
    DirBuilder::new()
        .mode(0o700)
        .create(&staging)
        .map_err(|error| {
            format!(
                "cannot create the per-command temporary directory {}: {error}",
                reported(&path)
            )
        })?;
    if let Err(reason) = write_owner(&staging, identity) {
        let _ = fs::remove_dir_all(&staging);
        return Err(reason);
    }
    if let Err(error) = fs::rename(&staging, &path) {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "cannot record the per-command temporary directory {}: {error}",
            reported(&path)
        ));
    }
    Ok(Owned {
        path,
        dev: pool_dev,
        identity: identity.clone(),
    })
}

/// Remove one owned directory after positive writer closure. Returns a
/// warning instead of failing the command; the slot's own accounting and
/// deletion are unchanged, so a kept leftover is never counted twice.
pub(crate) fn discard(owned: &Owned) -> Result<(), String> {
    remove(&owned.path, owned.dev, &owned.identity)
}

/// A directory held open for removal. Entries are named relative to it, so a
/// path that is swapped for a link after the handle was opened cannot redirect
/// a removal outside the tree the handle names.
pub(crate) struct Dir(OwnedFd);

/// One entry's identity and kind, read without following a link.
pub(crate) struct Entry {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) uid: u32,
    pub(crate) dir: bool,
}

/// Open flags for a directory handle: a link at the last component is refused.
const DIR_FLAGS: libc::c_int =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

impl Dir {
    /// Open `path` as a directory. Components above the last are resolved as
    /// any path is; the last one may not be a link.
    pub(crate) fn open(path: &std::path::Path) -> std::io::Result<Self> {
        let name = c_string(path.as_os_str())?;
        // SAFETY: `name` is a valid NUL-terminated path that lives for the call.
        owned(unsafe { libc::open(name.as_ptr(), DIR_FLAGS) }).map(Self)
    }

    pub(crate) fn try_clone(&self) -> std::io::Result<Self> {
        self.0.try_clone().map(Self)
    }

    /// The device and inode of this directory.
    pub(crate) fn identity(&self) -> std::io::Result<(u64, u64)> {
        let stat = stat_of(|stat| {
            // SAFETY: the descriptor is open for the call and `stat` is a valid out-pointer.
            unsafe { libc::fstat(self.0.as_raw_fd(), stat) }
        })?;
        Ok((stat.st_dev as u64, stat.st_ino as u64))
    }

    /// One entry of this directory, read without following a link.
    pub(crate) fn entry(&self, name: &OsStr) -> std::io::Result<Entry> {
        let name = c_string(name)?;
        let stat = stat_of(|stat| {
            // SAFETY: the descriptor is open, `name` is NUL-terminated and `stat` is a valid out-pointer.
            unsafe {
                libc::fstatat(
                    self.0.as_raw_fd(),
                    name.as_ptr(),
                    stat,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            }
        })?;
        Ok(Entry {
            dev: stat.st_dev as u64,
            ino: stat.st_ino as u64,
            uid: stat.st_uid,
            dir: stat.st_mode & libc::S_IFMT == libc::S_IFDIR,
        })
    }

    /// A child directory of this one, refused when the name is a link.
    pub(crate) fn open_dir(&self, name: &OsStr) -> std::io::Result<Self> {
        let name = c_string(name)?;
        // SAFETY: the descriptor is open and `name` is a valid NUL-terminated path.
        owned(unsafe { libc::openat(self.0.as_raw_fd(), name.as_ptr(), DIR_FLAGS) }).map(Self)
    }

    /// A regular child file, opened without following a link.
    pub(crate) fn open_file(&self, name: &OsStr) -> std::io::Result<std::fs::File> {
        let name = c_string(name)?;
        // SAFETY: the descriptor is open and `name` is a valid NUL-terminated path.
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        owned(fd).map(std::fs::File::from)
    }

    /// Unlink one entry of this directory; `dir` selects the directory form.
    /// A link is unlinked itself, never followed.
    pub(crate) fn remove(&self, name: &OsStr, dir: bool) -> std::io::Result<()> {
        let name = c_string(name)?;
        let flags = if dir { libc::AT_REMOVEDIR } else { 0 };
        // SAFETY: the descriptor is open and `name` is a valid NUL-terminated path.
        if unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), flags) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    /// The names in this directory, without `.` and `..`. A read that ends early
    /// leaves entries behind, so the directory's own removal then refuses.
    pub(crate) fn names(&self) -> std::io::Result<Vec<OsString>> {
        // fdopendir takes ownership of its descriptor, so it gets a duplicate.
        let fd = self.0.try_clone()?.into_raw_fd();
        // SAFETY: `fd` is an open directory; fdopendir owns it from here on.
        let dir = unsafe { libc::fdopendir(fd) };
        if dir.is_null() {
            let error = std::io::Error::last_os_error();
            // SAFETY: fdopendir failed, so `fd` is still ours to close.
            unsafe { libc::close(fd) };
            return Err(error);
        }
        let mut names = Vec::new();
        loop {
            // SAFETY: `dir` is open until closedir below.
            let entry = unsafe { libc::readdir(dir) };
            if entry.is_null() {
                break;
            }
            // SAFETY: readdir returned a live entry whose name is NUL-terminated.
            let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
            let bytes = name.to_bytes();
            if bytes != b"." && bytes != b".." {
                names.push(OsStr::from_bytes(bytes).to_owned());
            }
        }
        // SAFETY: `dir` is open and not used after this call.
        unsafe { libc::closedir(dir) };
        Ok(names)
    }
}

fn c_string(name: &OsStr) -> std::io::Result<std::ffi::CString> {
    std::ffi::CString::new(name.as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "a path holds a NUL byte")
    })
}

fn owned(fd: libc::c_int) -> std::io::Result<OwnedFd> {
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a non-negative result from open or openat is a fresh descriptor we own.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn stat_of(call: impl FnOnce(*mut libc::stat) -> libc::c_int) -> std::io::Result<libc::stat> {
    // SAFETY: an all-zero `stat` is a valid buffer for the kernel to fill.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if call(&mut stat) == 0 {
        Ok(stat)
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Read the marker through a held directory, without following a link and
/// without trusting a size the caller did not write.
fn read_owner(dir: &Dir, label: &std::path::Path) -> Result<String, String> {
    let path = label.join(OWNER_NAME);
    let file = dir
        .open_file(OsStr::new(OWNER_NAME))
        .map_err(|error| format!("cannot read the marker {}: {error}", reported(&path)))?;
    let meta = file
        .metadata()
        .map_err(|error| format!("cannot read the marker {}: {error}", reported(&path)))?;
    if !meta.is_file() || meta.uid() != effective_uid() || meta.len() > MAX_OWNER_BYTES {
        return Err(format!(
            "the marker {} is not a bounded file of this user",
            reported(&path)
        ));
    }
    let mut text = String::new();
    file.take(MAX_OWNER_BYTES)
        .read_to_string(&mut text)
        .map_err(|error| format!("cannot read the marker {}: {error}", reported(&path)))?;
    Ok(text)
}

/// Write and sync the ownership marker of a new directory.
fn write_owner(dir: &std::path::Path, identity: &Identity) -> Result<(), String> {
    let record = |error: std::io::Error| {
        format!(
            "cannot record the per-command temporary directory {}: {error}",
            reported(dir)
        )
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join(OWNER_NAME))
        .map_err(record)?;
    file.write_all(owner_text(identity).as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(record)
}

fn owner_text(identity: &Identity) -> String {
    format!(
        "{OWNER_VERSION}\nstore={}\nslot={}\nincarnation={}\nlease={}\n",
        identity.store_id, identity.slot_id, identity.incarnation, identity.lease_id
    )
}

fn parse_owner(text: &str) -> Option<Identity> {
    let mut lines = text.lines();
    if lines.next()? != OWNER_VERSION {
        return None;
    }
    let mut field = |name: &str| -> Option<String> {
        let line = lines.next()?;
        line.strip_prefix(name)?
            .strip_prefix('=')
            .map(str::to_owned)
    };
    let store_id = field("store")?;
    let slot_id = field("slot")?;
    let incarnation = field("incarnation")?;
    let lease_id = field("lease")?;
    if lines.next().is_some() {
        return None;
    }
    Some(Identity {
        store_id,
        slot_id,
        incarnation,
        lease_id,
    })
}

/// Remove this command's scratch, refusing anything that is no longer the
/// positively identified directory (marker, owner, device, real directory).
/// The parent is held open and every check and removal names its entry through
/// that handle, so no path component is followed after the checks.
pub(crate) fn remove(path: &std::path::Path, dev: u64, identity: &Identity) -> Result<(), String> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(format!("{} has no parent to hold it", reported(path)));
    };
    let parent_dir = Dir::open(parent).map_err(|error| {
        format!(
            "cannot read the parent of the per-command temporary directory {}: {error}",
            reported(path)
        )
    })?;
    let entry = match parent_dir.entry(name) {
        Ok(entry) => entry,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "cannot read the per-command temporary directory {}: {error}",
                reported(path)
            ))
        }
    };
    if !entry.dir {
        return Err(format!(
            "{} is not a real directory, so it is kept",
            reported(path)
        ));
    }
    if entry.uid != effective_uid() {
        return Err(format!(
            "{} is not owned by this user, so it is kept",
            reported(path)
        ));
    }
    if entry.dev != dev {
        return Err(format!(
            "{} is not on the pool's device, so it is kept",
            reported(path)
        ));
    }
    let top = parent_dir
        .open_dir(name)
        .map_err(|error| format!("cannot open {}: {error}", reported(path)))?;
    if top.identity().ok() != Some((entry.dev, entry.ino)) {
        return Err(format!(
            "{} changed while it was being checked, so it is kept",
            reported(path)
        ));
    }
    let text = read_owner(&top, path)?;
    let found = parse_owner(&text).ok_or_else(|| {
        format!(
            "{} has no Oxide ownership marker, so it is kept",
            reported(path)
        )
    })?;
    if found.binding() != identity.binding() {
        return Err(format!(
            "{} belongs to another slot incarnation, so it is kept",
            reported(path)
        ));
    }
    remove_contents(&top, dev, path)?;
    parent_dir.remove(name, true).map_err(|error| {
        format!(
            "cannot remove the per-command temporary directory {}: {error}",
            reported(path)
        )
    })
}

/// Deepest nesting removed below the directory being emptied. Matches the
/// bound `clean.rs` applies, so the two walks refuse the same trees.
const MAX_DEPTH: usize = 256;

/// One directory below the one being emptied: its open handle, its name in the
/// parent, and the entries not yet visited.
struct Level {
    dir: Dir,
    name: OsString,
    label: std::path::PathBuf,
    names: std::vec::IntoIter<OsString>,
}

fn entry_names(dir: &Dir, label: &std::path::Path) -> Result<std::vec::IntoIter<OsString>, String> {
    dir.names().map(Vec::into_iter).map_err(|error| {
        format!(
            "cannot read the per-command temporary directory {}: {error}",
            reported(label)
        )
    })
}

/// Remove everything inside `dir`, children first, refusing any other device.
///
/// The walk keeps an explicit stack of open directories rather than recursing,
/// so a deep tree cannot exhaust the call stack, and it refuses past
/// [`MAX_DEPTH`] rather than remove an incomplete tree.
fn remove_contents(dir: &Dir, dev: u64, label: &std::path::Path) -> Result<(), String> {
    let mut root_names = entry_names(dir, label)?;
    let mut levels: Vec<Level> = Vec::new();
    loop {
        let next = match levels.last_mut() {
            Some(level) => level.names.next(),
            None => root_names.next(),
        };
        let Some(name) = next else {
            // The innermost directory is empty: remove it from its parent and
            // continue with the parent's remaining entries.
            let Some(finished) = levels.pop() else {
                return Ok(());
            };
            let parent = levels.last().map_or(dir, |level| &level.dir);
            parent
                .remove(&finished.name, true)
                .map_err(|error| format!("cannot remove {}: {error}", reported(&finished.label)))?;
            continue;
        };
        let (current, current_label) = match levels.last() {
            Some(level) => (&level.dir, level.label.clone()),
            None => (dir, label.to_path_buf()),
        };
        let child = current_label.join(&name);
        let entry = current
            .entry(&name)
            .map_err(|error| format!("cannot read {}: {error}", reported(&child)))?;
        if entry.dev != dev {
            return Err(format!(
                "{} is on another device, so the directory is kept",
                reported(&child)
            ));
        }
        if !entry.dir {
            current
                .remove(&name, false)
                .map_err(|error| format!("cannot remove {}: {error}", reported(&child)))?;
            continue;
        }
        if levels.len() + 1 >= MAX_DEPTH {
            return Err(format!(
                "{} is nested deeper than {MAX_DEPTH} levels; the directory is kept",
                reported(&child)
            ));
        }
        let inner = current
            .open_dir(&name)
            .map_err(|error| format!("cannot open {}: {error}", reported(&child)))?;
        if inner.identity().ok() != Some((entry.dev, entry.ino)) {
            return Err(format!(
                "{} changed while it was being removed, so it is kept",
                reported(&child)
            ));
        }
        let names = entry_names(&inner, &child)?;
        levels.push(Level {
            dir: inner,
            name,
            label: child,
            names,
        });
    }
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// The status line for the selected pool's device, without creating
/// anything: the decision `choose` would make for a leased command there.
pub(crate) fn describe(inherited: Option<&OsStr>, pool_dev: u64) -> String {
    match inherited.and_then(|value| inheritable(value, pool_dev)) {
        Some(path) => format!(
            "temp: inherited {}",
            wrapper::shown(&path.to_string_lossy())
        ),
        None => "temp: oxide (per leased command, inside the selected pool's slot)".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    /// A fresh private fixture directory under the test temp root.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new(label: &str) -> Self {
            // Short, like a real pool: the fixture must leave room for a
            // slot path under the injected temporary-directory bound.
            let base = std::env::temp_dir().join(format!(
                "ot{}-{}-{label}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&base).expect("fixture root");
            Self(fs::canonicalize(&base).expect("canonical fixture root"))
        }

        fn pool(&self) -> PathBuf {
            let pool = self.0.join("payload");
            fs::create_dir_all(&pool).expect("payload");
            pool
        }

        fn slot(&self, pool: &std::path::Path) -> PathBuf {
            let slot = pool.join("slots").join("oxide").join("quick-1");
            fs::create_dir_all(slot.join("build")).expect("slot");
            slot
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The plan under test with a fixture-safe path bound: the ambient
    /// temporary root is long, and these cases are about routing, ownership
    /// and lifecycle, not the length limit.
    fn try_pick(
        inherited: Option<OsString>,
        pool: &std::path::Path,
        slot: &std::path::Path,
        identity: Identity,
    ) -> Result<Chosen, String> {
        choose_with(inherited, pool, slot, identity, 4096)
    }

    fn pick(
        inherited: Option<OsString>,
        pool: &std::path::Path,
        slot: &std::path::Path,
        identity: Identity,
    ) -> Chosen {
        try_pick(inherited, pool, slot, identity).expect("the plan under test")
    }

    /// The plan under test at the product's own path bound.
    fn pick_bounded(
        inherited: Option<OsString>,
        pool: &std::path::Path,
        slot: &std::path::Path,
        identity: Identity,
    ) -> Result<Chosen, String> {
        choose_with(inherited, pool, slot, identity, MAX_SCRATCH_PATH_BYTES)
    }

    fn identity(incarnation: &str, lease: &str) -> Identity {
        Identity {
            store_id: "store".to_owned(),
            slot_id: "1".to_owned(),
            incarnation: incarnation.to_owned(),
            lease_id: lease.to_owned(),
        }
    }

    fn device(path: &std::path::Path) -> u64 {
        fs::metadata(path).expect("metadata").dev()
    }

    fn envs(command: &std::process::Command) -> Vec<(String, String)> {
        command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.unwrap_or_default().to_string_lossy().into_owned(),
                )
            })
            .filter(|(name, _)| VARIABLES.contains(&name.as_str()))
            .collect()
    }

    #[test]
    fn an_unset_tmpdir_gets_one_owned_directory_and_overrides_the_aliases() {
        let fixture = Fixture::new("owned");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let chosen = pick(None, &pool, &slot, identity("1", "lease-a"));
        let owned = chosen.owned().expect("owned directory");
        assert_eq!(owned.path(), slot.join("scratch"));
        assert!(owned.path().is_dir());
        assert_eq!(
            owned.path().metadata().expect("metadata").mode() & 0o777,
            0o700
        );
        assert_eq!(
            chosen.note(),
            format!("temp: oxide {}", owned.path().display())
        );
        let mut command = std::process::Command::new("true");
        command.env("TMP", "/elsewhere").env("TEMP", "/third");
        chosen.apply(&mut command);
        let seen = envs(&command);
        for name in VARIABLES {
            assert_eq!(
                seen.iter()
                    .find(|(found, _)| found == name)
                    .map(|(_, value)| value.as_str()),
                Some(owned.path().to_string_lossy().as_ref()),
                "{name} does not name the owned directory"
            );
        }
    }

    #[test]
    fn an_inherited_directory_on_the_pool_device_is_kept_canonically() {
        let fixture = Fixture::new("inherited");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let inherited = pool.join("effigy-run");
        fs::create_dir_all(&inherited).expect("inherited");
        fs::write(inherited.join("evidence"), b"retained").expect("evidence");
        let chosen = pick(
            Some(inherited.clone().into_os_string()),
            &pool,
            &slot,
            identity("1", "lease-a"),
        );
        match &chosen {
            Chosen::Inherited(path) => {
                assert_eq!(path, &fs::canonicalize(&inherited).expect("canonical"))
            }
            other => panic!("expected the inherited directory, got {other:?}"),
        }
        assert!(chosen.owned().is_none());
        assert!(!slot.join(SCRATCH_NAME).exists());
        assert_eq!(
            fs::read_dir(&inherited).expect("read").count(),
            1,
            "Oxide wrote into the inherited directory"
        );
        assert!(chosen.note().starts_with("temp: inherited "));
        // Aliases follow the chosen value, whatever the caller set.
        let mut command = std::process::Command::new("true");
        command.env("TMP", "/somewhere/else");
        chosen.apply(&mut command);
        let seen = envs(&command);
        for name in VARIABLES {
            assert_eq!(
                seen.iter()
                    .find(|(found, _)| found == name)
                    .map(|(_, value)| value.as_str()),
                Some(inherited.to_string_lossy().as_ref()),
                "{name} does not follow the kept directory"
            );
        }
    }

    #[test]
    fn a_symlinked_inherited_value_publishes_the_canonical_directory() {
        // A `TMPDIR` a child could re-resolve — here through a link, in the
        // proof through a relative path — must reach children as the one
        // canonical directory this decision qualified.
        let fixture = Fixture::new("canonical");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let real = pool.join("real-tmp");
        fs::create_dir_all(&real).expect("real");
        let link = fixture.0.join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        assert_eq!(
            inheritable(link.as_os_str(), device(&pool)),
            Some(fs::canonicalize(&real).expect("canonical"))
        );
        let chosen = pick(
            Some(link.clone().into_os_string()),
            &pool,
            &slot,
            identity("1", "lease-a"),
        );
        match &chosen {
            Chosen::Inherited(path) => {
                assert_eq!(path, &fs::canonicalize(&real).expect("canonical"));
                assert_ne!(path, &link, "the raw link text reached the child");
            }
            other => panic!("expected the inherited directory, got {other:?}"),
        }
    }

    #[test]
    fn a_rejected_inherited_value_is_replaced_by_the_owned_directory() {
        let fixture = Fixture::new("replaced");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let other = std::env::temp_dir();
        assert_eq!(inheritable(OsStr::new(""), device(&pool)), None);
        assert_eq!(
            inheritable(&pool.join("missing").into_os_string(), device(&pool)),
            None
        );
        assert_eq!(inheritable(OsStr::new("relative/tmp"), device(&pool)), None);
        let file = pool.join("not-a-dir");
        fs::write(&file, b"file").expect("file");
        assert_eq!(inheritable(file.as_os_str(), device(&pool)), None);
        let closed = fixture.0.join("closed");
        fs::create_dir_all(&closed).expect("closed");
        fs::set_permissions(&closed, fs::Permissions::from_mode(0o500)).expect("chmod");
        let unwritable = inheritable(closed.as_os_str(), device(&pool));
        fs::set_permissions(&closed, fs::Permissions::from_mode(0o700)).expect("chmod");
        assert_eq!(
            unwritable, None,
            "a directory this user cannot write is replaced"
        );
        let escaped = fixture.0.join("escaped");
        std::os::unix::fs::symlink(pool.join("absent"), &escaped).expect("symlink");
        assert_eq!(inheritable(escaped.as_os_str(), device(&pool)), None);
        for value in [
            pool.join("missing").into_os_string(),
            OsString::from("relative/tmp"),
            file.clone().into_os_string(),
            escaped.into_os_string(),
        ] {
            let chosen = pick(Some(value), &pool, &slot, identity("1", "lease-a"));
            assert!(chosen.owned().is_some(), "not replaced: {chosen:?}");
        }
        if device(&other) != device(&pool) {
            assert_eq!(inheritable(other.as_os_str(), device(&pool)), None);
        }
    }

    #[test]
    fn an_unpreparable_owned_directory_refuses_instead_of_leaving_the_default() {
        let fixture = Fixture::new("refuse");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        // A path over the socket bound.
        let long = pool.join("slots").join("x".repeat(40)).join("quick-1");
        fs::create_dir_all(long.join("build")).expect("slot");
        match pick_bounded(None, &pool, &long, identity("1", "lease-a")) {
            Err(reason) => {
                assert!(reason.contains("Unix-socket bound"), "{reason}");
                assert!(reason.contains("bytes"), "{reason}");
            }
            Ok(other) => panic!("expected a refusal, got {other:?}"),
        }
        assert!(!long.join(SCRATCH_NAME).exists());
        // A foreign entry and a link in the owned position.
        let scratch = slot.join(SCRATCH_NAME);
        fs::create_dir_all(&scratch).expect("scratch");
        fs::write(scratch.join("not-ours"), b"keep").expect("foreign");
        match try_pick(None, &pool, &slot, identity("1", "lease-a")) {
            Err(reason) => assert!(reason.contains("marker"), "{reason}"),
            Ok(other) => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(fs::read(scratch.join("not-ours")).expect("read"), b"keep");
        fs::remove_dir_all(&scratch).expect("cleanup");

        let elsewhere = fixture.0.join("elsewhere");
        fs::create_dir_all(&elsewhere).expect("elsewhere");
        let keep = elsewhere.join("keep");
        fs::write(&keep, b"keep").expect("keep");
        std::os::unix::fs::symlink(&elsewhere, &scratch).expect("symlink");
        match try_pick(None, &pool, &slot, identity("1", "lease-a")) {
            Err(reason) => assert!(reason.contains("real directory"), "{reason}"),
            Ok(other) => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(fs::read(&keep).expect("read"), b"keep");
    }

    #[test]
    fn the_owner_marker_binds_pool_slot_and_incarnation() {
        let fixture = Fixture::new("marker");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let chosen = pick(None, &pool, &slot, identity("7", "lease-a"));
        let owned = chosen.owned().expect("owned");
        let text = fs::read_to_string(owned.path().join(OWNER_NAME)).expect("marker");
        let parsed = parse_owner(&text).expect("parse");
        assert_eq!(parsed.binding(), ("store", "1", "7"));
        assert_eq!(parsed.lease_id, "lease-a");
        assert_eq!(parse_owner(""), None);
        assert_eq!(parse_owner("oxide.scratch/1\nstore=x\n"), None);
        assert_eq!(
            parse_owner("oxide.scratch/2\nstore=a\nslot=1\nincarnation=1\nlease=l\n"),
            None
        );
        assert_eq!(
            parse_owner("oxide.scratch/1\nstore=a\nslot=1\nincarnation=1\nlease=l\nextra=1\n"),
            None
        );
    }

    #[test]
    fn a_same_incarnation_leftover_is_reconciled_and_another_incarnation_is_kept() {
        let fixture = Fixture::new("reconcile");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let owned = pick(None, &pool, &slot, identity("1", "lease-a"))
            .owned()
            .expect("owned");
        fs::write(owned.path().join("leftover"), b"x").expect("leftover");
        // Ownership binds to the slot and incarnation, not to one lease ID.
        assert!(remove(owned.path(), owned.dev, &identity("1", "lease-b")).is_ok());
        assert!(!owned.path().exists());

        let owned = pick(None, &pool, &slot, identity("1", "lease-b"))
            .owned()
            .expect("owned");
        fs::write(owned.path().join("leftover"), b"x").expect("leftover");
        assert!(discard(&owned).is_ok());
        assert!(!owned.path().exists());

        // A leftover bound to another incarnation is refused, not deleted.
        let scratch = slot.join(SCRATCH_NAME);
        fs::create_dir_all(&scratch).expect("leftover");
        fs::write(scratch.join("leftover"), b"keep").expect("leftover");
        fs::write(
            scratch.join(OWNER_NAME),
            owner_text(&identity("2", "lease-c")).as_bytes(),
        )
        .expect("marker");
        match try_pick(None, &pool, &slot, identity("3", "lease-d")) {
            Err(reason) => assert!(reason.contains("another slot incarnation"), "{reason}"),
            Ok(other) => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(fs::read(scratch.join("leftover")).expect("read"), b"keep");

        // The incarnation that owns it reconciles it as its own.
        let owned = match try_pick(None, &pool, &slot, identity("2", "lease-e")) {
            Ok(Chosen::Oxide(owned)) => owned,
            other => panic!("expected reconciliation by the owning incarnation, got {other:?}"),
        };
        assert!(!owned.path().join("leftover").exists());
        assert!(discard(&owned).is_ok());
        assert!(!owned.path().exists());
    }

    #[test]
    fn removal_refuses_a_device_or_incarnation_that_does_not_match() {
        let fixture = Fixture::new("device");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let owned = pick(None, &pool, &slot, identity("1", "lease-a"))
            .owned()
            .expect("owned");
        let wrong_device = owned.dev.wrapping_add(1);
        let reason = remove(owned.path(), wrong_device, &identity("1", "lease-a"))
            .expect_err("refuse another device");
        assert!(reason.contains("not on the pool's device"), "{reason}");
        let reason = remove(owned.path(), owned.dev, &identity("2", "lease-a"))
            .expect_err("refuse another incarnation");
        assert!(reason.contains("another slot incarnation"), "{reason}");
        assert!(owned.path().exists());
        assert!(discard(&owned).is_ok());
        assert!(!owned.path().exists());
    }

    #[test]
    fn a_hard_linked_pair_is_removed_once_and_a_link_is_never_followed() {
        let fixture = Fixture::new("hardlink");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let owned = pick(None, &pool, &slot, identity("1", "lease-a"))
            .owned()
            .expect("owned");
        let first = owned.path().join("a");
        fs::write(&first, b"x").expect("write");
        fs::hard_link(&first, owned.path().join("b")).expect("hard link");
        let outside = fixture.0.join("outside");
        fs::create_dir_all(&outside).expect("outside");
        let target = outside.join("keep");
        fs::write(&target, b"keep").expect("keep");
        std::os::unix::fs::symlink(&target, owned.path().join("escape")).expect("symlink");
        assert!(discard(&owned).is_ok());
        assert!(!owned.path().exists());
        assert_eq!(fs::read(&target).expect("read"), b"keep");
    }

    #[test]
    fn a_slot_that_is_a_link_or_on_another_device_is_refused() {
        let fixture = Fixture::new("slot");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let link = fixture.0.join("slot-link");
        std::os::unix::fs::symlink(&slot, &link).expect("symlink");
        match try_pick(None, &pool, &link, identity("1", "lease-a")) {
            Err(reason) => assert!(reason.contains("not a real directory"), "{reason}"),
            Ok(other) => panic!("expected a refusal, got {other:?}"),
        }
        let elsewhere = pool.parent().expect("parent").join("other-slot");
        fs::create_dir_all(&elsewhere).expect("other slot");
        if device(&elsewhere) != device(&pool) {
            assert!(try_pick(None, &pool, &elsewhere, identity("1", "lease-a")).is_err());
        }
    }

    #[test]
    fn the_status_line_names_the_decision_without_creating_anything() {
        let fixture = Fixture::new("status");
        let pool = fixture.pool();
        let inherited = pool.join("effigy-run");
        fs::create_dir_all(&inherited).expect("inherited");
        assert_eq!(
            describe(Some(inherited.as_os_str()), device(&pool)),
            format!(
                "temp: inherited {}",
                fs::canonicalize(&inherited).expect("canonical").display()
            )
        );
        assert_eq!(fs::read_dir(&inherited).expect("read").count(), 0);
        assert!(describe(None, device(&pool)).starts_with("temp: oxide"));
        assert!(describe(Some(OsStr::new("")), device(&pool)).starts_with("temp: oxide"));
    }

    #[test]
    fn the_note_is_one_escaped_bounded_line() {
        let chosen = Chosen::Inherited(PathBuf::from("/some/very/long/\npath"));
        let note = chosen.note();
        assert!(!note.contains('\n'), "{note}");
        assert!(note.starts_with("temp: inherited /some/very/long/\\npath"));
    }

    #[test]
    fn held_removal_refuses_links_and_never_leaves_its_tree() {
        let fixture = Fixture::new("held");
        let outside = fixture.0.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), b"keep").unwrap();
        let tree = fixture.0.join("tree");
        fs::create_dir(&tree).unwrap();
        fs::write(tree.join("own"), b"own").unwrap();
        std::os::unix::fs::symlink(&outside, tree.join("link")).unwrap();
        let held = Dir::open(&tree).unwrap();
        assert!(Dir::open(&tree.join("link")).is_err());
        assert!(held.open_dir(OsStr::new("link")).is_err());
        let dev = fs::metadata(&tree).unwrap().dev();
        remove_contents(&held, dev, &tree).unwrap();
        assert!(fs::symlink_metadata(tree.join("link")).is_err());
        assert!(fs::symlink_metadata(tree.join("own")).is_err());
        assert_eq!(fs::read(outside.join("keep")).unwrap(), b"keep");
    }

    /// A nested tree within the bound is removed completely, with its siblings.
    #[test]
    fn a_nested_tree_is_removed_completely_by_the_explicit_walk() {
        let fixture = Fixture::new("nested");
        let tree = fixture.0.join("tree");
        fs::create_dir(&tree).unwrap();
        let mut deep = tree.clone();
        for depth in 0..40 {
            fs::write(deep.join(format!("file{depth}")), b"x").unwrap();
            deep = deep.join("d");
            fs::create_dir(&deep).unwrap();
        }
        fs::create_dir_all(tree.join("sibling/a/b")).unwrap();
        fs::write(tree.join("sibling/a/b/leaf"), b"leaf").unwrap();
        let held = Dir::open(&tree).unwrap();
        let dev = fs::metadata(&tree).unwrap().dev();
        remove_contents(&held, dev, &tree).unwrap();
        assert_eq!(fs::read_dir(&tree).unwrap().count(), 0);
    }

    /// Regression: the recursive walk had no depth bound, so a tree nested
    /// without limit could exhaust the stack. It now refuses past `MAX_DEPTH`,
    /// keeping the tree rather than removing an incomplete part of it.
    #[test]
    fn a_tree_nested_past_the_depth_bound_is_refused() {
        let fixture = Fixture::new("deep");
        let tree = fixture.0.join("tree");
        fs::create_dir(&tree).unwrap();
        let mut deep = tree.clone();
        for _ in 0..MAX_DEPTH + 4 {
            deep = deep.join("d");
            fs::create_dir(&deep).unwrap();
        }
        let held = Dir::open(&tree).unwrap();
        let dev = fs::metadata(&tree).unwrap().dev();
        let error = remove_contents(&held, dev, &tree).expect_err("the walk must refuse");
        assert!(error.contains("nested deeper"), "{error}");
        assert!(deep.is_dir(), "the deepest directory must be kept");
    }

    #[test]
    fn the_socket_bound_is_below_the_platform_path() {
        // macOS `sun_path` is 104 bytes including the NUL; Linux is 108.
        assert!(MAX_SCRATCH_PATH_BYTES + 1 + 11 + 1 <= 104);
    }
}
