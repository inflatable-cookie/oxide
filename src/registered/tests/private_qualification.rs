//! Synthetic owner results, real compiled child and durable daemon. No Cargo
//! workload, Nucleus workflow, or benefit qualification is performed.
use super::*;
use crate::registered::cargo_freshness;
use crate::registered::graph_owner::{
    accept_graph_provision, issue_graph_provision, DepInfoMapping, GraphEdgeProvision,
    GraphOwnerAuthority, GraphProvision, GraphUnitProvision, RegisteredFileProvision, RerunMapping,
    WirePathTag, GRAPH_OWNER_ISSUER, GRAPH_OWNER_PROVISION_DOMAIN, GRAPH_OWNER_SCHEMA,
    GRAPH_OWNER_VERSION,
};
use crate::registered::qualification::*;
use crate::registered::runner_host::StorageIdentity;
use crate::storage::{
    CaptureReceipt, CooperativeCompletionEvidence, ProcessIdentityEvidence, SlotState,
    TerminalOutcome, TerminalRecord,
};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Child, Command, Stdio};
use std::time::{Instant, UNIX_EPOCH};

const OWNER_KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const STORAGE_KEY: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const MAP_KEY: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const SCOPE: &str = "synthetic-task029-private-proof";

struct ChildGuard(Child);
impl ChildGuard {
    fn finish(mut self, expected_success: bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(exit) = self.0.try_wait().unwrap() {
                assert_eq!(exit.success(), expected_success);
                return;
            }
            if Instant::now() >= deadline {
                panic!("own qualification child exceeded 30 seconds");
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
fn partition(f: &Fixture) -> PartitionKey {
    PartitionKey {
        project_component: "owned:core".into(),
        workflow: f.registration.id.clone(),
        cargo_provenance: "synthetic-cargo".into(),
        rustc_provenance: "synthetic-rustc".into(),
        host: "aarch64-apple-darwin".into(),
        target: "aarch64-apple-darwin".into(),
        profile: "full-debug".into(),
        features: vec![],
        config_digest: digest(b"config"),
        environment_digest: digest(b"env"),
        path_identity: digest(f.attempt.source_root.to_str().unwrap().as_bytes()),
    }
}
fn envelope(f: &Fixture, p: &PartitionKey, n: usize) -> RunEnvelope {
    let mut binding = f.attempt.binding.clone();
    binding.task_id = format!("task-{n}");
    binding.run_id = format!("run-{n}");
    binding.attempt_id = format!("attempt-{n}");
    RunEnvelope {
        binding,
        partition: p.clone(),
        admission_reference: format!("admission-{n}"),
        admission_credential: Credential::new(""),
    }
}
fn start(
    f: &Fixture,
    authority: &QualificationAuthority,
    mut envelope: RunEnvelope,
    n: usize,
) -> serde_json::Value {
    envelope.admission_credential = Credential::new(
        authority
            .receipt("oxide-admission-v1", &admission_subject(&envelope))
            .unwrap(),
    );
    serde_json::json!({"version":1,"operation":"run","id":format!("request-{n}"),"payload":{
        "registration":f.registration,"attempt": AttemptInput {
            binding:envelope.binding.clone(),root_id:f.attempt.root_id.clone(),allocation_epoch:n.to_string(),
            branch:format!("branch-{n}"),source_root:f.attempt.source_root.clone(),partition:f.attempt.partition.clone(),
            target:f.attempt.target.clone(),temp:f.attempt.temp.clone(),capture:f.attempt.capture.clone(),authorization:f.attempt.authorization.clone()
        },"acquire":{"envelope":envelope,"acquire_id":format!("acquire-{n}"),"required_bytes":1024*1024},"finalize_id":format!("finalize-{n}")}})
}
fn bootstrap(
    f: &Fixture,
    start: &serde_json::Value,
    socket: PathBuf,
    instance: &str,
    parent: PathBuf,
) -> Bootstrap {
    Bootstrap {
        version: 1,
        host_protocol: 1,
        storage_protocol: 1,
        map_pin: "d9ff17988c7eaa5cf57747bc55bfe4f138692f15".into(),
        expires_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 30,
        scope: SCOPE.into(),
        storage_instance_id: instance.into(),
        start_digest: canonical_digest(start).unwrap(),
        callback_key_hex: OWNER_KEY.into(),
        parent: Endpoint::capture(parent).unwrap(),
        storage: Endpoint::capture(socket).unwrap(),
        roots: vec![
            f.attempt.source_root.clone(),
            f.attempt.target.clone(),
            f.attempt.temp.clone(),
            f.attempt.capture.clone(),
        ],
        timing: None,
        graph_owner: None,
    }
}

struct ReadingMode {
    key: MacKey,
    start_digest: String,
    forged: bool,
    drop_dep_info: bool,
}

fn cargo_units(f: &Fixture) -> Vec<GraphUnitProvision> {
    let dep_info_anchor = "debug/deps/source.d";
    let package_file = |reference_path: &str, input_path: &str| RegisteredFileProvision {
        tag: WirePathTag::PackageRoot,
        path: reference_path.into(),
        input: source(input_path),
        association: observed_association(source(input_path), &[dep_info_anchor], None, None),
    };
    let generated_input = InputPath {
        root: InputRoot::Generated,
        path: "generated.rs".into(),
    };
    let output_anchor = "debug/.fingerprint/script-hash/output";
    let registry = f.owners.manifests.get("registry-receipt").unwrap();
    vec![
        GraphUnitProvision {
            id: "source-unit".into(),
            package_root: f.attempt.source_root.clone(),
            package_origin: InputRoot::Source,
            fingerprint_directory: "debug/.fingerprint/source-hash".into(),
            fingerprint_file: "lib-source.json".into(),
            directory_entries: set(&["lib-source.json"]),
            dep_info: vec![DepInfoMapping {
                path: dep_info_anchor.into(),
                files: vec![
                    package_file("src/lib.rs", "src.rs"),
                    RegisteredFileProvision {
                        tag: WirePathTag::BuildRoot,
                        path: "build/out.rs".into(),
                        input: generated_input.clone(),
                        association: observed_association(
                            generated_input,
                            &[dep_info_anchor],
                            Some("script-unit"),
                            None,
                        ),
                    },
                    package_file("Cargo.toml", "Cargo.toml"),
                ],
            }],
            rerun_files: Vec::new(),
            associations: Vec::new(),
            dependencies: vec![GraphEdgeProvision {
                package_id: 17,
                name: "script-unit".into(),
                public: false,
                unit: "script-unit".into(),
            }],
            effective_environment: f.registration.unit_environment["source-unit"].clone(),
            audited_precalculated_registry_receipt: None,
            additional_anchors: BTreeSet::new(),
        },
        GraphUnitProvision {
            id: "script-unit".into(),
            package_root: f.attempt.source_root.clone(),
            package_origin: InputRoot::Source,
            fingerprint_directory: "debug/.fingerprint/script-hash".into(),
            fingerprint_file: "run-build-script.json".into(),
            directory_entries: set(&["run-build-script.json", "output"]),
            dep_info: Vec::new(),
            rerun_files: vec![
                RerunMapping {
                    path: "build-input".into(),
                    association: observed_association(
                        source("build-input"),
                        &[output_anchor],
                        None,
                        None,
                    ),
                },
                RerunMapping {
                    path: "data".into(),
                    association: observed_association(source("data"), &[output_anchor], None, None),
                },
            ],
            associations: vec![
                observed_association(
                    InputPath {
                        root: InputRoot::Immutable("registry-receipt".into()),
                        path: "sqlite.c".into(),
                    },
                    &[output_anchor],
                    None,
                    Some("tool-receipt"),
                ),
                observed_association(
                    InputPath {
                        root: InputRoot::Immutable("tool-receipt".into()),
                        path: "header.h".into(),
                    },
                    &[output_anchor],
                    None,
                    Some("tool-receipt"),
                ),
            ],
            dependencies: Vec::new(),
            effective_environment: f.registration.unit_environment["script-unit"].clone(),
            audited_precalculated_registry_receipt: None,
            additional_anchors: BTreeSet::new(),
        },
        GraphUnitProvision {
            id: "registry-unit".into(),
            package_root: registry.root.clone(),
            package_origin: InputRoot::Immutable("registry-receipt".into()),
            fingerprint_directory: "debug/.fingerprint/registry-hash".into(),
            fingerprint_file: "lib-registry.json".into(),
            directory_entries: set(&["lib-registry.json"]),
            dep_info: Vec::new(),
            rerun_files: Vec::new(),
            associations: Vec::new(),
            dependencies: Vec::new(),
            effective_environment: f.registration.unit_environment["registry-unit"].clone(),
            audited_precalculated_registry_receipt: Some("registry-receipt".into()),
            additional_anchors: BTreeSet::new(),
        },
    ]
}

fn unsigned_graph(f: &Fixture) -> GraphProvision {
    GraphProvision {
        version: GRAPH_OWNER_VERSION,
        schema: GRAPH_OWNER_SCHEMA.into(),
        issuer: GRAPH_OWNER_ISSUER.into(),
        cargo_version: cargo_freshness::CARGO_VERSION.into(),
        cargo_source_commit: cargo_freshness::CARGO_SOURCE_COMMIT.into(),
        graph_digest: f.registration.graph_digest.clone(),
        units: cargo_units(f),
        receipt: String::new(),
    }
}

fn issue_for_attempt(
    f: &Fixture,
    key: &MacKey,
    start_digest: &str,
    attempt: &Attempt,
    storage: &StorageIdentity,
    mut provision: GraphProvision,
) -> GraphProvision {
    if provision.units.is_empty() {
        provision.units = cargo_units(f);
    }
    issue_graph_provision(
        key,
        SCOPE,
        &storage.storage_instance_id,
        start_digest,
        attempt,
        storage,
        provision,
    )
    .unwrap()
}

fn write_bootstrap(path: &Path, b: &Bootstrap) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    serde_json::to_writer(&mut file, b).unwrap();
    file.sync_all().unwrap();
}
fn spawn_child(file: &Path) -> ChildGuard {
    ChildGuard(
        Command::new(
            std::env::var_os("OXIDE_QUALIFICATION_CHILD").expect("Effigy compiled the child"),
        )
        .arg(file)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap(),
    )
}
fn accept_child(listener: &UnixListener) -> UnixStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "child did not connect");
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("accept: {e}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream
}
fn read_frame_line(reader: &mut BufReader<UnixStream>) -> serde_json::Value {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(!line.is_empty(), "host closed without a frame");
    serde_json::from_str(&line).unwrap()
}
fn synthetic_settlement(
    launch: &Launch,
    storage: &serde_json::Value,
    authority: &QualificationAuthority,
) -> (SettlementEvidence, serde_json::Value) {
    let c = CooperativeCompletionEvidence {
        lease_id: launch.lease_id.clone(),
        slot_id: storage["slot_id"].as_str().unwrap().into(),
        slot_path: launch.slot.to_str().unwrap().into(),
        temp_dir: launch.temp.to_str().unwrap().into(),
        capture_dir: launch.capture.to_str().unwrap().into(),
        workflow: launch.binding.workflow.clone(),
        workflow_definition_digest: digest(b"synthetic cooperative definition"),
        repository_head: launch.binding.commit.clone(),
        fixture_source_digest: launch.binding.source_tree_digest.clone(),
        host_run_id: format!("synthetic-host-{}", launch.binding.run_id),
        run_id: launch.binding.run_id.clone(),
        attempt_id: launch.binding.attempt_id.clone(),
        processes: vec![ProcessIdentityEvidence {
            pid: std::process::id(),
            start_identity: digest(b"synthetic process claim"),
        }],
        free_space_samples: vec![32 * 1024 * 1024 * 1024],
        exit_code: 0,
        stdout_digest: digest(b"synthetic stdout"),
        stderr_digest: digest(b""),
        capture_reference: format!("synthetic-capture-{}", launch.binding.run_id),
        capture_digest: digest(b"synthetic capture"),
        signature: String::new(),
    };
    let mut e = SettlementEvidence {
        binding: launch.binding.clone(),
        consumer_credential: Credential::new(""),
        terminal: Some(TerminalRecord {
            run_id: launch.binding.run_id.clone(),
            outcome: TerminalOutcome::Passed,
            credential: Credential::new(""),
        }),
        capture: Some(CaptureReceipt {
            reference: c.capture_reference.clone(),
            digest: c.capture_digest.clone(),
            host_run_id: c.host_run_id.clone(),
            run_id: launch.binding.run_id.clone(),
            attempt_id: launch.binding.attempt_id.clone(),
            credential: Credential::new(""),
        }),
        containment: ContainmentEvidence::CooperativeWorkflow(c.clone()),
    };
    let subject = settlement_subject(&e).unwrap();
    e.consumer_credential =
        Credential::new(authority.receipt("oxide-settlement-v1", &subject).unwrap());
    e.terminal.as_mut().unwrap().credential =
        Credential::new(authority.receipt("oxide-terminal-v1", &subject).unwrap());
    e.capture.as_mut().unwrap().credential =
        Credential::new(authority.receipt("oxide-capture-v1", &subject).unwrap());
    let claims = serde_json::json!({"consumer_credential":e.consumer_credential.expose(),
        "terminal":{"run_id":e.binding.run_id,"outcome":"passed","credential":e.terminal.as_ref().unwrap().credential.expose()},
        "capture":{"reference":c.capture_reference,"digest":c.capture_digest,"host_run_id":c.host_run_id,"run_id":c.run_id,"attempt_id":c.attempt_id,
            "credential":e.capture.as_ref().unwrap().credential.expose()},
        "completion":{"workflow_definition_digest":c.workflow_definition_digest,"repository_head":c.repository_head,"fixture_source_digest":c.fixture_source_digest,
            "host_run_id":c.host_run_id,"processes":c.processes,"free_space_samples":c.free_space_samples,"exit_code":c.exit_code,"stdout_digest":c.stdout_digest,
            "stderr_digest":c.stderr_digest,"capture_reference":c.capture_reference,"capture_digest":c.capture_digest}});
    (e, claims)
}

fn parent(
    mut stream: UnixStream,
    f: &Fixture,
    request: &serde_json::Value,
    auth: &CallbackAuthenticator,
    authority: &QualificationAuthority,
    map_key: &MacKey,
    interrupt: bool,
    reading: Option<&ReadingMode>,
) -> (serde_json::Value, Vec<serde_json::Value>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    serde_json::to_writer(&mut stream, request).unwrap();
    stream.write_all(b"\n").unwrap();
    let mut callbacks = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "child closed before result");
        let frame: serde_json::Value = serde_json::from_str(&line).unwrap();
        if interrupt {
            return (serde_json::json!({"local":"interrupted"}), vec![frame]);
        }
        if frame["operation"] == "result" || frame["operation"] == "error" {
            return (frame, callbacks);
        }
        let owner = frame["owner"].as_str().unwrap();
        let payload = &frame["payload"];
        let id = frame["id"].as_str().unwrap();
        let subject = serde_json::json!({"id":frame["id"],"owner":frame["owner"],"storage":frame["storage"],"partition":frame["partition"],"payload":payload});
        assert_eq!(frame["subject_digest"], canonical_digest(&subject).unwrap());
        let result = match owner {
            "registration.authenticate" => serde_json::to_value(
                f.owners
                    .authenticate(
                        &serde_json::from_value(payload["registration"].clone()).unwrap(),
                        &serde_json::from_value(payload["attempt"].clone()).unwrap(),
                    )
                    .unwrap(),
            )
            .unwrap(),
            "registration.audit" => {
                f.owners
                    .audit(
                        &serde_json::from_value(payload["registration"].clone()).unwrap(),
                        &serde_json::from_value(payload["source"].clone()).unwrap(),
                    )
                    .unwrap();
                serde_json::Value::Null
            }
            "input.immutable" => serde_json::to_value(
                f.owners
                    .immutable(
                        payload["receipt"].as_str().unwrap(),
                        &serde_json::from_value(payload["attempt"].clone()).unwrap(),
                    )
                    .unwrap(),
            )
            .unwrap(),
            "input.units" => {
                assert!(
                    reading.is_none(),
                    "collector mode must not fall back to parent UnitEvidence"
                );
                serde_json::to_value(
                    f.owners
                        .units(
                            &serde_json::from_value(payload["registration"].clone()).unwrap(),
                            &serde_json::from_value(payload["attempt"].clone()).unwrap(),
                        )
                        .unwrap(),
                )
                .unwrap()
            }
            "input.graph_owner" => {
                let reading =
                    reading.expect("graph owner callback requires installed reading mode");
                let attempt: Attempt = serde_json::from_value(payload["attempt"].clone()).unwrap();
                let storage = StorageIdentity {
                    slot_id: frame["storage"]["slot_id"].as_str().unwrap().into(),
                    storage_instance_id: frame["storage"]["storage_instance_id"]
                        .as_str()
                        .unwrap()
                        .into(),
                    generation: frame["storage"]["generation"].as_str().unwrap().into(),
                    reused: frame["storage"]["reused"].as_bool(),
                };
                let mut provision = unsigned_graph(f);
                if reading.drop_dep_info {
                    provision.units[0].dep_info.clear();
                }
                let mut issued = issue_for_attempt(
                    f,
                    &reading.key,
                    &reading.start_digest,
                    &attempt,
                    &storage,
                    provision,
                );
                if reading.forged {
                    issued.receipt = MacKey::from_hex(STORAGE_KEY)
                        .unwrap()
                        .sign(
                            GRAPH_OWNER_PROVISION_DOMAIN,
                            &[
                                SCOPE,
                                &storage.storage_instance_id,
                                &reading.start_digest,
                                "0",
                            ],
                        )
                        .unwrap();
                }
                serde_json::to_value(&issued).unwrap()
            }
            "execution.execute" => {
                let launch: Launch = serde_json::from_value(payload["launch"].clone()).unwrap();
                if reading.is_some() {
                    write_cargo_slot_files(&launch.slot);
                    fs::write(launch.slot.join("generated.rs"), b"generated.rs").unwrap();
                    serde_json::to_value(ExecutionReceipt::Completed(
                        "synthetic-capture-receipt".into(),
                    ))
                    .unwrap()
                } else {
                    serde_json::to_value(f.owners.execute(&launch).unwrap()).unwrap()
                }
            }
            "execution.verify_success" => {
                let launch: Launch = serde_json::from_value(payload["launch"].clone()).unwrap();
                f.owners
                    .verify_success(
                        &serde_json::from_value(payload["registration"].clone()).unwrap(),
                        &launch,
                        payload["receipt"].as_str().unwrap(),
                        &serde_json::from_value(payload["inputs"].clone()).unwrap(),
                    )
                    .unwrap();
                let (e, claims) = synthetic_settlement(&launch, &frame["storage"], authority);
                assert!(authority.verify_settlement(&e));
                for change in 0..4 {
                    let mut bad = e.clone();
                    match change {
                        0 => bad.binding.run_id.push('x'),
                        1 => bad.capture.as_mut().unwrap().digest = digest(b"forged"),
                        2 => bad.terminal.as_mut().unwrap().outcome = TerminalOutcome::Failed,
                        _ => {
                            if let ContainmentEvidence::CooperativeWorkflow(c) =
                                &mut bad.containment
                            {
                                c.slot_id.push('x');
                            }
                        }
                    }
                    assert!(!authority.verify_settlement(&bad));
                }
                claims
            }
            "input.seal_map" | "input.verify_map" => {
                let storage = &frame["storage"];
                let partition = canonical_digest(&frame["partition"]).unwrap();
                let registration =
                    canonical_digest(&serde_json::to_value(&f.registration).unwrap()).unwrap();
                let fields = [
                    SCOPE,
                    storage["storage_instance_id"].as_str().unwrap(),
                    storage["slot_id"].as_str().unwrap(),
                    storage["generation"].as_str().unwrap(),
                    &partition,
                    &registration,
                    payload["map_digest"].as_str().unwrap(),
                ];
                if owner == "input.seal_map" {
                    serde_json::json!({"seal":map_key.sign("oxide-map-v1",&fields).unwrap()})
                } else {
                    assert!(map_key.verify(
                        "oxide-map-v1",
                        &fields,
                        payload["seal"].as_str().unwrap()
                    ));
                    assert!(!MacKey::from_hex(OWNER_KEY).unwrap().verify(
                        "oxide-map-v1",
                        &fields,
                        payload["seal"].as_str().unwrap()
                    ));
                    serde_json::Value::Null
                }
            }
            "execution.finalize_idle" => serde_json::Value::Null,
            other => panic!("unexpected owner {other}"),
        };
        let rd = canonical_digest(&result).unwrap();
        let receipt = auth
            .receipt(
                owner,
                id,
                frame["subject_digest"].as_str().unwrap(),
                frame["policy_digest"].as_str().unwrap(),
                &rd,
            )
            .unwrap();
        let reply = serde_json::json!({"version":1,"operation":"callback_result","id":id,"owner":owner,"subject_digest":frame["subject_digest"],"policy_digest":frame["policy_digest"],"result_digest":rd,"receipt":receipt,"result":result});
        serde_json::to_writer(&mut stream, &reply).unwrap();
        stream.write_all(b"\n").unwrap();
        callbacks.push(frame);
    }
}

#[test]
fn actual_child_cold_warm_and_interrupted_hold() {
    let started = Instant::now();
    let mut f = Fixture::new();
    f.attempt.binding.commit = "1".repeat(40);
    f.attempt.allocation_epoch = "1".into();
    let p = partition(&f);
    f.attempt.partition = digest(&serde_json::to_vec(&p).unwrap());
    let allowed = (1..=3).map(|n| envelope(&f, &p, n)).collect::<Vec<_>>();
    let authority = Arc::new(
        QualificationAuthority::new(
            MacKey::from_hex(STORAGE_KEY).unwrap(),
            SCOPE.into(),
            allowed.clone(),
        )
        .unwrap(),
    );
    let root = f.base.join("store");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let daemon = Daemon::bind(
        DaemonConfig {
            root: root.clone(),
            budget: Budget {
                cap_bytes: 16 * 1024 * 1024,
                free_reserve_bytes: 0,
            },
        },
        authority.clone(),
        Arc::new(BridgeFreeSpace),
    )
    .unwrap();
    let instance = daemon.storage_instance_id().to_owned();
    authority.install_instance(&instance).unwrap();
    assert!(authority.install_instance(&"a".repeat(64)).is_err());
    let (socket, guard) = BridgeDaemon::start(daemon);
    let client = Client::new(socket.clone());
    let parent_path = f.base.join("parent.sock");
    let listener = UnixListener::bind(&parent_path).unwrap();
    fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o600)).unwrap();
    let map_key = MacKey::from_hex(MAP_KEY).unwrap();
    let mut previous = None;
    // Three real child sessions; a prior macOS harness failure consumed the
    // fourth author attempt. Reviewers use their own separately bounded scope.
    for (scenario, n, interrupt) in [(1, 1, false), (2, 2, false), (3, 3, true)] {
        let scenario_started = Instant::now();
        let request = start(&f, &authority, allowed[n - 1].clone(), n);
        let b = bootstrap(&f, &request, socket.clone(), &instance, parent_path.clone());
        let auth = CallbackAuthenticator::new(
            MacKey::from_hex(OWNER_KEY).unwrap(),
            b.callback_context().unwrap(),
        )
        .unwrap();
        let file = f.base.join(format!("bootstrap-{scenario}.json"));
        write_bootstrap(&file, &b);
        let child = spawn_child(&file);
        let stream = accept_child(&listener);
        let (response, callbacks) = parent(
            stream, &f, &request, &auth, &authority, &map_key, interrupt, None,
        );
        child.finish(scenario <= 2);
        assert!(!file.exists(), "bootstrap consumed before effects");
        if scenario <= 2 {
            assert_eq!(response["result"]["status"], "finished");
            assert_eq!(response["result"]["outcome"], "published");
            assert_eq!(response["result"]["storage"]["reused"], scenario == 2);
            if let Some(old) = &previous {
                assert_eq!(old, &response["result"]["storage"]["slot_id"]);
            }
            previous = Some(response["result"]["storage"]["slot_id"].clone());
            assert!(callbacks.iter().any(|c| c["owner"] == "input.seal_map"));
            if scenario == 2 {
                assert!(callbacks.iter().any(|c| c["owner"] == "input.verify_map"));
            }
            assert_eq!(
                client.inventory("proof").unwrap().slots[0].state,
                SlotState::Idle
            );
        } else {
            assert_eq!(response["local"], "interrupted");
            assert_eq!(callbacks.len(), 1);
            assert_eq!(
                client.inventory("held").unwrap().slots[0].state,
                SlotState::Leased
            );
        }
        assert!(scenario_started.elapsed() < Duration::from_secs(30));
        assert!(callbacks.iter().all(|c| !has_token_field(c)));
        let log = serde_json::to_vec(&callbacks).unwrap();
        for key in [OWNER_KEY, STORAGE_KEY, MAP_KEY] {
            assert!(!log.windows(key.len()).any(|w| w == key.as_bytes()));
        }
    }
    guard.stop();
    let daemon = Daemon::bind(
        DaemonConfig {
            root: root.clone(),
            budget: Budget {
                cap_bytes: 16 * 1024 * 1024,
                free_reserve_bytes: 0,
            },
        },
        authority.clone(),
        Arc::new(BridgeFreeSpace),
    )
    .unwrap();
    assert_eq!(daemon.storage_instance_id(), instance);
    let (_, guard) = BridgeDaemon::start(daemon);
    assert_eq!(
        client.inventory("restart").unwrap().slots[0].state,
        SlotState::Uncertain
    );
    assert!(client
        .acquire(AcquireRequest {
            envelope: serde_json::from_value(
                start(&f, &authority, allowed[2].clone(), 3)["payload"]["acquire"]["envelope"]
                    .clone()
            )
            .unwrap(),
            acquire_id: "acquire-3".into(),
            required_bytes: 1024 * 1024
        })
        .unwrap()
        .token
        .is_none());
    guard.stop();
    assert!(started.elapsed() < Duration::from_secs(120));
    println!("three actual child sessions: synthetic cold+warm published; interrupted owner held across restart; no Cargo or Nucleus benefit");
}

#[test]
fn framing_and_authenticated_receipts_refuse_forgery_stale_context_and_tuple() {
    let v: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/qualification-auth-v1.json")).unwrap();
    let key = MacKey::from_hex(v["key_hex"].as_str().unwrap()).unwrap();
    let domain = v["domain"].as_str().unwrap();
    let fields = v["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect::<Vec<_>>();
    let bytes = mac_frame(domain, &fields).unwrap();
    assert_eq!(
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        v["frame_hex"]
    );
    let receipt = key.sign(domain, &fields).unwrap();
    assert_eq!(receipt, v["receipt"]);
    assert!(key.verify(domain, &fields, &receipt));
    assert!(!key.verify("oxide-settlement-v1", &fields, &receipt));
    for index in 0..fields.len() {
        let mut changed = fields.clone();
        changed[index] = "changed";
        assert!(!key.verify(domain, &changed, &receipt));
    }
    assert!(!MacKey::from_hex(OWNER_KEY)
        .unwrap()
        .verify(domain, &fields, &receipt));
    assert!(!key.verify(domain, &fields, "hmac-sha256-v1:00"));
    assert!(MacKey::from_hex("").is_err());
    assert_eq!(
        canonical_digest(&v["canonical_subject"]).unwrap(),
        v["canonical_digest"]
    );
    let auth = CallbackAuthenticator::new(key, "exact-start-context".into()).unwrap();
    let r = auth
        .receipt("input.verify_map", "1", fields[3], fields[4], fields[5])
        .unwrap();
    assert!(auth.verify("input.verify_map", "1", fields[3], fields[4], fields[5], &r));
    assert!(!auth.verify("input.verify_map", "2", fields[3], fields[4], fields[5], &r));
    let stale = CallbackAuthenticator::new(
        MacKey::from_hex(v["key_hex"].as_str().unwrap()).unwrap(),
        "stale-start-context".into(),
    )
    .unwrap();
    assert!(!stale.verify("input.verify_map", "1", fields[3], fields[4], fields[5], &r));
    let map_key = MacKey::from_hex(MAP_KEY).unwrap();
    let tuple = [SCOPE, "instance", "slot", "1", "policy", "map"];
    let seal = map_key.sign("oxide-map-v1", &tuple).unwrap();
    for i in 1..tuple.len() {
        let mut bad = tuple;
        bad[i] = "changed";
        assert!(!map_key.verify("oxide-map-v1", &bad, &seal));
    }
    assert!(!MacKey::from_hex(OWNER_KEY)
        .unwrap()
        .verify("oxide-map-v1", &tuple, &seal));
    assert!(mac_frame(domain, &[&"x".repeat(1024 * 1024)]).is_err());
    assert!(!format!("{auth:?}").contains(v["key_hex"].as_str().unwrap()));
}

#[test]
fn unsafe_malformed_expired_replaced_and_unapproved_bootstrap_refuse_before_effects() {
    let mut f = Fixture::new();
    f.attempt.binding.commit = "1".repeat(40);
    f.attempt.allocation_epoch = "1".into();
    let p = partition(&f);
    let allowed = vec![envelope(&f, &p, 1)];
    let authority = Arc::new(
        QualificationAuthority::new(
            MacKey::from_hex(STORAGE_KEY).unwrap(),
            SCOPE.into(),
            allowed.clone(),
        )
        .unwrap(),
    );
    let root = f.base.join("store");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let daemon = Daemon::bind(
        DaemonConfig {
            root,
            budget: Budget {
                cap_bytes: 16 * 1024 * 1024,
                free_reserve_bytes: 0,
            },
        },
        authority.clone(),
        Arc::new(BridgeFreeSpace),
    )
    .unwrap();
    let instance = daemon.storage_instance_id().to_owned();
    authority.install_instance(&instance).unwrap();
    let (socket, guard) = BridgeDaemon::start(daemon);
    let parent_path = f.base.join("parent.sock");
    let listener = UnixListener::bind(&parent_path).unwrap();
    fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut admitted = allowed[0].clone();
    admitted.admission_credential = Credential::new(
        authority
            .receipt("oxide-admission-v1", &admission_subject(&admitted))
            .unwrap(),
    );
    assert!(authority.verify_admission(&admitted));
    for field in 0..3 {
        let mut changed = admitted.clone();
        match field {
            0 => changed.binding.run_id.push('x'),
            1 => changed.partition.profile.push('x'),
            _ => changed.admission_reference.push('x'),
        }
        changed.admission_credential = Credential::new(
            authority
                .receipt("oxide-admission-v1", &admission_subject(&changed))
                .unwrap(),
        );
        assert!(!authority.verify_admission(&changed));
    }
    let missing = QualificationAuthority::new(
        MacKey::from_hex(OWNER_KEY).unwrap(),
        SCOPE.into(),
        allowed.clone(),
    )
    .unwrap();
    assert!(!missing.verify_admission(&admitted));
    missing.install_instance(&instance).unwrap();
    assert!(!missing.verify_admission(&admitted));
    let recreated = QualificationAuthority::new(
        MacKey::from_hex(STORAGE_KEY).unwrap(),
        SCOPE.into(),
        allowed.clone(),
    )
    .unwrap();
    recreated.install_instance(&"c".repeat(64)).unwrap();
    assert!(!recreated.verify_admission(&admitted));
    admitted.admission_credential = Credential::new("");
    assert!(!authority.verify_admission(&admitted));
    let request = start(&f, &authority, allowed[0].clone(), 1);
    let base = bootstrap(&f, &request, socket.clone(), &instance, parent_path.clone());
    let value = serde_json::to_value(&base).unwrap();
    for (n, (field, wrong)) in [
        ("version", serde_json::json!(2)),
        ("host_protocol", serde_json::json!(2)),
        ("storage_protocol", serde_json::json!(2)),
        ("expires_unix_seconds", serde_json::json!(0)),
        ("callback_key_hex", serde_json::json!("malformed")),
        ("start_digest", serde_json::json!("malformed")),
        ("map_pin", serde_json::json!("older")),
        ("storage_instance_id", serde_json::json!("missing")),
        ("unexpected", serde_json::json!(true)),
    ]
    .into_iter()
    .enumerate()
    {
        let mut changed = value.clone();
        changed[field] = wrong;
        let file = f.base.join(format!("invalid-{n}.json"));
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&file)
            .unwrap();
        serde_json::to_writer(&mut out, &changed).unwrap();
        drop(out);
        assert!(serve_bootstrap(&file).is_err());
        assert!(file.exists());
    }
    let unsafe_file = f.base.join("unsafe.json");
    write_bootstrap(&unsafe_file, &base);
    fs::set_permissions(&unsafe_file, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(serve_bootstrap(&unsafe_file).is_err());
    let linked_file = f.base.join("linked.json");
    write_bootstrap(&linked_file, &base);
    fs::hard_link(&linked_file, f.base.join("alias.json")).unwrap();
    assert!(serve_bootstrap(&linked_file).is_err());
    let symlink_file = f.base.join("symlink.json");
    symlink(&linked_file, &symlink_file).unwrap();
    assert!(serve_bootstrap(&symlink_file).is_err());
    assert!(serve_bootstrap(&f.base.join("missing.json")).is_err());
    fs::remove_file(&parent_path).unwrap();
    let replacement = UnixListener::bind(&parent_path).unwrap();
    fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o600)).unwrap();
    let replaced = f.base.join("replaced.json");
    write_bootstrap(&replaced, &base);
    assert!(serve_bootstrap(&replaced).is_err());
    let mut unapproved = bootstrap(&f, &request, socket.clone(), &instance, parent_path.clone());
    unapproved.storage.path = f.base.join("unapproved.sock");
    let wrong = f.base.join("unapproved.json");
    write_bootstrap(&wrong, &unapproved);
    assert!(serve_bootstrap(&wrong).is_err());
    assert!(Client::new(socket)
        .inventory("no-effects")
        .unwrap()
        .slots
        .is_empty());
    drop(replacement);
    drop(listener);
    guard.stop();
}

#[test]
fn qualified_timing_refuses_unsupported_forged_or_moved_windows_before_connect() {
    let mut f = Fixture::new();
    f.attempt.binding.commit = "1".repeat(40);
    f.attempt.allocation_epoch = "1".into();
    let p = partition(&f);
    let allowed = vec![envelope(&f, &p, 1), envelope(&f, &p, 2)];
    let authority = QualificationAuthority::new(
        MacKey::from_hex(STORAGE_KEY).unwrap(),
        SCOPE.into(),
        allowed.clone(),
    )
    .unwrap();
    let instance = "a".repeat(64);
    authority.install_instance(&instance).unwrap();
    let storage = f.base.join("storage.sock");
    let _storage = UnixListener::bind(&storage).unwrap();
    fs::set_permissions(&storage, fs::Permissions::from_mode(0o600)).unwrap();
    let parent_path = f.base.join("parent.sock");
    let listener = UnixListener::bind(&parent_path).unwrap();
    fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let request = start(&f, &authority, allowed[0].clone(), 1);
    let other_request = start(&f, &authority, allowed[1].clone(), 2);
    let mut base = bootstrap(
        &f,
        &request,
        storage.clone(),
        &instance,
        parent_path.clone(),
    );
    base.timing = Some(
        TimingAuthority::issue(&base, Duration::from_secs(1), Duration::from_secs(2)).unwrap(),
    );
    let other = bootstrap(
        &f,
        &other_request,
        storage.clone(),
        &instance,
        parent_path.clone(),
    );
    let reminted =
        TimingAuthority::issue(&other, Duration::from_secs(1), Duration::from_secs(2)).unwrap();
    let value = serde_json::to_value(&base).unwrap();
    let mut variants = Vec::new();
    let mut change = |edit: &dyn Fn(&mut serde_json::Value)| {
        let mut changed = value.clone();
        edit(&mut changed);
        variants.push(changed);
    };
    change(&|v| v["timing"]["policy"] = serde_json::json!("oxide-qualification-timing-v0"));
    change(&|v| v["timing"]["callback_ms"] = serde_json::json!(0));
    change(&|v| v["timing"]["remaining_ms"] = serde_json::json!(2_700_001));
    change(&|v| v["timing"]["remaining_ms"] = serde_json::json!(1_500));
    change(&|v| v["timing"] = serde_json::to_value(&reminted).unwrap());
    change(&|v| {
        v["timing"].as_object_mut().unwrap().remove("receipt");
    });
    change(&|v| v["timing"]["unexpected"] = serde_json::json!(true));
    change(&|v| v["timing"]["receipt"] = serde_json::json!("hmac-sha256-v1:00"));
    for (n, changed) in variants.iter().enumerate() {
        let file = f.base.join(format!("timing-{n}.json"));
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&file)
            .unwrap();
        serde_json::to_writer(&mut out, changed).unwrap();
        drop(out);
        assert!(serve_bootstrap(&file).is_err(), "variant {n} must refuse");
        assert!(file.exists(), "variant {n} must not consume the bootstrap");
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "refused timing never connects to the parent"
    );
}

#[test]
fn qualified_callback_timeout_after_acquire_holds_lease_and_refuses_replay() {
    let started = Instant::now();
    let mut f = Fixture::new();
    f.attempt.binding.commit = "1".repeat(40);
    f.attempt.allocation_epoch = "1".into();
    let p = partition(&f);
    f.attempt.partition = digest(&serde_json::to_vec(&p).unwrap());
    let allowed = vec![envelope(&f, &p, 1)];
    let authority = Arc::new(
        QualificationAuthority::new(
            MacKey::from_hex(STORAGE_KEY).unwrap(),
            SCOPE.into(),
            allowed.clone(),
        )
        .unwrap(),
    );
    let root = f.base.join("store");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let daemon = Daemon::bind(
        DaemonConfig {
            root,
            budget: Budget {
                cap_bytes: 16 * 1024 * 1024,
                free_reserve_bytes: 0,
            },
        },
        authority.clone(),
        Arc::new(BridgeFreeSpace),
    )
    .unwrap();
    let instance = daemon.storage_instance_id().to_owned();
    authority.install_instance(&instance).unwrap();
    let (socket, guard) = BridgeDaemon::start(daemon);
    let client = Client::new(socket.clone());
    let parent_path = f.base.join("parent.sock");
    let listener = UnixListener::bind(&parent_path).unwrap();
    fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o600)).unwrap();

    // Synthetic 1s callback and 4s remaining window. The parent withholds the
    // first owner reply after acquisition, so only the host timeout may follow.
    let request = start(&f, &authority, allowed[0].clone(), 1);
    let mut timed = bootstrap(&f, &request, socket.clone(), &instance, parent_path.clone());
    timed.timing = Some(
        TimingAuthority::issue(&timed, Duration::from_secs(1), Duration::from_secs(4)).unwrap(),
    );
    let file = f.base.join("bootstrap-timeout.json");
    write_bootstrap(&file, &timed);
    let child = spawn_child(&file);
    let mut stream = accept_child(&listener);
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    serde_json::to_writer(&mut stream, &request).unwrap();
    stream.write_all(b"\n").unwrap();
    let callback = read_frame_line(&mut reader);
    assert_eq!(callback["owner"], "registration.authenticate");
    let error = read_frame_line(&mut reader);
    assert_eq!(error["operation"], "error");
    assert_eq!(error["code"], "timeout");
    assert!(error.get("result").is_none(), "no writer token or result");
    child.finish(false);
    assert!(!file.exists(), "bootstrap consumed before effects");
    assert_eq!(
        client.inventory("timeout").unwrap().slots[0].state,
        SlotState::Leased
    );

    // A fresh bootstrap for the same approved start is refused before acquire.
    let replay_file = f.base.join("bootstrap-replay.json");
    let mut replay = bootstrap(&f, &request, socket.clone(), &instance, parent_path.clone());
    replay.timing = Some(
        TimingAuthority::issue(&replay, Duration::from_secs(1), Duration::from_secs(4)).unwrap(),
    );
    write_bootstrap(&replay_file, &replay);
    let child = spawn_child(&replay_file);
    let mut stream = accept_child(&listener);
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    serde_json::to_writer(&mut stream, &request).unwrap();
    stream.write_all(b"\n").unwrap();
    let refusal = read_frame_line(&mut reader);
    assert_eq!(refusal["operation"], "error");
    assert_eq!(refusal["code"], "owner_rejected");
    child.finish(false);
    assert_eq!(
        client.inventory("replay").unwrap().slots[0].state,
        SlotState::Leased
    );
    assert!(started.elapsed() < Duration::from_secs(30));
    guard.stop();
}

fn test_storage() -> StorageIdentity {
    StorageIdentity {
        slot_id: "slot-0000000000000001".into(),
        storage_instance_id: "aa".repeat(32),
        generation: "1".into(),
        reused: Some(false),
    }
}

fn issued_fixture_graph(f: &Fixture, key: &MacKey, start_digest: &str) -> GraphProvision {
    issue_for_attempt(
        f,
        key,
        start_digest,
        &f.attempt,
        &test_storage(),
        unsigned_graph(f),
    )
}

#[test]
fn graph_owner_accepts_synthetic_context_and_shares_one_budget() {
    let fixture = Fixture::new();
    install_cargo_freshness(&fixture);
    let key = MacKey::from_hex(OWNER_KEY).unwrap();
    let start_digest = format!("sha256:{}", "ab".repeat(32));
    let provision = issued_fixture_graph(&fixture, &key, &start_digest);
    let mut budget = evidence_budget(Limits::NUCLEUS.collection_bytes);
    charge_budget(&mut budget, 1024).unwrap();
    let used_before = budget_used(&budget);
    let context = accept_graph_provision(
        provision,
        &fixture.registration,
        &fixture.attempt,
        &fixture.owners.manifests,
        &mut budget,
        &key,
        SCOPE,
        &start_digest,
        &test_storage(),
    )
    .unwrap();
    assert_eq!(context.build_root, fixture.attempt.slot);
    assert_eq!(context.cargo_version, cargo_freshness::CARGO_VERSION);
    let used_after_provision = budget_used(&budget);
    assert!(
        used_after_provision > used_before,
        "provision decode charges the shared budget"
    );
    let units = cargo_freshness::collect(
        &fixture.registration,
        &fixture.attempt,
        &fixture.owners.manifests,
        &mut budget,
        &context,
    )
    .unwrap();
    assert_eq!(units.len(), 3);
    assert!(budget_used(&budget) > used_after_provision);
    let source = units.iter().find(|unit| unit.id == "source-unit").unwrap();
    assert_eq!(
        source.locals.iter().find_map(|local| match local {
            LocalInput::RerunEnvironment { name, value } if name == "ABSENT" => Some(value),
            _ => None,
        }),
        Some(&None)
    );
}

#[test]
fn graph_owner_refuses_forged_mismatched_and_untrusted_fields() {
    let fixture = Fixture::new();
    install_cargo_freshness(&fixture);
    let key = MacKey::from_hex(OWNER_KEY).unwrap();
    let start_digest = format!("sha256:{}", "cd".repeat(32));
    let accept = |provision: GraphProvision, attempt: &Attempt, storage: &StorageIdentity| {
        let mut budget = evidence_budget(Limits::NUCLEUS.collection_bytes);
        accept_graph_provision(
            provision,
            &fixture.registration,
            attempt,
            &fixture.owners.manifests,
            &mut budget,
            &key,
            SCOPE,
            &start_digest,
            storage,
        )
    };
    let good = issued_fixture_graph(&fixture, &key, &start_digest);
    assert!(accept(good.clone(), &fixture.attempt, &test_storage()).is_ok());

    let mut unknown_issuer = good.clone();
    unknown_issuer.issuer = "unknown-issuer".into();
    unknown_issuer = issue_for_attempt(
        &fixture,
        &key,
        &start_digest,
        &fixture.attempt,
        &test_storage(),
        unknown_issuer,
    );
    assert_eq!(
        accept(unknown_issuer, &fixture.attempt, &test_storage()).unwrap_err(),
        Refusal::Identity
    );

    let mut wrong_pin = unsigned_graph(&fixture);
    wrong_pin.cargo_version = "1.0.0".into();
    let wrong_pin = issue_for_attempt(
        &fixture,
        &key,
        &start_digest,
        &fixture.attempt,
        &test_storage(),
        wrong_pin,
    );
    assert_eq!(
        accept(wrong_pin, &fixture.attempt, &test_storage()).unwrap_err(),
        Refusal::Identity
    );

    let mut wrong_graph = unsigned_graph(&fixture);
    wrong_graph.graph_digest = digest(b"other graph");
    let wrong_graph = issue_for_attempt(
        &fixture,
        &key,
        &start_digest,
        &fixture.attempt,
        &test_storage(),
        wrong_graph,
    );
    assert_eq!(
        accept(wrong_graph, &fixture.attempt, &test_storage()).unwrap_err(),
        Refusal::Identity
    );

    let mut duplicate = unsigned_graph(&fixture);
    duplicate.units[2].id = duplicate.units[0].id.clone();
    let duplicate = issue_for_attempt(
        &fixture,
        &key,
        &start_digest,
        &fixture.attempt,
        &test_storage(),
        duplicate,
    );
    assert_eq!(
        accept(duplicate, &fixture.attempt, &test_storage()).unwrap_err(),
        Refusal::Identity
    );

    let mut unsafe_path = unsigned_graph(&fixture);
    unsafe_path.units[0].fingerprint_directory = "../outside".into();
    let unsafe_path = issue_for_attempt(
        &fixture,
        &key,
        &start_digest,
        &fixture.attempt,
        &test_storage(),
        unsafe_path,
    );
    assert_eq!(
        accept(unsafe_path, &fixture.attempt, &test_storage()).unwrap_err(),
        Refusal::Unsupported
    );

    let mut forged = good.clone();
    forged.receipt.push('0');
    assert_eq!(
        accept(forged, &fixture.attempt, &test_storage()).unwrap_err(),
        Refusal::Authentication
    );

    let mut moved = fixture.attempt.clone();
    moved.slot = fixture.base.join("capture");
    assert_eq!(
        accept(good.clone(), &moved, &test_storage()).unwrap_err(),
        Refusal::Authentication
    );

    let mut replayed = fixture.attempt.clone();
    replayed.binding.run_id.push('x');
    assert_eq!(
        accept(good.clone(), &replayed, &test_storage()).unwrap_err(),
        Refusal::Authentication
    );

    let mut other_storage = test_storage();
    other_storage.slot_id = "slot-0000000000000002".into();
    assert_eq!(
        accept(good, &fixture.attempt, &other_storage).unwrap_err(),
        Refusal::Authentication
    );

    let extra_field = serde_json::to_string(&issued_fixture_graph(&fixture, &key, &start_digest))
        .unwrap()
        .replacen('{', "{\"unexpected\":true,", 1);
    assert!(serde_json::from_str::<GraphProvision>(&extra_field).is_err());

    let duplicate_key = serde_json::to_string(&issued_fixture_graph(&fixture, &key, &start_digest))
        .unwrap()
        .replacen("\"schema\":", "\"schema\":\"forged\",\"schema\":", 1);
    assert!(serde_json::from_str::<GraphProvision>(&duplicate_key).is_err());
}

#[test]
fn graph_owner_shared_budget_cannot_reset_or_publish_after_exhaustion() {
    let fixture = Fixture::new();
    install_cargo_freshness(&fixture);
    let key = MacKey::from_hex(OWNER_KEY).unwrap();
    let start_digest = format!("sha256:{}", "ef".repeat(32));
    let provision = issued_fixture_graph(&fixture, &key, &start_digest);
    let mut budget = evidence_budget(16_384);
    charge_budget(&mut budget, 16_000).unwrap();
    let used = budget_used(&budget);
    assert_eq!(
        accept_graph_provision(
            provision,
            &fixture.registration,
            &fixture.attempt,
            &fixture.owners.manifests,
            &mut budget,
            &key,
            SCOPE,
            &start_digest,
            &test_storage(),
        )
        .unwrap_err(),
        Refusal::Budget
    );
    assert_eq!(budget_used(&budget), used);
}

#[test]
fn graph_owner_missing_dep_info_and_rerun_mappings_refuse() {
    let fixture = Fixture::new();
    install_cargo_freshness(&fixture);
    let key = MacKey::from_hex(OWNER_KEY).unwrap();
    let start_digest = format!("sha256:{}", "11".repeat(32));
    let mut missing_dep = unsigned_graph(&fixture);
    missing_dep.units[0].dep_info.clear();
    let missing_dep = issue_for_attempt(
        &fixture,
        &key,
        &start_digest,
        &fixture.attempt,
        &test_storage(),
        missing_dep,
    );
    let mut budget = evidence_budget(Limits::NUCLEUS.collection_bytes);
    let context = accept_graph_provision(
        missing_dep,
        &fixture.registration,
        &fixture.attempt,
        &fixture.owners.manifests,
        &mut budget,
        &key,
        SCOPE,
        &start_digest,
        &test_storage(),
    )
    .unwrap();
    assert_eq!(
        cargo_freshness::collect(
            &fixture.registration,
            &fixture.attempt,
            &fixture.owners.manifests,
            &mut budget,
            &context,
        )
        .unwrap_err(),
        Refusal::MissingAuthority
    );

    let mut missing_rerun = unsigned_graph(&fixture);
    missing_rerun.units[1].rerun_files.clear();
    let missing_rerun = issue_for_attempt(
        &fixture,
        &key,
        &start_digest,
        &fixture.attempt,
        &test_storage(),
        missing_rerun,
    );
    let mut budget = evidence_budget(Limits::NUCLEUS.collection_bytes);
    let context = accept_graph_provision(
        missing_rerun,
        &fixture.registration,
        &fixture.attempt,
        &fixture.owners.manifests,
        &mut budget,
        &key,
        SCOPE,
        &start_digest,
        &test_storage(),
    )
    .unwrap();
    assert_eq!(
        cargo_freshness::collect(
            &fixture.registration,
            &fixture.attempt,
            &fixture.owners.manifests,
            &mut budget,
            &context,
        )
        .unwrap_err(),
        Refusal::MissingAuthority
    );
}

fn reading_bootstrap(
    f: &Fixture,
    request: &serde_json::Value,
    socket: PathBuf,
    instance: &str,
    parent: PathBuf,
) -> Bootstrap {
    let mut b = bootstrap(f, request, socket, instance, parent);
    b.graph_owner = Some(GraphOwnerAuthority::issue(&b).unwrap());
    b
}

#[test]
fn actual_child_cargo_reading_owner_collects_then_refuses_stale_mapping() {
    let started = Instant::now();
    let mut f = Fixture::new();
    f.attempt.binding.commit = "1".repeat(40);
    f.attempt.allocation_epoch = "1".into();
    let p = partition(&f);
    f.attempt.partition = digest(&serde_json::to_vec(&p).unwrap());
    let allowed = (1..=3).map(|n| envelope(&f, &p, n)).collect::<Vec<_>>();
    let authority = Arc::new(
        QualificationAuthority::new(
            MacKey::from_hex(STORAGE_KEY).unwrap(),
            SCOPE.into(),
            allowed.clone(),
        )
        .unwrap(),
    );
    let root = f.base.join("store");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let daemon = Daemon::bind(
        DaemonConfig {
            root: root.clone(),
            budget: Budget {
                cap_bytes: 16 * 1024 * 1024,
                free_reserve_bytes: 0,
            },
        },
        authority.clone(),
        Arc::new(BridgeFreeSpace),
    )
    .unwrap();
    let instance = daemon.storage_instance_id().to_owned();
    authority.install_instance(&instance).unwrap();
    let (socket, guard) = BridgeDaemon::start(daemon);
    let client = Client::new(socket.clone());
    let parent_path = f.base.join("parent.sock");
    let listener = UnixListener::bind(&parent_path).unwrap();
    fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o600)).unwrap();
    let map_key = MacKey::from_hex(MAP_KEY).unwrap();
    let owner_key = MacKey::from_hex(OWNER_KEY).unwrap();
    let mut previous = None;
    for (scenario, n) in [(1, 1), (2, 2), (3, 3)] {
        let scenario_started = Instant::now();
        let request = start(&f, &authority, allowed[n - 1].clone(), n);
        let b = reading_bootstrap(&f, &request, socket.clone(), &instance, parent_path.clone());
        let auth =
            CallbackAuthenticator::new(owner_key.clone(), b.callback_context().unwrap()).unwrap();
        let reading = ReadingMode {
            key: owner_key.clone(),
            start_digest: b.start_digest.clone(),
            forged: false,
            drop_dep_info: false,
        };
        if scenario == 3 {
            let slot = root.join("slots").join(previous.as_ref().unwrap());
            fs::write(slot.join("debug/deps/source.d"), b"stale-dep-info").unwrap();
        }
        let file = f.base.join(format!("bootstrap-reading-{scenario}.json"));
        write_bootstrap(&file, &b);
        let child = spawn_child(&file);
        let stream = accept_child(&listener);
        let (response, callbacks) = parent(
            stream,
            &f,
            &request,
            &auth,
            &authority,
            &map_key,
            false,
            Some(&reading),
        );
        if scenario <= 2 {
            child.finish(true);
            assert_eq!(response["result"]["status"], "finished");
            assert_eq!(response["result"]["outcome"], "published");
            assert!(callbacks.iter().any(|c| c["owner"] == "input.graph_owner"));
            assert!(callbacks.iter().all(|c| c["owner"] != "input.units"));
            previous = Some(
                response["result"]["storage"]["slot_id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
            assert_eq!(
                client.inventory("reading").unwrap().slots[0].state,
                SlotState::Idle
            );
        } else {
            child.finish(false);
            assert_eq!(response["operation"], "error");
            assert_eq!(
                response["result"].is_null() || response.get("result").is_none(),
                true
            );
            assert_eq!(
                client.inventory("stale").unwrap().slots[0].state,
                SlotState::Leased
            );
        }
        assert!(scenario_started.elapsed() < Duration::from_secs(30));
        assert!(callbacks.iter().all(|c| !has_token_field(c)));
    }
    guard.stop();
    assert!(started.elapsed() < Duration::from_secs(120));
}

#[test]
fn actual_child_forged_graph_owner_holds_acquired_lease() {
    let mut f = Fixture::new();
    f.attempt.binding.commit = "1".repeat(40);
    f.attempt.allocation_epoch = "1".into();
    let p = partition(&f);
    f.attempt.partition = digest(&serde_json::to_vec(&p).unwrap());
    let allowed = vec![envelope(&f, &p, 1)];
    let authority = Arc::new(
        QualificationAuthority::new(
            MacKey::from_hex(STORAGE_KEY).unwrap(),
            SCOPE.into(),
            allowed.clone(),
        )
        .unwrap(),
    );
    let root = f.base.join("store");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let daemon = Daemon::bind(
        DaemonConfig {
            root,
            budget: Budget {
                cap_bytes: 16 * 1024 * 1024,
                free_reserve_bytes: 0,
            },
        },
        authority.clone(),
        Arc::new(BridgeFreeSpace),
    )
    .unwrap();
    let instance = daemon.storage_instance_id().to_owned();
    authority.install_instance(&instance).unwrap();
    let (socket, guard) = BridgeDaemon::start(daemon);
    let client = Client::new(socket.clone());
    let parent_path = f.base.join("parent.sock");
    let listener = UnixListener::bind(&parent_path).unwrap();
    fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o600)).unwrap();
    let request = start(&f, &authority, allowed[0].clone(), 1);
    let b = reading_bootstrap(&f, &request, socket.clone(), &instance, parent_path.clone());
    let owner_key = MacKey::from_hex(OWNER_KEY).unwrap();
    let auth =
        CallbackAuthenticator::new(owner_key.clone(), b.callback_context().unwrap()).unwrap();
    let reading = ReadingMode {
        key: owner_key,
        start_digest: b.start_digest.clone(),
        forged: true,
        drop_dep_info: false,
    };
    let file = f.base.join("bootstrap-forged-graph.json");
    write_bootstrap(&file, &b);
    let child = spawn_child(&file);
    let stream = accept_child(&listener);
    let (response, callbacks) = parent(
        stream,
        &f,
        &request,
        &auth,
        &authority,
        &MacKey::from_hex(MAP_KEY).unwrap(),
        false,
        Some(&reading),
    );
    child.finish(false);
    assert_eq!(response["operation"], "error");
    assert!(callbacks.iter().any(|c| c["owner"] == "input.graph_owner"));
    assert!(callbacks.iter().all(|c| c["owner"] != "input.units"));
    assert_eq!(
        client.inventory("forged").unwrap().slots[0].state,
        SlotState::Leased
    );
    // A second acquire_id for the still-held attempt cannot mint a new token.
    assert_eq!(
        client.acquire(AcquireRequest {
            envelope: serde_json::from_value(
                start(&f, &authority, allowed[0].clone(), 1)["payload"]["acquire"]["envelope"]
                    .clone()
            )
            .unwrap(),
            acquire_id: "acquire-forged-replay".into(),
            required_bytes: 1024 * 1024
        }),
        Err(StorageError::Conflict)
    );
    guard.stop();
}

#[test]
fn graph_owner_bootstrap_unknown_issuer_refuses_before_connect() {
    let mut f = Fixture::new();
    f.attempt.binding.commit = "1".repeat(40);
    f.attempt.allocation_epoch = "1".into();
    let p = partition(&f);
    let allowed = vec![envelope(&f, &p, 1)];
    let authority = QualificationAuthority::new(
        MacKey::from_hex(STORAGE_KEY).unwrap(),
        SCOPE.into(),
        allowed.clone(),
    )
    .unwrap();
    let instance = "a".repeat(64);
    authority.install_instance(&instance).unwrap();
    let storage = f.base.join("storage.sock");
    let _storage = UnixListener::bind(&storage).unwrap();
    fs::set_permissions(&storage, fs::Permissions::from_mode(0o600)).unwrap();
    let parent_path = f.base.join("parent.sock");
    let listener = UnixListener::bind(&parent_path).unwrap();
    fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let request = start(&f, &authority, allowed[0].clone(), 1);
    let mut b = bootstrap(&f, &request, storage, &instance, parent_path);
    b.graph_owner = Some(GraphOwnerAuthority::issue(&b).unwrap());
    b.graph_owner.as_mut().unwrap().issuer = "production-issuer".into();
    let file = f.base.join("bootstrap-unknown-issuer.json");
    write_bootstrap(&file, &b);
    assert!(serve_bootstrap(&file).is_err());
    assert!(file.exists());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn graph_owner_issuance_vector_is_stable() {
    let v: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/qualification-graph-owner-v1.json"
    ))
    .unwrap();
    let key = MacKey::from_hex(v["key_hex"].as_str().unwrap()).unwrap();
    let domain = v["domain"].as_str().unwrap();
    let fields = v["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|field| field.as_str().unwrap())
        .collect::<Vec<_>>();
    let bytes = mac_frame(domain, &fields).unwrap();
    assert_eq!(
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        v["frame_hex"]
    );
    let receipt = key.sign(domain, &fields).unwrap();
    assert_eq!(receipt, v["receipt"]);
    assert!(key.verify(domain, &fields, &receipt));
    assert_eq!(domain, GRAPH_OWNER_PROVISION_DOMAIN);
    assert_eq!(
        canonical_digest(&v["canonical_subject"]).unwrap(),
        v["canonical_digest"]
    );
}
