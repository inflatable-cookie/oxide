//! Slot-label budget so a new slot's scratch path fits the Unix-socket
//! bound (oxide#376). The label is cosmetic — the slot key, identity and
//! ledger semantics never depend on it — so these tests assert byte budgets
//! on synthetic paths only, never pool state.

use super::*;
use std::os::unix::ffi::OsStrExt;

fn layout(folder: &str, profile: &str) -> SlotLayout {
    SlotLayout {
        folders: vec![folder.to_owned()],
        profile: profile.to_owned(),
    }
}

/// The full scratch path a label would produce in `slots_dir`, with a
/// representative 16-hex Barge id.
fn scratch_path(slots_dir: &Path, label: &str) -> PathBuf {
    slots_dir
        .join(format!("{}-{}", label, "f".repeat(16)))
        .join("scratch")
}

fn scratch_bytes(slots_dir: &Path, label: &str) -> usize {
    scratch_path(slots_dir, label).as_os_str().as_bytes().len()
}

#[test]
fn nested_pool_label_keeps_scratch_within_the_socket_bound() {
    // The relocated nested pool from the report.
    let slots = Path::new("/Volumes/Stash/builds/oxide/pool/slots");
    assert_eq!(slots.as_os_str().as_bytes().len(), 38);
    assert_eq!(max_barge_label_bytes(slots), 27);
    let label = barge_label(
        Some(&layout("oxide-canary-a00llrvf-dc1b3148", "quick")),
        max_barge_label_bytes(slots),
    );
    assert_eq!(label, "oxide-canary-a00llrvf-dc1b3");
    assert!(!label.ends_with('-'));
    assert!(scratch_bytes(slots, &label) <= MAX_SCRATCH_PATH_BYTES);
}

#[test]
fn short_pool_keeps_the_32_byte_label_cap() {
    let slots = Path::new("/p/slots");
    assert_eq!(max_barge_label_bytes(slots), 32);
    let label = barge_label(
        Some(&layout("averylongprojectfoldernamethatkeepsgoing", "quick")),
        max_barge_label_bytes(slots),
    );
    // No dashes, so the 32-byte truncation lands mid-word and the cap is
    // exact rather than shortened by trailing-dash stripping.
    assert_eq!(label.len(), 32);
    assert!(!label.ends_with('-'));
    assert!(scratch_bytes(slots, &label) <= MAX_SCRATCH_PATH_BYTES);
}

#[test]
fn tiny_budgets_fall_back_without_a_trailing_dash() {
    // A 64-byte slots dir leaves exactly one label byte; 63 and 62 leave
    // two and three.
    for (len, max) in [(64, 1), (63, 2), (62, 3)] {
        let slots = PathBuf::from("s".repeat(len));
        assert_eq!(max_barge_label_bytes(&slots), max);
        for layout in [
            None,
            Some(layout("", "slot")),
            // A label that sanitises to dashes only.
            Some(layout("---", "---")),
            Some(layout("oxide-canary-a00llrvf-dc1b3148", "quick")),
        ] {
            let label = barge_label(layout.as_ref(), max_barge_label_bytes(&slots));
            assert!(!label.is_empty(), "empty label for budget {max}");
            assert!(
                label.len() <= max,
                "{label:?} exceeds the {max}-byte budget"
            );
            assert!(!label.ends_with('-'), "{label:?} ends with '-'");
            assert!(scratch_bytes(&slots, &label) <= MAX_SCRATCH_PATH_BYTES);
        }
    }
}

#[test]
fn a_zero_budget_still_yields_a_valid_label() {
    // A slots dir that already exhausts the bound must not produce an
    // invalid label: the child-temporary decision keeps refusing the pool
    // path itself, and the label stays well-formed.
    let slots = PathBuf::from("s".repeat(200));
    assert_eq!(max_barge_label_bytes(&slots), 1);
    let label = barge_label(None, max_barge_label_bytes(&slots));
    assert_eq!(label, "s");
}

#[test]
fn existing_longer_labels_remain_valid_and_untouched() {
    // A slot created before the budget still splits as one Barge root: the
    // budget only shapes new labels, never renames or refuses old ones.
    let dir = format!("{}-{}", "l".repeat(32), "f".repeat(16));
    assert_eq!(split_slot_dir(&dir, 7), Some(("", dir.as_str())));
}
