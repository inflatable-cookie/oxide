//! Synthetic owner results, real compiled child and durable daemon. No Cargo
//! workload, Nucleus workflow, or benefit qualification is performed.
use super::*;
use crate::registered::qualification::*;
use crate::storage::{
    CaptureReceipt, CooperativeCompletionEvidence, ProcessIdentityEvidence, SlotState,
    TerminalOutcome, TerminalRecord,
};
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
    }
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
            "input.units" => serde_json::to_value(
                f.owners
                    .units(
                        &serde_json::from_value(payload["registration"].clone()).unwrap(),
                        &serde_json::from_value(payload["attempt"].clone()).unwrap(),
                    )
                    .unwrap(),
            )
            .unwrap(),
            "execution.execute" => serde_json::to_value(
                f.owners
                    .execute(&serde_json::from_value(payload["launch"].clone()).unwrap())
                    .unwrap(),
            )
            .unwrap(),
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
        let (response, callbacks) =
            parent(stream, &f, &request, &auth, &authority, &map_key, interrupt);
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
