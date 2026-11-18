//! Small no-follow directory reads and diagnostic names used by pool validation.
//! Storage ownership and deletion belong to Barge.

use std::ffi::{CStr, OsStr};
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;

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
/// 64 input bytes followed by `...` when longer.
pub(crate) fn entry_label(name: &OsStr) -> String {
    use std::fmt::Write as _;
    let bytes = name.as_bytes();
    let mut label = String::new();
    for &byte in bytes.iter().take(64) {
        if byte == b' ' || (byte.is_ascii_graphic() && byte != b'"' && byte != b'\\') {
            label.push(char::from(byte));
        } else {
            let _ = write!(label, "\\x{byte:02x}");
        }
    }
    if bytes.len() > 64 {
        label.push_str("...");
    }
    label
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
