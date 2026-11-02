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
use std::os::fd::AsRawFd;
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
/// `lock-create`, `rename`, `unlink`), suffixed with the source site so the
/// reconciler can bind each row to its obligation.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TapeEffectRow {
    pub(crate) kind: String,
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
    Refused { reason: String, exit_code: u8 },
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
    pub(crate) window_monotonic_start_ns: u64,
    pub(crate) window_monotonic_end_ns: u64,
    pub(crate) outcome: TapeEngineOutcome,
    pub(crate) truncation: TapeTruncation,
    pub(crate) effects: Vec<TapeEffectRow>,
    pub(crate) reads: Vec<TapeReadRow>,
    pub(crate) clocks: Vec<TapeClockRow>,
    pub(crate) classifications: Vec<TapeClassifyRow>,
    /// Additive (documented): the harness's own pre/post fixture identity
    /// capture. The diff of the two captures is ground truth for effects,
    /// including writes made inside the prebuilt library that this process
    /// cannot tape directly.
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
    /// Ground-truth effect rows derived from the pre/post diff.
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
    occurrences: std::collections::HashMap<String, u64>,
    truncation: TapeTruncation,
    document_bytes: usize,
    notes: Vec<String>,
    /// An authority read whose identity is not yet bound by its caller.
    pending_authority: Option<PendingAuthorityRead>,
    /// The next effect row's replacement kind, consumed once.
    pending_effect_kind: Option<String>,
}

struct PendingAuthorityRead {
    digest: String,
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
            occurrences: std::collections::HashMap::new(),
            truncation: TapeTruncation::default(),
            document_bytes: 0,
            notes: Vec::new(),
            pending_authority: None,
            pending_effect_kind: None,
        });
    });
}

pub(crate) fn active() -> bool {
    TAPE.with(|cell| cell.borrow().is_some())
}

/// Close the window and write the transcript. Always records the note when an
/// authority read was left unbound. The document is written once, bounded.
pub(crate) fn finish_and_write(
    path: &Path,
    outcome: TapeEngineOutcome,
    fixture_capture: Option<TapeFixtureCapture>,
) -> Result<(), String> {
    let taken = TAPE.with(|cell| cell.borrow_mut().take());
    let transcript = {
        let state = taken.ok_or_else(|| "the observation tape was not active".to_owned())?;
        let mut notes = state.notes.clone();
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
            window_monotonic_start_ns: 0,
            window_monotonic_end_ns: u64::try_from(state.started.elapsed().as_nanos())
                .unwrap_or(u64::MAX),
            outcome,
            truncation: state.truncation,
            effects: state.effects,
            reads: state.reads,
            clocks: state.clocks,
            classifications: state.classifications,
            fixture_capture,
            notes,
        }
    };
    let bytes = serde_json::to_vec(&transcript).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(format!("the observation transcript exceeds its {MAX_DOCUMENT_BYTES}-byte bound"));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temp, &bytes).map_err(|error| error.to_string())?;
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
    let parent = path.parent().map(|p| p.to_path_buf());
    let (parent_dev, parent_ino) = parent
        .as_deref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| {
            use std::os::unix::fs::MetadataExt;
            (Some(m.dev()), Some(m.ino()))
        })
        .unwrap_or((None, None));
    let (dev, ino) = match outcome {
        TapeOutcome::Opened { dev, ino } => (Some(*dev), Some(*ino)),
        _ => std::fs::symlink_metadata(path)
            .ok()
            .map(|m| {
                use std::os::unix::fs::MetadataExt;
                (Some(m.dev()), Some(m.ino()))
            })
            .unwrap_or((None, None)),
    };
    let key = format!(
        "file:{path}|parent={parent_dev:?}/{parent_ino:?}|dev={dev:?}/ino={ino:?}",
        path = path.display(),
    );
    TapeIdentity {
        path: Some(path.display().to_string()),
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

/// Record one complete read row for an object at `path`. The identity is
/// bound from the live tree (parent and own device/inode); a truncated or
/// over-cap row is still recorded, marked by the transcript's truncation
/// facts.
pub(crate) fn read_row(kind: &str, path: &Path, outcome: TapeOutcome, cap: u64) {
    with_state(|state| {
        let identity = file_identity(path, &outcome);
        let key = identity_key(&identity);
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
    with_state(|state| {
        let parent = path.parent().map(|p| p.to_path_buf());
        let (parent_dev, parent_ino) = parent
            .as_deref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| {
                use std::os::unix::fs::MetadataExt;
                (Some(m.dev()), Some(m.ino()))
            })
            .unwrap_or((None, None));
        let key = format!("file:{path}|dev={dev}/ino={ino}", path = path.display());
        let identity = TapeIdentity {
            path: Some(path.display().to_string()),
            parent_dev,
            parent_ino,
            dev: Some(dev),
            ino: Some(ino),
            uuid: None,
            mount_point: None,
            device: None,
            key: key.clone(),
        };
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
        let key = format!("component:parent={parent_dev}/{parent_ino}|name={name:?}",);
        let identity = TapeIdentity {
            path: None,
            parent_dev: Some(parent_dev),
            parent_ino: Some(parent_ino),
            dev: None,
            ino: None,
            uuid: None,
            mount_point: None,
            device: None,
            key,
        };
        let (occurrence, counted) = occurrence(state, &identity.key);
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
            "volume:mount={mount}|uuid={uuid:?}|epoch-window",
            mount = mount.display()
        );
        let identity = TapeIdentity {
            path: None,
            parent_dev: None,
            parent_ino: None,
            dev: None,
            ino: None,
            uuid: uuid.clone(),
            mount_point: Some(mount.display().to_string()),
            device: device.map(str::to_owned),
            key,
        };
        let (occurrence, counted) = occurrence(state, &identity.key);
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

/// Record one actual effect. Kinds come from the fixed vocabulary with their
/// source site: `save@control-state`, `fence@recorded-accounting`,
/// `lock-create@admission`, and so on. Only a positively completed write is
/// recorded; a refused write is the engine's refusal, never an effect. An
/// effect-kind override installed by `effect_kind_override` relabels exactly
/// the next effect row (used so a fence's save records its fence kind).
pub(crate) fn effect_row(kind: &str, path: &Path) {
    with_state(|state| {
        let epoch = next_epoch(state);
        let kind = state
            .pending_effect_kind
            .take()
            .unwrap_or_else(|| kind.to_owned());
        state.effects.push(TapeEffectRow {
            kind,
            path: path.display().to_string(),
            epoch,
        });
        charge_document(state, 96);
    });
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

/// Record one monotonic reading (nanoseconds since some fixed anchor of this
/// process; bound to the declared window by the transcript's window fields).
pub(crate) fn monotonic_row(kind: &str) {
    with_state(|state| {
        let epoch = next_epoch(state);
        state.clocks.push(TapeClockRow {
            kind: kind.to_owned(),
            value: monotonic_anchor_ns().saturating_add(
                u64::try_from(state.started.elapsed().as_nanos()).unwrap_or(u64::MAX),
            ),
            epoch,
        });
        charge_document(state, 48);
    });
}

fn monotonic_anchor_ns() -> u64 {
    // One stable anchor per process: the transcript's window fields carry the
    // same anchor's offsets, so rows reconcile without exposing raw epochs.
    0
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

/// The core of one `authority_read`, recorded inside the leaf: the digest of
/// the bytes actually returned, the cap charge and the budget state. On
/// refusal the row is published immediately and no identity bind is pending
/// (the caller's bind statement is never reached past `?`); on success the
/// digest is held for the caller's `authority_bind`, which publishes the
/// identity-bearing row for the same read.
pub(crate) fn authority_core(bytes: &[u8], cap: u64, budget_failed: bool, refused: bool) {
    with_state(|state| {
        let digest = sha256_hex(bytes);
        let outcome = if refused {
            TapeOutcome::Refusal {
                reason: "authority read refused".to_owned(),
            }
        } else {
            TapeOutcome::Digest {
                digest: digest.clone(),
            }
        };
        let _ = budget_failed;
        let key = "authority-read@core".to_owned();
        let identity = TapeIdentity {
            path: None,
            parent_dev: None,
            parent_ino: None,
            dev: None,
            ino: None,
            uuid: None,
            mount_point: None,
            device: None,
            key: key.clone(),
        };
        let (occurrence, counted) = occurrence(state, &key);
        let epoch = next_epoch(state);
        let row = TapeReadRow {
            kind: "authority-read@core".to_owned(),
            identity,
            outcome,
            cap,
            epoch,
            occurrence,
        };
        if counted {
            push_read(state, row);
        }
        state.pending_authority = (!refused).then(|| PendingAuthorityRead { digest, cap });
    });
}

/// Bind the pending authority read to its caller's source site and complete
/// object identity (absolute path, observed parent, own dev/ino). Called
/// immediately after the read statement at every authority-read call site.
pub(crate) fn authority_bind(site: &str, path: &Path, cap: u64) {
    with_state(|state| {
        let Some(pending) = state.pending_authority.take() else {
            state.notes.push(format!(
                "authority-bind at {site} found no pending read; the core row stays unbound"
            ));
            return;
        };
        let outcome = TapeOutcome::Digest {
            digest: pending.digest.clone(),
        };
        let identity = file_identity(path, &outcome);
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

/// Record a pool open outcome (the `Store::open_at`/`ledger_view` chain).
pub(crate) fn pool_row(kind: &str, pool: &Path, outcome: TapeOutcome) {
    read_row(kind, pool, outcome, 0);
}

/// Record one lock outcome (admission locks, pool locks).
pub(crate) fn lock_row(kind: &str, path: &Path, outcome: TapeOutcome) {
    read_row(kind, path, outcome, 0);
}

/// The authority budget's beginning: its deadline and input allowance are the
/// budget state every read charges against.
pub(crate) fn budget_begin(deadline_ms: u64, allowance_bytes: u64) {
    clock_row("authority-budget-deadline-ms", deadline_ms);
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

/// Test-only helpers for statements whose site holds a descriptor or path but
/// no recorded identity: one real observation of the live tree, used only to
/// bind a row's identity facts.
pub(crate) fn path_dev_ino(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(path)
        .ok()
        .map(|m| (m.dev(), m.ino()))
}

pub(crate) fn fd_dev_ino(fd: std::os::fd::BorrowedFd<'_>) -> Option<(u64, u64)> {
    // SAFETY: `fstat` has no preconditions beyond a live descriptor.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let status = unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) };
    if status != 0 {
        return None;
    }
    Some((u64::try_from(stat.st_dev).unwrap_or(u64::MAX), stat.st_ino))
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
    walk_fixture(root, root, &mut entries, &mut truncated);
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
) {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(_) => {
            *truncated = true;
            return;
        }
    };
    let mut names: Vec<(String, PathBuf)> = Vec::new();
    for entry in read_dir.flatten() {
        names.push((
            entry.file_name().to_string_lossy().into_owned(),
            entry.path(),
        ));
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
            std::fs::read(&path).ok().map(|bytes| sha256_hex(&bytes))
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
            walk_fixture(root, &path, entries, truncated);
        }
    }
}

/// The ground-truth effect set: the pre/post fixture capture diff. A created
/// entry is `create`, a removed entry is `unlink`, and a changed entry
/// (different dev/ino/size/bytes) is `save` at its path.
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
                kind: "create@fixture-diff".to_owned(),
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
                        kind: "save@fixture-diff".to_owned(),
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
                kind: "unlink@fixture-diff".to_owned(),
                path: entry.rel.clone(),
                epoch: 0,
            });
        }
    }
    effects.sort_by(|a, b| a.path.cmp(&b.path).then(a.kind.cmp(&b.kind)));
    effects
}
