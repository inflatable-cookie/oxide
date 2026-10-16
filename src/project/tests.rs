use super::*;
use crate::storage::{
    Budget, Daemon, DaemonConfig, EvidenceAuthority, FreeSpaceProbe, RunEnvelope, TerminalOutcome,
};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

mod cargo_pilot;
#[cfg(feature = "private-qualification")]
mod private_ordinary_cargo_process;

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(1);

struct FixtureRoot {
    path: PathBuf,
    preserve: bool,
}

impl FixtureRoot {
    fn new() -> Self {
        Self::create("p", false)
    }

    fn scenario(name: &str) -> Self {
        let scenario = name.split_once('-').map_or(name, |(number, _)| number);
        assert_eq!(scenario.len(), 2, "scenario has a two-digit fixture ID");
        assert!(scenario.bytes().all(|byte| byte.is_ascii_digit()));
        let mut root = Self::create(&format!("s/{}", scenario), true);
        root.preserve = true;
        root
    }

    fn create(name: &str, preserve: bool) -> Self {
        let parent = task_scratch_root().join(name);
        fs::create_dir_all(&parent).expect("create under task-owned scratch root");
        loop {
            let id = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("f{:x}-{:x}", std::process::id(), id));
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
        let name = match name {
            "storage" => "s",
            _ => name,
        };
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
                backing: None,
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
                path_identity: PathIdentity::Checkout,
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
            effective_rustflags: Some(Vec::new()),
            full_debug: true,
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
        required: bool,
        reject_symlinks: bool,
        files: &mut usize,
        bytes: &mut u64,
        largest: &mut u64,
        children: &mut usize,
    ) {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if !required && error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => panic!("read task-owned fixture root: {}", error),
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => panic!("read fixture entry: {}", error),
            };
            let metadata = match fs::symlink_metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => panic!("read fixture metadata: {}", error),
            };
            if metadata.file_type().is_symlink() {
                assert!(!reject_symlinks, "fixture contains no symlinks");
                *files += 1;
                *bytes = bytes.saturating_add(metadata.len());
                *largest = (*largest).max(metadata.len());
            } else if metadata.is_dir() {
                visit(
                    &entry.path(),
                    false,
                    reject_symlinks,
                    files,
                    bytes,
                    largest,
                    children,
                );
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
        true,
        true,
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
        true,
        false,
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

fn expect_held(
    readiness: native::NativeWriterReadiness,
    context: &str,
) -> native::NativeChildStarted {
    match readiness {
        native::NativeWriterReadiness::Held(started) => started,
        native::NativeWriterReadiness::Refused(reason) => panic!(
            "{context}: fixture refused before a held writer ({})",
            reason.label()
        ),
    }
}

/// A controlled sibling entry under the shared task scratch root. Another
/// isolated fixture test (the declared-workflow FIFO reader case) can create a
/// non-regular entry there while a native runner is active; this helper proves
/// the runner accounts for it instead of mistaking it for its own fixture.
struct SiblingSpecialEntry {
    parent: PathBuf,
    path: PathBuf,
}

impl SiblingSpecialEntry {
    fn fifo() -> Self {
        let parent = task_scratch_root().join("sibling").join(format!(
            "f{:x}-{:x}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&parent).expect("create sibling fixture directory");
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))
            .expect("restrict sibling fixture directory");
        let path = parent.join("in-use-fifo");
        let c_path =
            std::ffi::CString::new(path.as_os_str().as_bytes()).expect("nul-free fifo path");
        // SAFETY: `c_path` is a NUL-terminated path inside the task scratch tree.
        let status = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(status, 0, "create sibling fifo fixture");
        Self { parent, path }
    }
}

impl Drop for SiblingSpecialEntry {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(&self.parent);
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
    assert!(adapter.observations()[1]
        .events
        .iter()
        .any(|event| matches!(event.kind, ObservationKind::Acquired { reused: true, .. })));
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
    let active = expect_held(
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("known writer handle reached the held state"),
        "exclusive race",
    );
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
        let readiness = started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("native child started before cancellation");
        cancel_signal.store(true, Ordering::SeqCst);
        readiness
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
    let started = expect_held(
        cancellation.join().expect("cancellation controller joins"),
        "cancellation",
    );
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
    let started = expect_held(
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("known fixture writer remains active"),
        "retained writer",
    );
    assert!(started.pid > 0);
    assert!(runner.has_retained_writer() && runner.retained_writer_is_active());
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
fn native_retained_writer_readiness_accounts_for_a_sibling_special_entry() {
    // Regression proof for the shared task-root accounting walk: a concurrently
    // running isolated fixture (the declared-workflow FIFO reader) creates a
    // non-regular sibling entry. That entry is accounted, not treated as this
    // scenario's ownership failure, and the owned writer still reaches its
    // held state and stays reserved.
    let root = FixtureRoot::scenario("07-sibling-special");
    let space = NativeScenarioSpace::new();
    space.sample(&root.path);
    let sibling = SiblingSpecialEntry::fifo();
    let storage_root = root.dir("storage");
    let daemon = RunningDaemon::start_with_probe(&storage_root, space.probe());
    let invocation = fixture_invocation_named(&root, "sibling-special");
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
    let started = expect_held(
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("retained writer reaches readiness despite the sibling fifo"),
        "sibling special entry",
    );
    assert!(started.pid > 0);
    assert!(runner.has_retained_writer() && runner.retained_writer_is_active());
    assert_eq!(result.disposition, ExecutionDisposition::Uncertain);
    assert_eq!(consumer.last_terminal_outcome, None);
    assert_eq!(consumer.capture_calls, 0);
    assert!(invocation.capture_dir.join("writer-progress.txt").is_file());
    let slot = daemon
        .client
        .inventory("sibling-special-retained")
        .unwrap()
        .slots
        .pop()
        .expect("uncertain slot remains reserved");
    assert_eq!(slot.state, SlotState::Uncertain);
    runner
        .release_and_reap_retained()
        .expect("reap only the identified fixture writer");
    drop(sibling);
    write_free_space_record(&space, &root);
}

#[test]
fn native_retained_writer_readiness_refuses_a_foreign_start_marker() {
    // Deterministic pre-readiness error path: the child writes a start marker
    // that is not its own spawn identity and exits. Readiness must report a
    // bounded refusal, and the runner must neither retain nor claim a writer.
    let root = FixtureRoot::scenario("08-foreign-marker");
    let space = NativeScenarioSpace::new();
    space.sample(&root.path);
    let storage_root = root.dir("storage");
    let daemon = RunningDaemon::start_with_probe(&storage_root, space.probe());
    let invocation = fixture_invocation_named(&root, "foreign-marker");
    let (started_tx, started_rx) = mpsc::channel();
    let mut runner = native::NativeFixtureCommandRunner::bad_start_marker(started_tx)
        .expect("bounded refusal runner");
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut consumer = FixtureConsumer::new();
    let result = adapter.execute(
        &invocation,
        &mut consumer,
        &mut NoWait,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a pre-readiness refusal is always reported"),
        native::NativeWriterReadiness::Refused(native::NativeReadinessRefusal::StartMarker)
    );
    assert_eq!(runner.launched_children(), 1);
    assert!(!runner.has_retained_writer());
    assert!(!runner.retained_writer_is_active());
    assert_eq!(result.disposition, ExecutionDisposition::Uncertain);
    assert_eq!(consumer.last_terminal_outcome, None);
    assert_eq!(consumer.capture_calls, 0);
    let slot = daemon
        .client
        .inventory("foreign-marker")
        .unwrap()
        .slots
        .pop()
        .expect("refused attempt keeps its slot uncertain");
    assert_eq!(slot.state, SlotState::Uncertain);
    assert_eq!(daemon.client.evict("no-uncertain-eviction", 1).unwrap(), 0);
    write_free_space_record(&space, &root);
}

#[test]
fn native_capture_binding_and_injected_completion_claims_fail_closed() {
    let root = FixtureRoot::scenario("05-capture-binding");
    let space = NativeScenarioSpace::new();
    space.sample(&root.path);
    let injected_daemon = RunningDaemon::start_with_probe(&root.dir("i"), space.probe());
    let mut injected_adapter = ProjectAdapter::new(injected_daemon.client.clone());
    let injected_invocation = fixture_invocation_named(&root, "injected");
    let mut injected_consumer = FixtureConsumer::new();
    let mut injected_runner = FixtureRunner::complete();
    injected_runner.issue_test_certificate = false;
    let injected = injected_adapter.execute(
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
        injected_daemon
            .client
            .inventory("forged-proof")
            .unwrap()
            .slots[0]
            .state,
        SlotState::Uncertain
    );

    let native_daemon = RunningDaemon::start_with_probe(&root.dir("n"), space.probe());
    let mut native_adapter = ProjectAdapter::new(native_daemon.client.clone());
    let native_invocation = fixture_invocation_named(&root, "bound-capture");
    let mut consumer = FixtureConsumer::new();
    consumer.attempt_id = "attempt-bound".to_owned();
    consumer.run_id = "run-bound".to_owned();
    consumer.host_run_id = "host-run-bound".to_owned();
    consumer.wrong_capture_attempt = true;
    let mut runner = native::NativeFixtureCommandRunner::healthy().expect("bounded runner");
    let result = native_adapter.execute(
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
    assert!(daemon
        .client
        .inventory("no-old-cargo-slot")
        .unwrap()
        .slots
        .is_empty());
}

struct SourceMapFixture {
    root: FixtureRoot,
    disposable: PathBuf,
    checkout_a: PathBuf,
    checkout_b: PathBuf,
    checkout_c: PathBuf,
    slot: PathBuf,
    capture: PathBuf,
}

impl SourceMapFixture {
    fn new(scenario: &str) -> Self {
        let root = FixtureRoot::scenario(scenario);
        let disposable = root.dir("source-map-volume");
        let checkout_a = disposable.join("checkout-a");
        let checkout_b = disposable.join("checkout-b");
        let checkout_c = disposable.join("checkout-c");
        let slot = disposable.join("slot");
        let capture = root.dir("source-map-capture");
        for checkout in [&checkout_a, &checkout_b, &checkout_c] {
            fs::create_dir_all(checkout.join("src")).unwrap();
            fs::create_dir_all(checkout.join("assets")).unwrap();
            fs::write(checkout.join("src/lib.rs"), b"pub fn fixture() {}\n").unwrap();
            fs::write(checkout.join("build.rs"), b"fn main() {}\n").unwrap();
            fs::write(checkout.join("assets/data.txt"), b"fixture resource\n").unwrap();
        }
        fs::create_dir_all(&slot).unwrap();
        Self {
            root,
            disposable,
            checkout_a,
            checkout_b,
            checkout_c,
            slot,
            capture,
        }
    }

    fn policy(&self, roots: Vec<(&str, PathBuf)>, mode: SourcePathMode) -> SourceMapPolicy {
        SourceMapPolicy {
            version: SOURCE_MAP_VERSION.to_owned(),
            mode,
            roots: roots
                .into_iter()
                .map(|(id, path)| AuthorizedSourceRoot {
                    id: id.to_owned(),
                    path,
                })
                .collect(),
            relative_prefixes: Vec::new(),
            miss_controls: Some(MissControlEvidence {
                source: Some(SourceMissControl {
                    before_tree_digest: digest(b"tree-before"),
                    after_tree_digest: digest(b"tree-after"),
                    partition_digest: digest(b"same-partition"),
                    rebuilt_units: 1,
                }),
                configuration: Some(KeyMissControl {
                    before_input_digest: digest(b"config-before"),
                    after_input_digest: digest(b"config-after"),
                    before_partition_digest: digest(b"partition-before"),
                    after_partition_digest: digest(b"partition-after"),
                }),
                toolchain: Some(KeyMissControl {
                    before_input_digest: digest(b"toolchain-before"),
                    after_input_digest: digest(b"toolchain-after"),
                    before_partition_digest: digest(b"toolchain-partition-before"),
                    after_partition_digest: digest(b"toolchain-partition-after"),
                }),
            }),
        }
    }

    fn invocation(
        &self,
        workspace: &Path,
        tree_digest: &str,
        policy: SourceMapPolicy,
    ) -> ProjectInvocation {
        let name = format!("source-map-{}", NEXT_ROOT.fetch_add(1, Ordering::Relaxed));
        let mut invocation = fixture_invocation_named(&self.root, &name);
        invocation.descriptor.workspace_root = workspace.to_path_buf();
        invocation.source_tree_digest = tree_digest.to_owned();
        invocation.inputs.effective_rustflags = Some(vec!["--cfg=fixture_flag".to_owned()]);
        invocation.inputs.full_debug = true;
        invocation.descriptor.workflows[0].path_identity = PathIdentity::DisposableRoot {
            id: "fixture-volume".to_owned(),
            root: self.disposable.clone(),
            source_map: Some(policy),
        };
        invocation
    }

    fn resolved_map(
        &self,
        workspace: &Path,
        tree_digest: &str,
        policy: SourceMapPolicy,
    ) -> (resolve::ResolvedInvocation, ResolvedSourceMap) {
        let invocation = self.invocation(workspace, tree_digest, policy);
        let resolved = match resolve::resolve(&invocation) {
            Resolution::Ready(resolved) => resolved,
            _ => panic!("fixture source map resolves"),
        };
        let source_map = resolved.source_map.clone().expect("explicit source map");
        (resolved, source_map)
    }

    fn run_evidence(
        &self,
        map: &ResolvedSourceMap,
        compile_origin: (&str, &str),
    ) -> (SourceMapRunReceipt, PortabilityEvidence) {
        fs::create_dir_all(self.slot.join("out")).unwrap();
        fs::create_dir_all(self.slot.join("proc-macro")).unwrap();
        fs::write(self.slot.join("out/generated.rs"), b"generated\n").unwrap();
        fs::write(self.slot.join("proc-macro/input.rs"), b"macro input\n").unwrap();
        fs::write(self.capture.join("test.bin"), b"fixture binary\n").unwrap();
        fs::write(self.capture.join("stdout.log"), b"fixture run\n").unwrap();
        fs::write(self.capture.join("stderr.log"), b"fixture stderr\n").unwrap();
        let output_references = vec![
            "test.bin".to_owned(),
            "stdout.log".to_owned(),
            "stderr.log".to_owned(),
        ];
        let capture_digest = native::capture_receipt_digest(&self.capture, &output_references)
            .expect("bounded fixture capture digest");
        let binary_digest = digest(&fs::read(self.capture.join("test.bin")).unwrap());
        let receipt = SourceMapRunReceipt {
            policy_digest: map.policy_digest().to_owned(),
            map_digest: map.map_digest().to_owned(),
            build_identity_digest: map.build_identity_digest().to_owned(),
            authorized_roots: map.authorized_roots().to_vec(),
            requested_root_id: map.requested_root_id().to_owned(),
            requested_root: map.requested_root().to_path_buf(),
            logical_prefix: map.logical_prefix().map(Path::to_path_buf),
            requested_source_tree_digest: map.requested_tree_digest().to_owned(),
            compile_origins: vec![CompileOriginIdentity {
                root_id: compile_origin.0.to_owned(),
                tree_digest: compile_origin.1.to_owned(),
            }],
            slot_lease_id: "lease-fixture".to_owned(),
            run_id: "run-fixture".to_owned(),
            attempt_id: "attempt-fixture".to_owned(),
            host_run_id: "host-fixture".to_owned(),
            slot_path: self.slot.clone(),
            capture_dir: self.capture.clone(),
            output_references,
            binaries: vec![BinaryArtifactDigest {
                reference: "test.bin".to_owned(),
                digest: binary_digest,
            }],
            stdout_digest: digest(&fs::read(self.capture.join("stdout.log")).unwrap()),
            stderr_digest: digest(&fs::read(self.capture.join("stderr.log")).unwrap()),
            capture_digest,
        };
        let source_path = match map.logical_prefix() {
            Some(prefix) => prefix.join("src/lib.rs"),
            None => self.checkout_b.join("src/lib.rs"),
        };
        let source_paths = [
            SourcePathClass::CompilerDebug,
            SourcePathClass::FileMacro,
            SourcePathClass::PanicLocation,
        ]
        .into_iter()
        .map(|class| SourcePathObservation {
            class,
            observed: source_path.clone(),
            resolved: Some(self.checkout_b.join("src/lib.rs")),
        })
        .collect();
        let generation = "lease-fixture".to_owned();
        let tree = map.requested_tree_digest().to_owned();
        let behavior_paths = vec![
            BehaviorPathObservation {
                class: BehaviorPathClass::ManifestDirectory,
                resolved: Some(self.checkout_b.clone()),
                source_tree_digest: Some(tree.clone()),
                generation_lease_id: None,
            },
            BehaviorPathObservation {
                class: BehaviorPathClass::IncludeInput,
                resolved: Some(self.checkout_b.join("src/lib.rs")),
                source_tree_digest: Some(tree.clone()),
                generation_lease_id: None,
            },
            BehaviorPathObservation {
                class: BehaviorPathClass::BuildScriptInput,
                resolved: Some(self.checkout_b.join("build.rs")),
                source_tree_digest: Some(tree.clone()),
                generation_lease_id: None,
            },
            BehaviorPathObservation {
                class: BehaviorPathClass::OutDirectory,
                resolved: Some(self.slot.join("out")),
                source_tree_digest: Some(tree.clone()),
                generation_lease_id: Some(generation.clone()),
            },
            BehaviorPathObservation {
                class: BehaviorPathClass::GeneratedOutput,
                resolved: Some(self.slot.join("out/generated.rs")),
                source_tree_digest: Some(tree.clone()),
                generation_lease_id: Some(generation.clone()),
            },
            BehaviorPathObservation {
                class: BehaviorPathClass::ProcMacroInput,
                resolved: Some(self.slot.join("proc-macro/input.rs")),
                source_tree_digest: Some(tree.clone()),
                generation_lease_id: Some(generation),
            },
            BehaviorPathObservation {
                class: BehaviorPathClass::RuntimeResource,
                resolved: Some(self.checkout_b.join("assets/data.txt")),
                source_tree_digest: Some(tree),
                generation_lease_id: None,
            },
        ];
        let evidence = PortabilityEvidence {
            binary_scans: Some(vec![BinarySourceScan {
                reference: "test.bin".to_owned(),
                binary_digest: receipt.binaries[0].digest.clone(),
                scanner_version: SOURCE_SCAN_VERSION.to_owned(),
                complete: true,
                debug_sections: vec![
                    ".debug_info".to_owned(),
                    ".debug_line".to_owned(),
                    ".debug_abbrev".to_owned(),
                ],
                source_paths,
            }]),
            behavior_paths: Some(behavior_paths),
            apple_oso: Some(source_map::AppleOsoObservation::NotEmitted),
            provenance: source_map::EvidenceProvenance::TaskOwnedFixture,
            missing_classes: Vec::new(),
        };
        (receipt, evidence)
    }
}

#[derive(Clone, Copy)]
enum SourceMapFixtureTamper {
    None,
    MissingHook,
    BinaryAfterCollection,
    WrongLease,
    ChangedMap,
    PhysicalSourcePath,
    RequestedPhysicalSourcePath,
}

struct SourceMapFixtureRunner {
    tamper: SourceMapFixtureTamper,
}

impl FixtureCommandRunner for SourceMapFixtureRunner {
    fn execute(
        &mut self,
        plan: &FixtureExecutionPlan,
        _: &dyn CancellationProbe,
    ) -> Result<FixtureCommandOutput, BoundaryFailure> {
        let map = plan
            .command
            .source_map
            .as_ref()
            .ok_or(BoundaryFailure::InvalidIdentity)?;
        let capture = &plan.command.capture_dir;
        let slot = &plan.command.cargo_build_dir;
        let requested_root = map.requested_root();
        let generated = slot.join("out/generated.rs");
        let proc_macro_input = slot.join("proc-macro/input.rs");
        let target_dir = &plan.command.cargo_target_dir;
        fs::create_dir_all(generated.parent().ok_or(BoundaryFailure::InvalidIdentity)?)
            .map_err(|_| BoundaryFailure::Unavailable)?;
        fs::create_dir_all(
            proc_macro_input
                .parent()
                .ok_or(BoundaryFailure::InvalidIdentity)?,
        )
        .map_err(|_| BoundaryFailure::Unavailable)?;
        fs::create_dir_all(target_dir).map_err(|_| BoundaryFailure::Unavailable)?;
        fs::write(&generated, b"fixture generated output\n")
            .map_err(|_| BoundaryFailure::Unavailable)?;
        fs::write(&proc_macro_input, b"fixture proc-macro input\n")
            .map_err(|_| BoundaryFailure::Unavailable)?;
        fs::write(capture.join("stdout.log"), b"task-owned fixture stdout\n")
            .map_err(|_| BoundaryFailure::Unavailable)?;
        fs::write(capture.join("stderr.log"), b"task-owned fixture stderr\n")
            .map_err(|_| BoundaryFailure::Unavailable)?;
        let binary = target_dir.join("fixture.bin");
        fs::write(&binary, b"task-owned fixture binary\n")
            .map_err(|_| BoundaryFailure::Unavailable)?;

        let observed_source = match self.tamper {
            SourceMapFixtureTamper::PhysicalSourcePath => map
                .authorized_roots()
                .iter()
                .find(|root| root.id != map.requested_root_id())
                .ok_or(BoundaryFailure::InvalidIdentity)?
                .path
                .join("src/lib.rs"),
            SourceMapFixtureTamper::RequestedPhysicalSourcePath => {
                requested_root.join("src/lib.rs")
            }
            _ => map.logical_prefix().map_or_else(
                || requested_root.join("src/lib.rs"),
                |prefix| prefix.join("src/lib.rs"),
            ),
        };
        let binary_digest = digest(&fs::read(&binary).map_err(|_| BoundaryFailure::Unavailable)?);
        let origin = map
            .authorized_roots()
            .iter()
            .find(|root| root.id != map.requested_root_id())
            .ok_or(BoundaryFailure::InvalidIdentity)?;
        let generated_tree = map.requested_tree_digest();
        let behavior = [
            ("manifest-directory", requested_root.to_path_buf(), "-"),
            ("include-input", requested_root.join("src/lib.rs"), "-"),
            ("build-script-input", requested_root.join("build.rs"), "-"),
            (
                "out-directory",
                generated
                    .parent()
                    .ok_or(BoundaryFailure::InvalidIdentity)?
                    .to_path_buf(),
                plan.claim.lease_id.as_str(),
            ),
            (
                "generated-output",
                generated.clone(),
                plan.claim.lease_id.as_str(),
            ),
            (
                "proc-macro-input",
                proc_macro_input,
                plan.claim.lease_id.as_str(),
            ),
            (
                "runtime-resource",
                requested_root.join("assets/data.txt"),
                "-",
            ),
        ];
        let hook_path = target_dir.join("source-map-observations-v1.txt");
        if matches!(self.tamper, SourceMapFixtureTamper::MissingHook) {
            let _ = fs::remove_file(&hook_path);
        } else {
            let mut rows = vec![
                "oxide-cargo-source-map-observations-v1".to_owned(),
                format!(
                    "identity\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    map.policy_digest(),
                    map.map_digest(),
                    map.build_identity_digest(),
                    map.requested_tree_digest(),
                    plan.claim.lease_id,
                    plan.claim.binding.run_id,
                    plan.claim.binding.attempt_id,
                    plan.claim.host_run_id,
                ),
                format!(
                    "origin\t{}\t{}",
                    origin.id,
                    digest(b"fixture-compile-origin")
                ),
                "artifact\tfixture-binary\ttarget\tfixture.bin".to_owned(),
                format!(
                    "scan\tfixture-binary\t{}\t{}",
                    SOURCE_SCAN_VERSION, binary_digest
                ),
                "section\tfixture-binary\t.debug_info".to_owned(),
                "section\tfixture-binary\t.debug_line".to_owned(),
                "section\tfixture-binary\t.debug_abbrev".to_owned(),
            ];
            for class in ["compiler-debug", "file-macro", "panic-location"] {
                rows.push(format!(
                    "source\tfixture-binary\t{class}\t{}",
                    observed_source.display()
                ));
            }
            for (class, path, generation) in behavior {
                rows.push(format!(
                    "behavior\t{class}\t{}\t{generated_tree}\t{generation}",
                    path.display()
                ));
            }
            rows.push("apple-oso\tnot-emitted".to_owned());
            fs::write(hook_path, format!("{}\n", rows.join("\n")))
                .map_err(|_| BoundaryFailure::Unavailable)?;
        }
        let mut output_references = vec!["stdout.log".to_owned(), "stderr.log".to_owned()];
        let observations = source_map::read_registered_cargo_hook(
            source_map::EvidenceProvenance::TaskOwnedFixture,
            map,
            &plan.claim.lease_id,
            &plan.claim.binding,
            &plan.claim.host_run_id,
            slot,
            target_dir,
            capture,
            &mut output_references,
        );
        let mut collected = source_map::collect(
            map,
            &plan.claim.lease_id,
            &plan.claim.binding,
            &plan.claim.host_run_id,
            slot,
            &plan.command.cargo_target_dir,
            capture,
            &mut output_references,
            observations,
        )
        .map_err(|_| BoundaryFailure::InvalidIdentity)?;
        match self.tamper {
            SourceMapFixtureTamper::None | SourceMapFixtureTamper::MissingHook => {}
            SourceMapFixtureTamper::PhysicalSourcePath
            | SourceMapFixtureTamper::RequestedPhysicalSourcePath => {}
            SourceMapFixtureTamper::BinaryAfterCollection => {
                fs::write(capture.join("binary-0.bin"), b"changed after collection\n")
                    .map_err(|_| BoundaryFailure::Unavailable)?;
            }
            SourceMapFixtureTamper::WrongLease => {
                collected.receipt.slot_lease_id = "wrong-lease".to_owned();
            }
            SourceMapFixtureTamper::ChangedMap => {
                collected.receipt.map_digest = digest(b"changed-map");
            }
        }
        let mut output = FixtureCommandOutput::new(
            FixtureCommandStatus::Exited(0),
            output_references.clone(),
            Some(4),
            Some(3),
            Some(0),
            Some(1),
            FixtureCompletionProof::Stopped {
                reference: format!("fixture-run-{}", plan.claim.binding.run_id),
                credential: Credential::new("task-owned-fixture-runner"),
            },
        );
        output.source_map_receipt = Some(collected.receipt);
        output.portability_evidence = Some(collected.evidence);
        output.native_completion = Some(native::NativeCompletionCertificate::for_test(
            plan,
            output.status,
            &output_references,
        ));
        Ok(output)
    }
}

#[test]
fn finite_source_map_preserves_flags_and_separates_map_and_disposable_roots() {
    let fixture = SourceMapFixture::new("07-source-map-partition");
    let mode = SourcePathMode::ExperimentalLogical {
        accepted_policy: ACCEPTED_LOGICAL_POLICY.to_owned(),
        prefix: PathBuf::from("/oxide/fixture-component"),
    };
    let policy = fixture.policy(
        vec![
            ("root-a", fixture.checkout_a.clone()),
            ("root-b", fixture.checkout_b.clone()),
        ],
        mode.clone(),
    );
    let (first, first_map) =
        fixture.resolved_map(&fixture.checkout_a, &digest(b"tree-a"), policy.clone());
    let (second, second_map) =
        fixture.resolved_map(&fixture.checkout_b, &digest(b"tree-b"), policy);
    assert_eq!(
        first.partition.path_identity,
        second.partition.path_identity
    );
    assert_eq!(first_map.map_digest(), second_map.map_digest());
    let expected_flags = vec![
        "--cfg=fixture_flag".to_owned(),
        format!(
            "--remap-path-prefix={}=/oxide/fixture-component",
            fixture.checkout_a.display()
        ),
        format!(
            "--remap-path-prefix={}=/oxide/fixture-component",
            fixture.checkout_b.display()
        ),
    ];
    assert_eq!(first_map.effective_rustflags(), expected_flags.as_slice());

    let changed_policy = fixture.policy(
        vec![
            ("root-a", fixture.checkout_a.clone()),
            ("root-b", fixture.checkout_b.clone()),
            ("root-c", fixture.checkout_c.clone()),
        ],
        mode.clone(),
    );
    let (changed, _) =
        fixture.resolved_map(&fixture.checkout_a, &digest(b"tree-a"), changed_policy);
    assert_ne!(
        first.partition.path_identity,
        changed.partition.path_identity
    );

    let mut changed_flags = fixture.invocation(
        &fixture.checkout_a,
        &digest(b"tree-a"),
        fixture.policy(
            vec![
                ("root-a", fixture.checkout_a.clone()),
                ("root-b", fixture.checkout_b.clone()),
            ],
            mode.clone(),
        ),
    );
    changed_flags.inputs.effective_rustflags = Some(vec!["--cfg=another_flag".to_owned()]);
    let changed_flags = match resolve::resolve(&changed_flags) {
        Resolution::Ready(resolved) => resolved,
        _ => panic!("changed flags resolve to a separate partition"),
    };
    assert_ne!(
        first.partition.path_identity,
        changed_flags.partition.path_identity
    );

    let checkout_policy = || {
        fixture.policy(
            vec![
                ("root-a", fixture.checkout_a.clone()),
                ("root-b", fixture.checkout_b.clone()),
            ],
            SourcePathMode::StrictPhysical,
        )
    };
    let mut isolated_a =
        fixture.invocation(&fixture.checkout_a, &digest(b"tree-a"), checkout_policy());
    isolated_a.descriptor.workflows[0].path_identity = PathIdentity::DisposableRoot {
        id: "fixture-volume".to_owned(),
        root: fixture.disposable.clone(),
        source_map: None,
    };
    let mut isolated_b =
        fixture.invocation(&fixture.checkout_b, &digest(b"tree-b"), checkout_policy());
    isolated_b.descriptor.workflows[0].path_identity = PathIdentity::DisposableRoot {
        id: "fixture-volume".to_owned(),
        root: fixture.disposable.clone(),
        source_map: None,
    };
    let isolated_a = match resolve::resolve(&isolated_a) {
        Resolution::Ready(resolved) => resolved,
        _ => panic!("unmapped checkout resolves with physical identity"),
    };
    let isolated_b = match resolve::resolve(&isolated_b) {
        Resolution::Ready(resolved) => resolved,
        _ => panic!("second unmapped checkout resolves with physical identity"),
    };
    assert_ne!(
        isolated_a.partition.path_identity,
        isolated_b.partition.path_identity
    );

    let other_disposable = fixture.root.dir("second-disposable-root");
    let other_a = other_disposable.join("checkout-a");
    let other_b = other_disposable.join("checkout-b");
    fs::create_dir_all(&other_a).unwrap();
    fs::create_dir_all(&other_b).unwrap();
    let other_policy = fixture.policy(vec![("root-a", other_a.clone()), ("root-b", other_b)], mode);
    let mut other_invocation =
        fixture.invocation(&other_a, &digest(b"tree-a"), other_policy.clone());
    other_invocation.descriptor.workflows[0].path_identity = PathIdentity::DisposableRoot {
        id: "fixture-volume".to_owned(),
        root: other_disposable,
        source_map: Some(other_policy),
    };
    let other = match resolve::resolve(&other_invocation) {
        Resolution::Ready(resolved) => resolved,
        _ => panic!("second disposable root resolves"),
    };
    assert_ne!(first.partition.path_identity, other.partition.path_identity);
}

#[test]
fn incomplete_miss_gate_refuses_before_a_shared_partition_is_resolved() {
    let fixture = SourceMapFixture::new("08-source-map-miss-gate");
    let mut policy = fixture.policy(
        vec![
            ("root-a", fixture.checkout_a.clone()),
            ("root-b", fixture.checkout_b.clone()),
        ],
        SourcePathMode::StrictPhysical,
    );
    policy.miss_controls.as_mut().unwrap().toolchain = None;
    let invocation = fixture.invocation(&fixture.checkout_a, &digest(b"tree-a"), policy);
    assert!(matches!(
        resolve::resolve(&invocation),
        Resolution::Refused(AdapterRefusal::InputUnknown)
    ));

    let policy = fixture.policy(
        vec![
            ("root-a", fixture.checkout_a.clone()),
            ("root-b", fixture.checkout_b.clone()),
        ],
        SourcePathMode::ExperimentalLogical {
            accepted_policy: ACCEPTED_LOGICAL_POLICY.to_owned(),
            prefix: PathBuf::from("/oxide/fixture-component"),
        },
    );
    let mut unknown_flags = fixture.invocation(&fixture.checkout_a, &digest(b"tree-a"), policy);
    unknown_flags.inputs.effective_rustflags = None;
    assert!(matches!(
        resolve::resolve(&unknown_flags),
        Resolution::Refused(AdapterRefusal::InputUnknown)
    ));
}

#[test]
fn relative_source_prefix_is_registered_ordered_and_keyed_by_workspace_context() {
    let fixture = SourceMapFixture::new("14-relative-source-prefix");
    for root in [&fixture.checkout_a, &fixture.checkout_b] {
        fs::create_dir_all(root.join("probe/src")).unwrap();
        fs::write(root.join("probe/Cargo.toml"), b"[package]\nname='probe'\n").unwrap();
        fs::write(root.join("probe/src/lib.rs"), b"pub fn probe() {}\n").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/Cargo.toml"), b"[package]\nname='other'\n").unwrap();
    }
    let mode = SourcePathMode::ExperimentalLogical {
        accepted_policy: ACCEPTED_LOGICAL_POLICY.to_owned(),
        prefix: PathBuf::from("/oxide/fixture-component"),
    };
    let roots = vec![
        ("root-a", fixture.checkout_a.clone()),
        ("root-b", fixture.checkout_b.clone()),
    ];
    let mut policy = fixture.policy(roots.clone(), mode.clone());
    policy.relative_prefixes.push(RelativeSourcePrefix {
        root_ids: vec!["root-a".to_owned(), "root-b".to_owned()],
        compiler_working_directory: PathBuf::from("."),
        package_directory: PathBuf::from("probe"),
        prefix: PathBuf::from("probe"),
    });
    let (resolved, map) =
        fixture.resolved_map(&fixture.checkout_a, &digest(b"tree-a"), policy.clone());
    assert_eq!(
        map.effective_rustflags().last().map(String::as_str),
        Some("--remap-path-prefix=probe=/oxide/fixture-component/probe")
    );
    assert_eq!(
        map.effective_rustflags(),
        [
            "--cfg=fixture_flag".to_owned(),
            format!(
                "--remap-path-prefix={}=/oxide/fixture-component",
                fixture.checkout_a.display()
            ),
            format!(
                "--remap-path-prefix={}=/oxide/fixture-component",
                fixture.checkout_b.display()
            ),
            "--remap-path-prefix=probe=/oxide/fixture-component/probe".to_owned(),
        ]
    );
    let mut changed = policy.clone();
    changed.relative_prefixes[0].package_directory = PathBuf::from("src");
    changed.relative_prefixes[0].prefix = PathBuf::from("src");
    let (changed_resolved, changed_map) =
        fixture.resolved_map(&fixture.checkout_a, &digest(b"tree-a"), changed);
    assert_ne!(map.map_digest(), changed_map.map_digest());
    assert_ne!(map.policy_digest(), changed_map.policy_digest());
    assert_ne!(
        resolved.partition.path_identity,
        changed_resolved.partition.path_identity
    );

    let mut traversing = policy.clone();
    traversing.relative_prefixes[0].prefix = PathBuf::from("../probe");
    assert!(matches!(
        resolve::resolve(&fixture.invocation(&fixture.checkout_a, &digest(b"tree-a"), traversing)),
        Resolution::Refused(AdapterRefusal::AmbiguousInput)
            | Resolution::Refused(AdapterRefusal::UnsupportedInput)
    ));
    let mut unregistered = policy.clone();
    unregistered.relative_prefixes[0].root_ids[0] = "unregistered".to_owned();
    assert!(matches!(
        resolve::resolve(&fixture.invocation(
            &fixture.checkout_a,
            &digest(b"tree-a"),
            unregistered
        )),
        Resolution::Refused(AdapterRefusal::AmbiguousInput)
    ));
    let mut ambiguous = policy.clone();
    ambiguous
        .relative_prefixes
        .push(ambiguous.relative_prefixes[0].clone());
    assert!(matches!(
        resolve::resolve(&fixture.invocation(&fixture.checkout_a, &digest(b"tree-a"), ambiguous)),
        Resolution::Refused(AdapterRefusal::AmbiguousInput)
    ));
    let mut wrong_context = policy;
    wrong_context.relative_prefixes[0].compiler_working_directory = PathBuf::from("probe");
    assert!(matches!(
        resolve::resolve(&fixture.invocation(
            &fixture.checkout_a,
            &digest(b"tree-a"),
            wrong_context
        )),
        Resolution::Refused(AdapterRefusal::AmbiguousInput)
            | Resolution::Refused(AdapterRefusal::UnsupportedInput)
    ));
}

#[test]
fn logical_policy_rejects_unaccepted_or_debug_stripping_inputs() {
    let fixture = SourceMapFixture::new("09-logical-policy");
    let policy = |accepted_policy: &str| {
        fixture.policy(
            vec![
                ("root-a", fixture.checkout_a.clone()),
                ("root-b", fixture.checkout_b.clone()),
            ],
            SourcePathMode::ExperimentalLogical {
                accepted_policy: accepted_policy.to_owned(),
                prefix: PathBuf::from("/oxide/fixture-component"),
            },
        )
    };
    let unaccepted = fixture.invocation(
        &fixture.checkout_a,
        &digest(b"tree-a"),
        policy("unrecorded-policy"),
    );
    assert!(matches!(
        resolve::resolve(&unaccepted),
        Resolution::Refused(AdapterRefusal::UnsupportedInput)
    ));

    let root_prefix = fixture.invocation(
        &fixture.checkout_a,
        &digest(b"tree-a"),
        fixture.policy(
            vec![
                ("root-a", fixture.checkout_a.clone()),
                ("root-b", fixture.checkout_b.clone()),
            ],
            SourcePathMode::ExperimentalLogical {
                accepted_policy: ACCEPTED_LOGICAL_POLICY.to_owned(),
                prefix: PathBuf::from("/"),
            },
        ),
    );
    assert!(matches!(
        resolve::resolve(&root_prefix),
        Resolution::Refused(AdapterRefusal::UnsupportedInput)
    ));

    let mut stripped = fixture.invocation(
        &fixture.checkout_a,
        &digest(b"tree-a"),
        policy(ACCEPTED_LOGICAL_POLICY),
    );
    stripped.inputs.effective_rustflags = Some(vec!["-Cdebuginfo=0".to_owned()]);
    assert!(matches!(
        resolve::resolve(&stripped),
        Resolution::Refused(AdapterRefusal::UnsupportedInput)
    ));
    stripped.inputs.effective_rustflags = Some(vec!["--strip=symbols".to_owned()]);
    assert!(matches!(
        resolve::resolve(&stripped),
        Resolution::Refused(AdapterRefusal::UnsupportedInput)
    ));
}

#[test]
fn mapped_run_without_exact_run_receipt_keeps_its_slot_uncertain() {
    let fixture = SourceMapFixture::new("10-mapped-uncertain");
    let invocation = fixture.invocation(
        &fixture.checkout_b,
        &digest(b"tree-b"),
        fixture.policy(
            vec![
                ("root-a", fixture.checkout_a.clone()),
                ("root-b", fixture.checkout_b.clone()),
            ],
            SourcePathMode::ExperimentalLogical {
                accepted_policy: ACCEPTED_LOGICAL_POLICY.to_owned(),
                prefix: PathBuf::from("/oxide/fixture-component"),
            },
        ),
    );
    let storage_root = fixture.root.dir("storage");
    let daemon = RunningDaemon::start(&storage_root);
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut consumer = FixtureConsumer::new();
    let mut runner = FixtureRunner::complete();
    let result = adapter.execute(
        &invocation,
        &mut consumer,
        &mut NoWait,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(result.disposition, ExecutionDisposition::Uncertain);
    assert_eq!(
        daemon
            .client
            .inventory("mapped-receipt-required")
            .unwrap()
            .slots[0]
            .state,
        SlotState::Uncertain
    );
    assert!(!adapter.observations()[0]
        .events
        .iter()
        .any(|event| matches!(event.kind, ObservationKind::SourceMapQualified { .. })));
}

#[test]
fn logical_qualifier_binds_the_exact_tree_lease_binaries_and_behavior_paths() {
    let fixture = SourceMapFixture::new("11-logical-qualifier");
    let policy = fixture.policy(
        vec![
            ("root-a", fixture.checkout_a.clone()),
            ("root-b", fixture.checkout_b.clone()),
        ],
        SourcePathMode::ExperimentalLogical {
            accepted_policy: ACCEPTED_LOGICAL_POLICY.to_owned(),
            prefix: PathBuf::from("/oxide/fixture-component"),
        },
    );
    let (resolved_b, map) =
        fixture.resolved_map(&fixture.checkout_b, &digest(b"tree-b"), policy.clone());
    let (resolved_a, _) = fixture.resolved_map(&fixture.checkout_a, &digest(b"tree-a"), policy);
    assert_eq!(
        resolved_a.partition.path_identity,
        resolved_b.partition.path_identity
    );
    let (receipt, evidence) = fixture.run_evidence(&map, ("root-a", &digest(b"tree-a")));
    assert_eq!(
        source_map::qualify(&map, &receipt, &evidence),
        QualificationDisposition::Pass
    );

    let mut stale_receipt = receipt.clone();
    stale_receipt.requested_source_tree_digest = digest(b"other-tree");
    assert_eq!(
        source_map::qualify(&map, &stale_receipt, &evidence),
        QualificationDisposition::Fail
    );
    let mut too_many_origins = receipt.clone();
    too_many_origins.compile_origins =
        vec![receipt.compile_origins[0].clone(); source_map::MAX_COMPILE_ORIGINS + 1];
    assert_eq!(
        source_map::qualify(&map, &too_many_origins, &evidence),
        QualificationDisposition::Insufficient
    );
    let changed_map_policy = fixture.policy(
        vec![
            ("root-a", fixture.checkout_a.clone()),
            ("root-b", fixture.checkout_b.clone()),
            ("root-c", fixture.checkout_c.clone()),
        ],
        SourcePathMode::ExperimentalLogical {
            accepted_policy: ACCEPTED_LOGICAL_POLICY.to_owned(),
            prefix: PathBuf::from("/oxide/fixture-component"),
        },
    );
    let (_, changed_map) =
        fixture.resolved_map(&fixture.checkout_b, &digest(b"tree-b"), changed_map_policy);
    assert_eq!(
        source_map::qualify(&changed_map, &receipt, &evidence),
        QualificationDisposition::Fail
    );

    let mut missing_resolution = evidence.clone();
    missing_resolution.binary_scans.as_mut().unwrap()[0].source_paths[0].resolved = None;
    assert_eq!(
        source_map::qualify(&map, &receipt, &missing_resolution),
        QualificationDisposition::Insufficient
    );
    let mut physical_source_path = evidence.clone();
    let physical_source = fixture.checkout_b.join("src/lib.rs");
    let resolved_physical_source = source_map::resolve_observed_source_path(&map, &physical_source);
    assert_eq!(
        resolved_physical_source.as_ref(),
        Some(&physical_source),
        "the resolver preserves a concrete path outside the logical prefix"
    );
    physical_source_path.binary_scans.as_mut().unwrap()[0].source_paths[0].observed =
        physical_source.clone();
    physical_source_path.binary_scans.as_mut().unwrap()[0].source_paths[0].resolved =
        resolved_physical_source;
    assert_eq!(
        source_map::qualify(&map, &receipt, &physical_source_path),
        QualificationDisposition::Fail
    );
    let mut mismatched_binary = evidence.clone();
    mismatched_binary.binary_scans.as_mut().unwrap()[0].binary_digest = digest(b"other binary");
    assert_eq!(
        source_map::qualify(&map, &receipt, &mismatched_binary),
        QualificationDisposition::Fail
    );
    let mut incomplete_scan = evidence.clone();
    incomplete_scan.binary_scans.as_mut().unwrap()[0].complete = false;
    assert_eq!(
        source_map::qualify(&map, &receipt, &incomplete_scan),
        QualificationDisposition::Insufficient
    );
    let mut stale_behavior = evidence.clone();
    stale_behavior.behavior_paths.as_mut().unwrap()[6].resolved =
        Some(fixture.checkout_a.join("assets/data.txt"));
    assert_eq!(
        source_map::qualify(&map, &receipt, &stale_behavior),
        QualificationDisposition::Fail
    );
    let mut wrong_artifact = receipt;
    wrong_artifact.binaries[0].reference = "../outside.bin".to_owned();
    assert_eq!(
        source_map::qualify(&map, &wrong_artifact, &evidence),
        QualificationDisposition::Fail
    );
}

#[test]
fn source_map_collector_completes_adapter_reacquire_and_rejects_bound_mismatches() {
    let fixture = SourceMapFixture::new("13-source-map-adapter");
    let tree = digest(b"tree-b");
    let policy = fixture.policy(
        vec![
            ("root-a", fixture.checkout_a.clone()),
            ("root-b", fixture.checkout_b.clone()),
        ],
        SourcePathMode::ExperimentalLogical {
            accepted_policy: ACCEPTED_LOGICAL_POLICY.to_owned(),
            prefix: PathBuf::from("/oxide/fixture-component"),
        },
    );
    let invocation = fixture.invocation(&fixture.checkout_b, &tree, policy.clone());
    let storage_root = fixture.root.dir("storage");
    let daemon = RunningDaemon::start(&storage_root);
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut consumer = FixtureConsumer::new();
    consumer.run_id = "mapped-run-1".to_owned();
    consumer.attempt_id = "mapped-attempt-1".to_owned();
    consumer.host_run_id = "mapped-host-1".to_owned();
    let mut runner = SourceMapFixtureRunner {
        tamper: SourceMapFixtureTamper::None,
    };
    let first = adapter.execute(
        &invocation,
        &mut consumer,
        &mut NoWait,
        &mut runner,
        &NeverCancelled,
    );
    assert_eq!(first.disposition, ExecutionDisposition::Finished);
    assert_eq!(
        first.finalization.as_ref().map(|receipt| receipt.state),
        Some(SlotState::Idle)
    );
    let slot = daemon.client.inventory("mapped-first-idle").unwrap().slots[0].clone();
    assert_eq!(slot.state, SlotState::Idle);
    let evidence_report =
        fs::read_to_string(invocation.capture_dir.join("source-map-evidence.txt"))
            .expect("task-owned evidence capture");
    assert!(evidence_report.contains("provenance=task-owned-fixture\n"));
    assert!(evidence_report.contains("hook=observed\n"));
    assert!(evidence_report.contains("binary=binary-0.bin|sha256:"));
    assert!(evidence_report.contains("missing=none\n"));
    assert!(adapter.observations()[0]
        .events
        .iter()
        .any(|event| matches!(event.kind, ObservationKind::SourceMapQualified { .. })));

    let second_invocation = fixture.invocation(&fixture.checkout_b, &tree, policy.clone());
    let mut second_consumer = FixtureConsumer::new();
    second_consumer.run_id = "mapped-run-2".to_owned();
    second_consumer.attempt_id = "mapped-attempt-2".to_owned();
    second_consumer.host_run_id = "mapped-host-2".to_owned();
    let mut second_runner = SourceMapFixtureRunner {
        tamper: SourceMapFixtureTamper::None,
    };
    let second = adapter.execute(
        &second_invocation,
        &mut second_consumer,
        &mut NoWait,
        &mut second_runner,
        &NeverCancelled,
    );
    assert_eq!(second.disposition, ExecutionDisposition::Finished);
    assert_eq!(
        daemon.client.inventory("mapped-second-idle").unwrap().slots[0].slot_id,
        slot.slot_id
    );

    for (index, tamper, wrong_capture) in [
        (0, SourceMapFixtureTamper::None, true),
        (1, SourceMapFixtureTamper::BinaryAfterCollection, false),
        (2, SourceMapFixtureTamper::WrongLease, false),
        (3, SourceMapFixtureTamper::ChangedMap, false),
        (4, SourceMapFixtureTamper::MissingHook, false),
        (5, SourceMapFixtureTamper::PhysicalSourcePath, false),
        (
            6,
            SourceMapFixtureTamper::RequestedPhysicalSourcePath,
            false,
        ),
    ] {
        let invocation = fixture.invocation(&fixture.checkout_b, &tree, policy.clone());
        let failure_storage = fixture.root.dir(&format!("failure-storage-{index}"));
        let failure_daemon = RunningDaemon::start(&failure_storage);
        let mut failure_adapter = ProjectAdapter::new(failure_daemon.client.clone());
        let mut failure_consumer = FixtureConsumer::new();
        failure_consumer.run_id = format!("mapped-failure-{index}");
        failure_consumer.attempt_id = format!("mapped-failure-attempt-{index}");
        failure_consumer.host_run_id = format!("mapped-failure-host-{index}");
        failure_consumer.wrong_capture_digest = wrong_capture;
        let mut failure_runner = SourceMapFixtureRunner { tamper };
        let result = failure_adapter.execute(
            &invocation,
            &mut failure_consumer,
            &mut NoWait,
            &mut failure_runner,
            &NeverCancelled,
        );
        assert_eq!(result.disposition, ExecutionDisposition::Uncertain);
        assert_eq!(
            failure_daemon
                .client
                .inventory(&format!("mapped-failure-inventory-{index}"))
                .unwrap()
                .slots[0]
                .state,
            SlotState::Uncertain
        );
        if matches!(tamper, SourceMapFixtureTamper::MissingHook) {
            let report = fs::read_to_string(invocation.capture_dir.join("source-map-evidence.txt"))
                .expect("bounded missing-hook evidence capture");
            assert!(report.contains("hook=missing\n"));
            assert!(report.contains("observation-hook"));
            assert!(report.contains("binary-artifacts"));
        }
        if matches!(
            tamper,
            SourceMapFixtureTamper::PhysicalSourcePath
                | SourceMapFixtureTamper::RequestedPhysicalSourcePath
        ) {
            assert!(!failure_adapter.observations()[0]
                .events
                .iter()
                .any(|event| matches!(event.kind, ObservationKind::SourceMapQualified { .. })));
        }
    }
}

#[test]
fn mapped_source_map_run_with_retained_writer_keeps_slot_uncertain() {
    let fixture = SourceMapFixture::new("15-source-map-retained-writer");
    let space = NativeScenarioSpace::new();
    space.sample(&fixture.root.path);
    let policy = fixture.policy(
        vec![
            ("root-a", fixture.checkout_a.clone()),
            ("root-b", fixture.checkout_b.clone()),
        ],
        SourcePathMode::ExperimentalLogical {
            accepted_policy: ACCEPTED_LOGICAL_POLICY.to_owned(),
            prefix: PathBuf::from("/oxide/fixture-component"),
        },
    );
    let invocation = fixture.invocation(&fixture.checkout_b, &digest(b"tree-b"), policy.clone());
    assert!(matches!(
        resolve::resolve(&invocation),
        Resolution::Ready(resolved) if resolved.source_map.is_some()
    ));
    let storage_root = fixture.root.dir("storage");
    let daemon = RunningDaemon::start_with_probe(&storage_root, space.probe());
    let (started_tx, started_rx) = mpsc::channel();
    let mut runner = native::NativeFixtureCommandRunner::retain_writer(started_tx)
        .expect("bounded retained-writer runner");
    let mut adapter = ProjectAdapter::new(daemon.client.clone());
    let mut consumer = FixtureConsumer::new();
    consumer.run_id = "mapped-retained-run".to_owned();
    consumer.attempt_id = "mapped-retained-attempt".to_owned();
    consumer.host_run_id = "mapped-retained-host".to_owned();
    let result = adapter.execute(
        &invocation,
        &mut consumer,
        &mut NoWait,
        &mut runner,
        &NeverCancelled,
    );
    let started = expect_held(
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("mapped fixture writer remains active"),
        "mapped retained writer",
    );
    assert!(started.pid > 0);
    assert!(runner.has_retained_writer() && runner.retained_writer_is_active());
    assert_eq!(result.disposition, ExecutionDisposition::Uncertain);
    assert_eq!(consumer.last_terminal_outcome, None);
    assert_eq!(consumer.capture_calls, 0);
    assert!(invocation.capture_dir.join("writer-progress.txt").is_file());
    let slot = daemon
        .client
        .inventory("mapped-retained-writer")
        .unwrap()
        .slots
        .pop()
        .expect("mapped uncertain slot remains reserved");
    assert_eq!(slot.state, SlotState::Uncertain);

    let next_invocation = fixture.invocation(&fixture.checkout_b, &digest(b"tree-b"), policy);
    let mut next_consumer = FixtureConsumer::new();
    next_consumer.run_id = "mapped-after-retained".to_owned();
    next_consumer.attempt_id = "mapped-after-retained-attempt".to_owned();
    next_consumer.host_run_id = "mapped-after-retained-host".to_owned();
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
        .expect("reap only the identified fixture writer");
    let after_reap = daemon
        .client
        .inventory("mapped-retained-after-reap")
        .unwrap();
    assert_eq!(after_reap.slots[0].state, SlotState::Uncertain);
    assert_eq!(
        daemon
            .client
            .evict("mapped-no-uncertain-eviction", 1)
            .unwrap(),
        0
    );
    space.save(&fixture.root);
}

#[test]
fn strict_physical_mode_rejects_compile_origin_paths_from_another_root() {
    let fixture = SourceMapFixture::new("12-strict-physical");
    let policy = fixture.policy(
        vec![
            ("root-a", fixture.checkout_a.clone()),
            ("root-b", fixture.checkout_b.clone()),
        ],
        SourcePathMode::StrictPhysical,
    );
    let (_, map) = fixture.resolved_map(&fixture.checkout_b, &digest(b"tree-b"), policy);
    let (receipt, evidence) = fixture.run_evidence(&map, ("root-a", &digest(b"tree-a")));
    assert_eq!(
        source_map::qualify(&map, &receipt, &evidence),
        QualificationDisposition::Fail
    );
}
