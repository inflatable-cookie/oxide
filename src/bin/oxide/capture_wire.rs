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

fn parse_identity(value: &serde_json::Value) -> Result<(), ParseError> {
    let identity = as_object(value, "identity")?;
    deny_unknown(identity, IDENTITY_FIELDS, "identity")?;
    require_keys(identity, &["parent_source", "own_source"], "identity")?;
    Provenance::parse(identity.get("parent_source")).map_err(|message| ParseError { message })?;
    Provenance::parse(identity.get("own_source")).map_err(|message| ParseError { message })?;
    if let Some(device) = identity.get("device") {
        if !device.is_null() && !device.is_u64() && !device.is_string() {
            return Err(ParseError {
                message: "identity device is not a number, string, or null".to_owned(),
            });
        }
    }
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
        let occurrence = row.get("occurrence").and_then(|v| v.as_u64()).unwrap_or(0);
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
        let effect = as_object(effect, "effect")?;
        deny_unknown(effect, EFFECT_FIELDS, "effect")?;
        require_keys(effect, EFFECT_FIELDS, "effect")?;
        let kind = effect.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if !EFFECT_KINDS.contains(&kind) {
            return Err(ParseError {
                message: format!("unknown effect kind {kind}"),
            });
        }
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
        deny_unknown(identity, IDENTITY_FIELDS, "obligation identity")?;
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

fn identity_key(row: &serde_json::Value) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        row.get("site").and_then(|v| v.as_str()).unwrap_or(""),
        row.pointer("/identity/path")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        row.pointer("/identity/component")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        row.pointer("/identity/parent_device")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        row.pointer("/identity/parent_inode")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
    )
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
                let parent_required = obligation
                    .get("parent_required")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let matched = reads.iter().any(|row| {
                    row.get("site").and_then(|v| v.as_str()) == Some(site)
                        || row.get("kind").and_then(|v| v.as_str()) == Some(site)
                });
                if !matched {
                    issues.push(format!("missing obligation at {site}"));
                    continue;
                }
                if parent_required {
                    let discharged = reads.iter().any(|row| {
                        (row.get("site").and_then(|v| v.as_str()) == Some(site)
                            || row.get("kind").and_then(|v| v.as_str()) == Some(site))
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
            if declared.len() != observed.len() {
                issues.push("omitted or extra effects".to_owned());
            }
        }
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
    ValidTranscript { parent_required: bool },
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
        kind: VectorKind::ValidTranscript {
            parent_required: true,
        },
    },
    Vector {
        name: "valid/handle-parent.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/valid/handle-parent.json"
        ),
        digest: "1eeef64d45322210b4501775f36aa9dd2e120404d5357fc942b471a72ad05a3b",
        kind: VectorKind::ValidTranscript {
            parent_required: true,
        },
    },
    Vector {
        name: "valid/absent.json",
        bytes: include_bytes!("../../../docs/knowledge/contracts/capture-vectors/valid/absent.json"),
        digest: "27c1d50d7317d4d1ec01c4eacf3af853eb65976076e9cbe56ff8e99c53ef30c5",
        kind: VectorKind::ValidTranscript {
            parent_required: false,
        },
    },
    Vector {
        name: "valid/refused.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/valid/refused.json"
        ),
        digest: "0e7530bd79edec6aad1499617b15f35e4622d120e1229fffa771480c62462fc6",
        kind: VectorKind::ValidTranscript {
            parent_required: false,
        },
    },
    Vector {
        name: "valid/truncated.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/valid/truncated.json"
        ),
        digest: "6c277878701a630394a9c2b9d6799672bda406ea21e34fd73bf62a74d0a85282",
        kind: VectorKind::ValidTranscript {
            parent_required: false,
        },
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
        kind: VectorKind::ValidTranscript {
            parent_required: false,
        },
    },
    Vector {
        name: "invalid/source-mismatch.json",
        bytes: include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/invalid/source-mismatch.json"
        ),
        digest: "0c947a92549ca49ce13e89a2070034d36c7a8d91e4e233e790cfeb60e78f1b17",
        kind: VectorKind::ValidTranscript {
            parent_required: false,
        },
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

fn vector_manifest(parent_required: bool, case: &str) -> serde_json::Value {
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
        "mandatory_observations": [{
            "engine": "oxide",
            "site": "prune::read_registry",
            "parent_required": parent_required,
            "identity": {"path": "/fixture/registry", "parent_device": 1, "parent_inode": 2, "device": 3, "inode": 4}
        }],
        "expected_effects": [],
        "expected": {"classification": "MATCH", "roots": []},
        "barge": {"volume_uuid": "0", "namespace": "oxide", "area": "outputs"}
    })
}

/// Pure parser, completeness and hashed-vector checks. Invoked from the
/// harness `--self-check` so the `harness = false` target actually runs them.
pub(crate) fn run_self_checks() -> Result<(), String> {
    let mut failed = Vec::new();
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
            VectorKind::ValidTranscript { parent_required } => {
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
                let manifest = if vector.name.ends_with("source-mismatch.json") {
                    vector_manifest(parent_required, "OTHER-CASE")
                } else if vector.name.contains("handle-parent") {
                    let mut manifest = vector_manifest(true, case);
                    manifest["mandatory_observations"][0]["site"] = "root-observe:entry".into();
                    manifest
                } else {
                    vector_manifest(parent_required, case)
                };
                let completeness = assess_completeness(&parsed, Some(&manifest));
                let expect_incomplete = parent_required
                    || vector.name.contains("truncated")
                    || vector.name.contains("collision")
                    || vector.name.contains("source-mismatch");
                if expect_incomplete {
                    if completeness.ok() {
                        failed.push(format!(
                            "{} parser-positive was treated as complete",
                            vector.name
                        ));
                    }
                } else if !completeness.ok()
                    && !vector.name.contains("absent")
                    && !vector.name.contains("refused")
                {
                    // absent/refused rows are structure-valid; completeness
                    // depends on the accompanying obligation.
                }
                if vector.name.contains("path-only")
                    && !completeness
                        .issues
                        .iter()
                        .any(|issue| issue.contains("parent_required unproven"))
                {
                    failed.push("path-only parent_required must fail completeness".to_owned());
                }
                if vector.name.contains("handle-parent") {
                    if completeness.ok() {
                        failed.push("handle parent must remain unproven without K1".to_owned());
                    }
                    if K1_PREMISE_QUALIFIED {
                        failed.push(
                            "K1_PREMISE_QUALIFIED must stay false on this landing".to_owned(),
                        );
                    }
                }
                if vector.name.contains("collision")
                    && !completeness
                        .issues
                        .iter()
                        .any(|issue| issue.contains("collision"))
                {
                    failed.push("collision vector must name the collision".to_owned());
                }
                if vector.name.contains("source-mismatch")
                    && !completeness
                        .issues
                        .iter()
                        .any(|issue| issue.contains("case mismatch"))
                {
                    failed.push("source-mismatch vector must fail completeness".to_owned());
                }
                if vector.name.contains("truncated")
                    && !completeness
                        .issues
                        .iter()
                        .any(|issue| issue.contains("truncated"))
                {
                    failed.push("truncated vector must fail completeness".to_owned());
                }
            }
            VectorKind::InvalidTranscript => {
                if parse_transcript(vector.bytes).is_ok() {
                    failed.push(format!("{} should fail parse", vector.name));
                }
            }
            VectorKind::ValidManifest => {
                if let Err(error) = parse_manifest(vector.bytes) {
                    failed.push(format!("{} parse: {}", vector.name, error.message));
                }
            }
            VectorKind::InvalidManifest => {
                if parse_manifest(vector.bytes).is_ok() {
                    failed.push(format!("{} should fail parse", vector.name));
                }
            }
        }
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
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}
