//! Qualify the canonical namespace used for binding and pathname cleanup.

use std::ffi::{c_int, c_void, CString};
use std::fs;
use std::io;
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Path, PathBuf};
use std::ptr::{self, NonNull};

/// Establish trust from `/` down so an unchecked parent cannot replace an
/// already-checked entry. In a sticky directory, each next entry must itself
/// belong to a trusted owner to be protected against rename or removal.
pub(super) fn verify(root: &Path) -> io::Result<()> {
    // SAFETY: geteuid takes no arguments and has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let mut prefix = PathBuf::new();
    let mut device = None;
    for component in root.components() {
        prefix.push(component);
        let metadata = fs::symlink_metadata(&prefix)?;
        if !metadata.is_dir()
            || (metadata.uid() != uid && metadata.uid() != 0)
            || (metadata.mode() & 0o022 != 0 && metadata.mode() & u32::from(libc::S_ISVTX) == 0)
        {
            return Err(refuse(&prefix, "expected a directory owned by this user or root, with sticky mode if writable by others"));
        }
        if device != Some(metadata.dev()) {
            let mount = crate::store::statfs(&prefix).ok_or_else(|| {
                refuse(&prefix, "cannot verify that the volume enforces ownership")
            })?;
            if mount.f_flags & libc::MNT_IGNORE_OWNERSHIP as u32 != 0 {
                return Err(refuse(&prefix, "the volume ignores ownership"));
            }
            device = Some(metadata.dev());
        }
        if !deny_only_acl(&prefix)? {
            return Err(refuse(&prefix, "the ACL grants access"));
        }
    }
    Ok(())
}

fn refuse(path: &Path, reason: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "untrusted session broker directory {} ({reason})",
            path.display()
        ),
    )
}

// The macOS SDK's <sys/acl.h> defines these interfaces and values.
const ACL_TYPE_EXTENDED: c_int = 0x100;
const ACL_FIRST_ENTRY: c_int = 0;
const ACL_NEXT_ENTRY: c_int = -1;
const ACL_EXTENDED_DENY: c_int = 2;

unsafe extern "C" {
    fn acl_get_link_np(path: *const libc::c_char, kind: c_int) -> *mut c_void;
    fn acl_get_entry(acl: *mut c_void, index: c_int, entry: *mut *mut c_void) -> c_int;
    fn acl_get_tag_type(entry: *mut c_void, tag: *mut c_int) -> c_int;
    fn acl_free(acl: *mut c_void) -> c_int;
}

struct Acl(NonNull<c_void>);

impl Drop for Acl {
    fn drop(&mut self) {
        // SAFETY: this guard owns the ACL allocated by acl_get_link_np.
        unsafe { acl_free(self.0.as_ptr()) };
    }
}

fn deny_only_acl(path: &Path) -> io::Result<bool> {
    let name = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: name is a live NUL-terminated pathname and the ACL type is valid.
    let acl = unsafe { acl_get_link_np(name.as_ptr(), ACL_TYPE_EXTENDED) };
    let Some(acl) = NonNull::new(acl).map(Acl) else {
        let error = io::Error::last_os_error();
        // Darwin also returns ENOENT when an existing entry has no ACL.
        return match error.raw_os_error() {
            Some(libc::ENOENT | libc::ENOATTR) => Ok(true),
            _ => Err(error),
        };
    };
    let mut index = ACL_FIRST_ENTRY;
    loop {
        let mut entry = ptr::null_mut();
        // SAFETY: acl is live and entry is writable for the borrowed pointer.
        if unsafe { acl_get_entry(acl.0.as_ptr(), index, &mut entry) } != 0 {
            let error = io::Error::last_os_error();
            // The SDK documents EINVAL at the end of a valid ACL.
            return match error.raw_os_error() {
                Some(libc::EINVAL) => Ok(true),
                _ => Err(error),
            };
        }
        let mut tag = 0;
        // SAFETY: the successful call supplied an entry valid while acl lives.
        if unsafe { acl_get_tag_type(entry, &mut tag) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if tag != ACL_EXTENDED_DENY {
            return Ok(false);
        }
        index = ACL_NEXT_ENTRY;
    }
}
