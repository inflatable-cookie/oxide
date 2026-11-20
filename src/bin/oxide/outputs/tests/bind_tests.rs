use super::*;
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt};
use std::path::Path;
use std::time::Duration;

#[test]
fn a_dangling_member_link_never_transitions_automatically() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    let external = Selected {
        kind: RootKind::External,
        root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
        volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
        nonce: Some("99999999999999999999999999999999".to_owned()),
        generation: 1,
    };
    // The external ROOT exists and carries this checkout's marker, but the
    // member directory it should contain does not: the member link dangles.
    fs::create_dir_all(&external.root).unwrap();
    let marker = Marker {
        version: MARKER_VERSION.to_owned(),
        checkout: fixture.checkout.clone(),
        checkout_id: context.checkout_id.clone(),
        label: "checkout".to_owned(),
        recipe: RECIPE.to_owned(),
        kind: RootKind::External,
        volume_uuid: external.volume_uuid.clone(),
        nonce: external.nonce.clone(),
        checkout_volume_uuid: None,
        generation: 1,
        created_ms: 1,
    };
    fs::write(
        external.root.join(MARKER),
        serde_json::to_vec_pretty(&marker).unwrap(),
    )
    .unwrap();
    fs::create_dir_all(fixture.checkout.join("target")).unwrap();
    let link = fixture.checkout.join("target/debug");
    let dangling = member_destination(&external.root, "debug");
    symlink(&dangling, &link).unwrap();
    let mut receipt =
        Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt.place_member("debug").selected = Some(external.clone());
    let plan = Plan {
        whole_target: false,
        members: vec![PlannedMember {
            member: "debug".to_owned(),
            selected: Some(external.clone()),
        }],
    };
    let store = pool_store(&fixture.root.join("pool"));
    // With an internal fallback slot the refusal names the fallback.
    let fallback = Context::new(Some(fixture.control.clone()), fixture.checkout.clone())
        .with_pool_fallback(Some("container mismatch"));
    let refused = match bind_profile(&fallback, &store, 0, &receipt, Some(&plan), false) {
        Bind::Refuse(reason) => reason.to_string(),
        Bind::Bound { .. } => panic!("a dangling member link was bound"),
        Bind::Wait => panic!("a dangling refusal waited"),
    };
    assert!(refused.contains("not available"), "{refused}");
    assert!(refused.contains("fell back"), "{refused}");
    assert!(refused.contains("oxide outputs adopt"), "{refused}");
    // Without a fallback it still refuses rather than auto-transitioning.
    let refused = match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
        Bind::Refuse(reason) => reason.to_string(),
        Bind::Bound { .. } => panic!("a dangling member link was bound without a fallback"),
        Bind::Wait => panic!("a dangling refusal waited"),
    };
    assert!(refused.contains("not available"), "{refused}");
    // The link and the receipt are untouched.
    assert_eq!(fs::read_link(&link).unwrap(), dangling);
    write_receipt(&fixture.control, &receipt).unwrap();
}

#[test]
fn a_profile_fallback_slot_refuses_a_live_external_member_before_cargo() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    let mut receipt =
        Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    let external = Selected {
        kind: RootKind::External,
        root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
        volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
        nonce: Some("99999999999999999999999999999999".to_owned()),
        generation: 1,
    };
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt.place_member("debug").selected = Some(external.clone());
    let plan = Plan {
        whole_target: false,
        members: vec![PlannedMember {
            member: "debug".to_owned(),
            selected: Some(external.clone()),
        }],
    };
    // A real external root carrying this checkout's marker, with its
    // member directory; the link is live and matches the keyed external
    // destination, but the command's slot fell back to the internal pool.
    fs::create_dir_all(member_destination(&external.root, "debug")).unwrap();
    let marker = Marker {
        version: MARKER_VERSION.to_owned(),
        checkout: fixture.checkout.clone(),
        checkout_id: context.checkout_id.clone(),
        label: "checkout".to_owned(),
        recipe: RECIPE.to_owned(),
        kind: RootKind::External,
        volume_uuid: external.volume_uuid.clone(),
        nonce: external.nonce.clone(),
        checkout_volume_uuid: None,
        generation: 1,
        created_ms: 1,
    };
    fs::write(
        external.root.join(MARKER),
        serde_json::to_vec_pretty(&marker).unwrap(),
    )
    .unwrap();
    fs::create_dir_all(fixture.checkout.join("target")).unwrap();
    let link = fixture.checkout.join("target/debug");
    symlink(member_destination(&external.root, "debug"), &link).unwrap();
    let context = context.with_pool_fallback(Some("container mismatch"));
    let store = pool_store(&fixture.root.join("pool"));
    match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
        Bind::Refuse(reason) => {
            let reason = reason.to_string();
            assert!(
                reason.contains("fell back to the internal pool"),
                "{reason}"
            )
        }
        Bind::Bound { .. } => {
            panic!("an external member link was bound on an internal fallback")
        }
        Bind::Wait => panic!("a fallback refusal waited"),
    }
}

/// P4: a still-mounted volume becoming unusable makes the fresh plan stop
/// selecting the external root, so the legacy fallback refusal above never
/// sees an external destination. Binding must still refuse: relinking the
/// live external member to the internal root would be an automatic
/// external -> local transition. No fallback slot is present here, so the
/// refusal cannot come from the legacy #117 rule, and nothing changes.
#[test]
fn a_local_plan_never_relinks_a_committed_external_member() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    let mut receipt =
        Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    let external = Selected {
        kind: RootKind::External,
        root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
        volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
        nonce: Some("99999999999999999999999999999999".to_owned()),
        generation: 1,
    };
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt.place_member("debug").selected = Some(external.clone());
    write_receipt(&fixture.control, &receipt).unwrap();
    let recorded = fs::read(receipt_path(&fixture.control, &context.checkout_id)).unwrap();
    // The mounted volume's root is now unusable, so the fresh plan places
    // the member on the internal fallback instead.
    let local = Selected {
        kind: RootKind::Local,
        root: fixture.checkout.join(".oxide-outputs/local"),
        volume_uuid: None,
        nonce: Some("11111111111111111111111111111111".to_owned()),
        generation: 1,
    };
    let plan = Plan {
        whole_target: false,
        members: vec![PlannedMember {
            member: "debug".to_owned(),
            selected: Some(local.clone()),
        }],
    };
    fs::create_dir_all(fixture.checkout.join("target")).unwrap();
    let link = fixture.checkout.join("target/debug");
    let destination = member_destination(&external.root, "debug");
    symlink(&destination, &link).unwrap();
    let store = pool_store(&fixture.root.join("pool"));
    match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
        Bind::Refuse(reason) => {
            let reason = reason.to_string();
            assert!(
                reason
                    .contains("never moves an owned external output to another pool automatically"),
                "{reason}"
            );
            assert!(reason.contains("nothing was built"), "{reason}");
            assert!(
                reason.contains(&destination.display().to_string()),
                "{reason}"
            );
            assert!(
                reason.contains(
                    &member_destination(&local.root, "debug")
                        .display()
                        .to_string()
                ),
                "{reason}"
            );
            assert!(reason.contains("oxide outputs adopt"), "{reason}");
        }
        Bind::Bound { .. } => panic!("an external member was relinked to the internal pool"),
        Bind::Wait => panic!("an external-to-local refusal waited"),
    }
    // Nothing changed: the link still names the external member and the
    // committed receipt bytes are exactly what was recorded.
    assert_eq!(fs::read_link(&link).unwrap(), destination);
    assert_eq!(
        fs::read(receipt_path(&fixture.control, &context.checkout_id)).unwrap(),
        recorded
    );
    assert!(!fixture.checkout.join(".oxide-outputs").exists());
    // When this command's own intermediate slot came from the internal
    // pool, the refusal names that reason too (the proof's absence oracle)
    // and still changes nothing.
    let fallback = fixture
        .context()
        .with_pool_fallback(Some("the volume is absent"));
    match bind_profile(&fallback, &store, 0, &receipt, Some(&plan), false) {
        Bind::Refuse(reason) => {
            let reason = reason.to_string();
            assert!(
                reason.contains("fell back to the internal pool (the volume is absent)"),
                "{reason}"
            );
            assert!(
                reason
                    .contains("never moves an owned external output to another pool automatically"),
                "{reason}"
            );
            assert!(reason.contains("nothing was built"), "{reason}");
        }
        Bind::Bound { .. } => {
            panic!("a fallback relinked an external member to the internal pool")
        }
        Bind::Wait => panic!("a fallback external-to-local refusal waited"),
    }
    assert_eq!(fs::read_link(&link).unwrap(), destination);
    assert_eq!(
        fs::read(receipt_path(&fixture.control, &context.checkout_id)).unwrap(),
        recorded
    );
}

/// oxide#133 case 04 product regression. A member that JOINS a root this
/// checkout already owns needs no fresh reservation: the commit of that
/// root's first member consumed it. Demanding one made every later member
/// of the same root -- a host `--target` triple after `debug`/`release` --
/// refuse with "that reservation was cancelled or replaced", so the parent
/// of this fix fails and this test distinguishes it from the correct
/// behaviour instead of accepting any nonzero exit.
#[test]
fn a_new_member_joins_an_existing_owned_root_without_a_fresh_reservation() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    let store = pool_store(&fixture.root.join("pool"));
    // Committed debug + release on one owned root, exactly as a first
    // conversion leaves them: no reservation survives that commit.
    let mut receipt = profile_receipt(&fixture);
    receipt.reserved = None;
    write_receipt(&fixture.control, &receipt).unwrap();
    let selected = receipt.selected().cloned().unwrap();
    let root = selected.root.clone();
    let marker = Marker {
        version: MARKER_VERSION.to_owned(),
        checkout: fixture.checkout.clone(),
        checkout_id: context.checkout_id.clone(),
        label: "checkout".to_owned(),
        recipe: RECIPE.to_owned(),
        kind: RootKind::Local,
        volume_uuid: None,
        nonce: selected.nonce.clone(),
        checkout_volume_uuid: None,
        generation: 1,
        created_ms: 1,
    };
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join(MARKER),
        serde_json::to_vec_pretty(&marker).unwrap(),
    )
    .unwrap();
    fs::create_dir_all(fixture.checkout.join("target")).unwrap();
    for member in ["debug", "release"] {
        fs::create_dir_all(member_destination(&root, member)).unwrap();
        symlink(
            member_destination(&root, member),
            fixture.checkout.join("target").join(member),
        )
        .unwrap();
    }
    // The production planner keys the new triple member on the SAME owned
    // root, its existing nonce and the root's own generation.
    let triple_member = "aarch64-apple-darwin/debug";
    let declaration =
        crate::layout::Declaration::new([crate::layout::Member::parse(triple_member).unwrap()])
            .unwrap();
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    let wanted = plan.selected().cloned().unwrap();
    assert_eq!(
        wanted.root, root,
        "the planner did not reuse the owned root"
    );
    assert_eq!(
        wanted.nonce(),
        selected.nonce(),
        "the planner did not reuse the root nonce"
    );
    assert_eq!(wanted.generation, 1, "the planner moved the generation");
    let outcome = match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
        Bind::Bound { outcome, .. } => outcome,
        Bind::Refuse(reason) => panic!("a new member of an owned root was refused: {reason}"),
        Bind::Wait => panic!("a new member of an owned root waited"),
    };
    assert_eq!(outcome.root, Some(root.clone()));
    let note = outcome.note.unwrap_or_default();
    assert!(note.contains(triple_member), "{note}");
    assert!(note.contains("profile member links"), "{note}");
    // The new link names its own member destination under the shared root,
    // and the two committed links are untouched.
    assert_eq!(
        fs::read_link(fixture.checkout.join("target").join(triple_member)).unwrap(),
        member_destination(&root, triple_member)
    );
    for member in ["debug", "release"] {
        assert_eq!(
            fs::read_link(fixture.checkout.join("target").join(member)).unwrap(),
            member_destination(&root, member)
        );
    }
    // The receipt now places all three members on that one root and nonce.
    let committed = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    let mut members: Vec<&str> = Vec::new();
    for member in ["debug", "release", triple_member] {
        let placed = committed
            .selected_for(member)
            .unwrap_or_else(|| panic!("the receipt lost {member}"));
        assert_eq!(placed.root, root, "{member}");
        assert_eq!(placed.nonce(), selected.nonce(), "{member}");
        members.push(member);
    }
    assert_eq!(committed.links.len(), 3, "{members:?}");
}

/// oxide#165 counterfactual: with no recreate step, a converted checkout
/// whose `target` directory was deleted (`rm -rf target`, a task-local
/// build-output cleanup) fails inside the member transition with ENOENT on
/// the path beneath the absent directory and never recreates it. This is
/// the exact path the binding fix guards, driven directly here so the
/// failure mode stays recorded: the target is still absent afterwards and
/// the receipt is left prepared, which is why every later command kept
/// retrying and failing the same way.
#[test]
fn a_deleted_target_directory_fails_the_member_transition_with_enoent() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    let receipt = Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    let (committed, selected, _) = adopt_profile(&context, receipt).unwrap();
    fs::remove_dir_all(context.link()).unwrap();
    assert_eq!(
        target_state(&fixture.checkout, &context.checkout_id).unwrap(),
        TargetState::Missing
    );
    let refused = transition_at(
        &context,
        &committed,
        "debug",
        &spec_for(&selected),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .expect_err("a member transition published a link inside an absent target");
    assert!(refused.contains("No such file or directory"), "{refused}");
    assert!(
        refused.contains(&context.link().display().to_string()),
        "{refused}"
    );
    assert_eq!(
        target_state(&fixture.checkout, &context.checkout_id).unwrap(),
        TargetState::Missing,
        "the failed transition recreated the target directory"
    );
    let after = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        after.state,
        ReceiptState::Prepared,
        "the write-ahead receipt is gone"
    );
}

/// oxide#165: a converted checkout's `target` directory was deleted. The
/// binding recreates that one real directory exclusively and publishes the
/// owned member links through the existing transition, into the SAME root:
/// the root's payload, the committed root and nonce are kept and nothing is
/// relinked to a new place.
#[test]
fn a_deleted_target_directory_is_recreated_and_both_members_bind() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    let store = pool_store(&fixture.root.join("pool"));
    let receipt = Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    let (committed, selected, _) = adopt_profile(&context, receipt).unwrap();
    let root = selected.root().to_path_buf();
    fs::write(root.join("sentinel"), b"root").unwrap();
    fs::write(
        member_destination(&root, "debug").join("old-output"),
        b"old",
    )
    .unwrap();
    let declaration = crate::layout::declared(&["debug".to_owned(), "release".to_owned()]).unwrap();
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    fs::remove_dir_all(context.link()).unwrap();
    assert_eq!(
        target_state(&fixture.checkout, &context.checkout_id).unwrap(),
        TargetState::Missing
    );
    let outcome = match bind_profile(&context, &store, 0, &committed, Some(&plan), false) {
        Bind::Bound { outcome, .. } => outcome,
        Bind::Refuse(reason) => panic!("a deleted target directory was refused: {reason}"),
        Bind::Wait => panic!("a deleted target directory waited"),
    };
    let target = context.link();
    let metadata = fs::symlink_metadata(&target).unwrap();
    assert!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "the recreated target is not a real directory"
    );
    assert_eq!(
        metadata.mode() & 0o777,
        0o755,
        "the recreated target does not carry the conversion's mode"
    );
    assert_eq!(outcome.root, Some(root.clone()));
    assert!(
        outcome
            .note
            .unwrap_or_default()
            .contains("profile member links"),
        "the binding did not report the owned profile members"
    );
    for member in ["debug", "release"] {
        assert_eq!(
            fs::read_link(target.join(member)).unwrap(),
            member_destination(&root, member),
            "{member}"
        );
    }
    // The owned root is the same one: its payload and the old output are
    // exactly as they were.
    assert_eq!(fs::read(root.join("sentinel")).unwrap(), b"root");
    assert_eq!(
        fs::read(member_destination(&root, "debug").join("old-output")).unwrap(),
        b"old"
    );
    let after = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    assert_eq!(after.state, ReceiptState::Committed);
    assert!(after.generation > committed.generation);
    for member in ["debug", "release"] {
        let placed = after
            .selected_for(member)
            .unwrap_or_else(|| panic!("the receipt lost {member}"));
        assert_eq!(placed.root, root, "{member}");
        assert_eq!(placed.nonce(), selected.nonce(), "{member}");
    }
}

/// oxide#165 refusals: an `target` that is a file or a foreign link still
/// refuses before Cargo even when the command was keyed while the target
/// directory was real, and nothing at that path or behind a link is
/// replaced, followed or written into.
#[test]
fn a_file_or_foreign_link_at_the_target_refuses_a_profile_binding() {
    for outside in [false, true] {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        let store = pool_store(&fixture.root.join("pool"));
        let receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        let (committed, _selected, _) = adopt_profile(&context, receipt).unwrap();
        let declaration =
            crate::layout::declared(&["debug".to_owned(), "release".to_owned()]).unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
        )
        .unwrap()
        .unwrap();
        let target = context.link();
        let proof = receipt_proof(&fixture);
        fs::remove_dir_all(&target).unwrap();
        let referent = fixture.root.join("elsewhere");
        fs::create_dir(&referent).unwrap();
        fs::write(referent.join("sentinel"), b"foreign").unwrap();
        if outside {
            symlink(&referent, &target).unwrap();
        } else {
            fs::write(&target, b"file").unwrap();
        }
        let refused = match bind_profile(&context, &store, 0, &committed, Some(&plan), false) {
            Bind::Refuse(reason) => reason.to_string(),
            Bind::Bound { .. } => panic!("a target that is not a real directory was bound"),
            Bind::Wait => panic!("a target that is not a real directory waited"),
        };
        assert!(
            refused.contains("is never replaced or followed"),
            "{refused}"
        );
        assert!(refused.contains("Nothing was changed"), "{refused}");
        // The path is exactly what it was, and nothing was written behind
        // a link.
        if outside {
            assert_eq!(fs::read_link(&target).unwrap(), referent);
            assert_eq!(fs::read(referent.join("sentinel")).unwrap(), b"foreign");
            assert_eq!(fs::read_dir(&referent).unwrap().count(), 1);
        } else {
            assert_eq!(fs::read(&target).unwrap(), b"file");
        }
        // The refusal changed no receipt byte.
        assert_eq!(receipt_proof(&fixture), proof);
    }
}

/// oxide#165 race and scope: a `target` that appears while the absent
/// directory is being recreated is kept — only a real directory is accepted
/// as the creator's result, a file or a link is refused, never replaced —
/// and the recreate belongs to the profile layout alone, so a whole-target
/// checkout's deleted link is still recreated by the legacy transition and
/// a never-converted checkout is left with no target at all.
#[test]
fn a_concurrent_creator_is_kept_and_whole_target_is_unchanged() {
    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let target = fixture.checkout.join("target");
    recreate_target_directory(&fixture.checkout).unwrap();
    let metadata = fs::symlink_metadata(&target).unwrap();
    assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
    assert_eq!(metadata.mode() & 0o777, 0o755);
    // The creator that already made the real directory wins: its content is
    // left exactly as it is.
    fs::write(target.join("sentinel"), b"creator").unwrap();
    recreate_target_directory(&fixture.checkout).unwrap();
    assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"creator");
    // Anything else at that path is refused and kept.
    fs::remove_dir_all(&target).unwrap();
    fs::write(&target, b"file").unwrap();
    let refused = recreate_target_directory(&fixture.checkout).unwrap_err();
    assert!(refused.contains("is not a real directory"), "{refused}");
    assert_eq!(fs::read(&target).unwrap(), b"file");
    fs::remove_file(&target).unwrap();
    let referent = fixture.root.join("elsewhere");
    fs::create_dir(&referent).unwrap();
    fs::write(referent.join("sentinel"), b"foreign").unwrap();
    symlink(&referent, &target).unwrap();
    let refused = recreate_target_directory(&fixture.checkout).unwrap_err();
    assert!(refused.contains("is not a real directory"), "{refused}");
    assert_eq!(fs::read_link(&target).unwrap(), referent);
    assert_eq!(fs::read(referent.join("sentinel")).unwrap(), b"foreign");

    // Scope: a committed whole-target checkout whose owned link was
    // deleted still binds its own root through the legacy transition.
    let whole = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let context = whole.context();
    let store = pool_store(&whole.root.join("pool"));
    let receipt = transition(
        &context,
        &whole.receipt(),
        &whole.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    let whole_target = context.link();
    fs::remove_file(&whole_target).unwrap();
    let wanted = desired_selection(&context, &receipt).unwrap().unwrap();
    let plan = whole_target_plan(Some(wanted));
    match bind_under_binding(&context, &store, 0, &receipt, Some(&plan), false) {
        Bind::Bound { outcome, .. } => {
            assert_eq!(outcome.root.as_deref(), Some(whole.local().root()))
        }
        Bind::Refuse(reason) => panic!("a deleted whole-target link refused: {reason}"),
        Bind::Wait => panic!("a deleted whole-target link waited"),
    }
    assert_eq!(fs::read_link(&whole_target).unwrap(), whole.local().root());

    // Scope: a never-converted checkout keeps no target, no receipt and no
    // owned root — the recreate is never reached.
    let plain = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let plain_store = pool_store(&plain.root.join("pool"));
    let binding = bind_for_command(&plain_store, &plain.checkout, 0, None, None, false).unwrap();
    assert_eq!(binding.outcome.root, None);
    assert!(!plain.checkout.join("target").exists());
    assert!(!receipt_path(plain_store.control(), &context_id(&plain)).exists());
}

/// P4, ROOT check: the same refusal through the PRODUCTION planning path.
/// The fixture registers a permanent drive whose volume cannot be verified,
/// so `planned_destination` itself stops selecting the committed external
/// root and returns a local plan; binding that derived plan must still
/// refuse before Cargo. The manual `Plan` in the regression above is a
/// binding-only control, not this one.
#[test]
fn a_planner_derived_local_plan_refuses_a_committed_external_member() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    register(&fixture, DriveMode::Permanent);
    let external = Selected {
        kind: RootKind::External,
        root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
        volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
        nonce: Some("99999999999999999999999999999999".to_owned()),
        generation: 1,
    };
    let mut receipt =
        Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt.place_member("debug").selected = Some(external.clone());
    write_receipt(&fixture.control, &receipt).unwrap();
    // A real external root carrying this checkout's marker and member
    // directory, with the live owned link into it.
    fs::create_dir_all(member_destination(&external.root, "debug")).unwrap();
    let marker = Marker {
        version: MARKER_VERSION.to_owned(),
        checkout: fixture.checkout.clone(),
        checkout_id: context.checkout_id.clone(),
        label: "checkout".to_owned(),
        recipe: RECIPE.to_owned(),
        kind: RootKind::External,
        volume_uuid: external.volume_uuid.clone(),
        nonce: external.nonce.clone(),
        checkout_volume_uuid: None,
        generation: 1,
        created_ms: 1,
    };
    fs::write(
        external.root.join(MARKER),
        serde_json::to_vec_pretty(&marker).unwrap(),
    )
    .unwrap();
    fs::create_dir_all(fixture.checkout.join("target")).unwrap();
    let link = fixture.checkout.join("target/debug");
    let destination = member_destination(&external.root, "debug");
    symlink(&destination, &link).unwrap();
    let declaration =
        crate::layout::Declaration::new([crate::layout::Member::parse("debug").unwrap()]).unwrap();
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        plan.selected().map(Selected::kind),
        Some(RootKind::Local),
        "the production planner did not fall back to the local spec"
    );
    let store = pool_store(&fixture.root.join("pool"));
    match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
        Bind::Refuse(reason) => {
            let reason = reason.to_string();
            assert!(
                reason
                    .contains("never moves an owned external output to another pool automatically"),
                "{reason}"
            );
            assert!(reason.contains("nothing was built"), "{reason}");
            assert!(
                reason.contains(&destination.display().to_string()),
                "{reason}"
            );
        }
        Bind::Bound { .. } => {
            panic!("a planner-derived local plan relinked an external member")
        }
        Bind::Wait => panic!("a planner-derived refusal waited"),
    }
    assert_eq!(fs::read_link(&link).unwrap(), destination);
}

/// oxide#133 case14 CAUSAL regression. The diagnostic's case14 refused
/// with the right reason and then found the committed receipt bytes
/// changed. Traced through the production preflight: `reserve` runs before
/// the lease and, with the registered volume away, the fresh selection is
/// the local fallback root, so it durably reserved a nonce for a root this
/// command then never builds into and rewrote the receipt of a command that
/// builds nothing. The parent fails the first byte comparison below; the
/// refusal itself (and every authority field) must be unchanged.
#[test]
fn a_refusal_after_the_volume_is_away_reserves_nothing() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    register(&fixture, DriveMode::Permanent);
    let external = Selected {
        kind: RootKind::External,
        root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
        volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
        nonce: Some("99999999999999999999999999999999".to_owned()),
        generation: 1,
    };
    let mut receipt =
        Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt.place_member("debug").selected = Some(external.clone());
    write_receipt(&fixture.control, &receipt).unwrap();
    fs::create_dir_all(member_destination(&external.root, "debug")).unwrap();
    let marker = Marker {
        version: MARKER_VERSION.to_owned(),
        checkout: fixture.checkout.clone(),
        checkout_id: context.checkout_id.clone(),
        label: "checkout".to_owned(),
        recipe: RECIPE.to_owned(),
        kind: RootKind::External,
        volume_uuid: external.volume_uuid.clone(),
        nonce: external.nonce.clone(),
        checkout_volume_uuid: None,
        generation: 1,
        created_ms: 1,
    };
    fs::write(
        external.root.join(MARKER),
        serde_json::to_vec_pretty(&marker).unwrap(),
    )
    .unwrap();
    fs::create_dir_all(fixture.checkout.join("target")).unwrap();
    let link = fixture.checkout.join("target/debug");
    let destination = member_destination(&external.root, "debug");
    symlink(&destination, &link).unwrap();
    let declaration =
        crate::layout::Declaration::new([crate::layout::Member::parse("debug").unwrap()]).unwrap();
    let receipt_file = receipt_path(&fixture.control, &context.checkout_id);
    let before = fs::read(&receipt_file).unwrap();
    let store = pool_store(&fixture.root.join("pool"));
    // Production order: the pre-lease reservation, then the plan, then bind.
    reserve(
        &store,
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        Some("the registered volume is away"),
    )
    .unwrap();
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        plan.selected().map(Selected::kind),
        Some(RootKind::Local),
        "the production planner did not fall back to the local spec"
    );
    assert_eq!(
        fs::read(&receipt_file).unwrap(),
        before,
        "the pre-lease reservation rewrote the committed receipt of a refusing command"
    );
    // The command still refuses for the exact P4 reason with no effect.
    match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
        Bind::Refuse(reason) => {
            let reason = reason.to_string();
            assert!(
                reason
                    .contains("never moves an owned external output to another pool automatically"),
                "{reason}"
            );
            assert!(reason.contains("nothing was built"), "{reason}");
            assert!(
                reason.contains(&destination.display().to_string()),
                "{reason}"
            );
        }
        Bind::Bound { .. } => panic!("a fallback refusal relinked an external member"),
        Bind::Wait => panic!("a fallback refusal waited"),
    }
    assert_eq!(
        fs::read(&receipt_file).unwrap(),
        before,
        "the refusal changed the committed receipt"
    );
    assert_eq!(fs::read_link(&link).unwrap(), destination);
    let recorded = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    assert!(
        recorded.reserved().is_none(),
        "a refusing command left a root reservation behind"
    );
    assert_eq!(recorded.generation, 1, "the refusal moved the generation");
    assert_eq!(recorded.selected_for("debug"), Some(&external));
}

/// oxide#133 review finding (b): the pre-lease predicate must use binding's
/// REQUESTED member set, not any committed external placement. In a profile
/// checkout whose `debug` stays on an external root, a command that requests
/// only a NEW nested member is not refused by binding's P4 rule, so its
/// local future root keeps its reservation and the transition proceeds. The
/// checkout-wide predicate skipped that reservation and left
/// `reservation_matches` refusing.
#[test]
fn a_new_member_beside_a_committed_external_member_keeps_its_reservation() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    register(&fixture, DriveMode::Permanent);
    let external = Selected {
        kind: RootKind::External,
        root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
        volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
        nonce: Some("99999999999999999999999999999999".to_owned()),
        generation: 1,
    };
    let mut receipt =
        Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt.place_member("debug").selected = Some(external.clone());
    write_receipt(&fixture.control, &receipt).unwrap();
    fs::create_dir_all(member_destination(&external.root, "debug")).unwrap();
    let marker = Marker {
        version: MARKER_VERSION.to_owned(),
        checkout: fixture.checkout.clone(),
        checkout_id: context.checkout_id.clone(),
        label: "checkout".to_owned(),
        recipe: RECIPE.to_owned(),
        kind: RootKind::External,
        volume_uuid: external.volume_uuid.clone(),
        nonce: external.nonce.clone(),
        checkout_volume_uuid: None,
        generation: 1,
        created_ms: 1,
    };
    fs::write(
        external.root.join(MARKER),
        serde_json::to_vec_pretty(&marker).unwrap(),
    )
    .unwrap();
    fs::create_dir_all(fixture.checkout.join("target")).unwrap();
    let debug_link = fixture.checkout.join("target/debug");
    symlink(member_destination(&external.root, "debug"), &debug_link).unwrap();
    let nested = "x86_64-unknown-linux-gnu/debug";
    let declaration =
        crate::layout::Declaration::new([crate::layout::Member::parse(nested).unwrap()]).unwrap();
    let store = pool_store(&fixture.root.join("pool"));
    // Production order: reservation, then plan, then bind.
    reserve(
        &store,
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        Some("the registered volume is away"),
    )
    .unwrap();
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        plan.selected().map(Selected::kind),
        Some(RootKind::Local),
        "the production planner did not fall back to the local spec"
    );
    let reserved = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    let reservation = reserved
        .reserved()
        .expect("a requested new member lost its required reservation");
    assert_eq!(
        Some(reservation.kind),
        plan.selected().map(Selected::kind),
        "the reservation is not for the planned local root"
    );
    assert_eq!(
        Some(reservation.nonce.as_str()),
        plan.selected().and_then(Selected::nonce),
        "the reservation does not carry the keyed nonce"
    );
    // The exact gate binding applies to a MISSING member link
    // (`member_root_authority`): the committed sibling's external placement
    // must not block the new member's local reservation. On the parent this
    // is the refusal ("reservation was cancelled or replaced") that the
    // checkout-wide predicate caused.
    let wanted = plan.selected().unwrap();
    assert!(
        !refuses_committed_external(&reserved, wanted, &declaration),
        "the pre-lease predicate still claims this accepted command will refuse"
    );
    reservation_matches(&context, &reserved, wanted)
        .expect("the new member's local reservation is not usable at binding");
    assert_eq!(
        fs::read_link(&debug_link).unwrap(),
        member_destination(&external.root, "debug"),
        "the committed external member link moved"
    );
}

/// oxide#133 review finding (a): a whole-target receipt never refuses through
/// binding's per-member P4 rule, so its local fallback root keeps the
/// reservation `bind_under_binding` requires. The checkout-wide predicate
/// skipped it and left the #117 first local fallback refusing.
#[test]
fn a_whole_target_local_fallback_keeps_its_reservation() {
    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let (context, _external) = committed_whole_target_external(&fixture);
    let declaration =
        crate::layout::Declaration::new([crate::layout::Member::parse("debug").unwrap()]).unwrap();
    let store = pool_store(&fixture.root.join("pool"));
    // Production order: reservation, then plan, then bind.
    reserve(
        &store,
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        Some("the registered volume is away"),
    )
    .unwrap();
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    assert!(
        plan.whole_target,
        "the legacy checkout planned a profile layout"
    );
    assert_eq!(
        plan.selected().map(Selected::kind),
        Some(RootKind::Local),
        "the production planner did not fall back to the local spec"
    );
    let reserved = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    assert!(
        reserved.reserved().is_some_and(
            |reservation| Some(reservation.kind) == plan.selected().map(Selected::kind)
        ),
        "a whole-target local fallback lost its required reservation"
    );
    match bind_under_binding(&context, &store, 0, &reserved, Some(&plan), false) {
        Bind::Bound { outcome, .. } => {
            assert_eq!(
                outcome.root.as_deref(),
                Some(fixture.local().root()),
                "the whole-target fallback did not bind the local root"
            );
        }
        Bind::Refuse(reason) => panic!("the whole-target local fallback refused: {reason}"),
        Bind::Wait => panic!("the whole-target local fallback waited"),
    }
}

/// The committed whole-target fixture both reservation tests start from: a
/// permanent-mode receipt selecting a marker-carrying EXTERNAL root with the
/// legacy `target` link published, so a later command's desired selection
/// falls back to the (absent) local root. Returns its context and selection.

#[test]
fn profile_slot_keys_use_the_outputs_3_namespace_only() {
    let fixture = Fixture::new();
    let local = fixture.local();
    let selected = local
        .selected(3)
        .with_nonce(Some("88888888888888888888888888888888".to_owned()));
    let profile = Plan {
        whole_target: false,
        members: vec![
            PlannedMember {
                member: "debug".to_owned(),
                selected: Some(selected.clone()),
            },
            PlannedMember {
                member: "release".to_owned(),
                selected: None,
            },
        ],
    };
    let facts = destination_facts(Some(&profile));
    let lines: Vec<&str> = facts.lines().collect();
    assert_eq!(lines.len(), 2, "{facts}");
    assert!(lines[0].starts_with("outputs-3 "), "{facts}");
    // The token after the namespace is the root nonce, never another
    // namespace prefix (the fixture path itself contains "outputs-").
    let tokens: Vec<&str> = lines[0].split_whitespace().collect();
    assert_eq!(tokens[0], "outputs-3", "{facts}");
    assert_eq!(
        tokens[1], "88888888888888888888888888888888",
        "the second token is not the root nonce: {facts}"
    );
    assert!(lines[0].ends_with(" debug"), "{facts}");
    assert_eq!(lines[1], "real release");
    // The legacy namespace is exactly the published one, with no profile
    // suffix or extra token.
    let legacy = Plan {
        whole_target: true,
        members: vec![PlannedMember {
            member: MEMBER.to_owned(),
            selected: Some(selected.clone()),
        }],
    };
    let legacy_facts = destination_facts(Some(&legacy));
    // The namespace is the FIRST TOKEN only. The fixture path itself can
    // contain "outputs-3" (its random temporary suffix), so a substring
    // test is wrong: only a leading namespace token is a namespace.
    let legacy_tokens: Vec<&str> = legacy_facts.split_whitespace().collect();
    assert_eq!(legacy_tokens[0], "outputs-2", "{legacy_facts}");
    assert!(
        !legacy_facts
            .lines()
            .any(|line| line.split_whitespace().next() == Some("outputs-3")),
        "{legacy_facts}"
    );
    assert_eq!(legacy_facts.lines().count(), 1);
    assert!(legacy_facts.ends_with("g3"), "{legacy_facts}");
}

#[test]
fn ephemeral_mode_binds_a_converted_profile_checkout_without_relinking() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    let receipt = Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    let (receipt, selected, _) = adopt_profile(&context, receipt).unwrap();
    let declaration = crate::layout::Declaration::new([
        crate::layout::Member::parse("debug").unwrap(),
        crate::layout::Member::parse("release").unwrap(),
    ])
    .unwrap();
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    // The live member links are exactly what the slot was keyed for.
    let outcome = bind_profile_sealed(&context, &receipt, Some(&plan)).unwrap();
    assert_eq!(outcome.root.as_deref(), Some(selected.root()));
    assert!(outcome.note.is_some());
    // A member link that selects another root refuses: ephemeral mode
    // never relinks it.
    let debug = fixture.checkout.join("target/debug");
    fs::remove_file(&debug).unwrap();
    symlink("/tmp", &debug).unwrap();
    let refused = match bind_profile_sealed(&context, &receipt, Some(&plan)) {
        Ok(_) => panic!("a foreign member link was bound"),
        Err(reason) => reason,
    };
    assert!(refused.contains("ephemeral"), "{refused}");
    assert_eq!(
        fs::read_link(&debug).unwrap(),
        std::path::PathBuf::from("/tmp")
    );
}

#[test]
fn ephemeral_mode_uses_a_missing_converted_member_locally() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let store = pool_store(&fixture.root.join("pool"));
    let control = store.owner_control();
    let context = Context::new(Some(control.to_path_buf()), fixture.checkout.clone());
    let receipt = Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    let (receipt, selected, _) = adopt_profile(&context, receipt).unwrap();
    register_at(&fixture, control, DriveMode::Ephemeral);
    let debug = context.link().join("debug");
    let release = context.link().join("release");
    let old_debug = member_destination(selected.root(), "debug");
    fs::write(old_debug.join("retained"), b"old outputs").unwrap();
    fs::remove_file(&debug).unwrap();

    let declaration = declared(&["debug", "release"]);
    let plan = |members: &crate::layout::Declaration| {
        planned_destination(Some(control), &fixture.checkout, members, None)
            .unwrap()
            .unwrap()
    };
    let keyed = plan(&declaration);
    assert_eq!(keyed.members[0].member, "debug");
    assert!(keyed.members[0].selected.is_none());
    assert_eq!(
        keyed.members[1].selected.as_ref(),
        receipt.selected_for("release")
    );
    assert!(destination_facts(Some(&keyed))
        .lines()
        .any(|line| line == "real debug"));
    let derived = exec_declaration(Some(control), &fixture.checkout, None).unwrap();
    assert_eq!(
        destination_facts(Some(&plan(&derived))),
        destination_facts(Some(&keyed))
    );

    let bind =
        |keyed: &Plan| bind_for_command(&store, &fixture.checkout, 0, Some(keyed), None, false);
    let binding = bind(&keyed).unwrap();
    assert_eq!(
        binding._holds.len(),
        1,
        "the surviving release link still holds its root"
    );
    // Cargo's creation after binding stays on the checkout side.
    fs::create_dir(&debug).unwrap();
    fs::write(debug.join("new-output"), b"local output").unwrap();
    drop(binding);
    let derived = exec_declaration(Some(control), &fixture.checkout, None).unwrap();
    assert!(plan(&derived).members[0].selected.is_none());
    assert_eq!(context.receipt().unwrap(), Some(receipt.clone()));
    assert_eq!(
        fs::read(old_debug.join("retained")).unwrap(),
        b"old outputs"
    );
    assert!(!old_debug.join("new-output").exists());
    assert_eq!(
        fs::read_link(&release).unwrap(),
        member_destination(selected.root(), "release")
    );

    // A foreign link appearing after planning is refused under binding.
    fs::remove_dir_all(&debug).unwrap();
    let foreign = fixture.root.join("foreign");
    fs::create_dir(&foreign).unwrap();
    symlink(&foreign, &debug).unwrap();
    let Err(reason) = bind(&keyed) else {
        panic!("bound a foreign member link")
    };
    assert!(
        reason.contains("target/debug") && reason.contains("oxide outputs adopt"),
        "{reason}"
    );
    assert_eq!(fs::read_link(&debug).unwrap(), foreign);

    fs::remove_file(&debug).unwrap();
    fs::rename(selected.root(), fixture.root.join("retained-root")).unwrap();
    let Err(reason) = bind(&keyed) else {
        panic!("bound a link to the unavailable root")
    };
    assert!(
        reason.contains("target/release") && reason.contains("oxide outputs adopt"),
        "{reason}"
    );

    // When every link is gone, build locally even if the old root is
    // unavailable. A historical placement cannot require a running hold.
    fs::remove_dir_all(context.link()).unwrap();
    let debug_only = declared(&["debug"]);
    reserve(&store, Some(control), &fixture.checkout, &debug_only, None).unwrap();
    let binding = bind(&plan(&debug_only)).unwrap();
    assert!(binding._holds.is_empty());
    assert_eq!(context.receipt().unwrap(), Some(receipt));
}

/// Permanent mode never selects the external root for a command whose
/// intermediate slot fell back to the internal pool.
#[test]
fn a_pool_fallback_never_selects_the_external_root() {
    let fixture = Fixture::new();
    register(&fixture, DriveMode::Permanent);
    let context = fixture
        .context()
        .with_pool_fallback(Some("container mismatch"));
    let refused = usable_external(&context).unwrap_err();
    assert!(
        refused.contains("fell back to the internal pool"),
        "{refused}"
    );
    let (_, external) = committed_external(&fixture);
    let planned = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &crate::layout::Declaration::empty(),
        Some("container mismatch"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(planned.selected().unwrap().kind, RootKind::Local);
    assert_ne!(planned.selected().unwrap().root, external);
}

/// A live link to another owned root (an older selection restored by
/// hand) disagrees with the committed receipt: binding refuses, even
/// though that root's marker is valid, and nothing changes.
#[test]
fn a_link_restored_to_an_older_owned_root_is_refused() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let local = transition(
        &context,
        &fixture.receipt(),
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    let expected = local.selected().unwrap().clone();
    let mut receipt = local.clone();
    receipt.link_mut().selected = Some(Selected {
        kind: RootKind::External,
        root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
        volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
        nonce: None,
        generation: 1,
    });
    write_receipt(&fixture.control, &receipt).unwrap();
    let store = pool_store(&fixture.root.join("pool"));
    let plan = whole_target_plan(Some(expected.clone()));
    match bind_under_binding(&context, &store, 0, &receipt, Some(&plan), false) {
        Bind::Refuse(reason) => {
            let reason = reason.to_string();
            assert!(reason.contains("committed receipt selects"), "{reason}")
        }
        Bind::Bound { .. } => panic!("a link contradicting the committed receipt was bound"),
        Bind::Wait => panic!("a contradictory link waited instead of refusing"),
    }
    assert_eq!(fs::read_link(context.link()).unwrap(), expected.root);
    assert_eq!(
        read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap(),
        receipt
    );
}

/// A prepared receipt whose link names another checkout's target is
/// refused when read, and replay never renames over that target.

#[test]
fn a_receiptless_link_is_never_followed_in_permanent_mode() {
    let fixture = Fixture::new();
    let store = pool_store(&fixture.root.join("pool"));
    register_at(&fixture, store.owner_control(), DriveMode::Permanent);
    let context = fixture.context();
    let bind = |label: &str| match bind_for_command(&store, &fixture.checkout, 0, None, None, false)
    {
        Ok(_) => Ok(()),
        Err(reason) => Err(format!("{label}: {reason}")),
    };
    assert!(bind("missing").is_ok());
    fs::create_dir(context.link()).unwrap();
    assert!(bind("real").is_ok());
    fs::remove_dir(context.link()).unwrap();

    let foreign = fixture.root.join("foreign");
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("sentinel"), b"foreign").unwrap();
    symlink(&foreign, context.link()).unwrap();
    let refused = bind("foreign").unwrap_err();
    assert!(refused.contains("never follows"), "{refused}");
    assert_eq!(fs::read_link(context.link()).unwrap(), foreign);
    assert_eq!(fs::read_dir(&foreign).unwrap().count(), 1);

    // A marked root linked without its receipt is not owned either.
    fs::remove_file(context.link()).unwrap();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    symlink(spec.root(), context.link()).unwrap();
    let refused = bind("marked").unwrap_err();
    assert!(refused.contains("never follows"), "{refused}");
    assert!(context.receipt().unwrap().is_none());
}

/// A command keyed while its checkout was unconverted (no destination)
/// refuses if the checkout was converted while it waited, in either
/// mode; keyed for the owned root it binds normally.
#[test]
fn a_conversion_while_a_command_waits_refuses_its_unconverted_key() {
    for mode in [DriveMode::Permanent, DriveMode::Ephemeral] {
        let fixture = Fixture::new();
        let store = pool_store(&fixture.root.join("pool"));
        register_at(&fixture, store.owner_control(), mode);
        let context = Context::new(
            Some(store.owner_control().to_path_buf()),
            fixture.checkout.clone(),
        );
        // Keyed before conversion: no destination.
        let keyed = planned_destination(
            Some(store.owner_control()),
            &fixture.checkout,
            &crate::layout::Declaration::empty(),
            None,
        )
        .unwrap();
        assert!(keyed.is_none());
        // Adoption completes while the command waits.
        transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        match bind_for_command(&store, &fixture.checkout, 0, keyed.as_ref(), None, false) {
            Ok(_) => panic!("{mode:?}: bound an owned root with an unconverted slot key"),
            Err(reason) => assert!(reason.contains("converted to owned outputs"), "{reason}"),
        }
        // Control: a command keyed after conversion binds that root.
        let rekeyed = planned_destination(
            Some(store.owner_control()),
            &fixture.checkout,
            &crate::layout::Declaration::empty(),
            None,
        )
        .unwrap();
        assert!(rekeyed.is_some());
        match bind_for_command(&store, &fixture.checkout, 0, rekeyed.as_ref(), None, false) {
            Ok(binding) => assert_eq!(
                binding.outcome.root.as_deref(),
                Some(fixture.local().root())
            ),
            Err(reason) => panic!("{mode:?}: {reason}"),
        }
    }
}

/// A converted command in ephemeral mode holds the binding mutex until
/// it drops its binding (after it is recorded running), so a relinker
/// started by a switch to permanent mode waits for it.
#[test]
fn a_converted_ephemeral_command_holds_the_binding_mutex() {
    let fixture = Fixture::new();
    let store = pool_store(&fixture.root.join("pool"));
    register_at(&fixture, store.owner_control(), DriveMode::Ephemeral);
    let context = Context::new(
        Some(store.owner_control().to_path_buf()),
        fixture.checkout.clone(),
    );
    transition(
        &context,
        &fixture.receipt(),
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    let keyed = planned_destination(
        Some(store.owner_control()),
        &fixture.checkout,
        &crate::layout::Declaration::empty(),
        None,
    )
    .unwrap();
    let binding = bind_for_command(&store, &fixture.checkout, 0, keyed.as_ref(), None, false)
        .unwrap_or_else(|reason| panic!("{reason}"));
    // The mode switches to permanent and a relinker starts.
    register_at(&fixture, store.owner_control(), DriveMode::Permanent);
    let acquired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let relinker = {
        let acquired = acquired.clone();
        let root = fixture.root.join("pool");
        let checkout = fixture.checkout.clone();
        std::thread::spawn(move || {
            let store = pool_store(&root);
            let _lock = store.lock_binding(&checkout).unwrap();
            acquired.store(true, Ordering::SeqCst);
        })
    };
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !acquired.load(Ordering::SeqCst),
        "a relinker took the binding mutex while the converted command was starting"
    );
    // Control: once the command's binding is dropped, the relinker runs.
    drop(binding);
    relinker.join().unwrap();
    assert!(acquired.load(Ordering::SeqCst));
}

/// A receipt write publishes through the registry authority: a committed
/// transition has one selected row, and a prepared transition keeps that
/// committed root selected while the proposed root is pending.

#[test]
fn a_keyed_future_root_binds_exactly_its_surviving_reservation() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let (reserved, spec) = keyed_fixture(&fixture);
    let future = spec.root().to_path_buf();
    let reservation = reserved.reserved().cloned().unwrap();
    let bound = transition(
        &context,
        &reserved,
        &spec,
        Start::Any,
        NonceAuthority::Keyed(Some(&reservation.nonce)),
    )
    .unwrap();
    assert_eq!(
        read_marker(&future).unwrap().nonce.as_deref(),
        Some(reservation.nonce.as_str()),
        "the created root did not carry the reserved nonce"
    );
    assert!(
        bound.reserved().is_none(),
        "the commit kept its consumed reservation"
    );
    assert_eq!(
        fs::read_link(context.link()).unwrap(),
        member_destination(&future, MEMBER),
        "the owned link does not select the reserved root"
    );
    assert!(
        crate::prune::validate_reservation(&fixture.control, &reservation).is_err(),
        "the consumed reservation is still valid"
    );
}

/// A keyed slot whose reservation was cancelled refuses with the
/// cancelled-or-replaced reason inside the transition, before the prepared
/// receipt, the root, the link or any registry write: the control tree and
/// the absent root are byte-identical afterwards.
#[test]
fn a_cancelled_keyed_reservation_refuses_before_any_authority_effect() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let (reserved, spec) = keyed_fixture(&fixture);
    let future = spec.root().to_path_buf();
    let reservation = reserved.reserved().cloned().unwrap();
    let mut held = reserved.clone();
    let store = Store::for_tests(&fixture.root.join("pool"));
    cancel_reservation(&store, &fixture.control, &context, &mut held).unwrap();
    assert!(
        held.reserved().is_none(),
        "the cancellation kept the reservation"
    );
    let before = authority_tree(&fixture.control);
    let refused = transition(
        &context,
        &held,
        &spec,
        Start::Any,
        NonceAuthority::Keyed(Some(&reservation.nonce)),
    )
    .unwrap_err();
    assert!(refused.contains("cancelled or replaced"), "{refused}");
    assert!(
        !future.exists() && read_marker(&future).is_err(),
        "a refused keyed transition created the future root"
    );
    assert_eq!(
        authority_tree(&fixture.control),
        before,
        "a refused keyed transition wrote receipt or registry authority"
    );
    assert!(
        fs::symlink_metadata(context.link()).is_err(),
        "a refused keyed transition published the owned link"
    );
}

/// The two zero-effect negatives a keyed future-root slot must refuse: no
/// reservation at all (the shape the old fresh-nonce fallback silently
/// converted into a root plus a committed receipt), and a reservation
/// replaced by another nonce after a cancellation. Both refuse with the
/// cancelled-or-replaced reason and leave every authority byte unchanged;
/// the surviving replacement still binds when the slot names it.
#[test]
fn an_absent_or_replaced_keyed_reservation_refuses_before_any_authority_effect() {
    // Absent: the receipt holds no reservation at all.
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    let future = spec.root().to_path_buf();
    let receipt = fixture.receipt();
    let unbacked = crate::prune::fresh_nonce().unwrap();
    let before = authority_tree(&fixture.control);
    let refused = transition(
        &context,
        &receipt,
        &spec,
        Start::Any,
        NonceAuthority::Keyed(Some(&unbacked)),
    )
    .unwrap_err();
    assert!(refused.contains("cancelled or replaced"), "{refused}");
    assert!(
        !future.exists() && read_marker(&future).is_err(),
        "an unbacked keyed nonce created the future root"
    );
    assert_eq!(
        authority_tree(&fixture.control),
        before,
        "an unbacked keyed nonce wrote receipt or registry authority"
    );
    assert!(
        fs::symlink_metadata(context.link()).is_err(),
        "an unbacked keyed nonce published the owned link"
    );

    // Replaced: the same future root, cancelled and reserved again under a
    // new nonce; the stale keyed nonce refuses with no further effect.
    let (mut reserved, spec) = keyed_fixture(&fixture);
    let first = reserved.reserved().cloned().unwrap();
    let store = Store::for_tests(&fixture.root.join("pool"));
    cancel_reservation(&store, &fixture.control, &context, &mut reserved).unwrap();
    let second = publish_reservation(&fixture, &mut reserved, &future);
    assert_ne!(
        first.nonce, second.nonce,
        "the replacement reused its nonce"
    );
    let before = authority_tree(&fixture.control);
    let refused = transition(
        &context,
        &reserved,
        &spec,
        Start::Any,
        NonceAuthority::Keyed(Some(&first.nonce)),
    )
    .unwrap_err();
    assert!(refused.contains("cancelled or replaced"), "{refused}");
    assert!(
        !future.exists() && read_marker(&future).is_err(),
        "a replaced keyed nonce created the future root"
    );
    assert_eq!(
        authority_tree(&fixture.control),
        before,
        "a replaced keyed nonce wrote receipt or registry authority"
    );
    // The surviving replacement still binds when the slot names it.
    let bound = transition(
        &context,
        &reserved,
        &spec,
        Start::Any,
        NonceAuthority::Keyed(Some(&second.nonce)),
    )
    .unwrap();
    assert_eq!(
        read_marker(&future).unwrap().nonce.as_deref(),
        Some(second.nonce.as_str())
    );
    assert!(bound.reserved().is_none());
}

/// A committed profile checkout whose owned local root and both member
/// destinations exist, so every member link resolves (`OwnedLink`).

#[test]
fn a_dangling_requested_member_reserves_nothing_and_adopt_recovers() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    let store = pool_store(&fixture.root.join("pool"));
    let receipt = committed_local_profile(&fixture, &context);
    let old_nonce = receipt
        .selected()
        .and_then(Selected::nonce)
        .unwrap()
        .to_owned();
    let root = receipt.selected().unwrap().root().to_path_buf();
    let declaration = declared(&["debug", "release"]);
    let receipt_file = receipt_path(&fixture.control, &context.checkout_id);
    // The committed root disappears under its live member links: binding's
    // exact Dangling precondition.
    fs::remove_dir_all(&root).unwrap();
    for member in ["debug", "release"] {
        let state = link_state(
            &link_path(&context.workspace, member),
            &context.workspace,
            &context.checkout_id,
        )
        .unwrap();
        assert!(
            matches!(state, TargetState::Dangling(_)),
            "{member}: {state:?}"
        );
    }
    assert!(refuses_dangling_member(&context, &receipt, &declaration).unwrap());
    let before = authority_tree(&fixture.control);
    let receipt_bytes = fs::read(&receipt_file).unwrap();
    let revision = crate::prune::registry_revision(&fixture.control).unwrap();
    reserve(
        &store,
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap();
    assert!(
        context.receipt().unwrap().unwrap().reserved().is_none(),
        "a dangling requested member published a reservation"
    );
    assert_eq!(
        authority_tree(&fixture.control),
        before,
        "the refused reservation wrote receipt or registry authority"
    );
    assert_eq!(fs::read(&receipt_file).unwrap(), receipt_bytes);
    assert_eq!(
        crate::prune::registry_revision(&fixture.control).unwrap(),
        revision
    );
    assert!(
        read_marker(&root).is_err() && !root.exists(),
        "the refused reservation created the root"
    );
    // Binding then refuses exactly as before, still with no authority effect.
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
        Bind::Refuse(reason) => {
            assert!(reason.contains("not available now"), "{reason}")
        }
        _ => panic!("a dangling member bound"),
    }
    assert_eq!(
        authority_tree(&fixture.control),
        before,
        "binding wrote authority alongside its dangling refusal"
    );
    // The explicit adopt the refusal names recovers with a fresh nonce.
    let (committed, recovered, converted) = adopt_profile(&context, receipt.clone()).unwrap();
    assert!(converted, "the explicit adopt converted nothing");
    let fresh = committed
        .selected_for("debug")
        .and_then(Selected::nonce)
        .unwrap()
        .to_owned();
    assert_ne!(
        fresh, old_nonce,
        "the recovery reused the old committed nonce"
    );
    assert_ne!(recovered.root(), root.as_path());
    assert!(!root.exists(), "the old path was recreated");
    assert_eq!(
        read_marker(recovered.root()).unwrap().nonce.as_deref(),
        Some(fresh.as_str())
    );
    assert!(committed.reserved().is_none());
    let rows = crate::prune::test_rows(&fixture.control).unwrap();
    let root_rows: Vec<_> = rows
        .iter()
        .filter(|(row_root, ..)| row_root == recovered.root())
        .collect();
    assert_eq!(root_rows.len(), 2, "{rows:?}");
    assert!(
        root_rows
            .iter()
            .all(|(_, selected, pending)| *selected && !*pending),
        "the recovery left a pending or unselected row: {rows:?}"
    );
    // The next ordinary build binds the recovered root with its fresh nonce.
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    match bind_profile(&context, &store, 0, &committed, Some(&plan), false) {
        Bind::Bound { outcome, .. } => {
            assert_eq!(outcome.root.as_deref(), Some(recovered.root()))
        }
        Bind::Refuse(reason) => panic!("the recovered root did not bind: {reason}"),
        Bind::Wait => panic!("the recovered root waited"),
    }
}

/// The publication guard is scoped exactly like binding: only the REQUESTED
/// member set is classified, and only the exact dangling state refuses. An
/// unrequested dangling sibling cannot block another member's reservation, a
/// resolving owned link (the local→external redock) and a new member's
/// absent link keep theirs, and a whole-target layout is never converted
/// here.
#[test]
fn the_dangling_publication_guard_is_scoped_to_requested_members() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    let receipt = committed_local_profile(&fixture, &context);
    let root = receipt.selected().unwrap().root().to_path_buf();
    assert!(!refuses_dangling_member(&context, &receipt, &declared(&["debug"])).unwrap());
    assert!(!refuses_dangling_member(&context, &receipt, &declared(&["release"])).unwrap());
    // Only release's destination disappears: a debug-only command still
    // reserves, and a command that requests release is refused.
    fs::remove_dir_all(member_destination(&root, "release")).unwrap();
    assert!(
        !refuses_dangling_member(&context, &receipt, &declared(&["debug"])).unwrap(),
        "an unrequested dangling sibling blocked the reservation"
    );
    assert!(refuses_dangling_member(&context, &receipt, &declared(&["release"])).unwrap());
    // A new member has no committed placement and its link is absent: it
    // joins the owned root and is never treated as dangling.
    let new_member = "aarch64-apple-darwin/debug";
    assert!(
        !refuses_dangling_member(&context, &receipt, &declared(&[new_member])).unwrap(),
        "a new member was treated as a dangling committed member"
    );
    // A whole-target layout keeps its reservation: `reserve` for the legacy
    // layout needs it so `reservation_matches` can accept the transition.
    let whole = fixture.receipt();
    assert_eq!(whole.layout(), LayoutKind::WholeTarget);
    assert!(!refuses_dangling_member(&context, &whole, &declared(&["debug"])).unwrap());
}

/// Only an unkeyed explicit adoption may draw a fresh nonce for a future
/// root: `oxide outputs adopt` has no keyed slot, and the root it creates
/// carries a fresh 128-bit nonce that no reservation names.
#[test]
fn an_unkeyed_adoption_draws_a_fresh_future_root_nonce() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let receipt = Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    let (committed, _local, converted) = adopt_profile(&context, receipt).unwrap();
    assert!(converted, "the first adoption converted nothing");
    assert!(
        committed.reserved().is_none(),
        "an unkeyed adoption published a reservation"
    );
    assert_eq!(
        committed
            .selected_for("debug")
            .and_then(Selected::nonce)
            .map(str::len),
        Some(32),
        "the adoption did not draw a fresh 128-bit nonce"
    );
}

/// A receiptless legacy root's checkout gets an exclusive `pruning`
/// receipt: every replay/bind path refuses it, an existing receipt
/// refuses the creator, only this writer's unjournaled creation is
/// rebound, and the terminal receipt needs the same intent.

#[test]
fn paths_in_receipts_are_absolute() {
    assert!(safe_absolute(Path::new("/work/app/target")));
    assert!(!safe_absolute(Path::new("target")));
    assert!(!safe_absolute(Path::new("/work/../target")));
}
