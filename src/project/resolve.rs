use super::source_map::{self, ResolvedSourceMap, SourceMapPolicy};
use super::AdapterRefusal;
use crate::storage::PartitionKey;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// How checkout identity enters the compatibility key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PathIdentity {
    /// Fixed owned-fixture admission at one physical root; no logical remapping.
    SameRootCargoFixture { id: String, root: PathBuf },
    /// Default key: the canonical checkout path is part of identity.
    Checkout,
    /// Trial-only identity. Without an explicit qualified map it remains
    /// checkout-isolated inside the disposable root.
    DisposableRoot {
        id: String,
        root: PathBuf,
        source_map: Option<SourceMapPolicy>,
    },
}

/// An opted-in workflow and the inputs it permits the adapter to resolve.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowOptIn {
    pub workflow: String,
    pub selector: String,
    pub profile: String,
    pub features: Vec<String>,
    pub required_slot_bytes: u64,
    pub cargo_config_keys: Vec<String>,
    pub environment_keys: Vec<String>,
    pub path_identity: PathIdentity,
}

/// Minimal project declaration. Opt-in is exact to a workflow and selector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectDescriptor {
    pub project_id: String,
    pub component_id: String,
    pub workspace_root: PathBuf,
    pub workflows: Vec<WorkflowOptIn>,
}

/// Exact tool version and digest of the executable used by the workflow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolchainInput {
    pub version: String,
    pub executable_digest: String,
}

/// A resolved effective setting. `None` explicitly records that a variable is unset.
pub struct EffectiveValue {
    pub name: String,
    pub value: Option<String>,
}

impl fmt::Debug for EffectiveValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EffectiveValue")
            .field("name", &self.name)
            .field("value", &self.value.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

impl Clone for EffectiveValue {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            value: self.value.clone(),
        }
    }
}

impl PartialEq for EffectiveValue {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.value == other.value
    }
}

impl Eq for EffectiveValue {}

/// Whether a complete effective input set was resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectiveInputSet {
    Complete(Vec<EffectiveValue>),
    Unknown,
}

/// Effective toolchain, platform, Cargo configuration, and environment inputs.
#[derive(Clone, Eq, PartialEq)]
pub struct EffectiveBuildInputs {
    pub cargo: ToolchainInput,
    pub rustc: ToolchainInput,
    pub host: String,
    pub target: String,
    pub cargo_configuration: EffectiveInputSet,
    pub environment: EffectiveInputSet,
    /// Complete compiler flags after Cargo configuration and environment
    /// precedence have been resolved. `None` is insufficient for mapped use.
    pub effective_rustflags: Option<Vec<String>>,
    /// Effective profile configuration requested full debug information.
    pub full_debug: bool,
}

impl fmt::Debug for EffectiveBuildInputs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EffectiveBuildInputs")
            .field("cargo", &self.cargo)
            .field("rustc", &self.rustc)
            .field("host", &self.host)
            .field("target", &self.target)
            .field("cargo_configuration", &self.cargo_configuration)
            .field("environment", &self.environment)
            .field(
                "effective_rustflags",
                &self.effective_rustflags.as_ref().map(|_| "[redacted]"),
            )
            .field("full_debug", &self.full_debug)
            .finish()
    }
}

/// One invocation of an exactly declared project workflow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectInvocation {
    pub consumer_id: String,
    pub descriptor: ProjectDescriptor,
    pub workflow: String,
    pub selector: String,
    pub request_id: String,
    pub task_id: String,
    pub workspace_id: String,
    pub commit: String,
    pub source_tree_digest: String,
    pub inputs: EffectiveBuildInputs,
    /// Consumer-owned, invocation-private Cargo final output directory.
    pub target_dir: PathBuf,
    /// Consumer-owned, invocation-private capture directory.
    pub capture_dir: PathBuf,
    /// Allows the consumer to choose a private unmanaged build after a safe refusal.
    pub private_fallback_allowed: bool,
    pub wait_once: bool,
}

pub(super) struct ResolvedInvocation {
    pub workflow: WorkflowOptIn,
    pub partition: PartitionKey,
    pub commit: String,
    pub source_tree_digest: String,
    pub workspace_root: PathBuf,
    pub target_dir: PathBuf,
    pub capture_dir: PathBuf,
    pub environment: Vec<EffectiveValue>,
    pub source_map: Option<ResolvedSourceMap>,
}

pub(super) enum Resolution {
    NotOptedIn,
    Ready(ResolvedInvocation),
    Refused(AdapterRefusal),
}

pub(super) fn resolve(invocation: &ProjectInvocation) -> Resolution {
    let workflow = match select_workflow(invocation) {
        Ok(Some(workflow)) => workflow.clone(),
        Ok(None) => return Resolution::NotOptedIn,
        Err(reason) => return Resolution::Refused(reason),
    };
    match resolve_selected(invocation, workflow) {
        Ok(resolved) => Resolution::Ready(resolved),
        Err(reason) => Resolution::Refused(reason),
    }
}

fn select_workflow(
    invocation: &ProjectInvocation,
) -> Result<Option<&WorkflowOptIn>, AdapterRefusal> {
    validate_text(&invocation.workflow)?;
    validate_text(&invocation.selector)?;
    let mut selected = None;
    let mut seen_workflows = BTreeSet::new();
    for workflow in &invocation.descriptor.workflows {
        validate_text(&workflow.workflow)?;
        if !seen_workflows.insert(workflow.workflow.as_str()) {
            return Err(AdapterRefusal::AmbiguousInput);
        }
        if workflow.workflow == invocation.workflow && workflow.selector == invocation.selector {
            selected = Some(workflow);
        }
    }
    Ok(selected)
}

fn resolve_selected(
    invocation: &ProjectInvocation,
    workflow: WorkflowOptIn,
) -> Result<ResolvedInvocation, AdapterRefusal> {
    validate_text(&invocation.consumer_id)?;
    validate_text(&invocation.descriptor.project_id)?;
    validate_text(&invocation.descriptor.component_id)?;
    validate_text(&invocation.request_id)?;
    validate_text(&invocation.task_id)?;
    validate_text(&invocation.workspace_id)?;
    validate_hash(&invocation.commit, &[40, 64])?;
    validate_hash(&invocation.source_tree_digest, &[64])?;
    validate_text(&workflow.profile)?;
    if workflow.required_slot_bytes == 0 {
        return Err(AdapterRefusal::AmbiguousInput);
    }
    let mut features = workflow.features.clone();
    features.sort();
    if features.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(AdapterRefusal::AmbiguousInput);
    }
    for feature in &features {
        validate_text(feature)?;
    }

    let cargo_version = cargo_version(&invocation.inputs.cargo.version)?;
    if cargo_version < (1, 91) {
        return Err(AdapterRefusal::UnsupportedToolchain);
    }
    let cargo_provenance = toolchain_provenance(&invocation.inputs.cargo)?;
    let rustc_provenance = toolchain_provenance(&invocation.inputs.rustc)?;
    validate_text(&invocation.inputs.host)?;
    validate_text(&invocation.inputs.target)?;

    let cargo_configuration = resolved_values(
        &invocation.inputs.cargo_configuration,
        &workflow.cargo_config_keys,
        false,
    )?;
    let environment = resolved_values(
        &invocation.inputs.environment,
        &workflow.environment_keys,
        true,
    )?;
    let mut config_digest = digest_values(b"cargo-config-v1", &cargo_configuration);
    if let Some(flags) = &invocation.inputs.effective_rustflags {
        let borrowed: Vec<_> = flags.iter().map(String::as_str).collect();
        let flags_digest = source_map::digest_fields(&borrowed);
        config_digest = source_map::digest_fields(&[
            "cargo-config-with-rustflags-v1",
            config_digest.as_str(),
            flags_digest.as_str(),
        ]);
    }
    let environment_digest = digest_values(b"build-environment-v1", &environment);

    let workspace_root = canonical_directory(&invocation.descriptor.workspace_root)?;
    let target_dir = canonical_directory(&invocation.target_dir)?;
    let capture_dir = canonical_directory(&invocation.capture_dir)?;
    if paths_overlap(&target_dir, &capture_dir) {
        return Err(AdapterRefusal::AmbiguousInput);
    }
    let same_root = matches!(
        workflow.path_identity,
        PathIdentity::SameRootCargoFixture { .. }
    );
    let source_tree_digest = if same_root {
        super::freshness::validate_workflow(invocation, &workflow)?;
        super::freshness::requested_tree(&workspace_root)?
    } else {
        invocation.source_tree_digest.clone()
    };
    let (path_identity, mut source_map) = match &workflow.path_identity {
        PathIdentity::SameRootCargoFixture { id, root } => {
            let root = canonical_directory(root)?;
            if !workspace_root.starts_with(&root) || workspace_root == root {
                return Err(AdapterRefusal::UnsupportedInput);
            }
            let identity = source_map::disposable_root_identity(id, &root)?;
            (
                format!(
                    "same-root-fixture-v1:{identity}:{}",
                    workspace_root.display()
                ),
                None,
            )
        }
        PathIdentity::Checkout => (
            workspace_root
                .to_str()
                .ok_or(AdapterRefusal::UnsupportedInput)?
                .to_owned(),
            None,
        ),
        PathIdentity::DisposableRoot {
            id,
            root,
            source_map,
        } => {
            let root = canonical_directory(root)?;
            if workspace_root == root || !workspace_root.starts_with(&root) {
                return Err(AdapterRefusal::UnsupportedInput);
            }
            let root_digest = source_map::disposable_root_identity(id, &root)?;
            match source_map {
                Some(policy) => {
                    let resolved_map = source_map::resolve(
                        policy,
                        &root,
                        &workspace_root,
                        &invocation.source_tree_digest,
                        invocation.inputs.effective_rustflags.as_deref(),
                        invocation.inputs.full_debug,
                    )?;
                    let source_scope = if resolved_map.is_strict_physical() {
                        format!(":strict-root:{}", resolved_map.requested_root_id())
                    } else {
                        String::new()
                    };
                    let identity = format!(
                        "disposable-root:{id}:{root_digest}:{}{source_scope}",
                        resolved_map.map_digest(),
                    );
                    (identity, Some(resolved_map))
                }
                None => {
                    let checkout = workspace_root
                        .to_str()
                        .ok_or(AdapterRefusal::UnsupportedInput)?;
                    (
                        format!("disposable-root:{id}:{root_digest}:checkout:{checkout}"),
                        None,
                    )
                }
            }
        }
    };
    let project_component = format!(
        "{}:{}",
        invocation.descriptor.project_id, invocation.descriptor.component_id
    );
    let partition = PartitionKey {
        project_component,
        workflow: workflow.workflow.clone(),
        cargo_provenance,
        rustc_provenance,
        host: invocation.inputs.host.clone(),
        target: invocation.inputs.target.clone(),
        profile: workflow.profile.clone(),
        features,
        config_digest,
        environment_digest,
        path_identity,
    };
    if let Some(source_map) = source_map.as_mut() {
        source_map.bind_build_identity(&partition);
    }
    Ok(ResolvedInvocation {
        workflow,
        partition,
        commit: invocation.commit.clone(),
        source_tree_digest,
        workspace_root,
        target_dir,
        capture_dir,
        environment,
        source_map,
    })
}

fn resolved_values(
    set: &EffectiveInputSet,
    allowed_names: &[String],
    environment: bool,
) -> Result<Vec<EffectiveValue>, AdapterRefusal> {
    let values = match set {
        EffectiveInputSet::Complete(values) => values,
        EffectiveInputSet::Unknown => return Err(AdapterRefusal::InputUnknown),
    };
    let allowed: BTreeSet<_> = allowed_names.iter().map(String::as_str).collect();
    if allowed.len() != allowed_names.len() {
        return Err(AdapterRefusal::AmbiguousInput);
    }
    let mut names = BTreeSet::new();
    for value in values {
        validate_text(&value.name)?;
        if !allowed.contains(value.name.as_str()) || !names.insert(value.name.as_str()) {
            return Err(AdapterRefusal::UnsupportedInput);
        }
        if environment {
            if is_secret_name(&value.name)
                || matches!(
                    value.name.as_str(),
                    "CARGO_BUILD_BUILD_DIR" | "CARGO_TARGET_DIR"
                )
            {
                return Err(AdapterRefusal::UnsupportedInput);
            }
        }
        if value
            .value
            .as_ref()
            .is_some_and(|entry| entry.len() > 16 * 1024 || entry.as_bytes().contains(&0))
        {
            return Err(AdapterRefusal::UnsupportedInput);
        }
    }
    for name in allowed_names {
        validate_text(name)?;
        if environment && is_secret_name(name) {
            return Err(AdapterRefusal::UnsupportedInput);
        }
        if !names.contains(name.as_str()) {
            return Err(AdapterRefusal::InputUnknown);
        }
    }
    let mut sorted = values.clone();
    sorted.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(sorted)
}

fn toolchain_provenance(tool: &ToolchainInput) -> Result<String, AdapterRefusal> {
    validate_text(&tool.version)?;
    validate_hash(&tool.executable_digest, &[64])?;
    Ok(format!("{} {}", tool.version, tool.executable_digest))
}

fn cargo_version(version: &str) -> Result<(u32, u32), AdapterRefusal> {
    let version = version
        .split_whitespace()
        .find(|word| word.as_bytes().first().is_some_and(u8::is_ascii_digit))
        .ok_or(AdapterRefusal::UnsupportedToolchain)?;
    let mut parts = version.split('.');
    let major = parts
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or(AdapterRefusal::UnsupportedToolchain)?;
    let minor = parts
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or(AdapterRefusal::UnsupportedToolchain)?;
    let _patch = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .ok_or(AdapterRefusal::UnsupportedToolchain)?;
    if parts.next().is_some() {
        return Err(AdapterRefusal::UnsupportedToolchain);
    }
    Ok((major, minor))
}

fn digest_values(domain: &[u8], values: &[EffectiveValue]) -> String {
    let mut digest = Sha256::new();
    digest.update((domain.len() as u64).to_be_bytes());
    digest.update(domain);
    for entry in values {
        update_field(&mut digest, entry.name.as_bytes());
        match &entry.value {
            Some(value) => {
                digest.update([1]);
                update_field(&mut digest, value.as_bytes());
            }
            None => digest.update([0]),
        }
    }
    format!("sha256:{:x}", digest.finalize())
}

fn update_field(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn canonical_directory(path: &Path) -> Result<PathBuf, AdapterRefusal> {
    let canonical = fs::canonicalize(path).map_err(|_| AdapterRefusal::UnsupportedInput)?;
    let metadata = fs::metadata(&canonical).map_err(|_| AdapterRefusal::UnsupportedInput)?;
    if !metadata.is_dir() {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    Ok(canonical)
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn validate_hash(value: &str, lengths: &[usize]) -> Result<(), AdapterRefusal> {
    let raw = value.strip_prefix("sha256:").unwrap_or(value);
    if !lengths.contains(&raw.len()) || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AdapterRefusal::UnsupportedInput);
    }
    Ok(())
}

fn validate_text(value: &str) -> Result<(), AdapterRefusal> {
    if value.is_empty() || value.len() > 128 || value.as_bytes().contains(&0) {
        return Err(AdapterRefusal::AmbiguousInput);
    }
    Ok(())
}

fn is_secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    [
        "SECRET",
        "TOKEN",
        "PASSWORD",
        "CREDENTIAL",
        "PRIVATE_KEY",
        "ACCESS_KEY",
    ]
    .iter()
    .any(|marker| upper.contains(marker))
}
