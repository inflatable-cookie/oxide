//! The post-clone recipient walk of `oxide#137` §6.3 step 7.
//!
//! One bounded descriptor-relative walk of the recipient slot, made while the
//! slot is still held and the donor pin is in place, before any caller may
//! proceed. It records an **observed-while-held** fact and never a closed
//! record: it is not an admission figure and is never read as one.
//!
//! The walk publishes only when it is complete and ownership-clean: every
//! entry owned by this user, on the slot's device, readable and inside the
//! slot, within the capacity contract's entry, depth and elapsed bounds. Every
//! other outcome is a gap, including a root that cannot be opened, stat'd or
//! proven owned, and a directory read that fails: a walk never reports clean
//! coverage it did not establish. The caller abandons seeding on any gap.
//!
//! Byte figures apply the capacity contract's **within-slot inode union**: a
//! hard-linked pair inside this one slot is counted once, because the figure
//! describes this slot's own bytes and the seed never creates a link itself.

use super::super::current_uid;
use crate::owned_delete::open_directory_at;
use std::ffi::{CStr, CString};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
#[cfg(test)]
use std::os::unix::ffi::OsStrExt;
#[cfg(test)]
use std::path::Path;
use std::time::{Duration, Instant};

/// Bounds of one walk: the capacity contract's slot-walk limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WalkLimits {
    pub max_entries: u64,
    pub max_depth: usize,
    pub max_elapsed_ms: u64,
}

/// Why a walk is not clean. The first gap stops the walk: coverage is already
/// incomplete, so continuing would only spend work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WalkGap {
    EntryLimit,
    DepthLimit,
    Deadline,
    DeviceBoundary,
    Unreadable,
    ForeignOwner,
    UnexpectedType,
}

impl WalkGap {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EntryLimit => "entry-limit",
            Self::DepthLimit => "depth-limit",
            Self::Deadline => "deadline",
            Self::DeviceBoundary => "device-boundary",
            Self::Unreadable => "unreadable",
            Self::ForeignOwner => "foreign-owner",
            Self::UnexpectedType => "unexpected-type",
        }
    }
}

/// What one walk observed. It is a bounded observation of one held
/// generation, never proof of closure or an admission measurement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WalkOutcome {
    pub entries: u64,
    pub logical_bytes: u64,
    pub allocated_bytes: u64,
    pub gap: Option<WalkGap>,
    pub elapsed_ms: u64,
}

/// Walk `root` on `device` with `limits`. A root that cannot be opened,
/// stat'd or proven to be an owned directory on the pool's device is a gap,
/// never an error and never a clean zero-byte observation: the caller abandons
/// seeding on any gap.
///
/// This path-based entry exists for direct root-failure regressions. The
/// seeding call walks [`walk_slot_in`], from the slot descriptor it already
/// verified, so no path component can be redirected between the check and the
/// walk.
#[cfg(test)]
pub fn walk_slot(root: &Path, device: u64, limits: WalkLimits, started: Instant) -> WalkOutcome {
    match open_root(root) {
        Ok(fd) => walk_slot_in(fd.as_fd(), device, limits, started),
        Err(_) => WalkOutcome {
            entries: 0,
            logical_bytes: 0,
            allocated_bytes: 0,
            gap: Some(WalkGap::Unreadable),
            elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        },
    }
}

/// Walk an already-open slot root. The descriptor is the one custody verified,
/// so the walk cannot be redirected to another directory by a path change.
pub fn walk_slot_in(
    root: BorrowedFd<'_>,
    device: u64,
    limits: WalkLimits,
    started: Instant,
) -> WalkOutcome {
    let mut walk = Walk {
        limits,
        started,
        deadline: Duration::from_millis(limits.max_elapsed_ms),
        device,
        entries: 0,
        inodes: std::collections::HashMap::new(),
        gap: None,
    };
    match stat(root) {
        Ok(root_stat) if root_stat.st_mode & libc::S_IFMT != libc::S_IFDIR => {
            walk.stop(WalkGap::UnexpectedType)
        }
        Ok(root_stat) if root_stat.st_uid != current_uid() => walk.stop(WalkGap::ForeignOwner),
        Ok(root_stat) if root_stat.st_dev as u64 != device => walk.stop(WalkGap::DeviceBoundary),
        Ok(_) => walk.visit(root, 0),
        Err(_) => walk.stop(WalkGap::Unreadable),
    }
    WalkOutcome {
        entries: walk.entries,
        logical_bytes: walk.logical_bytes(),
        allocated_bytes: walk.allocated_bytes(),
        gap: walk.gap,
        elapsed_ms: walk.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    }
}

/// Whether an owned directory has no entries at all. Used for the recipient's
/// `build/` precondition: Finder metadata is an entry too.
pub fn directory_is_empty(fd: BorrowedFd<'_>) -> std::io::Result<bool> {
    Ok(list_names(fd, 1)?.is_empty())
}

struct Walk {
    limits: WalkLimits,
    started: Instant,
    deadline: Duration,
    device: u64,
    entries: u64,
    /// The within-slot inode union: one record per `(device, inode)`, so a
    /// hard-linked pair in this slot contributes its bytes once.
    inodes: std::collections::HashMap<(u64, u64), (u64, u64)>,
    gap: Option<WalkGap>,
}

impl Walk {
    fn logical_bytes(&self) -> u64 {
        self.inodes
            .values()
            .map(|(logical, _)| *logical)
            .fold(0u64, u64::saturating_add)
    }

    fn allocated_bytes(&self) -> u64 {
        self.inodes
            .values()
            .map(|(_, allocated)| *allocated)
            .fold(0u64, u64::saturating_add)
    }
}

impl Walk {
    fn stop(&mut self, gap: WalkGap) {
        if self.gap.is_none() {
            self.gap = Some(gap);
        }
    }

    fn out_of_time(&self) -> bool {
        self.started.elapsed() > self.deadline
    }

    fn visit(&mut self, directory: BorrowedFd<'_>, depth: usize) {
        if self.gap.is_some() {
            return;
        }
        let remaining = self.limits.max_entries.saturating_sub(self.entries);
        let names = match list_names(directory, remaining) {
            Ok(names) => names,
            Err(_) => {
                self.stop(WalkGap::Unreadable);
                return;
            }
        };
        for name in names {
            if self.gap.is_some() {
                return;
            }
            if self.out_of_time() {
                self.stop(WalkGap::Deadline);
                return;
            }
            self.entries += 1;
            if self.entries > self.limits.max_entries {
                self.stop(WalkGap::EntryLimit);
                return;
            }
            let entry = match stat_at(directory, &name) {
                Ok(entry) => entry,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    self.stop(WalkGap::Unreadable);
                    return;
                }
            };
            if entry.st_uid != current_uid() {
                self.stop(WalkGap::ForeignOwner);
                return;
            }
            if entry.st_dev as u64 != self.device {
                self.stop(WalkGap::DeviceBoundary);
                return;
            }
            match entry.st_mode & libc::S_IFMT {
                libc::S_IFDIR => {
                    if depth + 1 > self.limits.max_depth {
                        self.stop(WalkGap::DepthLimit);
                        return;
                    }
                    let child = match open_directory_at(directory, &name) {
                        Ok(child) => child,
                        Err(_) => {
                            self.stop(WalkGap::Unreadable);
                            return;
                        }
                    };
                    self.visit(child.as_fd(), depth + 1);
                }
                libc::S_IFREG => {
                    // Readable means the owner can open the file; the walk
                    // never reads its content.
                    match open_regular(&directory, &name) {
                        Ok(file) => drop(file),
                        Err(_) => {
                            self.stop(WalkGap::Unreadable);
                            return;
                        }
                    }
                    self.inodes.insert(
                        (entry.st_dev as u64, entry.st_ino as u64),
                        (
                            entry.st_size.max(0) as u64,
                            (entry.st_blocks.max(0) as u64).saturating_mul(512),
                        ),
                    );
                }
                _ => {
                    // A symlink, device, socket or FIFO is never something the
                    // seed created, so the generation is not ownership-clean.
                    self.stop(WalkGap::UnexpectedType);
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
fn open_root(root: &Path) -> std::io::Result<OwnedFd> {
    let root = CString::new(root.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("slot root holds a NUL"))?;
    // SAFETY: the path is a live NUL-terminated string.
    let fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` is a newly owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn stat(fd: BorrowedFd<'_>) -> std::io::Result<libc::stat> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `fd` is live and `stat` is valid for writes.
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(stat)
}

fn stat_at(directory: BorrowedFd<'_>, name: &CStr) -> std::io::Result<libc::stat> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: the descriptor and the name are live for the call.
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            &mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(stat)
}

pub(super) fn open_regular(directory: &BorrowedFd<'_>, name: &CStr) -> std::io::Result<OwnedFd> {
    // SAFETY: the descriptor and the name are live for the call.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` is a newly owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Read one directory's names, skipping `.` and `..`, and stop after
/// `remaining` more names than the caller may still visit. A `readdir` failure
/// is an error, never an empty or complete directory: `errno` is cleared before
/// each call and checked after a null result.
fn list_names(directory: BorrowedFd<'_>, remaining: u64) -> std::io::Result<Vec<CString>> {
    // fdopendir owns its descriptor, so hand it a duplicate.
    // SAFETY: `directory` is live.
    let duplicate = unsafe { libc::dup(directory.as_raw_fd()) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `duplicate` is an owned descriptor of a directory.
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        // SAFETY: `duplicate` was not adopted by fdopendir.
        unsafe { libc::close(duplicate) };
        return Err(std::io::Error::last_os_error());
    }
    let mut names = Vec::new();
    loop {
        clear_errno();
        // SAFETY: `stream` is a live directory stream.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            let error = current_errno();
            if error != 0 {
                // SAFETY: `stream` is live and not used again; it closes
                // `duplicate`.
                unsafe { libc::closedir(stream) };
                return Err(std::io::Error::from_raw_os_error(error));
            }
            break;
        }
        // SAFETY: readdir returned a live entry for the stream.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        let bytes = name.to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        names.push(name.to_owned());
        if names.len() as u64 > remaining {
            break;
        }
    }
    // SAFETY: `stream` is live and not used again; it closes `duplicate`.
    unsafe { libc::closedir(stream) };
    Ok(names)
}

fn clear_errno() {
    #[cfg(target_os = "macos")]
    // SAFETY: `__error` returns this thread's errno pointer.
    unsafe {
        *libc::__error() = 0;
    }
    #[cfg(not(target_os = "macos"))]
    // SAFETY: `__errno_location` returns this thread's errno pointer.
    unsafe {
        *libc::__errno_location() = 0;
    }
}

fn current_errno() -> i32 {
    #[cfg(target_os = "macos")]
    // SAFETY: `__error` returns this thread's errno pointer.
    unsafe {
        *libc::__error()
    }
    #[cfg(not(target_os = "macos"))]
    // SAFETY: `__errno_location` returns this thread's errno pointer.
    unsafe {
        *libc::__errno_location()
    }
}
