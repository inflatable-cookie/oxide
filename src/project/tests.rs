use super::*;
use crate::storage::{
    Budget, Daemon, DaemonConfig, EvidenceAuthority, FreeSpaceProbe, RunEnvelope, TerminalOutcome,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(1);

struct FixtureRoot {
    path: PathBuf,
    preserve: bool,
}

impl FixtureRoot {
    fn new() -> Self {
        Self::create("project-tests", false)
    }

    fn scenario(name: &str) -> Self {
        let mut root = Self::create(&format!("scenarios/{}", name), true);
        root.preserve = true;
        root
    }

    fn create(name: &str, preserve: bool) -> Self {
        let parent = task_scratch_root().join(name);
        fs::create_dir_all(&parent).expect("create under task-owned scratch root");
        loop {
            let id = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("ox-fixture-{}-{}", std::process::id(), id));
            match fs::create_dir(&path) {
                Ok(()) => {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                        .expect("private fixture root");
                    return Self { path, preserve };
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create task fixture root: {}", error),
            }
        }
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.path.join(name);
        fs::create_dir(&path).expect("create isolated fixture directory");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .expect("private fixture directory");
        path
    }
}

impl Drop for FixtureRoot {
    fn drop(&mut self) {
        if !self.preserve {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn task_scratch_root() -> PathBuf {
    let root = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let canonical = fs::canonicalize(&root).expect("task-owned scratch root exists");
    assert_eq!(canonical, root, "scratch root is canonical");
    canonical
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
                ContainmentEvidence::CooperativeWorkflow(completion) => {
                    !completion.signature.is_empty()
                }
                _ => true,
            }
    }
}

struct FixedFreeSpace(Option<u64>);

impl FreeSpaceProbe for FixedFreeSpace {
    fn available_bytes(&self, _: &Path) -> Result<Option<u64>, StorageError> {
        Ok(self.0)
    }
}

#[derive(Clone)]
struct ObservedFreeSpace(Arc<Mutex<Vec<u64>>>);

impl FreeSpaceProbe for ObservedFreeSpace {
    fn available_bytes(&self, root: &Path) -> Result<Option<u64>, StorageError> {
        match native::host_free_bytes(root) {
            Ok(bytes) => {
                self.0
                    .lock()
                    .map_err(|_| StorageError::Io("free-space sample lock poisoned".to_owned()))?
                    .push(bytes);
                Ok(Some(bytes))
            }
            Err(_) => Ok(None),
        }
    }
}

struct NativeScenarioSpace {
    readings: Arc<Mutex<Vec<u64>>>,
}

impl NativeScenarioSpace {
    fn new() -> Self {
        Self {
            readings: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn probe(&self) -> Arc<dyn FreeSpaceProbe> {
        Arc::new(ObservedFreeSpace(Arc::clone(&self.readings)))
    }

    fn sample(&self, root: &Path) -> u64 {
        let bytes = native::host_free_bytes(root).expect("host free-space reading available");
        assert!(
            bytes >= native::REQUIRED_FREE_BYTES,
            "native scenario refused below the 20 GiB host reserve: {} bytes",
            bytes
        );
        self.readings.lock().expect("sample list lock").push(bytes);
        bytes
    }

    fn save(&self, root: &FixtureRoot) -> PathBuf {
        let samples = self.readings.lock().expect("sample list lock");
        let mut content = String::from("oxide-host-free-space-v1\n");
        for (index, bytes) in samples.iter().enumerate() {
            content.push_str(&format!("sample_{}={}\n", index + 1, bytes));
        }
        let path = root.path.join("host-free-space.txt");
        fs::write(&path, content).expect("retain native scenario free-space readings");
        path
    }
}

struct RunningDaemon {
    client: Client,
    shutdown: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<Result<(), StorageError>>>,
}

impl RunningDaemon {
    fn start(root: &Path) -> Self {
        Self::start_with_free(root, Some(1_000_000))
    }

    fn start_with_free(root: &Path, free_bytes: Option<u64>) -> Self {
        Self::start_with_probe(root, Arc::new(FixedFreeSpace(free_bytes)))
    }

    fn start_with_probe(root: &Path, free_space: Arc<dyn FreeSpaceProbe>) -> Self {
        let daemon = Daemon::bind(
            DaemonConfig {
                root: root.to_path_buf(),
                budget: Budget {
                    cap_bytes: 10_000,
                    free_reserve_bytes: 100,
                },
            },
            Arc::new(FixtureAuthority),
            free_space,
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
    wrong_capture_attempt: bool,
    wrong_capture_digest: bool,
    last_authorized: Option<AuthorizedAttempt>,
    last_terminal_outcome: Option<TerminalOutcome>,
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
            wrong_capture_attempt: false,
            wrong_capture_digest: false,
            last_authorized: None,
            last_terminal_outcome: None,
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
        let authorized = AuthorizedAttempt {
            envelope: RunEnvelope {
                binding,
                partition: request.partition.clone(),
                admission_reference: "fixture-admission".to_owned(),
                admission_credential: Credential::new("admission-proof"),
            },
            acquire_id: format!("acquire-{}", self.attempt_id),
        };
        self.last_authorized = Some(authorized.clone());
        Ok(Some(authorized))
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
        assert!(
            request
                .cargo_build_dir
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("slot-"))
        );
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
        let attempt_id = if self.wrong_capture_attempt {
            "another-attempt".to_owned()
        } else {
            self.attempt_id.clone()
        };
        let digest = if self.wrong_capture_digest {
            digest(b"wrong-capture")
        } else {
            native::capture_receipt_digest(capture_dir, &output.output_references)
                .map_err(|_| BoundaryFailure::Unavailable)?
        };
        Ok(CaptureAcknowledgement {
            host_run_id: host_run_id.clone(),
            receipt: CaptureReceipt {
                reference: format!("capture/{}", self.run_id),
                digest,
                host_run_id,
                run_id: self.run_id.clone(),
                attempt_id,
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
        let output = output?;
        let outcome = match output.status {
            FixtureCommandStatus::Exited(0) => TerminalOutcome::Passed,
            FixtureCommandStatus::Exited(_) => TerminalOutcome::Failed,
            FixtureCommandStatus::Cancelled => TerminalOutcome::Cancelled,
            FixtureCommandStatus::Incomplete => return None,
        };
        self.last_terminal_outcome = Some(outcome);
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
    issue_test_certificate: bool,
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
            issue_test_certificate: true,
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
        fs::write(
            plan.command.capture_dir.join("stderr.log"),
            b"fixture stderr",
        )
        .map_err(|_| BoundaryFailure::Unavailable)?;
        let mut output = FixtureCommandOutput::new(
            self.status,
            vec!["stdout.log".to_owned(), "stderr.log".to_owned()],
            Some(7),
            Some(5),
            Some(2),
            Some(1),
            self.proof.clone(),
        );
        if self.issue_test_certificate {
            output.native_completion = Some(native::NativeCompletionCertificate::for_test(
                plan,
                self.status,
                &output.output_references,
            ));
        }
        Ok(output)
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
    fixture_invocation_named(root, "primary")
}

fn fixture_invocation_named(root: &FixtureRoot, name: &str) -> ProjectInvocation {
    let workspace = root.dir(&format!("workspace-{}", name));
    let target = root.dir(&format!("target-{}", name));
    let capture = root.dir(&format!("capture-{}", name));
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

fn another_attempt(
    base: &ProjectInvocation,
    root: &FixtureRoot,
    name: &str,
    attempt: &str,
    run_id: &str,
    host_run_id: &str,
) -> (ProjectInvocation, FixtureConsumer) {
    let mut invocation = base.clone();
    invocation.request_id = format!("request-{}", name);
    invocation.task_id = format!("task-{}", name);
    invocation.workspace_id = format!("workspace-{}", name);
    invocation.target_dir = root.dir(&format!("target-{}", name));
    invocation.capture_dir = root.dir(&format!("capture-{}", name));
    let mut consumer = FixtureConsumer::new();
    consumer.attempt_id = attempt.to_owned();
    consumer.run_id = run_id.to_owned();
    consumer.host_run_id = host_run_id.to_owned();
    (invocation, consumer)
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

fn assert_native_fixture_bounds(root: &FixtureRoot) {
    fn visit(
        path: &Path,
        files: &mut usize,
        bytes: &mut u64,
        largest: &mut u64,
        children: &mut usize,
    ) {
        for entry in fs::read_dir(path).expect("read task-owned fixture root") {
            let entry = entry.expect("read fixture entry");
            let metadata = fs::symlink_metadata(entry.path()).expect("read fixture metadata");
            assert!(
                !metadata.file_type().is_symlink(),
                "fixture contains no symlinks"
            );
            if metadata.is_dir() {
                visit(&entry.path(), files, bytes, largest, children);
            } else if metadata.is_file() {
                *files += 1;
                *bytes = bytes.saturating_add(metadata.len());
                *largest = (*largest).max(metadata.len());
                if entry.file_name().to_string_lossy() == "started" {
                    *children += 1;
                }
            }
        }
    }

    let mut files = 0;
    let mut bytes = 0;
    let mut largest = 0;
    let mut children = 0;
    visit(
        &root.path,
        &mut files,
        &mut bytes,
        &mut largest,
        &mut children,
    );
    assert!(
        files <= native::MAX_FIXTURE_FILES,
        "fixture file count: {}",
        files
    );
    assert!(
        largest <= native::MAX_FIXTURE_FILE_BYTES,
        "fixture max file bytes: {}",
        largest
    );
    assert!(
        children <= native::MAX_FIXTURE_CHILDREN,
        "fixture child count: {}",
        children
    );
    assert!(
        bytes <= native::MAX_DISPOSABLE_BYTES,
        "fixture bytes: {}",
        bytes
    );

    let task_root = task_scratch_root();
    let mut all_files = 0;
    let mut all_bytes = 0;
    let mut all_largest = 0;
    let mut all_children = 0;
    visit(
        &task_root,
        &mut all_files,
        &mut all_bytes,
        &mut all_largest,
        &mut all_children,
    );
    assert!(
        all_bytes <= native::MAX_DISPOSABLE_BYTES,
        "task scratch bytes: {}",
        all_bytes
    );
    assert!(
        all_largest <= native::MAX_FIXTURE_FILE_BYTES,
        "task scratch max file bytes: {}",
        all_largest
    );
    assert!(
        all_children <= native::MAX_FIXTURE_CHILDREN * 6,
        "task scratch child count: {}",
        all_children
    );
    let temporary = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .expect("forced TMPDIR");
    assert!(temporary.starts_with(&task_root));
    assert_eq!(
        fs::canonicalize(&temporary).expect("canonical TMPDIR"),
        temporary
    );
    for name in ["TMPDIR", "TMP", "TEMP"] {
        assert_eq!(
            std::env::var_os(name).map(PathBuf::from).as_ref(),
            Some(&temporary)
        );
    }
}

fn write_free_space_record(space: &NativeScenarioSpace, root: &FixtureRoot) {
    space.sample(&root.path);
    let record = space.save(root);
    assert!(record.starts_with(&root.path));
    assert_native_fixture_bounds(root);
}

#[test]
fn native_fixture_child_completion_releases_and_reuses_the_same_slot() {
    let root = FixtureRoot::scenario("01-healthy-reuse");
    let space = NativeScenarioSpace::new();
    space.sample(&root.path);
    let daemon = RunningDaemon::start_with_probe(&root.dir("storage"), space.probe());
    let invocation = fixture_invocation(&root);
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut consumer = FixtureConsumer::new();
    let mut runner = native::NativeFixtureCommandRunner::healthy().expect("bounded runner");
    let first = adapter.execute(
        &invocation,
        &mut consumer,
        &mut NoWait,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(first.disposition, ExecutionDisposition::Finished);
    assert_eq!(first.finalization.as_ref().unwrap().state, SlotState::Idle);
    assert_eq!(
        consumer.last_terminal_outcome,
        Some(TerminalOutcome::Passed)
    );
    assert_eq!(runner.launched_children(), 1);
    let first_capture = invocation.capture_dir.join("native-evidence.txt");
    let manifest = fs::read_to_string(&first_capture).expect("retained native completion record");
    let head = native::repository_head().expect("repository head");
    assert!(manifest.contains(&format!("head={}\n", head)));
    assert!(manifest.contains(&format!("run={}\n", consumer.run_id)));
    assert!(manifest.contains(&format!("attempt={}\n", consumer.attempt_id)));
    assert!(manifest.contains("process_pid="));
    assert!(manifest.contains("process_start_identity=sha256:"));
    assert!(manifest.contains("free_space_bytes="));
    let expected_prior = native::expected_slot_digest(&consumer.run_id, &consumer.attempt_id);
    let slot = daemon
        .client
        .inventory("healthy-idle")
        .expect("idle slot inventory")
        .slots
        .pop()
        .expect("one completed slot");
    assert_eq!(slot.state, SlotState::Idle);
    let mut second_invocation = invocation.clone();
    second_invocation.request_id = "request-second".to_owned();
    second_invocation.task_id = "task-second".to_owned();
    second_invocation.workspace_id = "workspace-second".to_owned();
    second_invocation.target_dir = root.dir("target-second");
    second_invocation.capture_dir = root.dir("capture-second");
    let mut second_consumer = FixtureConsumer::new();
    second_consumer.attempt_id = "attempt-second".to_owned();
    second_consumer.run_id = "run-second".to_owned();
    second_consumer.host_run_id = "host-run-second".to_owned();
    let mut second_runner = native::NativeFixtureCommandRunner::healthy().expect("bounded runner");
    second_runner.expect_prior_slot_digest(expected_prior);
    let second = adapter.execute(
        &second_invocation,
        &mut second_consumer,
        &mut NoWait,
        &mut second_runner,
        &NeverCancelled,
    );
    assert_eq!(second.disposition, ExecutionDisposition::Finished);
    assert_eq!(second.finalization.as_ref().unwrap().state, SlotState::Idle);
    assert_eq!(
        second_consumer.last_terminal_outcome,
        Some(TerminalOutcome::Passed)
    );
    assert!(
        adapter.observations()[1]
            .events
            .iter()
            .any(|event| matches!(event.kind, ObservationKind::Acquired { reused: true, .. }))
    );
    assert!(second_invocation.capture_dir.join("stdout.log").is_file());
    assert_eq!(second_runner.launched_children(), 1);
    write_free_space_record(&space, &root);
}

#[test]
fn native_active_owner_refuses_competitors_then_allows_one_racing_winner() {
    let root = FixtureRoot::scenario("02-exclusive-race");
    let space = NativeScenarioSpace::new();
    space.sample(&root.path);
    let daemon = RunningDaemon::start_with_probe(&root.dir("storage"), space.probe());
    let invocation = fixture_invocation(&root);
    let first_client = daemon.client.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let first_invocation = invocation.clone();
    let first = thread::spawn(move || {
        let mut adapter = ProjectAdapter::new(first_client);
        let mut consumer = FixtureConsumer::new();
        let mut runner = native::NativeFixtureCommandRunner::gated(started_tx, release_rx)
            .expect("bounded gated runner");
        let result = adapter.execute(
            &first_invocation,
            &mut consumer,
            &mut NoWait,
            &mut runner,
            &NeverCancelled,
        );
        (
            result,
            runner.launched_children(),
            consumer.last_terminal_outcome,
        )
    });
    let active = started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("known writer handle reached the held state");
    assert!(active.pid > 0 && active.start_identity.starts_with("sha256:"));
    let (busy_invocation, mut busy_consumer) = another_attempt(
        &invocation,
        &root,
        "active-contender",
        "attempt-busy",
        "run-busy",
        "host-run-busy",
    );
    let mut busy_adapter = ProjectAdapter::new(daemon.client.clone());
    let mut busy_runner = native::NativeFixtureCommandRunner::healthy().expect("bounded runner");
    let busy = busy_adapter.execute(
        &busy_invocation,
        &mut busy_consumer,
        &mut NoWait,
        &mut busy_runner,
        &NeverCancelled,
    );
    assert_eq!(
        busy.disposition,
        ExecutionDisposition::Refused(AdapterRefusal::Busy)
    );
    assert_eq!(busy_runner.launched_children(), 0);
    release_tx
        .send(())
        .expect("release only the owned fixture child");
    let (first_result, first_children, first_outcome) =
        first.join().expect("first adapter finishes");
    assert_eq!(first_result.disposition, ExecutionDisposition::Finished);
    assert_eq!(first_children, 1);
    assert_eq!(first_outcome, Some(TerminalOutcome::Passed));

    let (left_invocation, left_consumer) = another_attempt(
        &invocation,
        &root,
        "race-left",
        "attempt-left",
        "run-left",
        "host-run-left",
    );
    let (right_invocation, right_consumer) = another_attempt(
        &invocation,
        &root,
        "race-right",
        "attempt-right",
        "run-right",
        "host-run-right",
    );
    let gate = Arc::new(std::sync::Barrier::new(3));
    let left_client = daemon.client.clone();
    let left_gate = Arc::clone(&gate);
    let left = thread::spawn(move || {
        let mut adapter = ProjectAdapter::new(left_client);
        let mut consumer = left_consumer;
        let mut runner = native::NativeFixtureCommandRunner::healthy().expect("bounded runner");
        left_gate.wait();
        let result = adapter.execute(
            &left_invocation,
            &mut consumer,
            &mut NoWait,
            &mut runner,
            &NeverCancelled,
        );
        (
            result.disposition,
            runner.launched_children(),
            consumer.last_terminal_outcome,
        )
    });
    let right_client = daemon.client.clone();
    let right_gate = Arc::clone(&gate);
    let right = thread::spawn(move || {
        let mut adapter = ProjectAdapter::new(right_client);
        let mut consumer = right_consumer;
        let mut runner = native::NativeFixtureCommandRunner::healthy().expect("bounded runner");
        right_gate.wait();
        let result = adapter.execute(
            &right_invocation,
            &mut consumer,
            &mut NoWait,
            &mut runner,
            &NeverCancelled,
        );
        (
            result.disposition,
            runner.launched_children(),
            consumer.last_terminal_outcome,
        )
    });
    gate.wait();
    let left_result = left.join().expect("left racer completes");
    let right_result = right.join().expect("right racer completes");
    let results = [left_result, right_result];
    assert_eq!(
        results
            .iter()
            .filter(|result| result.0 == ExecutionDisposition::Finished)
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| result.0 == ExecutionDisposition::Refused(AdapterRefusal::Busy))
            .count(),
        1
    );
    assert_eq!(results.iter().map(|result| result.1).sum::<usize>(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| result.2 == Some(TerminalOutcome::Passed))
            .count(),
        1
    );
    write_free_space_record(&space, &root);
}

struct AtomicCancellation(Arc<AtomicBool>);

impl CancellationProbe for AtomicCancellation {
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[test]
fn native_cancellation_and_daemon_restart_keep_attempt_uncertain() {
    let root = FixtureRoot::scenario("03-cancellation-restart");
    let space = NativeScenarioSpace::new();
    space.sample(&root.path);
    let storage_root = root.dir("storage");
    let mut daemon = RunningDaemon::start_with_probe(&storage_root, space.probe());
    let invocation = fixture_invocation(&root);
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut consumer = FixtureConsumer::new();
    let (started_tx, started_rx) = mpsc::channel();
    let (_release_tx, release_rx) = mpsc::channel();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let cancel_signal = Arc::clone(&cancel_flag);
    let cancellation = thread::spawn(move || {
        let started = started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("native child started before cancellation");
        cancel_signal.store(true, Ordering::SeqCst);
        started
    });
    let mut runner = native::NativeFixtureCommandRunner::gated(started_tx, release_rx)
        .expect("bounded gated runner");
    let result = adapter.execute(
        &invocation,
        &mut consumer,
        &mut NoWait,
        &mut runner,
        &AtomicCancellation(cancel_flag),
    );
    let started = cancellation.join().expect("cancellation controller joins");
    assert!(started.pid > 0);
    assert_eq!(result.disposition, ExecutionDisposition::Uncertain);
    assert_eq!(consumer.capture_calls, 0);
    assert_eq!(
        consumer.last_terminal_outcome,
        Some(TerminalOutcome::Cancelled)
    );
    let uncertain_lease_id = result
        .finalization
        .as_ref()
        .expect("durable uncertain finalization")
        .lease_id
        .clone();
    let uncertain_slot_id = daemon.client.inventory("cancel-slot").unwrap().slots[0]
        .slot_id
        .clone();
    assert_eq!(
        daemon.client.inventory("cancel-state").unwrap().slots[0].state,
        SlotState::Uncertain
    );
    let attempt = consumer
        .last_authorized
        .clone()
        .expect("attempt identity retained");
    assert_eq!(attempt.envelope.binding.run_id, consumer.run_id);
    assert_eq!(attempt.envelope.binding.attempt_id, consumer.attempt_id);
    assert!(invocation.capture_dir.join("stdout.log").is_file());
    space.sample(&root.path);
    daemon.stop();
    daemon = RunningDaemon::start_with_probe(&storage_root, space.probe());
    let replay = daemon
        .client
        .acquire(AcquireRequest {
            envelope: attempt.envelope.clone(),
            acquire_id: attempt.acquire_id.clone(),
            required_bytes: 64,
        })
        .expect("replay the same acquire identity after restart");
    assert_eq!(replay.state, SlotState::Uncertain);
    assert_eq!(replay.lease_id, uncertain_lease_id);
    assert_eq!(replay.slot_id, uncertain_slot_id);
    assert!(replay.slot_path.is_none());
    assert!(replay.token.is_none());
    assert_eq!(
        daemon.client.inventory("after-replay").unwrap().slots[0]
            .owner
            .as_ref()
            .map(|owner| owner.attempt_id.as_str()),
        Some(attempt.envelope.binding.attempt_id.as_str())
    );
    write_free_space_record(&space, &root);
}

#[test]
fn native_retained_writer_blocks_release_and_remains_reserved_after_reap() {
    let root = FixtureRoot::scenario("04-retained-writer");
    let space = NativeScenarioSpace::new();
    space.sample(&root.path);
    let storage_root = root.dir("storage");
    let daemon = RunningDaemon::start_with_probe(&storage_root, space.probe());
    let invocation = fixture_invocation(&root);
    let (started_tx, started_rx) = mpsc::channel();
    let mut runner = native::NativeFixtureCommandRunner::retain_writer(started_tx)
        .expect("bounded retained-writer runner");
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut consumer = FixtureConsumer::new();
    let result = adapter.execute(
        &invocation,
        &mut consumer,
        &mut NoWait,
        &mut runner,
        &NeverCancelled,
    );
    let started = started_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("known fixture writer remains active");
    assert!(started.pid > 0 && runner.has_retained_writer());
    assert_eq!(result.disposition, ExecutionDisposition::Uncertain);
    assert_eq!(consumer.last_terminal_outcome, None);
    assert_eq!(consumer.capture_calls, 0);
    assert!(invocation.capture_dir.join("writer-progress.txt").is_file());
    let slot = daemon
        .client
        .inventory("retained-writer")
        .unwrap()
        .slots
        .pop()
        .expect("uncertain slot is retained");
    assert_eq!(slot.state, SlotState::Uncertain);
    let (next_invocation, mut next_consumer) = another_attempt(
        &invocation,
        &root,
        "after-retained",
        "attempt-after-retained",
        "run-after-retained",
        "host-after-retained",
    );
    let mut next_runner = native::NativeFixtureCommandRunner::healthy().expect("bounded runner");
    let refused = adapter.execute(
        &next_invocation,
        &mut next_consumer,
        &mut NoWait,
        &mut next_runner,
        &NeverCancelled,
    );
    assert_eq!(
        refused.disposition,
        ExecutionDisposition::Refused(AdapterRefusal::Busy)
    );
    assert_eq!(next_runner.launched_children(), 0);
    runner
        .release_and_reap_retained()
        .expect("reap only the positively identified fixture child");
    let after_reap = daemon.client.inventory("after-reap").unwrap();
    let slot = after_reap
        .slots
        .first()
        .expect("uncertain slot remains listed");
    assert_eq!(slot.state, SlotState::Uncertain);
    let slot_path = storage_root.join("slots").join(&slot.slot_id);
    let contents = fs::read(slot_path.join("fixture-output.bin")).unwrap();
    assert!(contents.ends_with(b"awaited-child-finished\n"));
    assert_eq!(daemon.client.evict("no-uncertain-eviction", 1).unwrap(), 0);
    write_free_space_record(&space, &root);
}

#[test]
fn native_capture_binding_and_injected_completion_claims_fail_closed() {
    let root = FixtureRoot::scenario("05-capture-binding");
    let space = NativeScenarioSpace::new();
    space.sample(&root.path);
    let daemon = RunningDaemon::start_with_probe(&root.dir("storage"), space.probe());
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let injected_invocation = fixture_invocation_named(&root, "injected");
    let mut injected_consumer = FixtureConsumer::new();
    let mut injected_runner = FixtureRunner::complete();
    injected_runner.issue_test_certificate = false;
    let injected = adapter.execute(
        &injected_invocation,
        &mut injected_consumer,
        &mut NoWait,
        &mut injected_runner,
        &NeverCancelled,
    );
    assert_eq!(injected.disposition, ExecutionDisposition::Uncertain);
    assert_eq!(injected_consumer.capture_calls, 0);
    assert_eq!(injected_runner.calls, 1);
    assert_eq!(
        daemon.client.inventory("forged-proof").unwrap().slots[0].state,
        SlotState::Uncertain
    );

    let native_invocation = fixture_invocation_named(&root, "bound-capture");
    let mut consumer = FixtureConsumer::new();
    consumer.attempt_id = "attempt-bound".to_owned();
    consumer.run_id = "run-bound".to_owned();
    consumer.host_run_id = "host-run-bound".to_owned();
    consumer.wrong_capture_attempt = true;
    let mut runner = native::NativeFixtureCommandRunner::healthy().expect("bounded runner");
    let result = adapter.execute(
        &native_invocation,
        &mut consumer,
        &mut NoWait,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(result.disposition, ExecutionDisposition::Uncertain);
    assert_eq!(consumer.capture_calls, 1);
    assert_eq!(
        result.finalization.as_ref().unwrap().state,
        SlotState::Uncertain
    );
    let manifest = fs::read_to_string(native_invocation.capture_dir.join("native-evidence.txt"))
        .expect("per-run native evidence retained independently");
    let stdout = fs::read(native_invocation.capture_dir.join("stdout.log")).unwrap();
    let stderr = fs::read(native_invocation.capture_dir.join("stderr.log")).unwrap();
    assert!(manifest.contains(&format!("stdout={}\n", digest(&stdout))));
    assert!(manifest.contains(&format!("stderr={}\n", digest(&stderr))));
    assert!(manifest.contains("attempt=attempt-bound\n"));
    assert!(stdout.len() <= native::MAX_CAPTURE_STREAM_BYTES);
    assert!(stderr.len() <= native::MAX_CAPTURE_STREAM_BYTES);
    write_free_space_record(&space, &root);
}

#[test]
fn native_unknown_inputs_space_and_bounds_refuse_without_launch() {
    let root = FixtureRoot::scenario("06-preflight-refusal");
    let space = NativeScenarioSpace::new();
    space.sample(&root.path);
    let daemon = RunningDaemon::start_with_free(&root.dir("storage"), None);
    let mut invocation = fixture_invocation(&root);
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut consumer = FixtureConsumer::new();
    let mut runner = native::NativeFixtureCommandRunner::healthy().expect("bounded runner");
    let unknown_space = adapter.execute(
        &invocation,
        &mut consumer,
        &mut NoWait,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(
        unknown_space.disposition,
        ExecutionDisposition::Refused(AdapterRefusal::Full)
    );
    assert_eq!(runner.executions(), 0);
    assert_eq!(runner.launched_children(), 0);

    invocation.request_id = "request-unknown-input".to_owned();
    invocation.inputs.environment = EffectiveInputSet::Unknown;
    let unknown_input = adapter.execute(
        &invocation,
        &mut consumer,
        &mut NoWait,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(
        unknown_input.disposition,
        ExecutionDisposition::Refused(AdapterRefusal::InputUnknown)
    );
    assert_eq!(runner.executions(), 0);
    let invalid_bounds = native::NativeFixtureLimits {
        files: native::MAX_FIXTURE_FILES + 1,
        ..native::NativeFixtureLimits::default()
    };
    assert_eq!(
        native::NativeFixtureCommandRunner::with_limits(invalid_bounds).err(),
        Some(BoundaryFailure::Rejected)
    );
    assert_eq!(runner.launched_children(), 0);
    write_free_space_record(&space, &root);
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
    assert!(
        observations[0]
            .events
            .iter()
            .any(|event| matches!(&event.kind, ObservationKind::CaptureCompleted { .. }))
    );
    assert!(
        format!("{:?}", observations)
            .find("admission-proof")
            .is_none()
    );
    assert!(
        format!("{:?}", observations)
            .find("fixture-private-value")
            .is_none()
    );
    assert!(observations[0].events.len() <= MAX_EVENTS_PER_RUN);
    assert!(
        observations[0]
            .events
            .iter()
            .all(|event| format!("{:?}", event).len() <= MAX_EVENT_BYTES)
    );
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
    assert!(
        adapter.observations()[0]
            .events
            .iter()
            .any(|event| matches!(&event.kind, ObservationKind::CaptureUnavailable))
    );
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
        issue_test_certificate: false,
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
    let missing_socket = root.path.join("missing-daemon.sock");
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
    assert!(
        daemon
            .client
            .inventory("no-old-cargo-slot")
            .unwrap()
            .slots
            .is_empty()
    );
}
