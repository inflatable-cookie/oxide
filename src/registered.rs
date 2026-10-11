//! Versioned, strict-physical input and execution seams for audited workflows.
//!
//! There is no built-in registration, authentication, immutable-evidence issuer,
//! or host scheduler. A bounded pinned Cargo dep-info decoder is available, but
//! the embedding consumer installs trusted owners independently of task data.
//! Synthetic owners are only test evidence.
//! Cargo owns graph construction and freshness; this module never writes its
//! fingerprints. The cooperative writer and immutable-owner promises are trust
//! boundaries, not an OS fence against other same-user writers.

use crate::storage::codec::{Reader, Writer};
use crate::RunBinding;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

pub mod cargo_freshness;
pub mod dep_info;
#[cfg(feature = "private-qualification")]
pub mod qualification;
pub mod runner_host;

pub const REGISTRATION_VERSION: u32 = 1;
pub const SOURCE_VERSION: u32 = 1;
pub const DEPENDENCY_VERSION: u32 = 1;
pub const CONFIGURATION_VERSION: u32 = 1;
pub const EVIDENCE_VERSION: u32 = 1;
pub const MAP_VERSION: u32 = 1;
const MAP: &str = "oxide-registered-map-v1";
const PENDING: &str = "oxide-registered-pending-v1";
const CANDIDATE: &str = "oxide-registered-candidate-v1";

/// Engineering ceilings, not measured Nucleus fit. Fixture limits are separate.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub file_bytes: usize,
    pub source_bytes: usize,
    pub collection_bytes: usize,
    pub map_bytes: usize,
    pub associations: usize,
    pub units: usize,
    pub entries_per_unit: usize,
    pub path_bytes: usize,
    pub depth: usize,
}
impl Limits {
    pub const NUCLEUS: Self = Self {
        file_bytes: 16 * 1024 * 1024,
        source_bytes: 256 * 1024 * 1024,
        collection_bytes: 256 * 1024 * 1024,
        map_bytes: 8 * 1024 * 1024,
        associations: 32768,
        units: 256,
        entries_per_unit: 64,
        path_bytes: 2048,
        depth: 32,
    };
    fn validate(self) -> Result<()> {
        let ceiling = Self::NUCLEUS;
        for (value, max) in [
            (self.file_bytes, ceiling.file_bytes),
            (self.source_bytes, ceiling.source_bytes),
            (self.collection_bytes, ceiling.collection_bytes),
            (self.map_bytes, ceiling.map_bytes),
            (self.associations, ceiling.associations),
            (self.units, ceiling.units),
            (self.entries_per_unit, ceiling.entries_per_unit),
            (self.path_bytes, ceiling.path_bytes),
            (self.depth, ceiling.depth),
        ] {
            if value == 0 || value > max {
                return Err(Refusal::Budget);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    MissingAuthority,
    Authentication,
    Unsupported,
    Identity,
    Integrity,
    ImmutableChanged,
    Stale,
    Membership,
    Budget,
    Unstable,
    Io,
    Ineligible,
    Completion,
}
pub type Result<T> = std::result::Result<T, Refusal>;
fn io<T>(_: T) -> Refusal {
    Refusal::Io
}
fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn valid_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value.as_bytes()[7..]
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}

/// Literal launch identity. Only non-secret, audited environment values belong here.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Argv {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub cwd: PathBuf,
    pub environment: BTreeMap<String, String>,
}
impl Argv {
    /// The trusted scheduler may use this command after admission. No shell,
    /// splitting, inherited environment, or implicit executable lookup is used.
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .args(&self.arguments)
            .current_dir(&self.cwd)
            .env_clear()
            .envs(&self.environment);
        command
    }
}

/// Registration authority must audit the entire value, including inventory and
/// every supported script/macro/native behavior read. A digest alone is no audit.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub version: u32,
    pub source_version: u32,
    pub dependency_version: u32,
    pub configuration_version: u32,
    pub id: String,
    pub graph_digest: String,
    pub configuration_digest: String,
    pub toolchain_digest: String,
    pub argv: Argv,
    pub source_files: BTreeSet<String>,
    pub immutable_receipts: BTreeSet<String>,
    pub units: BTreeSet<String>,
    /// Audited effective Cargo-generated and external environment per unit,
    /// including deliberate absence. This is distinct from the launcher env.
    pub unit_environment: BTreeMap<String, BTreeMap<String, Option<String>>>,
    pub tests: BTreeSet<String>,
    pub debug_policy_digest: String,
    pub limits: Limits,
}

/// Supplied task configuration; it has no authority until the consumer verifies
/// the lease, allocation epoch, clean committed source and exact run binding.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub binding: RunBinding,
    pub root_id: String,
    pub allocation_epoch: String,
    pub branch: String,
    pub source_root: PathBuf,
    pub slot: PathBuf,
    pub generation: String,
    pub partition: String,
    pub lease_id: String,
    pub target: PathBuf,
    pub temp: PathBuf,
    pub capture: PathBuf,
    pub authorization: String,
    pub reused: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceEvidence {
    pub version: u32,
    pub commit: String,
    pub tree_digest: String,
    pub clean_committed: bool,
    pub submodules: bool,
    pub lfs: bool,
    pub files: BTreeMap<String, String>,
}

/// Trusted consumer/qualification owner, installed by the embedder, never by a
/// task receipt. Authenticate verifies exclusive allocation and current lease;
/// audit separately verifies complete supported behavior closure and identities.
pub trait RegistrationOwner: Send + Sync {
    fn authenticate(
        &self,
        registration: &Registration,
        attempt: &Attempt,
    ) -> Result<SourceEvidence>;
    fn audit(&self, registration: &Registration, source: &SourceEvidence) -> Result<()>;
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ImmutableClass {
    Registry,
    ToolSdk,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveEvidence {
    pub path: PathBuf,
    pub locked_checksum: String,
}
/// Obtained from the accepted issuer, never from a caller declaration. Issuer
/// verifies archive-to-extraction provenance and no-replacement agreement for
/// this lease; Oxide independently checks all listed bytes and exact membership.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImmutableManifest {
    pub version: u32,
    pub issuer: String,
    pub source_identity: String,
    pub epoch: String,
    pub class: ImmutableClass,
    pub root: PathBuf,
    pub files: BTreeMap<String, String>,
    /// Relative directories, including empty directories, without trailing slash.
    pub directories: BTreeSet<String>,
    pub archive: Option<ArchiveEvidence>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub enum InputRoot {
    Source,
    Generated,
    Immutable(String),
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InputPath {
    pub root: InputRoot,
    pub path: String,
}

/// Semantic variants from pinned Cargo c980f4866 LocalFingerprint. These are
/// library seams, not a new Cargo capture wire format. The evidence owner must
/// verify the real unit graph and decode the pinned evidence without truncation.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub enum LocalInput {
    DepInfo {
        unit: String,
        anchor: String,
        files: Vec<InputPath>,
        environment: BTreeMap<String, Option<String>>,
    },
    RerunFiles {
        output: String,
        paths: Vec<InputPath>,
    },
    RerunEnvironment {
        name: String,
        value: Option<String>,
    },
    Precalculated {
        value: String,
        registry_receipt: String,
    },
    Unsupported,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Association {
    pub input: InputPath,
    pub anchors: BTreeSet<String>,
    /// Generated inputs must name a producer unit; native inputs must name an
    /// accepted ToolSdk receipt. These links do not assert closure by themselves.
    pub producer: Option<String>,
    pub native_receipt: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UnitEvidence {
    pub id: String,
    pub locals: Vec<LocalInput>,
    pub associations: Vec<Association>,
    /// Every real consuming anchor and identity/fingerprint evidence file.
    pub anchors: BTreeSet<String>,
}

/// Accepted input-evidence owner. Calls are bounded by the passed limits; an
/// implementation must not perform an unaccounted native/host scan. Map seals
/// authenticate durable map contents independently of task-provided data.
pub trait InputEvidenceOwner: Send + Sync {
    fn immutable(&self, receipt: &str, attempt: &Attempt) -> Result<ImmutableManifest>;
    fn units(&self, registration: &Registration, attempt: &Attempt) -> Result<Vec<UnitEvidence>>;
    /// Collectors that read Cargo evidence override this method so reads and
    /// decoded allocations share the runner's operation budget. The default is
    /// for owners that return in-memory evidence and perform no producer reads.
    fn units_with_budget(
        &self,
        registration: &Registration,
        attempt: &Attempt,
        _manifests: &BTreeMap<String, ImmutableManifest>,
        _budget: &mut EvidenceReadBudget,
    ) -> Result<Vec<UnitEvidence>> {
        self.units(registration, attempt)
    }
    fn seal_map(&self, attempt: &Attempt, map_digest: &str) -> Result<String>;
    fn verify_map(&self, attempt: &Attempt, map_digest: &str, seal: &str) -> Result<()>;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    pub argv: Argv,
    pub binding: RunBinding,
    pub lease_id: String,
    pub generation: String,
    pub allocation_epoch: String,
    pub branch: String,
    pub root_id: String,
    pub partition: String,
    pub authorization: String,
    pub slot: PathBuf,
    pub target: PathBuf,
    pub temp: PathBuf,
    pub capture: PathBuf,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub enum ExecutionReceipt {
    /// Independently owned terminal/test/artifact/debug evidence reference.
    Completed(String),
    /// Authentic proof that no submission or writer could have begun.
    NotSpawned(String),
}
/// Exact verified input identities that independently retained capture must
/// match, including generated bytes and dependency/tool source provenance.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureInputs {
    pub source_tree: String,
    pub source_files: BTreeMap<String, String>,
    pub immutable_identities: BTreeMap<String, String>,
    pub generated_files: BTreeMap<String, String>,
    pub map_digest: String,
}

/// Scheduler/capture/settlement owner under unchanged Contract010. No built-in
/// implementation exists. verify_success requires successful complete units,
/// always-executed requested tests, cooperative completion and full capture;
/// finalize_idle authenticates exact lease/run/epoch and confirms durable Idle.
pub trait ExecutionOwner: Send + Sync {
    fn execute(&self, launch: &Launch) -> Result<ExecutionReceipt>;
    fn verify_success(
        &self,
        registration: &Registration,
        launch: &Launch,
        receipt: &str,
        inputs: &CaptureInputs,
    ) -> Result<()>;
    fn finalize_idle(&self, launch: &Launch, receipt: &str) -> Result<()>;
    fn finalize_not_spawned(&self, launch: &Launch, receipt: &str) -> Result<()>;
}
#[derive(Debug, Eq, PartialEq)]
pub enum Outcome {
    Published,
    NotSpawned,
}

/// No default owners or eligible Nucleus constants. A genuine audited contract
/// can be supported without changing the fixed fixture's identity gate.
pub struct Runner<'a> {
    pub registration_owner: &'a dyn RegistrationOwner,
    pub input_owner: &'a dyn InputEvidenceOwner,
    pub execution_owner: &'a dyn ExecutionOwner,
}
impl std::fmt::Debug for Runner<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Runner { trusted owners: installed }")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileState {
    digest: String,
    mtime: u64,
    device: u64,
    inode: u64,
    length: u64,
    ctime: i64,
    ctime_nsec: i64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct InputState {
    file: FileState,
    anchors: BTreeSet<String>,
    immutable: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    publication: String,
    identity: String,
    tree: String,
    source: BTreeMap<String, FileState>,
    immutable: BTreeMap<String, String>,
    anchors: BTreeMap<String, FileState>,
    inputs: BTreeMap<InputPath, InputState>,
    directories: BTreeMap<InputPath, BTreeSet<String>>,
    semantics: String,
    watched: BTreeMap<PathBuf, FileState>,
    immutable_members: BTreeMap<PathBuf, (u64, u64, BTreeSet<String>)>,
}

/// Shared per-operation byte, allocation and directory-visit accounting.
/// Only the registered runner constructs a budget for an admitted collection.
pub struct EvidenceReadBudget {
    used: usize,
    maximum: usize,
    limits: Limits,
    files: BTreeMap<PathBuf, FileState>,
    visits: usize,
}
// Keep cached absolute paths out of diagnostic formatting.
impl std::fmt::Debug for EvidenceReadBudget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EvidenceReadBudget")
            .field("used_bytes", &self.used)
            .field("maximum_bytes", &self.maximum)
            .field("cached_file_count", &self.files.len())
            .field("directory_visits", &self.visits)
            .finish_non_exhaustive()
    }
}
type ReadBudget = EvidenceReadBudget;
impl EvidenceReadBudget {
    fn new(maximum: usize, limits: Limits) -> Self {
        Self {
            used: 0,
            maximum,
            limits,
            files: BTreeMap::new(),
            visits: 0,
        }
    }
    fn bytes(&mut self, path: &Path, maximum: usize) -> Result<Vec<u8>> {
        let before = fs::symlink_metadata(path).map_err(io)?;
        if !before.is_file() || before.file_type().is_symlink() {
            return Err(Refusal::Unsupported);
        }
        let limit = maximum.min(self.maximum.checked_sub(self.used).ok_or(Refusal::Budget)?);
        if before.len() > limit as u64 {
            return Err(Refusal::Budget);
        }
        let mut file = File::open(path).map_err(io)?;
        if stamp(&file.metadata().map_err(io)?)? != stamp(&before)? {
            return Err(Refusal::Unstable);
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() > limit {
            return Err(Refusal::Budget);
        }
        self.used += bytes.len();
        let after = fs::symlink_metadata(path).map_err(io)?;
        if stamp(&before)? != stamp(&after)?
            || before.len() != after.len()
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
            || bytes.len() as u64 != after.len()
        {
            return Err(Refusal::Unstable);
        }
        Ok(bytes)
    }
    fn evidence_bytes(&mut self, path: &Path, maximum: usize) -> Result<Vec<u8>> {
        let before = fs::symlink_metadata(path).map_err(io)?;
        let bytes = self.bytes(path, maximum)?;
        let after = fs::symlink_metadata(path).map_err(io)?;
        if stamp(&before)? != stamp(&after)?
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
        {
            return Err(Refusal::Unstable);
        }
        let (mtime, device, inode) = stamp(&after)?;
        let state = FileState {
            digest: digest(&bytes),
            mtime,
            device,
            inode,
            length: after.len(),
            ctime: after.ctime(),
            ctime_nsec: after.ctime_nsec(),
        };
        if self
            .files
            .get(path)
            .is_some_and(|existing| existing != &state)
        {
            return Err(Refusal::Unstable);
        }
        self.files.insert(path.to_owned(), state);
        Ok(bytes)
    }
    fn decoded(&mut self, bytes: usize) -> Result<()> {
        self.used = self
            .used
            .checked_add(bytes)
            .filter(|used| *used <= self.maximum)
            .ok_or(Refusal::Budget)?;
        Ok(())
    }
    fn file(&mut self, path: &Path) -> Result<FileState> {
        if let Some(state) = self.files.get(path) {
            stable_file(path, state)?;
            return Ok(state.clone());
        }
        let before = fs::symlink_metadata(path).map_err(io)?;
        let bytes = self.bytes(path, self.limits.file_bytes)?;
        let after = fs::symlink_metadata(path).map_err(io)?;
        if stamp(&before)? != stamp(&after)?
            || before.ctime() != after.ctime()
            || before.ctime_nsec() != after.ctime_nsec()
        {
            return Err(Refusal::Unstable);
        }
        let (mtime, device, inode) = stamp(&after)?;
        let state = FileState {
            digest: digest(&bytes),
            mtime,
            device,
            inode,
            length: after.len(),
            ctime: after.ctime(),
            ctime_nsec: after.ctime_nsec(),
        };
        self.files.insert(path.to_owned(), state.clone());
        Ok(state)
    }
}
fn stamp(metadata: &fs::Metadata) -> Result<(u64, u64, u64)> {
    let mtime = metadata
        .modified()
        .map_err(io)?
        .duration_since(UNIX_EPOCH)
        .map_err(io)?
        .as_nanos()
        .try_into()
        .map_err(|_| Refusal::Unsupported)?;
    Ok((mtime, metadata.dev(), metadata.ino()))
}
fn canonical_dir(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || fs::canonicalize(path).map_err(io)? != path
        || !fs::symlink_metadata(path).map_err(io)?.is_dir()
    {
        return Err(Refusal::Identity);
    }
    Ok(())
}
fn relative(name: &str, limits: Limits) -> Result<()> {
    let path = Path::new(name);
    if name.is_empty()
        || name.len() > limits.path_bytes
        || name.as_bytes().contains(&0)
        || path.components().count() > limits.depth
        || path
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err(Refusal::Unsupported);
    }
    Ok(())
}
fn owned(root: &Path, name: &str, limits: Limits) -> Result<PathBuf> {
    relative(name, limits)?;
    let mut path = root.to_owned();
    for part in Path::new(name).components() {
        path.push(part.as_os_str());
        if fs::symlink_metadata(&path)
            .map_err(io)?
            .file_type()
            .is_symlink()
        {
            return Err(Refusal::Unsupported);
        }
    }
    if fs::canonicalize(&path).map_err(io)? != path {
        return Err(Refusal::Identity);
    }
    Ok(path)
}
#[cfg(test)]
fn inventory(root: &Path, limits: Limits) -> Result<BTreeSet<String>> {
    membership(root, limits, false, &mut 0)
}
fn membership(
    root: &Path,
    limits: Limits,
    include_directories: bool,
    visits: &mut usize,
) -> Result<BTreeSet<String>> {
    fn walk(
        root: &Path,
        dir: &Path,
        depth: usize,
        limits: Limits,
        visits: &mut usize,
        files: &mut BTreeSet<String>,
        include_directories: bool,
    ) -> Result<()> {
        if depth > limits.depth {
            return Err(Refusal::Budget);
        }
        for entry in fs::read_dir(dir).map_err(io)? {
            *visits += 1;
            if *visits > limits.associations {
                return Err(Refusal::Budget);
            }
            let path = entry.map_err(io)?.path();
            let metadata = fs::symlink_metadata(&path).map_err(io)?;
            let name = path
                .strip_prefix(root)
                .map_err(io)?
                .to_str()
                .ok_or(Refusal::Unsupported)?;
            relative(name, limits)?;
            if metadata.file_type().is_symlink() {
                return Err(Refusal::Unsupported);
            }
            if metadata.is_dir() {
                if include_directories {
                    files.insert(format!("{name}/"));
                }
                walk(
                    root,
                    &path,
                    depth + 1,
                    limits,
                    visits,
                    files,
                    include_directories,
                )?;
            } else if metadata.is_file() {
                files.insert(name.to_owned());
            } else {
                return Err(Refusal::Unsupported);
            }
        }
        Ok(())
    }
    canonical_dir(root)?;
    let mut files = BTreeSet::new();
    walk(
        root,
        root,
        0,
        limits,
        visits,
        &mut files,
        include_directories,
    )?;
    Ok(files)
}

fn write_string(writer: &mut Writer, value: &str) -> Result<()> {
    writer.string(value).map_err(io)
}
fn frame(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
}
fn publication(attempt: &Attempt) -> Result<String> {
    let mut hash = Sha256::new();
    let b = &attempt.binding;
    for value in [
        &b.consumer,
        &b.project,
        &b.component,
        &b.workflow,
        &b.task_id,
        &b.workspace_id,
        &b.run_id,
        &b.attempt_id,
        &b.commit,
        &b.source_tree_digest,
        &attempt.branch,
        &attempt.allocation_epoch,
        &attempt.lease_id,
    ] {
        frame(&mut hash, value.as_bytes());
    }
    for path in [&attempt.target, &attempt.temp, &attempt.capture] {
        frame(
            &mut hash,
            path.to_str().ok_or(Refusal::Identity)?.as_bytes(),
        );
    }
    Ok(format!("sha256:{:x}", hash.finalize()))
}
fn identity(registration: &Registration, attempt: &Attempt) -> Result<String> {
    let mut hash = Sha256::new();
    for value in [
        &registration.id,
        &registration.graph_digest,
        &registration.configuration_digest,
        &registration.toolchain_digest,
        &registration.debug_policy_digest,
        &attempt.root_id,
        &attempt.generation,
        &attempt.partition,
    ] {
        frame(&mut hash, value.as_bytes());
    }
    for path in [
        &attempt.source_root,
        &attempt.slot,
        &registration.argv.executable,
        &registration.argv.cwd,
    ] {
        frame(
            &mut hash,
            path.to_str().ok_or(Refusal::Identity)?.as_bytes(),
        );
    }
    frame(&mut hash, b"argv");
    frame(
        &mut hash,
        &(registration.argv.arguments.len() as u64).to_le_bytes(),
    );
    for arg in &registration.argv.arguments {
        frame(&mut hash, arg.as_bytes());
    }
    frame(&mut hash, b"environment");
    frame(
        &mut hash,
        &(registration.argv.environment.len() as u64).to_le_bytes(),
    );
    for (key, value) in &registration.argv.environment {
        frame(&mut hash, key.as_bytes());
        frame(&mut hash, value.as_bytes());
    }
    frame(&mut hash, b"unit-environment");
    frame(
        &mut hash,
        &(registration.unit_environment.len() as u64).to_le_bytes(),
    );
    for (unit, values) in &registration.unit_environment {
        frame(&mut hash, unit.as_bytes());
        frame(&mut hash, &(values.len() as u64).to_le_bytes());
        for (name, value) in values {
            frame(&mut hash, name.as_bytes());
            frame(&mut hash, &[u8::from(value.is_some())]);
            if let Some(value) = value {
                frame(&mut hash, value.as_bytes());
            }
        }
    }
    for set in [
        &registration.source_files,
        &registration.immutable_receipts,
        &registration.units,
        &registration.tests,
    ] {
        frame(&mut hash, &(set.len() as u64).to_le_bytes());
        for value in set {
            frame(&mut hash, value.as_bytes());
        }
    }
    Ok(format!("sha256:{:x}", hash.finalize()))
}
fn validate(registration: &Registration, attempt: &Attempt) -> Result<()> {
    registration.limits.validate()?;
    let limits = registration.limits;
    if registration.version != REGISTRATION_VERSION
        || registration.source_version != SOURCE_VERSION
        || registration.dependency_version != DEPENDENCY_VERSION
        || registration.configuration_version != CONFIGURATION_VERSION
        || registration.argv.cwd != attempt.source_root
        || registration.units.is_empty()
        || registration.tests.is_empty()
        || registration.units.len() > limits.units
        || registration.source_files.is_empty()
        || registration.source_files.len() > limits.associations
        || registration.immutable_receipts.len() > limits.associations
        || registration.argv.arguments.len() > limits.associations
        || registration.argv.environment.len() > limits.associations
    {
        return Err(Refusal::Unsupported);
    }
    for value in [
        &registration.graph_digest,
        &registration.configuration_digest,
        &registration.toolchain_digest,
        &registration.debug_policy_digest,
        &attempt.partition,
        &attempt.binding.source_tree_digest,
    ] {
        if !valid_digest(value) {
            return Err(Refusal::Identity);
        }
    }
    for value in [
        &registration.id,
        &attempt.root_id,
        &attempt.allocation_epoch,
        &attempt.branch,
        &attempt.generation,
        &attempt.lease_id,
        &attempt.authorization,
        &attempt.binding.run_id,
        &attempt.binding.attempt_id,
    ] {
        if value.is_empty() || value.len() > limits.path_bytes {
            return Err(Refusal::Identity);
        }
    }
    for value in registration
        .argv
        .arguments
        .iter()
        .chain(registration.argv.environment.values())
        .chain(registration.immutable_receipts.iter())
        .chain(registration.units.iter())
        .chain(registration.tests.iter())
    {
        if value.len() > limits.path_bytes || value.as_bytes().contains(&0) {
            return Err(Refusal::Unsupported);
        }
    }
    for key in registration.argv.environment.keys() {
        if [
            "CARGO_BUILD_BUILD_DIR",
            "CARGO_TARGET_DIR",
            "TMPDIR",
            "TMP",
            "TEMP",
        ]
        .contains(&key.as_str())
        {
            return Err(Refusal::Identity);
        }
        if key.is_empty() || key.len() > limits.path_bytes || key.contains(['=', '\0']) {
            return Err(Refusal::Unsupported);
        }
    }
    for path in [
        &attempt.source_root,
        &attempt.slot,
        &attempt.target,
        &attempt.temp,
        &attempt.capture,
    ] {
        canonical_dir(path)?;
        if path.as_os_str().len() > limits.path_bytes {
            return Err(Refusal::Budget);
        }
    }
    if registration
        .unit_environment
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>()
        != registration.units
    {
        return Err(Refusal::Identity);
    }
    let mut env_entries = 0;
    for values in registration.unit_environment.values() {
        env_entries += values.len();
        if env_entries > limits.associations {
            return Err(Refusal::Budget);
        }
        for (name, value) in values {
            if name.is_empty()
                || name.len() > limits.path_bytes
                || name.contains(['=', '\0'])
                || value
                    .as_ref()
                    .is_some_and(|v| v.len() > limits.path_bytes || v.as_bytes().contains(&0))
            {
                return Err(Refusal::Unsupported);
            }
        }
    }
    let roots = [
        &attempt.source_root,
        &attempt.slot,
        &attempt.target,
        &attempt.temp,
        &attempt.capture,
    ];
    for (i, root) in roots.iter().enumerate() {
        for other in roots.iter().skip(i + 1) {
            if root.starts_with(other) || other.starts_with(root) {
                return Err(Refusal::Identity);
            }
        }
    }
    if !registration.argv.executable.is_absolute()
        || fs::canonicalize(&registration.argv.executable).map_err(io)?
            != registration.argv.executable
    {
        return Err(Refusal::Identity);
    }
    Ok(())
}

struct Inputs {
    snapshot: Snapshot,
    manifests: BTreeMap<String, ImmutableManifest>,
    budget: ReadBudget,
}
impl Runner<'_> {
    fn inputs(&self, registration: &Registration, attempt: &Attempt) -> Result<Inputs> {
        let source = self
            .registration_owner
            .authenticate(registration, attempt)?;
        self.registration_owner.audit(registration, &source)?;
        if source.version != SOURCE_VERSION
            || !source.clean_committed
            || source.submodules
            || source.lfs
            || source.commit != attempt.binding.commit
            || source.tree_digest != attempt.binding.source_tree_digest
            || source.files.keys().cloned().collect::<BTreeSet<_>>() != registration.source_files
        {
            return Err(Refusal::Authentication);
        }
        let limits = registration.limits;
        let mut source_budget = ReadBudget::new(limits.source_bytes, limits);
        // The consumer owns Git metadata and excludes it from this authenticated
        // inventory. Inspect only declared source files; no scan of .git/host.
        let mut files = BTreeMap::new();
        let mut source_hash = Sha256::new();
        for (name, expected) in source.files {
            let state = source_budget.file(&owned(&attempt.source_root, &name, limits)?)?;
            if !valid_digest(&expected) || state.digest != expected {
                return Err(Refusal::Integrity);
            }
            frame(&mut source_hash, name.as_bytes());
            frame(&mut source_hash, state.digest.as_bytes());
            files.insert(name, state);
        }
        if format!("sha256:{:x}", source_hash.finalize()) != source.tree_digest {
            return Err(Refusal::Integrity);
        }
        let mut budget = ReadBudget::new(limits.collection_bytes, limits);
        let mut manifests = BTreeMap::new();
        let mut immutable = BTreeMap::new();
        let mut executable_verified = false;
        let mut immutable_members = BTreeMap::new();
        for receipt in &registration.immutable_receipts {
            let manifest = self.input_owner.immutable(receipt, attempt)?;
            if manifest.version != EVIDENCE_VERSION
                || manifest.issuer.is_empty()
                || manifest.source_identity.is_empty()
                || manifest.epoch.is_empty()
                || manifest.files.is_empty()
                || manifest.files.len() + manifest.directories.len() > limits.associations
                || (manifest.class == ImmutableClass::Registry) != manifest.archive.is_some()
            {
                return Err(Refusal::Unsupported);
            }
            for value in [&manifest.issuer, &manifest.source_identity, &manifest.epoch] {
                receipt_bound(value, limits)?;
            }
            canonical_dir(&manifest.root)?;
            if [
                &attempt.source_root,
                &attempt.slot,
                &attempt.target,
                &attempt.temp,
                &attempt.capture,
            ]
            .iter()
            .any(|root| manifest.root.starts_with(root) || root.starts_with(&manifest.root))
            {
                return Err(Refusal::Identity);
            }
            let mut expected_members: BTreeSet<_> = manifest.files.keys().cloned().collect();
            for directory in &manifest.directories {
                relative(directory, limits)?;
                expected_members.insert(format!("{directory}/"));
            }
            if membership(&manifest.root, limits, true, &mut budget.visits)? != expected_members {
                return Err(Refusal::Membership);
            }
            let root_metadata = fs::metadata(&manifest.root).map_err(io)?;
            immutable_members.insert(
                manifest.root.clone(),
                (
                    root_metadata.dev(),
                    root_metadata.ino(),
                    expected_members.clone(),
                ),
            );
            let mut hash = Sha256::new();
            frame(
                &mut hash,
                &[match manifest.class {
                    ImmutableClass::Registry => 0,
                    ImmutableClass::ToolSdk => 1,
                }],
            );
            frame(&mut hash, &root_metadata.dev().to_le_bytes());
            frame(&mut hash, &root_metadata.ino().to_le_bytes());
            for member in &expected_members {
                frame(&mut hash, member.as_bytes());
            }
            for value in [&manifest.issuer, &manifest.source_identity, &manifest.epoch] {
                frame(&mut hash, value.as_bytes());
            }
            frame(
                &mut hash,
                manifest.root.to_str().ok_or(Refusal::Identity)?.as_bytes(),
            );
            for (name, expected) in &manifest.files {
                let path = owned(&manifest.root, name, limits)?;
                let state = budget.file(&path)?;
                if !valid_digest(expected) || state.digest != *expected {
                    return Err(Refusal::ImmutableChanged);
                }
                frame(&mut hash, name.as_bytes());
                frame(&mut hash, state.digest.as_bytes());
                frame(&mut hash, &state.device.to_le_bytes());
                frame(&mut hash, &state.inode.to_le_bytes());
                frame(&mut hash, &state.ctime.to_le_bytes());
                frame(&mut hash, &state.ctime_nsec.to_le_bytes());
                if manifest.class == ImmutableClass::ToolSdk && path == registration.argv.executable
                {
                    executable_verified = true;
                }
            }
            if let Some(archive) = &manifest.archive {
                if fs::canonicalize(&archive.path).map_err(io)? != archive.path {
                    return Err(Refusal::Identity);
                }
                let state = budget.file(&archive.path)?;
                if !valid_digest(&archive.locked_checksum)
                    || state.digest != archive.locked_checksum
                {
                    return Err(Refusal::ImmutableChanged);
                }
                frame(
                    &mut hash,
                    archive.path.to_str().ok_or(Refusal::Identity)?.as_bytes(),
                );
                frame(&mut hash, state.digest.as_bytes());
                frame(&mut hash, &state.device.to_le_bytes());
                frame(&mut hash, &state.inode.to_le_bytes());
                frame(&mut hash, &state.ctime.to_le_bytes());
                frame(&mut hash, &state.ctime_nsec.to_le_bytes());
            }
            immutable.insert(receipt.clone(), format!("sha256:{:x}", hash.finalize()));
            manifests.insert(receipt.clone(), manifest);
        }
        if !executable_verified {
            return Err(Refusal::MissingAuthority);
        }
        for (path, state) in &source_budget.files {
            stable_file(path, state)?;
        }
        for (path, state) in &budget.files {
            stable_file(path, state)?;
        }
        Ok(Inputs {
            snapshot: Snapshot {
                publication: publication(attempt)?,
                identity: identity(registration, attempt)?,
                tree: source.tree_digest,
                source: files,
                immutable,
                anchors: BTreeMap::new(),
                inputs: BTreeMap::new(),
                directories: BTreeMap::new(),
                semantics: String::new(),
                watched: budget.files.clone(),
                immutable_members,
            },
            manifests,
            budget,
        })
    }

    fn collect(
        &self,
        registration: &Registration,
        attempt: &Attempt,
        inputs: Inputs,
    ) -> Result<Snapshot> {
        let limits = registration.limits;
        let mut collector = Collector {
            registration,
            attempt,
            manifests: inputs.manifests,
            snapshot: inputs.snapshot,
            budget: inputs.budget,
            count: 0,
            semantic: Sha256::new(),
            generated: BTreeSet::new(),
        };
        let evidence = {
            let (manifests, budget) = (&collector.manifests, &mut collector.budget);
            self.input_owner
                .units_with_budget(registration, attempt, manifests, budget)?
        };
        if evidence.len() != registration.units.len() || evidence.len() > limits.units {
            return Err(Refusal::Unsupported);
        }
        // Immutable reads share the collection budget and deduplicated file states.
        let mut seen = BTreeSet::new();
        for unit in evidence {
            if !registration.units.contains(&unit.id)
                || !seen.insert(unit.id.clone())
                || unit.anchors.is_empty()
                || unit.anchors.len() > limits.entries_per_unit
                || unit.locals.len() > limits.entries_per_unit
            {
                return Err(Refusal::Unsupported);
            }
            if unit.associations.len() > limits.associations {
                return Err(Refusal::Budget);
            }
            for association in &unit.associations {
                if association.input.root == InputRoot::Generated {
                    if !matches!(&association.producer, Some(p) if registration.units.contains(p)) {
                        return Err(Refusal::Unsupported);
                    }
                    collector.generated.insert(association.input.clone());
                }
            }
            frame(&mut collector.semantic, unit.id.as_bytes());
            for anchor in &unit.anchors {
                let state = collector
                    .budget
                    .file(&owned(&attempt.slot, anchor, limits)?)?;
                collector.snapshot.anchors.insert(anchor.clone(), state);
            }
            let mut local_inputs = BTreeMap::<InputPath, BTreeSet<String>>::new();
            for local in unit.locals {
                match local {
                    LocalInput::DepInfo {
                        unit: evidence_unit,
                        anchor,
                        files,
                        environment,
                    } => {
                        if evidence_unit != unit.id {
                            return Err(Refusal::Identity);
                        }
                        collector.anchor(&unit.anchors, &anchor)?;
                        frame(&mut collector.semantic, b"dep-info");
                        frame(&mut collector.semantic, evidence_unit.as_bytes());
                        frame(&mut collector.semantic, anchor.as_bytes());
                        for input in files {
                            collector.budget.decoded(
                                input_path_allocation(&input)?
                                    .checked_add(anchor.len() + std::mem::size_of::<String>() + 64)
                                    .ok_or(Refusal::Budget)?,
                            )?;
                            let anchors = BTreeSet::from([anchor.clone()]);
                            local_inputs
                                .entry(input.clone())
                                .or_default()
                                .extend(anchors.iter().cloned());
                            collector.associate(input, anchors, false)?;
                        }
                        for (key, value) in environment {
                            collector.environment(&unit.id, &key, value.as_deref())?;
                        }
                    }
                    LocalInput::RerunFiles { output, paths } => {
                        collector.anchor(&unit.anchors, &output)?;
                        frame(&mut collector.semantic, b"rerun-files");
                        frame(&mut collector.semantic, output.as_bytes());
                        for input in paths {
                            collector.budget.decoded(
                                input_path_allocation(&input)?
                                    .checked_add(output.len() + std::mem::size_of::<String>() + 64)
                                    .ok_or(Refusal::Budget)?,
                            )?;
                            local_inputs
                                .entry(input.clone())
                                .or_default()
                                .insert(output.clone());
                            collector.associate(input, BTreeSet::from([output.clone()]), true)?;
                        }
                    }
                    LocalInput::RerunEnvironment { name, value } => {
                        frame(&mut collector.semantic, b"rerun-env");
                        collector.environment(&unit.id, &name, value.as_deref())?;
                    }
                    LocalInput::Precalculated {
                        value,
                        registry_receipt,
                    } => {
                        if !matches!(collector.manifests.get(&registry_receipt), Some(m) if m.class == ImmutableClass::Registry)
                            || value.is_empty()
                            || value.len() > limits.path_bytes
                        {
                            return Err(Refusal::Unsupported);
                        }
                        frame(&mut collector.semantic, b"precalculated");
                        frame(&mut collector.semantic, value.as_bytes());
                        frame(&mut collector.semantic, registry_receipt.as_bytes());
                        let files: Vec<_> = collector
                            .manifests
                            .get(&registry_receipt)
                            .ok_or(Refusal::MissingAuthority)?
                            .files
                            .keys()
                            .cloned()
                            .collect();
                        for path in files {
                            collector.associate(
                                InputPath {
                                    root: InputRoot::Immutable(registry_receipt.clone()),
                                    path,
                                },
                                unit.anchors.clone(),
                                false,
                            )?;
                        }
                    }
                    LocalInput::Unsupported => return Err(Refusal::Unsupported),
                }
            }
            for association in unit.associations {
                if association.anchors.is_empty() {
                    return Err(Refusal::Unsupported);
                }
                for anchor in &association.anchors {
                    collector.anchor(&unit.anchors, anchor)?;
                }
                match (&association.input.root, &association.producer) {
                    (InputRoot::Generated, Some(producer))
                        if registration.units.contains(producer) => {}
                    (InputRoot::Generated, _) => return Err(Refusal::Unsupported),
                    (_, Some(_)) => return Err(Refusal::Unsupported),
                    _ => {}
                }
                if let Some(receipt) = &association.native_receipt {
                    if !matches!(collector.manifests.get(receipt), Some(m) if m.class == ImmutableClass::ToolSdk)
                    {
                        return Err(Refusal::MissingAuthority);
                    }
                    frame(&mut collector.semantic, receipt.as_bytes());
                }
                if let Some(producer) = &association.producer {
                    frame(&mut collector.semantic, producer.as_bytes());
                }
                let mut extra_anchors = BTreeSet::new();
                if let Some(local) = local_inputs.get(&association.input) {
                    for anchor in association.anchors.difference(local) {
                        collector
                            .budget
                            .decoded(anchor.len() + std::mem::size_of::<String>() + 64)?;
                        extra_anchors.insert(anchor.clone());
                    }
                } else {
                    for anchor in &association.anchors {
                        collector
                            .budget
                            .decoded(anchor.len() + std::mem::size_of::<String>() + 64)?;
                        extra_anchors.insert(anchor.clone());
                    }
                }
                if !extra_anchors.is_empty() {
                    let directory = local_inputs.contains_key(&association.input)
                        && collector
                            .snapshot
                            .directories
                            .contains_key(&association.input);
                    collector.associate(association.input, extra_anchors, directory)?;
                }
            }
        }
        for name in &registration.source_files {
            if !collector.snapshot.inputs.contains_key(&InputPath {
                root: InputRoot::Source,
                path: name.clone(),
            }) {
                return Err(Refusal::Unsupported);
            }
        }
        for (path, state) in &collector.budget.files {
            stable_file(path, state)?;
        }
        stable_snapshot(&collector.snapshot, attempt, limits)?;
        collector.snapshot.semantics = format!("sha256:{:x}", collector.semantic.finalize());
        if collector.snapshot.inputs.is_empty() {
            return Err(Refusal::Unsupported);
        }
        if collector.snapshot.inputs.len() + collector.snapshot.anchors.len() > limits.associations
        {
            return Err(Refusal::Budget);
        }
        Ok(collector.snapshot)
    }

    /// Pre-spawn errors preserve the old map. Any error after pending creation
    /// leaves it invalid, including failed/partial completion or lost replies.
    pub fn run(&self, registration: &Registration, attempt: &Attempt) -> Result<Outcome> {
        validate(registration, attempt)?;
        let before_inputs = self.inputs(registration, attempt)?;
        let before = before_inputs.snapshot.clone();
        let admitted = if attempt.reused {
            absent(&attempt.slot.join(PENDING))?;
            absent(&attempt.slot.join(CANDIDATE))?;
            let mut budget = ReadBudget::new(registration.limits.map_bytes, registration.limits);
            let bytes = budget.bytes(
                &owned(&attempt.slot, MAP, registration.limits)?,
                registration.limits.map_bytes,
            )?;
            let (previous, seal) = decode(&bytes, registration.limits)?;
            self.input_owner.verify_map(
                attempt,
                &digest(&encode(&previous, "", registration.limits)?),
                &seal,
            )?;
            let current = self.collect(registration, attempt, before_inputs)?;
            admit(&previous, &current)?;
            current
        } else {
            if fs::read_dir(&attempt.slot).map_err(io)?.next().is_some() {
                return Err(Refusal::Ineligible);
            }
            before.clone()
        };
        stable_snapshot(&admitted, attempt, registration.limits)?;
        let mut argv = registration.argv.clone();
        for (name, path) in [
            ("CARGO_BUILD_BUILD_DIR", &attempt.slot),
            ("CARGO_TARGET_DIR", &attempt.target),
            ("TMPDIR", &attempt.temp),
            ("TMP", &attempt.temp),
            ("TEMP", &attempt.temp),
        ] {
            argv.environment.insert(
                name.to_owned(),
                path.to_str().ok_or(Refusal::Identity)?.to_owned(),
            );
        }
        let launch = Launch {
            argv,
            binding: attempt.binding.clone(),
            lease_id: attempt.lease_id.clone(),
            generation: attempt.generation.clone(),
            allocation_epoch: attempt.allocation_epoch.clone(),
            branch: attempt.branch.clone(),
            root_id: attempt.root_id.clone(),
            partition: attempt.partition.clone(),
            authorization: attempt.authorization.clone(),
            slot: attempt.slot.clone(),
            target: attempt.target.clone(),
            temp: attempt.temp.clone(),
            capture: attempt.capture.clone(),
        };
        // Persist invalidation before asking the scheduler to submit anything.
        durable_create(&attempt.slot, PENDING, attempt.lease_id.as_bytes())?;
        match self.execution_owner.execute(&launch)? {
            ExecutionReceipt::NotSpawned(receipt) => {
                receipt_bound(&receipt, registration.limits)?;
                self.execution_owner
                    .finalize_not_spawned(&launch, &receipt)?;
                remove_pending(attempt)?;
                Ok(Outcome::NotSpawned)
            }
            ExecutionReceipt::Completed(receipt) => {
                receipt_bound(&receipt, registration.limits)?;
                let after_inputs = self.inputs(registration, attempt)?;
                unchanged(&before, &after_inputs.snapshot)?;
                let after = self.collect(registration, attempt, after_inputs)?;
                let body = encode(&after, "", registration.limits)?;
                let captured_inputs = CaptureInputs {
                    source_tree: after.tree.clone(),
                    source_files: after
                        .source
                        .iter()
                        .map(|(name, state)| (name.clone(), state.digest.clone()))
                        .collect(),
                    immutable_identities: after.immutable.clone(),
                    generated_files: after
                        .inputs
                        .iter()
                        .filter_map(|(input, state)| {
                            (input.root == InputRoot::Generated)
                                .then(|| (input.path.clone(), state.file.digest.clone()))
                        })
                        .collect(),
                    map_digest: digest(&body),
                };
                self.execution_owner.verify_success(
                    registration,
                    &launch,
                    &receipt,
                    &captured_inputs,
                )?;
                let seal = self.input_owner.seal_map(attempt, &digest(&body))?;
                if seal.is_empty() || seal.len() > registration.limits.path_bytes {
                    return Err(Refusal::Authentication);
                }
                let bytes = encode(&after, &seal, registration.limits)?;
                durable_create(&attempt.slot, CANDIDATE, &bytes)?;
                // Capture and all input collection complete under the lease.
                // Confirm source/tool identity again before authenticated Idle.
                stable_snapshot(&after, attempt, registration.limits)?;
                fs::rename(attempt.slot.join(CANDIDATE), attempt.slot.join(MAP)).map_err(io)?;
                sync(&attempt.slot)?;
                self.execution_owner.finalize_idle(&launch, &receipt)?;
                stable_snapshot(&after, attempt, registration.limits)?;
                remove_pending(attempt)?;
                Ok(Outcome::Published)
            }
        }
    }
}
fn input_path_allocation(input: &InputPath) -> Result<usize> {
    std::mem::size_of::<InputPath>()
        .checked_add(input.path.len())
        .and_then(|size| {
            size.checked_add(match &input.root {
                InputRoot::Immutable(receipt) => receipt.len(),
                _ => 0,
            })
        })
        .and_then(|size| size.checked_add(64))
        .ok_or(Refusal::Budget)
}
fn receipt_bound(receipt: &str, limits: Limits) -> Result<()> {
    if receipt.is_empty() || receipt.len() > limits.path_bytes || receipt.as_bytes().contains(&0) {
        return Err(Refusal::Authentication);
    }
    Ok(())
}
fn unchanged(before: &Snapshot, after: &Snapshot) -> Result<()> {
    if before.identity != after.identity
        || before.tree != after.tree
        || before.source != after.source
    {
        return Err(Refusal::Integrity);
    }
    if before.immutable != after.immutable {
        return Err(Refusal::ImmutableChanged);
    }
    Ok(())
}
fn admit(previous: &Snapshot, current: &Snapshot) -> Result<()> {
    if previous.identity != current.identity {
        return Err(Refusal::Identity);
    }
    if previous.immutable != current.immutable {
        return Err(Refusal::ImmutableChanged);
    }
    if previous.anchors != current.anchors
        || previous.semantics != current.semantics
        || previous.inputs.keys().ne(current.inputs.keys())
    {
        return Err(Refusal::Integrity);
    }
    if previous.directories != current.directories {
        return Err(Refusal::Membership);
    }
    for (input, old) in &previous.inputs {
        let new = current.inputs.get(input).ok_or(Refusal::Integrity)?;
        if old.anchors != new.anchors || old.immutable != new.immutable {
            return Err(Refusal::Integrity);
        }
        if old.file.digest == new.file.digest {
            continue;
        }
        if old.immutable {
            return Err(Refusal::ImmutableChanged);
        }
        for anchor in &old.anchors {
            if new.file.mtime
                <= previous
                    .anchors
                    .get(anchor)
                    .ok_or(Refusal::Integrity)?
                    .mtime
            {
                return Err(Refusal::Stale);
            }
        }
    }
    Ok(())
}
struct Collector<'a> {
    registration: &'a Registration,
    attempt: &'a Attempt,
    manifests: BTreeMap<String, ImmutableManifest>,
    snapshot: Snapshot,
    budget: ReadBudget,
    count: usize,
    semantic: Sha256,
    generated: BTreeSet<InputPath>,
}
impl Collector<'_> {
    fn anchor(&self, unit: &BTreeSet<String>, anchor: &str) -> Result<()> {
        if !unit.contains(anchor) {
            return Err(Refusal::Unsupported);
        }
        Ok(())
    }
    fn environment(&mut self, unit: &str, key: &str, value: Option<&str>) -> Result<()> {
        if key.is_empty()
            || key.len() > self.registration.limits.path_bytes
            || value.is_some_and(|v| v.len() > self.registration.limits.path_bytes)
            || self
                .registration
                .unit_environment
                .get(unit)
                .and_then(|values| values.get(key))
                .map(|expected| expected.as_deref())
                != Some(value)
        {
            return Err(Refusal::Identity);
        }
        self.count += 1;
        if self.count > self.registration.limits.associations {
            return Err(Refusal::Budget);
        }
        frame(&mut self.semantic, key.as_bytes());
        frame(&mut self.semantic, &[u8::from(value.is_some())]);
        if let Some(value) = value {
            frame(&mut self.semantic, value.as_bytes());
        }
        Ok(())
    }
    fn base(&self, input: &InputPath) -> Result<&Path> {
        match &input.root {
            InputRoot::Source => Ok(&self.attempt.source_root),
            InputRoot::Generated => Ok(&self.attempt.slot),
            InputRoot::Immutable(receipt) => Ok(&self
                .manifests
                .get(receipt)
                .ok_or(Refusal::MissingAuthority)?
                .root),
        }
    }
    fn associate(
        &mut self,
        input: InputPath,
        anchors: BTreeSet<String>,
        directory: bool,
    ) -> Result<()> {
        let limits = self.registration.limits;
        relative(&input.path, limits)?;
        if input.root == InputRoot::Generated && !self.generated.contains(&input) {
            return Err(Refusal::Unsupported);
        }
        let path = owned(self.base(&input)?, &input.path, limits)?;
        if directory && path.is_dir() {
            if matches!(input.root, InputRoot::Immutable(_)) {
                return Err(Refusal::Unsupported);
            }
            let members = membership(&path, limits, true, &mut self.budget.visits)?;
            self.count += members.len() + 1;
            if self.count > limits.associations {
                return Err(Refusal::Budget);
            }
            self.snapshot
                .directories
                .insert(input.clone(), members.clone());
            for member in members.into_iter().filter(|name| !name.ends_with('/')) {
                self.associate(
                    InputPath {
                        root: input.root.clone(),
                        path: format!("{}/{member}", input.path),
                    },
                    anchors.clone(),
                    false,
                )?;
            }
            return Ok(());
        }
        self.count += anchors.len();
        if self.count > limits.associations {
            return Err(Refusal::Budget);
        }
        let file = match &input.root {
            InputRoot::Source => self
                .snapshot
                .source
                .get(&input.path)
                .cloned()
                .ok_or(Refusal::Unsupported)?,
            InputRoot::Immutable(receipt) => {
                let manifest = self
                    .manifests
                    .get(receipt)
                    .ok_or(Refusal::MissingAuthority)?;
                let expected = manifest
                    .files
                    .get(&input.path)
                    .ok_or(Refusal::Unsupported)?;
                let state = self.budget.file(&path)?;
                if state.digest != *expected {
                    return Err(Refusal::ImmutableChanged);
                }
                state
            }
            InputRoot::Generated => self.budget.file(&path)?,
        };
        // Raw Cargo dep-info must not be used to infer native/header closure.
        let immutable = matches!(input.root, InputRoot::Immutable(_));
        if let Some(existing) = self.snapshot.inputs.get_mut(&input) {
            existing.anchors.extend(anchors);
        } else {
            self.snapshot.inputs.insert(
                input,
                InputState {
                    file,
                    anchors,
                    immutable,
                },
            );
        }
        Ok(())
    }
}
fn absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(Refusal::Ineligible),
    }
}
fn sync(path: &Path) -> Result<()> {
    File::open(path).and_then(|f| f.sync_all()).map_err(io)
}
fn durable_create(slot: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(slot.join(name))
        .map_err(io)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(io)?;
    sync(slot)
}
fn remove_pending(attempt: &Attempt) -> Result<()> {
    let mut bytes = Vec::new();
    File::open(attempt.slot.join(PENDING))
        .map_err(io)?
        .take(2049)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes != attempt.lease_id.as_bytes() {
        return Err(Refusal::Authentication);
    }
    fs::remove_file(attempt.slot.join(PENDING)).map_err(io)?;
    sync(&attempt.slot)
}

#[cfg(test)]
mod tests;

fn stable_file(path: &Path, state: &FileState) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(io)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || stamp(&metadata)? != (state.mtime, state.device, state.inode)
        || metadata.len() != state.length
        || metadata.ctime() != state.ctime
        || metadata.ctime_nsec() != state.ctime_nsec
    {
        return Err(Refusal::Unstable);
    }
    Ok(())
}
fn stable_snapshot(snapshot: &Snapshot, attempt: &Attempt, limits: Limits) -> Result<()> {
    for (path, state) in &snapshot.watched {
        stable_file(path, state)?;
    }
    let mut visits = 0;
    for (root, (device, inode, members)) in &snapshot.immutable_members {
        canonical_dir(root)?;
        let metadata = fs::metadata(root).map_err(io)?;
        if metadata.dev() != *device
            || metadata.ino() != *inode
            || membership(root, limits, true, &mut visits)? != *members
        {
            return Err(Refusal::ImmutableChanged);
        }
    }
    for (input, members) in &snapshot.directories {
        let root = match input.root {
            InputRoot::Source => &attempt.source_root,
            InputRoot::Generated => &attempt.slot,
            InputRoot::Immutable(_) => return Err(Refusal::Unsupported),
        };
        if membership(
            &owned(root, &input.path, limits)?,
            limits,
            true,
            &mut visits,
        )? != *members
        {
            return Err(Refusal::Membership);
        }
    }
    for (name, state) in &snapshot.source {
        stable_file(&owned(&attempt.source_root, name, limits)?, state)?;
    }
    for (name, state) in &snapshot.anchors {
        stable_file(&owned(&attempt.slot, name, limits)?, state)?;
    }
    for (input, state) in &snapshot.inputs {
        if input.root == InputRoot::Generated {
            stable_file(&owned(&attempt.slot, &input.path, limits)?, &state.file)?;
        }
    }
    Ok(())
}

fn encode_file(writer: &mut Writer, file: &FileState) -> Result<()> {
    write_string(writer, &file.digest)?;
    for value in [
        file.mtime,
        file.device,
        file.inode,
        file.length,
        file.ctime as u64,
        file.ctime_nsec as u64,
    ] {
        writer.u64(value);
    }
    Ok(())
}
fn encode_path(writer: &mut Writer, input: &InputPath) -> Result<()> {
    match &input.root {
        InputRoot::Source => writer.u8(0),
        InputRoot::Generated => writer.u8(1),
        InputRoot::Immutable(receipt) => {
            writer.u8(2);
            write_string(writer, receipt)?;
        }
    }
    write_string(writer, &input.path)
}
fn encode_set(writer: &mut Writer, values: &BTreeSet<String>) -> Result<()> {
    writer.u32(values.len().try_into().map_err(|_| Refusal::Budget)?);
    for value in values {
        write_string(writer, value)?;
    }
    Ok(())
}
fn encode(snapshot: &Snapshot, seal: &str, limits: Limits) -> Result<Vec<u8>> {
    let mut writer = Writer::new();
    writer.u32(MAP_VERSION);
    for value in [
        &snapshot.publication,
        &snapshot.identity,
        &snapshot.tree,
        &snapshot.semantics,
    ] {
        write_string(&mut writer, value)?;
    }
    for files in [&snapshot.source, &snapshot.anchors] {
        writer.u32(files.len().try_into().map_err(|_| Refusal::Budget)?);
        for (name, file) in files {
            write_string(&mut writer, name)?;
            encode_file(&mut writer, file)?;
        }
    }
    writer.u32(
        snapshot
            .immutable
            .len()
            .try_into()
            .map_err(|_| Refusal::Budget)?,
    );
    for (receipt, hash) in &snapshot.immutable {
        write_string(&mut writer, receipt)?;
        write_string(&mut writer, hash)?;
    }
    writer.u32(
        snapshot
            .inputs
            .len()
            .try_into()
            .map_err(|_| Refusal::Budget)?,
    );
    for (input, state) in &snapshot.inputs {
        encode_path(&mut writer, input)?;
        encode_file(&mut writer, &state.file)?;
        encode_set(&mut writer, &state.anchors)?;
        writer.u8(u8::from(state.immutable));
    }
    writer.u32(
        snapshot
            .directories
            .len()
            .try_into()
            .map_err(|_| Refusal::Budget)?,
    );
    for (input, members) in &snapshot.directories {
        encode_path(&mut writer, input)?;
        encode_set(&mut writer, members)?;
    }
    write_string(&mut writer, seal)?;
    let bytes = writer.finish();
    if bytes.len() > limits.map_bytes {
        return Err(Refusal::Budget);
    }
    Ok(bytes)
}
fn count(reader: &mut Reader<'_>, maximum: usize) -> Result<usize> {
    let value = reader.u32().map_err(io)? as usize;
    if value > maximum {
        return Err(Refusal::Budget);
    }
    Ok(value)
}
fn text(reader: &mut Reader<'_>, limits: Limits) -> Result<String> {
    let value = reader.string().map_err(io)?;
    if value.len() > limits.path_bytes || value.as_bytes().contains(&0) {
        return Err(Refusal::Budget);
    }
    Ok(value)
}
fn read_file(reader: &mut Reader<'_>, limits: Limits) -> Result<FileState> {
    let digest = text(reader, limits)?;
    if !valid_digest(&digest) {
        return Err(Refusal::Integrity);
    }
    Ok(FileState {
        digest,
        mtime: reader.u64().map_err(io)?,
        device: reader.u64().map_err(io)?,
        inode: reader.u64().map_err(io)?,
        length: reader.u64().map_err(io)?,
        ctime: reader.u64().map_err(io)? as i64,
        ctime_nsec: reader.u64().map_err(io)? as i64,
    })
}
fn read_path(reader: &mut Reader<'_>, limits: Limits) -> Result<InputPath> {
    let root = match reader.u8().map_err(io)? {
        0 => InputRoot::Source,
        1 => InputRoot::Generated,
        2 => InputRoot::Immutable(text(reader, limits)?),
        _ => return Err(Refusal::Unsupported),
    };
    let path = text(reader, limits)?;
    relative(&path, limits)?;
    Ok(InputPath { root, path })
}
fn read_set(reader: &mut Reader<'_>, limits: Limits) -> Result<BTreeSet<String>> {
    let mut values = BTreeSet::new();
    for _ in 0..count(reader, limits.associations)? {
        if !values.insert(text(reader, limits)?) {
            return Err(Refusal::Integrity);
        }
    }
    Ok(values)
}
fn read_files(reader: &mut Reader<'_>, limits: Limits) -> Result<BTreeMap<String, FileState>> {
    let mut files = BTreeMap::new();
    for _ in 0..count(reader, limits.associations)? {
        let name = text(reader, limits)?;
        relative(&name, limits)?;
        if files.insert(name, read_file(reader, limits)?).is_some() {
            return Err(Refusal::Integrity);
        }
    }
    Ok(files)
}
fn decode(bytes: &[u8], limits: Limits) -> Result<(Snapshot, String)> {
    if bytes.len() > limits.map_bytes {
        return Err(Refusal::Budget);
    }
    let mut reader = Reader::new(bytes);
    if reader.u32().map_err(io)? != MAP_VERSION {
        return Err(Refusal::Unsupported);
    }
    let publication = text(&mut reader, limits)?;
    let identity = text(&mut reader, limits)?;
    let tree = text(&mut reader, limits)?;
    let semantics = text(&mut reader, limits)?;
    if [&publication, &identity, &tree, &semantics]
        .iter()
        .any(|value| !valid_digest(value))
    {
        return Err(Refusal::Integrity);
    }
    let source = read_files(&mut reader, limits)?;
    let anchors = read_files(&mut reader, limits)?;
    let mut immutable = BTreeMap::new();
    for _ in 0..count(&mut reader, limits.associations)? {
        let receipt = text(&mut reader, limits)?;
        let hash = text(&mut reader, limits)?;
        if !valid_digest(&hash) || immutable.insert(receipt, hash).is_some() {
            return Err(Refusal::Integrity);
        }
    }
    let mut inputs = BTreeMap::new();
    let mut associations = 0;
    for _ in 0..count(&mut reader, limits.associations)? {
        let input = read_path(&mut reader, limits)?;
        let file = read_file(&mut reader, limits)?;
        let references = read_set(&mut reader, limits)?;
        associations += references.len();
        if references.is_empty()
            || associations > limits.associations
            || references.iter().any(|r| !anchors.contains_key(r))
        {
            return Err(Refusal::Integrity);
        }
        let immutable = match reader.u8().map_err(io)? {
            0 => false,
            1 => true,
            _ => return Err(Refusal::Integrity),
        };
        if immutable != matches!(input.root, InputRoot::Immutable(_))
            || inputs
                .insert(
                    input,
                    InputState {
                        file,
                        anchors: references,
                        immutable,
                    },
                )
                .is_some()
        {
            return Err(Refusal::Integrity);
        }
    }
    let mut directories = BTreeMap::new();
    for _ in 0..count(&mut reader, limits.associations)? {
        let input = read_path(&mut reader, limits)?;
        let members = read_set(&mut reader, limits)?;
        for member in &members {
            relative(member, limits)?;
        }
        if directories.insert(input, members).is_some() {
            return Err(Refusal::Integrity);
        }
    }
    let seal = text(&mut reader, limits)?;
    if seal.is_empty() || !reader.done() {
        return Err(Refusal::Authentication);
    }
    Ok((
        Snapshot {
            publication,
            identity,
            tree,
            source,
            immutable,
            anchors,
            inputs,
            directories,
            semantics,
            watched: BTreeMap::new(),
            immutable_members: BTreeMap::new(),
        },
        seal,
    ))
}
