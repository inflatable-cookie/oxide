//! Explicit source identity and fail-closed portability evidence for the
//! experimental local adapter. This module does not qualify Cargo itself.

use super::AdapterRefusal;
use crate::storage::RunBinding;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const SOURCE_MAP_VERSION: &str = "oxide-source-map-v1";
pub const SOURCE_SCAN_VERSION: &str = "oxide-source-scan-v1";
pub const ACCEPTED_LOGICAL_POLICY: &str = "b5ea13cd-4c03-4a18-8cc3-93baab1f2039:v2";
pub(super) const MAX_COMPILE_ORIGINS: usize = 32;

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
    pub capture_digest: String,
}

impl fmt::Debug for SourceMapRunReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceMapRunReceipt")
            .field("policy_digest", &self.policy_digest)
            .field("map_digest", &self.map_digest)
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
            self.requested_root_id.clone(),
            self.requested_source_tree_digest.clone(),
            self.slot_lease_id.clone(),
            self.run_id.clone(),
            self.attempt_id.clone(),
            self.host_run_id.clone(),
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
    if !valid_digest(&receipt.requested_source_tree_digest)
        || receipt.compile_origins.is_empty()
        || receipt.compile_origins.len() > MAX_COMPILE_ORIGINS
        || receipt.compile_origins.iter().any(|origin| {
            !valid_digest(&origin.tree_digest)
                || !map.roots.iter().any(|root| root.id == origin.root_id)
        })
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
    if receipt.policy_digest != map.policy_digest
        || receipt.map_digest != map.map_digest
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
    let Some(binary_scans) = evidence.binary_scans.as_ref() else {
        return QualificationDisposition::Insufficient;
    };
    if binary_scans.len() != receipt.binaries.len() {
        return QualificationDisposition::Fail;
    }
    let mut source_paths = Vec::new();
    let mut scanned_references = BTreeSet::new();
    for binary in &receipt.binaries {
        let Some(scan) = binary_scans
            .iter()
            .find(|scan| scan.reference == binary.reference)
        else {
            return QualificationDisposition::Insufficient;
        };
        if !scanned_references.insert(scan.reference.as_str())
            || scan.binary_digest != binary.digest
            || scan.scanner_version != SOURCE_SCAN_VERSION
        {
            return QualificationDisposition::Fail;
        }
        if !scan.complete {
            return QualificationDisposition::Insufficient;
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
    for scan in binary_scans {
        if !receipt
            .binaries
            .iter()
            .any(|binary| binary.reference == scan.reference)
        {
            return QualificationDisposition::Fail;
        }
    }
    for required in [
        SourcePathClass::CompilerDebug,
        SourcePathClass::FileMacro,
        SourcePathClass::PanicLocation,
    ] {
        if !source_paths.iter().any(|path| path.class == required) {
            return QualificationDisposition::Insufficient;
        }
    }
    for observation in source_paths {
        let Some(resolved) = observation.resolved.as_ref() else {
            return QualificationDisposition::Insufficient;
        };
        if !source_path_matches(map, receipt, observation, resolved) {
            return QualificationDisposition::Fail;
        }
    }
    let Some(behavior_paths) = evidence.behavior_paths.as_ref() else {
        return QualificationDisposition::Insufficient;
    };
    for required in REQUIRED_BEHAVIOR_PATHS {
        if !behavior_paths.iter().any(|path| path.class == required) {
            return QualificationDisposition::Insufficient;
        }
    }
    for observation in behavior_paths {
        if !behavior_path_matches(receipt, observation) {
            return QualificationDisposition::Fail;
        }
    }
    QualificationDisposition::Pass
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
    if receipt.binaries.is_empty() || receipt.output_references.is_empty() {
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
    Ok(format!("sha256:{:x}", Sha256::digest(fs::read(path)?)))
}

pub(super) fn digest_fields(fields: &[&str]) -> String {
    let mut digest = Sha256::new();
    for field in fields {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field.as_bytes());
    }
    format!("sha256:{:x}", digest.finalize())
}
