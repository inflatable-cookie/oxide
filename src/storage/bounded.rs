//! Verified hard-bound payload storage (oxide#055).
//!
//! The directory-only daemon checks admission against a caller-supplied budget
//! but nothing constrains a writer after acquire. This module defines the one
//! supported opt-in backend that closes that gap on macOS: a fixed-size private
//! APFS volume inside a sparse disk image. The volume's finite size is the
//! enforced aggregate-store ceiling; the operating system refuses allocation
//! beyond it while a cooperative writer holds an open lease. Admission still
//! reserves against the recorded ceiling so a partition cannot silently consume
//! the remaining pool.
//!
//! The enforced bound is aggregate-store, never per-slot: every owned slot on
//! the backing shares one finite payload filesystem and there is no per-slot
//! isolation. Control state (ledger, socket and receipt) intentionally stays
//! outside the payload filesystem so a full payload can never wedge durable
//! ownership updates; the payload holds only slot directories.
//!
//! The verified capability is constructed only by code that checked the live
//! backing (the protected runtime on macOS, or synthetic fixtures inside this
//! crate). Caller JSON cannot upgrade a directory root to a hard-bound claim.

use super::codec::{Decode, Encode, Reader, Writer};
use super::types::StorageError;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The only supported bounded backend: one fixed-size private APFS volume in a
/// sparse disk image, provisioned and attached by the protected runtime.
pub const BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1: &str = "apfs-sparse-image-v1";

const MAX_IDENTITY_BYTES: usize = 128;

/// Durable identity of the owned bounded backing, recorded in the ledger and
/// bound to daemon startup. Restart refuses any mismatch instead of
/// reinterpreting the recorded store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedBackendDescriptor {
    /// Exactly `BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1`.
    pub backend: String,
    /// Canonical mount point of the owned payload volume.
    pub payload_root: PathBuf,
    /// Backend-verified volume identity recorded at provisioning.
    pub identity: String,
    /// Payload volume capacity verified at provisioning, in bytes. This is the
    /// recorded descriptor value used for identity binding; it is not a live
    /// statfs sample and not the admission ceiling.
    pub verified_capacity_bytes: u64,
}

impl BoundedBackendDescriptor {
    pub(crate) fn encode_into(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.backend.encode(writer)?;
        self.payload_root.encode(writer)?;
        self.identity.encode(writer)?;
        writer.u64(self.verified_capacity_bytes);
        Ok(())
    }

    pub(crate) fn decode_from(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        Ok(Self {
            backend: String::decode(reader)?,
            payload_root: PathBuf::decode(reader)?,
            identity: String::decode(reader)?,
            verified_capacity_bytes: reader.u64()?,
        })
    }
}

impl Encode for BoundedBackendDescriptor {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.encode_into(writer)
    }
}

impl Decode for BoundedBackendDescriptor {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        Self::decode_from(reader)
    }
}

/// Verified capability for the owned hard-bounded payload filesystem.
///
/// Construction is crate-private: only code that provisioned or opened the
/// backing and verified it against its capacity proof can mint one. The daemon
/// treats the capability as the authority to place slots on the payload
/// filesystem and binds its descriptor into the durable ledger.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedBacking {
    descriptor: BoundedBackendDescriptor,
}

impl BoundedBacking {
    /// Verifies the structural contract of a checked descriptor against the
    /// live payload root and seals it into a capability.
    ///
    /// The caller must already have verified the backend-specific live state
    /// (mounted image, capacity proof and volume identity). This constructor
    /// refuses descriptors that are structurally impossible: unknown backend,
    /// non-canonical or non-private payload root, or unbounded metadata.
    pub(crate) fn verified(descriptor: BoundedBackendDescriptor) -> Result<Self, StorageError> {
        if descriptor.backend != BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1 {
            return Err(StorageError::InvalidInput("unsupported bounded backend"));
        }
        if descriptor.verified_capacity_bytes == 0 {
            return Err(StorageError::InvalidInput(
                "bounded payload must have a positive verified capacity",
            ));
        }
        if descriptor.identity.is_empty()
            || descriptor.identity.len() > MAX_IDENTITY_BYTES
            || !descriptor
                .identity
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(StorageError::InvalidInput(
                "bounded backend identity must be bounded lowercase hex",
            ));
        }
        verify_payload_root(&descriptor.payload_root)?;
        Ok(Self { descriptor })
    }

    /// Recorded durable identity of the backing.
    pub fn descriptor(&self) -> &BoundedBackendDescriptor {
        &self.descriptor
    }

    /// Canonical mount point of the owned payload volume.
    pub fn payload_root(&self) -> &Path {
        &self.descriptor.payload_root
    }
}

/// Refuses anything but an existing, canonical, private directory as the
/// payload root. A symlink, alias or shared directory never binds.
pub(crate) fn verify_payload_root(path: &Path) -> Result<(), StorageError> {
    if !path.is_absolute() {
        return Err(StorageError::InvalidInput(
            "bounded payload root must be absolute",
        ));
    }
    let metadata = fs::symlink_metadata(path).map_err(super::store::io_error)?;
    if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(StorageError::InvalidInput(
            "bounded payload root must be a private directory",
        ));
    }
    let canonical = fs::canonicalize(path).map_err(super::store::io_error)?;
    if canonical != path {
        return Err(StorageError::InvalidInput(
            "bounded payload root must be supplied by its canonical path",
        ));
    }
    Ok(())
}

/// The payload filesystem may hold only the owned slot directory. Unknown
/// entries refuse so a foreign mount can never silently join the store.
pub(crate) fn validate_payload_entries(payload_root: &Path) -> Result<(), StorageError> {
    for entry in fs::read_dir(payload_root).map_err(super::store::io_error)? {
        let entry = entry.map_err(super::store::io_error)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == super::store::SLOTS_NAME {
            continue;
        }
        return Err(StorageError::Refused(
            super::types::RefusalReason::UnknownAccounting,
        ));
    }
    Ok(())
}
