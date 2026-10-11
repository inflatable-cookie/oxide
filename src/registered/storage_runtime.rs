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
    Budget, Credential, Daemon, DaemonConfig, FreeSpaceProbe, PartitionKey, RunBinding,
    RunEnvelope, StorageError,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::mem::MaybeUninit;
use std::net::Shutdown;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Protocol version shared by bootstrap, readiness, control and storage policy.
pub const PROTOCOL_VERSION: u32 = 1;
/// Nonrenewable monotonic ceiling. An owner window may be shorter, never longer.
pub const MAX_LIFETIME: Duration = Duration::from_secs(45 * 60);
/// Upper bound on the wait after a stop before an in-flight storage operation is abandoned.
pub const MAX_STOP_GRACE: Duration = Duration::from_secs(10);
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

/// Digest of the scope, budget and exact approved subjects. Excludes credentials.
pub fn policy_digest(
    scope: &str,
    budget: &RuntimeBudget,
    stop_grace_seconds: WireU64,
    admissions: &[ApprovedSubject],
) -> Result<String, HostError> {
    canonical_digest(&json!({
        "scope": scope,
        "storage_protocol": PROTOCOL_VERSION,
        "control_protocol": PROTOCOL_VERSION,
        "budget": serde_json::to_value(budget).map_err(|_| HostError::Protocol)?,
        "stop_grace_seconds": serde_json::to_value(stop_grace_seconds).map_err(|_| HostError::Protocol)?,
        "admissions": serde_json::to_value(admissions).map_err(|_| HostError::Protocol)?,
    }))
}

/// Parent-side check of a readiness frame. Verifies the control MAC over every
/// field, the bootstrap and policy identity, and the exact storage endpoint.
pub fn verify_ready(
    key: &MacKey,
    expectation: &ReadyExpectation,
    frame: &Value,
) -> Result<Ready, HostError> {
    let ready = verify_ready_frame(key, expectation, frame)?;
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
        || ready.storage_protocol != PROTOCOL_VERSION
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
    authority_key: MacKey,
    control_key: MacKey,
    deadline: Instant,
    stop_grace: Duration,
    expires_unix_seconds: u64,
}

/// Nonrenewable window measured from the entry instant and its Unix timestamp. The
/// owner's remaining seconds are counted from entry, so startup time is inside the window.
/// The ceiling leaves room for the stop grace. `None` means the window is already spent.
fn session_window(
    entry: Instant,
    entry_unix: u64,
    expires_unix: u64,
    stop_grace: Duration,
) -> Option<(Instant, u64)> {
    let remaining = Duration::from_secs(expires_unix.checked_sub(entry_unix)?);
    if remaining.is_zero() {
        return None;
    }
    let lifetime = remaining.min(MAX_LIFETIME - stop_grace);
    Some((entry + lifetime, entry_unix + lifetime.as_secs()))
}

impl RuntimeBootstrap {
    fn validate(
        &self,
        entry: Instant,
        entry_unix: u64,
        digest: &str,
    ) -> Result<(Plan, PathBuf), HostError> {
        if self.version != PROTOCOL_VERSION
            || self.storage_protocol != PROTOCOL_VERSION
            || self.control_protocol != PROTOCOL_VERSION
            || self.scope.is_empty()
            || self.scope.len() > MAX_SCOPE_BYTES
            || self.admissions.is_empty()
            || self.admissions.len() > MAX_ADMISSIONS
            || self.budget.cap_bytes.0 == 0
            || self.stop_grace_seconds.0 == 0
            || self.stop_grace_seconds.0 > MAX_STOP_GRACE.as_secs()
            || self.expires_unix_seconds.0 <= entry_unix
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
        let (deadline, expires_unix_seconds) =
            session_window(entry, entry_unix, self.expires_unix_seconds.0, stop_grace)
                .ok_or(HostError::Protocol)?;
        let plan = Plan {
            scope: self.scope.clone(),
            bootstrap_digest: digest.to_owned(),
            policy_digest: policy_digest(
                &self.scope,
                &self.budget,
                self.stop_grace_seconds,
                &self.admissions,
            )?,
            budget: self.budget,
            authority_key,
            control_key,
            deadline,
            stop_grace,
            expires_unix_seconds,
        };
        Ok((plan, self.storage_root.join(SOCKET_NAME)))
    }
}

/// Statvfs-backed probe, limited to the one owned canonical storage root.
#[derive(Debug)]
struct OwnedRootFreeSpace {
    root: PathBuf,
}

impl FreeSpaceProbe for OwnedRootFreeSpace {
    fn available_bytes(&self, root: &Path) -> Result<Option<u64>, StorageError> {
        if root != self.root {
            return Err(StorageError::InvalidInput(
                "free-space probe is limited to the owned storage root",
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
/// The replay marker is created exclusively before any storage effect. An
/// existing marker refuses even when a copied bootstrap is presented again.
/// Failures after the control connection send an authenticated `failed` frame.
pub fn serve_bootstrap(path: &Path) -> Result<Stop, HostError> {
    // The owner's window starts here, before the bootstrap is read. Reading, validation,
    // connect and startup all draw from it.
    let entry = Instant::now();
    let entry_unix = unix_now()?;
    let mut bytes = read_protected(path)?;
    let digest = bootstrap_digest(&bytes);
    let bootstrap: RuntimeBootstrap =
        serde_json::from_slice(&bytes).map_err(|_| HostError::Protocol)?;
    let (plan, socket) = bootstrap.validate(entry, entry_unix, &digest)?;
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
    fs::remove_file(path).map_err(|_| HostError::Io)?;
    bytes.fill(0);
    let mut control = UnixStream::connect(&bootstrap.control.path).map_err(|_| HostError::Io)?;
    let current = Endpoint::capture(bootstrap.control.path.clone())?;
    if current.device != bootstrap.control.device.0 || current.inode != bootstrap.control.inode.0 {
        return Err(HostError::Protocol);
    }
    match run(&bootstrap, plan, socket, &mut control) {
        Ok(stop) => Ok(stop),
        Err(error) => {
            let code = match error {
                HostError::OwnerRejected => "refused",
                HostError::Storage => "storage",
                HostError::Io | HostError::Timeout => "io",
                HostError::Protocol | HostError::RunnerRefused => "protocol",
            };
            if let Ok(frame) = failed_frame(&bootstrap, &digest, code) {
                let _ = write_frame(&mut control, &frame);
            }
            Err(error)
        }
    }
}

fn run(
    bootstrap: &RuntimeBootstrap,
    plan: Plan,
    socket: PathBuf,
    control: &mut UnixStream,
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
            .map_err(|_| HostError::Protocol)?,
    );
    let root = bootstrap.storage_root.clone();
    let daemon = Daemon::bind(
        DaemonConfig {
            root: root.clone(),
            budget: Budget {
                cap_bytes: plan.budget.cap_bytes.0,
                free_reserve_bytes: plan.budget.free_reserve_bytes.0,
            },
        },
        authority.clone(),
        Arc::new(OwnedRootFreeSpace { root }),
    )
    .map_err(|_| HostError::Storage)?;
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
    write_frame(control, &frame)?;

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
        "storage_protocol": PROTOCOL_VERSION,
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

fn write_frame(stream: &mut UnixStream, value: &Value) -> Result<(), HostError> {
    stream
        .set_write_timeout(Some(CONTROL_IO_TIMEOUT))
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

    /// Drives the frame parser from in-memory bytes. Socket timeouts are not used here.
    fn run_reader(bytes: Vec<u8>) -> Stop {
        read_frames(BufReader::new(std::io::Cursor::new(bytes)), &context())
    }

    #[test]
    fn private_qualification_runtime_window_is_counted_from_entry_not_after_startup() {
        let grace = Duration::from_secs(2);
        let entry = Instant::now() - Duration::from_secs(5);
        let entry_unix = unix_now().unwrap() - 5;
        let (deadline, expires) =
            session_window(entry, entry_unix, entry_unix + 10, grace).unwrap();
        // Five seconds of startup are inside the ten-second window, not added after it.
        assert_eq!(deadline, entry + Duration::from_secs(10));
        assert!(deadline < Instant::now() + Duration::from_secs(10));
        assert_eq!(expires, entry_unix + 10);
        // The 45-minute ceiling leaves room for the stop grace.
        let (capped, _) = session_window(entry, entry_unix, entry_unix + 100_000, grace).unwrap();
        assert_eq!(capped, entry + MAX_LIFETIME - grace);
        // An already spent window refuses.
        assert!(session_window(entry, entry_unix, entry_unix, grace).is_none());
        assert!(session_window(entry, entry_unix, entry_unix - 1, grace).is_none());
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
        let probe = OwnedRootFreeSpace {
            root: owned.clone(),
        };
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
