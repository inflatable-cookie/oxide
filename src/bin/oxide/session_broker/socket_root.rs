//! Qualify the canonical namespace used for binding and pathname cleanup.

#[cfg(any(target_os = "linux", test))]
mod posix_acl;

#[cfg(target_os = "linux")]
use std::ffi::CStr;
use std::ffi::CString;
#[cfg(target_os = "macos")]
use std::ffi::{c_int, c_void};
use std::fs;
use std::io;
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
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
            if !ownership_enforced(&mount) {
                return Err(refuse(
                    &prefix,
                    "the volume does not establish ownership enforcement",
                ));
            }
            device = Some(metadata.dev());
        }
        if !acl_is_clean(&prefix)? {
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

/// Darwin reports a volume that ignores ownership as a mount flag. A volume
/// without that flag is accepted, as it was before Linux parity.
#[cfg(target_os = "macos")]
fn ownership_enforced(mount: &libc::statfs) -> bool {
    (mount.f_flags & libc::MNT_IGNORE_OWNERSHIP as u32) == 0
}

/// Linux has no mount flag that says ownership is enforced, so the check is
/// positive. The filesystem type must be a local one known to enforce POSIX
/// ownership and modes on its own inodes. Every other type refuses, including
/// network, FUSE, FAT and exFAT volumes.
#[cfg(target_os = "linux")]
fn ownership_enforced(mount: &libc::statfs) -> bool {
    linux_enforces_ownership(mount.f_type as u64)
}

/// statfs(2) `f_type` magic numbers for local filesystems that enforce
/// ownership. Idmapped mounts are not inspected, so they are not established.
#[cfg(any(target_os = "linux", test))]
const OWNERSHIP_ENFORCING: [u64; 5] = [
    0xEF53,      // ext2, ext3 and ext4
    0x5846_5342, // XFS
    0x9123_683E, // Btrfs
    0x0102_1994, // tmpfs
    0x794C_7630, // overlayfs, which takes ownership from its local layers
];

#[cfg(any(target_os = "linux", test))]
fn linux_enforces_ownership(filesystem: u64) -> bool {
    OWNERSHIP_ENFORCING.contains(&filesystem)
}

#[cfg(test)]
mod tests {
    use super::linux_enforces_ownership;

    #[test]
    fn only_listed_local_filesystems_establish_ownership() {
        // ext4, XFS, Btrfs, tmpfs and overlayfs.
        for magic in [0xEF53, 0x5846_5342, 0x9123_683E, 0x0102_1994, 0x794C_7630] {
            assert!(linux_enforces_ownership(magic), "{magic:#x} should enforce");
        }
        // NFS, FUSE, CIFS, SMB2, FAT, exFAT, and an unknown or zero type.
        for magic in [
            0x6969,
            0x6573_5546,
            0xFF53_4D42,
            0xFE53_4D42,
            0x4D44,
            0x2011_BAB0,
            0,
        ] {
            assert!(!linux_enforces_ownership(magic), "{magic:#x} should refuse");
        }
    }
}

// The macOS SDK's <sys/acl.h> defines these interfaces and values.
#[cfg(target_os = "macos")]
const ACL_TYPE_EXTENDED: c_int = 0x100;
#[cfg(target_os = "macos")]
const ACL_FIRST_ENTRY: c_int = 0;
#[cfg(target_os = "macos")]
const ACL_NEXT_ENTRY: c_int = -1;
#[cfg(target_os = "macos")]
const ACL_EXTENDED_DENY: c_int = 2;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn acl_get_link_np(path: *const libc::c_char, kind: c_int) -> *mut c_void;
    fn acl_get_entry(acl: *mut c_void, index: c_int, entry: *mut *mut c_void) -> c_int;
    fn acl_get_tag_type(entry: *mut c_void, tag: *mut c_int) -> c_int;
    fn acl_free(acl: *mut c_void) -> c_int;
}

#[cfg(target_os = "macos")]
struct Acl(NonNull<c_void>);

#[cfg(target_os = "macos")]
impl Drop for Acl {
    fn drop(&mut self) {
        // SAFETY: this guard owns the ACL allocated by acl_get_link_np.
        unsafe { acl_free(self.0.as_ptr()) };
    }
}

/// True when the path has no ACL that grants access. A deny-only ACL is clean.
#[cfg(target_os = "macos")]
fn acl_is_clean(path: &Path) -> io::Result<bool> {
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

/// Linux keeps the access ACL in this attribute. A directory with only mode
/// bits has none. The access ACL governs the directory itself. A default ACL
/// only shapes children created later, and the leaf check covers those.
#[cfg(target_os = "linux")]
const POSIX_ACL_ACCESS: &CStr = c"system.posix_acl_access";

/// Users whose entries are trusted: the root user and the effective user.
#[cfg(target_os = "linux")]
fn trusted_principals() -> [u32; 2] {
    // SAFETY: geteuid takes no arguments and has no preconditions.
    [0, unsafe { libc::geteuid() }]
}

/// The access ACL bytes for `path`, or `None` when it has only mode bits.
#[cfg(target_os = "linux")]
fn access_acl(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let name = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: the name is NUL-terminated. A zero-size probe writes nothing.
    let size = unsafe {
        libc::lgetxattr(
            name.as_ptr(),
            POSIX_ACL_ACCESS.as_ptr(),
            std::ptr::null_mut(),
            0,
        )
    };
    if size < 0 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ENODATA) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    let capacity =
        usize::try_from(size).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    let mut value = vec![0u8; capacity];
    // SAFETY: `value` is writable for its length. An ACL that grew since the
    // probe fails with ERANGE and refuses, rather than being read partially.
    let read = unsafe {
        libc::lgetxattr(
            name.as_ptr(),
            POSIX_ACL_ACCESS.as_ptr(),
            value.as_mut_ptr().cast(),
            value.len(),
        )
    };
    if read < 0 {
        return Err(io::Error::last_os_error());
    }
    value.truncate(usize::try_from(read).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?);
    Ok(Some(value))
}

/// Effective permissions the access ACL grants to principals other than the
/// owner and the trusted users. A malformed ACL is an error, which refuses.
#[cfg(target_os = "linux")]
fn untrusted_access(path: &Path) -> io::Result<u16> {
    let Some(acl) = access_acl(path)? else {
        return Ok(0);
    };
    posix_acl::untrusted_permissions(&acl, &trusted_principals()).map_err(
        |posix_acl::Malformed(reason)| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("malformed POSIX ACL on {}: {reason}", path.display()),
            )
        },
    )
}

/// An ancestor may grant read and traverse to anyone, but never write.
#[cfg(target_os = "linux")]
fn acl_is_clean(path: &Path) -> io::Result<bool> {
    Ok(untrusted_access(path)? & posix_acl::WRITE == 0)
}

/// The private socket directory must be reachable by its owner and trusted
/// users only. Any other effective permission refuses the bind.
#[cfg(target_os = "linux")]
pub(super) fn verify_private_leaf(path: &Path) -> io::Result<()> {
    if untrusted_access(path)? != 0 {
        return Err(refuse(
            path,
            "the socket directory grants access to another principal",
        ));
    }
    Ok(())
}
