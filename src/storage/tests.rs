use super::*;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Barrier};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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
}

struct FixedFreeSpace(Option<u64>);

impl FreeSpaceProbe for FixedFreeSpace {
    fn available_bytes(&self, _: &Path) -> Result<Option<u64>, StorageError> {
        Ok(self.0)
    }
}

struct RunningDaemon {
    client: Client,
    shutdown: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<Result<(), StorageError>>>,
}

impl RunningDaemon {
    fn start(root: &Path, budget: Budget, free_bytes: Option<u64>) -> Self {
        let daemon = Daemon::bind(
            DaemonConfig {
                root: root.to_path_buf(),
                budget,
            },
            Arc::new(TestAuthority),
            Arc::new(FixedFreeSpace(free_bytes)),
        )
        .expect("bind daemon");
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
fn a_second_daemon_cannot_bind_the_same_private_root() {
    let root = TaskRoot::new();
    let daemon = RunningDaemon::start(root.path(), budget(128), Some(1024));
    let second = Daemon::bind(
        DaemonConfig {
            root: root.path().to_path_buf(),
            budget: budget(128),
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
    let retry = recovered.acquire(claim).expect("replay after restart");
    assert_eq!(retry.lease_id, grant.lease_id);
    assert_eq!(retry.state, SlotState::Uncertain);
    assert_eq!(retry.slot_path, None);
    assert_eq!(retry.token, None);
    let replayed_claim = request("attempt-restart", "unused", 64);
    let binding = &replayed_claim.envelope.binding;
    let evidence = SettlementEvidence {
        binding: binding.clone(),
        consumer_credential: Credential::new("consumer-proof"),
        terminal: None,
        capture: None,
        containment: ContainmentEvidence::Unknown,
    };
    let reconciled = recovered
        .reconcile(&grant.lease_id, None, "reconcile-restart", evidence)
        .expect("reconcile authenticated proof");
    assert_eq!(reconciled.state, SlotState::Uncertain);
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
        .finalize(&grant.lease_id, Some(token), "finalize-uncertain", evidence)
        .expect("incomplete proof is recorded conservatively");
    assert_eq!(settled.state, SlotState::Uncertain);
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
