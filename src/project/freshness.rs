//! Pinned Cargo 1.97.1 admission for the fixed, local, strict-physical fixture.
//! Cargo still owns graph freshness. This map rejects old/equal-mtime changes;
//! the durable barrier closes the partial-success/overall-failure hole.

use super::resolve::{EffectiveInputSet, PathIdentity, ProjectInvocation, WorkflowOptIn};
use super::{AdapterRefusal, FixtureExecutionPlan};
use crate::storage::codec::{Reader, Writer};
use crate::storage::{PartitionKey, SlotState};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

const MAP: &str = "oxide-freshness-v1.bin";
const BARRIER: &str = "oxide-freshness-pending-v1";
const TEMP: &str = "oxide-freshness-candidate-v1";
const MAX_ENTRIES: usize = 4096;
const MAX_MAP_BYTES: usize = 1024 * 1024;
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_READ_BYTES: usize = 32 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 1024;
const MAX_UNITS: usize = 16;
pub(super) const WORKFLOW: &str = "oxide-cargo-same-root-fixture";
pub(super) const SELECTOR: &str = "test --offline --locked --lib --bins --tests --message-format=json-render-diagnostics -- --nocapture";
/// The only noncanonical Rust source admitted is a future failed-build overlay.
pub(super) const FAILURE_OVERLAY: &[u8] = b"\noxide_intentional_syntax_error! {\n";
const FILES: [(&str, &[u8]); 9] = [
    (
        "Cargo.toml",
        include_bytes!("../../fixtures/cargo-source-map-probe/Cargo.toml"),
    ),
    (
        "Cargo.lock",
        include_bytes!("../../fixtures/cargo-source-map-probe/Cargo.lock"),
    ),
    (
        "probe/Cargo.toml",
        include_bytes!("../../fixtures/cargo-source-map-probe/probe/Cargo.toml"),
    ),
    (
        "probe/build.rs",
        include_bytes!("../../fixtures/cargo-source-map-probe/probe/build.rs"),
    ),
    (
        "probe/marker.txt",
        include_bytes!("../../fixtures/cargo-source-map-probe/probe/marker.txt"),
    ),
    (
        "probe/src/lib.rs",
        include_bytes!("../../fixtures/cargo-source-map-probe/probe/src/lib.rs"),
    ),
    (
        "probe/tests/probe.rs",
        include_bytes!("../../fixtures/cargo-source-map-probe/probe/tests/probe.rs"),
    ),
    (
        "probe-macro/Cargo.toml",
        include_bytes!("../../fixtures/cargo-source-map-probe/probe-macro/Cargo.toml"),
    ),
    (
        "probe-macro/src/lib.rs",
        include_bytes!("../../fixtures/cargo-source-map-probe/probe-macro/src/lib.rs"),
    ),
];

type Result<T> = std::result::Result<T, AdapterRefusal>;
fn unknown<T>(_: T) -> AdapterRefusal {
    AdapterRefusal::InputUnknown
}
fn hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub(super) fn selected(workflow: &WorkflowOptIn) -> bool {
    matches!(
        workflow.path_identity,
        PathIdentity::SameRootCargoFixture { .. }
    )
}

pub(super) fn validate_workflow(
    invocation: &ProjectInvocation,
    workflow: &WorkflowOptIn,
) -> Result<()> {
    if workflow.workflow != WORKFLOW
        || workflow.selector != SELECTOR
        || workflow.profile != "test"
        || !workflow.features.is_empty()
        || invocation.inputs.cargo.version != "cargo 1.97.1 (c980f4866 2026-06-30)"
        || invocation.inputs.rustc.version != "rustc 1.97.1 (8bab26f4f 2026-07-14)"
        || invocation.inputs.effective_rustflags.as_deref() != Some(&[])
        || !invocation.inputs.full_debug
        || invocation.inputs.host != invocation.inputs.target
    {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    // No user-config overrides, build-script overrides, wrappers or new backend.
    // The local driver resolves configuration and pins the actual executables.
    if !matches!(&invocation.inputs.cargo_configuration, EffectiveInputSet::Complete(values) if values.is_empty())
    {
        return Err(AdapterRefusal::InputUnknown);
    }
    let EffectiveInputSet::Complete(values) = &invocation.inputs.environment else {
        return Err(AdapterRefusal::InputUnknown);
    };
    let allowed = [
        "CARGO_HOME",
        "HOME",
        "PATH",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
        "CARGO_BUILD_JOBS",
    ];
    if values
        .iter()
        .any(|value| !allowed.contains(&value.name.as_str()))
    {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    if !values
        .iter()
        .any(|value| value.name == "RUSTUP_TOOLCHAIN" && value.value.as_deref() == Some("1.97.1"))
    {
        return Err(AdapterRefusal::InputUnknown);
    }
    if values.len() != allowed.len()
        || allowed.iter().any(|key| {
            !values
                .iter()
                .any(|value| value.name == *key && value.value.is_some())
        })
        || !values
            .iter()
            .any(|value| value.name == "CARGO_BUILD_JOBS" && value.value.as_deref() == Some("1"))
    {
        return Err(AdapterRefusal::InputUnknown);
    }
    let root = fs::canonicalize(&invocation.descriptor.workspace_root).map_err(unknown)?;
    // This fixed workflow has no effective Cargo configuration. Refuse any
    // ancestor/user configuration rather than assuming caller omission is complete.
    for parent in root.ancestors() {
        for name in [".cargo/config", ".cargo/config.toml"] {
            require_absent(&parent.join(name))?;
        }
    }
    for key in ["CARGO_HOME", "HOME"] {
        let value = values
            .iter()
            .find(|value| value.name == key)
            .and_then(|value| value.value.as_deref())
            .ok_or(AdapterRefusal::InputUnknown)?;
        let base = PathBuf::from(value);
        if fs::canonicalize(&base).map_err(unknown)? != base || !base.is_dir() {
            return Err(AdapterRefusal::InputUnknown);
        }
        for name in [
            "config",
            "config.toml",
            ".cargo/config",
            ".cargo/config.toml",
        ] {
            require_absent(&base.join(name))?;
        }
    }
    Ok(())
}
fn require_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(AdapterRefusal::InputUnknown),
    }
}

/// A separate body/read budget, not a 1 MiB assumption about source contents.
#[derive(Default)]
struct ReadBudget(usize);
impl ReadBudget {
    fn file(&mut self, path: &Path) -> Result<Vec<u8>> {
        let metadata = fs::symlink_metadata(path).map_err(unknown)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_FILE_BYTES as u64
        {
            return Err(AdapterRefusal::InputUnknown);
        }
        let remaining = MAX_READ_BYTES
            .checked_sub(self.0)
            .ok_or(AdapterRefusal::InputUnknown)?;
        let limit = remaining.min(MAX_FILE_BYTES);
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(unknown)?
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(unknown)?;
        if bytes.len() > limit {
            return Err(AdapterRefusal::InputUnknown);
        }
        self.0 += bytes.len();
        Ok(bytes)
    }
}

fn relative(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path.as_os_str().len() > MAX_PATH_BYTES
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(AdapterRefusal::InputUnknown);
    }
    Ok(path.to_path_buf())
}

fn owned_file(root: &Path, name: &str) -> Result<PathBuf> {
    let relative = relative(name)?;
    let mut path = root.to_path_buf();
    for part in relative.components() {
        path.push(part.as_os_str());
        let metadata = fs::symlink_metadata(&path).map_err(unknown)?;
        if metadata.file_type().is_symlink() {
            return Err(AdapterRefusal::InputUnknown);
        }
    }
    let canonical = fs::canonicalize(&path).map_err(unknown)?;
    if canonical != path || !canonical.starts_with(root) {
        return Err(AdapterRefusal::InputUnknown);
    }
    Ok(path)
}

/// Bounded walk: directories, symlinks and special files never disappear silently.
fn walk(root: &Path, dir: &Path, paths: &mut BTreeSet<String>, visits: &mut usize) -> Result<()> {
    for entry in fs::read_dir(dir).map_err(unknown)? {
        *visits += 1;
        if *visits > MAX_ENTRIES {
            return Err(AdapterRefusal::InputUnknown);
        }
        let path = entry.map_err(unknown)?.path();
        let metadata = fs::symlink_metadata(&path).map_err(unknown)?;
        if metadata.file_type().is_symlink() {
            return Err(AdapterRefusal::InputUnknown);
        }
        if metadata.is_dir() {
            walk(root, &path, paths, visits)?;
        } else if metadata.is_file() {
            let name = path
                .strip_prefix(root)
                .map_err(unknown)?
                .to_str()
                .ok_or(AdapterRefusal::InputUnknown)?
                .to_owned();
            relative(&name)?;
            if !paths.insert(name) {
                return Err(AdapterRefusal::InputUnknown);
            }
        } else {
            return Err(AdapterRefusal::InputUnknown);
        }
    }
    Ok(())
}

/// Recomputed before acquire and on closeout; fixture membership is exact.
pub(super) fn requested_tree(root: &Path) -> Result<String> {
    if fs::canonicalize(root).map_err(unknown)? != root {
        return Err(AdapterRefusal::InputUnknown);
    }
    let mut paths = BTreeSet::new();
    walk(root, root, &mut paths, &mut 0)?;
    if paths != FILES.iter().map(|(name, _)| (*name).to_owned()).collect() {
        return Err(AdapterRefusal::InputUnknown);
    }
    let mut budget = ReadBudget::default();
    let mut digest = Sha256::new();
    // Preserve the frozen fixture producer's component-wise Path ordering;
    // string ordering puts `probe-macro` before `probe/...` and changes T0/T1.
    let mut paths: Vec<_> = paths.into_iter().collect();
    paths.sort_by(|left, right| Path::new(left).cmp(Path::new(right)));
    for name in paths {
        let bytes = budget.file(&owned_file(root, &name)?)?;
        let expected = FILES
            .iter()
            .find(|(file, _)| *file == name)
            .ok_or(AdapterRefusal::InputUnknown)?
            .1;
        let eligible = match name.as_str() {
            "probe/marker.txt" => bytes == b"baseline-marker\n" || bytes == b"changed-marker\n",
            "probe/src/lib.rs" => {
                bytes == expected || bytes == [expected, FAILURE_OVERLAY].concat()
            }
            _ => bytes == expected,
        };
        if !eligible {
            return Err(AdapterRefusal::InputUnknown);
        }
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(&bytes);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Reference {
    path: String,
    digest: String,
    mtime: u64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Input {
    slot: bool,
    path: String,
    digest: String,
    references: BTreeSet<String>,
}
#[derive(Debug)]
pub(super) struct InputMap {
    root: String,
    partition: String,
    generation: String,
    tree: String,
    references: BTreeMap<String, Reference>,
    inputs: BTreeMap<(bool, String), Input>,
}

fn mtime(path: &Path) -> Result<u64> {
    fs::metadata(path)
        .map_err(unknown)?
        .modified()
        .map_err(unknown)?
        .duration_since(UNIX_EPOCH)
        .map_err(unknown)?
        .as_nanos()
        .try_into()
        .map_err(unknown)
}

impl InputMap {
    fn identity(
        root: &Path,
        partition: &PartitionKey,
        generation: &str,
        tree: String,
    ) -> Result<Self> {
        Ok(Self {
            root: root
                .to_str()
                .ok_or(AdapterRefusal::InputUnknown)?
                .to_owned(),
            partition: super::partition_digest(partition),
            generation: generation.to_owned(),
            tree,
            references: BTreeMap::new(),
            inputs: BTreeMap::new(),
        })
    }
    fn reference(&mut self, slot: &Path, name: &str, budget: &mut ReadBudget) -> Result<()> {
        let path = owned_file(slot, name)?;
        let entry = Reference {
            path: name.to_owned(),
            digest: hash(&budget.file(&path)?),
            mtime: mtime(&path)?,
        };
        if self.references.insert(name.to_owned(), entry).is_some() {
            return Err(AdapterRefusal::InputUnknown);
        }
        Ok(())
    }
    fn input(
        &mut self,
        root: &Path,
        slot: &Path,
        path: &Path,
        reference: &str,
        budget: &mut ReadBudget,
    ) -> Result<()> {
        let canonical = fs::canonicalize(path).map_err(unknown)?;
        let (is_slot, base) = if canonical.starts_with(root) {
            (false, root)
        } else if canonical.starts_with(slot) {
            (true, slot)
        } else {
            return Err(AdapterRefusal::InputUnknown);
        };
        let name = canonical
            .strip_prefix(base)
            .map_err(unknown)?
            .to_str()
            .ok_or(AdapterRefusal::InputUnknown)?
            .to_owned();
        if owned_file(base, &name)? != path {
            return Err(AdapterRefusal::InputUnknown);
        }
        if !self.references.contains_key(reference) {
            return Err(AdapterRefusal::InputUnknown);
        }
        let key = (is_slot, name.clone());
        if let Some(entry) = self.inputs.get_mut(&key) {
            entry.references.insert(reference.to_owned());
        } else {
            if self.inputs.len() >= MAX_ENTRIES {
                return Err(AdapterRefusal::InputUnknown);
            }
            let digest = hash(&budget.file(path)?);
            self.inputs.insert(
                key,
                Input {
                    slot: is_slot,
                    path: name,
                    digest,
                    references: BTreeSet::from([reference.to_owned()]),
                },
            );
        }
        Ok(())
    }
    fn encode(&self) -> Result<Vec<u8>> {
        if self.inputs.len() + self.references.len() > MAX_ENTRIES {
            return Err(AdapterRefusal::InputUnknown);
        }
        let mut writer = Writer::new();
        for value in [
            "oxide-freshness-map-v1",
            &self.root,
            &self.partition,
            &self.generation,
            &self.tree,
        ] {
            writer.string(value).map_err(unknown)?;
        }
        writer.u32(self.references.len() as u32);
        for entry in self.references.values() {
            writer.string(&entry.path).map_err(unknown)?;
            writer.string(&entry.digest).map_err(unknown)?;
            writer.u64(entry.mtime);
        }
        writer.u32(self.inputs.len() as u32);
        for entry in self.inputs.values() {
            writer.u8(u8::from(entry.slot));
            writer.string(&entry.path).map_err(unknown)?;
            writer.string(&entry.digest).map_err(unknown)?;
            writer.u32(entry.references.len() as u32);
            for reference in &entry.references {
                writer.string(reference).map_err(unknown)?;
            }
        }
        let mut bytes = writer.finish();
        let digest = Sha256::digest(&bytes);
        bytes.extend_from_slice(&digest);
        if bytes.len() > MAX_MAP_BYTES {
            return Err(AdapterRefusal::InputUnknown);
        }
        Ok(bytes)
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_MAP_BYTES || bytes.len() < 32 {
            return Err(AdapterRefusal::InputUnknown);
        }
        let (body, checksum) = bytes.split_at(bytes.len() - 32);
        if Sha256::digest(body).as_slice() != checksum {
            return Err(AdapterRefusal::InputUnknown);
        }
        let mut reader = Reader::new(body);
        if reader.string().map_err(unknown)? != "oxide-freshness-map-v1" {
            return Err(AdapterRefusal::InputUnknown);
        }
        let root = reader.string().map_err(unknown)?;
        let partition = reader.string().map_err(unknown)?;
        let generation = reader.string().map_err(unknown)?;
        let tree = reader.string().map_err(unknown)?;
        if !super::valid_digest(&tree) {
            return Err(AdapterRefusal::InputUnknown);
        }
        let mut references = BTreeMap::new();
        let count = reader.count().map_err(unknown)?;
        if count == 0 || count > MAX_UNITS * 3 {
            return Err(AdapterRefusal::InputUnknown);
        }
        for _ in 0..count {
            let path = reader.string().map_err(unknown)?;
            relative(&path)?;
            let digest = reader.string().map_err(unknown)?;
            let mtime = reader.u64().map_err(unknown)?;
            if references
                .insert(
                    path.clone(),
                    Reference {
                        path,
                        digest,
                        mtime,
                    },
                )
                .is_some()
            {
                return Err(AdapterRefusal::InputUnknown);
            }
        }
        let count = reader.count().map_err(unknown)?;
        if count == 0 || count + references.len() > MAX_ENTRIES {
            return Err(AdapterRefusal::InputUnknown);
        }
        let mut inputs = BTreeMap::new();
        for _ in 0..count {
            let slot = match reader.u8().map_err(unknown)? {
                0 => false,
                1 => true,
                _ => return Err(AdapterRefusal::InputUnknown),
            };
            let path = reader.string().map_err(unknown)?;
            relative(&path)?;
            let digest = reader.string().map_err(unknown)?;
            let count = reader.count().map_err(unknown)?;
            if count == 0 || count > MAX_UNITS * 3 {
                return Err(AdapterRefusal::InputUnknown);
            }
            let mut refs = BTreeSet::new();
            for _ in 0..count {
                let name = reader.string().map_err(unknown)?;
                if !references.contains_key(&name) || !refs.insert(name) {
                    return Err(AdapterRefusal::InputUnknown);
                }
            }
            let entry = Input {
                slot,
                path: path.clone(),
                digest,
                references: refs,
            };
            if inputs.insert((slot, path), entry).is_some() {
                return Err(AdapterRefusal::InputUnknown);
            }
        }
        if !reader.done() {
            return Err(AdapterRefusal::InputUnknown);
        }
        Ok(Self {
            root,
            partition,
            generation,
            tree,
            references,
            inputs,
        })
    }
}

/// Cold means both a new lease generation and actual emptiness, not a missing map.
pub(super) fn admit(
    root: &Path,
    slot: &Path,
    partition: &PartitionKey,
    generation: &str,
    reused: bool,
    tree: &str,
) -> Result<()> {
    if requested_tree(root)? != tree {
        return Err(AdapterRefusal::SourceStale);
    }
    if !reused {
        if fs::read_dir(slot).map_err(unknown)?.next().is_some() {
            return Err(AdapterRefusal::InputUnknown);
        }
        return Ok(());
    }
    if fs::symlink_metadata(slot.join(BARRIER)).is_ok()
        || fs::symlink_metadata(slot.join(TEMP)).is_ok()
    {
        return Err(AdapterRefusal::InputUnknown);
    }
    let mut budget = ReadBudget::default();
    let map = InputMap::decode(&budget.file(&owned_file(slot, MAP)?)?)?;
    if map.root != root.to_str().ok_or(AdapterRefusal::InputUnknown)?
        || map.partition != super::partition_digest(partition)
        || map.generation != generation
    {
        return Err(AdapterRefusal::InputUnknown);
    }
    // Re-discovery also verifies unit identity/reference hashes and rejects extra
    // units and changed generated/input path sets. Never trust a caller's map.
    let actual = collect(root, slot, partition, generation, map.tree.clone())?;
    if actual.references != map.references || actual.inputs.keys().ne(map.inputs.keys()) {
        return Err(AdapterRefusal::InputUnknown);
    }
    for (key, input) in &map.inputs {
        let current = actual.inputs.get(key).ok_or(AdapterRefusal::InputUnknown)?;
        let base = if input.slot { slot } else { root };
        let timestamp = mtime(&owned_file(base, &input.path)?)?;
        if input.references != current.references {
            return Err(AdapterRefusal::InputUnknown);
        }
        for reference in &input.references {
            let reference = map
                .references
                .get(reference)
                .ok_or(AdapterRefusal::InputUnknown)?;
            if timestamp <= reference.mtime && input.digest != current.digest {
                return Err(AdapterRefusal::SourceStale);
            }
        }
    }
    Ok(())
}

/// Barrier is committed before any untrusted runner is called. Any error keeps
/// eligibility invalid; a later reconcile-to-Idle cannot remove this marker.
pub(super) fn begin(slot: &Path, lease: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(slot.join(BARRIER))
        .map_err(unknown)?;
    file.write_all(lease.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(unknown)?;
    sync_directory(slot)
}
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(unknown)
}

pub(super) fn prepare(plan: &FixtureExecutionPlan, partition: &PartitionKey) -> Result<()> {
    if requested_tree(&plan.command.workspace_root)? != plan.claim.binding.source_tree_digest {
        return Err(AdapterRefusal::SourceStale);
    }
    let map = collect(
        &plan.command.workspace_root,
        &plan.claim.slot_path,
        partition,
        &plan.claim.slot_id,
        plan.claim.binding.source_tree_digest.clone(),
    )?;
    let bytes = map.encode()?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(plan.claim.slot_path.join(TEMP))
        .map_err(unknown)?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(unknown)?;
    fs::rename(
        plan.claim.slot_path.join(TEMP),
        plan.claim.slot_path.join(MAP),
    )
    .map_err(unknown)?;
    sync_directory(&plan.claim.slot_path)
}

/// Only the adapter's verified live Cargo completion/capture and confirmed Idle
/// finalization call this. Until then even a complete candidate is unusable.
pub(super) fn publish(plan: &FixtureExecutionPlan) -> Result<()> {
    let mut budget = ReadBudget::default();
    if budget.file(&owned_file(&plan.claim.slot_path, BARRIER)?)? != plan.claim.lease_id.as_bytes()
        || requested_tree(&plan.command.workspace_root)? != plan.claim.binding.source_tree_digest
    {
        return Err(AdapterRefusal::InputUnknown);
    }
    fs::remove_file(plan.claim.slot_path.join(BARRIER)).map_err(unknown)?;
    sync_directory(&plan.claim.slot_path)
}

/// Inspection only. Consumer-owned recovery policy is separate from acquire.
#[derive(Debug, Eq, PartialEq)]
pub enum RecoveryDecision {
    ReplaceOwnedGeneration,
    Refuse,
}
/// Inspect prerequisites only. Caller-supplied policy facts are not ownership
/// proof or permission to delete; runtime destructive recovery is disabled.
pub fn recovery(
    state: SlotState,
    known_owned_generation: bool,
    invalid_map: bool,
    grant: bool,
    capacity: bool,
) -> RecoveryDecision {
    if state == SlotState::Idle && known_owned_generation && invalid_map && grant && capacity {
        RecoveryDecision::ReplaceOwnedGeneration
    } else {
        RecoveryDecision::Refuse
    }
}

/// Small strict JSON reader for Cargo's pinned fingerprint shape. Duplicate
/// fields, escapes outside the tiny fixture vocabulary, nesting/size overflow,
/// unexpected tokens and trailing data refuse. No permissive substring lookup.
#[derive(Debug)]
enum Json {
    String(String),
    Number(u64),
    Bool(bool),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}
impl Json {
    fn object(&self) -> Result<&BTreeMap<String, Json>> {
        if let Self::Object(value) = self {
            Ok(value)
        } else {
            Err(AdapterRefusal::InputUnknown)
        }
    }
    fn array(&self) -> Result<&[Json]> {
        if let Self::Array(value) = self {
            Ok(value)
        } else {
            Err(AdapterRefusal::InputUnknown)
        }
    }
    fn string(&self) -> Result<&str> {
        if let Self::String(value) = self {
            Ok(value)
        } else {
            Err(AdapterRefusal::InputUnknown)
        }
    }
    fn number(&self) -> Result<u64> {
        if let Self::Number(value) = self {
            Ok(*value)
        } else {
            Err(AdapterRefusal::InputUnknown)
        }
    }
}
struct JsonReader<'a> {
    bytes: &'a [u8],
    offset: usize,
    tokens: usize,
}
impl<'a> JsonReader<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Json> {
        let mut reader = Self {
            bytes,
            offset: 0,
            tokens: 0,
        };
        let value = reader.value(0)?;
        reader.space();
        if reader.offset != bytes.len() {
            return Err(AdapterRefusal::InputUnknown);
        }
        Ok(value)
    }
    fn space(&mut self) {
        while self
            .bytes
            .get(self.offset)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.offset += 1;
        }
    }
    fn expect(&mut self, byte: u8) -> Result<()> {
        self.space();
        if self.bytes.get(self.offset) != Some(&byte) {
            return Err(AdapterRefusal::InputUnknown);
        }
        self.offset += 1;
        Ok(())
    }
    fn text(&mut self) -> Result<String> {
        self.expect(b'"')?;
        let mut value = Vec::new();
        loop {
            let byte = *self
                .bytes
                .get(self.offset)
                .ok_or(AdapterRefusal::InputUnknown)?;
            self.offset += 1;
            match byte {
                b'"' => break,
                b'\\' => {
                    let byte = *self
                        .bytes
                        .get(self.offset)
                        .ok_or(AdapterRefusal::InputUnknown)?;
                    self.offset += 1;
                    match byte {
                        b'"' | b'\\' | b'/' => value.push(byte),
                        _ => return Err(AdapterRefusal::InputUnknown),
                    }
                }
                0..=31 => return Err(AdapterRefusal::InputUnknown),
                _ => value.push(byte),
            }
            if value.len() > MAX_PATH_BYTES {
                return Err(AdapterRefusal::InputUnknown);
            }
        }
        String::from_utf8(value).map_err(unknown)
    }
    fn value(&mut self, depth: usize) -> Result<Json> {
        self.tokens += 1;
        self.space();
        if depth > 8 || self.tokens > MAX_ENTRIES {
            return Err(AdapterRefusal::InputUnknown);
        }
        match self
            .bytes
            .get(self.offset)
            .copied()
            .ok_or(AdapterRefusal::InputUnknown)?
        {
            b'"' => Ok(Json::String(self.text()?)),
            b'{' => {
                self.offset += 1;
                self.space();
                let mut object = BTreeMap::new();
                if self.bytes.get(self.offset) == Some(&b'}') {
                    self.offset += 1;
                    return Ok(Json::Object(object));
                }
                loop {
                    let key = self.text()?;
                    self.expect(b':')?;
                    if object.insert(key, self.value(depth + 1)?).is_some() {
                        return Err(AdapterRefusal::InputUnknown);
                    }
                    self.space();
                    if self.bytes.get(self.offset) == Some(&b'}') {
                        self.offset += 1;
                        break;
                    }
                    self.expect(b',')?;
                }
                Ok(Json::Object(object))
            }
            b'[' => {
                self.offset += 1;
                self.space();
                let mut array = Vec::new();
                if self.bytes.get(self.offset) == Some(&b']') {
                    self.offset += 1;
                    return Ok(Json::Array(array));
                }
                loop {
                    array.push(self.value(depth + 1)?);
                    self.space();
                    if self.bytes.get(self.offset) == Some(&b']') {
                        self.offset += 1;
                        break;
                    }
                    self.expect(b',')?;
                }
                Ok(Json::Array(array))
            }
            b't' if self.bytes.get(self.offset..self.offset + 4) == Some(b"true") => {
                self.offset += 4;
                Ok(Json::Bool(true))
            }
            b'f' if self.bytes.get(self.offset..self.offset + 5) == Some(b"false") => {
                self.offset += 5;
                Ok(Json::Bool(false))
            }
            b'0'..=b'9' => {
                let start = self.offset;
                while self.bytes.get(self.offset).is_some_and(u8::is_ascii_digit) {
                    self.offset += 1;
                }
                let text = std::str::from_utf8(
                    self.bytes
                        .get(start..self.offset)
                        .ok_or(AdapterRefusal::InputUnknown)?,
                )
                .map_err(unknown)?;
                if text.len() > 1 && text.starts_with('0') {
                    return Err(AdapterRefusal::InputUnknown);
                }
                Ok(Json::Number(text.parse().map_err(unknown)?))
            }
            _ => Err(AdapterRefusal::InputUnknown),
        }
    }
}
fn field<'a>(object: &'a BTreeMap<String, Json>, key: &str) -> Result<&'a Json> {
    object.get(key).ok_or(AdapterRefusal::InputUnknown)
}
fn exact_keys(object: &BTreeMap<String, Json>, keys: &[&str]) -> Result<()> {
    if object.len() != keys.len() || keys.iter().any(|key| !object.contains_key(*key)) {
        return Err(AdapterRefusal::InputUnknown);
    }
    Ok(())
}

struct DepInfo {
    files: Vec<PathBuf>,
}
fn dep_info(bytes: &[u8], package: &Path, slot: &Path) -> Result<DepInfo> {
    if bytes.len() > MAX_MAP_BYTES || !bytes.starts_with(&[1, 0, 0, 0, 255, 1]) {
        return Err(AdapterRefusal::InputUnknown);
    }
    let mut reader = Reader::new(bytes.get(6..).ok_or(AdapterRefusal::InputUnknown)?);
    let count = reader.count().map_err(unknown)?;
    if count == 0 || count > MAX_ENTRIES {
        return Err(AdapterRefusal::InputUnknown);
    }
    let mut files = BTreeSet::new();
    for _ in 0..count {
        let root = match reader.u8().map_err(unknown)? {
            0 => package,
            1 => slot,
            _ => return Err(AdapterRefusal::InputUnknown),
        };
        let name = reader.string().map_err(unknown)?;
        // Pinned Cargo permits absolute build-relative paths; only canonical,
        // declared paths inside the source root/slot survive input validation.
        let name = if Path::new(&name).is_absolute() {
            PathBuf::from(name)
        } else {
            root.join(relative(&name)?)
        };
        if reader.u8().map_err(unknown)? != 0 || !files.insert(name) {
            return Err(AdapterRefusal::InputUnknown);
        }
    }
    let count = reader.count().map_err(unknown)?;
    if count > MAX_ENTRIES {
        return Err(AdapterRefusal::InputUnknown);
    }
    let metadata = [
        "CARGO_PKG_DESCRIPTION",
        "CARGO_PKG_HOMEPAGE",
        "CARGO_PKG_REPOSITORY",
        "CARGO_PKG_LICENSE",
        "CARGO_PKG_LICENSE_FILE",
        "CARGO_PKG_AUTHORS",
        "CARGO_PKG_RUST_VERSION",
        "CARGO_PKG_README",
    ];
    let mut env = BTreeSet::new();
    for _ in 0..count {
        let key = reader.string().map_err(unknown)?;
        if !metadata.contains(&key.as_str()) || !env.insert(key) {
            return Err(AdapterRefusal::InputUnknown);
        }
        match reader.u8().map_err(unknown)? {
            0 => {}
            1 => {
                reader.string().map_err(unknown)?;
            }
            _ => return Err(AdapterRefusal::InputUnknown),
        }
    }
    if !reader.done() {
        return Err(AdapterRefusal::InputUnknown);
    }
    Ok(DepInfo {
        files: files.into_iter().collect(),
    })
}

fn collect(
    root: &Path,
    slot: &Path,
    partition: &PartitionKey,
    generation: &str,
    tree: String,
) -> Result<InputMap> {
    let mut map = InputMap::identity(root, partition, generation, tree)?;
    let fingerprints = slot.join("debug/.fingerprint");
    if fs::canonicalize(&fingerprints).map_err(unknown)? != fingerprints {
        return Err(AdapterRefusal::InputUnknown);
    }
    let mut budget = ReadBudget::default();
    let mut units = BTreeSet::new();
    let expected_units = BTreeSet::from([
        "lib-oxide_source_probe",
        "test-lib-oxide_source_probe",
        "test-integration-test-probe",
        "build-script-build-script-build",
        "run-build-script-build-script-build",
        "lib-oxide_source_probe_macro",
        "test-lib-oxide_source_probe_macro",
    ]);
    let mut generated_directory = None;
    let mut visits = 0;
    for entry in fs::read_dir(&fingerprints).map_err(unknown)? {
        visits += 1;
        if visits > MAX_UNITS {
            return Err(AdapterRefusal::InputUnknown);
        }
        let directory = entry.map_err(unknown)?.path();
        let name = directory
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(AdapterRefusal::InputUnknown)?;
        let (prefix, suffix) = name.rsplit_once('-').ok_or(AdapterRefusal::InputUnknown)?;
        if suffix.len() != 16 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AdapterRefusal::InputUnknown);
        }
        let package = match prefix {
            "oxide-source-probe" => root.join("probe"),
            "oxide-source-probe-macro" => root.join("probe-macro"),
            _ => return Err(AdapterRefusal::InputUnknown),
        };
        if fs::symlink_metadata(&directory)
            .map_err(unknown)?
            .file_type()
            .is_symlink()
            || !directory.is_dir()
        {
            return Err(AdapterRefusal::InputUnknown);
        }
        let mut found = false;
        let mut directory_entries = 0;
        for entry in fs::read_dir(&directory).map_err(unknown)? {
            directory_entries += 1;
            if directory_entries > 8 {
                return Err(AdapterRefusal::InputUnknown);
            }
            let path = entry.map_err(unknown)?.path();
            if fs::symlink_metadata(&path)
                .map_err(unknown)?
                .file_type()
                .is_symlink()
            {
                return Err(AdapterRefusal::InputUnknown);
            }
            if path.extension().and_then(|name| name.to_str()) != Some("json") {
                continue;
            }
            if found {
                return Err(AdapterRefusal::InputUnknown);
            }
            found = true;
            let unit = path
                .file_stem()
                .and_then(|name| name.to_str())
                .ok_or(AdapterRefusal::InputUnknown)?;
            if !expected_units.contains(unit)
                || !units.insert(unit.to_owned())
                || (prefix == "oxide-source-probe-macro")
                    != unit.ends_with("oxide_source_probe_macro")
            {
                return Err(AdapterRefusal::InputUnknown);
            }
            let json = JsonReader::parse(&budget.file(&path)?)?;
            let object = json.object()?;
            exact_keys(
                object,
                &[
                    "rustc",
                    "features",
                    "declared_features",
                    "target",
                    "profile",
                    "path",
                    "deps",
                    "local",
                    "rustflags",
                    "config",
                    "compile_kind",
                ],
            )?;
            for key in [
                "rustc",
                "target",
                "profile",
                "path",
                "config",
                "compile_kind",
            ] {
                field(object, key)?.number()?;
            }
            if field(object, "compile_kind")?.number()? != 0
                || !field(object, "rustflags")?.array()?.is_empty()
            {
                return Err(AdapterRefusal::InputUnknown);
            }
            for key in ["features", "declared_features"] {
                if !matches!(field(object, key)?.string()?, "[]" | "") {
                    return Err(AdapterRefusal::InputUnknown);
                }
            }
            for dep in field(object, "deps")?.array()? {
                let values = dep.array()?;
                if values.len() != 4 {
                    return Err(AdapterRefusal::InputUnknown);
                }
                values
                    .first()
                    .ok_or(AdapterRefusal::InputUnknown)?
                    .number()?;
                let name = values
                    .get(1)
                    .ok_or(AdapterRefusal::InputUnknown)?
                    .string()?;
                if ![
                    "oxide_source_probe",
                    "oxide_source_probe_macro",
                    "build_script_build",
                ]
                .contains(&name)
                    || !matches!(values.get(2), Some(Json::Bool(false)))
                {
                    return Err(AdapterRefusal::InputUnknown);
                }
                values
                    .get(3)
                    .ok_or(AdapterRefusal::InputUnknown)?
                    .number()?;
            }
            // Bind unit/dependency metadata and Cargo's short fingerprint bytes,
            // independently of the timestamp reference used by each input.
            for evidence in [path.clone(), directory.join(unit)] {
                let name = evidence
                    .strip_prefix(slot)
                    .map_err(unknown)?
                    .to_str()
                    .ok_or(AdapterRefusal::InputUnknown)?;
                map.reference(slot, name, &mut budget)?;
                map.references
                    .get_mut(name)
                    .ok_or(AdapterRefusal::InputUnknown)?
                    .mtime = 0;
            }
            let locals = field(object, "local")?.array()?;
            if locals.len() != 1 {
                return Err(AdapterRefusal::InputUnknown);
            }
            let local = locals
                .first()
                .ok_or(AdapterRefusal::InputUnknown)?
                .object()?;
            if unit == "run-build-script-build-script-build" {
                exact_keys(local, &["RerunIfChanged"])?;
                let rerun = field(local, "RerunIfChanged")?.object()?;
                exact_keys(rerun, &["output", "paths"])?;
                let output = field(rerun, "output")?.string()?;
                if output != format!("debug/build/{name}/output") {
                    return Err(AdapterRefusal::InputUnknown);
                }
                let paths = field(rerun, "paths")?.array()?;
                if paths.len() != 1
                    || paths
                        .first()
                        .ok_or(AdapterRefusal::InputUnknown)?
                        .string()?
                        != "marker.txt"
                {
                    return Err(AdapterRefusal::InputUnknown);
                }
                map.reference(slot, output, &mut budget)?;
                if budget.file(&owned_file(slot, output)?)?
                    != b"cargo:rerun-if-changed=marker.txt\n"
                {
                    return Err(AdapterRefusal::InputUnknown);
                }
                map.input(root, slot, &package.join("marker.txt"), output, &mut budget)?;
                generated_directory = Some((format!("debug/build/{name}/out"), output.to_owned()));
            } else {
                exact_keys(local, &["CheckDepInfo"])?;
                let info = field(local, "CheckDepInfo")?.object()?;
                exact_keys(info, &["dep_info", "checksum"])?;
                if !matches!(field(info, "checksum")?, Json::Bool(false)) {
                    return Err(AdapterRefusal::InputUnknown);
                }
                let reference = field(info, "dep_info")?.string()?;
                let expected = directory.join(format!("dep-{unit}"));
                if slot.join(relative(reference)?) != expected {
                    return Err(AdapterRefusal::InputUnknown);
                }
                map.reference(slot, reference, &mut budget)?;
                let info = dep_info(&budget.file(&owned_file(slot, reference)?)?, &package, slot)?;
                for path in info.files {
                    map.input(root, slot, &path, reference, &mut budget)?;
                }
            }
        }
        if !found {
            return Err(AdapterRefusal::InputUnknown);
        }
    }
    if units.iter().map(String::as_str).collect::<BTreeSet<_>>() != expected_units {
        return Err(AdapterRefusal::InputUnknown);
    }
    let (generated, script_reference) = generated_directory.ok_or(AdapterRefusal::InputUnknown)?;
    let references: Vec<String> = map
        .references
        .values()
        .filter(|entry| entry.mtime != 0)
        .map(|entry| entry.path.clone())
        .collect();
    // Full declared behavior boundary includes proc-macro's untracked read and
    // identity files. Maximum of all real per-unit references is conservative.
    for (name, _) in FILES {
        for reference in &references {
            map.input(root, slot, &root.join(name), reference, &mut budget)?;
        }
    }
    for name in [
        "generated.rs",
        "proc-macro-input.txt",
        "producer-observation.txt",
    ] {
        let path = owned_file(slot, &format!("{generated}/{name}"))?;
        for reference in &references {
            map.input(root, slot, &path, reference, &mut budget)?;
        }
    }
    // No unreported external/generated rustc paths may be discovered. Keep the
    // generated distinction even though all source inputs use a conservative max.
    if map.inputs.keys().any(|(is_slot, name)| {
        *is_slot
            && ![
                "generated.rs",
                "proc-macro-input.txt",
                "producer-observation.txt",
            ]
            .iter()
            .any(|file| name == &format!("{generated}/{file}"))
    }) {
        return Err(AdapterRefusal::InputUnknown);
    }
    if !map.references.contains_key(&script_reference) {
        return Err(AdapterRefusal::InputUnknown);
    }
    map.encode()?;
    Ok(map)
}

#[cfg(test)]
mod tests;
