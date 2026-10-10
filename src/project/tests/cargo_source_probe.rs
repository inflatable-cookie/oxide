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
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const ROOT: &str = "/private/var/tmp/oxide-rust-probe-20261010";
const WORKFLOW: &str = "oxide-cargo-source-map-probe";
const SELECTOR: &str =
    "test --offline --locked --message-format=json-render-diagnostics -- --nocapture";
const ROOT_ID: &str = "oxide-rust-probe-20261010";
const LOGICAL_PREFIX: &str = "/oxide-source-map-probe";
const MARKER_PATH: &str = "probe/marker.txt";
const BASELINE_MARKER: &str = "baseline-marker\n";
const CHANGED_MARKER: &str = "changed-marker\n";
const GIB: u64 = 1024 * 1024 * 1024;
const MAX_SESSION: Duration = Duration::from_secs(60 * 60);
const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);
const HOST_RESERVE: u64 = 20 * GIB;
const IMAGE_CAP: u64 = 8 * GIB;
const DISPOSABLE_CAP: u64 = 10 * GIB;
const SLOT_RESERVATION: u64 = GIB;

const ENVIRONMENT_KEYS: [&str; 9] = [
    "CARGO_BUILD_JOBS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_HOME",
    "HOME",
    "PATH",
    "RUSTFLAGS",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "TMPDIR",
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
    outside_directories: u64,
    started: Instant,
    last_sample: Mutex<Instant>,
    samples: Mutex<Vec<SpaceSample>>,
    failure: Mutex<Option<String>>,
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
            let reason = "the 60 minute session limit was reached".to_owned();
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
    fn new(
        mount: PathBuf,
        image: PathBuf,
        setup_temp: PathBuf,
        outside_directories: u64,
        started: Instant,
    ) -> Arc<Self> {
        Arc::new(Self {
            mount,
            image,
            setup_temp,
            outside_directories,
            started,
            last_sample: Mutex::new(Instant::now() - SAMPLE_INTERVAL),
            samples: Mutex::new(Vec::new()),
            failure: Mutex::new(None),
        })
    }

    fn sample(&self) -> Result<SpaceSample, String> {
        let result = self.read_sample();
        let mut last = self
            .last_sample
            .lock()
            .map_err(|_| "budget sample lock poisoned")?;
        *last = Instant::now();
        match result {
            Ok(sample) => {
                self.write_sample(&sample)?;
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
                Err(error)
            }
        }
    }

    fn read_sample(&self) -> Result<SpaceSample, String> {
        if self.started.elapsed() >= MAX_SESSION {
            return Err("the 60 minute session limit was reached".to_owned());
        }
        let host_free = native::host_free_bytes(Path::new("/private/var/tmp"))
            .map_err(|_| "host free-space observation unavailable".to_owned())?;
        if host_free < HOST_RESERVE {
            return Err(format!("host free space fell below 20 GiB: {host_free}"));
        }
        let (_, image_capacity, image_used, image_free) = df_space(&self.mount)?;
        if image_capacity > IMAGE_CAP || image_used > IMAGE_CAP || image_free > image_capacity {
            return Err(format!(
                "APFS volume exceeded or obscured its 8 GiB bound: capacity={image_capacity} used={image_used} free={image_free}"
            ));
        }
        let image_allocated = du_bytes(&self.image)?;
        let outside_allocated = self
            .outside_directories
            .saturating_add(du_bytes(&self.setup_temp)?);
        let disposable = image_allocated.saturating_add(outside_allocated);
        if disposable > DISPOSABLE_CAP {
            return Err(format!("10 GiB disposable ceiling exceeded: {disposable}"));
        }
        let expected = ["m", "oxide-rust-probe.sparseimage", "setup-tmp"];
        let mut names = fs::read_dir(ROOT)
            .map_err(|error| error.to_string())?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        names.sort();
        if names
            .iter()
            .map(String::as_str)
            .ne(expected.iter().copied())
        {
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
            "space\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            sample.elapsed_ms,
            sample.host_free,
            sample.image_capacity,
            sample.image_used,
            sample.image_free,
            sample.image_allocated,
            sample.outside_allocated,
        );
        append_evidence(&self.mount, &line)
    }
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
    let session_clock = Instant::now();
    let prior_elapsed = known_resume_elapsed()?;
    let started = session_clock
        .checked_sub(prior_elapsed)
        .ok_or_else(|| "could not preserve the original session start".to_owned())?;
    if started.elapsed() >= MAX_SESSION {
        return Err("the original 60 minute session limit was reached".to_owned());
    }
    let host_free = native::host_free_bytes(Path::new("/private/var/tmp"))
        .map_err(|_| "host free-space preflight unavailable".to_owned())?;
    if host_free < HOST_RESERVE {
        return Err(format!(
            "host free space below the 20 GiB start threshold: {host_free}"
        ));
    }
    let fixture_source =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/cargo-source-map-probe");
    let frozen = freeze_fixture(&fixture_source)?;
    verify_committed_fixture(&fixture_source, &frozen)?;
    let scanner_identity = producer::scanner_identity()?;
    let image = PathBuf::from(ROOT).join("oxide-rust-probe.sparseimage");
    let mount = PathBuf::from(ROOT).join("m");
    let setup_temp = PathBuf::from(ROOT).join("setup-tmp");
    let (attach, outside_directories) =
        setup_image(&image, &mount, &setup_temp, prior_elapsed > Duration::ZERO)?;
    let evidence = mount.join("evidence");
    fs::create_dir(&evidence).map_err(|error| error.to_string())?;
    fs::create_dir(mount.join("tmp")).map_err(|error| error.to_string())?;
    fs::create_dir(mount.join("home")).map_err(|error| error.to_string())?;
    fs::create_dir(mount.join("cargo-home")).map_err(|error| error.to_string())?;
    let budget = ProbeBudget::new(
        mount.clone(),
        image.clone(),
        setup_temp,
        outside_directories,
        started,
    );
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
            "session\tstart\t{}	fixture_tree={}\nscanner-provenance\n{}",
            local_timestamp(),
            frozen.digest,
            scanner_identity
        ),
    )?;

    let a = mount.join("a");
    let b = mount.join("b");
    write_frozen_fixture(&frozen, &a)?;
    write_frozen_fixture(&frozen, &b)?;
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
    let daemon = ProbeDaemon::start(&mount.join("storage"), Arc::clone(&budget));
    let client = daemon.client();
    let flags = remap_flags(&a, &b);
    let env = probe_environment(&mount, "1.97.1", &flags);
    let preflight_partition = preflight_partition(&mount, &a, &b, &stable, &env, &flags)?;

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
        &flags,
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
    detach_image(&attach)?;
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
        task_id: "6ca14e7a-2cf9-42d3-8f94-f45daad4b2dc".to_owned(),
        workspace_id: "probe-image-20261010".to_owned(),
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
    let temp_dir = mount.join("tmp");
    let claim = native::NativeExecutionClaim {
        lease_id: acquire.lease_id.clone(),
        slot_id: acquire.slot_id.clone(),
        slot_path: slot_path.clone(),
        temp_dir: temp_dir.clone(),
        capture_dir: capture_dir.clone(),
        binding: binding.clone(),
        host_run_id: host.id.clone(),
        workflow: WORKFLOW.to_owned(),
        selector: SELECTOR.to_owned(),
    };
    let plan = FixtureExecutionPlan {
        host_run: host.clone(),
        command: HostRunRequest {
            selector: SELECTOR.to_owned(),
            workflow: WORKFLOW.to_owned(),
            profile: "dev".to_owned(),
            features: Vec::new(),
            workspace_root: workspace.to_path_buf(),
            cargo_build_dir: slot_path.clone(),
            cargo_target_dir: target_dir.clone(),
            capture_dir: capture_dir.clone(),
            source_map: map.clone(),
            environment: environment.to_vec(),
        },
        claim,
    };
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

fn probe_environment(mount: &Path, channel: &str, flags: &[String]) -> Vec<EffectiveValue> {
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
    let encoded = flags.join("\u{1f}");
    [
        ("CARGO_BUILD_JOBS", None),
        ("CARGO_ENCODED_RUSTFLAGS", Some(encoded)),
        ("CARGO_HOME", Some(cargo_home)),
        ("HOME", Some(home)),
        ("PATH", Some(path)),
        ("RUSTFLAGS", None),
        ("RUSTUP_HOME", Some(rustup_home)),
        ("RUSTUP_TOOLCHAIN", Some(channel.to_owned())),
        (
            "TMPDIR",
            Some(mount.join("tmp").to_string_lossy().into_owned()),
        ),
    ]
    .into_iter()
    .map(|(name, value)| EffectiveValue {
        name: name.to_owned(),
        value,
    })
    .collect()
}

fn remap_flags(a: &Path, b: &Path) -> Vec<String> {
    [a, b]
        .into_iter()
        .map(|root| format!("--remap-path-prefix={}={LOGICAL_PREFIX}", root.display()))
        .collect()
}

fn preflight_partition(
    mount: &Path,
    a: &Path,
    b: &Path,
    tools: &Toolchain,
    environment: &[EffectiveValue],
    flags: &[String],
) -> Result<PartitionKey, String> {
    let zero_tree = format!("sha256:{}", "0".repeat(64));
    let base = invocation(
        mount,
        a,
        &zero_tree,
        tools,
        environment,
        None,
        "preflight-key",
    )?;
    let mut partition = match resolve::resolve(&base) {
        Resolution::Ready(resolved) => resolved.partition,
        _ => return Err("preflight input set did not resolve".to_owned()),
    };
    let policy_digest = source_map::digest_fields(&[
        "oxide-source-map-policy-v1",
        source_map::SOURCE_MAP_VERSION,
        "experimental-logical",
        source_map::ACCEPTED_LOGICAL_POLICY,
        LOGICAL_PREFIX,
    ]);
    let mut map_fields = vec![policy_digest];
    map_fields.push("a".to_owned());
    map_fields.push(a.to_string_lossy().into_owned());
    map_fields.push("b".to_owned());
    map_fields.push(b.to_string_lossy().into_owned());
    map_fields.extend(flags.iter().cloned());
    let borrowed: Vec<_> = map_fields.iter().map(String::as_str).collect();
    let map_digest = source_map::digest_fields(&borrowed);
    let disposable_digest = source_map::disposable_root_identity(ROOT_ID, mount)
        .map_err(|error| format!("probe disposable root identity failed: {error:?}"))?;
    partition.path_identity = format!("disposable-root:{ROOT_ID}:{disposable_digest}:{map_digest}");
    Ok(partition)
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
                profile: "dev".to_owned(),
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
        task_id: "6ca14e7a-2cf9-42d3-8f94-f45daad4b2dc".to_owned(),
        workspace_id: "probe-image-20261010".to_owned(),
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
    flags: &[String],
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
    let alternate_env = probe_environment(mount, "1.96.1", flags);
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
        generated_bytes.as_deref().map(hex_digest).unwrap_or_else(|| "missing".to_owned()),
        macro_input_bytes.as_deref().map(hex_digest).unwrap_or_else(|| "missing".to_owned()),
        values.get("file-macro").map(String::as_str).unwrap_or("missing"),
        values.get("panic-location").map(String::as_str).unwrap_or("missing"),
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
        return Ok(());
    }
    budget.sample()?;
    append_evidence(
        mount,
        &format!("probe-stop\t{reason}\tall leases settled; preserving image\n"),
    )?;
    drop(daemon);
    detach_image(attach)?;
    Ok(())
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

fn known_resume_elapsed() -> Result<Duration, String> {
    let root = Path::new(ROOT);
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Duration::ZERO),
        Err(error) => return Err(format!("could not inspect {ROOT}: {error}")),
        Ok(metadata)
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && fs::canonicalize(root).is_ok_and(|path| path == root) => {}
        Ok(_) => return Err(format!("refusing unknown pre-existing state at {ROOT}")),
    }

    let expected_root_entries = BTreeSet::from([
        "m".to_owned(),
        "oxide-rust-probe.sparseimage".to_owned(),
        "setup-tmp".to_owned(),
    ]);
    if directory_names(root)? != expected_root_entries {
        return Err(format!("refusing unknown pre-existing state at {ROOT}"));
    }
    let mount = root.join("m");
    let setup_temp = root.join("setup-tmp");
    for directory in [&mount, &setup_temp] {
        let metadata = fs::symlink_metadata(directory).map_err(|error| error.to_string())?;
        let canonical = fs::canonicalize(directory).map_err(|error| error.to_string())?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || canonical.as_path() != directory
        {
            return Err(
                "pre-existing probe directories are not owned canonical directories".to_owned(),
            );
        }
    }
    let expected_setup_entries = BTreeSet::from([
        "attempt-1.txt".to_owned(),
        "attempt-2.txt".to_owned(),
        "attempt-3.txt".to_owned(),
        "diskutil-attempt-3.plist".to_owned(),
        "hdiutil-image-map-attempt-3.txt".to_owned(),
        "imageinfo-after-attach-attempt-3.plist".to_owned(),
        "imageinfo-attempt-1.plist".to_owned(),
        "imageinfo-before-attach.plist".to_owned(),
        "resume-state.txt".to_owned(),
    ]);
    if directory_names(&setup_temp)? != expected_setup_entries {
        return Err("pre-existing probe setup contents are not the recorded task state".to_owned());
    }

    let state_path = setup_temp.join("resume-state.txt");
    let state = fs::read_to_string(&state_path).map_err(|error| error.to_string())?;
    let mut fields = BTreeMap::new();
    let mut lines = state.lines();
    if lines.next() != Some("oxide-rust-probe-resume-v1") {
        return Err("probe resume marker has an unknown version".to_owned());
    }
    for line in lines {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| "probe resume marker is malformed".to_owned())?;
        if fields.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err("probe resume marker repeats a field".to_owned());
        }
    }
    let required = [
        ("root", ROOT),
        ("image", "oxide-rust-probe.sparseimage"),
        ("image_virtual_capacity", "8589934592"),
        ("attempt_count", "3"),
        ("imageinfo_file", "imageinfo-after-attach-attempt-3.plist"),
    ];
    if fields.len() != 13
        || required
            .iter()
            .any(|(key, expected)| fields.get(*key).map(String::as_str) != Some(expected))
    {
        return Err("probe resume marker does not describe the approved image state".to_owned());
    }

    let image = root.join("oxide-rust-probe.sparseimage");
    let image_metadata = fs::symlink_metadata(&image).map_err(|error| error.to_string())?;
    if !image_metadata.is_file() || image_metadata.file_type().is_symlink() {
        return Err("recorded sparse image is not a regular file".to_owned());
    }
    let image_digest = digest_file_path(&image)?;
    let image_allocated_length = image_metadata.len().to_string();
    if fields.get("image_allocated_length").map(String::as_str)
        != Some(image_allocated_length.as_str())
        || fields.get("image_digest") != Some(&image_digest)
    {
        return Err("recorded sparse image changed since setup attempt 1".to_owned());
    }

    let info_path = setup_temp.join(
        fields
            .get("imageinfo_file")
            .ok_or_else(|| "probe resume marker lacks its hdiutil capture name".to_owned())?,
    );
    let info_metadata = fs::symlink_metadata(&info_path).map_err(|error| error.to_string())?;
    if !info_metadata.is_file() || info_metadata.file_type().is_symlink() {
        return Err("recorded hdiutil image information is not a regular file".to_owned());
    }
    let info_bytes = fs::read(&info_path).map_err(|error| error.to_string())?;
    let recorded_info_digest = hex_digest(&info_bytes);
    if fields.get("imageinfo_digest") != Some(&recorded_info_digest)
        || image_info(&image)? != info_bytes
        || image_capacity(&info_path)? != IMAGE_CAP
        || !String::from_utf8_lossy(&info_bytes).contains("<string>sparse</string>")
        || !String::from_utf8_lossy(&info_bytes).contains("<key>APFS</key>")
    {
        return Err(
            "recorded hdiutil evidence does not prove the exact sparse APFS image".to_owned(),
        );
    }
    let attempt =
        fs::read_to_string(setup_temp.join("attempt-1.txt")).map_err(|error| error.to_string())?;
    if !attempt.contains("Result: exit 101 before any fixture Cargo invocation")
        || !attempt.contains("15732736")
    {
        return Err("recorded first attempt lacks its setup failure evidence".to_owned());
    }
    let second_attempt =
        fs::read_to_string(setup_temp.join("attempt-2.txt")).map_err(|error| error.to_string())?;
    if !second_attempt
        .contains("stopped before image attach and before any fixture Cargo invocation")
        || !second_attempt.contains("does not prove the exact sparse APFS image")
    {
        return Err("recorded second attempt lacks its setup failure evidence".to_owned());
    }
    let third_attempt =
        fs::read_to_string(setup_temp.join("attempt-3.txt")).map_err(|error| error.to_string())?;
    if !third_attempt.contains("before any fixture Cargo invocation or storage lease")
        || !third_attempt.contains("mounted=/dev/disk7s1")
    {
        return Err("recorded third attempt lacks its setup failure evidence".to_owned());
    }

    let attached = Command::new("/usr/bin/hdiutil")
        .arg("info")
        .output()
        .map_err(|error| error.to_string())?;
    if !attached.status.success() {
        return Err("hdiutil attachment state is unknown".to_owned());
    }
    let attached_text = String::from_utf8_lossy(&attached.stdout);
    let attached_block = image_info_block(&attached_text, &image)
        .ok_or_else(|| "the recorded task image is not attached in hdiutil state".to_owned())?;
    let recorded_block = fs::read_to_string(setup_temp.join("hdiutil-image-map-attempt-3.txt"))
        .map_err(|error| error.to_string())?;
    let recorded_map_digest = hex_digest(recorded_block.as_bytes());
    let block_base_device = image_block_base_device(attached_block)
        .ok_or_else(|| "recorded image has no base device mapping".to_owned())?;
    let state_base_device = fields.get("attached_device").map(String::as_str);
    if attached_block != recorded_block
        || fields.get("attachment_map_digest") != Some(&recorded_map_digest)
        || !image_block_has_mount(attached_block, &mount, &block_base_device)
        || state_base_device != Some(block_base_device.as_str())
    {
        return Err(
            "recorded hdiutil mapping does not bind this image to the approved mount".to_owned(),
        );
    }
    let (mounted_device, capacity, used, free) = df_space(&mount)?;
    if fields.get("mounted_device").map(String::as_str) != Some(mounted_device.as_str())
        || capacity == 0
        || capacity > IMAGE_CAP
        || used > IMAGE_CAP
        || free > capacity
    {
        return Err("recorded APFS mount no longer has the expected bounded capacity".to_owned());
    }
    let diskutil_bytes =
        fs::read(setup_temp.join("diskutil-attempt-3.plist")).map_err(|error| error.to_string())?;
    let recorded_diskutil_digest = hex_digest(&diskutil_bytes);
    if fields.get("diskutil_digest") != Some(&recorded_diskutil_digest) {
        return Err("recorded diskutil APFS identity evidence changed".to_owned());
    }

    let elapsed_ms = fields
        .get("prior_elapsed_ms")
        .ok_or_else(|| "probe resume marker lacks prior elapsed time".to_owned())?
        .parse::<u64>()
        .map_err(|error| error.to_string())?;
    Ok(Duration::from_millis(elapsed_ms))
}

fn directory_names(directory: &Path) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "symlink refused in bounded probe setup: {}",
                entry.path().display()
            ));
        }
        names.insert(
            entry
                .file_name()
                .into_string()
                .map_err(|_| "non-UTF-8 bounded probe path refused".to_owned())?,
        );
    }
    Ok(names)
}

fn digest_file_path(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count =
            std::io::Read::read(&mut file, &mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
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

fn image_capacity(plist: &Path) -> Result<u64, String> {
    let result = Command::new("/usr/bin/plutil")
        .args(["-extract", "Size Information.Total Bytes", "raw", "-o", "-"])
        .arg(plist)
        .output()
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

fn setup_image(
    image: &Path,
    mount: &Path,
    setup_temp: &Path,
    resume: bool,
) -> Result<(String, u64), String> {
    if !resume {
        fs::create_dir(ROOT).map_err(|error| error.to_string())?;
        fs::create_dir(mount).map_err(|error| error.to_string())?;
        fs::create_dir(setup_temp).map_err(|error| error.to_string())?;
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
                "oxide-rust-probe",
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
    }
    let info = image_info(image)?;
    let image_info_capture = setup_temp.join("imageinfo-before-attach.plist");
    match fs::read(&image_info_capture) {
        Ok(previous) if resume && previous == info => {}
        Ok(_) => return Err("pre-attach image information changed since setup".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut info_file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&image_info_capture)
                .map_err(|error| error.to_string())?;
            info_file
                .write_all(&info)
                .map_err(|error| error.to_string())?;
            info_file.sync_all().map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }
    if image_capacity(&image_info_capture)? != IMAGE_CAP
        || !String::from_utf8_lossy(&info).contains("<string>sparse</string>")
        || !String::from_utf8_lossy(&info).contains("<key>APFS</key>")
    {
        return Err("hdiutil did not confirm the approved 8 GiB sparse APFS image".to_owned());
    }
    let outside_directories =
        allocated_blocks(Path::new(ROOT))?.saturating_add(allocated_blocks(mount)?);
    let device = if resume {
        let attached = Command::new("/usr/bin/hdiutil")
            .arg("info")
            .output()
            .map_err(|error| error.to_string())?;
        if !attached.status.success() {
            return Err("hdiutil attachment state is unknown".to_owned());
        }
        image_info_block(&String::from_utf8_lossy(&attached.stdout), image)
            .and_then(image_block_base_device)
            .ok_or_else(|| "the recorded task image is no longer attached".to_owned())?
    } else {
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
        String::from_utf8_lossy(&attached.stdout)
            .split_whitespace()
            .find(|value| value.starts_with("/dev/disk"))
            .ok_or_else(|| {
                format!(
                    "hdiutil attach did not identify its device: {}",
                    String::from_utf8_lossy(&attached.stdout)
                )
            })?
            .to_owned()
    };
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
    let output = Command::new("/bin/df")
        .arg("-k")
        .arg(path)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "df failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let output_text = String::from_utf8_lossy(&output.stdout);
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
    let output = Command::new("/usr/bin/du")
        .args(["-sk"])
        .arg(path)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "du failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let blocks = String::from_utf8_lossy(&output.stdout)
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

fn local_timestamp() -> String {
    let output = Command::new("/bin/date")
        .arg("-u")
        .arg("+%Y-%m-%dT%H:%M:%SZ")
        .output();
    output.map_or_else(
        |_| "unknown-utc".to_owned(),
        |output| String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    )
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
