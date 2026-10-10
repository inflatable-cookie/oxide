//! Explicit source identity and fail-closed portability evidence for the
//! experimental local adapter. This module does not qualify Cargo itself.

use super::AdapterRefusal;
use crate::storage::{PartitionKey, RunBinding};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

pub const SOURCE_MAP_VERSION: &str = "oxide-source-map-v1";
pub const SOURCE_SCAN_VERSION: &str = "oxide-source-scan-v1";
pub const ACCEPTED_LOGICAL_POLICY: &str = "b5ea13cd-4c03-4a18-8cc3-93baab1f2039:v2";
pub(super) const MAX_COMPILE_ORIGINS: usize = 32;
const MAX_SOURCE_MAP_BINARIES: usize = 128;
const MAX_SOURCE_MAP_CAPTURE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_SOURCE_MAP_OBSERVATIONS: usize = 256;
const MAX_SOURCE_MAP_REPORT_BYTES: usize = 64 * 1024;
const MAX_SOURCE_MAP_HOOK_BYTES: usize = 64 * 1024;

/// The path contract selected for one experimental qualification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourcePathMode {
    /// Project source references must retain their physical checkout path.
    StrictPhysical,
    /// Compiler names use the recorded, operator-accepted logical prefix.
    ExperimentalLogical {
        accepted_policy: String,
        prefix: PathBuf,
    },
}

/// One stable, explicitly authorized source root in the ordered map.
#[derive(Clone, Eq, PartialEq)]
pub struct AuthorizedSourceRoot {
    pub id: String,
    pub path: PathBuf,
}

impl fmt::Debug for AuthorizedSourceRoot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizedSourceRoot")
            .field("id", &self.id)
            .field("path", &"[redacted]")
            .finish()
    }
}

/// Evidence that changed source contents rebuilt units within one partition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceMissControl {
    pub before_tree_digest: String,
    pub after_tree_digest: String,
    pub partition_digest: String,
    pub rebuilt_units: u32,
}

/// Evidence that a changed configuration or toolchain input selected a new key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyMissControl {
    pub before_input_digest: String,
    pub after_input_digest: String,
    pub before_partition_digest: String,
    pub after_partition_digest: String,
}

/// The three required invalidation controls. Missing controls cannot authorize
/// a cross-root slot claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissControlEvidence {
    pub source: Option<SourceMissControl>,
    pub configuration: Option<KeyMissControl>,
    pub toolchain: Option<KeyMissControl>,
}

/// A finite source map and the explicit invalidation evidence required before
/// its partition can be shared.
#[derive(Clone, Eq, PartialEq)]
pub struct SourceMapPolicy {
    pub version: String,
    pub mode: SourcePathMode,
    /// Order is significant and is preserved in the remap flag vector.
    pub roots: Vec<AuthorizedSourceRoot>,
    pub miss_controls: Option<MissControlEvidence>,
}

impl fmt::Debug for SourceMapPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceMapPolicy")
            .field("version", &self.version)
            .field("mode", &self.mode)
            .field("root_count", &self.roots.len())
            .field("miss_controls_present", &self.miss_controls.is_some())
            .finish()
    }
}

/// Canonical mapping carried from resolution to the local host-run boundary.
#[derive(Clone, Eq, PartialEq)]
pub struct ResolvedSourceMap {
    mode: SourcePathMode,
    policy_digest: String,
    map_digest: String,
    build_identity_digest: String,
    roots: Vec<AuthorizedSourceRoot>,
    requested_root_id: String,
    requested_root: PathBuf,
    requested_tree_digest: String,
    logical_prefix: Option<PathBuf>,
    effective_rustflags: Vec<String>,
}

impl fmt::Debug for ResolvedSourceMap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedSourceMap")
            .field("mode", &self.mode)
            .field("policy_digest", &self.policy_digest)
            .field("map_digest", &self.map_digest)
            .field("build_identity_digest", &self.build_identity_digest)
            .field("root_count", &self.roots.len())
            .field("requested_root_id", &self.requested_root_id)
            .field("effective_rustflags", &"[redacted]")
            .finish()
    }
}

impl ResolvedSourceMap {
    pub fn policy_digest(&self) -> &str {
        &self.policy_digest
    }

    pub fn map_digest(&self) -> &str {
        &self.map_digest
    }

    pub(super) fn build_identity_digest(&self) -> &str {
        &self.build_identity_digest
    }

    pub(super) fn bind_build_identity(&mut self, partition: &PartitionKey) {
        let mut fields = vec![
            "oxide-source-map-build-identity-v1",
            partition.project_component.as_str(),
            partition.workflow.as_str(),
            partition.cargo_provenance.as_str(),
            partition.rustc_provenance.as_str(),
            partition.host.as_str(),
            partition.target.as_str(),
            partition.profile.as_str(),
            partition.config_digest.as_str(),
            partition.environment_digest.as_str(),
            partition.path_identity.as_str(),
        ];
        fields.extend(partition.features.iter().map(String::as_str));
        self.build_identity_digest = digest_fields(&fields);
    }

    pub fn requested_root_id(&self) -> &str {
        &self.requested_root_id
    }

    pub fn is_strict_physical(&self) -> bool {
        matches!(&self.mode, SourcePathMode::StrictPhysical)
    }

    pub fn requested_root(&self) -> &Path {
        &self.requested_root
    }

    pub fn authorized_roots(&self) -> &[AuthorizedSourceRoot] {
        &self.roots
    }

    pub fn requested_tree_digest(&self) -> &str {
        &self.requested_tree_digest
    }

    pub fn logical_prefix(&self) -> Option<&Path> {
        self.logical_prefix.as_deref()
    }

    pub fn effective_rustflags(&self) -> &[String] {
        &self.effective_rustflags
    }

    pub fn root_path(&self, id: &str) -> Option<&Path> {
        self.roots
            .iter()
            .find(|root| root.id == id)
            .map(|root| root.path.as_path())
    }
}

pub(super) fn resolve(
    policy: &SourceMapPolicy,
    disposable_root: &Path,
    workspace_root: &Path,
    requested_tree_digest: &str,
    effective_rustflags: Option<&[String]>,
    full_debug: bool,
) -> Result<ResolvedSourceMap, AdapterRefusal> {
    if policy.version != SOURCE_MAP_VERSION || !full_debug || !valid_digest(requested_tree_digest) {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    validate_miss_controls(policy.miss_controls.as_ref())?;
    let disposable_root = canonical_dir(disposable_root)?;
    let workspace_root = canonical_dir(workspace_root)?;
    if !workspace_root.starts_with(&disposable_root) {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    let mut roots = Vec::with_capacity(policy.roots.len());
    let mut ids = BTreeSet::new();
    for root in &policy.roots {
        if !valid_root_id(&root.id) || !ids.insert(root.id.as_str()) {
            return Err(AdapterRefusal::AmbiguousInput);
        }
        let path = canonical_dir(&root.path)?;
        if path == disposable_root || !path.starts_with(&disposable_root) {
            return Err(AdapterRefusal::UnsupportedInput);
        }
        if roots.iter().any(|existing: &AuthorizedSourceRoot| {
            existing.path.starts_with(&path) || path.starts_with(&existing.path)
        }) {
            return Err(AdapterRefusal::AmbiguousInput);
        }
        roots.push(AuthorizedSourceRoot {
            id: root.id.clone(),
            path,
        });
    }
    if roots.is_empty() {
        return Err(AdapterRefusal::InputUnknown);
    }
    let matching: Vec<_> = roots
        .iter()
        .filter(|root| workspace_root.starts_with(&root.path))
        .collect();
    let [requested] = matching.as_slice() else {
        return Err(if matching.is_empty() {
            AdapterRefusal::UnsupportedInput
        } else {
            AdapterRefusal::AmbiguousInput
        });
    };
    let mut effective_rustflags = effective_rustflags
        .ok_or(AdapterRefusal::InputUnknown)?
        .to_vec();
    for flag in &effective_rustflags {
        if flag.is_empty() || flag.len() > 16 * 1024 || flag.as_bytes().contains(&0) {
            return Err(AdapterRefusal::UnsupportedInput);
        }
    }
    if forbidden_flags(&effective_rustflags) {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    let logical_prefix = match &policy.mode {
        SourcePathMode::StrictPhysical => None,
        SourcePathMode::ExperimentalLogical {
            accepted_policy,
            prefix,
        } => {
            if accepted_policy != ACCEPTED_LOGICAL_POLICY || !valid_logical_prefix(prefix) {
                return Err(AdapterRefusal::UnsupportedInput);
            }
            let prefix = prefix.clone();
            for root in &roots {
                let from = root.path.to_str().ok_or(AdapterRefusal::UnsupportedInput)?;
                let to = prefix.to_str().ok_or(AdapterRefusal::UnsupportedInput)?;
                effective_rustflags.push(format!("--remap-path-prefix={from}={to}"));
            }
            Some(prefix)
        }
    };
    let policy_digest = digest_fields(&[
        "oxide-source-map-policy-v1",
        policy.version.as_str(),
        mode_name(&policy.mode),
        mode_ruling(&policy.mode),
        logical_prefix
            .as_ref()
            .and_then(|path| path.to_str())
            .unwrap_or(""),
    ]);
    let mut map_fields = vec![policy_digest.clone()];
    for root in &roots {
        map_fields.push(root.id.clone());
        map_fields.push(
            root.path
                .to_str()
                .ok_or(AdapterRefusal::UnsupportedInput)?
                .to_owned(),
        );
    }
    map_fields.extend(effective_rustflags.iter().cloned());
    let borrowed: Vec<_> = map_fields.iter().map(String::as_str).collect();
    let map_digest = digest_fields(&borrowed);
    let requested_root_id = requested.id.clone();
    let requested_root = requested.path.clone();
    let requested_tree_digest = requested_tree_digest.to_owned();
    Ok(ResolvedSourceMap {
        mode: policy.mode.clone(),
        policy_digest,
        map_digest,
        build_identity_digest: String::new(),
        roots,
        requested_root_id,
        requested_root,
        requested_tree_digest,
        logical_prefix,
        effective_rustflags,
    })
}

fn validate_miss_controls(controls: Option<&MissControlEvidence>) -> Result<(), AdapterRefusal> {
    let controls = controls.ok_or(AdapterRefusal::InputUnknown)?;
    let source = controls
        .source
        .as_ref()
        .ok_or(AdapterRefusal::InputUnknown)?;
    let config = controls
        .configuration
        .as_ref()
        .ok_or(AdapterRefusal::InputUnknown)?;
    let toolchain = controls
        .toolchain
        .as_ref()
        .ok_or(AdapterRefusal::InputUnknown)?;
    if source.before_tree_digest == source.after_tree_digest
        || source.rebuilt_units == 0
        || !valid_digest(&source.before_tree_digest)
        || !valid_digest(&source.after_tree_digest)
        || !valid_digest(&source.partition_digest)
        || !distinct_key_miss(config)
        || !distinct_key_miss(toolchain)
    {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    Ok(())
}

fn distinct_key_miss(control: &KeyMissControl) -> bool {
    control.before_input_digest != control.after_input_digest
        && control.before_partition_digest != control.after_partition_digest
        && valid_digest(&control.before_input_digest)
        && valid_digest(&control.after_input_digest)
        && valid_digest(&control.before_partition_digest)
        && valid_digest(&control.after_partition_digest)
}

fn forbidden_flags(flags: &[String]) -> bool {
    flags.iter().any(|flag| {
        matches!(
            flag.as_str(),
            "--remap-path-prefix" | "--remap-path-scope" | "--strip"
        ) || flag.starts_with("--remap-path-prefix=")
            || flag.starts_with("--remap-path-scope=")
            || flag.starts_with("--strip=")
            || flag.starts_with("-Cstrip")
            || flag.starts_with("-Cdebuginfo=0")
    }) || flags.windows(2).any(|pair| {
        pair[0] == "-C"
            && (pair[1] == "debuginfo=0" || pair[1].starts_with("strip=") || pair[1] == "strip")
    })
}

fn valid_logical_prefix(prefix: &Path) -> bool {
    prefix.is_absolute()
        && prefix
            .components()
            .any(|component| matches!(component, Component::Normal(_)))
        && prefix
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn valid_root_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && value != "."
        && value != ".."
}

fn mode_name(mode: &SourcePathMode) -> &'static str {
    match mode {
        SourcePathMode::StrictPhysical => "strict-physical",
        SourcePathMode::ExperimentalLogical { .. } => "experimental-logical",
    }
}

fn mode_ruling(mode: &SourcePathMode) -> &str {
    match mode {
        SourcePathMode::StrictPhysical => "explicit-strict-v1",
        SourcePathMode::ExperimentalLogical {
            accepted_policy, ..
        } => accepted_policy,
    }
}

fn canonical_dir(path: &Path) -> Result<PathBuf, AdapterRefusal> {
    let path = fs::canonicalize(path).map_err(|_| AdapterRefusal::UnsupportedInput)?;
    if !path.is_dir() {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    Ok(path)
}

fn valid_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

pub(super) fn disposable_root_identity(id: &str, root: &Path) -> Result<String, AdapterRefusal> {
    if !valid_root_id(id) {
        return Err(AdapterRefusal::AmbiguousInput);
    }
    let root = canonical_dir(root)?;
    let root = root.to_str().ok_or(AdapterRefusal::UnsupportedInput)?;
    Ok(digest_fields(&["disposable-root-v1", id, root]))
}

/// Compiler-source name categories are kept distinct from behavior-bearing
/// paths because rustc remapping does not normalize all filesystem use.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourcePathClass {
    CompilerDebug,
    FileMacro,
    PanicLocation,
    AppleOso,
}

/// Behavior paths that a portability run must exercise and resolve.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum BehaviorPathClass {
    ManifestDirectory,
    IncludeInput,
    BuildScriptInput,
    OutDirectory,
    GeneratedOutput,
    ProcMacroInput,
    RuntimeResource,
}

pub const REQUIRED_BEHAVIOR_PATHS: [BehaviorPathClass; 7] = [
    BehaviorPathClass::ManifestDirectory,
    BehaviorPathClass::IncludeInput,
    BehaviorPathClass::BuildScriptInput,
    BehaviorPathClass::OutDirectory,
    BehaviorPathClass::GeneratedOutput,
    BehaviorPathClass::ProcMacroInput,
    BehaviorPathClass::RuntimeResource,
];

#[derive(Clone, Eq, PartialEq)]
pub struct SourcePathObservation {
    pub class: SourcePathClass,
    pub observed: PathBuf,
    /// Exact file selected by the run-private logical or physical mapping.
    pub resolved: Option<PathBuf>,
}

impl fmt::Debug for SourcePathObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourcePathObservation")
            .field("class", &self.class)
            .field("observed", &"[redacted]")
            .field("resolved", &self.resolved.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct BehaviorPathObservation {
    pub class: BehaviorPathClass,
    pub resolved: Option<PathBuf>,
    pub source_tree_digest: Option<String>,
    pub generation_lease_id: Option<String>,
}

impl fmt::Debug for BehaviorPathObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BehaviorPathObservation")
            .field("class", &self.class)
            .field("resolved", &self.resolved.as_ref().map(|_| "[redacted]"))
            .field("source_tree_digest", &self.source_tree_digest)
            .field("generation_lease_id", &self.generation_lease_id)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinaryArtifactDigest {
    pub reference: String,
    pub digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinarySourceScan {
    pub reference: String,
    pub binary_digest: String,
    pub scanner_version: String,
    pub complete: bool,
    pub debug_sections: Vec<String>,
    pub source_paths: Vec<SourcePathObservation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileOriginIdentity {
    pub root_id: String,
    pub tree_digest: String,
}

/// Exact-run source mapping receipt. The compile-origin trees remain separate
/// from the requested tree; the outer capture digest binds this receipt to the
/// private run artifacts.
#[derive(Clone, Eq, PartialEq)]
pub struct SourceMapRunReceipt {
    pub policy_digest: String,
    pub map_digest: String,
    pub build_identity_digest: String,
    pub authorized_roots: Vec<AuthorizedSourceRoot>,
    pub requested_root_id: String,
    pub requested_root: PathBuf,
    pub logical_prefix: Option<PathBuf>,
    pub requested_source_tree_digest: String,
    pub compile_origins: Vec<CompileOriginIdentity>,
    pub slot_lease_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub host_run_id: String,
    pub slot_path: PathBuf,
    pub capture_dir: PathBuf,
    pub output_references: Vec<String>,
    pub binaries: Vec<BinaryArtifactDigest>,
    pub stdout_digest: String,
    pub stderr_digest: String,
    pub capture_digest: String,
}

impl fmt::Debug for SourceMapRunReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceMapRunReceipt")
            .field("policy_digest", &self.policy_digest)
            .field("map_digest", &self.map_digest)
            .field("build_identity_digest", &self.build_identity_digest)
            .field(
                "authorized_root_ids",
                &self
                    .authorized_roots
                    .iter()
                    .map(|root| root.id.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("requested_root_id", &self.requested_root_id)
            .field(
                "requested_source_tree_digest",
                &self.requested_source_tree_digest,
            )
            .field("compile_origins", &self.compile_origins)
            .field("slot_lease_id", &self.slot_lease_id)
            .field("run_id", &self.run_id)
            .field("attempt_id", &self.attempt_id)
            .field("host_run_id", &self.host_run_id)
            .field("slot_path", &"[redacted]")
            .field("capture_dir", &"[redacted]")
            .field("output_references", &self.output_references)
            .field("binaries", &self.binaries)
            .field("stdout_digest", &self.stdout_digest)
            .field("stderr_digest", &self.stderr_digest)
            .field("capture_digest", &self.capture_digest)
            .finish()
    }
}

impl SourceMapRunReceipt {
    pub fn receipt_digest(&self) -> String {
        let mut fields = vec![
            "oxide-source-map-run-v1".to_owned(),
            self.policy_digest.clone(),
            self.map_digest.clone(),
            self.build_identity_digest.clone(),
            self.requested_root_id.clone(),
            self.requested_source_tree_digest.clone(),
            self.slot_lease_id.clone(),
            self.run_id.clone(),
            self.attempt_id.clone(),
            self.host_run_id.clone(),
            self.stdout_digest.clone(),
            self.stderr_digest.clone(),
            self.capture_digest.clone(),
        ];
        for origin in &self.compile_origins {
            fields.push(origin.root_id.clone());
            fields.push(origin.tree_digest.clone());
        }
        for root in &self.authorized_roots {
            fields.push(root.id.clone());
            fields.push(digest_fields(&[root
                .path
                .to_str()
                .unwrap_or("invalid-path")]));
        }
        for binary in &self.binaries {
            fields.push(binary.reference.clone());
            fields.push(binary.digest.clone());
        }
        let borrowed: Vec<_> = fields.iter().map(String::as_str).collect();
        digest_fields(&borrowed)
    }

    pub fn binary_set_digest(&self) -> String {
        let fields: Vec<_> = self
            .binaries
            .iter()
            .flat_map(|binary| [binary.reference.as_str(), binary.digest.as_str()])
            .collect();
        digest_fields(&fields)
    }

    pub fn binds_run(
        &self,
        map: &ResolvedSourceMap,
        lease_id: &str,
        binding: &RunBinding,
        host_run_id: &str,
        capture_dir: &Path,
        capture_digest: &str,
    ) -> bool {
        self.policy_digest == map.policy_digest
            && self.map_digest == map.map_digest
            && self.build_identity_digest == map.build_identity_digest
            && self.authorized_roots == map.roots
            && self.requested_root_id == map.requested_root_id
            && self.requested_root == map.requested_root
            && self.requested_source_tree_digest == map.requested_tree_digest
            && self.logical_prefix == map.logical_prefix
            && self.requested_source_tree_digest == binding.source_tree_digest
            && self.slot_lease_id == lease_id
            && self.run_id == binding.run_id
            && self.attempt_id == binding.attempt_id
            && self.host_run_id == host_run_id
            && self.capture_dir == capture_dir
            && self.capture_digest == capture_digest
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortabilityEvidence {
    pub binary_scans: Option<Vec<BinarySourceScan>>,
    pub behavior_paths: Option<Vec<BehaviorPathObservation>>,
    pub apple_oso: Option<AppleOsoObservation>,
    pub(super) provenance: EvidenceProvenance,
    pub(super) missing_classes: Vec<MissingEvidenceClass>,
}

/// The supported Rust binary observation explicitly distinguishes an absent
/// Apple OSO entry from a missing scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppleOsoObservation {
    NotEmitted,
    Observed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EvidenceProvenance {
    RegisteredCargoWorkflow,
    TaskOwnedFixture,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MissingEvidenceClass {
    ObservationHook,
    CompileOrigins,
    BinaryArtifacts,
    BinaryScan,
    CompilerDebug,
    FileMacro,
    PanicLocation,
    AppleOsoScan,
    ManifestDirectory,
    IncludeInput,
    BuildScriptInput,
    OutDirectory,
    GeneratedOutput,
    ProcMacroInput,
    RuntimeResource,
}

impl MissingEvidenceClass {
    fn label(self) -> &'static str {
        match self {
            Self::ObservationHook => "observation-hook",
            Self::CompileOrigins => "compile-origins",
            Self::BinaryArtifacts => "binary-artifacts",
            Self::BinaryScan => "binary-scan",
            Self::CompilerDebug => "compiler-debug-path",
            Self::FileMacro => "file-macro-path",
            Self::PanicLocation => "panic-location-path",
            Self::AppleOsoScan => "apple-oso-emission-scan",
            Self::ManifestDirectory => "manifest-directory",
            Self::IncludeInput => "include-input",
            Self::BuildScriptInput => "build-script-input",
            Self::OutDirectory => "out-directory",
            Self::GeneratedOutput => "generated-output",
            Self::ProcMacroInput => "proc-macro-input",
            Self::RuntimeResource => "runtime-resource",
        }
    }
}

/// Raw observations supplied by the registered workflow collector or a
/// task-owned fixture. The collector verifies artifact and capture facts from
/// disk before it can build a qualification receipt.
pub(super) struct SourceMapObservations {
    pub provenance: EvidenceProvenance,
    pub hook_state: ObservationHookState,
    pub compile_origins: Vec<CompileOriginIdentity>,
    pub binary_artifacts: Vec<BinaryArtifactObservation>,
    pub binary_scans: Option<Vec<BinarySourceScan>>,
    pub behavior_paths: Option<Vec<BehaviorPathObservation>>,
    pub apple_oso: Option<AppleOsoObservation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ObservationHookState {
    Observed,
    Missing,
    Invalid,
}

pub(super) struct BinaryArtifactObservation {
    pub reference: String,
    pub path: PathBuf,
}

/// Reads the optional workflow hook from the invocation-private Cargo target
/// directory. The hook names observations; the collector later resolves and
/// hashes the referenced files itself.
pub(super) fn read_registered_cargo_hook(
    provenance: EvidenceProvenance,
    map: &ResolvedSourceMap,
    lease_id: &str,
    binding: &RunBinding,
    host_run_id: &str,
    slot_path: &Path,
    target_dir: &Path,
    capture_dir: &Path,
    output_references: &mut Vec<String>,
) -> SourceMapObservations {
    let hook_path = target_dir.join("source-map-observations-v1.txt");
    let invalid = || SourceMapObservations {
        provenance,
        hook_state: ObservationHookState::Invalid,
        compile_origins: Vec::new(),
        binary_artifacts: Vec::new(),
        binary_scans: None,
        behavior_paths: None,
        apple_oso: None,
    };
    let metadata = match fs::symlink_metadata(&hook_path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => metadata,
        Ok(_) => return invalid(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return SourceMapObservations {
                provenance,
                hook_state: ObservationHookState::Missing,
                compile_origins: Vec::new(),
                binary_artifacts: Vec::new(),
                binary_scans: None,
                behavior_paths: None,
                apple_oso: None,
            };
        }
        Err(_) => return invalid(),
    };
    if metadata.len() > MAX_SOURCE_MAP_HOOK_BYTES as u64
        || !fs::canonicalize(&hook_path).is_ok_and(|canonical| canonical == hook_path)
    {
        return invalid();
    }
    let hook_reference = "source-map-observations.txt";
    let capture_hook = capture_dir.join(hook_reference);
    if output_references
        .iter()
        .any(|reference| reference == hook_reference)
        || copy_new_capture_file(&hook_path, &capture_hook, metadata.len()).is_err()
    {
        return invalid();
    }
    output_references.push(hook_reference.to_owned());
    let bytes = match fs::read(&capture_hook) {
        Ok(bytes) if bytes.len() <= MAX_SOURCE_MAP_HOOK_BYTES => bytes,
        _ => return invalid(),
    };
    let Some(observations) = parse_registered_cargo_hook(
        map,
        lease_id,
        binding,
        host_run_id,
        slot_path,
        target_dir,
        &bytes,
        provenance,
    ) else {
        return invalid();
    };
    observations
}

fn parse_registered_cargo_hook(
    map: &ResolvedSourceMap,
    lease_id: &str,
    binding: &RunBinding,
    host_run_id: &str,
    slot_path: &Path,
    target_dir: &Path,
    bytes: &[u8],
    provenance: EvidenceProvenance,
) -> Option<SourceMapObservations> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    if lines.next()? != "oxide-cargo-source-map-observations-v1" {
        return None;
    }
    let mut identity_seen = false;
    let mut origins = Vec::new();
    let mut origin_ids = BTreeSet::new();
    let mut artifacts = BTreeMap::new();
    let mut scans = BTreeMap::new();
    let mut behavior_paths = Vec::new();
    let mut apple_oso = None;
    for (index, line) in lines.enumerate() {
        if index >= MAX_SOURCE_MAP_OBSERVATIONS || line.is_empty() || line.contains('\r') {
            return None;
        }
        let fields: Vec<_> = line.split('\t').collect();
        match fields.as_slice() {
            ["identity", policy, map_digest, build_identity, tree, lease, run, attempt, host_run]
                if !identity_seen
                    && *policy == map.policy_digest
                    && *map_digest == map.map_digest
                    && *build_identity == map.build_identity_digest
                    && *tree == map.requested_tree_digest
                    && *lease == lease_id
                    && *run == binding.run_id
                    && *attempt == binding.attempt_id
                    && *host_run == host_run_id =>
            {
                identity_seen = true;
            }
            ["origin", root_id, tree_digest]
                if valid_root_id(root_id)
                    && valid_digest(tree_digest)
                    && map.roots.iter().any(|root| root.id == *root_id)
                    && origin_ids.insert(*root_id) =>
            {
                origins.push(CompileOriginIdentity {
                    root_id: (*root_id).to_owned(),
                    tree_digest: (*tree_digest).to_owned(),
                });
            }
            ["artifact", reference, root_kind, relative]
                if safe_reference(reference) && safe_reference(relative) =>
            {
                let root = match *root_kind {
                    "slot" => slot_path,
                    "target" => target_dir,
                    _ => return None,
                };
                let path = root.join(relative);
                let canonical = fs::canonicalize(&path).ok()?;
                if canonical != path || !canonical.is_file() || !canonical.starts_with(root) {
                    return None;
                }
                if artifacts
                    .insert(
                        (*reference).to_owned(),
                        BinaryArtifactObservation {
                            reference: (*reference).to_owned(),
                            path,
                        },
                    )
                    .is_some()
                {
                    return None;
                }
            }
            ["scan", reference, scanner, digest]
                if safe_reference(reference) && valid_digest(digest) =>
            {
                let scan = BinarySourceScan {
                    reference: (*reference).to_owned(),
                    binary_digest: (*digest).to_owned(),
                    scanner_version: (*scanner).to_owned(),
                    complete: false,
                    debug_sections: Vec::new(),
                    source_paths: Vec::new(),
                };
                if scans.insert((*reference).to_owned(), scan).is_some() {
                    return None;
                }
            }
            ["section", reference, section] => {
                scans
                    .get_mut(*reference)?
                    .debug_sections
                    .push((*section).to_owned());
            }
            ["source", reference, class, observed] => {
                let class = parse_source_path_class(class)?;
                let observed = PathBuf::from(observed);
                let resolved = resolve_observed_source_path(map, &observed);
                scans
                    .get_mut(*reference)?
                    .source_paths
                    .push(SourcePathObservation {
                        class,
                        observed,
                        resolved,
                    });
            }
            ["behavior", class, path, tree, generation] if valid_digest(tree) => {
                behavior_paths.push(BehaviorPathObservation {
                    class: parse_behavior_path_class(class)?,
                    resolved: Some(PathBuf::from(path)),
                    source_tree_digest: Some((*tree).to_owned()),
                    generation_lease_id: if *generation == "-" {
                        None
                    } else {
                        Some((*generation).to_owned())
                    },
                });
            }
            ["apple-oso", "not-emitted"] if apple_oso.is_none() => {
                apple_oso = Some(AppleOsoObservation::NotEmitted);
            }
            ["apple-oso", "observed"] if apple_oso.is_none() => {
                apple_oso = Some(AppleOsoObservation::Observed);
            }
            _ => return None,
        }
    }
    if !identity_seen
        || origins.len() > MAX_COMPILE_ORIGINS
        || artifacts.len() > MAX_SOURCE_MAP_BINARIES
        || scans.len() != artifacts.len()
        || scans
            .keys()
            .any(|reference| !artifacts.contains_key(reference))
    {
        return None;
    }
    for scan in scans.values_mut() {
        scan.complete = [".debug_info", ".debug_line", ".debug_abbrev"]
            .iter()
            .all(|required| {
                scan.debug_sections
                    .iter()
                    .any(|section| section == required)
            });
    }
    Some(SourceMapObservations {
        provenance,
        hook_state: ObservationHookState::Observed,
        compile_origins: origins,
        binary_artifacts: artifacts.into_values().collect(),
        binary_scans: Some(scans.into_values().collect()),
        behavior_paths: Some(behavior_paths),
        apple_oso,
    })
}

fn parse_source_path_class(value: &str) -> Option<SourcePathClass> {
    match value {
        "compiler-debug" => Some(SourcePathClass::CompilerDebug),
        "file-macro" => Some(SourcePathClass::FileMacro),
        "panic-location" => Some(SourcePathClass::PanicLocation),
        "apple-oso" => Some(SourcePathClass::AppleOso),
        _ => None,
    }
}

fn parse_behavior_path_class(value: &str) -> Option<BehaviorPathClass> {
    match value {
        "manifest-directory" => Some(BehaviorPathClass::ManifestDirectory),
        "include-input" => Some(BehaviorPathClass::IncludeInput),
        "build-script-input" => Some(BehaviorPathClass::BuildScriptInput),
        "out-directory" => Some(BehaviorPathClass::OutDirectory),
        "generated-output" => Some(BehaviorPathClass::GeneratedOutput),
        "proc-macro-input" => Some(BehaviorPathClass::ProcMacroInput),
        "runtime-resource" => Some(BehaviorPathClass::RuntimeResource),
        _ => None,
    }
}

fn resolve_observed_source_path(map: &ResolvedSourceMap, observed: &Path) -> Option<PathBuf> {
    if let Some(prefix) = &map.logical_prefix {
        let relative = observed.strip_prefix(prefix).ok()?;
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        {
            return None;
        }
        Some(map.requested_root.join(relative))
    } else {
        Some(observed.to_path_buf())
    }
}

pub(super) struct CollectedSourceMapEvidence {
    pub receipt: SourceMapRunReceipt,
    pub evidence: PortabilityEvidence,
}

/// Builds an exact-run receipt from observed capture files. It does not accept
/// a qualification verdict: missing classes are recorded and the qualifier
/// still checks every observation and path afterward.
pub(super) fn collect(
    map: &ResolvedSourceMap,
    lease_id: &str,
    binding: &RunBinding,
    host_run_id: &str,
    slot_path: &Path,
    target_dir: &Path,
    capture_dir: &Path,
    output_references: &mut Vec<String>,
    mut observations: SourceMapObservations,
) -> Result<CollectedSourceMapEvidence, AdapterRefusal> {
    if map.build_identity_digest.is_empty()
        || output_references.len() > MAX_SOURCE_MAP_BINARIES + 8
        || !output_references
            .iter()
            .any(|reference| reference == "stdout.log")
        || !output_references
            .iter()
            .any(|reference| reference == "stderr.log")
    {
        return Err(AdapterRefusal::InputUnknown);
    }
    let canonical_capture_dir = canonical_dir(capture_dir)?;
    if canonical_capture_dir != capture_dir {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    let capture_dir = canonical_capture_dir.as_path();
    let observation_count = observations
        .compile_origins
        .len()
        .saturating_add(
            observations
                .binary_scans
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|scan| scan.source_paths.len().saturating_add(1))
                .sum::<usize>(),
        )
        .saturating_add(
            observations
                .behavior_paths
                .as_deref()
                .unwrap_or_default()
                .len(),
        );
    if observation_count > MAX_SOURCE_MAP_OBSERVATIONS
        || observations.compile_origins.len() > MAX_COMPILE_ORIGINS
        || observations
            .behavior_paths
            .as_deref()
            .is_some_and(|paths| paths.len() > MAX_SOURCE_MAP_OBSERVATIONS)
    {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    let binary_references = copy_binary_artifacts(
        slot_path,
        target_dir,
        capture_dir,
        output_references,
        &observations.binary_artifacts,
    )?;
    if let Some(scans) = observations.binary_scans.as_mut() {
        for scan in scans {
            if let Some(reference) = binary_references.get(&scan.reference) {
                scan.reference = reference.clone();
            }
        }
    }
    let binaries = collect_binary_digests(capture_dir, &output_references, &observations)?;
    let stdout_digest = digest_captured_file(capture_dir, "stdout.log")?;
    let stderr_digest = digest_captured_file(capture_dir, "stderr.log")?;
    let missing_classes = missing_evidence_classes(&observations, binaries.is_empty());
    let report = evidence_report(
        map,
        lease_id,
        binding,
        host_run_id,
        &output_references,
        &binaries,
        &stdout_digest,
        &stderr_digest,
        &observations,
        &missing_classes,
    );
    if report.len() > MAX_SOURCE_MAP_REPORT_BYTES {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    let report_reference = "source-map-evidence.txt";
    let report_path = capture_dir.join(report_reference);
    if output_references
        .iter()
        .any(|reference| reference == report_reference)
    {
        return Err(AdapterRefusal::AmbiguousInput);
    }
    write_new_capture_file(&report_path, report.as_bytes())
        .map_err(|_| AdapterRefusal::InputUnknown)?;
    output_references.push(report_reference.to_owned());
    let output_references = unique_safe_references(output_references)?;
    if !output_references
        .iter()
        .any(|reference| reference == report_reference)
    {
        return Err(AdapterRefusal::InputUnknown);
    }
    let capture_digest =
        crate::project::native::capture_receipt_digest(capture_dir, &output_references)
            .map_err(|_| AdapterRefusal::InputUnknown)?;
    let receipt = SourceMapRunReceipt {
        policy_digest: map.policy_digest.clone(),
        map_digest: map.map_digest.clone(),
        build_identity_digest: map.build_identity_digest.clone(),
        authorized_roots: map.roots.clone(),
        requested_root_id: map.requested_root_id.clone(),
        requested_root: map.requested_root.clone(),
        logical_prefix: map.logical_prefix.clone(),
        requested_source_tree_digest: map.requested_tree_digest.clone(),
        compile_origins: std::mem::take(&mut observations.compile_origins),
        slot_lease_id: lease_id.to_owned(),
        run_id: binding.run_id.clone(),
        attempt_id: binding.attempt_id.clone(),
        host_run_id: host_run_id.to_owned(),
        slot_path: slot_path.to_path_buf(),
        capture_dir: capture_dir.to_path_buf(),
        output_references,
        binaries,
        stdout_digest,
        stderr_digest,
        capture_digest,
    };
    Ok(CollectedSourceMapEvidence {
        receipt,
        evidence: PortabilityEvidence {
            binary_scans: observations.binary_scans,
            behavior_paths: observations.behavior_paths,
            apple_oso: observations.apple_oso,
            provenance: observations.provenance,
            missing_classes,
        },
    })
}

fn collect_binary_digests(
    capture_dir: &Path,
    output_references: &[String],
    observations: &SourceMapObservations,
) -> Result<Vec<BinaryArtifactDigest>, AdapterRefusal> {
    let scans = observations.binary_scans.as_deref().unwrap_or_default();
    if scans.len() > MAX_SOURCE_MAP_BINARIES {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    let mut binaries = Vec::with_capacity(scans.len());
    let mut total_bytes = 0_u64;
    let mut seen = BTreeSet::new();
    for scan in scans {
        if !seen.insert(scan.reference.as_str())
            || !output_references
                .iter()
                .any(|reference| reference == &scan.reference)
        {
            return Err(AdapterRefusal::AmbiguousInput);
        }
        let (digest, bytes) = digest_captured_file_with_size(capture_dir, &scan.reference)?;
        total_bytes = total_bytes
            .checked_add(bytes)
            .ok_or(AdapterRefusal::UnsupportedInput)?;
        if total_bytes > MAX_SOURCE_MAP_CAPTURE_BYTES {
            return Err(AdapterRefusal::UnsupportedInput);
        }
        binaries.push(BinaryArtifactDigest {
            reference: scan.reference.clone(),
            digest,
        });
    }
    Ok(binaries)
}

fn copy_binary_artifacts(
    slot_path: &Path,
    target_dir: &Path,
    capture_dir: &Path,
    output_references: &mut Vec<String>,
    artifacts: &[BinaryArtifactObservation],
) -> Result<BTreeMap<String, String>, AdapterRefusal> {
    if artifacts.len() > MAX_SOURCE_MAP_BINARIES {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    let canonical_slot_path = canonical_dir(slot_path)?;
    let canonical_target_dir = canonical_dir(target_dir)?;
    if canonical_slot_path != slot_path || canonical_target_dir != target_dir {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    let mut references = BTreeMap::new();
    let mut total_bytes = 0_u64;
    for (index, artifact) in artifacts.iter().enumerate() {
        if !safe_reference(&artifact.reference) || references.contains_key(&artifact.reference) {
            return Err(AdapterRefusal::AmbiguousInput);
        }
        let canonical =
            fs::canonicalize(&artifact.path).map_err(|_| AdapterRefusal::InputUnknown)?;
        if canonical != artifact.path
            || !canonical.is_file()
            || (!canonical.starts_with(&canonical_slot_path)
                && !canonical.starts_with(&canonical_target_dir))
        {
            return Err(AdapterRefusal::UnsupportedInput);
        }
        let length = fs::metadata(&canonical)
            .map_err(|_| AdapterRefusal::InputUnknown)?
            .len();
        total_bytes = total_bytes
            .checked_add(length)
            .ok_or(AdapterRefusal::UnsupportedInput)?;
        if length > MAX_SOURCE_MAP_CAPTURE_BYTES || total_bytes > MAX_SOURCE_MAP_CAPTURE_BYTES {
            return Err(AdapterRefusal::UnsupportedInput);
        }
        let reference = format!("binary-{index}.bin");
        if output_references
            .iter()
            .any(|existing| existing == &reference)
        {
            return Err(AdapterRefusal::AmbiguousInput);
        }
        copy_new_capture_file(&canonical, &capture_dir.join(&reference), length)
            .map_err(|_| AdapterRefusal::InputUnknown)?;
        output_references.push(reference.clone());
        references.insert(artifact.reference.clone(), reference);
    }
    Ok(references)
}

fn unique_safe_references(references: &[String]) -> Result<Vec<String>, AdapterRefusal> {
    let mut seen = BTreeSet::new();
    for reference in references {
        if !safe_reference(reference) || !seen.insert(reference.as_str()) {
            return Err(AdapterRefusal::AmbiguousInput);
        }
    }
    Ok(references.to_vec())
}

fn safe_reference(reference: &str) -> bool {
    let path = Path::new(reference);
    !reference.is_empty()
        && reference.len() <= 256
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn digest_captured_file(capture_dir: &Path, reference: &str) -> Result<String, AdapterRefusal> {
    digest_captured_file_with_size(capture_dir, reference).map(|(digest, _)| digest)
}

fn digest_captured_file_with_size(
    capture_dir: &Path,
    reference: &str,
) -> Result<(String, u64), AdapterRefusal> {
    if !safe_reference(reference) {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    let path = capture_dir.join(reference);
    let canonical = fs::canonicalize(&path).map_err(|_| AdapterRefusal::InputUnknown)?;
    if canonical != path || !canonical.starts_with(capture_dir) || !canonical.is_file() {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    let metadata = fs::metadata(&canonical).map_err(|_| AdapterRefusal::InputUnknown)?;
    let bytes = metadata.len();
    if bytes > MAX_SOURCE_MAP_CAPTURE_BYTES {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    let digest = digest_file(&canonical).map_err(|_| AdapterRefusal::InputUnknown)?;
    Ok((digest, bytes))
}

fn missing_evidence_classes(
    observations: &SourceMapObservations,
    no_binaries: bool,
) -> Vec<MissingEvidenceClass> {
    let mut missing = Vec::new();
    if observations.hook_state != ObservationHookState::Observed {
        missing.push(MissingEvidenceClass::ObservationHook);
    }
    if observations.compile_origins.is_empty() {
        missing.push(MissingEvidenceClass::CompileOrigins);
    }
    if no_binaries {
        missing.push(MissingEvidenceClass::BinaryArtifacts);
    }
    let scans = observations.binary_scans.as_deref().unwrap_or_default();
    if observations.binary_scans.is_none()
        || scans.is_empty()
        || scans.iter().any(|scan| !scan.complete)
    {
        missing.push(MissingEvidenceClass::BinaryScan);
    }
    for (class, missing_class) in [
        (
            SourcePathClass::CompilerDebug,
            MissingEvidenceClass::CompilerDebug,
        ),
        (SourcePathClass::FileMacro, MissingEvidenceClass::FileMacro),
        (
            SourcePathClass::PanicLocation,
            MissingEvidenceClass::PanicLocation,
        ),
    ] {
        if !scans
            .iter()
            .any(|scan| scan.source_paths.iter().any(|path| path.class == class))
        {
            missing.push(missing_class);
        }
    }
    if observations.apple_oso.is_none() {
        missing.push(MissingEvidenceClass::AppleOsoScan);
    }
    for (class, missing_class) in [
        (
            BehaviorPathClass::ManifestDirectory,
            MissingEvidenceClass::ManifestDirectory,
        ),
        (
            BehaviorPathClass::IncludeInput,
            MissingEvidenceClass::IncludeInput,
        ),
        (
            BehaviorPathClass::BuildScriptInput,
            MissingEvidenceClass::BuildScriptInput,
        ),
        (
            BehaviorPathClass::OutDirectory,
            MissingEvidenceClass::OutDirectory,
        ),
        (
            BehaviorPathClass::GeneratedOutput,
            MissingEvidenceClass::GeneratedOutput,
        ),
        (
            BehaviorPathClass::ProcMacroInput,
            MissingEvidenceClass::ProcMacroInput,
        ),
        (
            BehaviorPathClass::RuntimeResource,
            MissingEvidenceClass::RuntimeResource,
        ),
    ] {
        if !observations
            .behavior_paths
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|path| path.class == class)
        {
            missing.push(missing_class);
        }
    }
    missing
}

fn evidence_report(
    map: &ResolvedSourceMap,
    lease_id: &str,
    binding: &RunBinding,
    host_run_id: &str,
    output_references: &[String],
    binaries: &[BinaryArtifactDigest],
    stdout_digest: &str,
    stderr_digest: &str,
    observations: &SourceMapObservations,
    missing: &[MissingEvidenceClass],
) -> String {
    let provenance = match observations.provenance {
        EvidenceProvenance::RegisteredCargoWorkflow => "registered-cargo-workflow",
        EvidenceProvenance::TaskOwnedFixture => "task-owned-fixture",
    };
    let hook_state = match observations.hook_state {
        ObservationHookState::Observed => "observed",
        ObservationHookState::Missing => "missing",
        ObservationHookState::Invalid => "invalid",
    };
    let missing = if missing.is_empty() {
        "none".to_owned()
    } else {
        missing
            .iter()
            .map(|class| class.label())
            .collect::<Vec<_>>()
            .join(",")
    };
    let mut report = format!(
        "oxide-source-map-evidence-v1\nprovenance={provenance}\nhook={hook_state}\npolicy={}\nmap={}\nbuild_identity={}\nrequested_root_id={}\nrequested_tree={}\nlease={}\nrun={}\nattempt={}\nhost_run={}\nstdout={}\nstderr={}\nmissing={missing}\n",
        map.policy_digest,
        map.map_digest,
        map.build_identity_digest,
        encode_report_field(&map.requested_root_id),
        map.requested_tree_digest,
        encode_report_field(lease_id),
        encode_report_field(&binding.run_id),
        encode_report_field(&binding.attempt_id),
        encode_report_field(host_run_id),
        stdout_digest,
        stderr_digest,
    );
    if let Some(prefix) = &map.logical_prefix {
        report.push_str(&format!("logical_prefix={}\n", encode_path(prefix)));
    }
    for root in &map.roots {
        report.push_str(&format!(
            "authorized_root={}|{}\n",
            encode_report_field(&root.id),
            encode_path(&root.path)
        ));
    }
    for reference in output_references {
        report.push_str(&format!(
            "capture_reference={}\n",
            encode_report_field(reference)
        ));
    }
    for binary in binaries {
        report.push_str(&format!(
            "binary={}|{}\n",
            encode_report_field(&binary.reference),
            binary.digest
        ));
    }
    for origin in &observations.compile_origins {
        report.push_str(&format!(
            "compile_origin={}|{}\n",
            encode_report_field(&origin.root_id),
            origin.tree_digest
        ));
    }
    for scan in observations.binary_scans.as_deref().unwrap_or_default() {
        report.push_str(&format!(
            "binary_scan={}|{}|{}\n",
            encode_report_field(&scan.reference),
            encode_report_field(&scan.scanner_version),
            if scan.complete {
                "complete"
            } else {
                "incomplete"
            }
        ));
        for section in &scan.debug_sections {
            report.push_str(&format!(
                "debug_section={}|{}\n",
                encode_report_field(&scan.reference),
                encode_report_field(section)
            ));
        }
        for path in &scan.source_paths {
            report.push_str(&format!(
                "source_path={}|{}|{}|{}\n",
                encode_report_field(&scan.reference),
                source_path_class_name(path.class),
                encode_path(&path.observed),
                path.resolved
                    .as_deref()
                    .map(encode_path)
                    .unwrap_or_else(|| "unresolved".to_owned())
            ));
        }
    }
    for path in observations.behavior_paths.as_deref().unwrap_or_default() {
        report.push_str(&format!(
            "behavior_path={}|{}|{}|{}\n",
            behavior_path_class_name(path.class),
            path.resolved
                .as_deref()
                .map(encode_path)
                .unwrap_or_else(|| "unresolved".to_owned()),
            path.source_tree_digest.as_deref().unwrap_or("unknown"),
            path.generation_lease_id
                .as_deref()
                .map(encode_report_field)
                .unwrap_or_else(|| "none".to_owned())
        ));
    }
    let oso = match &observations.apple_oso {
        Some(AppleOsoObservation::NotEmitted) => "not-emitted",
        Some(AppleOsoObservation::Observed) => "observed",
        None => "not-scanned",
    };
    report.push_str(&format!("apple_oso={oso}\n"));
    report
}

fn encode_report_field(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('\t', "%09")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn encode_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('%', "%25")
        .replace('\t', "%09")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn source_path_class_name(class: SourcePathClass) -> &'static str {
    match class {
        SourcePathClass::CompilerDebug => "compiler-debug",
        SourcePathClass::FileMacro => "file-macro",
        SourcePathClass::PanicLocation => "panic-location",
        SourcePathClass::AppleOso => "apple-oso",
    }
}

fn behavior_path_class_name(class: BehaviorPathClass) -> &'static str {
    match class {
        BehaviorPathClass::ManifestDirectory => "manifest-directory",
        BehaviorPathClass::IncludeInput => "include-input",
        BehaviorPathClass::BuildScriptInput => "build-script-input",
        BehaviorPathClass::OutDirectory => "out-directory",
        BehaviorPathClass::GeneratedOutput => "generated-output",
        BehaviorPathClass::ProcMacroInput => "proc-macro-input",
        BehaviorPathClass::RuntimeResource => "runtime-resource",
    }
}

/// Qualification stops at the first concrete mismatch or missing proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QualificationDisposition {
    Pass,
    Fail,
    Insufficient,
}

pub(super) fn qualify(
    map: &ResolvedSourceMap,
    receipt: &SourceMapRunReceipt,
    evidence: &PortabilityEvidence,
) -> QualificationDisposition {
    let mut insufficient = !evidence.missing_classes.is_empty();
    if !valid_digest(&receipt.requested_source_tree_digest)
        || receipt.compile_origins.len() > MAX_COMPILE_ORIGINS
        || receipt
            .compile_origins
            .iter()
            .any(|origin| !valid_digest(&origin.tree_digest))
        || receipt.slot_lease_id.is_empty()
        || receipt.run_id.is_empty()
        || receipt.attempt_id.is_empty()
        || receipt.host_run_id.is_empty()
        || !map
            .roots
            .iter()
            .any(|root| root.id == receipt.requested_root_id)
    {
        return QualificationDisposition::Insufficient;
    }
    if receipt.compile_origins.is_empty() {
        insufficient = true;
    }
    for origin in &receipt.compile_origins {
        if !map.roots.iter().any(|root| root.id == origin.root_id) {
            return QualificationDisposition::Fail;
        }
    }
    if receipt.policy_digest != map.policy_digest
        || receipt.map_digest != map.map_digest
        || receipt.build_identity_digest != map.build_identity_digest
        || receipt.authorized_roots != map.roots
        || receipt.requested_root_id != map.requested_root_id
        || receipt.requested_root != map.requested_root
        || receipt.requested_source_tree_digest != map.requested_tree_digest
        || receipt.logical_prefix != map.logical_prefix
        || !valid_digest(&receipt.capture_digest)
    {
        return QualificationDisposition::Fail;
    }
    let capture = match verify_capture(receipt) {
        Some(value) => value,
        None => return QualificationDisposition::Fail,
    };
    if capture != receipt.capture_digest {
        return QualificationDisposition::Fail;
    }
    let mut source_paths = Vec::new();
    let mut scanned_references = BTreeSet::new();
    let binary_scans = evidence.binary_scans.as_deref().unwrap_or_default();
    if evidence.binary_scans.is_none() || receipt.binaries.is_empty() {
        insufficient = true;
    }
    if binary_scans.len() > receipt.binaries.len() {
        return QualificationDisposition::Fail;
    }
    for scan in binary_scans {
        if !scanned_references.insert(scan.reference.as_str())
            || !receipt
                .binaries
                .iter()
                .any(|binary| binary.reference == scan.reference)
        {
            return QualificationDisposition::Fail;
        }
    }
    for binary in &receipt.binaries {
        let Some(scan) = binary_scans
            .iter()
            .find(|scan| scan.reference == binary.reference)
        else {
            insufficient = true;
            continue;
        };
        if scan.binary_digest != binary.digest || scan.scanner_version != SOURCE_SCAN_VERSION {
            return QualificationDisposition::Fail;
        }
        if !scan.complete {
            insufficient = true;
        }
        if ![".debug_info", ".debug_line", ".debug_abbrev"]
            .iter()
            .all(|required| {
                scan.debug_sections
                    .iter()
                    .any(|section| section == required)
            })
        {
            return QualificationDisposition::Fail;
        }
        source_paths.extend(scan.source_paths.iter());
    }
    for required in [
        SourcePathClass::CompilerDebug,
        SourcePathClass::FileMacro,
        SourcePathClass::PanicLocation,
    ] {
        if !source_paths.iter().any(|path| path.class == required) {
            insufficient = true;
        }
    }
    for observation in &source_paths {
        let Some(resolved) = observation.resolved.as_ref() else {
            insufficient = true;
            continue;
        };
        if !source_path_matches(map, receipt, observation, resolved) {
            return QualificationDisposition::Fail;
        }
    }
    let behavior_paths = evidence.behavior_paths.as_deref().unwrap_or_default();
    if evidence.behavior_paths.is_none() {
        insufficient = true;
    }
    for required in REQUIRED_BEHAVIOR_PATHS {
        if !behavior_paths.iter().any(|path| path.class == required) {
            insufficient = true;
        }
    }
    for observation in behavior_paths {
        if !behavior_path_matches(receipt, observation) {
            return QualificationDisposition::Fail;
        }
    }
    let oso_paths = source_paths
        .iter()
        .any(|path| path.class == SourcePathClass::AppleOso);
    match &evidence.apple_oso {
        Some(AppleOsoObservation::NotEmitted) if oso_paths => {
            return QualificationDisposition::Fail;
        }
        Some(AppleOsoObservation::Observed) if !oso_paths => insufficient = true,
        None => insufficient = true,
        _ => {}
    }
    if evidence.provenance == EvidenceProvenance::TaskOwnedFixture && !cfg!(test) {
        return QualificationDisposition::Insufficient;
    }
    if insufficient {
        QualificationDisposition::Insufficient
    } else {
        QualificationDisposition::Pass
    }
}

fn source_path_matches(
    map: &ResolvedSourceMap,
    receipt: &SourceMapRunReceipt,
    observation: &SourcePathObservation,
    resolved: &Path,
) -> bool {
    let requested_root = &receipt.requested_root;
    let logical = map.logical_prefix.as_ref();
    if let Some(prefix) = logical {
        if !observation.observed.starts_with(prefix) {
            return false;
        }
        let Ok(relative) = observation.observed.strip_prefix(prefix) else {
            return false;
        };
        return safe_file_under(requested_root, resolved)
            && resolved == &requested_root.join(relative);
    }
    if !observation.observed.starts_with(requested_root)
        || resolved != &observation.observed
        || !safe_file_under(requested_root, resolved)
    {
        return false;
    }
    receipt
        .compile_origins
        .iter()
        .all(|origin| origin.root_id == receipt.requested_root_id)
}

fn behavior_path_matches(
    receipt: &SourceMapRunReceipt,
    observation: &BehaviorPathObservation,
) -> bool {
    let Some(path) = observation.resolved.as_ref() else {
        return false;
    };
    let Some(tree) = observation.source_tree_digest.as_ref() else {
        return false;
    };
    if tree != &receipt.requested_source_tree_digest {
        return false;
    }
    match observation.class {
        BehaviorPathClass::OutDirectory
        | BehaviorPathClass::GeneratedOutput
        | BehaviorPathClass::ProcMacroInput => {
            path.starts_with(&receipt.slot_path)
                && fs::canonicalize(path).is_ok_and(|canonical| {
                    canonical == *path && canonical.starts_with(&receipt.slot_path)
                })
                && observation.generation_lease_id.as_deref()
                    == Some(receipt.slot_lease_id.as_str())
        }
        BehaviorPathClass::ManifestDirectory => {
            path.starts_with(&receipt.requested_root)
                && safe_directory_under(&receipt.requested_root, path)
                && observation.generation_lease_id.is_none()
        }
        BehaviorPathClass::IncludeInput
        | BehaviorPathClass::BuildScriptInput
        | BehaviorPathClass::RuntimeResource => {
            path.starts_with(&receipt.requested_root)
                && safe_file_under(&receipt.requested_root, path)
                && observation.generation_lease_id.is_none()
        }
    }
}

fn verify_capture(receipt: &SourceMapRunReceipt) -> Option<String> {
    if receipt.output_references.is_empty()
        || !receipt
            .output_references
            .iter()
            .any(|reference| reference == "stdout.log")
        || !receipt
            .output_references
            .iter()
            .any(|reference| reference == "stderr.log")
    {
        return None;
    }
    let capture_dir = fs::canonicalize(&receipt.capture_dir).ok()?;
    if capture_dir != receipt.capture_dir {
        return None;
    }
    let mut references = BTreeSet::new();
    for reference in &receipt.output_references {
        let path = Path::new(reference);
        if path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            || !references.insert(reference.as_str())
        {
            return None;
        }
        let path = capture_dir.join(reference);
        let canonical = fs::canonicalize(&path).ok()?;
        if canonical != path || !canonical.is_file() || !canonical.starts_with(&capture_dir) {
            return None;
        }
    }
    for binary in &receipt.binaries {
        if !references.contains(binary.reference.as_str()) || !valid_digest(&binary.digest) {
            return None;
        }
        let path = capture_dir.join(&binary.reference);
        let canonical = fs::canonicalize(&path).ok()?;
        if canonical != path
            || !canonical.is_file()
            || digest_file(&canonical).ok()? != binary.digest
        {
            return None;
        }
    }
    if digest_file(&capture_dir.join("stdout.log")).ok()? != receipt.stdout_digest
        || digest_file(&capture_dir.join("stderr.log")).ok()? != receipt.stderr_digest
    {
        return None;
    }
    crate::project::native::capture_receipt_digest(&capture_dir, &receipt.output_references).ok()
}

fn safe_file_under(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
        && path.is_file()
        && fs::canonicalize(path)
            .is_ok_and(|canonical| canonical == path && canonical.starts_with(root))
}

fn safe_directory_under(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
        && path.is_dir()
        && fs::canonicalize(path)
            .is_ok_and(|canonical| canonical == path && canonical.starts_with(root))
}

fn digest_file(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn copy_new_capture_file(
    source_path: &Path,
    destination_path: &Path,
    expected_length: u64,
) -> std::io::Result<()> {
    let source = fs::File::open(source_path)?;
    let mut destination = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination_path)?;
    let result = (|| {
        let copied = std::io::copy(
            &mut source.take(expected_length.saturating_add(1)),
            &mut destination,
        )?;
        if copied != expected_length {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "capture source changed while being copied",
            ));
        }
        destination.flush()
    })();
    if result.is_err() {
        drop(destination);
        let _ = fs::remove_file(destination_path);
    }
    result
}

fn write_new_capture_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let result = file.write_all(bytes).and_then(|()| file.flush());
    if result.is_err() {
        drop(file);
        let _ = fs::remove_file(path);
    }
    result
}

pub(super) fn digest_fields(fields: &[&str]) -> String {
    let mut digest = Sha256::new();
    for field in fields {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field.as_bytes());
    }
    format!("sha256:{:x}", digest.finalize())
}
