use super::*;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Barrier};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(feature = "private-qualification")]
mod stable_reuse;

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(1);
const TEST_DAEMON_ROOT: &str = "OXIDE_STORAGE_TEST_DAEMON_ROOT";

struct TaskRoot(PathBuf);

impl TaskRoot {
    fn new() -> Self {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let parent = base.join("t");
        fs::create_dir_all(&parent).expect("create under task-owned scratch root");
        loop {
            let id = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("d{:x}-{:x}", std::process::id(), id));
            match fs::create_dir(&path) {
                Ok(()) => {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                        .expect("private task root");
                    return Self(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create private task root: {}", error),
            }
        }
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TaskRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct TestAuthority;

struct RejectingOrdinaryAuthority;

impl EvidenceAuthority for RejectingOrdinaryAuthority {
    fn verify_admission(&self, envelope: &RunEnvelope) -> bool {
        envelope.admission_reference == "admission-ref"
            && envelope.admission_credential.expose() == "admission-proof"
    }

    fn verify_settlement(&self, evidence: &SettlementEvidence) -> bool {
        evidence.consumer_credential.expose() == "consumer-proof"
    }
}

impl EvidenceAuthority for TestAuthority {
    fn verify_admission(&self, envelope: &RunEnvelope) -> bool {
        envelope.admission_reference == "admission-ref"
            && envelope.admission_credential.expose() == "admission-proof"
    }

    fn verify_settlement(&self, evidence: &SettlementEvidence) -> bool {
        evidence.consumer_credential.expose() == "consumer-proof"
            && evidence
                .terminal
                .as_ref()
                .map(|terminal| terminal.credential.expose() == "terminal-proof")
                .unwrap_or(true)
            && evidence
                .capture
                .as_ref()
                .map(|capture| capture.credential.expose() == "capture-proof")
                .unwrap_or(true)
            && match &evidence.containment {
                ContainmentEvidence::CooperativeWorkflow(completion) => {
                    !completion.signature.is_empty()
                }
                _ => true,
            }
    }

    fn verify_ordinary_cargo_admission(&self, request: &OrdinaryCargoRequest) -> bool {
        request.acceptance.policy_version == ORDINARY_CARGO_POLICY_VERSION
            && request.acceptance.installation_epoch == "install-1"
            && request.acceptance.acceptance_reference == "ordinary-acceptance"
            && request.acceptance.input_boundary_digest == format!("sha256:{}", "a".repeat(64))
            && request.acceptance.credential.expose() == "ordinary-proof"
            && request.root.consumer_root_id == "consumer-root"
    }

    fn verify_ordinary_cargo_settlement(
        &self,
        request: &OrdinaryCargoRequest,
        evidence: &SettlementEvidence,
    ) -> bool {
        self.verify_ordinary_cargo_admission(request)
            && self.verify_settlement(evidence)
            && evidence.binding == request.acquire.envelope.binding
    }
}

struct FixedFreeSpace(Option<u64>);

impl FreeSpaceProbe for FixedFreeSpace {
    fn available_bytes(&self, _: &Path) -> Result<Option<u64>, StorageError> {
        Ok(self.0)
    }
}

struct RunningDaemon {
    client: Client,
    socket_path: PathBuf,
    shutdown: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<Result<(), StorageError>>>,
}

impl RunningDaemon {
    fn start(root: &Path, budget: Budget, free_bytes: Option<u64>) -> Self {
        Self::start_with_authority(root, budget, free_bytes, Arc::new(TestAuthority))
    }

    fn start_with_authority(
        root: &Path,
        budget: Budget,
        free_bytes: Option<u64>,
        authority: Arc<dyn EvidenceAuthority>,
    ) -> Self {
        Self::bind(root, budget, free_bytes, authority, None)
    }

    fn bind(
        root: &Path,
        budget: Budget,
        free_bytes: Option<u64>,
        authority: Arc<dyn EvidenceAuthority>,
        backing: Option<BoundedBacking>,
    ) -> Self {
        let daemon = Daemon::bind(
            DaemonConfig {
                root: root.to_path_buf(),
                budget,
                backing,
            },
            authority,
            Arc::new(FixedFreeSpace(free_bytes)),
        )
        .expect("bind daemon");
        let socket_path = daemon.socket_path().to_path_buf();
        let client = Client::new(socket_path.clone());
        let (shutdown, receiver) = mpsc::channel();
        let thread = thread::spawn(move || daemon.serve(receiver));
        Self {
            client,
            socket_path,
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }

    fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            assert!(thread.join().expect("daemon thread joined").is_ok());
        }
    }
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        self.stop();
    }
}

struct ChildDaemon(Child);

impl ChildDaemon {
    fn start(root: &Path) -> Self {
        let executable = std::env::current_exe().expect("test executable path");
        let child = Command::new(executable)
            .arg("--exact")
            .arg("storage::tests::child_daemon_process")
            .arg("--nocapture")
            .env(TEST_DAEMON_ROOT, root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start task-owned daemon child");
        let mut daemon = Self(child);
        wait_until_ready(&mut daemon.0, root);
        daemon
    }

    fn crash(&mut self) {
        self.0.kill().expect("stop only this test's daemon process");
        let _ = self.0.wait().expect("reap task-owned daemon process");
    }
}

impl Drop for ChildDaemon {
    fn drop(&mut self) {
        if self
            .0
            .try_wait()
            .expect("inspect task-owned child")
            .is_none()
        {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn wait_until_ready(child: &mut Child, root: &Path) {
    let client = Client::new(root.join("daemon.sock"));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(exit) = child.try_wait().expect("inspect child status") {
            panic!("daemon child exited before readiness: {}", exit);
        }
        if client.inventory("ready").is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "daemon child did not become ready"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn budget(cap_bytes: u64) -> Budget {
    Budget {
        cap_bytes,
        free_reserve_bytes: 20,
    }
}

fn request(attempt: &str, acquire_id: &str, required_bytes: u64) -> AcquireRequest {
    let binding = RunBinding {
        consumer: "queue".to_owned(),
        project: "oxide".to_owned(),
        component: "core-tests".to_owned(),
        workflow: "rust:test".to_owned(),
        task_id: "task-7".to_owned(),
        workspace_id: "workspace-3".to_owned(),
        run_id: format!("run-{}", attempt),
        attempt_id: attempt.to_owned(),
        commit: "0123456789abcdef".to_owned(),
        source_tree_digest: "sha256:source-tree".to_owned(),
    };
    AcquireRequest {
        envelope: RunEnvelope {
            binding,
            partition: PartitionKey {
                project_component: "oxide:core-tests".to_owned(),
                workflow: "rust:test".to_owned(),
                cargo_provenance: "cargo 1.97.1".to_owned(),
                rustc_provenance: "rustc 1.97.1".to_owned(),
                host: "x86_64-apple-darwin".to_owned(),
                target: "x86_64-apple-darwin".to_owned(),
                profile: "test".to_owned(),
                features: vec!["default".to_owned()],
                config_digest: "sha256:cargo-config".to_owned(),
                environment_digest: "sha256:build-env".to_owned(),
                path_identity: "/task/checkouts/a".to_owned(),
            },
            admission_reference: "admission-ref".to_owned(),
            admission_credential: Credential::new("admission-proof"),
        },
        acquire_id: acquire_id.to_owned(),
        required_bytes,
    }
}

fn ordinary_request(
    source_root: &Path,
    attempt: &str,
    acquire_id: &str,
    required_bytes: u64,
    partition: OrdinaryCargoPartition,
) -> OrdinaryCargoRequest {
    let mut acquire = request(attempt, acquire_id, required_bytes);
    acquire.envelope.partition.path_identity = "ordinary-cargo-storage-v1".to_owned();
    OrdinaryCargoRequest {
        acquire,
        root: StrictPhysicalCargoRoot {
            source_root: source_root.to_path_buf(),
            consumer_root_id: "consumer-root".to_owned(),
            allocation_epoch: "allocation-1".to_owned(),
        },
        partition,
        acceptance: OrdinaryCargoAcceptance {
            policy_version: ORDINARY_CARGO_POLICY_VERSION,
            installation_epoch: "install-1".to_owned(),
            acceptance_reference: "ordinary-acceptance".to_owned(),
            input_boundary_digest: format!("sha256:{}", "a".repeat(64)),
            credential: Credential::new("ordinary-proof"),
        },
    }
}

fn passing_evidence(
    binding: &RunBinding,
    lease_id: &str,
    slot_id: &str,
    slot_path: &Path,
    token: &LeaseToken,
) -> SettlementEvidence {
    let mut evidence = complete_evidence(binding, lease_id, slot_id, slot_path, token);
    if let Some(terminal) = evidence.terminal.as_mut() {
        terminal.outcome = TerminalOutcome::Passed;
    }
    if let ContainmentEvidence::CooperativeWorkflow(completion) = &mut evidence.containment {
        completion.exit_code = 0;
        completion.sign(token);
    }
    evidence
}

fn complete_evidence(
    binding: &RunBinding,
    lease_id: &str,
    slot_id: &str,
    slot_path: &Path,
    token: &LeaseToken,
) -> SettlementEvidence {
    let capture = CaptureReceipt {
        reference: "task-capture/run-1".to_owned(),
        digest: "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
            .to_owned(),
        host_run_id: "host-run-1".to_owned(),
        run_id: binding.run_id.clone(),
        attempt_id: binding.attempt_id.clone(),
        credential: Credential::new("capture-proof"),
    };
    let mut completion = CooperativeCompletionEvidence {
        lease_id: lease_id.to_owned(),
        slot_id: slot_id.to_owned(),
        slot_path: slot_path.to_string_lossy().into_owned(),
        temp_dir: "/task/tmp".to_owned(),
        capture_dir: "/task/capture".to_owned(),
        workflow: binding.workflow.clone(),
        workflow_definition_digest:
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        repository_head: "0".repeat(40),
        fixture_source_digest:
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
        host_run_id: capture.host_run_id.clone(),
        run_id: binding.run_id.clone(),
        attempt_id: binding.attempt_id.clone(),
        processes: vec![ProcessIdentityEvidence {
            pid: 1,
            start_identity: format!("sha256:{}", "f".repeat(64)),
        }],
        free_space_samples: vec![20 * 1024 * 1024 * 1024],
        exit_code: 1,
        stdout_digest: "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
            .to_owned(),
        stderr_digest: "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
            .to_owned(),
        capture_reference: capture.reference.clone(),
        capture_digest: capture.digest.clone(),
        signature: String::new(),
    };
    completion.sign(token);
    SettlementEvidence {
        binding: binding.clone(),
        consumer_credential: Credential::new("consumer-proof"),
        terminal: Some(TerminalRecord {
            run_id: binding.run_id.clone(),
            outcome: TerminalOutcome::Failed,
            credential: Credential::new("terminal-proof"),
        }),
        capture: Some(capture),
        containment: ContainmentEvidence::CooperativeWorkflow(completion),
    }
}

#[test]
fn child_daemon_process() {
    let root = match std::env::var(TEST_DAEMON_ROOT) {
        Ok(root) => root,
        Err(_) => return,
    };
    let daemon = Daemon::bind(
        DaemonConfig {
            root: PathBuf::from(root),
            budget: budget(128),
            backing: None,
        },
        Arc::new(TestAuthority),
        Arc::new(FixedFreeSpace(Some(1024))),
    )
    .expect("bind task-owned child daemon");
    let (_shutdown, receiver) = mpsc::channel();
    daemon.serve(receiver).expect("serve child daemon");
}

#[test]
fn competing_claims_never_share_a_mutable_slot() {
    let root = TaskRoot::new();
    let daemon = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let gate = Arc::new(Barrier::new(3));
    let claims = [
        request("attempt-a", "acquire-a", 64),
        request("attempt-b", "acquire-b", 64),
    ];
    let threads: Vec<_> = claims
        .into_iter()
        .map(|claim| {
            let client = daemon.client.clone();
            let gate = Arc::clone(&gate);
            thread::spawn(move || {
                gate.wait();
                client.acquire(claim)
            })
        })
        .collect();
    gate.wait();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().expect("claim thread joined"))
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| {
                **result == Err(StorageError::Refused(RefusalReason::LeaseUnavailable))
            })
            .count(),
        1
    );
    assert_eq!(
        daemon
            .client
            .inventory("claims")
            .expect("inventory")
            .slots
            .len(),
        1
    );
}

#[test]
fn acquire_retry_reuses_lease_and_changed_identity_conflicts() {
    let root = TaskRoot::new();
    let daemon = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let first = request("attempt-a", "acquire-a", 64);
    let grant = daemon.client.acquire(first.clone()).expect("acquire");
    let retry = daemon.client.acquire(first).expect("retry same acquire");
    assert_eq!(retry, grant);
    let token = retry.token.as_ref().expect("active lease token");
    let ledger = fs::read(root.path().join("ledger.bin")).expect("read task ledger");
    assert!(!ledger
        .windows(token.value().len())
        .any(|window| window == token.value().as_bytes()));
    assert!(!ledger
        .windows(b"admission-proof".len())
        .any(|window| window == &b"admission-proof"[..]));

    let changed = request("attempt-a", "different-acquire", 64);
    assert_eq!(daemon.client.acquire(changed), Err(StorageError::Conflict));
}

#[test]
fn acquire_lookup_is_authenticated_exact_and_read_only() {
    let root = TaskRoot::new();
    let daemon = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let claim = request("attempt-lookup", "acquire-lookup", 64);
    let before_absent = fs::read(root.path().join("ledger.bin")).expect("read empty ledger");
    assert_eq!(
        daemon
            .client
            .lookup_acquire(request("attempt-cancelled", "acquire-cancelled", 64))
            .expect("lookup absent attempt"),
        LookupResult::Absent
    );
    assert_eq!(
        fs::read(root.path().join("ledger.bin")).expect("ledger after absent lookup"),
        before_absent
    );
    assert!(daemon
        .client
        .inventory("lookup-absent")
        .expect("inventory")
        .slots
        .is_empty());

    let grant = daemon.client.acquire(claim.clone()).expect("acquire");
    let after_acquire = fs::read(root.path().join("ledger.bin")).expect("read acquired ledger");
    let found = daemon
        .client
        .lookup_acquire(claim.clone())
        .expect("lookup exact acquire");
    assert_eq!(
        found,
        LookupResult::Present(LeaseIdentity {
            lease_id: grant.lease_id.clone(),
            slot_id: grant.slot_id.clone(),
            storage_instance_id: grant.storage_instance_id.clone(),
            generation: grant.generation.clone(),
            state: SlotState::Leased,
        })
    );
    assert_eq!(
        fs::read(root.path().join("ledger.bin")).expect("ledger after present lookup"),
        after_acquire
    );

    let mut changed_body = claim.clone();
    changed_body.envelope.binding.source_tree_digest = "sha256:changed".to_owned();
    assert_eq!(
        daemon.client.lookup_acquire(changed_body),
        Err(StorageError::Conflict)
    );
    let mut changed_subject = claim.clone();
    changed_subject.envelope.binding.task_id = "different-task".to_owned();
    assert_eq!(
        daemon.client.lookup_acquire(changed_subject),
        Err(StorageError::Conflict)
    );
    let mut changed_operation_subject = request("different-attempt", "acquire-lookup", 64);
    changed_operation_subject.envelope.binding.consumer = "other-consumer".to_owned();
    assert_eq!(
        daemon.client.lookup_acquire(changed_operation_subject),
        Err(StorageError::Conflict)
    );
    let mut changed_credential = claim;
    changed_credential.envelope.admission_credential = Credential::new("wrong-credential");
    assert_eq!(
        daemon.client.lookup_acquire(changed_credential),
        Err(StorageError::Unauthorized)
    );
    assert_eq!(
        daemon
            .client
            .inventory("lookup-still-leased")
            .expect("inventory after refusals")
            .slots
            .first()
            .expect("active slot")
            .state,
        SlotState::Leased
    );
}

#[test]
fn finalize_lookup_reports_absence_and_exact_acceptance_without_effects() {
    let root = TaskRoot::new();
    let daemon = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let claim = request("attempt-finalize-lookup", "acquire-finalize-lookup", 64);
    let grant = daemon.client.acquire(claim.clone()).expect("acquire");
    let token = grant.token.as_ref().expect("lease token");
    let evidence = complete_evidence(
        &claim.envelope.binding,
        &grant.lease_id,
        &grant.slot_id,
        grant.slot_path.as_ref().expect("slot path"),
        token,
    );

    let before_absent = fs::read(root.path().join("ledger.bin")).expect("read leased ledger");
    assert_eq!(
        daemon
            .client
            .lookup_finalize(&grant.lease_id, "finalize-recover", evidence.clone())
            .expect("lookup unaccepted finalize"),
        LookupResult::Absent
    );
    assert_eq!(
        fs::read(root.path().join("ledger.bin")).expect("ledger after absent finalize lookup"),
        before_absent
    );

    let accepted = daemon
        .client
        .finalize(
            &grant.lease_id,
            Some(token),
            "finalize-recover",
            evidence.clone(),
        )
        .expect("finalize");
    assert_eq!(accepted.state, SlotState::Idle);
    let after_finalize = fs::read(root.path().join("ledger.bin")).expect("read finalized ledger");
    assert_eq!(
        daemon
            .client
            .lookup_finalize(&grant.lease_id, "finalize-recover", evidence.clone())
            .expect("lookup accepted finalize"),
        LookupResult::Present(accepted)
    );
    assert_eq!(
        fs::read(root.path().join("ledger.bin")).expect("ledger after accepted lookup"),
        after_finalize
    );

    let mut changed_body = evidence.clone();
    if let Some(terminal) = changed_body.terminal.as_mut() {
        terminal.outcome = TerminalOutcome::Passed;
    }
    assert_eq!(
        daemon
            .client
            .lookup_finalize(&grant.lease_id, "finalize-recover", changed_body),
        Err(StorageError::Conflict)
    );
    let mut changed_subject = evidence.clone();
    changed_subject.binding.project = "other-project".to_owned();
    assert_eq!(
        daemon
            .client
            .lookup_finalize(&grant.lease_id, "finalize-recover", changed_subject),
        Err(StorageError::Conflict)
    );
    let mut changed_credential = evidence;
    changed_credential.consumer_credential = Credential::new("wrong-credential");
    assert_eq!(
        daemon
            .client
            .lookup_finalize(&grant.lease_id, "finalize-recover", changed_credential),
        Err(StorageError::Unauthorized)
    );
}

#[test]
fn generation_tracks_slot_incarnations_and_is_fenced_by_storage_instance() {
    let root = TaskRoot::new();
    let mut daemon = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let first_claim = request("attempt-generation-1", "acquire-generation-1", 64);
    let first = daemon
        .client
        .acquire(first_claim.clone())
        .expect("first slot acquire");
    assert_eq!(first.generation, "1");
    assert_eq!(first.storage_instance_id.len(), 64);
    let first_evidence = complete_evidence(
        &first_claim.envelope.binding,
        &first.lease_id,
        &first.slot_id,
        first.slot_path.as_ref().expect("first slot path"),
        first.token.as_ref().expect("first token"),
    );
    daemon
        .client
        .finalize(
            &first.lease_id,
            first.token.as_ref(),
            "finalize-generation-1",
            first_evidence,
        )
        .expect("finalize first lease");

    let mut second_claim = request("attempt-generation-2", "acquire-generation-2", 64);
    second_claim.envelope.binding.source_tree_digest = "sha256:next-source-tree".to_owned();
    let second = daemon
        .client
        .acquire(second_claim.clone())
        .expect("reuse slot for next source revision");
    assert_eq!(second.slot_id, first.slot_id);
    assert_eq!(second.generation, first.generation);
    assert_eq!(second.storage_instance_id, first.storage_instance_id);
    let second_evidence = complete_evidence(
        &second_claim.envelope.binding,
        &second.lease_id,
        &second.slot_id,
        second.slot_path.as_ref().expect("second slot path"),
        second.token.as_ref().expect("second token"),
    );
    daemon
        .client
        .finalize(
            &second.lease_id,
            second.token.as_ref(),
            "finalize-generation-2",
            second_evidence,
        )
        .expect("finalize second lease");
    let finalized_replay = daemon
        .client
        .acquire(second_claim.clone())
        .expect("acquire replay");
    assert_eq!(finalized_replay.generation, "1");
    assert_eq!(finalized_replay.state, SlotState::Idle);
    assert_eq!(finalized_replay.slot_path, None);
    assert_eq!(finalized_replay.token, None);
    assert_eq!(
        daemon
            .client
            .lookup_acquire(second_claim)
            .expect("lookup finalized replay"),
        LookupResult::Present(LeaseIdentity {
            lease_id: second.lease_id.clone(),
            slot_id: second.slot_id.clone(),
            storage_instance_id: second.storage_instance_id.clone(),
            generation: second.generation.clone(),
            state: SlotState::Idle,
        })
    );
    daemon
        .client
        .evict("evict-generation-1", 1)
        .expect("evict old slot");

    let third_claim = request("attempt-generation-3", "acquire-generation-3", 64);
    let third = daemon
        .client
        .acquire(third_claim.clone())
        .expect("allocate replacement slot");
    assert_eq!(third.generation, "2");
    assert_eq!(third.storage_instance_id, first.storage_instance_id);
    let third_evidence = complete_evidence(
        &third_claim.envelope.binding,
        &third.lease_id,
        &third.slot_id,
        third.slot_path.as_ref().expect("third slot path"),
        third.token.as_ref().expect("third token"),
    );
    daemon
        .client
        .finalize(
            &third.lease_id,
            third.token.as_ref(),
            "finalize-generation-3",
            third_evidence,
        )
        .expect("finalize replacement lease");
    daemon
        .client
        .evict("evict-generation-2", 1)
        .expect("evict replacement slot");
    daemon.stop();

    fs::remove_file(root.path().join("ledger.bin")).expect("recreate task-owned ledger fixture");
    let recreated = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let fresh = recreated
        .client
        .acquire(request(
            "attempt-generation-fresh",
            "acquire-generation-fresh",
            64,
        ))
        .expect("acquire under new storage instance");
    assert_eq!(fresh.generation, "1");
    assert_ne!(fresh.storage_instance_id, first.storage_instance_id);
}

#[test]
fn unsupported_legacy_wire_request_is_rejected_before_acquire_effects() {
    let root = TaskRoot::new();
    let daemon = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let before = fs::read(root.path().join("ledger.bin")).expect("read empty ledger");
    super::transport::send_legacy_acquire_and_forget(
        &root.path().join("daemon.sock"),
        request("attempt-legacy", "acquire-legacy", 64),
    )
    .expect("send legacy request to versioned endpoint");
    super::transport::send_unsupported_version_acquire_and_forget(
        &root.path().join("daemon.sock"),
        request("attempt-future", "acquire-future", 64),
    )
    .expect("send unsupported version to current endpoint");
    assert_eq!(
        fs::read(root.path().join("ledger.bin")).expect("ledger after legacy request"),
        before
    );
    assert!(daemon
        .client
        .inventory("legacy-refused")
        .expect("inventory")
        .slots
        .is_empty());
}

#[test]
fn unsupported_ledger_version_preserves_old_ledger_and_slot_evidence() {
    let root = TaskRoot::new();
    let mut daemon = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let grant = daemon
        .client
        .acquire(request("attempt-old-ledger", "acquire-old-ledger", 64))
        .expect("acquire slot");
    let evidence_path = grant
        .slot_path
        .as_ref()
        .expect("slot path")
        .join("retained-evidence.bin");
    fs::write(&evidence_path, b"preserve old evidence")
        .expect("write bounded task-owned evidence fixture");
    daemon.stop();

    let stale_temp = root.path().join("ledger.tmp-0123456789abcdef");
    fs::write(&stale_temp, b"preserve with unsupported ledger")
        .expect("write bounded task-owned ledger temp fixture");
    fs::set_permissions(&stale_temp, fs::Permissions::from_mode(0o600))
        .expect("make task-owned ledger temp private");
    let ledger_path = root.path().join("ledger.bin");
    let mut old_ledger = fs::read(&ledger_path).expect("read current ledger");
    let version = old_ledger
        .get_mut(12..16)
        .expect("ledger header contains version bytes");
    version.copy_from_slice(&3_u32.to_le_bytes());
    fs::write(&ledger_path, &old_ledger).expect("mark fixture with unsupported version");
    let result = Daemon::bind(
        DaemonConfig {
            root: root.path().to_path_buf(),
            budget: budget(128),
            backing: None,
        },
        Arc::new(TestAuthority),
        Arc::new(FixedFreeSpace(Some(1024))),
    );
    assert!(matches!(
        result,
        Err(StorageError::Refused(RefusalReason::UnknownAccounting))
    ));
    assert_eq!(
        fs::read(&ledger_path).expect("old ledger remains"),
        old_ledger
    );
    assert_eq!(
        fs::read(&evidence_path).expect("old slot evidence remains"),
        b"preserve old evidence"
    );
    assert_eq!(
        fs::read(&stale_temp).expect("unsupported ledger temp remains"),
        b"preserve with unsupported ledger"
    );
}

#[test]
fn a_second_daemon_cannot_bind_the_same_private_root() {
    let root = TaskRoot::new();
    let daemon = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let second = Daemon::bind(
        DaemonConfig {
            root: root.path().to_path_buf(),
            budget: budget(128),
            backing: None,
        },
        Arc::new(TestAuthority),
        Arc::new(FixedFreeSpace(Some(1024))),
    );
    assert!(matches!(
        second,
        Err(StorageError::Refused(RefusalReason::AlreadyRunning))
    ));
    assert!(daemon.client.inventory("single-owner").is_ok());
}

#[test]
fn process_restart_quarantines_until_authenticated_reconciliation() {
    let root = TaskRoot::new();
    let mut child = ChildDaemon::start(root.path());
    let client = Client::new(root.path().join("daemon.sock"));
    let claim = request("attempt-restart", "acquire-restart", 64);
    let grant = client.acquire(claim.clone()).expect("acquire before crash");
    child.crash();

    let restarted = ChildDaemon::start(root.path());
    let recovered = Client::new(root.path().join("daemon.sock"));
    let inventory = recovered
        .inventory("recovered")
        .expect("recovered inventory");
    assert_eq!(inventory.slots[0].state, SlotState::Uncertain);
    assert_eq!(inventory.slots[0].measured_bytes, None);
    let retry = recovered
        .acquire(claim.clone())
        .expect("replay after restart");
    assert_eq!(retry.lease_id, grant.lease_id);
    assert_eq!(retry.state, SlotState::Uncertain);
    assert_eq!(retry.slot_path, None);
    assert_eq!(retry.token, None);
    assert_eq!(retry.generation, grant.generation);
    assert_eq!(retry.storage_instance_id, grant.storage_instance_id);
    assert_eq!(
        recovered
            .lookup_acquire(claim)
            .expect("lookup uncertain acquire"),
        LookupResult::Uncertain(Some(LeaseIdentity {
            lease_id: grant.lease_id.clone(),
            slot_id: grant.slot_id.clone(),
            storage_instance_id: grant.storage_instance_id.clone(),
            generation: grant.generation.clone(),
            state: SlotState::Uncertain,
        }))
    );
    let replayed_claim = request("attempt-restart", "unused", 64);
    let binding = &replayed_claim.envelope.binding;
    let evidence = SettlementEvidence {
        binding: binding.clone(),
        consumer_credential: Credential::new("consumer-proof"),
        terminal: None,
        capture: None,
        containment: ContainmentEvidence::Unknown,
    };
    assert_eq!(
        recovered
            .lookup_finalize(&grant.lease_id, "finalize-not-recorded", evidence.clone())
            .expect("lookup absent finalize on uncertain lease"),
        LookupResult::Uncertain(None)
    );
    let reconciled = recovered
        .reconcile(&grant.lease_id, None, "reconcile-restart", evidence)
        .expect("reconcile authenticated proof");
    assert_eq!(reconciled.state, SlotState::Uncertain);
    assert_eq!(
        recovered
            .lookup_acquire(request("attempt-restart", "acquire-restart", 64))
            .expect("lookup after uncertain reconciliation"),
        LookupResult::Uncertain(Some(LeaseIdentity {
            lease_id: grant.lease_id.clone(),
            slot_id: grant.slot_id.clone(),
            storage_instance_id: grant.storage_instance_id.clone(),
            generation: grant.generation.clone(),
            state: SlotState::Uncertain,
        }))
    );
    assert_eq!(
        recovered.acquire(request("attempt-next", "acquire-next", 64)),
        Err(StorageError::Refused(RefusalReason::LeaseUnavailable))
    );
    drop(restarted);
}

#[test]
fn lost_acquire_reply_is_idempotent_and_does_not_start_a_second_lease() {
    let root = TaskRoot::new();
    let daemon = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let claim = request("attempt-lost", "acquire-lost", 64);
    super::transport::send_acquire_and_forget(&root.path().join("daemon.sock"), claim.clone())
        .expect("send request and drop response");
    let retry = daemon
        .client
        .acquire(claim)
        .expect("retry after lost reply");
    assert_eq!(retry.state, SlotState::Leased);
    assert_eq!(
        daemon
            .client
            .inventory("lost")
            .expect("inventory")
            .slots
            .len(),
        1
    );
}

#[test]
fn vanished_process_and_missing_proof_keep_storage_unavailable() {
    let root = TaskRoot::new();
    let daemon = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let claim = request("attempt-uncertain", "acquire-uncertain", 64);
    let grant = daemon.client.acquire(claim.clone()).expect("acquire");
    let token = grant.token.as_ref().expect("lease token");
    let mut evidence = complete_evidence(
        &claim.envelope.binding,
        &grant.lease_id,
        &grant.slot_id,
        grant.slot_path.as_ref().expect("slot path"),
        token,
    );
    evidence.containment = ContainmentEvidence::ProcessAbsent {
        pid: 999_999,
        start_identity: "gone".to_owned(),
    };
    let settled = daemon
        .client
        .finalize(
            &grant.lease_id,
            Some(token),
            "finalize-uncertain",
            evidence.clone(),
        )
        .expect("incomplete proof is recorded conservatively");
    assert_eq!(settled.state, SlotState::Uncertain);
    let accepted_ledger = fs::read(root.path().join("ledger.bin")).expect("read uncertain ledger");
    assert_eq!(
        daemon
            .client
            .lookup_finalize(&grant.lease_id, "finalize-uncertain", evidence)
            .expect("lookup accepted uncertain finalize"),
        LookupResult::Uncertain(Some(settled))
    );
    assert_eq!(
        fs::read(root.path().join("ledger.bin")).expect("ledger after uncertain lookup"),
        accepted_ledger
    );
    assert_eq!(
        daemon
            .client
            .evict("evict-uncertain", 1)
            .expect("safe refusal"),
        0
    );
    let inventory = daemon.client.inventory("uncertain").expect("inventory");
    assert_eq!(inventory.slots[0].state, SlotState::Uncertain);
    assert_eq!(inventory.slots[0].measured_bytes, None);
    assert_eq!(
        daemon
            .client
            .acquire(request("attempt-other", "acquire-other", 1)),
        Err(StorageError::Refused(RefusalReason::LeaseUnavailable))
    );
}

#[test]
fn mutated_cooperative_completion_cannot_release_a_slot() {
    let root = TaskRoot::new();
    let daemon = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let claim = request("attempt-mutated", "acquire-mutated", 64);
    let grant = daemon.client.acquire(claim.clone()).expect("acquire");
    let token = grant.token.as_ref().expect("lease token");
    let mut evidence = complete_evidence(
        &claim.envelope.binding,
        &grant.lease_id,
        &grant.slot_id,
        grant.slot_path.as_ref().expect("slot path"),
        token,
    );
    if let ContainmentEvidence::CooperativeWorkflow(completion) = &mut evidence.containment {
        completion.repository_head = "1".repeat(40);
    }
    let receipt = daemon
        .client
        .finalize(&grant.lease_id, Some(token), "finalize-mutated", evidence)
        .expect("record an unverifiable completion conservatively");
    assert_eq!(receipt.state, SlotState::Uncertain);
    assert_eq!(
        daemon
            .client
            .evict("evict-mutated", 1)
            .expect("eviction refusal"),
        0
    );
}

#[test]
fn finalize_is_idempotent_and_only_idle_owned_slots_are_evicted() {
    let root = TaskRoot::new();
    let daemon = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let claim = request("attempt-finalize", "acquire-finalize", 64);
    let grant = daemon.client.acquire(claim.clone()).expect("acquire");
    let path = grant.slot_path.as_ref().expect("active slot path");
    fs::write(path.join("fixture-output.bin"), b"abc").expect("write task-owned slot fixture");
    let sentinel = root.path().join("slots/sentinel.bin");
    fs::write(&sentinel, b"outside the slot").expect("write task-owned sentinel");
    symlink("../sentinel.bin", path.join("sentinel-link")).expect("link task-owned sentinel");
    assert_eq!(
        daemon
            .client
            .evict("evict-active", 1)
            .expect("active eviction refusal"),
        0
    );

    let evidence = complete_evidence(
        &claim.envelope.binding,
        &grant.lease_id,
        &grant.slot_id,
        path,
        grant.token.as_ref().expect("lease token"),
    );
    let first = daemon
        .client
        .finalize(
            &grant.lease_id,
            Some(grant.token.as_ref().expect("lease token")),
            "finalize-once",
            evidence.clone(),
        )
        .expect("finalize");
    let retry = daemon
        .client
        .finalize(
            &grant.lease_id,
            Some(grant.token.as_ref().expect("lease token")),
            "finalize-once",
            evidence.clone(),
        )
        .expect("idempotent finalize retry");
    assert_eq!(first, retry);
    assert_eq!(first.state, SlotState::Idle);
    let ledger = fs::read(root.path().join("ledger.bin")).expect("read task ledger");
    for secret in [
        "consumer-proof",
        "terminal-proof",
        "capture-proof",
        "guardian-proof",
    ] {
        assert!(!ledger
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()));
    }
    let measured = daemon.client.inventory("idle").expect("inventory").slots[0]
        .measured_bytes
        .expect("measured idle slot");
    assert!(measured > 3);

    let changed = SettlementEvidence {
        terminal: Some(TerminalRecord {
            run_id: claim.envelope.binding.run_id.clone(),
            outcome: TerminalOutcome::Passed,
            credential: Credential::new("terminal-proof"),
        }),
        ..evidence
    };
    assert_eq!(
        daemon.client.finalize(
            &grant.lease_id,
            Some(grant.token.as_ref().expect("lease token")),
            "finalize-once",
            changed,
        ),
        Err(StorageError::Conflict)
    );

    assert_eq!(
        daemon
            .client
            .evict("evict-idle", 1)
            .expect("evict idle slot"),
        measured
    );
    assert_eq!(
        fs::read(&sentinel).expect("outside sentinel remains"),
        b"outside the slot"
    );
    assert!(daemon
        .client
        .inventory("after-evict")
        .expect("inventory")
        .slots
        .is_empty());
}

#[test]
fn unknown_free_space_fails_closed_before_a_slot_is_created() {
    let root = TaskRoot::new();
    let daemon = RunningDaemon::start(root.path(), budget(128), None);
    assert_eq!(
        daemon
            .client
            .acquire(request("attempt-no-free", "acquire-no-free", 1)),
        Err(StorageError::Refused(RefusalReason::FreeSpaceUnavailable))
    );
    assert!(daemon
        .client
        .inventory("unknown-free")
        .expect("inventory")
        .slots
        .is_empty());
}

#[test]
fn daemon_refuses_symlinked_roots_and_preserves_unknown_root_files() {
    let symlink_root = TaskRoot::new();
    let alias = symlink_root.path().join("root-alias");
    let canonical_root = symlink_root.path().to_path_buf();
    symlink(&canonical_root, &alias).expect("create task-owned root alias");
    let aliased = Daemon::bind(
        DaemonConfig {
            root: alias,
            budget: budget(128),
            backing: None,
        },
        Arc::new(TestAuthority),
        Arc::new(FixedFreeSpace(Some(1024))),
    );
    assert!(matches!(aliased, Err(StorageError::InvalidInput(_))));

    let unknown_root = TaskRoot::new();
    let unrelated = unknown_root.path().join("unrelated.bin");
    fs::write(&unrelated, b"preserve this task-owned sentinel").expect("create unknown root entry");
    let result = Daemon::bind(
        DaemonConfig {
            root: unknown_root.path().to_path_buf(),
            budget: budget(128),
            backing: None,
        },
        Arc::new(TestAuthority),
        Arc::new(FixedFreeSpace(Some(1024))),
    );
    assert!(matches!(
        result,
        Err(StorageError::Refused(RefusalReason::UnknownAccounting))
    ));
    assert_eq!(
        fs::read(unrelated).expect("unknown entry was not deleted"),
        b"preserve this task-owned sentinel"
    );
}

#[test]
fn no_spawn_settlement_is_token_bound_and_cannot_reconcile_uncertain_ownership() {
    let root = TaskRoot::new();
    let daemon = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let request = request("no-spawn", "no-spawn-acquire", 64);
    let acquired = daemon.client.acquire(request.clone()).unwrap();
    let token = acquired.token.as_ref().unwrap();
    let proof = NoSpawnEvidence::new(&acquired, &request.envelope.binding, token).unwrap();
    let evidence = SettlementEvidence {
        binding: request.envelope.binding.clone(),
        consumer_credential: Credential::new("consumer-proof"),
        terminal: None,
        capture: None,
        containment: ContainmentEvidence::NotSpawned(proof.clone()),
    };
    let idle = daemon
        .client
        .finalize(&acquired.lease_id, Some(token), "not-started", evidence)
        .unwrap();
    assert_eq!(idle.state, SlotState::Idle);
    let mut next = request.clone();
    next.acquire_id = "second-acquire".into();
    next.envelope.binding.run_id = "second-run".into();
    next.envelope.binding.attempt_id = "second-attempt".into();
    let second = daemon.client.acquire(next.clone()).unwrap();
    let forged = SettlementEvidence {
        binding: next.envelope.binding.clone(),
        consumer_credential: Credential::new("consumer-proof"),
        terminal: None,
        capture: None,
        containment: ContainmentEvidence::NotSpawned(proof),
    };
    let uncertain = daemon
        .client
        .finalize(
            &second.lease_id,
            second.token.as_ref(),
            "wrong-lease-proof",
            forged,
        )
        .unwrap();
    assert_eq!(uncertain.state, SlotState::Uncertain);
    let new_proof = NoSpawnEvidence::new(
        &second,
        &next.envelope.binding,
        second.token.as_ref().unwrap(),
    )
    .unwrap();
    let evidence = SettlementEvidence {
        binding: next.envelope.binding,
        consumer_credential: Credential::new("consumer-proof"),
        terminal: None,
        capture: None,
        containment: ContainmentEvidence::NotSpawned(new_proof),
    };
    assert_eq!(
        daemon
            .client
            .reconcile(
                &second.lease_id,
                second.token.as_ref(),
                "no-spawn-after-unknown",
                evidence
            )
            .unwrap()
            .state,
        SlotState::Uncertain
    );
}

#[test]
fn same_root_acquire_retains_one_partition_and_never_evicts_without_recovery_policy() {
    let root = TaskRoot::new();
    let daemon = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let mut first = request("first", "first-acquire", 64);
    first.envelope.partition.path_identity = "same-root-fixture-v1:root-r".into();
    let acquired = daemon.client.acquire(first.clone()).unwrap();
    let token = acquired.token.as_ref().unwrap();
    fs::write(
        acquired
            .slot_path
            .as_ref()
            .unwrap()
            .join("invalid-map-bytes"),
        b"owned bytes",
    )
    .unwrap();
    let proof = NoSpawnEvidence::new(&acquired, &first.envelope.binding, token).unwrap();
    daemon
        .client
        .finalize(
            &acquired.lease_id,
            Some(token),
            "first-idle",
            SettlementEvidence {
                binding: first.envelope.binding.clone(),
                consumer_credential: Credential::new("consumer-proof"),
                terminal: None,
                capture: None,
                containment: ContainmentEvidence::NotSpawned(proof),
            },
        )
        .unwrap();
    let mut next = request("next", "next-acquire", 64);
    next.envelope.partition = first.envelope.partition;
    next.envelope.partition.config_digest = "different-config".into();
    assert_eq!(
        daemon.client.acquire(next),
        Err(StorageError::Refused(RefusalReason::BudgetExceeded))
    );
    let before = daemon.client.inventory("before-no-eviction").unwrap();
    assert_eq!(daemon.client.evict("no-recovery-grant", 128), Ok(0));
    assert_eq!(
        daemon.client.inventory("after-no-eviction").unwrap().slots,
        before.slots
    );
    assert_eq!(
        fs::read(acquired.slot_path.unwrap().join("invalid-map-bytes")).unwrap(),
        b"owned bytes"
    );
}

#[test]
fn ordinary_cargo_no_spawn_restores_empty_then_reusable_state_on_one_generation() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let daemon = RunningDaemon::start(storage.path(), budget(256), Some(1024));
    let first_request = ordinary_request(
        source.path(),
        "ordinary-first",
        "ordinary-acquire-first",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let first = daemon
        .client
        .acquire_ordinary_cargo(first_request.clone())
        .expect("authorized ordinary acquire");
    assert!(!first.lease.reused);
    assert_eq!(first.reuse_state, CargoReuseState::Pending);
    let token = first.lease.token.as_ref().expect("active token");
    let no_spawn =
        NoSpawnEvidence::new(&first.lease, &first_request.acquire.envelope.binding, token)
            .expect("no-spawn evidence");
    let empty_restore = daemon
        .client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: first_request.clone(),
            lease_id: first.lease.lease_id.clone(),
            token: Some(token.clone()),
            finalize_id: "ordinary-no-spawn-first".to_owned(),
            evidence: SettlementEvidence {
                binding: first_request.acquire.envelope.binding.clone(),
                consumer_credential: Credential::new("consumer-proof"),
                terminal: None,
                capture: None,
                containment: ContainmentEvidence::NotSpawned(no_spawn),
            },
        })
        .expect("restore empty state");
    assert_eq!(empty_restore.reuse_state, CargoReuseState::Empty);
    assert_eq!(empty_restore.finalization.state, SlotState::Idle);

    let second_request = ordinary_request(
        source.path(),
        "ordinary-second",
        "ordinary-acquire-second",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let second = daemon
        .client
        .acquire_ordinary_cargo(second_request.clone())
        .expect("cold admit after no-spawn");
    assert_eq!(second.lease.slot_id, first.lease.slot_id);
    assert_eq!(second.lease.generation, first.lease.generation);
    assert!(!second.lease.reused);

    let token = second.lease.token.as_ref().expect("active token");
    let passed = passing_evidence(
        &second_request.acquire.envelope.binding,
        &second.lease.lease_id,
        &second.lease.slot_id,
        second.lease.slot_path.as_ref().expect("slot path"),
        token,
    );
    let reusable = daemon
        .client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: second_request.clone(),
            lease_id: second.lease.lease_id.clone(),
            token: Some(token.clone()),
            finalize_id: "ordinary-passed".to_owned(),
            evidence: passed,
        })
        .expect("publish reusable state after complete capture");
    assert_eq!(reusable.reuse_state, CargoReuseState::Reusable);

    let compatibility_changes: [fn(&mut OrdinaryCargoRequest); 5] = [
        |request| request.acquire.envelope.partition.config_digest = "sha256:other-config".into(),
        |request| {
            request.acquire.envelope.partition.features = vec!["default".into(), "extra".into()]
        },
        |request| request.acquire.envelope.partition.cargo_provenance = "cargo other".into(),
        |request| request.acquire.envelope.partition.environment_digest = "sha256:other-env".into(),
        |request| request.root.allocation_epoch = "allocation-2".into(),
    ];
    for (index, change) in compatibility_changes.into_iter().enumerate() {
        let mut changed = ordinary_request(
            source.path(),
            &format!("ordinary-incompatible-{index}"),
            &format!("ordinary-acquire-incompatible-{index}"),
            64,
            OrdinaryCargoPartition::OrdinaryTest,
        );
        change(&mut changed);
        assert_eq!(
            daemon.client.acquire_ordinary_cargo(changed),
            Err(StorageError::Refused(
                RefusalReason::OrdinaryCargoIncompatible
            ))
        );
    }

    let mut third_request = ordinary_request(
        source.path(),
        "ordinary-third",
        "ordinary-acquire-third",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    third_request.acquire.envelope.binding.commit = "fedcba9876543210".to_owned();
    third_request.acquire.envelope.binding.source_tree_digest =
        "sha256:changed-source-tree".to_owned();
    let third = daemon
        .client
        .acquire_ordinary_cargo(third_request.clone())
        .expect("warm admit reusable generation");
    assert_eq!(third.lease.slot_id, first.lease.slot_id);
    assert!(third.lease.reused);

    let token = third.lease.token.as_ref().expect("active token");
    let no_spawn =
        NoSpawnEvidence::new(&third.lease, &third_request.acquire.envelope.binding, token)
            .expect("no-spawn evidence");
    let reusable_restore = daemon
        .client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: third_request.clone(),
            lease_id: third.lease.lease_id.clone(),
            token: Some(token.clone()),
            finalize_id: "ordinary-no-spawn-reusable".to_owned(),
            evidence: SettlementEvidence {
                binding: third_request.acquire.envelope.binding.clone(),
                consumer_credential: Credential::new("consumer-proof"),
                terminal: None,
                capture: None,
                containment: ContainmentEvidence::NotSpawned(no_spawn),
            },
        })
        .expect("restore reusable state");
    assert_eq!(reusable_restore.reuse_state, CargoReuseState::Reusable);
    let fourth_request = ordinary_request(
        source.path(),
        "ordinary-fourth",
        "ordinary-acquire-fourth",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let fourth = daemon
        .client
        .acquire_ordinary_cargo(fourth_request)
        .expect("warm admit after reusable no-spawn");
    assert_eq!(fourth.lease.slot_id, first.lease.slot_id);
    assert!(fourth.lease.reused);
}

#[test]
fn ordinary_cargo_failure_stays_cold_and_capacity_never_evicts_or_grows() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let daemon = RunningDaemon::start(storage.path(), budget(256), Some(1024));
    let first_request = ordinary_request(
        source.path(),
        "ordinary-failure",
        "ordinary-acquire-failure",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let first = daemon
        .client
        .acquire_ordinary_cargo(first_request.clone())
        .expect("cold acquire");
    let token = first.lease.token.as_ref().expect("lease token");
    let failed = complete_evidence(
        &first_request.acquire.envelope.binding,
        &first.lease.lease_id,
        &first.lease.slot_id,
        first.lease.slot_path.as_ref().expect("slot path"),
        token,
    );
    let receipt = daemon
        .client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: first_request.clone(),
            lease_id: first.lease.lease_id.clone(),
            token: Some(token.clone()),
            finalize_id: "ordinary-failed-finalize".to_owned(),
            evidence: failed,
        })
        .expect("settled failure becomes cold-only");
    assert_eq!(receipt.reuse_state, CargoReuseState::ColdOnly);

    let next = ordinary_request(
        source.path(),
        "ordinary-after-failure",
        "ordinary-acquire-after-failure",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    assert_eq!(
        daemon.client.acquire_ordinary_cargo(next),
        Err(StorageError::Refused(RefusalReason::OrdinaryCargoColdOnly))
    );
    assert_eq!(daemon.client.evict("ordinary-evict", 256), Ok(0));

    let full_debug = ordinary_request(
        source.path(),
        "ordinary-full-debug",
        "ordinary-acquire-full-debug",
        64,
        OrdinaryCargoPartition::FullDebug,
    );
    let second = daemon
        .client
        .acquire_ordinary_cargo(full_debug)
        .expect("second and final allowed partition");
    assert_ne!(second.lease.slot_id, first.lease.slot_id);
    let unsupported = ordinary_request(
        source.path(),
        "ordinary-third-partition",
        "ordinary-acquire-third-partition",
        1,
        OrdinaryCargoPartition::Other("full-debug-plus-symbols".to_owned()),
    );
    assert_eq!(
        daemon.client.acquire_ordinary_cargo(unsupported),
        Err(StorageError::Refused(
            RefusalReason::OrdinaryCargoPartitionLimit
        ))
    );
    let inventory = daemon.client.inventory("ordinary-protected").unwrap();
    assert_eq!(inventory.slots.len(), 2);
    assert!(inventory
        .slots
        .iter()
        .all(|slot| slot.cargo_reuse_state.is_some()));
    assert!(first.lease.slot_path.unwrap().is_dir());
}

#[test]
fn concurrent_ordinary_cargo_claims_commit_one_pending_barrier() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let daemon = RunningDaemon::start(storage.path(), budget(256), Some(1024));
    let gate = Arc::new(Barrier::new(3));
    let requests = [
        ordinary_request(
            source.path(),
            "ordinary-race-a",
            "ordinary-acquire-race-a",
            64,
            OrdinaryCargoPartition::OrdinaryTest,
        ),
        ordinary_request(
            source.path(),
            "ordinary-race-b",
            "ordinary-acquire-race-b",
            64,
            OrdinaryCargoPartition::OrdinaryTest,
        ),
    ];
    let threads: Vec<_> = requests
        .into_iter()
        .map(|request| {
            let client = daemon.client.clone();
            let gate = Arc::clone(&gate);
            thread::spawn(move || {
                gate.wait();
                client.acquire_ordinary_cargo(request)
            })
        })
        .collect();
    gate.wait();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().expect("ordinary acquire thread joined"))
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| {
                **result == Err(StorageError::Refused(RefusalReason::OrdinaryCargoPending))
            })
            .count(),
        1
    );
    let inventory = daemon.client.inventory("ordinary-race").unwrap();
    assert_eq!(inventory.slots.len(), 1);
    assert_eq!(
        inventory.slots[0].cargo_reuse_state,
        Some(CargoReuseState::Pending)
    );
    assert_eq!(inventory.slots[0].state, SlotState::Leased);
}

#[test]
fn ordinary_cargo_authentication_exact_lookups_and_generic_bypass_are_closed() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let daemon = RunningDaemon::start(storage.path(), budget(256), Some(1024));
    let request = ordinary_request(
        source.path(),
        "ordinary-lookup",
        "ordinary-acquire-lookup",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let mut bad_credential = request.clone();
    bad_credential.acceptance.credential = Credential::new("wrong-ordinary-proof");
    assert_eq!(
        daemon.client.acquire_ordinary_cargo(bad_credential),
        Err(StorageError::Unauthorized)
    );
    let mut unknown_inputs = request.clone();
    unknown_inputs.acceptance.input_boundary_digest = format!("sha256:{}", "b".repeat(64));
    assert_eq!(
        daemon.client.acquire_ordinary_cargo(unknown_inputs),
        Err(StorageError::Unauthorized)
    );

    let acquired = daemon
        .client
        .acquire_ordinary_cargo(request.clone())
        .expect("accepted acquire");
    let ledger_with_lease = fs::read(storage.path().join("ledger.bin")).unwrap();
    for secret in ["ordinary-proof", "admission-proof"] {
        assert!(!ledger_with_lease
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()));
    }
    let before_lookup = fs::read(storage.path().join("ledger.bin")).unwrap();
    let identity = daemon
        .client
        .lookup_ordinary_cargo(request.clone())
        .expect("read-only exact lookup");
    let LookupResult::Present(identity) = identity else {
        panic!("active lease should be present");
    };
    assert_eq!(identity.lease.state, SlotState::Leased);
    assert_eq!(identity.reuse_state, CargoReuseState::Pending);
    assert_eq!(
        fs::read(storage.path().join("ledger.bin")).unwrap(),
        before_lookup
    );
    let mut changed_root = request.clone();
    changed_root.root.allocation_epoch = "allocation-2".to_owned();
    assert_eq!(
        daemon.client.lookup_ordinary_cargo(changed_root.clone()),
        Err(StorageError::Conflict)
    );

    let token = acquired.lease.token.as_ref().expect("lease token");
    let passed = passing_evidence(
        &request.acquire.envelope.binding,
        &acquired.lease.lease_id,
        &acquired.lease.slot_id,
        acquired.lease.slot_path.as_ref().unwrap(),
        token,
    );
    assert_eq!(
        daemon.client.finalize(
            &acquired.lease.lease_id,
            Some(token),
            "generic-ordinary-finalize-bypass",
            passed.clone(),
        ),
        Err(StorageError::Conflict)
    );
    let finalize_request = OrdinaryCargoFinalizeRequest {
        request: request.clone(),
        lease_id: acquired.lease.lease_id.clone(),
        token: Some(token.clone()),
        finalize_id: "ordinary-finalize-lookup".to_owned(),
        evidence: passed,
    };
    assert_eq!(
        daemon
            .client
            .lookup_finalize_ordinary_cargo(finalize_request.clone())
            .unwrap(),
        LookupResult::Absent
    );
    let finalized = daemon
        .client
        .finalize_ordinary_cargo(finalize_request.clone())
        .expect("ordinary finalize");
    assert_eq!(finalized.reuse_state, CargoReuseState::Reusable);
    let mut tokenless = finalize_request.clone();
    tokenless.token = None;
    assert_eq!(
        daemon
            .client
            .lookup_finalize_ordinary_cargo(tokenless)
            .unwrap(),
        LookupResult::Present(finalized)
    );
    let mut changed_policy = finalize_request.clone();
    changed_policy.request.root.allocation_epoch = "allocation-2".to_owned();
    assert_eq!(
        daemon.client.lookup_finalize_ordinary_cargo(changed_policy),
        Err(StorageError::Conflict)
    );
    assert_eq!(
        daemon.client.lookup_finalize(
            &acquired.lease.lease_id,
            "ordinary-finalize-lookup",
            finalize_request.evidence.clone(),
        ),
        Err(StorageError::Conflict)
    );
    let replay = daemon
        .client
        .acquire_ordinary_cargo(request.clone())
        .expect("finalized exact acquire replay");
    assert_eq!(replay.lease.state, SlotState::Idle);
    assert!(replay.lease.slot_path.is_none());
    assert!(replay.lease.token.is_none());

    let mut generic_collision = request.acquire.clone();
    generic_collision.envelope.binding.attempt_id = "ordinary-generic-collision".to_owned();
    generic_collision.envelope.binding.run_id = "run-ordinary-generic-collision".to_owned();
    assert_eq!(
        daemon.client.acquire(generic_collision),
        Err(StorageError::Conflict)
    );
}

#[test]
fn ordinary_cargo_lost_acquire_and_finalize_replies_reconcile_exact_bodies() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let daemon = RunningDaemon::start(storage.path(), budget(256), Some(1024));
    let request = ordinary_request(
        source.path(),
        "ordinary-lost-reply",
        "ordinary-acquire-lost-reply",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    super::transport::send_ordinary_acquire_and_forget(
        daemon.socket_path.as_path(),
        request.clone(),
    )
    .expect("send acquire and discard response");
    let lookup = daemon
        .client
        .lookup_ordinary_cargo(request.clone())
        .unwrap();
    assert!(
        matches!(
            lookup,
            LookupResult::Present(OrdinaryCargoLeaseIdentity {
                reuse_state: CargoReuseState::Pending,
                lease: LeaseIdentity {
                    state: SlotState::Leased,
                    ..
                },
                ..
            })
        ),
        "unexpected ordinary acquire lookup result: {lookup:?}"
    );
    let acquired = daemon
        .client
        .acquire_ordinary_cargo(request.clone())
        .expect("exact acquire retry recovers active token");
    let token = acquired.lease.token.as_ref().expect("recovered token");
    let evidence = passing_evidence(
        &request.acquire.envelope.binding,
        &acquired.lease.lease_id,
        &acquired.lease.slot_id,
        acquired.lease.slot_path.as_ref().expect("recovered path"),
        token,
    );
    let finalize_request = OrdinaryCargoFinalizeRequest {
        request: request.clone(),
        lease_id: acquired.lease.lease_id.clone(),
        token: Some(token.clone()),
        finalize_id: "ordinary-finalize-lost-reply".to_owned(),
        evidence,
    };
    super::transport::send_ordinary_finalize_and_forget(
        daemon.socket_path.as_path(),
        finalize_request.clone(),
    )
    .expect("send finalize and discard response");
    let mut tokenless = finalize_request;
    tokenless.token = None;
    let lookup = daemon
        .client
        .lookup_finalize_ordinary_cargo(tokenless)
        .expect("read-only exact finalize lookup");
    let LookupResult::Present(receipt) = lookup else {
        panic!("durable finalize should be present");
    };
    assert_eq!(receipt.reuse_state, CargoReuseState::Reusable);
    let replay = daemon.client.acquire_ordinary_cargo(request).unwrap();
    assert!(replay.lease.token.is_none());
    assert!(replay.lease.slot_path.is_none());
}

#[test]
fn ordinary_cargo_bogus_capture_or_terminal_cannot_release_pending_state() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let daemon = RunningDaemon::start(storage.path(), budget(256), Some(1024));
    let request = ordinary_request(
        source.path(),
        "ordinary-bogus-capture",
        "ordinary-acquire-bogus-capture",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let acquired = daemon
        .client
        .acquire_ordinary_cargo(request.clone())
        .unwrap();
    let token = acquired.lease.token.as_ref().unwrap();
    let mut bad_capture = complete_evidence(
        &request.acquire.envelope.binding,
        &acquired.lease.lease_id,
        &acquired.lease.slot_id,
        acquired.lease.slot_path.as_ref().unwrap(),
        token,
    );
    bad_capture.capture.as_mut().unwrap().digest = "sha256:bad".to_owned();
    let uncertain = daemon
        .client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: request.clone(),
            lease_id: acquired.lease.lease_id.clone(),
            token: Some(token.clone()),
            finalize_id: "ordinary-bad-capture-finalize".to_owned(),
            evidence: bad_capture,
        })
        .unwrap();
    assert_eq!(uncertain.finalization.state, SlotState::Uncertain);
    assert_eq!(uncertain.reuse_state, CargoReuseState::Pending);

    let no_spawn_storage = TaskRoot::new();
    let no_spawn_daemon = RunningDaemon::start(no_spawn_storage.path(), budget(256), Some(1024));
    let no_spawn_request = ordinary_request(
        source.path(),
        "ordinary-bogus-terminal",
        "ordinary-acquire-bogus-terminal",
        64,
        OrdinaryCargoPartition::FullDebug,
    );
    let no_spawn = no_spawn_daemon
        .client
        .acquire_ordinary_cargo(no_spawn_request.clone())
        .unwrap();
    let token = no_spawn.lease.token.as_ref().unwrap();
    let proof = NoSpawnEvidence::new(
        &no_spawn.lease,
        &no_spawn_request.acquire.envelope.binding,
        token,
    )
    .unwrap();
    let bogus = no_spawn_daemon
        .client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: no_spawn_request.clone(),
            lease_id: no_spawn.lease.lease_id.clone(),
            token: Some(token.clone()),
            finalize_id: "ordinary-bogus-terminal-finalize".to_owned(),
            evidence: SettlementEvidence {
                binding: no_spawn_request.acquire.envelope.binding.clone(),
                consumer_credential: Credential::new("consumer-proof"),
                terminal: Some(TerminalRecord {
                    run_id: no_spawn_request.acquire.envelope.binding.run_id.clone(),
                    outcome: TerminalOutcome::Failed,
                    credential: Credential::new("terminal-proof"),
                }),
                capture: None,
                containment: ContainmentEvidence::NotSpawned(proof),
            },
        })
        .unwrap();
    assert_eq!(bogus.finalization.state, SlotState::Uncertain);
    assert_eq!(bogus.reuse_state, CargoReuseState::Pending);
}

#[test]
fn ordinary_cargo_defaults_to_rejecting_uninstalled_policy() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let daemon = Daemon::bind(
        DaemonConfig {
            root: storage.path().to_path_buf(),
            budget: budget(128),
            backing: None,
        },
        Arc::new(RejectingOrdinaryAuthority),
        Arc::new(FixedFreeSpace(Some(1024))),
    )
    .expect("bind daemon with safe default ordinary authority");
    let client = Client::new(daemon.socket_path().to_path_buf());
    let request = ordinary_request(
        source.path(),
        "ordinary-uninstalled",
        "ordinary-acquire-uninstalled",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let (shutdown, receiver) = mpsc::channel();
    let thread = thread::spawn(move || daemon.serve(receiver));
    assert_eq!(
        client.acquire_ordinary_cargo(request),
        Err(StorageError::Unauthorized)
    );
    let inventory = client.inventory("ordinary-uninstalled").unwrap();
    assert!(inventory.slots.is_empty());
    shutdown.send(()).unwrap();
    assert!(thread.join().unwrap().is_ok());
}

#[test]
fn ordinary_cargo_restart_keeps_pending_generation_uncertain() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let mut daemon = RunningDaemon::start(storage.path(), budget(128), Some(1024));
    let request = ordinary_request(
        source.path(),
        "ordinary-restart",
        "ordinary-acquire-restart",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let acquired = daemon
        .client
        .acquire_ordinary_cargo(request.clone())
        .expect("acquire before restart");
    daemon.stop();
    let restarted = RunningDaemon::start(storage.path(), budget(128), Some(1024));
    let result = restarted
        .client
        .lookup_ordinary_cargo(request.clone())
        .expect("lookup after restart");
    let LookupResult::Uncertain(Some(identity)) = result else {
        panic!("restart must retain uncertain ordinary ownership");
    };
    assert_eq!(identity.lease.state, SlotState::Uncertain);
    assert_eq!(identity.reuse_state, CargoReuseState::Pending);
    let retry = restarted
        .client
        .acquire_ordinary_cargo(request.clone())
        .expect("same request returns uncertain identity");
    assert_eq!(retry.lease.state, SlotState::Uncertain);
    assert!(retry.lease.slot_path.is_none());
    assert!(retry.lease.token.is_none());
    let next = ordinary_request(
        source.path(),
        "ordinary-after-restart",
        "ordinary-acquire-after-restart",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    assert_eq!(
        restarted.client.acquire_ordinary_cargo(next),
        Err(StorageError::Refused(RefusalReason::OrdinaryCargoPending))
    );
    assert_eq!(retry.lease.slot_id, acquired.lease.slot_id);
}

#[test]
fn ordinary_cargo_uses_checked_budget_and_free_space_and_refuses_incompatible_epochs() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let daemon = RunningDaemon::start(storage.path(), budget(64), Some(1024));
    let first_request = ordinary_request(
        source.path(),
        "ordinary-capacity-one",
        "ordinary-acquire-capacity-one",
        40,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let _first = daemon
        .client
        .acquire_ordinary_cargo(first_request.clone())
        .expect("first generation within bound");
    let second_partition = ordinary_request(
        source.path(),
        "ordinary-capacity-two",
        "ordinary-acquire-capacity-two",
        40,
        OrdinaryCargoPartition::FullDebug,
    );
    assert_eq!(
        daemon.client.acquire_ordinary_cargo(second_partition),
        Err(StorageError::Refused(RefusalReason::BudgetExceeded))
    );
    assert_eq!(
        daemon.client.inventory("capacity-one").unwrap().slots.len(),
        1
    );

    let unknown_storage = TaskRoot::new();
    let unknown_space = Daemon::bind(
        DaemonConfig {
            root: unknown_storage.path().to_path_buf(),
            budget: budget(64),
            backing: None,
        },
        Arc::new(TestAuthority),
        Arc::new(FixedFreeSpace(None)),
    )
    .expect("bind unknown-space daemon");
    let unknown_client = Client::new(unknown_space.socket_path().to_path_buf());
    let (shutdown, receiver) = mpsc::channel();
    let thread = thread::spawn(move || unknown_space.serve(receiver));
    let unknown_request = ordinary_request(
        source.path(),
        "ordinary-unknown-space",
        "ordinary-acquire-unknown-space",
        1,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    assert_eq!(
        unknown_client.acquire_ordinary_cargo(unknown_request),
        Err(StorageError::Refused(RefusalReason::FreeSpaceUnavailable))
    );
    shutdown.send(()).unwrap();
    assert!(thread.join().unwrap().is_ok());

    let overflow_storage = TaskRoot::new();
    let overflow = Daemon::bind(
        DaemonConfig {
            root: overflow_storage.path().to_path_buf(),
            budget: Budget {
                cap_bytes: 64,
                free_reserve_bytes: u64::MAX,
            },
            backing: None,
        },
        Arc::new(TestAuthority),
        Arc::new(FixedFreeSpace(Some(u64::MAX))),
    )
    .expect("bind overflow fixture daemon");
    let overflow_client = Client::new(overflow.socket_path().to_path_buf());
    let (shutdown, receiver) = mpsc::channel();
    let thread = thread::spawn(move || overflow.serve(receiver));
    let overflow_request = ordinary_request(
        source.path(),
        "ordinary-overflow",
        "ordinary-acquire-overflow",
        1,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    assert_eq!(
        overflow_client.acquire_ordinary_cargo(overflow_request),
        Err(StorageError::Refused(RefusalReason::FreeSpaceReserve))
    );
    shutdown.send(()).unwrap();
    assert!(thread.join().unwrap().is_ok());

    let epoch_storage = TaskRoot::new();
    let epoch_daemon = RunningDaemon::start(epoch_storage.path(), budget(128), Some(1024));
    let initial = ordinary_request(
        source.path(),
        "ordinary-epoch-one",
        "ordinary-acquire-epoch-one",
        1,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    epoch_daemon
        .client
        .acquire_ordinary_cargo(initial)
        .expect("admit initial allocation");
    let mut changed = ordinary_request(
        source.path(),
        "ordinary-epoch-two",
        "ordinary-acquire-epoch-two",
        1,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    changed.root.allocation_epoch = "allocation-2".to_owned();
    assert_eq!(
        epoch_daemon.client.acquire_ordinary_cargo(changed),
        Err(StorageError::Refused(
            RefusalReason::OrdinaryCargoIncompatible
        ))
    );
}

fn synthetic_backing(payload_root: &Path, identity: &str) -> BoundedBacking {
    BoundedBacking::verified(BoundedBackendDescriptor {
        backend: BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1.to_owned(),
        payload_root: payload_root.to_path_buf(),
        identity: identity.to_owned(),
        verified_capacity_bytes: 1024 * 1024,
    })
    .expect("synthetic verified backing")
}

fn start_bounded(
    root: &Path,
    payload_root: &Path,
    cap_bytes: u64,
    identity: &str,
) -> RunningDaemon {
    RunningDaemon::bind(
        root,
        budget(cap_bytes),
        Some(64 * 1024 * 1024),
        Arc::new(TestAuthority),
        Some(synthetic_backing(payload_root, identity)),
    )
}

fn request_at_path(
    attempt: &str,
    acquire_id: &str,
    required_bytes: u64,
    path: &str,
) -> AcquireRequest {
    let mut request = request(attempt, acquire_id, required_bytes);
    request.envelope.partition.path_identity = path.to_owned();
    request
}

#[test]
fn bounded_backing_binds_startup_ledger_and_refuses_mismatched_reopen() {
    let root = TaskRoot::new();
    let payload = TaskRoot::new();
    let identity = "a".repeat(16);
    let mut daemon = start_bounded(root.path(), payload.path(), 128, &identity);
    let grant = daemon
        .client
        .acquire(request("bounded-bind", "bounded-bind-acquire", 64))
        .expect("bounded acquire");
    let slot_path = grant.slot_path.expect("bounded slot path");
    assert!(slot_path.starts_with(payload.path().join("slots")));
    // Control state stays on the daemon root; the payload holds only slots.
    assert!(!root.path().join("slots").exists());
    let evidence = slot_path.join("bounded-evidence.bin");
    fs::write(&evidence, b"bounded task-owned fixture bytes")
        .expect("write bounded fixture evidence");
    let token = grant.token.expect("lease token");
    let receipt = daemon
        .client
        .finalize(
            &grant.lease_id,
            Some(&token),
            "bounded-bind-finalize",
            passing_evidence(
                &request("bounded-bind", "bounded-bind-acquire", 64)
                    .envelope
                    .binding,
                &grant.lease_id,
                &grant.slot_id,
                &slot_path,
                &token,
            ),
        )
        .expect("bounded finalize");
    assert_eq!(receipt.state, SlotState::Idle);
    let inventory = daemon.client.inventory("bounded-bind").unwrap();
    assert_eq!(
        inventory.accounted_bytes,
        inventory
            .slots
            .iter()
            .map(|slot| slot.measured_bytes.unwrap_or(0))
            .reduce(u64::saturating_add)
    );
    daemon.stop();

    // A different recorded identity, a directory reopen, or a missing
    // capability each refuse instead of reinterpreting the bounded ledger.
    let budget = budget(128);
    for mismatched in [
        Some(synthetic_backing(payload.path(), &"b".repeat(16))),
        None,
    ] {
        let result = Daemon::bind(
            DaemonConfig {
                root: root.path().to_path_buf(),
                budget,
                backing: mismatched,
            },
            Arc::new(TestAuthority),
            Arc::new(FixedFreeSpace(Some(1024))),
        );
        assert!(matches!(
            result,
            Err(StorageError::Refused(RefusalReason::UnknownAccounting))
        ));
    }
    // Matching reopen binds and preserves the settled generation.
    let reopened = start_bounded(root.path(), payload.path(), 128, &identity);
    let after = reopened.client.inventory("bounded-reopen").unwrap();
    assert_eq!(after.accounted_bytes, inventory.accounted_bytes);
    assert_eq!(after.slots.len(), 1);
    assert_eq!(after.slots[0].state, SlotState::Idle);
    assert_eq!(
        fs::read(&evidence).expect("bounded evidence remains"),
        b"bounded task-owned fixture bytes"
    );
}

#[test]
fn bounded_payload_layout_refuses_unknown_entries_symlinks_and_shared_roots() {
    let root = TaskRoot::new();
    // A foreign entry in the payload root refuses before any effect.
    let payload = TaskRoot::new();
    fs::write(payload.path().join("foreign.bin"), b"not owned")
        .expect("write bounded foreign fixture entry");
    let result = Daemon::bind(
        DaemonConfig {
            root: root.path().to_path_buf(),
            budget: budget(128),
            backing: Some(synthetic_backing(payload.path(), &"a".repeat(16))),
        },
        Arc::new(TestAuthority),
        Arc::new(FixedFreeSpace(Some(1024))),
    );
    assert!(matches!(
        result,
        Err(StorageError::Refused(RefusalReason::UnknownAccounting))
    ));

    // A symlinked payload root never binds, even with matching ledger intent:
    // the verified capability refuses it before any daemon state exists.
    let link_root = TaskRoot::new();
    let target = TaskRoot::new();
    let alias = link_root.path().join("payload-alias");
    symlink(target.path(), &alias).expect("create bounded symlink fixture");
    assert!(matches!(
        BoundedBacking::verified(BoundedBackendDescriptor {
            backend: BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1.to_owned(),
            payload_root: alias,
            identity: "a".repeat(16),
            verified_capacity_bytes: 1024 * 1024,
        }),
        Err(StorageError::InvalidInput(_))
    ));

    // Sharing the daemon control root as the payload refuses: the store
    // rejects a bounded control root that gains slot-shaped entries.
    let shared = TaskRoot::new();
    let result = Daemon::bind(
        DaemonConfig {
            root: shared.path().to_path_buf(),
            budget: budget(128),
            backing: Some(synthetic_backing(shared.path(), &"a".repeat(16))),
        },
        Arc::new(TestAuthority),
        Arc::new(FixedFreeSpace(Some(1024))),
    );
    assert!(matches!(
        result,
        Err(StorageError::Refused(RefusalReason::UnknownAccounting))
    ));
}

#[test]
fn bounded_ceiling_stops_new_admission_with_known_accounting() {
    let root = TaskRoot::new();
    let payload = TaskRoot::new();
    let daemon = start_bounded(root.path(), payload.path(), 4096, &"a".repeat(16));
    let first = daemon
        .client
        .acquire(request_at_path(
            "bounded-cap-a",
            "bounded-cap-acquire-a",
            4096,
            "/task/checkouts/a",
        ))
        .expect("first bounded lease fills the aggregate bound");
    // A different partition may not silently expand the aggregate pool.
    let second = daemon.client.acquire(request_at_path(
        "bounded-cap-b",
        "bounded-cap-acquire-b",
        4096,
        "/task/checkouts/b",
    ));
    assert!(matches!(
        second,
        Err(StorageError::Refused(RefusalReason::BudgetExceeded))
    ));
    // Identical retries replay the original lease unchanged.
    let replay = daemon
        .client
        .acquire(request_at_path(
            "bounded-cap-a",
            "bounded-cap-acquire-a",
            4096,
            "/task/checkouts/a",
        ))
        .expect("bounded replay");
    assert_eq!(replay.lease_id, first.lease_id);

    let token = first.token.expect("lease token");
    let slot_path = first.slot_path.clone().expect("slot path");
    fs::write(slot_path.join("unit.bin"), vec![0u8; 100]).expect("bounded fixture write");
    daemon
        .client
        .finalize(
            &first.lease_id,
            Some(&token),
            "bounded-cap-finalize",
            passing_evidence(
                &request_at_path(
                    "bounded-cap-a",
                    "bounded-cap-acquire-a",
                    4096,
                    "/task/checkouts/a",
                )
                .envelope
                .binding,
                &first.lease_id,
                &first.slot_id,
                &slot_path,
                &token,
            ),
        )
        .expect("bounded finalize");
    // A proven-idle settled slot may still be evicted inside the same finite
    // aggregate: eviction is the bounded relief valve, never pool expansion.
    let after_eviction = daemon
        .client
        .acquire(request_at_path(
            "bounded-cap-c",
            "bounded-cap-acquire-c",
            4096,
            "/task/checkouts/c",
        ))
        .expect("eviction frees the settled aggregate space");
    let mid = daemon.client.inventory("bounded-cap-mid").unwrap();
    assert_eq!(mid.slots.len(), 1);
    assert_ne!(mid.slots[0].slot_id, first.slot_id);
    assert_eq!(mid.accounted_bytes, Some(4096));
    // With the only slot leased, nothing evictable remains and the aggregate
    // bound stops new admission with a known refusal.
    let still_full = daemon.client.acquire(request_at_path(
        "bounded-cap-d",
        "bounded-cap-acquire-d",
        4096,
        "/task/checkouts/d",
    ));
    assert!(matches!(
        still_full,
        Err(StorageError::Refused(RefusalReason::BudgetExceeded))
    ));
    let _ = after_eviction;
}

#[test]
fn bounded_full_payload_keeps_metadata_and_ownership_durable() {
    let root = TaskRoot::new();
    let payload = TaskRoot::new();
    let daemon = start_bounded(
        root.path(),
        payload.path(),
        3 * 1024 * 1024,
        &"a".repeat(16),
    );
    let grant = daemon
        .client
        .acquire(request(
            "bounded-full",
            "bounded-full-acquire",
            3 * 1024 * 1024,
        ))
        .expect("bounded lease against the whole payload");
    let slot_path = grant.slot_path.expect("bounded slot path");
    // The cooperative writer fills the payload to its recorded ceiling while
    // the lease stays open.
    fs::write(slot_path.join("payload.bin"), vec![0u8; 2 * 1024 * 1024])
        .expect("write bounded full-payload fixture");
    let inventory = daemon.client.inventory("bounded-full").unwrap();
    assert_eq!(inventory.accounted_bytes, Some(3 * 1024 * 1024));
    // Control metadata remains durable while the payload is full: settlement
    // records its transition on the daemon root, never on the payload.
    let token = grant.token.expect("lease token");
    let receipt = daemon
        .client
        .finalize(
            &grant.lease_id,
            Some(&token),
            "bounded-full-finalize",
            passing_evidence(
                &request("bounded-full", "bounded-full-acquire", 3 * 1024 * 1024)
                    .envelope
                    .binding,
                &grant.lease_id,
                &grant.slot_id,
                &slot_path,
                &token,
            ),
        )
        .expect("bounded finalize over a full payload");
    assert_eq!(receipt.state, SlotState::Idle);
    let settled = daemon.client.inventory("bounded-full-settled").unwrap();
    let measured = settled.slots[0].measured_bytes.expect("measured payload");
    assert_eq!(settled.accounted_bytes, Some(measured));
    assert!(measured >= 2 * 1024 * 1024 && measured <= 3 * 1024 * 1024);
}
