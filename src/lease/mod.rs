//! Same-user local lease CLI over durable exclusive slots and bounded payload
//! storage.
//!
//! This is the unpublished lean product surface: a short-lived OS lock plus an
//! atomic local ledger, no resident daemon, HMAC, issuer, callback service or
//! Cargo invocation. Same-UID callers are trusted cooperators; private paths
//! and safety checks still refuse foreign paths and symlinked roots. A
//! durably recorded idle deletion unlinks an owned nested symlink entry without
//! following its target. Owning contract: `docs/knowledge/contracts/local-lease.md`.

mod backing;
mod pool;

use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub use pool::{
    capacity_status, evict_idle_slot, lease, managed, recover, release, status, LeaseRequest,
};

#[cfg(test)]
mod tests;

/// Version of the lease JSON, receipts, pool config and ledger.
pub const LEASE_VERSION: u32 = 1;
/// Unpublished binary name. Publication naming remains Q-005.
pub const BINARY_NAME: &str = "oxide-storage-lease";
/// Default rendered status cap in bytes.
pub const DEFAULT_STATUS_OUTPUT_BYTES: usize = 64 * 1024;
/// Minimum rendered status cap.
pub const MIN_STATUS_OUTPUT_BYTES: usize = 64;
/// Version of the separate status capacity envelope.
pub const CAPACITY_STATUS_VERSION: u32 = 1;
pub const CAPACITY_STATUS_NOTES: [&str; 4] = [
    "status.full compares owned slot allocation with declared cap; it is not filesystem headroom",
    "payload free and available bytes are samples, not reservations, build-fit predictions, attributable writes, or guaranteed reclaimable space",
    "image st_blocks is host file allocation, not APFS unique extents or snapshot accounting",
    "private target/temp, control, OS and snapshot overhead are excluded; whole-host cost and Finder purgeable are unknown",
];
/// Budget-only directory backend for the `oxide` front door: admission and
/// whole-idle-slot eviction against a recorded budget, with no OS ceiling.
/// Only [`managed`] opens it; the standalone CLI refuses it.
pub const BUDGET_BACKEND_DIRECTORY_V1: &str = "directory-budget-v1";
/// Finite slot count. Exhaustion refuses a new slot.
pub const MAX_SLOTS: usize = 32;
/// Finite ledger file size including the atomic temp.
pub const MAX_LEDGER_BYTES: u64 = 256 * 1024;
/// Finite holder identity.
pub const MAX_HOLDER_BYTES: usize = 128;
/// Finite canonical path.
pub const MAX_PATH_BYTES: usize = 1024;
/// Finite lease-id text.
pub const MAX_LEASE_ID_BYTES: usize = 160;
/// Cooperative inventory bound used for slot accounting.
const ACCOUNTING_MAX_ENTRIES: u64 = 50_000;
const ACCOUNTING_MAX_DEPTH: usize = 24;
const ACCOUNTING_MAX_ELAPSED_MS: u64 = 5_000;

const CONFIG_NAME: &str = "config.json";
const LEDGER_NAME: &str = "ledger.json";
const LOCK_NAME: &str = "lock";
const IMAGE_NAME: &str = "image.sparseimage";
const RECEIPT_NAME: &str = "receipt.json";
const PAYLOAD_NAME: &str = "payload";
const SLOTS_NAME: &str = "slots";
/// Private control directory owned by the `oxide` front door. The lease CLI
/// tolerates it as a real directory and never reads or writes it.
pub const MANAGED_NAME: &str = "oxide-managed";

/// Caller-selected Cargo profile label. The CLI records it and returns it; it
/// does not force incremental, debug or feature flags.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    Dev,
    FullDebug,
}

impl Profile {
    pub fn parse(value: &str) -> Result<Self, LeaseError> {
        match value {
            "dev" => Ok(Self::Dev),
            "full-debug" => Ok(Self::FullDebug),
            _ => Err(LeaseError::InvalidInput(
                "profile must be dev or full-debug",
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::FullDebug => "full-debug",
        }
    }
}

/// Occupancy recorded in the ledger and shown by status.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotOccupancy {
    Held,
    Idle,
    /// Reserved. This version never infers abandoned from TTL, PID or holder
    /// loss; recover still asserts writer closure on a held lease.
    Abandoned,
}

/// Why a command refused. Mapped to a small exit set by [`LeaseError::exit_code`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LeaseError {
    Usage(&'static str),
    InvalidInput(&'static str),
    Busy,
    Full,
    UnknownAccounting,
    UnsafePath(&'static str),
    HolderConflict,
    NotFound,
    BackingMismatch,
    LedgerExhausted,
    Unsupported(&'static str),
    Io(String),
}

impl LeaseError {
    /// 0 is success. 1 usage/invalid/io. 2 busy or holder conflict. 3 full,
    /// unknown accounting or ledger exhaustion. 4 unsafe path or mismatched
    /// backing. 5 unknown lease.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) | Self::InvalidInput(_) | Self::Io(_) | Self::Unsupported(_) => 1,
            Self::Busy | Self::HolderConflict => 2,
            Self::Full | Self::UnknownAccounting | Self::LedgerExhausted => 3,
            Self::UnsafePath(_) | Self::BackingMismatch => 4,
            Self::NotFound => 5,
        }
    }

    pub fn reason(&self) -> &'static str {
        match self {
            Self::Usage(_) => "usage",
            Self::InvalidInput(_) => "invalid_input",
            Self::Busy => "busy",
            Self::Full => "full",
            Self::UnknownAccounting => "unknown_accounting",
            Self::UnsafePath(_) => "unsafe_path",
            Self::HolderConflict => "holder_conflict",
            Self::NotFound => "not_found",
            Self::BackingMismatch => "backing_mismatch",
            Self::LedgerExhausted => "ledger_exhausted",
            Self::Unsupported(_) => "unsupported",
            Self::Io(_) => "io",
        }
    }
}

impl std::fmt::Display for LeaseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(message) | Self::InvalidInput(message) | Self::UnsafePath(message) => {
                formatter.write_str(message)
            }
            Self::Busy => formatter.write_str("compatible slot is held"),
            Self::Full => formatter.write_str("pool cap cannot admit a new slot"),
            Self::UnknownAccounting => {
                formatter.write_str("owned accounting is unknown or a foreign entry is present")
            }
            Self::HolderConflict => {
                formatter.write_str("holder is already bound to a different lease")
            }
            Self::NotFound => formatter.write_str("lease identity is not in this pool"),
            Self::BackingMismatch => {
                formatter.write_str("backing does not match the pinned pool descriptor")
            }
            Self::LedgerExhausted => {
                formatter.write_str("finite ledger or slot count is exhausted")
            }
            Self::Unsupported(message) => formatter.write_str(message),
            Self::Io(message) => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for LeaseError {}

/// Lease grant returned on stdout as versioned JSON.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseGrant {
    pub version: u32,
    pub lease_id: String,
    pub store_id: String,
    pub slot_id: String,
    pub incarnation: String,
    pub project: PathBuf,
    pub workspace: PathBuf,
    pub profile: Profile,
    pub holder: String,
    pub state: SlotOccupancy,
    pub reused_slot: bool,
    pub env: LeaseEnvironment,
}

/// Exact environment the caller should apply for this lease. Final target
/// output stays private to the workspace; intermediates live on the slot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseEnvironment {
    #[serde(rename = "CARGO_BUILD_BUILD_DIR")]
    pub cargo_build_build_dir: PathBuf,
    #[serde(rename = "CARGO_TARGET_DIR")]
    pub cargo_target_dir: PathBuf,
}

/// Idempotent release or recover receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseReceipt {
    pub version: u32,
    pub lease_id: String,
    pub state: SlotOccupancy,
    pub replay: bool,
}

/// Pool identity after init.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolInfo {
    pub version: u32,
    pub store_id: String,
    pub backend: String,
    pub cap_bytes: u64,
    pub verified_capacity_bytes: u64,
}

/// Status of one slot. No payload or secret content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotStatus {
    pub slot_id: String,
    pub incarnation: String,
    pub state: SlotOccupancy,
    pub project: PathBuf,
    pub workspace: PathBuf,
    pub profile: Profile,
    pub holder: Option<String>,
    pub lease_id: Option<String>,
    pub logical_bytes: Option<u64>,
    pub unique_allocated_bytes: Option<u64>,
}

/// Finite pool status. Partial or unknown accounting is explicit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolStatus {
    pub version: u32,
    pub store_id: String,
    pub backend: String,
    pub cap_bytes: u64,
    pub payload_logical_bytes: Option<u64>,
    pub payload_unique_allocated_bytes: Option<u64>,
    pub coverage: AccountingCoverage,
    pub full: Option<bool>,
    pub slots: Vec<SlotStatus>,
}

/// A byte sample is either an exact value or an explicit bounded unknown.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByteObservation {
    pub value_bytes: Option<u64>,
    pub unknown_reason: Option<ObservationUnknownReason>,
}

impl ByteObservation {
    pub const fn observed(value_bytes: u64) -> Self {
        Self {
            value_bytes: Some(value_bytes),
            unknown_reason: None,
        }
    }

    pub const fn unknown(reason: ObservationUnknownReason) -> Self {
        Self {
            value_bytes: None,
            unknown_reason: Some(reason),
        }
    }
}

/// Finite reasons for a capacity sample that could not be reported.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationUnknownReason {
    Unsupported,
    SampleFailed,
    Overflow,
}

/// Measurements from the explicitly verified pool payload and pinned image.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityObservations {
    pub declared_cap_bytes: u64,
    pub verified_capacity_bytes: u64,
    pub payload_free_bytes: ByteObservation,
    pub payload_available_bytes: ByteObservation,
    pub image_logical_bytes: ByteObservation,
    pub image_allocated_bytes: ByteObservation,
    pub host_available_bytes: ByteObservation,
    pub coverage: AccountingCoverage,
}

impl CapacityObservations {
    fn new(
        declared_cap_bytes: u64,
        verified_capacity_bytes: u64,
        payload_free_bytes: ByteObservation,
        payload_available_bytes: ByteObservation,
        image_logical_bytes: ByteObservation,
        image_allocated_bytes: ByteObservation,
        host_available_bytes: ByteObservation,
    ) -> Self {
        let known = [
            payload_free_bytes,
            payload_available_bytes,
            image_logical_bytes,
            image_allocated_bytes,
            host_available_bytes,
        ]
        .into_iter()
        .filter(|sample| sample.value_bytes.is_some())
        .count();
        let coverage = if known == 5 {
            AccountingCoverage::Complete
        } else if known == 0 {
            AccountingCoverage::Unknown
        } else {
            AccountingCoverage::Partial
        };
        Self {
            declared_cap_bytes,
            verified_capacity_bytes,
            payload_free_bytes,
            payload_available_bytes,
            image_logical_bytes,
            image_allocated_bytes,
            host_available_bytes,
            coverage,
        }
    }
}

/// Versioned capacity output. The nested status retains its strict legacy shape.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CapacityStatus {
    pub version: u32,
    pub status: PoolStatus,
    pub capacity: CapacityObservations,
    pub notes: [&'static str; 4],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingCoverage {
    Complete,
    Partial,
    Unknown,
}

/// Rendered status bytes plus whether the finite output cap cut the report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedStatus {
    pub bytes: Vec<u8>,
    pub truncated: bool,
    pub partial: bool,
}

pub fn init_pool(pool: &Path, cap_bytes: u64, image_bytes: u64) -> Result<PoolInfo, LeaseError> {
    pool::init(pool::InitRequest {
        pool: pool.to_path_buf(),
        cap_bytes,
        image_bytes,
    })
}

pub fn render_status_json(
    status: &PoolStatus,
    max_output_bytes: usize,
) -> Result<RenderedStatus, LeaseError> {
    render_bounded(
        serde_json::to_vec(status).map_err(|error| LeaseError::Io(error.to_string()))?,
        max_output_bytes,
        true,
        status.coverage != AccountingCoverage::Complete,
    )
}

pub fn render_status_human(
    status: &PoolStatus,
    max_output_bytes: usize,
) -> Result<RenderedStatus, LeaseError> {
    let mut text = format!(
        "oxide-storage-lease status\nstore {} backend {} cap {}\ncoverage {:?} full {:?}\npayload logical {:?} unique {:?}\nslots {}\n",
        status.store_id,
        status.backend,
        status.cap_bytes,
        status.coverage,
        status.full,
        status.payload_logical_bytes,
        status.payload_unique_allocated_bytes,
        status.slots.len()
    );
    for slot in &status.slots {
        text.push_str(&format!(
            "  slot {} inc {} {:?} project {} workspace {} profile {} holder {} lease {} logical {:?} unique {:?}\n",
            slot.slot_id,
            slot.incarnation,
            slot.state,
            slot.project.display(),
            slot.workspace.display(),
            slot.profile.as_str(),
            slot.holder.as_deref().unwrap_or("-"),
            slot.lease_id.as_deref().unwrap_or("-"),
            slot.logical_bytes,
            slot.unique_allocated_bytes
        ));
    }
    render_bounded(
        text.into_bytes(),
        max_output_bytes,
        false,
        status.coverage != AccountingCoverage::Complete,
    )
}

pub fn render_capacity_status_json(
    report: &CapacityStatus,
    max_output_bytes: usize,
) -> Result<RenderedStatus, LeaseError> {
    render_bounded(
        serde_json::to_vec(report).map_err(|error| LeaseError::Io(error.to_string()))?,
        max_output_bytes,
        true,
        report.status.coverage != AccountingCoverage::Complete
            || report.capacity.coverage != AccountingCoverage::Complete,
    )
}

pub fn render_capacity_status_human(
    report: &CapacityStatus,
    max_output_bytes: usize,
) -> Result<RenderedStatus, LeaseError> {
    let status = &report.status;
    let capacity = &report.capacity;
    let mut text = format!(
        "oxide-storage-lease status --capacity\nstore {} backend {}\nlegacy cap {} bytes; full {:?} (slot allocation vs declared cap, not filesystem headroom)\nslot accounting {:?}: logical {:?} bytes, unique allocated {:?} bytes\ncapacity observation coverage {:?}\n",
        status.store_id,
        status.backend,
        status.cap_bytes,
        status.full,
        status.coverage,
        status.payload_logical_bytes,
        status.payload_unique_allocated_bytes,
        capacity.coverage,
    );
    for slot in &status.slots {
        text.push_str(&format!(
            "  slot {} {:?} logical {:?} bytes, unique allocated {:?} bytes\n",
            slot.slot_id, slot.state, slot.logical_bytes, slot.unique_allocated_bytes
        ));
    }
    text.push_str(&format!(
        "declared payload cap {} bytes; verified payload ceiling {} bytes\n",
        capacity.declared_cap_bytes, capacity.verified_capacity_bytes
    ));
    append_observation(
        &mut text,
        "payload filesystem free (statfs f_bfree)",
        capacity.payload_free_bytes,
    );
    append_observation(
        &mut text,
        "payload filesystem available (statfs f_bavail)",
        capacity.payload_available_bytes,
    );
    append_observation(
        &mut text,
        "image file logical (st_size)",
        capacity.image_logical_bytes,
    );
    append_observation(
        &mut text,
        "image file allocated (st_blocks x 512; not APFS unique extents)",
        capacity.image_allocated_bytes,
    );
    append_observation(
        &mut text,
        "host filesystem available at pool (statfs f_bavail)",
        capacity.host_available_bytes,
    );
    text.push_str(
        "observations are not reservations, fit predictions or attributable writes\nnot whole-host cost: control, private target/temp, OS and snapshot overhead excluded; Finder purgeable unknown\n",
    );
    render_bounded(
        text.into_bytes(),
        max_output_bytes,
        false,
        status.coverage != AccountingCoverage::Complete
            || capacity.coverage != AccountingCoverage::Complete,
    )
}

fn append_observation(text: &mut String, label: &str, observation: ByteObservation) {
    match (observation.value_bytes, observation.unknown_reason) {
        (Some(bytes), None) => text.push_str(&format!("{label}: {bytes} bytes\n")),
        (None, Some(reason)) => text.push_str(&format!("{label}: unknown ({reason:?})\n")),
        _ => text.push_str(&format!("{label}: unknown (sample_failed)\n")),
    }
}

fn render_bounded(
    mut bytes: Vec<u8>,
    max_output_bytes: usize,
    json: bool,
    partial: bool,
) -> Result<RenderedStatus, LeaseError> {
    if max_output_bytes < MIN_STATUS_OUTPUT_BYTES {
        return Err(LeaseError::InvalidInput(
            "max-output-bytes must be at least 64",
        ));
    }
    if bytes.len() <= max_output_bytes {
        if json && !bytes.ends_with(&[b'\n']) {
            bytes.push(b'\n');
            if bytes.len() > max_output_bytes {
                bytes.pop();
            }
        }
        return Ok(RenderedStatus {
            truncated: false,
            partial,
            bytes,
        });
    }
    if json {
        return Ok(RenderedStatus {
            bytes: b"{\"truncated\":true,\"partial\":true}\n".to_vec(),
            truncated: true,
            partial: true,
        });
    }
    let marker = b"\n[output truncated at max_output_bytes]\n";
    if max_output_bytes <= marker.len() {
        return Ok(RenderedStatus {
            bytes: marker[..max_output_bytes].to_vec(),
            truncated: true,
            partial: true,
        });
    }
    let keep = max_output_bytes - marker.len();
    let mut cut = bytes[..keep].to_vec();
    while !cut.is_empty() && std::str::from_utf8(&cut).is_err() {
        cut.pop();
    }
    cut.extend_from_slice(marker);
    Ok(RenderedStatus {
        bytes: cut,
        truncated: true,
        partial: true,
    })
}

fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

fn io_error(error: std::io::Error) -> LeaseError {
    LeaseError::Io(error.to_string())
}

fn require_absolute(path: &Path, what: &'static str) -> Result<(), LeaseError> {
    if path.is_absolute() {
        Ok(())
    } else {
        let _ = what;
        Err(LeaseError::InvalidInput("paths must be absolute"))
    }
}

fn require_bounded_text(value: &str, max: usize, what: &'static str) -> Result<(), LeaseError> {
    if value.is_empty() || value.len() > max {
        let _ = what;
        return Err(LeaseError::InvalidInput(
            "identity text is empty or too long",
        ));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_graphic() && byte != b'/')
    {
        return Err(LeaseError::InvalidInput(
            "identity text must be printable ASCII without slash",
        ));
    }
    Ok(())
}

fn canonical_existing_dir(path: &Path, owner_only: bool) -> Result<PathBuf, LeaseError> {
    require_absolute(path, "path")?;
    if path.as_os_str().as_bytes().len() > MAX_PATH_BYTES {
        return Err(LeaseError::InvalidInput("path is too long"));
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|_| LeaseError::UnsafePath("path missing"))?;
    if metadata.file_type().is_symlink() {
        return Err(LeaseError::UnsafePath("symlinks are refused"));
    }
    if !metadata.file_type().is_dir() {
        return Err(LeaseError::InvalidInput("path must be a directory"));
    }
    if owner_only {
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(LeaseError::UnsafePath("directory must be owner-only"));
        }
        if metadata.uid() != current_uid() {
            return Err(LeaseError::UnsafePath(
                "directory must be owned by the caller",
            ));
        }
    }
    let canonical = fs::canonicalize(path).map_err(io_error)?;
    if canonical != path {
        return Err(LeaseError::UnsafePath(
            "path must be supplied in canonical form",
        ));
    }
    Ok(canonical)
}

fn ensure_private_dir(path: &Path) -> Result<(), LeaseError> {
    require_absolute(path, "directory")?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(LeaseError::UnsafePath("symlinks are refused"));
            }
            if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o077 != 0 {
                return Err(LeaseError::UnsafePath("directory must be owner-only"));
            }
            if metadata.uid() != current_uid() {
                return Err(LeaseError::UnsafePath(
                    "directory must be owned by the caller",
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(io_error)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
            if let Some(parent) = path.parent() {
                File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .map_err(io_error)?;
            }
        }
        Err(error) => return Err(io_error(error)),
    }
    let canonical = fs::canonicalize(path).map_err(io_error)?;
    if canonical != path {
        return Err(LeaseError::UnsafePath(
            "path must be supplied in canonical form",
        ));
    }
    Ok(())
}

fn persist_private_file(path: &Path, bytes: &[u8]) -> Result<(), LeaseError> {
    if bytes.len() as u64 > MAX_LEDGER_BYTES {
        return Err(LeaseError::LedgerExhausted);
    }
    let parent = path
        .parent()
        .ok_or(LeaseError::InvalidInput("file has no parent"))?;
    let mut temp_name = String::from("ledger.tmp-");
    temp_name.push_str(&random_hex(8)?);
    let temp_path = parent.join(temp_name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(&temp_path).map_err(io_error)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temp_path);
        return Err(io_error(error));
    }
    drop(file);
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(io_error(error));
    }
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)
}

fn read_private_file(path: &Path) -> Result<Vec<u8>, LeaseError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != current_uid()
        || metadata.len() > MAX_LEDGER_BYTES
    {
        return Err(LeaseError::UnknownAccounting);
    }
    let file = File::open(path).map_err(io_error)?;
    let mut bytes = Vec::new();
    file.take(MAX_LEDGER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > MAX_LEDGER_BYTES {
        return Err(LeaseError::LedgerExhausted);
    }
    Ok(bytes)
}

fn random_hex(bytes: usize) -> Result<String, LeaseError> {
    let mut value = vec![0_u8; bytes];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut value))
        .map_err(io_error)?;
    let mut result = String::with_capacity(bytes * 2);
    for byte in value {
        use std::fmt::Write as _;
        write!(&mut result, "{byte:02x}")
            .map_err(|_| LeaseError::Io("token encoding failed".into()))?;
    }
    Ok(result)
}

fn remove_tree_owned(root: &Path, path: &Path) -> Result<(), LeaseError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() {
        if metadata.uid() != current_uid() {
            return Err(LeaseError::UnsafePath(
                "refusing to delete a foreign symlink",
            ));
        }
        let parent = path.parent().ok_or(LeaseError::UnknownAccounting)?;
        let parent_metadata = fs::symlink_metadata(parent).map_err(io_error)?;
        if parent_metadata.file_type().is_symlink()
            || !parent_metadata.file_type().is_dir()
            || parent_metadata.uid() != current_uid()
        {
            return Err(LeaseError::UnsafePath(
                "symlink parent must be an owned directory",
            ));
        }
        let canonical_root = fs::canonicalize(root).map_err(io_error)?;
        let canonical_parent = fs::canonicalize(parent).map_err(io_error)?;
        if canonical_parent == canonical_root || !canonical_parent.starts_with(&canonical_root) {
            return Err(LeaseError::UnsafePath(
                "deletion is limited to recorded owned slot paths",
            ));
        }
        return fs::remove_file(path).map_err(io_error);
    }
    if metadata.file_type().is_file() {
        if metadata.uid() != current_uid() {
            return Err(LeaseError::UnsafePath("refusing to delete a foreign file"));
        }
        return fs::remove_file(path).map_err(io_error);
    }
    if !metadata.file_type().is_dir() {
        return Err(LeaseError::UnknownAccounting);
    }
    let canonical = fs::canonicalize(path).map_err(io_error)?;
    let canonical_root = fs::canonicalize(root).map_err(io_error)?;
    if !canonical.starts_with(&canonical_root) || canonical == canonical_root {
        return Err(LeaseError::UnsafePath(
            "deletion is limited to recorded owned slot paths",
        ));
    }
    if metadata.uid() != current_uid() {
        return Err(LeaseError::UnsafePath(
            "refusing to delete a foreign directory",
        ));
    }
    for entry in fs::read_dir(path).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        remove_tree_owned(root, &entry.path())?;
    }
    fs::remove_dir(path).map_err(io_error)
}
