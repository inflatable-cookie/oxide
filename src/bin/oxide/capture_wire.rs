//! Shared capture wire: `oxide.capture.transcript/1` and
//! `oxide.capture.manifest/1` parse and completeness.
//!
//! Parser-positive proves structure only. Completeness is a separate
//! judgement: path-only `parent_required` rows fail; `handle` parent still
//! fails while [`K1_PREMISE_QUALIFIED`] is false. `derived` is not a row
//! provenance class.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Transcript schema this landing extends. Historical `/1` field names that
/// this adapter no longer emits (`parent_dev`, `dev`, …) are unknown fields.
pub(crate) const TRANSCRIPT_SCHEMA: &str = "oxide.capture.transcript/1";
/// Manifest schema this landing extends with `parent_required`.
pub(crate) const MANIFEST_SCHEMA: &str = "oxide.capture.manifest/1";
/// Numeric identity and lifetime premise for a handle parent. Unqualified:
/// a closed `(device, inode)` pair does not prove the parent of the read.
pub(crate) const K1_PREMISE_QUALIFIED: bool = false;

const MAX_READ_ROWS: usize = 8192;
const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
const MAX_OCCURRENCES_PER_KEY: u64 = 64;
const MAX_PROCESS_ROWS: usize = 512;

const TRANSCRIPT_FIELDS: &[&str] = &[
    "schema",
    "version",
    "engine",
    "case",
    "manifest_sha256",
    "entrypoint",
    "window_monotonic_start_ns",
    "window_monotonic_end_ns",
    "window_elapsed_ns",
    "outcome",
    "truncation",
    "effects",
    "reads",
    "clocks",
    "classifications",
    "recorded_holders",
    "fixture_capture",
    "notes",
];

const READ_FIELDS: &[&str] = &[
    "kind",
    "site",
    "identity",
    "outcome",
    "cap",
    "epoch",
    "occurrence",
];

const IDENTITY_FIELDS: &[&str] = &[
    "path",
    "component",
    "parent_device",
    "parent_inode",
    "device",
    "inode",
    "pid",
    "start",
    "uid",
    "fd",
    "volume_uuid",
    "mount",
    "volume_device",
    "window_identity",
    "parent_source",
    "own_source",
];

const EFFECT_FIELDS: &[&str] = &["kind", "site", "attempted", "path", "epoch"];
const EFFECT_KINDS: &[&str] = &[
    "create",
    "save",
    "fence",
    "reconcile",
    "lock-create",
    "rename",
    "unlink",
];
const PROVENANCE: &[&str] = &["handle", "extracted", "context", "none"];

/// Parent or own-object provenance on a transcript identity. `derived` is a
/// reconciliation label only and must not appear on a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Provenance {
    Handle,
    Extracted,
    Context,
    None,
}

impl Provenance {
    fn parse(value: Option<&serde_json::Value>) -> Result<Self, String> {
        let text = value.and_then(|v| v.as_str()).ok_or("missing provenance")?;
        match text {
            "handle" => Ok(Self::Handle),
            "extracted" => Ok(Self::Extracted),
            "context" => Ok(Self::Context),
            "none" => Ok(Self::None),
            other => Err(format!("unknown provenance {other}")),
        }
    }
}

/// Structure-only parse failure. Never a completeness judgement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParseError {
    pub(crate) message: String,
}

/// Completeness issues. An empty list is still not success while K1 is
/// unqualified and a `parent_required` obligation remains.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Completeness {
    pub(crate) issues: Vec<String>,
}

impl Completeness {
    pub(crate) fn ok(&self) -> bool {
        self.issues.is_empty()
    }
}

fn as_object<'a>(
    value: &'a serde_json::Value,
    what: &str,
) -> Result<&'a serde_json::Map<String, serde_json::Value>, ParseError> {
    value.as_object().ok_or_else(|| ParseError {
        message: format!("{what} is not an object"),
    })
}

fn deny_unknown(
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
    what: &str,
) -> Result<(), ParseError> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(ParseError {
                message: format!("unknown {what} field {key}"),
            });
        }
    }
    Ok(())
}

fn require_keys(
    object: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
    what: &str,
) -> Result<(), ParseError> {
    for key in keys {
        if !object.contains_key(*key) {
            return Err(ParseError {
                message: format!("{what} missing {key}"),
            });
        }
    }
    Ok(())
}

fn schema_name(
    doc: &serde_json::Map<String, serde_json::Value>,
    expected: &str,
) -> Result<(), ParseError> {
    let schema = doc.get("schema").and_then(|v| v.as_str());
    let version = doc.get("version").and_then(|v| v.as_str());
    let found = schema.or(version).ok_or_else(|| ParseError {
        message: "missing schema".to_owned(),
    })?;
    if found != expected {
        return Err(ParseError {
            message: format!("unknown version {found}"),
        });
    }
    if let (Some(schema), Some(version)) = (schema, version) {
        if schema != version {
            return Err(ParseError {
                message: "schema and version disagree".to_owned(),
            });
        }
    }
    Ok(())
}

fn parse_err(message: impl Into<String>) -> ParseError {
    ParseError {
        message: message.into(),
    }
}

fn optional_string(value: &serde_json::Value, what: &str) -> Result<(), ParseError> {
    if value.is_null() || value.is_string() {
        Ok(())
    } else {
        Err(parse_err(format!("{what} is not a string or null")))
    }
}

fn optional_u64(value: &serde_json::Value, what: &str) -> Result<(), ParseError> {
    if value.is_null() || value.is_u64() {
        Ok(())
    } else {
        Err(parse_err(format!("{what} is not a u64 or null")))
    }
}

fn optional_u32(value: &serde_json::Value, what: &str) -> Result<(), ParseError> {
    if value.is_null() {
        return Ok(());
    }
    if value.as_u64().is_some_and(|n| n <= u64::from(u32::MAX)) {
        Ok(())
    } else {
        Err(parse_err(format!("{what} is not a u32 or null")))
    }
}

fn optional_i32(value: &serde_json::Value, what: &str) -> Result<(), ParseError> {
    if value.is_null() {
        return Ok(());
    }
    if value
        .as_i64()
        .is_some_and(|n| n >= i64::from(i32::MIN) && n <= i64::from(i32::MAX))
    {
        Ok(())
    } else {
        Err(parse_err(format!("{what} is not an i32 or null")))
    }
}

fn require_u64(value: Option<&serde_json::Value>, what: &str) -> Result<u64, ParseError> {
    value
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| parse_err(format!("{what} is not a u64")))
}

fn type_check_identity(
    identity: &serde_json::Map<String, serde_json::Value>,
    require_provenance: bool,
) -> Result<(), ParseError> {
    deny_unknown(identity, IDENTITY_FIELDS, "identity")?;
    if require_provenance {
        require_keys(identity, &["parent_source", "own_source"], "identity")?;
    }
    if let Some(value) = identity.get("parent_source") {
        Provenance::parse(Some(value)).map_err(parse_err)?;
    }
    if let Some(value) = identity.get("own_source") {
        Provenance::parse(Some(value)).map_err(parse_err)?;
    }
    for (key, value) in identity {
        match key.as_str() {
            "path" | "component" | "volume_uuid" | "mount" | "volume_device"
            | "window_identity" => optional_string(value, key)?,
            "parent_device" | "parent_inode" | "device" | "inode" | "start" => {
                optional_u64(value, key)?;
            }
            "pid" | "uid" => optional_u32(value, key)?,
            "fd" => optional_i32(value, key)?,
            "parent_source" | "own_source" => {}
            _ => {}
        }
    }
    Ok(())
}

fn parse_identity(value: &serde_json::Value) -> Result<(), ParseError> {
    let identity = as_object(value, "identity")?;
    type_check_identity(identity, true)
}

fn parse_outcome(value: &serde_json::Value) -> Result<(), ParseError> {
    let outcome = as_object(value, "outcome")?;
    let ty = outcome
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| parse_err("outcome missing type"))?;
    match ty {
        "digest" => {
            deny_unknown(outcome, &["type", "digest"], "outcome")?;
            let digest = outcome
                .get("digest")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| parse_err("outcome digest is not a SHA-256"))?;
            if digest.len() != 64 || digest.chars().any(|c| !c.is_ascii_hexdigit()) {
                return Err(parse_err("outcome digest is not a SHA-256"));
            }
        }
        "opened" => {
            deny_unknown(outcome, &["type", "dev", "ino"], "outcome")?;
            require_u64(outcome.get("dev"), "outcome dev")?;
            require_u64(outcome.get("ino"), "outcome ino")?;
        }
        "absent" => deny_unknown(outcome, &["type"], "outcome")?,
        "refusal" => {
            deny_unknown(outcome, &["type", "reason"], "outcome")?;
            if !outcome
                .get("reason")
                .is_some_and(serde_json::Value::is_string)
            {
                return Err(parse_err("outcome reason is not a string"));
            }
        }
        "facts" => {
            deny_unknown(outcome, &["type", "facts"], "outcome")?;
            if !outcome
                .get("facts")
                .is_some_and(serde_json::Value::is_string)
            {
                return Err(parse_err("outcome facts is not a string"));
            }
        }
        other => return Err(parse_err(format!("unknown outcome type {other}"))),
    }
    Ok(())
}

fn parse_effect(effect: &serde_json::Map<String, serde_json::Value>) -> Result<(), ParseError> {
    deny_unknown(effect, EFFECT_FIELDS, "effect")?;
    require_keys(effect, EFFECT_FIELDS, "effect")?;
    let kind = effect
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if !EFFECT_KINDS.contains(&kind) {
        return Err(parse_err(format!("unknown effect kind {kind}")));
    }
    if !effect.get("site").is_some_and(serde_json::Value::is_string) {
        return Err(parse_err("effect site is not a string"));
    }
    if !effect
        .get("attempted")
        .is_some_and(serde_json::Value::is_boolean)
    {
        return Err(parse_err("effect attempted is not a boolean"));
    }
    if !effect.get("path").is_some_and(serde_json::Value::is_string) {
        return Err(parse_err("effect path is not a string"));
    }
    require_u64(effect.get("epoch"), "effect epoch")?;
    Ok(())
}

/// Strict transcript parse. Success is structure only.
pub(crate) fn parse_transcript(bytes: &[u8]) -> Result<serde_json::Value, ParseError> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(ParseError {
            message: "transcript exceeds the 8 MiB document cap".to_owned(),
        });
    }
    let doc: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| ParseError {
        message: error.to_string(),
    })?;
    let map = as_object(&doc, "transcript")?;
    deny_unknown(map, TRANSCRIPT_FIELDS, "transcript")?;
    require_keys(
        map,
        &[
            "engine",
            "case",
            "manifest_sha256",
            "entrypoint",
            "outcome",
            "truncation",
            "effects",
            "reads",
        ],
        "transcript",
    )?;
    schema_name(map, TRANSCRIPT_SCHEMA)?;
    let reads = map
        .get("reads")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ParseError {
            message: "reads is not an array".to_owned(),
        })?;
    if reads.len() > MAX_READ_ROWS {
        return Err(ParseError {
            message: "reads exceed the 8192-row cap".to_owned(),
        });
    }
    for row in reads {
        let row = as_object(row, "read row")?;
        deny_unknown(row, READ_FIELDS, "read row")?;
        require_keys(row, READ_FIELDS, "read row")?;
        let site = row.get("site").and_then(|v| v.as_str()).unwrap_or("");
        let kind = row.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if site.is_empty() || kind.is_empty() {
            return Err(ParseError {
                message: "read row missing site".to_owned(),
            });
        }
        parse_identity(&row["identity"])?;
        parse_outcome(&row["outcome"])?;
        require_u64(row.get("cap"), "read row cap")?;
        require_u64(row.get("epoch"), "read row epoch")?;
        let occurrence = require_u64(row.get("occurrence"), "read row occurrence")?;
        if occurrence == 0 || occurrence > MAX_OCCURRENCES_PER_KEY {
            return Err(ParseError {
                message: "read row occurrence is outside 1..=64".to_owned(),
            });
        }
    }
    let effects = map
        .get("effects")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ParseError {
            message: "effects is not an array".to_owned(),
        })?;
    for effect in effects {
        parse_effect(as_object(effect, "effect")?)?;
    }
    if let Some(holders) = map.get("recorded_holders").and_then(|v| v.as_array()) {
        if holders.len() > MAX_PROCESS_ROWS {
            return Err(ParseError {
                message: "process rows exceed the 512-row cap".to_owned(),
            });
        }
    }
    Ok(doc)
}

const MANIFEST_FIELDS: &[&str] = &[
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

const OBLIGATION_FIELDS: &[&str] = &["engine", "site", "identity", "parent_required"];

/// Strict manifest parse. `parent_required` is mandatory on every obligation.
pub(crate) fn parse_manifest(bytes: &[u8]) -> Result<serde_json::Value, ParseError> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(ParseError {
            message: "manifest exceeds the 8 MiB document cap".to_owned(),
        });
    }
    let doc: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| ParseError {
        message: error.to_string(),
    })?;
    let map = as_object(&doc, "manifest")?;
    deny_unknown(map, MANIFEST_FIELDS, "manifest")?;
    require_keys(
        map,
        &[
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
        ],
        "manifest",
    )?;
    schema_name(map, MANIFEST_SCHEMA)?;
    let obligations = map
        .get("mandatory_observations")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ParseError {
            message: "mandatory_observations is not an array".to_owned(),
        })?;
    for obligation in obligations {
        let obligation = as_object(obligation, "obligation")?;
        deny_unknown(obligation, OBLIGATION_FIELDS, "obligation")?;
        require_keys(obligation, OBLIGATION_FIELDS, "obligation")?;
        let engine = obligation
            .get("engine")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !matches!(engine, "oxide" | "barge") {
            return Err(ParseError {
                message: format!("unknown obligation engine {engine}"),
            });
        }
        let site = obligation
            .get("site")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if site.is_empty() {
            return Err(ParseError {
                message: "obligation missing site".to_owned(),
            });
        }
        if !obligation
            .get("parent_required")
            .is_some_and(serde_json::Value::is_boolean)
        {
            return Err(ParseError {
                message: "obligation missing parent_required".to_owned(),
            });
        }
        let identity = as_object(&obligation["identity"], "obligation identity")?;
        type_check_identity(identity, false)?;
    }
    let effects = map
        .get("expected_effects")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ParseError {
            message: "expected_effects is not an array".to_owned(),
        })?;
    for effect in effects {
        let kind = effect.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if !EFFECT_KINDS.contains(&kind) {
            return Err(ParseError {
                message: format!("unknown effect kind {kind}"),
            });
        }
    }
    Ok(doc)
}

fn parent_source_of(row: &serde_json::Value) -> Provenance {
    Provenance::parse(row.pointer("/identity/parent_source")).unwrap_or(Provenance::None)
}

fn identity_part(value: Option<&serde_json::Value>) -> String {
    match value {
        None | Some(serde_json::Value::Null) => "none".to_owned(),
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Number(number)) => number.to_string(),
        Some(other) => other.to_string(),
    }
}

fn identity_key(row: &serde_json::Value) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        row.get("site").and_then(|v| v.as_str()).unwrap_or(""),
        identity_part(row.pointer("/identity/path")),
        identity_part(row.pointer("/identity/component")),
        identity_part(row.pointer("/identity/parent_device")),
        identity_part(row.pointer("/identity/parent_inode")),
    )
}

fn identity_fields_match(
    obligation_identity: &serde_json::Value,
    row_identity: &serde_json::Value,
) -> bool {
    let Some(want) = obligation_identity.as_object() else {
        return false;
    };
    let Some(got) = row_identity.as_object() else {
        return false;
    };
    for (key, expected) in want {
        if key == "parent_source" || key == "own_source" {
            continue;
        }
        match got.get(key) {
            Some(actual) if actual == expected => {}
            _ => return false,
        }
    }
    true
}

fn effect_bag(effects: &[serde_json::Value]) -> Vec<(String, String, String)> {
    let mut bag: Vec<(String, String, String)> = effects
        .iter()
        .map(|effect| {
            (
                effect
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                effect
                    .get("site")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                effect
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            )
        })
        .collect();
    bag.sort();
    bag
}

/// Completeness against an optional manifest. Never inferred from structure.
pub(crate) fn assess_completeness(
    transcript: &serde_json::Value,
    manifest: Option<&serde_json::Value>,
) -> Completeness {
    let mut issues = Vec::new();
    let truncation = transcript.get("truncation");
    for flag in [
        "read_rows_truncated",
        "occurrences_truncated",
        "document_truncated",
        "process_rows_truncated",
        "classifications_truncated",
    ] {
        if truncation
            .and_then(|t| t.get(flag))
            .and_then(|v| v.as_bool())
            == Some(true)
        {
            issues.push(format!("truncated: {flag}"));
        }
    }
    let reads = transcript
        .get("reads")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut seen: std::collections::BTreeMap<(String, u64), String> =
        std::collections::BTreeMap::new();
    for row in &reads {
        let key = identity_key(row);
        let occurrence = row.get("occurrence").and_then(|v| v.as_u64()).unwrap_or(0);
        let outcome = row
            .get("outcome")
            .map(ToString::to_string)
            .unwrap_or_default();
        if let Some(previous) = seen.insert((key.clone(), occurrence), outcome.clone()) {
            if previous != outcome {
                issues.push(format!("collision at {key} occurrence {occurrence}"));
            }
        }
    }
    if let Some(manifest) = manifest {
        let transcript_case = transcript.get("case").and_then(|v| v.as_str());
        let manifest_case = manifest.get("case").and_then(|v| v.as_str());
        if transcript_case != manifest_case {
            issues.push("source/run case mismatch".to_owned());
        }
        let digest = transcript
            .get("manifest_sha256")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if digest.len() != 64 || digest.chars().any(|c| !c.is_ascii_hexdigit()) {
            issues.push("transcript manifest digest is not a SHA-256".to_owned());
        }
        if let Some(obligations) = manifest
            .get("mandatory_observations")
            .and_then(|v| v.as_array())
        {
            for obligation in obligations {
                if obligation.get("engine").and_then(|v| v.as_str())
                    != transcript.get("engine").and_then(|v| v.as_str())
                {
                    continue;
                }
                let site = obligation
                    .get("site")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let parent_required = match obligation.get("parent_required") {
                    Some(serde_json::Value::Bool(value)) => *value,
                    _ => {
                        issues.push(format!("obligation missing parent_required at {site}"));
                        continue;
                    }
                };
                let at_site: Vec<&serde_json::Value> = reads
                    .iter()
                    .filter(|row| row.get("site").and_then(|v| v.as_str()) == Some(site))
                    .collect();
                if at_site.is_empty() {
                    issues.push(format!("missing obligation at {site}"));
                    continue;
                }
                let identity_matched = at_site
                    .iter()
                    .any(|row| identity_fields_match(&obligation["identity"], &row["identity"]));
                if !identity_matched {
                    issues.push(format!("wrong identity at {site}"));
                    continue;
                }
                if parent_required {
                    let discharged = at_site.iter().any(|row| {
                        identity_fields_match(&obligation["identity"], &row["identity"])
                            && parent_source_of(row) == Provenance::Handle
                            && K1_PREMISE_QUALIFIED
                    });
                    if !discharged {
                        issues.push(format!(
                            "parent_required unproven at {site}: handle parent still needs qualified K1"
                        ));
                    }
                }
            }
        }
        if let Some(declared) = manifest.get("expected_effects").and_then(|v| v.as_array()) {
            let observed = transcript
                .get("effects")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let want = effect_bag(declared);
            let got = effect_bag(&observed);
            if want.len() != got.len() {
                issues.push("omitted or extra effects".to_owned());
            } else if want != got {
                issues.push("substituted effects".to_owned());
            }
        }
    } else {
        issues.push("no manifest".to_owned());
    }
    Completeness { issues }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct Vector {
    name: &'static str,
    bytes: &'static [u8],
    digest: &'static str,
    kind: VectorKind,
}

enum VectorKind {
    ValidTranscript,
    InvalidTranscript,
    ValidManifest,
    InvalidManifest,
}

const VECTORS: &[Vector] = &[
    Vector {
        name: "valid/path-only-unproven.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/valid/path-only-unproven.json"
        ),
        digest: "d119a519c5fe050607117c43aaee32a7d9d474a965bcc0e7dae3b54a94c80563",
        kind: VectorKind::ValidTranscript,
    },
    Vector {
        name: "valid/handle-parent.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/valid/handle-parent.json"
        ),
        digest: "1eeef64d45322210b4501775f36aa9dd2e120404d5357fc942b471a72ad05a3b",
        kind: VectorKind::ValidTranscript,
    },
    Vector {
        name: "valid/absent.json",
        bytes: include_bytes!("../../../docs/knowledge/contracts/capture-vectors/valid/absent.json"),
        digest: "27c1d50d7317d4d1ec01c4eacf3af853eb65976076e9cbe56ff8e99c53ef30c5",
        kind: VectorKind::ValidTranscript,
    },
    Vector {
        name: "valid/refused.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/valid/refused.json"
        ),
        digest: "0e7530bd79edec6aad1499617b15f35e4622d120e1229fffa771480c62462fc6",
        kind: VectorKind::ValidTranscript,
    },
    Vector {
        name: "valid/truncated.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/valid/truncated.json"
        ),
        digest: "6c277878701a630394a9c2b9d6799672bda406ea21e34fd73bf62a74d0a85282",
        kind: VectorKind::ValidTranscript,
    },
    Vector {
        name: "invalid/unknown-version.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/unknown-version.json"
        ),
        digest: "5a9365d81d66b4b1eaddf55c773e6434d528e4530530aadb6d9572a8bf24fd69",
        kind: VectorKind::InvalidTranscript,
    },
    Vector {
        name: "invalid/unknown-field.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/unknown-field.json"
        ),
        digest: "c794624c469a4089b86a9669503415718f183888b876f8537f8d9795e0fd59fb",
        kind: VectorKind::InvalidTranscript,
    },
    Vector {
        name: "invalid/missing-site.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/missing-site.json"
        ),
        digest: "60bbe57b5546d510621684a1d7874247ba7e2bea6cdc04cfbb71a541a93485d2",
        kind: VectorKind::InvalidTranscript,
    },
    Vector {
        name: "invalid/bad-provenance.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/bad-provenance.json"
        ),
        digest: "d0d296b16873c798c78d0984bc848f7c57ef34df6050c09a3ec88ce75db55734",
        kind: VectorKind::InvalidTranscript,
    },
    Vector {
        name: "invalid/collision.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/collision.json"
        ),
        digest: "586b01537500d475945f27956d9e62957256fcf004ccc0e40f928490e7a60683",
        kind: VectorKind::ValidTranscript,
    },
    Vector {
        name: "invalid/source-mismatch.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/source-mismatch.json"
        ),
        digest: "0c947a92549ca49ce13e89a2070034d36c7a8d91e4e233e790cfeb60e78f1b17",
        kind: VectorKind::ValidTranscript,
    },
    Vector {
        name: "invalid/unknown-effect.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/unknown-effect.json"
        ),
        digest: "d608c47662440ff1778b167fab4e6668c2eaeb3cc75653da1e6ea69fcd2878cb",
        kind: VectorKind::InvalidTranscript,
    },
    Vector {
        name: "invalid/string-device.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/string-device.json"
        ),
        digest: "bc96a18bb1be86b6c3b1b704f579736667b44b9482af4df7c4a41f41d184f11d",
        kind: VectorKind::InvalidTranscript,
    },
    Vector {
        name: "invalid/string-parent-inode.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/string-parent-inode.json"
        ),
        digest: "efd944bf6a6f762e72fe631bb037c0a2c8a91910f523aff505d709afd84928dd",
        kind: VectorKind::InvalidTranscript,
    },
    Vector {
        name: "invalid/occurrence-zero.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/occurrence-zero.json"
        ),
        digest: "9064a508b7c35bcf284ce82cbfa040b272a7003a80e8688618e6781066ffe010",
        kind: VectorKind::InvalidTranscript,
    },
    Vector {
        name: "invalid/wrong-parent.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/wrong-parent.json"
        ),
        digest: "78e0c54f28ba6fb7e36ea639cca2295c61ae8915f9928a262f5c8816731be858",
        kind: VectorKind::ValidTranscript,
    },
    Vector {
        name: "invalid/wrong-inode.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/wrong-inode.json"
        ),
        digest: "c466a002728c7cde344b8e6a15a1fbf1fe96a36fc2fbf65a2498b183d2b70ae7",
        kind: VectorKind::ValidTranscript,
    },
    Vector {
        name: "invalid/omitted-obligation.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/omitted-obligation.json"
        ),
        digest: "f393749cd739660f100a80bb30bc13266627c3dedef35e7e7b27b4be7c1553e0",
        kind: VectorKind::ValidTranscript,
    },
    Vector {
        name: "invalid/extra-effect.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/extra-effect.json"
        ),
        digest: "16d01f5db93dea43de7267697989c09b6b4f66d9fd84b4ac56230b90f874f555",
        kind: VectorKind::ValidTranscript,
    },
    Vector {
        name: "invalid/substituted-effect.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/substituted-effect.json"
        ),
        digest: "da294866f4b9490890688e67456496614a6fadd0c3841f139bf34d98abf4cd5c",
        kind: VectorKind::ValidTranscript,
    },
    Vector {
        name: "invalid/omitted-effect.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/omitted-effect.json"
        ),
        digest: "41c6223d939f4dcb4dac7f514b16ac4662189acf3b9b37d797f1f0c71c5c3d56",
        kind: VectorKind::ValidTranscript,
    },
    Vector {
        name: "manifest/valid-parent-required.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/manifest/valid-parent-required.json"
        ),
        digest: "ee622767bfef80d78cb5025235faa8da380c10a01f7325067f1c19b094a5641a",
        kind: VectorKind::ValidManifest,
    },
    Vector {
        name: "manifest/invalid-missing-parent-required.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/manifest/invalid-missing-parent-required.json"
        ),
        digest: "b2a8f8a02a41d6bcea5b42e11aca714f6f25e2d57d92d29dd3ac1c7f0c3603e1",
        kind: VectorKind::InvalidManifest,
    },
    Vector {
        name: "manifest/invalid-extra-field.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/manifest/invalid-extra-field.json"
        ),
        digest: "f494fe90b091129428b37d4d56b033cb2a20ff35530af80eed290b038d30d999",
        kind: VectorKind::InvalidManifest,
    },
];

fn vector_manifest(
    case: &str,
    site: &str,
    parent_required: bool,
    identity: serde_json::Value,
    obligations: bool,
    expected_effects: serde_json::Value,
) -> serde_json::Value {
    let mandatory_observations = if obligations {
        serde_json::json!([{
            "engine": "oxide",
            "site": site,
            "parent_required": parent_required,
            "identity": identity
        }])
    } else {
        serde_json::json!([])
    };
    serde_json::json!({
        "schema": MANIFEST_SCHEMA,
        "case": case,
        "window": {"identity": "vector", "start_monotonic_ns": 1, "end_monotonic_ns": 2},
        "source": {
            "oxide_commit": "vector",
            "oxide_files": {},
            "cargo_lock_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
            "rust_toolchain_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
            "barge_pin": "4af9360bd09629e970c143080ca159de8eef2562",
            "barge_slice_commit": "vector",
            "barge_files": {},
            "staged_lock_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
            "oxide_toolchain": "vector",
            "barge_toolchain": "vector",
            "oxide_executable_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
            "barge_executable_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
            "oxide_tree": "0000000000000000000000000000000000000000"
        },
        "fixture": {"root": "/fixture", "device": 1, "inode": 2, "mode": 448, "entries": [], "roots": []},
        "dynamic": {"volumes": [], "writers": [], "external_allowances": []},
        "mandatory_observations": mandatory_observations,
        "expected_effects": expected_effects,
        "expected": {"classification": "MATCH", "roots": []},
        "barge": {"volume_uuid": "0", "namespace": "oxide", "area": "outputs"}
    })
}

fn completeness_fixture(name: &str, transcript_case: &str) -> serde_json::Value {
    match name {
        "valid/path-only-unproven.json" => vector_manifest(
            transcript_case,
            "prune::read_registry",
            true,
            serde_json::json!({"path": "/fixture/registry", "device": 3, "inode": 4}),
            true,
            serde_json::json!([]),
        ),
        "valid/handle-parent.json" => vector_manifest(
            transcript_case,
            "root-observe:entry",
            true,
            serde_json::json!({
                "component": "oxide-ra",
                "parent_device": 1,
                "parent_inode": 2,
                "device": 3,
                "inode": 4
            }),
            true,
            serde_json::json!([]),
        ),
        "valid/absent.json" => vector_manifest(
            transcript_case,
            "config:inspect:initial",
            false,
            serde_json::json!({"path": "/fixture/missing"}),
            true,
            serde_json::json!([]),
        ),
        "valid/refused.json" => vector_manifest(
            transcript_case,
            "prune::read_registry",
            false,
            serde_json::json!({"path": "/fixture/registry"}),
            true,
            serde_json::json!([]),
        ),
        "valid/truncated.json" => vector_manifest(
            transcript_case,
            "pure:read",
            false,
            serde_json::json!({"path": "/pure/object", "device": 3, "inode": 4}),
            true,
            serde_json::json!([]),
        ),
        "invalid/collision.json" => vector_manifest(
            transcript_case,
            "pure:read",
            false,
            serde_json::json!({"path": "/pure/object", "device": 3, "inode": 4}),
            true,
            serde_json::json!([]),
        ),
        "invalid/source-mismatch.json" => vector_manifest(
            "OTHER-CASE",
            "prune::read_registry",
            false,
            serde_json::json!({"path": "/fixture/registry"}),
            false,
            serde_json::json!([]),
        ),
        "invalid/wrong-parent.json" => vector_manifest(
            transcript_case,
            "root-observe:entry",
            false,
            serde_json::json!({
                "component": "oxide-ra",
                "parent_device": 1,
                "parent_inode": 2,
                "device": 3,
                "inode": 4
            }),
            true,
            serde_json::json!([]),
        ),
        "invalid/wrong-inode.json" => vector_manifest(
            transcript_case,
            "prune::read_registry",
            false,
            serde_json::json!({"path": "/fixture/registry", "device": 3, "inode": 99}),
            true,
            serde_json::json!([]),
        ),
        "invalid/omitted-obligation.json" => vector_manifest(
            transcript_case,
            "prune::read_registry",
            false,
            serde_json::json!({"path": "/fixture/registry"}),
            true,
            serde_json::json!([]),
        ),
        "invalid/extra-effect.json" => vector_manifest(
            transcript_case,
            "prune::read_registry",
            false,
            serde_json::json!({"path": "/fixture/registry"}),
            false,
            serde_json::json!([]),
        ),
        "invalid/omitted-effect.json" => vector_manifest(
            transcript_case,
            "prune::read_registry",
            false,
            serde_json::json!({"path": "/fixture/registry"}),
            false,
            serde_json::json!([{
                "kind": "save",
                "site": "ledger",
                "path": "/fixture/ledger"
            }]),
        ),
        "invalid/substituted-effect.json" => vector_manifest(
            transcript_case,
            "prune::read_registry",
            false,
            serde_json::json!({"path": "/fixture/registry"}),
            false,
            serde_json::json!([{
                "kind": "unlink",
                "site": "other",
                "path": "/fixture/other"
            }]),
        ),
        _ => vector_manifest(
            transcript_case,
            "prune::read_registry",
            false,
            serde_json::json!({"path": "/fixture/registry"}),
            true,
            serde_json::json!([]),
        ),
    }
}

fn expected_issues(name: &str) -> &'static [&'static str] {
    match name {
        "valid/path-only-unproven.json" => &[
            "parent_required unproven at prune::read_registry: handle parent still needs qualified K1",
        ],
        "valid/handle-parent.json" => &[
            "parent_required unproven at root-observe:entry: handle parent still needs qualified K1",
        ],
        "valid/absent.json" | "valid/refused.json" => &[],
        "valid/truncated.json" => &["truncated: read_rows_truncated"],
        "invalid/collision.json" => {
            &["collision at pure:read|/pure/object|none|none|none occurrence 1"]
        }
        "invalid/source-mismatch.json" => &["source/run case mismatch"],
        "invalid/wrong-parent.json" => &["wrong identity at root-observe:entry"],
        "invalid/wrong-inode.json" => &["wrong identity at prune::read_registry"],
        "invalid/omitted-obligation.json" => &["missing obligation at prune::read_registry"],
        "invalid/extra-effect.json" | "invalid/omitted-effect.json" => {
            &["omitted or extra effects"]
        }
        "invalid/substituted-effect.json" => &["substituted effects"],
        _ => &[],
    }
}

fn path_only_bytes() -> &'static [u8] {
    VECTORS
        .iter()
        .find(|vector| vector.name == "valid/path-only-unproven.json")
        .expect("path-only vector")
        .bytes
}

fn cap_checks() -> Result<usize, String> {
    let mut checked = 0usize;
    let oversize = vec![b'x'; MAX_DOCUMENT_BYTES + 1];
    if parse_transcript(&oversize).is_ok() {
        return Err("8 MiB+1 document must fail parse".to_owned());
    }
    checked += 1;

    let mut occurrence = serde_json::from_slice::<serde_json::Value>(path_only_bytes())
        .map_err(|error| error.to_string())?;
    occurrence["reads"][0]["occurrence"] = serde_json::json!(65);
    let bytes = serde_json::to_vec(&occurrence).map_err(|error| error.to_string())?;
    if parse_transcript(&bytes).is_ok() {
        return Err("occurrence 65 must fail parse".to_owned());
    }
    checked += 1;

    let mut reads = serde_json::from_slice::<serde_json::Value>(path_only_bytes())
        .map_err(|error| error.to_string())?;
    let row = reads["reads"][0].clone();
    reads["reads"] = serde_json::Value::Array(vec![row; MAX_READ_ROWS + 1]);
    let bytes = serde_json::to_vec(&reads).map_err(|error| error.to_string())?;
    if parse_transcript(&bytes).is_ok() {
        return Err("8193 reads must fail parse".to_owned());
    }
    checked += 1;

    let mut holders = serde_json::from_slice::<serde_json::Value>(path_only_bytes())
        .map_err(|error| error.to_string())?;
    holders["recorded_holders"] =
        serde_json::Value::Array(vec![serde_json::json!({"pid": 1}); MAX_PROCESS_ROWS + 1]);
    let bytes = serde_json::to_vec(&holders).map_err(|error| error.to_string())?;
    if parse_transcript(&bytes).is_ok() {
        return Err("513 process rows must fail parse".to_owned());
    }
    checked += 1;

    let parsed = parse_transcript(path_only_bytes()).map_err(|error| error.message)?;
    let without = assess_completeness(&parsed, None);
    if !without.issues.iter().any(|issue| issue == "no manifest") || without.ok() {
        return Err("assess_completeness without a manifest must issue no manifest".to_owned());
    }
    checked += 1;

    let mut missing = completeness_fixture("valid/path-only-unproven.json", "VECTOR-PATH-ONLY");
    missing["mandatory_observations"][0]
        .as_object_mut()
        .ok_or("path-only fixture obligation is not an object")?
        .remove("parent_required");
    let missing_issues = assess_completeness(&parsed, Some(&missing));
    if !missing_issues
        .issues
        .iter()
        .any(|issue| issue.contains("missing parent_required"))
    {
        return Err("missing parent_required must be an issue, not false".to_owned());
    }
    checked += 1;
    Ok(checked)
}

/// Pure parser, completeness and hashed-vector checks. Invoked from the
/// harness `--self-check` so the `harness = false` target actually runs them.
pub(crate) fn run_self_checks() -> Result<(), String> {
    let mut failed = Vec::new();
    let mut checked = 0usize;
    for vector in VECTORS {
        let digest = sha256_hex(vector.bytes);
        if digest != vector.digest {
            failed.push(format!(
                "{} digest {} != catalog {}",
                vector.name, digest, vector.digest
            ));
            continue;
        }
        match vector.kind {
            VectorKind::ValidTranscript => {
                let parsed = match parse_transcript(vector.bytes) {
                    Ok(doc) => doc,
                    Err(error) => {
                        failed.push(format!("{} parse: {}", vector.name, error.message));
                        continue;
                    }
                };
                let case = parsed
                    .get("case")
                    .and_then(|v| v.as_str())
                    .unwrap_or("VECTOR");
                let manifest = completeness_fixture(vector.name, case);
                let completeness = assess_completeness(&parsed, Some(&manifest));
                let expected: Vec<String> = expected_issues(vector.name)
                    .iter()
                    .map(|issue| (*issue).to_owned())
                    .collect();
                if completeness.issues != expected {
                    failed.push(format!(
                        "{} issues {:?} != {:?}",
                        vector.name, completeness.issues, expected
                    ));
                }
                if vector.name.contains("handle-parent") && K1_PREMISE_QUALIFIED {
                    failed.push("K1_PREMISE_QUALIFIED must stay false on this landing".to_owned());
                }
                checked += 1;
            }
            VectorKind::InvalidTranscript => {
                if parse_transcript(vector.bytes).is_ok() {
                    failed.push(format!("{} should fail parse", vector.name));
                } else {
                    checked += 1;
                }
            }
            VectorKind::ValidManifest => {
                if let Err(error) = parse_manifest(vector.bytes) {
                    failed.push(format!("{} parse: {}", vector.name, error.message));
                } else {
                    checked += 1;
                }
            }
            VectorKind::InvalidManifest => {
                if parse_manifest(vector.bytes).is_ok() {
                    failed.push(format!("{} should fail parse", vector.name));
                } else {
                    checked += 1;
                }
            }
        }
    }
    match cap_checks() {
        Ok(extra) => checked += extra,
        Err(error) => failed.push(error),
    }
    if PROVENANCE.len() != 4 {
        failed.push("provenance vocabulary drifted".to_owned());
    }
    if K1_PREMISE_QUALIFIED {
        failed
            .push("K1 is not qualified; completeness must not succeed on handle parent".to_owned());
    }
    let catalog = include_str!("../../../docs/knowledge/contracts/capture-vectors/SHA256SUMS");
    for vector in VECTORS {
        if !catalog.contains(vector.digest) {
            failed.push(format!("{} missing from SHA256SUMS", vector.name));
        }
    }
    if failed.is_empty() {
        println!("oxide: capture-wire: {checked} vector checks passed");
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}
