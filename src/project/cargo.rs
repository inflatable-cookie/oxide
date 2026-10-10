//! Experimental registered Cargo workflow runner for the standalone pilot.
//!
//! The runner launches one selector in its own process group, binds a per-spawn
//! start marker, samples host free space while it runs, and captures complete
//! bounded output. It issues a completion certificate only after a normal exit
//! with no leftover group members. Every other outcome returns an error, so the
//! adapter keeps the lease uncertain. A certificate alone never releases storage:
//! the storage core still checks the lease token, capture binding and bounds.

use super::native::{self, CargoCompletion};
use super::resolve::EffectiveValue;
use super::source_map;
use super::{
    BoundaryFailure, CancellationProbe, FixtureCommandOutput, FixtureCommandRunner,
    FixtureCommandStatus, FixtureCompletionProof, FixtureExecutionPlan,
};
use crate::storage::{Credential, ProcessIdentityEvidence};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Matches the storage verification bound on completion samples.
const MAX_SAMPLES: usize = 16;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const MAX_RUN_TIME: Duration = Duration::from_secs(60 * 60);
const REQUIRED_FREE_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const FORCED_ENVIRONMENT: [&str; 6] = [
    "TMPDIR",
    "TMP",
    "TEMP",
    "CARGO_BUILD_BUILD_DIR",
    "CARGO_TARGET_DIR",
    "OXIDE_CARGO_START_MARKER",
];

/// Runs one registered Cargo selector inside a disposable trial root.
pub(super) struct CargoWorkflowRunner {
    trial_root: PathBuf,
    host_probe: PathBuf,
    executable: PathBuf,
    executable_digest: String,
}

impl CargoWorkflowRunner {
    /// `trial_root` is the canonical disposable mount. `executable` is the absolute selector launcher.
    /// `host_probe` is any path on the host volume whose free space gates each boundary.
    pub(super) fn new(
        trial_root: PathBuf,
        host_probe: PathBuf,
        executable: PathBuf,
    ) -> Result<Self, BoundaryFailure> {
        let trial_root = canonical_directory(&trial_root)?;
        let host_probe = canonical_directory(&host_probe)?;
        if !executable.is_absolute() || !executable.is_file() {
            return Err(BoundaryFailure::Rejected);
        }
        let bytes = fs::read(&executable).map_err(|_| BoundaryFailure::Unavailable)?;
        Ok(Self {
            trial_root,
            host_probe,
            executable,
            executable_digest: digest_parts(&["launcher-v1", &hex_digest(&bytes)]),
        })
    }
}

/// Free-space samples. When full, adjacent samples merge to their minimum.
/// Every raw reading is at least the threshold, because a breach aborts the run.
#[derive(Default)]
struct FreeSpaceLog {
    samples: Vec<u64>,
}

impl FreeSpaceLog {
    fn record(&mut self, free: u64) {
        if self.samples.len() == MAX_SAMPLES {
            self.samples = self
                .samples
                .chunks(2)
                .map(|pair| pair.iter().copied().min().unwrap_or(0))
                .collect();
        }
        self.samples.push(free);
    }
}

impl FixtureCommandRunner for CargoWorkflowRunner {
    fn execute(
        &mut self,
        plan: &FixtureExecutionPlan,
        cancellation: &dyn CancellationProbe,
    ) -> Result<FixtureCommandOutput, BoundaryFailure> {
        let command = &plan.command;
        let claim = &plan.claim;
        for path in [
            &command.workspace_root,
            &command.cargo_build_dir,
            &command.cargo_target_dir,
            &command.capture_dir,
            &claim.temp_dir,
        ] {
            inside_trial(&self.trial_root, path)?;
        }
        if cancellation.is_cancelled() {
            return Err(BoundaryFailure::Rejected);
        }
        if command.source_map.as_ref().is_some_and(|source_map| {
            source_map.requested_tree_digest() != claim.binding.source_tree_digest
                || !command
                    .workspace_root
                    .starts_with(source_map.requested_root())
        }) {
            return Err(BoundaryFailure::InvalidIdentity);
        }
        let run_id = claim.binding.run_id.as_str();
        if !single_component(run_id) || command.environment.iter().any(forces_environment) {
            return Err(BoundaryFailure::Rejected);
        }
        let arguments: Vec<&str> = command.selector.split_whitespace().collect();
        if arguments.is_empty() {
            return Err(BoundaryFailure::Rejected);
        }

        let temp = claim.temp_dir.join(run_id);
        fs::create_dir(&temp).map_err(|_| BoundaryFailure::Rejected)?;
        let definition = digest_parts(&[
            "cargo-foreground-v1",
            &command.workflow,
            &command.selector,
            &self.executable_digest,
            &source_digest(),
        ]);
        let marker = digest_parts(&[
            "cargo-start-v1",
            &claim.binding.run_id,
            &claim.binding.attempt_id,
            &claim.host_run_id,
            &definition,
            &unique_nanos(),
        ]);

        let mut space = FreeSpaceLog::default();
        space.record(self.free_bytes()?);
        let mut process = Command::new(&self.executable);
        process
            .args(&arguments)
            .current_dir(&command.workspace_root)
            .env_clear()
            .envs(
                command
                    .environment
                    .iter()
                    .filter(|value| {
                        command.source_map.is_none()
                            || !matches!(
                                value.name.as_str(),
                                "RUSTFLAGS" | "CARGO_ENCODED_RUSTFLAGS"
                            )
                    })
                    .filter_map(|value| Some((value.name.as_str(), value.value.as_deref()?))),
            )
            .env("TMPDIR", &temp)
            .env("TMP", &temp)
            .env("TEMP", &temp)
            .env("CARGO_BUILD_BUILD_DIR", &command.cargo_build_dir)
            .env("CARGO_TARGET_DIR", &command.cargo_target_dir)
            .env("OXIDE_CARGO_START_MARKER", &marker)
            .stdin(Stdio::null())
            .stdout(File::create(command.capture_dir.join("stdout.log")).map_err(io_failure)?)
            .stderr(File::create(command.capture_dir.join("stderr.log")).map_err(io_failure)?)
            .process_group(0);
        if let Some(source_map) = &command.source_map {
            process.env(
                "CARGO_ENCODED_RUSTFLAGS",
                source_map.effective_rustflags().join("\u{1f}"),
            );
        }
        let started = Instant::now();
        let mut child = process.spawn().map_err(|_| BoundaryFailure::Unavailable)?;
        let pgid = child.id();

        let mut last_sample = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(_) => {
                    kill_group(pgid);
                    let _ = child.wait();
                    return Err(BoundaryFailure::Unavailable);
                }
            }
            if cancellation.is_cancelled() || started.elapsed() > MAX_RUN_TIME {
                kill_group(pgid);
                let _ = child.wait();
                return Err(BoundaryFailure::Rejected);
            }
            if last_sample.elapsed() >= SAMPLE_INTERVAL {
                match self.free_bytes() {
                    Ok(free) => space.record(free),
                    Err(error) => {
                        kill_group(pgid);
                        let _ = child.wait();
                        return Err(error);
                    }
                }
                last_sample = Instant::now();
            }
            thread::sleep(POLL_INTERVAL);
        };
        let elapsed_ms = elapsed_millis(started);
        if group_alive(pgid) {
            return Err(BoundaryFailure::Unavailable);
        }
        let exit_code = status.code().ok_or(BoundaryFailure::Unavailable)?;
        space.record(self.free_bytes()?);

        let stdout = read_capture(&command.capture_dir.join("stdout.log"))?;
        let stderr = read_capture(&command.capture_dir.join("stderr.log"))?;
        let stderr_text = String::from_utf8_lossy(&stderr).into_owned();
        let rebuilt: Vec<&str> = stderr_text
            .lines()
            .filter_map(|line| line.trim_start().strip_prefix("Compiling "))
            .collect();
        let environment_digest = environment_digest(&command.environment);
        let source_map_manifest = command.source_map.as_ref().map_or_else(
            || "source_map_policy=checkout-isolated\nsource_map_digest=none\n".to_owned(),
            |map| {
                format!(
                    "source_map_policy={}\nsource_map_digest={}\nsource_root_id={}\nsource_tree={}\n",
                    map.policy_digest(),
                    map.map_digest(),
                    map.requested_root_id(),
                    claim.binding.source_tree_digest,
                )
            },
        );
        let manifest = format!(
            "oxide-cargo-workflow-v1\nhead={}\nsource={}\ndefinition={}\nlauncher={}\nenvironment={}\n{}run={}\nattempt={}\nhost_run={}\nlease={}\nslot={}\nselector={}\nexit_code={}\nelapsed_ms={}\nfree_space_samples={}\nrebuilt_units={}\nrebuilt_names={}\nstdout={}\nstderr={}\n",
            repository_head(&command.workspace_root)?,
            source_digest(),
            definition,
            self.executable_digest,
            environment_digest,
            source_map_manifest,
            claim.binding.run_id,
            claim.binding.attempt_id,
            claim.host_run_id,
            claim.lease_id,
            command.cargo_build_dir.display(),
            command.selector,
            exit_code,
            elapsed_ms,
            space.samples.iter().map(u64::to_string).collect::<Vec<_>>().join(","),
            rebuilt.len(),
            rebuilt.join(","),
            hex_digest(&stdout),
            hex_digest(&stderr),
        );
        fs::write(
            command.capture_dir.join("manifest.txt"),
            manifest.as_bytes(),
        )
        .map_err(io_failure)?;
        let mut output_references = vec![
            "stdout.log".to_owned(),
            "stderr.log".to_owned(),
            "manifest.txt".to_owned(),
        ];
        let source_map_evidence = if let Some(source_map) = &command.source_map {
            let observations = source_map::read_registered_cargo_hook(
                source_map::EvidenceProvenance::RegisteredCargoWorkflow,
                source_map,
                &claim.lease_id,
                &claim.binding,
                &claim.host_run_id,
                &command.cargo_build_dir,
                &command.cargo_target_dir,
                &command.capture_dir,
                &mut output_references,
            );
            Some(
                source_map::collect(
                    source_map,
                    &claim.lease_id,
                    &claim.binding,
                    &claim.host_run_id,
                    &command.cargo_build_dir,
                    &command.cargo_target_dir,
                    &command.capture_dir,
                    &mut output_references,
                    observations,
                )
                .map_err(|reason| match reason {
                    super::AdapterRefusal::AmbiguousInput
                    | super::AdapterRefusal::UnsupportedInput => BoundaryFailure::Rejected,
                    super::AdapterRefusal::InputUnknown => BoundaryFailure::Unavailable,
                    _ => BoundaryFailure::InvalidIdentity,
                })?,
            )
        } else {
            None
        };
        let capture_digest =
            native::capture_receipt_digest(&command.capture_dir, &output_references)?;
        let certificate = native::NativeCompletionCertificate::cargo(CargoCompletion {
            claim: claim.clone(),
            repository_head: repository_head(&command.workspace_root)?,
            workflow_definition_digest: definition,
            fixture_source_digest: source_digest(),
            process: ProcessIdentityEvidence {
                pid: pgid,
                start_identity: digest_parts(&["cargo-process-v1", &marker, &pgid.to_string()]),
            },
            free_space_samples: space.samples,
            exit_code,
            stdout_digest: hex_digest(&stdout),
            stderr_digest: hex_digest(&stderr),
            capture_digest,
            output_references: output_references.clone(),
        });
        let mut output = FixtureCommandOutput::new(
            FixtureCommandStatus::Exited(exit_code),
            output_references,
            Some(elapsed_ms),
            None,
            None,
            Some(rebuilt.len() as u32),
            FixtureCompletionProof::Stopped {
                reference: format!("cargo-run-{run_id}"),
                credential: Credential::new("cargo-runner-proof"),
            },
        );
        if let Some(collected) = source_map_evidence {
            output.source_map_receipt = Some(collected.receipt);
            output.portability_evidence = Some(collected.evidence);
        }
        output.native_completion = Some(certificate);
        Ok(output)
    }
}

impl CargoWorkflowRunner {
    fn free_bytes(&self) -> Result<u64, BoundaryFailure> {
        let free = native::host_free_bytes(&self.host_probe)?;
        if free < REQUIRED_FREE_BYTES {
            return Err(BoundaryFailure::Rejected);
        }
        Ok(free)
    }
}

fn forces_environment(value: &EffectiveValue) -> bool {
    FORCED_ENVIRONMENT.contains(&value.name.as_str())
}

fn environment_digest(values: &[EffectiveValue]) -> String {
    let mut parts = vec!["cargo-environment-v1".to_owned()];
    for value in values {
        parts.push(value.name.clone());
        parts.push(value.value.clone().unwrap_or_else(|| "<unset>".to_owned()));
    }
    let borrowed: Vec<&str> = parts.iter().map(String::as_str).collect();
    digest_parts(&borrowed)
}

fn source_digest() -> String {
    digest_parts(&["cargo-runner-source-v1", include_str!("cargo.rs")])
}

fn repository_head(workspace: &Path) -> Result<String, BoundaryFailure> {
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(workspace)
        .args(["rev-parse", "HEAD"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stderr(Stdio::null())
        .output()
        .map_err(|_| BoundaryFailure::Unavailable)?;
    if !output.status.success() {
        return Err(BoundaryFailure::Unavailable);
    }
    let head = String::from_utf8(output.stdout).map_err(|_| BoundaryFailure::InvalidIdentity)?;
    let head = head.trim();
    if matches!(head.len(), 40 | 64) && head.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(head.to_owned())
    } else {
        Err(BoundaryFailure::InvalidIdentity)
    }
}

fn inside_trial(root: &Path, path: &Path) -> Result<(), BoundaryFailure> {
    let canonical = fs::canonicalize(path).map_err(|_| BoundaryFailure::Unavailable)?;
    if canonical != path || canonical == root || !canonical.starts_with(root) {
        return Err(BoundaryFailure::Rejected);
    }
    Ok(())
}

fn canonical_directory(path: &Path) -> Result<PathBuf, BoundaryFailure> {
    let canonical = fs::canonicalize(path).map_err(|_| BoundaryFailure::Unavailable)?;
    if !canonical.is_dir() {
        return Err(BoundaryFailure::Rejected);
    }
    Ok(canonical)
}

fn single_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.as_bytes().contains(&0)
}

fn read_capture(path: &Path) -> Result<Vec<u8>, BoundaryFailure> {
    let file = File::open(path).map_err(io_failure)?;
    let mut bytes = Vec::new();
    file.take(native::MAX_CAPTURE_STREAM_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_failure)?;
    if bytes.len() > native::MAX_CAPTURE_STREAM_BYTES {
        return Err(BoundaryFailure::Rejected);
    }
    Ok(bytes)
}

/// Signals the process group this runner created. Used only on abnormal exit.
fn kill_group(pgid: u32) {
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--"])
        .arg(format!("-{pgid}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Unknown probe results count as alive, so ambiguous cleanup stays uncertain.
fn group_alive(pgid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", "--"])
        .arg(format!("-{pgid}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(true)
}

fn io_failure(_: std::io::Error) -> BoundaryFailure {
    BoundaryFailure::Unavailable
}

fn elapsed_millis(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn unique_nanos() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn digest_parts(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part.as_bytes());
    }
    format!("sha256:{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_space_log_keeps_minimum_of_each_merged_pair() {
        let mut log = FreeSpaceLog::default();
        for value in 0..40_u64 {
            log.record(100 + value);
        }
        assert!(log.samples.len() <= MAX_SAMPLES);
        assert_eq!(log.samples.first().copied(), Some(100));
        assert!(log.samples.iter().all(|sample| *sample >= 100));
    }

    #[test]
    fn single_component_rejects_paths_and_empty_ids() {
        assert!(single_component("run-1"));
        assert!(!single_component(""));
        assert!(!single_component(".."));
        assert!(!single_component("a/b"));
    }
}
