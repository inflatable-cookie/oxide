use super::*;
use crate::storage::{
    Budget, Daemon, DaemonConfig, EvidenceAuthority, FreeSpaceProbe, RunEnvelope, TerminalOutcome,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(1);

struct FixtureRoot(PathBuf);

impl FixtureRoot {
    fn new() -> Self {
        let parent = fs::canonicalize(std::env::temp_dir()).expect("test TMPDIR exists");
        loop {
            let id = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("ox-fixture-{}-{}", std::process::id(), id));
            match fs::create_dir(&path) {
                Ok(()) => {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                        .expect("private fixture root");
                    return Self(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create task fixture root: {}", error),
            }
        }
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir(&path).expect("create isolated fixture directory");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .expect("private fixture directory");
        path
    }
}

impl Drop for FixtureRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct FixtureAuthority;

impl EvidenceAuthority for FixtureAuthority {
    fn verify_admission(&self, envelope: &RunEnvelope) -> bool {
        envelope.admission_reference == "fixture-admission"
            && envelope.admission_credential.expose() == "admission-proof"
    }

    fn verify_settlement(&self, evidence: &SettlementEvidence) -> bool {
        evidence.consumer_credential.expose() == "consumer-proof"
            && evidence
                .terminal
                .as_ref()
                .is_none_or(|record| record.credential.expose() == "terminal-proof")
            && evidence
                .capture
                .as_ref()
                .is_none_or(|receipt| receipt.credential.expose() == "capture-proof")
            && match &evidence.containment {
                ContainmentEvidence::DescendantsStopped { credential, .. } => {
                    credential.expose() == "fixture-guardian-proof"
                }
                _ => true,
            }
    }
}

struct FixedFreeSpace;

impl FreeSpaceProbe for FixedFreeSpace {
    fn available_bytes(&self, _: &Path) -> Result<Option<u64>, StorageError> {
        Ok(Some(1_000_000))
    }
}

struct RunningDaemon {
    client: Client,
    shutdown: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<Result<(), StorageError>>>,
}

impl RunningDaemon {
    fn start(root: &Path) -> Self {
        let daemon = Daemon::bind(
            DaemonConfig {
                root: root.to_path_buf(),
                budget: Budget {
                    cap_bytes: 10_000,
                    free_reserve_bytes: 100,
                },
            },
            Arc::new(FixtureAuthority),
            Arc::new(FixedFreeSpace),
        )
        .expect("bind fixture daemon");
        let client = Client::new(daemon.socket_path().to_path_buf());
        let (shutdown, receiver) = mpsc::channel();
        let thread = thread::spawn(move || daemon.serve(receiver));
        Self {
            client,
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }

    fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            assert!(thread.join().expect("fixture daemon joins").is_ok());
        }
    }
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        self.stop();
    }
}

struct FixtureConsumer {
    attempt_id: String,
    run_id: String,
    host_run_id: String,
    submit_calls: usize,
    refusal_calls: usize,
    capture_calls: usize,
    admit: bool,
    fail_submission: bool,
    fail_capture: bool,
    wrong_capture_run: bool,
}

impl FixtureConsumer {
    fn new() -> Self {
        Self {
            attempt_id: "attempt-1".to_owned(),
            run_id: "consumer-run-1".to_owned(),
            host_run_id: "host-run-1".to_owned(),
            submit_calls: 0,
            refusal_calls: 0,
            capture_calls: 0,
            admit: true,
            fail_submission: false,
            fail_capture: false,
            wrong_capture_run: false,
        }
    }
}

impl ProjectConsumerBoundary for FixtureConsumer {
    fn authorize_attempt(
        &mut self,
        request: &AttemptAuthorizationRequest,
    ) -> Result<Option<AuthorizedAttempt>, BoundaryFailure> {
        if !self.admit {
            return Ok(None);
        }
        let binding = RunBinding {
            consumer: request.pending.consumer_id.clone(),
            project: request.pending.project_id.clone(),
            component: request.component_id.clone(),
            workflow: request.pending.workflow.clone(),
            task_id: request.pending.task_id.clone(),
            workspace_id: request.workspace_id.clone(),
            run_id: self.run_id.clone(),
            attempt_id: self.attempt_id.clone(),
            commit: request.commit.clone(),
            source_tree_digest: request.source_tree_digest.clone(),
        };
        Ok(Some(AuthorizedAttempt {
            envelope: RunEnvelope {
                binding,
                partition: request.partition.clone(),
                admission_reference: "fixture-admission".to_owned(),
                admission_credential: Credential::new("admission-proof"),
            },
            acquire_id: format!("acquire-{}", self.attempt_id),
        }))
    }

    fn on_storage_refusal(&mut self, _: &AuthorizedAttempt, _: AdapterRefusal) {
        self.refusal_calls += 1;
    }

    fn cancel_unstarted_attempt(&mut self, _: &AuthorizedAttempt) {}

    fn submit_host_run(
        &mut self,
        _: &AuthorizedAttempt,
        request: &HostRunRequest,
    ) -> Result<HostRunIdentity, BoundaryFailure> {
        self.submit_calls += 1;
        assert_eq!(request.selector, "test:fixture");
        assert_eq!(request.workflow, "rust:test");
        assert!(request
            .cargo_build_dir
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("slot-")));
        assert_ne!(request.cargo_build_dir, request.cargo_target_dir);
        assert_ne!(request.cargo_target_dir, request.capture_dir);
        let overrides = request.cargo_directory_overrides();
        assert_eq!(overrides[0].0.to_string_lossy(), "CARGO_BUILD_BUILD_DIR");
        assert_eq!(
            overrides[0].1.as_os_str(),
            request.cargo_build_dir.as_os_str()
        );
        assert_eq!(overrides[1].0.to_string_lossy(), "CARGO_TARGET_DIR");
        assert_eq!(
            overrides[1].1.as_os_str(),
            request.cargo_target_dir.as_os_str()
        );
        if self.fail_submission {
            return Err(BoundaryFailure::Unavailable);
        }
        Ok(HostRunIdentity {
            id: self.host_run_id.clone(),
        })
    }

    fn capture(
        &mut self,
        host_run: &HostRunIdentity,
        output: &FixtureCommandOutput,
        capture_dir: &Path,
    ) -> Result<CaptureAcknowledgement, BoundaryFailure> {
        self.capture_calls += 1;
        if self.fail_capture {
            return Err(BoundaryFailure::Unavailable);
        }
        for reference in &output.output_references {
            if !capture_dir.join(reference).is_file() {
                return Err(BoundaryFailure::InvalidIdentity);
            }
        }
        let host_run_id = if self.wrong_capture_run {
            "another-host-run".to_owned()
        } else {
            host_run.id.clone()
        };
        Ok(CaptureAcknowledgement {
            host_run_id,
            receipt: CaptureReceipt {
                reference: format!("capture/{}", self.run_id),
                digest: digest(b"fixture-capture"),
                credential: Credential::new("capture-proof"),
            },
        })
    }

    fn terminal_record(
        &mut self,
        attempt: &AuthorizedAttempt,
        _: Option<&HostRunIdentity>,
        output: Option<&FixtureCommandOutput>,
    ) -> Option<TerminalRecord> {
        let outcome = output.map(|output| match output.status {
            FixtureCommandStatus::Exited(0) => TerminalOutcome::Passed,
            FixtureCommandStatus::Exited(_) => TerminalOutcome::Failed,
            FixtureCommandStatus::Cancelled => TerminalOutcome::Cancelled,
        })?;
        Some(TerminalRecord {
            run_id: attempt.envelope.binding.run_id.clone(),
            outcome,
            credential: Credential::new("terminal-proof"),
        })
    }

    fn settlement_credential(&mut self, _: &AuthorizedAttempt) -> Credential {
        Credential::new("consumer-proof")
    }
}

struct FixtureRunner {
    calls: usize,
    status: FixtureCommandStatus,
    proof: FixtureCompletionProof,
}

impl FixtureRunner {
    fn complete() -> Self {
        Self {
            calls: 0,
            status: FixtureCommandStatus::Exited(0),
            proof: FixtureCompletionProof::Stopped {
                reference: "fixture-complete".to_owned(),
                credential: Credential::new("fixture-guardian-proof"),
            },
        }
    }
}

impl FixtureCommandRunner for FixtureRunner {
    fn execute(
        &mut self,
        plan: &FixtureExecutionPlan,
        _: &dyn CancellationProbe,
    ) -> Result<FixtureCommandOutput, BoundaryFailure> {
        self.calls += 1;
        assert_eq!(plan.command.environment.len(), 2);
        assert_eq!(plan.command.environment[0].name, "RUSTFLAGS");
        assert_eq!(plan.command.environment[1].name, "SDKROOT");
        fs::write(
            plan.command
                .cargo_build_dir
                .join("fixture-intermediate.bin"),
            b"units",
        )
        .map_err(|_| BoundaryFailure::Unavailable)?;
        fs::write(
            plan.command.capture_dir.join("stdout.log"),
            b"fixture output",
        )
        .map_err(|_| BoundaryFailure::Unavailable)?;
        Ok(FixtureCommandOutput {
            status: self.status,
            output_references: vec!["stdout.log".to_owned()],
            compile_elapsed_ms: Some(7),
            test_elapsed_ms: Some(5),
            reused_units: Some(2),
            rebuilt_units: Some(1),
            completion: self.proof.clone(),
        })
    }
}

struct GatedRunner {
    runner: FixtureRunner,
    entered: mpsc::Sender<()>,
    resume: mpsc::Receiver<()>,
}

impl FixtureCommandRunner for GatedRunner {
    fn execute(
        &mut self,
        plan: &FixtureExecutionPlan,
        cancellation: &dyn CancellationProbe,
    ) -> Result<FixtureCommandOutput, BoundaryFailure> {
        let output = self.runner.execute(plan, cancellation)?;
        self.entered
            .send(())
            .map_err(|_| BoundaryFailure::Unavailable)?;
        self.resume
            .recv()
            .map_err(|_| BoundaryFailure::Unavailable)?;
        Ok(output)
    }
}

#[derive(Default)]
struct NoWait;

impl StorageWaitBoundary for NoWait {
    fn register(&mut self, _: &PendingStorageRequest) -> Result<(), BoundaryFailure> {
        Err(BoundaryFailure::Rejected)
    }

    fn wait(&mut self, _: &str, _: &dyn CancellationProbe) -> Result<WaitSignal, BoundaryFailure> {
        Err(BoundaryFailure::Rejected)
    }

    fn cancel(&mut self, _: &str) -> Result<(), BoundaryFailure> {
        Ok(())
    }
}

struct ScriptedWaiter {
    signal: WaitSignal,
    registrations: usize,
    waits: usize,
    cancellations: usize,
}

impl ScriptedWaiter {
    fn new(signal: WaitSignal) -> Self {
        Self {
            signal,
            registrations: 0,
            waits: 0,
            cancellations: 0,
        }
    }
}

impl StorageWaitBoundary for ScriptedWaiter {
    fn register(&mut self, request: &PendingStorageRequest) -> Result<(), BoundaryFailure> {
        assert_eq!(request.task_id, "task-1");
        self.registrations += 1;
        Ok(())
    }

    fn wait(&mut self, _: &str, _: &dyn CancellationProbe) -> Result<WaitSignal, BoundaryFailure> {
        self.waits += 1;
        Ok(self.signal)
    }

    fn cancel(&mut self, _: &str) -> Result<(), BoundaryFailure> {
        self.cancellations += 1;
        Ok(())
    }
}

struct NeverCancelled;

impl CancellationProbe for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

struct Cancelled;

impl CancellationProbe for Cancelled {
    fn is_cancelled(&self) -> bool {
        true
    }
}

struct FaultyStorage {
    client: Client,
    lose_acquire_reply: AtomicBool,
    lose_finalize_reply: AtomicBool,
    acquire_calls: AtomicUsize,
    finalize_calls: AtomicUsize,
}

impl FaultyStorage {
    fn new(client: Client) -> Self {
        Self {
            client,
            lose_acquire_reply: AtomicBool::new(true),
            lose_finalize_reply: AtomicBool::new(true),
            acquire_calls: AtomicUsize::new(0),
            finalize_calls: AtomicUsize::new(0),
        }
    }
}

impl StorageOperations for FaultyStorage {
    fn inventory(&self, request_id: &str) -> Result<Inventory, StorageError> {
        self.client.inventory(request_id)
    }

    fn acquire(&self, request: AcquireRequest) -> Result<AcquireResult, StorageError> {
        self.acquire_calls.fetch_add(1, Ordering::SeqCst);
        let result = self.client.acquire(request)?;
        if self.lose_acquire_reply.swap(false, Ordering::SeqCst) {
            Err(StorageError::Io("fixture lost acquire reply".to_owned()))
        } else {
            Ok(result)
        }
    }

    fn finalize(
        &self,
        lease_id: &str,
        token: Option<&LeaseToken>,
        finalize_id: &str,
        evidence: SettlementEvidence,
    ) -> Result<FinalizationReceipt, StorageError> {
        self.finalize_calls.fetch_add(1, Ordering::SeqCst);
        let result = self
            .client
            .finalize(lease_id, token, finalize_id, evidence)?;
        if self.lose_finalize_reply.swap(false, Ordering::SeqCst) {
            Err(StorageError::Io("fixture lost finalize reply".to_owned()))
        } else {
            Ok(result)
        }
    }
}

fn fixture_invocation(root: &FixtureRoot) -> ProjectInvocation {
    let workspace = root.dir("workspace");
    let target = root.dir("target");
    let capture = root.dir("capture");
    ProjectInvocation {
        consumer_id: "fixture-consumer".to_owned(),
        descriptor: ProjectDescriptor {
            project_id: "fixture-project".to_owned(),
            component_id: "fixture-component".to_owned(),
            workspace_root: workspace,
            workflows: vec![WorkflowOptIn {
                workflow: "rust:test".to_owned(),
                selector: "test:fixture".to_owned(),
                profile: "test".to_owned(),
                features: vec!["default".to_owned()],
                required_slot_bytes: 64,
                cargo_config_keys: vec!["build.rustflags".to_owned()],
                environment_keys: vec!["RUSTFLAGS".to_owned(), "SDKROOT".to_owned()],
            }],
        },
        workflow: "rust:test".to_owned(),
        selector: "test:fixture".to_owned(),
        request_id: "request-1".to_owned(),
        task_id: "task-1".to_owned(),
        workspace_id: "workspace-1".to_owned(),
        commit: "1111111111111111111111111111111111111111".to_owned(),
        source_tree_digest: digest(b"fixture-source"),
        inputs: EffectiveBuildInputs {
            cargo: ToolchainInput {
                version: "cargo 1.97.1".to_owned(),
                executable_digest: digest(b"cargo-binary"),
            },
            rustc: ToolchainInput {
                version: "rustc 1.97.1".to_owned(),
                executable_digest: digest(b"rustc-binary"),
            },
            host: "aarch64-apple-darwin".to_owned(),
            target: "aarch64-apple-darwin".to_owned(),
            cargo_configuration: EffectiveInputSet::Complete(vec![EffectiveValue {
                name: "build.rustflags".to_owned(),
                value: None,
            }]),
            environment: EffectiveInputSet::Complete(vec![
                EffectiveValue {
                    name: "RUSTFLAGS".to_owned(),
                    value: None,
                },
                EffectiveValue {
                    name: "SDKROOT".to_owned(),
                    value: None,
                },
            ]),
        },
        target_dir: target,
        capture_dir: capture,
        private_fallback_allowed: false,
        wait_once: false,
    }
}

fn direct_claim(invocation: &ProjectInvocation, run_id: &str, attempt_id: &str) -> AcquireRequest {
    let resolved = match resolve::resolve(invocation) {
        Resolution::Ready(resolved) => resolved,
        _ => panic!("fixture invocation resolves"),
    };
    AcquireRequest {
        envelope: RunEnvelope {
            binding: RunBinding {
                consumer: invocation.consumer_id.clone(),
                project: invocation.descriptor.project_id.clone(),
                component: invocation.descriptor.component_id.clone(),
                workflow: invocation.workflow.clone(),
                task_id: invocation.task_id.clone(),
                workspace_id: invocation.workspace_id.clone(),
                run_id: run_id.to_owned(),
                attempt_id: attempt_id.to_owned(),
                commit: invocation.commit.clone(),
                source_tree_digest: invocation.source_tree_digest.clone(),
            },
            partition: resolved.partition,
            admission_reference: "fixture-admission".to_owned(),
            admission_credential: Credential::new("admission-proof"),
        },
        acquire_id: format!("direct-{}", attempt_id),
        required_bytes: 64,
    }
}

fn digest(value: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{:x}", Sha256::digest(value))
}

#[test]
fn fixture_adapter_resolves_acquires_captures_and_finalizes() {
    let root = FixtureRoot::new();
    let storage_root = root.dir("storage");
    let daemon = RunningDaemon::start(&storage_root);
    let mut invocation = fixture_invocation(&root);
    if let EffectiveInputSet::Complete(environment) = &mut invocation.inputs.environment {
        environment[0].value = Some("fixture-private-value".to_owned());
    }
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut consumer = FixtureConsumer::new();
    let mut waiter = NoWait;
    let mut runner = FixtureRunner::complete();
    let result = adapter.execute(
        &invocation,
        &mut consumer,
        &mut waiter,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(result.disposition, ExecutionDisposition::Finished);
    assert_eq!(runner.calls, 1);
    assert_eq!(consumer.submit_calls, 1);
    assert_eq!(consumer.capture_calls, 1);
    let host_run = result.host_run.expect("separate host-run identity");
    assert_ne!(host_run.id, "consumer-run-1");
    assert_eq!(
        result.finalization.expect("finalization").state,
        SlotState::Idle
    );
    let observations = adapter.observations();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].host_run_id.as_deref(), Some("host-run-1"));
    assert_eq!(
        observations[0].source_tree_digest,
        invocation.source_tree_digest
    );
    assert_eq!(observations[0].task_id, "task-1");
    assert_eq!(observations[0].run_id, "consumer-run-1");
    assert_eq!(observations[0].workflow, "rust:test");
    assert_eq!(observations[0].selector, "test:fixture");
    assert_eq!(observations[0].commit, invocation.commit);
    assert_eq!(observations[0].backend, "cargo-build-dir");
    assert!(observations[0]
        .events
        .iter()
        .any(|event| matches!(&event.kind, ObservationKind::CaptureCompleted { .. })));
    assert!(format!("{:?}", observations)
        .find("admission-proof")
        .is_none());
    assert!(format!("{:?}", observations)
        .find("fixture-private-value")
        .is_none());
    assert!(observations[0].events.len() <= MAX_EVENTS_PER_RUN);
    assert!(observations[0]
        .events
        .iter()
        .all(|event| format!("{:?}", event).len() <= MAX_EVENT_BYTES));
}

#[test]
fn busy_after_one_wakeup_returns_before_host_submission() {
    let root = FixtureRoot::new();
    let storage_root = root.dir("storage");
    let daemon = RunningDaemon::start(&storage_root);
    let mut invocation = fixture_invocation(&root);
    invocation.wait_once = true;
    daemon
        .client
        .acquire(direct_claim(&invocation, "held-run", "held-attempt"))
        .expect("hold the only compatible slot");

    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut consumer = FixtureConsumer::new();
    let mut waiter = ScriptedWaiter::new(WaitSignal::Wake);
    let mut runner = FixtureRunner::complete();
    let result = adapter.execute(
        &invocation,
        &mut consumer,
        &mut waiter,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(
        result.disposition,
        ExecutionDisposition::Refused(AdapterRefusal::Busy)
    );
    assert_eq!(waiter.registrations, 1);
    assert_eq!(waiter.waits, 1);
    assert_eq!(waiter.cancellations, 1);
    assert_eq!(consumer.refusal_calls, 1);
    assert_eq!(consumer.submit_calls, 0);
    assert_eq!(runner.calls, 0);
    assert_eq!(
        daemon.client.inventory("still-held").unwrap().slots[0].state,
        SlotState::Leased
    );
}

#[test]
fn competing_project_adapters_do_not_share_a_leased_slot() {
    let root = FixtureRoot::new();
    let storage_root = root.dir("storage");
    let daemon = RunningDaemon::start(&storage_root);
    let first_invocation = fixture_invocation(&root);
    let mut second_invocation = first_invocation.clone();
    second_invocation.request_id = "request-2".to_owned();
    second_invocation.task_id = "task-2".to_owned();
    second_invocation.workspace_id = "workspace-2".to_owned();
    second_invocation.target_dir = root.dir("target-second");
    second_invocation.capture_dir = root.dir("capture-second");

    let first_client = daemon.client.clone();
    let (entered_sender, entered_receiver) = mpsc::channel();
    let (resume_sender, resume_receiver) = mpsc::channel();
    let first = thread::spawn(move || {
        let mut adapter = ProjectAdapter::new(first_client);
        let mut consumer = FixtureConsumer::new();
        let mut runner = GatedRunner {
            runner: FixtureRunner::complete(),
            entered: entered_sender,
            resume: resume_receiver,
        };
        let result = adapter.execute(
            &first_invocation,
            &mut consumer,
            &mut NoWait,
            &mut runner,
            &NeverCancelled,
        );
        result
    });

    entered_receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("first adapter holds the slot while its command is active");

    let mut second_adapter = ProjectAdapter::new(daemon.client.clone());
    let mut second_consumer = FixtureConsumer::new();
    second_consumer.attempt_id = "attempt-2".to_owned();
    second_consumer.run_id = "consumer-run-2".to_owned();
    second_consumer.host_run_id = "host-run-2".to_owned();
    let mut second_runner = FixtureRunner::complete();
    let second_result = second_adapter.execute(
        &second_invocation,
        &mut second_consumer,
        &mut NoWait,
        &mut second_runner,
        &NeverCancelled,
    );

    resume_sender
        .send(())
        .expect("release first fixture command");
    let first_result = first.join().expect("first adapter joins");

    assert_eq!(
        second_result.disposition,
        ExecutionDisposition::Refused(AdapterRefusal::Busy)
    );
    assert_eq!(second_consumer.refusal_calls, 1);
    assert_eq!(second_consumer.submit_calls, 0);
    assert_eq!(second_runner.calls, 0);
    assert_eq!(first_result.disposition, ExecutionDisposition::Finished);
    assert_eq!(
        first_result.finalization.expect("first finalization").state,
        SlotState::Idle
    );
}

#[test]
fn unknown_inputs_and_unlisted_selectors_refuse_without_storage_admission() {
    let root = FixtureRoot::new();
    let storage_root = root.dir("storage");
    let daemon = RunningDaemon::start(&storage_root);
    let mut invocation = fixture_invocation(&root);
    invocation.private_fallback_allowed = true;
    invocation.inputs.environment = EffectiveInputSet::Unknown;
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut consumer = FixtureConsumer::new();
    let mut waiter = NoWait;
    let mut runner = FixtureRunner::complete();
    let result = adapter.execute(
        &invocation,
        &mut consumer,
        &mut waiter,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(
        result.disposition,
        ExecutionDisposition::Refused(AdapterRefusal::InputUnknown)
    );
    assert!(result.private_build_allowed);
    assert_eq!(consumer.submit_calls, 0);
    assert_eq!(runner.calls, 0);

    let unlisted_root = FixtureRoot::new();
    let mut unlisted = fixture_invocation(&unlisted_root);
    unlisted.selector = "test:unlisted".to_owned();
    let result = adapter.execute(
        &unlisted,
        &mut consumer,
        &mut waiter,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(result.disposition, ExecutionDisposition::NotOptedIn);
    assert!(result.private_build_allowed);
    assert_eq!(runner.calls, 0);
}

#[test]
fn wrong_run_capture_acknowledgement_keeps_storage_uncertain() {
    let root = FixtureRoot::new();
    let storage_root = root.dir("storage");
    let daemon = RunningDaemon::start(&storage_root);
    let invocation = fixture_invocation(&root);
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut consumer = FixtureConsumer::new();
    consumer.wrong_capture_run = true;
    let mut waiter = NoWait;
    let mut runner = FixtureRunner::complete();
    let result = adapter.execute(
        &invocation,
        &mut consumer,
        &mut waiter,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(result.disposition, ExecutionDisposition::Uncertain);
    assert_eq!(
        result
            .finalization
            .expect("incomplete evidence is finalized conservatively")
            .state,
        SlotState::Uncertain
    );
    assert_eq!(consumer.capture_calls, 1);
    let mut next_consumer = FixtureConsumer::new();
    next_consumer.attempt_id = "attempt-2".to_owned();
    next_consumer.run_id = "consumer-run-2".to_owned();
    let mut next_runner = FixtureRunner::complete();
    let next = adapter.execute(
        &invocation,
        &mut next_consumer,
        &mut NoWait,
        &mut next_runner,
        &NeverCancelled,
    );
    assert_eq!(
        next.disposition,
        ExecutionDisposition::Refused(AdapterRefusal::Busy)
    );
    assert_eq!(next_runner.calls, 0);
}

#[test]
fn capture_failure_keeps_storage_unavailable() {
    let root = FixtureRoot::new();
    let storage_root = root.dir("storage");
    let daemon = RunningDaemon::start(&storage_root);
    let invocation = fixture_invocation(&root);
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut consumer = FixtureConsumer::new();
    consumer.fail_capture = true;
    let mut runner = FixtureRunner::complete();
    let result = adapter.execute(
        &invocation,
        &mut consumer,
        &mut NoWait,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(result.disposition, ExecutionDisposition::Uncertain);
    assert_eq!(consumer.capture_calls, 1);
    assert_eq!(runner.calls, 1);
    assert_eq!(
        result
            .finalization
            .expect("capture failure is finalized conservatively")
            .state,
        SlotState::Uncertain
    );
    assert!(adapter.observations()[0]
        .events
        .iter()
        .any(|event| matches!(&event.kind, ObservationKind::CaptureUnavailable)));
}

#[test]
fn cancelled_fixture_and_daemon_restart_leave_the_lease_unavailable() {
    let root = FixtureRoot::new();
    let storage_root = root.dir("storage");
    let mut daemon = RunningDaemon::start(&storage_root);
    let invocation = fixture_invocation(&root);
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut consumer = FixtureConsumer::new();
    let mut waiter = NoWait;
    let mut runner = FixtureRunner {
        calls: 0,
        status: FixtureCommandStatus::Cancelled,
        proof: FixtureCompletionProof::Unknown,
    };
    let result = adapter.execute(
        &invocation,
        &mut consumer,
        &mut waiter,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(result.disposition, ExecutionDisposition::Uncertain);
    assert_eq!(consumer.capture_calls, 0);
    assert_eq!(
        daemon.client.inventory("cancelled").unwrap().slots[0].state,
        SlotState::Uncertain
    );

    daemon.stop();
    let restarted = RunningDaemon::start(&storage_root);
    assert_eq!(
        restarted.client.inventory("after-restart").unwrap().slots[0].state,
        SlotState::Uncertain
    );
    let mut later_consumer = FixtureConsumer::new();
    later_consumer.attempt_id = "attempt-later".to_owned();
    later_consumer.run_id = "consumer-run-later".to_owned();
    let mut later_runner = FixtureRunner::complete();
    let later = ProjectAdapter::new(restarted.client.clone()).execute(
        &invocation,
        &mut later_consumer,
        &mut NoWait,
        &mut later_runner,
        &NeverCancelled,
    );
    assert_eq!(
        later.disposition,
        ExecutionDisposition::Refused(AdapterRefusal::Busy)
    );
    assert_eq!(later_runner.calls, 0);
}

#[test]
fn lost_acquire_and_finalize_replies_reuse_the_same_operation_ids() {
    let root = FixtureRoot::new();
    let storage_root = root.dir("storage");
    let daemon = RunningDaemon::start(&storage_root);
    let storage = FaultyStorage::new(daemon.client.clone());
    let mut adapter = ProjectAdapter::new(storage);
    let invocation = fixture_invocation(&root);
    let mut consumer = FixtureConsumer::new();
    let mut runner = FixtureRunner::complete();
    let result = adapter.execute(
        &invocation,
        &mut consumer,
        &mut NoWait,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(result.disposition, ExecutionDisposition::Finished);
    assert_eq!(runner.calls, 1);
    assert_eq!(
        adapter.storage.acquire_calls.load(Ordering::SeqCst),
        2,
        "acquire reply is retried once with its original ID"
    );
    assert_eq!(
        adapter.storage.finalize_calls.load(Ordering::SeqCst),
        2,
        "finalize reply is retried once with its original ID"
    );
    let observations = adapter.observations();
    let events = &observations[0].events;
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        ObservationKind::Requested { operation_id } if operation_id == "acquire-attempt-1"
    ) && event.retry_count == 1));
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        ObservationKind::Finalized {
            state: SlotState::Idle,
            ..
        }
    ) && event.retry_count == 1));
}

#[test]
fn daemon_unavailable_returns_only_the_consumer_approved_private_fallback() {
    let root = FixtureRoot::new();
    let invocation = fixture_invocation(&root);
    let missing_socket = root.0.join("missing-daemon.sock");
    let mut adapter = ProjectAdapter::new(Client::new(missing_socket));
    let mut consumer = FixtureConsumer::new();
    let mut runner = FixtureRunner::complete();
    let mut invocation = invocation;
    invocation.private_fallback_allowed = true;
    let result = adapter.execute(
        &invocation,
        &mut consumer,
        &mut NoWait,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(
        result.disposition,
        ExecutionDisposition::Refused(AdapterRefusal::DaemonUnavailable)
    );
    assert!(result.private_build_allowed);
    assert_eq!(consumer.submit_calls, 0);
    assert_eq!(runner.calls, 0);
}

#[test]
fn cancellation_during_wait_removes_registration_without_admission() {
    let root = FixtureRoot::new();
    let storage_root = root.dir("storage");
    let daemon = RunningDaemon::start(&storage_root);
    let mut invocation = fixture_invocation(&root);
    invocation.wait_once = true;
    daemon
        .client
        .acquire(direct_claim(&invocation, "held-run", "held-attempt"))
        .expect("occupy compatible storage");
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut waiter = ScriptedWaiter::new(WaitSignal::Cancelled);
    let mut consumer = FixtureConsumer::new();
    let mut runner = FixtureRunner::complete();
    let result = adapter.execute(
        &invocation,
        &mut consumer,
        &mut waiter,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(
        result.disposition,
        ExecutionDisposition::CancelledBeforeAcquire
    );
    assert_eq!(waiter.registrations, 1);
    assert_eq!(waiter.waits, 1);
    assert_eq!(waiter.cancellations, 1);
    assert_eq!(consumer.submit_calls, 0);
    assert_eq!(runner.calls, 0);
}

#[test]
fn pre_cancel_and_unsupported_cargo_do_not_acquire() {
    let root = FixtureRoot::new();
    let storage_root = root.dir("storage");
    let daemon = RunningDaemon::start(&storage_root);
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut consumer = FixtureConsumer::new();
    let mut runner = FixtureRunner::complete();
    let invocation = fixture_invocation(&root);
    let cancelled = adapter.execute(
        &invocation,
        &mut consumer,
        &mut NoWait,
        &mut runner,
        &Cancelled,
    );
    assert_eq!(
        cancelled.disposition,
        ExecutionDisposition::CancelledBeforeAcquire
    );
    assert_eq!(consumer.submit_calls, 0);

    let mut old_cargo = invocation;
    old_cargo.inputs.cargo.version = "cargo 1.90.0".to_owned();
    let refused = adapter.execute(
        &old_cargo,
        &mut consumer,
        &mut NoWait,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(
        refused.disposition,
        ExecutionDisposition::Refused(AdapterRefusal::UnsupportedToolchain)
    );
    assert_eq!(consumer.submit_calls, 0);
    assert!(daemon
        .client
        .inventory("no-old-cargo-slot")
        .unwrap()
        .slots
        .is_empty());
}
