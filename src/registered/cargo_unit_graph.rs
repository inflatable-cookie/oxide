//! Pure decoder and supplied-provenance verifier for pinned Cargo unit-graph v1.
//!
//! This module parses caller-supplied bytes only. It never invokes Cargo, reads
//! project or tool inputs, authenticates an issuer, or creates an accepted
//! `TrustedCargoGraphContext`.

use super::cargo_freshness::{CARGO_SOURCE_COMMIT, CARGO_VERSION};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path};

/// Maximum size of each graph or provenance JSON payload and framed source set.
pub const MAX_GRAPH_BYTES: usize = 8 * 1024 * 1024;
/// Maximum decoded Cargo units and registered roots accepted by the default parser.
pub const MAX_GRAPH_UNITS: usize = 256;
/// Maximum dependency edges or identity-map entries accepted by the default parser.
pub const MAX_GRAPH_ASSOCIATIONS: usize = 32_768;
/// Maximum source or registered-root path size in UTF-8 bytes.
pub const MAX_GRAPH_PATH_BYTES: usize = 2_048;
/// Maximum nested JSON value or path-component depth accepted by the default parser.
pub const MAX_GRAPH_DEPTH: usize = 32;

/// Provenance JSON schema accepted by [`verify_provenance`].
pub const CARGO_UNIT_GRAPH_PROVENANCE_SCHEMA: &str = "oxide-cargo-unit-graph-provenance-v1";
/// Provenance record version accepted by [`verify_provenance`].
pub const CARGO_UNIT_GRAPH_PROVENANCE_VERSION: u32 = 1;

/// Per-call ceilings. No value may exceed the repository's current graph/map
/// and collection limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CargoUnitGraphLimits {
    /// Maximum size of each supplied JSON payload and framed source set.
    pub bytes: usize,
    /// Maximum number of serialized units, roots, or registered roots.
    pub units: usize,
    /// Maximum total dependency edges or entries in an identity map.
    pub associations: usize,
    /// Maximum UTF-8 byte length of a path or JSON object key.
    pub path_bytes: usize,
    /// Maximum JSON nesting or path-component depth.
    pub depth: usize,
}

impl Default for CargoUnitGraphLimits {
    fn default() -> Self {
        Self {
            bytes: MAX_GRAPH_BYTES,
            units: MAX_GRAPH_UNITS,
            associations: MAX_GRAPH_ASSOCIATIONS,
            path_bytes: MAX_GRAPH_PATH_BYTES,
            depth: MAX_GRAPH_DEPTH,
        }
    }
}

impl CargoUnitGraphLimits {
    fn validate(self) -> Result<(), CargoUnitGraphRefusal> {
        if self.bytes > MAX_GRAPH_BYTES
            || self.units > MAX_GRAPH_UNITS
            || self.associations > MAX_GRAPH_ASSOCIATIONS
            || self.path_bytes > MAX_GRAPH_PATH_BYTES
            || self.depth > MAX_GRAPH_DEPTH
        {
            return Err(CargoUnitGraphRefusal::BudgetExceeded);
        }
        Ok(())
    }
}

/// Specific refusals from the bounded decoder and provenance verifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CargoUnitGraphRefusal {
    MalformedJson,
    DuplicateKey,
    TrailingBytes,
    UnknownField,
    MissingField,
    InvalidType,
    InvalidValue,
    UnsupportedVersion,
    UnsupportedUnit,
    UnsupportedEdge,
    InvalidIndex,
    InvalidSourcePath,
    UnregisteredSourcePath,
    BudgetExceeded,
    PinnedToolMismatch,
    GraphDigestMismatch,
    LockChanged,
    ProvenanceMismatch,
    InvalidAccounting,
}

impl fmt::Display for CargoUnitGraphRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}", self)
    }
}

impl std::error::Error for CargoUnitGraphRefusal {}

/// Decoded Cargo graph. Debug output intentionally omits package and source paths.
#[derive(Clone, Eq, PartialEq)]
pub struct CargoUnitGraph {
    pub version: u32,
    pub units: Vec<CargoUnit>,
    pub roots: Vec<usize>,
}

impl fmt::Debug for CargoUnitGraph {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CargoUnitGraph")
            .field("version", &self.version)
            .field("unit_count", &self.units.len())
            .field("root_count", &self.roots.len())
            .finish()
    }
}

/// One serialized Cargo unit. Profile is retained as its complete checked v1 JSON value.
#[derive(Clone, Eq, PartialEq)]
pub struct CargoUnit {
    pub package_id: String,
    pub target: CargoUnitTarget,
    pub profile: Value,
    /// `None` is Cargo's serialized host compile kind; `Some` is a target triple.
    pub platform: Option<String>,
    pub mode: CargoUnitMode,
    pub features: Vec<String>,
    /// Cargo omits false; this field therefore always has the normalized value.
    pub is_std: bool,
    pub dependencies: Vec<CargoUnitEdge>,
}

impl fmt::Debug for CargoUnit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CargoUnit")
            .field("target", &self.target)
            .field(
                "platform",
                &self.platform.as_ref().map(|_| "<target platform>"),
            )
            .field("mode", &self.mode)
            .field("feature_count", &self.features.len())
            .field("is_std", &self.is_std)
            .field("dependency_count", &self.dependencies.len())
            .finish_non_exhaustive()
    }
}

/// Cargo's complete serialized target summary.
#[derive(Clone, Eq, PartialEq)]
pub struct CargoUnitTarget {
    pub kind: Vec<String>,
    pub crate_types: Vec<String>,
    pub name: String,
    pub src_path: String,
    pub edition: String,
    /// Cargo omits this key only when target `required_features` is `None`.
    pub required_features: Option<Vec<String>>,
    pub doc: bool,
    pub doctest: bool,
    pub test: bool,
}

impl fmt::Debug for CargoUnitTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CargoUnitTarget")
            .field("kind", &self.kind)
            .field("crate_types", &self.crate_types)
            .field("name", &self.name)
            .field("src_path", &"<registered source path>")
            .field("edition", &self.edition)
            .field(
                "required_feature_count",
                &self.required_features.as_ref().map(Vec::len),
            )
            .field("doc", &self.doc)
            .field("doctest", &self.doctest)
            .field("test", &self.test)
            .finish()
    }
}

/// Serialized per-unit Cargo compile mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CargoUnitMode {
    Test,
    Build,
    Check,
    Doc,
    Doctest,
    Docscrape,
    RunCustomBuild,
}

/// Dependency edge retains Cargo's index, extern name and all serialized nightly fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CargoUnitEdge {
    pub index: usize,
    pub extern_crate_name: String,
    /// `None` preserves the stable serializer's omission; a supplied bool is retained.
    pub public: Option<bool>,
    /// This value is accepted only when absent or false.
    pub noprelude: Option<bool>,
    /// This value is accepted only when absent or false.
    pub nounused: Option<bool>,
}

/// Decode pinned Cargo unit-graph v1 bytes against constructor-declared source roots.
///
/// Roots are lexical absolute directories supplied by the caller. This pure
/// operation performs no filesystem inspection and accepts a target source only
/// when it is a strict descendant of one of those roots.
pub fn decode_unit_graph(
    bytes: &[u8],
    registered_roots: &[&Path],
    limits: CargoUnitGraphLimits,
) -> Result<CargoUnitGraph, CargoUnitGraphRefusal> {
    limits.validate()?;
    if registered_roots.len() > limits.units {
        return Err(CargoUnitGraphRefusal::BudgetExceeded);
    }
    for root in registered_roots {
        validate_absolute_path(
            root.to_str()
                .ok_or(CargoUnitGraphRefusal::InvalidSourcePath)?,
            limits,
        )?;
    }
    let value = parse_bounded_json(bytes, limits)?;
    let graph = object(&value)?;
    check_fields(graph, &["version", "units", "roots"], &[])?;
    let version = unsigned(field(graph, "version")?)?;
    if version != 1 {
        return Err(CargoUnitGraphRefusal::UnsupportedVersion);
    }
    let raw_units = array(field(graph, "units")?)?;
    let raw_roots = array(field(graph, "roots")?)?;
    if raw_units.len() > limits.units || raw_roots.len() > limits.units {
        return Err(CargoUnitGraphRefusal::BudgetExceeded);
    }
    let mut units = Vec::with_capacity(raw_units.len());
    for value in raw_units {
        units.push(parse_unit(value, registered_roots, limits)?);
    }
    let mut roots = Vec::with_capacity(raw_roots.len());
    for root in raw_roots {
        roots.push(index(root, units.len())?);
    }
    for unit in &units {
        for edge in &unit.dependencies {
            if edge.index >= units.len() {
                return Err(CargoUnitGraphRefusal::InvalidIndex);
            }
        }
    }
    Ok(CargoUnitGraph {
        version: 1,
        units,
        roots,
    })
}

/// Redacted supplied provenance record. It contains identity digests and run
/// accounting, never environment or credential values or actual private paths.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct CargoUnitGraphBindings {
    pub cargo_version: String,
    pub cargo_source_commit: String,
    pub graph_digest: String,
    pub rustc_version: String,
    pub rustc_verbose_version_digest: String,
    pub host_triple: String,
    /// Digest identities of installed tool receipts; `cargo` and `rustc` are required keys.
    pub tool_receipt_identities: BTreeMap<String, String>,
    pub source_git_commit: String,
    pub source_git_tree: String,
    /// Framed digest of the source path/content-digest set, separate from Git OIDs.
    pub source_content_digest: String,
    pub lock_digest_before: String,
    pub lock_digest_after: String,
    pub manifest_set_digest: String,
    pub config_set_digest: String,
    pub argv_digest: String,
    pub selector_digest: String,
    pub profile_digest: String,
    pub features_digest: String,
    pub target_digest: String,
    /// Digest identity includes environment names and presence/absence, not secret values.
    pub environment_digest: String,
    /// Digests for private paths such as CARGO_HOME, build, target and temp directories.
    pub private_path_identities: BTreeMap<String, String>,
    pub run: CargoUnitGraphRunAccounting,
}

/// Supplied run identity, invocation accounting and monotonic clock accounting.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct CargoUnitGraphRunAccounting {
    pub task_id: String,
    pub workspace_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub invocation_count: u32,
    pub invocation_limit: u32,
    pub clock_start_ms: u64,
    pub clock_end_ms: u64,
    pub clock_elapsed_ms: u64,
    pub clock_limit_ms: u64,
}

/// Compare supplied provenance bytes to independently constructor-supplied bindings.
///
/// `expected` must be assembled by the embedding owner from its own run context,
/// not copied from the untrusted record. Passing this check proves internal
/// correspondence only; it does not authenticate a producer or prove complete inputs.
pub fn verify_provenance(
    graph_bytes: &[u8],
    provenance_bytes: &[u8],
    expected: &CargoUnitGraphBindings,
    limits: CargoUnitGraphLimits,
) -> Result<(), CargoUnitGraphRefusal> {
    limits.validate()?;
    if graph_bytes.len() > limits.bytes {
        return Err(CargoUnitGraphRefusal::BudgetExceeded);
    }
    validate_bindings(expected, limits)?;
    let graph_digest = digest(graph_bytes);
    let value = parse_bounded_json(provenance_bytes, limits)?;
    let record = parse_provenance(&value, limits.associations)?;
    validate_bindings(&record, limits)?;
    if record.cargo_version != CARGO_VERSION || record.cargo_source_commit != CARGO_SOURCE_COMMIT {
        return Err(CargoUnitGraphRefusal::PinnedToolMismatch);
    }
    if record.graph_digest != graph_digest || expected.graph_digest != graph_digest {
        return Err(CargoUnitGraphRefusal::GraphDigestMismatch);
    }
    if record.lock_digest_before != record.lock_digest_after
        || record.lock_digest_before != expected.lock_digest_before
        || record.lock_digest_after != expected.lock_digest_after
    {
        return Err(CargoUnitGraphRefusal::LockChanged);
    }
    if &record != expected {
        return Err(CargoUnitGraphRefusal::ProvenanceMismatch);
    }
    Ok(())
}

/// Compute Oxide's separate framed source-content identity from source-relative
/// names and `sha256:` file digests. The map's byte ordering is UTF-8 lexical order.
pub fn framed_source_content_digest(
    files: &BTreeMap<String, String>,
    limits: CargoUnitGraphLimits,
) -> Result<String, CargoUnitGraphRefusal> {
    limits.validate()?;
    if files.len() > limits.associations {
        return Err(CargoUnitGraphRefusal::BudgetExceeded);
    }
    let mut bytes = 0usize;
    let mut hasher = Sha256::new();
    for (name, file_digest) in files {
        validate_relative_path(name, limits)?;
        validate_digest(file_digest)?;
        bytes = bytes
            .checked_add(16)
            .and_then(|size| size.checked_add(name.len()))
            .and_then(|size| size.checked_add(file_digest.len()))
            .ok_or(CargoUnitGraphRefusal::BudgetExceeded)?;
        if bytes > limits.bytes {
            return Err(CargoUnitGraphRefusal::BudgetExceeded);
        }
        update_frame(&mut hasher, name.as_bytes())?;
        update_frame(&mut hasher, file_digest.as_bytes())?;
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn parse_unit(
    value: &Value,
    registered_roots: &[&Path],
    limits: CargoUnitGraphLimits,
) -> Result<CargoUnit, CargoUnitGraphRefusal> {
    let unit = object(value)?;
    check_fields(
        unit,
        &[
            "pkg_id",
            "target",
            "profile",
            "platform",
            "mode",
            "features",
            "dependencies",
        ],
        &["is_std"],
    )?;
    let package_id = nonempty_string(field(unit, "pkg_id")?)?;
    let target = parse_target(field(unit, "target")?, registered_roots, limits)?;
    let profile = field(unit, "profile")?.clone();
    validate_profile(&profile)?;
    let platform = match field(unit, "platform")? {
        Value::Null => None,
        Value::String(value) if !value.is_empty() && !value.as_bytes().contains(&0) => {
            Some(value.clone())
        }
        _ => return Err(CargoUnitGraphRefusal::InvalidType),
    };
    let mode = match nonempty_string(field(unit, "mode")?)? {
        "test" => CargoUnitMode::Test,
        "build" => CargoUnitMode::Build,
        "check" => CargoUnitMode::Check,
        "doc" => CargoUnitMode::Doc,
        "doctest" => CargoUnitMode::Doctest,
        "docscrape" => CargoUnitMode::Docscrape,
        "run-custom-build" => CargoUnitMode::RunCustomBuild,
        _ => return Err(CargoUnitGraphRefusal::UnsupportedUnit),
    };
    let features = string_array(field(unit, "features")?)?;
    let is_std = optional_bool(unit, "is_std")?.unwrap_or(false);
    if is_std {
        return Err(CargoUnitGraphRefusal::UnsupportedUnit);
    }
    let dependencies = array(field(unit, "dependencies")?)?
        .iter()
        .map(parse_edge)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CargoUnit {
        package_id: package_id.to_owned(),
        target,
        profile,
        platform,
        mode,
        features,
        is_std,
        dependencies,
    })
}

fn parse_target(
    value: &Value,
    registered_roots: &[&Path],
    limits: CargoUnitGraphLimits,
) -> Result<CargoUnitTarget, CargoUnitGraphRefusal> {
    let target = object(value)?;
    check_fields(
        target,
        &[
            "kind",
            "crate_types",
            "name",
            "src_path",
            "edition",
            "doc",
            "doctest",
            "test",
        ],
        &["required-features"],
    )?;
    let kind = string_array(field(target, "kind")?)?;
    let crate_types = string_array(field(target, "crate_types")?)?;
    if kind.is_empty() || crate_types.is_empty() {
        return Err(CargoUnitGraphRefusal::UnsupportedUnit);
    }
    const KNOWN_TARGET_KINDS: &[&str] = &[
        "bin",
        "lib",
        "rlib",
        "dylib",
        "cdylib",
        "staticlib",
        "proc-macro",
        "example",
        "test",
        "bench",
        "custom-build",
    ];
    const KNOWN_CRATE_TYPES: &[&str] = &[
        "bin",
        "lib",
        "rlib",
        "dylib",
        "cdylib",
        "staticlib",
        "proc-macro",
    ];
    if kind
        .iter()
        .any(|value| !KNOWN_TARGET_KINDS.contains(&value.as_str()))
        || crate_types
            .iter()
            .any(|value| !KNOWN_CRATE_TYPES.contains(&value.as_str()))
    {
        return Err(CargoUnitGraphRefusal::UnsupportedUnit);
    }
    let name = nonempty_string(field(target, "name")?)?;
    let src_path = match field(target, "src_path")? {
        Value::String(path) => path,
        Value::Null => return Err(CargoUnitGraphRefusal::UnregisteredSourcePath),
        _ => return Err(CargoUnitGraphRefusal::InvalidType),
    };
    validate_absolute_path(src_path, limits)?;
    let path = Path::new(src_path);
    let registered = registered_roots.iter().any(|root| {
        path.strip_prefix(root)
            .is_ok_and(|relative| !relative.as_os_str().is_empty())
    });
    if !registered {
        return Err(CargoUnitGraphRefusal::UnregisteredSourcePath);
    }
    let edition = nonempty_string(field(target, "edition")?)?;
    if !matches!(edition, "2015" | "2018" | "2021" | "2024") {
        return Err(CargoUnitGraphRefusal::UnsupportedUnit);
    }
    let required_features = target
        .get("required-features")
        .map(string_array)
        .transpose()?;
    Ok(CargoUnitTarget {
        kind,
        crate_types,
        name: name.to_owned(),
        src_path: src_path.clone(),
        edition: edition.to_owned(),
        required_features,
        doc: required_bool(target, "doc")?,
        doctest: required_bool(target, "doctest")?,
        test: required_bool(target, "test")?,
    })
}

fn parse_edge(value: &Value) -> Result<CargoUnitEdge, CargoUnitGraphRefusal> {
    let edge = object(value)?;
    check_fields(
        edge,
        &["index", "extern_crate_name"],
        &["public", "noprelude", "nounused"],
    )?;
    let index = index_number(field(edge, "index")?)?;
    let extern_crate_name = nonempty_string(field(edge, "extern_crate_name")?)?.to_owned();
    let public = optional_bool(edge, "public")?;
    let noprelude = optional_bool(edge, "noprelude")?;
    let nounused = optional_bool(edge, "nounused")?;
    if noprelude == Some(true) || nounused == Some(true) {
        return Err(CargoUnitGraphRefusal::UnsupportedEdge);
    }
    Ok(CargoUnitEdge {
        index,
        extern_crate_name,
        public,
        noprelude,
        nounused,
    })
}

fn validate_profile(value: &Value) -> Result<(), CargoUnitGraphRefusal> {
    let profile = object(value)?;
    check_fields(
        profile,
        &[
            "name",
            "opt_level",
            "lto",
            "codegen_backend",
            "codegen_units",
            "debuginfo",
            "split_debuginfo",
            "debug_assertions",
            "overflow_checks",
            "rpath",
            "incremental",
            "panic",
            "strip",
        ],
        &["rustflags", "trim_paths", "hint_mostly_unused"],
    )?;
    nonempty_string(field(profile, "name")?)?;
    if !matches!(
        nonempty_string(field(profile, "opt_level")?)?,
        "0" | "1" | "2" | "3" | "s" | "z"
    ) {
        return Err(CargoUnitGraphRefusal::UnsupportedUnit);
    }
    nonempty_string(field(profile, "lto")?)?;
    nullable_string(field(profile, "codegen_backend")?)?;
    match field(profile, "codegen_units")? {
        Value::Null => {}
        Value::Number(value) if value.as_u64().is_some_and(|n| n <= u32::MAX as u64) => {}
        _ => return Err(CargoUnitGraphRefusal::InvalidType),
    }
    match field(profile, "debuginfo")? {
        Value::Number(value) if matches!(value.as_u64(), Some(0..=2)) => {}
        Value::String(value)
            if matches!(value.as_str(), "line-directives-only" | "line-tables-only") => {}
        _ => return Err(CargoUnitGraphRefusal::InvalidType),
    }
    match field(profile, "split_debuginfo")? {
        Value::Null => {}
        Value::String(value) if !value.is_empty() && !value.as_bytes().contains(&0) => {}
        _ => return Err(CargoUnitGraphRefusal::InvalidType),
    }
    for field_name in [
        "debug_assertions",
        "overflow_checks",
        "rpath",
        "incremental",
    ] {
        required_bool(profile, field_name)?;
    }
    if !matches!(
        nonempty_string(field(profile, "panic")?)?,
        "unwind" | "abort" | "immediate-abort"
    ) {
        return Err(CargoUnitGraphRefusal::UnsupportedUnit);
    }
    validate_strip(field(profile, "strip")?)?;
    if let Some(rustflags) = profile.get("rustflags") {
        let flags = string_array(rustflags)?;
        if flags.is_empty() {
            return Err(CargoUnitGraphRefusal::InvalidValue);
        }
    }
    if let Some(trim_paths) = profile.get("trim_paths") {
        match trim_paths {
            Value::Null => {}
            Value::Array(values)
                if values.iter().all(|value| {
                    value
                        .as_str()
                        .is_some_and(|s| matches!(s, "diagnostics" | "macro" | "object"))
                }) => {}
            _ => return Err(CargoUnitGraphRefusal::InvalidType),
        }
    }
    if let Some(value) = profile.get("hint_mostly_unused") {
        if !value.is_boolean() {
            return Err(CargoUnitGraphRefusal::InvalidType);
        }
    }
    Ok(())
}

fn validate_strip(value: &Value) -> Result<(), CargoUnitGraphRefusal> {
    let strip = object(value)?;
    if strip.len() != 1 {
        return Err(CargoUnitGraphRefusal::InvalidValue);
    }
    if strip
        .keys()
        .any(|name| name != "resolved" && name != "deferred")
    {
        return Err(CargoUnitGraphRefusal::UnknownField);
    }
    let inner = strip
        .get("resolved")
        .or_else(|| strip.get("deferred"))
        .ok_or(CargoUnitGraphRefusal::InvalidValue)?;
    match inner {
        Value::String(value) if value == "None" => Ok(()),
        Value::Object(inner)
            if inner.len() == 1
                && inner
                    .get("Named")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty() && !value.as_bytes().contains(&0)) =>
        {
            Ok(())
        }
        _ => Err(CargoUnitGraphRefusal::InvalidType),
    }
}

fn parse_provenance(
    value: &Value,
    max_associations: usize,
) -> Result<CargoUnitGraphBindings, CargoUnitGraphRefusal> {
    let record = object(value)?;
    check_fields(record, &["schema", "version", "bindings"], &[])?;
    if nonempty_string(field(record, "schema")?)? != CARGO_UNIT_GRAPH_PROVENANCE_SCHEMA
        || unsigned(field(record, "version")?)? != u64::from(CARGO_UNIT_GRAPH_PROVENANCE_VERSION)
    {
        return Err(CargoUnitGraphRefusal::UnsupportedVersion);
    }
    let bindings = object(field(record, "bindings")?)?;
    check_fields(
        bindings,
        &[
            "cargo_version",
            "cargo_source_commit",
            "graph_digest",
            "rustc_version",
            "rustc_verbose_version_digest",
            "host_triple",
            "tool_receipt_identities",
            "source_git_commit",
            "source_git_tree",
            "source_content_digest",
            "lock_digest_before",
            "lock_digest_after",
            "manifest_set_digest",
            "config_set_digest",
            "argv_digest",
            "selector_digest",
            "profile_digest",
            "features_digest",
            "target_digest",
            "environment_digest",
            "private_path_identities",
            "run",
        ],
        &[],
    )?;
    Ok(CargoUnitGraphBindings {
        cargo_version: string_field(bindings, "cargo_version")?.to_owned(),
        cargo_source_commit: string_field(bindings, "cargo_source_commit")?.to_owned(),
        graph_digest: string_field(bindings, "graph_digest")?.to_owned(),
        rustc_version: string_field(bindings, "rustc_version")?.to_owned(),
        rustc_verbose_version_digest: string_field(bindings, "rustc_verbose_version_digest")?
            .to_owned(),
        host_triple: string_field(bindings, "host_triple")?.to_owned(),
        tool_receipt_identities: string_map(
            field(bindings, "tool_receipt_identities")?,
            max_associations,
        )?,
        source_git_commit: string_field(bindings, "source_git_commit")?.to_owned(),
        source_git_tree: string_field(bindings, "source_git_tree")?.to_owned(),
        source_content_digest: string_field(bindings, "source_content_digest")?.to_owned(),
        lock_digest_before: string_field(bindings, "lock_digest_before")?.to_owned(),
        lock_digest_after: string_field(bindings, "lock_digest_after")?.to_owned(),
        manifest_set_digest: string_field(bindings, "manifest_set_digest")?.to_owned(),
        config_set_digest: string_field(bindings, "config_set_digest")?.to_owned(),
        argv_digest: string_field(bindings, "argv_digest")?.to_owned(),
        selector_digest: string_field(bindings, "selector_digest")?.to_owned(),
        profile_digest: string_field(bindings, "profile_digest")?.to_owned(),
        features_digest: string_field(bindings, "features_digest")?.to_owned(),
        target_digest: string_field(bindings, "target_digest")?.to_owned(),
        environment_digest: string_field(bindings, "environment_digest")?.to_owned(),
        private_path_identities: string_map(
            field(bindings, "private_path_identities")?,
            max_associations,
        )?,
        run: parse_run(field(bindings, "run")?)?,
    })
}

fn parse_run(value: &Value) -> Result<CargoUnitGraphRunAccounting, CargoUnitGraphRefusal> {
    let run = object(value)?;
    check_fields(
        run,
        &[
            "task_id",
            "workspace_id",
            "run_id",
            "attempt_id",
            "invocation_count",
            "invocation_limit",
            "clock_start_ms",
            "clock_end_ms",
            "clock_elapsed_ms",
            "clock_limit_ms",
        ],
        &[],
    )?;
    Ok(CargoUnitGraphRunAccounting {
        task_id: string_field(run, "task_id")?.to_owned(),
        workspace_id: string_field(run, "workspace_id")?.to_owned(),
        run_id: string_field(run, "run_id")?.to_owned(),
        attempt_id: string_field(run, "attempt_id")?.to_owned(),
        invocation_count: u32_value(field(run, "invocation_count")?)?,
        invocation_limit: u32_value(field(run, "invocation_limit")?)?,
        clock_start_ms: unsigned(field(run, "clock_start_ms")?)?,
        clock_end_ms: unsigned(field(run, "clock_end_ms")?)?,
        clock_elapsed_ms: unsigned(field(run, "clock_elapsed_ms")?)?,
        clock_limit_ms: unsigned(field(run, "clock_limit_ms")?)?,
    })
}

fn validate_bindings(
    bindings: &CargoUnitGraphBindings,
    limits: CargoUnitGraphLimits,
) -> Result<(), CargoUnitGraphRefusal> {
    if bindings.cargo_version != CARGO_VERSION
        || bindings.cargo_source_commit != CARGO_SOURCE_COMMIT
        || bindings.rustc_version.is_empty()
        || bindings.rustc_version.len() > 256
        || bindings.rustc_version.as_bytes().contains(&0)
        || bindings.host_triple.is_empty()
        || bindings.host_triple.len() > 256
        || bindings.host_triple.as_bytes().contains(&0)
    {
        return Err(CargoUnitGraphRefusal::PinnedToolMismatch);
    }
    for value in [
        &bindings.graph_digest,
        &bindings.rustc_verbose_version_digest,
        &bindings.source_content_digest,
        &bindings.lock_digest_before,
        &bindings.lock_digest_after,
        &bindings.manifest_set_digest,
        &bindings.config_set_digest,
        &bindings.argv_digest,
        &bindings.selector_digest,
        &bindings.profile_digest,
        &bindings.features_digest,
        &bindings.target_digest,
        &bindings.environment_digest,
    ] {
        validate_digest(value)?;
    }
    for oid in [&bindings.source_git_commit, &bindings.source_git_tree] {
        if !valid_git_oid(oid) {
            return Err(CargoUnitGraphRefusal::InvalidValue);
        }
    }
    if bindings.source_git_commit.len() != bindings.source_git_tree.len() {
        return Err(CargoUnitGraphRefusal::InvalidValue);
    }
    for required in ["cargo", "rustc"] {
        if !bindings.tool_receipt_identities.contains_key(required) {
            return Err(CargoUnitGraphRefusal::InvalidValue);
        }
    }
    if bindings.tool_receipt_identities.len() > limits.associations
        || bindings.private_path_identities.len() > limits.associations
    {
        return Err(CargoUnitGraphRefusal::BudgetExceeded);
    }
    for (name, identity) in &bindings.tool_receipt_identities {
        validate_label(name)?;
        validate_digest(identity)?;
    }
    for required in ["cargo_home", "build_dir", "target_dir", "temp_dir"] {
        if !bindings.private_path_identities.contains_key(required) {
            return Err(CargoUnitGraphRefusal::InvalidValue);
        }
    }
    for (name, identity) in &bindings.private_path_identities {
        validate_label(name)?;
        validate_digest(identity)?;
    }
    let run = &bindings.run;
    for identity in [
        &run.task_id,
        &run.workspace_id,
        &run.run_id,
        &run.attempt_id,
    ] {
        validate_label(identity)?;
    }
    if run.invocation_count == 0
        || run.invocation_limit == 0
        || run.invocation_count > run.invocation_limit
        || run.clock_limit_ms == 0
        || run.clock_end_ms < run.clock_start_ms
        || run.clock_end_ms - run.clock_start_ms != run.clock_elapsed_ms
        || run.clock_elapsed_ms > run.clock_limit_ms
    {
        return Err(CargoUnitGraphRefusal::InvalidAccounting);
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<(), CargoUnitGraphRefusal> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(CargoUnitGraphRefusal::InvalidValue);
    };
    if hex.len() != 64
        || !hex
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(CargoUnitGraphRefusal::InvalidValue);
    }
    Ok(())
}

fn valid_git_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn validate_label(value: &str) -> Result<(), CargoUnitGraphRefusal> {
    if value.is_empty() || value.len() > 256 || value.as_bytes().contains(&0) {
        return Err(CargoUnitGraphRefusal::InvalidValue);
    }
    Ok(())
}

fn validate_absolute_path(
    value: &str,
    limits: CargoUnitGraphLimits,
) -> Result<(), CargoUnitGraphRefusal> {
    let tail = value.strip_prefix('/').unwrap_or("");
    if value.is_empty()
        || value.len() > limits.path_bytes
        || value.as_bytes().contains(&0)
        || !Path::new(value).is_absolute()
        || (!tail.is_empty()
            && tail
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".."))
    {
        return Err(if value.len() > limits.path_bytes {
            CargoUnitGraphRefusal::BudgetExceeded
        } else {
            CargoUnitGraphRefusal::InvalidSourcePath
        });
    }
    let mut depth = 0usize;
    for component in Path::new(value).components() {
        match component {
            Component::RootDir | Component::Prefix(_) => {}
            Component::Normal(_) => depth += 1,
            _ => return Err(CargoUnitGraphRefusal::InvalidSourcePath),
        }
    }
    if depth > limits.depth {
        return Err(CargoUnitGraphRefusal::BudgetExceeded);
    }
    Ok(())
}

fn validate_relative_path(
    value: &str,
    limits: CargoUnitGraphLimits,
) -> Result<(), CargoUnitGraphRefusal> {
    if value.is_empty()
        || value.len() > limits.path_bytes
        || value.as_bytes().contains(&0)
        || value.starts_with('/')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(if value.len() > limits.path_bytes {
            CargoUnitGraphRefusal::BudgetExceeded
        } else {
            CargoUnitGraphRefusal::InvalidSourcePath
        });
    }
    let path = Path::new(value);
    if path.components().count() > limits.depth
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(CargoUnitGraphRefusal::BudgetExceeded);
    }
    Ok(())
}

fn update_frame(hasher: &mut Sha256, value: &[u8]) -> Result<(), CargoUnitGraphRefusal> {
    let length = u64::try_from(value.len()).map_err(|_| CargoUnitGraphRefusal::BudgetExceeded)?;
    hasher.update(length.to_le_bytes());
    hasher.update(value);
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn parse_bounded_json(
    bytes: &[u8],
    limits: CargoUnitGraphLimits,
) -> Result<Value, CargoUnitGraphRefusal> {
    if bytes.len() > limits.bytes {
        return Err(CargoUnitGraphRefusal::BudgetExceeded);
    }
    let mut budget = ParseBudget {
        limits,
        unit_count: 0,
        association_count: 0,
        refusal: None,
    };
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let parsed = ValueSeed {
        budget: &mut budget,
        depth: 0,
        array_kind: ArrayKind::Other,
        map_kind: MapKind::Other,
        string_kind: StringKind::Other,
    }
    .deserialize(&mut deserializer);
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(_) => {
            return Err(budget
                .refusal
                .unwrap_or(CargoUnitGraphRefusal::MalformedJson))
        }
    };
    deserializer
        .end()
        .map_err(|_| CargoUnitGraphRefusal::TrailingBytes)?;
    Ok(parsed)
}

struct ParseBudget {
    limits: CargoUnitGraphLimits,
    unit_count: usize,
    association_count: usize,
    refusal: Option<CargoUnitGraphRefusal>,
}

#[derive(Clone, Copy)]
enum ArrayKind {
    Other,
    Units,
    Roots,
    Dependencies,
}

#[derive(Clone, Copy)]
enum MapKind {
    Other,
    IdentityEntries,
}

#[derive(Clone, Copy)]
enum StringKind {
    Other,
    SourcePath,
}

struct ValueSeed<'a> {
    budget: &'a mut ParseBudget,
    depth: usize,
    array_kind: ArrayKind,
    map_kind: MapKind,
    string_kind: StringKind,
}

impl<'de> DeserializeSeed<'de> for ValueSeed<'_> {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if self.depth > self.budget.limits.depth {
            return Err(fail(
                self.budget,
                CargoUnitGraphRefusal::BudgetExceeded,
                "JSON depth exceeds the configured bound",
            ));
        }
        deserializer.deserialize_any(ValueVisitor {
            budget: self.budget,
            depth: self.depth,
            array_kind: self.array_kind,
            map_kind: self.map_kind,
            string_kind: self.string_kind,
        })
    }
}

struct ValueVisitor<'a> {
    budget: &'a mut ParseBudget,
    depth: usize,
    array_kind: ArrayKind,
    map_kind: MapKind,
    string_kind: StringKind,
}

impl<'de> Visitor<'de> for ValueVisitor<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded JSON value without duplicate keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(mut self, value: &str) -> Result<Value, E>
    where
        E: de::Error,
    {
        self.check_string::<E>(value)?;
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(mut self, value: String) -> Result<Value, E>
    where
        E: de::Error,
    {
        self.check_string::<E>(&value)?;
        Ok(Value::String(value))
    }

    fn visit_unit<E>(self) -> Result<Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Null)
    }

    fn visit_none<E>(self) -> Result<Value, E>
    where
        E: de::Error,
    {
        Ok(Value::Null)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        loop {
            let remaining = sequence.size_hint();
            let over_limit = match self.array_kind {
                ArrayKind::Units => remaining.is_some_and(|count| {
                    self.budget.unit_count.saturating_add(count) > self.budget.limits.units
                }),
                ArrayKind::Roots => remaining.is_some_and(|count| count > self.budget.limits.units),
                ArrayKind::Dependencies => remaining.is_some_and(|count| {
                    self.budget.association_count.saturating_add(count)
                        > self.budget.limits.associations
                }),
                ArrayKind::Other => false,
            };
            if over_limit {
                return Err(fail(
                    self.budget,
                    CargoUnitGraphRefusal::BudgetExceeded,
                    "JSON array exceeds its configured bound",
                ));
            }
            let child = ValueSeed {
                budget: self.budget,
                depth: self.depth + 1,
                array_kind: ArrayKind::Other,
                map_kind: MapKind::Other,
                string_kind: StringKind::Other,
            };
            let Some(value) = sequence.next_element_seed(child)? else {
                break;
            };
            match self.array_kind {
                ArrayKind::Units => {
                    self.budget.unit_count += 1;
                    if self.budget.unit_count > self.budget.limits.units {
                        return Err(fail(
                            self.budget,
                            CargoUnitGraphRefusal::BudgetExceeded,
                            "unit count exceeds the configured bound",
                        ));
                    }
                }
                ArrayKind::Roots if values.len() >= self.budget.limits.units => {
                    return Err(fail(
                        self.budget,
                        CargoUnitGraphRefusal::BudgetExceeded,
                        "root count exceeds the configured bound",
                    ));
                }
                ArrayKind::Dependencies => {
                    self.budget.association_count += 1;
                    if self.budget.association_count > self.budget.limits.associations {
                        return Err(fail(
                            self.budget,
                            CargoUnitGraphRefusal::BudgetExceeded,
                            "dependency count exceeds the configured bound",
                        ));
                    }
                }
                ArrayKind::Other | ArrayKind::Roots => {}
            }
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        let mut seen = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if key.len() > self.budget.limits.path_bytes || key.as_bytes().contains(&0) {
                return Err(fail(
                    self.budget,
                    CargoUnitGraphRefusal::BudgetExceeded,
                    "JSON object key exceeds the configured path bound",
                ));
            }
            if !seen.insert(key.clone()) {
                return Err(fail(
                    self.budget,
                    CargoUnitGraphRefusal::DuplicateKey,
                    "duplicate JSON object key",
                ));
            }
            if matches!(self.map_kind, MapKind::IdentityEntries) {
                self.budget.association_count += 1;
                if self.budget.association_count > self.budget.limits.associations {
                    return Err(fail(
                        self.budget,
                        CargoUnitGraphRefusal::BudgetExceeded,
                        "provenance identity entries exceed the configured bound",
                    ));
                }
            }
            let array_kind = match key.as_str() {
                "units" => ArrayKind::Units,
                "roots" => ArrayKind::Roots,
                "dependencies" => ArrayKind::Dependencies,
                _ => ArrayKind::Other,
            };
            let string_kind = if key == "src_path" {
                StringKind::SourcePath
            } else {
                StringKind::Other
            };
            let map_kind = match key.as_str() {
                "tool_receipt_identities" | "private_path_identities" => MapKind::IdentityEntries,
                _ => MapKind::Other,
            };
            let value = map.next_value_seed(ValueSeed {
                budget: self.budget,
                depth: self.depth + 1,
                array_kind,
                map_kind,
                string_kind,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

impl ValueVisitor<'_> {
    fn check_string<E>(&mut self, value: &str) -> Result<(), E>
    where
        E: de::Error,
    {
        if matches!(self.string_kind, StringKind::SourcePath)
            && (value.len() > self.budget.limits.path_bytes || value.as_bytes().contains(&0))
        {
            return Err(fail(
                self.budget,
                CargoUnitGraphRefusal::BudgetExceeded,
                "source path exceeds the configured bound",
            ));
        }
        Ok(())
    }
}

fn fail<E: de::Error>(
    budget: &mut ParseBudget,
    refusal: CargoUnitGraphRefusal,
    message: &str,
) -> E {
    budget.refusal = Some(refusal);
    E::custom(message)
}

fn object(value: &Value) -> Result<&Map<String, Value>, CargoUnitGraphRefusal> {
    value.as_object().ok_or(CargoUnitGraphRefusal::InvalidType)
}

fn array(value: &Value) -> Result<&[Value], CargoUnitGraphRefusal> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or(CargoUnitGraphRefusal::InvalidType)
}

fn field<'a>(
    object: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a Value, CargoUnitGraphRefusal> {
    object.get(name).ok_or(CargoUnitGraphRefusal::MissingField)
}

fn check_fields(
    object: &Map<String, Value>,
    required: &[&str],
    optional: &[&str],
) -> Result<(), CargoUnitGraphRefusal> {
    for name in object.keys() {
        if !required.contains(&name.as_str()) && !optional.contains(&name.as_str()) {
            return Err(CargoUnitGraphRefusal::UnknownField);
        }
    }
    if required.iter().any(|name| !object.contains_key(*name)) {
        return Err(CargoUnitGraphRefusal::MissingField);
    }
    Ok(())
}

fn nonempty_string(value: &Value) -> Result<&str, CargoUnitGraphRefusal> {
    value
        .as_str()
        .filter(|value| !value.is_empty() && !value.as_bytes().contains(&0))
        .ok_or(CargoUnitGraphRefusal::InvalidType)
}

fn string_field<'a>(
    object: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a str, CargoUnitGraphRefusal> {
    field(object, name)?
        .as_str()
        .ok_or(CargoUnitGraphRefusal::InvalidType)
}

fn nullable_string(value: &Value) -> Result<(), CargoUnitGraphRefusal> {
    if value.is_null()
        || value
            .as_str()
            .is_some_and(|value| !value.as_bytes().contains(&0))
    {
        Ok(())
    } else {
        Err(CargoUnitGraphRefusal::InvalidType)
    }
}

fn string_array(value: &Value) -> Result<Vec<String>, CargoUnitGraphRefusal> {
    array(value)?
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.as_bytes().contains(&0))
                .map(str::to_owned)
                .ok_or(CargoUnitGraphRefusal::InvalidType)
        })
        .collect()
}

fn required_bool(object: &Map<String, Value>, name: &str) -> Result<bool, CargoUnitGraphRefusal> {
    field(object, name)?
        .as_bool()
        .ok_or(CargoUnitGraphRefusal::InvalidType)
}

fn optional_bool(
    object: &Map<String, Value>,
    name: &str,
) -> Result<Option<bool>, CargoUnitGraphRefusal> {
    object
        .get(name)
        .map(|value| value.as_bool().ok_or(CargoUnitGraphRefusal::InvalidType))
        .transpose()
}

fn unsigned(value: &Value) -> Result<u64, CargoUnitGraphRefusal> {
    value.as_u64().ok_or(CargoUnitGraphRefusal::InvalidType)
}

fn u32_value(value: &Value) -> Result<u32, CargoUnitGraphRefusal> {
    u32::try_from(unsigned(value)?).map_err(|_| CargoUnitGraphRefusal::InvalidValue)
}

fn index(value: &Value, unit_count: usize) -> Result<usize, CargoUnitGraphRefusal> {
    let index = index_number(value)?;
    if index >= unit_count {
        return Err(CargoUnitGraphRefusal::InvalidIndex);
    }
    Ok(index)
}

fn index_number(value: &Value) -> Result<usize, CargoUnitGraphRefusal> {
    match value.as_u64() {
        Some(value) => usize::try_from(value).map_err(|_| CargoUnitGraphRefusal::InvalidIndex),
        None if value.is_number() => Err(CargoUnitGraphRefusal::InvalidIndex),
        None => Err(CargoUnitGraphRefusal::InvalidType),
    }
}

fn string_map(
    value: &Value,
    max_associations: usize,
) -> Result<BTreeMap<String, String>, CargoUnitGraphRefusal> {
    let object = object(value)?;
    if object.len() > max_associations {
        return Err(CargoUnitGraphRefusal::BudgetExceeded);
    }
    object
        .iter()
        .map(|(key, value)| {
            let value = value.as_str().ok_or(CargoUnitGraphRefusal::InvalidType)?;
            Ok((key.clone(), value.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn profile(name: &str) -> Value {
        json!({
            "name": name,
            "opt_level": "0",
            "lto": "false",
            "codegen_backend": null,
            "codegen_units": null,
            "debuginfo": 2,
            "split_debuginfo": null,
            "debug_assertions": true,
            "overflow_checks": true,
            "rpath": false,
            "incremental": true,
            "panic": "unwind",
            "strip": {"deferred": "None"}
        })
    }

    fn host_profile(name: &str) -> Value {
        let mut profile = profile(name);
        profile["strip"] = json!({"deferred": {"Named": "debuginfo"}});
        profile
    }

    fn target(kind: &str, crate_type: &str, name: &str, src_path: &str) -> Value {
        json!({
            "kind": [kind],
            "crate_types": [crate_type],
            "name": name,
            "src_path": src_path,
            "edition": "2021",
            "doc": matches!(kind, "lib" | "proc-macro"),
            "doctest": kind == "lib",
            "test": matches!(kind, "lib" | "test" | "proc-macro")
        })
    }

    fn graph_value() -> Value {
        let mut test_target = target("test", "bin", "smoke", "/workspace/app/tests/smoke.rs");
        test_target
            .as_object_mut()
            .expect("fixture target object")
            .insert("required-features".to_owned(), json!(["feature-a"]));

        let mut test_profile = profile("test");
        test_profile
            .as_object_mut()
            .expect("fixture profile object")
            .insert("rustflags".to_owned(), json!(["--cap-lints=warn"]));
        test_profile
            .as_object_mut()
            .expect("fixture profile object")
            .insert("trim_paths".to_owned(), json!(["object"]));
        test_profile
            .as_object_mut()
            .expect("fixture profile object")
            .insert("hint_mostly_unused".to_owned(), json!(false));

        json!({
            "version": 1,
            "units": [
                {
                    "pkg_id": "app 1.0.0 (path+file:///workspace/app)",
                    "target": target("lib", "lib", "app", "/workspace/app/src/lib.rs"),
                    "profile": profile("dev"),
                    "platform": null,
                    "mode": "build",
                    "features": ["default", "feature-a"],
                    "dependencies": [{"index": 4, "extern_crate_name": "helper", "public": true}]
                },
                {
                    "pkg_id": "app 1.0.0 (path+file:///workspace/app)",
                    "target": test_target,
                    "profile": test_profile,
                    "platform": null,
                    "mode": "test",
                    "features": ["default"],
                    "dependencies": [{"index": 0, "extern_crate_name": "app", "public": false, "noprelude": false}]
                },
                {
                    "pkg_id": "app 1.0.0 (path+file:///workspace/app)",
                    "target": target("custom-build", "bin", "build-script-build", "/workspace/app/build.rs"),
                    "profile": host_profile("dev"),
                    "platform": null,
                    "mode": "build",
                    "features": [],
                    "dependencies": [{"index": 4, "extern_crate_name": "helper", "public": false, "noprelude": false, "nounused": false}]
                },
                {
                    "pkg_id": "app 1.0.0 (path+file:///workspace/app)",
                    "target": target("custom-build", "bin", "build-script-build", "/workspace/app/build.rs"),
                    "profile": host_profile("dev"),
                    "platform": null,
                    "mode": "run-custom-build",
                    "features": [],
                    "dependencies": [{"index": 2, "extern_crate_name": "build_script_build", "public": false}]
                },
                {
                    "pkg_id": "helper 1.0.0 (path+file:///workspace/app/macros)",
                    "target": target("proc-macro", "proc-macro", "helper", "/workspace/app/macros/src/lib.rs"),
                    "profile": profile("dev"),
                    "platform": null,
                    "mode": "build",
                    "features": ["proc-macro"],
                    "dependencies": []
                }
            ],
            "roots": [0, 1]
        })
    }

    fn graph_bytes() -> Vec<u8> {
        serde_json::to_vec(&graph_value()).expect("serialize owned graph fixture")
    }

    fn roots() -> [&'static Path; 1] {
        [Path::new("/workspace/app")]
    }

    fn limits() -> CargoUnitGraphLimits {
        CargoUnitGraphLimits::default()
    }

    fn digest_id(value: u8) -> String {
        format!("sha256:{:064x}", u64::from(value))
    }

    fn expected_bindings(graph: &[u8]) -> CargoUnitGraphBindings {
        let files = BTreeMap::from([
            ("Cargo.toml".to_owned(), digest_id(30)),
            ("src/lib.rs".to_owned(), digest_id(31)),
        ]);
        CargoUnitGraphBindings {
            cargo_version: CARGO_VERSION.to_owned(),
            cargo_source_commit: CARGO_SOURCE_COMMIT.to_owned(),
            graph_digest: digest(graph),
            rustc_version: "rustc 1.97.1 (fixture)".to_owned(),
            rustc_verbose_version_digest: digest_id(17),
            host_triple: "aarch64-apple-darwin".to_owned(),
            tool_receipt_identities: BTreeMap::from([
                ("cargo".to_owned(), digest_id(1)),
                ("rustc".to_owned(), digest_id(2)),
                ("clang".to_owned(), digest_id(3)),
            ]),
            source_git_commit: "a".repeat(40),
            source_git_tree: "b".repeat(40),
            source_content_digest: framed_source_content_digest(&files, limits())
                .expect("frame fixture source identity"),
            lock_digest_before: digest_id(4),
            lock_digest_after: digest_id(4),
            manifest_set_digest: digest_id(5),
            config_set_digest: digest_id(6),
            argv_digest: digest_id(7),
            selector_digest: digest_id(8),
            profile_digest: digest_id(9),
            features_digest: digest_id(10),
            target_digest: digest_id(11),
            environment_digest: digest_id(12),
            private_path_identities: BTreeMap::from([
                ("cargo_home".to_owned(), digest_id(13)),
                ("build_dir".to_owned(), digest_id(14)),
                ("target_dir".to_owned(), digest_id(15)),
                ("temp_dir".to_owned(), digest_id(16)),
            ]),
            run: CargoUnitGraphRunAccounting {
                task_id: "oxide-task-fixture".to_owned(),
                workspace_id: "workspace-fixture".to_owned(),
                run_id: "run-fixture".to_owned(),
                attempt_id: "attempt-fixture".to_owned(),
                invocation_count: 2,
                invocation_limit: 4,
                clock_start_ms: 100,
                clock_end_ms: 150,
                clock_elapsed_ms: 50,
                clock_limit_ms: 500,
            },
        }
    }

    fn provenance_bytes(bindings: &CargoUnitGraphBindings) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema": CARGO_UNIT_GRAPH_PROVENANCE_SCHEMA,
            "version": CARGO_UNIT_GRAPH_PROVENANCE_VERSION,
            "bindings": bindings
        }))
        .expect("serialize owned provenance fixture")
    }

    #[test]
    fn decodes_v1_units_edges_and_source_root_bindings_without_deduplicating_packages() {
        let decoded = decode_unit_graph(&graph_bytes(), &roots(), limits()).expect("v1 graph");
        assert_eq!(decoded.version, 1);
        assert_eq!(decoded.roots, vec![0, 1]);
        assert_eq!(decoded.units.len(), 5);
        assert_eq!(decoded.units[0].package_id, decoded.units[1].package_id);
        assert_eq!(decoded.units[0].mode, CargoUnitMode::Build);
        assert_eq!(decoded.units[1].mode, CargoUnitMode::Test);
        assert_eq!(decoded.units[2].mode, CargoUnitMode::Build);
        assert_eq!(decoded.units[3].mode, CargoUnitMode::RunCustomBuild);
        assert_eq!(
            decoded.units[2].profile["strip"],
            json!({"deferred": {"Named": "debuginfo"}})
        );
        assert_eq!(
            decoded.units[4].target.crate_types,
            vec!["proc-macro".to_owned()]
        );
        assert_eq!(decoded.units[0].dependencies[0].public, Some(true));
        assert_eq!(decoded.units[1].dependencies[0].public, Some(false));
        assert_eq!(decoded.units[0].dependencies[0].noprelude, None);
        assert_eq!(decoded.units[1].dependencies[0].noprelude, Some(false));
        assert!(!decoded.units[0].is_std);
        assert_eq!(
            decoded.units[1].target.required_features,
            Some(vec!["feature-a".to_owned()])
        );
        assert_eq!(decoded.units[0].profile["name"], "dev");
        assert_eq!(decoded.units[1].profile["trim_paths"], json!(["object"]));
    }

    #[test]
    fn refuses_duplicate_unknown_malformed_and_trailing_json() {
        let bytes = graph_bytes();
        let duplicate = String::from_utf8(bytes.clone())
            .expect("fixture JSON")
            .replacen("\"version\":1", "\"version\":1,\"version\":1", 1);
        assert_eq!(
            decode_unit_graph(duplicate.as_bytes(), &roots(), limits()),
            Err(CargoUnitGraphRefusal::DuplicateKey)
        );

        let mut unknown = graph_value();
        unknown["units"][0]["unrecognized"] = json!(true);
        assert_eq!(
            decode_unit_graph(&serde_json::to_vec(&unknown).unwrap(), &roots(), limits()),
            Err(CargoUnitGraphRefusal::UnknownField)
        );
        let mut unknown_profile = graph_value();
        unknown_profile["units"][0]["profile"]["future_profile_key"] = json!(true);
        assert_eq!(
            decode_unit_graph(
                &serde_json::to_vec(&unknown_profile).unwrap(),
                &roots(),
                limits()
            ),
            Err(CargoUnitGraphRefusal::UnknownField)
        );
        assert_eq!(
            decode_unit_graph(b"{", &roots(), limits()),
            Err(CargoUnitGraphRefusal::MalformedJson)
        );
        let trailing = [bytes.as_slice(), b"{}"].concat();
        assert_eq!(
            decode_unit_graph(&trailing, &roots(), limits()),
            Err(CargoUnitGraphRefusal::TrailingBytes)
        );
    }

    #[test]
    fn refuses_unsupported_versions_types_modes_and_build_std() {
        let mut value = graph_value();
        value["version"] = json!(2);
        assert_eq!(
            decode_unit_graph(&serde_json::to_vec(&value).unwrap(), &roots(), limits()),
            Err(CargoUnitGraphRefusal::UnsupportedVersion)
        );
        let mut value = graph_value();
        value["units"][0]["profile"]["incremental"] = json!("true");
        assert_eq!(
            decode_unit_graph(&serde_json::to_vec(&value).unwrap(), &roots(), limits()),
            Err(CargoUnitGraphRefusal::InvalidType)
        );
        let mut value = graph_value();
        value["units"][0]["mode"] = json!("future-mode");
        assert_eq!(
            decode_unit_graph(&serde_json::to_vec(&value).unwrap(), &roots(), limits()),
            Err(CargoUnitGraphRefusal::UnsupportedUnit)
        );
        let mut value = graph_value();
        value["units"][0]["is_std"] = json!(true);
        assert_eq!(
            decode_unit_graph(&serde_json::to_vec(&value).unwrap(), &roots(), limits()),
            Err(CargoUnitGraphRefusal::UnsupportedUnit)
        );
    }

    #[test]
    fn refuses_bad_indices_edge_values_and_unregistered_paths() {
        let mut value = graph_value();
        value["roots"][0] = json!(5);
        assert_eq!(
            decode_unit_graph(&serde_json::to_vec(&value).unwrap(), &roots(), limits()),
            Err(CargoUnitGraphRefusal::InvalidIndex)
        );
        let mut value = graph_value();
        value["units"][0]["dependencies"][0]["index"] = json!(5);
        assert_eq!(
            decode_unit_graph(&serde_json::to_vec(&value).unwrap(), &roots(), limits()),
            Err(CargoUnitGraphRefusal::InvalidIndex)
        );
        let mut value = graph_value();
        value["units"][0]["dependencies"][0]["noprelude"] = json!(true);
        assert_eq!(
            decode_unit_graph(&serde_json::to_vec(&value).unwrap(), &roots(), limits()),
            Err(CargoUnitGraphRefusal::UnsupportedEdge)
        );
        let mut value = graph_value();
        value["units"][0]["dependencies"][0]["public"] = json!(1);
        assert_eq!(
            decode_unit_graph(&serde_json::to_vec(&value).unwrap(), &roots(), limits()),
            Err(CargoUnitGraphRefusal::InvalidType)
        );
        let mut value = graph_value();
        value["units"][0]["target"]["src_path"] = json!("/outside/app.rs");
        assert_eq!(
            decode_unit_graph(&serde_json::to_vec(&value).unwrap(), &roots(), limits()),
            Err(CargoUnitGraphRefusal::UnregisteredSourcePath)
        );
        let mut value = graph_value();
        value["units"][0]["target"]["src_path"] = json!("/workspace/app/../outside.rs");
        assert_eq!(
            decode_unit_graph(&serde_json::to_vec(&value).unwrap(), &roots(), limits()),
            Err(CargoUnitGraphRefusal::InvalidSourcePath)
        );
    }

    #[test]
    fn enforces_exact_and_over_budget_byte_unit_edge_path_and_depth_limits() {
        let bytes = graph_bytes();
        let mut exact = limits();
        exact.bytes = bytes.len();
        exact.units = 5;
        exact.associations = 4;
        assert!(decode_unit_graph(&bytes, &roots(), exact).is_ok());

        let mut empty_graph_limits = limits();
        empty_graph_limits.units = 0;
        empty_graph_limits.associations = 0;
        assert!(decode_unit_graph(
            br#"{"version":1,"units":[],"roots":[]}"#,
            &[],
            empty_graph_limits
        )
        .is_ok());

        for over_limit in [
            CargoUnitGraphLimits {
                bytes: MAX_GRAPH_BYTES + 1,
                ..limits()
            },
            CargoUnitGraphLimits {
                units: MAX_GRAPH_UNITS + 1,
                ..limits()
            },
            CargoUnitGraphLimits {
                associations: MAX_GRAPH_ASSOCIATIONS + 1,
                ..limits()
            },
            CargoUnitGraphLimits {
                path_bytes: MAX_GRAPH_PATH_BYTES + 1,
                ..limits()
            },
            CargoUnitGraphLimits {
                depth: MAX_GRAPH_DEPTH + 1,
                ..limits()
            },
        ] {
            assert_eq!(
                decode_unit_graph(&bytes, &roots(), over_limit),
                Err(CargoUnitGraphRefusal::BudgetExceeded)
            );
        }

        let root_limit = CargoUnitGraphLimits {
            units: 1,
            ..limits()
        };
        let two_roots = [Path::new("/workspace/app"), Path::new("/workspace")];
        assert_eq!(
            decode_unit_graph(&bytes, &two_roots, root_limit),
            Err(CargoUnitGraphRefusal::BudgetExceeded)
        );

        let mut too_small = exact;
        too_small.bytes -= 1;
        assert_eq!(
            decode_unit_graph(&bytes, &roots(), too_small),
            Err(CargoUnitGraphRefusal::BudgetExceeded)
        );
        too_small = exact;
        too_small.units = 4;
        assert_eq!(
            decode_unit_graph(&bytes, &roots(), too_small),
            Err(CargoUnitGraphRefusal::BudgetExceeded)
        );
        too_small = exact;
        too_small.associations = 3;
        assert_eq!(
            decode_unit_graph(&bytes, &roots(), too_small),
            Err(CargoUnitGraphRefusal::BudgetExceeded)
        );
        let mut path_boundary = exact;
        path_boundary.bytes = MAX_GRAPH_BYTES;
        path_boundary.path_bytes = 128;
        let mut value = graph_value();
        let boundary_path = format!("/workspace/app/{}.rs", "x".repeat(110));
        assert_eq!(boundary_path.len(), path_boundary.path_bytes);
        value["units"][0]["target"]["src_path"] = json!(boundary_path);
        let path_bytes = serde_json::to_vec(&value).unwrap();
        assert!(decode_unit_graph(&path_bytes, &roots(), path_boundary).is_ok());
        path_boundary.path_bytes -= 1;
        assert_eq!(
            decode_unit_graph(&path_bytes, &roots(), path_boundary),
            Err(CargoUnitGraphRefusal::BudgetExceeded)
        );
        let mut depth_boundary = exact;
        depth_boundary.depth = 6;
        assert!(decode_unit_graph(&bytes, &roots(), depth_boundary).is_ok());
        depth_boundary.depth -= 1;
        assert_eq!(
            decode_unit_graph(&bytes, &roots(), depth_boundary),
            Err(CargoUnitGraphRefusal::BudgetExceeded)
        );
    }

    #[test]
    fn verifies_exact_graph_bytes_and_all_supplied_binding_categories() {
        let graph = graph_bytes();
        let expected = expected_bindings(&graph);
        assert_ne!(expected.source_content_digest, expected.source_git_tree);
        let record = provenance_bytes(&expected);
        assert_eq!(
            verify_provenance(&graph, &record, &expected, limits()),
            Ok(())
        );

        let changed_graph = [graph.as_slice(), b" "].concat();
        assert_eq!(
            verify_provenance(&changed_graph, &record, &expected, limits()),
            Err(CargoUnitGraphRefusal::GraphDigestMismatch)
        );
        let mut changed = expected.clone();
        changed
            .tool_receipt_identities
            .insert("rustc".to_owned(), digest_id(20));
        assert_eq!(
            verify_provenance(&graph, &provenance_bytes(&changed), &expected, limits()),
            Err(CargoUnitGraphRefusal::ProvenanceMismatch)
        );
        let mut changed = expected.clone();
        changed.source_git_commit = "c".repeat(40);
        assert_eq!(
            verify_provenance(&graph, &provenance_bytes(&changed), &expected, limits()),
            Err(CargoUnitGraphRefusal::ProvenanceMismatch)
        );
        let mut changed = expected.clone();
        changed.source_git_tree = "c".repeat(40);
        assert_eq!(
            verify_provenance(&graph, &provenance_bytes(&changed), &expected, limits()),
            Err(CargoUnitGraphRefusal::ProvenanceMismatch)
        );
        let mut changed = expected.clone();
        changed.source_content_digest = digest_id(21);
        assert_eq!(
            verify_provenance(&graph, &provenance_bytes(&changed), &expected, limits()),
            Err(CargoUnitGraphRefusal::ProvenanceMismatch)
        );
        let mut changed = expected.clone();
        changed.manifest_set_digest = digest_id(22);
        changed.config_set_digest = digest_id(23);
        changed.argv_digest = digest_id(24);
        changed.selector_digest = digest_id(25);
        changed.profile_digest = digest_id(26);
        changed.features_digest = digest_id(27);
        changed.target_digest = digest_id(28);
        changed.environment_digest = digest_id(29);
        changed
            .private_path_identities
            .insert("target_dir".to_owned(), digest_id(32));
        assert_eq!(
            verify_provenance(&graph, &provenance_bytes(&changed), &expected, limits()),
            Err(CargoUnitGraphRefusal::ProvenanceMismatch)
        );
        let mut changed = expected.clone();
        changed.lock_digest_after = digest_id(33);
        assert_eq!(
            verify_provenance(&graph, &provenance_bytes(&changed), &expected, limits()),
            Err(CargoUnitGraphRefusal::LockChanged)
        );
        let mut changed = expected.clone();
        changed.run.invocation_count = 5;
        assert_eq!(
            verify_provenance(&graph, &provenance_bytes(&changed), &expected, limits()),
            Err(CargoUnitGraphRefusal::InvalidAccounting)
        );
        let mut changed = expected.clone();
        changed.run.clock_elapsed_ms = 49;
        assert_eq!(
            verify_provenance(&graph, &provenance_bytes(&changed), &expected, limits()),
            Err(CargoUnitGraphRefusal::InvalidAccounting)
        );
        let mut changed = expected.clone();
        changed.run.run_id = "other-run".to_owned();
        assert_eq!(
            verify_provenance(&graph, &provenance_bytes(&changed), &expected, limits()),
            Err(CargoUnitGraphRefusal::ProvenanceMismatch)
        );
        let mut changed = expected.clone();
        changed.rustc_verbose_version_digest = digest_id(34);
        changed.host_triple = "x86_64-unknown-linux-gnu".to_owned();
        assert_eq!(
            verify_provenance(&graph, &provenance_bytes(&changed), &expected, limits()),
            Err(CargoUnitGraphRefusal::ProvenanceMismatch)
        );
    }

    #[test]
    fn provenance_refuses_missing_duplicate_unknown_and_unpinned_bindings() {
        let graph = graph_bytes();
        let expected = expected_bindings(&graph);
        let valid = String::from_utf8(provenance_bytes(&expected)).expect("fixture JSON");
        let mut missing: Value = serde_json::from_str(&valid).expect("fixture JSON");
        missing["bindings"]
            .as_object_mut()
            .expect("fixture bindings object")
            .remove("features_digest");
        assert_eq!(
            verify_provenance(
                &graph,
                &serde_json::to_vec(&missing).unwrap(),
                &expected,
                limits()
            ),
            Err(CargoUnitGraphRefusal::MissingField)
        );
        let mut unknown: Value = serde_json::from_str(&valid).expect("fixture JSON");
        unknown["bindings"]["unrecognized"] = json!(true);
        assert_eq!(
            verify_provenance(
                &graph,
                &serde_json::to_vec(&unknown).unwrap(),
                &expected,
                limits()
            ),
            Err(CargoUnitGraphRefusal::UnknownField)
        );
        let duplicate = valid.replacen("\"schema\":", "\"schema\":\"wrong\",\"schema\":", 1);
        assert_eq!(
            verify_provenance(&graph, duplicate.as_bytes(), &expected, limits()),
            Err(CargoUnitGraphRefusal::DuplicateKey)
        );
        let mut unpinned = expected.clone();
        unpinned.cargo_version = "nightly".to_owned();
        assert_eq!(
            verify_provenance(&graph, &provenance_bytes(&unpinned), &expected, limits()),
            Err(CargoUnitGraphRefusal::PinnedToolMismatch)
        );
        let record = provenance_bytes(&expected);
        let mut exact = limits();
        exact.bytes = graph.len().max(record.len());
        assert_eq!(verify_provenance(&graph, &record, &expected, exact), Ok(()));
        exact.bytes -= 1;
        assert_eq!(
            verify_provenance(&graph, &record, &expected, exact),
            Err(CargoUnitGraphRefusal::BudgetExceeded)
        );
        exact = limits();
        exact.associations = 7;
        assert_eq!(verify_provenance(&graph, &record, &expected, exact), Ok(()));
        exact.associations = 6;
        assert_eq!(
            verify_provenance(&graph, &record, &expected, exact),
            Err(CargoUnitGraphRefusal::BudgetExceeded)
        );
    }

    #[test]
    fn framed_source_content_digest_is_stable_and_framed() {
        let first = BTreeMap::from([
            ("a.rs".to_owned(), digest_id(40)),
            ("b.rs".to_owned(), digest_id(41)),
        ]);
        let second = BTreeMap::from([
            ("b.rs".to_owned(), digest_id(41)),
            ("a.rs".to_owned(), digest_id(40)),
        ]);
        assert_eq!(
            framed_source_content_digest(&first, limits()),
            framed_source_content_digest(&second, limits())
        );
        assert_ne!(
            framed_source_content_digest(&first, limits()),
            framed_source_content_digest(
                &BTreeMap::from([
                    ("ab.rs".to_owned(), digest_id(40)),
                    (".rs".to_owned(), digest_id(41))
                ]),
                limits()
            )
        );

        let one_file = BTreeMap::from([("a.rs".to_owned(), digest_id(42))]);
        let mut exact = limits();
        exact.bytes = 91;
        exact.path_bytes = 4;
        exact.associations = 1;
        assert!(framed_source_content_digest(&one_file, exact).is_ok());
        exact.bytes -= 1;
        assert_eq!(
            framed_source_content_digest(&one_file, exact),
            Err(CargoUnitGraphRefusal::BudgetExceeded)
        );
        exact.bytes = 91;
        exact.path_bytes -= 1;
        assert_eq!(
            framed_source_content_digest(&one_file, exact),
            Err(CargoUnitGraphRefusal::BudgetExceeded)
        );
    }
}
