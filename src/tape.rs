//! `#[cfg(test)]` observation tape for the library leaves (X-S1).
//!
//! Owning contract: `docs/knowledge/contracts/barge-adoption.md` (X-S1). This
//! module is compiled only under `cfg(test)`; a production build contains
//! none of it. The leaf functions carry the same signatures as the binary
//! tape (`src/bin/oxide/tape.rs`) so the instrumented statements in
//! `src/volume.rs`, `src/inventory.rs` and `src/lease/pool.rs` compile in
//! every crate that compiles those files with `cfg(test)` set: the library's
//! own unit-test compilation uses this module, and the binary harness crate
//! re-roots the binary tape under `crate::tape` with the same signatures.
//! The re-rooted harness crate uses its own shared binary tape for both
//! module sets; this sink remains for the library's unit-test compilation.
//!
//! Like the binary tape this is an observation tape: records are buffered
//! under one bounded thread-local sink and fed nowhere. Epochs are
//! engine-local, monotonic, assigned at the leaf; occurrence numbers are
//! counted per identity key under the fixed 64 cap; the buffer carries the
//! fixed 8192-row cap.

use std::cell::RefCell;
use std::path::Path;

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) const MAX_ROWS: usize = 8192;
pub(crate) const MAX_OCCURRENCES_PER_KEY: usize = 64;

#[derive(Clone, Debug)]
pub(crate) enum TapeOutcome {
    Digest { digest: String },
    Opened { dev: u64, ino: u64 },
    Absent,
    Refusal { reason: String },
    Facts { facts: String },
}

#[derive(Clone, Debug)]
pub(crate) struct TapeRecord {
    pub(crate) kind: String,
    pub(crate) identity: String,
    pub(crate) outcome: String,
    pub(crate) cap: u64,
    pub(crate) epoch: u64,
    pub(crate) occurrence: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct TapeEffect {
    pub(crate) kind: String,
    pub(crate) path: String,
    pub(crate) epoch: u64,
}

#[derive(Default)]
struct TapeState {
    epoch: u64,
    records: Vec<TapeRecord>,
    effects: Vec<TapeEffect>,
    occurrences: std::collections::HashMap<String, u64>,
    rows_truncated: bool,
    occurrences_truncated: bool,
}

thread_local! {
    static TAPE: RefCell<TapeState> = RefCell::new(TapeState::default());
}

fn with_state<R>(work: impl FnOnce(&mut TapeState) -> R) -> R {
    TAPE.with(|cell| work(&mut cell.borrow_mut()))
}

fn next_epoch(state: &mut TapeState) -> u64 {
    state.epoch = state.epoch.saturating_add(1);
    state.epoch
}

fn occurrence(state: &mut TapeState, key: &str) -> (u64, bool) {
    let next = state.occurrences.entry(key.to_owned()).or_insert(0);
    *next = next.saturating_add(1);
    if *next > MAX_OCCURRENCES_PER_KEY as u64 {
        state.occurrences_truncated = true;
        (*next, false)
    } else {
        (*next, true)
    }
}

fn outcome_label(outcome: &TapeOutcome) -> String {
    match outcome {
        TapeOutcome::Digest { digest } => format!("digest:{digest}"),
        TapeOutcome::Opened { dev, ino } => format!("opened:{dev}/{ino}"),
        TapeOutcome::Absent => "absent".to_owned(),
        TapeOutcome::Refusal { reason } => format!("refusal:{reason}"),
        TapeOutcome::Facts { facts } => format!("facts:{facts}"),
    }
}

fn push_record(
    state: &mut TapeState,
    kind: &str,
    identity: String,
    outcome: &TapeOutcome,
    cap: u64,
) {
    if state.records.len() >= MAX_ROWS {
        state.rows_truncated = true;
        return;
    }
    let (occurrence, counted) = occurrence(state, &format!("{kind}|{identity}"));
    if !counted {
        return;
    }
    let epoch = next_epoch(state);
    let outcome = outcome_label(outcome);
    state.records.push(TapeRecord {
        kind: kind.to_owned(),
        identity,
        outcome,
        cap,
        epoch,
        occurrence,
    });
}

/// Record one read row for an object at `path`, identity bound from the live
/// tree (own and parent device/inode).
pub(crate) fn read_row(kind: &str, path: &Path, outcome: TapeOutcome, cap: u64) {
    with_state(|state| {
        let identity = format!("file:{}|identity=unproven", path.display());
        push_record(state, kind, identity, &outcome, cap);
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
        let identity = format!("file:{}|parent=unproven|dev={dev}/ino={ino}", path.display());
        push_record(state, kind, identity, &outcome, cap);
    });
}

/// Record one component read of a no-follow walk.
pub(crate) fn component_row(
    kind: &str,
    parent_dev: u64,
    parent_ino: u64,
    name: &std::ffi::OsStr,
    outcome: TapeOutcome,
) {
    with_state(|state| {
        let identity = format!("component:parent={parent_dev}/{parent_ino}|name={name:?}");
        push_record(state, kind, identity, &outcome, 0);
    });
}

/// Record one volume observation.
pub(crate) fn volume_row(
    kind: &str,
    mount: &Path,
    uuid: Option<&str>,
    device: Option<&str>,
    outcome: TapeOutcome,
) {
    with_state(|state| {
        let identity = format!(
            "volume:mount={mount}|uuid={uuid:?}|device={device:?}",
            mount = mount.display()
        );
        push_record(state, kind, identity, &outcome, 0);
    });
}

/// Record one lock outcome.
pub(crate) fn lock_row(kind: &str, path: &Path, outcome: TapeOutcome) {
    read_row(kind, path, outcome, 0);
}

/// Record one pool open outcome.
pub(crate) fn pool_row(kind: &str, pool: &Path, outcome: TapeOutcome) {
    read_row(kind, pool, outcome, 0);
}

/// Record one classification observation.
pub(crate) fn classify_row(kind: &str, identity: &str, value: &str) {
    with_state(|state| {
        if state.records.len() >= MAX_ROWS {
            state.rows_truncated = true;
            return;
        }
        let (occurrence, counted) = occurrence(state, &format!("classify:{kind}|{identity}"));
        if !counted {
            return;
        }
        let epoch = next_epoch(state);
        state.records.push(TapeRecord {
            kind: format!("classify:{kind}"),
            identity: identity.to_owned(),
            outcome: value.to_owned(),
            cap: 0,
            epoch,
            occurrence,
        });
    });
}

/// Record one actual effect (positively completed write only).
pub(crate) fn effect_row(kind: &str, path: &Path) {
    with_state(|state| {
        let epoch = next_epoch(state);
        state.effects.push(TapeEffect {
            kind: kind.to_owned(),
            path: path.display().to_string(),
            epoch,
        });
    });
}

/// The buffered records of this thread's window, then cleared. Library unit
/// tests use this to assert recording happened without feeding anything back.
#[cfg(test)]
pub(crate) fn take_records() -> (Vec<TapeRecord>, Vec<TapeEffect>, bool, bool) {
    with_state(|state| {
        let records = std::mem::take(&mut state.records);
        let effects = std::mem::take(&mut state.effects);
        (
            records,
            effects,
            state.rows_truncated,
            state.occurrences_truncated,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The library leaves' recorder statements fire in this crate's own
    /// `cfg(test)` compilation: one `Binding::open_in` over a plain fixture
    /// directory records its rows (the binding itself refuses — a plain
    /// directory is not a mounted volume — and the refusal is the actual
    /// result the tape observed), and `take_records` returns them without
    /// feeding anything back.
    #[test]
    fn volume_leaf_recorders_fire_in_the_library_test_compilation() {
        let base = std::env::temp_dir().join(format!(
            "oxide-tape-{}-{}",
            std::process::id(),
            std::process::id()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let mount = base.join("mount");
        std::fs::create_dir_all(&mount).unwrap();
        std::fs::create_dir_all(base.join("other")).unwrap();
        let outcome = crate::volume::Binding::open_in(&base, "mount", None);
        let (records, _effects, rows_truncated, occurrences_truncated) = take_records();
        std::fs::remove_dir_all(&base).unwrap();
        // The binding refused: a plain directory is not a mounted volume.
        assert!(outcome.is_err());
        assert!(
            records
                .iter()
                .any(|record| record.kind == "volume:open-parent"),
            "the parent-open row is recorded: {records:?}"
        );
        assert!(
            records
                .iter()
                .any(|record| record.kind == "volume:open-mount"),
            "the mount-open row is recorded: {records:?}"
        );
        assert!(
            records.iter().any(|record| record.kind == "volume:observe"),
            "the observe facts row is recorded: {records:?}"
        );
        assert!(!rows_truncated);
        assert!(!occurrences_truncated);
        let _ = PathBuf::new();
    }
}
