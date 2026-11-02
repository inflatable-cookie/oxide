//! `#[cfg(test)]` observation tape for the prune capture harness (X-S1).
//!
//! Owning contract: `docs/knowledge/contracts/barge-adoption.md` (the shared
//! capture contract, X-S1). This module is compiled only under `cfg(test)`;
//! a production build contains none of it, which `effigy rust:compile-release`
//! and the per-file diff discipline prove. It is an observation tape, not an
//! injection stub: every leaf records what the real syscalls and parsers
//! actually returned, feeds nothing back, and never changes a decision.
//!
//! The transcript implements the fixed `oxide.capture.transcript/1` schema and
//! caps: at most 8192 read rows, 8 MiB of document, 64 occurrences per
//! identity key and 512 process rows. An exhausted cap is recorded as
//! truncation and marks the transcript insufficient; it never silently drops
//! a row or reports completeness. Epochs are engine-local, monotonic and
//! assigned at the leaf; occurrence numbers are counted per identity key.
//!
//! `Evaluation` export, effect rows and the harness entry are provided to
//! `src/bin/oxide/tape_harness.rs` (the `[[test]]` target whose argv contract
//! is `--fixture-root --manifest --transcript-out --case`). The library-side
//! leaves (`src/volume.rs`, `src/inventory.rs`, `src/lease/pool.rs`) resolve
//! `crate::tape` to this same sink in the re-rooted harness crate.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The fixed transcript schema version (barge-adoption.md, capture contract).
pub(crate) const TRANSCRIPT_VERSION: &str = "oxide.capture.transcript/1";
/// The fixed manifest schema version the harness reads and binds.
pub(crate) const MANIFEST_VERSION: &str = "oxide.capture.manifest/1";
/// Fixed caps (barge-adoption.md, serialized schemas and caps).
pub(crate) const MAX_READ_ROWS: usize = 8192;
pub(crate) const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_OCCURRENCES_PER_KEY: usize = 64;
pub(crate) const MAX_PROCESS_ROWS: usize = 512;
/// The manifest read bound; an oversized manifest is a harness refusal.
pub(crate) const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
/// Fixture-entry bound carried with the manifest and the harness capture.
pub(crate) const MAX_FIXTURE_ENTRIES: usize = 4096;

/// The outcome of one recorded read or open, in the fixed vocabulary: a bytes
/// digest, an explicit absent, a refusal class, or a positively opened object
/// with its own identity.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub(crate) enum TapeOutcome {
    Digest { digest: String },
    Opened { dev: u64, ino: u64 },
    Absent,
    Refusal { reason: String },
    Facts { facts: String },
}

impl TapeOutcome {
    fn kind(&self) -> &'static str {
        match self {
            Self::Digest { .. } => "digest",
            Self::Opened { .. } => "opened",
            Self::Absent => "absent",
            Self::Refusal { .. } => "refusal",
            Self::Facts { .. } => "facts",
        }
    }
}

/// The complete object identity of one row: absolute path, observed parent
/// and own device/inode where the leaf saw them, or the volume's UUID, mount
/// point and device. `key` is the canonical identity key occurrence counting
/// binds to.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TapeIdentity {
    pub(crate) path: Option<String>,
    pub(crate) component: Option<String>,
    pub(crate) parent_dev: Option<u64>,
    pub(crate) parent_ino: Option<u64>,
    pub(crate) dev: Option<u64>,
    pub(crate) ino: Option<u64>,
    pub(crate) uuid: Option<String>,
    pub(crate) mount_point: Option<String>,
    pub(crate) device: Option<String>,
    pub(crate) key: String,
}

/// One recorded read row: `{kind, identity, outcome, cap, epoch, occurrence}`.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TapeReadRow {
    pub(crate) kind: String,
    pub(crate) identity: TapeIdentity,
    pub(crate) outcome: TapeOutcome,
    pub(crate) cap: u64,
    pub(crate) epoch: u64,
    pub(crate) occurrence: u64,
}

/// One recorded effect row: `{kind, path, epoch}`. Kinds are drawn from the
/// fixed effect vocabulary (`create`, `save`, `fence`, `reconcile`,
/// `lock-create`, `rename`, `unlink`); the source site is a separate field.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TapeEffectRow {
    pub(crate) kind: String,
    pub(crate) site: String,
    pub(crate) attempted: bool,
    pub(crate) path: String,
    pub(crate) epoch: u64,
}

/// One recorded clock row, bound to the declared window.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TapeClockRow {
    pub(crate) kind: String,
    pub(crate) value: u64,
    pub(crate) epoch: u64,
}

/// One recorded classification row (writer-custody record classes, boot
/// classifications, budget checks). Classifications are not file reads, so
/// they ride in their own bounded array instead of bending the read-row
/// outcome vocabulary. Additive field, documented in the owning contract.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TapeClassifyRow {
    pub(crate) kind: String,
    pub(crate) identity: String,
    pub(crate) value: String,
    pub(crate) epoch: u64,
    pub(crate) occurrence: u64,
}

/// The engine's actual outcome, in its own vocabulary.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub(crate) enum TapeEngineOutcome {
    Completed { result: serde_json::Value },
    Refused { reason: String, exit_code: u8, source: &'static str },
}

/// A holder as recorded in the parsed control file. Its userspace wall start
/// is not a kernel process-start identity and no UID is inferred.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TapeRecordedHolder {
    pub(crate) site: String,
    pub(crate) record_id: u64,
    pub(crate) pid: u32,
    pub(crate) recorded_start_ms: u64,
    pub(crate) kernel_start: Option<u64>,
    pub(crate) uid: Option<u32>,
    pub(crate) epoch: u64,
}

/// Truncation facts: which cap closed the document, never a quiet drop.
#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct TapeTruncation {
    pub(crate) read_rows_truncated: bool,
    pub(crate) occurrences_truncated: bool,
    pub(crate) document_truncated: bool,
    pub(crate) process_rows_truncated: bool,
    pub(crate) classifications_truncated: bool,
}

/// The transcript document, `oxide.capture.transcript/1`.
#[derive(Debug, Serialize)]
pub(crate) struct TapeTranscript {
    pub(crate) version: &'static str,
    pub(crate) engine: String,
    pub(crate) case: String,
    pub(crate) manifest_sha256: String,
    pub(crate) entrypoint: String,
    pub(crate) window_monotonic_start_ns: Option<u64>,
    pub(crate) window_monotonic_end_ns: Option<u64>,
    pub(crate) window_elapsed_ns: u64,
    pub(crate) outcome: TapeEngineOutcome,
    pub(crate) truncation: TapeTruncation,
    pub(crate) effects: Vec<TapeEffectRow>,
    pub(crate) reads: Vec<TapeReadRow>,
    pub(crate) clocks: Vec<TapeClockRow>,
    pub(crate) classifications: Vec<TapeClassifyRow>,
    pub(crate) recorded_holders: Vec<TapeRecordedHolder>,
    /// Additive (documented): the harness's own pre/post fixture identity
    /// capture. The diff is an independent cross-check for recorded effects;
    /// it cannot substitute for an observation at a write site.
    pub(crate) fixture_capture: Option<TapeFixtureCapture>,
    /// Additive (documented): notes the harness must carry, e.g. an unbound
    /// authority read pending at close or an unknown extra identity.
    pub(crate) notes: Vec<String>,
}

/// One fixture entry as the harness captured it, before or after the window.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TapeFixtureEntry {
    pub(crate) rel: String,
    pub(crate) kind: String,
    pub(crate) size: u64,
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) parent_dev: Option<u64>,
    pub(crate) parent_ino: Option<u64>,
    pub(crate) mode: u32,
    pub(crate) bytes_sha256: Option<String>,
}

/// The harness's pre/post fixture identity capture and its diff.
#[derive(Debug, Serialize)]
pub(crate) struct TapeFixtureCapture {
    pub(crate) root: String,
    pub(crate) truncated: bool,
    pub(crate) pre: Vec<TapeFixtureEntry>,
    pub(crate) post: Vec<TapeFixtureEntry>,
    /// Observed net fixture changes derived from the pre/post diff.
    pub(crate) diff_effects: Vec<TapeEffectRow>,
}

/// The recorded state of one active window. Recording is synchronous on the
/// engine thread; a second `begin` in one thread is refused.
struct TapeState {
    engine: String,
    case: String,
    manifest_sha256: String,
    entrypoint: String,
    started: Instant,
    epoch: u64,
    effects: Vec<TapeEffectRow>,
    reads: Vec<TapeReadRow>,
    clocks: Vec<TapeClockRow>,
    classifications: Vec<TapeClassifyRow>,
    recorded_holders: Vec<TapeRecordedHolder>,
    occurrences: std::collections::HashMap<String, u64>,
    truncation: TapeTruncation,
    document_bytes: usize,
    notes: Vec<String>,
    /// An authority read whose identity is not yet bound by its caller.
    pending_authority: Option<PendingAuthorityRead>,
    /// The next effect row's replacement kind, consumed once.
    pending_effect_kind: Option<String>,
    disconnected_pool_lock: bool,
    disconnected_lock_observations: Vec<(PathBuf, TapeOutcome)>,
}

struct PendingAuthorityRead {
    outcome: TapeOutcome,
    cap: u64,
}

thread_local! {
    static TAPE: RefCell<Option<TapeState>> = const { RefCell::new(None) };
}

/// Begin one engine window. A second begin on one thread is refused: the
/// harness runs exactly one engine window per process.
pub(crate) fn begin(engine: &str, case: &str, manifest_sha256: &str, entrypoint: &str) {
    TAPE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_some() {
            panic!("the observation tape began twice on one thread");
        }
        *slot = Some(TapeState {
            engine: engine.to_owned(),
            case: case.to_owned(),
            manifest_sha256: manifest_sha256.to_owned(),
            entrypoint: entrypoint.to_owned(),
            started: Instant::now(),
            epoch: 0,
            effects: Vec::new(),
            reads: Vec::new(),
            clocks: Vec::new(),
            classifications: Vec::new(),
            recorded_holders: Vec::new(),
            occurrences: std::collections::HashMap::new(),
            truncation: TapeTruncation::default(),
            document_bytes: 0,
            notes: Vec::new(),
            pending_authority: None,
            pending_effect_kind: None,
            disconnected_pool_lock: false,
            disconnected_lock_observations: Vec::new(),
        });
    });
}

/// Bind the exact manifest bytes after the already-open observation window.
pub(crate) fn bind_manifest_sha256(digest: &str) {
    with_state(|state| state.manifest_sha256 = digest.to_owned());
}

pub(crate) fn active() -> bool {
    TAPE.with(|cell| cell.borrow().is_some())
}

/// Evaluate manifest observation obligations without changing the engine's
/// result. Only fields actually represented by the tape can establish a match;
/// absent parent, process, component or volume identity remains insufficient.
fn file_identity_matches(row: &TapeReadRow, identity: &serde_json::Value) -> bool {
    let i = &row.identity;
    identity.get("path").and_then(|v| v.as_str()) == i.path.as_deref()
        && identity.get("parent_device").and_then(|v| v.as_u64()) == i.parent_dev
        && identity.get("parent_inode").and_then(|v| v.as_u64()) == i.parent_ino
        && identity.get("device").and_then(|v| v.as_u64()) == i.dev
        && identity.get("inode").and_then(|v| v.as_u64()) == i.ino
        && i.path.is_some() && i.parent_dev.is_some() && i.parent_ino.is_some()
        && i.dev.is_some() && i.ino.is_some()
}

fn identity_matches(row: &TapeReadRow, identity: &serde_json::Value, window: Option<&str>) -> bool {
    if identity.get("path").is_some() {
        return file_identity_matches(row, identity);
    }
    if let Some(name) = identity.get("component").and_then(|v| v.as_str()) {
        return row.identity.component.as_deref() == Some(name)
            && row.identity.parent_dev.is_some() && row.identity.parent_ino.is_some()
            && identity.get("parent_device").and_then(|v| v.as_u64()) == row.identity.parent_dev
            && identity.get("parent_inode").and_then(|v| v.as_u64()) == row.identity.parent_ino;
    }
    if let Some(uuid) = identity.get("volume_uuid").and_then(|v| v.as_str()) {
        return row.identity.uuid.as_deref() == Some(uuid)
            && identity.get("mount").and_then(|v| v.as_str()) == row.identity.mount_point.as_deref()
            && identity.get("device").and_then(|v| v.as_str()) == row.identity.device.as_deref()
            && row.identity.mount_point.is_some() && row.identity.device.is_some()
            && window.is_some() && identity.get("window_identity").and_then(|v| v.as_str()) == window;
    }
    // A PID alone or a process-start identity not present on this row cannot
    // be upgraded into a match by parsing the display key.
    false
}

pub(crate) fn check_manifest_obligations(manifest: &serde_json::Value, root: &Path) {
    with_state(|state| {
        let mut issues = std::collections::BTreeSet::new();
        if let Some(obligations) = manifest.get("mandatory_observations").and_then(|v| v.as_array()) {
            for obligation in obligations {
                if obligation.get("engine").and_then(|v| v.as_str()) != Some("oxide") {
                    continue;
                }
                let site = obligation.get("site").and_then(|v| v.as_str()).unwrap_or("");
                let identity = &obligation["identity"];
                if !state.reads.iter().any(|row| row.kind == site && identity_matches(row, identity, manifest.pointer("/window/identity").and_then(|v| v.as_str()))) {
                    issues.insert(format!("mandatory observation missing or identity unproven at {site}"));
                }
            }
        }
        let entries = manifest.pointer("/fixture/entries").and_then(|v| v.as_array());
        for row in &state.reads {
            let bound_file = row.identity.path.as_deref().is_some_and(|path| {
                let fixture_match = entries.is_some_and(|entries| entries.iter().any(|entry| {
                    let Some(rel) = entry.get("rel").and_then(|v| v.as_str()) else { return false; };
                    let relative = Path::new(rel);
                    if !relative.components().all(|part| matches!(part, std::path::Component::Normal(_))) {
                        return false;
                    }
                    root.join(relative) == Path::new(path)
                        && entry.get("parent_device").and_then(|v| v.as_u64()) == row.identity.parent_dev
                        && entry.get("parent_inode").and_then(|v| v.as_u64()) == row.identity.parent_ino
                        && row.identity.parent_dev.is_some() && row.identity.parent_ino.is_some()
                        && match &row.outcome {
                            TapeOutcome::Absent => entry.get("kind").and_then(|v| v.as_str()) == Some("absent"),
                            TapeOutcome::Refusal { reason } => entry.get("kind").and_then(|v| v.as_str()) == Some("unreadable")
                                && entry.get("reason").and_then(|v| v.as_str()) == Some(reason.as_str()),
                            outcome => entry.get("device").and_then(|v| v.as_u64()) == row.identity.dev
                                && entry.get("inode").and_then(|v| v.as_u64()) == row.identity.ino
                                && row.identity.dev.is_some() && row.identity.ino.is_some()
                                && match outcome {
                                    TapeOutcome::Digest { digest } => entry.get("bytes_sha256").and_then(|v| v.as_str()) == Some(digest.as_str()),
                                    _ => true,
                                },
                        }
                }));
                // The proposed allowance carries a path/class but no observed
                // parent or object identity; it cannot establish a binding.
                fixture_match
            });
            let bound_component = row.identity.component.as_deref().is_some_and(|name| {
                row.identity.parent_dev.is_some() && row.identity.parent_ino.is_some()
                    && entries.is_some_and(|items| items.iter().any(|entry| {
                        let rel = entry.get("rel").and_then(|value| value.as_str()).map(Path::new);
                        rel.is_some_and(|path| path.components().all(|part| matches!(part, std::path::Component::Normal(_))))
                            && rel.and_then(Path::file_name).and_then(|value| value.to_str()) == Some(name)
                            && entry.get("parent_device").and_then(|v| v.as_u64()) == row.identity.parent_dev
                            && entry.get("parent_inode").and_then(|v| v.as_u64()) == row.identity.parent_ino
                            && match &row.outcome {
                                TapeOutcome::Absent => entry.get("kind").and_then(|v| v.as_str()) == Some("absent"),
                                TapeOutcome::Refusal { reason } => entry.get("kind").and_then(|v| v.as_str()) == Some("unreadable")
                                    && entry.get("reason").and_then(|v| v.as_str()) == Some(reason.as_str()),
                                TapeOutcome::Opened { dev, ino } => entry.get("device").and_then(|v| v.as_u64()) == Some(*dev)
                                    && entry.get("inode").and_then(|v| v.as_u64()) == Some(*ino),
                                _ => !matches!(entry.get("kind").and_then(|v| v.as_str()), Some("absent" | "unreadable")),
                            }
                    }))
            });
            let bound_volume = manifest.pointer("/dynamic/volumes").and_then(|v| v.as_array())
                .is_some_and(|volumes| volumes.iter().any(|volume| {
                    let window = manifest.pointer("/window/identity").and_then(|v| v.as_str());
                    row.identity.uuid.is_some() && row.identity.mount_point.is_some() && row.identity.device.is_some()
                        && volume.get("uuid").and_then(|v| v.as_str()) == row.identity.uuid.as_deref()
                        && volume.get("mount").and_then(|v| v.as_str()) == row.identity.mount_point.as_deref()
                        && volume.get("device").and_then(|v| v.as_str()) == row.identity.device.as_deref()
                        && window.is_some() && volume.get("window_identity").and_then(|v| v.as_str()) == window
                }));
            if !bound_file && !bound_component && !bound_volume {
                issues.insert(format!("extra or unbound observation at {}", row.kind));
            }
        }
        for issue in issues {
            push_note(state, issue);
        }
    });
}

/// Compare exact declared and observed effect multisets. Attempts remain
/// insufficient even if a manifest declared them.
pub(crate) fn check_manifest_effects(manifest: &serde_json::Value) {
    with_state(|state| {
        let mut declared = std::collections::BTreeMap::new();
        if let Some(rows) = manifest.get("expected_effects").and_then(|value| value.as_array()) {
            for row in rows {
                let key = (row.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_owned(),
                    row.get("site").and_then(|v| v.as_str()).unwrap_or("").to_owned(),
                    row.get("path").and_then(|v| v.as_str()).unwrap_or("").to_owned(),
                    row.get("attempted").and_then(|v| v.as_bool()).unwrap_or(true));
                *declared.entry(key).or_insert(0usize) += 1;
            }
        }
        let mut actual = std::collections::BTreeMap::new();
        for row in &state.effects {
            let key = (row.kind.clone(), row.site.clone(), row.path.clone(), row.attempted);
            *actual.entry(key).or_insert(0usize) += 1;
        }
        let mut issues = Vec::new();
        for (key, count) in &declared {
            let observed = actual.get(key).copied().unwrap_or(0);
            if observed < *count {
                issues.push(format!("missing declared effect {} at {}: {}", key.0, key.1, key.2));
            }
        }
        for (key, count) in &actual {
            let expected = declared.get(key).copied().unwrap_or(0);
            if *count > expected {
                issues.push(format!("extra observed effect {} at {}: {}", key.0, key.1, key.2));
            }
        }
        for issue in issues {
            push_note(state, issue);
        }
    });
}

/// A final fixture diff can corroborate a persistent effect, never replace a
/// temporal observation. Unmatched changes or a truncated capture are explicit.
pub(crate) fn check_fixture_diff(root: &Path, capture: &TapeFixtureCapture) {
    with_state(|state| {
        if capture.truncated {
            push_note(state, "fixture capture truncated; effect agreement insufficient".to_owned());
        }
        let mut issues = Vec::new();
        for diff in &capture.diff_effects {
            let path = root.join(&diff.path);
            let matched = state.effects.iter().any(|effect| {
                !effect.attempted && Path::new(&effect.path) == path.as_path()
                    && match diff.kind.as_str() {
                        "create" => matches!(effect.kind.as_str(), "create" | "lock-create" | "rename"),
                        "save" => matches!(effect.kind.as_str(), "save" | "fence" | "reconcile" | "rename"),
                        "unlink" => matches!(effect.kind.as_str(), "unlink" | "rename"),
                        _ => false,
                    }
            });
            if !matched {
                issues.push(format!("fixture change lacks a matching observed effect: {}", diff.path));
            }
        }
        for issue in issues {
            push_note(state, issue);
        }
    });
}

/// Close the window and write the transcript. Always records the note when an
/// authority read was left unbound. The document is written once, bounded.
pub(crate) fn finish_and_write(
    path: &Path,
    outcome: TapeEngineOutcome,
    fixture_capture: Option<TapeFixtureCapture>,
) -> Result<(), String> {
    let taken = TAPE.with(|cell| cell.borrow_mut().take());
    let mut transcript = {
        let state = taken.ok_or_else(|| "the observation tape was not active".to_owned())?;
        let mut notes = state.notes.clone();
        if state.disconnected_pool_lock {
            notes.push(format!("counterfactual diverted pool-lock observations: {}",
                state.disconnected_lock_observations.len()));
        }
        notes.push("absolute monotonic window origin and end unproven".to_owned());
        if state.pending_authority.is_some() {
            notes.push(
                "an authority read ended its window without its caller binding its identity"
                    .to_owned(),
            );
        }
        TapeTranscript {
            version: TRANSCRIPT_VERSION,
            engine: state.engine,
            case: state.case,
            manifest_sha256: state.manifest_sha256,
            entrypoint: state.entrypoint,
            window_monotonic_start_ns: None,
            window_monotonic_end_ns: None,
            window_elapsed_ns: u64::try_from(state.started.elapsed().as_nanos())
                .unwrap_or(u64::MAX),
            outcome,
            truncation: state.truncation,
            effects: state.effects,
            reads: state.reads,
            clocks: state.clocks,
            classifications: state.classifications,
            recorded_holders: state.recorded_holders,
            fixture_capture,
            notes,
        }
    };
    let mut bytes = serde_json::to_vec(&transcript).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        transcript.truncation.document_truncated = true;
        transcript.reads.clear();
        transcript.effects.clear();
        transcript.clocks.clear();
        transcript.classifications.clear();
        transcript.recorded_holders.clear();
        transcript.fixture_capture = None;
        transcript.notes.clear();
        bytes = serde_json::to_vec(&transcript).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_DOCUMENT_BYTES {
            transcript.outcome = match transcript.outcome {
                TapeEngineOutcome::Completed { .. } => TapeEngineOutcome::Completed {
                    result: serde_json::json!({"truncated": true}),
                },
                TapeEngineOutcome::Refused { exit_code, source, .. } => TapeEngineOutcome::Refused {
                    reason: "refusal detail exceeded the document cap".to_owned(),
                    exit_code,
                    source,
                },
            };
            bytes = serde_json::to_vec(&transcript).map_err(|error| error.to_string())?;
        }
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err("transcript header exceeds the document cap".to_owned());
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temp, path).map_err(|error| error.to_string())?;
    Ok(())
}

fn with_state<R>(work: impl FnOnce(&mut TapeState) -> R) -> Option<R> {
    TAPE.with(|cell| {
        let mut slot = cell.borrow_mut();
        slot.as_mut().map(work)
    })
}

/// One engine-local epoch, assigned at the leaf.
fn next_epoch(state: &mut TapeState) -> u64 {
    state.epoch = state.epoch.saturating_add(1);
    state.epoch
}

/// Reserve the next occurrence number for one identity key, or report
/// exhaustion (recorded as truncation, never a dropped row).
fn occurrence(state: &mut TapeState, key: &str) -> (u64, bool) {
    if !state.occurrences.contains_key(key) && state.occurrences.len() >= MAX_READ_ROWS * 2 {
        state.truncation.occurrences_truncated = true;
        return (0, false);
    }
    let next = state.occurrences.entry(key.to_owned()).or_insert(0);
    *next = next.saturating_add(1);
    if *next > MAX_OCCURRENCES_PER_KEY as u64 {
        state.truncation.occurrences_truncated = true;
        (*next, false)
    } else {
        (*next, true)
    }
}

fn charge_document(state: &mut TapeState, row_bytes: usize) {
    state.document_bytes = state.document_bytes.saturating_add(row_bytes);
    if state.document_bytes > MAX_DOCUMENT_BYTES {
        state.truncation.document_truncated = true;
    }
}

fn push_note(state: &mut TapeState, note: String) {
    if state.notes.len() >= MAX_READ_ROWS {
        state.truncation.document_truncated = true;
    } else {
        charge_document(state, note.len().saturating_add(16));
        state.notes.push(note);
    }
}

fn push_read(state: &mut TapeState, row: TapeReadRow) {
    if state.reads.len() >= MAX_READ_ROWS {
        state.truncation.read_rows_truncated = true;
        return;
    }
    charge_document(state, 256);
    state.reads.push(row);
}

fn identity_key(identity: &TapeIdentity) -> String {
    identity.key.clone()
}

fn file_identity(path: &Path, outcome: &TapeOutcome) -> TapeIdentity {
    // A later pathname lookup does not identify the object used by the leaf.
    // Keep only identity returned by the leaf itself; parent binding is absent.
    let (parent_dev, parent_ino) = (None::<u64>, None::<u64>);
    let (dev, ino) = match outcome {
        TapeOutcome::Opened { dev, ino } => (Some(*dev), Some(*ino)),
        _ => (None, None),
    };
    let key = format!(
        "file:{path}|parent={parent_dev:?}/{parent_ino:?}|dev={dev:?}/ino={ino:?}",
        path = path.display(),
    );
    TapeIdentity {
        path: path.to_str().map(str::to_owned),
        component: None,
        parent_dev,
        parent_ino,
        dev,
        ino,
        uuid: None,
        mount_point: None,
        device: None,
        key,
    }
}

/// Record the facts supplied by a leaf. Unknown identities remain unproven;
/// the recorder does not make a later pathname lookup to invent a binding.
pub(crate) fn read_row(kind: &str, path: &Path, outcome: TapeOutcome, cap: u64) {
    with_state(|state| {
        let identity = file_identity(path, &outcome);
        push_note(state, format!("object or parent identity unproven at {kind}"));
        let key = format!("{kind}|{}", identity_key(&identity));
        let (occurrence, counted) = occurrence(state, &key);
        let epoch = next_epoch(state);
        let row = TapeReadRow {
            kind: kind.to_owned(),
            identity,
            outcome,
            cap,
            epoch,
            occurrence,
        };
        if counted {
            push_read(state, row);
        }
    });
}

/// Record one read row whose own identity the leaf already observed.
pub(crate) fn read_row_dev(
    kind: &str,
    path: &Path,
    dev: u64,
    ino: u64,
    outcome: TapeOutcome,
    cap: u64,
) {
    read_row_without_parent(kind, path, dev, ino, outcome, cap);
}

/// Record one component read of a no-follow walk: the component name bound to
/// its observed parent identity, never to a string alone.
pub(crate) fn component_row(
    kind: &str,
    parent_dev: u64,
    parent_ino: u64,
    name: &std::ffi::OsStr,
    outcome: TapeOutcome,
) {
    with_state(|state| {
        let (dev, ino) = match &outcome {
            TapeOutcome::Opened { dev, ino } => (Some(*dev), Some(*ino)),
            _ => (None, None),
        };
        let key = format!(
            "component:parent={parent_dev}/{parent_ino}|name={name:?}|dev={dev:?}/ino={ino:?}"
        );
        let identity = TapeIdentity {
            path: None,
            component: name.to_str().map(str::to_owned),
            parent_dev: Some(parent_dev),
            parent_ino: Some(parent_ino),
            dev,
            ino,
            uuid: None,
            mount_point: None,
            device: None,
            key,
        };
        let (occurrence, counted) = occurrence(state, &format!("{kind}|{}", identity.key));
        let epoch = next_epoch(state);
        let row = TapeReadRow {
            kind: kind.to_owned(),
            identity,
            outcome,
            cap: 0,
            epoch,
            occurrence,
        };
        if counted {
            push_read(state, row);
        }
    });
}

/// Record an actual component result when its held parent was not itself
/// observed at this epoch. The missing parent never becomes a complete key.
pub(crate) fn component_without_parent(kind: &str, name: &std::ffi::OsStr, outcome: TapeOutcome) {
    with_state(|state| {
        let (dev, ino) = match &outcome {
            TapeOutcome::Opened { dev, ino } => (Some(*dev), Some(*ino)),
            _ => (None, None),
        };
        let key = format!("component:parent=unproven|name={name:?}|dev={dev:?}/ino={ino:?}");
        let identity = TapeIdentity {
            path: None, component: name.to_str().map(str::to_owned), parent_dev: None, parent_ino: None, dev, ino,
            uuid: None, mount_point: None, device: None, key: key.clone(),
        };
        push_note(state, format!("component parent identity unproven at {kind}"));
        let (occurrence, counted) = occurrence(state, &format!("{kind}|{key}"));
        let epoch = next_epoch(state);
        if counted {
            push_read(state, TapeReadRow { kind: kind.to_owned(), identity, outcome, cap: 0, epoch, occurrence });
        }
    });
}

/// Record the start object of an absence walk (the held `/` or mount root).
pub(crate) fn walk_start_row(kind: &str, path: &Path, dev: u64, ino: u64) {
    read_row_dev(kind, path, dev, ino, TapeOutcome::Opened { dev, ino }, 0);
}

/// Record one volume observation: mount point, volume UUID, device and the
/// outcome of the binding attempt.
pub(crate) fn volume_row(
    kind: &str,
    mount: &Path,
    uuid: Option<&str>,
    device: Option<&str>,
    outcome: TapeOutcome,
) {
    with_state(|state| {
        let epoch = next_epoch(state);
        let uuid = uuid.map(str::to_owned);
        let key = format!(
            "volume:mount={mount}|uuid={uuid:?}|device={device:?}",
            mount = mount.display()
        );
        let identity = TapeIdentity {
            path: None,
            component: None,
            parent_dev: None,
            parent_ino: None,
            dev: None,
            ino: None,
            uuid: uuid.clone(),
            mount_point: mount.to_str().map(str::to_owned),
            device: device.map(str::to_owned),
            key,
        };
        let (occurrence, counted) = occurrence(state, &format!("{kind}|{}", identity.key));
        let row = TapeReadRow {
            kind: kind.to_owned(),
            identity,
            outcome,
            cap: 0,
            epoch,
            occurrence,
        };
        if counted {
            push_read(state, row);
        }
    });
}

/// An actual effect at a descriptor-only site that cannot bind a path is
/// explicitly insufficient; a pathname is never guessed after the operation.
pub(crate) fn unbound_effect(site: &str, succeeded: bool) {
    let saved_errno = observer_errno();
    with_state(|state| {
        push_note(state, format!(
            "descriptor-only effect at {site}: succeeded={succeeded}; identity unproven; insufficient"
        ));
    });
    restore_observer_errno(saved_errno);
}

/// Record a completed effect or an explicitly unqualified create attempt.
/// Internal labels carry `kind@site`; serialized fields separate them. A fence
/// override relabels only the next save, avoiding a duplicate effect row.
pub(crate) fn effect_row(kind: &str, path: &Path) {
    let saved_errno = observer_errno();
    with_state(|state| {
        let epoch = next_epoch(state);
        let kind = if kind == "save@control-state" {
            state.pending_effect_kind.take().unwrap_or_else(|| kind.to_owned())
        } else {
            kind.to_owned()
        };
        let (verb, site) = kind.split_once('@').unwrap_or((&kind, "unknown"));
        let attempted = matches!(verb, "attempt" | "attempt-save" | "attempt-rename");
        let verb = match verb {
            "attempt" => "create",
            "attempt-save" => "save",
            "attempt-rename" => "rename",
            _ => verb,
        };
        if !matches!(verb, "create" | "save" | "fence" | "reconcile" | "lock-create" | "rename" | "unlink") {
            push_note(state, format!("unknown effect kind at {kind}"));
            return;
        }
        if state.effects.len() >= MAX_READ_ROWS {
            state.truncation.document_truncated = true;
            return;
        }
        let Some(path_text) = path.to_str() else {
            push_note(state, format!("effect path at {site} is not UTF-8; identity unproven; insufficient"));
            return;
        };
        if attempted {
            push_note(state, format!("attempted effect at {site} remains insufficient"));
        }
        state.effects.push(TapeEffectRow {
            kind: verb.to_owned(),
            site: site.to_owned(),
            attempted,
            path: path_text.to_owned(),
            epoch,
        });
        charge_document(state, 96);
    });
    restore_observer_errno(saved_errno);
}

/// Relabel the next effect row (and only that one) with `kind`. The guard
/// clears the override when dropped, so an effect that never happens cannot
/// mislabel a later one.
pub(crate) fn effect_kind_override(kind: &'static str) -> EffectKindGuard {
    with_state(|state| state.pending_effect_kind = Some(kind.to_owned()));
    EffectKindGuard
}

pub(crate) struct EffectKindGuard;

impl Drop for EffectKindGuard {
    fn drop(&mut self) {
        with_state(|state| state.pending_effect_kind = None);
    }
}

/// Record one clock reading (policy wall clock in milliseconds).
pub(crate) fn clock_row(kind: &str, value_ms: u64) {
    with_state(|state| {
        let epoch = next_epoch(state);
        state.clocks.push(TapeClockRow {
            kind: kind.to_owned(),
            value: value_ms,
            epoch,
        });
        charge_document(state, 48);
    });
}

/// Record an Instant the policy already sampled, relative to this window's
/// own monotonic start. This does not infer a system-wide monotonic origin.
pub(crate) fn monotonic_observed(kind: &str, observed: Instant) {
    let saved_errno = observer_errno();
    with_state(|state| {
        if state.clocks.len() >= MAX_READ_ROWS {
            state.truncation.document_truncated = true;
            return;
        }
        match observed.checked_duration_since(state.started) {
            Some(duration) => {
                let epoch = next_epoch(state);
                state.clocks.push(TapeClockRow {
                    kind: kind.to_owned(),
                    value: u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX),
                    epoch,
                });
                charge_document(state, 48);
            }
            None => push_note(state, format!("monotonic reading before window at {kind}; insufficient")),
        }
    });
    restore_observer_errno(saved_errno);
}

/// A clock the engine could not obtain remains explicitly unproven.
pub(crate) fn clock_unproven(kind: &str) {
    with_state(|state| push_note(state, format!("clock unproven at {kind}")));
}

pub(crate) fn recorded_holder(site: &str, record_id: u64, pid: u32, recorded_start_ms: u64) {
    let saved_errno = observer_errno();
    with_state(|state| {
        if state.recorded_holders.len() >= MAX_PROCESS_ROWS {
            state.truncation.process_rows_truncated = true;
            return;
        }
        let epoch = next_epoch(state);
        state.recorded_holders.push(TapeRecordedHolder {
            site: site.to_owned(), record_id, pid, recorded_start_ms,
            kernel_start: None, uid: None, epoch,
        });
        push_note(state, format!("recorded holder at {site} has no kernel start or UID; insufficient"));
        charge_document(state, 96);
    });
    restore_observer_errno(saved_errno);
}

/// Record one classification observation (custody record classes, boot
/// classifications, budget checks).
pub(crate) fn classify_row(kind: &str, identity: &str, value: &str) {
    with_state(|state| {
        const MAX_CLASSIFY_ROWS: usize = MAX_READ_ROWS;
        if state.classifications.len() >= MAX_CLASSIFY_ROWS {
            state.truncation.classifications_truncated = true;
            return;
        }
        let (occurrence, counted) = occurrence(state, &format!("classify:{kind}|{identity}"));
        let epoch = next_epoch(state);
        let row = TapeClassifyRow {
            kind: kind.to_owned(),
            identity: identity.to_owned(),
            value: value.to_owned(),
            epoch,
            occurrence,
        };
        if counted {
            state.classifications.push(row);
            charge_document(state, 96);
        }
    });
}

/// Hold the result until its caller binds the actual read identity. This is
/// bookkeeping for one read, not a second synthetic read row.
pub(crate) fn authority_core(bytes: &[u8], cap: u64, budget_failed: bool, refused: bool) {
    with_state(|state| {
        if state.pending_authority.is_some() {
            state
                .notes
                .push("an authority read was replaced before identity binding".to_owned());
        }
        let outcome = if refused {
            TapeOutcome::Refusal {
                reason: "authority read refused".to_owned(),
            }
        } else {
            TapeOutcome::Digest {
                digest: sha256_hex(bytes),
            }
        };
        if budget_failed {
            push_note(state, "the authority budget failed".to_owned());
        }
        state.pending_authority = Some(PendingAuthorityRead { outcome, cap });
    });
}

/// Bind the pending authority read to its caller's source site and complete
/// object identity (absolute path, observed parent, own dev/ino). Called
/// immediately after the read statement at every authority-read call site.
pub(crate) fn authority_bind(site: &str, path: &Path, cap: u64) {
    with_state(|state| {
        let Some(pending) = state.pending_authority.take() else {
            push_note(state, format!(
                "authority-bind at {site} found no pending read; the read stays unbound"
            ));
            return;
        };
        let outcome = pending.outcome;
        let identity = file_identity(path, &outcome);
        push_note(state, format!("descriptor and parent identity unproven at {site}"));
        let key = format!("{site}|{}", identity.key);
        let (occurrence, counted) = occurrence(state, &key);
        let epoch = next_epoch(state);
        let row = TapeReadRow {
            kind: format!("authority-read@{site}"),
            identity,
            outcome,
            cap: pending.cap.max(cap),
            epoch,
            occurrence,
        };
        if counted {
            push_read(state, row);
        }
    });
}

/// Record an observed object without inventing the identity of its parent.
/// A path lookup after the leaf operation cannot bind that parent to the read.
pub(crate) fn read_row_without_parent(
    kind: &str,
    path: &Path,
    dev: u64,
    ino: u64,
    outcome: TapeOutcome,
    cap: u64,
) {
    with_state(|state| {
        let key = format!(
            "file:{}|parent=unproven|dev={dev}/ino={ino}",
            path.display()
        );
        let identity = TapeIdentity {
            path: path.to_str().map(str::to_owned),
            component: None,
            parent_dev: None,
            parent_ino: None,
            dev: Some(dev),
            ino: Some(ino),
            uuid: None,
            mount_point: None,
            device: None,
            key: key.clone(),
        };
        push_note(state, format!("parent identity unproven at {kind}"));
        let (occurrence, counted) = occurrence(state, &format!("{kind}|{key}"));
        let epoch = next_epoch(state);
        if counted {
            push_read(
                state,
                TapeReadRow {
                    kind: kind.to_owned(),
                    identity,
                    outcome,
                    cap,
                    epoch,
                    occurrence,
                },
            );
        }
    });
}

/// Record a read against the descriptor that supplied the bytes.
pub(crate) fn authority_bind_descriptor(
    site: &str,
    path: &Path,
    dev: u64,
    ino: u64,
    cap: u64,
    outcome: TapeOutcome,
) {
    with_state(|state| {
        state.pending_authority = None;
    });
    read_row_without_parent(
        &format!("authority-read@{site}"),
        path,
        dev,
        ino,
        outcome,
        cap,
    );
}

pub(crate) fn authority_unbound(site: &str, reason: &str) {
    with_state(|state| {
        state.pending_authority = None;
        push_note(state, format!(
            "descriptor identity unavailable at {site}: {reason}"
        ));
    });
}

/// Record a pool open outcome (the `Store::open_at`/`ledger_view` chain).
pub(crate) fn pool_row(kind: &str, pool: &Path, outcome: TapeOutcome) {
    read_row(kind, pool, outcome, 0);
}

/// A held descriptor's already-observed identity, without inventing its path,
/// parent or process-start identity.
pub(crate) fn descriptor_row(site: &str, fd: i32, outcome: TapeOutcome) {
    let saved_errno = observer_errno();
    with_state(|state| {
        let (dev, ino) = match &outcome {
            TapeOutcome::Opened { dev, ino } => (Some(*dev), Some(*ino)),
            _ => (None, None),
        };
        let identity = TapeIdentity {
            path: None, component: None, parent_dev: None, parent_ino: None, dev, ino,
            uuid: None, mount_point: None, device: None,
            key: format!("descriptor:{fd}|process-start=unproven|dev={dev:?}|ino={ino:?}"),
        };
        push_note(state, format!("descriptor path, parent and process-start unproven at {site}"));
        let (occurrence, counted) = occurrence(state, &format!("{site}|{}", identity.key));
        let epoch = next_epoch(state);
        if counted {
            push_read(state, TapeReadRow { kind: site.to_owned(), identity, outcome,
                cap: 0, epoch, occurrence });
        }
    });
    restore_observer_errno(saved_errno);
}

/// Record one lock outcome (admission locks, pool locks).
pub(crate) fn lock_row(kind: &str, path: &Path, outcome: TapeOutcome) {
    let disconnected = with_state(|state| {
        if kind == "pool:lock" && state.disconnected_pool_lock {
            if state.disconnected_lock_observations.len() < MAX_READ_ROWS {
                state.disconnected_lock_observations.push((path.to_path_buf(), outcome.clone()));
            } else {
                state.truncation.read_rows_truncated = true;
            }
            push_note(state, "counterfactual: pool lock routed away from shared tape; insufficient".to_owned());
            true
        } else {
            false
        }
    }).unwrap_or(false);
    if !disconnected {
        read_row(kind, path, outcome, 0);
    }
}

/// Negative control for the harness's actual engine invocation. It changes
/// observation only and makes the resulting transcript explicitly insufficient.
pub(crate) fn disconnect_pool_lock_for_counterfactual() {
    with_state(|state| state.disconnected_pool_lock = true);
}

/// The authority budget's beginning: its deadline and input allowance are the
/// budget state every read charges against.
pub(crate) fn budget_begin(allowance_bytes: u64) {
    with_state(|state| {
        push_note(state, "authority monotonic deadline identity unproven".to_owned());
    });
    classify_row(
        "authority-budget",
        "allowance",
        &allowance_bytes.to_string(),
    );
}

/// The authority budget's end: whether it failed and what remained.
pub(crate) fn budget_end(failed: bool, remaining: u64) {
    classify_row(
        "authority-budget",
        "final",
        &format!("failed={failed} remaining={remaining}"),
    );
}

/// An observation site without a captured syscall result is insufficient.
/// Never issue a second path or descriptor syscall to fill that gap.
pub(crate) fn fd_dev_ino(_fd: std::os::fd::BorrowedFd<'_>) -> Option<(u64, u64)> {
    with_state(|state| {
        push_note(state, "descriptor identity unproven at observation site".to_owned());
    });
    None
}

/// Test-only helper: SHA-256 of bytes, lowercase hex.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

fn config_bounded(text: &str) -> String {
    const LIMIT: usize = 240;
    text.chars().take(LIMIT).collect()
}

/// The harness's fixture capture: walk `root` bounded, binding every entry's
/// identity (rel path, kind, size, dev, ino, parent identity, mode and bytes
/// digest). Over the entry cap the capture is truncated and marked.
pub(crate) fn capture_fixture(root: &Path) -> TapeFixtureCapture {
    let mut entries = Vec::new();
    let mut truncated = false;
    let mut remaining = 32_u64 * 1024 * 1024;
    walk_fixture(root, root, &mut entries, &mut truncated, &mut remaining);
    TapeFixtureCapture {
        root: root.display().to_string(),
        truncated,
        pre: Vec::new(),
        post: entries,
        diff_effects: Vec::new(),
    }
}

fn walk_fixture(
    root: &Path,
    dir: &Path,
    entries: &mut Vec<TapeFixtureEntry>,
    truncated: &mut bool,
    remaining: &mut u64,
) {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(_) => {
            *truncated = true;
            return;
        }
    };
    let mut names: Vec<(String, PathBuf)> = Vec::new();
    for entry in read_dir {
        match entry {
            Ok(entry) => {
                if let Some(name) = entry.file_name().to_str() {
                    names.push((name.to_owned(), entry.path()));
                } else {
                    // A lossy name could alias a different fixture entry.
                    *truncated = true;
                }
            }
            Err(_) => *truncated = true,
        }
    }
    names.sort();
    for (_name, path) in names {
        if entries.len() >= MAX_FIXTURE_ENTRIES {
            *truncated = true;
            return;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            *truncated = true;
            continue;
        };
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let rel = path
            .strip_prefix(root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| path.display().to_string());
        let kind = if meta.file_type().is_symlink() {
            "symlink".to_owned()
        } else if meta.is_dir() {
            "dir".to_owned()
        } else if meta.is_file() {
            "file".to_owned()
        } else {
            "other".to_owned()
        };
        let bytes_sha256 = if meta.is_file() {
            use std::io::Read;
            let mut bytes = Vec::new();
            let read = std::fs::File::open(&path)
                .and_then(|file| file.take(remaining.saturating_add(1)).read_to_end(&mut bytes));
            match read {
                Ok(_) if bytes.len() as u64 <= *remaining => {
                    *remaining -= bytes.len() as u64;
                    Some(sha256_hex(&bytes))
                }
                _ => {
                    *truncated = true;
                    None
                }
            }
        } else {
            None
        };
        entries.push(TapeFixtureEntry {
            rel,
            kind,
            size: meta.len(),
            dev: meta.dev(),
            ino: meta.ino(),
            parent_dev: std::fs::metadata(dir).ok().map(|m| m.dev()),
            parent_ino: std::fs::metadata(dir).ok().map(|m| m.ino()),
            mode: meta.permissions().mode(),
            bytes_sha256,
        });
        if meta.is_dir() && !meta.file_type().is_symlink() {
            walk_fixture(root, &path, entries, truncated, remaining);
        }
    }
}

/// A net pre/post fixture diff for cross-checking recorded effects. It cannot
/// observe transient or failed writes and is never a substitute for a tape.
pub(crate) fn diff_fixture(
    pre: &[TapeFixtureEntry],
    post: &[TapeFixtureEntry],
) -> Vec<TapeEffectRow> {
    let mut effects = Vec::new();
    let mut index = std::collections::HashMap::new();
    for entry in pre {
        index.insert(entry.rel.clone(), entry);
    }
    let mut seen = std::collections::HashSet::new();
    for entry in post {
        seen.insert(entry.rel.clone());
        match index.get(&entry.rel) {
            None => effects.push(TapeEffectRow {
                kind: "create".to_owned(),
                site: "fixture-diff".to_owned(),
                attempted: false,
                path: entry.rel.clone(),
                epoch: 0,
            }),
            Some(before) => {
                // A directory's size moves whenever an entry is added or
                // removed beside it; only its own identity and mode are its
                // own effect facts. Files compare bytes and size.
                let is_dir = entry.kind == "dir";
                let changed = before.dev != entry.dev
                    || before.ino != entry.ino
                    || before.mode != entry.mode
                    || (!is_dir
                        && (before.size != entry.size
                            || before.bytes_sha256 != entry.bytes_sha256))
                    || (is_dir && before.bytes_sha256 != entry.bytes_sha256);
                if changed {
                    effects.push(TapeEffectRow {
                        kind: "save".to_owned(),
                site: "fixture-diff".to_owned(),
                attempted: false,
                        path: entry.rel.clone(),
                        epoch: 0,
                    });
                }
            }
        }
    }
    for entry in pre {
        if !seen.contains(&entry.rel) {
            effects.push(TapeEffectRow {
                kind: "unlink".to_owned(),
                site: "fixture-diff".to_owned(),
                attempted: false,
                path: entry.rel.clone(),
                epoch: 0,
            });
        }
    }
    effects.sort_by(|a, b| a.path.cmp(&b.path).then(a.kind.cmp(&b.kind)));
    effects
}

// Preserve the evaluator thread's errno around test-only observation.
#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
fn observer_errno_slot() -> Option<*mut libc::c_int> {
    // SAFETY: libc returns this thread's errno location.
    Some(unsafe { libc::__errno_location() })
}

#[cfg(all(test, any(target_os = "macos", target_os = "ios")))]
fn observer_errno_slot() -> Option<*mut libc::c_int> {
    // SAFETY: libc returns this thread's errno location.
    Some(unsafe { libc::__error() })
}

#[cfg(all(
    test,
    not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))
))]
fn observer_errno_slot() -> Option<*mut libc::c_int> {
    None
}

#[cfg(test)]
pub(crate) fn observer_errno() -> Option<libc::c_int> {
    // SAFETY: the pointer, when available, is this thread's errno slot.
    observer_errno_slot().map(|slot| unsafe { *slot })
}

#[cfg(test)]
pub(crate) fn restore_observer_errno(value: Option<libc::c_int>) {
    if let (Some(slot), Some(value)) = (observer_errno_slot(), value) {
        // SAFETY: the pointer is this thread's errno slot.
        unsafe { *slot = value };
    }
}
