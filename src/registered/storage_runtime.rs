//! Unpublished private storage-runtime process (oxide#033).
//!
//! A trusted embedder creates the protected bootstrap and the owner-only parent
//! control listener. This process consumes the bootstrap, binds the actual
//! `Daemon` with the separately installed `QualificationAuthority` and a
//! root-only free-space probe, then serves the existing storage protocol.
//! Readiness and shutdown use a distinct control key and domains. Ready is
//! metadata only: it grants no lease, token, submission or completion proof.
//! No install, service, production issuer, Cargo workload or consumer mutation
//! is part of this surface.

use super::qualification::{
    canonical_digest, decode_hex, owned_directory, private_directory, read_protected, Endpoint,
    MacKey, QualificationAuthority,
};
use super::runner_host::{HostError, WireU64};
use crate::storage::{
    BoundedBackendDescriptor, BoundedBacking, Budget, Credential, Daemon, DaemonConfig,
    FreeSpaceProbe, PartitionKey, RunBinding, RunEnvelope, StorageError,
    BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::mem::MaybeUninit;
use std::net::Shutdown;
use std::os::unix::ffi::OsStrExt;
#[cfg(feature = "private-qualification-fixture")]
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Version for bootstrap, readiness framing, and control commands.
pub const PROTOCOL_VERSION: u32 = 1;
/// Version spoken by the storage socket, independently versioned from control.
pub const STORAGE_PROTOCOL_VERSION: u32 = crate::storage::STORAGE_PROTOCOL_VERSION;
/// Nonrenewable monotonic ceiling. An owner window may be shorter, never longer.
pub const MAX_LIFETIME: Duration = Duration::from_secs(45 * 60);
/// Upper bound on the wait after a stop before an in-flight storage operation is abandoned.
pub const MAX_STOP_GRACE: Duration = Duration::from_secs(10);
/// Bounded tolerance for the owned child to finish writing and exit once its stop is decided.
/// It also bounds every runtime control write, including failure frames.
pub const EXIT_TOLERANCE: Duration = Duration::from_secs(2);
/// Readiness is written only while at least this much of the owner window remains.
const READY_MARGIN: Duration = Duration::from_secs(1);
/// Per-frame bound for every control frame, including the ready frame.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
/// Aggregate bound for all control bytes read by the runtime.
pub const MAX_CONTROL_BYTES: usize = 256 * 1024;
/// Per-call bound for control reads and writes. Idle reads resume; the lifetime is supervised separately.
pub const CONTROL_IO_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ADMISSIONS: usize = 4;
const MAX_SCOPE_BYTES: usize = 4096;
const MAX_ID_BYTES: usize = 128;
const SOCKET_NAME: &str = "daemon.sock";
const READY_DOMAIN: &str = "oxide-runtime-ready-v1";
const SHUTDOWN_DOMAIN: &str = "oxide-runtime-shutdown-v1";
const STOPPED_DOMAIN: &str = "oxide-runtime-stopped-v1";
const FAILED_DOMAIN: &str = "oxide-runtime-failed-v1";

/// Trusted, one-use bootstrap. Unknown fields refuse. Keys are owner-created
/// 256-bit values and never come from task data, environment or argv.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBootstrap {
    pub version: u32,
    pub storage_protocol: u32,
    pub control_protocol: u32,
    pub scope: String,
    /// Owner's remaining qualification window, as Unix seconds.
    pub expires_unix_seconds: WireU64,
    /// Wait after a stop before an in-flight storage operation is abandoned. Bounded 1..=10.
    pub stop_grace_seconds: WireU64,
    /// Canonical, owner-only storage root. The daemon socket is `daemon.sock` inside it.
    pub storage_root: PathBuf,
    pub budget: RuntimeBudget,
    /// Exact approved subjects. Credentials are minted by the owner after readiness.
    pub admissions: Vec<ApprovedSubject>,
    /// Exact ordinary-Cargo request digests admitted by the protected owner.
    /// Absent or empty keeps the ordinary route disabled.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ordinary_cargo_request_digests: Vec<String>,
    /// Opt-in hard-bounded payload backing. Absent keeps the directory-only
    /// budget-only store. Present requires the supported macOS backend and
    /// changes neither the control protocol version nor the storage wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounded: Option<RuntimeBoundedStorage>,
    /// Admission and settlement authenticator. Distinct from `control_key_hex`.
    pub authority_key_hex: String,
    /// Readiness and shutdown authenticator. Distinct purposes and domains.
    pub control_key_hex: String,
    /// Pinned parent control listener.
    pub control: RuntimeEndpoint,
}

/// Explicit finite storage budget. Not proof of kernel enforcement.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBudget {
    pub cap_bytes: WireU64,
    pub free_reserve_bytes: WireU64,
}

/// Owner-declared opt-in bounded backing paths. The runtime provisions or
/// opens the recorded image under these exact names; the payload ceiling is
/// the bootstrap budget cap, so every bounded ceiling is bound to startup.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBoundedStorage {
    /// Exactly the one supported backend tag. Anything else refuses.
    pub backend: String,
    /// Owner-sparse image file, created if absent with its receipt.
    pub image_path: PathBuf,
    /// Capacity proof written at provisioning and required at every open.
    pub receipt_path: PathBuf,
    /// Canonical mount point of the payload volume.
    pub mount_point: PathBuf,
    /// Nominal image size. Must cover the ceiling plus filesystem metadata,
    /// because the usable volume capacity is always smaller.
    pub image_bytes: WireU64,
    /// Must equal the bootstrap budget cap so there is one ceiling source.
    pub payload_ceiling_bytes: WireU64,
}

/// Durable capacity proof for one provisioned image. Written once by the
/// runtime; every later open verifies the live volume against it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct BoundedReceipt {
    version: u32,
    backend: String,
    image_path: PathBuf,
    mount_point: PathBuf,
    image_bytes: WireU64,
    payload_ceiling_bytes: WireU64,
    verified_capacity_bytes: WireU64,
    identity: String,
}

/// Socket identity with lossless integer encoding for TypeScript owners.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEndpoint {
    pub path: PathBuf,
    pub device: WireU64,
    pub inode: WireU64,
}

/// One approved admission subject. The owner mints its credential after readiness.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedSubject {
    pub binding: RunBinding,
    pub partition: PartitionKey,
    pub admission_reference: String,
}

/// Why the runtime stopped serving. None of these is writer-completion proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stop {
    /// Authenticated shutdown from the owner; the stopped acknowledgement was sent.
    Shutdown(String),
    /// Owner window or the nonrenewable runtime deadline elapsed.
    Lifetime,
    /// Parent closed the control channel.
    ControlClosed,
    /// A control frame failed bounds, authentication or protocol checks.
    ControlRefused,
    /// A storage operation was still running after the stop grace. The process exits
    /// like a crash: the ledger keeps its last committed state and leases stay held.
    Abandoned,
    /// The owner window ended before readiness was written. No ready frame was published.
    /// Any replay marker or partial state stays as it was.
    StartupExpired,
}

/// Parent-side expectation. The owner derives every field from its own bootstrap.
pub struct ReadyExpectation {
    pub scope: String,
    pub bootstrap_digest: String,
    pub policy_digest: String,
    pub storage_socket: PathBuf,
}

/// Authenticated readiness identity. Metadata only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ready {
    pub storage_instance_id: String,
    pub storage: RuntimeEndpoint,
    pub budget: RuntimeBudget,
    pub expires_unix_seconds: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadyBody {
    version: u32,
    operation: String,
    scope: String,
    bootstrap_digest: String,
    policy_digest: String,
    storage_instance_id: String,
    storage: RuntimeEndpoint,
    storage_protocol: u32,
    control_protocol: u32,
    budget: RuntimeBudget,
    expires_unix_seconds: WireU64,
}

/// Digest of the exact bootstrap bytes, including keys. This is the replay identity.
pub fn bootstrap_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Digest of the scope, budget, bounded backing and exact approved subjects.
/// Excludes credentials. The bounded key is present only when bounded mode is
/// declared, so a directory bootstrap keeps its recorded digest.
pub fn policy_digest(
    scope: &str,
    budget: &RuntimeBudget,
    stop_grace_seconds: WireU64,
    admissions: &[ApprovedSubject],
    ordinary_cargo_request_digests: &[String],
    bounded: Option<&RuntimeBoundedStorage>,
) -> Result<String, HostError> {
    let mut policy = json!({
        "scope": scope,
        "storage_protocol": STORAGE_PROTOCOL_VERSION,
        "control_protocol": PROTOCOL_VERSION,
        "budget": serde_json::to_value(budget).map_err(|_| HostError::Protocol)?,
        "stop_grace_seconds": serde_json::to_value(stop_grace_seconds).map_err(|_| HostError::Protocol)?,
        "admissions": serde_json::to_value(admissions).map_err(|_| HostError::Protocol)?,
        "ordinary_cargo_request_digests": ordinary_cargo_request_digests,
    });
    if let Some(bounded) = bounded {
        policy["bounded"] = serde_json::to_value(bounded).map_err(|_| HostError::Protocol)?;
    }
    canonical_digest(&policy)
}

/// Parent-side check of a readiness frame. Verifies the control MAC over every
/// field, the bootstrap and policy identity, and the exact storage endpoint.
pub fn verify_ready(
    key: &MacKey,
    expectation: &ReadyExpectation,
    frame: &Value,
) -> Result<Ready, HostError> {
    let ready = verify_ready_frame(key, expectation, frame)?;
    // A ready frame is accepted only while its window end has not passed on the parent side.
    if unix_now()? >= ready.expires_unix_seconds {
        return Err(HostError::OwnerRejected);
    }
    let current = Endpoint::capture(ready.storage.path.clone())?;
    if current.device != ready.storage.device.0 || current.inode != ready.storage.inode.0 {
        return Err(HostError::OwnerRejected);
    }
    Ok(ready)
}

/// Authentication and identity fields of a readiness frame, without filesystem access.
pub fn verify_ready_frame(
    key: &MacKey,
    expectation: &ReadyExpectation,
    frame: &Value,
) -> Result<Ready, HostError> {
    let mut body = frame.clone();
    let mac = body
        .as_object_mut()
        .and_then(|object| object.remove("mac"))
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or(HostError::Protocol)?;
    let ready: ReadyBody = serde_json::from_value(body.clone()).map_err(|_| HostError::Protocol)?;
    if ready.version != PROTOCOL_VERSION
        || ready.operation != "ready"
        || ready.scope != expectation.scope
        || ready.bootstrap_digest != expectation.bootstrap_digest
        || ready.policy_digest != expectation.policy_digest
        || ready.storage_protocol != STORAGE_PROTOCOL_VERSION
        || ready.control_protocol != PROTOCOL_VERSION
        || decode_hex(&ready.storage_instance_id).is_none()
        || ready.storage.path != expectation.storage_socket
    {
        return Err(HostError::OwnerRejected);
    }
    let body_digest = canonical_digest(&body)?;
    if !key.verify(
        READY_DOMAIN,
        &[
            &expectation.scope,
            &expectation.bootstrap_digest,
            &expectation.policy_digest,
            &ready.storage_instance_id,
            &body_digest,
        ],
        &mac,
    ) {
        return Err(HostError::OwnerRejected);
    }
    Ok(Ready {
        storage_instance_id: ready.storage_instance_id,
        storage: ready.storage,
        budget: ready.budget,
        expires_unix_seconds: ready.expires_unix_seconds.0,
    })
}

/// Parent-side authenticated shutdown command bound to the ready instance.
pub fn shutdown_frame(
    key: &MacKey,
    scope: &str,
    instance: &str,
    bootstrap_digest: &str,
    id: &str,
) -> Result<Value, HostError> {
    valid_id(id)?;
    let mac = key.sign(SHUTDOWN_DOMAIN, &[scope, instance, bootstrap_digest, id])?;
    Ok(json!({"version": PROTOCOL_VERSION, "operation": "shutdown", "id": id, "mac": mac}))
}

/// Parent-side check of the authenticated stopped acknowledgement.
pub fn verify_stopped(
    key: &MacKey,
    scope: &str,
    instance: &str,
    bootstrap_digest: &str,
    id: &str,
    frame: &Value,
) -> bool {
    // Exactly the four ack fields. Any extra field refuses, matching the unknown-field contract.
    let exact_fields = frame.as_object().is_some_and(|object| {
        object.len() == 4
            && ["version", "operation", "id", "mac"]
                .iter()
                .all(|key| object.contains_key(*key))
    });
    exact_fields
        && frame["version"] == PROTOCOL_VERSION
        && frame["operation"] == "stopped"
        && frame["id"] == id
        && frame["mac"].as_str().is_some_and(|mac| {
            key.verify(
                STOPPED_DOMAIN,
                &[scope, instance, bootstrap_digest, id],
                mac,
            )
        })
}

fn valid_id(id: &str) -> Result<(), HostError> {
    if id.is_empty()
        || id.len() > MAX_ID_BYTES
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(HostError::Protocol);
    }
    Ok(())
}

/// Validated, pre-effect plan. Keys are process-lifetime memory only.
struct Plan {
    scope: String,
    bootstrap_digest: String,
    policy_digest: String,
    budget: RuntimeBudget,
    bounded: Option<RuntimeBoundedStorage>,
    ordinary_cargo_request_digests: Vec<String>,
    authority_key: MacKey,
    control_key: MacKey,
    deadline: Instant,
    stop_grace: Duration,
    expires_unix_seconds: u64,
}

/// Entry sample for the owner window: the monotonic instant and the wall clock, taken as a
/// pair. The instant is sampled first, so any gap between the two shortens the window
/// instead of extending it.
#[derive(Clone, Copy, Debug)]
struct Entry {
    instant: Instant,
    wall: Duration,
}

fn sample_entry() -> Result<Entry, HostError> {
    let instant = Instant::now();
    let wall = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| HostError::Protocol)?;
    Ok(Entry { instant, wall })
}

/// Nonrenewable window from the entry sample, in exact arithmetic. The owner's end is
/// compared with the full-precision entry wall clock, so no fractional second is dropped.
/// The monotonic deadline is `entry.instant + lifetime`, and `lifetime` never exceeds
/// `owner_end - entry.wall`, so the deadline never passes the owner's end. The ceiling
/// leaves room for the stop grace and the exit tolerance. The published expiry is the whole
/// second at or before `entry.wall + lifetime`. It is never later than the deadline, so the
/// parent may refuse up to one second early and never late. `None` means the window is
/// already spent or cannot be represented.
fn session_window(entry: Entry, expires_unix: u64, stop_grace: Duration) -> Option<(Instant, u64)> {
    let remaining = Duration::from_secs(expires_unix).checked_sub(entry.wall)?;
    if remaining.is_zero() {
        return None;
    }
    let cap = MAX_LIFETIME
        .checked_sub(stop_grace)?
        .checked_sub(EXIT_TOLERANCE)?;
    let lifetime = remaining.min(cap);
    let deadline = entry.instant.checked_add(lifetime)?;
    let published = entry.wall.checked_add(lifetime)?.as_secs();
    Some((deadline, published))
}

impl RuntimeBootstrap {
    fn validate(&self, entry: Entry, digest: &str) -> Result<(Plan, PathBuf), HostError> {
        if self.version != PROTOCOL_VERSION
            || self.storage_protocol != STORAGE_PROTOCOL_VERSION
            || self.control_protocol != PROTOCOL_VERSION
            || self.scope.is_empty()
            || self.scope.len() > MAX_SCOPE_BYTES
            || self.admissions.is_empty()
            || self.admissions.len() > MAX_ADMISSIONS
            || self.ordinary_cargo_request_digests.len() > MAX_ADMISSIONS
            || self
                .ordinary_cargo_request_digests
                .iter()
                .any(|digest| !valid_sha256_digest(digest))
            || self
                .ordinary_cargo_request_digests
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != self.ordinary_cargo_request_digests.len()
            || self.budget.cap_bytes.0 == 0
            || self.stop_grace_seconds.0 == 0
            || self.stop_grace_seconds.0 > MAX_STOP_GRACE.as_secs()
            || self.authority_key_hex == self.control_key_hex
            || !self.storage_root.is_absolute()
        {
            return Err(HostError::Protocol);
        }
        for approved in &self.admissions {
            if approved.admission_reference.is_empty()
                || approved.admission_reference.len() > MAX_SCOPE_BYTES
            {
                return Err(HostError::Protocol);
            }
        }
        let authority_key = MacKey::from_hex(&self.authority_key_hex)?;
        let control_key = MacKey::from_hex(&self.control_key_hex)?;
        // Owned, canonical, owner-only storage root; never a symlink or shared root.
        owned_directory(&self.storage_root)?;
        private_directory(&self.storage_root)?;
        // Control parent must be private and must not be the storage state root.
        let control_parent = self.control.path.parent().ok_or(HostError::Protocol)?;
        if control_parent == self.storage_root {
            return Err(HostError::Protocol);
        }
        let control = Endpoint::capture(self.control.path.clone())?;
        if control.device != self.control.device.0 || control.inode != self.control.inode.0 {
            return Err(HostError::Protocol);
        }
        let stop_grace = Duration::from_secs(self.stop_grace_seconds.0);
        if let Some(bounded) = self.bounded.as_ref() {
            validate_bounded(bounded, &self.storage_root, control_parent, &self.budget)?;
        }
        let (deadline, expires_unix_seconds) =
            session_window(entry, self.expires_unix_seconds.0, stop_grace)
                .ok_or(HostError::Protocol)?;
        let plan = Plan {
            scope: self.scope.clone(),
            bootstrap_digest: digest.to_owned(),
            policy_digest: policy_digest(
                &self.scope,
                &self.budget,
                self.stop_grace_seconds,
                &self.admissions,
                &self.ordinary_cargo_request_digests,
                self.bounded.as_ref(),
            )?,
            budget: self.budget,
            bounded: self.bounded.clone(),
            ordinary_cargo_request_digests: self.ordinary_cargo_request_digests.clone(),
            authority_key,
            control_key,
            deadline,
            stop_grace,
            expires_unix_seconds,
        };
        Ok((plan, self.storage_root.join(SOCKET_NAME)))
    }
}

/// Refuses any bounded declaration that is not the one supported macOS
/// backend with one ceiling source. This runs before any host effect, so an
/// unsupported platform or unknown backend is an explicit startup refusal.
fn validate_bounded(
    bounded: &RuntimeBoundedStorage,
    storage_root: &Path,
    control_parent: &Path,
    budget: &RuntimeBudget,
) -> Result<(), HostError> {
    if !cfg!(target_os = "macos")
        || bounded.backend != BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1
        || bounded.image_bytes.0 <= bounded.payload_ceiling_bytes.0
        || bounded.payload_ceiling_bytes.0 != budget.cap_bytes.0
    {
        return Err(HostError::Protocol);
    }
    let image = bounded.image_path.as_os_str().as_bytes();
    let receipt = bounded.receipt_path.as_os_str().as_bytes();
    let mount = bounded.mount_point.as_os_str().as_bytes();
    let empty_or_relative = |path: &[u8]| path.is_empty() || path[0] != b'/';
    if empty_or_relative(image)
        || empty_or_relative(receipt)
        || empty_or_relative(mount)
        || image == receipt
        || image == mount
        || receipt == mount
        || bounded.image_path == storage_root
        || bounded.receipt_path == storage_root
        || bounded.mount_point == storage_root
        || bounded.image_path == control_parent
        || bounded.receipt_path == control_parent
        || bounded.mount_point == control_parent
        || bounded.mount_point == storage_root.join(SOCKET_NAME)
    {
        return Err(HostError::Protocol);
    }
    Ok(())
}

/// One live payload-volume sample. The sample only verifies that the mounted
/// filesystem at the declared mount point is the supported APFS backend and
/// that its capacity matches the recorded proof; the recorded descriptor, not
/// a fresh sample, is the durable accounting truth.
#[cfg(target_os = "macos")]
struct VolumeSample {
    capacity_bytes: u64,
}

#[cfg(target_os = "macos")]
fn c_field_bytes(field: &[libc::c_char]) -> Vec<u8> {
    field
        .iter()
        .take_while(|&&value| value != 0)
        .map(|&value| value as u8)
        .collect()
}

/// Samples the mounted filesystem at one path and refuses anything but the
/// supported APFS backend mounted exactly there. Unsupported platforms never
/// reach this: the bounded declaration refuses before any effect.
#[cfg(target_os = "macos")]
fn sample_volume(mount_point: &Path) -> Result<VolumeSample, HostError> {
    let path = CString::new(mount_point.as_os_str().as_bytes()).map_err(|_| HostError::Protocol)?;
    let mut stat = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `stat` is writable storage for one
    // statfs record, which a zero return fully initializes.
    let status = unsafe { libc::statfs(path.as_ptr(), stat.as_mut_ptr()) };
    if status != 0 {
        return Err(HostError::Io);
    }
    // SAFETY: a zero return means statfs initialized the complete record.
    let stat = unsafe { stat.assume_init() };
    if c_field_bytes(&stat.f_fstypename) != b"apfs"
        || c_field_bytes(&stat.f_mntonname) != mount_point.as_os_str().as_bytes()
    {
        return Err(HostError::Storage);
    }
    let capacity_bytes = u64::from(stat.f_blocks)
        .checked_mul(u64::from(stat.f_bsize))
        .ok_or(HostError::Storage)?;
    if capacity_bytes == 0 {
        return Err(HostError::Storage);
    }
    Ok(VolumeSample { capacity_bytes })
}

/// Stable identity of the owned image file itself. Device and inode cannot
/// change while the file exists, so a replaced or recreated image always
/// refuses against the recorded receipt.
fn image_identity(image_path: &Path) -> Result<String, HostError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(image_path).map_err(|_| HostError::Io)?;
    if !metadata.is_file() {
        return Err(HostError::Storage);
    }
    Ok(format!("{:016x}{:016x}", metadata.dev(), metadata.ino()))
}

/// Routes one bounded declaration to provisioning or opening. An image
/// without its capacity proof, or a receipt without its image, refuses.
#[cfg(target_os = "macos")]
fn open_or_provision_bounded(
    bounded: &RuntimeBoundedStorage,
    ceiling: u64,
) -> Result<BoundedBacking, HostError> {
    let image_exists = bounded.image_path.symlink_metadata().is_ok();
    let receipt_exists = bounded.receipt_path.symlink_metadata().is_ok();
    match (image_exists, receipt_exists) {
        (false, false) => provision_bounded(bounded, ceiling),
        (true, false) => Err(HostError::Storage),
        (_, true) => open_bounded(bounded, ceiling),
    }
}

#[cfg(not(target_os = "macos"))]
fn open_or_provision_bounded(
    _bounded: &RuntimeBoundedStorage,
    _ceiling: u64,
) -> Result<BoundedBacking, HostError> {
    // Unreachable: the bounded declaration refuses before any effect.
    Err(HostError::Storage)
}

/// Creates one fresh fixed-size sparse APFS image, attaches it at the
/// declared mount point, records the capacity proof, and returns the verified
/// capability. No host setting, service or mount outside the owner scratch
/// paths is touched.
#[cfg(target_os = "macos")]
fn provision_bounded(
    bounded: &RuntimeBoundedStorage,
    ceiling: u64,
) -> Result<BoundedBacking, HostError> {
    // A fresh capability needs a fresh image: an existing file without a
    // receipt is a mismatched or partial backing and refuses.
    if bounded.image_path.symlink_metadata().is_ok()
        || bounded.receipt_path.symlink_metadata().is_ok()
    {
        return Err(HostError::Storage);
    }
    let scratch = bounded
        .image_path
        .parent()
        .ok_or(HostError::Protocol)?
        .to_path_buf();
    owned_directory(&scratch)?;
    fs::create_dir(&bounded.mount_point).map_err(|_| HostError::Io)?;
    owned_directory(&bounded.mount_point)?;
    let hdiutil = |arguments: &[&str], extra_env: bool| {
        let mut command = Command::new("/usr/bin/hdiutil");
        command
            .args(arguments)
            .env("TMPDIR", &scratch)
            .env("TMP", &scratch)
            .env("TEMP", &scratch);
        if extra_env {
            command.current_dir(&scratch);
        }
        command.output().map_err(|_| HostError::Io)
    };
    let size = bounded.image_bytes.0.to_string();
    let image = bounded
        .image_path
        .to_str()
        .ok_or(HostError::Protocol)?
        .to_owned();
    let mount = bounded
        .mount_point
        .to_str()
        .ok_or(HostError::Protocol)?
        .to_owned();
    let created = hdiutil(
        &[
            "create",
            "-size",
            &size,
            "-type",
            "SPARSE",
            "-fs",
            "APFS",
            "-volname",
            "oxide-bounded-v1",
            "-nospotlight",
            "-quiet",
            &image,
        ],
        false,
    )?;
    if !created.status.success() {
        return Err(HostError::Storage);
    }
    let attached = hdiutil(
        &[
            "attach",
            "-nobrowse",
            "-noverify",
            "-mountpoint",
            &mount,
            &image,
        ],
        false,
    )?;
    if !attached.status.success() {
        return Err(HostError::Storage);
    }
    if fs::canonicalize(&bounded.mount_point).map_err(|_| HostError::Io)? != bounded.mount_point {
        return Err(HostError::Storage);
    }
    // A fresh APFS volume root mounts world-readable. The payload is private
    // scratch, so provisioning makes the root owner-only before anything is
    // written; a later open refuses a volume that is not private.
    fs::set_permissions(&bounded.mount_point, fs::Permissions::from_mode(0o700))
        .map_err(|_| HostError::Io)?;
    let sample = sample_volume(&bounded.mount_point)?;
    let identity = image_identity(&bounded.image_path)?;
    // The admission ceiling must fit inside the verified payload volume, so
    // reservations stay meaningful in front of the OS-enforced bound.
    if sample.capacity_bytes < ceiling {
        return Err(HostError::Storage);
    }
    let receipt = BoundedReceipt {
        version: 1,
        backend: BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1.to_owned(),
        image_path: bounded.image_path.clone(),
        mount_point: bounded.mount_point.clone(),
        image_bytes: bounded.image_bytes,
        payload_ceiling_bytes: WireU64(ceiling),
        verified_capacity_bytes: WireU64(sample.capacity_bytes),
        identity,
    };
    let bytes = serde_json::to_vec(&receipt).map_err(|_| HostError::Protocol)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&bounded.receipt_path)
        .map_err(|_| HostError::Io)?;
    file.write_all(&bytes).map_err(|_| HostError::Io)?;
    file.sync_all().map_err(|_| HostError::Io)?;
    drop(file);
    fs::File::open(scratch)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| HostError::Io)?;
    bounded_descriptor(bounded, sample.capacity_bytes)
}

/// Opens an existing provisioned backing: the capacity proof must match the
/// declaration and the live mounted volume exactly, otherwise the bounded
/// mode refuses rather than reinterpreting the recorded store.
#[cfg(target_os = "macos")]
fn open_bounded(
    bounded: &RuntimeBoundedStorage,
    ceiling: u64,
) -> Result<BoundedBacking, HostError> {
    let bytes = read_protected(&bounded.receipt_path)?;
    let receipt: BoundedReceipt =
        serde_json::from_slice(&bytes).map_err(|_| HostError::Protocol)?;
    if receipt.version != 1
        || receipt.backend != BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1
        || receipt.image_path != bounded.image_path
        || receipt.mount_point != bounded.mount_point
        || receipt.image_bytes != bounded.image_bytes
        || receipt.payload_ceiling_bytes.0 != ceiling
        || bounded.image_path.symlink_metadata().is_err()
    {
        return Err(HostError::Storage);
    }
    let sample = sample_volume(&bounded.mount_point)?;
    let identity = image_identity(&bounded.image_path)?;
    if identity != receipt.identity
        || sample.capacity_bytes != receipt.verified_capacity_bytes.0
        || sample.capacity_bytes < ceiling
    {
        return Err(HostError::Storage);
    }
    bounded_descriptor(bounded, sample.capacity_bytes)
}

#[cfg(target_os = "macos")]
fn bounded_descriptor(
    bounded: &RuntimeBoundedStorage,
    verified_capacity_bytes: u64,
) -> Result<BoundedBacking, HostError> {
    let identity = image_identity(&bounded.image_path)?;
    BoundedBacking::verified(BoundedBackendDescriptor {
        backend: BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1.to_owned(),
        payload_root: bounded.mount_point.clone(),
        identity,
        verified_capacity_bytes,
    })
    .map_err(|_| HostError::Storage)
}

fn valid_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Statvfs-backed probe, limited to the owned canonical roots: the daemon
/// control root and, in bounded mode, the payload volume root.
#[derive(Debug)]
struct OwnedRootFreeSpace {
    roots: Vec<PathBuf>,
}

impl OwnedRootFreeSpace {
    fn for_roots(roots: Vec<PathBuf>) -> Self {
        Self { roots }
    }
}

impl FreeSpaceProbe for OwnedRootFreeSpace {
    fn available_bytes(&self, root: &Path) -> Result<Option<u64>, StorageError> {
        if !self.roots.iter().any(|owned| owned == root) {
            return Err(StorageError::InvalidInput(
                "free-space probe is limited to the owned storage roots",
            ));
        }
        let path = CString::new(root.as_os_str().as_bytes())
            .map_err(|_| StorageError::InvalidInput("storage root contains NUL"))?;
        let mut stat = MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: `path` is NUL-terminated and `stat` is writable storage for statvfs.
        let status = unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) };
        if status != 0 {
            return Err(StorageError::Io(
                "statvfs failed for owned storage root".into(),
            ));
        }
        // SAFETY: a zero return means statvfs initialized the complete struct.
        let stat = unsafe { stat.assume_init() };
        let bytes = u64::from(stat.f_bavail)
            .checked_mul(u64::from(stat.f_frsize))
            .ok_or(StorageError::Protocol("free-space byte count overflowed"))?;
        Ok(Some(bytes))
    }
}

/// Consumes one protected bootstrap and serves the actual daemon until stop.
/// The whole process runs under one nonrenewable bound counted from entry. See
/// `supervise_startup`. The replay marker is created exclusively before any storage
/// effect, and an existing marker refuses even when a copied bootstrap is presented again.
/// Failures after the control connection send an authenticated `failed` frame, bounded
/// by `EXIT_TOLERANCE`.
pub fn serve_bootstrap(path: &Path) -> Result<Stop, HostError> {
    let entry = sample_entry()?;
    let path = path.to_path_buf();
    supervise_startup(entry.instant, move |events| startup(&path, entry, events))
}

/// Progress from the startup worker to the process supervisor.
#[derive(Debug)]
enum Event {
    /// The owner window end, known once the bootstrap has been validated.
    Window(Instant),
    /// Readiness was written. The bound moves to the serve phase.
    Ready(Instant),
    /// The worker has finished with this result.
    Finished(Result<Stop, HostError>),
}

/// The supervisor's decision for one event or timeout.
#[derive(Debug)]
enum Step {
    Continue { deadline: Instant, ready: bool },
    Done(Result<Stop, HostError>),
}

/// Applies one event. The current monotonic time is checked first. A queued event that
/// arrives after the bound never changes readiness or extends the deadline, and a worker
/// that finishes after the bound before readiness is never promoted to a result.
fn step(deadline: Instant, ready: bool, event: Event, now: Instant) -> Step {
    let expired = if ready {
        Stop::Abandoned
    } else {
        Stop::StartupExpired
    };
    match event {
        Event::Window(_) if now >= deadline => Step::Done(Ok(expired)),
        Event::Window(end) => Step::Continue {
            deadline: deadline.min(end),
            ready,
        },
        Event::Ready(_) if now >= deadline => Step::Done(Ok(expired)),
        Event::Ready(_) if ready => Step::Done(Err(HostError::Protocol)),
        Event::Ready(end) => Step::Continue {
            deadline: end,
            ready: true,
        },
        Event::Finished(Err(HostError::Timeout)) if !ready => Step::Done(Ok(Stop::StartupExpired)),
        Event::Finished(_) if !ready && now >= deadline => Step::Done(Ok(Stop::StartupExpired)),
        Event::Finished(result) => Step::Done(result),
    }
}

/// Runs the whole process from entry under one nonrenewable bound. Before the bootstrap
/// is read, the only bound is the process ceiling from entry. Validation shrinks it to
/// the owner's window. Readiness moves it to the serve-phase bound. A blocked worker is
/// abandoned at that bound. Returning from here lets the binary exit, which ends any
/// blocked thread without releasing, finalizing or deleting durable state.
fn supervise_startup<F>(entry: Instant, body: F) -> Result<Stop, HostError>
where
    F: FnOnce(&mpsc::Sender<Event>) -> Result<Stop, HostError> + Send + 'static,
{
    let (events, received) = mpsc::channel::<Event>();
    let worker_events = events.clone();
    thread::Builder::new()
        .name("oxide-storage-startup".to_owned())
        .spawn(move || {
            let result = body(&worker_events);
            let _ = worker_events.send(Event::Finished(result));
        })
        .map_err(|_| HostError::Io)?;
    drop(events);
    let mut deadline = entry + MAX_LIFETIME;
    let mut ready = false;
    loop {
        let next = match received.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(event) => step(deadline, ready, event, Instant::now()),
            Err(mpsc::RecvTimeoutError::Timeout) => Step::Done(Ok(if ready {
                Stop::Abandoned
            } else {
                Stop::StartupExpired
            })),
            Err(mpsc::RecvTimeoutError::Disconnected) => Step::Done(Err(HostError::Protocol)),
        };
        match next {
            Step::Continue {
                deadline: next_deadline,
                ready: next_ready,
            } => {
                deadline = next_deadline;
                ready = next_ready;
            }
            Step::Done(result) => return result,
        }
    }
}

/// Startup from the bootstrap to the connected serve loop. Each blocking step checks the
/// window first, so an expired window stops before the next effect.
fn startup(path: &Path, entry: Entry, events: &mpsc::Sender<Event>) -> Result<Stop, HostError> {
    let mut bytes = read_protected(path)?;
    let digest = bootstrap_digest(&bytes);
    let bootstrap: RuntimeBootstrap =
        serde_json::from_slice(&bytes).map_err(|_| HostError::Protocol)?;
    let (plan, socket) = bootstrap.validate(entry, &digest)?;
    let window_end = plan.deadline;
    let _ = events.send(Event::Window(window_end));
    before(window_end)?;
    let marker = bootstrap
        .control
        .path
        .parent()
        .ok_or(HostError::Protocol)?
        .join(format!("runtime-{}", &digest[7..]));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&marker)
        .map_err(|_| HostError::OwnerRejected)?;
    file.write_all(b"oxide-storage-runtime-v1\n")
        .map_err(|_| HostError::Io)?;
    file.sync_all().map_err(|_| HostError::Io)?;
    fs::File::open(marker.parent().ok_or(HostError::Protocol)?)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| HostError::Io)?;
    before(window_end)?;
    fs::remove_file(path).map_err(|_| HostError::Io)?;
    bytes.fill(0);
    before(window_end)?;
    let mut control = UnixStream::connect(&bootstrap.control.path).map_err(|_| HostError::Io)?;
    let current = Endpoint::capture(bootstrap.control.path.clone())?;
    if current.device != bootstrap.control.device.0 || current.inode != bootstrap.control.inode.0 {
        return Err(HostError::Protocol);
    }
    fixture_stall(
        bootstrap.control.path.parent().ok_or(HostError::Protocol)?,
        "before-bind",
    );
    before(window_end)?;
    match run(&bootstrap, plan, socket, &mut control, events) {
        Ok(stop) => Ok(stop),
        Err(error) => {
            let code = match error {
                HostError::OwnerRejected => "refused",
                HostError::Storage => "storage",
                HostError::Timeout => "expired",
                HostError::Io => "io",
                HostError::Protocol | HostError::RunnerRefused => "protocol",
            };
            if let Ok(frame) = failed_frame(&bootstrap, &digest, code) {
                let _ = write_frame(&mut control, &frame);
            }
            Err(error)
        }
    }
}

/// Fails with `Timeout` once the window has ended.
fn before(deadline: Instant) -> Result<(), HostError> {
    if Instant::now() < deadline {
        Ok(())
    } else {
        Err(HostError::Timeout)
    }
}

/// Readiness needs at least `READY_MARGIN` of owner window left.
fn ready_allowed(deadline: Instant) -> bool {
    deadline.saturating_duration_since(Instant::now()) >= READY_MARGIN
}

fn run(
    bootstrap: &RuntimeBootstrap,
    plan: Plan,
    socket: PathBuf,
    control: &mut UnixStream,
    events: &mpsc::Sender<Event>,
) -> Result<Stop, HostError> {
    let allowed = bootstrap
        .admissions
        .iter()
        .map(|approved| RunEnvelope {
            binding: approved.binding.clone(),
            partition: approved.partition.clone(),
            admission_reference: approved.admission_reference.clone(),
            admission_credential: Credential::new(""),
        })
        .collect();
    let authority = Arc::new(
        QualificationAuthority::new(plan.authority_key.clone(), plan.scope.clone(), allowed)
            .and_then(|authority| {
                authority.with_ordinary_cargo_request_digests(
                    plan.ordinary_cargo_request_digests.clone(),
                )
            })
            .map_err(|_| HostError::Protocol)?,
    );
    let root = bootstrap.storage_root.clone();
    // The verified bounded capability is the only way slots leave the daemon
    // root. Provisioning happens after the replay marker, so a refused or
    // failed backing leaves no partial store behind.
    let backing = match plan.bounded.as_ref() {
        Some(bounded) => Some(
            open_or_provision_bounded(bounded, plan.budget.cap_bytes.0)
                .map_err(|_| HostError::Storage)?,
        ),
        None => None,
    };
    let mut probe_roots = vec![root.clone()];
    if let Some(backing) = backing.as_ref() {
        probe_roots.push(backing.payload_root().to_path_buf());
    }
    let daemon = Daemon::bind(
        DaemonConfig {
            root: root.clone(),
            budget: Budget {
                cap_bytes: plan.budget.cap_bytes.0,
                free_reserve_bytes: plan.budget.free_reserve_bytes.0,
            },
            backing,
        },
        authority.clone(),
        Arc::new(OwnedRootFreeSpace::for_roots(probe_roots)),
    )
    .map_err(|_| HostError::Storage)?;
    before(plan.deadline)?;
    let instance = daemon.storage_instance_id().to_owned();
    authority
        .install_instance(&instance)
        .map_err(|_| HostError::Protocol)?;
    let storage = Endpoint::capture(socket.clone())?;
    let mut frame = ready_body(&plan, &instance, &socket, &storage)?;
    let body_digest = canonical_digest(&frame)?;
    let mac = plan.control_key.sign(
        READY_DOMAIN,
        &[
            &plan.scope,
            &plan.bootstrap_digest,
            &plan.policy_digest,
            &instance,
            &body_digest,
        ],
    )?;
    frame["mac"] = json!(mac);
    fixture_stall(
        bootstrap.control.path.parent().ok_or(HostError::Protocol)?,
        "before-ready",
    );
    if !ready_allowed(plan.deadline) {
        return Err(HostError::Timeout);
    }
    // The write is bounded by the owner window, not by a fresh tolerance. A write that ends
    // after the window is not a readiness event.
    write_frame_until(control, &frame, plan.deadline)?;
    before(plan.deadline)?;
    // The serve-phase bound covers the stop grace and the bounded exit tolerance.
    let _ = events.send(Event::Ready(
        plan.deadline + plan.stop_grace + EXIT_TOLERANCE,
    ));

    // The main thread is the single authority on stop. The first reason from the
    // control reader or the deadline supervisor is forwarded to the storage thread.
    let (stop_sender, stop_receiver) = mpsc::channel::<Stop>();
    let (finished_sender, finished_receiver) = mpsc::channel::<()>();
    let supervisor = {
        let stop = stop_sender.clone();
        let deadline = plan.deadline;
        thread::Builder::new()
            .name("oxide-storage-deadline".to_owned())
            .spawn(move || supervise_deadline(&finished_receiver, deadline, &stop))
            .map_err(|_| HostError::Io)?
    };
    let reader_stream = control.try_clone().map_err(|_| HostError::Io)?;
    let context = ControlContext {
        key: plan.control_key.clone(),
        scope: plan.scope.clone(),
        instance: instance.clone(),
        bootstrap_digest: plan.bootstrap_digest.clone(),
    };
    {
        let stop = stop_sender.clone();
        thread::Builder::new()
            .name("oxide-storage-control".to_owned())
            .spawn(move || {
                let reason = read_commands(reader_stream, &context);
                let _ = stop.send(reason);
            })
            .map_err(|_| HostError::Io)?;
    }
    drop(stop_sender);

    // Storage runs on its own thread so that no in-flight operation can delay the stop path.
    let (daemon_sender, daemon_receiver) = mpsc::channel::<()>();
    let (served_sender, served_receiver) = mpsc::channel::<Result<(), StorageError>>();
    thread::Builder::new()
        .name("oxide-storage-serve".to_owned())
        .spawn(move || {
            let _ = served_sender.send(daemon.serve(daemon_receiver));
        })
        .map_err(|_| HostError::Io)?;

    let stop = stop_receiver.recv().map_err(|_| HostError::Protocol)?;
    let _ = daemon_sender.send(());
    match served_receiver.recv_timeout(plan.stop_grace) {
        Ok(served) => served.map_err(|_| HostError::Storage)?,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // The blocked operation is abandoned with the process. Nothing here
            // releases, finalizes or deletes durable state.
            return Ok(Stop::Abandoned);
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => return Err(HostError::Storage),
    }
    // Storage has stopped. Wake a control read that may still be blocked; the write side
    // stays open so an authenticated shutdown acknowledgement can still be sent.
    let _ = control.shutdown(Shutdown::Read);
    drop(finished_sender);
    supervisor.join().map_err(|_| HostError::Protocol)?;
    if let Stop::Shutdown(id) = &stop {
        let ack = stopped_frame(&plan, &instance, id)?;
        write_frame(control, &ack)?;
    }
    Ok(stop)
}

/// Sends the single lifetime stop when the nonrenewable deadline passes before the
/// storage session finishes. Control traffic never moves this deadline.
fn supervise_deadline(finished: &mpsc::Receiver<()>, deadline: Instant, stop: &mpsc::Sender<Stop>) {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if let Err(mpsc::RecvTimeoutError::Timeout) = finished.recv_timeout(remaining) {
        let _ = stop.send(Stop::Lifetime);
    }
}

struct ControlContext {
    key: MacKey,
    scope: String,
    instance: String,
    bootstrap_digest: String,
}

/// Reads bounded control frames until a verified shutdown, EOF or refusal. Each
/// blocked read waits at most `CONTROL_IO_TIMEOUT` and then resumes. The lifetime
/// is supervised separately, so no control traffic can move it.
fn read_commands(stream: UnixStream, context: &ControlContext) -> Stop {
    if stream.set_read_timeout(Some(CONTROL_IO_TIMEOUT)).is_err() {
        return Stop::ControlRefused;
    }
    read_frames(BufReader::new(stream), context)
}

/// Frame parser over any reader. Socket timeouts are set by `read_commands`.
fn read_frames<R: Read>(mut reader: BufReader<R>, context: &ControlContext) -> Stop {
    let mut pending = Vec::new();
    let mut aggregate = 0_usize;
    loop {
        let allowance = (MAX_FRAME_BYTES - pending.len()) as u64;
        let read = match (&mut reader)
            .take(allowance)
            .read_until(b'\n', &mut pending)
        {
            Ok(0) if pending.is_empty() => return Stop::ControlClosed,
            Ok(0) => return Stop::ControlRefused,
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue
            }
            Err(_) => return Stop::ControlRefused,
        };
        aggregate = aggregate.saturating_add(read);
        if aggregate > MAX_CONTROL_BYTES {
            return Stop::ControlRefused;
        }
        if pending.last() != Some(&b'\n') {
            if pending.len() >= MAX_FRAME_BYTES {
                return Stop::ControlRefused;
            }
            continue;
        }
        let frame = std::mem::take(&mut pending);
        return match verify_command(context, &frame) {
            Ok(id) => Stop::Shutdown(id),
            Err(_) => Stop::ControlRefused,
        };
    }
}

fn verify_command(context: &ControlContext, frame: &[u8]) -> Result<String, HostError> {
    let value: Value = serde_json::from_slice(frame).map_err(|_| HostError::Protocol)?;
    if value.as_object().map(|object| object.len()) != Some(4)
        || value["version"] != PROTOCOL_VERSION
        || value["operation"] != "shutdown"
    {
        return Err(HostError::Protocol);
    }
    let id = value["id"].as_str().ok_or(HostError::Protocol)?;
    valid_id(id)?;
    let mac = value["mac"].as_str().ok_or(HostError::Protocol)?;
    if !context.key.verify(
        SHUTDOWN_DOMAIN,
        &[
            &context.scope,
            &context.instance,
            &context.bootstrap_digest,
            id,
        ],
        mac,
    ) {
        return Err(HostError::OwnerRejected);
    }
    Ok(id.to_owned())
}

fn ready_body(
    plan: &Plan,
    instance: &str,
    socket: &Path,
    storage: &Endpoint,
) -> Result<Value, HostError> {
    let path = socket.to_str().ok_or(HostError::Protocol)?;
    Ok(json!({
        "version": PROTOCOL_VERSION,
        "operation": "ready",
        "scope": plan.scope,
        "bootstrap_digest": plan.bootstrap_digest,
        "policy_digest": plan.policy_digest,
        "storage_instance_id": instance,
        "storage": {
            "path": path,
            "device": WireU64(storage.device),
            "inode": WireU64(storage.inode),
        },
        "storage_protocol": STORAGE_PROTOCOL_VERSION,
        "control_protocol": PROTOCOL_VERSION,
        "budget": plan.budget,
        "expires_unix_seconds": WireU64(plan.expires_unix_seconds),
    }))
}

fn stopped_frame(plan: &Plan, instance: &str, id: &str) -> Result<Value, HostError> {
    let mac = plan.control_key.sign(
        STOPPED_DOMAIN,
        &[&plan.scope, instance, &plan.bootstrap_digest, id],
    )?;
    Ok(json!({"version": PROTOCOL_VERSION, "operation": "stopped", "id": id, "mac": mac}))
}

fn failed_frame(
    bootstrap: &RuntimeBootstrap,
    digest: &str,
    code: &str,
) -> Result<Value, HostError> {
    let key = MacKey::from_hex(&bootstrap.control_key_hex)?;
    let mac = key.sign(FAILED_DOMAIN, &[&bootstrap.scope, digest, code])?;
    Ok(
        json!({"version": PROTOCOL_VERSION, "operation": "failed", "scope": bootstrap.scope,
        "bootstrap_digest": digest, "code": code, "mac": mac}),
    )
}

/// Writes one whole frame, re-arming each partial write with the remaining window. A
/// frame cannot be held past `until`, and the timeout is passed in whole milliseconds.
fn write_frame_until(
    stream: &mut UnixStream,
    value: &Value,
    until: Instant,
) -> Result<(), HostError> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| HostError::Protocol)?;
    if bytes.len() >= MAX_FRAME_BYTES {
        return Err(HostError::Protocol);
    }
    bytes.push(b'\n');
    let mut written = 0;
    while written < bytes.len() {
        let remaining = until
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(HostError::Timeout)?;
        let millis = u64::try_from(remaining.as_millis())
            .unwrap_or(u64::MAX)
            .max(1);
        stream
            .set_write_timeout(Some(Duration::from_millis(millis)))
            .map_err(|_| HostError::Io)?;
        match stream.write(&bytes[written..]) {
            Ok(0) => return Err(HostError::Io),
            Ok(count) => written += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                return Err(HostError::Timeout)
            }
            Err(_) => return Err(HostError::Io),
        }
    }
    Ok(())
}

/// Test-only blocking point. It is compiled only with `private-qualification-fixture`,
/// which is unpublished and off by default. It blocks on an owned FIFO that the trusted
/// test creates in the owner's control directory. The bootstrap, environment and argv
/// cannot select or disable it. The test opens no writer, so the step blocks until the
/// supervisor expires the window.
#[cfg(feature = "private-qualification-fixture")]
fn fixture_stall(control_dir: &Path, point: &str) {
    let path = control_dir.join(format!("fixture-stall-{point}"));
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return;
    };
    if !metadata.file_type().is_fifo() {
        return;
    }
    eprintln!("fixture stall entered: {point}");
    let _ = OpenOptions::new().read(true).open(&path);
}

#[cfg(not(feature = "private-qualification-fixture"))]
fn fixture_stall(_control_dir: &Path, _point: &str) {}

fn write_frame(stream: &mut UnixStream, value: &Value) -> Result<(), HostError> {
    stream
        .set_write_timeout(Some(EXIT_TOLERANCE))
        .map_err(|_| HostError::Io)?;
    let mut bytes = serde_json::to_vec(value).map_err(|_| HostError::Protocol)?;
    if bytes.len() >= MAX_FRAME_BYTES {
        return Err(HostError::Protocol);
    }
    bytes.push(b'\n');
    stream.write_all(&bytes).map_err(|_| HostError::Io)?;
    stream.flush().map_err(|_| HostError::Io)
}

fn unix_now() -> Result<u64, HostError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| HostError::Protocol)?
        .as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_qualification_runtime_policy_digest_matches_storage_v2_vector() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../fixtures/storage-runtime-ready-v1.json"))
                .unwrap();
        let inputs = &fixture["policy_inputs"];
        let budget: RuntimeBudget = serde_json::from_value(inputs["budget"].clone()).unwrap();
        let grace: WireU64 = serde_json::from_value(inputs["stop_grace_seconds"].clone()).unwrap();
        let admissions: Vec<ApprovedSubject> =
            serde_json::from_value(inputs["admissions"].clone()).unwrap();
        let ordinary: Vec<String> =
            serde_json::from_value(inputs["ordinary_cargo_request_digests"].clone()).unwrap();
        assert_eq!(
            policy_digest(
                inputs["scope"].as_str().unwrap(),
                &budget,
                grace,
                &admissions,
                &ordinary,
                None,
            )
            .unwrap(),
            fixture["policy_digest"].as_str().unwrap()
        );
        let enabled = vec![format!("sha256:{}", "a".repeat(64))];
        assert_ne!(
            policy_digest(
                inputs["scope"].as_str().unwrap(),
                &budget,
                grace,
                &admissions,
                &enabled,
                None,
            )
            .unwrap(),
            fixture["policy_digest"].as_str().unwrap()
        );
    }

    #[test]
    fn private_qualification_runtime_wire_endpoint_keeps_u64_values_above_js_safe_range_exact() {
        let value =
            json!({"path":"/x","device":"18446744073709551615","inode":9_007_199_254_740_991_u64});
        let endpoint: RuntimeEndpoint = serde_json::from_value(value).unwrap();
        assert_eq!(endpoint.device.0, u64::MAX);
        assert_eq!(endpoint.inode.0, 9_007_199_254_740_991);
        let unsafe_number = json!({"path":"/x","device":9_007_199_254_740_992_u64,"inode":1});
        assert!(serde_json::from_value::<RuntimeEndpoint>(unsafe_number).is_err());
        let leading_zero = json!({"path":"/x","device":"01","inode":"1"});
        assert!(serde_json::from_value::<RuntimeEndpoint>(leading_zero).is_err());
    }

    fn context() -> ControlContext {
        ControlContext {
            key: MacKey::from_hex(&"5".repeat(64)).unwrap(),
            scope: "unit-scope".into(),
            instance: "a".repeat(64),
            bootstrap_digest: digest_of(b"unit bootstrap"),
        }
    }

    fn digest_of(bytes: &[u8]) -> String {
        bootstrap_digest(bytes)
    }

    fn command(context: &ControlContext, id: &str) -> Vec<u8> {
        let mut frame = shutdown_frame(
            &context.key,
            &context.scope,
            &context.instance,
            &context.bootstrap_digest,
            id,
        )
        .unwrap()
        .to_string();
        frame.push('\n');
        frame.into_bytes()
    }

    #[test]
    fn private_qualification_runtime_late_queued_events_never_move_readiness_or_extend() {
        let deadline = Instant::now() + Duration::from_secs(5);
        let late = deadline + Duration::from_millis(1);
        // A ready event that is processed after the bound is refused, not applied.
        assert!(matches!(
            step(
                deadline,
                false,
                Event::Ready(late + Duration::from_secs(60)),
                late
            ),
            Step::Done(Ok(Stop::StartupExpired))
        ));
        // A window event processed after the bound also expires instead of continuing.
        assert!(matches!(
            step(
                deadline,
                false,
                Event::Window(late + Duration::from_secs(60)),
                late
            ),
            Step::Done(Ok(Stop::StartupExpired))
        ));
        // A worker that finishes after the bound before readiness is never a success.
        assert!(matches!(
            step(
                deadline,
                false,
                Event::Finished(Ok(Stop::Shutdown("x".into()))),
                late
            ),
            Step::Done(Ok(Stop::StartupExpired))
        ));
        // Readiness in time moves the bound to the serve phase and never back.
        let serve = deadline + Duration::from_secs(12);
        match step(deadline, false, Event::Ready(serve), Instant::now()) {
            Step::Continue {
                deadline: next,
                ready,
            } => {
                assert!(ready);
                assert_eq!(next, serve);
            }
            other => panic!("ready in time must continue: {other:?}"),
        }
        // A second ready is a protocol failure, not an extension.
        assert!(matches!(
            step(
                serve,
                true,
                Event::Ready(serve + Duration::from_secs(60)),
                Instant::now()
            ),
            Step::Done(Err(HostError::Protocol))
        ));
    }

    #[test]
    fn private_qualification_runtime_bounded_ready_write_stops_at_the_owner_window() {
        // A write that cannot finish inside the window fails with Timeout and never reports
        // readiness. The peer never reads, so only the window can end the write.
        let (mut writer, _peer) = UnixStream::pair().unwrap();
        let big = serde_json::json!({"fill": "x".repeat(MAX_FRAME_BYTES - 64)});
        let until = Instant::now() + Duration::from_millis(200);
        let started = Instant::now();
        let mut result = Ok(());
        for _ in 0..64 {
            result = write_frame_until(&mut writer, &big, until);
            if result.is_err() {
                break;
            }
        }
        assert_eq!(result, Err(HostError::Timeout));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    /// Drives the frame parser from in-memory bytes. Socket timeouts are not used here.
    fn run_reader(bytes: Vec<u8>) -> Stop {
        read_frames(BufReader::new(std::io::Cursor::new(bytes)), &context())
    }

    fn entry_at(instant: Instant, wall_millis: u64) -> Entry {
        Entry {
            instant,
            wall: Duration::from_millis(wall_millis),
        }
    }

    #[test]
    fn private_qualification_runtime_fractional_entry_never_extends_the_owner_window() {
        let grace = Duration::from_secs(2);
        let instant = Instant::now();
        // Entry wall 199.8 against an owner end of 200 leaves 200 ms. The deadline is exact,
        // not a whole second later, and the published expiry is the whole second not past it.
        let (deadline, published) = session_window(entry_at(instant, 199_800), 200, grace).unwrap();
        assert_eq!(deadline, instant + Duration::from_millis(200));
        assert_eq!(published, 200);
        // Subsecond entries and a 1 ms remainder never exceed the owner's remaining time.
        let owner_end = Duration::from_secs(200);
        for wall in [199_000_u64, 199_001, 199_500, 199_999] {
            let entry = entry_at(instant, wall);
            let (deadline, published) = session_window(entry, 200, grace).unwrap();
            assert!(deadline <= instant + (owner_end - entry.wall));
            assert!(published <= 200);
            assert!(Duration::from_secs(published) <= entry.wall + (deadline - instant));
        }
        // A whole-second entry gets exactly the whole second that remains.
        let (deadline, _) = session_window(entry_at(instant, 199_000), 200, grace).unwrap();
        assert_eq!(deadline, instant + Duration::from_secs(1));
    }

    #[test]
    fn private_qualification_runtime_expired_and_unrepresentable_windows_refuse() {
        let grace = Duration::from_secs(2);
        let instant = Instant::now();
        // At, just past and far past the owner's end are all spent.
        assert!(session_window(entry_at(instant, 200_000), 200, grace).is_none());
        assert!(session_window(entry_at(instant, 200_001), 200, grace).is_none());
        assert!(session_window(entry_at(instant, 999_999_000), 200, grace).is_none());
        // A zero expiry is spent at any entry.
        assert!(session_window(entry_at(instant, 0), 0, grace).is_none());
    }

    #[test]
    fn private_qualification_runtime_window_cap_and_overflow_stay_exact() {
        let grace = Duration::from_secs(2);
        let instant = Instant::now();
        let cap = MAX_LIFETIME - grace - EXIT_TOLERANCE;
        // A long owner window takes the cap exactly, measured from entry.
        let entry = entry_at(instant, 1_000_000_000_000);
        let (deadline, published) = session_window(entry, 1_000_000_100_000, grace).unwrap();
        assert_eq!(deadline, instant + cap);
        assert_eq!(published, 1_000_000_000 + cap.as_secs());
        // The largest representable expiry takes the cap and does not overflow.
        let (capped, _) = session_window(entry_at(instant, 1_000_000), u64::MAX, grace).unwrap();
        assert_eq!(capped, instant + cap);
    }

    #[test]
    fn private_qualification_runtime_stopped_ack_refuses_unknown_fields() {
        let key = MacKey::from_hex(&"5".repeat(64)).unwrap();
        let scope = "unit-scope";
        let instance = "a".repeat(64);
        let bootstrap = digest_of(b"unit bootstrap");
        let mac = key
            .sign(STOPPED_DOMAIN, &[scope, &instance, &bootstrap, "stop-1"])
            .unwrap();
        let ack = json!({"version": 1, "operation": "stopped", "id": "stop-1", "mac": mac});
        assert!(verify_stopped(
            &key, scope, &instance, &bootstrap, "stop-1", &ack
        ));
        let mut extra = ack.clone();
        extra["unauthenticated"] = json!(true);
        assert!(!verify_stopped(
            &key, scope, &instance, &bootstrap, "stop-1", &extra
        ));
        let mut missing = ack.clone();
        missing.as_object_mut().unwrap().remove("id");
        assert!(!verify_stopped(
            &key, scope, &instance, &bootstrap, "stop-1", &missing
        ));
    }

    #[test]
    fn private_qualification_runtime_blocked_startup_is_abandoned_at_the_owner_window() {
        // A worker that blocks before readiness is abandoned at its window end, not after
        // its own sleep. The worker's later result is never observed.
        let entry = Instant::now();
        let started = Instant::now();
        let result = supervise_startup(entry, move |events| {
            let _ = events.send(Event::Window(Instant::now() + Duration::from_millis(300)));
            thread::sleep(Duration::from_secs(20));
            Ok(Stop::Lifetime)
        })
        .unwrap();
        assert_eq!(result, Stop::StartupExpired);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn private_qualification_runtime_ready_needs_the_margin_and_the_window_is_checked_before_effects(
    ) {
        let now = Instant::now();
        assert!(!ready_allowed(now + Duration::from_millis(500)));
        assert!(ready_allowed(now + Duration::from_secs(5)));
        assert!(before(now + Duration::from_secs(5)).is_ok());
        assert_eq!(
            before(now - Duration::from_millis(1)),
            Err(HostError::Timeout)
        );
    }

    #[test]
    fn private_qualification_runtime_worker_result_after_startup_expiry_is_not_observed() {
        // A worker that finishes after the window does not turn an expiry into a success.
        let entry = Instant::now();
        let result = supervise_startup(entry, move |events| {
            let _ = events.send(Event::Window(Instant::now() + Duration::from_millis(200)));
            thread::sleep(Duration::from_millis(600));
            Ok(Stop::Shutdown("late".into()))
        })
        .unwrap();
        assert_eq!(result, Stop::StartupExpired);
    }

    /// The same bytes the Node verifier checks. Rust authenticates them through the production path.
    #[test]
    fn private_qualification_runtime_independent_ready_vector_verifies_and_refuses_substitution() {
        let vector: Value =
            serde_json::from_str(include_str!("../../fixtures/storage-runtime-ready-v1.json"))
                .unwrap();
        let key = MacKey::from_hex(vector["key_hex"].as_str().unwrap()).unwrap();
        let expected =
            |scope: &str, bootstrap: &str, policy: &str, socket: &str| ReadyExpectation {
                scope: scope.into(),
                bootstrap_digest: bootstrap.into(),
                policy_digest: policy.into(),
                storage_socket: PathBuf::from(socket),
            };
        let scope = vector["scope"].as_str().unwrap();
        let bootstrap = vector["bootstrap_digest"].as_str().unwrap();
        let policy = vector["policy_digest"].as_str().unwrap();
        let socket = vector["storage_socket"].as_str().unwrap();
        let mut frame = vector["body"].clone();
        frame["mac"] = vector["mac"].clone();
        let ready = verify_ready_frame(&key, &expected(scope, bootstrap, policy, socket), &frame)
            .expect("independent vector authenticates");
        assert_eq!(ready.storage.device.0, u64::MAX);
        assert_eq!(ready.storage.inode.0, 9_007_199_254_740_993);
        let ok = expected(scope, bootstrap, policy, socket);
        let mut forged = frame.clone();
        forged["mac"] = json!(format!("hmac-sha256-v1:{}", "0".repeat(64)));
        assert!(verify_ready_frame(&key, &ok, &forged).is_err());
        let mut device = frame.clone();
        device["storage"]["device"] = json!("18446744073709551614");
        assert!(verify_ready_frame(&key, &ok, &device).is_err());
        let mut inode = frame.clone();
        inode["storage"]["inode"] = json!("9007199254740992");
        assert!(verify_ready_frame(&key, &ok, &inode).is_err());
        let mut instance = frame.clone();
        instance["storage_instance_id"] = json!("c".repeat(64));
        assert!(verify_ready_frame(&key, &ok, &instance).is_err());
        let mut budget = frame.clone();
        budget["budget"]["cap_bytes"] = json!(1);
        assert!(verify_ready_frame(&key, &ok, &budget).is_err());
        assert!(verify_ready_frame(
            &key,
            &expected("other-scope", bootstrap, policy, socket),
            &frame
        )
        .is_err());
        assert!(verify_ready_frame(
            &key,
            &expected(
                scope,
                &bootstrap_digest(b"replayed bootstrap"),
                policy,
                socket
            ),
            &frame
        )
        .is_err());
        assert!(verify_ready_frame(
            &key,
            &expected(
                scope,
                bootstrap,
                &format!("sha256:{}", "0".repeat(64)),
                socket
            ),
            &frame
        )
        .is_err());
        assert!(
            verify_ready_frame(&MacKey::from_hex(&"7".repeat(64)).unwrap(), &ok, &frame).is_err()
        );
    }

    #[test]
    fn private_qualification_runtime_control_reader_accepts_only_an_exact_authenticated_shutdown() {
        let context = context();
        assert_eq!(
            run_reader(command(&context, "stop-1")),
            Stop::Shutdown("stop-1".into())
        );
        let mut forged: Value = serde_json::from_slice(&command(&context, "stop-2")).unwrap();
        forged["mac"] = json!(format!("hmac-sha256-v1:{}", "0".repeat(64)));
        let forged = format!("{forged}\n").into_bytes();
        assert_eq!(run_reader(forged), Stop::ControlRefused);
        let other_scope = {
            let mut other = context;
            other.scope = "other-scope".into();
            command(&other, "stop-3")
        };
        assert_eq!(run_reader(other_scope), Stop::ControlRefused);
        let extra = format!(
            "{}\n",
            json!({"version":1,"operation":"shutdown","id":"stop-4","mac":"x","renew":true})
        )
        .into_bytes();
        assert_eq!(run_reader(extra), Stop::ControlRefused);
    }

    #[test]
    fn private_qualification_runtime_control_reader_bounds_frames_and_reports_eof_and_deadline() {
        let oversized = vec![b'x'; MAX_FRAME_BYTES + 1];
        assert_eq!(run_reader(oversized), Stop::ControlRefused);
        assert_eq!(run_reader(Vec::new()), Stop::ControlClosed);
        let partial = b"{\"version\":1".to_vec();
        assert_eq!(run_reader(partial), Stop::ControlRefused);
        // An elapsed deadline is reported by the supervisor, never by reading a frame.
        let (finished_sender, finished_receiver) = mpsc::channel::<()>();
        let (stop_sender, stop_receiver) = mpsc::channel::<Stop>();
        supervise_deadline(
            &finished_receiver,
            Instant::now() - Duration::from_secs(1),
            &stop_sender,
        );
        assert_eq!(stop_receiver.try_recv(), Ok(Stop::Lifetime));
        drop(finished_sender);
    }

    #[test]
    fn private_qualification_runtime_runtime_free_space_probe_is_limited_to_its_owned_root() {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let owned = base.join(format!("probe-owned-{}", std::process::id()));
        let other = base.join(format!("probe-other-{}", std::process::id()));
        fs::create_dir_all(&owned).unwrap();
        fs::create_dir_all(&other).unwrap();
        let probe = OwnedRootFreeSpace::for_roots(vec![owned.clone()]);
        let free = probe
            .available_bytes(&owned)
            .unwrap()
            .expect("known capacity");
        assert!(free > 0);
        assert!(probe.available_bytes(&other).is_err());
        fs::remove_dir_all(&owned).unwrap();
        fs::remove_dir_all(&other).unwrap();
    }
}
