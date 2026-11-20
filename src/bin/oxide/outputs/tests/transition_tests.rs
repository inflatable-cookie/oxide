use super::*;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

#[test]
fn an_empty_root_gets_a_marker_and_a_nonempty_foreign_root_is_left_alone() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    assert!(spec.root().join(MARKER).is_file());
    let marker = read_marker(spec.root()).unwrap();
    assert_eq!(marker.kind, RootKind::Local);
    assert_eq!(marker.generation, 1);
    // Reusing the root keeps the marker and its creation generation.
    assert_eq!(ensure_root(&context, &spec, 4).unwrap().generation, 1);

    let other = Fixture::new();
    let foreign = other.local();
    fs::create_dir_all(foreign.root()).unwrap();
    fs::write(foreign.root().join("keep"), b"content").unwrap();
    let refused = ensure_root(&other.context(), &foreign, 1).unwrap_err();
    assert!(refused.contains("holds content"), "{refused}");
    assert_eq!(fs::read(foreign.root().join("keep")).unwrap(), b"content");
}

#[test]
fn a_marker_for_another_checkout_is_not_owned() {
    let fixture = Fixture::new();
    let context = fixture.context();
    ensure_root(&context, &fixture.local(), 1).unwrap();
    let other = Fixture::new();
    let foreign = other.local();
    fs::create_dir_all(foreign.root()).unwrap();
    fs::write(
        foreign.root().join(MARKER),
        serde_json::to_vec(&Marker {
            version: MARKER_VERSION.to_owned(),
            checkout: fixture.checkout.clone(),
            checkout_id: context.checkout_id.clone(),
            label: "x".to_owned(),
            recipe: RECIPE.to_owned(),
            kind: RootKind::Local,
            volume_uuid: None,
            nonce: Some("11111111111111111111111111111111".to_owned()),
            checkout_volume_uuid: None,
            generation: 1,
            created_ms: 1,
        })
        .unwrap(),
    )
    .unwrap();
    let refused = ensure_root(&other.context(), &foreign, 1).unwrap_err();
    assert!(refused.contains("not this checkout"), "{refused}");
}

#[test]
fn a_transition_publishes_an_owned_link_and_keeps_the_root() {
    let fixture = Fixture::new();
    let receipt = transition(
        &fixture.context(),
        &fixture.receipt(),
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    let link = fixture.checkout.join("target");
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&link).unwrap(), fixture.local().root());
    assert_eq!(
        fixture.context().target().unwrap(),
        TargetState::OwnedLink(fixture.local().root().to_path_buf())
    );
    assert_eq!(receipt.state, ReceiptState::Committed);
    assert_eq!(receipt.generation, 1);
    assert_eq!(receipt.roots.len(), 1);
    // A second transition to the same place is a new generation, not a
    // new root.
    let second = transition(
        &fixture.context(),
        &receipt,
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    assert_eq!(second.generation, 2);
    assert_eq!(second.roots.len(), 1);
    assert!(!sibling_link(&link, 2).exists());
    assert!(fixture.local().root().join(MARKER).is_file());
}

#[test]
fn a_nonempty_real_target_and_a_foreign_link_are_never_replaced() {
    let fixture = Fixture::new();
    let target = fixture.checkout.join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("keep"), b"content").unwrap();
    let refused = transition(
        &fixture.context(),
        &fixture.receipt(),
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap_err();
    assert!(refused.contains("never moves"), "{refused}");
    assert_eq!(fs::read(target.join("keep")).unwrap(), b"content");
    assert!(fs::symlink_metadata(&target).unwrap().is_dir());
    assert!(!fixture.local().root().exists());

    let linked = Fixture::new();
    let outside = linked.root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("sentinel"), b"keep").unwrap();
    symlink(&outside, linked.checkout.join("target")).unwrap();
    let refused = transition(
        &linked.context(),
        &linked.receipt(),
        &linked.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap_err();
    assert!(refused.contains("never moves"), "{refused}");
    assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"keep");
    assert!(!linked.local().root().exists());
    assert_eq!(
        fs::read_link(linked.checkout.join("target")).unwrap(),
        outside
    );
}

#[test]
fn an_empty_real_target_is_converted_explicitly() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.checkout.join("target")).unwrap();
    let receipt = transition(
        &fixture.context(),
        &fixture.receipt(),
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    assert_eq!(receipt.generation, 1);
    assert!(fs::symlink_metadata(fixture.checkout.join("target"))
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn a_prepared_transition_with_a_live_sibling_is_retried() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let mut receipt = fixture.receipt();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    let prepared = Prepared {
        generation: 1,
        to: spec.selected(1),
        link: context.link(),
    };
    receipt.state = ReceiptState::Prepared;
    receipt.link_mut().prepared = Some(prepared.clone());
    write_receipt(&fixture.control, &receipt).unwrap();
    symlink(&prepared.to.root, sibling_link(&prepared.link, 1)).unwrap();
    let replayed = replay(&context, receipt).unwrap();
    assert_eq!(replayed.state, ReceiptState::Committed);
    assert_eq!(replayed.generation, 1);
    // A prepared whole-target first conversion keeps its recorded layout
    // (oxide#160, S4b): replay never rewrites it into a profile receipt.
    assert_eq!(replayed.layout(), LayoutKind::WholeTarget);
    assert_eq!(fs::read_link(context.link()).unwrap(), prepared.to.root);
    assert!(!sibling_link(&prepared.link, 1).exists());
}

#[test]
fn a_prepared_transition_after_the_rename_commits() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let mut receipt = fixture.receipt();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    let prepared = Prepared {
        generation: 1,
        to: spec.selected(1),
        link: context.link(),
    };
    receipt.state = ReceiptState::Prepared;
    receipt.link_mut().prepared = Some(prepared.clone());
    write_receipt(&fixture.control, &receipt).unwrap();
    symlink(&prepared.to.root, &prepared.link).unwrap();
    let replayed = replay(&context, receipt).unwrap();
    assert_eq!(replayed.state, ReceiptState::Committed);
    assert_eq!(replayed.selected().unwrap().root, prepared.to.root);
}

#[test]
fn a_prepared_transition_with_nothing_published_rolls_back() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let mut receipt = fixture.receipt();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    receipt.state = ReceiptState::Prepared;
    receipt.link_mut().prepared = Some(Prepared {
        generation: 1,
        to: spec.selected(1),
        link: context.link(),
    });
    write_receipt(&fixture.control, &receipt).unwrap();
    let replayed = replay(&context, receipt).unwrap();
    assert_eq!(replayed.state, ReceiptState::Detached);
    assert_eq!(replayed.layout(), LayoutKind::WholeTarget);
    assert_eq!(replayed.generation, 0);
    assert_eq!(replayed.roots.len(), 1);
    assert!(!context.link().exists());
    assert!(spec.root().is_dir());
}

#[test]
fn a_contradictory_prepared_transition_stops_and_keeps_both_roots() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let mut receipt = fixture.receipt();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    receipt.state = ReceiptState::Prepared;
    receipt.link_mut().prepared = Some(Prepared {
        generation: 1,
        to: spec.selected(1),
        link: context.link(),
    });
    write_receipt(&fixture.control, &receipt).unwrap();
    let outside = fixture.root.join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, context.link()).unwrap();
    let refused = replay(&context, receipt).unwrap_err();
    assert!(refused.contains("cannot be replayed"), "{refused}");
    assert!(fs::symlink_metadata(context.link())
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(spec.root().is_dir());
    let loaded = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.state, ReceiptState::Prepared);
}

#[test]
fn the_slot_destination_is_empty_until_a_checkout_is_converted() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let empty = crate::layout::Declaration::empty();
    let planned =
        planned_destination(Some(&fixture.control), &fixture.checkout, &empty, None).unwrap();
    assert!(planned.is_none());
    assert_eq!(destination_facts(None), "");
    let receipt = transition(
        &context,
        &fixture.receipt(),
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    let planned = planned_destination(Some(&fixture.control), &fixture.checkout, &empty, None)
        .unwrap()
        .unwrap();
    let selected = planned.selected().unwrap();
    assert_eq!(selected.kind, RootKind::Local);
    assert_eq!(selected.root, fixture.local().root());
    let first = destination_facts(Some(&planned));
    assert!(first.starts_with("outputs-2 "), "{first}");
    assert!(first.ends_with("g1"), "{first}");
    // Relinking to the same root keeps the same destination identity, so
    // a warm slot survives a relink.
    let again = transition(
        &context,
        &receipt,
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    assert_eq!(again.generation, 2);
    let planned = planned_destination(Some(&fixture.control), &fixture.checkout, &empty, None)
        .unwrap()
        .unwrap();
    assert_eq!(destination_facts(Some(&planned)), first);
}

/// A crash during the first conversion leaves a prepared receipt with no
/// committed selection. A link that appeared afterwards is not this
/// receipt's: replay never renames over it, even when it dangles and a
/// matching prepared sibling exists.
/// The complete reused-root check: marker identity, kind, volume and the
/// generation the slot was keyed with. A root that does not exist (or is
/// empty and unmarked) may be created; anything contradictory refuses.
#[test]
fn the_reused_root_check_refuses_contradictions_and_allows_creation() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    assert!(matches!(
        check_root(&context, 1, &spec.selected(1)),
        RootCheck::Missing
    ));
    fs::create_dir_all(spec.root()).unwrap();
    assert!(matches!(
        check_root(&context, 1, &spec.selected(1)),
        RootCheck::Missing
    ));
    fs::write(spec.root().join("content"), b"x").unwrap();
    assert!(matches!(
        check_root(&context, 1, &spec.selected(1)),
        RootCheck::Contradiction(_)
    ));
    fs::remove_file(spec.root().join("content")).unwrap();
    ensure_root(&context, &spec, 1).unwrap();
    assert!(matches!(
        check_root(&context, 1, &spec.selected(1)),
        RootCheck::Valid
    ));
    // A slot keyed for another generation, kind or volume refuses.
    assert!(matches!(
        check_root(&context, 1, &spec.selected(2)),
        RootCheck::Contradiction(_)
    ));
    let mut external = spec.selected(1);
    external.kind = RootKind::External;
    assert!(matches!(
        check_root(&context, 1, &external),
        RootCheck::Contradiction(_)
    ));
    let mut other = spec.selected(1);
    other.volume_uuid = Some("00000000-0000-0000-0000-000000000000".to_owned());
    assert!(matches!(
        check_root(&context, 1, &other),
        RootCheck::Contradiction(_)
    ));
    // A marker ahead of the receipt refuses even at the keyed generation.
    assert!(matches!(
        check_root(&context, 0, &spec.selected(1)),
        RootCheck::Contradiction(_)
    ));
}

#[test]
fn a_first_conversion_never_renames_over_a_link_that_appeared_after_the_crash() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    let mut receipt = fixture.receipt();
    let prepared = Prepared {
        generation: 1,
        to: spec.selected(1),
        link: context.link(),
    };
    receipt.state = ReceiptState::Prepared;
    receipt.link_mut().prepared = Some(prepared.clone());
    write_receipt(&fixture.control, &receipt).unwrap();
    symlink(&prepared.to.root, sibling_link(&prepared.link, 1)).unwrap();
    let foreign = fixture.root.join("foreign-absent");
    symlink(&foreign, context.link()).unwrap();

    let refused = replay(&context, receipt).unwrap_err();
    assert!(refused.contains("cannot be replayed"), "{refused}");
    assert_eq!(fs::read_link(context.link()).unwrap(), foreign);
    assert!(
        os_symlink_exists(&sibling_link(&prepared.link, 1)),
        "the prepared sibling was removed"
    );
    assert!(spec.root().is_dir());
}

#[test]
fn a_prepared_replacement_of_a_committed_root_rolls_back_only_over_its_old_link() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    let first = transition(
        &context,
        &fixture.receipt(),
        &spec,
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    let old = first.selected().unwrap().root.clone();
    let mut receipt = first.clone();
    let prepared = Prepared {
        generation: first.generation + 1,
        to: spec.selected(first.generation + 1),
        link: context.link(),
    };
    receipt.state = ReceiptState::Prepared;
    receipt.link_mut().prepared = Some(prepared);
    write_receipt(&fixture.control, &receipt).unwrap();
    // The old link is still live: roll back to it.
    let replayed = replay(&context, receipt.clone()).unwrap();
    assert_eq!(replayed.state, ReceiptState::Committed);
    assert_eq!(replayed.selected().unwrap().root, old);
    // A foreign dangling link in its place is not the old generation.
    fs::remove_file(context.link()).unwrap();
    let foreign = fixture.root.join("elsewhere");
    symlink(&foreign, context.link()).unwrap();
    let refused = replay(&context, receipt).unwrap_err();
    assert!(refused.contains("cannot be replayed"), "{refused}");
    assert_eq!(fs::read_link(context.link()).unwrap(), foreign);
}

/// With a committed selection, a dangling link that is not that selection
/// is foreign for a transition too.
#[test]
fn a_committed_receipt_refuses_a_dangling_link_that_is_not_its_own() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let receipt = transition(
        &context,
        &fixture.receipt(),
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    let foreign = fixture.root.join("foreign-absent");
    fs::remove_file(context.link()).unwrap();
    symlink(&foreign, context.link()).unwrap();
    let refused = transition(
        &context,
        &receipt,
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap_err();
    assert!(refused.contains("never moves"), "{refused}");
    assert_eq!(fs::read_link(context.link()).unwrap(), foreign);
}

#[test]
fn a_dangling_link_names_its_root_but_is_not_owned() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let absent = fixture.root.join("absent-root");
    symlink(&absent, context.link()).unwrap();
    let state = context.target().unwrap();
    assert_eq!(state, TargetState::Dangling(absent.clone()));
    assert_eq!(state.link_root(), Some(absent.as_path()));
    assert_eq!(state.root(), None);
    assert!(state.label().contains("not available"), "{}", state.label());
}

#[test]
fn a_transition_away_from_the_receipts_dangling_link_is_allowed() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let mut receipt = fixture.receipt();
    let absent = fixture.root.join("absent-root");
    let previous = Selected {
        kind: RootKind::Local,
        root: absent.clone(),
        volume_uuid: None,
        nonce: None,
        generation: 1,
    };
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt.link_mut().selected = Some(previous.clone());
    receipt.roots = vec![previous];
    write_receipt(&fixture.control, &receipt).unwrap();
    symlink(&absent, context.link()).unwrap();
    let committed = transition(
        &context,
        &receipt,
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    assert_eq!(committed.generation, 2);
    assert_eq!(
        fs::read_link(context.link()).unwrap(),
        fixture.local().root()
    );
    assert!(!absent.exists(), "the absent root was created");
}

#[test]
fn a_dangling_link_without_the_receipts_selection_is_never_replaced() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let absent = fixture.root.join("absent-root");
    symlink(&absent, context.link()).unwrap();
    let refused = transition(
        &context,
        &fixture.receipt(),
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap_err();
    assert!(refused.contains("never moves"), "{refused}");
    assert_eq!(fs::read_link(context.link()).unwrap(), absent);
}

#[test]
fn a_root_with_a_wrong_volume_or_kind_is_not_owned() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    let mut selected = spec.selected(1);
    selected.volume_uuid = Some("00000000-0000-0000-0000-000000000000".to_owned());
    let refused =
        validate_selected(&fixture.checkout, &context.checkout_id, &selected, None, 1).unwrap_err();
    assert!(refused.contains("another root kind or volume"), "{refused}");
    let mut external = spec.selected(1);
    external.kind = RootKind::External;
    let refused =
        validate_selected(&fixture.checkout, &context.checkout_id, &external, None, 1).unwrap_err();
    let text = refused.to_string();
    assert!(
        text.contains("another root kind or volume") || text.contains("not usable"),
        "{text}"
    );
}

#[test]
fn a_marker_generation_ahead_of_the_receipt_refuses() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 3).unwrap();
    let refused = validate_selected(
        &fixture.checkout,
        &context.checkout_id,
        &spec.selected(3),
        None,
        2,
    )
    .unwrap_err();
    assert!(refused.contains("ahead of the receipt"), "{refused}");
}

/// Replay validates the prepared root in full before it renames: a marker
/// ahead of the prepared generation, or one for another volume, stops
/// with the target, the sibling, the root and the prepared receipt kept.
#[test]
fn replay_refuses_a_contradictory_prepared_root_before_renaming() {
    for contradiction in ["ahead", "volume"] {
        let fixture = Fixture::new();
        let context = fixture.context();
        let spec = fixture.local();
        let mut to = spec.selected(1);
        match contradiction {
            "ahead" => {
                ensure_root(&context, &spec, 4).unwrap();
            }
            _ => {
                ensure_root(&context, &spec, 1).unwrap();
                to.volume_uuid = Some("00000000-0000-0000-0000-000000000000".to_owned());
            }
        }
        let prepared = Prepared {
            generation: 1,
            to,
            link: context.link(),
        };
        let mut receipt = fixture.receipt();
        receipt.state = ReceiptState::Prepared;
        receipt.link_mut().prepared = Some(prepared.clone());
        write_receipt_document(&fixture, &serde_json::to_value(&receipt).unwrap());
        let sibling = sibling_link(&prepared.link, 1);
        symlink(&prepared.to.root, &sibling).unwrap();
        let refused = replay(&context, receipt.clone()).unwrap_err();
        assert!(
            refused.contains("cannot be replayed"),
            "{contradiction}: {refused}"
        );
        assert!(!os_symlink_exists(&context.link()), "{contradiction}");
        assert_eq!(fs::read_link(&sibling).unwrap(), prepared.to.root);
        assert!(prepared.to.root.join(MARKER).is_file());
        let kept = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        assert_eq!(kept, receipt, "{contradiction}");
    }
}

/// A symlinked `.oxide-outputs` on the same device is not the real local
/// root: reuse, establishment and clean's device check refuse, and the
/// tree it points at is left exactly as it was.
#[test]
fn a_symlinked_local_ancestor_is_refused_without_mutation() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    let receipt = transition(
        &context,
        &fixture.receipt(),
        &spec,
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    let selected = receipt.selected().unwrap().clone();
    let outer = fixture.checkout.join(LOCAL_DIR);
    let moved = fixture.root.join("moved-outputs");
    fs::rename(&outer, &moved).unwrap();
    symlink(&moved, &outer).unwrap();
    let listing = |path: &Path| {
        let mut names: Vec<_> = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let before = listing(&moved.join(LOCAL_ROOT));

    match check_root(&context, receipt.generation, &selected) {
        RootCheck::Contradiction(reason) => {
            assert!(reason.contains("not a real directory"), "{reason}")
        }
        _ => panic!("a symlinked local ancestor must contradict"),
    }
    let refused = validate_root_device(&fixture.checkout, &selected, None).unwrap_err();
    assert!(refused.contains("not a real directory"), "{refused}");
    assert!(ensure_root(&context, &spec, receipt.generation).is_err());
    assert_eq!(listing(&moved.join(LOCAL_ROOT)), before);
    assert_eq!(fs::read_link(&outer).unwrap(), moved);

    // Establishing a new root through a symlinked ancestor writes nothing.
    let fresh = Fixture::new();
    let context = fresh.context();
    let elsewhere = fresh.root.join("elsewhere");
    fs::create_dir_all(elsewhere.join(LOCAL_ROOT)).unwrap();
    symlink(&elsewhere, fresh.checkout.join(LOCAL_DIR)).unwrap();
    assert!(ensure_root(&context, &fresh.local(), 1).is_err());
    assert!(listing(&elsewhere.join(LOCAL_ROOT)).is_empty());
}

#[test]
fn ephemeral_mode_plans_the_committed_root() {
    let fixture = Fixture::new();
    register(&fixture, DriveMode::Ephemeral);
    let (_, external) = committed_external(&fixture);
    let planned = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &crate::layout::Declaration::empty(),
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(planned.selected().unwrap().kind, RootKind::External);
    assert_eq!(planned.selected().unwrap().root, external);
    // With the intermediate slot on the internal pool, the external root
    // would be written unsupervised: ephemeral binding refuses.
    let context = fixture
        .context()
        .with_pool_fallback(Some("container mismatch"));
    let receipt = context.receipt().unwrap().expect("the committed receipt");
    let refused = match bind_ephemeral(&context, receipt, Some(&planned)) {
        Ok(_) => panic!("an unsupervised external root was bound"),
        Err(reason) => reason,
    };
    assert!(refused.contains("would not be supervised"), "{refused}");
}

/// A detached checkout stays detached under permanent mode (oxide#124):
/// its receipt plans no owned root, so a command is keyed unconverted and
/// the auto-adoption rule (any receipt blocks it) keeps it that way.
#[test]
fn a_detached_receipt_plans_no_root_in_permanent_mode() {
    let fixture = Fixture::new();
    register(&fixture, DriveMode::Permanent);
    let (mut receipt, _) = committed_external(&fixture);
    receipt.state = ReceiptState::Detached;
    receipt.link_mut().selected = None;
    write_receipt(&fixture.control, &receipt).unwrap();
    let planned = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &crate::layout::Declaration::empty(),
        None,
    )
    .unwrap();
    assert!(planned.is_none(), "{planned:?}");
    let mut facts = all_facts();
    facts.receipt_absent = false;
    assert!(!auto_adopts(&facts));
}

#[test]
fn a_prepared_link_outside_this_checkout_is_never_renamed() {
    let fixture = Fixture::new();
    let other = Fixture::new();
    let context = fixture.context();
    let committed = transition(
        &context,
        &fixture.receipt(),
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    let mut receipt = committed.clone();
    let foreign_link = other.checkout.join("target");
    receipt.state = ReceiptState::Prepared;
    receipt.link_mut().prepared = Some(Prepared {
        generation: committed.generation + 1,
        to: committed.selected().unwrap().clone(),
        link: foreign_link.clone(),
    });
    assert!(receipt
        .validate(&fixture.checkout, &context.checkout_id)
        .is_err());
    let elsewhere = other.root.join("elsewhere");
    symlink(&elsewhere, &foreign_link).unwrap();
    let sibling = sibling_link(&foreign_link, committed.generation + 1);
    symlink(&committed.selected().unwrap().root, &sibling).unwrap();
    let refused = replay(&context, receipt).unwrap_err();
    assert!(refused.contains("not this checkout's target"), "{refused}");
    assert_eq!(fs::read_link(&foreign_link).unwrap(), elsewhere);
    assert_eq!(
        fs::read_link(&sibling).unwrap(),
        committed.selected().unwrap().root
    );
    assert_eq!(
        fs::read_link(context.link()).unwrap(),
        committed.selected().unwrap().root
    );
}

/// Automatic replay is gated on cross-pool custody: with a quarantined
/// writer in the other pool it refuses before committing (or renaming),
/// and the link, prepared receipt and root stay unchanged. Without the
/// gate this prepared state commits at once.
#[test]
fn automatic_replay_refuses_unknown_custody_in_the_other_pool() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let committed = transition(
        &context,
        &fixture.receipt(),
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    let root = committed.selected().unwrap().root.clone();
    let mut receipt = committed.clone();
    receipt.state = ReceiptState::Prepared;
    receipt.link_mut().prepared = Some(Prepared {
        generation: committed.generation + 1,
        to: committed.selected().unwrap().clone(),
        link: context.link(),
    });
    write_receipt(&fixture.control, &receipt).unwrap();
    let path = receipt_path(&fixture.control, &context.checkout_id);
    let receipt_bytes = fs::read(&path).unwrap();

    let managed = oxide_storage::lease::MANAGED_NAME;
    let removable = pool_store(&fixture.root.join("removable"));
    removable.quarantine_for_tests(&fixture.checkout);
    let internal = pool_store(&fixture.root.join("internal"))
        .with_sibling(&fixture.root.join("removable").join(managed), false);
    let refused = gated_replay(&context, &internal, 0, receipt.clone()).unwrap_err();
    assert!(refused.contains("never confirmed closed"), "{refused}");
    assert_eq!(fs::read_link(context.link()).unwrap(), root);
    assert_eq!(fs::read(&path).unwrap(), receipt_bytes);
    assert!(root.join(MARKER).is_file());

    // The control: with clear custody the same state commits.
    let clear = pool_store(&fixture.root.join("clear"));
    let replayed = gated_replay(&context, &clear, 0, receipt).unwrap().unwrap();
    assert_eq!(replayed.state, ReceiptState::Committed);
}

/// Rollback needs a positively absent sibling: a sibling selecting
/// another root, or a regular file in its place, stops with the receipt,
/// target, sibling and roots unchanged. A genuinely absent sibling still
/// rolls back.
#[test]
fn a_contradictory_sibling_with_the_old_link_live_is_never_rolled_back() {
    for occupant in ["other-link", "file", "absent"] {
        let fixture = Fixture::new();
        let context = fixture.context();
        let committed = transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        let old = committed.selected().unwrap().root.clone();
        let new_root = fixture.root.join("new-root");
        let mut receipt = committed.clone();
        receipt.state = ReceiptState::Prepared;
        receipt.link_mut().prepared = Some(Prepared {
            generation: committed.generation + 1,
            to: Selected {
                kind: RootKind::Local,
                root: new_root.clone(),
                volume_uuid: None,
                nonce: None,
                generation: committed.generation + 1,
            },
            link: context.link(),
        });
        write_receipt(&fixture.control, &receipt).unwrap();
        let path = receipt_path(&fixture.control, &context.checkout_id);
        let bytes = fs::read(&path).unwrap();
        let sibling = sibling_link(&context.link(), committed.generation + 1);
        let elsewhere = fixture.root.join("elsewhere");
        match occupant {
            "other-link" => symlink(&elsewhere, &sibling).unwrap(),
            "file" => fs::write(&sibling, b"occupied").unwrap(),
            _ => {}
        }
        let result = replay(&context, receipt);
        if occupant == "absent" {
            assert_eq!(result.unwrap().state, ReceiptState::Committed);
            continue;
        }
        let refused = result.unwrap_err();
        assert!(
            refused.contains("cannot be replayed"),
            "{occupant}: {refused}"
        );
        assert_eq!(fs::read(&path).unwrap(), bytes, "{occupant}");
        assert_eq!(fs::read_link(context.link()).unwrap(), old);
        match occupant {
            "other-link" => assert_eq!(fs::read_link(&sibling).unwrap(), elsewhere),
            _ => assert_eq!(fs::read(&sibling).unwrap(), b"occupied"),
        }
        assert!(old.join(MARKER).is_file());
    }
}

/// With the target already selecting the prepared root, the commit runs
/// only when the sibling is positively absent: a sibling selecting
/// another root, a regular file, or the prepared link itself (which a
/// rename would have consumed) stops with receipt bytes, target, sibling
/// and both roots unchanged. An absent sibling still commits.
#[test]
fn an_already_renamed_replay_commits_only_without_a_sibling() {
    for occupant in ["other-link", "file", "prepared-link", "absent"] {
        let fixture = Fixture::new();
        let context = fixture.context();
        let old_root = fixture.root.join("old-root");
        fs::create_dir(&old_root).unwrap();
        let spec = fixture.local();
        ensure_root(&context, &spec, 2).unwrap();
        let new_root = spec.root().to_path_buf();
        let mut receipt = fixture.receipt();
        receipt.state = ReceiptState::Prepared;
        receipt.generation = 1;
        receipt.link_mut().selected = Some(Selected {
            kind: RootKind::Local,
            root: old_root.clone(),
            volume_uuid: None,
            nonce: None,
            generation: 1,
        });
        receipt.link_mut().prepared = Some(Prepared {
            generation: 2,
            to: spec.selected(2),
            link: context.link(),
        });
        write_receipt_document(&fixture, &serde_json::to_value(&receipt).unwrap());
        symlink(&new_root, context.link()).unwrap();
        let path = receipt_path(&fixture.control, &context.checkout_id);
        let bytes = fs::read(&path).unwrap();
        let sibling = sibling_link(&context.link(), 2);
        let elsewhere = fixture.root.join("elsewhere");
        match occupant {
            "other-link" => symlink(&elsewhere, &sibling).unwrap(),
            "file" => fs::write(&sibling, b"occupied").unwrap(),
            "prepared-link" => symlink(&new_root, &sibling).unwrap(),
            _ => {}
        }
        let result = replay(&context, receipt);
        if occupant == "absent" {
            let committed = result.unwrap();
            assert_eq!(committed.state, ReceiptState::Committed);
            assert_eq!(committed.selected().unwrap().root, new_root);
            continue;
        }
        let refused = result.unwrap_err();
        assert!(
            refused.contains("cannot be replayed"),
            "{occupant}: {refused}"
        );
        assert_eq!(fs::read(&path).unwrap(), bytes, "{occupant}");
        assert_eq!(fs::read_link(context.link()).unwrap(), new_root);
        match occupant {
            "other-link" => assert_eq!(fs::read_link(&sibling).unwrap(), elsewhere),
            "prepared-link" => assert_eq!(fs::read_link(&sibling).unwrap(), new_root),
            _ => assert_eq!(fs::read(&sibling).unwrap(), b"occupied"),
        }
        assert!(old_root.is_dir() && new_root.join(MARKER).is_file());
    }
}

// ---- Task-local cleanup × interrupted transitions (oxide#236) ----
//
// A task-local build-output cleanup removes `<checkout>/target` — with it
// every owned member link and a prepared transition's sibling link. The
// receipt the crash left behind is replayed exactly as the design says:
// a positively absent sibling and old link mean nothing was published, so
// the preparation rolls back; a live sibling or a finished rename still
// commits; anything foreign or changed stops with everything kept.

/// A committed generation-1 profile receipt over a local root whose
/// marker carries generation 1, so every placement's nonce matches the
/// root's own.
fn committed_profile_receipt(fixture: &Fixture, root: &Path) -> Receipt {
    let nonce = read_marker(root).unwrap().nonce;
    let selected = fixture.local().selected(1).with_nonce(nonce);
    let mut receipt = Receipt::first_with(
        &fixture.checkout,
        &identity::digest(&fixture.checkout.to_string_lossy()),
        LayoutKind::Profile,
    );
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt.place_member("debug").selected = Some(selected.clone());
    receipt.place_member("release").selected = Some(selected.clone());
    receipt.roots = vec![selected];
    receipt
}

/// The committed profile checkout's live layout: a real `target`
/// directory holding owned links for both members into `root`.
fn publish_committed_profile_members(fixture: &Fixture, root: &Path) {
    fs::create_dir(fixture.checkout.join("target")).unwrap();
    for member in ["debug", "release"] {
        ensure_member_directory(root, member).unwrap();
        symlink(
            member_destination(root, member),
            link_path(&fixture.checkout, member),
        )
        .unwrap();
    }
}

/// A task-local cleanup removes a prepared profile member's sibling link
/// together with the checkout's whole target directory. The replay reads
/// that as "nothing was published" and rolls the preparation back: the
/// receipt returns to the committed generation with the placement's old
/// selection and no prepared transition, and nothing is created in the
/// checkout or the root. The next build then recreates the links (the
/// binding test below covers that end to end).
#[test]
fn a_task_local_target_cleanup_rolls_a_prepared_profile_member_back() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    let root = spec.root().to_path_buf();
    publish_committed_profile_members(&fixture, &root);
    let mut receipt = committed_profile_receipt(&fixture, &root);
    let nonce = read_marker(&root).unwrap().nonce;
    receipt.state = ReceiptState::Prepared;
    receipt.place_member("release").prepared = Some(Prepared {
        generation: 2,
        to: spec.selected(1).with_nonce(nonce),
        link: link_path(&fixture.checkout, "release"),
    });
    write_receipt(&fixture.control, &receipt).unwrap();
    symlink(
        member_destination(&root, "release"),
        sibling_link(&link_path(&fixture.checkout, "release"), 2),
    )
    .unwrap();
    // The cleanup: the whole target directory, its member links and the
    // prepared sibling are gone.
    fs::remove_dir_all(fixture.checkout.join("target")).unwrap();
    let path = receipt_path(&fixture.control, &context.checkout_id);
    let prepared_bytes = fs::read(&path).unwrap();

    let rolled = replay(&context, receipt).unwrap();
    assert_eq!(rolled.state, ReceiptState::Committed);
    assert_eq!(rolled.generation, 1);
    assert!(rolled.prepared_ref().is_none());
    let release = rolled.selected_for("release").unwrap();
    assert_eq!(release.root, root);
    assert_eq!(release.generation, 1);
    assert_eq!(rolled.selected_for("debug"), rolled.selected_for("release"));
    // Nothing was recreated in the checkout and the root is untouched.
    assert!(!fixture.checkout.join("target").exists());
    assert!(root.join(MARKER).is_file());
    let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    assert_ne!(fs::read(&path).unwrap(), prepared_bytes);
    assert_eq!(stored, rolled);
    assert!(stored.prepared_ref().is_none());
}

/// A cleanup after a crash that was adding a NEW member (its placement
/// has no committed selection) rolls back to the committed receipt with
/// that member left unselected: the interruption is forgotten, not
/// guessed, and the member keeps its real-directory semantics.
#[test]
fn a_prepared_new_member_rolls_back_after_task_local_cleanup() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    let root = spec.root().to_path_buf();
    publish_committed_profile_members(&fixture, &root);
    let member = "x86_64-apple-darwin/release";
    let mut receipt = committed_profile_receipt(&fixture, &root);
    let nonce = read_marker(&root).unwrap().nonce;
    receipt.place_member(member).selected = None;
    receipt.state = ReceiptState::Prepared;
    receipt.place_member(member).prepared = Some(Prepared {
        generation: 2,
        to: spec.selected(1).with_nonce(nonce),
        link: link_path(&fixture.checkout, member),
    });
    write_receipt(&fixture.control, &receipt).unwrap();
    fs::remove_dir_all(fixture.checkout.join("target")).unwrap();

    let rolled = replay(&context, receipt).unwrap();
    assert_eq!(rolled.state, ReceiptState::Committed);
    assert!(rolled.prepared_ref().is_none());
    assert!(rolled.selected_for(member).is_none());
    assert!(rolled.selected_for("debug").is_some());
    assert!(!fixture.checkout.join("target").exists());
    let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored, rolled);
}

/// A valid pending transition publishes even without any cleanup: a
/// prepared new member whose sibling link is live (the crash happened
/// between staging and the rename) republishes that sibling and commits,
/// exactly as the whole-target replay does.
#[test]
fn a_prepared_profile_member_with_a_live_sibling_republishes_and_commits() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    let root = spec.root().to_path_buf();
    publish_committed_profile_members(&fixture, &root);
    let member = "x86_64-apple-darwin/release";
    let link = link_path(&fixture.checkout, member);
    let mut receipt = committed_profile_receipt(&fixture, &root);
    let nonce = read_marker(&root).unwrap().nonce;
    receipt.place_member(member).selected = None;
    receipt.state = ReceiptState::Prepared;
    receipt.place_member(member).prepared = Some(Prepared {
        generation: 2,
        to: spec.selected(1).with_nonce(nonce),
        link: link.clone(),
    });
    write_receipt(&fixture.control, &receipt).unwrap();
    ensure_member_directory(&root, member).unwrap();
    let sibling = sibling_link(&link, 2);
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(member_destination(&root, member), &sibling).unwrap();

    let replayed = replay(&context, receipt).unwrap();
    assert_eq!(replayed.state, ReceiptState::Committed);
    assert_eq!(replayed.generation, 2);
    assert!(replayed.prepared_ref().is_none());
    // The placement keeps the root's own creation generation.
    assert_eq!(replayed.selected_for(member).unwrap().generation, 1);
    assert_eq!(
        fs::read_link(&link).unwrap(),
        member_destination(&root, member)
    );
    assert!(!sibling.exists());
}

/// A rename that finished before the crash commits on replay: the member
/// link already selects the prepared destination and the sibling was
/// consumed by the rename.
#[test]
fn a_renamed_prepared_profile_member_commits() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    let root = spec.root().to_path_buf();
    publish_committed_profile_members(&fixture, &root);
    let mut receipt = committed_profile_receipt(&fixture, &root);
    let nonce = read_marker(&root).unwrap().nonce;
    receipt.state = ReceiptState::Prepared;
    receipt.place_member("release").prepared = Some(Prepared {
        generation: 2,
        to: spec.selected(1).with_nonce(nonce),
        link: link_path(&fixture.checkout, "release"),
    });
    write_receipt(&fixture.control, &receipt).unwrap();
    let sibling = sibling_link(&link_path(&fixture.checkout, "release"), 2);
    fs::rename(link_path(&fixture.checkout, "release"), &sibling).unwrap();
    fs::rename(&sibling, link_path(&fixture.checkout, "release")).unwrap();

    let replayed = replay(&context, receipt).unwrap();
    assert_eq!(replayed.state, ReceiptState::Committed);
    assert_eq!(replayed.generation, 2);
    assert_eq!(
        fs::read_link(link_path(&fixture.checkout, "release")).unwrap(),
        member_destination(&root, "release")
    );
    assert!(!sibling.exists());
}

/// A cleanup (or any other hand) that leaves a foreign link where the
/// prepared member link lived is an unknown transition state: the replay
/// stops with the receipt bytes, the foreign entry, the sibling path and
/// both roots exactly as they are.
#[test]
fn a_cleanup_that_leaves_a_foreign_member_link_stops_the_replay_and_keeps_everything() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    let root = spec.root().to_path_buf();
    publish_committed_profile_members(&fixture, &root);
    let mut receipt = committed_profile_receipt(&fixture, &root);
    let nonce = read_marker(&root).unwrap().nonce;
    receipt.state = ReceiptState::Prepared;
    receipt.place_member("release").prepared = Some(Prepared {
        generation: 2,
        to: spec.selected(1).with_nonce(nonce),
        link: link_path(&fixture.checkout, "release"),
    });
    write_receipt(&fixture.control, &receipt).unwrap();
    fs::remove_dir_all(fixture.checkout.join("target")).unwrap();
    fs::create_dir(fixture.checkout.join("target")).unwrap();
    fs::create_dir(fixture.root.join("elsewhere")).unwrap();
    let foreign = link_path(&fixture.checkout, "release");
    symlink(fixture.root.join("elsewhere"), &foreign).unwrap();
    let path = receipt_path(&fixture.control, &context.checkout_id);
    let bytes = fs::read(&path).unwrap();

    let refused = replay(&context, receipt).unwrap_err();
    assert!(refused.contains("cannot be replayed"), "{refused}");
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(
        fs::read_link(&foreign).unwrap(),
        fixture.root.join("elsewhere")
    );
    assert!(root.join(MARKER).is_file());
    let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, ReceiptState::Prepared);
}

/// A record that may still be writing holds the replay: automatic replay
/// waits (the binding loop retries) instead of committing or rolling
/// back under a live writer, and the prepared receipt stays as it is.
#[test]
fn a_live_writer_waits_the_cleanup_replay_instead_of_replaying() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    let root = spec.root().to_path_buf();
    publish_committed_profile_members(&fixture, &root);
    let mut receipt = committed_profile_receipt(&fixture, &root);
    let nonce = read_marker(&root).unwrap().nonce;
    receipt.state = ReceiptState::Prepared;
    receipt.place_member("release").prepared = Some(Prepared {
        generation: 2,
        to: spec.selected(1).with_nonce(nonce),
        link: link_path(&fixture.checkout, "release"),
    });
    write_receipt(&fixture.control, &receipt).unwrap();
    fs::remove_dir_all(fixture.checkout.join("target")).unwrap();
    let path = receipt_path(&fixture.control, &context.checkout_id);
    let bytes = fs::read(&path).unwrap();

    let held = pool_store(&fixture.root.join("held"));
    let lock = crate::store::hold_record_for_tests(&held, &fixture.checkout, 1);
    let waited = gated_replay(&context, &held, 0, receipt).unwrap();
    assert!(waited.is_none(), "a live writer must wait, not replay");
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, ReceiptState::Prepared);
    drop(lock);
}

/// The papercut's own continuation, end to end at the binding: a
/// permanent-mode leased build whose slot was keyed for the committed
/// root meets a prepared transition and a task-local cleanup that removed
/// the whole target directory. The binding replays the transition (a
/// pure rollback, under clear custody), recreates the real target
/// directory (oxide#165), and republishes both member links into the
/// same verified root — the build proceeds with nothing adopted,
/// moved or deleted.
#[test]
fn a_permanent_build_replays_a_cleanup_interrupted_transition_and_republishes_the_members() {
    let fixture = Fixture::new();
    let store = pool_store(&fixture.root.join("pool"));
    let control = store.owner_control().to_path_buf();
    register_at(&fixture, &control, DriveMode::Permanent);
    let context = Context::new(Some(control.clone()), fixture.checkout.clone());
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    let root = spec.root().to_path_buf();
    publish_committed_profile_members(&fixture, &root);
    let mut receipt = committed_profile_receipt(&fixture, &root);
    let nonce = read_marker(&root).unwrap().nonce;
    receipt.state = ReceiptState::Prepared;
    receipt.place_member("release").prepared = Some(Prepared {
        generation: 2,
        to: spec.selected(1).with_nonce(nonce.clone()),
        link: link_path(&fixture.checkout, "release"),
    });
    write_receipt(&control, &receipt).unwrap();
    symlink(
        member_destination(&root, "release"),
        sibling_link(&link_path(&fixture.checkout, "release"), 2),
    )
    .unwrap();
    // The task-local cleanup between the crash and the next command.
    fs::remove_dir_all(fixture.checkout.join("target")).unwrap();

    let declaration = crate::layout::Declaration::new(
        ["debug", "release"]
            .iter()
            .map(|name| crate::layout::Member::parse(name).unwrap()),
    )
    .unwrap();
    let planned = planned_destination(Some(&control), &fixture.checkout, &declaration, None)
        .unwrap()
        .unwrap();
    assert!(!planned.whole_target);
    for planned_member in &planned.members {
        let selected = planned_member.selected.as_ref().unwrap();
        assert_eq!(selected.root, root);
        assert_eq!(selected.generation, 1);
        assert_eq!(selected.nonce(), nonce.as_deref());
    }

    let binding = bind_for_command(&store, &fixture.checkout, 0, Some(&planned), None, false)
        .unwrap_or_else(|reason| {
            panic!("the next permanent-mode build refused after the cleanup: {reason}")
        });
    assert_eq!(binding.outcome.root.as_deref(), Some(root.as_path()));
    // The replay rolled back first, then the two member transitions each
    // advanced the generation: 1 -> 2 -> 3.
    let stored = read_receipt(&control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, ReceiptState::Committed);
    assert_eq!(stored.generation, 3);
    assert!(stored.prepared_ref().is_none());
    // The live layout is exactly a converted checkout again.
    let target = context.target().unwrap();
    assert!(matches!(
        target,
        TargetState::RealEmpty | TargetState::RealNonEmpty
    ));
    for member in ["debug", "release"] {
        assert_eq!(
            fs::read_link(link_path(&fixture.checkout, member)).unwrap(),
            member_destination(&root, member)
        );
        assert!(root.join(member).is_dir());
    }
    // The root's immutable creation generation is untouched.
    assert_eq!(read_marker(&root).unwrap().generation, 1);
}

/// The papercut's remedy, end to end: after the cleanup an explicit
/// `oxide outputs adopt` replays the prepared transition, recreates the
/// real target directory and republishes both members, so the same
/// selector passes again.
#[test]
fn an_explicit_adopt_settles_a_prepared_transition_after_task_local_cleanup() {
    let fixture = Fixture::new();
    let store = Store::for_tests(&fixture.root.join("pool"));
    let control = store.owner_control().to_path_buf();
    register_at(&fixture, &control, DriveMode::Permanent);
    let context = Context::new(Some(control.clone()), fixture.checkout.clone());
    // The adoption's replay write needs the ready registry this pool has
    // never initialized.
    crate::prune::ensure_ready(&store, &fixture.checkout).unwrap();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    let root = spec.root().to_path_buf();
    publish_committed_profile_members(&fixture, &root);
    let mut receipt = committed_profile_receipt(&fixture, &root);
    let nonce = read_marker(&root).unwrap().nonce;
    receipt.state = ReceiptState::Prepared;
    receipt.place_member("release").prepared = Some(Prepared {
        generation: 2,
        to: spec.selected(1).with_nonce(nonce.clone()),
        link: link_path(&fixture.checkout, "release"),
    });
    write_receipt(&control, &receipt).unwrap();
    fs::remove_dir_all(fixture.checkout.join("target")).unwrap();

    let status = adopt_at(&context, || Ok(Some(store.handle_for_tests()))).unwrap();
    assert_eq!(status, 0);
    let stored = read_receipt(&control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, ReceiptState::Committed);
    assert_eq!(stored.generation, 3);
    assert!(stored.prepared_ref().is_none());
    assert!(fs::symlink_metadata(fixture.checkout.join("target"))
        .unwrap()
        .is_dir());
    for member in ["debug", "release"] {
        assert_eq!(
            fs::read_link(link_path(&fixture.checkout, member)).unwrap(),
            member_destination(&root, member)
        );
    }
    assert_eq!(read_marker(&root).unwrap().generation, 1);
}

/// In permanent mode a checkout with no receipt never hands Cargo a
/// link: a foreign link and a marked link whose receipt is missing both
/// refuse with nothing written through them, while a real or missing
/// target keeps the unconverted behaviour.

/// A member directory is created component by component beneath the
/// root's own descriptor: a symlinked intermediate is refused and nothing
/// is created outside the root, where `create_dir_all` used to walk
/// through it.
#[test]
fn a_member_directory_never_follows_a_symlinked_ancestor() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    let outside = fixture.root.join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, spec.root().join("aarch64-apple-darwin")).unwrap();
    let refused = ensure_member_directory(spec.root(), "aarch64-apple-darwin/debug")
        .expect_err("a symlinked member ancestor was followed");
    assert!(
        refused.contains("not a real directory"),
        "{refused}"
    );
    assert!(
        !outside.join("debug").exists(),
        "the create escaped the owned root"
    );
    // The ordinary nested create still works and stays inside the root.
    let created = ensure_member_directory(spec.root(), "x86_64-unknown-linux-gnu/debug")
        .expect("a member directory inside the root was refused");
    assert_eq!(
        created,
        spec.root().join("x86_64-unknown-linux-gnu").join("debug")
    );
    assert!(created.is_dir());
}

#[test]
fn a_committed_profile_member_replays_when_its_old_link_is_live() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let old_spec = fixture.local();
    ensure_root(&context, &old_spec, 1).unwrap();
    let old_root = old_spec.root().to_path_buf();
    publish_committed_profile_members(&fixture, &old_root);

    let new_root = fixture.checkout.join(".oxide-outputs/local-2");
    let new_spec = RootSpec::Local {
        root: new_root.clone(),
    };
    ensure_root(&context, &new_spec, 2).unwrap();
    ensure_member_directory(&new_root, "release").unwrap();
    let new_nonce = read_marker(&new_root).unwrap().nonce;
    let new_selected = new_spec.selected(2).with_nonce(new_nonce);

    // 1. Crash at "prepared" (no sibling yet) with the old profile link live:
    //    replay rolls back cleanly instead of miscomparing against bare root.
    let mut prepared_only = committed_profile_receipt(&fixture, &old_root);
    let release_link = link_path(&fixture.checkout, "release");
    prepared_only.state = ReceiptState::Prepared;
    prepared_only.place_member("release").prepared = Some(Prepared {
        generation: 2,
        to: new_selected.clone(),
        link: release_link.clone(),
    });
    write_receipt(&fixture.control, &prepared_only).unwrap();
    let rolled = replay(&context, prepared_only).unwrap();
    assert_eq!(rolled.state, ReceiptState::Committed);
    assert_eq!(rolled.generation, 1);
    assert_eq!(rolled.selected_for("release").unwrap().root, old_root);
    assert_eq!(
        fs::read_link(&release_link).unwrap(),
        member_destination(&old_root, "release")
    );

    // 2. Crash at "linked" (live sibling) with the old profile link live:
    //    replay completes the rename and commits the prepared root.
    let mut linked = committed_profile_receipt(&fixture, &old_root);
    linked.state = ReceiptState::Prepared;
    linked.place_member("release").prepared = Some(Prepared {
        generation: 2,
        to: new_selected,
        link: release_link.clone(),
    });
    write_receipt(&fixture.control, &linked).unwrap();
    let sibling = sibling_link(&release_link, 2);
    symlink(member_destination(&new_root, "release"), &sibling).unwrap();

    let committed = replay(&context, linked).unwrap();
    assert_eq!(committed.state, ReceiptState::Committed);
    assert_eq!(committed.generation, 2);
    assert!(committed.prepared_ref().is_none());
    assert_eq!(committed.selected_for("release").unwrap().root, new_root);
    assert_eq!(
        fs::read_link(&release_link).unwrap(),
        member_destination(&new_root, "release")
    );
    assert!(!sibling.exists());
}
