mod cargo;
mod native;
mod observations;
mod resolve;
mod source_map;
#[cfg(test)]
mod source_map_probe;

#[cfg(test)]
mod tests;

pub use observations::{
    ObservationEvent, ObservationKind, RunObservation, WriteAttribution, MAX_EVENTS_PER_RUN,
    MAX_EVENT_BYTES, OBSERVATION_RING_BYTES,
};
pub use resolve::{
    EffectiveBuildInputs, EffectiveInputSet, EffectiveValue, PathIdentity, ProjectDescriptor,
    ProjectInvocation, ToolchainInput, WorkflowOptIn,
};
pub use source_map::{
    AppleOsoObservation, AuthorizedSourceRoot, BehaviorPathClass, BehaviorPathObservation,
    BinaryArtifactDigest, BinarySourceScan, CompileOriginIdentity, KeyMissControl,
    MissControlEvidence, PortabilityEvidence, QualificationDisposition, RelativeSourcePrefix,
    ResolvedSourceMap, SourceMapPolicy, SourceMapRunReceipt, SourceMissControl, SourcePathClass,
    SourcePathMode, SourcePathObservation, ACCEPTED_LOGICAL_POLICY, REQUIRED_BEHAVIOR_PATHS,
    SOURCE_MAP_VERSION, SOURCE_SCAN_VERSION,
};

use crate::storage::{
    AcquireRequest, AcquireResult, CaptureReceipt, Client, ContainmentEvidence, Credential,
    FinalizationReceipt, Inventory, LeaseToken, PartitionKey, RunBinding, RunEnvelope,
    SettlementEvidence, SlotState, StorageError, TerminalRecord,
};
use observations::{ObservationKind as Event, ObservationRing, RunObservation as Observation};
use resolve::{Resolution, ResolvedInvocation};
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

const OBSERVATION_BACKEND: &str = "cargo-build-dir";

/// Safe refusal from resolution, storage admission, or the fixture boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterRefusal {
    AmbiguousInput,
    UnsupportedInput,
    InputUnknown,
    UnsupportedToolchain,
    Busy,
    Full,
    DaemonUnavailable,
    WaitUnavailable,
    NotAdmitted,
}

/// Result of an adapter attempt. It contains no QA verdict.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectExecutionResult {
    pub disposition: ExecutionDisposition,
    pub host_run: Option<HostRunIdentity>,
    pub finalization: Option<FinalizationReceipt>,
    pub private_build_allowed: bool,
}

/// Adapter lifecycle status, separate from the consumer's command/test result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionDisposition {
    NotOptedIn,
    CancelledBeforeAcquire,
    NotAdmitted,
    Refused(AdapterRefusal),
    Finished,
    Uncertain,
}

/// Storage operations used by the adapter. `Client` is the local daemon implementation.
pub trait StorageOperations: Send + Sync {
    fn inventory(&self, request_id: &str) -> Result<Inventory, StorageError>;
    fn acquire(&self, request: AcquireRequest) -> Result<AcquireResult, StorageError>;
    fn finalize(
        &self,
        lease_id: &str,
        token: Option<&LeaseToken>,
        finalize_id: &str,
        evidence: SettlementEvidence,
    ) -> Result<FinalizationReceipt, StorageError>;
}

impl StorageOperations for Client {
    fn inventory(&self, request_id: &str) -> Result<Inventory, StorageError> {
        Client::inventory(self, request_id)
    }

    fn acquire(&self, request: AcquireRequest) -> Result<AcquireResult, StorageError> {
        Client::acquire(self, request)
    }

    fn finalize(
        &self,
        lease_id: &str,
        token: Option<&LeaseToken>,
        finalize_id: &str,
        evidence: SettlementEvidence,
    ) -> Result<FinalizationReceipt, StorageError> {
        Client::finalize(self, lease_id, token, finalize_id, evidence)
    }
}

/// Task/attempt request identity used before a host run exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingStorageRequest {
    pub request_id: String,
    pub consumer_id: String,
    pub project_id: String,
    pub workflow: String,
    pub selector: String,
    pub task_id: String,
    pub partition_digest: String,
}

/// Consumer-owned authorization request. It deliberately has no host-run ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptAuthorizationRequest {
    pub pending: PendingStorageRequest,
    pub component_id: String,
    pub workspace_id: String,
    pub commit: String,
    pub source_tree_digest: String,
    pub partition: PartitionKey,
}

/// Consumer-issued admitted attempt envelope and stable acquire operation ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizedAttempt {
    pub envelope: RunEnvelope,
    pub acquire_id: String,
}

/// Consumer-owned identity created only after successful storage acquisition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostRunIdentity {
    pub id: String,
}

/// Paths and Cargo settings passed to the injected host-run boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostRunRequest {
    pub selector: String,
    pub workflow: String,
    pub profile: String,
    pub features: Vec<String>,
    pub workspace_root: PathBuf,
    pub cargo_build_dir: PathBuf,
    pub cargo_target_dir: PathBuf,
    pub capture_dir: PathBuf,
    /// Explicit map and complete effective compiler flag vector, when selected.
    pub source_map: Option<ResolvedSourceMap>,
    /// Complete allowlisted overrides; `None` means remove that inherited variable.
    pub environment: Vec<EffectiveValue>,
}

impl HostRunRequest {
    /// Per-invocation Cargo directory variables the injected host boundary must force.
    pub fn cargo_directory_overrides(&self) -> [(OsString, OsString); 2] {
        [
            (
                OsString::from("CARGO_BUILD_BUILD_DIR"),
                self.cargo_build_dir.as_os_str().to_os_string(),
            ),
            (
                OsString::from("CARGO_TARGET_DIR"),
                self.cargo_target_dir.as_os_str().to_os_string(),
            ),
        ]
    }
}

/// Execution context for a fake or lightweight task-owned command runner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureExecutionPlan {
    pub host_run: HostRunIdentity,
    pub command: HostRunRequest,
    pub(crate) claim: native::NativeExecutionClaim,
}

/// Non-verdict process completion status returned by the fixture runner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixtureCommandStatus {
    Exited(i32),
    Cancelled,
    Incomplete,
}

/// Completion proof from controlled fixture construction, never native containment proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FixtureCompletionProof {
    Unknown,
    Stopped {
        reference: String,
        credential: Credential,
    },
}

/// Metrics and per-run output references returned by the fixture command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureCommandOutput {
    pub status: FixtureCommandStatus,
    pub output_references: Vec<String>,
    pub compile_elapsed_ms: Option<u64>,
    pub test_elapsed_ms: Option<u64>,
    pub reused_units: Option<u32>,
    pub rebuilt_units: Option<u32>,
    pub completion: FixtureCompletionProof,
    /// Exact-path evidence from the experimental local qualifier. Ordinary
    /// injected runners cannot qualify mapped reuse by returning a status.
    pub(crate) source_map_receipt: Option<SourceMapRunReceipt>,
    pub(crate) portability_evidence: Option<PortabilityEvidence>,
    pub(crate) native_completion: Option<native::NativeCompletionCertificate>,
}

impl FixtureCommandOutput {
    /// Constructs output from an injected runner. Injected completion claims
    /// remain untrusted and cannot release managed storage.
    pub fn new(
        status: FixtureCommandStatus,
        output_references: Vec<String>,
        compile_elapsed_ms: Option<u64>,
        test_elapsed_ms: Option<u64>,
        reused_units: Option<u32>,
        rebuilt_units: Option<u32>,
        completion: FixtureCompletionProof,
    ) -> Self {
        Self {
            status,
            output_references,
            compile_elapsed_ms,
            test_elapsed_ms,
            reused_units,
            rebuilt_units,
            completion,
            source_map_receipt: None,
            portability_evidence: None,
            native_completion: None,
        }
    }
}

/// Consumer acknowledgement that binds output capture to one host run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureAcknowledgement {
    pub host_run_id: String,
    pub receipt: CaptureReceipt,
}

/// A fixed boundary failure. Implementations should not include raw commands or secrets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundaryFailure {
    Unavailable,
    Rejected,
    InvalidIdentity,
}

/// Consumer admission, host submission, capture, and settlement authority.
pub trait ProjectConsumerBoundary {
    /// Returns `None` when this task attempt has not been admitted.
    fn authorize_attempt(
        &mut self,
        request: &AttemptAuthorizationRequest,
    ) -> Result<Option<AuthorizedAttempt>, BoundaryFailure>;

    /// Receives a not-started result; it is not a consumer run or QA settlement.
    fn on_storage_refusal(&mut self, attempt: &AuthorizedAttempt, reason: AdapterRefusal);

    /// Withdraws an authorized task attempt that was cancelled before acquire.
    fn cancel_unstarted_attempt(&mut self, attempt: &AuthorizedAttempt);

    /// Submits a host run after acquire; the returned ID is distinct from task and attempt IDs.
    fn submit_host_run(
        &mut self,
        attempt: &AuthorizedAttempt,
        request: &HostRunRequest,
    ) -> Result<HostRunIdentity, BoundaryFailure>;

    /// Captures consumer-owned output and returns its authenticated run-bound receipt.
    fn capture(
        &mut self,
        host_run: &HostRunIdentity,
        output: &FixtureCommandOutput,
        capture_dir: &Path,
    ) -> Result<CaptureAcknowledgement, BoundaryFailure>;

    /// Supplies a consumer-owned terminal record, if one exists.
    fn terminal_record(
        &mut self,
        attempt: &AuthorizedAttempt,
        host_run: Option<&HostRunIdentity>,
        output: Option<&FixtureCommandOutput>,
    ) -> Option<TerminalRecord>;

    /// Supplies the consumer credential used by the fixture's evidence authority.
    fn settlement_credential(&mut self, attempt: &AuthorizedAttempt) -> Credential;
}

/// Cancellation state supplied by the fixture consumer or runner.
pub trait CancellationProbe {
    fn is_cancelled(&self) -> bool;
}

/// One wake or cancellation result; the adapter performs no timer retry loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitSignal {
    Wake,
    Cancelled,
}

/// Non-reserving readiness registration and one cancellable state-change wait.
pub trait StorageWaitBoundary {
    fn register(&mut self, request: &PendingStorageRequest) -> Result<(), BoundaryFailure>;
    fn wait(
        &mut self,
        request_id: &str,
        cancellation: &dyn CancellationProbe,
    ) -> Result<WaitSignal, BoundaryFailure>;
    fn cancel(&mut self, request_id: &str) -> Result<(), BoundaryFailure>;
}

/// Isolated task-fixture command runner. No native process implementation is provided.
/// A `Stopped` proof is valid only when the runner has joined every fixture writer before return.
pub trait FixtureCommandRunner {
    fn execute(
        &mut self,
        plan: &FixtureExecutionPlan,
        cancellation: &dyn CancellationProbe,
    ) -> Result<FixtureCommandOutput, BoundaryFailure>;
}

/// Opt-in adapter around the storage core and injected consumer/fixture boundaries.
pub struct ProjectAdapter<S> {
    storage: S,
    observations: ObservationRing,
}

impl<S> fmt::Debug for ProjectAdapter<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProjectAdapter")
            .field("observations", &self.observations.snapshot().len())
            .finish()
    }
}

impl<S: StorageOperations> ProjectAdapter<S> {
    /// Uses the existing local storage API. No daemon or process is started here.
    pub fn new(storage: S) -> Self {
        Self {
            storage,
            observations: ObservationRing::default(),
        }
    }

    /// Runs at most one managed attempt; a busy refusal never submits a host run.
    pub fn execute<C, W, R>(
        &mut self,
        invocation: &ProjectInvocation,
        consumer: &mut C,
        waiter: &mut W,
        runner: &mut R,
        cancellation: &dyn CancellationProbe,
    ) -> ProjectExecutionResult
    where
        C: ProjectConsumerBoundary,
        W: StorageWaitBoundary,
        R: FixtureCommandRunner,
    {
        let resolved = match resolve::resolve(invocation) {
            Resolution::NotOptedIn => {
                return result(ExecutionDisposition::NotOptedIn, None, None, true);
            }
            Resolution::Refused(reason) => {
                return result(
                    ExecutionDisposition::Refused(reason),
                    None,
                    None,
                    invocation.private_fallback_allowed,
                );
            }
            Resolution::Ready(resolved) => resolved,
        };
        if cancellation.is_cancelled() {
            return result(
                ExecutionDisposition::CancelledBeforeAcquire,
                None,
                None,
                false,
            );
        }

        let pending = pending_request(invocation, &resolved);
        let readiness = match self.readiness(invocation, &resolved) {
            Ok(readiness) => readiness,
            Err(reason) => {
                return result(
                    ExecutionDisposition::Refused(reason),
                    None,
                    None,
                    fallback_allowed(invocation, reason),
                );
            }
        };
        let readiness = if readiness == Readiness::Busy && invocation.wait_once {
            match wait_once(waiter, &pending, cancellation) {
                Ok(WaitSignal::Wake) => match self.readiness(invocation, &resolved) {
                    Ok(Readiness::Ready) => Readiness::Ready,
                    Ok(Readiness::Busy) => Readiness::Busy,
                    Ok(Readiness::Full) => Readiness::Full,
                    Err(reason) => {
                        return result(
                            ExecutionDisposition::Refused(reason),
                            None,
                            None,
                            fallback_allowed(invocation, reason),
                        );
                    }
                },
                Ok(WaitSignal::Cancelled) => {
                    return result(
                        ExecutionDisposition::CancelledBeforeAcquire,
                        None,
                        None,
                        false,
                    );
                }
                Err(_) => {
                    return result(
                        ExecutionDisposition::Refused(AdapterRefusal::WaitUnavailable),
                        None,
                        None,
                        false,
                    );
                }
            }
        } else {
            readiness
        };
        if cancellation.is_cancelled() {
            return result(
                ExecutionDisposition::CancelledBeforeAcquire,
                None,
                None,
                false,
            );
        }

        let auth_request = authorization_request(invocation, &resolved, pending);
        let attempt = match consumer.authorize_attempt(&auth_request) {
            Ok(Some(attempt)) => attempt,
            Ok(None) => return result(ExecutionDisposition::NotAdmitted, None, None, false),
            Err(_) => {
                return result(
                    ExecutionDisposition::Refused(AdapterRefusal::NotAdmitted),
                    None,
                    None,
                    false,
                );
            }
        };
        if !valid_authorization(invocation, &resolved, &attempt) {
            return result(
                ExecutionDisposition::Refused(AdapterRefusal::NotAdmitted),
                None,
                None,
                false,
            );
        }
        if cancellation.is_cancelled() {
            consumer.cancel_unstarted_attempt(&attempt);
            return result(
                ExecutionDisposition::CancelledBeforeAcquire,
                None,
                None,
                false,
            );
        }
        self.begin_observation(&resolved, &attempt);
        self.observations.event(
            &attempt.envelope.binding.consumer,
            &attempt.envelope.binding.run_id,
            &attempt.envelope.binding.attempt_id,
            Event::Requested {
                operation_id: attempt.acquire_id.clone(),
            },
        );

        if readiness == Readiness::Busy || readiness == Readiness::Full {
            let reason = if readiness == Readiness::Busy {
                AdapterRefusal::Busy
            } else {
                AdapterRefusal::Full
            };
            self.observations.event(
                &attempt.envelope.binding.consumer,
                &attempt.envelope.binding.run_id,
                &attempt.envelope.binding.attempt_id,
                Event::Refused {
                    reason: refusal_label(reason),
                },
            );
            consumer.on_storage_refusal(&attempt, reason);
            self.observations.settle(
                &attempt.envelope.binding.consumer,
                &attempt.envelope.binding.run_id,
                &attempt.envelope.binding.attempt_id,
            );
            return result(
                ExecutionDisposition::Refused(reason),
                None,
                None,
                fallback_allowed(invocation, reason),
            );
        }

        let acquire = AcquireRequest {
            envelope: attempt.envelope.clone(),
            acquire_id: attempt.acquire_id.clone(),
            required_bytes: resolved.workflow.required_slot_bytes,
        };
        let (acquired, acquire_retried) = match self.acquire_once(acquire) {
            Ok(acquired) => acquired,
            Err(error) => {
                if matches!(&error, StorageError::Io(_) | StorageError::Protocol(_)) {
                    self.observations.event(
                        &attempt.envelope.binding.consumer,
                        &attempt.envelope.binding.run_id,
                        &attempt.envelope.binding.attempt_id,
                        Event::Uncertain {
                            reason: "acquire-reply-unconfirmed",
                        },
                    );
                    return result(ExecutionDisposition::Uncertain, None, None, false);
                }
                let reason = refusal_for_storage(&error);
                self.observations.event(
                    &attempt.envelope.binding.consumer,
                    &attempt.envelope.binding.run_id,
                    &attempt.envelope.binding.attempt_id,
                    Event::Refused {
                        reason: refusal_label(reason),
                    },
                );
                consumer.on_storage_refusal(&attempt, reason);
                self.observations.settle(
                    &attempt.envelope.binding.consumer,
                    &attempt.envelope.binding.run_id,
                    &attempt.envelope.binding.attempt_id,
                );
                return result(
                    ExecutionDisposition::Refused(reason),
                    None,
                    None,
                    fallback_allowed(invocation, reason),
                );
            }
        };
        if acquire_retried {
            self.observations.event(
                &attempt.envelope.binding.consumer,
                &attempt.envelope.binding.run_id,
                &attempt.envelope.binding.attempt_id,
                Event::Requested {
                    operation_id: attempt.acquire_id.clone(),
                },
            );
        }
        if acquired.state != SlotState::Leased {
            self.observations.event(
                &attempt.envelope.binding.consumer,
                &attempt.envelope.binding.run_id,
                &attempt.envelope.binding.attempt_id,
                Event::Uncertain {
                    reason: "lease-not-active",
                },
            );
            return result(ExecutionDisposition::Uncertain, None, None, false);
        }
        let (Some(slot_path), Some(token)) = (acquired.slot_path.clone(), acquired.token.clone())
        else {
            self.observations.event(
                &attempt.envelope.binding.consumer,
                &attempt.envelope.binding.run_id,
                &attempt.envelope.binding.attempt_id,
                Event::Uncertain {
                    reason: "lease-reply-incomplete",
                },
            );
            return result(ExecutionDisposition::Uncertain, None, None, false);
        };
        if !safe_slot_path(&slot_path, &resolved) {
            let finalization =
                self.finalize_incomplete(&attempt, &acquired, &token, consumer, None, None);
            return result(ExecutionDisposition::Uncertain, None, finalization, false);
        }
        self.observations.event(
            &attempt.envelope.binding.consumer,
            &attempt.envelope.binding.run_id,
            &attempt.envelope.binding.attempt_id,
            Event::Acquired {
                operation_id: attempt.acquire_id.clone(),
                reused: acquired.reused,
            },
        );
        self.record_storage(invocation, &resolved, &attempt);

        if cancellation.is_cancelled() {
            let finalization =
                self.finalize_incomplete(&attempt, &acquired, &token, consumer, None, None);
            self.observations.event(
                &attempt.envelope.binding.consumer,
                &attempt.envelope.binding.run_id,
                &attempt.envelope.binding.attempt_id,
                Event::Uncertain {
                    reason: "cancelled-after-acquire",
                },
            );
            return result(ExecutionDisposition::Uncertain, None, finalization, false);
        }

        let command = host_request(&resolved, &slot_path);
        let host_run = match consumer.submit_host_run(&attempt, &command) {
            Ok(host_run) if valid_host_run(&host_run, &attempt.envelope.binding) => host_run,
            _ => {
                let finalization =
                    self.finalize_incomplete(&attempt, &acquired, &token, consumer, None, None);
                self.observations.event(
                    &attempt.envelope.binding.consumer,
                    &attempt.envelope.binding.run_id,
                    &attempt.envelope.binding.attempt_id,
                    Event::Uncertain {
                        reason: "host-run-submission-unconfirmed",
                    },
                );
                return result(ExecutionDisposition::Uncertain, None, finalization, false);
            }
        };
        self.observations.event(
            &attempt.envelope.binding.consumer,
            &attempt.envelope.binding.run_id,
            &attempt.envelope.binding.attempt_id,
            Event::HostRunSubmitted {
                host_run_id: host_run.id.clone(),
            },
        );
        self.observations.set_host_run(
            &attempt.envelope.binding.consumer,
            &attempt.envelope.binding.run_id,
            &attempt.envelope.binding.attempt_id,
            host_run.id.clone(),
        );
        let plan = FixtureExecutionPlan {
            host_run: host_run.clone(),
            command: command.clone(),
            claim: native::NativeExecutionClaim::new(
                acquired.lease_id.clone(),
                acquired.slot_id.clone(),
                slot_path.clone(),
                attempt.envelope.binding.clone(),
                host_run.clone(),
                resolved.workflow.workflow.clone(),
                resolved.workflow.selector.clone(),
                resolved.capture_dir.clone(),
            ),
        };
        let started = Instant::now();
        let output = match runner.execute(&plan, cancellation) {
            Ok(output) if valid_output(&output) => output,
            _ => {
                let finalization = self.finalize_incomplete(
                    &attempt,
                    &acquired,
                    &token,
                    consumer,
                    Some(&host_run),
                    None,
                );
                self.observations.event(
                    &attempt.envelope.binding.consumer,
                    &attempt.envelope.binding.run_id,
                    &attempt.envelope.binding.attempt_id,
                    Event::Uncertain {
                        reason: "fixture-command-incomplete",
                    },
                );
                return result(
                    ExecutionDisposition::Uncertain,
                    Some(host_run),
                    finalization,
                    false,
                );
            }
        };
        let elapsed_ms = elapsed_millis(started);
        self.observations.event(
            &attempt.envelope.binding.consumer,
            &attempt.envelope.binding.run_id,
            &attempt.envelope.binding.attempt_id,
            Event::WorkCompleted {
                elapsed_ms,
                compile_elapsed_ms: output.compile_elapsed_ms,
                test_elapsed_ms: output.test_elapsed_ms,
                reused_units: output.reused_units,
                rebuilt_units: output.rebuilt_units,
                exit_code: match output.status {
                    FixtureCommandStatus::Exited(code) => Some(code),
                    FixtureCommandStatus::Cancelled | FixtureCommandStatus::Incomplete => None,
                },
                cancelled: matches!(output.status, FixtureCommandStatus::Cancelled),
            },
        );

        let native_completion = output
            .native_completion
            .as_ref()
            .filter(|completion| completion.matches(&plan, &output));
        let capture = if native_completion.is_some() {
            let capture_started = Instant::now();
            match consumer.capture(&host_run, &output, &resolved.capture_dir) {
                Ok(ack)
                    if ack.host_run_id == host_run.id
                        && ack.receipt.host_run_id == host_run.id
                        && ack.receipt.run_id == attempt.envelope.binding.run_id
                        && ack.receipt.attempt_id == attempt.envelope.binding.attempt_id
                        && valid_capture_receipt(&ack.receipt)
                        && native_completion.is_some_and(|proof| {
                            proof.capture_digest() == ack.receipt.digest.as_str()
                        }) =>
                {
                    self.observations.event(
                        &attempt.envelope.binding.consumer,
                        &attempt.envelope.binding.run_id,
                        &attempt.envelope.binding.attempt_id,
                        Event::CaptureCompleted {
                            reference: ack.receipt.reference.clone(),
                            digest: ack.receipt.digest.clone(),
                            elapsed_ms: elapsed_millis(capture_started),
                        },
                    );
                    Some(ack.receipt)
                }
                _ => {
                    self.observations.event(
                        &attempt.envelope.binding.consumer,
                        &attempt.envelope.binding.run_id,
                        &attempt.envelope.binding.attempt_id,
                        Event::CaptureUnavailable,
                    );
                    None
                }
            }
        } else {
            self.observations.event(
                &attempt.envelope.binding.consumer,
                &attempt.envelope.binding.run_id,
                &attempt.envelope.binding.attempt_id,
                Event::CaptureUnavailable,
            );
            None
        };
        if let Some(source_map) = resolved.source_map.as_ref() {
            let qualified = match (
                output.source_map_receipt.as_ref(),
                output.portability_evidence.as_ref(),
                capture.as_ref(),
            ) {
                (Some(receipt), Some(evidence), Some(capture_receipt)) => {
                    receipt.slot_path == slot_path
                        && native_completion
                            .is_some_and(|completion| completion.binds_source_map_receipt(receipt))
                        && receipt.binds_run(
                            source_map,
                            &acquired.lease_id,
                            &attempt.envelope.binding,
                            &host_run.id,
                            &resolved.capture_dir,
                            &capture_receipt.digest,
                        )
                        && source_map::qualify(source_map, receipt, evidence)
                            == QualificationDisposition::Pass
                }
                _ => false,
            };
            if !qualified {
                let finalization = self.finalize_incomplete(
                    &attempt,
                    &acquired,
                    &token,
                    consumer,
                    Some(&host_run),
                    None,
                );
                self.observations.event(
                    &attempt.envelope.binding.consumer,
                    &attempt.envelope.binding.run_id,
                    &attempt.envelope.binding.attempt_id,
                    Event::Uncertain {
                        reason: "source-map-qualification-incomplete",
                    },
                );
                return result(
                    ExecutionDisposition::Uncertain,
                    Some(host_run),
                    finalization,
                    false,
                );
            }
            if let Some(receipt) = output.source_map_receipt.as_ref() {
                self.observations.set_compile_origins(
                    &attempt.envelope.binding.consumer,
                    &attempt.envelope.binding.run_id,
                    &attempt.envelope.binding.attempt_id,
                    receipt
                        .compile_origins
                        .iter()
                        .map(|origin| origin.tree_digest.clone())
                        .collect(),
                );
                self.observations.event(
                    &attempt.envelope.binding.consumer,
                    &attempt.envelope.binding.run_id,
                    &attempt.envelope.binding.attempt_id,
                    Event::SourceMapQualified {
                        receipt_digest: receipt.receipt_digest(),
                        binary_set_digest: receipt.binary_set_digest(),
                    },
                );
            }
        }
        let finalization = self.finalize_run(
            &attempt,
            &acquired,
            &token,
            consumer,
            Some(&host_run),
            Some(&output),
            native_completion,
            capture,
        );
        let disposition = match &finalization {
            Some(receipt) if receipt.state == SlotState::Idle => ExecutionDisposition::Finished,
            _ => ExecutionDisposition::Uncertain,
        };
        result(disposition, Some(host_run), finalization, false)
    }

    /// Returns a bounded copy of recent correlated fixture observations.
    pub fn observations(&self) -> Vec<RunObservation> {
        self.observations.snapshot()
    }

    /// Number of observations dropped to preserve event and ring bounds.
    pub fn dropped_observations(&self) -> u64 {
        self.observations.dropped()
    }

    fn readiness(
        &self,
        invocation: &ProjectInvocation,
        resolved: &ResolvedInvocation,
    ) -> Result<Readiness, AdapterRefusal> {
        let inventory = self
            .storage
            .inventory(&format!("{}:ready", invocation.request_id))
            .map_err(|error| refusal_for_storage(&error))?;
        readiness(
            &inventory,
            &resolved.partition,
            resolved.workflow.required_slot_bytes,
        )
    }

    fn acquire_once(&self, request: AcquireRequest) -> Result<(AcquireResult, bool), StorageError> {
        match self.storage.acquire(request.clone()) {
            Err(StorageError::Io(_)) => self.storage.acquire(request).map(|result| (result, true)),
            Ok(result) => Ok((result, false)),
            Err(error) => Err(error),
        }
    }

    fn begin_observation(&mut self, resolved: &ResolvedInvocation, attempt: &AuthorizedAttempt) {
        let binding = &attempt.envelope.binding;
        let run = Observation {
            project_component: attempt.envelope.partition.project_component.clone(),
            consumer: binding.consumer.clone(),
            task_id: binding.task_id.clone(),
            workspace_id: binding.workspace_id.clone(),
            run_id: binding.run_id.clone(),
            attempt_id: binding.attempt_id.clone(),
            host_run_id: None,
            workflow: binding.workflow.clone(),
            selector: resolved.workflow.selector.clone(),
            commit: binding.commit.clone(),
            source_tree_digest: binding.source_tree_digest.clone(),
            partition_digest: partition_digest(&resolved.partition),
            config_digest: resolved.partition.config_digest.clone(),
            source_map_policy_digest: resolved
                .source_map
                .as_ref()
                .map(|source_map| source_map.policy_digest().to_owned()),
            source_map_digest: resolved
                .source_map
                .as_ref()
                .map(|source_map| source_map.map_digest().to_owned()),
            compile_origin_tree_digests: Vec::new(),
            backend: OBSERVATION_BACKEND,
            events: Vec::new(),
            settled: false,
        };
        self.observations.begin(run);
    }

    fn record_storage(
        &mut self,
        invocation: &ProjectInvocation,
        resolved: &ResolvedInvocation,
        attempt: &AuthorizedAttempt,
    ) {
        let snapshot = self
            .storage
            .inventory(&format!("{}:storage", invocation.request_id));
        let (accounted_bytes, free_bytes) = match snapshot {
            Ok(inventory) => (inventory.accounted_bytes, inventory.free_bytes),
            Err(_) => (None, None),
        };
        self.observations.event(
            &attempt.envelope.binding.consumer,
            &attempt.envelope.binding.run_id,
            &attempt.envelope.binding.attempt_id,
            Event::Storage {
                accounted_bytes,
                free_bytes,
                reserved_bytes: resolved.workflow.required_slot_bytes,
                write_attribution: super::WriteAttribution::Unavailable,
            },
        );
    }

    fn finalize_incomplete<C: ProjectConsumerBoundary>(
        &mut self,
        attempt: &AuthorizedAttempt,
        acquired: &AcquireResult,
        token: &LeaseToken,
        consumer: &mut C,
        host_run: Option<&HostRunIdentity>,
        output: Option<&FixtureCommandOutput>,
    ) -> Option<FinalizationReceipt> {
        self.finalize_run(
            attempt, acquired, token, consumer, host_run, output, None, None,
        )
    }

    fn finalize_run<C: ProjectConsumerBoundary>(
        &mut self,
        attempt: &AuthorizedAttempt,
        acquired: &AcquireResult,
        token: &LeaseToken,
        consumer: &mut C,
        host_run: Option<&HostRunIdentity>,
        output: Option<&FixtureCommandOutput>,
        native_completion: Option<&native::NativeCompletionCertificate>,
        capture: Option<CaptureReceipt>,
    ) -> Option<FinalizationReceipt> {
        let terminal = consumer.terminal_record(attempt, host_run, output);
        let terminal = terminal.filter(|record| record.run_id == attempt.envelope.binding.run_id);
        let containment = match (native_completion, capture.as_ref()) {
            (Some(completion), Some(capture)) => completion.settlement_evidence(
                &acquired.lease_id,
                &acquired.slot_id,
                acquired
                    .slot_path
                    .as_deref()
                    .unwrap_or_else(|| Path::new("")),
                host_run,
                capture,
                token,
            ),
            _ => None,
        }
        .map(ContainmentEvidence::CooperativeWorkflow)
        .unwrap_or(ContainmentEvidence::Unknown);
        let evidence = SettlementEvidence {
            binding: attempt.envelope.binding.clone(),
            consumer_credential: consumer.settlement_credential(attempt),
            terminal,
            capture,
            containment,
        };
        let finalize_id = format!("{}:finalize", attempt.acquire_id);
        let result = self.storage.finalize(
            &acquired.lease_id,
            Some(token),
            &finalize_id,
            evidence.clone(),
        );
        let (finalization, finalize_retried) = match result {
            Err(StorageError::Io(_)) => (
                self.storage
                    .finalize(&acquired.lease_id, Some(token), &finalize_id, evidence),
                true,
            ),
            result => (result, false),
        };
        match finalization {
            Ok(receipt) => {
                let finalized = Event::Finalized {
                    operation_id: finalize_id,
                    state: receipt.state,
                };
                self.observations.event(
                    &attempt.envelope.binding.consumer,
                    &attempt.envelope.binding.run_id,
                    &attempt.envelope.binding.attempt_id,
                    finalized.clone(),
                );
                if finalize_retried {
                    self.observations.event(
                        &attempt.envelope.binding.consumer,
                        &attempt.envelope.binding.run_id,
                        &attempt.envelope.binding.attempt_id,
                        finalized,
                    );
                }
                if receipt.state == SlotState::Idle {
                    self.observations.settle(
                        &attempt.envelope.binding.consumer,
                        &attempt.envelope.binding.run_id,
                        &attempt.envelope.binding.attempt_id,
                    );
                } else {
                    self.observations.event(
                        &attempt.envelope.binding.consumer,
                        &attempt.envelope.binding.run_id,
                        &attempt.envelope.binding.attempt_id,
                        Event::Uncertain {
                            reason: "completion-proof-incomplete",
                        },
                    );
                }
                Some(receipt)
            }
            Err(_) => {
                self.observations.event(
                    &attempt.envelope.binding.consumer,
                    &attempt.envelope.binding.run_id,
                    &attempt.envelope.binding.attempt_id,
                    Event::Uncertain {
                        reason: "finalization-unconfirmed",
                    },
                );
                None
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Readiness {
    Ready,
    Busy,
    Full,
}

fn readiness(
    inventory: &Inventory,
    partition: &PartitionKey,
    required_bytes: u64,
) -> Result<Readiness, AdapterRefusal> {
    if inventory
        .slots
        .iter()
        .any(|slot| slot.partition == *partition && slot.state != SlotState::Idle)
    {
        return Ok(Readiness::Busy);
    }
    if inventory
        .slots
        .iter()
        .any(|slot| slot.partition == *partition && slot.state == SlotState::Idle)
    {
        return Ok(Readiness::Ready);
    }
    let accounted = inventory.accounted_bytes.ok_or(AdapterRefusal::Full)?;
    let free = inventory.free_bytes.ok_or(AdapterRefusal::Full)?;
    let reclaimable = inventory
        .slots
        .iter()
        .filter(|slot| slot.state == SlotState::Idle)
        .try_fold(0_u64, |total, slot| {
            total.checked_add(slot.measured_bytes?.max(slot.reserved_bytes))
        })
        .unwrap_or(0);
    let projected_accounted = accounted.saturating_sub(reclaimable);
    let projected_free = free.saturating_add(reclaimable);
    let cap_fits = projected_accounted
        .checked_add(required_bytes)
        .is_some_and(|total| total <= inventory.budget.cap_bytes);
    let free_fits = inventory
        .budget
        .free_reserve_bytes
        .checked_add(required_bytes)
        .is_some_and(|required| projected_free >= required);
    if cap_fits && free_fits {
        Ok(Readiness::Ready)
    } else {
        Ok(Readiness::Full)
    }
}

fn wait_once<W: StorageWaitBoundary>(
    waiter: &mut W,
    request: &PendingStorageRequest,
    cancellation: &dyn CancellationProbe,
) -> Result<WaitSignal, BoundaryFailure> {
    waiter.register(request)?;
    if cancellation.is_cancelled() {
        waiter.cancel(&request.request_id)?;
        return Ok(WaitSignal::Cancelled);
    }
    let signal = waiter.wait(&request.request_id, cancellation);
    let cancellation = waiter.cancel(&request.request_id);
    match signal {
        Ok(signal) => {
            cancellation?;
            Ok(signal)
        }
        Err(error) => Err(error),
    }
}

fn pending_request(
    invocation: &ProjectInvocation,
    resolved: &ResolvedInvocation,
) -> PendingStorageRequest {
    PendingStorageRequest {
        request_id: invocation.request_id.clone(),
        consumer_id: invocation.consumer_id.clone(),
        project_id: invocation.descriptor.project_id.clone(),
        workflow: resolved.workflow.workflow.clone(),
        selector: invocation.selector.clone(),
        task_id: invocation.task_id.clone(),
        partition_digest: partition_digest(&resolved.partition),
    }
}

fn authorization_request(
    invocation: &ProjectInvocation,
    resolved: &ResolvedInvocation,
    pending: PendingStorageRequest,
) -> AttemptAuthorizationRequest {
    AttemptAuthorizationRequest {
        pending,
        component_id: invocation.descriptor.component_id.clone(),
        workspace_id: invocation.workspace_id.clone(),
        commit: resolved.commit.clone(),
        source_tree_digest: resolved.source_tree_digest.clone(),
        partition: resolved.partition.clone(),
    }
}

fn valid_authorization(
    invocation: &ProjectInvocation,
    resolved: &ResolvedInvocation,
    attempt: &AuthorizedAttempt,
) -> bool {
    let binding = &attempt.envelope.binding;
    binding.consumer == invocation.consumer_id
        && binding.project == invocation.descriptor.project_id
        && binding.component == invocation.descriptor.component_id
        && binding.workflow == resolved.workflow.workflow
        && binding.task_id == invocation.task_id
        && binding.workspace_id == invocation.workspace_id
        && binding.commit == resolved.commit
        && binding.source_tree_digest == resolved.source_tree_digest
        && attempt.envelope.partition == resolved.partition
        && valid_id(&binding.run_id)
        && valid_id(&binding.attempt_id)
        && valid_id(&attempt.acquire_id)
        && valid_id(&attempt.envelope.admission_reference)
        && !attempt.envelope.admission_credential.expose().is_empty()
}

fn host_request(resolved: &ResolvedInvocation, slot_path: &Path) -> HostRunRequest {
    HostRunRequest {
        selector: resolved.workflow.selector.clone(),
        workflow: resolved.workflow.workflow.clone(),
        profile: resolved.workflow.profile.clone(),
        features: resolved.partition.features.clone(),
        workspace_root: resolved.workspace_root.clone(),
        cargo_build_dir: slot_path.to_path_buf(),
        cargo_target_dir: resolved.target_dir.clone(),
        capture_dir: resolved.capture_dir.clone(),
        source_map: resolved.source_map.clone(),
        environment: resolved.environment.clone(),
    }
}

fn safe_slot_path(slot_path: &Path, resolved: &ResolvedInvocation) -> bool {
    let Ok(canonical) = fs::canonicalize(slot_path) else {
        return false;
    };
    let Ok(metadata) = fs::metadata(&canonical) else {
        return false;
    };
    metadata.is_dir()
        && !paths_overlap(&canonical, &resolved.target_dir)
        && !paths_overlap(&canonical, &resolved.capture_dir)
        && !paths_overlap(&canonical, &resolved.workspace_root)
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn valid_output(output: &FixtureCommandOutput) -> bool {
    !output.output_references.is_empty()
        && output.output_references.iter().all(|reference| {
            let path = Path::new(reference);
            !reference.is_empty()
                && reference.len() <= 256
                && !path.is_absolute()
                && path
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_)))
        })
        && match &output.completion {
            FixtureCompletionProof::Unknown => true,
            FixtureCompletionProof::Stopped {
                reference,
                credential,
            } => valid_id(reference) && !credential.expose().is_empty(),
        }
}

fn valid_capture_receipt(receipt: &CaptureReceipt) -> bool {
    valid_id(&receipt.reference)
        && valid_digest(&receipt.digest)
        && valid_id(&receipt.host_run_id)
        && valid_id(&receipt.run_id)
        && valid_id(&receipt.attempt_id)
        && !receipt.credential.expose().is_empty()
}

fn valid_digest(value: &str) -> bool {
    let digest = value.strip_prefix("sha256:").unwrap_or(value);
    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.as_bytes().contains(&0)
}

fn valid_host_run(host_run: &HostRunIdentity, binding: &RunBinding) -> bool {
    valid_id(&host_run.id)
        && host_run.id != binding.run_id
        && host_run.id != binding.task_id
        && host_run.id != binding.attempt_id
}

fn partition_digest(partition: &PartitionKey) -> String {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    for field in [
        partition.project_component.as_str(),
        partition.workflow.as_str(),
        partition.cargo_provenance.as_str(),
        partition.rustc_provenance.as_str(),
        partition.host.as_str(),
        partition.target.as_str(),
        partition.profile.as_str(),
        partition.config_digest.as_str(),
        partition.environment_digest.as_str(),
        partition.path_identity.as_str(),
    ] {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field.as_bytes());
    }
    for feature in &partition.features {
        digest.update((feature.len() as u64).to_be_bytes());
        digest.update(feature.as_bytes());
    }
    format!("sha256:{:x}", digest.finalize())
}

fn fallback_allowed(invocation: &ProjectInvocation, reason: AdapterRefusal) -> bool {
    invocation.private_fallback_allowed
        && matches!(
            reason,
            AdapterRefusal::UnsupportedInput
                | AdapterRefusal::AmbiguousInput
                | AdapterRefusal::InputUnknown
                | AdapterRefusal::UnsupportedToolchain
                | AdapterRefusal::DaemonUnavailable
                | AdapterRefusal::Full
        )
}

fn refusal_for_storage(error: &StorageError) -> AdapterRefusal {
    match error {
        StorageError::Io(_) | StorageError::Protocol(_) => AdapterRefusal::DaemonUnavailable,
        StorageError::Refused(crate::storage::RefusalReason::LeaseUnavailable)
        | StorageError::Refused(crate::storage::RefusalReason::NoIdleStorage) => {
            AdapterRefusal::Busy
        }
        StorageError::Refused(
            crate::storage::RefusalReason::UnknownAccounting
            | crate::storage::RefusalReason::FreeSpaceUnavailable
            | crate::storage::RefusalReason::FreeSpaceReserve
            | crate::storage::RefusalReason::BudgetExceeded,
        ) => AdapterRefusal::Full,
        StorageError::Refused(crate::storage::RefusalReason::AlreadyRunning) => {
            AdapterRefusal::DaemonUnavailable
        }
        StorageError::InvalidInput(_) | StorageError::Conflict | StorageError::Unauthorized => {
            AdapterRefusal::AmbiguousInput
        }
    }
}

fn refusal_label(reason: AdapterRefusal) -> &'static str {
    match reason {
        AdapterRefusal::AmbiguousInput => "ambiguous-input",
        AdapterRefusal::UnsupportedInput => "unsupported-input",
        AdapterRefusal::InputUnknown => "input-unknown",
        AdapterRefusal::UnsupportedToolchain => "unsupported-toolchain",
        AdapterRefusal::Busy => "busy",
        AdapterRefusal::Full => "full",
        AdapterRefusal::DaemonUnavailable => "daemon-unavailable",
        AdapterRefusal::WaitUnavailable => "wait-unavailable",
        AdapterRefusal::NotAdmitted => "not-admitted",
    }
}

fn result(
    disposition: ExecutionDisposition,
    host_run: Option<HostRunIdentity>,
    finalization: Option<FinalizationReceipt>,
    private_build_allowed: bool,
) -> ProjectExecutionResult {
    ProjectExecutionResult {
        disposition,
        host_run,
        finalization,
        private_build_allowed,
    }
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
