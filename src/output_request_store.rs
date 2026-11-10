//! File custody for the immutable original requests in oxide#304.
//!
//! This layer stores opaque bytes; the caller must validate the strict wire
//! schema and its identity/request binding. It cannot select a deleter, infer
//! absence, initialize a registry or invoke Barge. Missing or invalid bytes are
//! always errors. No command uses this store until the recovery-format upgrade
//! and its linked reader prerequisite are connected.

use crate::owned_delete::{names_bounded, open_directory_at, rename_exclusive};
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;

/// The fixed request-document byte limit.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// All retained names count: partial, orphan and terminal, not just live ones.
pub const MAX_REQUEST_FILES: usize = 256;
const DIRECTORY: &str = "prune-delegations";

/// A held private directory, opened relative to the caller's verified outputs
/// descriptor. This store never resolves a caller-supplied absolute filename.
#[derive(Debug)]
pub struct RequestStore {
    directory: File,
}

impl RequestStore {
    /// Open the request directory without following a link.
    ///
    /// The caller must retain its verified outputs ancestry/binding. A read
    /// (`create == false`) never creates anything. Creation syncs the child
    /// and its parent even when the child already existed: an earlier attempt
    /// may have failed between creation and its directory sync.
    pub fn open(outputs: BorrowedFd<'_>, create: bool) -> io::Result<Self> {
        let parent = File::from(outputs.try_clone_to_owned()?);
        private(&parent, true)?;
        let name = c_name(DIRECTORY)?;
        if create {
            // SAFETY: outputs is held for the call and name owns a terminated
            // single-component C string. mkdirat borrows both and keeps neither.
            if unsafe { libc::mkdirat(outputs.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
            }
        }
        let directory = File::from(open_directory_at(outputs, &name)?);
        private(&directory, true)?;
        if directory.metadata()?.dev() != parent.metadata()?.dev() {
            return Err(invalid(
                "request directory crosses the verified outputs device",
            ));
        }
        if create {
            directory.sync_all()?;
            parent.sync_all()?;
        }
        Ok(Self { directory })
    }

    /// Exclusively publish bytes and return the SHA-256 of their exact image.
    ///
    /// The caller must serialize writers with Oxide publication custody and
    /// keep it until the intent and route are durably published. File sync
    /// precedes exclusive rename, which precedes directory sync. An error
    /// never authorizes an adapter call; any partial or published file stays
    /// retained, and this id must not be reused. No existing name is replaced.
    pub fn publish(&self, id: &str, bytes: &[u8]) -> io::Result<String> {
        validate_id(id)?;
        if bytes.is_empty() || bytes.len() > MAX_REQUEST_BYTES {
            return Err(invalid("request bytes are empty or exceed 64 KiB"));
        }
        self.capacity()?;
        let partial = c_name(&format!(".{id}.partial"))?;
        let final_name = c_name(&format!("{id}.json"))?;
        let mut file = self.open_file(&partial, true)?;
        private(&file, false)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        private(&file, false)?;
        rename_exclusive(self.directory.as_fd(), &partial, &final_name)?;
        self.directory.sync_all()?;
        Ok(digest(bytes))
    }

    /// Read a complete request whose exact bytes match the journal's hash.
    ///
    /// Missing files, links, non-private files, oversized images and mismatches
    /// all refuse. This byte check does not validate a schema or grant.
    pub fn read(&self, id: &str, expected_sha256: &str) -> io::Result<Vec<u8>> {
        private(&self.directory, true)?;
        validate_id(id)?;
        if expected_sha256.len() != 64 || !expected_sha256.bytes().all(lower_hex) {
            return Err(invalid("the request byte digest is not SHA-256 hex"));
        }
        let file = self.open_file(&c_name(&format!("{id}.json"))?, false)?;
        private(&file, false)?;
        let mut bytes = Vec::new();
        (&file)
            .take(MAX_REQUEST_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        private(&file, false)?;
        if bytes.is_empty() || bytes.len() > MAX_REQUEST_BYTES || digest(&bytes) != expected_sha256
        {
            return Err(invalid("request bytes do not match their recorded image"));
        }
        Ok(bytes)
    }

    fn capacity(&self) -> io::Result<()> {
        private(&self.directory, true)?;
        let names = names_bounded(self.directory.as_fd(), MAX_REQUEST_FILES)?;
        if names.len() >= MAX_REQUEST_FILES {
            return Err(invalid("the retained request-file limit is reached"));
        }
        // Inspect every entry, partials included. A foreign, linked, oversized
        // or unreadable entry is not spare capacity and is never cleaned up.
        for name in names {
            let name = CString::new(name.as_bytes()).map_err(|_| invalid("NUL in filename"))?;
            let file = self.open_file(&name, false)?;
            private(&file, false)?;
        }
        Ok(())
    }

    fn open_file(&self, name: &CString, create: bool) -> io::Result<File> {
        let flags = if create {
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL
        } else {
            libc::O_RDONLY
        };
        // SAFETY: directory is held and name is a terminated component. openat
        // borrows them only for the call; a successful fd transfers ownership
        // below. NOFOLLOW rejects final links; NONBLOCK avoids a foreign FIFO
        // hanging before the regular-file validation.
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                0o600,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd is a fresh successful openat result, owned exactly once.
        let file = unsafe { File::from_raw_fd(fd) };
        if file.metadata()?.dev() != self.directory.metadata()?.dev() {
            return Err(invalid("request file crosses the held directory device"));
        }
        Ok(file)
    }
}

fn private(file: &File, directory: bool) -> io::Result<()> {
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no preconditions or borrowed arguments.
    let uid = unsafe { libc::geteuid() };
    if metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
                || metadata.nlink() != 1
                || metadata.len() > MAX_REQUEST_BYTES as u64
        }
    {
        return Err(invalid(
            "request storage is not private, regular and bounded",
        ));
    }
    Ok(())
}

fn validate_id(id: &str) -> io::Result<()> {
    if id.len() != 32 || !id.bytes().all(lower_hex) {
        return Err(invalid("the caller intent id is not 128-bit lowercase hex"));
    }
    Ok(())
}

fn lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

fn c_name(name: &str) -> io::Result<CString> {
    CString::new(name).map_err(|_| invalid("NUL in request filename"))
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn invalid(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

#[cfg(test)]
#[path = "output_request_store_tests.rs"]
mod tests;
