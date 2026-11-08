//! Shared capture wire: `oxide.capture.transcript/2` and
//! `oxide.capture.manifest/2` parse, completeness and the one identity and
//! effect-parity rule the in-window harness also uses.
//!
//! `/2` is the current test-only wire. A `/1` document is refused as proof
//! input, never upgraded or reinterpreted. Parser-positive proves structure
//! only. Completeness is a separate judgement: path-only `parent_required`
//! rows fail; `handle` parent still fails while [`K1_PREMISE_QUALIFIED`] is
//! false; a recorded holder without a kernel start and a clock without a
//! value are unproven; and only an attempted or unsucceeded effect is never
//! completed parity. `derived` is not a row provenance class.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Transcript schema. Every `/1` field name and vocabulary is refused.
pub(crate) const TRANSCRIPT_SCHEMA: &str = "oxide.capture.transcript/2";
/// Manifest schema.
pub(crate) const MANIFEST_SCHEMA: &str = "oxide.capture.manifest/2";
/// Numeric identity and lifetime premise for a handle parent. Unqualified:
/// a closed `(device, inode)` pair does not prove the parent of the read.
pub(crate) const K1_PREMISE_QUALIFIED: bool = false;

const MAX_READ_ROWS: usize = 8192;
const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
const MAX_OCCURRENCES_PER_KEY: u64 = 64;
const MAX_PROCESS_ROWS: usize = 512;
const MAX_FIXTURE_ENTRIES: usize = 4096;

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

const TRUNCATION_FIELDS: &[&str] = &[
    "read_rows_truncated",
    "occurrences_truncated",
    "document_truncated",
    "process_rows_truncated",
    "classifications_truncated",
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

/// Every transcript identity carries all fifteen fields; absence is `null`.
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
    "parent_source",
    "own_source",
];

/// Manifest obligation identities are partial by design; any listed key may
/// appear, but none may be unknown.
const OBLIGATION_IDENTITY_FIELDS: &[&str] = &[
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

const EFFECT_FIELDS: &[&str] = &["kind", "site", "attempted", "path", "epoch", "result"];
const CLOCK_FIELDS: &[&str] = &["kind", "value", "epoch"];
const CLASSIFY_FIELDS: &[&str] = &["kind", "identity", "value", "epoch", "occurrence"];
const HOLDER_FIELDS: &[&str] = &[
    "site",
    "record_id",
    "pid",
    "recorded_start_ms",
    "kernel_start",
    "uid",
    "epoch",
];
const FIXTURE_FIELDS: &[&str] = &["root", "truncated", "pre", "post", "diff_effects"];
const FIXTURE_ENTRY_FIELDS: &[&str] = &[
    "rel",
    "kind",
    "size",
    "device",
    "inode",
    "parent_device",
    "parent_inode",
    "mode",
    "bytes_sha256",
];
const FIXTURE_ENTRY_KINDS: &[&str] = &["symlink", "dir", "file", "other"];
const EFFECT_KINDS: &[&str] = &[
    "create",
    "save",
    "fence",
    "reconcile",
    "lock-create",
    "rename",
    "unlink",
    "descriptor-effect",
];
/// `descriptor-effect` is an effect on an already-held descriptor whose path
/// the recorder could not bind. It never carries a path.
const DESCRIPTOR_EFFECT: &str = "descriptor-effect";
const EFFECT_RESULTS: &[&str] = &["succeeded", "failed", "partial", "unknown"];

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

type Object = serde_json::Map<String, serde_json::Value>;

fn parse_err(message: impl Into<String>) -> ParseError {
    ParseError {
        message: message.into(),
    }
}

fn object<'a>(value: &'a serde_json::Value, what: &str) -> Result<&'a Object, ParseError> {
    value
        .as_object()
        .ok_or_else(|| parse_err(format!("{what} is not an object")))
}

/// Every listed key must be present (an explicit `null` is a value, not an
/// absence) and no other key may appear.
fn exact_fields(object: &Object, keys: &[&str], what: &str) -> Result<(), ParseError> {
    for key in object.keys() {
        if !keys.contains(&key.as_str()) {
            return Err(parse_err(format!("unknown {what} field {key}")));
        }
    }
    for key in keys {
        if !object.contains_key(*key) {
            return Err(parse_err(format!("{what} missing {key}")));
        }
    }
    Ok(())
}

fn field<'a>(
    object: &'a Object,
    key: &str,
    what: &str,
) -> Result<&'a serde_json::Value, ParseError> {
    object
        .get(key)
        .ok_or_else(|| parse_err(format!("{what} missing {key}")))
}

fn string_field<'a>(object: &'a Object, key: &str, what: &str) -> Result<&'a str, ParseError> {
    field(object, key, what)?
        .as_str()
        .ok_or_else(|| parse_err(format!("{what} {key} is not a string")))
}

fn nonempty_field<'a>(object: &'a Object, key: &str, what: &str) -> Result<&'a str, ParseError> {
    let text = string_field(object, key, what)?;
    if text.is_empty() {
        return Err(parse_err(format!("{what} {key} is empty")));
    }
    Ok(text)
}

fn bool_field(object: &Object, key: &str, what: &str) -> Result<bool, ParseError> {
    field(object, key, what)?
        .as_bool()
        .ok_or_else(|| parse_err(format!("{what} {key} is not a boolean")))
}

fn u64_field(object: &Object, key: &str, what: &str) -> Result<u64, ParseError> {
    field(object, key, what)?
        .as_u64()
        .ok_or_else(|| parse_err(format!("{what} {key} is not a u64")))
}

/// An epoch is engine-local, starts at one and is never zero on a state row.
fn epoch_field(object: &Object, what: &str) -> Result<u64, ParseError> {
    let epoch = u64_field(object, "epoch", what)?;
    if epoch == 0 {
        return Err(parse_err(format!("{what} epoch is zero")));
    }
    Ok(epoch)
}

fn occurrence_field(object: &Object, what: &str) -> Result<(), ParseError> {
    let occurrence = u64_field(object, "occurrence", what)?;
    if occurrence == 0 || occurrence > MAX_OCCURRENCES_PER_KEY {
        return Err(parse_err(format!("{what} occurrence is outside 1..=64")));
    }
    Ok(())
}

fn null_or_u64(object: &Object, key: &str, what: &str) -> Result<Option<u64>, ParseError> {
    let value = field(object, key, what)?;
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_u64()
        .map(Some)
        .ok_or_else(|| parse_err(format!("{what} {key} is not a u64 or null")))
}

fn null_or_u32(object: &Object, key: &str, what: &str) -> Result<(), ParseError> {
    match null_or_u64(object, key, what)? {
        Some(value) if value > u64::from(u32::MAX) => {
            Err(parse_err(format!("{what} {key} is not a u32 or null")))
        }
        _ => Ok(()),
    }
}

fn null_or_string(object: &Object, key: &str, what: &str) -> Result<Option<String>, ParseError> {
    let value = field(object, key, what)?;
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_str()
        .map(|text| Some(text.to_owned()))
        .ok_or_else(|| parse_err(format!("{what} {key} is not a string or null")))
}

fn null_or_i32(object: &Object, key: &str, what: &str) -> Result<(), ParseError> {
    let value = field(object, key, what)?;
    if value.is_null() {
        return Ok(());
    }
    if value
        .as_i64()
        .is_some_and(|n| (0..=i64::from(i32::MAX)).contains(&n))
    {
        Ok(())
    } else {
        Err(parse_err(format!(
            "{what} {key} is not a non-negative i32 or null"
        )))
    }
}

fn is_lower_hex(text: &str, length: usize) -> bool {
    text.len() == length && text.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

fn hex64_field(object: &Object, key: &str, what: &str) -> Result<(), ParseError> {
    let text = string_field(object, key, what)?;
    if is_lower_hex(text, 64) {
        Ok(())
    } else {
        Err(parse_err(format!("{what} {key} is not a SHA-256")))
    }
}

fn provenance_field(object: &Object, key: &str, what: &str) -> Result<Provenance, ParseError> {
    Provenance::parse(Some(field(object, key, what)?)).map_err(parse_err)
}

/// Refuse a `/1` capture document by name before any structural check.
fn refuse_historical(found: &str) -> Result<(), ParseError> {
    if found.starts_with("oxide.capture.") && found.ends_with("/1") {
        return Err(parse_err(format!(
            "historical /1 capture document {found} is not current proof input"
        )));
    }
    Ok(())
}

/// Transcripts name the schema in both `schema` and `version`, and both must
/// be the current string.
fn transcript_schema(doc: &Object) -> Result<(), ParseError> {
    let schema = string_field(doc, "schema", "transcript")?;
    refuse_historical(schema)?;
    let version = string_field(doc, "version", "transcript")?;
    if schema != version {
        return Err(parse_err("schema and version disagree"));
    }
    if schema != TRANSCRIPT_SCHEMA {
        return Err(parse_err(format!("unknown version {schema}")));
    }
    Ok(())
}

/// Parse one identity on a transcript row. Every key is typed explicitly, and
/// the pairs that must agree are checked together.
fn parse_row_identity(value: &serde_json::Value) -> Result<(), ParseError> {
    let identity = object(value, "identity")?;
    exact_fields(identity, IDENTITY_FIELDS, "identity")?;
    for key in ["path", "component", "volume_uuid", "mount", "volume_device"] {
        null_or_string(identity, key, "identity")?;
    }
    for key in ["parent_device", "parent_inode", "device", "inode", "start"] {
        null_or_u64(identity, key, "identity")?;
    }
    null_or_u32(identity, "pid", "identity")?;
    null_or_u32(identity, "uid", "identity")?;
    null_or_i32(identity, "fd", "identity")?;
    let parent_source = provenance_field(identity, "parent_source", "identity")?;
    let own_source = provenance_field(identity, "own_source", "identity")?;
    let parent_null = field(identity, "parent_device", "identity")?.is_null();
    let parent_inode_null = field(identity, "parent_inode", "identity")?.is_null();
    let device_null = field(identity, "device", "identity")?.is_null();
    let inode_null = field(identity, "inode", "identity")?.is_null();
    if parent_null != parent_inode_null {
        return Err(parse_err(
            "parent device and inode must be both present or both null",
        ));
    }
    if device_null != inode_null {
        return Err(parse_err(
            "device and inode must be both present or both null",
        ));
    }
    if parent_null != (parent_source == Provenance::None) {
        return Err(parse_err(
            "parent_source is none exactly when the parent identity is absent",
        ));
    }
    if device_null != (own_source == Provenance::None) {
        return Err(parse_err(
            "own_source is none exactly when the device and inode are absent",
        ));
    }
    Ok(())
}

/// A read outcome in the fixed vocabulary.
fn parse_outcome(value: &serde_json::Value) -> Result<(), ParseError> {
    let outcome = object(value, "outcome")?;
    match string_field(outcome, "type", "outcome")? {
        "digest" => {
            exact_fields(outcome, &["type", "digest"], "outcome")?;
            hex64_field(outcome, "digest", "outcome")?;
        }
        "opened" => {
            exact_fields(outcome, &["type", "dev", "ino"], "outcome")?;
            u64_field(outcome, "dev", "outcome")?;
            u64_field(outcome, "ino", "outcome")?;
        }
        "absent" => exact_fields(outcome, &["type"], "outcome")?,
        "refusal" => {
            exact_fields(outcome, &["type", "reason"], "outcome")?;
            string_field(outcome, "reason", "outcome")?;
        }
        "facts" => {
            exact_fields(outcome, &["type", "facts"], "outcome")?;
            string_field(outcome, "facts", "outcome")?;
        }
        other => return Err(parse_err(format!("unknown outcome type {other}"))),
    }
    Ok(())
}

fn parse_read_row(row: &serde_json::Value) -> Result<u64, ParseError> {
    let row = object(row, "read row")?;
    exact_fields(row, READ_FIELDS, "read row")?;
    let kind = nonempty_field(row, "kind", "read row")?;
    if string_field(row, "site", "read row")? != kind {
        return Err(parse_err("read row site differs from its kind"));
    }
    parse_row_identity(field(row, "identity", "read row")?)?;
    parse_outcome(field(row, "outcome", "read row")?)?;
    null_or_u64(row, "cap", "read row")?;
    let epoch = epoch_field(row, "read row")?;
    occurrence_field(row, "read row")?;
    Ok(epoch)
}

fn parse_effect(effect: &serde_json::Value, what: &str) -> Result<u64, ParseError> {
    let effect = object(effect, what)?;
    exact_fields(effect, EFFECT_FIELDS, what)?;
    let kind = string_field(effect, "kind", what)?;
    if !EFFECT_KINDS.contains(&kind) {
        return Err(parse_err(format!("unknown effect kind {kind}")));
    }
    nonempty_field(effect, "site", what)?;
    let attempted = bool_field(effect, "attempted", what)?;
    let path = null_or_string(effect, "path", what)?;
    let result = string_field(effect, "result", what)?;
    if !EFFECT_RESULTS.contains(&result) {
        return Err(parse_err(format!("unknown effect result {result}")));
    }
    if attempted && result == "succeeded" {
        return Err(parse_err("an attempted effect cannot report succeeded"));
    }
    if (kind == DESCRIPTOR_EFFECT) != path.is_none() {
        return Err(parse_err(
            "a descriptor effect carries no path and every other effect carries one",
        ));
    }
    epoch_field(effect, what)
}

/// A fixture diff effect is an observed net change. Its writer result was not
/// observed, so its result is always `unknown` and its epoch is zero.
fn parse_diff_effect(effect: &serde_json::Value) -> Result<(), ParseError> {
    let effect = object(effect, "diff effect")?;
    exact_fields(effect, EFFECT_FIELDS, "diff effect")?;
    let kind = string_field(effect, "kind", "diff effect")?;
    if !matches!(kind, "create" | "save" | "unlink") {
        return Err(parse_err(format!("unknown diff effect kind {kind}")));
    }
    if string_field(effect, "site", "diff effect")? != "fixture-diff" {
        return Err(parse_err("diff effect site is not fixture-diff"));
    }
    if bool_field(effect, "attempted", "diff effect")? {
        return Err(parse_err("diff effect cannot be attempted"));
    }
    string_field(effect, "path", "diff effect")?;
    if u64_field(effect, "epoch", "diff effect")? != 0 {
        return Err(parse_err("diff effect epoch is not zero"));
    }
    if string_field(effect, "result", "diff effect")? != "unknown" {
        return Err(parse_err("diff effect result is not unknown"));
    }
    Ok(())
}

fn parse_fixture_entry(entry: &serde_json::Value) -> Result<(), ParseError> {
    let entry = object(entry, "fixture entry")?;
    exact_fields(entry, FIXTURE_ENTRY_FIELDS, "fixture entry")?;
    let rel = nonempty_field(entry, "rel", "fixture entry")?;
    let relative = std::path::Path::new(rel);
    if !relative
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        return Err(parse_err("fixture entry rel is not a strict relative path"));
    }
    let kind = string_field(entry, "kind", "fixture entry")?;
    if !FIXTURE_ENTRY_KINDS.contains(&kind) {
        return Err(parse_err(format!("unknown fixture entry kind {kind}")));
    }
    u64_field(entry, "size", "fixture entry")?;
    u64_field(entry, "device", "fixture entry")?;
    u64_field(entry, "inode", "fixture entry")?;
    let parent_null = field(entry, "parent_device", "fixture entry")?.is_null();
    if parent_null != field(entry, "parent_inode", "fixture entry")?.is_null() {
        return Err(parse_err(
            "fixture entry parent device and inode must be both present or both null",
        ));
    }
    null_or_u64(entry, "parent_device", "fixture entry")?;
    null_or_u64(entry, "parent_inode", "fixture entry")?;
    null_or_u32(entry, "mode", "fixture entry")?;
    if field(entry, "bytes_sha256", "fixture entry")?.is_null() {
        return Ok(());
    }
    if kind != "file" {
        return Err(parse_err(
            "only a file fixture entry carries a bytes digest",
        ));
    }
    hex64_field(entry, "bytes_sha256", "fixture entry")
}

fn parse_fixture_capture(value: &serde_json::Value) -> Result<(), ParseError> {
    if value.is_null() {
        return Ok(());
    }
    let capture = object(value, "fixture_capture")?;
    exact_fields(capture, FIXTURE_FIELDS, "fixture_capture")?;
    string_field(capture, "root", "fixture_capture")?;
    bool_field(capture, "truncated", "fixture_capture")?;
    for key in ["pre", "post"] {
        let entries = array_field(capture, key, "fixture_capture")?;
        if entries.len() > MAX_FIXTURE_ENTRIES {
            return Err(parse_err(format!(
                "fixture_capture {key} exceeds the 4096-entry cap"
            )));
        }
        for entry in entries {
            parse_fixture_entry(entry)?;
        }
    }
    for effect in array_field(capture, "diff_effects", "fixture_capture")? {
        parse_diff_effect(effect)?;
    }
    Ok(())
}

fn array_field<'a>(
    object: &'a Object,
    key: &str,
    what: &str,
) -> Result<&'a Vec<serde_json::Value>, ParseError> {
    field(object, key, what)?
        .as_array()
        .ok_or_else(|| parse_err(format!("{what} {key} is not an array")))
}

fn parse_holder(holder: &serde_json::Value) -> Result<u64, ParseError> {
    let holder = object(holder, "recorded holder")?;
    exact_fields(holder, HOLDER_FIELDS, "recorded holder")?;
    nonempty_field(holder, "site", "recorded holder")?;
    u64_field(holder, "record_id", "recorded holder")?;
    null_or_u32(holder, "pid", "recorded holder")?;
    if field(holder, "pid", "recorded holder")?.is_null() {
        return Err(parse_err("recorded holder pid is null"));
    }
    u64_field(holder, "recorded_start_ms", "recorded holder")?;
    null_or_u64(holder, "kernel_start", "recorded holder")?;
    null_or_u32(holder, "uid", "recorded holder")?;
    epoch_field(holder, "recorded holder")
}

fn parse_clock(clock: &serde_json::Value) -> Result<u64, ParseError> {
    let clock = object(clock, "clock")?;
    exact_fields(clock, CLOCK_FIELDS, "clock")?;
    nonempty_field(clock, "kind", "clock")?;
    null_or_u64(clock, "value", "clock")?;
    epoch_field(clock, "clock")
}

fn parse_classification(row: &serde_json::Value) -> Result<u64, ParseError> {
    let row = object(row, "classification")?;
    exact_fields(row, CLASSIFY_FIELDS, "classification")?;
    nonempty_field(row, "kind", "classification")?;
    string_field(row, "identity", "classification")?;
    string_field(row, "value", "classification")?;
    occurrence_field(row, "classification")?;
    epoch_field(row, "classification")
}

/// Record each state-row epoch once. Gaps are allowed (a row that was counted
/// out still consumed its epoch); reuse is not.
fn claim_epochs(
    seen: &mut std::collections::BTreeSet<u64>,
    epochs: &[u64],
) -> Result<(), ParseError> {
    for epoch in epochs {
        if !seen.insert(*epoch) {
            return Err(parse_err(format!("epoch {epoch} is reused")));
        }
    }
    Ok(())
}

fn bounded_rows<'a>(
    map: &'a Object,
    key: &str,
    cap: usize,
) -> Result<&'a Vec<serde_json::Value>, ParseError> {
    let rows = array_field(map, key, "transcript")?;
    if rows.len() > cap {
        return Err(parse_err(format!("{key} exceeds its {cap}-row cap")));
    }
    Ok(rows)
}

/// Strict transcript parse. Success is structure only.
pub(crate) fn parse_transcript(bytes: &[u8]) -> Result<serde_json::Value, ParseError> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(parse_err("transcript exceeds the 8 MiB document cap"));
    }
    let doc: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| parse_err(error.to_string()))?;
    let map = object(&doc, "transcript")?;
    transcript_schema(map)?;
    exact_fields(map, TRANSCRIPT_FIELDS, "transcript")?;
    nonempty_field(map, "engine", "transcript")?;
    nonempty_field(map, "case", "transcript")?;
    let manifest_sha256 = field(map, "manifest_sha256", "transcript")?;
    if !manifest_sha256.is_null() {
        hex64_field(map, "manifest_sha256", "transcript")?;
    }
    string_field(map, "entrypoint", "transcript")?;
    null_or_u64(map, "window_monotonic_start_ns", "transcript")?;
    null_or_u64(map, "window_monotonic_end_ns", "transcript")?;
    u64_field(map, "window_elapsed_ns", "transcript")?;
    parse_engine_outcome(field(map, "outcome", "transcript")?)?;
    let truncation = object(field(map, "truncation", "transcript")?, "truncation")?;
    exact_fields(truncation, TRUNCATION_FIELDS, "truncation")?;
    for flag in TRUNCATION_FIELDS {
        bool_field(truncation, flag, "truncation")?;
    }
    let mut epochs = std::collections::BTreeSet::new();
    for row in bounded_rows(map, "effects", MAX_READ_ROWS)? {
        let epoch = parse_effect(row, "effect")?;
        claim_epochs(&mut epochs, &[epoch])?;
    }
    for row in bounded_rows(map, "reads", MAX_READ_ROWS)? {
        let epoch = parse_read_row(row)?;
        claim_epochs(&mut epochs, &[epoch])?;
    }
    for row in bounded_rows(map, "clocks", MAX_READ_ROWS)? {
        let epoch = parse_clock(row)?;
        claim_epochs(&mut epochs, &[epoch])?;
    }
    for row in bounded_rows(map, "classifications", MAX_READ_ROWS)? {
        let epoch = parse_classification(row)?;
        claim_epochs(&mut epochs, &[epoch])?;
    }
    for row in bounded_rows(map, "recorded_holders", MAX_PROCESS_ROWS)? {
        let epoch = parse_holder(row)?;
        claim_epochs(&mut epochs, &[epoch])?;
    }
    parse_fixture_capture(field(map, "fixture_capture", "transcript")?)?;
    let notes = array_field(map, "notes", "transcript")?;
    if notes.len() > MAX_READ_ROWS || notes.iter().any(|note| !note.is_string()) {
        return Err(parse_err("notes is not a bounded array of strings"));
    }
    Ok(doc)
}

/// The engine outcome: a completed engine result object, or a refusal whose
/// source is always the engine. An apparatus refusal is never a transcript.
fn parse_engine_outcome(value: &serde_json::Value) -> Result<(), ParseError> {
    let outcome = object(value, "outcome")?;
    match string_field(outcome, "state", "outcome")? {
        "completed" => {
            exact_fields(outcome, &["state", "result"], "outcome")?;
            object(field(outcome, "result", "outcome")?, "outcome result")?;
        }
        "refused" => {
            exact_fields(
                outcome,
                &["state", "reason", "exit_code", "source"],
                "outcome",
            )?;
            string_field(outcome, "reason", "outcome")?;
            if u64_field(outcome, "exit_code", "outcome")? > u64::from(u8::MAX) {
                return Err(parse_err("outcome exit_code is not a u8"));
            }
            if string_field(outcome, "source", "outcome")? != "engine" {
                return Err(parse_err(
                    "a refused outcome must name the engine as source",
                ));
            }
        }
        other => return Err(parse_err(format!("unknown outcome state {other}"))),
    }
    Ok(())
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

const MANIFEST_REQUIRED: &[&str] = &[
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
];

const SOURCE_FIELDS: &[&str] = &[
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
    "oxide_tree",
];

const OBLIGATION_FIELDS: &[&str] = &["engine", "site", "identity", "parent_required"];
const MANIFEST_EFFECT_FIELDS: &[&str] = &["kind", "site", "path"];
const MANIFEST_FIXTURE_FIELDS: &[&str] = &["root", "device", "inode", "mode", "entries", "roots"];

/// Strict manifest parse. `parent_required` is mandatory on every obligation.
pub(crate) fn parse_manifest(bytes: &[u8]) -> Result<serde_json::Value, ParseError> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(parse_err("manifest exceeds the 8 MiB document cap"));
    }
    let doc: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| parse_err(error.to_string()))?;
    let map = object(&doc, "manifest")?;
    refuse_historical(string_field(map, "schema", "manifest")?)?;
    if string_field(map, "schema", "manifest")? != MANIFEST_SCHEMA {
        return Err(parse_err("unknown manifest schema"));
    }
    for key in map.keys() {
        if !MANIFEST_FIELDS.contains(&key.as_str()) {
            return Err(parse_err(format!("unknown manifest field {key}")));
        }
    }
    for key in MANIFEST_REQUIRED {
        field(map, key, "manifest")?;
    }
    nonempty_field(map, "case", "manifest")?;
    let window = object(field(map, "window", "manifest")?, "window")?;
    exact_fields(
        window,
        &["identity", "start_monotonic_ns", "end_monotonic_ns"],
        "window",
    )?;
    nonempty_field(window, "identity", "window")?;
    let start = u64_field(window, "start_monotonic_ns", "window")?;
    let end = u64_field(window, "end_monotonic_ns", "window")?;
    if start > end {
        return Err(parse_err("window starts after it ends"));
    }
    let source = object(field(map, "source", "manifest")?, "source")?;
    exact_fields(source, SOURCE_FIELDS, "source")?;
    for key in SOURCE_FIELDS {
        if key.ends_with("_files") {
            object(field(source, key, "source")?, key)?;
        } else if key.ends_with("_sha256") {
            hex64_field(source, key, "source")?;
        } else if *key == "barge_pin" || *key == "oxide_tree" {
            let text = string_field(source, key, "source")?;
            if !is_lower_hex(text, 40) {
                return Err(parse_err(format!("source {key} is not a 40-hex identity")));
            }
        } else {
            string_field(source, key, "source")?;
        }
    }
    let fixture = object(field(map, "fixture", "manifest")?, "fixture")?;
    exact_fields(fixture, MANIFEST_FIXTURE_FIELDS, "fixture")?;
    string_field(fixture, "root", "fixture")?;
    u64_field(fixture, "device", "fixture")?;
    u64_field(fixture, "inode", "fixture")?;
    null_or_u32(fixture, "mode", "fixture")?;
    if array_field(fixture, "entries", "fixture")?.len() > MAX_FIXTURE_ENTRIES {
        return Err(parse_err("fixture entries exceed the 4096-entry cap"));
    }
    array_field(fixture, "roots", "fixture")?;
    let dynamic = object(field(map, "dynamic", "manifest")?, "dynamic")?;
    exact_fields(
        dynamic,
        &["volumes", "writers", "external_allowances"],
        "dynamic",
    )?;
    for key in ["volumes", "writers", "external_allowances"] {
        array_field(dynamic, key, "dynamic")?;
    }
    let expected = object(field(map, "expected", "manifest")?, "expected")?;
    exact_fields(expected, &["classification", "roots"], "expected")?;
    string_field(expected, "classification", "expected")?;
    array_field(expected, "roots", "expected")?;
    object(field(map, "barge", "manifest")?, "barge")?;
    for obligation in array_field(map, "mandatory_observations", "manifest")? {
        let obligation = object(obligation, "obligation")?;
        exact_fields(obligation, OBLIGATION_FIELDS, "obligation")?;
        let engine = string_field(obligation, "engine", "obligation")?;
        if !matches!(engine, "oxide" | "barge") {
            return Err(parse_err(format!("unknown obligation engine {engine}")));
        }
        nonempty_field(obligation, "site", "obligation")?;
        bool_field(obligation, "parent_required", "obligation")?;
        let identity = object(
            field(obligation, "identity", "obligation")?,
            "obligation identity",
        )?;
        if identity.is_empty() {
            return Err(parse_err("obligation identity is empty"));
        }
        type_check_obligation_identity(identity)?;
    }
    for effect in array_field(map, "expected_effects", "manifest")? {
        let effect = object(effect, "expected effect")?;
        exact_fields(effect, MANIFEST_EFFECT_FIELDS, "expected effect")?;
        let kind = string_field(effect, "kind", "expected effect")?;
        if !EFFECT_KINDS.contains(&kind) || kind == DESCRIPTOR_EFFECT {
            return Err(parse_err(format!("unknown effect kind {kind}")));
        }
        nonempty_field(effect, "site", "expected effect")?;
        nonempty_field(effect, "path", "expected effect")?;
    }
    Ok(doc)
}

/// Obligation identities are partial, so any listed key may be absent. A
/// `device` is a string only for a volume obligation (`volume_uuid` present).
fn type_check_obligation_identity(identity: &Object) -> Result<(), ParseError> {
    exact_or_known(identity, OBLIGATION_IDENTITY_FIELDS, "obligation identity")?;
    let volume_shape = identity
        .get("volume_uuid")
        .is_some_and(serde_json::Value::is_string);
    for (key, value) in identity {
        match key.as_str() {
            "path" | "component" | "volume_uuid" | "mount" | "volume_device"
            | "window_identity" => {
                if !value.is_string() {
                    return Err(parse_err(format!("obligation {key} is not a string")));
                }
            }
            "device" if volume_shape => {
                if !value.is_string() {
                    return Err(parse_err("volume obligation device is not a string"));
                }
            }
            "parent_source" | "own_source" => {
                Provenance::parse(Some(value)).map_err(parse_err)?;
            }
            "fd" => {
                if !value
                    .as_i64()
                    .is_some_and(|n| (0..=i64::from(i32::MAX)).contains(&n))
                {
                    return Err(parse_err("obligation fd is not a non-negative i32"));
                }
            }
            "pid" | "uid" => {
                if !value.as_u64().is_some_and(|n| n <= u64::from(u32::MAX)) {
                    return Err(parse_err(format!("obligation {key} is not a u32")));
                }
            }
            _ => {
                if !value.is_u64() {
                    return Err(parse_err(format!("obligation {key} is not a u64")));
                }
            }
        }
    }
    Ok(())
}

fn exact_or_known(object: &Object, allowed: &[&str], what: &str) -> Result<(), ParseError> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(parse_err(format!("unknown {what} field {key}")));
        }
    }
    Ok(())
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

/// The one identity rule. Every key the obligation names must equal the row's
/// value; a `null` row value never equals a named key. A volume obligation's
/// `device` maps to the row's `volume_device`, and `window_identity` is the
/// manifest window, not a row field.
pub(crate) fn identity_fields_match(
    obligation_identity: &serde_json::Value,
    row_identity: &serde_json::Value,
    window: Option<&str>,
) -> bool {
    let Some(want) = obligation_identity.as_object() else {
        return false;
    };
    let Some(got) = row_identity.as_object() else {
        return false;
    };
    let volume = want
        .get("volume_uuid")
        .is_some_and(serde_json::Value::is_string);
    for (key, expected) in want {
        if key == "parent_source" || key == "own_source" {
            continue;
        }
        if key == "window_identity" {
            match expected.as_str() {
                Some(want_window) if Some(want_window) == window => continue,
                _ => return false,
            }
        }
        let row_key = if volume && key == "device" {
            "volume_device"
        } else {
            key.as_str()
        };
        match got.get(row_key) {
            Some(actual) if !actual.is_null() && actual == expected => {}
            _ => return false,
        }
    }
    true
}

/// The one effect-parity rule. Only a completed, identified effect (not
/// attempted, `succeeded`, with a path) can match a declared effect.
/// Attempted, failed, partial or unknown rows are named as not completed and
/// never counted as parity.
pub(crate) fn effect_parity_issues(
    declared: &[serde_json::Value],
    observed: &[serde_json::Value],
) -> Vec<String> {
    let mut issues = Vec::new();
    let mut completed = Vec::new();
    for effect in observed {
        let site = effect.get("site").and_then(|v| v.as_str()).unwrap_or("");
        let result = effect.get("result").and_then(|v| v.as_str()).unwrap_or("");
        let attempted = effect
            .get("attempted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        if attempted || result != "succeeded" {
            issues.push(format!("effect not completed at {site} ({result})"));
            continue;
        }
        let Some(path) = effect.get("path").and_then(|v| v.as_str()) else {
            issues.push(format!("effect identity unproven at {site}"));
            continue;
        };
        completed.push((
            effect
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned(),
            site.to_owned(),
            path.to_owned(),
        ));
    }
    let want = effect_bag(declared);
    completed.sort();
    if want.len() != completed.len() {
        issues.push("omitted or extra effects".to_owned());
    } else if want != completed {
        issues.push("substituted effects".to_owned());
    }
    issues
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
    if transcript
        .pointer("/fixture_capture/truncated")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        issues.push("fixture capture truncated".to_owned());
    }
    if transcript
        .get("window_monotonic_start_ns")
        .is_some_and(serde_json::Value::is_null)
        || transcript
            .get("window_monotonic_end_ns")
            .is_some_and(serde_json::Value::is_null)
    {
        issues.push("window origin and end unproven".to_owned());
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
    let observed = transcript
        .get("effects")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
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
        if !is_lower_hex(digest, 64) {
            issues.push("transcript manifest digest is not a SHA-256".to_owned());
        }
        if let Some(obligations) = manifest
            .get("mandatory_observations")
            .and_then(|v| v.as_array())
        {
            let window = manifest
                .pointer("/window/identity")
                .and_then(serde_json::Value::as_str);
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
                let identity_matched = at_site.iter().any(|row| {
                    identity_fields_match(&obligation["identity"], &row["identity"], window)
                });
                if !identity_matched {
                    issues.push(format!("wrong identity at {site}"));
                    continue;
                }
                if parent_required {
                    let discharged = at_site.iter().any(|row| {
                        identity_fields_match(&obligation["identity"], &row["identity"], window)
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
            issues.extend(effect_parity_issues(declared, &observed));
        }
    } else {
        issues.push("no manifest".to_owned());
    }
    for holder in transcript
        .get("recorded_holders")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        if holder
            .get("kernel_start")
            .is_some_and(serde_json::Value::is_null)
        {
            let site = holder.get("site").and_then(|v| v.as_str()).unwrap_or("");
            issues.push(format!("process lifetime unproven at {site}"));
        }
    }
    for clock in transcript
        .get("clocks")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        if clock.get("value").is_some_and(serde_json::Value::is_null) {
            let kind = clock.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            issues.push(format!("clock unproven at {kind}"));
        }
    }
    Completeness { issues }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VectorKind {
    /// Parses, and completeness matches the expected issue list.
    ValidTranscript,
    /// Parser-positive, with the completeness issues it must produce.
    CompletenessInvalid,
    /// Refused by the parser (structure, type, cap, version or identity).
    InvalidTranscript,
    ValidManifest,
    InvalidManifest,
}

struct Vector {
    name: &'static str,
    bytes: &'static [u8],
    digest: &'static str,
    kind: VectorKind,
}

macro_rules! vector {
    ($name:literal, $kind:ident, $digest:literal) => {
        Vector {
            name: $name,
            bytes: include_bytes!(concat!(
                "../../../docs/knowledge/contracts/capture-vectors/v2/",
                $name
            )),
            digest: $digest,
            kind: VectorKind::$kind,
        }
    };
}

const VECTORS: &[Vector] = &[
    vector!(
        "invalid/apparatus-diagnostic.json",
        InvalidTranscript,
        "185bcaec007b9f6767845285b0104fcf88a392edb22e7e4fb4b672f4f1abd152"
    ),
    vector!(
        "invalid/attempt-succeeded.json",
        InvalidTranscript,
        "8f27e654a136e86c7080995ac4af1d20f55cbdf918322beca19b23bee5e39183"
    ),
    vector!(
        "invalid/attempted-as-completed.json",
        CompletenessInvalid,
        "0c842e4d78610b80f11633b8ec9bf07d4b59117c71170023f9dcd0e0b1b8234f"
    ),
    vector!(
        "invalid/cap-absent.json",
        InvalidTranscript,
        "14019c90f1fbc93866a463a5613a04edb84dbbdd8ae7d310e3ccc7ea378a62ea"
    ),
    vector!(
        "invalid/cap-string.json",
        InvalidTranscript,
        "3dd09217d9ce0364dec0f9ea4d0ad3c364e9a4c224ddd20e3961b183f4c4a614"
    ),
    vector!(
        "invalid/clock-string.json",
        InvalidTranscript,
        "a8c1b70c47c695889329c3088959493414b0a2dbe8af7d2137d09defd265e3f2"
    ),
    vector!(
        "invalid/collision.json",
        CompletenessInvalid,
        "72eb3a69cd1209d4782453c86fb3345d0eea4a99565ef410d8139ab2c9a637a3"
    ),
    vector!(
        "invalid/derived-provenance.json",
        InvalidTranscript,
        "78cc67035fb02a8e21892c3cdbd36e45c4f525b63e5866fc7981a896a0810fed"
    ),
    vector!(
        "invalid/duplicate-epoch.json",
        InvalidTranscript,
        "090fd04e378f5156d35b47edb1a9f7fcd3835f1c24a7bf0d9c624b78ecccf048"
    ),
    vector!(
        "invalid/effect-result-maybe.json",
        InvalidTranscript,
        "9cb1f8c68caa4048c8d00bba7931b3e98bf6a1bfa4fe9582c1dd8c16359c5c26"
    ),
    vector!(
        "invalid/extra-effect.json",
        CompletenessInvalid,
        "52281a2ef287976130a52b4d2724c9deaac147511a4b5db099247b9e0e381b3c"
    ),
    vector!(
        "invalid/fixture-device-string.json",
        InvalidTranscript,
        "081d3fa07487046556f7bc43558af34d0ff0ac8fb0cbeaf6dbaf704bf58dd59a"
    ),
    vector!(
        "invalid/fixture-diff-epoch.json",
        InvalidTranscript,
        "e6cea045808a0f3fb9b4a80e4e8666a8af676634941b10aef084ae2992cff849"
    ),
    vector!(
        "invalid/historical-v1.json",
        InvalidTranscript,
        "14f1c5cf9a81660b749c59f7d2e2b0ac99ccffed9dc57d6daf5f41b3fc8f9307"
    ),
    vector!(
        "invalid/holder-kernel-missing.json",
        InvalidTranscript,
        "577dd0803feb5097748b2996231dd46672af7558f758aec7b2dedfdba6375520"
    ),
    vector!(
        "invalid/occurrence-zero.json",
        InvalidTranscript,
        "89d06ccade8711e0c62f48a5e2d5ccf0376a66b115df00498370cbfc541e9821"
    ),
    vector!(
        "invalid/omitted-effect.json",
        CompletenessInvalid,
        "206611cc45b792c4ffdea3eb8f162b8b67d77412294e2636ab9445be59fd20bb"
    ),
    vector!(
        "invalid/omitted-obligation.json",
        CompletenessInvalid,
        "5f9c2e7384cee18d126f316f9a7e98540e4cfb784b2d3cf5e51baee1940f2171"
    ),
    vector!(
        "invalid/outcome-state-unknown.json",
        InvalidTranscript,
        "d9bd03139f514c8f20da7731c8e49c3a4d29ab0de6dcc861439caf9f28529661"
    ),
    vector!(
        "invalid/parent-pair-mismatch.json",
        InvalidTranscript,
        "8a576589088e43511987f5224a7e54c8aafabe3e4fa5930cc8f5c185ed89b963"
    ),
    vector!(
        "invalid/source-mismatch.json",
        CompletenessInvalid,
        "142b909a1747ee531a9095addce1a4587ebc8070724ce00a0be9414d9b15b5f6"
    ),
    vector!(
        "invalid/substituted-effect.json",
        CompletenessInvalid,
        "4d8cf98a59af3194942f65d18adf5f9be073d706271fe135a9c2d7d16c7109aa"
    ),
    vector!(
        "invalid/truncation-missing-flag.json",
        InvalidTranscript,
        "db7b3b323b0c50224a1fb19ba590e3eb534f159940c47a2478b203e3a1ca9ee4"
    ),
    vector!(
        "invalid/unknown-field-nested.json",
        InvalidTranscript,
        "4fef961450de6bf388f1fca40219a81afb12fef557f45fa5a81db5f040eb14e9"
    ),
    vector!(
        "invalid/wrong-inode.json",
        CompletenessInvalid,
        "06d9c40ef3336e8965b6f3319adbc9528de34e435cdd4e3d33ce3c80a88740ba"
    ),
    vector!(
        "manifest/invalid-extra-field.json",
        InvalidManifest,
        "134be79f3550f4b3b20fa9224c60d7fb51f2c068f2605cac91a7785bf1ab4449"
    ),
    vector!(
        "manifest/invalid-missing-parent-required.json",
        InvalidManifest,
        "624daf5575b513de5ff52adbd8737c874dbcbf9be54ea1ffa050923753cdaf45"
    ),
    vector!(
        "manifest/invalid-source-digest.json",
        InvalidManifest,
        "4ff4d08f8122c43dad96f27e001ab100d66a73f207964ddb76d42676a29ecf0d"
    ),
    vector!(
        "manifest/invalid-window-order.json",
        InvalidManifest,
        "73dfa2166407416ec76b3660456e8f46a42d6e7d51880619099a9af5d107d110"
    ),
    vector!(
        "manifest/valid-parent-required.json",
        ValidManifest,
        "c126ef82324ede68dadaa4ed7a6a53314a9c5126bbee2dac889301214024d1f6"
    ),
    vector!(
        "manifest/valid-volume-identity.json",
        ValidManifest,
        "7e04e0604bcc66bfd7b8101c8dbda461b0c6b45173f0fcdb18f4db406ce437f0"
    ),
    vector!(
        "valid/absent.json",
        ValidTranscript,
        "0b66bc5eb8f763fb987da0f99de2ba8aacfc670ce1725cf5351b60878770ac1b"
    ),
    vector!(
        "valid/clocks-holders.json",
        ValidTranscript,
        "e4942d791bda245d4dceb2597fa9946d2a877bff33a176cb3feb55e1b5f3ca26"
    ),
    vector!(
        "valid/effects.json",
        ValidTranscript,
        "73eb71443fd73cd0d69adf43e94f53888bf915a851783f3b128500ef78b3d8f7"
    ),
    vector!(
        "valid/fixture-capture.json",
        ValidTranscript,
        "adb69ad100cedf65fb148a29ea67ae60852d518508c4716c6a0e5c2d63bf3202"
    ),
    vector!(
        "valid/handle-parent.json",
        ValidTranscript,
        "c9323c97126a2a1e35630806613dd81ee4d71da8de956a12310d2ec8a522346a"
    ),
    vector!(
        "valid/refused.json",
        ValidTranscript,
        "b4b2426f3ab2151fd9bf5ef6d0447a65054d0ed46263d8c2eb85646740d7f47a"
    ),
    vector!(
        "valid/truncated.json",
        ValidTranscript,
        "0fc5925ac91ba7d480b126b4d1a2f682271250a09171b1a5630257888ae56e51"
    ),
    vector!(
        "valid/volume.json",
        ValidTranscript,
        "d095f465134a1dc4bf8cf72d0a80c33928c5d6b91b8a6622da2df8fc631c2f89"
    ),
];

// ---- self-check helpers -------------------------------------------------

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

/// The manifest each completeness vector is judged against. Declared effects
/// are completed parity only.
fn completeness_fixture(name: &str, transcript_case: &str) -> serde_json::Value {
    let save_ledger =
        serde_json::json!([{"kind": "save", "site": "ledger", "path": "/fixture/ledger"}]);
    let registry = serde_json::json!({"path": "/fixture/registry"});
    match name {
        "valid/absent.json" | "valid/truncated.json" => vector_manifest(
            transcript_case,
            "config:inspect:initial",
            false,
            serde_json::json!({"path": "/fixture/missing"}),
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
        "valid/effects.json" => vector_manifest(
            transcript_case,
            "prune::read_registry",
            false,
            registry,
            true,
            save_ledger,
        ),
        "valid/volume.json" => vector_manifest(
            transcript_case,
            "volume-site",
            false,
            serde_json::json!({
                "volume_uuid": "volume",
                "mount": "/mount",
                "device": "/dev/example",
                "window_identity": "vector"
            }),
            true,
            serde_json::json!([]),
        ),
        "invalid/source-mismatch.json" => vector_manifest(
            "OTHER-CASE",
            "prune::read_registry",
            false,
            registry,
            false,
            serde_json::json!([]),
        ),
        "invalid/wrong-inode.json" => vector_manifest(
            transcript_case,
            "prune::read_registry",
            false,
            serde_json::json!({"path": "/fixture/registry", "device": 3, "inode": 4}),
            true,
            serde_json::json!([]),
        ),
        "invalid/omitted-effect.json"
        | "invalid/extra-effect.json"
        | "invalid/attempted-as-completed.json" => vector_manifest(
            transcript_case,
            "prune::read_registry",
            false,
            registry,
            false,
            save_ledger,
        ),
        "invalid/substituted-effect.json" => vector_manifest(
            transcript_case,
            "prune::read_registry",
            false,
            registry,
            false,
            save_ledger,
        ),
        "invalid/collision.json" => vector_manifest(
            transcript_case,
            "prune::read_registry",
            false,
            registry,
            true,
            serde_json::json!([]),
        ),
        "valid/clocks-holders.json"
        | "valid/refused.json"
        | "valid/fixture-capture.json"
        | "invalid/omitted-obligation.json" => vector_manifest(
            transcript_case,
            "prune::read_registry",
            false,
            registry,
            true,
            serde_json::json!([]),
        ),
        _ => vector_manifest(
            transcript_case,
            "prune::read_registry",
            false,
            registry,
            true,
            serde_json::json!([]),
        ),
    }
}

/// The exact completeness issues each completeness vector must produce, in
/// the order the completeness judgement emits them.
fn expected_issues(name: &str) -> &'static [&'static str] {
    match name {
        "valid/absent.json" => &["window origin and end unproven"],
        "valid/handle-parent.json" => &[
            "window origin and end unproven",
            "parent_required unproven at root-observe:entry: handle parent still needs qualified K1",
        ],
        "valid/effects.json" => &[
            "window origin and end unproven",
            "effect not completed at control-state (unknown)",
            "effect identity unproven at open-verified-step:fchmod",
        ],
        "valid/refused.json" | "valid/fixture-capture.json" | "valid/volume.json" => {
            &["window origin and end unproven"]
        }
        "valid/truncated.json" => &["truncated: read_rows_truncated", "window origin and end unproven"],
        "valid/clocks-holders.json" => &[
            "window origin and end unproven",
            "process lifetime unproven at lock-holder",
            "clock unproven at boot-session-ms",
        ],
        "invalid/source-mismatch.json" => &["window origin and end unproven", "source/run case mismatch"],
        "invalid/wrong-inode.json" => &["window origin and end unproven", "wrong identity at prune::read_registry"],
        "invalid/omitted-obligation.json" => &[
            "window origin and end unproven",
            "missing obligation at prune::read_registry",
        ],
        "invalid/omitted-effect.json" | "invalid/extra-effect.json" => {
            &["window origin and end unproven", "omitted or extra effects"]
        }
        "invalid/substituted-effect.json" => &["window origin and end unproven", "substituted effects"],
        "invalid/attempted-as-completed.json" => &[
            "window origin and end unproven",
            "effect not completed at ledger (unknown)",
            "omitted or extra effects",
        ],
        "invalid/collision.json" => &[
            "window origin and end unproven",
            "collision at prune::read_registry|/fixture/registry|none|none|none occurrence 1",
        ],
        _ => &[],
    }
}

fn base_bytes() -> &'static [u8] {
    VECTORS
        .iter()
        .find(|vector| vector.name == "valid/absent.json")
        .expect("base vector")
        .bytes
}

/// Caps are constructed, not stored: each oversize document is built from the
/// base vector and must fail parse naming its own limit.
fn cap_checks() -> Result<usize, String> {
    let mut checked = 0usize;
    let mut oversize = serde_json::from_slice::<serde_json::Value>(base_bytes())
        .map_err(|error| error.to_string())?;
    oversize["notes"] = serde_json::json!(["x".repeat(MAX_DOCUMENT_BYTES)]);
    let bytes = serde_json::to_vec(&oversize).map_err(|error| error.to_string())?;
    match parse_transcript(&bytes) {
        Err(error) if error.message.contains("8 MiB") => {}
        other => {
            return Err(format!(
                "8 MiB JSON document must fail naming the cap, got {other:?}"
            ));
        }
    }
    checked += 1;

    let mut occurrence = serde_json::from_slice::<serde_json::Value>(base_bytes())
        .map_err(|error| error.to_string())?;
    occurrence["reads"][0]["occurrence"] = serde_json::json!(65);
    let bytes = serde_json::to_vec(&occurrence).map_err(|error| error.to_string())?;
    if parse_transcript(&bytes).is_ok() {
        return Err("occurrence 65 must fail parse".to_owned());
    }
    checked += 1;

    let mut reads = serde_json::from_slice::<serde_json::Value>(base_bytes())
        .map_err(|error| error.to_string())?;
    let row = reads["reads"][0].clone();
    reads["reads"] = serde_json::Value::Array(vec![row; MAX_READ_ROWS + 1]);
    let bytes = serde_json::to_vec(&reads).map_err(|error| error.to_string())?;
    if parse_transcript(&bytes).is_ok() {
        return Err("8193 reads must fail parse".to_owned());
    }
    checked += 1;

    let mut holders = serde_json::from_slice::<serde_json::Value>(base_bytes())
        .map_err(|error| error.to_string())?;
    holders["recorded_holders"] = serde_json::Value::Array(vec![
        serde_json::json!({
            "site": "pure-holder",
            "record_id": 1,
            "pid": 1,
            "recorded_start_ms": 1,
            "kernel_start": null,
            "uid": null,
            "epoch": 1
        });
        MAX_PROCESS_ROWS + 1
    ]);
    let bytes = serde_json::to_vec(&holders).map_err(|error| error.to_string())?;
    if parse_transcript(&bytes).is_ok() {
        return Err("513 process rows must fail parse".to_owned());
    }
    checked += 1;

    let parsed = parse_transcript(base_bytes()).map_err(|error| error.message)?;
    let without = assess_completeness(&parsed, None);
    if !without.issues.iter().any(|issue| issue == "no manifest") || without.ok() {
        return Err("assess_completeness without a manifest must issue no manifest".to_owned());
    }
    checked += 1;

    let mut missing = completeness_fixture("valid/absent.json", "VECTOR-ABSENT");
    missing["mandatory_observations"][0]
        .as_object_mut()
        .ok_or("base fixture obligation is not an object")?
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

/// Pure parser, completeness, emitted-shape and hashed-vector checks. Invoked
/// from the harness `--self-check` so the `harness = false` target runs them.
pub(crate) fn run_self_checks() -> Result<(), String> {
    let mut failed = Vec::new();
    let mut checked = 0usize;
    let catalog = include_str!("../../../docs/knowledge/contracts/capture-vectors/v2/SHA256SUMS");
    for vector in VECTORS {
        let digest = sha256_hex(vector.bytes);
        if digest != vector.digest {
            failed.push(format!(
                "{} digest {} != catalog {}",
                vector.name, digest, vector.digest
            ));
            continue;
        }
        if !catalog.contains(vector.digest) {
            failed.push(format!("{} missing from v2 SHA256SUMS", vector.name));
            continue;
        }
        match vector.kind {
            VectorKind::ValidTranscript | VectorKind::CompletenessInvalid => {
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
                checked += 1;
            }
            VectorKind::InvalidTranscript => {
                if parse_transcript(vector.bytes).is_ok() {
                    failed.push(format!("{} should fail parse", vector.name));
                } else {
                    checked += 1;
                }
            }
            VectorKind::ValidManifest => match parse_manifest(vector.bytes) {
                Ok(_) => checked += 1,
                Err(error) => failed.push(format!("{} parse: {}", vector.name, error.message)),
            },
            VectorKind::InvalidManifest => {
                if parse_manifest(vector.bytes).is_ok() {
                    failed.push(format!("{} should fail parse", vector.name));
                } else {
                    checked += 1;
                }
            }
        }
    }
    if K1_PREMISE_QUALIFIED {
        failed
            .push("K1 is not qualified; completeness must not succeed on handle parent".to_owned());
    }
    match cap_checks() {
        Ok(extra) => checked += extra,
        Err(error) => failed.push(error),
    }
    if failed.is_empty() {
        println!("oxide: capture-wire: {checked} vector checks passed");
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}
