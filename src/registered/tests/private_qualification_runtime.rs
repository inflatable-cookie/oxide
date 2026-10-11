//! Private storage-runtime proof (oxide#033). Each session is an actual compiled
//! `oxide-storage-runtime` child serving a real daemon; the parent uses the real
//! storage `Client`. Evidence is synthetic. No Cargo workload, Nucleus benefit or
//! production issuer is involved, and no test constructs `Daemon` in the parent.
use crate::registered::qualification::{
    admission_subject, settlement_subject, Endpoint, MacKey, QualificationAuthority,
};
use crate::registered::storage_runtime::{
    bootstrap_digest, policy_digest, serve_bootstrap, shutdown_frame, verify_ready, verify_stopped,
    ApprovedSubject, ReadyExpectation, RuntimeBootstrap, RuntimeBudget, RuntimeEndpoint,
};
use crate::storage::{
    AcquireRequest, AcquireResult, CaptureReceipt, Client, ContainmentEvidence,
    CooperativeCompletionEvidence, Credential, PartitionKey, ProcessIdentityEvidence, RunBinding,
    RunEnvelope, SettlementEvidence, SlotState, StorageError, TerminalOutcome, TerminalRecord,
};
use crate::storage::{FinalizationReceipt, RefusalReason};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(feature = "private-qualification-fixture")]
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SCOPE: &str = "synthetic-task033-runtime-proof";
const AUTHORITY_KEY: &str = "4444444444444444444444444444444444444444444444444444444444444444";
const CONTROL_KEY: &str = "5555555555555555555555555555555555555555555555555555555555555555";
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Owns the child for the duration of one session and kills it if a test panics.
struct ChildGuard(Option<Child>);
impl ChildGuard {
    fn stderr_after_exit(&mut self) -> String {
        let mut text = String::new();
        if let Some(mut stderr) = self.0.as_mut().and_then(|child| child.stderr.take()) {
            let _ = stderr.read_to_string(&mut text);
        }
        text
    }
    fn wait_exit(&mut self, limit: Duration) -> ExitStatus {
        let child = self.0.as_mut().expect("child present");
        let deadline = Instant::now() + limit;
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "runtime child exceeded {limit:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn scratch(name: &str) -> PathBuf {
    let root = PathBuf::from(
        std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").expect("canonical task-owned scratch"),
    );
    assert_eq!(fs::canonicalize(&root).unwrap(), root);
    let base = root.join(format!(
        "rt-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&base).unwrap();
    fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
    base
}

fn private_dir(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn binding(run: usize) -> RunBinding {
    RunBinding {
        consumer: "synthetic-consumer".into(),
        project: "oxide-synthetic".into(),
        component: "owned:core".into(),
        workflow: "synthetic-workflow".into(),
        task_id: format!("task-{run}"),
        workspace_id: "workspace-033".into(),
        run_id: format!("run-{run}"),
        attempt_id: format!("attempt-{run}"),
        commit: "1".repeat(40),
        source_tree_digest: digest(b"synthetic source"),
    }
}

fn partition() -> PartitionKey {
    PartitionKey {
        project_component: "oxide-synthetic:owned:core".into(),
        workflow: "synthetic-workflow".into(),
        cargo_provenance: "synthetic-cargo".into(),
        rustc_provenance: "synthetic-rustc".into(),
        host: "aarch64-apple-darwin".into(),
        target: "aarch64-apple-darwin".into(),
        profile: "full-debug".into(),
        features: vec![],
        config_digest: digest(b"config"),
        environment_digest: digest(b"env"),
        path_identity: digest(b"synthetic path"),
    }
}

fn approved(run: usize) -> ApprovedSubject {
    ApprovedSubject {
        binding: binding(run),
        partition: partition(),
        admission_reference: format!("admission-{run}"),
    }
}

fn budget() -> RuntimeBudget {
    RuntimeBudget {
        cap_bytes: serde_json::from_value(json!(16 * 1024 * 1024)).unwrap(),
        free_reserve_bytes: serde_json::from_value(json!(1024 * 1024)).unwrap(),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

const DEFAULT_GRACE: u64 = 2;

fn bootstrap_for(
    storage_root: &Path,
    control: &Path,
    subjects: Vec<ApprovedSubject>,
    expires: u64,
    grace: u64,
) -> RuntimeBootstrap {
    let endpoint = Endpoint::capture(control.to_path_buf()).unwrap();
    RuntimeBootstrap {
        version: 1,
        storage_protocol: super::super::storage_runtime::STORAGE_PROTOCOL_VERSION,
        control_protocol: 1,
        scope: SCOPE.into(),
        expires_unix_seconds: serde_json::from_value(json!(expires)).unwrap(),
        stop_grace_seconds: serde_json::from_value(json!(grace)).unwrap(),
        storage_root: storage_root.to_path_buf(),
        budget: budget(),
        admissions: subjects,
        ordinary_cargo_request_digests: Vec::new(),
        authority_key_hex: AUTHORITY_KEY.into(),
        control_key_hex: CONTROL_KEY.into(),
        control: RuntimeEndpoint {
            path: control.to_path_buf(),
            device: serde_json::from_value(json!(endpoint.device)).unwrap(),
            inode: serde_json::from_value(json!(endpoint.inode)).unwrap(),
        },
    }
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

/// One actual runtime session: the parent owns its control stream and verified ready identity.
struct Runtime {
    child: ChildGuard,
    stream: Option<std::os::unix::net::UnixStream>,
    reader: Option<BufReader<std::os::unix::net::UnixStream>>,
    ready: Value,
    instance: String,
    bootstrap_digest: String,
    policy_digest: String,
    storage_socket: PathBuf,
    subjects: Vec<ApprovedSubject>,
}

impl Runtime {
    fn start(
        base: &Path,
        name: &str,
        storage_root: &Path,
        subjects: Vec<ApprovedSubject>,
        window: u64,
    ) -> Self {
        Self::start_with_grace(base, name, storage_root, subjects, window, DEFAULT_GRACE)
    }

    fn start_with_grace(
        base: &Path,
        name: &str,
        storage_root: &Path,
        subjects: Vec<ApprovedSubject>,
        window: u64,
        grace: u64,
    ) -> Self {
        let control_dir = base.join(format!("ctl-{name}"));
        private_dir(&control_dir);
        let control = control_dir.join("p.sock");
        let listener = UnixListener::bind(&control).unwrap();
        fs::set_permissions(&control, fs::Permissions::from_mode(0o600)).unwrap();
        let bootstrap = bootstrap_for(
            storage_root,
            &control,
            subjects.clone(),
            now() + window,
            grace,
        );
        let bytes = serde_json::to_vec(&bootstrap).unwrap();
        let file = base.join(format!("bootstrap-{name}.json"));
        write_private(&file, &bytes);
        let mut child = ChildGuard(Some(
            Command::new(
                std::env::var_os("OXIDE_STORAGE_RUNTIME").expect("Effigy compiled the runtime"),
            )
            .arg(&file)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
        ));
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        let stderr = child.stderr_after_exit();
                        panic!("runtime did not connect; child stderr: {stderr}");
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let ready: Value = serde_json::from_str(&line).expect("ready frame");
        let bootstrap_digest = bootstrap_digest(&bytes);
        let policy = policy_digest(
            SCOPE,
            &budget(),
            serde_json::from_value(json!(grace)).unwrap(),
            &subjects,
            &[],
        )
        .unwrap();
        let storage_socket = storage_root.join("daemon.sock");
        let key = MacKey::from_hex(CONTROL_KEY).unwrap();
        let expectation = ReadyExpectation {
            scope: SCOPE.into(),
            bootstrap_digest: bootstrap_digest.clone(),
            policy_digest: policy.clone(),
            storage_socket: storage_socket.clone(),
        };
        let identity = verify_ready(&key, &expectation, &ready).expect("authenticated ready");
        assert_eq!(identity.storage.path, storage_socket);
        assert!(identity.expires_unix_seconds <= now() + window);
        // The bootstrap is consumed before effects; a copy cannot be presented again.
        assert!(!file.exists(), "bootstrap consumed before effects");
        Self {
            child,
            stream: Some(stream),
            reader: Some(reader),
            ready,
            instance: identity.storage_instance_id,
            bootstrap_digest,
            policy_digest: policy,
            storage_socket,
            subjects,
        }
    }

    fn client(&self) -> Client {
        Client::new(self.storage_socket.clone())
    }

    fn authority(&self) -> QualificationAuthority {
        let allowed = self
            .subjects
            .iter()
            .map(|subject| RunEnvelope {
                binding: subject.binding.clone(),
                partition: subject.partition.clone(),
                admission_reference: subject.admission_reference.clone(),
                admission_credential: Credential::new(""),
            })
            .collect();
        let authority = QualificationAuthority::new(
            MacKey::from_hex(AUTHORITY_KEY).unwrap(),
            SCOPE.into(),
            allowed,
        )
        .unwrap();
        authority.install_instance(&self.instance).unwrap();
        authority
    }

    fn shutdown_and_expect_ack(&mut self, id: &str) -> ExitStatus {
        let key = MacKey::from_hex(CONTROL_KEY).unwrap();
        let frame =
            shutdown_frame(&key, SCOPE, &self.instance, &self.bootstrap_digest, id).unwrap();
        let stream = self.stream.as_mut().unwrap();
        stream.write_all(format!("{frame}\n").as_bytes()).unwrap();
        let mut line = String::new();
        self.reader.as_mut().unwrap().read_line(&mut line).unwrap();
        let ack: Value = serde_json::from_str(&line).unwrap();
        assert!(verify_stopped(
            &key,
            SCOPE,
            &self.instance,
            &self.bootstrap_digest,
            id,
            &ack
        ));
        self.child.wait_exit(Duration::from_secs(10))
    }

    /// Closes the parent's control channel without any shutdown command.
    fn lose_control(&mut self) {
        self.reader.take();
        self.stream.take();
    }
}

fn envelope(authority: &QualificationAuthority, subject: &ApprovedSubject) -> RunEnvelope {
    let mut envelope = RunEnvelope {
        binding: subject.binding.clone(),
        partition: subject.partition.clone(),
        admission_reference: subject.admission_reference.clone(),
        admission_credential: Credential::new(""),
    };
    envelope.admission_credential = Credential::new(
        authority
            .receipt("oxide-admission-v1", &admission_subject(&envelope))
            .unwrap(),
    );
    envelope
}

fn acquire(
    client: &Client,
    authority: &QualificationAuthority,
    subject: &ApprovedSubject,
    n: usize,
) -> Result<AcquireResult, StorageError> {
    client.acquire(AcquireRequest {
        envelope: envelope(authority, subject),
        acquire_id: format!("acquire-{n}"),
        required_bytes: 1024 * 1024,
    })
}

/// Synthetic cooperative completion signed with the daemon-issued lease token.
fn finalize_cooperative(
    client: &Client,
    authority: &QualificationAuthority,
    acquired: &AcquireResult,
    subject: &ApprovedSubject,
    n: usize,
) -> Result<FinalizationReceipt, StorageError> {
    let token = acquired.token.as_ref().expect("token from acquire");
    let slot = acquired.slot_path.clone().expect("slot path from acquire");
    let mut completion = CooperativeCompletionEvidence {
        lease_id: acquired.lease_id.clone(),
        slot_id: acquired.slot_id.clone(),
        slot_path: slot.to_str().unwrap().into(),
        temp_dir: slot.join("temp").to_str().unwrap().into(),
        capture_dir: slot.join("capture").to_str().unwrap().into(),
        workflow: subject.binding.workflow.clone(),
        workflow_definition_digest: digest(b"synthetic runtime definition"),
        repository_head: "1".repeat(40),
        fixture_source_digest: subject.binding.source_tree_digest.clone(),
        host_run_id: format!("synthetic-host-{n}"),
        run_id: subject.binding.run_id.clone(),
        attempt_id: subject.binding.attempt_id.clone(),
        processes: vec![ProcessIdentityEvidence {
            pid: std::process::id(),
            start_identity: digest(b"synthetic process claim"),
        }],
        free_space_samples: vec![32 * 1024 * 1024 * 1024],
        exit_code: 0,
        stdout_digest: digest(b"synthetic stdout"),
        stderr_digest: digest(b""),
        capture_reference: format!("synthetic-capture-{n}"),
        capture_digest: digest(b"synthetic capture"),
        signature: String::new(),
    };
    completion.sign(token);
    let mut evidence = SettlementEvidence {
        binding: subject.binding.clone(),
        consumer_credential: Credential::new(""),
        terminal: Some(TerminalRecord {
            run_id: subject.binding.run_id.clone(),
            outcome: TerminalOutcome::Passed,
            credential: Credential::new(""),
        }),
        capture: Some(CaptureReceipt {
            reference: completion.capture_reference.clone(),
            digest: completion.capture_digest.clone(),
            host_run_id: completion.host_run_id.clone(),
            run_id: subject.binding.run_id.clone(),
            attempt_id: subject.binding.attempt_id.clone(),
            credential: Credential::new(""),
        }),
        containment: ContainmentEvidence::CooperativeWorkflow(completion),
    };
    let subject_value = settlement_subject(&evidence).unwrap();
    evidence.consumer_credential = Credential::new(
        authority
            .receipt("oxide-settlement-v1", &subject_value)
            .unwrap(),
    );
    evidence.terminal.as_mut().unwrap().credential = Credential::new(
        authority
            .receipt("oxide-terminal-v1", &subject_value)
            .unwrap(),
    );
    evidence.capture.as_mut().unwrap().credential = Credential::new(
        authority
            .receipt("oxide-capture-v1", &subject_value)
            .unwrap(),
    );
    client.finalize(
        &acquired.lease_id,
        Some(token),
        &format!("finalize-{n}"),
        evidence,
    )
}

#[test]
fn private_qualification_runtime_cold_reuse_shutdown_and_authority_refusals() {
    let base = scratch("cold");
    let storage_root = base.join("store");
    private_dir(&storage_root);
    let subjects = vec![approved(1), approved(2), approved(3)];
    let mut runtime = Runtime::start(&base, "one", &storage_root, subjects.clone(), 60);
    let client = runtime.client();
    let authority = runtime.authority();

    // Authenticated readiness refuses substitution, replay and wrong expectations.
    let key = MacKey::from_hex(CONTROL_KEY).unwrap();
    let expectation = ReadyExpectation {
        scope: SCOPE.into(),
        bootstrap_digest: runtime.bootstrap_digest.clone(),
        policy_digest: runtime.policy_digest.clone(),
        storage_socket: runtime.storage_socket.clone(),
    };
    let mut forged = runtime.ready.clone();
    forged["storage"]["inode"] = json!(0);
    assert!(verify_ready(&key, &expectation, &forged).is_err());
    let mut replayed = runtime.ready.clone();
    replayed["storage_instance_id"] = json!("a".repeat(64));
    assert!(verify_ready(&key, &expectation, &replayed).is_err());
    let wrong_bootstrap = ReadyExpectation {
        bootstrap_digest: digest(b"another bootstrap"),
        ..expectation
    };
    assert!(verify_ready(&key, &wrong_bootstrap, &runtime.ready).is_err());
    let wrong_key = MacKey::from_hex(AUTHORITY_KEY).unwrap();
    assert!(verify_ready(
        &wrong_key,
        &ReadyExpectation {
            scope: SCOPE.into(),
            bootstrap_digest: runtime.bootstrap_digest.clone(),
            policy_digest: runtime.policy_digest.clone(),
            storage_socket: runtime.storage_socket.clone(),
        },
        &runtime.ready
    )
    .is_err());

    // Cold publish-equivalent: acquire, cooperative finalize to Idle.
    let first = acquire(&client, &authority, &subjects[0], 1).unwrap();
    assert_eq!(first.state, SlotState::Leased);
    assert!(!first.reused);
    let slot_path = first.slot_path.clone().unwrap();
    assert!(slot_path.starts_with(storage_root.join("slots")));
    assert_eq!(
        finalize_cooperative(&client, &authority, &first, &subjects[0], 1)
            .unwrap()
            .state,
        SlotState::Idle
    );

    // Sequential same-slot reuse with changed run identity.
    let second = acquire(&client, &authority, &subjects[1], 2).unwrap();
    assert!(second.reused);
    assert_eq!(second.slot_id, first.slot_id);
    assert_eq!(second.slot_path.as_ref(), Some(&slot_path));
    assert_eq!(
        finalize_cooperative(&client, &authority, &second, &subjects[1], 2)
            .unwrap()
            .state,
        SlotState::Idle
    );

    // Unapproved admission refuses before any lease, and forged settlement refuses.
    let mut unapproved = approved(9);
    unapproved.admission_reference = "unapproved".into();
    assert!(matches!(
        acquire(&client, &authority, &unapproved, 9),
        Err(StorageError::Unauthorized)
    ));
    let third = acquire(&client, &authority, &subjects[2], 3).unwrap();
    let forged_authority = QualificationAuthority::new(
        MacKey::from_hex(CONTROL_KEY).unwrap(),
        SCOPE.into(),
        subjects
            .iter()
            .map(|s| RunEnvelope {
                binding: s.binding.clone(),
                partition: s.partition.clone(),
                admission_reference: s.admission_reference.clone(),
                admission_credential: Credential::new(""),
            })
            .collect(),
    )
    .unwrap();
    forged_authority
        .install_instance(&runtime.instance)
        .unwrap();
    assert!(matches!(
        finalize_cooperative(&client, &forged_authority, &third, &subjects[2], 3),
        Err(StorageError::Unauthorized)
    ));
    assert_eq!(
        finalize_cooperative(&client, &authority, &third, &subjects[2], 3)
            .unwrap()
            .state,
        SlotState::Idle
    );

    let status = runtime.shutdown_and_expect_ack("stop-cold");
    assert!(status.success(), "authenticated shutdown exits zero");
    assert!(!runtime.storage_socket.exists(), "daemon endpoint removed");
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn private_qualification_runtime_control_loss_holds_then_restart_is_uncertain() {
    let base = scratch("loss");
    let storage_root = base.join("store");
    private_dir(&storage_root);
    let subjects = vec![approved(1), approved(2)];
    let mut first = Runtime::start(&base, "loss-a", &storage_root, subjects.clone(), 60);
    let authority = first.authority();
    let client = first.client();
    let held = acquire(&client, &authority, &subjects[0], 1).unwrap();
    assert!(held.token.is_some());
    first.lose_control();
    assert_eq!(
        first.child.wait_exit(Duration::from_secs(10)).code(),
        Some(2)
    );
    assert!(first.child.stderr_after_exit().contains("ControlClosed"));
    assert!(
        !first.storage_socket.exists(),
        "daemon stopped with the channel"
    );

    // A fresh bootstrap restarts the same durable root; the held lease is never Idle.
    let mut second = Runtime::start(&base, "loss-b", &storage_root, subjects.clone(), 60);
    let restarted_authority = second.authority();
    let restarted = second.client();
    assert_eq!(
        restarted.inventory("restart-held").unwrap().slots[0].state,
        SlotState::Uncertain
    );
    assert_eq!(second.instance, held.storage_instance_id);
    let refused = acquire(&restarted, &restarted_authority, &subjects[1], 2);
    match refused {
        Ok(result) => {
            assert_eq!(result.state, SlotState::Uncertain);
            assert!(result.token.is_none() && result.slot_path.is_none());
        }
        Err(StorageError::Refused(reason)) => {
            assert!(matches!(
                reason,
                RefusalReason::LeaseUnavailable | RefusalReason::UnknownAccounting
            ));
        }
        Err(other) => panic!("unexpected acquire outcome {other}"),
    }
    assert!(second.shutdown_and_expect_ack("stop-loss").success());
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn private_qualification_runtime_lifetime_expiry_is_nonrenewing_and_holds_state() {
    let base = scratch("expiry");
    let storage_root = base.join("store");
    private_dir(&storage_root);
    let subjects = vec![approved(1), approved(2)];
    let mut first = Runtime::start(&base, "expiry-a", &storage_root, subjects.clone(), 3);
    let started = Instant::now();
    let authority = first.authority();
    let client = first.client();
    let held = acquire(&client, &authority, &subjects[0], 1).unwrap();
    assert!(held.token.is_some());
    // Acquisition and any other request do not renew the window.
    let status = first.child.wait_exit(Duration::from_secs(10));
    assert_eq!(status.code(), Some(2));
    assert!(first.child.stderr_after_exit().contains("Lifetime"));
    assert!(started.elapsed() < Duration::from_secs(8));
    assert!(!first.storage_socket.exists());

    let mut second = Runtime::start(&base, "expiry-b", &storage_root, subjects.clone(), 60);
    assert_eq!(
        second.client().inventory("after-expiry").unwrap().slots[0].state,
        SlotState::Uncertain
    );
    assert!(second.shutdown_and_expect_ack("stop-expiry").success());
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn private_qualification_runtime_blocked_storage_operation_is_abandoned_at_the_grace() {
    let base = scratch("blocked");
    let storage_root = base.join("store");
    private_dir(&storage_root);
    let subjects = vec![approved(1)];
    let mut runtime = Runtime::start_with_grace(&base, "blocked", &storage_root, subjects, 3, 1);
    // A stalled client keeps the storage thread inside one request until its 5-second read timeout.
    let stalled = UnixStream::connect(&runtime.storage_socket).unwrap();
    let started = Instant::now();
    let status = runtime.child.wait_exit(Duration::from_secs(10));
    assert_eq!(
        status.code(),
        Some(3),
        "lifetime plus grace, not the blocked read"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(runtime
        .child
        .stderr_after_exit()
        .contains("abandoned a blocked storage operation"));
    drop(stalled);
    fs::remove_dir_all(&base).unwrap();
}

/// Spawns one actual runtime child with a bootstrap whose window ends at `expires`. It does
/// not wait for readiness. The caller decides what the child must or must not publish.
fn spawn_unready(
    base: &Path,
    name: &str,
    storage_root: &Path,
    subjects: Vec<ApprovedSubject>,
    expires: u64,
) -> (ChildGuard, UnixListener, PathBuf, PathBuf) {
    let control_dir = base.join(format!("ctl-{name}"));
    private_dir(&control_dir);
    let control = control_dir.join("p.sock");
    let listener = UnixListener::bind(&control).unwrap();
    fs::set_permissions(&control, fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let bootstrap = bootstrap_for(storage_root, &control, subjects, expires, DEFAULT_GRACE);
    let file = base.join(format!("bootstrap-{name}.json"));
    write_private(&file, &serde_json::to_vec(&bootstrap).unwrap());
    let child = ChildGuard(Some(
        Command::new(
            std::env::var_os("OXIDE_STORAGE_RUNTIME").expect("Effigy compiled the runtime"),
        )
        .arg(&file)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap(),
    ));
    (child, listener, control_dir, file)
}

#[test]
fn private_qualification_runtime_window_ending_at_setup_publishes_no_ready_and_exits_bounded() {
    // A one-second window cannot leave the readiness margin once setup has run, so the child
    // must exit before it publishes ready. Setup may also see the window spent before it starts.
    let base = scratch("before-ready");
    let storage_root = base.join("store");
    private_dir(&storage_root);
    let subjects = vec![approved(1)];
    let (mut child, listener, _control_dir, _file) =
        spawn_unready(&base, "before-ready", &storage_root, subjects, now() + 1);
    let started = Instant::now();
    let status = child.wait_exit(Duration::from_secs(10));
    assert!(started.elapsed() < Duration::from_secs(10), "bounded exit");
    assert!(
        matches!(status.code(), Some(1) | Some(4)),
        "refused or expired, never success: {status:?}"
    );
    // Any connection the child made carries no ready frame. The child closed it on exit.
    if let Ok((mut stream, _)) = listener.accept() {
        // The child has already exited, so the peer is closed and this read cannot block.
        // Socket timeouts are not set here: macOS rejects them on a disconnected socket.
        stream.set_nonblocking(false).unwrap();
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes);
        // Only an authenticated failure frame may appear here, never a ready frame.
        for line in String::from_utf8_lossy(&bytes).lines() {
            let frame: Value = serde_json::from_str(line).unwrap();
            assert_ne!(
                frame["operation"], "ready",
                "no ready before readiness: {line}"
            );
            assert_eq!(frame["operation"], "failed", "only a failure frame: {line}");
        }
    }
    assert!(
        !storage_root.join("daemon.sock").exists(),
        "no storage endpoint remains serving"
    );
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn private_qualification_runtime_already_expired_bootstrap_is_retained_without_effects() {
    // An expired window is refused before the replay marker and before the bootstrap is
    // consumed, so the owner keeps the file and no startup effect exists.
    let base = scratch("expired");
    let storage_root = base.join("store");
    private_dir(&storage_root);
    let subjects = vec![approved(1)];
    let (mut child, _listener, control_dir, file) =
        spawn_unready(&base, "expired", &storage_root, subjects, now() - 10);
    let status = child.wait_exit(Duration::from_secs(10));
    assert_eq!(status.code(), Some(1));
    assert!(file.exists(), "expired bootstrap is retained for its owner");
    let markers = fs::read_dir(&control_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("runtime-"))
        .count();
    assert_eq!(markers, 0, "no replay marker was created");
    assert!(!storage_root.join("daemon.sock").exists());
    fs::remove_dir_all(&base).unwrap();
}

/// Spawns a child whose startup reaches the named controlled step and blocks there. The step
/// is an owned FIFO in the owner's control directory. This test never opens it for writing,
/// so the child blocks until the supervisor expires the window.
#[cfg(feature = "private-qualification-fixture")]
fn stalled_child(
    base: &Path,
    name: &str,
    point: &str,
    window: u64,
) -> (ChildGuard, UnixListener, PathBuf, PathBuf, Vec<u8>, PathBuf) {
    let storage_root = base.join(format!("store-{name}"));
    private_dir(&storage_root);
    let control_dir = base.join(format!("ctl-{name}"));
    private_dir(&control_dir);
    let control = control_dir.join("p.sock");
    let listener = UnixListener::bind(&control).unwrap();
    fs::set_permissions(&control, fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let fifo = control_dir.join(format!("fixture-stall-{point}"));
    let fifo_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    // SAFETY: the path is a NUL-terminated string inside this test's private control directory.
    let made = unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) };
    assert_eq!(made, 0, "owned stall FIFO created");
    let bootstrap = bootstrap_for(
        &storage_root,
        &control,
        vec![approved(1)],
        now() + window,
        DEFAULT_GRACE,
    );
    let bytes = serde_json::to_vec(&bootstrap).unwrap();
    let file = base.join(format!("bootstrap-{name}.json"));
    write_private(&file, &bytes);
    let child = ChildGuard(Some(
        Command::new(
            std::env::var_os("OXIDE_STORAGE_RUNTIME").expect("Effigy compiled the runtime"),
        )
        .arg(&file)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap(),
    ));
    (child, listener, storage_root, file, bytes, control_dir)
}

/// Bytes of the one connection the child made, after it has exited. The read cannot block.
#[cfg(feature = "private-qualification-fixture")]
fn drained_connection(listener: &UnixListener) -> Vec<u8> {
    let Ok((mut stream, _)) = listener.accept() else {
        return Vec::new();
    };
    stream.set_nonblocking(false).unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    bytes
}

#[cfg(feature = "private-qualification-fixture")]
fn replay_markers(dir: &Path) -> usize {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("runtime-"))
        .count()
}

#[cfg(feature = "private-qualification-fixture")]
#[test]
fn private_qualification_runtime_actual_child_stall_before_bind_expires_without_ready() {
    // Controlled step: after control connect and endpoint capture, before Daemon::bind.
    let base = scratch("stall-bind");
    // Leave the cold child enough time to validate its bootstrap and reach the FIFO; a
    // shorter window can expire during process startup and exercise early refusal instead.
    let (mut child, listener, storage_root, file, bytes, control_dir) =
        stalled_child(&base, "stall-bind", "before-bind", 8);
    let started = Instant::now();
    let status = child.wait_exit(Duration::from_secs(15));
    assert!(started.elapsed() < Duration::from_secs(10), "bounded exit");
    let stderr = child.stderr_after_exit();
    assert_eq!(
        status.code(),
        Some(4),
        "expired before readiness; child stderr: {stderr}"
    );
    assert!(
        stderr.contains("fixture stall entered: before-bind"),
        "blocked phase entered: {stderr}"
    );
    assert!(stderr.contains("startup expired"), "stderr: {stderr}");
    // The child had connected before the stall. No ready frame, failure frame or serve followed.
    assert!(
        drained_connection(&listener).is_empty(),
        "no late ready or serve"
    );
    assert!(!storage_root.join("daemon.sock").exists(), "bind never ran");
    // Durable intent is the replay marker alone, and the bootstrap was consumed.
    assert_eq!(replay_markers(&control_dir), 1);
    assert!(!file.exists());
    // Replay of the identical bytes is refused by the spent window and the marker, and the
    // file is retained for its owner. No effect follows.
    write_private(&file, &bytes);
    assert!(serve_bootstrap(&file).is_err());
    assert!(file.exists(), "refused bootstrap is retained");
    assert!(!storage_root.join("daemon.sock").exists());
    fs::remove_dir_all(&base).unwrap();
}

#[cfg(feature = "private-qualification-fixture")]
#[test]
fn private_qualification_runtime_actual_child_stall_before_ready_expires_without_ready() {
    // Controlled step: after Daemon::bind and frame construction, before the ready write.
    let base = scratch("stall-ready");
    let (mut child, listener, storage_root, file, bytes, control_dir) =
        stalled_child(&base, "stall-ready", "before-ready", 3);
    let started = Instant::now();
    let status = child.wait_exit(Duration::from_secs(15));
    assert!(started.elapsed() < Duration::from_secs(10), "bounded exit");
    assert_eq!(status.code(), Some(4), "expired before readiness");
    let stderr = child.stderr_after_exit();
    assert!(
        stderr.contains("fixture stall entered: before-ready"),
        "blocked phase entered: {stderr}"
    );
    // The stall ran after bind, so the endpoint file exists. Exit does not run the daemon's
    // drop, so the stale endpoint remains. It was never served, and the next bind refuses it
    // through the existing stale-socket check.
    assert!(
        drained_connection(&listener).is_empty(),
        "no ready frame before expiry"
    );
    assert!(storage_root.join("daemon.sock").exists());
    assert_eq!(replay_markers(&control_dir), 1);
    assert!(!file.exists());
    write_private(&file, &bytes);
    assert!(serve_bootstrap(&file).is_err());
    assert!(file.exists(), "refused bootstrap is retained");
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn private_qualification_runtime_bootstrap_substitution_and_replay_refuse_before_effects() {
    let base = scratch("refuse");
    let storage_root = base.join("store");
    private_dir(&storage_root);
    let control_dir = base.join("ctl");
    private_dir(&control_dir);
    let control = control_dir.join("p.sock");
    let _listener = UnixListener::bind(&control).unwrap();
    fs::set_permissions(&control, fs::Permissions::from_mode(0o600)).unwrap();
    let subjects = vec![approved(1)];
    let valid = bootstrap_for(
        &storage_root,
        &control,
        subjects.clone(),
        now() + 60,
        DEFAULT_GRACE,
    );
    let no_effects = |root: &Path| {
        assert!(!root.join("daemon.sock").exists());
        assert!(!root.join("slots").exists());
    };

    // Replay marker: the same exact bootstrap bytes refuse, and the file is not consumed.
    let bytes = serde_json::to_vec(&valid).unwrap();
    let replay = base.join("replay.json");
    write_private(&replay, &bytes);
    write_private(
        &control_dir.join(format!("runtime-{}", &bootstrap_digest(&bytes)[7..])),
        b"earlier",
    );
    assert!(serve_bootstrap(&replay).is_err());
    assert!(
        replay.exists(),
        "refused bootstrap is retained for its owner"
    );
    no_effects(&storage_root);

    let refuse = |name: &str, value: Value| {
        let file = base.join(format!("{name}.json"));
        write_private(&file, &serde_json::to_vec(&value).unwrap());
        assert!(serve_bootstrap(&file).is_err(), "{name} must refuse");
        assert!(file.exists(), "{name} is retained");
        no_effects(&storage_root);
    };
    let base_value = serde_json::to_value(&valid).unwrap();
    let mut mutated = base_value.clone();
    mutated["expires_unix_seconds"] = json!(1);
    refuse("expired", mutated);
    let mut mutated = base_value.clone();
    mutated["budget"]["cap_bytes"] = json!(0);
    refuse("zero-capacity", mutated);
    let mut mutated = base_value.clone();
    mutated["control_key_hex"] = json!(AUTHORITY_KEY);
    refuse("shared-key", mutated);
    let mut mutated = base_value.clone();
    mutated["unexpected"] = json!(true);
    refuse("unknown-field", mutated);
    let mut mutated = base_value.clone();
    mutated["control"]["inode"] = json!("1");
    refuse("control-substitution", mutated);
    let mut mutated = base_value.clone();
    mutated["admissions"] = json!([]);
    refuse("no-admissions", mutated);
    let mut mutated = base_value.clone();
    mutated["storage_root"] = json!(base.join("ghost").to_str().unwrap());
    refuse("missing-root", mutated);

    // Symlinked storage root is not the canonical owned root.
    let linked_root = base.join("linked-root");
    symlink(&storage_root, &linked_root).unwrap();
    let mut mutated = base_value.clone();
    mutated["storage_root"] = json!(linked_root.to_str().unwrap());
    refuse("symlinked-root", mutated);

    // Control parent equal to the storage state root is refused.
    let shared_socket = storage_root.join("p.sock");
    let _shared_listener = UnixListener::bind(&shared_socket).unwrap();
    fs::set_permissions(&shared_socket, fs::Permissions::from_mode(0o600)).unwrap();
    let shared = bootstrap_for(
        &storage_root,
        &shared_socket,
        subjects.clone(),
        now() + 60,
        DEFAULT_GRACE,
    );
    let shared_file = base.join("shared.json");
    write_private(&shared_file, &serde_json::to_vec(&shared).unwrap());
    assert!(serve_bootstrap(&shared_file).is_err());
    assert!(shared_file.exists());
    no_effects(&storage_root);

    // Group-readable bootstrap is refused before any read is trusted.
    let loose = base.join("loose.json");
    write_private(&loose, &serde_json::to_vec(&valid).unwrap());
    fs::set_permissions(&loose, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(serve_bootstrap(&loose).is_err());
    no_effects(&storage_root);

    fs::remove_dir_all(&base).unwrap();
}
