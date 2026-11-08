//! The prune capture harness (X-S1, barge-adoption.md).
//!
//! One `[[test]]` target with `harness = false`. Its argv contract is exactly
//! the contract the shared capture contract fixes:
//!
//! ```text
//! tape_prune_eval --fixture-root <dir> --manifest <path> \
//!                 --transcript-out <path> --case <CASE>
//! ```
//!
//! It follows the actual `dry_run` decision prefix through the actual
//! `evaluate` (via `prune::tape_entry::run_chain`), exports the full bounded
//! actual `Evaluation` (or the actual pre-evaluate refusal) into one
//! `oxide.capture.transcript/2` document, always writes the transcript, and
//! exits 0 when the transcript was written (any engine outcome). It never
//! calls `dry_run`, never renders a preview, and never redirects stdout.
//!
//! Run with no arguments (the `oxide test` route), it executes its embedded
//! unit fixture end to end: the pure tape checks plus the BASE,
//! EFFECT-1 (absent admission lock), EFFECT-2 (missing accounting fences),
//! REGISTRY-MISSING and UNSETTLED-INTENT unit fixtures, each a fresh
//! exclusively owned fixture and its own engine child. `--self-check` runs
//! only the pure checks. The engine child is this executable re-invoked with
//! a scrubbed environment and the fixture facts; the parent waits and reaps
//! every child it starts.
//!
#![allow(dead_code)]
// The harness crate compiles the binary's modules for the tape; the command dispatch stays in the binary.

//! Both binary and library modules are re-rooted in this crate; the self
//! alias resolves the unchanged library paths to the same test tape. The
//! fixture also uses a debug home override, whose authority is under review.
//! `HoldingSweep` is only reachable from the separate release command.

extern crate self as oxide_storage;

#[path = "../../dep_info.rs"]
pub mod dep_info;
#[path = "../../inventory.rs"]
pub mod inventory;
#[path = "../../lease/mod.rs"]
pub mod lease;
#[path = "../../owned_delete.rs"]
pub mod owned_delete;
#[cfg(debug_assertions)]
#[path = "../../proof.rs"]
pub mod proof;
#[path = "../../release_trace.rs"]
pub mod release_trace;
#[path = "../../seed_source/mod.rs"]
pub mod seed_source;
#[path = "../../supervisor.rs"]
pub mod supervisor;
#[path = "../../test_fs.rs"]
mod test_fs;
#[path = "../../volume.rs"]
pub mod volume;

#[path = "admission.rs"]
mod admission;
#[path = "adopt.rs"]
mod adopt;
#[path = "budget.rs"]
mod budget;
#[path = "capacity.rs"]
mod capacity;
#[path = "cargo_home.rs"]
mod cargo_home;
#[path = "census.rs"]
mod census;
#[path = "child_temp.rs"]
mod child_temp;
#[path = "cli.rs"]
mod cli;
#[path = "config.rs"]
mod config;
#[path = "describe.rs"]
mod describe;
#[path = "freshness.rs"]
mod freshness;
#[path = "guidance.rs"]
mod guidance;
#[path = "handoff_trace.rs"]
mod handoff_trace;
#[path = "identity.rs"]
mod identity;
#[path = "layout.rs"]
mod layout;
#[path = "one_use.rs"]
mod one_use;
#[path = "outputs.rs"]
mod outputs;
#[path = "process.rs"]
mod process;
#[path = "prune.rs"]
mod prune;
#[path = "removable.rs"]
mod removable;
#[path = "scratch_admission.rs"]
mod scratch_admission;
#[path = "scratch_io.rs"]
mod scratch_io;
#[path = "seed_catalog.rs"]
mod seed_catalog;
#[path = "seed_context.rs"]
mod seed_context;
#[path = "seed_lock.rs"]
mod seed_lock;
#[path = "seed_plan.rs"]
mod seed_plan;
#[path = "seed_sources.rs"]
mod seed_sources;
#[path = "store.rs"]
mod store;
#[path = "tape.rs"]
mod tape;
#[path = "watchdog.rs"]
mod watchdog;
#[path = "wrapper.rs"]
mod wrapper;

use std::path::{Path, PathBuf};
use std::process::Command;
use tape::{TapeEngineOutcome, TapeFixtureCapture};

/// The command name the binary crate root defines; modules reference it.
pub(crate) const CARGO: &str = "cargo";
/// The binary crate root's mebibyte constant.
pub(crate) const MIB: u64 = 1024 * 1024;

/// The command layer's holding pass. The prune chain never evicts, so this
/// harness stub starts no worker and finishes nothing.
pub(crate) struct HoldingSweep;

impl HoldingSweep {
    fn start(_pool: PathBuf, _prefer: Option<String>) -> Self {
        Self
    }

    fn finish(&mut self) {}
}

// ---------------------------------------------------------------------------
// Case definitions and the manifest
// ---------------------------------------------------------------------------

const MANIFEST_VERSION: &str = "oxide.capture.manifest/2";
const TRANSCRIPT_VERSION_CHECK: &str = "oxide.capture.transcript/2";
const ENGINE: &str = "oxide";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    Base,
    Effect1,
    Effect2,
    RegistryMissing,
    UnsettledIntent,
}

impl Case {
    fn name(self) -> &'static str {
        match self {
            Self::Base => "BASE",
            Self::Effect1 => "EFFECT-1",
            Self::Effect2 => "EFFECT-2",
            Self::RegistryMissing => "REGISTRY-MISSING",
            Self::UnsettledIntent => "UNSETTLED-INTENT",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "BASE" => Self::Base,
            "EFFECT-1" => Self::Effect1,
            "EFFECT-2" => Self::Effect2,
            "REGISTRY-MISSING" => Self::RegistryMissing,
            "UNSETTLED-INTENT" => Self::UnsettledIntent,
            _ => return None,
        })
    }

    /// Historical EFFECT names remain stable; attempts never imply completion.
    fn static_case(self) -> bool {
        matches!(
            self,
            Self::Base | Self::RegistryMissing | Self::UnsettledIntent
        )
    }
}

/// Fields consumed by the evaluator; remaining fields are preserved by the
/// digest and checked by the shared reconciliation step.
#[derive(Debug, serde::Deserialize)]
struct HarnessManifest {
    schema: String,
    case: String,
    #[serde(default)]
    operator_list: Option<String>,
}

// ---------------------------------------------------------------------------
// Fixture builder
// ---------------------------------------------------------------------------

struct FixturePaths {
    root: PathBuf,
    pool: PathBuf,
    control: PathBuf,
    outputs_dir: PathBuf,
    checkout_present: PathBuf,
    checkout_gone: PathBuf,
    root_ra: PathBuf,
    root_rb: PathBuf,
}

fn hex_const(chars: char, count: usize) -> String {
    std::iter::repeat(chars).take(count).collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One fixture root's real volume identity: the `/Volumes/<name>` mount above
/// `root` when it sits on a mounted external volume, else `None` (the
/// internal route, where external roots cannot bind).
/// Whether `root` is itself a `/Volumes/<name>` mount root, the only shape
/// where a fixture's own `oxide-outputs` can sit at the mount and the external
/// root binding can verify. The engine's `Binding::open` accepts exactly
/// `/Volumes/<name>`; a fixture under a shared volume's scratch folders binds
/// no external root, so its honest outcome is an incomplete batch with every
/// root kept. The stronger BASE surface (an absent checkout's root as an
/// actual candidate) needs the dedicated proof volume X-S2 attaches.
fn mount_root(root: &Path) -> bool {
    let mut components: Vec<std::ffi::OsString> = Vec::new();
    for component in root.components() {
        use std::path::Component;
        match component {
            Component::RootDir => {}
            Component::Normal(name) => components.push(name.to_os_string()),
            _ => return false,
        }
    }
    components.len() == 2 && components[0].as_encoded_bytes() == b"Volumes".as_slice()
}

fn fixture_volume(root: &Path) -> Option<(PathBuf, String)> {
    let mut components: Vec<std::ffi::OsString> = Vec::new();
    for component in root.components() {
        use std::path::Component;
        match component {
            Component::RootDir => {}
            Component::Normal(name) => components.push(name.to_os_string()),
            _ => return None,
        }
    }
    if components.first().map(|c| c.as_encoded_bytes()) != Some(b"Volumes".as_slice()) {
        return None;
    }
    let mount = Path::new("/Volumes").join(&components[1]);
    let facts = oxide_storage::volume::observe_path(&mount).ok()?;
    let uuid = facts.uuid?;
    Some((mount, uuid))
}

/// Write every file with owner-only mode through one helper, refusing symlinks.
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        write_private_dir(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    Ok(())
}

fn write_private_dir(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    // Recursive builders apply their mode to the leaf; every fixture level
    // must be owner-only, so each missing component is created in turn.
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        if prefix.as_os_str().is_empty() {
            continue;
        }
        if prefix.is_dir() {
            continue;
        }
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&prefix)
            .map_err(|error| error.to_string())?;
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(format!("{} is not an owner-only directory", path.display()));
    }
    Ok(())
}

/// Validate the finite JSON Schema vocabulary used by the checked-in
/// proposal. Unknown schema keywords fail closed rather than weakening it.
fn schema_matches(value: &serde_json::Value, schema: &serde_json::Value) -> bool {
    let Some(spec) = schema.as_object() else {
        return false;
    };
    const KEYS: &[&str] = &[
        "$schema",
        "$id",
        "title",
        "description",
        "type",
        "properties",
        "required",
        "additionalProperties",
        "items",
        "maxItems",
        "minimum",
        "minLength",
        "pattern",
        "const",
        "enum",
        "oneOf",
        "anyOf",
    ];
    if spec.keys().any(|key| !KEYS.contains(&key.as_str())) {
        return false;
    }
    if let Some(expected) = spec.get("const") {
        if value != expected {
            return false;
        }
    }
    if let Some(options) = spec.get("enum").and_then(|v| v.as_array()) {
        if !options.contains(value) {
            return false;
        }
    }
    for (key, exact) in [("oneOf", true), ("anyOf", false)] {
        if let Some(options) = spec.get(key).and_then(|v| v.as_array()) {
            let count = options.iter().filter(|s| schema_matches(value, s)).count();
            if (exact && count != 1) || (!exact && count == 0) {
                return false;
            }
        }
    }
    let matches_type = |kind: &str| match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    };
    if let Some(kind) = spec.get("type") {
        let valid = if let Some(name) = kind.as_str() {
            matches_type(name)
        } else if let Some(names) = kind.as_array() {
            names.iter().filter_map(|v| v.as_str()).any(matches_type)
        } else {
            false
        };
        if !valid {
            return false;
        }
    }
    if let Some(minimum) = spec.get("minimum").and_then(|v| v.as_i64()) {
        if value
            .as_i64()
            .map(|n| n < minimum)
            .unwrap_or_else(|| value.as_u64().is_none())
        {
            return false;
        }
    }
    if let Some(text) = value.as_str() {
        if spec
            .get("minLength")
            .and_then(|v| v.as_u64())
            .is_some_and(|min| text.chars().count() < min as usize)
        {
            return false;
        }
        if let Some(pattern) = spec.get("pattern") {
            let length = match pattern.as_str() {
                Some("^[0-9a-f]{40}$") => 40,
                Some("^[0-9a-f]{64}$") => 64,
                _ => return false,
            };
            if text.len() != length
                || !text
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return false;
            }
        }
    }
    if let Some(items) = value.as_array() {
        if spec
            .get("maxItems")
            .and_then(|v| v.as_u64())
            .is_some_and(|max| items.len() > max as usize)
        {
            return false;
        }
        if let Some(item_schema) = spec.get("items") {
            if !items.iter().all(|item| schema_matches(item, item_schema)) {
                return false;
            }
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = spec.get("required").and_then(|v| v.as_array()) {
            if !required
                .iter()
                .all(|key| key.as_str().is_some_and(|key| object.contains_key(key)))
            {
                return false;
            }
        }
        let properties = spec.get("properties").and_then(|v| v.as_object());
        for (key, item) in object {
            if let Some(child) = properties.and_then(|p| p.get(key)) {
                if !schema_matches(item, child) {
                    return false;
                }
            } else if let Some(additional) = spec.get("additionalProperties") {
                if additional == &serde_json::Value::Bool(false)
                    || (additional.is_object() && !schema_matches(item, additional))
                {
                    return false;
                }
            }
        }
    }
    true
}

/// Bounded manifest envelope validation. It never consumes expected outcomes
/// as evaluator inputs. Detailed reconciliation remains X-S2's oracle.
fn validate_manifest_envelope(doc: &serde_json::Value) -> Result<(), String> {
    let schema: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/knowledge/contracts/oxide-capture-manifest.v2.schema.json"
    ))
    .map_err(|error| format!("manifest schema invalid: {error}"))?;
    if !schema_matches(doc, &schema) {
        return Err("manifest schema violation".to_owned());
    }
    let allowed = [
        "schema",
        "case",
        "window",
        "source",
        "fixture",
        "dynamic",
        "mandatory_observations",
        "expected_effects",
        "expected",
        "barge",
        "operator_list",
    ];
    let object = doc.as_object().ok_or("manifest is not an object")?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err("unknown manifest field".to_owned());
    }
    for key in [
        "schema",
        "case",
        "window",
        "source",
        "fixture",
        "dynamic",
        "mandatory_observations",
        "expected_effects",
        "expected",
        "barge",
    ] {
        if doc.get(key).is_none() {
            return Err(format!("manifest missing {key}"));
        }
    }
    for (pointer, limit) in [
        ("/fixture/entries", 4096usize),
        ("/fixture/roots", 64),
        ("/dynamic/volumes", 64),
        ("/dynamic/writers", 512),
        ("/mandatory_observations", 256),
    ] {
        let rows = doc
            .pointer(pointer)
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("invalid {pointer}"))?;
        if rows.len() > limit {
            return Err(format!("{pointer} exceeds cap"));
        }
    }
    for (pointer, keys) in [
        (
            "/window",
            &["identity", "start_monotonic_ns", "end_monotonic_ns"][..],
        ),
        (
            "/source",
            &[
                "oxide_commit",
                "oxide_files",
                "cargo_lock_sha256",
                "rust_toolchain_sha256",
                "barge_pin",
                "barge_slice_commit",
                "barge_files",
                "staged_lock_sha256",
                "oxide_toolchain",
                "barge_toolchain",
                "oxide_executable_sha256",
                "barge_executable_sha256",
            ][..],
        ),
        (
            "/fixture",
            &["root", "device", "inode", "mode", "entries", "roots"][..],
        ),
        (
            "/dynamic",
            &["volumes", "writers", "external_allowances"][..],
        ),
        ("/barge", &["volume_uuid", "namespace", "area"][..]),
    ] {
        let object = doc
            .pointer(pointer)
            .and_then(|v| v.as_object())
            .ok_or_else(|| format!("invalid {pointer}"))?;
        if keys.iter().any(|key| !object.contains_key(*key)) {
            return Err(format!("missing field in {pointer}"));
        }
    }
    let window = doc
        .pointer("/window/identity")
        .and_then(|v| v.as_str())
        .ok_or("missing window identity")?;
    for volume in doc
        .pointer("/dynamic/volumes")
        .and_then(|v| v.as_array())
        .ok_or("missing volumes")?
    {
        if volume.get("window_identity").and_then(|v| v.as_str()) != Some(window) {
            return Err("volume belongs to a different window".to_owned());
        }
    }
    for row in doc["mandatory_observations"]
        .as_array()
        .ok_or("invalid obligations")?
    {
        let engine = row
            .get("engine")
            .and_then(|v| v.as_str())
            .ok_or("missing obligation engine")?;
        let site = row
            .get("site")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .ok_or("missing obligation site")?;
        let identity = row
            .get("identity")
            .and_then(|v| v.as_object())
            .ok_or("missing obligation identity")?;
        let complete = ["path", "parent_device", "parent_inode", "device", "inode"]
            .iter()
            .all(|k| identity.contains_key(*k))
            || ["component", "parent_device", "parent_inode"]
                .iter()
                .all(|k| identity.contains_key(*k))
            || ["pid", "start", "uid"]
                .iter()
                .all(|k| identity.contains_key(*k))
            || ["volume_uuid", "mount", "device"]
                .iter()
                .all(|k| identity.contains_key(*k));
        if identity.contains_key("volume_uuid")
            && identity.get("window_identity").and_then(|v| v.as_str()) != Some(window)
        {
            return Err("volume obligation belongs to a different window".to_owned());
        }
        if row
            .get("parent_required")
            .and_then(|v| v.as_bool())
            .is_none()
        {
            return Err("obligation missing parent_required".to_owned());
        }
        if !matches!(engine, "oxide" | "barge") || site.is_empty() || !complete {
            return Err("incomplete mandatory observation".to_owned());
        }
    }
    for entry in doc
        .pointer("/fixture/entries")
        .and_then(|v| v.as_array())
        .ok_or("missing entries")?
    {
        let rel = entry
            .get("rel")
            .and_then(|v| v.as_str())
            .ok_or("missing fixture relative path")?;
        if rel
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
            || !Path::new(rel)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            return Err("fixture relative path is not a plain relative path".to_owned());
        }
    }
    let bytes = doc
        .pointer("/fixture/entries")
        .and_then(|v| v.as_array())
        .ok_or("missing entries")?
        .iter()
        .try_fold(0u64, |sum, entry| {
            sum.checked_add(entry.get("size").and_then(|v| v.as_u64()).unwrap_or(0))
        })
        .ok_or("fixture byte count overflow")?;
    if bytes > 32 * 1024 * 1024 {
        return Err("fixture byte cap exceeded".to_owned());
    }
    Ok(())
}

/// Resolve the same directory policy as the engine with fixture-only inputs.
fn fixture_facts(root: &Path) -> config::Facts {
    let home = root.join("home");
    config::Facts {
        macos: cfg!(target_os = "macos"),
        home: Some(home.clone().into_os_string()),
        env_home: Some(home.clone().into_os_string()),
        home_source: config::HomeSource::Injected,
        directories: config::Directories::Injected(
            longhorn_storage_layout::PlatformEnvironment::default().with_home_dir(Some(home)),
        ),
        oxide_config: Some(root.join("config.toml").into_os_string()),
        oxide_pool: Some(root.join("scratch/pool").into_os_string()),
        oxide_pool_cap: Some("1073741824".into()),
        ..config::Facts::default()
    }
}

/// The guard/wrapper supplies TMPDIR within the selected volume's owned
/// scratch namespace. Re-derive and validate that namespace without reading
/// host pool configuration (engine children have only fixture configuration).
fn fixture_namespace() -> Result<PathBuf, String> {
    use std::os::unix::fs::MetadataExt;
    let temp = std::env::temp_dir();
    let parts: Vec<_> = temp.components().collect();
    if parts.len() < 5
        || !temp.starts_with("/Volumes")
        || parts[3].as_os_str() != "scratch"
        || !matches!(
            parts[4].as_os_str().to_str(),
            Some("oxide" | "oxide-validation")
        )
    {
        return Err("TMPDIR is outside selected-volume scratch namespaces".to_owned());
    }
    let namespace: PathBuf = parts[..5].iter().collect();
    if std::fs::canonicalize(&namespace).map_err(|e| e.to_string())? != namespace {
        return Err("scratch namespace is not canonical".to_owned());
    }
    let boot = std::fs::metadata("/").map_err(|e| e.to_string())?.dev();
    // SAFETY: getuid takes no arguments and has no caller preconditions.
    let uid = unsafe { libc::getuid() };
    for ancestor in namespace.ancestors().take(3) {
        let meta = std::fs::symlink_metadata(ancestor).map_err(|e| e.to_string())?;
        if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o022 != 0 || meta.dev() == boot {
            return Err("unsafe selected-volume scratch namespace".to_owned());
        }
    }
    Ok(namespace)
}

/// Check every existing component without following a symlink. The root must
/// be the canonical owner-only fixture; missing suffixes remain under its held
/// namespace and are created only by the fixture owner.
fn validate_fixture_path(root: &Path, path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if !root.starts_with(fixture_namespace()?)
        || std::fs::canonicalize(root).map_err(|e| e.to_string())? != root
    {
        return Err("fixture root is not canonical task scratch".to_owned());
    }
    let root_meta = std::fs::symlink_metadata(root).map_err(|e| e.to_string())?;
    if !root_meta.is_dir()
        || root_meta.uid() != unsafe { libc::getuid() }
        || root_meta.permissions().mode() & 0o077 != 0
    {
        return Err("fixture root is not owner-only".to_owned());
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "path escapes fixture")?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err("non-normal fixture path".to_owned());
        };
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(meta)
                if meta.file_type().is_symlink()
                    || meta.uid() != unsafe { libc::getuid() }
                    || meta.permissions().mode() & 0o077 != 0 =>
            {
                return Err("unsafe fixture component".to_owned())
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

fn validate_engine_environment(root: &Path) -> Result<(), String> {
    if std::env::var_os(config::TEST_HOME_FROM_ENV).as_deref() != Some(std::ffi::OsStr::new("1"))
        || std::env::var_os(config::TEST_ACCOUNT_HOME).is_some()
    {
        return Err("fixture home control missing or conflicting".to_owned());
    }
    for (name, expected) in [
        ("HOME", root.join("home")),
        ("OXIDE_CONFIG", root.join("config.toml")),
        ("OXIDE_POOL", root.join("scratch/pool")),
    ] {
        if std::env::var_os(name).as_deref() != Some(expected.as_os_str()) {
            return Err(format!("{name} is not the fixture path"));
        }
        validate_fixture_path(root, &expected)?;
    }
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy();
        if !matches!(
            name.as_ref(),
            "HOME"
                | "TMPDIR"
                | "PATH"
                | "OXIDE_TEST_HOME_FROM_ENV"
                | "OXIDE_CONFIG"
                | "OXIDE_POOL"
                | "OXIDE_POOL_CAP"
                | "OXIDE_SCRATCH_ROOT"
        ) {
            return Err(format!("unexpected environment control {name}"));
        }
    }
    if std::env::var_os("PATH").as_deref()
        != Some(std::ffi::OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin"))
    {
        return Err("fixture PATH is not the fixed system path".to_owned());
    }
    if std::env::var_os("OXIDE_POOL_CAP").as_deref() != Some(std::ffi::OsStr::new("1073741824")) {
        return Err("fixture pool cap is missing or invalid".to_owned());
    }
    let scratch =
        std::env::var_os("OXIDE_SCRATCH_ROOT").ok_or("fixture scratch root is missing")?;
    if Path::new(&scratch) != root.join("scratch") {
        return Err("scratch root is not fixture-owned".to_owned());
    }
    validate_fixture_path(root, Path::new(&scratch))?;
    let actual = config::Facts::from_env();
    let injected = fixture_facts(root);
    let actual_state = actual.state_dir()?;
    if actual_state != injected.state_dir()? {
        return Err("fixture state directory differs".to_owned());
    }
    for path in [
        actual_state.clone(),
        actual_state.join("pools.json"),
        actual_state.join("scratch-pools"),
    ] {
        validate_fixture_path(root, &path)?;
    }
    let actual_cache = config::default_pool(&actual).ok_or("fixture cache root unavailable")?;
    if Some(&actual_cache) != config::default_pool(&injected).as_ref() {
        return Err("fixture cache root differs".to_owned());
    }
    validate_fixture_path(root, &actual_cache)?;
    let settings = config::settings(&actual)?;
    if !settings.file.explicit || settings.file.path != root.join("config.toml") {
        return Err("resolved config differs from fixture".to_owned());
    }
    if settings.pool != root.join("scratch/pool") {
        return Err("resolved pool differs from fixture".to_owned());
    }
    validate_fixture_path(
        root,
        &settings.pool.join("oxide-managed/outputs/registry.json"),
    )?;
    Ok(())
}

struct FixtureSpec {
    case: Case,
    volume: Option<(PathBuf, String)>,
    ready: bool,
    admission_lock: bool,
    fenced_state: bool,
    registry: bool,
    intent: bool,
}

/// Materialize one fresh fixture instance: pool, control, registry, receipts,
/// markers and checkouts. Every path is inside `root`; nothing links outside.
fn build_fixture(root: &Path, spec: &FixtureSpec) -> Result<FixturePaths, String> {
    let paths = FixturePaths {
        root: root.to_path_buf(),
        pool: root.join("scratch/pool"),
        control: root.join("scratch/pool/oxide-managed"),
        outputs_dir: root.join("oxide-outputs"),
        checkout_present: root.join("checkouts/ra-present"),
        checkout_gone: root.join("checkouts/rb-gone"),
        root_ra: root.join("oxide-outputs/oxide-ra"),
        root_rb: root.join("oxide-outputs/oxide-rb"),
    };
    let parent = root.parent().ok_or("fixture has no parent")?;
    validate_fixture_path(parent, root)?;
    if root.exists() {
        return Err("fixture root already exists".to_owned());
    }
    write_private_dir(&paths.root)?;
    write_private_dir(&root.join("scratch"))?;
    write_private_dir(&root.join("home"))?;
    write_private_dir(&paths.pool.join("payload"))?;
    // A real budget-only pool's payload carries its slots root; an empty
    // payload root is a foreign entry the ledger refuses.
    write_private_dir(&paths.pool.join("payload/slots"))?;
    write_private_dir(&paths.control.join("outputs"))?;
    write_private_dir(&paths.checkout_present)?;
    // rb-gone: positively absent; its parent chain exists.
    write_private_dir(&paths.checkout_gone.parent().ok_or("no parent")?)?;
    write_private_dir(&paths.outputs_dir)?;
    write_private_dir(&paths.root_ra)?;
    write_private_dir(&paths.root_rb)?;

    let uuid = spec.volume.as_ref().map(|(_, uuid)| uuid.clone());
    let store_id = hex_const('1', 32);
    let checkout_id_ra = hex_const('a', 64);
    let checkout_id_rb = hex_const('b', 64);
    let checkout_id_rc = hex_const('c', 64);
    let nonce_ra = hex_const('4', 32);
    let nonce_rb = hex_const('5', 32);
    let transaction = hex_const('7', 64);
    let generation: u64 = 7;
    let created = now_ms();

    // The recorded root identity: the actual device and inode of the root
    // directory, and the exact marker bytes.
    let mut rows = Vec::new();
    let mut receipts = Vec::new();
    for (which, root_dir, checkout, checkout_id, nonce) in [
        (
            "ra",
            &paths.root_ra,
            &paths.checkout_present,
            &checkout_id_ra,
            &nonce_ra,
        ),
        (
            "rb",
            &paths.root_rb,
            &paths.checkout_gone,
            &checkout_id_rb,
            &nonce_rb,
        ),
    ] {
        let marker_bytes = marker_document(
            checkout,
            checkout_id,
            uuid.as_deref(),
            nonce,
            generation,
            created,
        );
        write_private_file(&root_dir.join(outputs::MARKER), marker_bytes.as_bytes())?;
        let meta = std::fs::symlink_metadata(root_dir).map_err(|error| error.to_string())?;
        use std::os::unix::fs::MetadataExt;
        let digest = tape::sha256_hex(marker_bytes.as_bytes());
        let receipt_path = paths.control.join(format!("outputs/{checkout_id}.json"));
        let selected = uuid.as_ref().map(|volume_uuid| {
            serde_json::json!({
                "kind": "external",
                "root": root_dir.display().to_string(),
                "volume_uuid": volume_uuid,
                "nonce": nonce,
                "generation": generation,
            })
        });
        receipts.push((
            receipt_path.clone(),
            serde_json::json!({
                "version": "oxide.outputs.transition/2",
                "checkout": checkout.display().to_string(),
                "checkout_id": checkout_id,
                "generation": generation,
                "state": "committed",
                "links": [{
                    "member": "target",
                    "selected": selected,
                }],
                "roots": [],
                "updated_ms": created,
            }),
        ));
        if which == "ra" || uuid.is_some() {
            rows.push(serde_json::json!({
                "checkout_id": checkout_id,
                "checkout": checkout.display().to_string(),
                "checkout_volume_uuid": uuid,
                "receipt": receipt_path.display().to_string(),
                "member": "target",
                "root": root_dir.display().to_string(),
                "root_kind": "external",
                "volume_uuid": uuid,
                "recipe": "dev",
                "nonce": nonce,
                "generation": generation,
                "root_dev": meta.dev(),
                "root_ino": meta.ino(),
                "marker_digest": digest,
                "selected": true,
                "pending": false,
            }));
        }
    }
    // R-C: the legacy row (no nonce, device, inode or digest), absent
    // checkout, unlisted. Its receipt carries no selected placement, so the
    // receipt-reference cross-check still holds.
    let receipt_rc = paths.control.join(format!("outputs/{checkout_id_rc}.json"));
    receipts.push((
        receipt_rc.clone(),
        serde_json::json!({
            "version": "oxide.outputs.transition/2",
            "checkout": paths.checkout_gone.join("rc").display().to_string(),
            "checkout_id": checkout_id_rc,
            "generation": 2,
            "state": "committed",
            "links": [{"member": "target"}],
            "roots": [],
            "updated_ms": created,
        }),
    ));
    if uuid.is_some() {
        rows.push(serde_json::json!({
            "checkout_id": checkout_id_rc,
            "checkout": paths.checkout_gone.join("rc").display().to_string(),
            "receipt": receipt_rc.display().to_string(),
            "member": "target",
            "root": paths.outputs_dir.join("oxide-rc").display().to_string(),
            "root_kind": "external",
            "recipe": "dev",
            "generation": 2,
            "selected": false,
            "pending": false,
        }));
    }

    // The unfinished intent for the UNSETTLED-INTENT case.
    let intents: Vec<serde_json::Value> = if spec.intent {
        vec![serde_json::json!({
            "version": "oxide.outputs.prune-intent/1",
            "id": hex_const('9', 32),
            "identity": {
                "checkout_id": checkout_id_rb,
                "checkout": paths.checkout_gone.display().to_string(),
                "receipt": paths.control.join(format!("outputs/{checkout_id_rb}.json")).display().to_string(),
                "member": "target",
                "root": paths.root_rb.display().to_string(),
                "root_kind": "external",
                "volume_uuid": uuid,
                "nonce": nonce_rb,
                "generation": generation,
                "dev": 1,
                "ino": 1,
                "marker_digest": hex_const('8', 64),
            },
            "phase": "intent",
            "quarantine": paths.outputs_dir.join(".prune-oxide-rb-9").display().to_string(),
            "preview_sha256": hex_const('6', 64),
            "created_ms": created,
            "updated_ms": created,
            "receipt_settled": false,
        })]
    } else {
        Vec::new()
    };

    let registry = serde_json::json!({
        "version": "oxide.outputs.registry/2",
        "schema": 2,
        "revision": 1,
        "complete": true,
        "created_by": transaction,
        "rows": rows,
        "intents": intents,
    });
    if spec.registry {
        write_private_file(
            &paths.control.join("outputs/registry.json"),
            &serde_json::to_vec_pretty(&registry).map_err(|error| error.to_string())?,
        )?;
    }
    for (path, document) in receipts {
        write_private_file(
            &path,
            &serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?,
        )?;
    }

    // The control state: fully fenced version 9, or the unfenced successor
    // document the EFFECT-2 pool-open path upgrades.
    let state = if spec.fenced_state {
        serde_json::json!({
            "version": 9,
            "next_id": 1,
            "next_use": 1,
            "tickets": [],
            "cleans": [],
            "records": [],
            "recoveries": [],
            "flushes": [],
            "owned_outputs": "oxide.outputs/3",
            "outputs_init": {"ready": {"transaction": transaction, "ready_ms": created}},
            "recorded_accounting": "oxide.accounting/1",
            "eviction_accounting": "oxide.eviction-accounting/1",
            "slot_ceiling": "oxide.slots/256",
            "holding": "oxide.holding/1",
        })
    } else {
        serde_json::json!({
            "version": 4,
            "next_id": 1,
            "next_use": 1,
            "tickets": [],
            "cleans": [],
            "records": [],
            "recoveries": [],
            "flushes": [],
            "owned_outputs": "oxide.outputs/3",
            "outputs_init": {"ready": {"transaction": transaction, "ready_ms": created}},
        })
    };
    if spec.ready {
        write_private_file(
            &paths.control.join("state.json"),
            &serde_json::to_vec_pretty(&state).map_err(|error| error.to_string())?,
        )?;
    }
    if spec.admission_lock {
        write_private_file(&paths.control.join("admission.lock"), b"")?;
    }

    // The pool's ledger and config (a real directory-budget pool, empty).
    let cap_bytes: u64 = 1024 * 1024 * 1024;
    write_private_file(
        &paths.pool.join("config.json"),
        serde_json::json!({
            "version": 1,
            "backend": "directory-budget-v1",
            "store_id": store_id,
            "cap_bytes": cap_bytes,
            "image_bytes": 0,
        })
        .to_string()
        .as_bytes(),
    )?;
    // The pool's flock lock file, pre-created the way `Pool::open` finds it,
    // so a prepared fixture opens without an effect.
    write_private_file(&paths.pool.join("lock"), b"")?;
    write_private_file(
        &paths.pool.join("ledger.json"),
        serde_json::json!({
            "version": 1,
            "store_id": store_id,
            "cap_bytes": cap_bytes,
            "next_slot": 1,
            "next_lease": 1,
            "slots": [],
        })
        .to_string()
        .as_bytes(),
    )?;

    // The declared-scratch documents, when the fixture sits on a mounted
    // external volume: the pool's binding and the account-state record.
    if let Some((mount, volume_uuid)) = &spec.volume {
        let scratch_root = root.join("scratch");
        let binding = serde_json::json!({
            "schema": "oxide.scratch-pool/1",
            "volume_uuid": volume_uuid,
            "mount_point": mount.display().to_string(),
            "scratch_root": scratch_root.display().to_string(),
            "pool": paths.pool.display().to_string(),
            "created_ms": created,
        });
        write_private_file(
            &paths.pool.join("oxide-managed/scratch.json"),
            &serde_json::to_vec_pretty(&binding).map_err(|error| error.to_string())?,
        )?;
        let record = serde_json::json!({
            "schema": "oxide.scratch-pool-record/1",
            "volume_uuid": volume_uuid,
            "mount_point": mount.display().to_string(),
            "scratch_root": scratch_root.display().to_string(),
            "pool": paths.pool.display().to_string(),
            "created_ms": created,
        });
        // Resolve the fixture's state using the same platform resolver with
        // injected facts; preparation must never consult the account home.
        let state_dir = fixture_facts(root).state_dir()?;
        validate_fixture_path(root, &state_dir)?;
        let hash = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(paths.pool.as_os_str().as_encoded_bytes());
            let digest = hasher.finalize();
            digest
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        let record_path = state_dir.join("scratch-pools").join(format!("{hash}.json"));
        validate_fixture_path(root, &record_path)?;
        write_private_file(
            &record_path,
            &serde_json::to_vec_pretty(&record).map_err(|error| error.to_string())?,
        )?;
    }
    Ok(paths)
}

fn marker_document(
    checkout: &Path,
    checkout_id: &str,
    volume_uuid: Option<&str>,
    nonce: &str,
    generation: u64,
    created_ms: u64,
) -> String {
    format!(
        r#"{{"version":"oxide.outputs.root/2","checkout":{},"checkout_id":{},"label":"{}","recipe":"dev","kind":"external","volume_uuid":{},"nonce":{},"checkout_volume_uuid":{},"generation":{},"created_ms":{}}}"#,
        json_string(&checkout.display().to_string()),
        json_string(checkout_id),
        json_string(
            &checkout
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        ),
        volume_uuid
            .map(|u| json_string(u))
            .unwrap_or_else(|| "null".to_owned()),
        json_string(nonce),
        volume_uuid
            .map(|u| json_string(u))
            .unwrap_or_else(|| "null".to_owned()),
        generation,
        created_ms,
    )
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned())
}

// ---------------------------------------------------------------------------
// Engine child: one case run
// ---------------------------------------------------------------------------

fn run_case(
    fixture_root: &Path,
    manifest_path: &Path,
    transcript_out: &Path,
    case: &str,
    disconnected: bool,
) -> Result<(), String> {
    validate_fixture_path(fixture_root, transcript_out)?;
    match std::fs::symlink_metadata(transcript_out) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => return Err("transcript destination already exists".to_owned()),
        Err(error) => return Err(error.to_string()),
    }
    // Configuration resolution reads real files; open the same tape before
    // preflight so those observations belong to this transcript.
    tape::begin(ENGINE, case, "", "oxide::prune::tape_entry::run_chain");
    if disconnected {
        tape::disconnect_pool_lock_for_counterfactual();
    }
    let preflight = validate_engine_environment(fixture_root);
    let read = (|| -> Result<Vec<u8>, String> {
        use std::io::Read;
        if let Err(reason) = &preflight {
            return Err(reason.clone());
        }
        validate_fixture_path(fixture_root, manifest_path)?;
        let file = std::fs::File::open(manifest_path).map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        file.take(tape::MAX_MANIFEST_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        Ok(bytes)
    })();
    let manifest_sha = read
        .as_ref()
        .ok()
        .filter(|bytes| bytes.len() as u64 <= tape::MAX_MANIFEST_BYTES)
        .map(|bytes| tape::sha256_hex(bytes))
        .unwrap_or_default();
    tape::bind_manifest_sha256(&manifest_sha);
    let parsed = (|| -> Result<(HarnessManifest, serde_json::Value), String> {
        preflight?;
        let bytes = read?;
        if bytes.len() as u64 > tape::MAX_MANIFEST_BYTES {
            return Err("manifest exceeds byte cap".to_owned());
        }
        let doc: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        validate_manifest_envelope(&doc)?;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let root_meta =
            std::fs::symlink_metadata(fixture_root).map_err(|error| error.to_string())?;
        if doc.pointer("/fixture/root").and_then(|v| v.as_str()) != fixture_root.to_str()
            || doc.pointer("/fixture/device").and_then(|v| v.as_u64()) != Some(root_meta.dev())
            || doc.pointer("/fixture/inode").and_then(|v| v.as_u64()) != Some(root_meta.ino())
            || doc.pointer("/fixture/mode").and_then(|v| v.as_u64())
                != Some(u64::from(root_meta.permissions().mode() & 0o777))
        {
            return Err("fixture identity differs from manifest".to_owned());
        }
        for effect in doc["expected_effects"]
            .as_array()
            .ok_or("invalid effects")?
        {
            let text = effect
                .get("path")
                .and_then(|value| value.as_str())
                .ok_or("missing effect path")?;
            let path = Path::new(text);
            let relative = path
                .strip_prefix(fixture_root)
                .map_err(|_| "effect path escapes fixture")?;
            if relative.as_os_str().is_empty()
                || text.split('/').any(|part| part == "." || part == "..")
                || !relative
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
            {
                return Err("effect path is not strictly inside the fixture".to_owned());
            }
        }
        let manifest: HarnessManifest =
            serde_json::from_value(doc.clone()).map_err(|error| error.to_string())?;
        if manifest.schema != MANIFEST_VERSION || manifest.case != case {
            return Err("manifest case or schema mismatch".to_owned());
        }
        Ok((manifest, doc))
    })();
    let (manifest, manifest_document) = match parsed {
        Ok(value) => value,
        Err(reason) => {
            return tape::finish_apparatus(transcript_out, case, &manifest_sha, &reason);
        }
    };
    let operator_list = manifest
        .operator_list
        .as_ref()
        .map(|rel| fixture_root.join(rel));

    if let Some(path) = operator_list.as_deref() {
        if let Err(reason) = validate_fixture_path(fixture_root, path) {
            return tape::finish_apparatus(transcript_out, case, &manifest_sha, &reason);
        }
    }
    let pre = tape::capture_fixture(fixture_root);
    let outcome = match prune::tape_entry::run_chain(operator_list.as_deref()) {
        Ok(prune::tape_entry::ChainOutcome::Evaluated { result }) => {
            TapeEngineOutcome::Completed { result }
        }
        Ok(prune::tape_entry::ChainOutcome::Refused(refusal)) => TapeEngineOutcome::Refused {
            reason: refusal.reason,
            exit_code: refusal.exit_code,
            source: "engine",
        },
        Err(reason) => TapeEngineOutcome::Refused {
            reason,
            exit_code: 1,
            source: "engine",
        },
    };
    tape::check_manifest_obligations(&manifest_document, fixture_root);
    tape::check_manifest_effects(&manifest_document);
    let mut post = tape::capture_fixture(fixture_root);
    post.pre = pre.post.clone();
    post.diff_effects = tape::diff_fixture(&pre.post, &post.post);
    let capture = TapeFixtureCapture {
        root: post.root.clone(),
        truncated: post.truncated || pre.truncated,
        pre: pre.post,
        post: post.post,
        diff_effects: post.diff_effects,
    };
    tape::check_fixture_diff(fixture_root, &capture);
    tape::finish_and_write(transcript_out, outcome, Some(capture))
}

// ---------------------------------------------------------------------------
// Transcript reading for the self-check
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct TranscriptDoc {
    version: String,
    engine: String,
    case: String,
    manifest_sha256: Option<String>,
    outcome: serde_json::Value,
    truncation: serde_json::Value,
    effects: Vec<serde_json::Value>,
    reads: Vec<serde_json::Value>,
    clocks: Vec<serde_json::Value>,
    #[serde(default)]
    recorded_holders: Vec<serde_json::Value>,
    #[serde(default)]
    classifications: Vec<serde_json::Value>,
    #[serde(default)]
    fixture_capture: Option<serde_json::Value>,
    #[serde(default)]
    notes: Vec<String>,
}

fn read_transcript(path: &Path) -> Result<TranscriptDoc, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let doc: TranscriptDoc = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    Ok(doc)
}

fn outcome_state(doc: &TranscriptDoc) -> String {
    doc.outcome
        .get("state")
        .and_then(|state| state.as_str())
        .unwrap_or_default()
        .to_owned()
}

fn outcome_reason(doc: &TranscriptDoc) -> String {
    doc.outcome
        .get("reason")
        .and_then(|reason| reason.as_str())
        .unwrap_or_default()
        .to_owned()
}

// ---------------------------------------------------------------------------
// Self-check: pure tape checks and the embedded unit fixtures
// ---------------------------------------------------------------------------

struct CheckFailure {
    what: String,
}

fn check(condition: bool, what: &str) -> Result<(), CheckFailure> {
    if condition {
        Ok(())
    } else {
        Err(CheckFailure {
            what: what.to_owned(),
        })
    }
}

/// Live held-descriptor fstat: identity matches the open file, extra
/// observation cost is classified, and a later path replacement does not
/// change the recorded inode.
fn live_held_descriptor_check(temp: &Path) -> Result<(), CheckFailure> {
    use std::os::fd::AsFd;
    use std::os::unix::fs::MetadataExt;
    let path = temp.join("held-descriptor");
    write_private_file(&path, b"held").map_err(|what| CheckFailure { what })?;
    let file = std::fs::File::open(&path).map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    let meta = file.metadata().map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    tape::begin("oxide", "LIVE-HELD", "", "pure");
    let observed = tape::fd_dev_ino(file.as_fd());
    std::fs::rename(&path, temp.join("held-original")).map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    write_private_file(&path, b"replacement").map_err(|what| CheckFailure { what })?;
    let after_replace = tape::fd_dev_ino(file.as_fd());
    let transcript = temp.join("live-held.json");
    tape::finish_and_write(
        &transcript,
        TapeEngineOutcome::Completed {
            result: serde_json::json!({}),
        },
        None,
    )
    .map_err(|what| CheckFailure { what })?;
    let doc = read_transcript(&transcript).map_err(|what| CheckFailure { what })?;
    check(
        observed == Some((meta.dev(), meta.ino())),
        "live fstat matches the held file",
    )?;
    check(
        after_replace == observed,
        "path replacement does not change the held descriptor identity",
    )?;
    check(
        doc.classifications.iter().any(|row| {
            row.get("kind").and_then(|v| v.as_str()) == Some("observe:fstat")
                && row.get("value").and_then(|v| v.as_str()) == Some("ok")
        }),
        "extra fstat cost is classified",
    )
}

/// Adapter emission round-trips through the shared parser; path-only
/// parent_required stays incomplete.
fn emitted_wire_round_trip_check(temp: &Path) -> Result<(), CheckFailure> {
    tape::begin("oxide", "WIRE-ROUND-TRIP", "", "pure");
    tape::read_row_dev(
        "prune::read_registry",
        Path::new("/fixture/registry"),
        3,
        4,
        tape::TapeOutcome::Digest {
            digest: tape::sha256_hex(b"bytes"),
        },
        Some(64),
    );
    let path = temp.join("wire-round-trip.json");
    tape::finish_and_write(
        &path,
        TapeEngineOutcome::Completed {
            result: serde_json::json!({"complete": false}),
        },
        None,
    )
    .map_err(|what| CheckFailure { what })?;
    let bytes = std::fs::read(&path).map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    let parsed = tape::parse_transcript(&bytes).map_err(|error| CheckFailure {
        what: error.message,
    })?;
    check(
        parsed.get("schema").and_then(|v| v.as_str()) == Some(tape::TRANSCRIPT_VERSION)
            && parsed.get("version").and_then(|v| v.as_str()) == Some(tape::TRANSCRIPT_VERSION),
        "emitted transcript names schema /2",
    )?;
    check(
        parsed
            .pointer("/reads/0/identity/parent_source")
            .and_then(|v| v.as_str())
            == Some("none")
            && parsed
                .pointer("/reads/0/identity/own_source")
                .and_then(|v| v.as_str())
                == Some("extracted")
            && parsed
                .pointer("/reads/0/identity/parent_device")
                .is_some_and(|v| v.is_null()),
        "path-only emission keeps parent none and unproven",
    )?;
    let manifest = serde_json::json!({
        "schema": tape::MANIFEST_VERSION,
        "case": "WIRE-ROUND-TRIP",
        "mandatory_observations": [{
            "engine": "oxide",
            "site": "prune::read_registry",
            "parent_required": true,
            "identity": {"path": "/fixture/registry"}
        }],
        "expected_effects": []
    });
    let completeness = tape::assess_completeness(&parsed, Some(&manifest));
    check(
        !completeness.ok()
            && completeness
                .issues
                .iter()
                .any(|issue| issue.contains("parent_required unproven")),
        "path-only parent_required fails completeness",
    )
}

/// Pure recorder checks: deterministic serialization, epoch and occurrence
/// bookkeeping, per-key occurrence cap, read-row cap, document bound and the
/// explicit truncation markers.
/// Emitted-shape regressions. Every row kind is written through the real
/// recorder leaves; the emitted bytes must parse as `/2`, and the same bytes
/// with one field tampered, a `/1` version, or the apparatus diagnostic must
/// be refused. Hand-written vectors alone are not emitter conformance.
fn emitted_shape_check(temp: &Path) -> Result<(), CheckFailure> {
    let io = |error: std::io::Error| CheckFailure {
        what: error.to_string(),
    };
    let fixture = temp.join("emitted-shape-fixture");
    std::fs::create_dir_all(fixture.join("nested")).map_err(io)?;
    std::fs::write(fixture.join("a.json"), b"one").map_err(io)?;
    let pre = tape::capture_fixture(&fixture);
    tape::begin("oxide", "EMITTED-SHAPE", "", "pure");
    tape::read_row(
        "config:inspect:absent",
        Path::new("/fixture/missing"),
        tape::TapeOutcome::Absent,
        None,
    );
    tape::read_row(
        "config:read-bounded",
        &fixture.join("a.json"),
        tape::TapeOutcome::Digest {
            digest: tape::sha256_hex(b"one"),
        },
        Some(0),
    );
    tape::read_row(
        "config:read-capped",
        &fixture.join("a.json"),
        tape::TapeOutcome::Digest {
            digest: tape::sha256_hex(b"one"),
        },
        Some(64),
    );
    tape::component_row(
        "root-observe:entry",
        1,
        2,
        std::ffi::OsStr::new("oxide-ra"),
        tape::TapeOutcome::Opened { dev: 3, ino: 4 },
    );
    tape::volume_row(
        "volume-site",
        Path::new("/mount"),
        Some("volume"),
        Some("/dev/example"),
        tape::TapeOutcome::Facts {
            facts: "mounted".to_owned(),
        },
    );
    tape::descriptor_row(
        "held:descriptor",
        7,
        tape::TapeOutcome::Opened { dev: 5, ino: 6 },
    );
    tape::recorded_holder("lock-holder", 9, 42, 77);
    tape::classify_row("authority-budget", "allowance", "64");
    tape::clock_row("policy-wall-ms", 1234);
    tape::clock_unproven("boot-session-ms");
    tape::effect_row("attempt@ledger", Path::new("/fixture/ledger"));
    tape::effect_row("save@ledger", Path::new("/fixture/ledger"));
    tape::unbound_effect("descriptor:fchmod", true);
    tape::unbound_effect("descriptor:rename", false);
    tape::effect_result_row(
        "save@partial-ledger",
        Path::new("/fixture/partial"),
        Some("partial"),
    );
    tape::effect_result_row(
        "save@failed-ledger",
        Path::new("/fixture/failed"),
        Some("failed"),
    );
    tape::effect_row("save@dup-ledger", Path::new("/fixture/dup"));
    tape::effect_row("save@dup-ledger", Path::new("/fixture/dup"));
    std::fs::write(fixture.join("a.json"), b"two").map_err(io)?;
    std::fs::write(fixture.join("nested").join("b.json"), b"new").map_err(io)?;
    let post = tape::capture_fixture(&fixture);
    let diff_effects = tape::diff_fixture(&pre.post, &post.post);
    let capture = TapeFixtureCapture {
        root: post.root.clone(),
        truncated: post.truncated || pre.truncated,
        pre: pre.post.clone(),
        post: post.post,
        diff_effects,
    };
    let manifest = serde_json::json!({
        "schema": tape::MANIFEST_VERSION,
        "case": "EMITTED-SHAPE",
        "mandatory_observations": [{
            "engine": "oxide",
            "site": "config:inspect:absent",
            "parent_required": false,
            "identity": {"path": "/fixture/missing"}
        }],
        "expected_effects": [{"kind": "save", "site": "ledger", "path": "/fixture/ledger", "attempted": false}]
    });
    tape::check_manifest_effects(&manifest);
    let path = temp.join("emitted-shape.json");
    tape::finish_and_write(
        &path,
        TapeEngineOutcome::Completed {
            result: serde_json::json!({"complete": false}),
        },
        Some(capture),
    )
    .map_err(|what| CheckFailure { what })?;
    let bytes = std::fs::read(&path).map_err(io)?;
    let parsed = tape::parse_transcript(&bytes).map_err(|error| CheckFailure {
        what: format!("emitted /2 transcript must parse: {}", error.message),
    })?;
    let at = |pointer: &str| {
        parsed
            .pointer(pointer)
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    };
    check(
        at("/reads/0/cap").is_null(),
        "an absent read cap is emitted as null",
    )?;
    check(
        at("/reads/1/cap") == serde_json::json!(0),
        "a real zero cap is emitted as 0",
    )?;
    check(
        at("/reads/2/cap") == serde_json::json!(64),
        "a real cap is emitted as its value",
    )?;
    check(
        at("/reads/2/identity/fd").is_null() && at("/reads/3/identity/parent_source") == "handle",
        "a component row keeps its handle parent and null descriptor",
    )?;
    check(
        at("/reads/4/identity/volume_uuid") == "volume"
            && at("/reads/4/identity/volume_device") == "/dev/example",
        "a volume row keeps its UUID and device",
    )?;
    check(
        at("/reads/5/identity/fd") == serde_json::json!(7)
            && at("/reads/5/identity/own_source") == "extracted",
        "a descriptor row keeps its fd and extracted own identity",
    )?;
    check(
        at("/recorded_holders/0/kernel_start").is_null() && at("/recorded_holders/0/uid").is_null(),
        "a recorded holder keeps its unproven kernel start and uid as null",
    )?;
    check(
        at("/clocks/0/value") == serde_json::json!(1234) && at("/clocks/1/value").is_null(),
        "an unobserved clock is null, not zero",
    )?;
    check(
        at("/effects/0/result") == "unknown" && at("/effects/0/attempted") == true,
        "an attempt is recorded as unknown, never succeeded",
    )?;
    check(
        at("/effects/1/result") == "succeeded" && at("/effects/1/attempted") == false,
        "a completed effect is recorded as succeeded",
    )?;
    check(
        at("/effects/2/kind") == "descriptor-effect"
            && at("/effects/2/path").is_null()
            && at("/effects/2/result") == "succeeded"
            && at("/effects/3/result") == "failed",
        "a descriptor-only effect keeps its observed result and no path",
    )?;
    check(
        at("/fixture_capture/post/0/device").is_u64()
            && at("/fixture_capture/post/0/dev").is_null(),
        "fixture entries use the wire identity names device and inode",
    )?;
    check(
        at("/effects/4/result") == "partial" && at("/effects/5/result") == "failed",
        "a reported partial or failed effect keeps its reported result",
    )?;
    check(
        at("/effects/6/attempted") == false && at("/effects/7/path") == "/fixture/dup",
        "repeated completed effects are emitted once per occurrence",
    )?;
    check(
        at("/fixture_capture/diff_effects/0/result") == "unknown",
        "a fixture diff never reports a writer result",
    )?;
    let completeness = tape::assess_completeness(&parsed, Some(&manifest));
    for expected in [
        "effect not completed at ledger (unknown)",
        "effect identity unproven at descriptor:fchmod",
        "effect not completed at descriptor:rename (failed)",
        "effect not completed at partial-ledger (partial)",
        "effect not completed at failed-ledger (failed)",
        "omitted or extra effects",
        "window origin and end unproven",
    ] {
        check(
            completeness.issues.iter().any(|issue| issue == expected),
            &format!("completeness names {expected}"),
        )?;
    }
    check(
        !completeness
            .issues
            .iter()
            .any(|issue| issue.contains("collision")),
        "distinct emitted reads do not collide",
    )?;

    let mut tampered: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| CheckFailure {
            what: error.to_string(),
        })?;
    tampered["reads"][1]["cap"] = serde_json::json!("0");
    let tampered_bytes = serde_json::to_vec(&tampered).map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    check(
        tape::parse_transcript(&tampered_bytes).is_err(),
        "a string cap in an emitted document is refused",
    )?;
    let mut missing: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| CheckFailure {
            what: error.to_string(),
        })?;
    missing["reads"][0]
        .as_object_mut()
        .map(|row| row.remove("cap"));
    let missing_bytes = serde_json::to_vec(&missing).map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    check(
        tape::parse_transcript(&missing_bytes).is_err(),
        "an emitted read without an explicit cap is refused",
    )?;
    let mut historical: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| CheckFailure {
            what: error.to_string(),
        })?;
    historical["schema"] = serde_json::json!("oxide.capture.transcript/1");
    historical["version"] = serde_json::json!("oxide.capture.transcript/1");
    let historical_bytes = serde_json::to_vec(&historical).map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    check(
        tape::parse_transcript(&historical_bytes)
            .err()
            .is_some_and(|error| error.message.contains("historical /1")),
        "a historical /1 document is refused by name",
    )?;
    let apparatus = temp.join("emitted-shape-apparatus.json");
    tape::begin("oxide", "EMITTED-SHAPE", "", "pure");
    tape::finish_apparatus(&apparatus, "EMITTED-SHAPE", "", "manifest refused")
        .map_err(|what| CheckFailure { what })?;
    let apparatus_bytes = std::fs::read(&apparatus).map_err(io)?;
    check(
        tape::parse_transcript(&apparatus_bytes).is_err(),
        "an apparatus diagnostic is never a conforming transcript",
    )?;
    Ok(())
}

fn pure_checks(temp: &Path) -> Result<(), CheckFailure> {
    use std::os::unix::fs::PermissionsExt;
    // The covering Rust recipe supplies a fresh child of this sibling as
    // TMPDIR. Exercise the real guard on that owned root when it does so;
    // never create an independent fixture outside the caller's temp root.
    if temp.starts_with(fixture_namespace().map_err(|what| CheckFailure { what })?) {
        check(
            validate_fixture_path(temp, &temp.join("new-entry")).is_ok(),
            "canonical private validation sibling is accepted",
        )?;
    }
    let namespace = fixture_namespace().map_err(|what| CheckFailure { what })?;
    let lookalike = namespace.with_extension("lookalike").join("not-a-fixture");
    check(
        validate_fixture_path(&lookalike, &lookalike).is_err(),
        "validation sibling lookalike is refused",
    )?;
    let insecure = temp.join("insecure-validation-root");
    write_private_dir(&insecure).map_err(|what| CheckFailure { what })?;
    std::fs::set_permissions(&insecure, std::fs::Permissions::from_mode(0o755)).map_err(
        |error| CheckFailure {
            what: error.to_string(),
        },
    )?;
    let refused = validate_fixture_path(&insecure, &insecure).is_err();
    std::fs::set_permissions(&insecure, std::fs::Permissions::from_mode(0o700)).map_err(
        |error| CheckFailure {
            what: error.to_string(),
        },
    )?;
    check(refused, "non-owner-only fixture root is refused")?;
    let private_state = fixture_facts(temp)
        .state_dir()
        .map_err(|error| CheckFailure { what: error })?;
    check(
        validate_fixture_path(temp, &private_state).is_ok(),
        "injected state stays in fixture",
    )?;
    check(
        validate_fixture_path(temp, &temp.join("../escape")).is_err(),
        "parent traversal is refused",
    )?;
    let capture_root = temp.join("capture-cap");
    write_private_dir(&capture_root).map_err(|what| CheckFailure { what })?;
    let oversized = capture_root.join("oversized");
    write_private_file(&oversized, b"").map_err(|what| CheckFailure { what })?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&oversized)
        .and_then(|file| file.set_len(32 * 1024 * 1024 + 1))
        .map_err(|error| CheckFailure {
            what: error.to_string(),
        })?;
    let capture = tape::capture_fixture(&capture_root);
    check(
        capture.truncated
            && capture
                .post
                .iter()
                .any(|entry| entry.rel == "oversized" && entry.bytes_sha256.is_none()),
        "a fixture exceeding the byte cap is explicitly truncated",
    )?;
    let link = temp.join("escape-link");
    std::os::unix::fs::symlink("/", &link).map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    check(
        validate_fixture_path(temp, &link.join("outside")).is_err(),
        "symlink escape is refused",
    )?;
    std::fs::remove_file(&link).map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    let example: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/knowledge/contracts/capture-examples/v2/manifest-example.json"
    ))
    .map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    check(
        validate_manifest_envelope(&example).is_ok(),
        "manifest example has required envelope",
    )?;
    let mut missing_parent_required = example.clone();
    missing_parent_required["mandatory_observations"][0]
        .as_object_mut()
        .expect("obligation")
        .remove("parent_required");
    check(
        validate_manifest_envelope(&missing_parent_required).is_err(),
        "an obligation requires parent_required",
    )?;
    let effect_example: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/knowledge/contracts/capture-examples/v2/manifest-effects-all-kinds.json"
    ))
    .map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    check(
        validate_manifest_envelope(&effect_example).is_ok(),
        "all fixed effect kinds are accepted",
    )?;
    let mut invalid_effect = effect_example.clone();
    invalid_effect["expected_effects"][0]["kind"] = "attempt".into();
    check(
        validate_manifest_envelope(&invalid_effect).is_err(),
        "attempt is not an effect kind",
    )?;
    let mut missing_effect_site = effect_example.clone();
    missing_effect_site["expected_effects"][0]
        .as_object_mut()
        .expect("effect object")
        .remove("site");
    check(
        validate_manifest_envelope(&missing_effect_site).is_err(),
        "effect source site is required",
    )?;
    let mut missing_attempt = effect_example.clone();
    missing_attempt["expected_effects"][0]
        .as_object_mut()
        .expect("effect object")
        .remove("attempted");
    check(
        validate_manifest_envelope(&missing_attempt).is_err(),
        "effect attempt status is required",
    )?;
    let mut missing_tree = example.clone();
    missing_tree["source"]
        .as_object_mut()
        .expect("source object")
        .remove("oxide_tree");
    check(
        validate_manifest_envelope(&missing_tree).is_err(),
        "source tree is required",
    )?;
    let mut escaped_entry = example.clone();
    escaped_entry["fixture"]["entries"][0]["rel"] = "../outside".into();
    check(
        validate_manifest_envelope(&escaped_entry).is_err(),
        "fixture entry traversal refuses",
    )?;
    let mut incomplete_unreadable = example.clone();
    incomplete_unreadable["fixture"]["entries"][2]
        .as_object_mut()
        .expect("entry")
        .remove("reason");
    check(
        validate_manifest_envelope(&incomplete_unreadable).is_err(),
        "an unreadable entry requires its actual reason",
    )?;
    let mut incomplete_directory = example.clone();
    incomplete_directory["fixture"]["entries"][3]
        .as_object_mut()
        .expect("entry")
        .remove("inode");
    check(
        validate_manifest_envelope(&incomplete_directory).is_err(),
        "a directory entry requires its observed inode",
    )?;
    let mut unreadable_file = example.clone();
    unreadable_file["fixture"]["entries"][0]["bytes_sha256"] = serde_json::Value::Null;
    check(
        validate_manifest_envelope(&unreadable_file).is_err(),
        "a present file needs a digest; unreadable files use the refusal variant",
    )?;
    let mut volume_obligation = example.clone();
    volume_obligation["mandatory_observations"][0]["identity"] = serde_json::json!({
        "volume_uuid": "example-uuid", "mount": "/example", "device": "/dev/example",
        "window_identity": "example-only"
    });
    check(
        validate_manifest_envelope(&volume_obligation).is_ok(),
        "complete volume identity is accepted",
    )?;
    volume_obligation["mandatory_observations"][0]["identity"]["device"] = 1.into();
    check(
        validate_manifest_envelope(&volume_obligation).is_err(),
        "volume device requires its string identity",
    )?;
    let mut stale_volume = example.clone();
    stale_volume["dynamic"]["volumes"][0]["window_identity"] = "other-window".into();
    check(
        validate_manifest_envelope(&stale_volume).is_err(),
        "stale volume window refuses",
    )?;
    let mut invalid_digest = example.clone();
    invalid_digest["source"]["cargo_lock_sha256"] = "not-a-digest".into();
    check(
        validate_manifest_envelope(&invalid_digest).is_err(),
        "invalid source digest refuses",
    )?;
    let mut descriptor = example.clone();
    descriptor["dynamic"]["writers"][0]["descriptor_targets"] = serde_json::json!([
        {"path": "/fixture/file", "parent_device": 1, "parent_inode": 2,
         "device": 1, "inode": 3, "fd": 4}
    ]);
    check(
        validate_manifest_envelope(&descriptor).is_ok(),
        "complete descriptor target is accepted",
    )?;
    descriptor["dynamic"]["writers"][0]["descriptor_targets"][0]
        .as_object_mut()
        .expect("descriptor target")
        .remove("inode");
    check(
        validate_manifest_envelope(&descriptor).is_err(),
        "descriptor target requires object identity",
    )?;
    let mut invalid_identity = example.clone();
    invalid_identity["mandatory_observations"][0]["identity"] = serde_json::json!({"pid": 42});
    check(
        validate_manifest_envelope(&invalid_identity).is_err(),
        "incomplete process identity refuses",
    )?;
    let mut unknown_nested = example.clone();
    unknown_nested["source"]["unknown"] = true.into();
    check(
        validate_manifest_envelope(&unknown_nested).is_err(),
        "unknown nested source field refuses",
    )?;
    let mut over_bytes = example.clone();
    over_bytes["fixture"]["entries"][0]["size"] = (32u64 * 1024 * 1024 + 1).into();
    check(
        validate_manifest_envelope(&over_bytes).is_err(),
        "manifest fixture-byte cap refuses",
    )?;
    for (pointer, limit) in [
        ("/fixture/entries", 4096usize),
        ("/fixture/roots", 64),
        ("/dynamic/volumes", 64),
        ("/dynamic/writers", 512),
        ("/mandatory_observations", 256),
    ] {
        let mut over = example.clone();
        *over.pointer_mut(pointer).ok_or_else(|| CheckFailure {
            what: format!("missing {pointer}"),
        })? = serde_json::Value::Array(vec![serde_json::Value::Null; limit + 1]);
        check(
            validate_manifest_envelope(&over).is_err(),
            "manifest collection cap refuses",
        )?;
    }
    for invalid in [
        include_str!(
            "../../../docs/knowledge/contracts/capture-examples/invalid-missing-mandatory.json"
        ),
        include_str!(
            "../../../docs/knowledge/contracts/capture-examples/invalid-missing-source.json"
        ),
        include_str!(
            "../../../docs/knowledge/contracts/capture-examples/invalid-unknown-field.json"
        ),
    ] {
        let doc: serde_json::Value =
            serde_json::from_str(invalid).map_err(|error| CheckFailure {
                what: error.to_string(),
            })?;
        check(
            validate_manifest_envelope(&doc).is_err(),
            "invalid manifest envelope refuses",
        )?;
    }
    // These assignments compile only if the unchanged oxide_storage paths
    // resolve to the same actual types as the re-rooted crate modules.
    let _: fn(&crate::volume::Binding) = |_: &oxide_storage::volume::Binding| {};
    let _: fn(&crate::inventory::InventoryLimits) =
        |_: &oxide_storage::inventory::InventoryLimits| {};
    let _: fn(&crate::lease::LedgerView) = |_: &oxide_storage::lease::LedgerView| {};
    // Each byte string is compiled from the same path used by the re-rooted
    // module declaration. The digest list is reported as a source map, not as
    // a substitute for a library call or build-artifact provenance.
    for (name, bytes) in [
        ("inventory", include_bytes!("../../inventory.rs").as_slice()),
        ("lease", include_bytes!("../../lease/mod.rs").as_slice()),
        ("pool", include_bytes!("../../lease/pool.rs").as_slice()),
        ("volume", include_bytes!("../../volume.rs").as_slice()),
    ] {
        check(!bytes.is_empty(), "re-rooted source is present")?;
        println!("source-map {name} sha256={}", tape::sha256_hex(bytes));
    }
    tape::begin("oxide", "PURE-DOCUMENT-CAP", "", "pure");
    let capped_path = temp.join("capped-transcript.json");
    tape::finish_and_write(
        &capped_path,
        TapeEngineOutcome::Completed {
            result: serde_json::json!({"oversized": "x".repeat(tape::MAX_DOCUMENT_BYTES + 1)}),
        },
        None,
    )
    .map_err(|error| CheckFailure { what: error })?;
    let capped = std::fs::read(&capped_path).map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    let capped_doc: serde_json::Value =
        serde_json::from_slice(&capped).map_err(|error| CheckFailure {
            what: error.to_string(),
        })?;
    check(
        capped.len() <= tape::MAX_DOCUMENT_BYTES
            && capped_doc
                .pointer("/truncation/document_truncated")
                .and_then(|v| v.as_bool())
                == Some(true),
        "serialized document cap is explicit",
    )?;
    let observed_path = temp.join("observed-input");
    write_private_file(&observed_path, b"original")
        .map_err(|error| CheckFailure { what: error })?;
    let observed_file = std::fs::File::open(&observed_path).map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    use std::os::unix::fs::MetadataExt;
    let observed_identity = observed_file
        .metadata()
        .map_err(|error| CheckFailure {
            what: error.to_string(),
        })?
        .ino();
    std::fs::rename(&observed_path, temp.join("observed-original")).map_err(|error| {
        CheckFailure {
            what: error.to_string(),
        }
    })?;
    write_private_file(&observed_path, b"replacement")
        .map_err(|error| CheckFailure { what: error })?;
    tape::begin("oxide", "PURE-DESCRIPTOR", "", "pure");
    let mut observed_bytes = Vec::new();
    prune::authority_read_observed(
        &observed_file,
        &observed_path,
        "prune::read_bounded",
        64,
        &mut observed_bytes,
    )
    .map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    let descriptor_path = temp.join("descriptor-transcript.json");
    tape::finish_and_write(
        &descriptor_path,
        TapeEngineOutcome::Completed {
            result: serde_json::json!({}),
        },
        None,
    )
    .map_err(|error| CheckFailure { what: error })?;
    let descriptor_doc =
        read_transcript(&descriptor_path).map_err(|error| CheckFailure { what: error })?;
    check(
        observed_bytes == b"original"
            && descriptor_doc.reads.iter().any(|row| {
                row.get("kind").and_then(|v| v.as_str())
                    == Some("authority-read@prune::read_bounded")
                    && row.pointer("/identity/inode").and_then(|v| v.as_u64())
                        == Some(observed_identity)
                    && row
                        .pointer("/identity/parent_device")
                        .is_some_and(|v| v.is_null())
                    && row
                        .pointer("/identity/parent_inode")
                        .is_some_and(|v| v.is_null())
            }),
        "descriptor binding survives path replacement",
    )?;
    check(
        descriptor_doc.reads.len() == 2,
        "one descriptor observation binds the open and the authority read",
    )?;
    check(
        descriptor_doc
            .notes
            .iter()
            .any(|note| note.contains("parent identity unproven")),
        "unobserved descriptor parent remains explicitly unproven",
    )?;
    let build = |digest: String| -> Vec<u8> {
        tape::begin("oxide", "PURE", "", "pure");
        for occurrence in 0..3 {
            tape::read_row_dev(
                "pure:read",
                Path::new("/pure/object"),
                3,
                4,
                tape::TapeOutcome::Digest {
                    digest: format!("{digest}-{occurrence}"),
                },
                Some(128),
            );
        }
        tape::effect_row("save@pure", Path::new("/pure/state.json"));
        tape::clock_row("pure:clock-ms", 1_234);
        tape::classify_row("pure:classify", "one", "value");
        tape::budget_begin(1_024);
        let path = temp.join("pure.json");
        let outcome = TapeEngineOutcome::Completed {
            result: serde_json::json!({"complete": true, "entries": []}),
        };
        if let Err(error) = tape::finish_and_write(&path, outcome, None) {
            panic!("pure tape window did not close: {error}");
        }
        std::fs::read(&path).unwrap_or_default()
    };
    let first = build(tape::sha256_hex(b"first"));
    let second = build(tape::sha256_hex(b"first"));
    let strip_windows = |bytes: &[u8]| -> String {
        let mut doc: serde_json::Value =
            serde_json::from_slice(bytes).expect("pure transcript parses");
        if let Some(object) = doc.as_object_mut() {
            object.remove("window_monotonic_start_ns");
            object.remove("window_monotonic_end_ns");
            object.remove("window_elapsed_ns");
        }
        serde_json::to_string(&doc).expect("normalized transcript serializes")
    };
    check(
        strip_windows(&first) == strip_windows(&second),
        "the same facts serialize to byte-identical transcripts (field order and rows)",
    )?;
    let doc: TranscriptDoc = serde_json::from_slice(&first).map_err(|error| CheckFailure {
        what: format!("the pure transcript does not parse: {error}"),
    })?;
    check(
        doc.version == TRANSCRIPT_VERSION_CHECK,
        "the transcript carries the fixed schema version",
    )?;
    check(doc.reads.len() == 3, "every recorded read row is carried")?;
    check(
        doc.reads
            .iter()
            .all(|row| row.get("occurrence").is_some() && row.get("epoch").is_some()),
        "read rows carry epoch and occurrence",
    )?;
    let occurrences: Vec<u64> = doc
        .reads
        .iter()
        .filter_map(|row| row.get("occurrence").and_then(|v| v.as_u64()))
        .collect();
    check(
        occurrences == vec![1, 2, 3],
        "occurrences count per identity key in order",
    )?;
    check(
        doc.effects.len() == 1
            && doc.effects[0].get("kind").and_then(|k| k.as_str()) == Some("save")
            && doc.effects[0].get("site").and_then(|k| k.as_str()) == Some("pure")
            && doc.effects[0].get("attempted").and_then(|v| v.as_bool()) == Some(false),
        "the effect row carries its kind",
    )?;
    check(
        doc.clocks.len() == 1,
        "the actual clock row is carried without a fabricated deadline",
    )?;
    check(
        doc.classifications.len() == 2,
        "the classification rows are carried (one site row, one budget row)",
    )?;
    check(
        outcome_state(&doc) == "completed",
        "the outcome is the engine's own completed document",
    )?;
    check(
        doc.notes
            .iter()
            .any(|note| note.contains("parent identity unproven")),
        "the missing parent remains explicitly unproven",
    )?;
    Ok(())
}

fn finish_pure(path: PathBuf) -> bool {
    let outcome = TapeEngineOutcome::Completed {
        result: serde_json::json!({"complete": true, "entries": []}),
    };
    tape::finish_and_write(&path, outcome, None).is_ok()
}

/// The occurrence cap: the sixty-fifth occurrence of one key marks the
/// transcript's occurrence truncation instead of dropping the row.
fn occurrence_cap_check(temp: &Path) -> Result<(), CheckFailure> {
    tape::begin("oxide", "PURE-CAP", "", "pure");
    for _ in 0..(tape::MAX_OCCURRENCES_PER_KEY + 2) {
        tape::read_row_dev(
            "cap:read",
            Path::new("/cap/one"),
            9,
            9,
            tape::TapeOutcome::Digest {
                digest: "digest".to_owned(),
            },
            Some(1),
        );
    }
    let outcome = TapeEngineOutcome::Completed {
        result: serde_json::json!({"complete": false, "entries": []}),
    };
    let path = temp.join("cap.json");
    tape::finish_and_write(&path, outcome, None).map_err(|error| CheckFailure { what: error })?;
    let doc = read_transcript(&path).map_err(|error| CheckFailure { what: error })?;
    let truncated = doc
        .truncation
        .get("occurrences_truncated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    check(truncated, "the occurrence cap records truncation")?;
    check(
        doc.reads.len() == tape::MAX_OCCURRENCES_PER_KEY,
        "the per-key occurrence cap holds the fixed bound and marks the excess",
    )?;
    Ok(())
}

/// The read-row cap: past 8192 rows the document says so.
fn read_row_cap_check(temp: &Path) -> Result<(), CheckFailure> {
    tape::begin("oxide", "PURE-ROWS", "", "pure");
    for index in 0..(tape::MAX_READ_ROWS + 4) {
        let path = PathBuf::from(format!("/rows/{index}"));
        tape::read_row(
            "rows:read",
            &path,
            tape::TapeOutcome::Digest {
                digest: "d".to_owned(),
            },
            Some(1),
        );
    }
    let outcome = TapeEngineOutcome::Completed {
        result: serde_json::json!({"complete": false, "entries": []}),
    };
    let path = temp.join("rows.json");
    tape::finish_and_write(&path, outcome, None).map_err(|error| CheckFailure { what: error })?;
    let doc = read_transcript(&path).map_err(|error| CheckFailure { what: error })?;
    let truncated = doc
        .truncation
        .get("read_rows_truncated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    check(truncated, "the read-row cap records truncation")?;
    check(
        doc.reads.len() == tape::MAX_READ_ROWS,
        "the read-row cap holds the fixed bound",
    )
}

/// The unbound authority read: a pending read left at window close is an
/// explicit note, never silence.
fn volume_window_reconciliation_check(temp: &Path) -> Result<(), CheckFailure> {
    let manifest = serde_json::json!({
        "window": {"identity": "current"}, "fixture": {"entries": []},
        "dynamic": {"volumes": [{"uuid": "volume", "mount": "/mount", "device": "/dev/example", "window_identity": "current"}]},
        "mandatory_observations": [{"engine": "oxide", "site": "volume-site", "parent_required": false, "identity": {
            "volume_uuid": "volume", "mount": "/mount", "device": "/dev/example", "window_identity": "current"
        }}]
    });
    tape::begin("oxide", "VOLUME-WINDOW", "", "pure");
    tape::volume_row(
        "volume-site",
        Path::new("/mount"),
        Some("volume"),
        Some("/dev/example"),
        tape::TapeOutcome::Facts {
            facts: "observed".to_owned(),
        },
    );
    tape::check_manifest_obligations(&manifest, temp);
    let path = temp.join("volume-window.json");
    tape::finish_and_write(
        &path,
        TapeEngineOutcome::Completed {
            result: serde_json::json!({"complete": false, "entries": []}),
        },
        None,
    )
    .map_err(|what| CheckFailure { what })?;
    let doc = read_transcript(&path).map_err(|what| CheckFailure { what })?;
    check(
        !doc.notes.iter().any(|note| {
            note.contains("mandatory observation missing")
                || note.contains("extra or unbound observation")
        }),
        "a volume identity binds within its declared window",
    )
}

fn effect_reconciliation_check(temp: &Path) -> Result<(), CheckFailure> {
    let manifest = serde_json::json!({"expected_effects": [
        {"kind": "fence", "site": "declared", "path": "/fixture/state", "attempted": false}
    ]});
    tape::begin("oxide", "EFFECT-MISSING-EXTRA", "", "pure");
    tape::effect_row("save@unexpected", Path::new("/fixture/other"));
    tape::check_manifest_effects(&manifest);
    let transcript = temp.join("effect-missing-extra.json");
    tape::finish_and_write(
        &transcript,
        TapeEngineOutcome::Completed {
            result: serde_json::json!({"complete": false, "entries": []}),
        },
        None,
    )
    .map_err(|what| CheckFailure { what })?;
    let doc = read_transcript(&transcript).map_err(|what| CheckFailure { what })?;
    check(
        doc.notes
            .iter()
            .any(|note| note.contains("substituted effects")),
        "a declared effect not matched by the observed one is explicit",
    )?;
    check(
        doc.notes
            .iter()
            .any(|note| note.contains("substituted effects")),
        "an unmatched observed effect is explicit",
    )
}

fn obligation_reconciliation_check(temp: &Path) -> Result<(), CheckFailure> {
    let manifest = serde_json::json!({
        "window": {"identity": "pure-window"},
        "fixture": {"entries": [{"rel": "entry", "kind": "file",
            "parent_device": 1, "parent_inode": 2, "device": 3, "inode": 4}]},
        "dynamic": {"volumes": [], "external_allowances": []},
        "mandatory_observations": [{"engine": "oxide", "site": "component-site",
            "parent_required": false,
            "identity": {"component": "entry", "parent_device": 1, "parent_inode": 2}}]
    });
    tape::begin("oxide", "OBLIGATION-PRESENT", "", "pure");
    tape::component_row(
        "component-site",
        1,
        2,
        std::ffi::OsStr::new("entry"),
        tape::TapeOutcome::Opened { dev: 3, ino: 4 },
    );
    tape::check_manifest_obligations(&manifest, temp);
    let present_path = temp.join("obligation-present.json");
    tape::finish_and_write(
        &present_path,
        TapeEngineOutcome::Completed {
            result: serde_json::json!({"complete": false, "entries": []}),
        },
        None,
    )
    .map_err(|what| CheckFailure { what })?;
    let present = read_transcript(&present_path).map_err(|what| CheckFailure { what })?;
    check(
        !present.notes.iter().any(|note| {
            note.contains("mandatory observation missing")
                || note.contains("extra or unbound observation")
        }),
        "a fully bound component satisfies its obligation",
    )?;

    tape::begin("oxide", "OBLIGATION-MISSING-EXTRA", "", "pure");
    tape::component_row(
        "unexpected-site",
        7,
        8,
        std::ffi::OsStr::new("other"),
        tape::TapeOutcome::Opened { dev: 9, ino: 10 },
    );
    tape::check_manifest_obligations(&manifest, temp);
    let missing_path = temp.join("obligation-missing-extra.json");
    tape::finish_and_write(
        &missing_path,
        TapeEngineOutcome::Completed {
            result: serde_json::json!({"complete": false, "entries": []}),
        },
        None,
    )
    .map_err(|what| CheckFailure { what })?;
    let missing = read_transcript(&missing_path).map_err(|what| CheckFailure { what })?;
    check(
        missing
            .notes
            .iter()
            .any(|note| note.contains("mandatory observation missing")),
        "a missing mandatory component is insufficient",
    )?;
    check(
        missing
            .notes
            .iter()
            .any(|note| note.contains("extra or unbound observation")),
        "an extra component is insufficient",
    )
}

fn recorded_holder_cap_check(temp: &Path) -> Result<(), CheckFailure> {
    tape::begin("oxide", "HOLDER-CAP", "", "pure");
    for id in 0..=tape::MAX_PROCESS_ROWS {
        tape::recorded_holder("pure-holder", id as u64, id as u32, 42);
    }
    let path = temp.join("holder-cap.json");
    tape::finish_and_write(
        &path,
        TapeEngineOutcome::Completed {
            result: serde_json::json!({"complete": false, "entries": []}),
        },
        None,
    )
    .map_err(|what| CheckFailure { what })?;
    let doc = read_transcript(&path).map_err(|what| CheckFailure { what })?;
    check(
        doc.truncation
            .get("process_rows_truncated")
            .and_then(|v| v.as_bool())
            == Some(true),
        "recorded holder cap is explicit",
    )?;
    check(
        doc.recorded_holders.len() == tape::MAX_PROCESS_ROWS,
        "recorded holder rows remain bounded",
    )?;
    check(
        doc.recorded_holders[0]["kernel_start"].is_null()
            && doc.recorded_holders[0]["uid"].is_null(),
        "recorded wall start is not a kernel identity",
    )
}

fn unique_key_cap_check(temp: &Path) -> Result<(), CheckFailure> {
    tape::begin("oxide", "UNIQUE-KEY-CAP", "", "pure");
    for index in 0..=tape::MAX_READ_ROWS * 2 {
        tape::read_row(
            "unique-key-cap",
            &temp.join(index.to_string()),
            tape::TapeOutcome::Absent,
            None,
        );
    }
    let transcript = temp.join("unique-key-cap.json");
    tape::finish_and_write(
        &transcript,
        TapeEngineOutcome::Completed {
            result: serde_json::json!({"complete": false, "entries": []}),
        },
        None,
    )
    .map_err(|what| CheckFailure { what })?;
    let doc = read_transcript(&transcript).map_err(|what| CheckFailure { what })?;
    check(
        doc.truncation
            .get("occurrences_truncated")
            .and_then(|v| v.as_bool())
            == Some(true),
        "the bounded distinct-key table reports truncation",
    )
}

fn effect_attempt_check(temp: &Path) -> Result<(), CheckFailure> {
    tape::begin("oxide", "EFFECT-ATTEMPT", "", "pure");
    {
        let _fence = tape::effect_kind_override("fence@expected");
        tape::effect_row("attempt@temporary-open", Path::new("/fixture/temp"));
        tape::effect_row("attempt-save@temporary-write", Path::new("/fixture/temp"));
        tape::effect_row(
            "attempt-rename@temporary-move",
            Path::new("/fixture/destination"),
        );
        tape::effect_row(
            "attempt-unlink@temporary-cleanup",
            Path::new("/fixture/temp"),
        );
        tape::effect_row("save@control-state", Path::new("/fixture/state"));
    }
    let transcript = temp.join("effect-attempt.json");
    tape::finish_and_write(
        &transcript,
        TapeEngineOutcome::Completed {
            result: serde_json::json!({"complete": false, "entries": []}),
        },
        None,
    )
    .map_err(|what| CheckFailure { what })?;
    let doc = read_transcript(&transcript).map_err(|what| CheckFailure { what })?;
    check(
        doc.effects.len() == 5,
        "all attempted and completed effects are retained",
    )?;
    check(
        doc.effects[0]["kind"] == "create" && doc.effects[0]["attempted"] == true,
        "uncertain create stays an attempt",
    )?;
    check(
        doc.effects[1]["kind"] == "save" && doc.effects[1]["attempted"] == true,
        "failed write stays an attempt",
    )?;
    check(
        doc.effects[2]["kind"] == "rename" && doc.effects[2]["attempted"] == true,
        "failed rename stays an attempt",
    )?;
    check(
        doc.effects[3]["kind"] == "unlink" && doc.effects[3]["attempted"] == true,
        "failed cleanup stays an attempt",
    )?;
    check(
        doc.effects[4]["kind"] == "fence" && doc.effects[4]["site"] == "expected",
        "temporary attempts do not consume the final fence override",
    )?;
    check(
        doc.notes
            .iter()
            .any(|note| note.contains("attempted effect") && note.contains("insufficient")),
        "attempts cannot be silently treated as parity",
    )
}

fn config_inspect_absence_check(temp: &Path) -> Result<(), CheckFailure> {
    let missing = temp.join("missing-config-inspect-pool");
    tape::begin("oxide", "CONFIG-INSPECT-ABSENT", "", "pure");
    let found = config::inspect(&missing);
    let result = TapeEngineOutcome::Completed {
        result: serde_json::json!({"complete": false, "entries": []}),
    };
    let transcript = temp.join("config-inspect-absent.json");
    tape::finish_and_write(&transcript, result, None).map_err(|what| CheckFailure { what })?;
    let doc = read_transcript(&transcript).map_err(|what| CheckFailure { what })?;
    check(
        found == config::Found::Absent,
        "the actual missing pool is absent",
    )?;
    check(
        doc.reads.iter().any(|row| {
            row.get("kind").and_then(|value| value.as_str()) == Some("config:inspect:initial")
                && row
                    .pointer("/outcome/type")
                    .and_then(|value| value.as_str())
                    == Some("absent")
        }),
        "the actual missing metadata result is taped",
    )
}

fn unbound_authority_note_check(temp: &Path) -> Result<(), CheckFailure> {
    tape::begin("oxide", "PURE-UNBOUND", "", "pure");
    tape::authority_core(b"bytes", 64, false, false);
    let outcome = TapeEngineOutcome::Completed {
        result: serde_json::json!({"complete": false, "entries": []}),
    };
    let path = temp.join("unbound.json");
    tape::finish_and_write(&path, outcome, None).map_err(|error| CheckFailure { what: error })?;
    let doc = read_transcript(&path).map_err(|error| CheckFailure { what: error })?;
    check(
        doc.notes
            .iter()
            .any(|note| note.contains("without its caller binding its identity")),
        "an unbound authority read is an explicit note",
    )?;
    tape::begin("oxide", "PURE-BOUND", "", "pure");
    tape::authority_core(b"bytes", 64, false, false);
    tape::authority_bind("pure::site", Path::new("/bound/file"), 64);
    let outcome = TapeEngineOutcome::Completed {
        result: serde_json::json!({"complete": false, "entries": []}),
    };
    tape::finish_and_write(&temp.join("bound.json"), outcome, None)
        .map_err(|error| CheckFailure { what: error })?;
    let doc =
        read_transcript(&temp.join("bound.json")).map_err(|error| CheckFailure { what: error })?;
    check(
        doc.reads.iter().any(|row| {
            row.get("kind").and_then(|k| k.as_str()) == Some("authority-read@pure::site")
                && row
                    .pointer("/identity/device")
                    .is_some_and(|value| value.is_null())
                && row
                    .pointer("/identity/parent_device")
                    .is_some_and(|value| value.is_null())
        }),
        "an unobserved descriptor and parent are not invented",
    )?;
    check(
        doc.notes
            .iter()
            .any(|note| note.contains("identity unproven")),
        "an unobserved identity explicitly prevents proof",
    )
}

fn has_read(rows: &[serde_json::Value], kind: &str) -> bool {
    rows.iter()
        .any(|row| row.get("kind").and_then(|value| value.as_str()) == Some(kind))
}

fn fixture_case(temp: &Path, case: Case, disconnected: bool) -> Result<(), CheckFailure> {
    let spec = FixtureSpec {
        case,
        volume: fixture_volume(temp),
        ready: true,
        admission_lock: !matches!(case, Case::Effect1),
        fenced_state: !matches!(case, Case::Effect2),
        registry: !matches!(case, Case::RegistryMissing),
        intent: matches!(case, Case::UnsettledIntent),
    };
    let root = temp.join(format!(
        "fixture-{}{}",
        case.name().to_lowercase(),
        if disconnected { "-disconnected" } else { "" }
    ));
    let paths = build_fixture(&root, &spec).map_err(|error| CheckFailure { what: error })?;
    // Schema-example data is a unit-test envelope, not source or fixture
    // qualification evidence. X-S2 supplies observed manifest facts.
    let mut manifest: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/knowledge/contracts/capture-examples/v2/manifest-example.json"
    ))
    .map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    manifest["case"] = serde_json::Value::String(case.name().to_owned());
    manifest["fixture"]["root"] = serde_json::Value::String(root.display().to_string());
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let root_meta = std::fs::symlink_metadata(&root).map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    manifest["fixture"]["device"] = root_meta.dev().into();
    manifest["fixture"]["inode"] = root_meta.ino().into();
    manifest["fixture"]["mode"] = u64::from(root_meta.permissions().mode() & 0o777).into();
    let manifest_path = root.join("manifest.json");
    write_private_file(
        &manifest_path,
        &serde_json::to_vec_pretty(&manifest).map_err(|error| CheckFailure {
            what: error.to_string(),
        })?,
    )
    .map_err(|error| CheckFailure { what: error })?;
    let transcript_path = root.join("transcript.json");

    // The engine child: this executable, one case, scrubbed fixture env.
    let exe = std::env::current_exe().map_err(|error| CheckFailure {
        what: error.to_string(),
    })?;
    let mut envs: Vec<(String, String)> = vec![
        ("HOME".to_owned(), root.join("home").display().to_string()),
        ("OXIDE_TEST_HOME_FROM_ENV".to_owned(), "1".to_owned()),
        (
            "OXIDE_CONFIG".to_owned(),
            root.join("config.toml").display().to_string(),
        ),
        ("OXIDE_POOL".to_owned(), paths.pool.display().to_string()),
        ("OXIDE_POOL_CAP".to_owned(), "1073741824".to_owned()),
    ];
    if spec.volume.is_some() {
        envs.push((
            "OXIDE_SCRATCH_ROOT".to_owned(),
            root.join("scratch").display().to_string(),
        ));
    }
    let mut command = Command::new(&exe);
    if disconnected {
        command.arg("--counterfactual-disconnected");
    }
    let output = command
        .arg("--fixture-root")
        .arg(&root)
        .arg("--manifest")
        .arg(&manifest_path)
        .arg("--transcript-out")
        .arg(&transcript_path)
        .arg("--case")
        .arg(case.name())
        .env_clear()
        .envs(envs)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("TMPDIR", std::env::temp_dir())
        .output()
        .map_err(|error| CheckFailure {
            what: format!("cannot run the engine child: {error}"),
        })?;
    // The child was waited for above (`output()`); it is reaped by the OS
    // through that wait. This proves only that the direct child was reaped;
    // it does not establish closure of any descendant the child may start.
    check(
        output.status.success(),
        &format!(
            "the {} engine child exits 0 ({}):\n{}",
            case.name(),
            output.status,
            String::from_utf8_lossy(&output.stderr),
        ),
    )?;
    let doc = read_transcript(&transcript_path).map_err(|error| CheckFailure { what: error })?;
    check(
        doc.version == TRANSCRIPT_VERSION_CHECK && doc.engine == ENGINE && doc.case == case.name(),
        "the transcript binds engine, case and schema",
    )?;
    let manifest_sha =
        tape::sha256_hex(
            &std::fs::read(&manifest_path).map_err(|error| CheckFailure {
                what: error.to_string(),
            })?,
        );
    check(
        doc.manifest_sha256.as_deref() == Some(manifest_sha.as_str()),
        "the transcript binds the manifest digest",
    )?;
    check(
        doc.clocks.iter().any(|clock| {
            clock.get("kind").and_then(|value| value.as_str())
                == Some("authority:begin:window-offset-ns")
                && clock
                    .get("value")
                    .and_then(|value| value.as_u64())
                    .is_some()
        }),
        "the actual authority start has a monotonic offset from this window",
    )?;
    let diff_effects: Vec<serde_json::Value> = doc
        .fixture_capture
        .as_ref()
        .and_then(|capture| capture.get("diff_effects"))
        .and_then(|effects| effects.as_array())
        .cloned()
        .unwrap_or_default();

    if matches!(case, Case::Base | Case::Effect1 | Case::Effect2) {
        let expected: &[(&str, &str, &str, &str)] = match case {
            Case::Base => &[],
            Case::Effect1 => &[],
            Case::Effect2 => &[(
                "fence",
                "managed-control",
                "save",
                "scratch/pool/oxide-managed/state.json",
            )],
            _ => unreachable!(),
        };
        let transient_count = if case == Case::Effect2 { 2 } else { 0 };
        let completed_effects = doc
            .effects
            .iter()
            .filter(|effect| {
                effect.get("attempted").and_then(|value| value.as_bool()) == Some(false)
            })
            .count();
        check(
            completed_effects == expected.len() + transient_count
                && diff_effects.len() == expected.len(),
            "completed transient and persistent effects match their declared counts",
        )?;
        check(
            doc.effects.iter().any(|effect| {
                effect.get("site").and_then(|value| value.as_str()) == Some("pool-lock-open")
                    && effect.get("attempted").and_then(|value| value.as_bool()) == Some(true)
            }),
            "the O_CREAT pool-lock open remains an explicitly uncertain attempt",
        )?;
        if case == Case::Effect2 {
            for (kind, site, attempted) in [
                ("create", "managed-admission-open", true),
                ("create", "managed-control-temp", false),
                ("save", "managed-control-temp", false),
            ] {
                check(
                    doc.effects
                        .iter()
                        .filter(|effect| {
                            effect.get("kind").and_then(|value| value.as_str()) == Some(kind)
                                && effect.get("site").and_then(|value| value.as_str()) == Some(site)
                                && effect.get("attempted").and_then(|value| value.as_bool())
                                    == Some(attempted)
                        })
                        .count()
                        == 1,
                    "the reached temporary or uncertain effect occurs exactly once",
                )?;
            }
        }
        for (taped, site, diff, rel) in expected {
            check(
                doc.effects.iter().any(|effect| {
                    effect.get("kind").and_then(|v| v.as_str()) == Some(*taped)
                        && effect.get("site").and_then(|v| v.as_str()) == Some(*site)
                        && effect.get("attempted").and_then(|v| v.as_bool()) == Some(false)
                        && effect.get("path").and_then(|v| v.as_str()) == root.join(rel).to_str()
                }),
                "declared effect appears on tape",
            )?;
            check(
                diff_effects.iter().any(|effect| {
                    effect.get("kind").and_then(|v| v.as_str()) == Some(*diff)
                        && effect.get("path").and_then(|v| v.as_str()) == Some(*rel)
                }),
                "declared effect appears in fixture diff",
            )?;
        }
    }

    match case {
        Case::Base => {
            check(
                outcome_state(&doc) == "completed",
                &format!("BASE completes: {}", outcome_reason(&doc)),
            )?;
            let complete = doc
                .outcome
                .get("result")
                .and_then(|result| result.get("complete"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let entries = doc
                .outcome
                .get("result")
                .and_then(|result| result.get("entries"))
                .and_then(|entries| entries.as_array())
                .cloned()
                .unwrap_or_default();
            check(entries.len() == 3, "BASE evaluates all three roots")?;
            let candidate_roots: Vec<String> = entries
                .iter()
                .filter(|entry| entry.get("candidate").map_or(false, |c| !c.is_null()))
                .filter_map(|entry| entry.get("root").and_then(|r| r.as_str()))
                .map(str::to_owned)
                .collect();
            let kept_roots: Vec<String> = entries
                .iter()
                .filter(|entry| entry.get("kept").map_or(false, |k| !k.is_null()))
                .filter_map(|entry| entry.get("root").and_then(|r| r.as_str()))
                .map(str::to_owned)
                .collect();
            if mount_root(&root) {
                // The dedicated proof-volume route: the fixture's own
                // oxide-outputs sits at the mount, so the external roots bind
                // and the real per-root fates are exercised end to end.
                check(complete, "BASE evaluates a complete snapshot")?;
                check(
                    kept_roots.iter().any(|root| root.ends_with("oxide-ra")),
                    "the present checkout's root is kept",
                )?;
                check(
                    kept_roots.iter().any(|root| root.ends_with("oxide-rc")),
                    "the unlisted legacy root is kept",
                )?;
                check(
                    candidate_roots
                        .iter()
                        .any(|root| root.ends_with("oxide-rb")),
                    "the positively absent checkout's root is a candidate",
                )?;
            } else {
                // A fixture under a shared volume's scratch (or the internal
                // fallback) binds no external root: the honest outcome is an
                // incomplete batch with every root kept, recorded with the
                // engine's own reason.
                check(
                    !complete && candidate_roots.is_empty() && kept_roots.len() == 3,
                    "without a dedicated volume mount the batch is kept and incomplete",
                )?;
                check(
                    entries.iter().all(|entry| {
                        entry
                            .get("kept")
                            .and_then(|k| k.as_str())
                            .map(|kept| kept.contains("the registry snapshot is incomplete"))
                            .unwrap_or(false)
                    }),
                    "every kept entry carries the engine's own incomplete-snapshot reason",
                )?;
            }
            check(
                has_read(&doc.reads, "pool:lock") != disconnected,
                "the reached library pool lock distinguishes the connected sink",
            )?;
            if disconnected {
                check(
                    doc.notes.iter().any(|note| {
                        note.contains("counterfactual: pool lock") && note.contains("insufficient")
                    }),
                    "the actual disconnected engine run is explicitly insufficient",
                )?;
            }
            check(
                has_read(&doc.reads, "pool:entries:read-dir-open")
                    && has_read(&doc.reads, "pool:entries:end")
                    && has_read(&doc.reads, "pool:payload-entries:read-dir-open")
                    && has_read(&doc.reads, "pool:payload-entries:end"),
                "the actual pool and payload enumeration opens and ends reach the tape",
            )?;
            check(
                doc.effects.iter().all(|effect| effect.get("attempted").and_then(|v| v.as_bool()) == Some(true))
                    && diff_effects.is_empty(),
                &format!(
                    "BASE has no confirmed effect; uncertain attempts remain insufficient (tape {:?}, diff {:?})",
                    doc.effects, diff_effects
                ),
            )?;
            check(
                doc.reads.iter().any(|row| {
                    row.get("kind").and_then(|k| k.as_str())
                        == Some("authority-read@prune::read_registry")
                }),
                "the registry read is bound",
            )?;
            check(
                doc.reads.iter().any(|row| {
                    row.get("kind").and_then(|k| k.as_str()) == Some("pool:ledger-view")
                }),
                "the pool open is recorded",
            )?;
        }
        Case::Effect1 => {
            check(
                doc.effects
                    .iter()
                    .all(|effect| effect.get("attempted").and_then(|v| v.as_bool()) == Some(true))
                    && diff_effects.is_empty(),
                "missing-lock custody has no completed effect or fixture diff",
            )?;
            // The evaluator completes with a conservative batch refusal, not
            // a chain error: every root carries the missing-lock reason.
            let result = doc.outcome.get("result");
            let entries = result
                .and_then(|result| result.get("entries"))
                .and_then(|entries| entries.as_array());
            check(
                outcome_state(&doc) == "completed"
                    && result.and_then(|result| result.get("complete"))
                        .and_then(|value| value.as_bool()) == Some(false)
                    && entries.is_some_and(|entries| entries.len() == 3 && entries.iter().all(|entry| {
                        entry.get("candidate").is_some_and(|value| value.is_null())
                            && entry.get("kept").and_then(|value| value.as_str()).is_some_and(|reason| {
                                reason.contains("writer custody could not be witnessed (the internal pool's admission lock is missing)")
                            })
                    })),
                "the missing admission lock refuses custody for the whole incomplete batch",
            )?;
            check(
                !paths.control.join("admission.lock").exists(),
                "the missing admission lock remains absent",
            )?;
        }
        Case::Effect2 => {
            let state_changed = diff_effects.iter().any(|effect| {
                effect.get("path").and_then(|p| p.as_str())
                    == Some("scratch/pool/oxide-managed/state.json")
            });
            check(state_changed, "the missing fences are durably saved")?;
            let lock_untouched = !diff_effects.iter().any(|effect| {
                effect.get("path").and_then(|p| p.as_str())
                    == Some("scratch/pool/oxide-managed/admission.lock")
            });
            check(lock_untouched, "the present lock is not recreated")?;
        }
        Case::RegistryMissing => {
            check(
                outcome_state(&doc) == "refused",
                "a missing registry is the engine's own refusal",
            )?;
            check(
                outcome_reason(&doc).contains("is missing"),
                &format!(
                    "the refusal names the missing registry: {}",
                    outcome_reason(&doc)
                ),
            )?;
            let exit = doc
                .outcome
                .get("exit_code")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            check(
                exit == 2,
                "the registry refusal carries the engine's exit code",
            )?;
            check(
                doc.outcome.get("source").and_then(|v| v.as_str()) == Some("engine"),
                "the registry refusal is attributed to the engine",
            )?;
            check(
                doc.effects
                    .iter()
                    .all(|effect| effect.get("attempted").and_then(|v| v.as_bool()) == Some(true))
                    && diff_effects.is_empty(),
                "a pre-evaluate refusal has no confirmed effect; attempts remain insufficient",
            )?;
        }
        Case::UnsettledIntent => {
            check(
                outcome_state(&doc) == "refused",
                "an unfinished intent refuses before evaluation",
            )?;
            check(
                outcome_reason(&doc).contains("unfinished"),
                "the refusal names the unfinished intent",
            )?;
            let exit = doc
                .outcome
                .get("exit_code")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            check(exit == 3, "the unsettled refusal carries exit 3")?;
            check(
                doc.effects
                    .iter()
                    .all(|effect| effect.get("attempted").and_then(|v| v.as_bool()) == Some(true))
                    && diff_effects.is_empty(),
                "the unsettled refusal has no confirmed effect; attempts remain insufficient",
            )?;
        }
    }
    Ok(())
}

fn self_check(pure_only: bool, diagnostic: bool) -> Result<(), String> {
    let base = if diagnostic {
        PathBuf::from(
            std::env::var_os("OXIDE_PROOF_EVIDENCE")
                .ok_or("diagnostic requires the guard evidence directory")?,
        )
    } else {
        std::env::temp_dir()
    };
    let dir = base.join(format!(
        "tape-prune-self-check-{}-{}",
        std::process::id(),
        now_ms()
    ));
    write_private_dir(&dir)
        .map_err(|error| format!("cannot create the self-check root: {error}"))?;
    let result = (|| -> Result<(), String> {
        tape::run_self_checks().map_err(|what| format!("capture-wire: {what}"))?;
        live_held_descriptor_check(&dir).map_err(|failure| failure.what)?;
        emitted_wire_round_trip_check(&dir).map_err(|failure| failure.what)?;
        emitted_shape_check(&dir).map_err(|failure| failure.what)?;
        pure_checks(&dir).map_err(|failure| failure.what)?;
        tape::capture_verdict::run_checks()
            .map_err(|what| format!("limited-verdict policy: {what}"))?;
        occurrence_cap_check(&dir).map_err(|failure| failure.what)?;
        read_row_cap_check(&dir).map_err(|failure| failure.what)?;
        unbound_authority_note_check(&dir).map_err(|failure| failure.what)?;
        config_inspect_absence_check(&dir).map_err(|failure| failure.what)?;
        effect_attempt_check(&dir).map_err(|failure| failure.what)?;
        unique_key_cap_check(&dir).map_err(|failure| failure.what)?;
        recorded_holder_cap_check(&dir).map_err(|failure| failure.what)?;
        obligation_reconciliation_check(&dir).map_err(|failure| failure.what)?;
        effect_reconciliation_check(&dir).map_err(|failure| failure.what)?;
        volume_window_reconciliation_check(&dir).map_err(|failure| failure.what)?;
        if !pure_only {
            let mut failures = Vec::new();
            for case in [
                Case::RegistryMissing,
                Case::UnsettledIntent,
                Case::Base,
                Case::Effect1,
                Case::Effect2,
            ] {
                let result = fixture_case(&dir, case, false);
                println!(
                    "oxide: embedded {}: {}",
                    case.name(),
                    if result.is_ok() { "passed" } else { "failed" }
                );
                if let Err(failure) = result {
                    if !diagnostic {
                        return Err(failure.what);
                    }
                    failures.push(format!("{}: {}", case.name(), failure.what));
                }
            }
            let result = fixture_case(&dir, Case::Base, true);
            println!(
                "oxide: embedded BASE disconnected: {}",
                if result.is_ok() { "passed" } else { "failed" }
            );
            if let Err(failure) = result {
                failures.push(format!("BASE disconnected: {}", failure.what));
            }
            if diagnostic {
                return Err(format!(
                    "uncounted diagnostic; qualified=false; failures={failures:?}"
                ));
            }
            if !failures.is_empty() {
                return Err(failures.join("; "));
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!(
            "oxide: the self-check root is kept for inspection: {}",
            dir.display()
        );
        return Err(format!("self-check failed: {error}"));
    }
    std::fs::remove_dir_all(&dir)
        .map_err(|error| format!("cannot remove the self-check root: {error}"))?;
    println!("oxide: prune capture harness self-check passed");
    Ok(())
}

fn usage() -> String {
    "usage: tape_prune_eval [--self-check | --diagnostic | --limited-plan] [--fixture-root <dir> --manifest <path> --transcript-out <path> --case <CASE>]".to_owned()
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let disconnected = args
        .first()
        .is_some_and(|arg| arg == "--counterfactual-disconnected");
    if disconnected {
        args.remove(0);
        if args.first().map(String::as_str) != Some("--fixture-root") {
            eprintln!("oxide: disconnected counterfactual requires the fixture entry");
            std::process::exit(2);
        }
    }
    if args.is_empty() {
        let code = match self_check(false, false) {
            Ok(()) => 0,
            Err(reason) => {
                eprintln!("oxide: prune capture harness: {reason}");
                1
            }
        };
        std::process::exit(code);
    }
    match args.first().map(String::as_str) {
        // The effect-free first-batch plan: prints one document, writes and
        // runs nothing (no fixture, engine, staging, Cargo, image or probe).
        Some("--limited-plan") if args.len() == 1 => {
            match serde_json::to_string_pretty(&tape::capture_verdict::plan_document()) {
                Ok(text) => {
                    println!("{text}");
                    std::process::exit(0);
                }
                Err(error) => {
                    eprintln!("oxide: limited plan: {error}");
                    std::process::exit(2);
                }
            }
        }
        Some("--path-preflight") => {
            let result = (|| -> Result<(), String> {
                if args.len() != 2 {
                    return Err("path preflight needs one root".to_owned());
                }
                if !cfg!(debug_assertions) {
                    return Err("path preflight requires the debug cfg".to_owned());
                }
                let root = Path::new(&args[1]);
                validate_engine_environment(root)?;
                let facts = config::Facts::from_env();
                let settings = config::settings(&facts)?;
                let state = facts.state_dir()?;
                let cache = config::default_pool(&facts).ok_or("cache unavailable")?;
                println!("debug_assertions=true config={} state={} state_registry={} scratch_records={} cache={} pool={} output_registry={}",
                    settings.file.path.display(), state.display(), state.join("pools.json").display(),
                    state.join("scratch-pools").display(), cache.display(), settings.pool.display(),
                    settings.pool.join("oxide-managed/outputs/registry.json").display());
                println!(
                    "config_source_sha256={}",
                    tape::sha256_hex(include_bytes!("config.rs"))
                );
                Ok(())
            })();
            match result {
                Ok(()) => std::process::exit(0),
                Err(reason) => {
                    eprintln!("oxide: path preflight: {reason}");
                    std::process::exit(2);
                }
            }
        }
        Some("--diagnostic") if args.len() == 1 => {
            let reason = self_check(false, true)
                .err()
                .unwrap_or_else(|| "diagnostic unexpectedly passed".to_owned());
            eprintln!("oxide: prune capture harness: {reason}");
            std::process::exit(1);
        }
        Some("--self-check") => {
            let code = match self_check(true, false) {
                Ok(()) => 0,
                Err(reason) => {
                    eprintln!("oxide: prune capture harness: {reason}");
                    1
                }
            };
            std::process::exit(code);
        }
        Some("--fixture-root") => {
            let parse = || -> Result<(PathBuf, PathBuf, PathBuf, String), String> {
                let mut fixture = None;
                let mut manifest = None;
                let mut transcript = None;
                let mut case = None;
                let mut index = 0;
                while index < args.len() {
                    let value = args
                        .get(index + 1)
                        .ok_or_else(|| format!("{} needs a value", args[index]))?;
                    match args[index].as_str() {
                        "--fixture-root" => fixture = Some(PathBuf::from(value)),
                        "--manifest" => manifest = Some(PathBuf::from(value)),
                        "--transcript-out" => transcript = Some(PathBuf::from(value)),
                        "--case" => case = Some(value.to_owned()),
                        other => return Err(format!("unknown argument {other}; {}", usage())),
                    }
                    index += 2;
                }
                match (fixture, manifest, transcript, case) {
                    (Some(fixture), Some(manifest), Some(transcript), Some(case)) => {
                        Ok((fixture, manifest, transcript, case))
                    }
                    _ => Err(usage()),
                }
            };
            let parsed = parse();
            let code = match parsed {
                Ok((fixture_root, manifest, transcript_out, case)) => {
                    match run_case(
                        &fixture_root,
                        &manifest,
                        &transcript_out,
                        &case,
                        disconnected,
                    ) {
                        Ok(()) => {
                            println!(
                                "oxide: prune capture harness wrote {} for {}",
                                transcript_out.display(),
                                case
                            );
                            0
                        }
                        Err(reason) => {
                            eprintln!("oxide: prune capture harness: {reason}");
                            2
                        }
                    }
                }
                Err(reason) => {
                    eprintln!("oxide: prune capture harness: {reason}");
                    2
                }
            };
            std::process::exit(code);
        }
        _ => {
            eprintln!("oxide: prune capture harness: {}", usage());
            std::process::exit(2);
        }
    }
}
