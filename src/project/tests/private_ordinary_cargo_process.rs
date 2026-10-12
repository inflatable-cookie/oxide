use super::{task_scratch_root, FixedFreeSpace, FixtureRoot};
use crate::project::ordinary_cargo::{
    ordinary_cargo_callback_context, ordinary_cargo_callback_context_v2,
    ordinary_cargo_capture_budget_digest, ordinary_cargo_input_boundary_digest,
    ordinary_cargo_input_boundary_digest_with_capture_budget, validate_acquired_slot_path,
    validate_invocation, validate_invocation_with_capture_budget, OrdinaryCargoCaptureBudget,
    OrdinaryCargoInvocation, OrdinaryCargoOperationIds, ORDINARY_CARGO_ATTEMPT_OPERATION,
    ORDINARY_CARGO_ATTEMPT_VERSION, ORDINARY_CARGO_CAPTURE_ATTEMPT_OPERATION,
    ORDINARY_CARGO_CAPTURE_ATTEMPT_VERSION,
};
use crate::registered::qualification::{
    admission_subject, bundle_ordinary_credentials, canonical_digest,
    ordinary_cargo_request_digest, ordinary_cargo_settlement_subject, settlement_subject,
    CallbackAuthenticator, Endpoint, MacKey, QualificationAuthority,
};
use crate::registered::runner_host::WireU64;
use crate::storage::{
    ordinary_cargo_stable_reuse_reference, AcquireRequest, Budget, CaptureReceipt, CargoReuseState,
    Client, ContainmentEvidence, CooperativeCompletionEvidence, Credential, Daemon, DaemonConfig,
    EvidenceAuthority, FreeSpaceProbe, NoSpawnEvidence, OrdinaryCargoAcceptance,
    OrdinaryCargoPartition, OrdinaryCargoRequest, PartitionKey, ProcessIdentityEvidence,
    RunBinding, RunEnvelope, SettlementEvidence, SlotState, StorageError, StrictPhysicalCargoRoot,
    TerminalOutcome, TerminalRecord,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const SCOPE: &str = "synthetic-ordinary-cargo-scope";
const STORAGE_KEY_HEX: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CALLBACK_KEY_HEX: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const MAX_FRAME_BYTES: usize = 1024 * 1024;
const REMAINING_MS: u64 = 25_000;
// Fixture window for each callback exchange, shared with the parent trace so
// deadline offsets are reported against the same value the child receives.
const CALLBACK_TIMEOUT_MS: u64 = 15_000;
// Withheld replies outlive one callback window by a fixed margin, so the child
// has reached its deadline before any late reply is written.
const WITHHELD_REPLY_MS: u64 = 18_000;
const TRACE_MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy)]
enum Scenario {
    Passed,
    Failed,
    NotSubmitted,
    Timeout,
    LostReply,
    FullTupleChanged,
    CaptureDelayed,
    CaptureMismatch,
    CaptureLost,
    BudgetReuse,
}

/// Parent-side scenario trace: fixed phase labels, monotonic offsets and
/// bounded reasons. It never records frame bodies, keys or environment values.
#[derive(Clone)]
struct Trace(Arc<Mutex<TraceLog>>);

struct TraceLog {
    start: Instant,
    lines: Vec<String>,
    bytes: usize,
    truncated: bool,
}

impl Trace {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(TraceLog {
            start: Instant::now(),
            lines: Vec::new(),
            bytes: 0,
            truncated: false,
        })))
    }

    fn mark(&self, phase: &str, reason: &str) {
        let mut log = self.0.lock().unwrap();
        if log.truncated {
            return;
        }
        let line = format!(
            "{:>9.3}s {phase} {reason}",
            log.start.elapsed().as_secs_f64()
        );
        if log.bytes + line.len() + 1 > TRACE_MAX_BYTES {
            log.truncated = true;
            log.lines.push("trace truncated at 64 KiB".into());
            return;
        }
        log.bytes += line.len() + 1;
        log.lines.push(line);
    }

    fn render(&self) -> String {
        self.0.lock().unwrap().lines.join("\n")
    }
}

struct TestDaemon {
    client: Client,
    socket: PathBuf,
    instance: String,
    shutdown: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<Result<(), StorageError>>>,
}

impl TestDaemon {
    fn start(root: &Path, allowed: RunEnvelope, request_digest: String) -> Self {
        Self::start_many(root, vec![allowed], vec![request_digest])
    }

    fn start_many(root: &Path, allowed: Vec<RunEnvelope>, request_digests: Vec<String>) -> Self {
        let authority = Arc::new(
            QualificationAuthority::new(
                MacKey::from_hex(STORAGE_KEY_HEX).unwrap(),
                SCOPE.into(),
                allowed,
            )
            .unwrap()
            .with_ordinary_cargo_request_digests(request_digests)
            .unwrap(),
        );
        let daemon = Daemon::bind(
            DaemonConfig {
                root: root.to_path_buf(),
                budget: Budget {
                    cap_bytes: 16 * 1024 * 1024,
                    free_reserve_bytes: 0,
                },
            },
            authority.clone() as Arc<dyn EvidenceAuthority>,
            Arc::new(FixedFreeSpace(Some(u64::MAX))) as Arc<dyn FreeSpaceProbe>,
        )
        .unwrap();
        let instance = daemon.storage_instance_id().to_owned();
        authority.install_instance(&instance).unwrap();
        let socket = daemon.socket_path().to_path_buf();
        let client = Client::new(socket.clone());
        let (shutdown, receiver) = mpsc::channel();
        let thread = thread::spawn(move || daemon.serve(receiver));
        Self {
            client,
            socket,
            instance,
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }

    fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            assert!(thread.join().unwrap().is_ok());
        }
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        self.stop();
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn invocation(case: &FixtureRoot, number: &str) -> OrdinaryCargoInvocation {
    let source = case.dir("source");
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    let target = case.dir("target");
    let temp = case.dir("temp");
    let capture = case.dir("capture");
    let binding = RunBinding {
        consumer: "synthetic-consumer".into(),
        project: "synthetic-project".into(),
        component: "synthetic-component".into(),
        workflow: "synthetic-ordinary-workflow".into(),
        task_id: format!("synthetic-task-{number}"),
        workspace_id: format!("synthetic-workspace-{number}"),
        run_id: format!("synthetic-run-{number}"),
        attempt_id: format!("synthetic-attempt-{number}"),
        commit: "1".repeat(40),
        source_tree_digest: digest(b"synthetic source tree"),
    };
    let partition = PartitionKey {
        project_component: "synthetic-project:synthetic-component".into(),
        workflow: binding.workflow.clone(),
        cargo_provenance: "cargo 1.97.1 synthetic".into(),
        rustc_provenance: "rustc 1.97.1 synthetic".into(),
        host: "aarch64-apple-darwin".into(),
        target: "aarch64-apple-darwin".into(),
        profile: "dev".into(),
        features: vec!["default".into()],
        config_digest: digest(b"synthetic cargo config"),
        environment_digest: digest(b"synthetic complete environment"),
        path_identity: "ordinary-cargo-storage-v1".into(),
    };
    let request = OrdinaryCargoRequest {
        acquire: AcquireRequest {
            envelope: RunEnvelope {
                binding,
                partition,
                admission_reference: format!("synthetic-admission-{number}"),
                admission_credential: Credential::new("synthetic-admission-placeholder"),
            },
            acquire_id: format!("synthetic-acquire-{number}"),
            required_bytes: 1024,
        },
        root: StrictPhysicalCargoRoot {
            source_root: source.clone(),
            consumer_root_id: format!("synthetic-root-{number}"),
            allocation_epoch: format!("synthetic-allocation-{number}"),
        },
        partition: OrdinaryCargoPartition::OrdinaryTest,
        acceptance: OrdinaryCargoAcceptance {
            policy_version: 1,
            installation_epoch: "synthetic-installation-epoch".into(),
            acceptance_reference: "synthetic-accepted-workflow".into(),
            input_boundary_digest: digest(b"placeholder boundary"),
            credential: Credential::new("synthetic-acceptance-placeholder"),
        },
    };
    let mut invocation = OrdinaryCargoInvocation {
        request,
        branch_id: format!("synthetic-branch-{number}"),
        source_content_digest: digest(b"synthetic source content"),
        selector: "rust:test".into(),
        argv: vec!["test".into(), "--workspace".into(), "--all-targets".into()],
        cwd: source,
        environment: BTreeMap::from([
            ("CARGO_BUILD_BUILD_DIR".into(), None),
            ("CARGO_TARGET_DIR".into(), None),
            ("TMPDIR".into(), None),
            ("TMP".into(), None),
            ("TEMP".into(), None),
            ("RUSTFLAGS".into(), None),
            ("CARGO_HOME".into(), None),
        ]),
        target_dir: target,
        temp_dir: temp,
        capture_dir: capture,
        expected_outputs: vec![
            "source-archive".into(),
            "cargo-log".into(),
            "test-result".into(),
            "binary".into(),
            "full-debug".into(),
        ],
        operation_ids: OrdinaryCargoOperationIds {
            submit: format!("submit-{number}"),
            capture: format!("capture-{number}"),
            not_submitted: format!("not-submitted-{number}"),
            finalize: format!("finalize-{number}"),
        },
    };
    invocation.request.acceptance.input_boundary_digest =
        ordinary_cargo_input_boundary_digest(&invocation).unwrap();
    invocation
}

fn install_request_credentials(
    invocation: &mut OrdinaryCargoInvocation,
    instance: &str,
    request_digest: &str,
) {
    let key = MacKey::from_hex(STORAGE_KEY_HEX).unwrap();
    let admission_digest =
        canonical_digest(&admission_subject(&invocation.request.acquire.envelope)).unwrap();
    invocation.request.acquire.envelope.admission_credential = Credential::new(
        key.sign("oxide-admission-v1", &[SCOPE, instance, &admission_digest])
            .unwrap(),
    );
    invocation.request.acceptance.credential = Credential::new(
        key.sign(
            "oxide-ordinary-cargo-admission-v1",
            &[SCOPE, instance, request_digest],
        )
        .unwrap(),
    );
}

fn endpoint_json(endpoint: &Endpoint) -> Value {
    json!({
        "path": endpoint.path,
        "device": WireU64(endpoint.device),
        "inode": WireU64(endpoint.inode),
    })
}

fn invocation_json(invocation: &OrdinaryCargoInvocation) -> Value {
    json!({
        "acquire": {
            "envelope": invocation.request.acquire.envelope,
            "acquire_id": invocation.request.acquire.acquire_id,
            "required_bytes": WireU64(invocation.request.acquire.required_bytes),
        },
        "root": {
            "source_root": invocation.request.root.source_root,
            "consumer_root_id": invocation.request.root.consumer_root_id,
            "allocation_epoch": invocation.request.root.allocation_epoch,
        },
        "partition": invocation.request.partition.label(),
        "acceptance": {
            "policy_version": invocation.request.acceptance.policy_version,
            "installation_epoch": invocation.request.acceptance.installation_epoch,
            "acceptance_reference": invocation.request.acceptance.acceptance_reference,
            "input_boundary_digest": invocation.request.acceptance.input_boundary_digest,
            "credential": invocation.request.acceptance.credential,
        },
        "branch_id": invocation.branch_id,
        "source_content_digest": invocation.source_content_digest,
        "selector": invocation.selector,
        "argv": invocation.argv,
        "cwd": invocation.cwd,
        "environment": invocation.environment,
        "target_dir": invocation.target_dir,
        "temp_dir": invocation.temp_dir,
        "capture_dir": invocation.capture_dir,
        "expected_outputs": invocation.expected_outputs,
        "operation_ids": {
            "submit": invocation.operation_ids.submit,
            "capture": invocation.operation_ids.capture,
            "not_submitted": invocation.operation_ids.not_submitted,
            "finalize": invocation.operation_ids.finalize,
        },
    })
}

fn create_bootstrap(
    case: &FixtureRoot,
    name: &str,
    invocation: &OrdinaryCargoInvocation,
    storage: &Path,
    parent: &Path,
    request_digest: &str,
    capture_budget: Option<&OrdinaryCargoCaptureBudget>,
) -> (PathBuf, String) {
    let storage_endpoint = Endpoint::capture(storage.to_path_buf()).unwrap();
    let parent_endpoint = Endpoint::capture(parent.to_path_buf()).unwrap();
    let (version, operation, callback_context) = match capture_budget {
        Some(_) => (
            ORDINARY_CARGO_CAPTURE_ATTEMPT_VERSION,
            ORDINARY_CARGO_CAPTURE_ATTEMPT_OPERATION,
            ordinary_cargo_callback_context_v2(SCOPE, request_digest).unwrap(),
        ),
        None => (
            ORDINARY_CARGO_ATTEMPT_VERSION,
            ORDINARY_CARGO_ATTEMPT_OPERATION,
            ordinary_cargo_callback_context(SCOPE, request_digest).unwrap(),
        ),
    };
    let mut bootstrap = json!({
        "version": version,
        "operation": operation,
        "scope": SCOPE,
        "remaining_ms": WireU64(REMAINING_MS),
        "callback_timeout_ms": WireU64(CALLBACK_TIMEOUT_MS),
        "storage": endpoint_json(&storage_endpoint),
        "parent": endpoint_json(&parent_endpoint),
        "callback_key_hex": CALLBACK_KEY_HEX,
        "callback_context": callback_context,
        "invocation": invocation_json(invocation),
    });
    if let Some(capture_budget) = capture_budget {
        bootstrap["capture_budget"] = json!(capture_budget);
    }
    let bytes = serde_json::to_vec(&bootstrap).unwrap();
    let path = case
        .path
        .join("callbacks")
        .join(format!("bootstrap-{name}.json"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    (path, callback_context)
}

fn read_frame(stream: &mut UnixStream) -> std::io::Result<Value> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame bound",
        ));
    }
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn write_frame(stream: &mut UnixStream, value: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).unwrap();
    let length = u32::try_from(body.len()).unwrap();
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(&body)
}

fn signed_callback_reply(request: &Value, payload: Value, context: &str) -> Value {
    let method = request["method"].as_str().unwrap();
    let id = request["id"].as_str().unwrap();
    let launch_digest = request["launch_digest"].as_str().unwrap();
    let acquired_digest = request["acquired_digest"].as_str().unwrap();
    let command_digest = request["command_digest"].as_str().unwrap();
    let version = request["version"].as_u64().unwrap() as u32;
    let operation = request["operation"].as_str().unwrap();
    let result_digest = canonical_digest(&json!({
        "method": method,
        "id": id,
        "launch_digest": launch_digest,
        "acquired_digest": acquired_digest,
        "command_digest": command_digest,
        "payload": payload,
    }))
    .unwrap();
    let authenticator = CallbackAuthenticator::new(
        MacKey::from_hex(CALLBACK_KEY_HEX).unwrap(),
        context.to_owned(),
    )
    .unwrap();
    let receipt = authenticator
        .receipt(method, id, launch_digest, operation, &result_digest)
        .unwrap();
    json!({
        "version": version,
        "method": method,
        "id": id,
        "launch_digest": launch_digest,
        "acquired_digest": acquired_digest,
        "command_digest": command_digest,
        "payload": payload,
        "receipt": receipt,
    })
}

fn ordinary_storage_credential(
    request: &OrdinaryCargoRequest,
    evidence: &SettlementEvidence,
    generic_domain: &str,
    instance: &str,
) -> String {
    let key = MacKey::from_hex(STORAGE_KEY_HEX).unwrap();
    let generic_digest = canonical_digest(&settlement_subject(evidence).unwrap()).unwrap();
    let generic = key
        .sign(generic_domain, &[SCOPE, instance, &generic_digest])
        .unwrap();
    let ordinary_digest = canonical_digest(
        &ordinary_cargo_settlement_subject(
            &ordinary_cargo_request_digest(request).unwrap(),
            evidence,
        )
        .unwrap(),
    )
    .unwrap();
    let ordinary = key
        .sign(
            "oxide-ordinary-cargo-settlement-v1",
            &[SCOPE, instance, &ordinary_digest],
        )
        .unwrap();
    bundle_ordinary_credentials(&generic, &ordinary).unwrap()
}

fn make_no_submit_payload(
    callback_request: &Value,
    request: &OrdinaryCargoRequest,
    instance: &str,
) -> Value {
    let acquired = &callback_request["acquired"];
    let run = callback_request["no_submit_claim"]["run_identity"].clone();
    let proof = NoSpawnEvidence {
        lease_id: acquired["lease_id"].as_str().unwrap().into(),
        slot_id: acquired["slot_id"].as_str().unwrap().into(),
        slot_path: acquired["slot_path"].as_str().unwrap().into(),
        run_id: run["run_id"].as_str().unwrap().into(),
        attempt_id: run["attempt_id"].as_str().unwrap().into(),
        signature: "synthetic-placeholder".into(),
    };
    let evidence = SettlementEvidence {
        binding: request.acquire.envelope.binding.clone(),
        consumer_credential: Credential::new("placeholder"),
        terminal: None,
        capture: None,
        containment: ContainmentEvidence::NotSpawned(proof),
    };
    let settlement =
        ordinary_storage_credential(request, &evidence, "oxide-settlement-v1", instance);
    json!({
        "status": "not_submitted",
        "submission_state": "never_began",
        "writer_state": "none",
        "closure_reference": "synthetic-no-writer-closure",
        "run": run,
        "settlement_credential": settlement,
    })
}

fn run_identity(source: &Value, mutate_workspace: bool) -> Value {
    let mut identity = json!({
        "host_run_id": "synthetic-host-run",
        "consumer": source["consumer"],
        "project": source["project"],
        "component": source["component"],
        "workflow": source["workflow"],
        "task_id": source["task_id"],
        "workspace_id": source["workspace_id"],
        "branch_id": source["branch_id"],
        "run_id": source["run_id"],
        "attempt_id": source["attempt_id"],
        "commit": source["commit"],
        "source_tree_digest": source["source_tree_digest"],
        "source_content_digest": source["source_content_digest"],
    });
    if mutate_workspace {
        identity["workspace_id"] = json!("synthetic-other-workspace");
    }
    identity
}

fn uncredentialed_capture(
    callback_request: &Value,
    request: &OrdinaryCargoRequest,
    failed: bool,
) -> Value {
    let run = callback_request["run_identity"].clone();
    let acquired = &callback_request["acquired"];
    let command = &callback_request["command"];
    let reference = "synthetic-capture-record";
    let capture_digest = digest(b"synthetic retained output index");
    let exit_code = if failed { 1 } else { 0 };
    let outcome = if failed { "failed" } else { "passed" };
    let large_synthetic_receipt = !callback_request["capture_budget"].is_null();
    let outputs: Vec<Value> = callback_request["expected_outputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|role| {
            json!({
                "role": role,
                "digest": digest(role.as_str().unwrap().as_bytes()),
                "bytes": if large_synthetic_receipt
                    && role.as_str() == Some("binary")
                {
                    88_595_944
                } else if large_synthetic_receipt {
                    1_048_576
                } else {
                    64
                },
            })
        })
        .collect();
    let mut capture = json!({
        "reference": reference,
        "digest": capture_digest,
        "host_run_id": run["host_run_id"],
        "run_id": run["run_id"],
        "attempt_id": run["attempt_id"],
        "credential": "placeholder-capture",
        "outputs": outputs,
    });
    if !callback_request["capture_budget_digest"].is_null() {
        capture["capture_budget_digest"] = callback_request["capture_budget_digest"].clone();
    }
    json!({
        "run": run,
        "outcome": outcome,
        "terminal_credential": "placeholder-terminal",
        "capture": capture,
        "completion": {
            "lease_id": acquired["lease_id"],
            "slot_id": acquired["slot_id"],
            "slot_path": acquired["slot_path"],
            "temp_dir": command["temp_dir"],
            "capture_dir": command["capture_dir"],
            "workflow": request.acquire.envelope.binding.workflow,
            "workflow_definition_digest": digest(b"synthetic workflow definition"),
            "repository_head": run["commit"],
            "fixture_source_digest": run["source_content_digest"],
            "host_run_id": run["host_run_id"],
            "run_id": run["run_id"],
            "attempt_id": run["attempt_id"],
            "processes": [{
                "pid": 1,
                "start_identity": digest(b"synthetic process identity"),
            }],
            "free_space_samples": [WireU64(u64::MAX)],
            "exit_code": exit_code,
            "stdout_digest": digest(b"synthetic stdout"),
            "stderr_digest": digest(b"synthetic stderr"),
            "capture_reference": reference,
            "capture_digest": capture_digest,
        },
        "settlement_credential": "placeholder-settlement",
    })
}

fn settlement_evidence(
    request: &OrdinaryCargoRequest,
    payload: &Value,
    failed: bool,
) -> SettlementEvidence {
    let run = &payload["run"];
    let capture = &payload["capture"];
    let completion = &payload["completion"];
    SettlementEvidence {
        binding: request.acquire.envelope.binding.clone(),
        consumer_credential: Credential::new("placeholder"),
        terminal: Some(TerminalRecord {
            run_id: run["run_id"].as_str().unwrap().into(),
            outcome: if failed {
                TerminalOutcome::Failed
            } else {
                TerminalOutcome::Passed
            },
            credential: Credential::new("placeholder"),
        }),
        capture: Some(CaptureReceipt {
            reference: capture["reference"].as_str().unwrap().into(),
            digest: capture["digest"].as_str().unwrap().into(),
            host_run_id: capture["host_run_id"].as_str().unwrap().into(),
            run_id: capture["run_id"].as_str().unwrap().into(),
            attempt_id: capture["attempt_id"].as_str().unwrap().into(),
            credential: Credential::new("placeholder"),
        }),
        containment: ContainmentEvidence::CooperativeWorkflow(CooperativeCompletionEvidence {
            lease_id: completion["lease_id"].as_str().unwrap().into(),
            slot_id: completion["slot_id"].as_str().unwrap().into(),
            slot_path: completion["slot_path"].as_str().unwrap().into(),
            temp_dir: completion["temp_dir"].as_str().unwrap().into(),
            capture_dir: completion["capture_dir"].as_str().unwrap().into(),
            workflow: completion["workflow"].as_str().unwrap().into(),
            workflow_definition_digest: completion["workflow_definition_digest"]
                .as_str()
                .unwrap()
                .into(),
            repository_head: completion["repository_head"].as_str().unwrap().into(),
            fixture_source_digest: completion["fixture_source_digest"].as_str().unwrap().into(),
            host_run_id: completion["host_run_id"].as_str().unwrap().into(),
            run_id: completion["run_id"].as_str().unwrap().into(),
            attempt_id: completion["attempt_id"].as_str().unwrap().into(),
            processes: vec![ProcessIdentityEvidence {
                pid: 1,
                start_identity: completion["processes"][0]["start_identity"]
                    .as_str()
                    .unwrap()
                    .into(),
            }],
            free_space_samples: vec![u64::MAX],
            exit_code: completion["exit_code"].as_i64().unwrap() as i32,
            stdout_digest: completion["stdout_digest"].as_str().unwrap().into(),
            stderr_digest: completion["stderr_digest"].as_str().unwrap().into(),
            capture_reference: completion["capture_reference"].as_str().unwrap().into(),
            capture_digest: completion["capture_digest"].as_str().unwrap().into(),
            signature: String::new(),
        }),
    }
}

fn capture_payload(
    callback_request: &Value,
    request: &OrdinaryCargoRequest,
    instance: &str,
    failed: bool,
) -> Value {
    let mut payload = uncredentialed_capture(callback_request, request, failed);
    let evidence = settlement_evidence(request, &payload, failed);
    payload["settlement_credential"] = json!(ordinary_storage_credential(
        request,
        &evidence,
        "oxide-settlement-v1",
        instance,
    ));
    payload["terminal_credential"] = json!(ordinary_storage_credential(
        request,
        &evidence,
        "oxide-terminal-v1",
        instance,
    ));
    payload["capture"]["credential"] = json!(ordinary_storage_credential(
        request,
        &evidence,
        "oxide-capture-v1",
        instance,
    ));
    payload
}

fn start_callback_server(
    listener: UnixListener,
    scenario: Scenario,
    request: OrdinaryCargoRequest,
    instance: String,
    context: String,
    acquired: Arc<Mutex<Option<Value>>>,
    trace: Trace,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        trace.mark("callback_accepted", "");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let submit = read_frame(&mut stream).unwrap();
        assert_eq!(submit["method"], "ordinary_cargo.submit");
        trace.mark("submit_received", "");
        *acquired.lock().unwrap() = Some(submit["acquired"].clone());
        match scenario {
            Scenario::Timeout => {
                trace.mark("submit_withheld", "no reply inside callback window");
                thread::sleep(Duration::from_millis(WITHHELD_REPLY_MS));
                return;
            }
            Scenario::LostReply => {
                trace.mark("submit_reply", "partial frame then close");
                stream.write_all(&100u32.to_be_bytes()).unwrap();
                stream.write_all(b"{}").unwrap();
                return;
            }
            Scenario::NotSubmitted => {
                let response = signed_callback_reply(
                    &submit,
                    json!({"status":"not_started","run":null}),
                    &context,
                );
                write_frame(&mut stream, &response).unwrap();
                let no_submit = read_frame(&mut stream).unwrap();
                assert_eq!(no_submit["method"], "ordinary_cargo.not_submitted");
                let payload = make_no_submit_payload(&no_submit, &request, &instance);
                let response = signed_callback_reply(&no_submit, payload, &context);
                write_frame(&mut stream, &response).unwrap();
                return;
            }
            _ => {}
        }
        let run = run_identity(
            &submit["source"],
            matches!(scenario, Scenario::FullTupleChanged),
        );
        let response =
            signed_callback_reply(&submit, json!({"status":"started","run":run}), &context);
        write_frame(&mut stream, &response).unwrap();
        trace.mark("started_reply", "written");
        if matches!(scenario, Scenario::FullTupleChanged) {
            return;
        }
        let capture = match read_frame(&mut stream) {
            Ok(frame) => frame,
            Err(error) => {
                trace.mark("capture_read", &format!("failed {:?}", error.kind()));
                return;
            }
        };
        assert_eq!(capture["method"], "ordinary_cargo.capture");
        trace.mark(
            "capture_received",
            &format!("callback window ends at +{CALLBACK_TIMEOUT_MS}ms (child send is earlier)"),
        );
        let response = match scenario {
            Scenario::CaptureLost => {
                trace.mark("capture_reply", "partial frame then close");
                stream.write_all(&100u32.to_be_bytes()).unwrap();
                stream.write_all(b"{}").unwrap();
                return;
            }
            Scenario::CaptureDelayed => {
                thread::sleep(Duration::from_millis(WITHHELD_REPLY_MS));
                trace.mark("capture_reply", "valid reply written after window");
                signed_callback_reply(
                    &capture,
                    capture_payload(&capture, &request, &instance, false),
                    &context,
                )
            }
            Scenario::CaptureMismatch => {
                // Authentic for a different operation: correspondence must refuse.
                let mut foreign = capture.clone();
                foreign["id"] = json!("synthetic-foreign-capture-operation");
                trace.mark("capture_reply", "valid signature for foreign operation id");
                signed_callback_reply(
                    &foreign,
                    capture_payload(&capture, &request, &instance, false),
                    &context,
                )
            }
            _ => {
                trace.mark("capture_reply", "valid reply written inside window");
                signed_callback_reply(
                    &capture,
                    capture_payload(
                        &capture,
                        &request,
                        &instance,
                        matches!(scenario, Scenario::Failed),
                    ),
                    &context,
                )
            }
        };
        if let Err(error) = write_frame(&mut stream, &response) {
            trace.mark("capture_write", &format!("failed {:?}", error.kind()));
        }
    })
}

fn run_child(bootstrap: &Path, expected_success: bool, trace: &Trace) {
    let binary = std::env::var_os("OXIDE_ORDINARY_CARGO_ATTEMPT")
        .expect("Effigy compiled the private ordinary-Cargo process");
    let mut child = Command::new(binary)
        .arg(bootstrap)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the compiled private route fixture");
    trace.mark("child_spawned", "");
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            trace.mark("child_killed", "exceeded 30-second scenario bound");
            panic!(
                "ordinary-Cargo child exceeded its 30-second scenario bound\ntrace:\n{}",
                trace.render()
            );
        }
        thread::sleep(Duration::from_millis(10));
    };
    trace.mark("child_exited", &format!("code {:?}", status.code()));
    let mut stdout = String::new();
    let mut stderr = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(!stdout.contains(STORAGE_KEY_HEX));
    assert!(!stderr.contains(STORAGE_KEY_HEX));
    assert!(!stdout.contains(CALLBACK_KEY_HEX));
    assert!(!stderr.contains(CALLBACK_KEY_HEX));
    // The binary prints only fixed category messages, so the first line is a
    // bounded, nonsecret held/refused reason for the trace.
    trace.mark("child_stderr", stderr.lines().next().unwrap_or("(empty)"));
    assert_eq!(
        status.success(),
        expected_success,
        "child stderr: {stderr}\ntrace:\n{}",
        trace.render()
    );
}

fn one_scenario(
    name: &str,
    number: &str,
    scenario: Scenario,
    expected_success: bool,
    expected_state: CargoReuseState,
) {
    let case = FixtureRoot::scenario(name);
    let storage_root = case.dir("storage");
    let mut invocation = invocation(&case, number);
    let request_digest = ordinary_cargo_request_digest(&invocation.request).unwrap();
    let mut daemon = TestDaemon::start(
        &storage_root,
        invocation.request.acquire.envelope.clone(),
        request_digest.clone(),
    );
    install_request_credentials(&mut invocation, &daemon.instance, &request_digest);

    let callback_dir = case.dir("callbacks");
    let callback_path = callback_dir.join("callback.sock");
    let listener = UnixListener::bind(&callback_path).unwrap();
    fs::set_permissions(&callback_path, fs::Permissions::from_mode(0o600)).unwrap();
    let (bootstrap, context) = create_bootstrap(
        &case,
        number,
        &invocation,
        &daemon.socket,
        &callback_path,
        &request_digest,
        None,
    );
    let acquired = Arc::new(Mutex::new(None));
    let trace = Trace::new();
    trace.mark("scenario", name);
    let callbacks = start_callback_server(
        listener,
        scenario,
        invocation.request.clone(),
        daemon.instance.clone(),
        context,
        acquired.clone(),
        trace.clone(),
    );
    run_child(&bootstrap, expected_success, &trace);
    callbacks.join().unwrap();
    let report = trace.render();

    let tuple = acquired
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| panic!("submit had acquired identity\ntrace:\n{report}"));
    let lease = crate::storage::AcquireResult {
        lease_id: tuple["lease_id"].as_str().unwrap().into(),
        slot_id: tuple["slot_id"].as_str().unwrap().into(),
        storage_instance_id: tuple["storage_instance_id"].as_str().unwrap().into(),
        generation: tuple["generation"].as_str().unwrap().into(),
        state: SlotState::Leased,
        slot_path: Some(PathBuf::from(tuple["slot_path"].as_str().unwrap())),
        token: None,
        reused: tuple["reused"].as_bool().unwrap(),
    };
    assert!(validate_acquired_slot_path(&storage_root, &lease).is_ok());
    let mut changed_path = lease.clone();
    changed_path.slot_path = Some(case.path.join("capture"));
    assert!(validate_acquired_slot_path(&storage_root, &changed_path).is_err());

    let inventory = daemon.client.inventory("synthetic-process-result").unwrap();
    let slot = inventory
        .slots
        .iter()
        .find(|slot| slot.slot_id == lease.slot_id)
        .unwrap();
    assert_eq!(
        slot.cargo_reuse_state,
        Some(expected_state),
        "trace:\n{report}"
    );
    match expected_state {
        CargoReuseState::Pending => assert_eq!(slot.state, SlotState::Leased, "trace:\n{report}"),
        _ => assert_eq!(slot.state, SlotState::Idle, "trace:\n{report}"),
    }
    daemon.stop();
}

fn capture_policy(total: u64, binary: u64, full_debug: u64) -> OrdinaryCargoCaptureBudget {
    OrdinaryCargoCaptureBudget::new(
        total,
        BTreeMap::from([
            ("source-archive".into(), 16 * 1024 * 1024),
            ("cargo-log".into(), 16 * 1024 * 1024),
            ("test-result".into(), 16 * 1024 * 1024),
            ("binary".into(), binary),
            ("full-debug".into(), full_debug),
        ]),
    )
    .unwrap()
}

fn stable_budget_invocation(
    mut invocation: OrdinaryCargoInvocation,
    attempt: &str,
    capture_budget: &OrdinaryCargoCaptureBudget,
) -> OrdinaryCargoInvocation {
    let binding = &mut invocation.request.acquire.envelope.binding;
    binding.task_id = format!("synthetic-task-{attempt}");
    binding.workspace_id = format!("synthetic-workspace-{attempt}");
    binding.run_id = format!("synthetic-run-{attempt}");
    binding.attempt_id = format!("synthetic-attempt-{attempt}");
    binding.commit = if attempt == "budget-first" {
        "a".repeat(40)
    } else {
        "b".repeat(40)
    };
    binding.source_tree_digest = digest(format!("source tree {attempt}").as_bytes());
    invocation.request.acquire.envelope.admission_reference =
        format!("synthetic-admission-{attempt}");
    invocation.request.acquire.acquire_id = format!("synthetic-acquire-{attempt}");
    invocation.request.acceptance.policy_version =
        crate::storage::ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION;
    invocation.request.acceptance.input_boundary_digest = digest(b"uninstalled placeholder");
    invocation.request.acceptance.acceptance_reference = ordinary_cargo_stable_reuse_reference(
        &invocation.request,
        "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
    )
    .unwrap();
    invocation.branch_id = format!("synthetic-branch-{attempt}");
    invocation.source_content_digest = digest(format!("source content {attempt}").as_bytes());
    if attempt != "budget-first" {
        invocation.target_dir =
            PathBuf::from(invocation.target_dir.parent().unwrap()).join("target-budget-second");
        invocation.temp_dir =
            PathBuf::from(invocation.temp_dir.parent().unwrap()).join("temp-budget-second");
        invocation.capture_dir =
            PathBuf::from(invocation.capture_dir.parent().unwrap()).join("capture-budget-second");
        for path in [
            &invocation.target_dir,
            &invocation.temp_dir,
            &invocation.capture_dir,
        ] {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    invocation.expected_outputs = vec![
        "source-archive".into(),
        "cargo-log".into(),
        "test-result".into(),
        "binary".into(),
        "full-debug".into(),
    ];
    invocation.operation_ids = OrdinaryCargoOperationIds {
        submit: format!("submit-{attempt}"),
        capture: format!("capture-{attempt}"),
        not_submitted: format!("not-submitted-{attempt}"),
        finalize: format!("finalize-{attempt}"),
    };
    invocation.request.acceptance.input_boundary_digest =
        ordinary_cargo_input_boundary_digest_with_capture_budget(&invocation, capture_budget)
            .unwrap();
    invocation
}

fn run_budget_attempt(
    case: &FixtureRoot,
    name: &str,
    invocation: &mut OrdinaryCargoInvocation,
    capture_budget: &OrdinaryCargoCaptureBudget,
    daemon: &TestDaemon,
) -> Value {
    let request_digest = ordinary_cargo_request_digest(&invocation.request).unwrap();
    install_request_credentials(invocation, &daemon.instance, &request_digest);
    let callback_path = case
        .path
        .join("callbacks")
        .join(format!("callback-{name}.sock"));
    let listener = UnixListener::bind(&callback_path).unwrap();
    fs::set_permissions(&callback_path, fs::Permissions::from_mode(0o600)).unwrap();
    let (bootstrap, context) = create_bootstrap(
        case,
        name,
        invocation,
        &daemon.socket,
        &callback_path,
        &request_digest,
        Some(capture_budget),
    );
    let acquired = Arc::new(Mutex::new(None));
    let trace = Trace::new();
    trace.mark("scenario", name);
    let callbacks = start_callback_server(
        listener,
        Scenario::BudgetReuse,
        invocation.request.clone(),
        daemon.instance.clone(),
        context,
        acquired.clone(),
        trace.clone(),
    );
    run_child(&bootstrap, true, &trace);
    callbacks.join().unwrap();
    let report = trace.render();
    let acquired = acquired
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| panic!("submit had acquired identity\ntrace:\n{report}"));
    let expected_policy_digest = ordinary_cargo_capture_budget_digest(capture_budget).unwrap();
    assert_eq!(acquired["capture_budget"], json!(capture_budget));
    assert_eq!(
        acquired["capture_budget_digest"],
        json!(expected_policy_digest)
    );
    assert_eq!(
        acquired["input_boundary_digest"],
        json!(invocation.request.acceptance.input_boundary_digest)
    );
    acquired
}

#[test]
fn version_two_capture_budget_reuses_stable_generation_with_new_exact_boundaries() {
    let case = FixtureRoot::scenario("10-budget-stable-reuse");
    let storage_root = case.dir("storage");
    case.dir("callbacks");
    let first_budget = capture_policy(512 * 1024 * 1024, 128 * 1024 * 1024, 256 * 1024 * 1024);
    let second_budget = capture_policy(256 * 1024 * 1024, 96 * 1024 * 1024, 128 * 1024 * 1024);
    let base = invocation(&case, "budget-first");
    let mut first = stable_budget_invocation(base, "budget-first", &first_budget);
    let mut second = stable_budget_invocation(first.clone(), "budget-second", &second_budget);
    assert_eq!(
        first.request.acceptance.acceptance_reference,
        second.request.acceptance.acceptance_reference
    );
    assert_ne!(
        first.request.acceptance.input_boundary_digest,
        second.request.acceptance.input_boundary_digest
    );
    assert_ne!(first.target_dir, second.target_dir);
    assert_ne!(first.temp_dir, second.temp_dir);
    assert_ne!(first.capture_dir, second.capture_dir);
    assert!(validate_invocation_with_capture_budget(&first, Some(&first_budget)).is_ok());
    assert!(validate_invocation_with_capture_budget(&second, Some(&second_budget)).is_ok());

    let first_request_digest = ordinary_cargo_request_digest(&first.request).unwrap();
    let second_request_digest = ordinary_cargo_request_digest(&second.request).unwrap();
    let mut daemon = TestDaemon::start_many(
        &storage_root,
        vec![
            first.request.acquire.envelope.clone(),
            second.request.acquire.envelope.clone(),
        ],
        vec![first_request_digest, second_request_digest],
    );
    let first_acquired = run_budget_attempt(&case, "first", &mut first, &first_budget, &daemon);
    let second_acquired = run_budget_attempt(&case, "second", &mut second, &second_budget, &daemon);
    assert_eq!(first_acquired["reused"], json!(false));
    assert_eq!(second_acquired["reused"], json!(true));
    assert_eq!(first_acquired["slot_id"], second_acquired["slot_id"]);
    assert_eq!(first_acquired["generation"], second_acquired["generation"]);
    assert_ne!(
        first_acquired["input_boundary_digest"],
        second_acquired["input_boundary_digest"]
    );
    let inventory = daemon.client.inventory("budget-reuse-result").unwrap();
    assert_eq!(inventory.slots.len(), 1);
    assert_eq!(
        inventory.slots[0].cargo_reuse_state,
        Some(CargoReuseState::Reusable)
    );
    daemon.stop();
}

#[test]
fn private_ordinary_cargo_compiled_process_lifecycle_and_controls() {
    assert!(task_scratch_root().is_absolute());
    let bounded = Trace::new();
    for _ in 0..10_000 {
        bounded.mark("bound-control", "repeated bounded reason");
    }
    assert!(bounded.render().len() <= TRACE_MAX_BYTES + 64);
    assert!(bounded.render().ends_with("trace truncated at 64 KiB"));
    let controls = FixtureRoot::scenario("00-controls");
    let mut invocation = invocation(&controls, "00");
    invocation.request.acceptance.input_boundary_digest =
        ordinary_cargo_input_boundary_digest(&invocation).unwrap();
    assert!(validate_invocation(&invocation).is_ok());
    let mut changed_source = invocation.clone();
    changed_source.source_content_digest = digest(b"different source content");
    assert!(validate_invocation(&changed_source).is_err());
    let mut changed_root = invocation.clone();
    changed_root
        .request
        .root
        .consumer_root_id
        .push_str("-changed");
    assert!(validate_invocation(&changed_root).is_err());
    let mut changed_epoch = invocation.clone();
    changed_epoch
        .request
        .root
        .allocation_epoch
        .push_str("-changed");
    assert!(validate_invocation(&changed_epoch).is_err());
    let mut changed_argv = invocation.clone();
    changed_argv.argv.push("--changed".into());
    assert!(validate_invocation(&changed_argv).is_err());
    let mut reused_operation_id = invocation.clone();
    reused_operation_id.operation_ids.capture = reused_operation_id.operation_ids.submit.clone();
    assert!(validate_invocation(&reused_operation_id).is_err());
    let mut changed_environment = invocation;
    changed_environment
        .environment
        .insert("RUSTFLAGS".into(), Some("-Zunstable-options".into()));
    assert!(validate_invocation(&changed_environment).is_err());

    let cases = [
        (
            "01-pass",
            "01",
            Scenario::Passed,
            true,
            CargoReuseState::Reusable,
        ),
        (
            "02-fail",
            "02",
            Scenario::Failed,
            true,
            CargoReuseState::ColdOnly,
        ),
        (
            "03-no-submit",
            "03",
            Scenario::NotSubmitted,
            true,
            CargoReuseState::Empty,
        ),
        (
            "04-timeout",
            "04",
            Scenario::Timeout,
            false,
            CargoReuseState::Pending,
        ),
        (
            "05-lost-reply",
            "05",
            Scenario::LostReply,
            false,
            CargoReuseState::Pending,
        ),
        (
            "06-full-tuple",
            "06",
            Scenario::FullTupleChanged,
            false,
            CargoReuseState::Pending,
        ),
        (
            "07-capture-delayed",
            "07",
            Scenario::CaptureDelayed,
            false,
            CargoReuseState::Pending,
        ),
        (
            "08-capture-mismatch",
            "08",
            Scenario::CaptureMismatch,
            false,
            CargoReuseState::Pending,
        ),
        (
            "09-capture-lost",
            "09",
            Scenario::CaptureLost,
            false,
            CargoReuseState::Pending,
        ),
    ];
    for (name, number, scenario, success, state) in cases {
        one_scenario(name, number, scenario, success, state);
    }
}
