use super::*;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Barrier};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

mod handoff;

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
            // Two synthetic fixture allocations; any other root identity is
            // uninstalled and refused. Retirement inherits this check.
            && matches!(
                request.root.consumer_root_id.as_str(),
                "consumer-root" | "consumer-root-2"
            )
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

    /// Clearly synthetic retirement authority for fixtures: the exact
    /// admitted ordinary request plus independently named retained
    /// source/artifact/debug references. This does not implement a
    /// production Nucleus receipt issuer.
    fn verify_ordinary_cargo_retirement(&self, request: &OrdinaryCargoRetireRequest) -> bool {
        self.verify_ordinary_cargo_admission(&request.request)
            && request.retention.source_evidence_reference == "retained-source-v1"
            && request.retention.artifact_evidence_reference == "retained-artifact-v1"
            && request.retention.debug_evidence_reference == "retained-debug-v1"
            && request.retention.credential.expose() == "retention-proof"
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

fn retention_grant() -> OrdinaryCargoRetention {
    OrdinaryCargoRetention {
        source_evidence_reference: "retained-source-v1".to_owned(),
        artifact_evidence_reference: "retained-artifact-v1".to_owned(),
        debug_evidence_reference: "retained-debug-v1".to_owned(),
        credential: Credential::new("retention-proof"),
    }
}

fn retire_request(
    base: &OrdinaryCargoRequest,
    slot_id: &str,
    storage_instance_id: &str,
    generation: &str,
    retire_id: &str,
) -> OrdinaryCargoRetireRequest {
    OrdinaryCargoRetireRequest {
        request: base.clone(),
        slot_id: slot_id.to_owned(),
        storage_instance_id: storage_instance_id.to_owned(),
        generation: generation.to_owned(),
        retire_id: retire_id.to_owned(),
        retention: retention_grant(),
    }
}

fn settle_reusable(
    client: &Client,
    request: &OrdinaryCargoRequest,
    acquired: &OrdinaryCargoAcquireResult,
    finalize_id: &str,
) -> OrdinaryCargoFinalizationReceipt {
    let token = acquired.lease.token.as_ref().expect("active lease token");
    client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: request.clone(),
            lease_id: acquired.lease.lease_id.clone(),
            token: Some(token.clone()),
            finalize_id: finalize_id.to_owned(),
            evidence: passing_evidence(
                &request.acquire.envelope.binding,
                &acquired.lease.lease_id,
                &acquired.lease.slot_id,
                acquired.lease.slot_path.as_ref().expect("slot path"),
                token,
            ),
        })
        .expect("publish reusable generation after complete capture")
}

fn settle_cold_only(
    client: &Client,
    request: &OrdinaryCargoRequest,
    acquired: &OrdinaryCargoAcquireResult,
    finalize_id: &str,
) -> OrdinaryCargoFinalizationReceipt {
    let token = acquired.lease.token.as_ref().expect("active lease token");
    client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: request.clone(),
            lease_id: acquired.lease.lease_id.clone(),
            token: Some(token.clone()),
            finalize_id: finalize_id.to_owned(),
            evidence: complete_evidence(
                &request.acquire.envelope.binding,
                &acquired.lease.lease_id,
                &acquired.lease.slot_id,
                acquired.lease.slot_path.as_ref().expect("slot path"),
                token,
            ),
        })
        .expect("settled failure becomes cold-only")
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

#[test]
fn ordinary_cargo_retire_reusable_idle_replaces_with_fresh_ordinal() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let daemon = RunningDaemon::start(storage.path(), budget(256), Some(1024));
    let first_request = ordinary_request(
        source.path(),
        "ordinary-retire-reusable",
        "ordinary-acquire-retire-reusable",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let acquired = daemon
        .client
        .acquire_ordinary_cargo(first_request.clone())
        .expect("cold admit reusable candidate");
    let slot_path = acquired.lease.slot_path.clone().expect("leased slot path");
    // A small real payload proves deletion removes owned bytes only.
    fs::write(slot_path.join("unit.rlib"), vec![7u8; 4096]).expect("write owned fixture payload");
    let reusable = settle_reusable(
        &daemon.client,
        &first_request,
        &acquired,
        "ordinary-retire-reusable-finalize",
    );
    assert_eq!(reusable.reuse_state, CargoReuseState::Reusable);
    let slot_id = acquired.lease.slot_id.clone();
    let instance = acquired.lease.storage_instance_id.clone();
    let generation = acquired.lease.generation.clone();
    assert_eq!(generation, "1");

    // Without the retention grant the whole retirement refuses and the
    // proven-idle generation stays warm and reusable.
    let mut denied = retire_request(&first_request, &slot_id, &instance, &generation, "retire-1");
    denied.retention.credential = Credential::new("wrong-retention-proof");
    assert_eq!(
        daemon.client.retire_ordinary_cargo(denied),
        Err(StorageError::Unauthorized)
    );
    let mut unknown_coverage =
        retire_request(&first_request, &slot_id, &instance, &generation, "retire-1");
    unknown_coverage.retention.debug_evidence_reference = String::new();
    assert!(daemon
        .client
        .retire_ordinary_cargo(unknown_coverage)
        .is_err());
    let inventory = daemon.client.inventory("retire-denied").unwrap();
    assert_eq!(inventory.slots.len(), 1);
    assert_eq!(
        inventory.slots[0].cargo_reuse_state,
        Some(CargoReuseState::Reusable)
    );
    assert!(slot_path.is_dir());

    // The authenticated retirement publishes the tombstone receipt before
    // the root/partition may admit a replacement.
    let receipt = daemon
        .client
        .retire_ordinary_cargo(retire_request(
            &first_request,
            &slot_id,
            &instance,
            &generation,
            "retire-1",
        ))
        .expect("retire proven-idle reusable generation");
    assert_eq!(receipt.slot_id, slot_id);
    assert_eq!(receipt.storage_instance_id, instance);
    assert_eq!(receipt.generation, generation);
    assert_eq!(receipt.retire_id, "retire-1");
    assert_eq!(receipt.retired_reuse_state, CargoReuseState::Reusable);
    assert_eq!(receipt.root, first_request.root);
    assert_eq!(receipt.partition, OrdinaryCargoPartition::OrdinaryTest);
    assert!(!slot_path.exists());
    let ledger = fs::read(storage.path().join("ledger.bin")).expect("read retired ledger");
    for secret in ["retention-proof", "ordinary-proof", "admission-proof"] {
        assert!(
            !ledger
                .windows(secret.len())
                .any(|window| window == secret.as_bytes()),
            "retirement must not persist {secret}"
        );
    }
    let inventory = daemon.client.inventory("retired").unwrap();
    assert!(inventory.slots.is_empty());

    // Old finalized acquire/finalize/retire replays stay access-free and
    // cannot resurrect bytes.
    let replay = daemon
        .client
        .acquire_ordinary_cargo(first_request.clone())
        .expect("old exact acquire replay stays bound");
    assert_eq!(replay.lease.lease_id, acquired.lease.lease_id);
    assert_eq!(replay.lease.state, SlotState::Idle);
    assert!(replay.lease.slot_path.is_none());
    assert!(replay.lease.token.is_none());
    // The slot directory is gone but the recorded path string is unchanged,
    // so the exact original evidence still replays its finalize receipt.
    let finalized_lookup = daemon
        .client
        .lookup_finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: first_request.clone(),
            lease_id: acquired.lease.lease_id.clone(),
            token: None,
            finalize_id: "ordinary-retire-reusable-finalize".to_owned(),
            evidence: passing_evidence(
                &first_request.acquire.envelope.binding,
                &acquired.lease.lease_id,
                &acquired.lease.slot_id,
                &slot_path,
                acquired.lease.token.as_ref().expect("original token"),
            ),
        })
        .expect("old finalize lookup replays after retirement");
    assert!(
        matches!(finalized_lookup, LookupResult::Present(ref receipt) if receipt.reuse_state == CargoReuseState::Reusable),
        "unexpected finalize replay: {finalized_lookup:?}"
    );
    assert_eq!(
        daemon
            .client
            .lookup_retire_ordinary_cargo(retire_request(
                &first_request,
                &slot_id,
                &instance,
                &generation,
                "retire-1",
            ))
            .expect("retire lookup replays the tombstone receipt"),
        LookupResult::Present(receipt.clone())
    );
    assert_eq!(
        daemon.client.retire_ordinary_cargo(retire_request(
            &first_request,
            &slot_id,
            &instance,
            &generation,
            "retire-1",
        )),
        Ok(receipt.clone())
    );

    // A replacement for the same root and partition admits cold work with a
    // new monotonic ordinal bound to the same storage instance.
    let mut next_request = ordinary_request(
        source.path(),
        "ordinary-retire-replacement",
        "ordinary-acquire-retire-replacement",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    next_request.acquire.envelope.binding.commit = "aaaaaaaa00000000".to_owned();
    let replacement = daemon
        .client
        .acquire_ordinary_cargo(next_request.clone())
        .expect("replacement admits cold work after receipt");
    assert_ne!(replacement.lease.slot_id, slot_id);
    assert_eq!(replacement.lease.generation, "2");
    assert_eq!(replacement.lease.storage_instance_id, instance);
    assert!(!replacement.lease.reused);
    assert_eq!(replacement.reuse_state, CargoReuseState::Pending);

    // The other label's generation is untouched by the retirement.
    let debug_request = ordinary_request(
        source.path(),
        "ordinary-retire-debug",
        "ordinary-acquire-retire-debug",
        64,
        OrdinaryCargoPartition::FullDebug,
    );
    let debug = daemon
        .client
        .acquire_ordinary_cargo(debug_request)
        .expect("other label admits independently");
    assert_eq!(debug.lease.generation, "3");
}

#[test]
fn ordinary_cargo_retire_cold_only_needs_grant_then_admits_replacement() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let daemon = RunningDaemon::start(storage.path(), budget(256), Some(1024));
    let first_request = ordinary_request(
        source.path(),
        "ordinary-retire-cold",
        "ordinary-acquire-retire-cold",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let acquired = daemon
        .client
        .acquire_ordinary_cargo(first_request.clone())
        .expect("cold admit");
    let cold = settle_cold_only(
        &daemon.client,
        &first_request,
        &acquired,
        "ordinary-retire-cold-finalize",
    );
    assert_eq!(cold.reuse_state, CargoReuseState::ColdOnly);
    let slot_id = acquired.lease.slot_id.clone();
    let instance = acquired.lease.storage_instance_id.clone();
    let generation = acquired.lease.generation.clone();

    // Failure alone is not permission to retire: without the retention
    // grant the cold-only generation is held as-is.
    assert_eq!(
        daemon.client.acquire_ordinary_cargo(ordinary_request(
            source.path(),
            "ordinary-retire-cold-retry",
            "ordinary-acquire-retire-cold-retry",
            64,
            OrdinaryCargoPartition::OrdinaryTest,
        )),
        Err(StorageError::Refused(RefusalReason::OrdinaryCargoColdOnly))
    );
    let mut denied = retire_request(&first_request, &slot_id, &instance, &generation, "retire-1");
    denied.retention.artifact_evidence_reference = "unretained".to_owned();
    assert_eq!(
        daemon.client.retire_ordinary_cargo(denied),
        Err(StorageError::Unauthorized)
    );

    let receipt = daemon
        .client
        .retire_ordinary_cargo(retire_request(
            &first_request,
            &slot_id,
            &instance,
            &generation,
            "retire-1",
        ))
        .expect("retire cold-only generation with retention grant");
    assert_eq!(receipt.retired_reuse_state, CargoReuseState::ColdOnly);

    // The retired cold-only pool no longer strands the root: a replacement
    // admits cold work with a fresh ordinal instead of refusing cold-only.
    let next_request = ordinary_request(
        source.path(),
        "ordinary-retire-cold-next",
        "ordinary-acquire-retire-cold-next",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let replacement = daemon
        .client
        .acquire_ordinary_cargo(next_request)
        .expect("replacement after cold-only retirement");
    assert_ne!(replacement.lease.slot_id, slot_id);
    assert_eq!(replacement.lease.generation, "2");
    assert!(!replacement.lease.reused);
}

#[test]
fn ordinary_cargo_retire_refuses_held_generic_and_foreign_scopes() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let other_source = TaskRoot::new();
    let daemon = RunningDaemon::start(storage.path(), budget(512), Some(1024));

    // A generic slot is acquired first: once an uncertain generation
    // exists, global accounting is unknown and every admission refuses.
    let generic = daemon
        .client
        .acquire(request(
            "attempt-generic-retire",
            "acquire-generic-retire",
            64,
        ))
        .expect("generic acquire");

    // A leased (pending) generation is held: no live, uncertain, or pending
    // lease may be retired.
    let pending_request = ordinary_request(
        source.path(),
        "ordinary-retire-pending",
        "ordinary-acquire-retire-pending",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let pending = daemon
        .client
        .acquire_ordinary_cargo(pending_request.clone())
        .expect("pending acquire");
    let pending_retire = retire_request(
        &pending_request,
        &pending.lease.slot_id,
        &pending.lease.storage_instance_id,
        &pending.lease.generation,
        "retire-pending",
    );
    assert_eq!(
        daemon.client.retire_ordinary_cargo(pending_retire),
        Err(StorageError::Refused(RefusalReason::LeaseUnavailable))
    );

    // An unproven empty generation is admitted before anything turns
    // uncertain: unknown accounting would otherwise refuse all admission.
    let empty_request = ordinary_request(
        source.path(),
        "ordinary-retire-empty",
        "ordinary-acquire-retire-empty",
        64,
        OrdinaryCargoPartition::FullDebug,
    );
    let empty = daemon
        .client
        .acquire_ordinary_cargo(empty_request.clone())
        .expect("empty candidate");
    let empty_token = empty.lease.token.as_ref().expect("lease token");
    let no_spawn = NoSpawnEvidence::new(
        &empty.lease,
        &empty_request.acquire.envelope.binding,
        empty_token,
    )
    .expect("no-spawn proof");
    daemon
        .client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: empty_request.clone(),
            lease_id: empty.lease.lease_id.clone(),
            token: Some(empty_token.clone()),
            finalize_id: "ordinary-retire-empty-finalize".to_owned(),
            evidence: SettlementEvidence {
                binding: empty_request.acquire.envelope.binding.clone(),
                consumer_credential: Credential::new("consumer-proof"),
                terminal: None,
                capture: None,
                containment: ContainmentEvidence::NotSpawned(no_spawn),
            },
        })
        .expect("restore empty state");

    // An uncertain generation is held as well.
    let mut uncertain_request = ordinary_request(
        other_source.path(),
        "ordinary-retire-uncertain",
        "ordinary-acquire-retire-uncertain",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    uncertain_request.root.consumer_root_id = "consumer-root-2".to_owned();
    let uncertain = daemon
        .client
        .acquire_ordinary_cargo(uncertain_request.clone())
        .expect("uncertain acquire");
    let uncertain_token = uncertain.lease.token.as_ref().expect("lease token");
    let mut bogus = complete_evidence(
        &uncertain_request.acquire.envelope.binding,
        &uncertain.lease.lease_id,
        &uncertain.lease.slot_id,
        uncertain.lease.slot_path.as_ref().expect("slot path"),
        uncertain_token,
    );
    bogus.capture.as_mut().unwrap().digest = "sha256:bad".to_owned();
    daemon
        .client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: uncertain_request.clone(),
            lease_id: uncertain.lease.lease_id.clone(),
            token: Some(uncertain_token.clone()),
            finalize_id: "ordinary-retire-uncertain-finalize".to_owned(),
            evidence: bogus,
        })
        .expect("bogus capture stays uncertain");
    assert_eq!(
        daemon.client.retire_ordinary_cargo(retire_request(
            &uncertain_request,
            &uncertain.lease.slot_id,
            &uncertain.lease.storage_instance_id,
            &uncertain.lease.generation,
            "retire-uncertain",
        )),
        Err(StorageError::Refused(RefusalReason::LeaseUnavailable))
    );

    // A generic slot is not an ordinary generation: the ordinary retire
    // surface cannot operate on it.
    assert_eq!(
        daemon.client.retire_ordinary_cargo(retire_request(
            &pending_request,
            &generic.slot_id,
            &generic.storage_instance_id,
            &generic.generation,
            "retire-generic",
        )),
        Err(StorageError::Conflict)
    );

    // Well-formed but foreign tuples refuse without effects.
    let instance = pending.lease.storage_instance_id.clone();
    let foreign_instance = retire_request(
        &pending_request,
        &pending.lease.slot_id,
        &"c".repeat(64),
        &pending.lease.generation,
        "retire-foreign",
    );
    assert_eq!(
        daemon.client.retire_ordinary_cargo(foreign_instance),
        Err(StorageError::Conflict)
    );
    let wrong_generation = retire_request(
        &pending_request,
        &pending.lease.slot_id,
        &instance,
        "999",
        "retire-wrong-generation",
    );
    assert_eq!(
        daemon.client.retire_ordinary_cargo(wrong_generation),
        Err(StorageError::Conflict)
    );
    let unknown_slot = retire_request(
        &pending_request,
        "slot-00000000000000ff",
        &instance,
        "255",
        "retire-unknown",
    );
    assert_eq!(
        daemon.client.retire_ordinary_cargo(unknown_slot),
        Err(StorageError::Refused(RefusalReason::LeaseUnavailable))
    );
    assert_eq!(
        daemon
            .client
            .retire_ordinary_cargo(OrdinaryCargoRetireRequest {
                request: pending_request.clone(),
                slot_id: "not-a-slot".to_owned(),
                storage_instance_id: instance.clone(),
                generation: "1".to_owned(),
                retire_id: "retire-malformed".to_owned(),
                retention: retention_grant(),
            }),
        // The wire keeps errors fixed-shape: the specific in-process
        // reason stays server-side, the class crosses the socket.
        Err(StorageError::InvalidInput("remote request was invalid"))
    );

    // A mismatched root allocation or epoch is incompatible, not a scope
    // that may delete the recorded generation.
    let mut moved_root = pending_request.clone();
    moved_root.root.source_root = other_source.path().to_path_buf();
    assert_eq!(
        daemon
            .client
            .retire_ordinary_cargo(OrdinaryCargoRetireRequest {
                request: moved_root,
                slot_id: pending.lease.slot_id.clone(),
                storage_instance_id: instance.clone(),
                generation: pending.lease.generation.clone(),
                retire_id: "retire-moved".to_owned(),
                retention: retention_grant(),
            }),
        Err(StorageError::Refused(
            RefusalReason::OrdinaryCargoIncompatible
        ))
    );

    // An unproven empty generation is not proven idle and cannot retire.
    assert_eq!(
        daemon.client.retire_ordinary_cargo(retire_request(
            &empty_request,
            &empty.lease.slot_id,
            &empty.lease.storage_instance_id,
            &empty.lease.generation,
            "retire-empty",
        )),
        Err(StorageError::Refused(
            RefusalReason::OrdinaryCargoIncompatible
        ))
    );

    // The pending lease still holds its own generation afterwards, and
    // every other record is intact: generic, pending, uncertain, and empty.
    let inventory = daemon.client.inventory("held").unwrap();
    assert_eq!(inventory.slots.len(), 4);
    assert!(inventory
        .slots
        .iter()
        .any(|slot| slot.slot_id == pending.lease.slot_id && slot.state == SlotState::Leased));
}

#[test]
fn ordinary_cargo_retire_replays_conflicts_and_marks_retired_scopes() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let other_source = TaskRoot::new();
    let daemon = RunningDaemon::start(storage.path(), budget(512), Some(1024));
    let first_request = ordinary_request(
        source.path(),
        "ordinary-retire-replay",
        "ordinary-acquire-retire-replay",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let acquired = daemon
        .client
        .acquire_ordinary_cargo(first_request.clone())
        .expect("retire candidate");
    settle_reusable(
        &daemon.client,
        &first_request,
        &acquired,
        "ordinary-retire-replay-finalize",
    );
    let slot_id = acquired.lease.slot_id.clone();
    let instance = acquired.lease.storage_instance_id.clone();
    let generation = acquired.lease.generation.clone();
    let exact = retire_request(&first_request, &slot_id, &instance, &generation, "retire-1");
    let receipt = daemon
        .client
        .retire_ordinary_cargo(exact.clone())
        .expect("retire with receipt");

    // The same operation ID with changed data conflicts instead of
    // replaying or deleting again. The changed reservation keeps the
    // synthetic authority accepted so the conflict is exact, not denied.
    let mut changed_body = exact.clone();
    changed_body.request.acquire.required_bytes = 65;
    assert_eq!(
        daemon.client.retire_ordinary_cargo(changed_body.clone()),
        Err(StorageError::Conflict)
    );
    assert_eq!(
        daemon.client.lookup_retire_ordinary_cargo(changed_body),
        Err(StorageError::Conflict)
    );
    // A new operation ID against the retired triple is explicitly refused
    // as already retired; its lookup was never recorded.
    let mut new_operation = exact.clone();
    new_operation.retire_id = "retire-2".to_owned();
    assert_eq!(
        daemon.client.retire_ordinary_cargo(new_operation.clone()),
        Err(StorageError::Refused(RefusalReason::OrdinaryCargoRetired))
    );
    assert_eq!(
        daemon
            .client
            .lookup_retire_ordinary_cargo(new_operation)
            .expect("unknown retire operation lookup"),
        LookupResult::Absent
    );
    // A foreign generation naming the retired slot conflicts.
    let mut wrong_generation = exact.clone();
    wrong_generation.generation = "999".to_owned();
    assert_eq!(
        daemon.client.retire_ordinary_cargo(wrong_generation),
        Err(StorageError::Conflict)
    );
    // The exact replay still returns the stored receipt without effects.
    assert_eq!(
        daemon.client.retire_ordinary_cargo(exact.clone()),
        Ok(receipt.clone())
    );
    assert_eq!(
        daemon
            .client
            .lookup_retire_ordinary_cargo(exact)
            .expect("exact retire lookup"),
        LookupResult::Present(receipt)
    );

    // An operation ID is ledger-scoped: reusing it for a different live
    // generation conflicts instead of retiring the wrong bytes.
    let mut second_request = ordinary_request(
        other_source.path(),
        "ordinary-retire-replay-second",
        "ordinary-acquire-retire-replay-second",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    second_request.root.consumer_root_id = "consumer-root-2".to_owned();
    let second = daemon
        .client
        .acquire_ordinary_cargo(second_request.clone())
        .expect("second generation");
    settle_reusable(
        &daemon.client,
        &second_request,
        &second,
        "ordinary-retire-replay-second-finalize",
    );
    assert_eq!(
        daemon.client.retire_ordinary_cargo(retire_request(
            &second_request,
            &second.lease.slot_id,
            &second.lease.storage_instance_id,
            &second.lease.generation,
            "retire-1",
        )),
        Err(StorageError::Conflict)
    );
    // The second generation is untouched and still reusable.
    let inventory = daemon.client.inventory("replay").unwrap();
    assert_eq!(inventory.slots.len(), 1);
    assert_eq!(inventory.slots[0].slot_id, second.lease.slot_id);
}

#[test]
fn ordinary_cargo_retire_lost_reply_reconciles_exact_body() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let daemon = RunningDaemon::start(storage.path(), budget(256), Some(1024));
    let first_request = ordinary_request(
        source.path(),
        "ordinary-retire-lost",
        "ordinary-acquire-retire-lost",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let acquired = daemon
        .client
        .acquire_ordinary_cargo(first_request.clone())
        .expect("retire candidate");
    fs::write(
        acquired
            .lease
            .slot_path
            .as_ref()
            .expect("slot path")
            .join("unit.rlib"),
        vec![9u8; 2048],
    )
    .expect("write owned fixture payload");
    settle_reusable(
        &daemon.client,
        &first_request,
        &acquired,
        "ordinary-retire-lost-finalize",
    );
    let exact = retire_request(
        &first_request,
        &acquired.lease.slot_id,
        &acquired.lease.storage_instance_id,
        &acquired.lease.generation,
        "retire-lost",
    );
    // The effect commits even though this caller discards its reply.
    super::transport::send_ordinary_retire_and_forget(daemon.socket_path.as_path(), exact.clone())
        .expect("send retire and discard response");
    let lookup = daemon
        .client
        .lookup_retire_ordinary_cargo(exact.clone())
        .expect("lost retire reply reconciles");
    let LookupResult::Present(receipt) = lookup else {
        panic!("durable retirement should be present: {lookup:?}");
    };
    assert_eq!(receipt.retire_id, "retire-lost");
    assert_eq!(receipt.retired_reuse_state, CargoReuseState::Reusable);
    // The identical retry returns the stored receipt; replacement gets a
    // fresh ordinal and never the deleted bytes.
    assert_eq!(
        daemon.client.retire_ordinary_cargo(exact),
        Ok(receipt.clone())
    );
    assert!(!acquired
        .lease
        .slot_path
        .as_ref()
        .expect("recorded slot path")
        .exists());
    let next_request = ordinary_request(
        source.path(),
        "ordinary-retire-lost-next",
        "ordinary-acquire-retire-lost-next",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let replacement = daemon
        .client
        .acquire_ordinary_cargo(next_request)
        .expect("replacement after lost retire reply");
    assert_eq!(replacement.lease.generation, "2");
    assert_eq!(
        replacement.lease.storage_instance_id,
        receipt.storage_instance_id
    );
}

#[test]
fn ordinary_cargo_retire_crash_mid_deletion_resumes_recorded_scope() {
    let root = TaskRoot::new();
    let source = TaskRoot::new();
    let mut child = ChildDaemon::start(root.path());
    let client = Client::new(root.path().join("daemon.sock"));

    // A sibling generation proves an interrupted deletion never touches
    // another generation's bytes.
    let sibling_request = ordinary_request(
        source.path(),
        "ordinary-retire-crash-sibling",
        "ordinary-acquire-retire-crash-sibling",
        64,
        OrdinaryCargoPartition::FullDebug,
    );
    let sibling = client
        .acquire_ordinary_cargo(sibling_request.clone())
        .expect("sibling generation");
    settle_reusable(
        &client,
        &sibling_request,
        &sibling,
        "crash-sibling-finalize",
    );

    let first_request = ordinary_request(
        source.path(),
        "ordinary-retire-crash",
        "ordinary-acquire-retire-crash",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let acquired = client
        .acquire_ordinary_cargo(first_request.clone())
        .expect("crash retire candidate");
    let slot_path = acquired.lease.slot_path.clone().expect("leased slot path");
    // Many owned files widen the deletion window so the kill lands after
    // the persisted intent; the total stays far below the 8 MiB bound.
    for index in 0..300 {
        fs::write(
            slot_path.join(format!("object-{index:04}.o")),
            vec![3u8; 4096],
        )
        .expect("write owned fixture objects");
    }
    settle_reusable(
        &client,
        &first_request,
        &acquired,
        "ordinary-retire-crash-finalize",
    );
    let exact = retire_request(
        &first_request,
        &acquired.lease.slot_id,
        &acquired.lease.storage_instance_id,
        &acquired.lease.generation,
        "retire-crash",
    );
    let caller = client.clone();
    let exact_for_thread = exact.clone();
    let retiring = thread::spawn(move || caller.retire_ordinary_cargo(exact_for_thread));
    thread::sleep(Duration::from_millis(2));
    child.crash();
    let _ = retiring.join().expect("retire thread joined");
    drop(child);

    // Startup recovery resumes only the recorded scope: the bytes are gone
    // and the tombstone receipt is published, or nothing was recorded and
    // the generation is intact. Either outcome is safe; none serves
    // half-deleted bytes as reusable.
    let _restarted = ChildDaemon::start(root.path());
    let recovered = Client::new(root.path().join("daemon.sock"));
    let lookup = recovered
        .lookup_retire_ordinary_cargo(exact.clone())
        .expect("retire lookup after crash");
    match lookup {
        LookupResult::Present(receipt) => {
            assert_eq!(receipt.retire_id, "retire-crash");
            assert_eq!(receipt.retired_reuse_state, CargoReuseState::Reusable);
            assert!(!slot_path.exists());
        }
        LookupResult::Absent => {
            assert!(slot_path.is_dir());
            let replayed = recovered
                .retire_ordinary_cargo(exact.clone())
                .expect("exact retry after pre-intent crash");
            assert_eq!(replayed.retire_id, "retire-crash");
            assert!(!slot_path.exists());
        }
        LookupResult::Uncertain(_) => {
            panic!("a recorded retire intent completes during recovery, never stays uncertain");
        }
    }
    // The sibling generation survived the crash and the resumed deletion
    // with its identity intact.
    let sibling_inventory = recovered
        .inventory("crash-sibling")
        .expect("inventory after crash");
    assert_eq!(sibling_inventory.slots.len(), 1);
    assert_eq!(sibling_inventory.slots[0].slot_id, sibling.lease.slot_id);
    assert_eq!(
        sibling_inventory.slots[0].cargo_reuse_state,
        Some(CargoReuseState::Reusable)
    );
    // Replacement admits cold work with a fresh ordinal, never the retired
    // slot identity.
    let next_request = ordinary_request(
        source.path(),
        "ordinary-retire-crash-next",
        "ordinary-acquire-retire-crash-next",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let replacement = recovered
        .acquire_ordinary_cargo(next_request)
        .expect("replacement after crash recovery");
    assert_ne!(replacement.lease.slot_id, acquired.lease.slot_id);
    assert_eq!(replacement.lease.generation, "3");
    assert!(!replacement.lease.reused);
}

#[test]
fn ordinary_cargo_retirement_history_is_finite_and_replacement_obeys_budget() {
    // The tombstone history is finite control history: filling it refuses
    // the next retirement explicitly instead of trimming evidence, while
    // old receipts still replay and the live generation stays usable.
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let daemon = RunningDaemon::start(storage.path(), budget(4096), Some(1024));
    let base = ordinary_request(
        source.path(),
        "ordinary-retire-cycle",
        "ordinary-acquire-retire-cycle",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    for cycle in 0..64 {
        let mut attempt = base.clone();
        attempt.acquire.envelope.binding.run_id = format!("run-retire-cycle-{cycle}");
        attempt.acquire.envelope.binding.attempt_id = format!("attempt-retire-cycle-{cycle}");
        attempt.acquire.acquire_id = format!("acquire-retire-cycle-{cycle}");
        let acquired = daemon
            .client
            .acquire_ordinary_cargo(attempt.clone())
            .unwrap_or_else(|error| panic!("cycle {cycle} admits replacement: {error:?}"));
        // Every replacement is a fresh cold generation: retirement never
        // recycles the retired slot identity.
        assert!(!acquired.lease.reused);
        let token = acquired.lease.token.as_ref().expect("cycle token");
        daemon
            .client
            .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
                request: attempt.clone(),
                lease_id: acquired.lease.lease_id.clone(),
                token: Some(token.clone()),
                finalize_id: format!("finalize-retire-cycle-{cycle}"),
                evidence: passing_evidence(
                    &attempt.acquire.envelope.binding,
                    &acquired.lease.lease_id,
                    &acquired.lease.slot_id,
                    acquired.lease.slot_path.as_ref().expect("cycle slot path"),
                    token,
                ),
            })
            .expect("cycle publishes reusable generation");
        daemon
            .client
            .retire_ordinary_cargo(OrdinaryCargoRetireRequest {
                request: attempt,
                slot_id: acquired.lease.slot_id.clone(),
                storage_instance_id: acquired.lease.storage_instance_id.clone(),
                generation: acquired.lease.generation.clone(),
                retire_id: format!("retire-cycle-{cycle}"),
                retention: retention_grant(),
            })
            .unwrap_or_else(|error| panic!("cycle {cycle} retires: {error:?}"));
    }
    // One more live generation proves the pool is usable; its retirement
    // then refuses on the finite history cap.
    let mut last = base.clone();
    last.acquire.envelope.binding.run_id = "run-retire-cycle-last".to_owned();
    last.acquire.envelope.binding.attempt_id = "attempt-retire-cycle-last".to_owned();
    last.acquire.acquire_id = "acquire-retire-cycle-last".to_owned();
    let live = daemon
        .client
        .acquire_ordinary_cargo(last.clone())
        .expect("live generation after full history");
    settle_reusable(&daemon.client, &last, &live, "finalize-retire-cycle-last");
    assert_eq!(
        daemon.client.retire_ordinary_cargo(retire_request(
            &last,
            &live.lease.slot_id,
            &live.lease.storage_instance_id,
            &live.lease.generation,
            "retire-cycle-overflow",
        )),
        Err(StorageError::Refused(RefusalReason::UnknownAccounting))
    );
    // The refused retirement recorded no intent: the live generation stays
    // warm and reusable, and old receipts still replay.
    let warm = daemon
        .client
        .acquire_ordinary_cargo({
            let mut retry = base.clone();
            retry.acquire.envelope.binding.run_id = "run-retire-cycle-warm".to_owned();
            retry.acquire.envelope.binding.attempt_id = "attempt-retire-cycle-warm".to_owned();
            retry.acquire.acquire_id = "acquire-retire-cycle-warm".to_owned();
            retry
        })
        .expect("live generation still reusable after refused retirement");
    assert!(warm.lease.reused);
    assert_eq!(warm.lease.slot_id, live.lease.slot_id);

    // Replacement under a full aggregate budget refuses instead of growing
    // the pool, and under a tight host reserve instead of breaching it.
    let capped_storage = TaskRoot::new();
    let capped_source = TaskRoot::new();
    let capped_other = TaskRoot::new();
    let capped = RunningDaemon::start(capped_storage.path(), budget(128), Some(1024));
    let capped_first = ordinary_request(
        capped_source.path(),
        "ordinary-retire-capped",
        "ordinary-acquire-retire-capped",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let capped_acquired = capped
        .client
        .acquire_ordinary_cargo(capped_first.clone())
        .expect("capped first generation");
    settle_reusable(
        &capped.client,
        &capped_first,
        &capped_acquired,
        "capped-first-finalize",
    );
    let mut capped_second = ordinary_request(
        capped_other.path(),
        "ordinary-retire-capped-second",
        "ordinary-acquire-retire-capped-second",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    capped_second.root.consumer_root_id = "consumer-root-2".to_owned();
    let capped_second_acquired = capped
        .client
        .acquire_ordinary_cargo(capped_second.clone())
        .expect("capped second generation fills the budget");
    // Measured payload (not just the reservation) occupies the budget:
    // finalize remeasures the slot directory.
    fs::write(
        capped_second_acquired
            .lease
            .slot_path
            .as_ref()
            .expect("capped slot path")
            .join("unit.rlib"),
        vec![5u8; 60],
    )
    .expect("write measured capped payload");
    settle_reusable(
        &capped.client,
        &capped_second,
        &capped_second_acquired,
        "capped-second-finalize",
    );
    capped
        .client
        .retire_ordinary_cargo(retire_request(
            &capped_first,
            &capped_acquired.lease.slot_id,
            &capped_acquired.lease.storage_instance_id,
            &capped_acquired.lease.generation,
            "retire-capped-first",
        ))
        .expect("retire frees exactly one reservation");
    let mut oversized = ordinary_request(
        capped_source.path(),
        "ordinary-retire-capped-big",
        "ordinary-acquire-retire-capped-big",
        100,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    oversized.acquire.envelope.binding.run_id = "run-capped-big".to_owned();
    oversized.acquire.envelope.binding.attempt_id = "attempt-capped-big".to_owned();
    assert_eq!(
        capped.client.acquire_ordinary_cargo(oversized),
        Err(StorageError::Refused(RefusalReason::BudgetExceeded))
    );
    let mut fitting = ordinary_request(
        capped_source.path(),
        "ordinary-retire-capped-fit",
        "ordinary-acquire-retire-capped-fit",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    fitting.acquire.envelope.binding.run_id = "run-capped-fit".to_owned();
    fitting.acquire.envelope.binding.attempt_id = "attempt-capped-fit".to_owned();
    let fitted = capped
        .client
        .acquire_ordinary_cargo(fitting)
        .expect("fitting replacement admits after retirement");
    assert_eq!(fitted.lease.generation, "3");

    let tight_storage = TaskRoot::new();
    let tight_source = TaskRoot::new();
    let tight_other = TaskRoot::new();
    let tight = RunningDaemon::start(tight_storage.path(), budget(10_000), Some(200));
    let tight_first = ordinary_request(
        tight_source.path(),
        "ordinary-retire-tight",
        "ordinary-acquire-retire-tight",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let tight_acquired = tight
        .client
        .acquire_ordinary_cargo(tight_first.clone())
        .expect("tight first generation");
    settle_reusable(
        &tight.client,
        &tight_first,
        &tight_acquired,
        "tight-first-finalize",
    );
    let mut tight_second = ordinary_request(
        tight_other.path(),
        "ordinary-retire-tight-second",
        "ordinary-acquire-retire-tight-second",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    tight_second.root.consumer_root_id = "consumer-root-2".to_owned();
    let tight_second_acquired = tight
        .client
        .acquire_ordinary_cargo(tight_second.clone())
        .expect("tight second generation");
    settle_reusable(
        &tight.client,
        &tight_second,
        &tight_second_acquired,
        "tight-second-finalize",
    );
    tight
        .client
        .retire_ordinary_cargo(retire_request(
            &tight_first,
            &tight_acquired.lease.slot_id,
            &tight_acquired.lease.storage_instance_id,
            &tight_acquired.lease.generation,
            "retire-tight-first",
        ))
        .expect("tight retire frees one reservation");
    let mut reserve_breaking = ordinary_request(
        tight_source.path(),
        "ordinary-retire-tight-big",
        "ordinary-acquire-retire-tight-big",
        200,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    reserve_breaking.acquire.envelope.binding.run_id = "run-tight-big".to_owned();
    reserve_breaking.acquire.envelope.binding.attempt_id = "attempt-tight-big".to_owned();
    assert_eq!(
        tight.client.acquire_ordinary_cargo(reserve_breaking),
        Err(StorageError::Refused(RefusalReason::FreeSpaceReserve))
    );
}

#[test]
fn retired_ledger_version_refuses_version_five_without_mutation() {
    let root = TaskRoot::new();
    let source = TaskRoot::new();
    let mut daemon = RunningDaemon::start(root.path(), budget(256), Some(1024));
    let first_request = ordinary_request(
        source.path(),
        "ordinary-ledger-version",
        "ordinary-acquire-ledger-version",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let acquired = daemon
        .client
        .acquire_ordinary_cargo(first_request.clone())
        .expect("version fixture acquire");
    settle_reusable(
        &daemon.client,
        &first_request,
        &acquired,
        "ordinary-ledger-version-finalize",
    );
    daemon
        .client
        .retire_ordinary_cargo(retire_request(
            &first_request,
            &acquired.lease.slot_id,
            &acquired.lease.storage_instance_id,
            &acquired.lease.generation,
            "retire-ledger-version",
        ))
        .expect("version fixture retire");
    daemon.stop();

    // A version-5 ledger predates retirement intents and tombstones. It is
    // refused without migration or reinterpretation, byte-for-byte intact.
    let ledger_path = root.path().join("ledger.bin");
    let version_six = fs::read(&ledger_path).expect("read version-6 ledger");
    let mut version_five = version_six.clone();
    version_five
        .get_mut(12..16)
        .expect("ledger header contains version bytes")
        .copy_from_slice(&5_u32.to_le_bytes());
    fs::write(&ledger_path, &version_five).expect("mark fixture with version 5");
    let result = Daemon::bind(
        DaemonConfig {
            root: root.path().to_path_buf(),
            budget: budget(256),
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
        version_five
    );
}

const V2_INVALIDATION_DIGEST: &str =
    "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

/// Clearly synthetic v2 authority for retirement fixtures: the recomputed
/// stable eligibility reference plus exact attempt authentication. This
/// does not implement a production Nucleus receipt issuer.
struct V2RetireAuthority;

impl V2RetireAuthority {
    fn stable_reference(request: &OrdinaryCargoRequest) -> Option<String> {
        ordinary_cargo_stable_reuse_reference(request, V2_INVALIDATION_DIGEST).ok()
    }

    fn boundary_is_canonical(digest: &str) -> bool {
        digest
            .strip_prefix("sha256:")
            .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
    }
}

impl EvidenceAuthority for V2RetireAuthority {
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
    }

    fn verify_ordinary_cargo_admission(&self, request: &OrdinaryCargoRequest) -> bool {
        request.acceptance.policy_version == ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION
            && request.acceptance.installation_epoch == "install-1"
            && request.acceptance.credential.expose() == "ordinary-proof"
            && request.root.consumer_root_id == "consumer-root"
            && Self::boundary_is_canonical(&request.acceptance.input_boundary_digest)
            && Self::stable_reference(request)
                .is_some_and(|reference| reference == request.acceptance.acceptance_reference)
    }

    fn verify_ordinary_cargo_stable_reuse_policy(&self, request: &OrdinaryCargoRequest) -> bool {
        request.acceptance.policy_version == ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION
            && Self::stable_reference(request)
                .is_some_and(|reference| reference == request.acceptance.acceptance_reference)
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

    fn verify_ordinary_cargo_retirement(&self, request: &OrdinaryCargoRetireRequest) -> bool {
        self.verify_ordinary_cargo_admission(&request.request)
            && request.retention.source_evidence_reference == "retained-source-v2"
            && request.retention.artifact_evidence_reference == "retained-artifact-v2"
            && request.retention.debug_evidence_reference == "retained-debug-v2"
            && request.retention.credential.expose() == "retention-proof-v2"
    }
}

fn v2_request(
    source: &Path,
    attempt: &str,
    acquire_id: &str,
    boundary_mark: char,
) -> OrdinaryCargoRequest {
    let mut request = ordinary_request(
        source,
        attempt,
        acquire_id,
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    request.acceptance.policy_version = ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION;
    let reference = ordinary_cargo_stable_reuse_reference(&request, V2_INVALIDATION_DIGEST)
        .expect("typed stable reuse reference");
    request.acceptance.acceptance_reference = reference;
    request.acceptance.input_boundary_digest =
        format!("sha256:{}", boundary_mark.to_string().repeat(64));
    request
}

fn v2_retire_request(
    base: &OrdinaryCargoRequest,
    slot_id: &str,
    storage_instance_id: &str,
    generation: &str,
    retire_id: &str,
) -> OrdinaryCargoRetireRequest {
    OrdinaryCargoRetireRequest {
        request: base.clone(),
        slot_id: slot_id.to_owned(),
        storage_instance_id: storage_instance_id.to_owned(),
        generation: generation.to_owned(),
        retire_id: retire_id.to_owned(),
        retention: OrdinaryCargoRetention {
            source_evidence_reference: "retained-source-v2".to_owned(),
            artifact_evidence_reference: "retained-artifact-v2".to_owned(),
            debug_evidence_reference: "retained-debug-v2".to_owned(),
            credential: Credential::new("retention-proof-v2"),
        },
    }
}

#[test]
fn ordinary_cargo_stable_v2_generation_retires_and_replacement_keeps_eligibility() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let mut daemon = RunningDaemon::start_with_authority(
        storage.path(),
        budget(256),
        Some(1024),
        Arc::new(V2RetireAuthority),
    );
    let first_request = v2_request(
        source.path(),
        "ordinary-v2-retire-first",
        "ordinary-acquire-v2-retire-first",
        'e',
    );
    let second_request = v2_request(
        source.path(),
        "ordinary-v2-retire-second",
        "ordinary-acquire-v2-retire-second",
        'f',
    );
    assert_eq!(
        first_request.acceptance.acceptance_reference,
        second_request.acceptance.acceptance_reference
    );
    assert_ne!(
        first_request.acceptance.input_boundary_digest,
        second_request.acceptance.input_boundary_digest
    );
    let first = daemon
        .client
        .acquire_ordinary_cargo(first_request.clone())
        .expect("v2 cold admit");
    settle_reusable(
        &daemon.client,
        &first_request,
        &first,
        "ordinary-v2-retire-first-finalize",
    );
    let slot_id = first.lease.slot_id.clone();
    let instance = first.lease.storage_instance_id.clone();
    let generation = first.lease.generation.clone();

    let mut denied = v2_retire_request(
        &first_request,
        &slot_id,
        &instance,
        &generation,
        "retire-v2-1",
    );
    denied.retention.credential = Credential::new("wrong-proof");
    assert_eq!(
        daemon.client.retire_ordinary_cargo(denied),
        Err(StorageError::Unauthorized)
    );
    let exact = v2_retire_request(
        &first_request,
        &slot_id,
        &instance,
        &generation,
        "retire-v2-1",
    );
    let receipt = daemon
        .client
        .retire_ordinary_cargo(exact.clone())
        .expect("v2 retire with retention grant");
    assert_eq!(receipt.retired_reuse_state, CargoReuseState::Reusable);

    // The tombstoned v2 identity still replays access-free, and the next
    // exact attempt under the same stable eligibility admits a fresh
    // ordinal instead of reusing retired bytes.
    let replay = daemon
        .client
        .acquire_ordinary_cargo(first_request.clone())
        .expect("v2 old exact replay stays bound");
    assert_eq!(replay.lease.state, SlotState::Idle);
    assert!(replay.lease.slot_path.is_none());
    assert!(replay.lease.token.is_none());
    let replacement = daemon
        .client
        .acquire_ordinary_cargo(second_request.clone())
        .expect("v2 replacement keeps stable eligibility");
    assert_ne!(replacement.lease.slot_id, slot_id);
    assert_ne!(replacement.lease.generation, generation);
    assert!(!replacement.lease.reused);
    daemon.stop();

    // Restart reloads the v2 tombstone and recomputes its stable
    // reference without reinterpretation.
    let daemon = RunningDaemon::start_with_authority(
        storage.path(),
        budget(256),
        Some(1024),
        Arc::new(V2RetireAuthority),
    );
    assert_eq!(
        daemon
            .client
            .lookup_retire_ordinary_cargo(exact)
            .expect("v2 tombstone survives restart"),
        LookupResult::Present(receipt)
    );
}
