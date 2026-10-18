//! Verified hard-bound payload storage (oxide#055).
//!
//! The one supported opt-in backend on macOS is a fixed-size private APFS
//! volume inside a sparse disk image. The volume's finite size is the enforced
//! aggregate ceiling; the operating system refuses allocation beyond it while a
//! cooperative writer holds an open lease. The bound is aggregate, never
//! per-slot: every slot on the backing shares one finite payload filesystem.
//! Control state stays outside the payload filesystem so a full payload cannot
//! wedge durable ownership updates.
//!
//! The verified capability is constructed only by code that checked the live
//! backing (the lease backend on macOS, or synthetic fixtures inside this
//! crate). Caller JSON cannot upgrade a directory root to a hard-bound claim.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The only supported bounded backend: one fixed-size private APFS volume in a
/// sparse disk image, provisioned and attached by the lease backend.
pub const BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1: &str = "apfs-sparse-image-v1";

const MAX_IDENTITY_BYTES: usize = 128;

/// Durable identity of the owned bounded backing. Reopen refuses any mismatch
/// instead of reinterpreting the recorded store.
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

/// Structural refusal while sealing a bounded capability. Callers map it to
/// their own fail-closed error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoundedRefusal {
    UnsupportedBackend,
    ZeroCapacity,
    InvalidIdentity,
    RelativePayloadRoot,
    UnreadablePayloadRoot,
    NonPrivatePayloadRoot,
    NonCanonicalPayloadRoot,
}

/// Verified capability for the owned hard-bounded payload filesystem.
///
/// Construction is crate-private: only code that provisioned or opened the
/// backing and verified it against its capacity proof can mint one. The lease
/// backend treats the capability as the authority to place slots on the payload
/// filesystem and binds its descriptor into its durable pool state.
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
    pub(crate) fn verified(descriptor: BoundedBackendDescriptor) -> Result<Self, BoundedRefusal> {
        if descriptor.backend != BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1 {
            return Err(BoundedRefusal::UnsupportedBackend);
        }
        if descriptor.verified_capacity_bytes == 0 {
            return Err(BoundedRefusal::ZeroCapacity);
        }
        if descriptor.identity.is_empty()
            || descriptor.identity.len() > MAX_IDENTITY_BYTES
            || !descriptor
                .identity
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(BoundedRefusal::InvalidIdentity);
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
pub(crate) fn verify_payload_root(path: &Path) -> Result<(), BoundedRefusal> {
    if !path.is_absolute() {
        return Err(BoundedRefusal::RelativePayloadRoot);
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| BoundedRefusal::UnreadablePayloadRoot)?;
    if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(BoundedRefusal::NonPrivatePayloadRoot);
    }
    let canonical = fs::canonicalize(path).map_err(|_| BoundedRefusal::UnreadablePayloadRoot)?;
    if canonical != path {
        return Err(BoundedRefusal::NonCanonicalPayloadRoot);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(1);

    struct Fixture {
        path: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let parent = fs::canonicalize(base)
                .expect("canonical scratch")
                .join("bounded");
            fs::create_dir_all(&parent).expect("bounded fixture parent");
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("{:x}-{id:x}", std::process::id()));
            fs::create_dir(&path).expect("bounded fixture");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("private");
            Self { path }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn descriptor(payload_root: PathBuf) -> BoundedBackendDescriptor {
        BoundedBackendDescriptor {
            backend: BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1.to_owned(),
            payload_root,
            identity: "a".repeat(16),
            verified_capacity_bytes: 1024 * 1024,
        }
    }

    fn refusal(descriptor: BoundedBackendDescriptor) -> BoundedRefusal {
        BoundedBacking::verified(descriptor).expect_err("structural refusal")
    }

    #[test]
    fn verified_backing_refuses_structurally_impossible_descriptors() {
        let fixture = Fixture::new();
        let payload = fixture.path.join("payload");
        fs::create_dir(&payload).expect("payload");
        fs::set_permissions(&payload, fs::Permissions::from_mode(0o700)).expect("private");
        let backing = BoundedBacking::verified(descriptor(payload.clone())).expect("verified");
        assert_eq!(backing.payload_root(), payload);

        let mut unknown = descriptor(payload.clone());
        unknown.backend = "directory".to_owned();
        assert_eq!(refusal(unknown), BoundedRefusal::UnsupportedBackend);

        let mut empty = descriptor(payload.clone());
        empty.verified_capacity_bytes = 0;
        assert_eq!(refusal(empty), BoundedRefusal::ZeroCapacity);

        for identity in [
            String::new(),
            "A".repeat(16),
            "a".repeat(MAX_IDENTITY_BYTES + 1),
        ] {
            let mut bad = descriptor(payload.clone());
            bad.identity = identity;
            assert_eq!(refusal(bad), BoundedRefusal::InvalidIdentity);
        }

        assert_eq!(
            refusal(descriptor(PathBuf::from("payload"))),
            BoundedRefusal::RelativePayloadRoot
        );
        assert_eq!(
            refusal(descriptor(fixture.path.join("missing"))),
            BoundedRefusal::UnreadablePayloadRoot
        );

        let alias = fixture.path.join("payload-alias");
        symlink(&payload, &alias).expect("symlink fixture");
        assert_eq!(
            refusal(descriptor(alias)),
            BoundedRefusal::NonPrivatePayloadRoot
        );

        assert_eq!(
            refusal(descriptor(payload.join("..").join("payload"))),
            BoundedRefusal::NonCanonicalPayloadRoot
        );

        let shared = fixture.path.join("shared");
        fs::create_dir(&shared).expect("shared");
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).expect("shared mode");
        assert_eq!(
            refusal(descriptor(shared)),
            BoundedRefusal::NonPrivatePayloadRoot
        );
    }
}
