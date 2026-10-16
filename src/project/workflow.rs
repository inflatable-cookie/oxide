//! One explicit, versioned project/workflow declaration and its read-only plan.
//!
//! This module is the product front end over the private ordinary-Cargo
//! storage route. A project describes one supported Rust workflow once; an
//! operator-owned callable resolves an effective plan with no side effects and
//! binds that declaration to one exact owner-issued attempt before storage is
//! acquired.
//!
//! The declaration is reusable, non-secret policy intent. It never carries
//! task/run/source identities, allocation epochs, credentials, storage
//! endpoints, slot paths, retention proofs or host tokens.
//!
//! Planning is read-only. It performs no storage acquire, daemon provisioning,
//! directory creation, compiler execution, installation or host scan.

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

/// The only declaration format this crate accepts. Mixed or unknown versions
/// refuse; an older/newer format is never reinterpreted.
pub const WORKFLOW_DECLARATION_FORMAT: &str = "oxide-workflow-declaration-v1";
/// Enforced declaration byte ceiling, checked before any parse result is used.
pub const MAX_WORKFLOW_DECLARATION_BYTES: usize = 64 * 1024;
/// Enforced rendered plan byte ceiling, including any fallback/error line.
pub const MAX_WORKFLOW_PLAN_BYTES: usize = 64 * 1024;
/// Finite entry limits for one declaration.
pub const MAX_WORKFLOW_ARGV_ENTRIES: usize = 256;
pub const MAX_WORKFLOW_ARGV_BYTES: usize = 16 * 1024;
pub const MAX_WORKFLOW_ENVIRONMENT_NAMES: usize = 64;
pub const MAX_WORKFLOW_CAPTURE_ROLES: usize = 32;
/// The two compatibility partitions ordinary Cargo reuse supports.
pub const SUPPORTED_WORKFLOW_PROFILES: [&str; 2] = ["dev", "full-debug"];
/// Byte ceiling for one non-secret refusal detail, enforced as `<= 512` bytes
/// on a character boundary.
const REFUSAL_DETAIL_BYTES: usize = 512;
/// Appended only when the rendered plan reaches the output ceiling, so a
/// truncated plan is explicit and still fits `MAX_WORKFLOW_PLAN_BYTES`.
const PLAN_TRUNCATION_MARKER: &str = "\n...(plan truncated at 64 KiB)\n";
/// Per-role capture roles the ordinary route can account for.
pub const SUPPORTED_CAPTURE_ROLES: [&str; 5] = [
    "source-archive",
    "cargo-log",
    "test-result",
    "binary",
    "full-debug",
];

/// A bounded, non-secret refusal from declaration parsing, resolution or
/// binding. `detail` never contains environment values or credentials.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowRefusal {
    pub code: &'static str,
    pub detail: String,
}

impl WorkflowRefusal {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self {
            code,
            detail: bounded_prefix(&detail, REFUSAL_DETAIL_BYTES).to_owned(),
        }
    }

    /// One bounded, non-secret refusal line.
    pub fn render(&self) -> String {
        format!("refused: {} ({})", self.code, self.detail)
    }
}

impl fmt::Display for WorkflowRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

impl std::error::Error for WorkflowRefusal {}

/// Read one explicit local declaration file with the 64 KiB ceiling enforced
/// while reading.
///
/// The named path is opened once. A FIFO, socket, device or directory is
/// refused from `lstat` without opening it. A symlink is opened with
/// `O_NONBLOCK` so a special-file target cannot block the process; the opened
/// descriptor must still be a regular file. At most `cap + 1` bytes are
/// allocated and copied, so a size hint or a file that grows cannot cause
/// unbounded allocation. Refusals are bounded and never include file
/// contents, secrets or the full path.
///
/// Open and read remain ordinary filesystem syscalls. This function does not
/// impose a hard deadline against an uninterruptible kernel wait, walk
/// directories or discover other host paths.
pub fn read_workflow_declaration(path: &Path) -> Result<Vec<u8>, WorkflowRefusal> {
    read_declaration_file(path, MAX_WORKFLOW_DECLARATION_BYTES)
}

fn read_declaration_file(path: &Path, cap: usize) -> Result<Vec<u8>, WorkflowRefusal> {
    let named = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return Err(unreadable_declaration()),
    };
    let named_type = named.file_type();
    if named_type.is_dir()
        || named_type.is_fifo()
        || named_type.is_socket()
        || named_type.is_block_device()
        || named_type.is_char_device()
    {
        return Err(not_regular_declaration());
    }
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(_) => return Err(unreadable_declaration()),
    };
    let opened = match file.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return Err(unreadable_declaration()),
    };
    if !opened.is_file() {
        return Err(not_regular_declaration());
    }
    read_capped(&mut file, cap)
}

/// Copy at most `cap + 1` bytes from `reader`. Used by the file reader and as
/// the allocation/read-count test seam so a huge source is unnecessary.
fn read_capped<R: Read>(reader: &mut R, cap: usize) -> Result<Vec<u8>, WorkflowRefusal> {
    let mut bytes = Vec::with_capacity(cap.saturating_add(1));
    let take_limit = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
    if reader.take(take_limit).read_to_end(&mut bytes).is_err() {
        return Err(unreadable_declaration());
    }
    if bytes.len() > cap {
        return Err(WorkflowRefusal::new(
            "declaration-too-large",
            format!("declaration exceeds the {cap} byte ceiling"),
        ));
    }
    Ok(bytes)
}

fn unreadable_declaration() -> WorkflowRefusal {
    WorkflowRefusal::new(
        "declaration-unreadable",
        "declaration file could not be read",
    )
}

fn not_regular_declaration() -> WorkflowRefusal {
    WorkflowRefusal::new(
        "declaration-not-regular",
        "declaration path is not a regular file",
    )
}

/// Declared project/component/workflow identity, the declared project root and
/// the workspace relative to it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectDeclaration {
    root: PathBuf,
    consumer: String,
    project: String,
    component: String,
    workflow: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowCommand {
    selector: String,
    argv: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowIntent {
    profile: String,
    incremental: bool,
    debug: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureLimitsWire {
    version: u32,
    total_bytes: u64,
    role_bytes: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowLimits {
    storage_bytes: u64,
    capture: CaptureLimitsWire,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclarationWire {
    format: String,
    project: ProjectDeclaration,
    workspace: PathBuf,
    workflow: WorkflowCommand,
    intent: WorkflowIntent,
    environment_names: Vec<String>,
    limits: WorkflowLimits,
}

/// Finite capture ceilings restated from the declaration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureLimits {
    pub version: u32,
    pub total_bytes: u64,
    pub role_bytes: BTreeMap<String, u64>,
}

/// One honest, non-fabricated unresolved item the plan can show without
/// claiming a compatibility digest or a warm hit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Unavailable {
    pub code: &'static str,
    pub detail: String,
}

/// The effective, read-only workflow plan. It is produced with no side effect
/// and carries only non-secret policy intent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowPlan {
    pub declaration_digest: String,
    pub project_root: PathBuf,
    pub source_root: PathBuf,
    pub consumer: String,
    pub project: String,
    pub component: String,
    pub workflow: String,
    pub selector: String,
    pub argv: Vec<String>,
    pub profile: String,
    pub partition: &'static str,
    pub incremental: bool,
    pub debug: String,
    pub environment_names: Vec<String>,
    pub storage_bytes: u64,
    pub capture: CaptureLimits,
    pub unavailable: Vec<Unavailable>,
}

/// Fields observed on an exact owner-issued attempt that the declaration must
/// bind. Values are never echoed; only names and digests.
#[derive(Clone, Debug)]
pub struct ObservedWorkflow<'a> {
    pub source_root: &'a Path,
    pub consumer: &'a str,
    pub project: &'a str,
    pub component: &'a str,
    pub workflow: &'a str,
    pub selector: &'a str,
    pub argv: &'a [String],
    pub profile: &'a str,
    pub partition: &'a str,
    pub required_bytes: u64,
    pub environment_names: &'a BTreeSet<String>,
    pub capture: &'a CaptureLimits,
}

impl WorkflowPlan {
    /// Resolve one declaration into an effective plan with no side effects.
    pub fn resolve(bytes: &[u8]) -> Result<Self, WorkflowRefusal> {
        if bytes.is_empty() {
            return Err(WorkflowRefusal::new(
                "declaration-too-large",
                "declaration is empty",
            ));
        }
        if bytes.len() > MAX_WORKFLOW_DECLARATION_BYTES {
            return Err(WorkflowRefusal::new(
                "declaration-too-large",
                format!(
                    "declaration exceeds the {} byte ceiling",
                    MAX_WORKFLOW_DECLARATION_BYTES
                ),
            ));
        }
        if std::str::from_utf8(bytes).is_err() {
            return Err(WorkflowRefusal::new(
                "declaration-not-utf8",
                "declaration is not UTF-8",
            ));
        }
        let value: Value = parse_unique_json(bytes)?;
        let declaration_digest = canonical_digest(&value)?;
        let wire: DeclarationWire = serde_json::from_value(value).map_err(|error| {
            let message = error.to_string();
            if message.contains("unknown field") {
                WorkflowRefusal::new("unknown-field", message)
            } else {
                WorkflowRefusal::new("declaration-malformed", message)
            }
        })?;
        if wire.format != WORKFLOW_DECLARATION_FORMAT {
            return Err(WorkflowRefusal::new(
                "unsupported-format",
                "declaration format is not the supported version",
            ));
        }
        validate_text_field("project.consumer", &wire.project.consumer)?;
        validate_text_field("project.project", &wire.project.project)?;
        validate_text_field("project.component", &wire.project.component)?;
        validate_text_field("project.workflow", &wire.project.workflow)?;
        validate_identity_token("project.consumer", &wire.project.consumer)?;
        validate_identity_token("project.project", &wire.project.project)?;
        validate_identity_token("project.component", &wire.project.component)?;
        validate_identity_token("project.workflow", &wire.project.workflow)?;
        let source_root = resolve_workspace(&wire.project.root, &wire.workspace)?;
        validate_selector(&wire.workflow.selector)?;
        validate_argv(&wire.workflow.argv)?;
        let (profile, partition, debug) = resolve_intent(&wire.intent)?;
        let environment_names = validate_environment_names(&wire.environment_names)?;
        let capture = validate_limits(&wire.limits)?;
        Ok(Self {
            declaration_digest,
            project_root: wire.project.root,
            source_root,
            consumer: wire.project.consumer,
            project: wire.project.project,
            component: wire.project.component,
            workflow: wire.project.workflow,
            selector: wire.workflow.selector,
            argv: wire.workflow.argv,
            profile,
            partition,
            incremental: wire.intent.incremental,
            debug,
            environment_names,
            storage_bytes: wire.limits.storage_bytes,
            capture,
            unavailable: unresolved_items(),
        })
    }

    /// Bind this declaration to the exact fields of one owner-issued attempt.
    /// Any mismatch refuses; the request is never reinterpreted.
    pub fn verify_observed(&self, observed: &ObservedWorkflow<'_>) -> Result<(), WorkflowRefusal> {
        let mismatch = |field: &'static str| {
            Err(WorkflowRefusal::new(
                "observed-mismatch",
                format!("owner-issued attempt does not match declared {field}"),
            ))
        };
        if observed.source_root != self.source_root {
            return mismatch("source root");
        }
        if observed.consumer != self.consumer {
            return mismatch("consumer");
        }
        if observed.project != self.project {
            return mismatch("project");
        }
        if observed.component != self.component {
            return mismatch("component");
        }
        if observed.workflow != self.workflow {
            return mismatch("workflow");
        }
        if observed.selector != self.selector {
            return mismatch("selector");
        }
        if observed.argv != self.argv.as_slice() {
            return mismatch("argv");
        }
        if observed.profile != self.profile {
            return mismatch("profile");
        }
        if observed.partition != self.partition {
            return mismatch("partition");
        }
        if observed.required_bytes != self.storage_bytes {
            return mismatch("storage limit");
        }
        if observed.capture != &self.capture {
            return mismatch("capture limits");
        }
        let declared: BTreeSet<&str> = self.environment_names.iter().map(String::as_str).collect();
        if observed.environment_names.len() != declared.len()
            || observed
                .environment_names
                .iter()
                .any(|name| !declared.contains(name.as_str()))
        {
            return mismatch("environment-policy names");
        }
        Ok(())
    }

    /// Render the bounded effective plan. This is the only operator-visible
    /// plan surface and it never contains a secret or a fabricated digest.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("oxide-workflow-plan v1\n");
        out.push_str(&format!("format: {}\n", WORKFLOW_DECLARATION_FORMAT));
        out.push_str(&format!(
            "declaration_digest: {}\n",
            self.declaration_digest
        ));
        out.push_str(&format!("project_root: {}\n", display(&self.project_root)));
        out.push_str(&format!("source_root: {}\n", display(&self.source_root)));
        out.push_str(&format!(
            "project_identity: consumer={} project={} component={} workflow={}\n",
            self.consumer, self.project, self.component, self.workflow
        ));
        out.push_str(&format!("selector: {}\n", self.selector));
        out.push_str(&format!("argv: {}\n", render_argv(&self.argv)));
        out.push_str(&format!("cwd: {}\n", display(&self.source_root)));
        out.push_str(&format!("profile: {}\n", self.profile));
        out.push_str(&format!("partition: {}\n", self.partition));
        out.push_str(&format!("incremental: {}\n", self.incremental));
        out.push_str(&format!("debug: {}\n", self.debug));
        out.push_str(&format!(
            "environment_names: {}\n",
            render_names(&self.environment_names)
        ));
        out.push_str(&format!("storage_bytes: {}\n", self.storage_bytes));
        out.push_str(&format!(
            "capture: version={} total_bytes={} roles={}\n",
            self.capture.version,
            self.capture.total_bytes,
            render_role_bytes(&self.capture.role_bytes)
        ));
        out.push_str("command_form: exact argv array; no shell string and no hidden flags\n");
        out.push_str("owner_grants_required:\n");
        out.push_str("  - oxide-storage ordinary-Cargo policy v3 acceptance credential and exact request allowlist\n");
        out.push_str("  - protected storage runtime instance and OXSTOR01 version-2 endpoint\n");
        out.push_str("  - owner callback key and ordinary-cargo-attempt-v3 callback context\n");
        out.push_str("  - consumer submit, capture and not_submitted Effigy owner installation\n");
        out.push_str("unavailable:\n");
        for item in &self.unavailable {
            out.push_str(&format!("  - {}: {}\n", item.code, item.detail));
        }
        out.push_str("effects: none (no storage acquire, daemon provisioning, directory creation, compiler execution, installation or host scan)\n");
        if out.len() > MAX_WORKFLOW_PLAN_BYTES {
            let body_budget = MAX_WORKFLOW_PLAN_BYTES - PLAN_TRUNCATION_MARKER.len();
            let body = bounded_prefix(&out, body_budget).to_owned();
            out = body;
            out.push_str(PLAN_TRUNCATION_MARKER);
        }
        debug_assert!(out.len() <= MAX_WORKFLOW_PLAN_BYTES);
        out
    }
}

/// Longest byte prefix of `value` that ends on a UTF-8 character boundary.
/// Truncation is always bounded and never panics on in-bounds multi-byte input.
fn bounded_prefix(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn unresolved_items() -> Vec<Unavailable> {
    vec![
        Unavailable {
            code: "owner-grant-missing",
            detail: "no owner-issued policy v3 acceptance credential is referenced by the declaration; the operator must install it".into(),
        },
        Unavailable {
            code: "compatibility-unknown",
            detail: "the compatibility partition is fixed only by the owner-issued request boundary".into(),
        },
        Unavailable {
            code: "warm-reuse-unknown",
            detail: "no warm or cold result exists before an authenticated attempt; the plan never fabricates a hit".into(),
        },
        Unavailable {
            code: "consumer-adapter-absent",
            detail: "no current Nucleus or Effigy adapter consumes this declaration; installation remains separately gated".into(),
        },
    ]
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

fn render_argv(argv: &[String]) -> String {
    let mut out = String::from("[");
    for (index, argument) in argv.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!("{:?}", argument));
    }
    out.push(']');
    out
}

fn render_names(names: &[String]) -> String {
    let mut out = String::from("[");
    for (index, name) in names.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(name);
    }
    out.push(']');
    out
}

fn render_role_bytes(roles: &BTreeMap<String, u64>) -> String {
    let mut out = String::from("{");
    for (index, (role, bytes)) in roles.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!("{role}:{bytes}"));
    }
    out.push('}');
    out
}

/// Same canonical encoding the private route uses: UTF-8, recursively sorted
/// object keys by UTF-8 byte order, no lossy number conversion.
fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>, serde_json::Error> {
    fn sort_objects(value: &Value) -> Value {
        match value {
            Value::Object(fields) => {
                let mut keys: Vec<_> = fields.keys().collect();
                keys.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
                let mut sorted = serde_json::Map::new();
                for key in keys {
                    sorted.insert(key.clone(), sort_objects(&fields[key]));
                }
                Value::Object(sorted)
            }
            Value::Array(values) => Value::Array(values.iter().map(sort_objects).collect()),
            value => value.clone(),
        }
    }

    serde_json::to_vec(&sort_objects(value))
}

fn canonical_digest(value: &Value) -> Result<String, WorkflowRefusal> {
    let bytes = canonical_json_bytes(value).map_err(|_| {
        WorkflowRefusal::new("declaration-malformed", "JSON is not canonicalizable")
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(&bytes)))
}

fn validate_text_field(field: &'static str, value: &str) -> Result<(), WorkflowRefusal> {
    if value.is_empty() {
        return Err(WorkflowRefusal::new(
            "empty-value",
            format!("{field} is empty"),
        ));
    }
    if value.len() > 4096 || value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(WorkflowRefusal::new(
            "oversized-value",
            format!("{field} is not a bounded plain string"),
        ));
    }
    Ok(())
}

fn validate_identity_token(field: &'static str, value: &str) -> Result<(), WorkflowRefusal> {
    if !value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
    }) {
        return Err(WorkflowRefusal::new(
            "empty-value",
            format!("{field} contains a character outside the accepted identity set"),
        ));
    }
    Ok(())
}

fn resolve_workspace(root: &Path, workspace: &Path) -> Result<PathBuf, WorkflowRefusal> {
    if !root.is_absolute() {
        return Err(WorkflowRefusal::new(
            "project-root-not-absolute",
            "declared project root must be an absolute path",
        ));
    }
    let metadata = fs::symlink_metadata(root).map_err(|_| {
        WorkflowRefusal::new(
            "project-root-not-directory",
            "declared project root is absent",
        )
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(WorkflowRefusal::new(
            "project-root-not-directory",
            "declared project root must be a real directory, not a symlink",
        ));
    }
    let canonical_root = fs::canonicalize(root).map_err(|_| {
        WorkflowRefusal::new("project-root-not-canonical", "project root is unresolvable")
    })?;
    if canonical_root != root {
        return Err(WorkflowRefusal::new(
            "project-root-not-canonical",
            "declared project root must already be canonical",
        ));
    }
    if workspace.as_os_str().is_empty() {
        return Err(WorkflowRefusal::new(
            "workspace-escape",
            "workspace must name a relative path beneath the project root",
        ));
    }
    if workspace.is_absolute()
        || workspace
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(WorkflowRefusal::new(
            "workspace-escape",
            "workspace contains an absolute, parent or current-directory component",
        ));
    }
    let joined = root.join(workspace);
    let metadata = fs::symlink_metadata(&joined)
        .map_err(|_| WorkflowRefusal::new("workspace-missing", "declared workspace is absent"))?;
    if metadata.file_type().is_symlink() {
        return Err(WorkflowRefusal::new(
            "workspace-symlink-escape",
            "declared workspace must be a real directory, not a symlink",
        ));
    }
    if !metadata.is_dir() {
        return Err(WorkflowRefusal::new(
            "workspace-missing",
            "declared workspace must be a directory",
        ));
    }
    let canonical = fs::canonicalize(&joined).map_err(|_| {
        WorkflowRefusal::new("workspace-symlink-escape", "workspace is unresolvable")
    })?;
    if canonical != joined || !canonical.starts_with(root) {
        return Err(WorkflowRefusal::new(
            "workspace-symlink-escape",
            "workspace must resolve to a real path beneath the declared project root",
        ));
    }
    Ok(canonical)
}

fn validate_selector(selector: &str) -> Result<(), WorkflowRefusal> {
    validate_text_field("workflow.selector", selector)?;
    if selector.len() > 4096
        || selector.bytes().any(|byte| {
            byte.is_ascii_whitespace() || byte == b'"' || byte == b'\'' || byte == b'\\'
        })
    {
        return Err(WorkflowRefusal::new(
            "ambiguous-command",
            "selector must be one exact token without quoting or shell syntax",
        ));
    }
    Ok(())
}

fn validate_argv(argv: &[String]) -> Result<(), WorkflowRefusal> {
    if argv.is_empty() {
        return Err(WorkflowRefusal::new(
            "ambiguous-command",
            "argv must be a nonempty exact argument array",
        ));
    }
    if argv.len() > MAX_WORKFLOW_ARGV_ENTRIES {
        return Err(WorkflowRefusal::new(
            "ambiguous-command",
            "argv exceeds the accepted entry ceiling",
        ));
    }
    for argument in argv {
        if argument.as_bytes().contains(&0) {
            return Err(WorkflowRefusal::new(
                "ambiguous-command",
                "argv contains a NUL byte",
            ));
        }
        if argument.len() > MAX_WORKFLOW_ARGV_BYTES {
            return Err(WorkflowRefusal::new(
                "oversized-value",
                "an argv entry exceeds the accepted byte ceiling",
            ));
        }
        if argument
            .bytes()
            .any(|byte| byte.is_ascii_control() && byte != b'\n' && byte != b'\t')
        {
            return Err(WorkflowRefusal::new(
                "ambiguous-command",
                "argv contains a control byte",
            ));
        }
    }
    Ok(())
}

fn resolve_intent(
    intent: &WorkflowIntent,
) -> Result<(String, &'static str, String), WorkflowRefusal> {
    if !intent.incremental {
        return Err(WorkflowRefusal::new(
            "unsupported-incremental-policy",
            "the ordinary-Cargo route requires incremental intermediates; non-incremental intent is unsupported",
        ));
    }
    let partition = match intent.profile.as_str() {
        "dev" => "ordinary-test",
        "full-debug" => "full-debug",
        _ => {
            return Err(WorkflowRefusal::new(
                "unsupported-profile",
                "profile must be one of the two supported ordinary-Cargo partitions",
            ))
        }
    };
    let expected_debug = if intent.profile == "full-debug" {
        "full"
    } else {
        "line-tables"
    };
    if intent.debug != "line-tables" && intent.debug != "full" {
        return Err(WorkflowRefusal::new(
            "unsupported-debug-intent",
            "debug intent must be line-tables or full",
        ));
    }
    if intent.debug != expected_debug {
        return Err(WorkflowRefusal::new(
            "unsupported-debug-intent",
            "debug intent does not match the selected profile partition",
        ));
    }
    Ok((intent.profile.clone(), partition, intent.debug.clone()))
}

fn validate_environment_names(names: &[String]) -> Result<Vec<String>, WorkflowRefusal> {
    if names.len() > MAX_WORKFLOW_ENVIRONMENT_NAMES {
        return Err(WorkflowRefusal::new(
            "invalid-limits",
            "environment_names exceeds the accepted entry ceiling",
        ));
    }
    let mut seen = BTreeSet::new();
    for name in names {
        if name.is_empty()
            || name.len() > 256
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(WorkflowRefusal::new(
                "empty-value",
                "environment_names entries must be plain variable names",
            ));
        }
        if !seen.insert(name.as_str()) {
            return Err(WorkflowRefusal::new(
                "duplicate-field",
                "environment_names contains a duplicate name",
            ));
        }
    }
    Ok(names.to_vec())
}

fn validate_limits(limits: &WorkflowLimits) -> Result<CaptureLimits, WorkflowRefusal> {
    if limits.storage_bytes == 0 {
        return Err(WorkflowRefusal::new(
            "invalid-limits",
            "storage_bytes must be a positive finite byte ceiling",
        ));
    }
    let capture = &limits.capture;
    if capture.version != 1
        || capture.total_bytes == 0
        || capture.role_bytes.is_empty()
        || capture.role_bytes.len() > MAX_WORKFLOW_CAPTURE_ROLES
    {
        return Err(WorkflowRefusal::new(
            "invalid-limits",
            "capture limits must use version 1 and a finite nonempty role set",
        ));
    }
    for (role, bytes) in &capture.role_bytes {
        if !SUPPORTED_CAPTURE_ROLES.contains(&role.as_str()) {
            return Err(WorkflowRefusal::new(
                "invalid-role",
                "capture role set contains an unsupported role",
            ));
        }
        if *bytes == 0 || *bytes > capture.total_bytes {
            return Err(WorkflowRefusal::new(
                "invalid-limits",
                "each capture role ceiling must be positive and no larger than the total ceiling",
            ));
        }
    }
    Ok(CaptureLimits {
        version: capture.version,
        total_bytes: capture.total_bytes,
        role_bytes: capture.role_bytes.clone(),
    })
}

/// Reject duplicate JSON object keys before any value is trusted.
struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = UniqueValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON with unique object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(UniqueValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(UniqueValue(value)) = sequence.next_element()? {
            values.push(value);
            if values.len() > 4096 {
                return Err(serde::de::Error::custom("JSON array exceeds limit"));
            }
        }
        Ok(UniqueValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = serde_json::Map::new();
        while let Some((key, UniqueValue(value))) = object.next_entry::<String, UniqueValue>()? {
            if values.insert(key, value).is_some() {
                return Err(serde::de::Error::custom("duplicate JSON object key"));
            }
            if values.len() > 4096 {
                return Err(serde::de::Error::custom("JSON object exceeds limit"));
            }
        }
        Ok(UniqueValue(Value::Object(values)))
    }
}

fn parse_unique_json(bytes: &[u8]) -> Result<Value, WorkflowRefusal> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let UniqueValue(value) = UniqueValue::deserialize(&mut deserializer).map_err(|error| {
        let message = error.to_string();
        if message.contains("duplicate JSON object key") {
            WorkflowRefusal::new("duplicate-field", message)
        } else {
            WorkflowRefusal::new("declaration-malformed", message)
        }
    })?;
    deserializer.end().map_err(|_| {
        WorkflowRefusal::new(
            "declaration-malformed",
            "trailing bytes after the declaration",
        )
    })?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::io::{self, Read};
    use std::os::unix::ffi::OsStrExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

    struct TempRoot {
        path: PathBuf,
    }

    impl TempRoot {
        fn new(name: &str) -> Self {
            let parent = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join("declared-workflow")
                .join(name);
            fs::create_dir_all(&parent).expect("create declaration scratch parent");
            loop {
                let id = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
                let path = parent.join(format!("r{:x}-{:x}", std::process::id(), id));
                match fs::create_dir(&path) {
                    Ok(()) => return Self { path },
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create declaration scratch root: {error}"),
                }
            }
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn declaration(root: &Path) -> String {
        format!(
            r#"{{
  "format": "oxide-workflow-declaration-v1",
  "project": {{
    "root": "{}",
    "consumer": "synthetic-consumer",
    "project": "synthetic-project",
    "component": "synthetic-component",
    "workflow": "synthetic-workflow"
  }},
  "workspace": "workspace",
  "workflow": {{"selector": "rust:test", "argv": ["test", "--workspace"]}},
  "intent": {{"profile": "dev", "incremental": true, "debug": "line-tables"}},
  "environment_names": ["CARGO_HOME", "RUSTFLAGS"],
  "limits": {{
    "storage_bytes": 1024,
    "capture": {{
      "version": 1,
      "total_bytes": 4096,
      "role_bytes": {{"test-result": 4096}}
    }}
  }}
}}"#,
            root.display()
        )
    }

    fn fixture(name: &str) -> (TempRoot, PathBuf) {
        let root = TempRoot::new(name);
        let workspace = root.path.join("workspace");
        fs::create_dir(&workspace).unwrap();
        let mut permissions = fs::metadata(&root.path).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o700);
        fs::set_permissions(&root.path, permissions).unwrap();
        (root, workspace)
    }

    #[test]
    fn declared_workflow_plan_resolves_without_effects() {
        let (root, workspace) = fixture("plan");
        let bytes = declaration(&root.path);
        assert!(bytes.len() <= MAX_WORKFLOW_DECLARATION_BYTES);
        let before: Vec<_> = fs::read_dir(&root.path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let plan = WorkflowPlan::resolve(bytes.as_bytes()).unwrap();
        let after: Vec<_> = fs::read_dir(&root.path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(before, after, "planning must not create or remove entries");
        assert_eq!(plan.source_root, workspace);
        assert_eq!(plan.partition, "ordinary-test");
        assert_eq!(plan.profile, "dev");
        assert_eq!(plan.debug, "line-tables");
        assert!(plan.incremental);
        assert_eq!(plan.storage_bytes, 1024);
        assert_eq!(plan.capture.total_bytes, 4096);
        assert_eq!(plan.environment_names, vec!["CARGO_HOME", "RUSTFLAGS"]);
        assert!(plan.declaration_digest.starts_with("sha256:"));
        let rendered = plan.render();
        assert!(rendered.len() <= MAX_WORKFLOW_PLAN_BYTES);
        assert!(rendered.contains("partition: ordinary-test"));
        assert!(rendered.contains("effects: none"));
        assert!(rendered.contains("owner-grant-missing"));
        assert!(rendered.contains("warm-reuse-unknown"));
        assert!(!rendered.contains("RUSTFLAGS="));
    }

    #[test]
    fn committed_example_declaration_resolves_against_a_real_root() {
        let (root, workspace) = fixture("example");
        let example: Value = serde_json::from_str(include_str!(
            "../../fixtures/ordinary-cargo-declaration-v1.json"
        ))
        .expect("committed example declaration parses");
        let mut example = example;
        example["project"]["root"] = serde_json::json!(root.path);
        let bytes = serde_json::to_vec(&example).unwrap();
        let plan = WorkflowPlan::resolve(&bytes).unwrap();
        assert_eq!(plan.source_root, workspace);
        assert_eq!(plan.consumer, "nucleus");
        assert_eq!(plan.workflow, "core-test");
        assert_eq!(plan.selector, "check:core:test");
        assert_eq!(plan.partition, "ordinary-test");
        assert_eq!(plan.storage_bytes, 1073741824);
        assert_eq!(plan.capture.total_bytes, 268435456);
        // The committed example is refused as written because its placeholder
        // root is not a real directory; no compatibility result is fabricated.
        assert_eq!(
            WorkflowPlan::resolve(
                include_str!("../../fixtures/ordinary-cargo-declaration-v1.json").as_bytes()
            )
            .unwrap_err()
            .code,
            "project-root-not-directory"
        );
    }

    #[test]
    fn declared_workflow_bounds_multibyte_refusals_and_plan_output() {
        // The byte-prefix helper is exhaustively boundary-safe for every ceiling.
        for prefix in 0..4usize {
            let value = format!("{}{}", "a".repeat(prefix), "€".repeat(8));
            for ceiling in 0..=value.len() {
                let bounded = bounded_prefix(&value, ceiling);
                assert!(bounded.len() <= ceiling);
                assert!(value.is_char_boundary(bounded.len()));
            }
        }

        let (root, _) = fixture("bounds");
        // A bounded refusal for an in-bounds declaration whose unknown field
        // name is long and multi-byte must not panic.
        let mut value: Value = serde_json::from_str(&declaration(&root.path)).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("é".repeat(400), serde_json::json!(1));
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(bytes.len() <= MAX_WORKFLOW_DECLARATION_BYTES);
        let refusal = WorkflowPlan::resolve(&bytes).unwrap_err();
        assert_eq!(refusal.code, "unknown-field");
        assert!(refusal.detail.len() <= REFUSAL_DETAIL_BYTES);
        assert!(!refusal.render().is_empty());

        // An in-bounds declaration whose plan exceeds the output ceiling must
        // truncate on a character boundary for every byte alignment.
        let base = WorkflowPlan::resolve(declaration(&root.path).as_bytes()).unwrap();
        for extra in 0..6usize {
            let mut plan = base.clone();
            plan.argv = vec![format!("{}{}", "a".repeat(extra), "€".repeat(22_000))];
            let rendered = plan.render();
            assert!(
                rendered.len() <= MAX_WORKFLOW_PLAN_BYTES,
                "plan output must stay within the ceiling (extra={extra})"
            );
            assert!(
                rendered.contains("(plan truncated at 64 KiB)"),
                "an over-ceiling plan must say it was truncated (extra={extra})"
            );
        }
        // A normal plan is unaffected by the truncation path.
        let rendered = base.render();
        assert!(rendered.len() < MAX_WORKFLOW_PLAN_BYTES);
        assert!(!rendered.contains("plan truncated"));
        assert!(rendered.contains("effects: none"));

        // The reviewer's exact shape: a declaration that is itself within the
        // 64 KiB ceiling and whose plan output crosses it, for every byte
        // alignment of a multi-byte argv run.
        let mut over_ceiling = 0usize;
        for extra in 0..6usize {
            let mut value: Value = serde_json::from_str(&declaration(&root.path)).unwrap();
            let chunks: Vec<String> = (0..5)
                .map(|index| {
                    let prefix = if index == 0 {
                        "a".repeat(extra)
                    } else {
                        String::new()
                    };
                    format!("{prefix}{}", "€".repeat(4_300))
                })
                .collect();
            value["workflow"]["argv"] = serde_json::json!(chunks);
            let bytes = serde_json::to_vec(&value).unwrap();
            assert!(bytes.len() <= MAX_WORKFLOW_DECLARATION_BYTES);
            let plan = WorkflowPlan::resolve(&bytes).unwrap();
            let rendered = plan.render();
            assert!(
                rendered.len() <= MAX_WORKFLOW_PLAN_BYTES,
                "plan output must stay within the ceiling (extra={extra})"
            );
            if rendered.contains("(plan truncated at 64 KiB)") {
                over_ceiling += 1;
            }
        }
        assert!(
            over_ceiling > 0,
            "the over-ceiling render path must be exercised"
        );
    }

    #[test]
    fn declared_workflow_plan_binds_only_the_exact_observed_attempt() {
        let (root, workspace) = fixture("bind");
        let plan = WorkflowPlan::resolve(declaration(&root.path).as_bytes()).unwrap();
        let names: BTreeSet<String> = ["CARGO_HOME", "RUSTFLAGS"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let observed = ObservedWorkflow {
            source_root: &workspace,
            consumer: "synthetic-consumer",
            project: "synthetic-project",
            component: "synthetic-component",
            workflow: "synthetic-workflow",
            selector: "rust:test",
            argv: &["test".to_owned(), "--workspace".to_owned()],
            profile: "dev",
            partition: "ordinary-test",
            required_bytes: 1024,
            environment_names: &names,
            capture: &plan.capture,
        };
        assert!(plan.verify_observed(&observed).is_ok());
        let mut wrong_argv = vec!["test".to_owned(), "--changed".to_owned()];
        let changed = ObservedWorkflow {
            argv: &mut wrong_argv,
            ..observed.clone()
        };
        assert_eq!(
            plan.verify_observed(&changed).unwrap_err().code,
            "observed-mismatch"
        );
        let changed = ObservedWorkflow {
            required_bytes: 2048,
            ..observed.clone()
        };
        assert_eq!(
            plan.verify_observed(&changed).unwrap_err().code,
            "observed-mismatch"
        );
        let changed = ObservedWorkflow {
            selector: "rust:other",
            ..observed.clone()
        };
        assert_eq!(
            plan.verify_observed(&changed).unwrap_err().code,
            "observed-mismatch"
        );
        let fewer: BTreeSet<String> = ["CARGO_HOME"].into_iter().map(str::to_owned).collect();
        let changed = ObservedWorkflow {
            environment_names: &fewer,
            ..observed
        };
        assert_eq!(
            plan.verify_observed(&changed).unwrap_err().code,
            "observed-mismatch"
        );
    }

    #[test]
    fn declared_workflow_refuses_malformed_unknown_and_mixed_versions() {
        let (root, _) = fixture("refuse");
        let good = declaration(&root.path);
        let plan = WorkflowPlan::resolve(good.as_bytes()).unwrap();

        let duplicate = good.replace(
            "\"selector\": \"rust:test\"",
            "\"selector\": \"rust:test\", \"selector\": \"rust:other\"",
        );
        assert_eq!(
            WorkflowPlan::resolve(duplicate.as_bytes())
                .unwrap_err()
                .code,
            "duplicate-field"
        );
        let unknown = good.replace("\"workspace\"", "\"workspace_extra\"");
        assert_eq!(
            WorkflowPlan::resolve(unknown.as_bytes()).unwrap_err().code,
            "unknown-field"
        );
        let mixed = good.replace(WORKFLOW_DECLARATION_FORMAT, "oxide-workflow-declaration-v2");
        assert_eq!(
            WorkflowPlan::resolve(mixed.as_bytes()).unwrap_err().code,
            "unsupported-format"
        );
        let truncated = &good[..good.len() - 4];
        assert_eq!(
            WorkflowPlan::resolve(truncated.as_bytes())
                .unwrap_err()
                .code,
            "declaration-malformed"
        );
        let oversized = vec![b' '; MAX_WORKFLOW_DECLARATION_BYTES + 1];
        assert_eq!(
            WorkflowPlan::resolve(&oversized).unwrap_err().code,
            "declaration-too-large"
        );
        // A shell-command string form is not expressible: it is an unknown field.
        let shell = good.replace(
            "\"workflow\": {\"selector\": \"rust:test\", \"argv\": [\"test\", \"--workspace\"]}",
            "\"workflow\": {\"selector\": \"rust:test\", \"argv\": [\"test\", \"--workspace\"], \"command\": \"cargo test\"}",
        );
        assert_eq!(
            WorkflowPlan::resolve(shell.as_bytes()).unwrap_err().code,
            "unknown-field"
        );
        // An explicit environment value is refused; only policy names are declarable.
        let env_value = good.replace(
            "\"environment_names\": [\"CARGO_HOME\", \"RUSTFLAGS\"]",
            "\"environment\": {\"RUSTFLAGS\": \"-Cdebuginfo=2\"}",
        );
        assert_eq!(
            WorkflowPlan::resolve(env_value.as_bytes())
                .unwrap_err()
                .code,
            "unknown-field"
        );
        // Unsupported intent refuses rather than silently selecting a partition.
        let non_incremental = good.replace("\"incremental\": true", "\"incremental\": false");
        assert_eq!(
            WorkflowPlan::resolve(non_incremental.as_bytes())
                .unwrap_err()
                .code,
            "unsupported-incremental-policy"
        );
        let bad_debug = good.replace("\"debug\": \"line-tables\"", "\"debug\": \"full\"");
        assert_eq!(
            WorkflowPlan::resolve(bad_debug.as_bytes())
                .unwrap_err()
                .code,
            "unsupported-debug-intent"
        );
        let bad_profile = good.replace("\"profile\": \"dev\"", "\"profile\": \"release\"");
        assert_eq!(
            WorkflowPlan::resolve(bad_profile.as_bytes())
                .unwrap_err()
                .code,
            "unsupported-profile"
        );
        assert_eq!(plan.selector, "rust:test");
    }

    #[test]
    fn declared_workflow_refuses_path_escape_and_symlink_escape() {
        let (root, _) = fixture("escape");
        let outside = TempRoot::new("escape-outside");
        fs::create_dir(outside.path.join("target")).unwrap();
        let escape = declaration(&root.path).replace(
            "\"workspace\": \"workspace\"",
            "\"workspace\": \"../workspace\"",
        );
        assert_eq!(
            WorkflowPlan::resolve(escape.as_bytes()).unwrap_err().code,
            "workspace-escape"
        );
        let symlink = declaration(&outside.path)
            .replace("\"workspace\": \"workspace\"", "\"workspace\": \"link\"");
        std::os::unix::fs::symlink(root.path.join("workspace"), outside.path.join("link")).unwrap();
        assert_eq!(
            WorkflowPlan::resolve(symlink.as_bytes()).unwrap_err().code,
            "workspace-symlink-escape"
        );
        let absent = declaration(&root.path)
            .replace("\"workspace\": \"workspace\"", "\"workspace\": \"missing\"");
        assert_eq!(
            WorkflowPlan::resolve(absent.as_bytes()).unwrap_err().code,
            "workspace-missing"
        );
    }

    struct CountingReader {
        remaining: usize,
        copied: usize,
        reads: usize,
    }

    impl Read for CountingReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.reads += 1;
            let n = buf.len().min(self.remaining);
            for slot in buf.iter_mut().take(n) {
                *slot = b'x';
            }
            self.remaining -= n;
            self.copied += n;
            Ok(n)
        }
    }

    #[test]
    fn capped_reader_stops_at_cap_plus_one_without_a_huge_source() {
        let mut reader = CountingReader {
            remaining: usize::MAX,
            copied: 0,
            reads: 0,
        };
        let refusal = read_capped(&mut reader, 32).unwrap_err();
        assert_eq!(refusal.code, "declaration-too-large");
        assert_eq!(reader.copied, 33);
        assert!(reader.reads >= 1);
        let rendered = refusal.render();
        assert!(rendered.contains("declaration-too-large"));
        assert!(!rendered.contains("xxxx"));
    }

    #[test]
    fn declaration_file_reader_accepts_regular_files_and_refuses_oversize() {
        let (root, _) = fixture("declaration-file");
        let in_bounds = root.path.join("ok.json");
        let body = declaration(&root.path);
        fs::write(&in_bounds, &body).unwrap();
        let bytes = read_workflow_declaration(&in_bounds).unwrap();
        assert_eq!(bytes, body.as_bytes());
        let plan = WorkflowPlan::resolve(&bytes).unwrap();
        assert_eq!(plan.selector, "rust:test");

        let oversize = root.path.join("oversize.json");
        fs::write(&oversize, vec![b'y'; MAX_WORKFLOW_DECLARATION_BYTES + 1]).unwrap();
        let refusal = read_workflow_declaration(&oversize).unwrap_err();
        assert_eq!(refusal.code, "declaration-too-large");
        let rendered = refusal.render();
        assert!(rendered.contains("65536 byte ceiling"));
        assert!(!rendered.contains("yyyy"));
        assert!(!rendered.contains("oversize.json"));

        let tiny = root.path.join("tiny.bin");
        fs::write(&tiny, [b'z'; 9]).unwrap();
        let refusal = read_declaration_file(&tiny, 8).unwrap_err();
        assert_eq!(refusal.code, "declaration-too-large");
        let exact = read_declaration_file(&tiny, 9).unwrap();
        assert_eq!(exact.len(), 9);

        let missing = root.path.join("missing.json");
        assert_eq!(
            read_workflow_declaration(&missing).unwrap_err().code,
            "declaration-unreadable"
        );
    }

    #[test]
    fn declaration_file_reader_refuses_non_regular_inputs_without_blocking() {
        let root = TempRoot::new("declaration-special");
        let directory = root.path.join("dir");
        fs::create_dir(&directory).unwrap();
        assert_eq!(
            read_workflow_declaration(&directory).unwrap_err().code,
            "declaration-not-regular"
        );

        let fifo = root.path.join("fifo");
        let c_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c_path` is a NUL-terminated path in the task scratch tree.
        let status = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        assert_eq!(status, 0, "create fifo fixture");
        let fifo_for_thread = fifo.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(read_workflow_declaration(&fifo_for_thread));
        });
        let refusal = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("FIFO open must return without blocking")
            .unwrap_err();
        assert_eq!(refusal.code, "declaration-not-regular");
        assert!(!refusal.render().contains("fifo"));

        let (plan_root, _) = fixture("declaration-symlink-target");
        let target = plan_root.path.join("regular.json");
        fs::write(&target, declaration(&plan_root.path)).unwrap();
        let link = root.path.join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let bytes = read_workflow_declaration(&link).unwrap();
        assert_eq!(WorkflowPlan::resolve(&bytes).unwrap().selector, "rust:test");

        let fifo_link = root.path.join("fifo-link");
        std::os::unix::fs::symlink(&fifo, &fifo_link).unwrap();
        let fifo_link_for_thread = fifo_link.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(read_workflow_declaration(&fifo_link_for_thread));
        });
        let refusal = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("symlink-to-FIFO open must return without blocking")
            .unwrap_err();
        assert_eq!(refusal.code, "declaration-not-regular");
    }
}
