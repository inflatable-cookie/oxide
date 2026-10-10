//! Ignored standalone pilot driver. It runs only when `OXIDE_PILOT_RUN=1`.
//!
//! The driver starts a local storage daemon under the retained disposable root,
//! then runs the registered Cargo workflow through the reviewed adapter. The
//! consumer identities are local qualification receipts, not Queue or Nucleus
//! settlement. Failed assertions stop the session without retry.

use super::super::cargo::CargoWorkflowRunner;
use super::super::native;
use super::*;
use crate::storage::{Budget, Client, Daemon, DaemonConfig, FreeSpaceProbe, StorageError};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::Instant;

#[path = "cargo_same_root.rs"]
mod same_root_probe;
#[path = "cargo_source_probe.rs"]
mod source_map_probe;

const GIB: u64 = 1024 * 1024 * 1024;
const WORKFLOW: &str = "nucleus-core-test";
const SELECTOR: &str =
    "check:core:test -- -p nucleus-core --test planning --test build_diagnostics";
const EFFIGY: &str = "/Users/tom/.local/bin/effigy";
const CHECKOUT_BASE: &str = "/private/var/tmp/oxide-cargo-8e87a74d/m/a";
const CHECKOUT_TRIAL: &str = "/private/var/tmp/oxide-cargo-8e87a74d/m/b";
const PATCH_FILE: &str = "crates/nucleus-core/tests/build_diagnostics.rs";
const PATCH_ANCHOR: &str = "fn development_profiles_preserve_diagnostics_and_checks() {\n";
const PATCH_LINE: &str = "    std::hint::black_box(\"oxide-source-miss-control\");\n";
const ENVIRONMENT_KEYS: [&str; 6] = [
    "CARGO_HOME",
    "HOME",
    "PATH",
    "RUSTFLAGS",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
];

struct PilotSpace;

/// Host free space gates the pilot. The disposable volume's own free space is reported separately.
const HOST_PROBE: &str = "/private/var/tmp";

impl FreeSpaceProbe for PilotSpace {
    fn available_bytes(&self, _root: &Path) -> Result<Option<u64>, StorageError> {
        native::host_free_bytes(Path::new(HOST_PROBE))
            .map(Some)
            .map_err(|_| StorageError::Io("free-space probe unavailable".to_owned()))
    }
}

/// Local qualification consumer. It records identities only for this pilot.
struct PilotConsumer {
    run_id: String,
    host_run_id: String,
    submitted: Option<HostRunRequest>,
}

impl PilotConsumer {
    fn for_run(run_id: &str) -> Self {
        Self {
            run_id: run_id.to_owned(),
            host_run_id: format!("host-{run_id}"),
            submitted: None,
        }
    }
}

impl ProjectConsumerBoundary for PilotConsumer {
    fn authorize_attempt(
        &mut self,
        request: &AttemptAuthorizationRequest,
    ) -> Result<Option<AuthorizedAttempt>, BoundaryFailure> {
        let binding = RunBinding {
            consumer: request.pending.consumer_id.clone(),
            project: request.pending.project_id.clone(),
            component: request.component_id.clone(),
            workflow: request.pending.workflow.clone(),
            task_id: request.pending.task_id.clone(),
            workspace_id: request.workspace_id.clone(),
            run_id: self.run_id.clone(),
            attempt_id: "attempt-1".to_owned(),
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
            acquire_id: format!("acquire-{}", self.run_id),
        }))
    }

    fn on_storage_refusal(&mut self, _: &AuthorizedAttempt, reason: AdapterRefusal) {
        log(&format!(
            "event=storage-refusal run={} reason={reason:?}",
            self.run_id
        ));
    }

    fn cancel_unstarted_attempt(&mut self, _: &AuthorizedAttempt) {}

    fn submit_host_run(
        &mut self,
        _: &AuthorizedAttempt,
        request: &HostRunRequest,
    ) -> Result<HostRunIdentity, BoundaryFailure> {
        self.submitted = Some(request.clone());
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
        for reference in &output.output_references {
            if !capture_dir.join(reference).is_file() {
                return Err(BoundaryFailure::InvalidIdentity);
            }
        }
        let digest = native::capture_receipt_digest(capture_dir, &output.output_references)?;
        Ok(CaptureAcknowledgement {
            host_run_id: host_run.id.clone(),
            receipt: CaptureReceipt {
                reference: format!("capture/{}", self.run_id),
                digest,
                host_run_id: host_run.id.clone(),
                run_id: self.run_id.clone(),
                attempt_id: "attempt-1".to_owned(),
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
        let outcome = match output?.status {
            FixtureCommandStatus::Exited(0) => TerminalOutcome::Passed,
            FixtureCommandStatus::Exited(_) => TerminalOutcome::Failed,
            FixtureCommandStatus::Cancelled => TerminalOutcome::Cancelled,
            FixtureCommandStatus::Incomplete => return None,
        };
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

/// Toolchain identity: version line plus SHA-256 of the cargo and rustc binaries.
struct Toolchain {
    cargo: ToolchainInput,
    rustc: ToolchainInput,
    channel: &'static str,
}

fn toolchain(channel: &'static str) -> Toolchain {
    let home = std::env::var("HOME").expect("HOME");
    let bin = format!("{home}/.rustup/toolchains/{channel}-aarch64-apple-darwin/bin");
    let input = |tool: &str| {
        let path = format!("{bin}/{tool}");
        let version = Command::new(&path)
            .arg("-V")
            .output()
            .expect("toolchain version");
        let bytes = fs::read(&path).expect("toolchain binary");
        ToolchainInput {
            version: String::from_utf8_lossy(&version.stdout).trim().to_owned(),
            executable_digest: format!("sha256:{:x}", Sha256::digest(&bytes)),
        }
    };
    Toolchain {
        cargo: input("cargo"),
        rustc: input("rustc"),
        channel,
    }
}

fn environment(toolchain: &str, rustflags: Option<&str>) -> Vec<EffectiveValue> {
    let home = std::env::var("HOME").expect("HOME");
    vec![
        EffectiveValue {
            name: "CARGO_HOME".to_owned(),
            value: Some(format!("{home}/.cargo")),
        },
        EffectiveValue {
            name: "HOME".to_owned(),
            value: Some(home.clone()),
        },
        EffectiveValue {
            name: "PATH".to_owned(),
            value: Some(format!(
                "{home}/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin"
            )),
        },
        EffectiveValue {
            name: "RUSTFLAGS".to_owned(),
            value: rustflags.map(str::to_owned),
        },
        EffectiveValue {
            name: "RUSTUP_HOME".to_owned(),
            value: Some(format!("{home}/.rustup")),
        },
        EffectiveValue {
            name: "RUSTUP_TOOLCHAIN".to_owned(),
            value: Some(toolchain.to_owned()),
        },
    ]
}

fn git(checkout: &Path, args: &[&str]) -> String {
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(checkout)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("git");
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8(output.stdout).expect("git utf8")
}

/// SHA-256 over tracked paths and their working-tree bytes.
fn tree_digest(checkout: &Path) -> String {
    let listing = git(checkout, &["ls-files", "-z"]);
    let mut paths: Vec<&str> = listing
        .split('\0')
        .filter(|path| !path.is_empty())
        .collect();
    paths.sort_unstable();
    let mut digest = Sha256::new();
    for path in paths {
        let bytes = fs::read(checkout.join(path)).expect("tracked file");
        digest.update((path.len() as u64).to_le_bytes());
        digest.update(path.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(&bytes);
    }
    format!("sha256:{:x}", digest.finalize())
}

fn head(checkout: &Path) -> String {
    git(checkout, &["rev-parse", "HEAD"]).trim().to_owned()
}

fn invocation(
    mount: &Path,
    checkout: &Path,
    run: &str,
    toolchain: &Toolchain,
    environment: Vec<EffectiveValue>,
    tree: &str,
    commit: &str,
) -> ProjectInvocation {
    let runs = mount.join("runs").join(run);
    ProjectInvocation {
        consumer_id: "oxide-pilot-local".to_owned(),
        descriptor: ProjectDescriptor {
            project_id: "nucleus".to_owned(),
            component_id: "nucleus-core".to_owned(),
            workspace_root: checkout.to_path_buf(),
            workflows: vec![WorkflowOptIn {
                workflow: WORKFLOW.to_owned(),
                selector: SELECTOR.to_owned(),
                profile: "dev".to_owned(),
                features: Vec::new(),
                required_slot_bytes: 4 * GIB,
                cargo_config_keys: Vec::new(),
                environment_keys: ENVIRONMENT_KEYS.iter().map(|key| key.to_string()).collect(),
                path_identity: PathIdentity::DisposableRoot {
                    id: "pilot-image-oxide-005".to_owned(),
                    root: mount.to_path_buf(),
                    source_map: None,
                },
            }],
        },
        workflow: WORKFLOW.to_owned(),
        selector: SELECTOR.to_owned(),
        request_id: format!("request-{run}"),
        task_id: "task-pilot".to_owned(),
        workspace_id: "workspace-pilot".to_owned(),
        commit: commit.to_owned(),
        source_tree_digest: tree.to_owned(),
        inputs: EffectiveBuildInputs {
            cargo: toolchain.cargo.clone(),
            rustc: toolchain.rustc.clone(),
            host: "aarch64-apple-darwin".to_owned(),
            target: "aarch64-apple-darwin".to_owned(),
            cargo_configuration: EffectiveInputSet::Complete(Vec::new()),
            environment: EffectiveInputSet::Complete(environment),
            effective_rustflags: None,
            full_debug: true,
        },
        target_dir: runs.join("target"),
        capture_dir: runs.join("capture"),
        private_fallback_allowed: false,
        wait_once: false,
    }
}

/// Partition digest from the reviewed resolver. Used for key-separation controls.
fn partition_digest(inv: &ProjectInvocation) -> String {
    match super::super::resolve::resolve(inv) {
        super::super::resolve::Resolution::Ready(resolved) => {
            format!(
                "sha256:{:x}",
                Sha256::digest(format!("{:?}", resolved.partition))
            )
        }
        _ => panic!("pilot invocation did not resolve"),
    }
}

fn create_run_dirs(mount: &Path, run: &str) {
    let runs = mount.join("runs").join(run);
    fs::create_dir_all(runs.join("target")).expect("target dir");
    fs::create_dir_all(runs.join("capture")).expect("capture dir");
}

fn free_gib(_mount: &Path) -> u64 {
    native::host_free_bytes(Path::new(HOST_PROBE)).expect("free space") / GIB
}

fn evidence_root() -> PathBuf {
    PathBuf::from(std::env::var("OXIDE_PILOT_EVIDENCE").expect("OXIDE_PILOT_EVIDENCE"))
}

fn log(line: &str) {
    let path = evidence_root().join("events.log");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("events log");
    writeln!(file, "{line}").expect("events write");
}

/// Counts occurrences of a byte pattern across the regular files in a directory.
fn count_in_binaries(directory: &Path, prefixes: &[&str], pattern: &str) -> Vec<(String, usize)> {
    let mut found = Vec::new();
    for entry in fs::read_dir(directory).expect("deps dir") {
        let entry = entry.expect("deps entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if !prefixes.iter().any(|prefix| name.starts_with(prefix)) || name.contains('.') {
            continue;
        }
        let bytes = fs::read(entry.path()).expect("binary");
        let count = bytes
            .windows(pattern.len())
            .filter(|window| *window == pattern.as_bytes())
            .count();
        found.push((name, count));
    }
    found.sort();
    found
}

struct DaemonGuard {
    socket: PathBuf,
    shutdown: Option<Sender<()>>,
    thread: Option<std::thread::JoinHandle<Result<(), StorageError>>>,
}

impl DaemonGuard {
    fn start(storage_root: &Path) -> Self {
        let daemon = Daemon::bind(
            DaemonConfig {
                root: storage_root.to_path_buf(),
                budget: Budget {
                    cap_bytes: 7 * GIB,
                    free_reserve_bytes: 20 * GIB,
                },
            },
            Arc::new(FixtureAuthority),
            Arc::new(PilotSpace),
        )
        .expect("bind pilot daemon");
        let socket = daemon.socket_path().to_path_buf();
        let (shutdown, receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || daemon.serve(receiver));
        Self {
            socket,
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }

    fn client(&self) -> Client {
        Client::new(self.socket.clone())
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_adapter(
    client: Client,
    invocation: &ProjectInvocation,
    consumer: &mut PilotConsumer,
    runner: &mut CargoWorkflowRunner,
) -> ProjectExecutionResult {
    let mut adapter = ProjectAdapter::new(client);
    adapter.execute(invocation, consumer, &mut NoWait, runner, &NeverCancelled)
}

#[test]
#[ignore = "standalone Cargo pilot; run only with OXIDE_PILOT_RUN=1"]
fn nucleus_cargo_pilot() {
    if std::env::var("OXIDE_PILOT_RUN").as_deref() != Ok("1") {
        return;
    }
    let mount = PathBuf::from("/private/var/tmp/oxide-cargo-8e87a74d/m");
    std::env::set_var("TMPDIR", mount.join("tmp"));
    let checkout_base = PathBuf::from(CHECKOUT_BASE);
    let checkout_trial = PathBuf::from(CHECKOUT_TRIAL);
    let started = Instant::now();
    log(&format!(
        "event=session-start free_gib={}",
        free_gib(&mount)
    ));

    let stable = toolchain("1.97.1");
    let alternate = toolchain("1.96.1");
    let base_head = head(&checkout_base);
    let trial_head = head(&checkout_trial);
    assert_eq!(base_head, trial_head);
    assert_eq!(base_head, "bca75ed0143f96cd20b88ff72a421f1fb5f4ef19");
    let base_tree = tree_digest(&checkout_base);
    assert_eq!(base_tree, tree_digest(&checkout_trial));
    log(&format!(
        "event=source head={base_head} tree={base_tree} cargo=\"{}\" rustc=\"{}\"",
        stable.cargo.version, stable.rustc.version
    ));

    let env = environment(stable.channel, None);
    let mut runner = CargoWorkflowRunner::new(
        mount.clone(),
        PathBuf::from(HOST_PROBE),
        PathBuf::from(EFFIGY),
    )
    .expect("cargo runner");

    // Scenario 1: isolated cold reference, private build directory on the image.
    create_run_dirs(&mount, "s1");
    let s1 = mount.join("runs/s1");
    fs::create_dir_all(s1.join("build")).expect("s1 build");
    let s1_binding = RunBinding {
        consumer: "oxide-pilot-local".to_owned(),
        project: "nucleus".to_owned(),
        component: "nucleus-core".to_owned(),
        workflow: WORKFLOW.to_owned(),
        task_id: "task-pilot".to_owned(),
        workspace_id: "workspace-pilot".to_owned(),
        run_id: "run-s1".to_owned(),
        attempt_id: "attempt-1".to_owned(),
        commit: base_head.clone(),
        source_tree_digest: base_tree.clone(),
    };
    let s1_host = HostRunIdentity {
        id: "host-run-s1".to_owned(),
    };
    let s1_plan = FixtureExecutionPlan {
        host_run: s1_host.clone(),
        command: HostRunRequest {
            selector: SELECTOR.to_owned(),
            workflow: WORKFLOW.to_owned(),
            profile: "dev".to_owned(),
            features: Vec::new(),
            workspace_root: checkout_base.clone(),
            cargo_build_dir: s1.join("build"),
            cargo_target_dir: s1.join("target"),
            capture_dir: s1.join("capture"),
            source_map: None,
            environment: env.clone(),
        },
        claim: native::NativeExecutionClaim::new(
            "lease-s1".to_owned(),
            "slot-s1".to_owned(),
            s1.join("build"),
            s1_binding,
            s1_host,
            WORKFLOW.to_owned(),
            SELECTOR.to_owned(),
            s1.join("capture"),
        ),
    };
    let s1_started = Instant::now();
    let s1_output = runner
        .execute(&s1_plan, &NeverCancelled)
        .expect("scenario 1 cargo run");
    let s1_elapsed = s1_started.elapsed().as_millis();
    assert_eq!(
        s1_output.status,
        FixtureCommandStatus::Exited(0),
        "scenario 1 tests failed"
    );
    assert!(s1_output.native_completion.is_some());
    log(&format!(
        "event=s1-cold-reference status=exited0 elapsed_ms={s1_elapsed} rebuilt_units={:?} free_gib={}",
        s1_output.rebuilt_units,
        free_gib(&mount)
    ));

    // Scenarios 2 to 4 use the reviewed adapter, one daemon, and one slot.
    let daemon = DaemonGuard::start(&mount.join("storage"));
    let inv2 = invocation(
        &mount,
        &checkout_base,
        "s2",
        &stable,
        env.clone(),
        &base_tree,
        &base_head,
    );
    create_run_dirs(&mount, "s2");
    let mut consumer2 = PilotConsumer::for_run("run-s2");
    let t2 = Instant::now();
    let result2 = run_adapter(daemon.client(), &inv2, &mut consumer2, &mut runner);
    let s2_submitted = consumer2.submitted.clone();
    log(&format!(
        "event=s2-cold-slot disposition={:?} finalization={:?} elapsed_ms={} free_gib={}",
        result2.disposition,
        result2.finalization.as_ref().map(|receipt| receipt.state),
        t2.elapsed().as_millis(),
        free_gib(&mount)
    ));
    assert_eq!(result2.disposition, ExecutionDisposition::Finished);
    let slot_s2 = s2_submitted
        .as_ref()
        .expect("s2 submitted")
        .cargo_build_dir
        .clone();

    let inv3 = invocation(
        &mount,
        &checkout_trial,
        "s3",
        &stable,
        env.clone(),
        &base_tree,
        &base_head,
    );
    create_run_dirs(&mount, "s3");
    let mut consumer3 = PilotConsumer::for_run("run-s3");
    let t3 = Instant::now();
    let result3 = run_adapter(daemon.client(), &inv3, &mut consumer3, &mut runner);
    let slot_s3 = consumer3
        .submitted
        .as_ref()
        .expect("s3 submitted")
        .cargo_build_dir
        .clone();
    log(&format!(
        "event=s3-warm-checkout-b disposition={:?} same_slot={} partition_same={} elapsed_ms={} free_gib={}",
        result3.disposition,
        slot_s2 == slot_s3,
        partition_digest(&inv2) == partition_digest(&inv3),
        t3.elapsed().as_millis(),
        free_gib(&mount)
    ));
    assert_eq!(result3.disposition, ExecutionDisposition::Finished);
    assert_eq!(slot_s2, slot_s3, "scenario 3 must reuse the released slot");
    // Test binaries live under the slot build directory, not the final target directory.
    let s3_target = slot_s3.join("debug/deps");
    let base_refs = count_in_binaries(
        &s3_target,
        &["planning-", "build_diagnostics-"],
        CHECKOUT_BASE,
    );
    let trial_refs = count_in_binaries(
        &s3_target,
        &["planning-", "build_diagnostics-"],
        CHECKOUT_TRIAL,
    );
    log(&format!(
        "event=s3-path-proof base_path_hits={base_refs:?} trial_path_hits={trial_refs:?}"
    ));

    // Scenario 4: one meaningful source change in checkout B only.
    let patched_path = checkout_trial.join(PATCH_FILE);
    let original = fs::read_to_string(&patched_path).expect("patch target");
    assert!(original.contains(PATCH_ANCHOR), "patch anchor missing");
    fs::write(
        &patched_path,
        original.replacen(PATCH_ANCHOR, &format!("{PATCH_ANCHOR}{PATCH_LINE}"), 1),
    )
    .expect("apply patch");
    let patch = git(&checkout_trial, &["diff"]);
    fs::write(evidence_root().join("s4-source-miss.patch"), &patch).expect("save patch");
    let patched_tree = tree_digest(&checkout_trial);
    assert_ne!(patched_tree, base_tree);
    create_run_dirs(&mount, "s4");
    let inv4 = invocation(
        &mount,
        &checkout_trial,
        "s4",
        &stable,
        env.clone(),
        &patched_tree,
        &base_head,
    );
    let mut consumer4 = PilotConsumer::for_run("run-s4");
    let t4 = Instant::now();
    let result4 = run_adapter(daemon.client(), &inv4, &mut consumer4, &mut runner);
    let s4_elapsed = t4.elapsed().as_millis();
    let restore = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(&checkout_trial)
        .args(["checkout", "--", PATCH_FILE])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .status()
        .expect("restore patch");
    assert!(restore.success(), "patch restore failed");
    let restored_tree = tree_digest(&checkout_trial);
    assert_eq!(
        restored_tree, base_tree,
        "restore did not return the frozen tree"
    );
    let s4_manifest =
        fs::read_to_string(mount.join("runs/s4/capture/manifest.txt")).unwrap_or_default();
    let rebuilt_line = s4_manifest
        .lines()
        .find(|line| line.starts_with("rebuilt_units="))
        .unwrap_or("rebuilt_units=unknown")
        .to_owned();
    log(&format!(
        "event=s4-source-miss disposition={:?} patched_tree={patched_tree} {rebuilt_line} elapsed_ms={s4_elapsed} restored_tree_matches_base={}",
        result4.disposition,
        restored_tree == base_tree
    ));
    assert_eq!(result4.disposition, ExecutionDisposition::Finished);
    assert!(consumer4.submitted.is_some());

    // Scenarios 5 and 6: key separation only, with no build.
    let inv5 = invocation(
        &mount,
        &checkout_base,
        "s5",
        &stable,
        environment(stable.channel, Some("-C debuginfo=1")),
        &base_tree,
        &base_head,
    );
    let inv6 = invocation(
        &mount,
        &checkout_base,
        "s6",
        &alternate,
        environment(alternate.channel, None),
        &base_tree,
        &base_head,
    );
    let base_partition = partition_digest(&inv2);
    log(&format!(
        "event=s5-config-key-separation separated={} base_partition={base_partition}",
        partition_digest(&inv5) != base_partition
    ));
    log(&format!(
        "event=s6-toolchain-key-separation separated={} alternate_cargo=\"{}\"",
        partition_digest(&inv6) != base_partition,
        alternate.cargo.version
    ));
    assert_ne!(partition_digest(&inv5), base_partition);
    assert_ne!(partition_digest(&inv6), base_partition);

    let inventory = daemon
        .client()
        .inventory("inventory-final")
        .expect("final inventory");
    log(&format!(
        "event=final-inventory slots={} states={:?} accounted_bytes={:?} free_gib={} elapsed_s={}",
        inventory.slots.len(),
        inventory
            .slots
            .iter()
            .map(|slot| slot.state)
            .collect::<Vec<_>>(),
        inventory.accounted_bytes,
        free_gib(&mount),
        started.elapsed().as_secs()
    ));
}
