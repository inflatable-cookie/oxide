//! Inactive, qualification-only ordinary-Cargo attempt route.
//!
//! This route acquires the actual ordinary-Cargo storage API and delegates one
//! immutable command to a trusted consumer callback. It never starts Cargo or
//! supplies the registered Runner's graph/audit evidence.

use crate::registered::qualification::{
    canonical_digest, owner_uid, private_directory, read_protected, CallbackAuthenticator,
    Endpoint, MacKey,
};
use crate::registered::runner_host::{OwnerReceiptVerifier, WireU64};
use crate::registered::storage_runtime::bootstrap_digest;
use crate::storage::{
    AcquireResult, CaptureReceipt, CargoReuseState, Client, ContainmentEvidence,
    CooperativeCompletionEvidence, Credential, LookupResult, NoSpawnEvidence,
    OrdinaryCargoAcquireResult, OrdinaryCargoFinalizationReceipt,
    OrdinaryCargoFinalizeRequest as StorageOrdinaryCargoFinalizeRequest, OrdinaryCargoPartition,
    OrdinaryCargoRequest, PartitionKey, ProcessIdentityEvidence, RunEnvelope, SettlementEvidence,
    SlotState, StorageError, TerminalOutcome, TerminalRecord,
};
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Map, Number, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub const ORDINARY_CARGO_ATTEMPT_VERSION: u32 = 1;
pub const ORDINARY_CARGO_ATTEMPT_OPERATION: &str = "ordinary-cargo-attempt-v1";
pub const MAX_ATTEMPT_LIFETIME: Duration = Duration::from_secs(45 * 60);
pub const MAX_ATTEMPT_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_ATTEMPT_AGGREGATE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CAPTURE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_INVOCATION_BYTES: usize = 48 * 1024;
const CALLBACK_POLICY: &str = "ordinary-cargo-attempt-v1";
const RESERVED_ENVIRONMENT: [&str; 5] = [
    "CARGO_BUILD_BUILD_DIR",
    "CARGO_TARGET_DIR",
    "TMPDIR",
    "TMP",
    "TEMP",
];

/// Reserved operation identities for submit, capture, no-submit, and finalize.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OrdinaryCargoOperationIds {
    pub submit: String,
    pub capture: String,
    pub not_submitted: String,
    pub finalize: String,
}

/// Complete command and source identity accepted before the storage acquire.
/// `environment` maps each relevant name to `Some(value)` or explicit absence.
#[derive(Clone)]
pub struct OrdinaryCargoInvocation {
    pub request: OrdinaryCargoRequest,
    pub branch_id: String,
    pub source_content_digest: String,
    pub selector: String,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub environment: BTreeMap<String, Option<String>>,
    pub target_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub capture_dir: PathBuf,
    pub expected_outputs: Vec<String>,
    pub operation_ids: OrdinaryCargoOperationIds,
}

impl fmt::Debug for OrdinaryCargoInvocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OrdinaryCargoInvocation")
            .field("binding", &self.request.acquire.envelope.binding)
            .field("root_id", &self.request.root.consumer_root_id)
            .field("allocation_epoch", &self.request.root.allocation_epoch)
            .field("source_content_digest", &self.source_content_digest)
            .field("selector", &self.selector)
            .field("argv_count", &self.argv.len())
            .field("cwd", &self.cwd)
            .field("environment", &"[redacted]")
            .field("target_dir", &self.target_dir)
            .field("temp_dir", &self.temp_dir)
            .field("capture_dir", &self.capture_dir)
            .field("operation_ids", &self.operation_ids)
            .finish()
    }
}

/// Cargo command delegated to the consumer's existing host-submit path.
#[derive(Serialize)]
pub struct OrdinaryCargoCommand {
    pub selector: String,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub environment: BTreeMap<String, Option<String>>,
    pub target_dir: PathBuf,
    pub temp_dir: PathBuf,
    pub capture_dir: PathBuf,
}

impl fmt::Debug for OrdinaryCargoCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OrdinaryCargoCommand")
            .field("selector", &self.selector)
            .field("argv_count", &self.argv.len())
            .field("cwd", &self.cwd)
            .field("environment", &"[redacted]")
            .field("target_dir", &self.target_dir)
            .field("temp_dir", &self.temp_dir)
            .field("capture_dir", &self.capture_dir)
            .finish()
    }
}

/// Immutable identity committed before the one submit callback. The lease token
/// is deliberately absent and stays in the Rust process.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AcquiredCargoIdentity {
    pub policy_version: u32,
    pub installation_epoch: String,
    pub acceptance_reference: String,
    pub input_boundary_digest: String,
    pub source_root: PathBuf,
    pub root_id: String,
    pub allocation_epoch: String,
    pub partition: String,
    pub reuse_state: String,
    pub state: String,
    pub storage_instance_id: String,
    pub slot_id: String,
    pub generation: String,
    pub lease_id: String,
    pub slot_path: PathBuf,
    pub reused: bool,
    pub consumer: String,
    pub project: String,
    pub component: String,
    pub workflow: String,
    pub task_id: String,
    pub workspace_id: String,
    pub branch_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub commit: String,
    pub source_tree_digest: String,
    pub source_content_digest: String,
    pub cargo_partition: PartitionKey,
}

/// Absolute, nonrenewable budget. Both durations are constructor-bounded by the
/// brief's 45-minute engineering ceiling.
#[derive(Clone, Copy, Debug)]
pub struct OrdinaryCargoAttemptBudget {
    deadline: Instant,
    callback_timeout: Duration,
}

impl OrdinaryCargoAttemptBudget {
    pub fn new(remaining: Duration, callback_timeout: Duration) -> Result<Self, AttemptError> {
        Self::from_entry(Instant::now(), remaining, callback_timeout)
    }

    fn from_entry(
        entry: Instant,
        remaining: Duration,
        callback_timeout: Duration,
    ) -> Result<Self, AttemptError> {
        if remaining.is_zero()
            || remaining > MAX_ATTEMPT_LIFETIME
            || callback_timeout.is_zero()
            || callback_timeout > remaining
            || callback_timeout > MAX_ATTEMPT_LIFETIME
        {
            return Err(AttemptError::InvalidInput);
        }
        Ok(Self {
            deadline: entry
                .checked_add(remaining)
                .ok_or(AttemptError::InvalidInput)?,
            callback_timeout,
        })
    }

    fn check(self) -> Result<(), AttemptError> {
        if Instant::now() < self.deadline {
            Ok(())
        } else {
            Err(AttemptError::Timeout)
        }
    }

    fn callback_deadline(self) -> Result<Instant, AttemptError> {
        self.check()?;
        Ok(Instant::now()
            .checked_add(self.callback_timeout)
            .ok_or(AttemptError::Timeout)?
            .min(self.deadline))
    }
}

/// Authenticated callback exchange. Implementations must use one bounded,
/// versioned private channel and classify pre-delivery separately from loss.
trait OrdinaryCargoCallbacks: Send {
    fn exchange(&mut self, request: Value, deadline: Instant) -> Result<Value, CallbackFailure>;
}

/// Constructor-installed storage, callback authentication and dispatch policy.
/// Task invocation data can select an approved body, but cannot replace these
/// owners or extend the absolute deadline.
pub struct OrdinaryCargoAttemptRunner {
    client: Client,
    storage_root: PathBuf,
    callback_root: PathBuf,
    callbacks: Box<dyn OrdinaryCargoCallbacks>,
    callback_authenticator: CallbackAuthenticator,
    budget: OrdinaryCargoAttemptBudget,
}

impl fmt::Debug for OrdinaryCargoAttemptRunner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OrdinaryCargoAttemptRunner")
            .field("storage_root", &self.storage_root)
            .field("budget", &self.budget)
            .field("callback_authenticator", &"[installed]")
            .finish_non_exhaustive()
    }
}

impl OrdinaryCargoAttemptRunner {
    pub fn new(
        client: Client,
        storage_root: PathBuf,
        parent_endpoint: Endpoint,
        callback_authenticator: CallbackAuthenticator,
        budget: OrdinaryCargoAttemptBudget,
    ) -> Result<Self, AttemptError> {
        validate_private_directory(&storage_root)?;
        let callback_root = parent_endpoint
            .path
            .parent()
            .ok_or(AttemptError::InvalidInput)?
            .to_path_buf();
        private_directory(&callback_root).map_err(|_| AttemptError::InvalidInput)?;
        check_endpoint(&parent_endpoint)?;
        if paths_overlap(&storage_root, &callback_root) {
            return Err(AttemptError::InvalidInput);
        }
        Ok(Self {
            client,
            storage_root,
            callback_root,
            callbacks: Box::new(UnixCallbacks::new(parent_endpoint)),
            callback_authenticator,
            budget,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallbackFailure {
    /// The request frame was not written to the consumer at all.
    NotDelivered,
    /// Delivery or the consumer's result is ambiguous; ownership must remain held.
    Ambiguous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingPhase {
    Acquire,
    Submit,
    Capture,
    Finalize,
    Uncertain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OrdinaryCargoAttemptResult {
    Finalized(OrdinaryCargoFinalizationReceipt),
    Refused,
    Held(PendingPhase),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttemptError {
    InvalidInput,
    Protocol,
    Timeout,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CallbackReply {
    version: u32,
    method: String,
    id: String,
    launch_digest: String,
    acquired_digest: String,
    command_digest: String,
    payload: Value,
    receipt: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SubmitReply {
    status: SubmitStatus,
    run: Option<OrdinaryRunIdentity>,
}

struct AttemptHandoff {
    invocation: OrdinaryCargoInvocation,
    acquired: OrdinaryCargoAcquireResult,
    tuple: AcquiredCargoIdentity,
    tuple_digest: String,
    command_digest: String,
    launch_digest: String,
    command: Value,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SubmitStatus {
    Started,
    NotStarted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct OrdinaryRunIdentity {
    host_run_id: String,
    consumer: String,
    project: String,
    component: String,
    workflow: String,
    task_id: String,
    workspace_id: String,
    branch_id: String,
    run_id: String,
    attempt_id: String,
    commit: String,
    source_tree_digest: String,
    source_content_digest: String,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CaptureReply {
    run: OrdinaryRunIdentity,
    outcome: CapturedOutcome,
    terminal_credential: String,
    capture: CapturedOutput,
    completion: CapturedCompletion,
    settlement_credential: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CapturedOutcome {
    Passed,
    Failed,
    Cancelled,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CapturedOutput {
    reference: String,
    digest: String,
    host_run_id: String,
    run_id: String,
    attempt_id: String,
    credential: String,
    outputs: Vec<CapturedOutputEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CapturedOutputEntry {
    role: String,
    digest: String,
    bytes: WireU64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CapturedCompletion {
    lease_id: String,
    slot_id: String,
    slot_path: String,
    temp_dir: String,
    capture_dir: String,
    workflow: String,
    workflow_definition_digest: String,
    repository_head: String,
    fixture_source_digest: String,
    host_run_id: String,
    run_id: String,
    attempt_id: String,
    processes: Vec<ProcessIdentityEvidence>,
    free_space_samples: Vec<WireU64>,
    exit_code: i32,
    stdout_digest: String,
    stderr_digest: String,
    capture_reference: String,
    capture_digest: String,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct NotSubmittedReply {
    status: String,
    submission_state: String,
    writer_state: String,
    closure_reference: String,
    run: OrdinaryRunIdentity,
    settlement_credential: String,
}

/// Run one accepted invocation through acquire, one submit callback, complete
/// capture, and authenticated ordinary finalization. It never executes `argv`.
pub fn run_attempt(
    mut runner: OrdinaryCargoAttemptRunner,
    invocation: OrdinaryCargoInvocation,
) -> Result<OrdinaryCargoAttemptResult, AttemptError> {
    let deadline = runner.budget.deadline;
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("oxide-ordinary-cargo-attempt".to_owned())
        .spawn(move || {
            let result = run_attempt_inner(&mut runner, invocation);
            let _ = sender.send(result);
        })
        .map_err(|_| AttemptError::Protocol)?;
    match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(Err(AttemptError::Timeout)) => {
            Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Uncertain))
        }
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Uncertain))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(AttemptError::Protocol),
    }
}

fn run_attempt_inner(
    runner: &mut OrdinaryCargoAttemptRunner,
    invocation: OrdinaryCargoInvocation,
) -> Result<OrdinaryCargoAttemptResult, AttemptError> {
    let client = &runner.client;
    let storage_root = &runner.storage_root;
    let callback_root = &runner.callback_root;
    let callbacks = runner.callbacks.as_mut();
    let callback_authenticator = &runner.callback_authenticator;
    let budget = runner.budget;
    budget.check()?;
    validate_private_directory(storage_root)?;
    if paths_overlap(storage_root, callback_root)
        || paths_overlap(storage_root, &invocation.request.root.source_root)
        || paths_overlap(callback_root, &invocation.request.root.source_root)
    {
        return Err(AttemptError::InvalidInput);
    }
    let request_digest = validate_invocation(&invocation)?;
    for path in [
        invocation.target_dir.as_path(),
        invocation.temp_dir.as_path(),
        invocation.capture_dir.as_path(),
    ] {
        if paths_overlap(path, storage_root) || paths_overlap(path, callback_root) {
            return Err(AttemptError::InvalidInput);
        }
    }
    let acquired = match acquire_exact(client, &invocation.request, budget)? {
        AcquireState::Leased(acquired) => acquired,
        AcquireState::Refused => return Ok(OrdinaryCargoAttemptResult::Refused),
        AcquireState::Held => return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Acquire)),
    };
    budget.check()?;
    let command = command_for(&invocation, &acquired.lease)?;
    let tuple = acquired_identity(storage_root, &invocation, &acquired)?;
    let tuple_digest =
        canonical_digest(&serde_json::to_value(&tuple).map_err(|_| AttemptError::Protocol)?)
            .map_err(|_| AttemptError::Protocol)?;
    let command_value = serde_json::to_value(&command).map_err(|_| AttemptError::Protocol)?;
    let command_digest = canonical_digest(&command_value).map_err(|_| AttemptError::Protocol)?;
    let launch_digest = canonical_digest(&json!({
        "operation": ORDINARY_CARGO_ATTEMPT_OPERATION,
        "request_digest": request_digest,
        "branch_id": invocation.branch_id,
        "source_content_digest": invocation.source_content_digest,
        "selector": command.selector,
        "argv": command.argv,
        "cwd": command.cwd,
        "environment": command.environment,
        "target_dir": command.target_dir,
        "temp_dir": command.temp_dir,
        "capture_dir": command.capture_dir,
        "expected_outputs": invocation.expected_outputs,
        "operation_ids": invocation.operation_ids,
        "acquired": tuple,
    }))
    .map_err(|_| AttemptError::Protocol)?;
    let submit_id = invocation.operation_ids.submit.clone();
    let submit_request = callback_request(
        "ordinary_cargo.submit",
        &submit_id,
        &launch_digest,
        &tuple,
        &command_value,
        &tuple_digest,
        &command_digest,
        &invocation,
    );
    let submit_deadline = budget.callback_deadline()?;
    let submit_response = match callbacks.exchange(submit_request, submit_deadline) {
        Ok(value) => value,
        Err(CallbackFailure::Ambiguous) => {
            return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Submit));
        }
        Err(CallbackFailure::NotDelivered) => {
            return finish_not_submitted(
                client,
                AttemptHandoff {
                    invocation,
                    acquired,
                    tuple,
                    tuple_digest,
                    command_digest,
                    launch_digest,
                    command: command_value.clone(),
                },
                callbacks,
                callback_authenticator,
                budget,
            );
        }
    };
    if Instant::now() >= submit_deadline {
        return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Submit));
    }
    budget.check()?;
    let submit_reply: CallbackReply = parse_callback_reply(submit_response)?;
    if !verify_callback(
        &submit_reply,
        "ordinary_cargo.submit",
        &submit_id,
        &launch_digest,
        &tuple_digest,
        &command_digest,
        callback_authenticator,
    ) {
        return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Submit));
    }
    let submit: SubmitReply = match serde_json::from_value(submit_reply.payload) {
        Ok(reply) => reply,
        Err(_) => return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Submit)),
    };
    match submit.status {
        SubmitStatus::NotStarted if submit.run.is_none() => finish_not_submitted(
            client,
            AttemptHandoff {
                invocation,
                acquired,
                tuple,
                tuple_digest,
                command_digest,
                launch_digest,
                command: command_value.clone(),
            },
            callbacks,
            callback_authenticator,
            budget,
        ),
        SubmitStatus::Started => {
            let Some(run) = submit.run else {
                return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Submit));
            };
            if !matches_run(&run, &invocation) {
                return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Submit));
            }
            budget.check()?;
            let mut capture_request = callback_request(
                "ordinary_cargo.capture",
                &invocation.operation_ids.capture,
                &launch_digest,
                &tuple,
                &command_value,
                &tuple_digest,
                &command_digest,
                &invocation,
            );
            capture_request["run_identity"] = json!(run);
            let capture_deadline = budget.callback_deadline()?;
            let capture_response = match callbacks.exchange(capture_request, capture_deadline) {
                Ok(value) => value,
                Err(_) => return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Capture)),
            };
            if Instant::now() >= capture_deadline {
                return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Capture));
            }
            budget.check()?;
            let capture_reply: CallbackReply = parse_callback_reply(capture_response)?;
            if !verify_callback(
                &capture_reply,
                "ordinary_cargo.capture",
                &invocation.operation_ids.capture,
                &launch_digest,
                &tuple_digest,
                &command_digest,
                callback_authenticator,
            ) {
                return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Capture));
            }
            let captured: CaptureReply = match serde_json::from_value(capture_reply.payload) {
                Ok(reply) => reply,
                Err(_) => return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Capture)),
            };
            let Some(evidence) =
                capture_evidence(&invocation, &acquired.lease, &tuple, &run, captured)?
            else {
                return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Capture));
            };
            finalize(client, invocation, acquired.lease, evidence, budget)
        }
        _ => Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Submit)),
    }
}

enum AcquireState {
    Leased(OrdinaryCargoAcquireResult),
    Refused,
    Held,
}

fn acquire_exact(
    client: &Client,
    request: &OrdinaryCargoRequest,
    budget: OrdinaryCargoAttemptBudget,
) -> Result<AcquireState, AttemptError> {
    budget.check()?;
    match client.acquire_ordinary_cargo(request.clone()) {
        Ok(result) => return verify_acquire_barrier(client, result, request, budget),
        Err(
            StorageError::Refused(_)
            | StorageError::Unauthorized
            | StorageError::InvalidInput(_)
            | StorageError::Conflict,
        ) => return Ok(AcquireState::Refused),
        Err(StorageError::Io(_) | StorageError::Protocol(_)) => {}
    }
    budget.check()?;
    match client.lookup_ordinary_cargo(request.clone()) {
        Ok(LookupResult::Absent) => {}
        Ok(LookupResult::Present(_) | LookupResult::Uncertain(_)) | Err(_) => {
            return Ok(AcquireState::Held);
        }
    }
    budget.check()?;
    match client.acquire_ordinary_cargo(request.clone()) {
        Ok(result) => verify_acquire_barrier(client, result, request, budget),
        Err(
            StorageError::Refused(_)
            | StorageError::Unauthorized
            | StorageError::InvalidInput(_)
            | StorageError::Conflict,
        ) => Ok(AcquireState::Refused),
        Err(StorageError::Io(_) | StorageError::Protocol(_)) => {
            let _ = client.lookup_ordinary_cargo(request.clone());
            Ok(AcquireState::Held)
        }
    }
}

fn validate_acquire_result(
    result: OrdinaryCargoAcquireResult,
    request: &OrdinaryCargoRequest,
) -> Result<AcquireState, AttemptError> {
    if result.policy_version != request.acceptance.policy_version
        || result.root != request.root
        || result.partition != request.partition
        || result.reuse_state != CargoReuseState::Pending
        || result.lease.state != SlotState::Leased
        || result.lease.token.is_none()
        || result.lease.slot_path.is_none()
    {
        return Ok(AcquireState::Held);
    }
    Ok(AcquireState::Leased(result))
}

/// Cross-check the authenticated acquire reply against the access-free lookup
/// and durable inventory before exposing the acquired identity to the callback.
fn verify_acquire_barrier(
    client: &Client,
    result: OrdinaryCargoAcquireResult,
    request: &OrdinaryCargoRequest,
    budget: OrdinaryCargoAttemptBudget,
) -> Result<AcquireState, AttemptError> {
    let AcquireState::Leased(result) = validate_acquire_result(result, request)? else {
        return Ok(AcquireState::Held);
    };
    budget.check()?;
    let lookup = match client.lookup_ordinary_cargo(request.clone()) {
        Ok(LookupResult::Present(identity)) => identity,
        Ok(LookupResult::Absent | LookupResult::Uncertain(_)) | Err(_) => {
            return Ok(AcquireState::Held)
        }
    };
    let lease = &result.lease;
    if lookup.policy_version != result.policy_version
        || lookup.root != result.root
        || lookup.partition != result.partition
        || lookup.reuse_state != CargoReuseState::Pending
        || lookup.lease.lease_id != lease.lease_id
        || lookup.lease.slot_id != lease.slot_id
        || lookup.lease.storage_instance_id != lease.storage_instance_id
        || lookup.lease.generation != lease.generation
        || lookup.lease.state != SlotState::Leased
    {
        return Ok(AcquireState::Held);
    }
    budget.check()?;
    let inventory = match client.inventory(&request.acquire.acquire_id) {
        Ok(inventory) => inventory,
        Err(_) => return Ok(AcquireState::Held),
    };
    let Some(slot) = inventory
        .slots
        .iter()
        .find(|slot| slot.slot_id == lease.slot_id)
    else {
        return Ok(AcquireState::Held);
    };
    if slot.state != SlotState::Leased
        || slot.owner.as_ref() != Some(&request.acquire.envelope.binding)
        || slot.cargo_reuse_state != Some(CargoReuseState::Pending)
        || slot.ordinary_root.as_ref() != Some(&request.root)
        || slot.ordinary_partition.as_ref() != Some(&request.partition)
        || slot.partition != request.acquire.envelope.partition
    {
        return Ok(AcquireState::Held);
    }
    budget.check()?;
    Ok(AcquireState::Leased(result))
}

pub(super) fn validate_invocation(
    invocation: &OrdinaryCargoInvocation,
) -> Result<String, AttemptError> {
    let binding = &invocation.request.acquire.envelope.binding;
    if invocation.request.acceptance.policy_version != 1
        || invocation.request.acquire.envelope.partition.path_identity
            != "ordinary-cargo-storage-v1"
        || invocation.request.root.source_root != invocation.cwd
        || binding.commit.is_empty()
        || !valid_digest(&binding.source_tree_digest)
        || !valid_digest(&invocation.source_content_digest)
        || invocation.selector.is_empty()
        || invocation.selector.len() > 4096
        || invocation.argv.is_empty()
        || invocation.argv.len() > 256
        || invocation.environment.len() > 256
        || invocation.expected_outputs.is_empty()
        || invocation.expected_outputs.len() > 32
        || !valid_text(&invocation.branch_id)
        || !valid_text(&invocation.request.root.consumer_root_id)
        || !valid_text(&invocation.request.root.allocation_epoch)
        || !valid_text(&invocation.request.acceptance.installation_epoch)
        || !valid_text(&invocation.request.acceptance.acceptance_reference)
        || !valid_id(&invocation.operation_ids.submit)
        || !valid_id(&invocation.operation_ids.capture)
        || !valid_id(&invocation.operation_ids.not_submitted)
        || !valid_id(&invocation.operation_ids.finalize)
    {
        return Err(AttemptError::InvalidInput);
    }
    let ids = [
        invocation.request.acquire.acquire_id.as_str(),
        invocation.operation_ids.submit.as_str(),
        invocation.operation_ids.capture.as_str(),
        invocation.operation_ids.not_submitted.as_str(),
        invocation.operation_ids.finalize.as_str(),
    ];
    if ids.iter().any(|id| !valid_id(id))
        || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
        || invocation.argv.iter().any(|arg| arg.len() > 16 * 1024)
        || invocation.environment.iter().any(|(name, value)| {
            name.is_empty()
                || name.len() > 256
                || name
                    .bytes()
                    .any(|byte| !(byte.is_ascii_alphanumeric() || byte == b'_'))
                || value.as_ref().is_some_and(|value| value.len() > 16 * 1024)
        })
        || RESERVED_ENVIRONMENT.iter().any(|name| {
            invocation
                .environment
                .get(*name)
                .is_some_and(Option::is_some)
        })
    {
        return Err(AttemptError::InvalidInput);
    }
    let binding = &invocation.request.acquire.envelope.binding;
    if [
        &binding.consumer,
        &binding.project,
        &binding.component,
        &binding.workflow,
        &binding.task_id,
        &binding.workspace_id,
        &binding.run_id,
        &binding.attempt_id,
        &binding.commit,
    ]
    .iter()
    .any(|value| !valid_text(value.as_str()))
        || !valid_digest(&invocation.request.acceptance.input_boundary_digest)
    {
        return Err(AttemptError::InvalidInput);
    }
    match &invocation.request.partition {
        OrdinaryCargoPartition::OrdinaryTest
            if invocation.request.acquire.envelope.partition.profile != "dev" =>
        {
            return Err(AttemptError::InvalidInput);
        }
        OrdinaryCargoPartition::FullDebug
            if invocation.request.acquire.envelope.partition.profile != "full-debug" =>
        {
            return Err(AttemptError::InvalidInput);
        }
        OrdinaryCargoPartition::Other(_) => return Err(AttemptError::InvalidInput),
        _ => {}
    }
    validate_owned_source_directory(&invocation.request.root.source_root)?;
    for path in [
        invocation.target_dir.as_path(),
        invocation.temp_dir.as_path(),
        invocation.capture_dir.as_path(),
    ] {
        validate_private_directory(path)?;
    }
    if invocation.request.root.source_root != invocation.cwd
        || invocation
            .request
            .root
            .source_root
            .canonicalize()
            .map_err(|_| AttemptError::InvalidInput)?
            != invocation.request.root.source_root
    {
        return Err(AttemptError::InvalidInput);
    }
    let private_paths = [
        invocation.target_dir.as_path(),
        invocation.temp_dir.as_path(),
        invocation.capture_dir.as_path(),
    ];
    if private_paths.iter().enumerate().any(|(index, left)| {
        private_paths[index + 1..]
            .iter()
            .any(|right| left.starts_with(right) || right.starts_with(left))
    }) || private_paths.iter().any(|path| {
        path.starts_with(&invocation.request.root.source_root)
            || invocation.request.root.source_root.starts_with(path)
    }) {
        return Err(AttemptError::InvalidInput);
    }
    let expected_roles: BTreeSet<&str> = invocation
        .expected_outputs
        .iter()
        .map(String::as_str)
        .collect();
    if expected_roles.len() != invocation.expected_outputs.len()
        || expected_roles.iter().any(|role| {
            !matches!(
                *role,
                "source-archive" | "cargo-log" | "test-result" | "binary" | "full-debug"
            )
        })
    {
        return Err(AttemptError::InvalidInput);
    }
    let digest =
        crate::registered::qualification::ordinary_cargo_request_digest(&invocation.request)
            .map_err(|_| AttemptError::InvalidInput)?;
    let boundary = ordinary_cargo_input_boundary_digest(invocation)?;
    if boundary != invocation.request.acceptance.input_boundary_digest {
        return Err(AttemptError::InvalidInput);
    }
    let encoded_boundary = ordinary_cargo_input_boundary(invocation)?;
    if serde_json::to_vec(&encoded_boundary)
        .map_err(|_| AttemptError::InvalidInput)?
        .len()
        > MAX_INVOCATION_BYTES
    {
        return Err(AttemptError::InvalidInput);
    }
    Ok(digest)
}

/// Canonical accepted command/source/root boundary to store as the ordinary
/// request's `input_boundary_digest` before its credential is minted.
pub fn ordinary_cargo_input_boundary_digest(
    invocation: &OrdinaryCargoInvocation,
) -> Result<String, AttemptError> {
    let boundary = ordinary_cargo_input_boundary(invocation)?;
    canonical_digest(&boundary).map_err(|_| AttemptError::InvalidInput)
}

fn ordinary_cargo_input_boundary(
    invocation: &OrdinaryCargoInvocation,
) -> Result<Value, AttemptError> {
    let binding = &invocation.request.acquire.envelope.binding;
    Ok(json!({
        "binding": binding,
        "root": {
            "source_root": invocation.request.root.source_root,
            "root_id": invocation.request.root.consumer_root_id,
            "allocation_epoch": invocation.request.root.allocation_epoch,
        },
        "ordinary_partition": invocation.request.partition.label(),
        "cargo_partition": invocation.request.acquire.envelope.partition,
        "required_bytes": invocation.request.acquire.required_bytes.to_string(),
        "branch_id": invocation.branch_id,
        "source_content_digest": invocation.source_content_digest,
        "policy_version": invocation.request.acceptance.policy_version,
        "installation_epoch": invocation.request.acceptance.installation_epoch,
        "acceptance_reference": invocation.request.acceptance.acceptance_reference,
        "selector": invocation.selector,
        "argv": invocation.argv,
        "cwd": invocation.cwd,
        "environment": invocation.environment,
        "target_dir": invocation.target_dir,
        "temp_dir": invocation.temp_dir,
        "capture_dir": invocation.capture_dir,
        "expected_outputs": invocation.expected_outputs,
    }))
}

/// Callback HMAC context installed in the protected process bootstrap.
pub fn ordinary_cargo_callback_context(
    scope: &str,
    request_digest: &str,
) -> Result<String, AttemptError> {
    if !valid_text(scope) || !valid_digest(request_digest) {
        return Err(AttemptError::InvalidInput);
    }
    canonical_digest(&json!({
        "scope": scope,
        "operation": ORDINARY_CARGO_ATTEMPT_OPERATION,
        "request_digest": request_digest,
        "storage_protocol": crate::storage::STORAGE_PROTOCOL_VERSION,
    }))
    .map_err(|_| AttemptError::InvalidInput)
}

fn validate_private_directory(path: &Path) -> Result<(), AttemptError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| AttemptError::InvalidInput)?;
    if !metadata.is_dir()
        || metadata.uid() != owner_uid()
        || metadata.mode() & 0o7777 != 0o700
        || fs::canonicalize(path).map_err(|_| AttemptError::InvalidInput)? != path
    {
        return Err(AttemptError::InvalidInput);
    }
    Ok(())
}

fn validate_owned_source_directory(path: &Path) -> Result<(), AttemptError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| AttemptError::InvalidInput)?;
    if !metadata.is_dir()
        || metadata.uid() != owner_uid()
        || metadata.mode() & 0o022 != 0
        || fs::canonicalize(path).map_err(|_| AttemptError::InvalidInput)? != path
    {
        return Err(AttemptError::InvalidInput);
    }
    Ok(())
}

fn command_for(
    invocation: &OrdinaryCargoInvocation,
    acquired: &AcquireResult,
) -> Result<OrdinaryCargoCommand, AttemptError> {
    let slot = acquired.slot_path.as_ref().ok_or(AttemptError::Protocol)?;
    let mut environment = invocation.environment.clone();
    environment.insert("CARGO_BUILD_BUILD_DIR".to_owned(), Some(path_text(slot)?));
    environment.insert(
        "CARGO_TARGET_DIR".to_owned(),
        Some(path_text(&invocation.target_dir)?),
    );
    for name in ["TMPDIR", "TMP", "TEMP"] {
        environment.insert(name.to_owned(), Some(path_text(&invocation.temp_dir)?));
    }
    Ok(OrdinaryCargoCommand {
        selector: invocation.selector.clone(),
        argv: invocation.argv.clone(),
        cwd: invocation.cwd.clone(),
        environment,
        target_dir: invocation.target_dir.clone(),
        temp_dir: invocation.temp_dir.clone(),
        capture_dir: invocation.capture_dir.clone(),
    })
}

fn acquired_identity(
    storage_root: &Path,
    invocation: &OrdinaryCargoInvocation,
    acquired: &OrdinaryCargoAcquireResult,
) -> Result<AcquiredCargoIdentity, AttemptError> {
    let lease = &acquired.lease;
    let slot_path = validate_acquired_slot_path(storage_root, lease)?;
    let binding = &invocation.request.acquire.envelope.binding;
    Ok(AcquiredCargoIdentity {
        policy_version: acquired.policy_version,
        source_root: acquired.root.source_root.clone(),
        root_id: acquired.root.consumer_root_id.clone(),
        allocation_epoch: acquired.root.allocation_epoch.clone(),
        installation_epoch: invocation.request.acceptance.installation_epoch.clone(),
        acceptance_reference: invocation.request.acceptance.acceptance_reference.clone(),
        input_boundary_digest: invocation.request.acceptance.input_boundary_digest.clone(),
        partition: acquired.partition.label().to_owned(),
        reuse_state: "pending".to_owned(),
        state: "leased".to_owned(),
        storage_instance_id: lease.storage_instance_id.clone(),
        slot_id: lease.slot_id.clone(),
        generation: lease.generation.clone(),
        lease_id: lease.lease_id.clone(),
        slot_path,
        reused: lease.reused,
        consumer: binding.consumer.clone(),
        project: binding.project.clone(),
        component: binding.component.clone(),
        workflow: binding.workflow.clone(),
        task_id: binding.task_id.clone(),
        workspace_id: binding.workspace_id.clone(),
        branch_id: invocation.branch_id.clone(),
        run_id: binding.run_id.clone(),
        attempt_id: binding.attempt_id.clone(),
        commit: binding.commit.clone(),
        source_tree_digest: binding.source_tree_digest.clone(),
        source_content_digest: invocation.source_content_digest.clone(),
        cargo_partition: invocation.request.acquire.envelope.partition.clone(),
    })
}

pub(super) fn validate_acquired_slot_path(
    storage_root: &Path,
    lease: &AcquireResult,
) -> Result<PathBuf, AttemptError> {
    let slot_path = lease.slot_path.as_ref().ok_or(AttemptError::Protocol)?;
    let expected_slot = storage_root.join("slots").join(&lease.slot_id);
    if slot_path != &expected_slot
        || slot_path
            .canonicalize()
            .map_err(|_| AttemptError::Protocol)?
            != *slot_path
    {
        return Err(AttemptError::Protocol);
    }
    validate_private_directory(slot_path)?;
    Ok(slot_path.clone())
}

fn callback_request(
    method: &str,
    id: &str,
    launch_digest: &str,
    acquired: &AcquiredCargoIdentity,
    command: &Value,
    acquired_digest: &str,
    command_digest: &str,
    invocation: &OrdinaryCargoInvocation,
) -> Value {
    json!({
        "version": ORDINARY_CARGO_ATTEMPT_VERSION,
        "operation": ORDINARY_CARGO_ATTEMPT_OPERATION,
        "method": method,
        "id": id,
        "launch_digest": launch_digest,
        "acquired_digest": acquired_digest,
        "command_digest": command_digest,
        "acquired": acquired,
        "command": command,
        "source": {
            "consumer": invocation.request.acquire.envelope.binding.consumer,
            "project": invocation.request.acquire.envelope.binding.project,
            "component": invocation.request.acquire.envelope.binding.component,
            "workflow": invocation.request.acquire.envelope.binding.workflow,
            "task_id": invocation.request.acquire.envelope.binding.task_id,
            "workspace_id": invocation.request.acquire.envelope.binding.workspace_id,
            "branch_id": invocation.branch_id,
            "run_id": invocation.request.acquire.envelope.binding.run_id,
            "attempt_id": invocation.request.acquire.envelope.binding.attempt_id,
            "commit": invocation.request.acquire.envelope.binding.commit,
            "source_tree_digest": invocation.request.acquire.envelope.binding.source_tree_digest,
            "source_content_digest": invocation.source_content_digest,
        },
        "expected_outputs": invocation.expected_outputs,
    })
}

fn parse_callback_reply(value: Value) -> Result<CallbackReply, AttemptError> {
    serde_json::from_value(value).map_err(|_| AttemptError::Protocol)
}

fn verify_callback(
    reply: &CallbackReply,
    method: &str,
    id: &str,
    launch_digest: &str,
    acquired_digest: &str,
    command_digest: &str,
    authenticator: &CallbackAuthenticator,
) -> bool {
    if reply.version != ORDINARY_CARGO_ATTEMPT_VERSION
        || reply.method != method
        || reply.id != id
        || reply.launch_digest != launch_digest
        || reply.acquired_digest != acquired_digest
        || reply.command_digest != command_digest
    {
        return false;
    }
    let Ok(result_digest) = canonical_digest(&json!({
        "method": reply.method,
        "id": reply.id,
        "launch_digest": reply.launch_digest,
        "acquired_digest": reply.acquired_digest,
        "command_digest": reply.command_digest,
        "payload": reply.payload,
    })) else {
        return false;
    };
    authenticator.verify(
        method,
        id,
        launch_digest,
        CALLBACK_POLICY,
        &result_digest,
        &reply.receipt,
    )
}

fn matches_run(run: &OrdinaryRunIdentity, invocation: &OrdinaryCargoInvocation) -> bool {
    let binding = &invocation.request.acquire.envelope.binding;
    valid_text(&run.host_run_id)
        && run.consumer == binding.consumer
        && run.project == binding.project
        && run.component == binding.component
        && run.workflow == binding.workflow
        && run.task_id == binding.task_id
        && run.workspace_id == binding.workspace_id
        && run.branch_id == invocation.branch_id
        && run.run_id == binding.run_id
        && run.attempt_id == binding.attempt_id
        && run.commit == binding.commit
        && run.source_tree_digest == binding.source_tree_digest
        && run.source_content_digest == invocation.source_content_digest
}

fn capture_evidence(
    invocation: &OrdinaryCargoInvocation,
    acquired: &AcquireResult,
    tuple: &AcquiredCargoIdentity,
    run: &OrdinaryRunIdentity,
    captured: CaptureReply,
) -> Result<Option<SettlementEvidence>, AttemptError> {
    if captured.run != *run
        || !matches_run(&captured.run, invocation)
        || captured.capture.host_run_id != run.host_run_id
        || captured.capture.run_id != run.run_id
        || captured.capture.attempt_id != run.attempt_id
        || captured.completion.lease_id != tuple.lease_id
        || captured.completion.slot_id != tuple.slot_id
        || captured.completion.slot_path != path_text(&tuple.slot_path)?
        || captured.completion.temp_dir != path_text(&invocation.temp_dir)?
        || captured.completion.capture_dir != path_text(&invocation.capture_dir)?
        || captured.completion.workflow != invocation.request.acquire.envelope.binding.workflow
        || captured.completion.repository_head != run.commit
        || captured.completion.fixture_source_digest != run.source_content_digest
        || captured.completion.host_run_id != run.host_run_id
        || captured.completion.run_id != run.run_id
        || captured.completion.attempt_id != run.attempt_id
        || captured.completion.capture_reference != captured.capture.reference
        || captured.completion.capture_digest != captured.capture.digest
        || !valid_id(&captured.capture.reference)
        || !valid_digest(&captured.capture.digest)
        || !valid_digest(&captured.completion.workflow_definition_digest)
        || !valid_digest(&captured.completion.stdout_digest)
        || !valid_digest(&captured.completion.stderr_digest)
        || captured.completion.processes.is_empty()
        || captured.completion.processes.len() > 8
        || captured
            .completion
            .processes
            .iter()
            .any(|process| process.pid == 0 || !valid_digest(&process.start_identity))
        || captured.completion.free_space_samples.is_empty()
        || captured.completion.free_space_samples.len() > 16
        || captured.completion.stdout_digest.len() > 256
        || captured.completion.stderr_digest.len() > 256
        || !complete_outputs(&invocation.expected_outputs, &captured.capture.outputs)
        || matches!(captured.outcome, CapturedOutcome::Cancelled)
        || (captured.outcome == CapturedOutcome::Passed && captured.completion.exit_code != 0)
        || (captured.outcome == CapturedOutcome::Failed && captured.completion.exit_code == 0)
    {
        return Ok(None);
    }
    let outcome = match captured.outcome {
        CapturedOutcome::Passed => TerminalOutcome::Passed,
        CapturedOutcome::Failed => TerminalOutcome::Failed,
        CapturedOutcome::Cancelled => return Ok(None),
    };
    let token = acquired.token.as_ref().ok_or(AttemptError::Protocol)?;
    let mut completion = CooperativeCompletionEvidence {
        lease_id: tuple.lease_id.clone(),
        slot_id: tuple.slot_id.clone(),
        slot_path: path_text(&tuple.slot_path)?,
        temp_dir: path_text(&invocation.temp_dir)?,
        capture_dir: path_text(&invocation.capture_dir)?,
        workflow: captured.completion.workflow,
        workflow_definition_digest: captured.completion.workflow_definition_digest,
        repository_head: run.commit.clone(),
        fixture_source_digest: run.source_content_digest.clone(),
        host_run_id: run.host_run_id.clone(),
        run_id: run.run_id.clone(),
        attempt_id: run.attempt_id.clone(),
        processes: captured.completion.processes,
        free_space_samples: captured
            .completion
            .free_space_samples
            .into_iter()
            .map(|sample| sample.0)
            .collect(),
        exit_code: captured.completion.exit_code,
        stdout_digest: captured.completion.stdout_digest,
        stderr_digest: captured.completion.stderr_digest,
        capture_reference: captured.capture.reference.clone(),
        capture_digest: captured.capture.digest.clone(),
        signature: String::new(),
    };
    completion.sign(token);
    Ok(Some(SettlementEvidence {
        binding: invocation.request.acquire.envelope.binding.clone(),
        consumer_credential: Credential::new(captured.settlement_credential),
        terminal: Some(TerminalRecord {
            run_id: run.run_id.clone(),
            outcome,
            credential: Credential::new(captured.terminal_credential),
        }),
        capture: Some(CaptureReceipt {
            reference: captured.capture.reference,
            digest: captured.capture.digest,
            host_run_id: captured.capture.host_run_id,
            run_id: captured.capture.run_id,
            attempt_id: captured.capture.attempt_id,
            credential: Credential::new(captured.capture.credential),
        }),
        containment: ContainmentEvidence::CooperativeWorkflow(completion),
    }))
}

fn complete_outputs(expected: &[String], outputs: &[CapturedOutputEntry]) -> bool {
    if outputs.len() != expected.len() {
        return false;
    }
    let mut total = 0u64;
    let mut roles = BTreeSet::new();
    for output in outputs {
        if !valid_digest(&output.digest)
            || !roles.insert(output.role.as_str())
            || !expected.iter().any(|role| role == &output.role)
        {
            return false;
        }
        let Some(bytes) = total.checked_add(output.bytes.0) else {
            return false;
        };
        total = bytes;
    }
    total <= MAX_CAPTURE_BYTES && expected.iter().all(|role| roles.contains(role.as_str()))
}

fn finish_not_submitted(
    client: &Client,
    handoff: AttemptHandoff,
    callbacks: &mut dyn OrdinaryCargoCallbacks,
    authenticator: &CallbackAuthenticator,
    budget: OrdinaryCargoAttemptBudget,
) -> Result<OrdinaryCargoAttemptResult, AttemptError> {
    let AttemptHandoff {
        invocation,
        acquired,
        tuple,
        tuple_digest,
        command_digest,
        launch_digest,
        command,
    } = handoff;
    budget.check()?;
    let binding = &invocation.request.acquire.envelope.binding;
    let run_identity = OrdinaryRunIdentity {
        host_run_id: String::new(),
        consumer: binding.consumer.clone(),
        project: binding.project.clone(),
        component: binding.component.clone(),
        workflow: binding.workflow.clone(),
        task_id: binding.task_id.clone(),
        workspace_id: binding.workspace_id.clone(),
        branch_id: invocation.branch_id.clone(),
        run_id: binding.run_id.clone(),
        attempt_id: binding.attempt_id.clone(),
        commit: binding.commit.clone(),
        source_tree_digest: binding.source_tree_digest.clone(),
        source_content_digest: invocation.source_content_digest.clone(),
    };
    let mut request = callback_request(
        "ordinary_cargo.not_submitted",
        &invocation.operation_ids.not_submitted,
        &launch_digest,
        &tuple,
        &command,
        &tuple_digest,
        &command_digest,
        &invocation,
    );
    request["no_submit_claim"] = json!({
        "run_identity": run_identity,
        "submission_state": "never_began",
        "writer_state": "none",
    });
    let callback_deadline = budget.callback_deadline()?;
    let response = match callbacks.exchange(request, callback_deadline) {
        Ok(value) => value,
        Err(_) => return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Submit)),
    };
    if Instant::now() >= callback_deadline {
        return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Submit));
    }
    budget.check()?;
    let reply: CallbackReply = parse_callback_reply(response)?;
    if !verify_callback(
        &reply,
        "ordinary_cargo.not_submitted",
        &invocation.operation_ids.not_submitted,
        &launch_digest,
        &tuple_digest,
        &command_digest,
        authenticator,
    ) {
        return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Submit));
    }
    let evidence_reply: NotSubmittedReply = match serde_json::from_value(reply.payload) {
        Ok(value) => value,
        Err(_) => return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Submit)),
    };
    if !valid_not_submitted(&evidence_reply, &run_identity) {
        return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Submit));
    }
    let token = acquired
        .lease
        .token
        .as_ref()
        .ok_or(AttemptError::Protocol)?;
    let Some(not_spawned) = NoSpawnEvidence::new(&acquired.lease, binding, token) else {
        return Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Submit));
    };
    let evidence = SettlementEvidence {
        binding: binding.clone(),
        consumer_credential: Credential::new(evidence_reply.settlement_credential),
        terminal: None,
        capture: None,
        containment: ContainmentEvidence::NotSpawned(not_spawned),
    };
    finalize(client, invocation, acquired.lease, evidence, budget)
}

fn valid_not_submitted(reply: &NotSubmittedReply, expected: &OrdinaryRunIdentity) -> bool {
    reply.status == "not_submitted"
        && reply.submission_state == "never_began"
        && reply.writer_state == "none"
        && valid_id(&reply.closure_reference)
        && reply.run == *expected
        && reply.settlement_credential.starts_with("hmac-bundle-v1:")
}

fn finalize(
    client: &Client,
    invocation: OrdinaryCargoInvocation,
    lease: AcquireResult,
    evidence: SettlementEvidence,
    budget: OrdinaryCargoAttemptBudget,
) -> Result<OrdinaryCargoAttemptResult, AttemptError> {
    budget.check()?;
    let request = StorageOrdinaryCargoFinalizeRequest {
        request: invocation.request,
        lease_id: lease.lease_id.clone(),
        token: lease.token.clone(),
        finalize_id: invocation.operation_ids.finalize,
        evidence,
    };
    match client.finalize_ordinary_cargo(request.clone()) {
        Ok(receipt) => {
            if budget.check().is_err() {
                Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Finalize))
            } else {
                Ok(OrdinaryCargoAttemptResult::Finalized(receipt))
            }
        }
        Err(
            StorageError::Refused(_)
            | StorageError::Unauthorized
            | StorageError::InvalidInput(_)
            | StorageError::Conflict,
        ) => Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Finalize)),
        Err(StorageError::Io(_) | StorageError::Protocol(_)) => {
            budget.check()?;
            match client.lookup_finalize_ordinary_cargo(request.clone()) {
                Ok(LookupResult::Present(receipt)) if budget.check().is_ok() => {
                    Ok(OrdinaryCargoAttemptResult::Finalized(receipt))
                }
                Ok(LookupResult::Present(_)) => {
                    Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Finalize))
                }
                Ok(LookupResult::Absent) => {
                    budget.check()?;
                    match client.finalize_ordinary_cargo(request.clone()) {
                        Ok(receipt) => Ok(OrdinaryCargoAttemptResult::Finalized(receipt)),
                        Err(StorageError::Io(_) | StorageError::Protocol(_)) => {
                            if budget.check().is_err() {
                                return Ok(OrdinaryCargoAttemptResult::Held(
                                    PendingPhase::Finalize,
                                ));
                            }
                            match client.lookup_finalize_ordinary_cargo(request.clone()) {
                                Ok(LookupResult::Present(receipt)) if budget.check().is_ok() => {
                                    Ok(OrdinaryCargoAttemptResult::Finalized(receipt))
                                }
                                _ => Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Finalize)),
                            }
                        }
                        Err(_) => Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Finalize)),
                    }
                }
                Ok(LookupResult::Uncertain(_)) | Err(_) => {
                    Ok(OrdinaryCargoAttemptResult::Held(PendingPhase::Finalize))
                }
            }
        }
    }
}

fn path_text(path: &Path) -> Result<String, AttemptError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or(AttemptError::InvalidInput)
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && !value.bytes().any(|byte| byte.is_ascii_control())
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttemptBootstrap {
    version: u32,
    operation: String,
    scope: String,
    remaining_ms: WireU64,
    callback_timeout_ms: WireU64,
    storage: AttemptEndpoint,
    parent: AttemptEndpoint,
    callback_key_hex: String,
    callback_context: String,
    invocation: InvocationWire,
}

/// Lossless TypeScript-facing endpoint identity; large filesystem numbers are
/// encoded as decimal strings by `WireU64`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttemptEndpoint {
    path: PathBuf,
    device: WireU64,
    inode: WireU64,
}

impl AttemptEndpoint {
    fn into_endpoint(self) -> Endpoint {
        Endpoint {
            path: self.path,
            device: self.device.0,
            inode: self.inode.0,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InvocationWire {
    acquire: AcquireWire,
    root: RootWire,
    partition: String,
    acceptance: AcceptanceWire,
    branch_id: String,
    source_content_digest: String,
    selector: String,
    argv: Vec<String>,
    cwd: PathBuf,
    environment: BTreeMap<String, Option<String>>,
    target_dir: PathBuf,
    temp_dir: PathBuf,
    capture_dir: PathBuf,
    expected_outputs: Vec<String>,
    operation_ids: OrdinaryCargoOperationIdsWire,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AcquireWire {
    envelope: RunEnvelope,
    acquire_id: String,
    required_bytes: WireU64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RootWire {
    source_root: PathBuf,
    consumer_root_id: String,
    allocation_epoch: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptanceWire {
    policy_version: u32,
    installation_epoch: String,
    acceptance_reference: String,
    input_boundary_digest: String,
    credential: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OrdinaryCargoOperationIdsWire {
    submit: String,
    capture: String,
    not_submitted: String,
    finalize: String,
}

impl InvocationWire {
    fn into_invocation(self) -> Result<OrdinaryCargoInvocation, AttemptError> {
        let partition = match self.partition.as_str() {
            "ordinary-test" => OrdinaryCargoPartition::OrdinaryTest,
            "full-debug" => OrdinaryCargoPartition::FullDebug,
            _ => return Err(AttemptError::InvalidInput),
        };
        Ok(OrdinaryCargoInvocation {
            request: OrdinaryCargoRequest {
                acquire: crate::storage::AcquireRequest {
                    envelope: self.acquire.envelope,
                    acquire_id: self.acquire.acquire_id,
                    required_bytes: self.acquire.required_bytes.0,
                },
                root: crate::storage::StrictPhysicalCargoRoot {
                    source_root: self.root.source_root,
                    consumer_root_id: self.root.consumer_root_id,
                    allocation_epoch: self.root.allocation_epoch,
                },
                partition,
                acceptance: crate::storage::OrdinaryCargoAcceptance {
                    policy_version: self.acceptance.policy_version,
                    installation_epoch: self.acceptance.installation_epoch,
                    acceptance_reference: self.acceptance.acceptance_reference,
                    input_boundary_digest: self.acceptance.input_boundary_digest,
                    credential: Credential::new(self.acceptance.credential),
                },
            },
            branch_id: self.branch_id,
            source_content_digest: self.source_content_digest,
            selector: self.selector,
            argv: self.argv,
            cwd: self.cwd,
            environment: self.environment,
            target_dir: self.target_dir,
            temp_dir: self.temp_dir,
            capture_dir: self.capture_dir,
            expected_outputs: self.expected_outputs,
            operation_ids: OrdinaryCargoOperationIds {
                submit: self.operation_ids.submit,
                capture: self.operation_ids.capture,
                not_submitted: self.operation_ids.not_submitted,
                finalize: self.operation_ids.finalize,
            },
        })
    }
}

impl fmt::Debug for AttemptBootstrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OrdinaryCargoAttemptBootstrap([private])")
    }
}

/// Consume one protected bootstrap and serve one ordinary-Cargo attempt.
/// Process startup is the monotonic clock origin; timeout exits leave any lease
/// pending and never infer no-writer completion.
pub fn serve_bootstrap(path: &Path) -> Result<OrdinaryCargoAttemptResult, AttemptError> {
    let entry = Instant::now();
    let mut bytes = read_protected(path).map_err(|_| AttemptError::InvalidInput)?;
    let bootstrap_digest = bootstrap_digest(&bytes);
    let bootstrap: AttemptBootstrap = parse_json_without_duplicates(&bytes)?;
    let invocation = bootstrap.invocation.into_invocation()?;
    let remaining = Duration::from_millis(bootstrap.remaining_ms.0);
    let callback_timeout = Duration::from_millis(bootstrap.callback_timeout_ms.0);
    let budget = OrdinaryCargoAttemptBudget::from_entry(entry, remaining, callback_timeout)?;
    if bootstrap.version != ORDINARY_CARGO_ATTEMPT_VERSION
        || bootstrap.operation != ORDINARY_CARGO_ATTEMPT_OPERATION
        || bootstrap.scope.is_empty()
        || bootstrap.scope.len() > 4096
        || bootstrap.callback_context.len() > 4096
        || bootstrap.callback_key_hex == ""
        || bootstrap.parent.path == bootstrap.storage.path
    {
        return Err(AttemptError::InvalidInput);
    }
    let request_digest = validate_invocation(&invocation)?;
    let expected_context = ordinary_cargo_callback_context(&bootstrap.scope, &request_digest)?;
    if bootstrap.callback_context != expected_context {
        return Err(AttemptError::InvalidInput);
    }
    let storage_endpoint = bootstrap.storage.into_endpoint();
    let parent_endpoint = bootstrap.parent.into_endpoint();
    check_endpoint(&storage_endpoint)?;
    check_endpoint(&parent_endpoint)?;
    let marker_parent = parent_endpoint
        .path
        .parent()
        .ok_or(AttemptError::InvalidInput)?;
    private_directory(marker_parent).map_err(|_| AttemptError::InvalidInput)?;
    let marker_path = marker_parent.join(format!("attempt-{}", &bootstrap_digest[7..]));
    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&marker_path)
        .map_err(|_| AttemptError::InvalidInput)?;
    marker
        .write_all(b"oxide-ordinary-cargo-attempt-v1\n")
        .map_err(|_| AttemptError::Protocol)?;
    marker.sync_all().map_err(|_| AttemptError::Protocol)?;
    fs::File::open(marker_parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| AttemptError::Protocol)?;
    fs::remove_file(path).map_err(|_| AttemptError::Protocol)?;
    bytes.fill(0);
    let mut callback_key_bytes = bootstrap.callback_key_hex.into_bytes();
    let callback_key = std::str::from_utf8(&callback_key_bytes)
        .ok()
        .and_then(|value| MacKey::from_hex(value).ok());
    callback_key_bytes.fill(0);
    let callback_key = callback_key.ok_or(AttemptError::InvalidInput)?;
    let authenticator = CallbackAuthenticator::new(callback_key, bootstrap.callback_context)
        .map_err(|_| AttemptError::InvalidInput)?;
    let storage_root = storage_endpoint
        .path
        .parent()
        .ok_or(AttemptError::InvalidInput)?
        .to_path_buf();
    if marker_parent == storage_root.as_path() {
        return Err(AttemptError::InvalidInput);
    }
    let client = Client::new(storage_endpoint.path);
    let runner = OrdinaryCargoAttemptRunner::new(
        client,
        storage_root,
        parent_endpoint,
        authenticator,
        budget,
    )?;
    run_attempt(runner, invocation)
}

fn check_endpoint(endpoint: &Endpoint) -> Result<(), AttemptError> {
    let current =
        Endpoint::capture(endpoint.path.clone()).map_err(|_| AttemptError::InvalidInput)?;
    if current.device != endpoint.device || current.inode != endpoint.inode {
        return Err(AttemptError::InvalidInput);
    }
    Ok(())
}

struct UnixCallbacks {
    endpoint: Endpoint,
    stream: Option<UnixStream>,
    aggregate_bytes: usize,
    calls: u16,
}

impl UnixCallbacks {
    fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            stream: None,
            aggregate_bytes: 0,
            calls: 0,
        }
    }
}

impl OrdinaryCargoCallbacks for UnixCallbacks {
    fn exchange(&mut self, request: Value, deadline: Instant) -> Result<Value, CallbackFailure> {
        if self.calls >= 3 {
            return Err(CallbackFailure::Ambiguous);
        }
        self.calls += 1;
        if self.stream.is_none() {
            check_endpoint(&self.endpoint).map_err(|_| CallbackFailure::NotDelivered)?;
            self.stream = Some(
                UnixStream::connect(&self.endpoint.path)
                    .map_err(|_| CallbackFailure::NotDelivered)?,
            );
        }
        let stream = self.stream.as_mut().ok_or(CallbackFailure::NotDelivered)?;
        let bytes = serde_json::to_vec(&request).map_err(|_| CallbackFailure::NotDelivered)?;
        if bytes.is_empty() || bytes.len() > MAX_ATTEMPT_FRAME_BYTES {
            return Err(CallbackFailure::NotDelivered);
        }
        let next = self
            .aggregate_bytes
            .checked_add(bytes.len())
            .and_then(|total| total.checked_add(4))
            .ok_or(CallbackFailure::NotDelivered)?;
        if next > MAX_ATTEMPT_AGGREGATE_BYTES {
            return Err(CallbackFailure::NotDelivered);
        }
        let length = u32::try_from(bytes.len()).map_err(|_| CallbackFailure::NotDelivered)?;
        let mut written = 0usize;
        for part in [&length.to_be_bytes()[..], bytes.as_slice()] {
            let mut offset = 0;
            while offset < part.len() {
                if Instant::now() >= deadline {
                    return Err(if written == 0 {
                        CallbackFailure::NotDelivered
                    } else {
                        CallbackFailure::Ambiguous
                    });
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if stream.set_write_timeout(Some(remaining)).is_err() {
                    return Err(CallbackFailure::Ambiguous);
                }
                match stream.write(&part[offset..]) {
                    Ok(0) => return Err(CallbackFailure::Ambiguous),
                    Ok(count) => {
                        offset += count;
                        written += count;
                    }
                    Err(_) => {
                        return Err(if written == 0 {
                            CallbackFailure::NotDelivered
                        } else {
                            CallbackFailure::Ambiguous
                        })
                    }
                }
            }
        }
        self.aggregate_bytes = next;
        let response = read_callback_frame(stream, deadline, &mut self.aggregate_bytes)?;
        let parsed =
            parse_json_without_duplicates(&response).map_err(|_| CallbackFailure::Ambiguous)?;
        Ok(parsed)
    }
}

fn read_callback_frame(
    stream: &mut UnixStream,
    deadline: Instant,
    aggregate: &mut usize,
) -> Result<Vec<u8>, CallbackFailure> {
    let mut header = [0u8; 4];
    read_until(stream, &mut header, deadline)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_ATTEMPT_FRAME_BYTES {
        return Err(CallbackFailure::Ambiguous);
    }
    let total = aggregate
        .checked_add(4 + length)
        .ok_or(CallbackFailure::Ambiguous)?;
    if total > MAX_ATTEMPT_AGGREGATE_BYTES {
        return Err(CallbackFailure::Ambiguous);
    }
    let mut body = vec![0u8; length];
    read_until(stream, &mut body, deadline)?;
    *aggregate = total;
    Ok(body)
}

fn read_until(
    stream: &mut UnixStream,
    output: &mut [u8],
    deadline: Instant,
) -> Result<(), CallbackFailure> {
    let mut offset = 0;
    while offset < output.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || stream.set_read_timeout(Some(remaining)).is_err() {
            return Err(CallbackFailure::Ambiguous);
        }
        match stream.read(&mut output[offset..]) {
            Ok(0) => return Err(CallbackFailure::Ambiguous),
            Ok(count) => offset += count,
            Err(_) => return Err(CallbackFailure::Ambiguous),
        }
    }
    Ok(())
}

struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = UniqueValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON with unique object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(UniqueValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(UniqueValue(value)) = sequence.next_element()? {
            values.push(value);
            if values.len() > 4096 {
                return Err(serde::de::Error::custom("JSON array exceeds limit"));
            }
        }
        Ok(UniqueValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some((key, UniqueValue(value))) = object.next_entry::<String, UniqueValue>()? {
            if values.insert(key, value).is_some() {
                return Err(serde::de::Error::custom("duplicate JSON object key"));
            }
            if values.len() > 4096 {
                return Err(serde::de::Error::custom("JSON object exceeds limit"));
            }
        }
        Ok(UniqueValue(Value::Object(values)))
    }
}

fn parse_json_without_duplicates<T: for<'de> Deserialize<'de>>(
    bytes: &[u8],
) -> Result<T, AttemptError> {
    if bytes.is_empty() || bytes.len() > MAX_ATTEMPT_FRAME_BYTES {
        return Err(AttemptError::InvalidInput);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let UniqueValue(value) =
        UniqueValue::deserialize(&mut deserializer).map_err(|_| AttemptError::InvalidInput)?;
    deserializer.end().map_err(|_| AttemptError::InvalidInput)?;
    serde_json::from_value(value).map_err(|_| AttemptError::InvalidInput)
}

#[cfg(test)]
mod route_controls {
    use super::*;
    use crate::registered::qualification::{
        ordinary_cargo_request_digest, ordinary_cargo_settlement_subject, CallbackAuthenticator,
        MacKey, QualificationAuthority,
    };
    use crate::storage::EvidenceAuthority;
    use sha2::Digest;

    fn digest(value: &str) -> String {
        format!("sha256:{:x}", sha2::Sha256::digest(value.as_bytes()))
    }

    fn invocation() -> OrdinaryCargoInvocation {
        let binding = crate::storage::RunBinding {
            consumer: "synthetic-consumer".into(),
            project: "synthetic-project".into(),
            component: "synthetic-component".into(),
            workflow: "synthetic-workflow".into(),
            task_id: "synthetic-task".into(),
            workspace_id: "synthetic-workspace".into(),
            run_id: "synthetic-run".into(),
            attempt_id: "synthetic-attempt".into(),
            commit: "a".repeat(40),
            source_tree_digest: digest("tree"),
        };
        let partition = crate::storage::PartitionKey {
            project_component: "synthetic-project:synthetic-component".into(),
            workflow: binding.workflow.clone(),
            cargo_provenance: "cargo synthetic".into(),
            rustc_provenance: "rustc synthetic".into(),
            host: "aarch64-apple-darwin".into(),
            target: "aarch64-apple-darwin".into(),
            profile: "dev".into(),
            features: vec!["private-qualification".into()],
            config_digest: digest("config"),
            environment_digest: digest("environment"),
            path_identity: "ordinary-cargo-storage-v1".into(),
        };
        OrdinaryCargoInvocation {
            request: OrdinaryCargoRequest {
                acquire: crate::storage::AcquireRequest {
                    envelope: RunEnvelope {
                        binding,
                        partition,
                        admission_reference: "synthetic-admission".into(),
                        admission_credential: Credential::new("synthetic-admission-credential"),
                    },
                    acquire_id: "synthetic-acquire".into(),
                    required_bytes: 1024,
                },
                root: crate::storage::StrictPhysicalCargoRoot {
                    source_root: PathBuf::from("/synthetic/source"),
                    consumer_root_id: "synthetic-root".into(),
                    allocation_epoch: "synthetic-epoch".into(),
                },
                partition: OrdinaryCargoPartition::OrdinaryTest,
                acceptance: crate::storage::OrdinaryCargoAcceptance {
                    policy_version: 1,
                    installation_epoch: "synthetic-install".into(),
                    acceptance_reference: "synthetic-acceptance".into(),
                    input_boundary_digest: digest("boundary"),
                    credential: Credential::new("synthetic-acceptance-credential"),
                },
            },
            branch_id: "synthetic-branch".into(),
            source_content_digest: digest("source-content"),
            selector: "rust:test".into(),
            argv: vec!["test".into(), "--workspace".into()],
            cwd: PathBuf::from("/synthetic/source"),
            environment: BTreeMap::from([
                ("CARGO_BUILD_BUILD_DIR".into(), None),
                ("CARGO_TARGET_DIR".into(), None),
                ("TMPDIR".into(), None),
                ("TMP".into(), None),
                ("TEMP".into(), None),
                ("RUSTFLAGS".into(), None),
            ]),
            target_dir: PathBuf::from("/synthetic/target"),
            temp_dir: PathBuf::from("/synthetic/temp"),
            capture_dir: PathBuf::from("/synthetic/capture"),
            expected_outputs: vec!["test-result".into()],
            operation_ids: OrdinaryCargoOperationIds {
                submit: "synthetic-submit".into(),
                capture: "synthetic-capture".into(),
                not_submitted: "synthetic-not-submitted".into(),
                finalize: "synthetic-finalize".into(),
            },
        }
    }

    fn acquired() -> (AcquiredCargoIdentity, AcquireResult) {
        let slot_path = PathBuf::from("/synthetic/storage/slots/slot-1");
        let tuple = AcquiredCargoIdentity {
            policy_version: 1,
            installation_epoch: "synthetic-install".into(),
            acceptance_reference: "synthetic-acceptance".into(),
            input_boundary_digest: digest("boundary"),
            source_root: PathBuf::from("/synthetic/source"),
            root_id: "synthetic-root".into(),
            allocation_epoch: "synthetic-epoch".into(),
            partition: "ordinary-test".into(),
            reuse_state: "pending".into(),
            state: "leased".into(),
            storage_instance_id: "b".repeat(64),
            slot_id: "slot-1".into(),
            generation: "1".into(),
            lease_id: "synthetic-lease".into(),
            slot_path: slot_path.clone(),
            reused: false,
            consumer: "synthetic-consumer".into(),
            project: "synthetic-project".into(),
            component: "synthetic-component".into(),
            workflow: "synthetic-workflow".into(),
            task_id: "synthetic-task".into(),
            workspace_id: "synthetic-workspace".into(),
            branch_id: "synthetic-branch".into(),
            run_id: "synthetic-run".into(),
            attempt_id: "synthetic-attempt".into(),
            commit: "a".repeat(40),
            source_tree_digest: digest("tree"),
            source_content_digest: digest("source-content"),
            cargo_partition: invocation().request.acquire.envelope.partition,
        };
        let lease = AcquireResult {
            lease_id: tuple.lease_id.clone(),
            slot_id: tuple.slot_id.clone(),
            storage_instance_id: tuple.storage_instance_id.clone(),
            generation: tuple.generation.clone(),
            state: SlotState::Leased,
            slot_path: Some(slot_path),
            token: Some(crate::storage::LeaseToken::new("synthetic-token".into())),
            reused: false,
        };
        (tuple, lease)
    }

    fn run_identity(invocation: &OrdinaryCargoInvocation) -> OrdinaryRunIdentity {
        let binding = &invocation.request.acquire.envelope.binding;
        OrdinaryRunIdentity {
            host_run_id: "synthetic-host-run".into(),
            consumer: binding.consumer.clone(),
            project: binding.project.clone(),
            component: binding.component.clone(),
            workflow: binding.workflow.clone(),
            task_id: binding.task_id.clone(),
            workspace_id: binding.workspace_id.clone(),
            branch_id: invocation.branch_id.clone(),
            run_id: binding.run_id.clone(),
            attempt_id: binding.attempt_id.clone(),
            commit: binding.commit.clone(),
            source_tree_digest: binding.source_tree_digest.clone(),
            source_content_digest: invocation.source_content_digest.clone(),
        }
    }

    fn captured(
        invocation: &OrdinaryCargoInvocation,
        tuple: &AcquiredCargoIdentity,
        run: &OrdinaryRunIdentity,
    ) -> CaptureReply {
        let reference = "synthetic-capture".to_owned();
        let capture_digest = digest("capture");
        CaptureReply {
            run: run.clone(),
            outcome: CapturedOutcome::Passed,
            terminal_credential: "synthetic-terminal-credential".into(),
            capture: CapturedOutput {
                reference: reference.clone(),
                digest: capture_digest.clone(),
                host_run_id: run.host_run_id.clone(),
                run_id: run.run_id.clone(),
                attempt_id: run.attempt_id.clone(),
                credential: "synthetic-capture-credential".into(),
                outputs: vec![CapturedOutputEntry {
                    role: "test-result".into(),
                    digest: digest("test result"),
                    bytes: WireU64(12),
                }],
            },
            completion: CapturedCompletion {
                lease_id: tuple.lease_id.clone(),
                slot_id: tuple.slot_id.clone(),
                slot_path: tuple.slot_path.to_string_lossy().into_owned(),
                temp_dir: invocation.temp_dir.to_string_lossy().into_owned(),
                capture_dir: invocation.capture_dir.to_string_lossy().into_owned(),
                workflow: invocation.request.acquire.envelope.binding.workflow.clone(),
                workflow_definition_digest: digest("workflow definition"),
                repository_head: run.commit.clone(),
                fixture_source_digest: run.source_content_digest.clone(),
                host_run_id: run.host_run_id.clone(),
                run_id: run.run_id.clone(),
                attempt_id: run.attempt_id.clone(),
                processes: vec![ProcessIdentityEvidence {
                    pid: 1,
                    start_identity: digest("synthetic process identity"),
                }],
                free_space_samples: vec![WireU64(20 * 1024 * 1024 * 1024)],
                exit_code: 0,
                stdout_digest: digest("stdout"),
                stderr_digest: digest("stderr"),
                capture_reference: reference,
                capture_digest,
            },
            settlement_credential: "synthetic-settlement-credential".into(),
        }
    }

    #[test]
    fn callback_receipt_metadata_and_duplicate_json_are_fail_closed() {
        let context = "sha256:callback-context".to_owned();
        let authenticator =
            CallbackAuthenticator::new(MacKey::from_hex(&"c".repeat(64)).unwrap(), context)
                .unwrap();
        let mut reply = CallbackReply {
            version: ORDINARY_CARGO_ATTEMPT_VERSION,
            method: "ordinary_cargo.submit".into(),
            id: "submit-1".into(),
            launch_digest: digest("launch"),
            acquired_digest: digest("acquired"),
            command_digest: digest("command"),
            payload: json!({"status":"not_started","run":null}),
            receipt: String::new(),
        };
        let result_digest = canonical_digest(&json!({
            "method": reply.method,
            "id": reply.id,
            "launch_digest": reply.launch_digest,
            "acquired_digest": reply.acquired_digest,
            "command_digest": reply.command_digest,
            "payload": reply.payload,
        }))
        .unwrap();
        reply.receipt = authenticator
            .receipt(
                &reply.method,
                &reply.id,
                &reply.launch_digest,
                CALLBACK_POLICY,
                &result_digest,
            )
            .unwrap();
        assert!(verify_callback(
            &reply,
            "ordinary_cargo.submit",
            "submit-1",
            &reply.launch_digest,
            &reply.acquired_digest,
            &reply.command_digest,
            &authenticator,
        ));
        assert!(!verify_callback(
            &reply,
            "ordinary_cargo.capture",
            "submit-1",
            &reply.launch_digest,
            &reply.acquired_digest,
            &reply.command_digest,
            &authenticator,
        ));
        let mut changed_version = reply.clone();
        changed_version.version += 1;
        assert!(!verify_callback(
            &changed_version,
            "ordinary_cargo.submit",
            "submit-1",
            &reply.launch_digest,
            &reply.acquired_digest,
            &reply.command_digest,
            &authenticator,
        ));
        let mut changed_id = reply.clone();
        changed_id.id = "submit-2".into();
        assert!(!verify_callback(
            &changed_id,
            "ordinary_cargo.submit",
            "submit-1",
            &reply.launch_digest,
            &reply.acquired_digest,
            &reply.command_digest,
            &authenticator,
        ));
        let mut changed_receipt = reply.clone();
        changed_receipt.receipt.push('0');
        assert!(!verify_callback(
            &changed_receipt,
            "ordinary_cargo.submit",
            "submit-1",
            &reply.launch_digest,
            &reply.acquired_digest,
            &reply.command_digest,
            &authenticator,
        ));
        let wire = br#"{"version":1,"version":1}"#;
        assert!(parse_json_without_duplicates::<Value>(wire).is_err());
        assert!(parse_json_without_duplicates::<Value>(br#"{"x":1} trailing"#).is_err());
        let mut unknown_field = serde_json::to_value(&reply).unwrap();
        unknown_field["unexpected"] = json!(true);
        assert!(serde_json::from_value::<CallbackReply>(unknown_field).is_err());
        assert!(serde_json::from_str::<WireU64>("9007199254740992").is_err());
        assert_eq!(
            serde_json::from_str::<WireU64>("\"18446744073709551615\"")
                .unwrap()
                .0,
            u64::MAX
        );
    }

    #[test]
    fn typescript_request_and_hmac_vectors_match_rust() {
        let vector: Value =
            serde_json::from_str(include_str!("../../fixtures/ordinary-cargo-auth-v1.json"))
                .unwrap();
        assert_eq!(
            ordinary_cargo_input_boundary_digest(&invocation()).unwrap(),
            vector["input_boundary_digest"].as_str().unwrap()
        );
        let subject = &vector["request_subject"];
        let envelope_subject = &subject["acquire"]["envelope"];
        let request = OrdinaryCargoRequest {
            acquire: crate::storage::AcquireRequest {
                envelope: RunEnvelope {
                    binding: serde_json::from_value(envelope_subject["binding"].clone()).unwrap(),
                    partition: serde_json::from_value(envelope_subject["partition"].clone())
                        .unwrap(),
                    admission_reference: envelope_subject["admission_reference"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                    admission_credential: Credential::new("excluded-from-request-digest"),
                },
                acquire_id: subject["acquire"]["acquire_id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                required_bytes: subject["acquire"]["required_bytes"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap(),
            },
            root: crate::storage::StrictPhysicalCargoRoot {
                source_root: PathBuf::from(subject["root"]["source_root"].as_str().unwrap()),
                consumer_root_id: subject["root"]["consumer_root_id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                allocation_epoch: subject["root"]["allocation_epoch"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            },
            partition: OrdinaryCargoPartition::OrdinaryTest,
            acceptance: crate::storage::OrdinaryCargoAcceptance {
                policy_version: subject["acceptance"]["policy_version"].as_u64().unwrap() as u32,
                installation_epoch: subject["acceptance"]["installation_epoch"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                acceptance_reference: subject["acceptance"]["acceptance_reference"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                input_boundary_digest: subject["acceptance"]["input_boundary_digest"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                credential: Credential::new("excluded-from-request-digest"),
            },
        };
        assert_eq!(
            ordinary_cargo_request_digest(&request).unwrap(),
            vector["request_digest"].as_str().unwrap()
        );
        assert_eq!(
            ordinary_cargo_callback_context(
                vector["callback_context_subject"]["scope"]
                    .as_str()
                    .unwrap(),
                vector["request_digest"].as_str().unwrap(),
            )
            .unwrap(),
            vector["callback_context_digest"].as_str().unwrap()
        );

        let settlement_vector = &vector["settlement"];
        let subject = &settlement_vector["subject"]["settlement"];
        let completion = &subject["containment"];
        let evidence = SettlementEvidence {
            binding: request.acquire.envelope.binding.clone(),
            consumer_credential: Credential::new("excluded"),
            terminal: Some(TerminalRecord {
                run_id: subject["terminal"]["run_id"].as_str().unwrap().to_owned(),
                outcome: TerminalOutcome::Passed,
                credential: Credential::new("excluded"),
            }),
            capture: Some(CaptureReceipt {
                reference: subject["capture"]["reference"].as_str().unwrap().to_owned(),
                digest: subject["capture"]["digest"].as_str().unwrap().to_owned(),
                host_run_id: subject["capture"]["host_run_id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                run_id: subject["capture"]["run_id"].as_str().unwrap().to_owned(),
                attempt_id: subject["capture"]["attempt_id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                credential: Credential::new("excluded"),
            }),
            containment: ContainmentEvidence::CooperativeWorkflow(CooperativeCompletionEvidence {
                lease_id: completion["lease_id"].as_str().unwrap().to_owned(),
                slot_id: completion["slot_id"].as_str().unwrap().to_owned(),
                slot_path: completion["slot_path"].as_str().unwrap().to_owned(),
                temp_dir: completion["temp_dir"].as_str().unwrap().to_owned(),
                capture_dir: completion["capture_dir"].as_str().unwrap().to_owned(),
                workflow: completion["workflow"].as_str().unwrap().to_owned(),
                workflow_definition_digest: completion["workflow_definition_digest"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                repository_head: completion["repository_head"].as_str().unwrap().to_owned(),
                fixture_source_digest: completion["fixture_source_digest"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                host_run_id: completion["host_run_id"].as_str().unwrap().to_owned(),
                run_id: completion["run_id"].as_str().unwrap().to_owned(),
                attempt_id: completion["attempt_id"].as_str().unwrap().to_owned(),
                processes: vec![ProcessIdentityEvidence {
                    pid: completion["processes"][0]["pid"].as_u64().unwrap() as u32,
                    start_identity: completion["processes"][0]["start_identity"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                }],
                free_space_samples: vec![completion["free_space_samples"][0].as_u64().unwrap()],
                exit_code: completion["exit_code"].as_i64().unwrap() as i32,
                stdout_digest: completion["stdout_digest"].as_str().unwrap().to_owned(),
                stderr_digest: completion["stderr_digest"].as_str().unwrap().to_owned(),
                capture_reference: completion["capture_reference"].as_str().unwrap().to_owned(),
                capture_digest: completion["capture_digest"].as_str().unwrap().to_owned(),
                signature: String::new(),
            }),
        };
        let actual_subject = ordinary_cargo_settlement_subject(
            settlement_vector["request_digest"].as_str().unwrap(),
            &evidence,
        )
        .unwrap();
        assert_eq!(actual_subject, settlement_vector["subject"]);
        assert_eq!(
            canonical_digest(&actual_subject).unwrap(),
            settlement_vector["subject_digest"].as_str().unwrap()
        );

        let key = MacKey::from_hex(vector["admission"]["key_hex"].as_str().unwrap()).unwrap();
        for name in ["admission", "callback", "settlement"] {
            let entry = &vector[name];
            let fields: Vec<&str> = entry["fields"]
                .as_array()
                .unwrap()
                .iter()
                .map(Value::as_str)
                .collect::<Option<Vec<_>>>()
                .unwrap();
            assert!(key.verify(
                entry["domain"].as_str().unwrap(),
                &fields,
                entry["receipt"].as_str().unwrap(),
            ));
        }
    }

    #[test]
    fn ordinary_admission_requires_an_explicit_installed_request_allowlist() {
        let key = MacKey::from_hex(&"d".repeat(64)).unwrap();
        let instance = "e".repeat(64);
        let scope = "ordinary-policy-test";
        let mut request = invocation().request;
        let digest = ordinary_cargo_request_digest(&request).unwrap();
        request.acceptance.credential = Credential::new(
            key.sign(
                "oxide-ordinary-cargo-admission-v1",
                &[scope, &instance, &digest],
            )
            .unwrap(),
        );

        let default_authority = QualificationAuthority::new(
            key.clone(),
            scope.into(),
            vec![request.acquire.envelope.clone()],
        )
        .unwrap();
        default_authority.install_instance(&instance).unwrap();
        assert!(!default_authority.verify_ordinary_cargo_admission(&request));

        let installed_authority =
            QualificationAuthority::new(key, scope.into(), vec![request.acquire.envelope.clone()])
                .unwrap()
                .with_ordinary_cargo_request_digests(vec![digest])
                .unwrap();
        installed_authority.install_instance(&instance).unwrap();
        assert!(installed_authority.verify_ordinary_cargo_admission(&request));
    }

    #[test]
    fn complete_tuple_capture_terminal_and_no_submit_controls_refuse_mutations() {
        let invocation = invocation();
        let (tuple, lease) = acquired();
        let run = run_identity(&invocation);
        let mut changed_consumer = run.clone();
        changed_consumer.consumer.push_str("-other");
        assert!(!matches_run(&changed_consumer, &invocation));
        let mut changed_component = run.clone();
        changed_component.component.push_str("-other");
        assert!(!matches_run(&changed_component, &invocation));
        let mut changed_workflow = run.clone();
        changed_workflow.workflow.push_str("-other");
        assert!(!matches_run(&changed_workflow, &invocation));
        let mut changed_workspace = run.clone();
        changed_workspace.workspace_id.push_str("-other");
        assert!(!matches_run(&changed_workspace, &invocation));
        let good = captured(&invocation, &tuple, &run);
        assert!(
            capture_evidence(&invocation, &lease, &tuple, &run, good.clone())
                .unwrap()
                .is_some()
        );

        let mut changed_capture = good.clone();
        changed_capture.capture.run_id.push_str("-other");
        assert!(
            capture_evidence(&invocation, &lease, &tuple, &run, changed_capture)
                .unwrap()
                .is_none()
        );
        let mut changed_terminal = good;
        changed_terminal.completion.exit_code = 1;
        assert!(
            capture_evidence(&invocation, &lease, &tuple, &run, changed_terminal)
                .unwrap()
                .is_none()
        );

        let expected = run_identity(&invocation);
        let no_submit = NotSubmittedReply {
            status: "not_submitted".into(),
            submission_state: "never_began".into(),
            writer_state: "none".into(),
            closure_reference: "synthetic-closure".into(),
            run: expected.clone(),
            settlement_credential: "hmac-bundle-v1:synthetic".into(),
        };
        assert!(valid_not_submitted(&no_submit, &expected));
        let mut changed_writer = no_submit.clone();
        changed_writer.writer_state = "unknown".into();
        assert!(!valid_not_submitted(&changed_writer, &expected));
        let mut changed_run = no_submit;
        changed_run.run.attempt_id.push_str("-other");
        assert!(!valid_not_submitted(&changed_run, &expected));

        let roles = vec!["test-result".to_owned()];
        let outputs = vec![CapturedOutputEntry {
            role: "test-result".into(),
            digest: digest("output"),
            bytes: WireU64(MAX_CAPTURE_BYTES + 1),
        }];
        assert!(!complete_outputs(&roles, &outputs));
    }
}
