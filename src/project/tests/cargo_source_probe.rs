//! One deliberately ignored, task-owned Cargo source-map experiment.

use super::super::super::native;
use super::super::super::resolve::{
    self, EffectiveBuildInputs, EffectiveInputSet, EffectiveValue, PathIdentity, ProjectDescriptor,
    ProjectInvocation, Resolution, WorkflowOptIn,
};
use super::super::super::source_map;
use super::super::super::source_map_probe as producer;
use super::super::*;
use super::{git, toolchain, CargoWorkflowRunner, Toolchain};
use crate::storage::{
    AcquireRequest, Budget, CaptureReceipt, Client, ContainmentEvidence, Credential, Daemon,
    DaemonConfig, EvidenceAuthority, FreeSpaceProbe, PartitionKey, RunBinding, RunEnvelope,
    SettlementEvidence, SlotState, StorageError, TerminalOutcome, TerminalRecord,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[allow(dead_code)]
#[path = "../../../fixtures/cargo-source-map-probe/probe/build.rs"]
mod owned_probe_build_script;

const ROOT: &str = "/private/var/tmp/oxide-rust-probe-proof-613d9b91-v3";
const SESSION_ID: &str = "oxide-probe-session-613d9b91-v3";
const WORKFLOW: &str = "oxide-cargo-source-map-probe";
const SELECTOR: &str =
    "test --offline --locked --message-format=json-render-diagnostics -- --nocapture";
const ROOT_ID: &str = "oxide-rust-probe-proof-613d9b91-v3";
const LOCAL_TASK_ID: &str = "oxide-local-probe-proof-613d9b91-v3";
const LOCAL_WORKSPACE_ID: &str = "oxide-local-probe-image-proof-613d9b91-v3";
const LOGICAL_PREFIX: &str = "/oxide-source-map-probe";
const MARKER_PATH: &str = "probe/marker.txt";
const BASELINE_MARKER: &str = "baseline-marker\n";
const CHANGED_MARKER: &str = "changed-marker\n";
const FROZEN_FIXTURE_TREE_DIGEST: &str =
    "sha256:550e62cefc7f647fef97b12489fc2c1e1ba67fd56a7529401fce25c97b0ce2e9";
const GIB: u64 = 1024 * 1024 * 1024;
const MAX_SESSION: Duration = Duration::from_secs(20 * 60);
const MAX_INVOCATIONS: u32 = 4;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);
const HOST_RESERVE: u64 = 20 * GIB;
const IMAGE_CAP: u64 = 8 * GIB;
const DISPOSABLE_CAP: u64 = 10 * GIB;
const SLOT_RESERVATION: u64 = GIB;

const ENVIRONMENT_KEYS: [&str; 14] = [
    "CARGO_BUILD_JOBS",
    "CARGO_PROFILE_DEV_DEBUG",
    "CARGO_PROFILE_DEV_SPLIT_DEBUGINFO",
    "CARGO_PROFILE_DEV_STRIP",
    "CARGO_PROFILE_TEST_DEBUG",
    "CARGO_PROFILE_TEST_SPLIT_DEBUGINFO",
    "CARGO_PROFILE_TEST_STRIP",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_HOME",
    "HOME",
    "PATH",
    "RUSTFLAGS",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
];

struct FrozenFixture {
    files: BTreeMap<PathBuf, Vec<u8>>,
    digest: String,
}

#[derive(Clone)]
struct SpaceSample {
    elapsed_ms: u128,
    host_free: u64,
    image_capacity: u64,
    image_used: u64,
    image_free: u64,
    image_allocated: u64,
    outside_allocated: u64,
}

struct ProbeBudget {
    mount: PathBuf,
    image: PathBuf,
    setup_temp: PathBuf,
    mounted: AtomicBool,
    outside_directories: AtomicU64,
    sample_lock: Mutex<()>,
    started: Instant,
    last_sample: Mutex<Instant>,
    samples: Mutex<Vec<SpaceSample>>,
    failure: Mutex<Option<String>>,
}

struct PeriodicSampler {
    stop: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl PeriodicSampler {
    fn start(interval: Duration, mut sample: impl FnMut() + Send + 'static) -> Self {
        let (stop, receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            while matches!(
                receiver.recv_timeout(interval),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                sample();
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for PeriodicSampler {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[test]
fn independent_sampler_runs_while_capture_blocks_over_a_sample_interval() {
    let samples = Arc::new(Mutex::new(Vec::new()));
    let collected = Arc::clone(&samples);
    let started = Instant::now();
    let monitor = PeriodicSampler::start(Duration::from_millis(20), move || {
        collected
            .lock()
            .expect("test samples")
            .push(started.elapsed());
    });
    // A bounded blocked helper models scanner/capture and setup waits without
    // running Cargo. The production timer uses the same mechanism at 20s.
    let output = Command::new("/bin/sh")
        .args(["-c", "sleep 0.12; printf captured"])
        .output()
        .expect("bounded test helper");
    assert!(output.status.success());
    drop(monitor);
    let samples = samples.lock().expect("test samples");
    assert!(
        samples.len() >= 2,
        "independent timer stalled behind helper"
    );
    assert!(samples.iter().all(|elapsed| *elapsed < started.elapsed()));
}

struct ProbeCancellation(Arc<ProbeBudget>);

impl CancellationProbe for ProbeCancellation {
    fn is_cancelled(&self) -> bool {
        if self
            .0
            .failure
            .lock()
            .expect("budget failure lock")
            .is_some()
        {
            return true;
        }
        if self.0.started.elapsed() >= MAX_SESSION {
            let reason = "the 20 minute session limit was reached".to_owned();
            if let Ok(mut failure) = self.0.failure.lock() {
                *failure = Some(reason.clone());
            }
            let _ = append_evidence(&self.0.mount, &format!("resource-stop\t{reason}\n"));
            return true;
        }
        let due = self
            .0
            .last_sample
            .lock()
            .expect("budget sample lock")
            .elapsed()
            >= SAMPLE_INTERVAL;
        due && self.0.sample().is_err()
    }
}

impl ProbeBudget {
    fn new(mount: PathBuf, image: PathBuf, setup_temp: PathBuf, started: Instant) -> Arc<Self> {
        Arc::new(Self {
            mount,
            image,
            setup_temp,
            mounted: AtomicBool::new(false),
            outside_directories: AtomicU64::new(0),
            sample_lock: Mutex::new(()),
            started,
            last_sample: Mutex::new(Instant::now() - SAMPLE_INTERVAL),
            samples: Mutex::new(Vec::new()),
            failure: Mutex::new(None),
        })
    }

    fn sample(&self) -> Result<SpaceSample, String> {
        let _guard = self
            .sample_lock
            .lock()
            .map_err(|_| "sample serialization lock poisoned")?;
        if let Some(reason) = self
            .failure
            .lock()
            .map_err(|_| "budget failure lock poisoned")?
            .as_ref()
        {
            return Err(reason.clone());
        }
        let gap = self
            .last_sample
            .lock()
            .map_err(|_| "sample clock lock poisoned")?
            .elapsed();
        let result = if gap > SAMPLE_INTERVAL
            && !self
                .samples
                .lock()
                .map_err(|_| "samples lock poisoned")?
                .is_empty()
        {
            Err(format!(
                "host sampling cadence exceeded 30 seconds: {} ms",
                gap.as_millis()
            ))
        } else {
            self.read_sample()
        };
        let result = result.and_then(|sample| {
            if self
                .samples
                .lock()
                .map_err(|_| "samples lock poisoned")?
                .last()
                .is_some_and(|previous| {
                    sample.elapsed_ms.saturating_sub(previous.elapsed_ms)
                        > SAMPLE_INTERVAL.as_millis()
                })
            {
                return Err("completed host samples exceeded 30-second cadence".to_owned());
            }
            self.write_sample(&sample)?;
            Ok(sample)
        });
        let mut last = self
            .last_sample
            .lock()
            .map_err(|_| "budget sample lock poisoned")?;
        *last = Instant::now();
        match result {
            Ok(sample) => {
                self.samples
                    .lock()
                    .map_err(|_| "budget samples lock poisoned")?
                    .push(sample.clone());
                Ok(sample)
            }
            Err(error) => {
                *self
                    .failure
                    .lock()
                    .map_err(|_| "budget failure lock poisoned")? = Some(error.clone());
                let _ = append_evidence(&self.mount, &format!("resource-stop\t{error}\n"));
                let _ = fs::write(
                    self.setup_temp.join("resource-stop.txt"),
                    format!(
                        "elapsed_ms={}\n{error}\n",
                        self.started.elapsed().as_millis()
                    ),
                );
                Err(error)
            }
        }
    }

    fn read_sample(&self) -> Result<SpaceSample, String> {
        if self.started.elapsed() >= MAX_SESSION {
            return Err("the 20 minute session limit was reached".to_owned());
        }
        let host_free = native::host_free_bytes(Path::new("/private/var/tmp"))
            .map_err(|_| "host free-space observation unavailable".to_owned())?;
        if host_free < HOST_RESERVE {
            return Err(format!("host free space fell below 20 GiB: {host_free}"));
        }
        let (image_capacity, image_used, image_free) = if self.mounted.load(Ordering::Acquire) {
            let (_, capacity, used, free) = df_space(&self.mount)?;
            (capacity, used, free)
        } else {
            (0, 0, 0)
        };
        if image_capacity > IMAGE_CAP || image_used > IMAGE_CAP || image_free > image_capacity {
            return Err(format!(
                "APFS volume exceeded or obscured its 8 GiB bound: capacity={image_capacity} used={image_used} free={image_free}"
            ));
        }
        let image_allocated = if self.image.exists() {
            du_bytes(&self.image)?
        } else {
            0
        };
        let root = self.image.parent().ok_or("image has no parent")?;
        let outside_allocated = allocated_blocks(root)?
            .max(self.outside_directories.load(Ordering::Acquire))
            .saturating_add(du_bytes(&self.setup_temp)?);
        let disposable = image_allocated.saturating_add(outside_allocated);
        if disposable > DISPOSABLE_CAP {
            return Err(format!("10 GiB disposable ceiling exceeded: {disposable}"));
        }
        let expected = ["m", "oxide-rust-probe.sparseimage", "setup-tmp"];
        let mut names = fs::read_dir(root)
            .map_err(|error| error.to_string())?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        names.sort();
        if names.iter().any(|name| !expected.contains(&name.as_str())) {
            return Err(format!("unexpected disposable-root entries: {names:?}"));
        }
        Ok(SpaceSample {
            elapsed_ms: self.started.elapsed().as_millis(),
            host_free,
            image_capacity,
            image_used,
            image_free,
            image_allocated,
            outside_allocated,
        })
    }

    fn write_sample(&self, sample: &SpaceSample) -> Result<(), String> {
        let line = format!(
            "space\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            machine_utc()?,
            sample.elapsed_ms,
            sample.host_free,
            sample.image_capacity,
            sample.image_used,
            sample.image_free,
            sample.image_allocated,
            sample.outside_allocated,
        );
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.setup_temp.join("host-space-samples.tsv"))
            .map_err(|error| error.to_string())?;
        file.write_all(line.as_bytes())
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())
    }
}

#[test]
fn probe_plan_passes_runner_validation_before_acquire_and_rejects_forced_overrides() {
    let root = super::super::FixtureRoot::new();
    let trial = root.dir("trial");
    let workspace = trial.join("workspace");
    let target = trial.join("target");
    let capture = trial.join("capture");
    let temp = trial.join("tmp");
    let home = trial.join("home");
    let cargo_home = trial.join("cargo-home");
    for path in [&workspace, &target, &capture, &temp, &home, &cargo_home] {
        fs::create_dir(path).expect("probe validation directory");
    }
    fs::write(workspace.join("Cargo.toml"), b"[workspace]\n").expect("workspace marker");
    let environment = probe_environment(&trial, "1.97.1", None);
    let cargo = toolchain_binary("1.97.1", "cargo");
    let runner = CargoWorkflowRunner::new(trial.clone(), PathBuf::from("/private/var/tmp"), cargo)
        .expect("registered runner");
    let tree = format!("sha256:{}", "a".repeat(64));
    let binding = RunBinding {
        consumer: "oxide-local-probe".to_owned(),
        project: "oxide".to_owned(),
        component: "cargo-source-map-probe".to_owned(),
        workflow: WORKFLOW.to_owned(),
        task_id: LOCAL_TASK_ID.to_owned(),
        workspace_id: LOCAL_WORKSPACE_ID.to_owned(),
        run_id: "run-launch-contract".to_owned(),
        attempt_id: "attempt-1".to_owned(),
        commit: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        source_tree_digest: tree,
    };
    let host = HostRunIdentity {
        id: "host-launch-contract".to_owned(),
    };
    let plan = probe_execution_plan(
        &host,
        &workspace,
        &target,
        &target,
        &capture,
        &temp,
        None,
        &environment,
        &binding,
        "unacquired",
        "unacquired",
    );
    runner
        .validate_plan(&plan, &super::super::NeverCancelled)
        .expect("valid probe plan must pass pre-launch validation");

    let mut tmpdir_override = plan.clone();
    tmpdir_override.command.environment.push(EffectiveValue {
        name: "TMPDIR".to_owned(),
        value: Some(temp.to_string_lossy().into_owned()),
    });
    assert_eq!(
        runner.validate_plan(&tmpdir_override, &super::super::NeverCancelled),
        Err(BoundaryFailure::Rejected)
    );

    let mut target_override = plan;
    target_override.command.environment.push(EffectiveValue {
        name: "CARGO_TARGET_DIR".to_owned(),
        value: Some(target.to_string_lossy().into_owned()),
    });
    assert_eq!(
        runner.validate_plan(&target_override, &super::super::NeverCancelled),
        Err(BoundaryFailure::Rejected)
    );
}

#[test]
fn session_clock_persists_original_machine_timestamps() {
    let root = super::super::FixtureRoot::new();
    let setup_temp = root.dir("setup-tmp");
    record_session_clock(&setup_temp, "2026-10-10T13:00:00Z", "2026-10-10T13:20:00Z")
        .expect("session clock");
    let body = fs::read_to_string(setup_temp.join("session-clock.txt")).expect("clock file");
    assert!(body.contains("clock=std::time::Instant\n"));
    assert!(body.contains("start_elapsed_ms=0\ndeadline_elapsed_ms=1200000\n"));
    assert!(body.contains("start_utc=2026-10-10T13:00:00Z\ndeadline_utc=2026-10-10T13:20:00Z\n"));
    assert!(body.contains("invocation_count=0\nmax_invocations=4\n"));
}

#[test]
fn live_session_deadline_uses_duration_scale_and_refuses_expired_elapsed() {
    assert_eq!(
        session_remaining(Duration::from_millis(5)),
        Ok(Duration::from_millis(1_199_995))
    );
    assert!(session_remaining(Duration::from_millis(1_200_000)).is_err());
    assert!(session_remaining(Duration::from_secs(1_200_000)).is_err());
    let started = Instant::now()
        .checked_sub(MAX_SESSION)
        .expect("test monotonic range");
    assert!(refuse_session_timeout(&started).is_err());
    let root = super::super::FixtureRoot::new();
    let temp = root.dir("clock");
    record_session_clock(&temp, "2026-10-10T13:00:00Z", "2026-10-10T13:20:00Z")
        .expect("initial clock");
    assert!(record_session_clock(&temp, "later", "later").is_err());
}

#[test]
fn source_probe_fixture_matches_the_frozen_tree() {
    let fixture_source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/cargo-source-map-probe");
    let frozen = freeze_fixture(&fixture_source).expect("freeze source-probe fixture");
    assert_eq!(frozen.digest, FROZEN_FIXTURE_TREE_DIGEST);
    verify_committed_fixture(&fixture_source, &frozen).expect("committed source-probe fixture");
}

#[test]
#[ignore = "approved four-invocation Cargo evidence probe; run only with OXIDE_CARGO_SOURCE_PROBE_RUN=1"]
fn oxide_cargo_source_map_probe() {
    if std::env::var("OXIDE_CARGO_SOURCE_PROBE_RUN").as_deref() != Ok("1") {
        return;
    }
    if let Err(error) = run_probe() {
        panic!("bounded Cargo probe stopped: {error}");
    }
}

fn run_probe() -> Result<(), String> {
    let started = Instant::now();
    let start_utc = machine_utc()?;
    let deadline_utc = machine_date(&[
        "-u",
        "-r",
        &(machine_epoch_utc(&start_utc)? + MAX_SESSION.as_secs()).to_string(),
        "+%Y-%m-%dT%H:%M:%SZ",
    ])?;
    // create_dir refuses any earlier session, including an empty retained root.
    fs::create_dir(ROOT)
        .map_err(|error| format!("refusing existing or unavailable v3 root: {error}"))?;
    let setup_temp = PathBuf::from(ROOT).join("setup-tmp");
    fs::create_dir(&setup_temp).map_err(|error| error.to_string())?;
    record_session_clock(&setup_temp, &start_utc, &deadline_utc)?;
    let image = PathBuf::from(ROOT).join("oxide-rust-probe.sparseimage");
    let mount = PathBuf::from(ROOT).join("m");
    let budget = ProbeBudget::new(mount.clone(), image.clone(), setup_temp.clone(), started);
    budget.sample()?;
    let monitor_budget = Arc::clone(&budget);
    let monitor = PeriodicSampler::start(Duration::from_secs(20), move || {
        let _ = monitor_budget.sample();
    });
    refuse_session_timeout(&started)?;
    let host_free = native::host_free_bytes(Path::new("/private/var/tmp"))
        .map_err(|_| "host free-space preflight unavailable".to_owned())?;
    if host_free < HOST_RESERVE {
        return Err(format!(
            "host free space below the 20 GiB start threshold: {host_free}"
        ));
    }
    refuse_session_timeout(&started)?;
    let fixture_source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/cargo-source-map-probe");
    let frozen = freeze_fixture(&fixture_source)?;
    verify_committed_fixture(&fixture_source, &frozen)?;
    refuse_session_timeout(&started)?;
    let scanner_identity = producer::scanner_identity()?;
    refuse_session_timeout(&started)?;
    let (attach, outside_directories) = setup_image(&image, &mount, &setup_temp)?;
    budget
        .outside_directories
        .store(outside_directories, Ordering::Release);
    budget.mounted.store(true, Ordering::Release);
    refuse_session_timeout(&started)?;
    let evidence = mount.join("evidence");
    ensure_probe_directory(&evidence)?;
    ensure_probe_directory(&mount.join("tmp"))?;
    ensure_probe_directory(&mount.join("home"))?;
    ensure_probe_directory(&mount.join("cargo-home"))?;

    let setup_sample = budget.sample()?;
    append_evidence(
        &mount,
        &format!(
            "mount-preflight\tdevice={attach}\thost_free={}\timage_capacity={}\timage_used={}\timage_free={}\timage_allocated={}\toutside_allocated={}\n",
            setup_sample.host_free,
            setup_sample.image_capacity,
            setup_sample.image_used,
            setup_sample.image_free,
            setup_sample.image_allocated,
            setup_sample.outside_allocated,
        ),
    )?;
    append_evidence(
        &mount,
        &format!(
            "session\tstart_utc={start_utc}\tdeadline_utc={deadline_utc}\tobserved_start_utc={}\telapsed_ms={}\tfixture_tree={}\nprofile\ttest/dev debug=2 strip=none split-debuginfo=packed\nscanner-provenance\n{}",
            machine_utc()?,
            started.elapsed().as_millis(),
            frozen.digest,
            scanner_identity
        ),
    )?;

    let a = mount.join("a");
    let b = mount.join("b");
    prepare_fixture_root(&frozen, &a)?;
    prepare_fixture_root(&frozen, &b)?;
    if producer::source_tree_digest(&a)? != frozen.digest
        || producer::source_tree_digest(&b)? != frozen.digest
        || producer::source_tree_digest(&fixture_source)? != frozen.digest
    {
        return Err("fixture bytes changed while the disposable roots were prepared".to_owned());
    }
    budget.sample()?;
    let stable = toolchain("1.97.1");
    let alternate = toolchain("1.96.1");
    if !stable.cargo.version.starts_with("cargo 1.97.1")
        || !stable.rustc.version.starts_with("rustc 1.97.1")
    {
        return Err("the exact pinned Cargo/rustc 1.97.1 tools are not available".to_owned());
    }
    if !alternate.cargo.version.starts_with("cargo 1.96.1")
        || !alternate.rustc.version.starts_with("rustc 1.96.1")
    {
        return Err("the preinstalled Cargo/rustc 1.96.1 key control is unavailable".to_owned());
    }
    append_evidence(
        &mount,
        &format!(
            "toolchain\tbaseline\tchannel={}\tcargo={}\tcargo_sha256={}\trustc={}\trustc_sha256={}\nalternate\tchannel={}\tcargo={}\tcargo_sha256={}\trustc={}\trustc_sha256={}\n",
            stable.channel,
            stable.cargo.version,
            stable.cargo.executable_digest,
            stable.rustc.version,
            stable.rustc.executable_digest,
            alternate.channel,
            alternate.cargo.version,
            alternate.cargo.executable_digest,
            alternate.rustc.version,
            alternate.rustc.executable_digest,
        ),
    )?;
    let cargo = toolchain_binary("1.97.1", "cargo");
    let mut runner =
        CargoWorkflowRunner::new(mount.clone(), PathBuf::from("/private/var/tmp"), cargo)
            .map_err(|error| format!("could not create registered runner: {error:?}"))?;
    let storage_root = mount.join("storage");
    ensure_probe_directory(&storage_root)?;
    fs::set_permissions(&storage_root, fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let daemon = ProbeDaemon::start(&storage_root, Arc::clone(&budget));
    let client = daemon.client();
    let (preflight_partition, preflight_source_map) = preflight_partition(&mount, &a, &b, &stable)?;
    let env = probe_environment(&mount, "1.97.1", Some(&preflight_source_map));
    let mut invocations = 0u32;

    let a_tree = producer::source_tree_digest(&a)?;
    let run1 = execute_owned(
        &mount,
        &client,
        &mut runner,
        &ProbeCancellation(Arc::clone(&budget)),
        &preflight_partition,
        None,
        &a,
        &a_tree,
        &env,
        "reference-a",
        &mut invocations,
    )?;
    budget.sample()?;
    if !run1.settled_idle || run1.exit_code != Some(0) {
        return stop_after_settled_runs(
            &mount,
            &client,
            daemon,
            &attach,
            &budget,
            started,
            "cold A reference did not complete cleanly",
        );
    }
    if !run1.preflight_scans_complete {
        append_evidence(
            &mount,
            "stop\tpreflight A binary scanners were incomplete\n",
        )?;
        return finish_if_idle(
            &mount,
            &client,
            daemon,
            &attach,
            &budget,
            "preflight A scanner coverage incomplete",
        );
    }
    if !run1.probe_values_match {
        append_evidence(
            &mount,
            "stop\tA probe output did not match the independently read source bytes\n",
        )?;
        return finish_if_idle(
            &mount,
            &client,
            daemon,
            &attach,
            &budget,
            "cold A output mismatch",
        );
    }
    let control_slot = run1.slot_path.clone();

    // Step 2: one resource-only marker change at B under the same preflight lease key.
    let marker_b = b.join(MARKER_PATH);
    fs::write(&marker_b, CHANGED_MARKER).map_err(|error| error.to_string())?;
    let changed_tree = producer::source_tree_digest(&b)?;
    if changed_tree == frozen.digest {
        return Err("the controlled B source change did not change the tree digest".to_owned());
    }
    append_evidence(
        &mount,
        &format!(
            "source-miss-patch\t{MARKER_PATH}\tbaseline={}\tchanged={}\tbefore={}\tafter={}\n",
            hex_digest(BASELINE_MARKER.as_bytes()),
            hex_digest(CHANGED_MARKER.as_bytes()),
            frozen.digest,
            changed_tree
        ),
    )?;
    let run2 = execute_owned(
        &mount,
        &client,
        &mut runner,
        &ProbeCancellation(Arc::clone(&budget)),
        &preflight_partition,
        None,
        &b,
        &changed_tree,
        &env,
        "source-miss-b",
        &mut invocations,
    )?;
    budget.sample()?;
    let stdout2 =
        fs::read(run2.capture_dir.join("stdout.log")).map_err(|error| error.to_string())?;
    let summary2 = producer::inspect_cargo_output(&stdout2);
    if !run2.settled_idle {
        append_evidence(
            &mount,
            "source-miss\townership-or-capture-uncertain; B retained\n",
        )?;
        return Err("source-miss execution did not settle; preserved the image and B".to_owned());
    }
    let rebuilt_probe = summary2.artifacts.iter().any(|artifact| {
        artifact.manifest.starts_with(&b)
            && artifact.target_kinds.iter().any(|kind| kind == "test")
            && !artifact.fresh
    });
    let rebuilt_build_script = summary2.artifacts.iter().any(|artifact| {
        artifact.manifest.starts_with(&b)
            && artifact
                .target_kinds
                .iter()
                .any(|kind| kind == "custom-build")
            && !artifact.fresh
    });
    let source_miss_pass = run2.exit_code == Some(0)
        && rebuilt_probe
        && rebuilt_build_script
        && run2.preflight_scans_complete
        && run2.probe_values_match;
    if !source_miss_pass {
        append_evidence(
            &mount,
            &format!(
                "source-miss\tfail\trebuilt_probe={rebuilt_probe}\trebuilt_build_script={rebuilt_build_script}\texit={:?}\n",
                run2.exit_code
            ),
        )?;
    } else {
        append_evidence(&mount, "source-miss\tpass\n")?;
    }
    // Restore only after the B run settled and its private capture was finalized.
    fs::write(&marker_b, BASELINE_MARKER).map_err(|error| error.to_string())?;
    if producer::source_tree_digest(&b)? != frozen.digest {
        return Err("B did not restore to the frozen fixture tree".to_owned());
    }
    append_evidence(&mount, "source-miss-restore\tB equals frozen tree\n")?;
    if !source_miss_pass {
        return finish_if_idle(
            &mount,
            &client,
            daemon,
            &attach,
            &budget,
            "source invalidation control failed",
        );
    }
    if run2.slot_path != control_slot || !run2.reused_slot {
        append_evidence(
            &mount,
            "source-miss\tfail\tcontrol intermediate slot changed\n",
        )?;
        return finish_if_idle(
            &mount,
            &client,
            daemon,
            &attach,
            &budget,
            "source control did not reuse its exclusive slot",
        );
    }

    // Step 3: real resolver key-separation controls, without compiling.
    let controls = match key_controls(
        &mount,
        &a,
        &frozen.digest,
        &stable,
        &alternate,
        &env,
        &preflight_source_map,
    ) {
        Ok(controls) => controls,
        Err(error) => {
            append_evidence(&mount, &format!("key-control\tinsufficient\t{error}\n"))?;
            return finish_if_idle(
                &mount,
                &client,
                daemon,
                &attach,
                &budget,
                "key-separation control unavailable",
            );
        }
    };
    append_evidence(&mount, &controls.log)?;
    let source_control = source_map::SourceMissControl {
        before_tree_digest: frozen.digest.clone(),
        after_tree_digest: changed_tree,
        partition_digest: partition_digest(&preflight_partition),
        rebuilt_units: summary2.rebuilt_units as u32,
    };
    let miss_controls = source_map::MissControlEvidence {
        source: Some(source_control),
        configuration: Some(controls.configuration),
        toolchain: Some(controls.toolchain),
    };
    let policy = source_policy(&a, &b, miss_controls.clone());
    let mut mapped_invocation = invocation(
        &mount,
        &a,
        &frozen.digest,
        &stable,
        &env,
        Some(policy.clone()),
        "shared-cold-a",
    )?;
    let mapped_a = resolve::resolve(&mapped_invocation);
    let mut mapped_a = match mapped_a {
        Resolution::Ready(resolved) => resolved,
        _ => {
            return Err(
                "mapped A source-map partition did not resolve after measured controls".to_owned(),
            )
        }
    };
    let mapped_partition_digest = partition_digest(&mapped_a.partition);
    if mapped_partition_digest != partition_digest(&preflight_partition) {
        append_evidence(
            &mount,
            "control\tinsufficient\tpreflight and mapped partition identities differ\n",
        )?;
        return finish_if_idle(
            &mount,
            &client,
            daemon,
            &attach,
            &budget,
            "preflight map key did not match the registered map",
        );
    }
    mapped_invocation.descriptor.workspace_root = b.clone();
    mapped_invocation.source_tree_digest = frozen.digest.clone();
    mapped_invocation.request_id = "resolve-warm-b".to_owned();
    mapped_invocation.target_dir = mount.join("runs/resolve-warm-b/target");
    mapped_invocation.capture_dir = mount.join("runs/resolve-warm-b/capture");
    fs::create_dir_all(&mapped_invocation.target_dir).map_err(|error| error.to_string())?;
    fs::create_dir_all(&mapped_invocation.capture_dir).map_err(|error| error.to_string())?;
    let resolved_b = match resolve::resolve(&mapped_invocation) {
        Resolution::Ready(resolved) => resolved,
        _ => return Err("mapped B did not resolve under the same frozen root map".to_owned()),
    };
    if resolved_b.partition != mapped_a.partition {
        return Err("registered mapped A/B partitions differ".to_owned());
    }
    let map_a = mapped_a
        .source_map
        .take()
        .ok_or_else(|| "mapped A lost source map".to_owned())?;
    let map_b = resolved_b
        .source_map
        .ok_or_else(|| "mapped B lost source map".to_owned())?;
    if map_a.effective_rustflags() != preflight_source_map.effective_rustflags() {
        return Err("preflight Cargo flags differ from the resolved mapped A flags".to_owned());
    }
    if map_a.policy_digest() != map_b.policy_digest()
        || map_a.map_digest() != map_b.map_digest()
        || map_a.build_identity_digest() != map_b.build_identity_digest()
        || map_a.authorized_roots() != map_b.authorized_roots()
        || map_a.logical_prefix() != map_b.logical_prefix()
        || map_a.effective_rustflags() != map_b.effective_rustflags()
    {
        return Err("resolved A/B source maps differ".to_owned());
    }
    append_evidence(
        &mount,
        &format!(
            "source-map\tpolicy={}\tmap={}\tbuild_identity={}\tlogical_prefix={}\n",
            map_a.policy_digest(),
            map_a.map_digest(),
            map_a.build_identity_digest(),
            LOGICAL_PREFIX
        ),
    )?;

    // Step 4: shared-slot cold A, with the actual registered Cargo producer.
    let shared_partition = mapped_a.partition.clone();
    let mapped_env = env.clone();
    let run4 = execute_owned(
        &mount,
        &client,
        &mut runner,
        &ProbeCancellation(Arc::clone(&budget)),
        &shared_partition,
        Some(map_a),
        &a,
        &frozen.digest,
        &mapped_env,
        "shared-cold-a",
        &mut invocations,
    )?;
    budget.sample()?;
    append_qualification(&mount, &run4, "shared-cold-a")?;
    if !run4.probe_values_match
        || run4.qualification != Some(source_map::QualificationDisposition::Pass)
        || !run4.settled_idle
        || run4.exit_code != Some(0)
    {
        return Err(
            "shared cold A did not pass the observed path predicate; lease/image retained"
                .to_owned(),
        );
    }

    // Step 5: same slot at B. A stale value or unobservable path is a stop, not a hit.
    let run5 = execute_owned(
        &mount,
        &client,
        &mut runner,
        &ProbeCancellation(Arc::clone(&budget)),
        &shared_partition,
        Some(map_b),
        &b,
        &frozen.digest,
        &mapped_env,
        "warm-b",
        &mut invocations,
    )?;
    budget.sample()?;
    append_qualification(&mount, &run5, "warm-b")?;
    if run5.slot_path != run4.slot_path || !run5.reused_slot {
        return Err("warm B did not reacquire the same durable slot".to_owned());
    }
    if !run5.probe_values_match
        || run5.qualification != Some(source_map::QualificationDisposition::Pass)
        || !run5.settled_idle
        || run5.exit_code != Some(0)
    {
        append_evidence(
            &mount,
            "reuse-result\tfail-or-insufficient\tno reuse claim\n",
        )?;
        return Err("warm B path or runtime predicate failed; lease/image retained".to_owned());
    }

    let inventory = client
        .inventory("oxide-probe-final-inventory")
        .map_err(|error| error.to_string())?;
    if inventory
        .slots
        .iter()
        .any(|slot| slot.state != SlotState::Idle)
    {
        return Err("an unsettled lease remains; image stays mounted and retained".to_owned());
    }
    append_evidence(
        &mount,
        &format!(
            "reuse-result\tpass\tshared_slot={}\trebuilt_cold={}\trebuilt_warm={}\telapsed_ms={}\n",
            run4.slot_id,
            run4.rebuilt_units,
            run5.rebuilt_units,
            started.elapsed().as_millis()
        ),
    )?;
    budget.sample()?;
    drop(daemon);
    drop(monitor);
    detach_image(&attach)?;
    refuse_session_timeout(&started)?;
    Ok(())
}

struct OwnedRun {
    capture_dir: PathBuf,
    slot_path: PathBuf,
    slot_id: String,
    exit_code: Option<i32>,
    rebuilt_units: u32,
    reused_slot: bool,
    settled_idle: bool,
    qualification: Option<source_map::QualificationDisposition>,
    probe_values_match: bool,
    preflight_scans_complete: bool,
}

#[test]
fn cargo_encoded_rustflags_follow_the_resolved_source_map_order() {
    let root = super::super::FixtureRoot::new();
    let disposable = root.dir("resolved-map-flags");
    let a = disposable.join("a");
    let b = disposable.join("b");
    for workspace in [&a, &b] {
        let package = workspace.join("probe");
        fs::create_dir_all(&package).expect("workspace package directory");
        fs::write(
            package.join("Cargo.toml"),
            b"[package]\nname = \"probe\"\nversion = \"0.1.0\"\n",
        )
        .expect("package manifest");
    }
    let tree = format!("sha256:{}", "a".repeat(64));
    let resolved = source_map::resolve(
        &source_policy(&a, &b, identity_only_miss_controls_for_key_preview()),
        &disposable,
        &a,
        &tree,
        Some(&[]),
        true,
    )
    .expect("registered source map resolves");

    let environment = probe_environment(&disposable, "1.97.1", Some(&resolved));
    let encoded = environment
        .iter()
        .find(|value| value.name == "CARGO_ENCODED_RUSTFLAGS")
        .and_then(|value| value.value.as_deref())
        .expect("the fixture passes an explicit Cargo rustflags value");
    let cargo_flags = encoded
        .split('\u{1f}')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(cargo_flags, resolved.effective_rustflags());
}

fn probe_execution_plan(
    host: &HostRunIdentity,
    workspace: &Path,
    cargo_build_dir: &Path,
    cargo_target_dir: &Path,
    capture_dir: &Path,
    temp_dir: &Path,
    source_map: Option<source_map::ResolvedSourceMap>,
    environment: &[EffectiveValue],
    binding: &RunBinding,
    lease_id: &str,
    slot_id: &str,
) -> FixtureExecutionPlan {
    FixtureExecutionPlan {
        host_run: host.clone(),
        command: HostRunRequest {
            selector: SELECTOR.to_owned(),
            workflow: WORKFLOW.to_owned(),
            profile: "test".to_owned(),
            features: Vec::new(),
            workspace_root: workspace.to_path_buf(),
            cargo_build_dir: cargo_build_dir.to_path_buf(),
            cargo_target_dir: cargo_target_dir.to_path_buf(),
            capture_dir: capture_dir.to_path_buf(),
            source_map,
            environment: environment.to_vec(),
        },
        claim: native::NativeExecutionClaim {
            lease_id: lease_id.to_owned(),
            slot_id: slot_id.to_owned(),
            slot_path: cargo_build_dir.to_path_buf(),
            temp_dir: temp_dir.to_path_buf(),
            capture_dir: capture_dir.to_path_buf(),
            binding: binding.clone(),
            host_run_id: host.id.clone(),
            workflow: WORKFLOW.to_owned(),
            selector: SELECTOR.to_owned(),
        },
    }
}

fn execute_owned(
    mount: &Path,
    client: &Client,
    runner: &mut CargoWorkflowRunner,
    cancellation: &dyn CancellationProbe,
    partition: &PartitionKey,
    map: Option<source_map::ResolvedSourceMap>,
    workspace: &Path,
    tree: &str,
    environment: &[EffectiveValue],
    run: &str,
    invocations: &mut u32,
) -> Result<OwnedRun, String> {
    let run_root = mount.join("runs").join(run);
    let target_dir = run_root.join("target");
    let capture_dir = run_root.join("capture");
    fs::create_dir_all(&target_dir).map_err(|error| error.to_string())?;
    fs::create_dir_all(&capture_dir).map_err(|error| error.to_string())?;
    let binding = RunBinding {
        consumer: "oxide-local-probe".to_owned(),
        project: "oxide".to_owned(),
        component: "cargo-source-map-probe".to_owned(),
        workflow: WORKFLOW.to_owned(),
        task_id: LOCAL_TASK_ID.to_owned(),
        workspace_id: LOCAL_WORKSPACE_ID.to_owned(),
        run_id: format!("run-{run}"),
        attempt_id: "attempt-1".to_owned(),
        commit: git(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &["rev-parse", "HEAD"],
        )
        .trim()
        .to_owned(),
        source_tree_digest: tree.to_owned(),
    };
    let host = HostRunIdentity {
        id: format!("host-{run}"),
    };
    let temp_dir = mount.join("tmp");
    let validation_plan = probe_execution_plan(
        &host,
        workspace,
        &target_dir,
        &target_dir,
        &capture_dir,
        &temp_dir,
        map.clone(),
        environment,
        &binding,
        "unacquired",
        "unacquired",
    );
    runner
        .validate_plan(&validation_plan, cancellation)
        .map_err(|error| format!("{run} failed pre-launch validation: {error:?}"))?;
    if *invocations >= MAX_INVOCATIONS {
        return Err(format!(
            "{run} refused: the four-invocation Cargo cap was already used"
        ));
    }
    let acquire = client
        .acquire(AcquireRequest {
            envelope: RunEnvelope {
                binding: binding.clone(),
                partition: partition.clone(),
                admission_reference: format!("local-admission-{run}"),
                admission_credential: Credential::new("admission-proof"),
            },
            acquire_id: format!("acquire-{run}"),
            required_bytes: SLOT_RESERVATION,
        })
        .map_err(|error| error.to_string())?;
    let (Some(slot_path), Some(token)) = (acquire.slot_path.clone(), acquire.token.clone()) else {
        return Err(format!("{run} acquired no active lease path"));
    };
    let plan = probe_execution_plan(
        &host,
        workspace,
        &slot_path,
        &target_dir,
        &capture_dir,
        &temp_dir,
        map.clone(),
        environment,
        &binding,
        &acquire.lease_id,
        &acquire.slot_id,
    );
    if cancellation.is_cancelled() {
        return Err("probe stopped before invocation at a resource/session bound".to_owned());
    }
    *invocations += 1;
    append_evidence(mount, &format!("cargo-invocation\t{invocations}\t{run}\n"))?;
    let output = match runner.execute(&plan, cancellation) {
        Ok(output) => output,
        Err(error) => {
            append_evidence(
                mount,
                &format!(
                    "run-stop\t{run}\t{error:?}\tlease={} retained\n",
                    acquire.lease_id
                ),
            )?;
            return Err(format!(
                "{run} command/capture outcome is uncertain: {error:?}"
            ));
        }
    };
    let exit_code = match output.status {
        FixtureCommandStatus::Exited(code) => Some(code),
        FixtureCommandStatus::Cancelled | FixtureCommandStatus::Incomplete => None,
    };
    let qualification = match (
        &map,
        output.source_map_receipt.as_ref(),
        output.portability_evidence.as_ref(),
    ) {
        (Some(map), Some(receipt), Some(evidence))
            if output
                .native_completion
                .as_ref()
                .is_some_and(|completion| completion.binds_source_map_receipt(receipt)) =>
        {
            Some(source_map::qualify(map, receipt, evidence))
        }
        (Some(_), _, _) => Some(source_map::QualificationDisposition::Insufficient),
        (None, _, _) => None,
    };
    let (probe_values_match, observation_report) =
        verify_probe_values(&output, &capture_dir, workspace, mount, tree)?;
    append_evidence(
        mount,
        &format!("probe-observations\t{run}\t{observation_report}"),
    )?;
    let preflight_scans_complete = if map.is_none() {
        fs::read_to_string(capture_dir.join("preflight-observations.txt"))
            .is_ok_and(|report| report.lines().any(|line| line == "scan_complete=true"))
    } else {
        true
    };
    let capture_digest =
        native::cargo_capture_receipt_digest(&capture_dir, &output.output_references)
            .map_err(|error| format!("{run} private capture digest failed: {error:?}"))?;
    let capture = CaptureReceipt {
        reference: format!("evidence/{run}"),
        digest: capture_digest,
        host_run_id: host.id.clone(),
        run_id: binding.run_id.clone(),
        attempt_id: binding.attempt_id.clone(),
        credential: Credential::new("capture-proof"),
    };
    let native_completion = output.native_completion.as_ref();
    let qualifies = qualification
        .is_none_or(|result| result == source_map::QualificationDisposition::Pass)
        && exit_code == Some(0)
        && probe_values_match;
    let containment = if qualifies {
        native_completion
            .and_then(|proof| {
                proof.settlement_evidence(
                    &acquire.lease_id,
                    &acquire.slot_id,
                    &slot_path,
                    Some(&host),
                    &capture,
                    &token,
                )
            })
            .map(ContainmentEvidence::CooperativeWorkflow)
            .unwrap_or(ContainmentEvidence::Unknown)
    } else {
        ContainmentEvidence::Unknown
    };
    let evidence = SettlementEvidence {
        binding,
        consumer_credential: Credential::new("consumer-proof"),
        terminal: exit_code.map(|code| TerminalRecord {
            run_id: format!("run-{run}"),
            outcome: if code == 0 {
                TerminalOutcome::Passed
            } else {
                TerminalOutcome::Failed
            },
            credential: Credential::new("terminal-proof"),
        }),
        capture: Some(capture),
        containment,
    };
    let finalization = client
        .finalize(
            &acquire.lease_id,
            Some(&token),
            &format!("finalize-{run}"),
            evidence,
        )
        .map_err(|error| format!("{run} settlement failed: {error}"))?;
    let rebuilt_units = output.rebuilt_units.unwrap_or(0);
    append_evidence(
        mount,
        &format!(
            "run\t{run}\trun_id=run-{run}\tattempt=attempt-1\thost_run={}\tlease={}\tslot={}\tslot_reused={}\tslot_path={}\ttree={}\tpartition={}\texit={:?}\trebuilt={}\tqualification={:?}\tfinalization={:?}\n",
            host.id,
            acquire.lease_id,
            acquire.slot_id,
            acquire.reused,
            slot_path.display(),
            tree,
            partition_digest(partition),
            exit_code,
            rebuilt_units,
            qualification,
            finalization.state
        ),
    )?;
    Ok(OwnedRun {
        capture_dir,
        slot_path,
        slot_id: acquire.slot_id,
        exit_code,
        rebuilt_units,
        reused_slot: acquire.reused,
        settled_idle: finalization.state == SlotState::Idle,
        qualification,
        probe_values_match,
        preflight_scans_complete,
    })
}

struct ProbeDaemon {
    socket: PathBuf,
    shutdown: Option<Sender<()>>,
    thread: Option<JoinHandle<Result<(), StorageError>>>,
}

impl ProbeDaemon {
    fn start(root: &Path, free_space: Arc<ProbeBudget>) -> Self {
        let daemon = Daemon::bind(
            DaemonConfig {
                root: root.to_path_buf(),
                budget: Budget {
                    cap_bytes: 7 * GIB,
                    free_reserve_bytes: HOST_RESERVE,
                },
            },
            Arc::new(FixtureAuthority),
            free_space,
        )
        .expect("bind task-owned probe daemon");
        let socket = daemon.socket_path().to_path_buf();
        let (shutdown, receiver) = mpsc::channel();
        let thread = thread::spawn(move || daemon.serve(receiver));
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

impl Drop for ProbeDaemon {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct FixtureAuthority;

impl EvidenceAuthority for FixtureAuthority {
    fn verify_admission(&self, envelope: &RunEnvelope) -> bool {
        envelope.admission_reference.starts_with("local-admission-")
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
                .is_none_or(|capture| capture.credential.expose() == "capture-proof")
            && match &evidence.containment {
                ContainmentEvidence::CooperativeWorkflow(completion) => {
                    !completion.signature.is_empty()
                }
                _ => true,
            }
    }
}

impl FreeSpaceProbe for ProbeBudget {
    fn available_bytes(&self, _root: &Path) -> Result<Option<u64>, StorageError> {
        native::host_free_bytes(Path::new("/private/var/tmp"))
            .map(Some)
            .map_err(|_| {
                StorageError::Io("probe host free-space measurement unavailable".to_owned())
            })
    }
}

fn toolchain_binary(channel: &str, name: &str) -> PathBuf {
    let home = std::env::var_os("HOME").expect("HOME");
    PathBuf::from(home)
        .join(".rustup/toolchains")
        .join(format!("{channel}-aarch64-apple-darwin/bin/{name}"))
}

fn probe_environment(
    mount: &Path,
    channel: &str,
    resolved_source_map: Option<&source_map::ResolvedSourceMap>,
) -> Vec<EffectiveValue> {
    let toolchain_bin = toolchain_binary(channel, "cargo")
        .parent()
        .expect("toolchain bin")
        .to_path_buf();
    let home = mount.join("home").to_string_lossy().into_owned();
    let cargo_home = mount.join("cargo-home").to_string_lossy().into_owned();
    let rustup_home = std::env::var("RUSTUP_HOME").unwrap_or_else(|_| {
        PathBuf::from(std::env::var_os("HOME").expect("HOME"))
            .join(".rustup")
            .to_string_lossy()
            .into_owned()
    });
    let path = format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", toolchain_bin.display());
    let encoded = resolved_source_map
        .map(|map| map.effective_rustflags().join("\u{1f}"))
        .unwrap_or_default();
    [
        ("CARGO_BUILD_JOBS", None),
        ("CARGO_PROFILE_DEV_DEBUG", Some("2".to_owned())),
        (
            "CARGO_PROFILE_DEV_SPLIT_DEBUGINFO",
            Some("packed".to_owned()),
        ),
        ("CARGO_PROFILE_DEV_STRIP", Some("none".to_owned())),
        ("CARGO_PROFILE_TEST_DEBUG", Some("2".to_owned())),
        (
            "CARGO_PROFILE_TEST_SPLIT_DEBUGINFO",
            Some("packed".to_owned()),
        ),
        ("CARGO_PROFILE_TEST_STRIP", Some("none".to_owned())),
        ("CARGO_ENCODED_RUSTFLAGS", Some(encoded)),
        ("CARGO_HOME", Some(cargo_home)),
        ("HOME", Some(home)),
        ("PATH", Some(path)),
        ("RUSTFLAGS", None),
        ("RUSTUP_HOME", Some(rustup_home)),
        ("RUSTUP_TOOLCHAIN", Some(channel.to_owned())),
    ]
    .into_iter()
    .map(|(name, value)| EffectiveValue {
        name: name.to_owned(),
        value,
    })
    .collect()
}

fn preflight_partition(
    mount: &Path,
    a: &Path,
    b: &Path,
    tools: &Toolchain,
) -> Result<(PartitionKey, source_map::ResolvedSourceMap), String> {
    let zero_tree = format!("sha256:{}", "0".repeat(64));
    let preview_environment = probe_environment(mount, &tools.channel, None);
    let preview = invocation(
        mount,
        a,
        &zero_tree,
        tools,
        &preview_environment,
        // This pass previews the eventual partition key before measured
        // controls are available. Placeholder controls never leave this
        // helper or count as evidence.
        Some(source_policy(
            a,
            b,
            identity_only_miss_controls_for_key_preview(),
        )),
        "preflight-map-preview",
    )?;
    let preview = match resolve::resolve(&preview) {
        Resolution::Ready(resolved) => resolved,
        _ => return Err("preflight input set did not resolve".to_owned()),
    };
    let preview_source_map = preview
        .source_map
        .ok_or_else(|| "preflight source map was not resolved".to_owned())?;
    let environment = probe_environment(mount, &tools.channel, Some(&preview_source_map));
    let final_invocation = invocation(
        mount,
        a,
        &zero_tree,
        tools,
        &environment,
        Some(source_policy(
            a,
            b,
            identity_only_miss_controls_for_key_preview(),
        )),
        "preflight-key",
    )?;
    let resolved = match resolve::resolve(&final_invocation) {
        Resolution::Ready(resolved) => resolved,
        _ => return Err("preflight input set with resolved flags did not resolve".to_owned()),
    };
    let source_map = resolved
        .source_map
        .ok_or_else(|| "preflight source map with resolved flags was not returned".to_owned())?;
    if source_map.effective_rustflags() != preview_source_map.effective_rustflags() {
        return Err(
            "preflight resolved source-map flags changed with Cargo environment".to_owned(),
        );
    }
    Ok((resolved.partition, source_map))
}

fn identity_only_miss_controls_for_key_preview() -> source_map::MissControlEvidence {
    let digest = |value: &str| source_map::digest_fields(&["probe-control", value]);
    source_map::MissControlEvidence {
        source: Some(source_map::SourceMissControl {
            before_tree_digest: digest("source-before"),
            after_tree_digest: digest("source-after"),
            partition_digest: digest("source-partition"),
            rebuilt_units: 1,
        }),
        configuration: Some(source_map::KeyMissControl {
            before_input_digest: digest("config-before"),
            after_input_digest: digest("config-after"),
            before_partition_digest: digest("config-partition-before"),
            after_partition_digest: digest("config-partition-after"),
        }),
        toolchain: Some(source_map::KeyMissControl {
            before_input_digest: digest("tool-before"),
            after_input_digest: digest("tool-after"),
            before_partition_digest: digest("tool-partition-before"),
            after_partition_digest: digest("tool-partition-after"),
        }),
    }
}

fn source_policy(
    a: &Path,
    b: &Path,
    miss_controls: source_map::MissControlEvidence,
) -> source_map::SourceMapPolicy {
    source_map::SourceMapPolicy {
        version: source_map::SOURCE_MAP_VERSION.to_owned(),
        mode: source_map::SourcePathMode::ExperimentalLogical {
            accepted_policy: source_map::ACCEPTED_LOGICAL_POLICY.to_owned(),
            prefix: PathBuf::from(LOGICAL_PREFIX),
        },
        roots: vec![
            source_map::AuthorizedSourceRoot {
                id: "a".to_owned(),
                path: a.to_path_buf(),
            },
            source_map::AuthorizedSourceRoot {
                id: "b".to_owned(),
                path: b.to_path_buf(),
            },
        ],
        relative_prefixes: vec![source_map::RelativeSourcePrefix {
            root_ids: vec!["a".to_owned(), "b".to_owned()],
            compiler_working_directory: PathBuf::from("."),
            package_directory: PathBuf::from("probe"),
            prefix: PathBuf::from("probe"),
        }],
        miss_controls: Some(miss_controls),
    }
}

fn invocation(
    mount: &Path,
    workspace: &Path,
    tree: &str,
    tools: &Toolchain,
    environment: &[EffectiveValue],
    source_map_policy: Option<source_map::SourceMapPolicy>,
    run: &str,
) -> Result<ProjectInvocation, String> {
    let run_root = mount.join("runs").join(run);
    fs::create_dir_all(run_root.join("target")).map_err(|error| error.to_string())?;
    fs::create_dir_all(run_root.join("capture")).map_err(|error| error.to_string())?;
    Ok(ProjectInvocation {
        consumer_id: "oxide-local-probe".to_owned(),
        descriptor: ProjectDescriptor {
            project_id: "oxide".to_owned(),
            component_id: "cargo-source-map-probe".to_owned(),
            workspace_root: workspace.to_path_buf(),
            workflows: vec![WorkflowOptIn {
                workflow: WORKFLOW.to_owned(),
                selector: SELECTOR.to_owned(),
                profile: "test".to_owned(),
                features: Vec::new(),
                required_slot_bytes: SLOT_RESERVATION,
                cargo_config_keys: Vec::new(),
                environment_keys: ENVIRONMENT_KEYS
                    .iter()
                    .map(|key| (*key).to_owned())
                    .collect(),
                path_identity: PathIdentity::DisposableRoot {
                    id: ROOT_ID.to_owned(),
                    root: mount.to_path_buf(),
                    source_map: source_map_policy,
                },
            }],
        },
        workflow: WORKFLOW.to_owned(),
        selector: SELECTOR.to_owned(),
        request_id: format!("request-{run}"),
        task_id: LOCAL_TASK_ID.to_owned(),
        workspace_id: LOCAL_WORKSPACE_ID.to_owned(),
        commit: git(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &["rev-parse", "HEAD"],
        )
        .trim()
        .to_owned(),
        source_tree_digest: tree.to_owned(),
        inputs: EffectiveBuildInputs {
            cargo: tools.cargo.clone(),
            rustc: tools.rustc.clone(),
            host: "aarch64-apple-darwin".to_owned(),
            target: "aarch64-apple-darwin".to_owned(),
            cargo_configuration: EffectiveInputSet::Complete(Vec::new()),
            environment: EffectiveInputSet::Complete(environment.to_vec()),
            effective_rustflags: Some(Vec::new()),
            full_debug: true,
        },
        target_dir: run_root.join("target"),
        capture_dir: run_root.join("capture"),
        private_fallback_allowed: false,
        wait_once: false,
    })
}

struct KeyControls {
    configuration: source_map::KeyMissControl,
    toolchain: source_map::KeyMissControl,
    log: String,
}

fn key_controls(
    mount: &Path,
    workspace: &Path,
    tree: &str,
    stable: &Toolchain,
    alternate: &Toolchain,
    environment: &[EffectiveValue],
    resolved_source_map: &source_map::ResolvedSourceMap,
) -> Result<KeyControls, String> {
    let mut config_after = environment.to_vec();
    if let Some(value) = config_after
        .iter_mut()
        .find(|value| value.name == "CARGO_BUILD_JOBS")
    {
        value.value = Some("1".to_owned());
    }
    let config_before_invocation = invocation(
        mount,
        workspace,
        tree,
        stable,
        environment,
        None,
        "config-before",
    )?;
    let config_after_invocation = invocation(
        mount,
        workspace,
        tree,
        stable,
        &config_after,
        None,
        "config-after",
    )?;
    let before_config_partition = resolved_partition(&config_before_invocation)?;
    let after_config_partition = resolved_partition(&config_after_invocation)?;
    if before_config_partition == after_config_partition {
        return Err("Cargo build-jobs input failed to separate its resolver key".to_owned());
    }
    let configuration = source_map::KeyMissControl {
        before_input_digest: source_map::digest_fields(&["config", "CARGO_BUILD_JOBS", "<unset>"]),
        after_input_digest: source_map::digest_fields(&["config", "CARGO_BUILD_JOBS", "1"]),
        before_partition_digest: partition_digest(&before_config_partition),
        after_partition_digest: partition_digest(&after_config_partition),
    };

    let stable_invocation = invocation(
        mount,
        workspace,
        tree,
        stable,
        environment,
        None,
        "toolchain-before",
    )?;
    let alternate_env = probe_environment(mount, "1.96.1", Some(resolved_source_map));
    let alternate_invocation = invocation(
        mount,
        workspace,
        tree,
        alternate,
        &alternate_env,
        None,
        "toolchain-after",
    )?;
    let before_toolchain_partition = resolved_partition(&stable_invocation)?;
    let after_toolchain_partition = resolved_partition(&alternate_invocation)?;
    if before_toolchain_partition == after_toolchain_partition {
        return Err("installed Cargo/rustc 1.96.1 failed to separate its resolver key".to_owned());
    }
    let before_toolchain = toolchain_input_digest(stable);
    let after_toolchain = toolchain_input_digest(alternate);
    let toolchain = source_map::KeyMissControl {
        before_input_digest: before_toolchain,
        after_input_digest: after_toolchain,
        before_partition_digest: partition_digest(&before_toolchain_partition),
        after_partition_digest: partition_digest(&after_toolchain_partition),
    };
    let log = format!(
        "key-control\tconfiguration\tpass\tbefore={}\tafter={}\tpartition_before={}\tpartition_after={}\nkey-control\ttoolchain\tpass\tbefore={}\tafter={}\tpartition_before={}\tpartition_after={}\n",
        configuration.before_input_digest,
        configuration.after_input_digest,
        configuration.before_partition_digest,
        configuration.after_partition_digest,
        toolchain.before_input_digest,
        toolchain.after_input_digest,
        toolchain.before_partition_digest,
        toolchain.after_partition_digest,
    );
    Ok(KeyControls {
        configuration,
        toolchain,
        log,
    })
}

fn resolved_partition(invocation: &ProjectInvocation) -> Result<PartitionKey, String> {
    match resolve::resolve(invocation) {
        Resolution::Ready(resolved) => Ok(resolved.partition),
        _ => Err("resolver refused a key-separation control".to_owned()),
    }
}

fn toolchain_input_digest(toolchain: &Toolchain) -> String {
    source_map::digest_fields(&[
        "toolchain-control-v1",
        &toolchain.cargo.version,
        &toolchain.cargo.executable_digest,
        &toolchain.rustc.version,
        &toolchain.rustc.executable_digest,
        toolchain.channel,
    ])
}

fn partition_digest(partition: &PartitionKey) -> String {
    let mut fields = vec![
        "oxide-probe-partition-v1".to_owned(),
        partition.project_component.clone(),
        partition.workflow.clone(),
        partition.cargo_provenance.clone(),
        partition.rustc_provenance.clone(),
        partition.host.clone(),
        partition.target.clone(),
        partition.profile.clone(),
        partition.config_digest.clone(),
        partition.environment_digest.clone(),
        partition.path_identity.clone(),
    ];
    fields.extend(partition.features.iter().cloned());
    let borrowed: Vec<_> = fields.iter().map(String::as_str).collect();
    source_map::digest_fields(&borrowed)
}

fn verify_probe_values(
    output: &FixtureCommandOutput,
    capture_dir: &Path,
    workspace: &Path,
    disposable_mount: &Path,
    expected_tree: &str,
) -> Result<(bool, String), String> {
    let stdout = fs::read(capture_dir.join("stdout.log")).map_err(|error| error.to_string())?;
    let summary = producer::inspect_cargo_output(&stdout);
    let values = summary.probe_values;
    let source_tree = producer::source_tree_digest(workspace)?;
    let marker_path = workspace.join(MARKER_PATH);
    let marker_bytes = fs::read(&marker_path).map_err(|error| error.to_string())?;
    let marker = std::str::from_utf8(&marker_bytes)
        .map_err(|_| "requested marker source is not UTF-8".to_owned())?
        .trim()
        .to_owned();
    let marker_digest = hex_digest(&marker_bytes);
    let test_source = workspace.join("probe/tests/probe.rs");
    let test_source_bytes = fs::read(&test_source).map_err(|error| error.to_string())?;
    let test_source_digest = hex_digest(&test_source_bytes);
    let logical_test_source = PathBuf::from(LOGICAL_PREFIX).join("probe/tests/probe.rs");
    let out_directory = values.get("out-directory").map(PathBuf::from);
    let generated_path = values.get("generated-output-path").map(PathBuf::from);
    let macro_input_path = values.get("proc-macro-input-path").map(PathBuf::from);
    let valid_image_path = |path: &Path, directory: bool| {
        path.starts_with(disposable_mount)
            && fs::canonicalize(path).is_ok_and(|canonical| {
                canonical == path
                    && canonical.starts_with(disposable_mount)
                    && if directory {
                        canonical.is_dir()
                    } else {
                        canonical.is_file()
                    }
            })
    };
    let out_directory_valid = out_directory
        .as_ref()
        .is_some_and(|path| valid_image_path(path, true));
    let generated_path_valid = generated_path
        .as_ref()
        .is_some_and(|path| valid_image_path(path, false));
    let macro_input_path_valid = macro_input_path
        .as_ref()
        .is_some_and(|path| valid_image_path(path, false));
    let generated_path_matches_out = match (&generated_path, &out_directory) {
        (Some(generated), Some(directory)) => generated == &directory.join("generated.rs"),
        _ => false,
    };
    let macro_input_path_matches_out = match (&macro_input_path, &out_directory) {
        (Some(input), Some(directory)) => input == &directory.join("proc-macro-input.txt"),
        _ => false,
    };
    let generated_bytes = generated_path
        .as_ref()
        .filter(|_| generated_path_valid)
        .and_then(|path| fs::read(path).ok());
    let macro_input_bytes = macro_input_path
        .as_ref()
        .filter(|_| macro_input_path_valid)
        .and_then(|path| fs::read(path).ok());
    let manifest = workspace.join("probe");
    let expected_source_path = marker_path.to_string_lossy().into_owned();
    let file_macro_matches =
        values.get("file-macro") == Some(&logical_test_source.to_string_lossy().into_owned());
    let panic_location_matches = values
        .get("panic-location")
        .and_then(|value| value.rsplit_once(':').map(|(file, _)| file))
        == Some(logical_test_source.to_string_lossy().as_ref());
    let generated_value_matches = generated_bytes.as_ref().is_some_and(|bytes| {
        String::from_utf8_lossy(bytes).contains(&format!("BUILD_VALUE: &str = {marker:?}"))
    });
    let macro_input_matches = macro_input_bytes.as_deref() == Some(marker.as_bytes());
    let values_match = source_tree == expected_tree
        && values.get("manifest-directory") == Some(&manifest.to_string_lossy().into_owned())
        && values.get("include-input-path") == Some(&expected_source_path)
        && values.get("build-input-path") == Some(&expected_source_path)
        && values.get("runtime-resource-path") == Some(&expected_source_path)
        && [
            "include-value",
            "build-value",
            "generated-value",
            "proc-macro-value",
            "runtime-value",
        ]
        .iter()
        .all(|key| values.get(*key).is_some_and(|value| value.trim() == marker))
        && out_directory_valid
        && generated_path_valid
        && macro_input_path_valid
        && generated_path_matches_out
        && macro_input_path_matches_out
        && generated_value_matches
        && macro_input_matches
        && file_macro_matches
        && panic_location_matches
        && output.status == FixtureCommandStatus::Exited(0);
    let report = format!(
        "tree={source_tree}\texpected_tree={expected_tree}\tmarker={marker_digest}\ttest_source={test_source_digest}\tgenerated={}\tproc_macro_input={}\tfile_macro={}\tpanic_location={}\tvalues_match={values_match}\n",
        generated_bytes
            .as_deref()
            .map(hex_digest)
            .unwrap_or_else(|| "missing".to_owned()),
        macro_input_bytes
            .as_deref()
            .map(hex_digest)
            .unwrap_or_else(|| "missing".to_owned()),
        values
            .get("file-macro")
            .map(String::as_str)
            .unwrap_or("missing"),
        values
            .get("panic-location")
            .map(String::as_str)
            .unwrap_or("missing"),
    );
    Ok((values_match, report))
}

fn append_qualification(mount: &Path, run: &OwnedRun, label: &str) -> Result<(), String> {
    let report_path = run.capture_dir.join("source-map-evidence.txt");
    let report = fs::read_to_string(&report_path).unwrap_or_else(|_| "missing\n".to_owned());
    append_evidence(
        mount,
        &format!("qualification\t{label}\t{:?}\n{report}", run.qualification),
    )
}

fn stop_after_settled_runs(
    mount: &Path,
    client: &Client,
    daemon: ProbeDaemon,
    attach: &str,
    budget: &ProbeBudget,
    started: Instant,
    reason: &str,
) -> Result<(), String> {
    append_evidence(
        mount,
        &format!("probe-stop\t{reason}\t{}\n", started.elapsed().as_millis()),
    )?;
    finish_if_idle(mount, client, daemon, attach, budget, reason)
}

fn finish_if_idle(
    mount: &Path,
    client: &Client,
    daemon: ProbeDaemon,
    attach: &str,
    budget: &ProbeBudget,
    reason: &str,
) -> Result<(), String> {
    let inventory = client
        .inventory("oxide-probe-stopped-inventory")
        .map_err(|error| error.to_string())?;
    if inventory
        .slots
        .iter()
        .any(|slot| slot.state != SlotState::Idle)
    {
        append_evidence(
            mount,
            &format!("probe-stop\t{reason}\tunsettled lease; image stays mounted\n"),
        )?;
        return Err(format!("{reason}; unsettled lease; image stays mounted"));
    }
    budget.sample()?;
    append_evidence(
        mount,
        &format!("probe-stop\t{reason}\tall leases settled; preserving image\n"),
    )?;
    drop(daemon);
    let _ = attach;
    Err(format!(
        "{reason}; all leases settled; image and mount preserved"
    ))
}

fn freeze_fixture(root: &Path) -> Result<FrozenFixture, String> {
    let mut files = BTreeMap::new();
    collect_fixture(root, root, &mut files)?;
    let mut digest = Sha256::new();
    for (path, bytes) in &files {
        let name = path.to_string_lossy();
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    Ok(FrozenFixture {
        files,
        digest: format!("sha256:{:x}", digest.finalize()),
    })
}

fn verify_committed_fixture(root: &Path, frozen: &FrozenFixture) -> Result<(), String> {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let relative_root = Path::new("fixtures/cargo-source-map-probe");
    let tracked = git(
        &repository,
        &["ls-files", "--", "fixtures/cargo-source-map-probe"],
    );
    let mut tracked_paths = tracked
        .lines()
        .filter_map(|path| Path::new(path).strip_prefix(relative_root).ok())
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    tracked_paths.sort();
    let frozen_paths = frozen.files.keys().cloned().collect::<Vec<_>>();
    if tracked_paths != frozen_paths {
        return Err("the frozen probe fixture does not match the committed file set".to_owned());
    }
    let diff = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(&repository)
        .args([
            "diff",
            "--quiet",
            "HEAD",
            "--",
            "fixtures/cargo-source-map-probe",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .status()
        .map_err(|error| error.to_string())?;
    if !diff.success() {
        return Err("the probe fixture must be committed before the evidence session".to_owned());
    }
    let current_digest = producer::source_tree_digest(root)?;
    if current_digest != frozen.digest {
        return Err("the committed fixture changed after it was frozen".to_owned());
    }
    Ok(())
}

fn collect_fixture(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!("fixture symlink refused: {}", path.display()));
        }
        if metadata.is_dir() {
            collect_fixture(root, &path, files)?;
        } else if metadata.is_file() {
            files.insert(
                path.strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .to_path_buf(),
                fs::read(path).map_err(|error| error.to_string())?,
            );
        }
    }
    Ok(())
}

fn ensure_probe_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(format!("probe path is not a directory: {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }
    let canonical = fs::canonicalize(path).map_err(|error| error.to_string())?;
    if canonical != path {
        return Err(format!(
            "probe directory is not canonical: {}",
            path.display()
        ));
    }
    Ok(())
}

fn prepare_fixture_root(frozen: &FrozenFixture, destination: &Path) -> Result<(), String> {
    match fs::symlink_metadata(destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return write_frozen_fixture(frozen, destination)
        }
        Err(error) => return Err(error.to_string()),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            if producer::source_tree_digest(destination)? != frozen.digest {
                return Err(format!(
                    "prepared fixture tree changed at {}",
                    destination.display()
                ));
            }
        }
        Ok(_) => {
            return Err(format!(
                "prepared fixture root is not a directory: {}",
                destination.display()
            ))
        }
    }
    Ok(())
}

fn write_frozen_fixture(frozen: &FrozenFixture, destination: &Path) -> Result<(), String> {
    fs::create_dir(destination).map_err(|error| error.to_string())?;
    for (relative, bytes) in &frozen.files {
        let path = destination.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn refuse_session_timeout(started: &Instant) -> Result<(), String> {
    session_remaining(started.elapsed()).map(|_| ())
}

fn session_remaining(elapsed: Duration) -> Result<Duration, String> {
    MAX_SESSION
        .checked_sub(elapsed)
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| "the 20 minute session limit was reached".to_owned())
}

fn record_session_clock(
    setup_temp: &Path,
    start_utc: &str,
    deadline_utc: &str,
) -> Result<(), String> {
    let body = format!(
        "oxide-probe-session-clock-v2\nsession_id={SESSION_ID}\nclock=std::time::Instant\norigin=driver-start-before-setup\nmonotonic_unit=milliseconds\nstart_elapsed_ms=0\ndeadline_elapsed_ms=1200000\nstart_utc={start_utc}\ndeadline_utc={deadline_utc}\nmax_session_secs={}\ninvocation_count=0\nmax_invocations={MAX_INVOCATIONS}\n",
        MAX_SESSION.as_secs(),
    );
    let path = setup_temp.join("session-clock.txt");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| error.to_string())?;
    file.write_all(body.as_bytes())
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

fn machine_epoch_utc(stamp: &str) -> Result<u64, String> {
    machine_date(&["-j", "-u", "-f", "%Y-%m-%dT%H:%M:%SZ", stamp, "+%s"])?
        .parse()
        .map_err(|_| "machine UTC timestamp could not be parsed".to_owned())
}

fn machine_utc() -> Result<String, String> {
    machine_date(&["-u", "+%Y-%m-%dT%H:%M:%SZ"])
}

fn machine_date(args: &[&str]) -> Result<String, String> {
    let output = Command::new("/bin/date")
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "machine clock unavailable: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stamp = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if stamp.is_empty() {
        return Err("machine clock returned an empty timestamp".to_owned());
    }
    Ok(stamp)
}

fn image_info(image: &Path) -> Result<Vec<u8>, String> {
    let result = Command::new("/usr/bin/hdiutil")
        .args(["imageinfo", "-plist"])
        .arg(image)
        .output()
        .map_err(|error| error.to_string())?;
    if result.status.success() {
        Ok(result.stdout)
    } else {
        Err(format!(
            "hdiutil imageinfo failed: {}",
            String::from_utf8_lossy(&result.stderr)
        ))
    }
}

fn image_capacity_from_bytes(plist: &[u8]) -> Result<u64, String> {
    let mut child = Command::new("/usr/bin/plutil")
        .args(["-extract", "Size Information.Total Bytes", "raw", "-o", "-"])
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    child
        .stdin
        .take()
        .ok_or_else(|| "plutil stdin is unavailable".to_owned())?
        .write_all(plist)
        .map_err(|error| error.to_string())?;
    let result = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !result.status.success() {
        return Err(format!(
            "plutil could not read hdiutil virtual capacity: {}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    String::from_utf8_lossy(&result.stdout)
        .trim()
        .parse::<u64>()
        .map_err(|error| error.to_string())
}

fn is_sparse_apfs_info(info: &[u8]) -> bool {
    let info = String::from_utf8_lossy(info);
    info.contains("<string>sparse</string>") && info.contains("<key>APFS</key>")
}

fn image_info_block<'a>(info: &'a str, image: &Path) -> Option<&'a str> {
    let expected = image.to_str()?;
    info.split("================================================")
        .find(|block| {
            block.lines().any(|line| {
                line.split_once(':').is_some_and(|(key, value)| {
                    key.trim() == "image-path" && value.trim() == expected
                })
            })
        })
}

fn image_block_base_device(block: &str) -> Option<String> {
    block.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let device = fields.next()?;
        (device.starts_with("/dev/disk") && fields.next() == Some("GUID_partition_scheme"))
            .then(|| device.to_owned())
    })
}

fn image_block_has_mount(block: &str, mount: &Path, device: &str) -> bool {
    let Some(mount) = mount.to_str() else {
        return false;
    };
    image_block_base_device(block).as_deref() == Some(device)
        && block.lines().any(|line| {
            let mut fields = line.split_whitespace();
            let observed_device = fields.next();
            let _filesystem = fields.next();
            let mounted_path = fields.next_back();
            observed_device.is_some_and(|observed| observed.starts_with("/dev/disk"))
                && mounted_path == Some(mount)
        })
}

fn setup_image(image: &Path, mount: &Path, setup_temp: &Path) -> Result<(String, u64), String> {
    fs::create_dir(mount).map_err(|error| error.to_string())?;
    let created = Command::new("/usr/bin/hdiutil")
        .args([
            "create",
            "-size",
            "8g",
            "-type",
            "SPARSE",
            "-fs",
            "APFS",
            "-volname",
            "oxide-rust-probe-proof-v3",
            "-nospotlight",
            "-quiet",
        ])
        .arg(image)
        .env("TMPDIR", setup_temp)
        .env("TMP", setup_temp)
        .env("TEMP", setup_temp)
        .output()
        .map_err(|error| error.to_string())?;
    if !created.status.success() {
        return Err(format!(
            "hdiutil create failed: {}",
            String::from_utf8_lossy(&created.stderr)
        ));
    }
    let info = image_info(image)?;
    let image_info_capture = setup_temp.join("imageinfo-before-attach.plist");
    let mut info_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&image_info_capture)
        .map_err(|error| error.to_string())?;
    info_file
        .write_all(&info)
        .map_err(|error| error.to_string())?;
    info_file.sync_all().map_err(|error| error.to_string())?;
    if image_capacity_from_bytes(&info)? != IMAGE_CAP || !is_sparse_apfs_info(&info) {
        return Err("hdiutil did not confirm the approved 8 GiB sparse APFS image".to_owned());
    }
    let outside_directories =
        allocated_blocks(Path::new(ROOT))?.saturating_add(allocated_blocks(mount)?);
    let attached = Command::new("/usr/bin/hdiutil")
        .args(["attach", "-nobrowse", "-noverify", "-mountpoint"])
        .arg(mount)
        .arg(image)
        .env("TMPDIR", setup_temp)
        .env("TMP", setup_temp)
        .env("TEMP", setup_temp)
        .output()
        .map_err(|error| error.to_string())?;
    if !attached.status.success() {
        return Err(format!(
            "hdiutil attach failed: {}",
            String::from_utf8_lossy(&attached.stderr)
        ));
    }
    let device = String::from_utf8_lossy(&attached.stdout)
        .split_whitespace()
        .find(|value| value.starts_with("/dev/disk"))
        .ok_or_else(|| {
            format!(
                "hdiutil attach did not identify its device: {}",
                String::from_utf8_lossy(&attached.stdout)
            )
        })?
        .to_owned();
    let canonical_mount = fs::canonicalize(mount).map_err(|error| error.to_string())?;
    if canonical_mount.as_path() != mount {
        return Err("mounted disposable volume is not at its canonical approved path".to_owned());
    }
    let (mounted_device, capacity, used, free) = df_space(mount)?;
    let attached = Command::new("/usr/bin/hdiutil")
        .arg("info")
        .output()
        .map_err(|error| error.to_string())?;
    let attached_text = String::from_utf8_lossy(&attached.stdout);
    let image_block = if attached.status.success() {
        image_info_block(&attached_text, image)
    } else {
        None
    };
    if !image_block.is_some_and(|block| image_block_has_mount(block, mount, &device))
        || !mounted_device.starts_with("/dev/disk")
        || capacity == 0
        || capacity > IMAGE_CAP
        || used > IMAGE_CAP
        || free > capacity
    {
        return Err(format!(
            "mounted image failed device/capacity preflight: attached={device} mounted={mounted_device} capacity={capacity} used={used} free={free}"
        ));
    }
    Ok((device.to_owned(), outside_directories))
}

fn allocated_blocks(path: &Path) -> Result<u64, String> {
    fs::symlink_metadata(path)
        .map_err(|error| error.to_string())?
        .blocks()
        .checked_mul(512)
        .ok_or_else(|| "allocated directory byte count overflow".to_owned())
}

fn detach_image(device: &str) -> Result<(), String> {
    let result = Command::new("/usr/bin/hdiutil")
        .arg("detach")
        .arg(device)
        .output()
        .map_err(|error| error.to_string())?;
    if result.status.success() {
        Ok(())
    } else {
        Err(format!(
            "normal hdiutil detach failed: {}",
            String::from_utf8_lossy(&result.stderr)
        ))
    }
}

fn df_space(path: &Path) -> Result<(String, u64, u64, u64), String> {
    let output = producer::run_bounded(
        "/bin/df",
        &["-k", path.to_str().ok_or("df path is not UTF-8")?],
    )?;
    let output_text = String::from_utf8_lossy(&output);
    let line = output_text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .last()
        .ok_or_else(|| "df returned no volume row".to_owned())?;
    let fields: Vec<_> = line.split_whitespace().collect();
    if fields.len() < 4 {
        return Err(format!("df volume row is incomplete: {line}"));
    }
    let to_bytes = |value: &str| {
        value
            .parse::<u64>()
            .ok()
            .and_then(|blocks| blocks.checked_mul(1024))
    };
    let capacity = to_bytes(fields[1]).ok_or_else(|| "df capacity is invalid".to_owned())?;
    let used = to_bytes(fields[2]).ok_or_else(|| "df used count is invalid".to_owned())?;
    let free = to_bytes(fields[3]).ok_or_else(|| "df free count is invalid".to_owned())?;
    Ok((fields[0].to_owned(), capacity, used, free))
}

fn du_bytes(path: &Path) -> Result<u64, String> {
    let output = producer::run_bounded(
        "/usr/bin/du",
        &["-sk", path.to_str().ok_or("du path is not UTF-8")?],
    )?;
    let blocks = String::from_utf8_lossy(&output)
        .split_whitespace()
        .next()
        .ok_or_else(|| "du returned no allocated-size value".to_owned())?
        .parse::<u64>()
        .map_err(|error| error.to_string())?;
    blocks
        .checked_mul(1024)
        .ok_or_else(|| "du allocated-size overflow".to_owned())
}

fn append_evidence(mount: &Path, line: &str) -> Result<(), String> {
    let path = mount.join("evidence/events.tsv");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(line.as_bytes())
        .map_err(|error| error.to_string())?;
    file.sync_data().map_err(|error| error.to_string())
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
