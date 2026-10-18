//! Provision and open the verified APFS sparse-image payload backend.
//!
//! The verified macOS backend (oxide#055): one fresh sparse image, attach at an
//! owner-only mount point, record a capacity receipt, and mint `BoundedBacking`
//! only after live verification. Caller JSON cannot upgrade a directory to a
//! hard-bound claim.

#[cfg(target_os = "macos")]
use super::ObservationUnknownReason;
use super::{
    io_error, read_private_file, ByteObservation, LeaseError, IMAGE_NAME, PAYLOAD_NAME,
    RECEIPT_NAME,
};
use crate::storage::bounded::verify_payload_root;
use crate::{BoundedBackendDescriptor, BoundedBacking, BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use std::fs::OpenOptions;
#[cfg(target_os = "macos")]
use std::io::Write;
#[cfg(target_os = "macos")]
use std::mem::MaybeUninit;
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
#[cfg(target_os = "macos")]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
#[cfg(target_os = "macos")]
use std::os::unix::io::AsRawFd;
#[cfg(target_os = "macos")]
use std::process::Command;

const RECEIPT_VERSION: u32 = 2;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BoundedReceipt {
    version: u32,
    backend: String,
    image_path: PathBuf,
    mount_point: PathBuf,
    image_bytes: u64,
    payload_ceiling_bytes: u64,
    verified_capacity_bytes: u64,
    identity: String,
}

pub(super) struct BackingPaths {
    pub image: PathBuf,
    pub receipt: PathBuf,
    pub mount: PathBuf,
}

pub(super) struct CapacitySample {
    pub payload_free_bytes: ByteObservation,
    pub payload_available_bytes: ByteObservation,
    pub image_logical_bytes: ByteObservation,
    pub image_allocated_bytes: ByteObservation,
    pub host_available_bytes: ByteObservation,
}

impl BackingPaths {
    pub(super) fn under(pool: &Path) -> Self {
        Self {
            image: pool.join(IMAGE_NAME),
            receipt: pool.join(RECEIPT_NAME),
            mount: pool.join(PAYLOAD_NAME),
        }
    }
}

#[cfg(target_os = "macos")]
pub(super) fn image_identity(image_path: &Path) -> Result<String, LeaseError> {
    let metadata = fs::symlink_metadata(image_path).map_err(io_error)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(LeaseError::BackingMismatch);
    }
    Ok(format!("{:016x}{:016x}", metadata.dev(), metadata.ino()))
}

#[cfg(target_os = "macos")]
pub(super) fn provision(
    paths: &BackingPaths,
    cap_bytes: u64,
    image_bytes: u64,
) -> Result<BoundedBacking, LeaseError> {
    if cap_bytes == 0 || image_bytes != cap_bytes {
        return Err(LeaseError::InvalidInput(
            "cap-bytes must equal image-bytes so the volume size is the enforced ceiling",
        ));
    }
    if paths.image.symlink_metadata().is_ok() || paths.receipt.symlink_metadata().is_ok() {
        return Err(LeaseError::BackingMismatch);
    }
    super::ensure_private_dir(&paths.mount)?;
    let scratch = paths
        .image
        .parent()
        .ok_or(LeaseError::InvalidInput("image has no parent"))?;
    let size = image_bytes.to_string();
    let image = path_text(&paths.image)?;
    let mount = path_text(&paths.mount)?;
    let created = hdiutil(
        scratch,
        &[
            "create",
            "-size",
            &size,
            "-type",
            "SPARSE",
            "-fs",
            "APFS",
            "-volname",
            "oxide-lease-v1",
            "-nospotlight",
            "-quiet",
            &image,
        ],
    )?;
    if !created.status.success() {
        return Err(LeaseError::Io("hdiutil create failed".into()));
    }
    let attached = hdiutil(
        scratch,
        &[
            "attach",
            "-nobrowse",
            "-noverify",
            "-mountpoint",
            &mount,
            &image,
        ],
    )?;
    if !attached.status.success() {
        return Err(LeaseError::Io("hdiutil attach failed".into()));
    }
    fs::set_permissions(&paths.mount, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    let verified_capacity_bytes = sample_volume(&paths.mount)?;
    // A volume larger than the declared ceiling would leave that ceiling
    // unenforced: an admitted writer could exceed it before ENOSPC. Refuse
    // that shape. A shortfall below the declared size is APFS metadata
    // overhead; the measured value below is the effective enforced bound and
    // writers cannot exceed the declared ceiling through it.
    if verified_capacity_bytes > cap_bytes {
        return Err(LeaseError::BackingMismatch);
    }
    let identity = image_identity(&paths.image)?;
    let receipt = BoundedReceipt {
        version: RECEIPT_VERSION,
        backend: BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1.to_owned(),
        image_path: paths.image.clone(),
        mount_point: paths.mount.clone(),
        image_bytes,
        payload_ceiling_bytes: cap_bytes,
        verified_capacity_bytes,
        identity: identity.clone(),
    };
    let bytes = serde_json::to_vec(&receipt).map_err(|error| LeaseError::Io(error.to_string()))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&paths.receipt)
        .map_err(io_error)?;
    file.write_all(&bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    drop(file);
    File::open(scratch)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)?;
    mint_backing(paths, identity, verified_capacity_bytes)
}

#[cfg(not(target_os = "macos"))]
pub(super) fn provision(
    _paths: &BackingPaths,
    _cap_bytes: u64,
    _image_bytes: u64,
) -> Result<BoundedBacking, LeaseError> {
    Err(LeaseError::Unsupported(
        "bounded APFS payload backing is macOS-only",
    ))
}

#[cfg(target_os = "macos")]
pub(super) fn open(
    paths: &BackingPaths,
    cap_bytes: u64,
    image_bytes: u64,
) -> Result<BoundedBacking, LeaseError> {
    let (receipt, identity) = verify_receipt(paths, cap_bytes, image_bytes)?;
    // Pools whose declared cap differs from the image size predate the
    // enforced-ceiling rule and are refused without migration.
    ensure_attached(paths)?;
    let verified_capacity_bytes = sample_volume(&paths.mount)?;
    if identity != receipt.identity
        || verified_capacity_bytes != receipt.verified_capacity_bytes
        || verified_capacity_bytes > cap_bytes
    {
        return Err(LeaseError::BackingMismatch);
    }
    mint_backing(paths, identity, verified_capacity_bytes)
}

/// Verify and sample the already-mounted pinned backing without attaching it.
#[cfg(target_os = "macos")]
pub(super) fn open_observational(
    paths: &BackingPaths,
    cap_bytes: u64,
    image_bytes: u64,
) -> Result<(BoundedBacking, CapacitySample), LeaseError> {
    let (receipt, identity) = verify_receipt(paths, cap_bytes, image_bytes)?;
    let (verified_capacity_bytes, payload_free_bytes, payload_available_bytes) =
        sample_volume_observations(&paths.mount)?;
    if identity != receipt.identity
        || verified_capacity_bytes != receipt.verified_capacity_bytes
        || verified_capacity_bytes > cap_bytes
    {
        return Err(LeaseError::BackingMismatch);
    }
    let image_metadata = image_metadata(&paths.image)?;
    let sampled_identity = format!("{:016x}{:016x}", image_metadata.dev(), image_metadata.ino());
    if sampled_identity != identity {
        return Err(LeaseError::BackingMismatch);
    }
    let image_logical_bytes = ByteObservation::observed(image_metadata.len());
    let image_allocated_bytes = blocks_observation(image_metadata.blocks(), 512);
    let host_available_bytes = paths
        .image
        .parent()
        .map(host_available_observation)
        .unwrap_or_else(|| ByteObservation::unknown(ObservationUnknownReason::Unsupported));
    let backing = mint_backing(paths, identity, verified_capacity_bytes)?;
    Ok((
        backing,
        CapacitySample {
            payload_free_bytes,
            payload_available_bytes,
            image_logical_bytes,
            image_allocated_bytes,
            host_available_bytes,
        },
    ))
}

#[cfg(not(target_os = "macos"))]
pub(super) fn open(
    _paths: &BackingPaths,
    _cap_bytes: u64,
    _image_bytes: u64,
) -> Result<BoundedBacking, LeaseError> {
    Err(LeaseError::Unsupported(
        "bounded APFS payload backing is macOS-only",
    ))
}

#[cfg(not(target_os = "macos"))]
pub(super) fn open_observational(
    _paths: &BackingPaths,
    _cap_bytes: u64,
    _image_bytes: u64,
) -> Result<(BoundedBacking, CapacitySample), LeaseError> {
    Err(LeaseError::Unsupported(
        "bounded APFS payload backing is macOS-only",
    ))
}

#[cfg(target_os = "macos")]
fn verify_receipt(
    paths: &BackingPaths,
    cap_bytes: u64,
    image_bytes: u64,
) -> Result<(BoundedReceipt, String), LeaseError> {
    if paths.image.symlink_metadata().is_err() || paths.receipt.symlink_metadata().is_err() {
        return Err(LeaseError::BackingMismatch);
    }
    if image_bytes != cap_bytes {
        return Err(LeaseError::BackingMismatch);
    }
    let bytes = read_private_file(&paths.receipt)?;
    let receipt: BoundedReceipt =
        serde_json::from_slice(&bytes).map_err(|_| LeaseError::BackingMismatch)?;
    if receipt.version != RECEIPT_VERSION
        || receipt.backend != BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1
        || receipt.image_path != paths.image
        || receipt.mount_point != paths.mount
        || receipt.image_bytes != image_bytes
        || receipt.payload_ceiling_bytes != cap_bytes
    {
        return Err(LeaseError::BackingMismatch);
    }
    let identity = image_identity(&paths.image)?;
    Ok((receipt, identity))
}

fn mint_backing(
    paths: &BackingPaths,
    identity: String,
    verified_capacity_bytes: u64,
) -> Result<BoundedBacking, LeaseError> {
    verify_payload_root(&paths.mount).map_err(|_| LeaseError::BackingMismatch)?;
    BoundedBacking::verified(BoundedBackendDescriptor {
        backend: BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1.to_owned(),
        payload_root: paths.mount.clone(),
        identity,
        verified_capacity_bytes,
    })
    .map_err(|_| LeaseError::BackingMismatch)
}

/// Test helper: mint a verified capability over an owner-only directory.
/// This is structural verification only. It is not an OS hard bound and the
/// product CLI never uses it.
#[cfg(test)]
pub(super) fn inject_directory(mount: &Path, cap_bytes: u64) -> Result<BoundedBacking, LeaseError> {
    super::ensure_private_dir(mount)?;
    let metadata = fs::symlink_metadata(mount).map_err(io_error)?;
    let identity = format!("{:016x}{:016x}", metadata.dev(), metadata.ino());
    BoundedBacking::verified(BoundedBackendDescriptor {
        backend: BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1.to_owned(),
        payload_root: mount.to_path_buf(),
        identity,
        verified_capacity_bytes: cap_bytes.max(1),
    })
    .map_err(|_| LeaseError::BackingMismatch)
}

#[cfg(target_os = "macos")]
fn ensure_attached(paths: &BackingPaths) -> Result<(), LeaseError> {
    if sample_volume(&paths.mount).is_ok() {
        return Ok(());
    }
    let scratch = paths
        .image
        .parent()
        .ok_or(LeaseError::InvalidInput("image has no parent"))?;
    super::ensure_private_dir(&paths.mount)?;
    let image = path_text(&paths.image)?;
    let mount = path_text(&paths.mount)?;
    let attached = hdiutil(
        scratch,
        &[
            "attach",
            "-nobrowse",
            "-noverify",
            "-mountpoint",
            &mount,
            &image,
        ],
    )?;
    if !attached.status.success() {
        return Err(LeaseError::Io("hdiutil attach failed".into()));
    }
    fs::set_permissions(&paths.mount, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn hdiutil(scratch: &Path, arguments: &[&str]) -> Result<std::process::Output, LeaseError> {
    Command::new("/usr/bin/hdiutil")
        .args(arguments)
        .env("TMPDIR", scratch)
        .env("TMP", scratch)
        .env("TEMP", scratch)
        .output()
        .map_err(io_error)
}

#[cfg(target_os = "macos")]
fn sample_volume(mount_point: &Path) -> Result<u64, LeaseError> {
    Ok(sample_volume_observations(mount_point)?.0)
}

#[cfg(target_os = "macos")]
fn sample_volume_observations(
    mount_point: &Path,
) -> Result<(u64, ByteObservation, ByteObservation), LeaseError> {
    let stat = sample_statfs(mount_point).map_err(|_| LeaseError::BackingMismatch)?;
    if c_field_bytes(&stat.f_fstypename) != b"apfs"
        || c_field_bytes(&stat.f_mntonname) != mount_point.as_os_str().as_bytes()
    {
        return Err(LeaseError::BackingMismatch);
    }
    let block_size = u64::try_from(stat.f_bsize).map_err(|_| LeaseError::BackingMismatch)?;
    let blocks = u64::try_from(stat.f_blocks).map_err(|_| LeaseError::BackingMismatch)?;
    let capacity_bytes = blocks
        .checked_mul(block_size)
        .filter(|bytes| *bytes > 0)
        .ok_or(LeaseError::BackingMismatch)?;
    Ok((
        capacity_bytes,
        blocks_observation(stat.f_bfree, block_size),
        blocks_observation(stat.f_bavail, block_size),
    ))
}

#[cfg(target_os = "macos")]
fn host_available_observation(path: &Path) -> ByteObservation {
    let Ok(stat) = sample_statfs(path) else {
        return ByteObservation::unknown(ObservationUnknownReason::SampleFailed);
    };
    let block_size = u64::from(stat.f_bsize);
    if block_size == 0 {
        return ByteObservation::unknown(ObservationUnknownReason::Unsupported);
    }
    blocks_observation(stat.f_bavail, block_size)
}

#[cfg(target_os = "macos")]
fn sample_statfs(path: &Path) -> Result<libc::statfs, std::io::Error> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)?;
    let mut stat = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: the open directory descriptor is valid; zero initializes statfs.
    let status = unsafe { libc::fstatfs(directory.as_raw_fd(), stat.as_mut_ptr()) };
    if status != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: zero return means the record is initialized.
    Ok(unsafe { stat.assume_init() })
}

#[cfg(target_os = "macos")]
fn blocks_observation<T>(blocks: T, block_size: u64) -> ByteObservation
where
    T: TryInto<u64>,
{
    let Ok(blocks) = blocks.try_into() else {
        return ByteObservation::unknown(ObservationUnknownReason::Unsupported);
    };
    match blocks.checked_mul(block_size) {
        Some(bytes) => ByteObservation::observed(bytes),
        None => ByteObservation::unknown(ObservationUnknownReason::Overflow),
    }
}

#[cfg(target_os = "macos")]
fn image_metadata(image_path: &Path) -> Result<fs::Metadata, LeaseError> {
    let metadata = fs::symlink_metadata(image_path).map_err(io_error)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(LeaseError::BackingMismatch);
    }
    Ok(metadata)
}

#[cfg(target_os = "macos")]
fn c_field_bytes(field: &[libc::c_char]) -> Vec<u8> {
    field
        .iter()
        .take_while(|&&value| value != 0)
        .map(|&value| value as u8)
        .collect()
}

fn path_text(path: &Path) -> Result<String, LeaseError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or(LeaseError::InvalidInput("path is not UTF-8"))
}
