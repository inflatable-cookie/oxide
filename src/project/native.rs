use super::source_map::SourceMapRunReceipt;
use super::{
    BoundaryFailure, CancellationProbe, FixtureCommandOutput, FixtureCommandRunner,
    FixtureCommandStatus, FixtureCompletionProof, FixtureExecutionPlan,
};
use crate::storage::{
    CooperativeCompletionEvidence, Credential, LeaseToken, ProcessIdentityEvidence, RunBinding,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(super) const MAX_FIXTURE_CHILDREN: usize = 8;
pub(super) const MAX_FIXTURE_FILES: usize = 16;
pub(super) const MAX_FIXTURE_FILE_BYTES: u64 = 1024 * 1024;
pub(super) const MAX_CAPTURE_STREAM_BYTES: usize = 64 * 1024;
pub(super) const MAX_CARGO_CAPTURE_FILE_BYTES: usize = 512 * 1024 * 1024;
const MAX_CARGO_CAPTURE_FILES: usize = 256;
pub(super) const MAX_DISPOSABLE_BYTES: u64 = 10 * 1024 * 1024 * 1024;
pub(super) const REQUIRED_FREE_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const MINIMUM_FREE_BYTES: u64 = REQUIRED_FREE_BYTES;
const MAX_MANIFEST_BYTES: usize = 16 * 1024;
const CHILD_WAIT_LIMIT: Duration = Duration::from_secs(8);
const SPACE_PROBE_WAIT_LIMIT: Duration = Duration::from_secs(2);
const SPACE_SAMPLE_INTERVAL: Duration = Duration::from_secs(30);
const CONTROL_NAME: &str = "oxide-native-control";
const CHILD_MODE_ENV: &str = "OXIDE_NATIVE_CHILD_MODE";
const CHILD_SLOT_ENV: &str = "OXIDE_NATIVE_CHILD_SLOT";
const CHILD_CONTROL_ENV: &str = "OXIDE_NATIVE_CHILD_CONTROL";
const CHILD_TOKEN_ENV: &str = "OXIDE_NATIVE_CHILD_TOKEN";
const CHILD_RUN_ENV: &str = "OXIDE_NATIVE_CHILD_RUN";
const CHILD_ATTEMPT_ENV: &str = "OXIDE_NATIVE_CHILD_ATTEMPT";
const CHILD_EXPECT_ENV: &str = "OXIDE_NATIVE_CHILD_EXPECT";
const MAX_NEW_FIXTURE_FILES: usize = 7;
const MAX_NEW_FIXTURE_BYTES: u64 = MAX_NEW_FIXTURE_FILES as u64 * MAX_FIXTURE_FILE_BYTES;

static SCENARIO_CHILDREN: OnceLock<Mutex<std::collections::BTreeMap<PathBuf, usize>>> =
    OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct NativeFixtureLimits {
    pub child_processes: usize,
    pub files: usize,
    pub file_bytes: u64,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub total_bytes: u64,
}

impl Default for NativeFixtureLimits {
    fn default() -> Self {
        Self {
            child_processes: MAX_FIXTURE_CHILDREN,
            files: MAX_FIXTURE_FILES,
            file_bytes: MAX_FIXTURE_FILE_BYTES,
            stdout_bytes: MAX_CAPTURE_STREAM_BYTES,
            stderr_bytes: MAX_CAPTURE_STREAM_BYTES,
            total_bytes: MAX_FIXTURE_FILES as u64 * MAX_FIXTURE_FILE_BYTES,
        }
    }
}

impl NativeFixtureLimits {
    pub(super) fn validate(self) -> Result<(), BoundaryFailure> {
        let maximum = (self.files as u64)
            .checked_mul(self.file_bytes)
            .ok_or(BoundaryFailure::Rejected)?;
        if self.child_processes == 0
            || self.child_processes > MAX_FIXTURE_CHILDREN
            || self.files == 0
            || self.files > MAX_FIXTURE_FILES
            || self.file_bytes != MAX_FIXTURE_FILE_BYTES
            || self.stdout_bytes > MAX_CAPTURE_STREAM_BYTES
            || self.stderr_bytes > MAX_CAPTURE_STREAM_BYTES
            || self.total_bytes == 0
            || self.total_bytes > MAX_DISPOSABLE_BYTES
            || maximum > self.total_bytes
        {
            return Err(BoundaryFailure::Rejected);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NativeExecutionClaim {
    pub lease_id: String,
    pub slot_id: String,
    pub slot_path: PathBuf,
    pub temp_dir: PathBuf,
    pub capture_dir: PathBuf,
    pub binding: RunBinding,
    pub host_run_id: String,
    pub workflow: String,
    pub selector: String,
}

impl NativeExecutionClaim {
    pub(super) fn new(
        lease_id: String,
        slot_id: String,
        slot_path: PathBuf,
        binding: RunBinding,
        host_run: super::HostRunIdentity,
        workflow: String,
        selector: String,
        capture_dir: PathBuf,
    ) -> Self {
        let temp_dir = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self {
            lease_id,
            slot_id,
            slot_path,
            temp_dir,
            capture_dir,
            binding,
            host_run_id: host_run.id,
            workflow,
            selector,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NativeCompletionCertificate {
    claim: NativeExecutionClaim,
    repository_head: String,
    workflow_definition_digest: String,
    fixture_source_digest: String,
    process: ProcessIdentityEvidence,
    free_space_samples: Vec<u64>,
    exit_code: i32,
    stdout_digest: String,
    stderr_digest: String,
    capture_digest: String,
    output_references: Vec<String>,
    capture_max_files: usize,
    capture_max_file_bytes: usize,
    native: bool,
}

impl NativeCompletionCertificate {
    pub(super) fn is_registered_cargo(&self) -> bool {
        self.native
            && self.fixture_source_digest == super::cargo::source_digest()
            && self.claim.workflow == super::freshness::WORKFLOW
            && self.claim.selector == super::freshness::SELECTOR
    }

    pub(super) fn capture_digest(&self) -> &str {
        &self.capture_digest
    }

    pub(super) fn binds_source_map_receipt(&self, receipt: &SourceMapRunReceipt) -> bool {
        self.claim.lease_id == receipt.slot_lease_id
            && self.claim.binding.run_id == receipt.run_id
            && self.claim.binding.attempt_id == receipt.attempt_id
            && self.claim.binding.source_tree_digest == receipt.requested_source_tree_digest
            && self.claim.host_run_id == receipt.host_run_id
            && self.claim.slot_path == receipt.slot_path
            && self.claim.capture_dir == receipt.capture_dir
            && self.output_references == receipt.output_references
            && self.stdout_digest == receipt.stdout_digest
            && self.stderr_digest == receipt.stderr_digest
            && self.capture_digest == receipt.capture_digest
    }

    pub(super) fn matches(
        &self,
        plan: &FixtureExecutionPlan,
        output: &FixtureCommandOutput,
    ) -> bool {
        let status_matches = output.status == FixtureCommandStatus::Exited(self.exit_code);
        let native_allowed = self.native || cfg!(test);
        native_allowed
            && status_matches
            && output.output_references == self.output_references
            && matches!(&output.completion, FixtureCompletionProof::Stopped { .. })
            && self.claim == plan.claim
            && self.claim.host_run_id == plan.host_run.id
            && self.claim.slot_path == plan.command.cargo_build_dir
            && self.claim.capture_dir == plan.command.capture_dir
            && valid_digest(&self.repository_head)
            && valid_digest(&self.workflow_definition_digest)
            && valid_digest(&self.fixture_source_digest)
            && self.process.pid > 0
            && valid_digest(&self.process.start_identity)
            && !self.free_space_samples.is_empty()
            && self.free_space_samples.len() <= 16
            && self
                .free_space_samples
                .iter()
                .all(|sample| *sample >= MINIMUM_FREE_BYTES)
            && valid_digest(&self.stdout_digest)
            && valid_digest(&self.stderr_digest)
            && valid_digest(&self.capture_digest)
            && ((self.capture_max_files == MAX_FIXTURE_FILES
                && self.capture_max_file_bytes == MAX_CAPTURE_STREAM_BYTES)
                || (self.capture_max_files == MAX_CARGO_CAPTURE_FILES
                    && self.capture_max_file_bytes == MAX_CARGO_CAPTURE_FILE_BYTES))
            && capture_bundle_digest_with_limits(
                &plan.command.capture_dir,
                &output.output_references,
                self.capture_max_files,
                self.capture_max_file_bytes,
            )
            .is_ok_and(|digest| digest == self.capture_digest)
    }

    pub(super) fn settlement_evidence(
        &self,
        lease_id: &str,
        slot_id: &str,
        slot_path: &Path,
        host_run: Option<&super::HostRunIdentity>,
        capture: &crate::storage::CaptureReceipt,
        token: &LeaseToken,
    ) -> Option<CooperativeCompletionEvidence> {
        let host_run = host_run?;
        if !self.native_allowed()
            || self.claim.lease_id != lease_id
            || self.claim.slot_id != slot_id
            || self.claim.slot_path.as_path() != slot_path
            || self.claim.host_run_id != host_run.id
            || capture.host_run_id != host_run.id
            || capture.run_id != self.claim.binding.run_id
            || capture.attempt_id != self.claim.binding.attempt_id
            || capture.digest != self.capture_digest
        {
            return None;
        }
        let mut evidence = CooperativeCompletionEvidence {
            lease_id: lease_id.to_owned(),
            slot_id: slot_id.to_owned(),
            slot_path: slot_path.to_string_lossy().into_owned(),
            temp_dir: self.claim.temp_dir.to_string_lossy().into_owned(),
            capture_dir: self.claim.capture_dir.to_string_lossy().into_owned(),
            workflow: self.claim.workflow.clone(),
            workflow_definition_digest: self.workflow_definition_digest.clone(),
            repository_head: self.repository_head.clone(),
            fixture_source_digest: self.fixture_source_digest.clone(),
            host_run_id: host_run.id.clone(),
            run_id: self.claim.binding.run_id.clone(),
            attempt_id: self.claim.binding.attempt_id.clone(),
            processes: vec![self.process.clone()],
            free_space_samples: self.free_space_samples.clone(),
            exit_code: self.exit_code,
            stdout_digest: self.stdout_digest.clone(),
            stderr_digest: self.stderr_digest.clone(),
            capture_reference: capture.reference.clone(),
            capture_digest: capture.digest.clone(),
            signature: String::new(),
        };
        evidence.sign(token);
        Some(evidence)
    }

    fn native_allowed(&self) -> bool {
        self.native || cfg!(test)
    }

    #[cfg(test)]
    pub(super) fn for_test(
        plan: &FixtureExecutionPlan,
        status: FixtureCommandStatus,
        output_references: &[String],
    ) -> Self {
        let exit_code = match status {
            FixtureCommandStatus::Exited(code) => code,
            FixtureCommandStatus::Cancelled | FixtureCommandStatus::Incomplete => -1,
        };
        let stdout_digest = file_digest(&plan.command.capture_dir.join("stdout.log"))
            .expect("fixture stdout capture");
        let stderr_digest = file_digest(&plan.command.capture_dir.join("stderr.log"))
            .expect("fixture stderr capture");
        let capture_digest = capture_bundle_digest_with_limits(
            &plan.command.capture_dir,
            output_references,
            MAX_FIXTURE_FILES,
            MAX_CAPTURE_STREAM_BYTES,
        )
        .expect("fixture capture bundle");
        Self {
            claim: plan.claim.clone(),
            repository_head: "0".repeat(40),
            workflow_definition_digest: workflow_definition_digest(
                &plan.claim.workflow,
                &plan.claim.selector,
                &fixture_source_digest(),
            ),
            fixture_source_digest: fixture_source_digest(),
            process: ProcessIdentityEvidence {
                pid: 1,
                start_identity: format!("sha256:{}", "f".repeat(64)),
            },
            free_space_samples: vec![MINIMUM_FREE_BYTES],
            exit_code,
            stdout_digest,
            stderr_digest,
            capture_digest,
            output_references: output_references.to_vec(),
            capture_max_files: MAX_FIXTURE_FILES,
            capture_max_file_bytes: MAX_CAPTURE_STREAM_BYTES,
            native: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct NativeChildStarted {
    pub pid: u32,
    pub start_identity: String,
}

/// The single bounded readiness outcome for a requested fixture rendezvous.
///
/// The runner always reports exactly one of these when a caller asked for a
/// rendezvous, including every pre-readiness refusal, so a caller never has to
/// read a channel timeout or a dropped sender as a statement about the writer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum NativeWriterReadiness {
    /// The owned child proved its held-writer marker and is still active.
    Held(NativeChildStarted),
    /// The fixture refused or failed before a held writer could be proved.
    Refused(NativeReadinessRefusal),
}

/// A bounded, non-secret reason a requested rendezvous did not reach a writer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NativeReadinessRefusal {
    Ownership,
    Budget,
    Reserve,
    Space,
    Directory,
    Identity,
    Spawn,
    StartMarker,
    WriterMarker,
    ChildExit,
    Refused,
    Unavailable,
}

impl NativeReadinessRefusal {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Ownership => "ownership",
            Self::Budget => "budget",
            Self::Reserve => "reserve",
            Self::Space => "space",
            Self::Directory => "directory",
            Self::Identity => "identity",
            Self::Spawn => "spawn",
            Self::StartMarker => "start-marker",
            Self::WriterMarker => "writer-marker",
            Self::ChildExit => "child-exit",
            Self::Refused => "refused",
            Self::Unavailable => "unavailable",
        }
    }

    fn from_boundary(failure: BoundaryFailure) -> Self {
        match failure {
            BoundaryFailure::Unavailable => Self::Unavailable,
            BoundaryFailure::InvalidIdentity => Self::Identity,
            BoundaryFailure::Rejected => Self::Refused,
        }
    }
}

/// Reports at most one bounded readiness outcome for a requested rendezvous.
/// The drop backstop closes the channel with a `Refused` outcome even if an
/// early return forgot to report, so the caller observes a bounded refusal
/// rather than an unrelated timeout.
struct ReadinessReporter {
    sender: Option<Sender<NativeWriterReadiness>>,
}

impl ReadinessReporter {
    fn held(&mut self, started: NativeChildStarted) {
        self.report(NativeWriterReadiness::Held(started));
    }

    fn refuse(&mut self, reason: NativeReadinessRefusal) {
        self.report(NativeWriterReadiness::Refused(reason));
    }

    fn report(&mut self, readiness: NativeWriterReadiness) {
        if let Some(sender) = self.sender.take() {
            // A dropped receiver means the caller no longer observes this
            // rendezvous; the fixture outcome is unchanged either way.
            let _ = sender.send(readiness);
        }
    }
}

impl Drop for ReadinessReporter {
    fn drop(&mut self) {
        self.refuse(NativeReadinessRefusal::Refused);
    }
}

/// Inputs supplied by the registered Cargo runner. The storage core still verifies them.
pub(super) struct CargoCompletion {
    pub claim: NativeExecutionClaim,
    pub repository_head: String,
    pub workflow_definition_digest: String,
    pub fixture_source_digest: String,
    pub process: ProcessIdentityEvidence,
    pub free_space_samples: Vec<u64>,
    pub exit_code: i32,
    pub stdout_digest: String,
    pub stderr_digest: String,
    pub capture_digest: String,
    pub output_references: Vec<String>,
}

impl NativeCompletionCertificate {
    /// Issued only by the registered Cargo runner after a normal exit and complete capture.
    pub(super) fn cargo(parts: CargoCompletion) -> Self {
        Self {
            claim: parts.claim,
            repository_head: parts.repository_head,
            workflow_definition_digest: parts.workflow_definition_digest,
            fixture_source_digest: parts.fixture_source_digest,
            process: parts.process,
            free_space_samples: parts.free_space_samples,
            exit_code: parts.exit_code,
            stdout_digest: parts.stdout_digest,
            stderr_digest: parts.stderr_digest,
            capture_digest: parts.capture_digest,
            output_references: parts.output_references,
            capture_max_files: MAX_CARGO_CAPTURE_FILES,
            capture_max_file_bytes: MAX_CARGO_CAPTURE_FILE_BYTES,
            native: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChildBehavior {
    Complete,
    WaitForRelease,
    LeaveWriterRunning,
    /// Test-only: the child writes a start marker that is not this spawn's
    /// identity and exits, so the parent must refuse before readiness.
    #[cfg(test)]
    WriteBadStartMarker,
}

pub(super) struct NativeFixtureCommandRunner {
    task_root: PathBuf,
    limits: NativeFixtureLimits,
    behavior: ChildBehavior,
    readiness: Option<Sender<NativeWriterReadiness>>,
    release: Option<Receiver<()>>,
    expected_prior: Option<String>,
    pending: Option<(OwnedChild, PathBuf)>,
    executions: usize,
    launched_children: usize,
}

impl NativeFixtureCommandRunner {
    pub(super) fn healthy() -> Result<Self, BoundaryFailure> {
        Self::new(ChildBehavior::Complete, None, None)
    }

    pub(super) fn gated(
        readiness: Sender<NativeWriterReadiness>,
        release: Receiver<()>,
    ) -> Result<Self, BoundaryFailure> {
        Self::new(
            ChildBehavior::WaitForRelease,
            Some(readiness),
            Some(release),
        )
    }

    pub(super) fn retain_writer(
        readiness: Sender<NativeWriterReadiness>,
    ) -> Result<Self, BoundaryFailure> {
        Self::new(ChildBehavior::LeaveWriterRunning, Some(readiness), None)
    }

    #[cfg(test)]
    pub(super) fn bad_start_marker(
        readiness: Sender<NativeWriterReadiness>,
    ) -> Result<Self, BoundaryFailure> {
        Self::new(ChildBehavior::WriteBadStartMarker, Some(readiness), None)
    }

    pub(super) fn with_limits(limits: NativeFixtureLimits) -> Result<Self, BoundaryFailure> {
        limits.validate()?;
        let task_root = task_root()?;
        Ok(Self {
            task_root,
            limits,
            behavior: ChildBehavior::Complete,
            readiness: None,
            release: None,
            expected_prior: None,
            pending: None,
            executions: 0,
            launched_children: 0,
        })
    }

    fn new(
        behavior: ChildBehavior,
        readiness: Option<Sender<NativeWriterReadiness>>,
        release: Option<Receiver<()>>,
    ) -> Result<Self, BoundaryFailure> {
        let mut runner = Self::with_limits(NativeFixtureLimits::default())?;
        runner.behavior = behavior;
        runner.readiness = readiness;
        runner.release = release;
        Ok(runner)
    }

    pub(super) fn executions(&self) -> usize {
        self.executions
    }

    pub(super) fn launched_children(&self) -> usize {
        self.launched_children
    }

    pub(super) fn has_retained_writer(&self) -> bool {
        self.pending.is_some()
    }

    /// Proves the owned writer is still active: its held-writer marker exists
    /// and its child has not exited. This is the readiness claim a retained
    /// writer test must establish before asserting the uncertain outcome.
    pub(super) fn retained_writer_is_active(&mut self) -> bool {
        self.pending.as_mut().is_some_and(|(child, control_dir)| {
            control_dir.join("writer-held").is_file() && child.0.try_wait().ok().flatten().is_none()
        })
    }

    pub(super) fn expect_prior_slot_digest(&mut self, digest: String) {
        self.expected_prior = Some(digest);
    }

    pub(super) fn release_and_reap_retained(&mut self) -> Result<(), BoundaryFailure> {
        let (mut child, control_dir) = self.pending.take().ok_or(BoundaryFailure::Rejected)?;
        write_control(&control_dir.join("release"))?;
        let deadline = Instant::now() + CHILD_WAIT_LIMIT;
        loop {
            if child
                .0
                .try_wait()
                .map_err(|_| BoundaryFailure::Unavailable)?
                .is_some()
            {
                child.0.wait().map_err(|_| BoundaryFailure::Unavailable)?;
                return Ok(());
            }
            if Instant::now() >= deadline {
                let _ = child.0.kill();
                let _ = child.0.wait();
                return Err(BoundaryFailure::Unavailable);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl FixtureCommandRunner for NativeFixtureCommandRunner {
    fn execute(
        &mut self,
        plan: &FixtureExecutionPlan,
        cancellation: &dyn CancellationProbe,
    ) -> Result<FixtureCommandOutput, BoundaryFailure> {
        self.executions = self.executions.saturating_add(1);
        let mut readiness = ReadinessReporter {
            sender: self.readiness.take(),
        };
        let result = self.execute_inner(plan, cancellation, &mut readiness);
        if let Err(error) = &result {
            readiness.refuse(NativeReadinessRefusal::from_boundary(*error));
        }
        result
    }
}

impl NativeFixtureCommandRunner {
    fn execute_inner(
        &mut self,
        plan: &FixtureExecutionPlan,
        cancellation: &dyn CancellationProbe,
        readiness: &mut ReadinessReporter,
    ) -> Result<FixtureCommandOutput, BoundaryFailure> {
        if self.limits.validate().is_err() {
            readiness.refuse(NativeReadinessRefusal::Budget);
            return Err(BoundaryFailure::Rejected);
        }
        if cancellation.is_cancelled() || !claim_is_task_owned(&plan.claim, &self.task_root) {
            readiness.refuse(NativeReadinessRefusal::Ownership);
            return Err(BoundaryFailure::Rejected);
        }
        let fixture_root =
            fixture_root(&self.task_root, &plan.claim.capture_dir).map_err(|error| {
                readiness.refuse(NativeReadinessRefusal::Ownership);
                error
            })?;
        let new_files = match self.behavior {
            ChildBehavior::Complete => 4,
            ChildBehavior::WaitForRelease => 6,
            ChildBehavior::LeaveWriterRunning => 4,
            #[cfg(test)]
            ChildBehavior::WriteBadStartMarker => 4,
        } + usize::from(
            !plan
                .command
                .cargo_build_dir
                .join("fixture-output.bin")
                .exists(),
        );
        check_fixture_budget(&self.task_root, &fixture_root, new_files, self.limits).map_err(
            |error| {
                readiness.refuse(NativeReadinessRefusal::Budget);
                error
            },
        )?;
        let mut child_reservation =
            reserve_scenario_child(&fixture_root, self.limits.child_processes).map_err(
                |error| {
                    readiness.refuse(NativeReadinessRefusal::Reserve);
                    error
                },
            )?;
        let mut space = SpaceMonitor::new(self.task_root.clone());
        space.check().map_err(|error| {
            readiness.refuse(NativeReadinessRefusal::Space);
            error
        })?;
        fs::create_dir_all(&plan.command.capture_dir).map_err(|_| {
            readiness.refuse(NativeReadinessRefusal::Directory);
            BoundaryFailure::Unavailable
        })?;
        let control_dir = plan.command.capture_dir.join(CONTROL_NAME);
        fs::create_dir(&control_dir).map_err(|_| {
            readiness.refuse(NativeReadinessRefusal::Directory);
            BoundaryFailure::Unavailable
        })?;
        let repository_head = repository_head().map_err(|error| {
            readiness.refuse(NativeReadinessRefusal::Identity);
            error
        })?;
        let source_digest = fixture_source_digest();
        let definition_digest =
            workflow_definition_digest(&plan.claim.workflow, &plan.claim.selector, &source_digest);
        let start_identity = spawn_identity(&plan.claim);
        let expected_prior = self.expected_prior.take();
        let child_mode = match self.behavior {
            ChildBehavior::Complete => "complete",
            ChildBehavior::WaitForRelease => "wait",
            ChildBehavior::LeaveWriterRunning => "wait",
            #[cfg(test)]
            ChildBehavior::WriteBadStartMarker => "bad-marker",
        };
        let current_exe = std::env::current_exe().map_err(|_| BoundaryFailure::Unavailable)?;
        let test_name = "project::native::native_fixture_child_entry";
        let mut command = Command::new(current_exe);
        command
            .arg("--exact")
            .arg(test_name)
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env_clear()
            .env("TMPDIR", &plan.claim.temp_dir)
            .env("TMP", &plan.claim.temp_dir)
            .env("TEMP", &plan.claim.temp_dir)
            .env(CHILD_MODE_ENV, child_mode)
            .env(CHILD_SLOT_ENV, &plan.command.cargo_build_dir)
            .env(CHILD_CONTROL_ENV, &control_dir)
            .env(CHILD_TOKEN_ENV, &start_identity)
            .env(CHILD_RUN_ENV, &plan.claim.binding.run_id)
            .env(CHILD_ATTEMPT_ENV, &plan.claim.binding.attempt_id)
            .env(CHILD_EXPECT_ENV, expected_prior.as_deref().unwrap_or(""));
        let captures_stdout = self.behavior != ChildBehavior::LeaveWriterRunning;
        if captures_stdout {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
        } else {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
        let child = command.spawn().map_err(|_| {
            readiness.refuse(NativeReadinessRefusal::Spawn);
            BoundaryFailure::Unavailable
        })?;
        child_reservation.mark_spawned();
        self.launched_children = self.launched_children.saturating_add(1);
        if self.launched_children > self.limits.child_processes {
            let mut child = OwnedChild(child);
            let _ = child.0.kill();
            let _ = child.0.wait();
            readiness.refuse(NativeReadinessRefusal::Reserve);
            return Err(BoundaryFailure::Rejected);
        }
        let mut child = OwnedChild(child);
        let pid = child.0.id();
        let stdout = if captures_stdout {
            Some(spawn_bounded_reader(
                child.0.stdout.take().ok_or_else(|| {
                    readiness.refuse(NativeReadinessRefusal::Spawn);
                    BoundaryFailure::Unavailable
                })?,
                self.limits.stdout_bytes,
            ))
        } else {
            None
        };
        let stderr = if captures_stdout {
            Some(spawn_bounded_reader(
                child.0.stderr.take().ok_or(BoundaryFailure::Unavailable)?,
                self.limits.stderr_bytes,
            ))
        } else {
            None
        };
        if !wait_for_start_marker(&mut child.0, &control_dir, pid, &start_identity, &mut space)? {
            let _ = child.0.kill();
            let _ = child.0.wait();
            let _ = join_reader(stdout)?;
            let _ = join_reader(stderr)?;
            readiness.refuse(NativeReadinessRefusal::StartMarker);
            return Ok(incomplete_output(FixtureCommandStatus::Incomplete));
        }
        if self.behavior != ChildBehavior::Complete
            && !wait_for_control_marker(&mut child.0, &control_dir.join("writer-held"), &mut space)?
        {
            let _ = child.0.kill();
            let _ = child.0.wait();
            let _ = join_reader(stdout)?;
            let _ = join_reader(stderr)?;
            readiness.refuse(NativeReadinessRefusal::WriterMarker);
            return Ok(incomplete_output(FixtureCommandStatus::Incomplete));
        }

        if self.behavior == ChildBehavior::LeaveWriterRunning {
            if child
                .0
                .try_wait()
                .map_err(|_| BoundaryFailure::Unavailable)?
                .is_some()
            {
                child.0.wait().map_err(|_| BoundaryFailure::Unavailable)?;
                readiness.refuse(NativeReadinessRefusal::ChildExit);
                return Ok(incomplete_output(FixtureCommandStatus::Incomplete));
            }
            write_bounded(
                &plan.command.capture_dir.join("writer-progress.txt"),
                b"fixture writer remains active; storage is unavailable\n",
                256,
            )?;
            self.pending = Some((child, control_dir));
            // Readiness is reported only after the owned writer is retained, so
            // a successful outcome proves the held writer really was captured.
            readiness.held(NativeChildStarted {
                pid,
                start_identity: start_identity.clone(),
            });
            return Ok(FixtureCommandOutput::new(
                FixtureCommandStatus::Incomplete,
                vec!["writer-progress.txt".to_owned()],
                None,
                None,
                None,
                None,
                FixtureCompletionProof::Unknown,
            ));
        }

        if self.behavior == ChildBehavior::WaitForRelease {
            let release = self.release.take().ok_or(BoundaryFailure::Rejected)?;
            readiness.held(NativeChildStarted {
                pid,
                start_identity: start_identity.clone(),
            });
            let deadline = Instant::now() + CHILD_WAIT_LIMIT;
            loop {
                if cancellation.is_cancelled() {
                    return cancelled_output(
                        child,
                        stdout,
                        stderr,
                        &plan.command.capture_dir,
                        &mut space,
                    );
                }
                if child
                    .0
                    .try_wait()
                    .map_err(|_| BoundaryFailure::Unavailable)?
                    .is_some()
                {
                    break;
                }
                if let Ok(()) = release.try_recv() {
                    write_control(&control_dir.join("release"))?;
                    break;
                }
                if Instant::now() >= deadline {
                    let _ = child.0.kill();
                    let _ = child.0.wait();
                    return Ok(incomplete_output(FixtureCommandStatus::Incomplete));
                }
                space.check_if_due()?;
                thread::sleep(Duration::from_millis(10));
            }
        }

        let status = wait_child(&mut child.0, cancellation, &mut space)?;
        let Some(status) = status else {
            return cancelled_output(child, stdout, stderr, &plan.command.capture_dir, &mut space);
        };
        let Some(exit_code) = status.code() else {
            return cancelled_or_incomplete_output(
                status,
                stdout,
                stderr,
                &plan.command.capture_dir,
                &mut space,
            );
        };
        let stdout = join_reader(stdout)?;
        let stderr = join_reader(stderr)?;
        let stdout_digest = digest(&stdout.bytes);
        let stderr_digest = digest(&stderr.bytes);
        write_bounded(
            &plan.command.capture_dir.join("stdout.log"),
            &stdout.bytes,
            self.limits.stdout_bytes as u64,
        )?;
        write_bounded(
            &plan.command.capture_dir.join("stderr.log"),
            &stderr.bytes,
            self.limits.stderr_bytes as u64,
        )?;
        space.check()?;
        let identity = ProcessIdentityEvidence {
            pid,
            start_identity: start_identity.clone(),
        };
        let manifest = evidence_manifest(
            &plan.claim,
            &repository_head,
            &definition_digest,
            &source_digest,
            &identity,
            &space.samples,
            exit_code,
            &stdout_digest,
            &stderr_digest,
        );
        if manifest.len() > MAX_MANIFEST_BYTES || !stdout.within_limit || !stderr.within_limit {
            space.check()?;
            return Ok(FixtureCommandOutput {
                status: FixtureCommandStatus::Exited(exit_code),
                output_references: vec!["stdout.log".to_owned(), "stderr.log".to_owned()],
                compile_elapsed_ms: None,
                test_elapsed_ms: None,
                reused_units: None,
                rebuilt_units: None,
                completion: FixtureCompletionProof::Unknown,
                source_map_receipt: None,
                portability_evidence: None,
                native_completion: None,
            });
        }
        write_bounded(
            &plan.command.capture_dir.join("native-evidence.txt"),
            manifest.as_bytes(),
            MAX_MANIFEST_BYTES as u64,
        )?;
        let output_references = vec![
            "stdout.log".to_owned(),
            "stderr.log".to_owned(),
            "native-evidence.txt".to_owned(),
        ];
        let capture_digest = capture_bundle_digest_with_limits(
            &plan.command.capture_dir,
            &output_references,
            MAX_FIXTURE_FILES,
            MAX_CAPTURE_STREAM_BYTES,
        )?;
        let certificate = NativeCompletionCertificate {
            claim: plan.claim.clone(),
            repository_head,
            workflow_definition_digest: definition_digest,
            fixture_source_digest: source_digest,
            process: identity,
            free_space_samples: space.samples.clone(),
            exit_code,
            stdout_digest,
            stderr_digest,
            capture_digest,
            output_references: output_references.clone(),
            capture_max_files: MAX_FIXTURE_FILES,
            capture_max_file_bytes: MAX_CAPTURE_STREAM_BYTES,
            native: true,
        };
        Ok(FixtureCommandOutput {
            status: FixtureCommandStatus::Exited(exit_code),
            output_references,
            compile_elapsed_ms: None,
            test_elapsed_ms: None,
            reused_units: None,
            rebuilt_units: None,
            completion: FixtureCompletionProof::Stopped {
                reference: format!("native:{}", plan.claim.host_run_id),
                credential: Credential::new("native-fixture-runner"),
            },
            source_map_receipt: None,
            portability_evidence: None,
            native_completion: Some(certificate),
        })
    }
}

impl Drop for NativeFixtureCommandRunner {
    fn drop(&mut self) {
        if let Some((mut child, _)) = self.pending.take() {
            if child.0.try_wait().ok().flatten().is_none() {
                let _ = child.0.kill();
                let _ = child.0.wait();
            }
        }
    }
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

struct BoundedOutput {
    bytes: Vec<u8>,
    within_limit: bool,
}

fn spawn_bounded_reader<R: Read + Send + 'static>(
    reader: R,
    maximum: usize,
) -> JoinHandle<BoundedOutput> {
    thread::spawn(move || {
        let mut reader = reader;
        let mut bytes = Vec::with_capacity(maximum.min(4096));
        let mut buffer = [0_u8; 4096];
        let mut within_limit = true;
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let remaining = maximum.saturating_sub(bytes.len());
                    let kept = count.min(remaining);
                    bytes.extend_from_slice(&buffer[..kept]);
                    within_limit &= kept == count;
                }
            }
        }
        BoundedOutput {
            bytes,
            within_limit,
        }
    })
}

fn join_reader(
    reader: Option<JoinHandle<BoundedOutput>>,
) -> Result<BoundedOutput, BoundaryFailure> {
    reader
        .map(|reader| reader.join().map_err(|_| BoundaryFailure::Unavailable))
        .transpose()
        .map(|output| {
            output.unwrap_or(BoundedOutput {
                bytes: Vec::new(),
                within_limit: false,
            })
        })
}

fn cancelled_output(
    mut child: OwnedChild,
    stdout: Option<JoinHandle<BoundedOutput>>,
    stderr: Option<JoinHandle<BoundedOutput>>,
    capture_dir: &Path,
    space: &mut SpaceMonitor,
) -> Result<FixtureCommandOutput, BoundaryFailure> {
    let _ = child.0.kill();
    let _ = child.0.wait();
    let stdout = join_reader(stdout)?;
    let stderr = join_reader(stderr)?;
    write_bounded(
        &capture_dir.join("stdout.log"),
        &stdout.bytes,
        MAX_CAPTURE_STREAM_BYTES as u64,
    )?;
    write_bounded(
        &capture_dir.join("stderr.log"),
        &stderr.bytes,
        MAX_CAPTURE_STREAM_BYTES as u64,
    )?;
    space.check()?;
    Ok(FixtureCommandOutput::new(
        FixtureCommandStatus::Cancelled,
        vec!["stdout.log".to_owned(), "stderr.log".to_owned()],
        None,
        None,
        None,
        None,
        FixtureCompletionProof::Unknown,
    ))
}

fn cancelled_or_incomplete_output(
    status: ExitStatus,
    stdout: Option<JoinHandle<BoundedOutput>>,
    stderr: Option<JoinHandle<BoundedOutput>>,
    capture_dir: &Path,
    space: &mut SpaceMonitor,
) -> Result<FixtureCommandOutput, BoundaryFailure> {
    let stdout = join_reader(stdout)?;
    let stderr = join_reader(stderr)?;
    write_bounded(
        &capture_dir.join("stdout.log"),
        &stdout.bytes,
        MAX_CAPTURE_STREAM_BYTES as u64,
    )?;
    write_bounded(
        &capture_dir.join("stderr.log"),
        &stderr.bytes,
        MAX_CAPTURE_STREAM_BYTES as u64,
    )?;
    space.check()?;
    let output = FixtureCommandOutput::new(
        FixtureCommandStatus::Incomplete,
        vec!["stdout.log".to_owned(), "stderr.log".to_owned()],
        None,
        None,
        None,
        None,
        FixtureCompletionProof::Unknown,
    );
    let _ = status;
    Ok(output)
}

fn wait_child(
    child: &mut Child,
    cancellation: &dyn CancellationProbe,
    space: &mut SpaceMonitor,
) -> Result<Option<ExitStatus>, BoundaryFailure> {
    let deadline = Instant::now() + CHILD_WAIT_LIMIT;
    loop {
        if cancellation.is_cancelled() {
            let _ = child.kill();
            child.wait().map_err(|_| BoundaryFailure::Unavailable)?;
            return Ok(None);
        }
        if let Some(status) = child.try_wait().map_err(|_| BoundaryFailure::Unavailable)? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(BoundaryFailure::Unavailable);
        }
        space.check_if_due()?;
        thread::sleep(Duration::from_millis(10));
    }
}

/// The bounded observation of a child's written start marker against the
/// exact identity this spawn expected. A partial write is not a mismatch: the
/// reader keeps waiting until the file either completes or diverges.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartMarkerObservation {
    Complete,
    Partial,
    Mismatch,
}

fn observe_start_marker(expected: &str, value: &str) -> StartMarkerObservation {
    if value == expected {
        StartMarkerObservation::Complete
    } else if expected.starts_with(value) {
        StartMarkerObservation::Partial
    } else {
        StartMarkerObservation::Mismatch
    }
}

fn wait_for_start_marker(
    child: &mut Child,
    control_dir: &Path,
    expected_pid: u32,
    expected_token: &str,
    space: &mut SpaceMonitor,
) -> Result<bool, BoundaryFailure> {
    let marker = control_dir.join("started");
    let deadline = Instant::now() + CHILD_WAIT_LIMIT;
    let expected = format!("{}\n{}\n", expected_pid, expected_token);
    loop {
        if marker.is_file() {
            let value = fs::read_to_string(&marker).map_err(|_| BoundaryFailure::Unavailable)?;
            match observe_start_marker(&expected, &value) {
                StartMarkerObservation::Complete => return Ok(true),
                // The child creates this marker before writing its identity. A
                // reader can observe that empty or partially written file, so
                // wait for the exact marker instead of classifying that race
                // as an identity mismatch. Any bytes outside the expected
                // prefix remain a fail-closed mismatch.
                StartMarkerObservation::Partial => {}
                StartMarkerObservation::Mismatch => return Ok(false),
            }
        }
        if child
            .try_wait()
            .map_err(|_| BoundaryFailure::Unavailable)?
            .is_some()
        {
            child.wait().map_err(|_| BoundaryFailure::Unavailable)?;
            return Ok(false);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        space.check_if_due()?;
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_control_marker(
    child: &mut Child,
    marker: &Path,
    space: &mut SpaceMonitor,
) -> Result<bool, BoundaryFailure> {
    let deadline = Instant::now() + CHILD_WAIT_LIMIT;
    loop {
        if marker.is_file() {
            return Ok(true);
        }
        if child
            .try_wait()
            .map_err(|_| BoundaryFailure::Unavailable)?
            .is_some()
        {
            child.wait().map_err(|_| BoundaryFailure::Unavailable)?;
            return Ok(false);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        space.check_if_due()?;
        thread::sleep(Duration::from_millis(10));
    }
}

fn incomplete_output(status: FixtureCommandStatus) -> FixtureCommandOutput {
    FixtureCommandOutput::new(
        status,
        Vec::new(),
        None,
        None,
        None,
        None,
        FixtureCompletionProof::Unknown,
    )
}

fn write_control(path: &Path) -> Result<(), BoundaryFailure> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(b"release\n"))
        .map_err(|_| BoundaryFailure::Unavailable)
}

fn write_bounded(path: &Path, bytes: &[u8], maximum: u64) -> Result<(), BoundaryFailure> {
    if bytes.len() as u64 > maximum {
        return Err(BoundaryFailure::Rejected);
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| BoundaryFailure::Unavailable)?;
    file.write_all(bytes)
        .map_err(|_| BoundaryFailure::Unavailable)?;
    file.sync_all().map_err(|_| BoundaryFailure::Unavailable)
}

fn capture_bundle_digest(
    directory: &Path,
    references: &[String],
    per_file_limit: usize,
) -> Result<String, BoundaryFailure> {
    capture_bundle_digest_with_limits(directory, references, MAX_FIXTURE_FILES, per_file_limit)
}

fn capture_bundle_digest_with_limits(
    directory: &Path,
    references: &[String],
    max_files: usize,
    per_file_limit: usize,
) -> Result<String, BoundaryFailure> {
    if references.is_empty() || references.len() > max_files {
        return Err(BoundaryFailure::Rejected);
    }
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut unique = BTreeSet::new();
    for reference in references {
        let relative = Path::new(reference);
        if relative.is_absolute()
            || !relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
            || !unique.insert(reference.as_str())
        {
            return Err(BoundaryFailure::Rejected);
        }
        let path = directory.join(relative);
        let metadata = fs::symlink_metadata(&path).map_err(|_| BoundaryFailure::Unavailable)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(BoundaryFailure::Rejected);
        }
        let canonical = fs::canonicalize(&path).map_err(|_| BoundaryFailure::Unavailable)?;
        if canonical != path || !canonical.starts_with(directory) {
            return Err(BoundaryFailure::Rejected);
        }
        let length = metadata.len();
        if length > per_file_limit as u64 {
            return Err(BoundaryFailure::Rejected);
        }
        total = total
            .checked_add(length)
            .filter(|total| *total <= MAX_DISPOSABLE_BYTES)
            .ok_or(BoundaryFailure::Rejected)?;
        update_field(&mut digest, reference.as_bytes());
        digest.update(length.to_le_bytes());
        let mut file = File::open(&path).map_err(|_| BoundaryFailure::Unavailable)?;
        let mut copied = 0_u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|_| BoundaryFailure::Unavailable)?;
            if read == 0 {
                break;
            }
            copied = copied
                .checked_add(read as u64)
                .filter(|copied| *copied <= length)
                .ok_or(BoundaryFailure::Rejected)?;
            digest.update(&buffer[..read]);
        }
        if copied != length {
            return Err(BoundaryFailure::Unavailable);
        }
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

pub(super) fn capture_receipt_digest(
    directory: &Path,
    references: &[String],
) -> Result<String, BoundaryFailure> {
    capture_bundle_digest(directory, references, MAX_CAPTURE_STREAM_BYTES)
}

pub(super) fn cargo_capture_receipt_digest(
    directory: &Path,
    references: &[String],
) -> Result<String, BoundaryFailure> {
    capture_bundle_digest_with_limits(
        directory,
        references,
        MAX_CARGO_CAPTURE_FILES,
        MAX_CARGO_CAPTURE_FILE_BYTES,
    )
}

fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, BoundaryFailure> {
    let file = File::open(path).map_err(|_| BoundaryFailure::Unavailable)?;
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| BoundaryFailure::Unavailable)?;
    if bytes.len() > maximum {
        return Err(BoundaryFailure::Rejected);
    }
    Ok(bytes)
}

fn file_digest(path: &Path) -> Result<String, BoundaryFailure> {
    Ok(digest(&read_bounded(path, MAX_CAPTURE_STREAM_BYTES)?))
}

fn evidence_manifest(
    claim: &NativeExecutionClaim,
    repository_head: &str,
    definition_digest: &str,
    source_digest: &str,
    process: &ProcessIdentityEvidence,
    free_space_samples: &[u64],
    exit_code: i32,
    stdout_digest: &str,
    stderr_digest: &str,
) -> String {
    format!(
        "oxide-cooperative-fixture-v1\nhead={}\nsource={}\ndefinition={}\nrun={}\nattempt={}\nhost_run={}\nlease={}\nslot={}\nslot_path={}\ntmpdir={}\ncapture_dir={}\nprocess_pid={}\nprocess_start_identity={}\nfree_space_bytes={}\nexit_code={}\nstdout={}\nstderr={}\n",
        repository_head,
        source_digest,
        definition_digest,
        claim.binding.run_id,
        claim.binding.attempt_id,
        claim.host_run_id,
        claim.lease_id,
        claim.slot_id,
        hex(claim.slot_path.to_string_lossy().as_bytes()),
        hex(claim.temp_dir.to_string_lossy().as_bytes()),
        hex(claim.capture_dir.to_string_lossy().as_bytes()),
        process.pid,
        process.start_identity,
        free_space_samples
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(","),
        exit_code,
        stdout_digest,
        stderr_digest,
    )
}

pub(super) fn repository_head() -> Result<String, BoundaryFailure> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let value = fs::read_to_string(&dot_git).map_err(|_| BoundaryFailure::Unavailable)?;
        let relative = value
            .trim()
            .strip_prefix("gitdir: ")
            .ok_or(BoundaryFailure::InvalidIdentity)?;
        let path = PathBuf::from(relative);
        if path.is_absolute() {
            path
        } else {
            root.join(path)
        }
    };
    let head_file =
        fs::read_to_string(git_dir.join("HEAD")).map_err(|_| BoundaryFailure::Unavailable)?;
    let head_value = head_file.trim();
    let head = if let Some(reference) = head_value.strip_prefix("ref: ") {
        let reference_path = Path::new(reference);
        if reference_path.is_absolute()
            || !reference_path
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
        {
            return Err(BoundaryFailure::InvalidIdentity);
        }
        let common_dir = fs::read_to_string(git_dir.join("commondir"))
            .ok()
            .map(|value| {
                let path = PathBuf::from(value.trim());
                if path.is_absolute() {
                    path
                } else {
                    git_dir.join(path)
                }
            })
            .unwrap_or_else(|| git_dir.clone());
        fs::read_to_string(git_dir.join(reference))
            .or_else(|_| fs::read_to_string(common_dir.join(reference)))
            .ok()
            .map(|value| value.trim().to_owned())
            .or_else(|| packed_ref(&common_dir.join("packed-refs"), reference))
            .ok_or(BoundaryFailure::Unavailable)?
    } else {
        head_value.to_owned()
    };
    if !valid_digest(&head) {
        return Err(BoundaryFailure::InvalidIdentity);
    }
    Ok(head)
}

fn packed_ref(path: &Path, expected: &str) -> Option<String> {
    fs::read_to_string(path).ok()?.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let digest = fields.next()?;
        let reference = fields.next()?;
        (reference == expected).then(|| digest.to_owned())
    })
}

fn fixture_source_digest() -> String {
    let mut digest = Sha256::new();
    update_field(&mut digest, include_bytes!("native.rs"));
    update_field(&mut digest, include_bytes!("mod.rs"));
    update_field(&mut digest, include_bytes!("tests.rs"));
    format!("sha256:{:x}", digest.finalize())
}

fn workflow_definition_digest(workflow: &str, selector: &str, source: &str) -> String {
    let mut digest = Sha256::new();
    for value in ["cooperative-foreground-v1", workflow, selector, source] {
        update_field(&mut digest, value.as_bytes());
    }
    format!("sha256:{:x}", digest.finalize())
}

fn spawn_identity(claim: &NativeExecutionClaim) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut digest = Sha256::new();
    for value in [
        claim.binding.run_id.as_str(),
        claim.binding.attempt_id.as_str(),
        claim.host_run_id.as_str(),
    ] {
        update_field(&mut digest, value.as_bytes());
    }
    digest.update(std::process::id().to_le_bytes());
    digest.update(timestamp.to_le_bytes());
    format!("sha256:{:x}", digest.finalize())
}

fn claim_is_task_owned(claim: &NativeExecutionClaim, task_root: &Path) -> bool {
    [
        claim.slot_path.as_path(),
        claim.temp_dir.as_path(),
        claim.capture_dir.as_path(),
    ]
    .into_iter()
    .all(|path| {
        path.starts_with(task_root)
            && path.is_absolute()
            && fs::canonicalize(path).is_ok_and(|canonical| canonical == path)
    })
}

fn fixture_root(task_root: &Path, capture_dir: &Path) -> Result<PathBuf, BoundaryFailure> {
    let relative = capture_dir
        .strip_prefix(task_root)
        .map_err(|_| BoundaryFailure::Rejected)?;
    let mut components = relative.components();
    if components.next().and_then(|part| part.as_os_str().to_str()) != Some("s") {
        return Err(BoundaryFailure::Rejected);
    }
    let scenario = components.next().ok_or(BoundaryFailure::Rejected)?;
    let fixture = components.next().ok_or(BoundaryFailure::Rejected)?;
    let scenario_name = scenario.as_os_str().to_string_lossy();
    let fixture_name = fixture.as_os_str().to_string_lossy();
    let valid_scenario =
        scenario_name.len() == 2 && scenario_name.bytes().all(|byte| byte.is_ascii_digit());
    let valid_fixture = fixture_name.strip_prefix('f').is_some_and(|identity| {
        identity.split_once('-').is_some_and(|(pid, id)| {
            !pid.is_empty()
                && !id.is_empty()
                && pid.bytes().all(|byte| byte.is_ascii_hexdigit())
                && id.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    });
    if !valid_scenario || !valid_fixture {
        return Err(BoundaryFailure::Rejected);
    }
    let root = task_root
        .join("s")
        .join(scenario.as_os_str())
        .join(fixture.as_os_str());
    if !capture_dir.starts_with(&root)
        || fs::canonicalize(&root).map_err(|_| BoundaryFailure::Unavailable)? != root
    {
        return Err(BoundaryFailure::Rejected);
    }
    Ok(root)
}

struct TreeUsage {
    files: usize,
    bytes: u64,
    largest_file: u64,
}

fn tree_usage(root: &Path, account_sibling_entries: bool) -> Result<TreeUsage, BoundaryFailure> {
    let mut usage = TreeUsage {
        files: 0,
        bytes: 0,
        largest_file: 0,
    };
    fn visit(
        root: &Path,
        required: bool,
        account_sibling_entries: bool,
        usage: &mut TreeUsage,
    ) -> Result<(), BoundaryFailure> {
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if !required && error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(BoundaryFailure::Unavailable),
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => return Err(BoundaryFailure::Unavailable),
            };
            let path = entry.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => return Err(BoundaryFailure::Unavailable),
            };
            if metadata.file_type().is_symlink()
                || (!metadata.is_dir() && !metadata.is_file() && !metadata.file_type().is_socket())
            {
                if !account_sibling_entries {
                    return Err(BoundaryFailure::Rejected);
                }
                // The task-wide accounting walk overlaps other isolated fixture
                // tests that deliberately create symlinks and other non-regular
                // entries (a FIFO, for example) under the shared task root. Count
                // the directory entry without following it; the active scenario
                // walk still rejects any non-regular entry in its own tree.
                usage.files = usage
                    .files
                    .checked_add(1)
                    .ok_or(BoundaryFailure::Rejected)?;
                usage.bytes = usage
                    .bytes
                    .checked_add(metadata.len())
                    .ok_or(BoundaryFailure::Rejected)?;
                usage.largest_file = usage.largest_file.max(metadata.len());
                continue;
            }
            if metadata.is_dir() {
                visit(&path, false, account_sibling_entries, usage)?;
            } else if metadata.is_file() {
                usage.files = usage
                    .files
                    .checked_add(1)
                    .ok_or(BoundaryFailure::Rejected)?;
                usage.bytes = usage
                    .bytes
                    .checked_add(metadata.len())
                    .ok_or(BoundaryFailure::Rejected)?;
                usage.largest_file = usage.largest_file.max(metadata.len());
            }
        }
        Ok(())
    }
    visit(root, true, account_sibling_entries, &mut usage)?;
    Ok(usage)
}

fn check_fixture_budget(
    task_root: &Path,
    fixture_root: &Path,
    new_files: usize,
    limits: NativeFixtureLimits,
) -> Result<(), BoundaryFailure> {
    let scenario = tree_usage(fixture_root, false)?;
    let task = tree_usage(task_root, true)?;
    if scenario
        .files
        .checked_add(new_files)
        .ok_or(BoundaryFailure::Rejected)?
        > limits.files
        || scenario.largest_file > limits.file_bytes
        || scenario
            .bytes
            .checked_add(MAX_NEW_FIXTURE_BYTES)
            .ok_or(BoundaryFailure::Rejected)?
            > limits.total_bytes
        || task
            .bytes
            .checked_add(MAX_NEW_FIXTURE_BYTES)
            .ok_or(BoundaryFailure::Rejected)?
            > MAX_DISPOSABLE_BYTES
    {
        return Err(BoundaryFailure::Rejected);
    }
    Ok(())
}

struct ScenarioChildReservation {
    scenario: PathBuf,
    recorded: bool,
}

impl ScenarioChildReservation {
    fn mark_spawned(&mut self) {
        self.recorded = true;
    }
}

impl Drop for ScenarioChildReservation {
    fn drop(&mut self) {
        if self.recorded {
            return;
        }
        if let Some(counts) = SCENARIO_CHILDREN.get() {
            if let Ok(mut counts) = counts.lock() {
                let remove = if let Some(count) = counts.get_mut(&self.scenario) {
                    *count = count.saturating_sub(1);
                    *count == 0
                } else {
                    false
                };
                if remove {
                    counts.remove(&self.scenario);
                }
            }
        }
    }
}

fn reserve_scenario_child(
    scenario: &Path,
    maximum: usize,
) -> Result<ScenarioChildReservation, BoundaryFailure> {
    let counts = SCENARIO_CHILDREN.get_or_init(|| Mutex::new(std::collections::BTreeMap::new()));
    let mut counts = counts.lock().map_err(|_| BoundaryFailure::Unavailable)?;
    let count = counts.entry(scenario.to_path_buf()).or_default();
    if *count >= maximum {
        return Err(BoundaryFailure::Rejected);
    }
    *count += 1;
    Ok(ScenarioChildReservation {
        scenario: scenario.to_path_buf(),
        recorded: false,
    })
}

fn task_root() -> Result<PathBuf, BoundaryFailure> {
    let path = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
        .map(PathBuf::from)
        .ok_or(BoundaryFailure::Rejected)?;
    let canonical = fs::canonicalize(&path).map_err(|_| BoundaryFailure::Unavailable)?;
    if canonical != path {
        return Err(BoundaryFailure::Rejected);
    }
    Ok(canonical)
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub(super) fn expected_slot_digest(run: &str, attempt: &str) -> String {
    digest(format!("run={}\nattempt={}\n", run, attempt).as_bytes())
}

fn valid_digest(value: &str) -> bool {
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    (value.len() == 64 || value.len() == 40) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn update_field(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{:02x}", byte)).collect()
}

struct SpaceMonitor {
    root: PathBuf,
    last_sample: Option<Instant>,
    samples: Vec<u64>,
}

impl SpaceMonitor {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            last_sample: None,
            samples: Vec::new(),
        }
    }

    fn check(&mut self) -> Result<u64, BoundaryFailure> {
        let free = host_free_bytes(&self.root)?;
        if free < MINIMUM_FREE_BYTES {
            return Err(BoundaryFailure::Rejected);
        }
        self.samples.push(free);
        self.last_sample = Some(Instant::now());
        Ok(free)
    }

    fn check_if_due(&mut self) -> Result<(), BoundaryFailure> {
        if self
            .last_sample
            .is_none_or(|last| last.elapsed() >= SPACE_SAMPLE_INTERVAL)
        {
            self.check()?;
        }
        Ok(())
    }
}

pub(super) fn host_free_bytes(path: &Path) -> Result<u64, BoundaryFailure> {
    let mut child = OwnedChild(
        Command::new("/bin/df")
            .args(["-kP"])
            .arg(path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| BoundaryFailure::Unavailable)?,
    );
    let stdout = spawn_bounded_reader(
        child.0.stdout.take().ok_or(BoundaryFailure::Unavailable)?,
        MAX_CAPTURE_STREAM_BYTES,
    );
    let deadline = Instant::now() + SPACE_PROBE_WAIT_LIMIT;
    let status = loop {
        match child.0.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.0.kill();
                let _ = child.0.wait();
                let _ = stdout.join();
                return Err(BoundaryFailure::Unavailable);
            }
            Err(_) => {
                let _ = child.0.kill();
                let _ = child.0.wait();
                let _ = stdout.join();
                return Err(BoundaryFailure::Unavailable);
            }
        }
    };
    child.0.wait().map_err(|_| BoundaryFailure::Unavailable)?;
    let output = stdout.join().map_err(|_| BoundaryFailure::Unavailable)?;
    if !status.success() || !output.within_limit {
        return Err(BoundaryFailure::Unavailable);
    }
    let text = std::str::from_utf8(&output.bytes).map_err(|_| BoundaryFailure::Unavailable)?;
    let row = text.lines().last().ok_or(BoundaryFailure::Unavailable)?;
    let fields: Vec<_> = row.split_whitespace().collect();
    let available_kib: u64 = fields
        .get(3)
        .ok_or(BoundaryFailure::Unavailable)?
        .parse()
        .map_err(|_| BoundaryFailure::Unavailable)?;
    available_kib
        .checked_mul(1024)
        .ok_or(BoundaryFailure::Unavailable)
}

#[cfg(test)]
#[test]
fn native_fixture_child_entry() {
    if std::env::var_os(CHILD_MODE_ENV).is_none() {
        return;
    }
    child_fixture().expect("bounded native fixture child");
}

fn child_fixture() -> io::Result<()> {
    let mode = std::env::var(CHILD_MODE_ENV).map_err(invalid_data)?;
    let slot = PathBuf::from(std::env::var_os(CHILD_SLOT_ENV).ok_or_else(|| invalid_data("slot"))?);
    let control =
        PathBuf::from(std::env::var_os(CHILD_CONTROL_ENV).ok_or_else(|| invalid_data("control"))?);
    let token = std::env::var(CHILD_TOKEN_ENV).map_err(invalid_data)?;
    let run = std::env::var(CHILD_RUN_ENV).map_err(invalid_data)?;
    let attempt = std::env::var(CHILD_ATTEMPT_ENV).map_err(invalid_data)?;
    let expected = std::env::var(CHILD_EXPECT_ENV).unwrap_or_default();
    if mode == "bad-marker" {
        // Test-only refusal path: write a start marker that is not this
        // spawn's identity and exit, so the parent must refuse before
        // readiness rather than reaching a held writer.
        write_bounded(
            &control.join("started"),
            b"not-the-owned-fixture-child\n",
            256,
        )
        .map_err(|_| invalid_data("bad start marker"))?;
        return Ok(());
    }
    let marker = format!("{}\n{}\n", std::process::id(), token);
    write_bounded(&control.join("started"), marker.as_bytes(), 256)
        .map_err(|_| invalid_data("start marker"))?;
    let output = slot.join("fixture-output.bin");
    if !expected.is_empty() {
        let previous = read_bounded(&output, MAX_FIXTURE_FILE_BYTES as usize)
            .map_err(|_| invalid_data("prior slot file"))?;
        if digest(&previous) != expected {
            return Err(invalid_data("unexpected prior slot bytes"));
        }
    }
    let content = format!("run={}\nattempt={}\n", run, attempt);
    if content.len() as u64 > MAX_FIXTURE_FILE_BYTES {
        return Err(invalid_data("fixture file exceeds limit"));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&output)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    println!("fixture child completed bounded write");
    eprintln!("fixture child stderr bounded");
    if mode == "wait" {
        write_bounded(&control.join("writer-held"), b"open slot handle\n", 256)
            .map_err(|_| invalid_data("writer marker"))?;
        let deadline = Instant::now() + CHILD_WAIT_LIMIT;
        while !control.join("release").exists() {
            if Instant::now() >= deadline {
                return Err(invalid_data("release control timed out"));
            }
            thread::sleep(Duration::from_millis(10));
        }
        file.seek(std::io::SeekFrom::End(0))?;
        file.write_all(b"awaited-child-finished\n")?;
        file.sync_all()?;
    }
    Ok(())
}

fn invalid_data(message: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

#[cfg(test)]
mod tests {
    use super::{observe_start_marker, StartMarkerObservation};

    #[test]
    fn start_marker_observation_separates_partial_from_mismatch() {
        let expected = "4321\nsha256:abcdef\n";
        assert_eq!(
            observe_start_marker(expected, "4321\nsha256:abcdef\n"),
            StartMarkerObservation::Complete
        );
        assert_eq!(
            observe_start_marker(expected, ""),
            StartMarkerObservation::Partial
        );
        assert_eq!(
            observe_start_marker(expected, "4321\nsha256:ab"),
            StartMarkerObservation::Partial
        );
        assert_eq!(
            observe_start_marker(expected, "4321\nsha256:abcdef\nstray"),
            StartMarkerObservation::Mismatch
        );
        assert_eq!(
            observe_start_marker(expected, "1234\n"),
            StartMarkerObservation::Mismatch
        );
    }
}
