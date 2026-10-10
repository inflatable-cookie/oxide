//! Ignored future proof. Enabling it requires a new scoped Queue execution record.
//! It uses a fresh fixed image and never opens previous probe sessions.

use super::super::super::freshness;
use super::super::super::source_map_probe as producer;
use super::source_map_probe as resources;
use super::*;
use crate::storage::{
    AcquireRequest, AcquireResult, CaptureReceipt, Credential, FinalizationReceipt, Inventory,
    LeaseToken, RunBinding, RunEnvelope, SettlementEvidence, SlotState, TerminalOutcome,
    TerminalRecord,
};
use std::fs::File;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, UNIX_EPOCH};

const ROOT: &str = "/private/var/tmp/oxide-rust-probe-proof-021-reuse-v2";
const SCOPE: &str = "oxide-021-same-root-v2";
const T0: &str = "sha256:550e62cefc7f647fef97b12489fc2c1e1ba67fd56a7529401fce25c97b0ce2e9";
const T1: &str = "sha256:784b4c701a7b752a11e0b5b99e7fff36f796af643cd3e8514ae16147b237ee26";

struct CountingRunner {
    runner: CargoWorkflowRunner,
    invocations: Arc<AtomicU32>,
    mount: PathBuf,
    client: Client,
}
impl FixtureCommandRunner for CountingRunner {
    fn execute(
        &mut self,
        plan: &FixtureExecutionPlan,
        cancellation: &dyn CancellationProbe,
    ) -> Result<FixtureCommandOutput, BoundaryFailure> {
        // Counts a possible launch even if the underlying runner fails before
        // its spawn response; no retry can exceed four actual compiler calls.
        let count = self.invocations.fetch_add(1, Ordering::SeqCst) + 1;
        if count > 4 {
            return Err(BoundaryFailure::Rejected);
        }
        resources::append_evidence(
            &self.mount,
            &format!(
                "compiler-invocation\tcount={count}\trun={}\n",
                plan.claim.binding.run_id
            ),
        )
        .map_err(|_| BoundaryFailure::Unavailable)?;
        let inventory = self
            .client
            .inventory("proof-second-claim")
            .map_err(|_| BoundaryFailure::Unavailable)?;
        let slot = inventory
            .slots
            .iter()
            .find(|slot| slot.slot_id == plan.claim.slot_id)
            .ok_or(BoundaryFailure::InvalidIdentity)?;
        let mut binding = plan.claim.binding.clone();
        binding.run_id = format!("second-{count}");
        binding.attempt_id = format!("second-{count}");
        let second = self.client.acquire(AcquireRequest {
            envelope: RunEnvelope {
                binding,
                partition: slot.partition.clone(),
                admission_reference: format!("local-admission-second-{count}"),
                admission_credential: Credential::new("admission-proof"),
            },
            acquire_id: format!("acquire-second-{count}"),
            required_bytes: GIB,
        });
        if second
            != Err(StorageError::Refused(
                crate::storage::RefusalReason::LeaseUnavailable,
            ))
        {
            return Err(BoundaryFailure::InvalidIdentity);
        }
        resources::append_evidence(&self.mount, "exclusive-second-claim\trefused-no-spawn\n")
            .map_err(|_| BoundaryFailure::Unavailable)?;
        self.runner.execute(plan, cancellation)
    }
}

struct ObservedStorage {
    client: Client,
    acquired: Arc<Mutex<Option<AcquireResult>>>,
}
impl StorageOperations for ObservedStorage {
    fn inventory(&self, id: &str) -> Result<Inventory, StorageError> {
        self.client.inventory(id)
    }
    fn acquire(&self, request: AcquireRequest) -> Result<AcquireResult, StorageError> {
        let result = self.client.acquire(request)?;
        *self
            .acquired
            .lock()
            .map_err(|_| StorageError::Protocol("proof capture lock poisoned"))? =
            Some(result.clone());
        Ok(result)
    }
    fn finalize(
        &self,
        lease: &str,
        token: Option<&LeaseToken>,
        id: &str,
        evidence: SettlementEvidence,
    ) -> Result<FinalizationReceipt, StorageError> {
        self.client.finalize(lease, token, id, evidence)
    }
}

struct Consumer {
    run: String,
    output: Option<FixtureCommandOutput>,
}
impl ProjectConsumerBoundary for Consumer {
    fn authorize_attempt(
        &mut self,
        request: &AttemptAuthorizationRequest,
    ) -> Result<Option<AuthorizedAttempt>, BoundaryFailure> {
        Ok(Some(AuthorizedAttempt {
            envelope: RunEnvelope {
                binding: RunBinding {
                    consumer: request.pending.consumer_id.clone(),
                    project: request.pending.project_id.clone(),
                    component: request.component_id.clone(),
                    workflow: request.pending.workflow.clone(),
                    task_id: request.pending.task_id.clone(),
                    workspace_id: request.workspace_id.clone(),
                    run_id: format!("run-{}", self.run),
                    attempt_id: format!("attempt-{}", self.run),
                    commit: request.commit.clone(),
                    source_tree_digest: request.source_tree_digest.clone(),
                },
                partition: request.partition.clone(),
                admission_reference: format!("local-admission-{}", self.run),
                admission_credential: Credential::new("admission-proof"),
            },
            acquire_id: format!("acquire-{}", self.run),
        }))
    }
    fn on_storage_refusal(&mut self, _: &AuthorizedAttempt, _: AdapterRefusal) {}
    fn cancel_unstarted_attempt(&mut self, _: &AuthorizedAttempt) {}
    fn submit_host_run(
        &mut self,
        _: &AuthorizedAttempt,
        _: &HostRunRequest,
    ) -> Result<HostRunIdentity, BoundaryFailure> {
        Ok(HostRunIdentity {
            id: format!("host-{}", self.run),
        })
    }
    fn capture(
        &mut self,
        host: &HostRunIdentity,
        output: &FixtureCommandOutput,
        directory: &Path,
    ) -> Result<CaptureAcknowledgement, BoundaryFailure> {
        self.output = Some(output.clone());
        Ok(CaptureAcknowledgement {
            host_run_id: host.id.clone(),
            receipt: CaptureReceipt {
                reference: format!("capture-{}", self.run),
                digest: native::cargo_capture_receipt_digest(directory, &output.output_references)?,
                host_run_id: host.id.clone(),
                run_id: format!("run-{}", self.run),
                attempt_id: format!("attempt-{}", self.run),
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
        Some(TerminalRecord {
            run_id: attempt.envelope.binding.run_id.clone(),
            outcome: if output.is_some_and(|out| out.status == FixtureCommandStatus::Exited(0)) {
                TerminalOutcome::Passed
            } else {
                TerminalOutcome::Failed
            },
            credential: Credential::new("terminal-proof"),
        })
    }
    fn settlement_credential(&mut self, _: &AuthorizedAttempt) -> Credential {
        Credential::new("consumer-proof")
    }
}
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

#[test]
#[ignore = "requires a fresh approved Queue scope; four compiler calls, no live recovery"]
fn same_root_cargo_freshness_owned_proof() {
    if std::env::var("OXIDE_SAME_ROOT_PROOF_RUN").as_deref() != Ok("1") {
        return;
    }
    assert_eq!(
        std::env::var("OXIDE_SAME_ROOT_PROOF_SCOPE").as_deref(),
        Ok(SCOPE),
        "a scoped later execution record is required"
    );
    let started = Instant::now(); // original live monotonic clock before setup
    run_proof(started).expect("same-root proof stopped; preserve all state and evidence");
}

fn run_proof(started: Instant) -> Result<(), String> {
    let parent = PathBuf::from(ROOT);
    if parent.exists() {
        return Err("fresh session path already exists; preserve it".into());
    }
    let start_utc = resources::machine_utc()?;
    let deadline_epoch = resources::machine_epoch_utc(&start_utc)?
        .checked_add(1200)
        .ok_or("UTC deadline overflow")?;
    let deadline_utc = resources::machine_date(&[
        "-u",
        "-r",
        &deadline_epoch.to_string(),
        "+%Y-%m-%dT%H:%M:%SZ",
    ])?;
    fs::create_dir(&parent).map_err(|error| error.to_string())?;
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let mount = parent.join("m");
    let image = parent.join("oxide-rust-probe.sparseimage");
    let setup = parent.join("setup-tmp");
    resources::ensure_probe_directory(&setup)?;
    resources::record_session_clock(&setup, &start_utc, &deadline_utc, SCOPE)?;
    let budget = resources::ProbeBudget::new(mount.clone(), image.clone(), setup.clone(), started);
    budget.sample()?;
    let sampled = Arc::clone(&budget);
    let _timer = resources::PeriodicSampler::start(Duration::from_secs(20), move || {
        let _ = sampled.sample();
    });
    let cancellation = resources::ProbeCancellation(Arc::clone(&budget));
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/cargo-source-map-probe");
    let frozen = resources::freeze_fixture(&source)?;
    resources::verify_committed_fixture(&source, &frozen)?;
    let (_device, outside) =
        resources::setup_image(&image, &mount, &setup, "oxide-same-root-021-v2")?;
    budget.outside_directories.store(outside, Ordering::Release);
    budget.mounted.store(true, Ordering::Release);
    for name in ["evidence", "tmp", "home", "cargo-home", "runs", "launch"] {
        resources::ensure_probe_directory(&mount.join(name))?;
    }
    // All compiler temporaries and task fixtures live within the disposable cap.
    for name in ["OXIDE_TASK_SCRATCH_ROOT", "TMPDIR", "TMP", "TEMP"] {
        std::env::set_var(name, mount.join("tmp"));
    }
    let root = mount.join("r");
    resources::prepare_fixture_root(&frozen, &root)?;
    if freshness::requested_tree(&root)
        .map_err(|reason| format!("baseline eligibility: {reason:?}"))?
        != T0
    {
        return Err("frozen T0 changed".into());
    }
    let stable = toolchain("1.97.1");
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME missing")?);
    let tools = home.join(".rustup/toolchains/1.97.1-aarch64-apple-darwin/bin");
    let effigy = locate_effigy()?;
    let launch = mount.join("launch");
    let cargo = tools.join("cargo");
    // Literal paths are quoted independently; these fixed task-owned paths have
    // no shell metacharacters. The launcher validates the exact selector vector.
    let command = format!(
        "cd '{}' && exec '{}' {}",
        root.display(),
        cargo.display(),
        freshness::SELECTOR
    );
    fs::write(
        launch.join("effigy.toml"),
        format!("[tasks]\n\"same-root:probe\" = \"{command}\"\n"),
    )
    .map_err(|error| error.to_string())?;
    let launcher = launch.join("selector");
    fs::write(
        &launcher,
        format!(
            "#!/bin/sh\n[ \"$*\" = '{}' ] || exit 64\nexec '{}' --repo '{}' same-root:probe\n",
            freshness::SELECTOR,
            effigy.display(),
            launch.display()
        ),
    )
    .map_err(|error| error.to_string())?;
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let environment = vec![
        EffectiveValue {
            name: "CARGO_HOME".into(),
            value: Some(mount.join("cargo-home").to_string_lossy().into_owned()),
        },
        EffectiveValue {
            name: "HOME".into(),
            value: Some(mount.join("home").to_string_lossy().into_owned()),
        },
        EffectiveValue {
            name: "PATH".into(),
            value: Some(format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", tools.display())),
        },
        EffectiveValue {
            name: "RUSTUP_HOME".into(),
            value: Some(home.join(".rustup").to_string_lossy().into_owned()),
        },
        EffectiveValue {
            name: "RUSTUP_TOOLCHAIN".into(),
            value: Some("1.97.1".into()),
        },
        EffectiveValue {
            name: "CARGO_BUILD_JOBS".into(),
            value: Some("1".into()),
        },
    ];
    validate_launch(
        &effigy,
        &launch,
        &environment,
        &setup,
        &cancellation,
        "setup",
        &command,
    )?;
    budget.sample()?;
    let storage = mount.join("storage");
    create_private_storage(&storage)?;
    let daemon = resources::ProbeDaemon::start(&storage, Arc::clone(&budget));
    let client = daemon.client();
    let acquired = Arc::new(Mutex::new(None));
    let observed = ObservedStorage {
        client: client.clone(),
        acquired: Arc::clone(&acquired),
    };
    let mut adapter = ProjectAdapter::new(observed);
    let count = Arc::new(AtomicU32::new(0));
    let mut runner = CountingRunner {
        runner: CargoWorkflowRunner::new(
            mount.clone(),
            PathBuf::from("/private/var/tmp"),
            launcher,
        )
        .map_err(|reason| format!("runner: {reason:?}"))?,
        invocations: Arc::clone(&count),
        mount: mount.clone(),
        client: client.clone(),
    };
    let mut previous_slot = None;
    for (step, label) in [
        "cold-t0",
        "warm-t1",
        "retry-t1",
        "stale-refusal",
        "failed-compile",
        "post-failure-refusal",
    ]
    .iter()
    .enumerate()
    {
        budget.sample()?;
        let marker = root.join("probe/marker.txt");
        let library = root.join("probe/src/lib.rs");
        match step {
            1 => {
                fs::write(&marker, b"changed-marker\n").map_err(|error| error.to_string())?;
            }
            3 => {
                fs::write(&marker, b"baseline-marker\n").map_err(|error| error.to_string())?;
                old_mtime(&marker)?;
            }
            4 => {
                fs::write(&marker, b"changed-marker\n").map_err(|error| error.to_string())?;
                let canonical =
                    fs::read(source.join("probe/src/lib.rs")).map_err(|error| error.to_string())?;
                fs::write(
                    &library,
                    [canonical.as_slice(), freshness::FAILURE_OVERLAY].concat(),
                )
                .map_err(|error| error.to_string())?;
            }
            5 => {
                fs::write(
                    &library,
                    fs::read(source.join("probe/src/lib.rs")).map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?;
                fs::write(&marker, b"baseline-marker\n").map_err(|error| error.to_string())?;
                old_mtime(&library)?;
                old_mtime(&marker)?;
            }
            _ => {}
        }
        let tree = freshness::requested_tree(&root)
            .map_err(|reason| format!("source eligibility: {reason:?}"))?;
        if step == 1 && tree != T1 {
            return Err("frozen marker-only T1 changed".into());
        }
        let run_root = mount.join("runs").join(label);
        fs::create_dir(&run_root).map_err(|error| error.to_string())?;
        for name in ["target", "capture"] {
            resources::ensure_probe_directory(&run_root.join(name))?;
        }
        let invocation = invocation(
            &root,
            &mount,
            &run_root,
            label,
            &stable,
            &environment,
            &tree,
        );
        // Validate actual Effigy routing before acquire on every attempt,
        // including refusal steps. --plan must not start any compiler.
        validate_launch(
            &effigy,
            &launch,
            &environment,
            &setup,
            &cancellation,
            label,
            &command,
        )?;
        let before = count.load(Ordering::SeqCst);
        let mut consumer = Consumer {
            run: label.to_string(),
            output: None,
        };
        let result = adapter.execute(
            &invocation,
            &mut consumer,
            &mut NoWait,
            &mut runner,
            &cancellation,
        );
        let lease = acquired
            .lock()
            .map_err(|_| "acquire observation poisoned")?
            .clone()
            .ok_or("proof did not acquire")?;
        let slot_path = lease.slot_path.as_ref().ok_or("slot path absent")?;
        resources::append_evidence(&mount, &format!("step\t{label}\ttree={tree}\tslot={}\treused={}\tbefore={before}\tafter={}\tdisposition={:?}\tfinalization={:?}\tobservations={:?}\n", lease.slot_id, lease.reused, count.load(Ordering::SeqCst), result.disposition, result.finalization, adapter.observations()))?;
        if previous_slot
            .as_ref()
            .is_some_and(|slot| slot != &lease.slot_id)
        {
            return Err("same-root revision allocated another slot".into());
        }
        previous_slot = Some(lease.slot_id.clone());
        if matches!(step, 3 | 5) {
            let expected = if step == 3 {
                AdapterRefusal::SourceStale
            } else {
                AdapterRefusal::InputUnknown
            };
            if result.disposition != ExecutionDisposition::Refused(expected)
                || count.load(Ordering::SeqCst) != before
                || result
                    .finalization
                    .as_ref()
                    .is_none_or(|receipt| receipt.state != SlotState::Idle)
            {
                return Err("no-compiler refusal gate failed".into());
            }
            if step == 5 {
                let decision = freshness::recovery(SlotState::Idle, true, true, false, true);
                resources::append_evidence(
                    &mount,
                    &format!("recovery-inspection\t{decision:?}\tgrant=false\tno-deletion=true\n"),
                )?;
            }
        } else if step == 4 {
            let output = consumer
                .output
                .as_ref()
                .ok_or("failed compiler completion/capture unknown; preserve lease")?;
            if !matches!(output.status, FixtureCommandStatus::Exited(code) if code > 0) {
                return Err("intentional compiler failure did not fail".into());
            }
            if result
                .finalization
                .as_ref()
                .is_none_or(|receipt| receipt.state != SlotState::Idle)
            {
                return Err(
                    "failed compile retained Uncertain ownership; stop without restore".into(),
                );
            }
            if !slot_path.join("oxide-freshness-pending-v1").is_file() {
                return Err("failed compile left map usable".into());
            }
        } else {
            if result.disposition != ExecutionDisposition::Finished {
                return Err("successful proof run did not publish a reusable map".into());
            }
            let summary = producer::inspect_cargo_output(
                &fs::read(run_root.join("capture/stdout.log"))
                    .map_err(|error| error.to_string())?,
            );
            if step == 1
                && (!summary.artifacts.iter().any(|artifact| {
                    artifact.target_name == "oxide_source_probe_macro" && artifact.fresh
                }) || summary.artifacts.iter().any(|artifact| {
                    artifact.target_name == "oxide_source_probe_macro" && !artifact.fresh
                }))
            {
                return Err("warm different-tree run did not retain proc-macro work".into());
            }
            if step == 2 && summary.rebuilt_units != 0 {
                return Err("same-tree retry rebuilt units".into());
            }
        }
        budget.sample()?;
    }
    if count.load(Ordering::SeqCst) != 4 {
        return Err("proof did not use exactly four compiler invocations".into());
    }
    resources::append_evidence(
        &mount,
        "same-root-proof\tpass\tno-live-recovery\tconsumer-adoption-unqualified\n",
    )?;
    budget.sample()?;
    // The later session owns any ordinary detach decision. This implementation
    // driver leaves the new resources inspectable and never clears evidence.
    Ok(())
}

// Create only a new owned root; never repair permissions on an existing path.
fn create_private_storage(root: &Path) -> Result<(), String> {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(root)
        .map_err(|error| format!("create private probe storage: {error}"))?;
    resources::ensure_probe_directory(root)
}

#[test]
fn private_storage_setup_binds_daemon_without_repairing_existing_roots() {
    let owner = super::super::FixtureRoot::new();
    let parent = owner.dir("setup");
    let exposed = parent.join("exposed");
    resources::ensure_probe_directory(&exposed).unwrap();
    // Model normal permissive directory creation without changing process umask.
    fs::set_permissions(&exposed, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(create_private_storage(&exposed).is_err());
    assert_eq!(
        fs::metadata(&exposed).unwrap().permissions().mode() & 0o777,
        0o755
    );

    let storage = parent.join("storage");
    create_private_storage(&storage).unwrap();
    assert_eq!(fs::canonicalize(&storage).unwrap(), storage);
    assert_eq!(
        fs::metadata(&storage).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let daemon = Daemon::bind(
        DaemonConfig {
            root: storage.clone(),
            budget: Budget {
                cap_bytes: GIB,
                free_reserve_bytes: 0,
            },
        },
        Arc::new(super::super::FixtureAuthority),
        Arc::new(super::super::FixedFreeSpace(Some(10 * GIB))),
    )
    .expect("same live setup path supplies a private canonical root to the real bind");
    assert_eq!(daemon.socket_path(), storage.join("daemon.sock"));
    assert!(create_private_storage(&storage).is_err());
}

fn invocation(
    root: &Path,
    mount: &Path,
    run_root: &Path,
    label: &str,
    toolchain: &Toolchain,
    environment: &[EffectiveValue],
    tree: &str,
) -> ProjectInvocation {
    ProjectInvocation {
        descriptor: ProjectDescriptor {
            project_id: "oxide".into(),
            component_id: "cargo-same-root-fixture".into(),
            workspace_root: root.to_path_buf(),
            workflows: vec![WorkflowOptIn {
                workflow: freshness::WORKFLOW.into(),
                selector: freshness::SELECTOR.into(),
                profile: "test".into(),
                features: vec![],
                required_slot_bytes: GIB,
                cargo_config_keys: vec![],
                environment_keys: environment.iter().map(|value| value.name.clone()).collect(),
                path_identity: PathIdentity::SameRootCargoFixture {
                    id: SCOPE.into(),
                    root: mount.to_path_buf(),
                },
            }],
        },
        consumer_id: "oxide-local-probe".into(),
        request_id: format!("request-{label}"),
        task_id: format!("task-{label}"),
        workspace_id: format!("workspace-{label}"),
        workflow: freshness::WORKFLOW.into(),
        selector: freshness::SELECTOR.into(),
        commit: git(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &["rev-parse", "HEAD"],
        )
        .trim()
        .into(),
        source_tree_digest: tree.into(),
        target_dir: run_root.join("target"),
        capture_dir: run_root.join("capture"),
        inputs: EffectiveBuildInputs {
            cargo: toolchain.cargo.clone(),
            rustc: toolchain.rustc.clone(),
            host: "aarch64-apple-darwin".into(),
            target: "aarch64-apple-darwin".into(),
            cargo_configuration: EffectiveInputSet::Complete(vec![]),
            environment: EffectiveInputSet::Complete(environment.to_vec()),
            effective_rustflags: Some(vec![]),
            full_debug: true,
        },
        wait_once: false,
        private_fallback_allowed: false,
    }
}
fn old_mtime(path: &Path) -> Result<(), String> {
    File::options()
        .write(true)
        .open(path)
        .and_then(|file| {
            file.set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1)))
        })
        .map_err(|error| error.to_string())
}
fn locate_effigy() -> Result<PathBuf, String> {
    std::env::split_paths(&std::env::var_os("PATH").ok_or("PATH missing")?)
        .map(|path| path.join("effigy"))
        .find(|path| path.is_file())
        .ok_or("real Effigy launcher unavailable".into())
        .and_then(|path| fs::canonicalize(path).map_err(|error| error.to_string()))
}
fn validate_launch(
    effigy: &Path,
    launch: &Path,
    environment: &[EffectiveValue],
    setup: &Path,
    cancellation: &dyn CancellationProbe,
    label: &str,
    expected_command: &str,
) -> Result<(), String> {
    let stdout = setup.join(format!("effigy-launch-{label}.json"));
    let stderr = setup.join(format!("effigy-launch-{label}.stderr"));
    let mut command = Command::new(effigy);
    command
        .args([
            "--repo",
            launch.to_str().ok_or("launch path not UTF-8")?,
            "same-root:probe",
            "--plan",
            "--json",
        ])
        .env_clear()
        .envs(
            environment
                .iter()
                .filter_map(|value| Some((&value.name, value.value.as_ref()?))),
        )
        .stdout(
            File::options()
                .write(true)
                .create_new(true)
                .open(&stdout)
                .map_err(|error| error.to_string())?,
        )
        .stderr(
            File::options()
                .write(true)
                .create_new(true)
                .open(&stderr)
                .map_err(|error| error.to_string())?,
        );
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let started = Instant::now();
    loop {
        if [&stdout, &stderr]
            .iter()
            .any(|path| fs::metadata(path).map_or(true, |metadata| metadata.len() > 1024 * 1024))
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Effigy launch output limit exceeded".into());
        }
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            let bytes = fs::read(&stdout).map_err(|error| error.to_string())?;
            let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
            if !status.success()
                || bytes.len() > 1024 * 1024
                || !text.contains("same-root:probe")
                || !text.contains(expected_command)
            {
                return Err("real Effigy launch plan was not verified".into());
            }
            return Ok(());
        }
        if cancellation.is_cancelled() || started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Effigy launch validation timed out".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn regression_invocation(owner: &super::super::FixtureRoot) -> ProjectInvocation {
    let mount = owner.dir("same-root");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/cargo-source-map-probe");
    let frozen = resources::freeze_fixture(&source).unwrap();
    let root = mount.join("r");
    resources::prepare_fixture_root(&frozen, &root).unwrap();
    let run = owner.dir("run");
    for name in ["target", "capture"] {
        fs::create_dir(run.join(name)).unwrap();
    }
    let mut environment = vec![];
    for (name, suffix) in [
        ("HOME", "home"),
        ("CARGO_HOME", "cargo"),
        ("RUSTUP_HOME", "rustup"),
    ] {
        let directory = mount.join(suffix);
        fs::create_dir(&directory).unwrap();
        environment.push(EffectiveValue {
            name: name.into(),
            value: Some(directory.to_string_lossy().into_owned()),
        });
    }
    environment.extend([
        EffectiveValue {
            name: "PATH".into(),
            value: Some("/usr/bin:/bin".into()),
        },
        EffectiveValue {
            name: "RUSTUP_TOOLCHAIN".into(),
            value: Some("1.97.1".into()),
        },
        EffectiveValue {
            name: "CARGO_BUILD_JOBS".into(),
            value: Some("1".into()),
        },
    ]);
    let toolchain = Toolchain {
        cargo: ToolchainInput {
            version: "cargo 1.97.1 (c980f4866 2026-06-30)".into(),
            executable_digest: format!("sha256:{}", "a".repeat(64)),
        },
        rustc: ToolchainInput {
            version: "rustc 1.97.1 (8bab26f4f 2026-07-14)".into(),
            executable_digest: format!("sha256:{}", "b".repeat(64)),
        },
        channel: "1.97.1",
    };
    let mut invocation = invocation(
        &root,
        &mount,
        &run,
        "regression",
        &toolchain,
        &environment,
        &format!("sha256:{}", "f".repeat(64)),
    );
    invocation.descriptor.workflows[0].required_slot_bytes = 4096;
    invocation
}

#[test]
fn resolution_recomputes_tree_before_authorization_without_partition_per_revision() {
    use crate::project::resolve::{self, Resolution};
    let owner = super::super::FixtureRoot::new();
    let invocation = regression_invocation(&owner);
    let first = match resolve::resolve(&invocation) {
        Resolution::Ready(resolved) => resolved,
        _ => panic!("eligible fixture"),
    };
    assert_eq!(first.source_tree_digest, T0);
    assert_ne!(first.source_tree_digest, invocation.source_tree_digest);
    fs::write(
        invocation
            .descriptor
            .workspace_root
            .join("probe/marker.txt"),
        b"changed-marker\n",
    )
    .unwrap();
    let next = match resolve::resolve(&invocation) {
        Resolution::Ready(resolved) => resolved,
        _ => panic!("new eligible tree"),
    };
    assert_eq!(next.source_tree_digest, T1);
    assert_eq!(first.partition, next.partition);
    fs::write(
        invocation
            .descriptor
            .workspace_root
            .join("probe/Cargo.lock"),
        b"unknown package lock",
    )
    .unwrap();
    assert!(matches!(
        resolve::resolve(&invocation),
        Resolution::Refused(AdapterRefusal::InputUnknown)
    ));
}

#[test]
fn caller_success_claim_cannot_publish_a_live_map_or_release_writers() {
    struct Authority;
    impl crate::storage::EvidenceAuthority for Authority {
        fn verify_admission(&self, envelope: &RunEnvelope) -> bool {
            envelope.admission_reference.starts_with("local-admission-")
                && envelope.admission_credential.expose() == "admission-proof"
        }
        fn verify_settlement(&self, evidence: &SettlementEvidence) -> bool {
            evidence.consumer_credential.expose() == "consumer-proof"
        }
    }
    struct Space;
    impl FreeSpaceProbe for Space {
        fn available_bytes(&self, _: &Path) -> Result<Option<u64>, StorageError> {
            Ok(Some(10 * GIB))
        }
    }
    struct ForgedRunner;
    impl FixtureCommandRunner for ForgedRunner {
        fn execute(
            &mut self,
            plan: &FixtureExecutionPlan,
            _: &dyn CancellationProbe,
        ) -> Result<FixtureCommandOutput, BoundaryFailure> {
            fs::write(
                plan.command.capture_dir.join("stdout.log"),
                b"caller says tests passed",
            )
            .unwrap();
            Ok(FixtureCommandOutput::new(
                FixtureCommandStatus::Exited(0),
                vec!["stdout.log".into()],
                None,
                None,
                Some(7),
                Some(0),
                FixtureCompletionProof::Stopped {
                    reference: "caller-stopped".into(),
                    credential: Credential::new("caller-proof"),
                },
            ))
        }
    }
    let owner = super::super::FixtureRoot::new();
    let invocation = regression_invocation(&owner);
    let storage = owner.dir("storage");
    let daemon = Daemon::bind(
        DaemonConfig {
            root: storage.clone(),
            budget: Budget {
                cap_bytes: GIB,
                free_reserve_bytes: 0,
            },
        },
        Arc::new(Authority),
        Arc::new(Space),
    )
    .unwrap();
    let client = Client::new(daemon.socket_path());
    let (shutdown, receive) = mpsc::channel();
    let serving = std::thread::spawn(move || daemon.serve(receive));
    let observed = Arc::new(Mutex::new(None));
    let mut adapter = ProjectAdapter::new(ObservedStorage {
        client: client.clone(),
        acquired: Arc::clone(&observed),
    });
    let mut consumer = Consumer {
        run: "mock".into(),
        output: None,
    };
    let result = adapter.execute(
        &invocation,
        &mut consumer,
        &mut NoWait,
        &mut ForgedRunner,
        &super::super::NeverCancelled,
    );
    assert_eq!(result.disposition, ExecutionDisposition::Uncertain);
    assert!(consumer.output.is_none());
    let acquired = observed.lock().unwrap().clone().unwrap();
    let slot = acquired.slot_path.unwrap();
    assert!(slot.join("oxide-freshness-pending-v1").exists());
    assert!(!slot.join("oxide-freshness-v1.bin").exists());
    assert_eq!(
        client.inventory("mock-result").unwrap().slots[0].state,
        SlotState::Uncertain
    );
    shutdown.send(()).unwrap();
    serving.join().unwrap().unwrap();
}
