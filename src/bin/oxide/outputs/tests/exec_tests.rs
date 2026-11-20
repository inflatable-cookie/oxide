use super::*;
use std::fs;
use std::os::unix::fs::symlink;

#[test]
fn exec_derives_the_receipt_pair_and_matches_a_complete_declaration() {
    let fixture = Fixture::new();
    placed_profile(&fixture, &["debug", "release"], &[]);
    assert_eq!(derived_exec(&fixture), ["debug", "release"]);
    let declaration = exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap();
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    let facts = destination_facts(Some(&plan));
    assert_eq!(facts.lines().count(), 2, "{facts}");
    assert!(
        facts.lines().all(|line| line.starts_with("outputs-3 ")),
        "{facts}"
    );
    assert!(facts.contains(" debug"), "{facts}");
    assert!(facts.contains(" release"), "{facts}");
    let declared = crate::layout::declared(&["debug".to_owned(), "release".to_owned()]).unwrap();
    let declared_plan =
        planned_destination(Some(&fixture.control), &fixture.checkout, &declared, None)
            .unwrap()
            .unwrap();
    assert_eq!(destination_facts(Some(&declared_plan)), facts);
}

/// S1 counterfactual 2 + 15: a receipt-recorded unselected member is
/// planned `real`, an unrecorded real member is in neither the plan nor the
/// key, and the existing bounded notice names the real directory.
#[test]
fn exec_keeps_a_recorded_real_member_local_and_ignores_an_unrecorded_one() {
    let fixture = Fixture::new();
    let (context, receipt, _root) = placed_profile(&fixture, &["debug"], &["release"]);
    fs::create_dir(fixture.checkout.join("target/release")).unwrap();
    // An unrecorded real triple member is local and path-stable: a
    // declared command would key it, a derived one never does.
    let triple = "x86_64-unknown-linux-gnu/debug";
    fs::create_dir_all(fixture.checkout.join("target").join(triple)).unwrap();
    assert_eq!(derived_exec(&fixture), ["debug", "release"]);
    let declaration = exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap();
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    let facts = destination_facts(Some(&plan));
    assert!(facts.contains("real release"), "{facts}");
    assert!(!facts.contains(triple), "{facts}");
    assert_eq!(facts.lines().count(), 2, "{facts}");
    let declared = crate::layout::declared(&[triple.to_owned()]).unwrap();
    let declared_plan =
        planned_destination(Some(&fixture.control), &fixture.checkout, &declared, None)
            .unwrap()
            .unwrap();
    assert!(destination_facts(Some(&declared_plan)).contains(triple));
    // The binding keeps `release` local and reports it once, in the
    // existing grammar.
    let store = pool_store(&fixture.root.join("pool"));
    let outcome = match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
        Bind::Bound { outcome, .. } => outcome,
        Bind::Refuse(reason) => panic!("a recorded real member refused the command: {reason}"),
        Bind::Wait => panic!("a recorded real member waited"),
    };
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("release") && warning.contains("real")),
        "{:?}",
        outcome.warnings
    );
}

/// S1 counterfactual 3: a detached profile receipt owns no destination, so
/// the derived set is empty, the plan is `None` and binding runs locally
/// with no link.
#[test]
fn a_detached_profile_receipt_plans_no_destination_and_binds_locally() {
    let fixture = Fixture::new();
    let (context, mut receipt, _root) = placed_profile(&fixture, &["debug", "release"], &[]);
    for member in ["debug", "release"] {
        fs::remove_file(fixture.checkout.join("target").join(member)).unwrap();
        fs::create_dir(fixture.checkout.join("target").join(member)).unwrap();
        receipt.place_member(member).selected = None;
    }
    receipt.state = ReceiptState::Detached;
    write_receipt(&fixture.control, &receipt).unwrap();
    assert!(derived_exec(&fixture).is_empty());
    assert!(planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &crate::layout::Declaration::empty(),
        None
    )
    .unwrap()
    .is_none());
    let store = pool_store(&fixture.root.join("pool"));
    match bind_profile(&context, &store, 0, &receipt, None, false) {
        Bind::Bound { outcome, .. } => {
            assert!(outcome.root.is_none());
            assert_eq!(outcome.note.as_deref(), Some("outputs detached"));
        }
        Bind::Refuse(reason) => panic!("a detached exec was refused: {reason}"),
        Bind::Wait => panic!("a detached exec waited"),
    }
    // A slot keyed while the checkout was still committed refuses rather
    // than running through a link the receipt no longer selects.
    let stale = Plan {
        whole_target: false,
        members: vec![PlannedMember {
            member: "debug".to_owned(),
            selected: None,
        }],
    };
    assert!(matches!(
        bind_profile(&context, &store, 0, &receipt, Some(&stale), false),
        Bind::Refuse(_)
    ));
}

/// S1 counterfactual 4: a receipt in any custody state other than committed
/// or detached refuses the derivation, naming the existing rule.
#[test]
fn exec_refuses_a_pending_or_pruned_profile_receipt() {
    let fixture = Fixture::new();
    let (_, receipt, _root) = placed_profile(&fixture, &["debug"], &[]);
    let spec = fixture.local();
    let mut prepared = receipt.clone();
    prepared.state = ReceiptState::Prepared;
    prepared.place_member("debug").prepared = Some(Prepared {
        generation: 2,
        to: spec.selected(2),
        link: fixture.checkout.join("target/debug"),
    });
    prepared.generation = 1;
    let mut pruned = receipt.clone();
    pruned.state = ReceiptState::Pruned;
    for (candidate, expected) in [(prepared, "interrupted"), (pruned, "was pruned")] {
        let label = candidate.state;
        write_receipt(&fixture.control, &candidate).unwrap();
        let refused =
            exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap_err();
        assert!(refused.contains(expected), "{label:?}: {refused}");
    }
    // The unmodified committed receipt still derives: a control.
    write_receipt(&fixture.control, &receipt).unwrap();
    assert_eq!(derived_exec(&fixture), ["debug"]);
}

/// S1 counterfactuals 5 and 6: a whole-target receipt and a checkout with
/// no receipt derive nothing, so their behaviour is unchanged.
#[test]
fn exec_derives_nothing_for_a_whole_target_or_unconverted_checkout() {
    let fixture = Fixture::new();
    // Never converted.
    assert!(derived_exec(&fixture).is_empty());
    assert!(planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &crate::layout::Declaration::empty(),
        None
    )
    .unwrap()
    .is_none());
    let context = fixture.context();
    let committed = transition(
        &context,
        &fixture.receipt(),
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    assert_eq!(committed.layout(), LayoutKind::WholeTarget);
    assert!(derived_exec(&fixture).is_empty());
    // A modelled command keeps the legacy plan and key, untouched.
    let declaration = declaration_of(&["debug".to_owned()]);
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    assert!(plan.whole_target);
    let facts = destination_facts(Some(&plan));
    assert_eq!(facts.lines().count(), 1, "{facts}");
    assert!(facts.starts_with("outputs-2 "), "{facts}");
    // A legacy key renders only `outputs-2` lines. The check is on the line
    // prefix, not a substring: a fixture path may contain `outputs-3`.
    assert!(
        facts.lines().all(|line| line.starts_with("outputs-2 ")),
        "{facts}"
    );
}

/// S1 counterfactuals 7, 8, 13 and 16: the classification is non-vacuous.
/// An owned member link is accepted while the same path with another
/// destination, a link the receipt does not place, and a member-shaped
/// regular file all refuse before Cargo with nothing changed.
#[test]
fn exec_refuses_every_member_shaped_path_it_does_not_own() {
    // The accepted control first.
    let accepted = Fixture::new();
    placed_profile(&accepted, &["debug"], &[]);
    assert_eq!(derived_exec(&accepted), ["debug"]);
    // Another destination, everything else identical.
    let elsewhere = Fixture::new();
    let (_, receipt, root) = placed_profile(&elsewhere, &["debug"], &[]);
    let outside = elsewhere.root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    let link = elsewhere.checkout.join("target/debug");
    fs::remove_file(&link).unwrap();
    symlink(&outside, &link).unwrap();
    let refused =
        exec_declaration(Some(&elsewhere.control), &elsewhere.checkout, None).unwrap_err();
    assert!(refused.contains("target/debug"), "{refused}");
    assert!(refused.contains("oxide outputs adopt"), "{refused}");
    assert_eq!(fs::read_link(&link).unwrap(), outside);
    assert!(root.join("debug").read_dir().unwrap().next().is_none());
    let _ = receipt;
    // A link the receipt does not place refuses as well, owned or foreign.
    let unplaced = Fixture::new();
    let (context, _receipt, root) = placed_profile(&unplaced, &["debug"], &[]);
    fs::create_dir_all(member_destination(&root, "release")).unwrap();
    symlink(
        &member_destination(&root, "release"),
        &unplaced.checkout.join("target/release"),
    )
    .unwrap();
    let refused = exec_declaration(Some(&unplaced.control), &unplaced.checkout, None).unwrap_err();
    assert!(refused.contains("target/release"), "{refused}");
    assert!(refused.contains("does not place it"), "{refused}");
    let _ = context;
    // A member-shaped regular file refuses too.
    let filed = Fixture::new();
    let (_context, _receipt, _root) = placed_profile(&filed, &["debug"], &[]);
    fs::write(filed.checkout.join("target/release"), b"file").unwrap();
    let refused = exec_declaration(Some(&filed.control), &filed.checkout, None).unwrap_err();
    assert!(refused.contains("target/release"), "{refused}");
    assert_eq!(
        fs::read(filed.checkout.join("target/release")).unwrap(),
        b"file"
    );
}

/// oxide#162: a receipt-recorded member link whose destination directory is
/// absent inside the checkout's own verified, committed root is created by
/// the binding instead of being refused as "root not available". The
/// observed shape: a migration left `target/release` linked before the root
/// had ever held a release build.
///
/// Counterfactual: reverting the derivation change makes `derived_exec`
/// return an error here, and reverting the binding change makes it refuse.
#[test]
fn exec_creates_an_absent_member_of_the_verified_owned_root() {
    let fixture = Fixture::new();
    register(&fixture, DriveMode::Permanent);
    let (context, receipt, root) = placed_profile(&fixture, &["debug", "release"], &[]);
    fs::remove_dir(root.join("release")).unwrap();
    let link = fixture.checkout.join("target/release");
    assert_eq!(fs::read_link(&link).unwrap(), root.join("release"));
    assert!(!root.join("release").exists());
    assert_eq!(
        link_state(&link, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .code(),
        "dangling-link"
    );
    let before = fs::read(receipt_path(&fixture.control, &context.checkout_id)).unwrap();
    assert_eq!(derived_exec(&fixture), ["debug", "release"]);
    let declaration = exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap();
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    assert!(destination_facts(Some(&plan)).contains("release"));
    let store = pool_store(&fixture.root.join("pool"));
    let outcome = match bind_profile(&context, &store, 0, &receipt, Some(&plan), true) {
        Bind::Bound { outcome, .. } => outcome,
        Bind::Refuse(reason) => {
            let reason = reason.to_string();
            panic!("an absent member of the verified root refused: {reason}")
        }
        Bind::Wait => panic!("an absent member of the verified root waited"),
    };
    assert_eq!(outcome.root.as_deref(), Some(root.as_path()));
    let metadata = fs::symlink_metadata(root.join("release")).unwrap();
    assert!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "the binding did not create a real member directory"
    );
    assert!(fs::read_dir(root.join("release")).unwrap().next().is_none());
    // Nothing was relinked and the receipt bytes are untouched.
    assert_eq!(fs::read_link(&link).unwrap(), root.join("release"));
    assert!(fs::metadata(&link).unwrap().is_dir());
    assert_eq!(
        fs::read(receipt_path(&fixture.control, &context.checkout_id)).unwrap(),
        before
    );
    // A second binding sees the member as an ordinary owned link.
    match bind_profile(&context, &store, 0, &receipt, Some(&plan), true) {
        Bind::Bound { outcome, .. } => {
            assert_eq!(outcome.root.as_deref(), Some(root.as_path()))
        }
        Bind::Refuse(reason) => panic!("the created member refused a second binding: {reason}"),
        Bind::Wait => panic!("the created member waited"),
    }
}

/// oxide#165: a converted checkout's whole `target` directory was deleted,
/// so every member link is positively absent while the owned root verifies.
/// The derived `exec` plans the receipt's members and the binding recreates
/// the real directory and republishes the pair into the same root; an
/// unverified root still refuses and creates nothing.
#[test]
fn exec_plans_deleted_member_links_of_the_verified_owned_root() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    register(&fixture, DriveMode::Permanent);
    let context = fixture.context();
    let receipt = Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    let (committed, selected, _) = adopt_profile(&context, receipt).unwrap();
    let root = selected.root().to_path_buf();
    fs::remove_dir_all(fixture.checkout.join("target")).unwrap();
    assert_eq!(
        target_state(&fixture.checkout, &context.checkout_id).unwrap(),
        TargetState::Missing
    );
    assert_eq!(derived_exec(&fixture), ["debug", "release"]);
    let declaration = exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap();
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    assert!(
        plan.members.iter().all(|member| member.selected.is_some()),
        "the derived plan did not key the owned members"
    );
    let store = pool_store(&fixture.root.join("pool"));
    let outcome = match bind_profile(&context, &store, 0, &committed, Some(&plan), true) {
        Bind::Bound { outcome, .. } => outcome,
        Bind::Refuse(reason) => panic!("a deleted target directory refused: {reason}"),
        Bind::Wait => panic!("a deleted target directory waited"),
    };
    assert_eq!(outcome.root.as_deref(), Some(root.as_path()));
    let target = fixture.checkout.join("target");
    let metadata = fs::symlink_metadata(&target).unwrap();
    assert!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "the binding did not recreate a real target directory"
    );
    for member in ["debug", "release"] {
        assert_eq!(
            fs::read_link(target.join(member)).unwrap(),
            member_destination(&root, member),
            "{member}"
        );
        assert!(
            root.join(member).is_dir(),
            "{member}: the member directory is not real"
        );
    }

    // Control: with the root's marker gone the root is not verified, so the
    // absent links still refuse and nothing is created.
    let unverified = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    register(&unverified, DriveMode::Permanent);
    let unverified_context = unverified.context();
    let unverified_receipt = Receipt::first_with(
        &unverified.checkout,
        &unverified_context.checkout_id,
        LayoutKind::Profile,
    );
    let (_committed, unverified_selected, _) =
        adopt_profile(&unverified_context, unverified_receipt).unwrap();
    fs::remove_dir_all(unverified.checkout.join("target")).unwrap();
    fs::remove_file(unverified_selected.root().join(MARKER)).unwrap();
    let refused =
        exec_declaration(Some(&unverified.control), &unverified.checkout, None).unwrap_err();
    assert!(refused.contains("target/debug"), "{refused}");
    assert!(refused.contains("oxide outputs adopt"), "{refused}");
    assert_eq!(
        target_state(&unverified.checkout, &unverified_context.checkout_id).unwrap(),
        TargetState::Missing,
        "a refused derived exec created the target directory"
    );
}

/// The same shape with a keyed plan whose nonce the root does not carry: a
/// different root nonce is a contradiction, so nothing is created and the
/// link stays dangling. The receipt and the link are unchanged.
#[test]
fn exec_creates_no_member_directory_for_a_root_with_another_nonce() {
    let fixture = Fixture::new();
    register(&fixture, DriveMode::Permanent);
    let (context, receipt, root) = placed_profile(&fixture, &["debug", "release"], &[]);
    fs::remove_dir(root.join("release")).unwrap();
    let declaration = exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap();
    let mut plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    for member in &mut plan.members {
        if let Some(selected) = &mut member.selected {
            selected.nonce = Some("0123456789abcdef0123456789abcdef".to_owned());
        }
    }
    let store = pool_store(&fixture.root.join("pool"));
    let refused = match bind_profile(&context, &store, 0, &receipt, Some(&plan), true) {
        Bind::Refuse(reason) => reason.to_string(),
        Bind::Bound { .. } => panic!("another root nonce was accepted"),
        Bind::Wait => panic!("another root nonce waited"),
    };
    assert!(refused.contains("nonce"), "{refused}");
    assert!(!root.join("release").exists());
    assert!(
        fs::symlink_metadata(fixture.checkout.join("target/release"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

/// A link into an unverified root, or into a root that is gone, is still
/// refused by the derived `exec` before Cargo, and nothing is created.
#[test]
fn exec_still_refuses_an_absent_member_of_an_unverified_or_missing_root() {
    // An unmarked directory is not this checkout's root: the link text
    // names its own absent `release`, and no directory is created there.
    let unverified = Fixture::new();
    register(&unverified, DriveMode::Permanent);
    let (unverified_context, _receipt, _root) =
        placed_profile(&unverified, &["debug", "release"], &[]);
    let outside = unverified.root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    let link = unverified.checkout.join("target/release");
    fs::remove_file(&link).unwrap();
    symlink(outside.join("release"), &link).unwrap();
    let refused =
        exec_declaration(Some(&unverified.control), &unverified.checkout, None).unwrap_err();
    assert!(refused.contains("target/release"), "{refused}");
    assert!(refused.contains("oxide outputs adopt"), "{refused}");
    assert!(!outside.join("release").exists());
    assert_eq!(fs::read_link(&link).unwrap(), outside.join("release"));
    let _ = unverified_context;
    // A root that is gone entirely: the link names its own destination,
    // but the root no longer passes the bound check, so nothing is made.
    let missing = Fixture::new();
    register(&missing, DriveMode::Permanent);
    let (missing_context, _receipt, missing_root) =
        placed_profile(&missing, &["debug", "release"], &[]);
    fs::remove_dir(missing_root.join("release")).unwrap();
    fs::rename(&missing_root, missing.root.join("moved-root")).unwrap();
    let refused = exec_declaration(Some(&missing.control), &missing.checkout, None).unwrap_err();
    assert!(refused.contains("oxide outputs adopt"), "{refused}");
    assert!(!missing_root.exists());
    assert_eq!(
        fs::read_link(missing.checkout.join("target/release")).unwrap(),
        missing_root.join("release")
    );
    let _ = missing_context;
}

/// S1 counterfactual 9: more receipt members than one command binds refuse,
/// naming the explicit detach.
#[test]
fn exec_refuses_a_checkout_placing_more_members_than_one_command_binds() {
    let fixture = Fixture::new();
    let names = (0..9)
        .map(|index| format!("t{index}/debug"))
        .collect::<Vec<_>>();
    let borrowed = names.iter().map(String::as_str).collect::<Vec<_>>();
    placed_profile(&fixture, &borrowed, &[]);
    let refused = exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap_err();
    assert!(refused.contains("oxide outputs detach"), "{refused}");
    assert!(refused.contains('9'), "{refused}");
    // The explicit override still declares a bounded subset.
    let declared = crate::layout::declared(&["t0/debug".to_owned()]).unwrap();
    assert_eq!(declared.members().len(), 1);
}

/// S1 counterfactuals 11 and 12: an explicit `--output` set stays complete
/// and may name a member the receipt does not place, while a checkout path
/// containing a space derives exactly the same set.
#[test]
fn exec_declarations_stay_complete_and_derivation_ignores_the_checkout_path() {
    let fixture = Fixture::new();
    let (_, _receipt, root) = placed_profile(&fixture, &["debug"], &[]);
    // The derived set never invents the unplaced member.
    assert_eq!(derived_exec(&fixture), ["debug"]);
    // A declared override is still complete: it names the member the
    // receipt does not place and plans the owned root for it.
    let declared = crate::layout::declared(&["release".to_owned()]).unwrap();
    let plan = planned_destination(Some(&fixture.control), &fixture.checkout, &declared, None)
        .unwrap()
        .unwrap();
    assert_eq!(plan.members.len(), 1);
    assert_eq!(plan.selected().map(Selected::root), Some(root.as_path()));
    // A checkout path with a space changes nothing: member names carry no
    // checkout path.
    let spaced = Fixture::in_tree("check out", "/target\n.oxide-outputs/\n");
    placed_profile(&spaced, &["debug", "release"], &[]);
    assert_eq!(derived_exec(&spaced), ["debug", "release"]);
    assert_eq!(
        destination_facts(
            planned_destination(
                Some(&spaced.control),
                &spaced.checkout,
                &exec_declaration(Some(&spaced.control), &spaced.checkout, None).unwrap(),
                None
            )
            .unwrap()
            .as_ref()
        )
        .lines()
        .count(),
        2
    );
}

/// oxide#164: an external subcommand (`cargo-<name>` run by Cargo) uses the
/// SAME derivation as a derived `oxide exec`, so it derives the receipt's
/// member set and renders a byte-identical slot key.
///
/// Counterfactual: restoring the pre-#164 rule — derive only for
/// `exec`, and call `refuse_unknown_layout` for every other unmodelled
/// command — makes `lease_declaration` return an error here instead of a
/// declaration, so this test fails.
#[test]
fn an_external_subcommand_derives_the_receipt_pair_like_exec() {
    let fixture = Fixture::new();
    placed_profile(&fixture, &["debug", "release"], &[]);
    // The real parser, so the routing input is the one the CLI produces:
    // an external subcommand's argv is untouched and it declares nothing.
    let external = crate::cli::parse_leased("oxfixture", &[OsString::from("run")]).unwrap();
    assert_eq!(
        external.cargo_args,
        [OsString::from("oxfixture"), OsString::from("run")]
    );
    assert!(external.declared_outputs.is_empty());
    assert!(crate::cli::derives_from_receipt(
        &external.command,
        &external.declared_outputs
    ));
    let declaration =
        lease_declaration(&external, Some(&fixture.control), &fixture.checkout, None).unwrap();
    let names: Vec<&str> = declaration
        .members()
        .iter()
        .map(crate::layout::Member::name)
        .collect();
    assert_eq!(names, ["debug", "release"]);
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap();
    let facts = destination_facts(plan.as_ref());
    assert_eq!(facts.lines().count(), 2, "{facts}");
    assert!(
        facts.lines().all(|line| line.starts_with("outputs-3 ")),
        "{facts}"
    );
    // One derivation, one key: the shipped `exec` path renders the same
    // bytes for the same checkout.
    let exec = exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap();
    let exec_plan =
        planned_destination(Some(&fixture.control), &fixture.checkout, &exec, None).unwrap();
    assert_eq!(destination_facts(exec_plan.as_ref()), facts);
    // The old refusal still exists for a classification this layout cannot
    // model, and it would have refused this profile checkout — which is
    // exactly why the routing no longer calls it here.
    assert!(refuse_unknown_layout(
        Some(&fixture.control),
        &fixture.checkout,
        "`oxfixture` is not a command whose profile outputs Oxide models",
    )
    .is_err());
}

/// oxide#164: a whole-target receipt and a never-converted checkout are
/// unchanged for an external subcommand: the derivation adds nothing, the
/// external command plans exactly the legacy whole-target key a modelled
/// command plans, and an unconverted checkout plans no destination at all.
#[test]
fn an_external_subcommand_is_unchanged_for_whole_target_and_unconverted_checkouts() {
    let external = crate::cli::parse_leased("oxfixture", &[]).unwrap();
    // Never converted: no declaration, no destination, no keyed member.
    let plain = Fixture::new();
    assert!(
        lease_declaration(&external, Some(&plain.control), &plain.checkout, None)
            .unwrap()
            .is_empty()
    );
    assert!(planned_destination(
        Some(&plain.control),
        &plain.checkout,
        &crate::layout::Declaration::empty(),
        None
    )
    .unwrap()
    .is_none());
    // A committed whole-target receipt: still empty, so the legacy plan
    // and its `outputs-2` key are exactly a modelled command's.
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
    assert_eq!(committed.layout(), LayoutKind::WholeTarget);
    let declaration =
        lease_declaration(&external, Some(&fixture.control), &fixture.checkout, None).unwrap();
    assert!(declaration.is_empty());
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    assert!(plan.whole_target);
    let facts = destination_facts(Some(&plan));
    assert!(
        facts.lines().all(|line| line.starts_with("outputs-2 ")),
        "{facts}"
    );
    let modelled = declaration_of(&["debug".to_owned()]);
    let modelled_plan =
        planned_destination(Some(&fixture.control), &fixture.checkout, &modelled, None).unwrap();
    assert_eq!(destination_facts(modelled_plan.as_ref()), facts);
}

/// oxide#164: the #162 absent-member repair is a property of the derivation,
/// not of the command name, so an external subcommand in the same shape
/// creates the one absent member directory under its verified root.
#[test]
fn an_external_subcommand_creates_an_absent_member_of_the_verified_owned_root() {
    let fixture = Fixture::new();
    register(&fixture, DriveMode::Permanent);
    let (context, receipt, root) = placed_profile(&fixture, &["debug", "release"], &[]);
    fs::remove_dir(root.join("release")).unwrap();
    let external = crate::cli::parse_leased("oxfixture", &[]).unwrap();
    let repair = crate::cli::derives_from_receipt(&external.command, &external.declared_outputs);
    assert!(
        repair,
        "an external subcommand is not routed through the derivation"
    );
    let declaration =
        lease_declaration(&external, Some(&fixture.control), &fixture.checkout, None).unwrap();
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    assert!(destination_facts(Some(&plan)).contains("release"));
    let store = pool_store(&fixture.root.join("pool"));
    match bind_profile(&context, &store, 0, &receipt, Some(&plan), repair) {
        Bind::Bound { outcome, .. } => {
            assert_eq!(outcome.root.as_deref(), Some(root.as_path()))
        }
        Bind::Refuse(reason) => {
            let reason = reason.to_string();
            panic!("an absent member of the verified root refused: {reason}")
        }
        Bind::Wait => panic!("an absent member of the verified root waited"),
    }
    assert!(root.join("release").is_dir());
    assert_eq!(
        fs::read_link(fixture.checkout.join("target/release")).unwrap(),
        root.join("release")
    );
}

/// oxide#164: every derivation refusal is preserved for an external
/// subcommand — a foreign or unowned member link, a member-shaped file and
/// a receipt over the member cap all refuse before Cargo, with nothing
/// changed.
#[test]
fn an_external_subcommand_keeps_the_derivation_refusals() {
    let external = crate::cli::parse_leased("oxfixture", &[]).unwrap();
    // A foreign member link into another destination.
    let foreign = Fixture::new();
    let (_context, _receipt, _root) = placed_profile(&foreign, &["debug"], &[]);
    let outside = foreign.root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    let link = foreign.checkout.join("target/debug");
    fs::remove_file(&link).unwrap();
    symlink(&outside, &link).unwrap();
    let refused =
        lease_declaration(&external, Some(&foreign.control), &foreign.checkout, None).unwrap_err();
    assert!(refused.contains("target/debug"), "{refused}");
    assert!(refused.contains("oxide outputs adopt"), "{refused}");
    assert_eq!(fs::read_link(&link).unwrap(), outside);
    // A member-shaped regular file at a receipt-recorded member path.
    let filed = Fixture::new();
    let (_context, _receipt, _root) = placed_profile(&filed, &["debug"], &[]);
    fs::write(filed.checkout.join("target/release"), b"file").unwrap();
    let refused =
        lease_declaration(&external, Some(&filed.control), &filed.checkout, None).unwrap_err();
    assert!(refused.contains("target/release"), "{refused}");
    assert_eq!(
        fs::read(filed.checkout.join("target/release")).unwrap(),
        b"file"
    );
    // More receipt members than one command binds.
    let capped = Fixture::new();
    let names = (0..9)
        .map(|index| format!("t{index}/debug"))
        .collect::<Vec<_>>();
    let borrowed = names.iter().map(String::as_str).collect::<Vec<_>>();
    placed_profile(&capped, &borrowed, &[]);
    let refused =
        lease_declaration(&external, Some(&capped.control), &capped.checkout, None).unwrap_err();
    assert!(refused.contains("oxide outputs detach"), "{refused}");
    assert!(refused.contains('9'), "{refused}");
}

/// oxide#164: the tool-directory notice is bounded and precise. It names
/// only directories that APPEARED directly under a real target, never a
/// profile member, never Cargo's own `tmp`, never a file, and never through
/// a linked target; its list is capped.
#[test]
fn the_tool_directory_notice_names_only_new_non_member_directories() {
    let fixture = Fixture::new();
    placed_profile(&fixture, &["debug"], &[]);
    let before = target_entries(&fixture.checkout).unwrap();
    // Nothing new: no notice at all.
    assert!(tool_directories_note(&fixture.checkout, Some(&before)).is_none());
    // A tool's own directory is named, once.
    fs::create_dir(fixture.checkout.join("target/nextest")).unwrap();
    let note = tool_directories_note(&fixture.checkout, Some(&before)).unwrap();
    assert!(note.contains("(nextest)"), "{note}");
    assert_eq!(note.matches("nextest").count(), 1, "{note}");
    // Cargo's own `tmp`, a member-shaped directory, and a file are not
    // this notice's business.
    fs::create_dir(fixture.checkout.join("target/tmp")).unwrap();
    fs::create_dir(fixture.checkout.join("target/release")).unwrap();
    fs::write(fixture.checkout.join("target/.rustc_info.json"), b"{}").unwrap();
    let note = tool_directories_note(&fixture.checkout, Some(&before)).unwrap();
    assert!(note.contains("(nextest)"), "{note}");
    assert!(!note.contains("tmp"), "{note}");
    assert!(!note.contains("release"), "{note}");
    assert!(!note.contains("rustc_info"), "{note}");
    // The printed list is capped at eight names plus a bounded remainder.
    for index in 0..9 {
        fs::create_dir(fixture.checkout.join(format!("target/tool{index}"))).unwrap();
    }
    let note = tool_directories_note(&fixture.checkout, Some(&before)).unwrap();
    assert!(note.contains("and 2 more"), "{note}");
    // A target that is a link (the whole-target layout) is not observed.
    let linked = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let outside = linked.root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, linked.checkout.join("target")).unwrap();
    assert!(target_entries(&linked.checkout).is_none());
    assert!(tool_directories_note(&linked.checkout, Some(&[])).is_none());
    // An absent target is not observed either, and no observation means no
    // notice.
    assert!(target_entries(&linked.root.join("absent")).is_none());
    assert!(tool_directories_note(&fixture.checkout, None).is_none());
}

/// oxide#164 review: the tool-directory notice belongs to the
/// receipt-derived PROFILE-MEMBER layout only. An unconverted, ephemeral,
/// whole-target or detached checkout plans nothing (or the whole target), a
/// modelled command and an explicit `--output` declaration are not derived,
/// so none of them may gain the extra line: their shipped stderr stays.
#[test]
fn the_tool_directory_notice_applies_only_to_a_derived_profile_plan() {
    let fixture = Fixture::new();
    let selected = fixture.local().selected(1);
    let owned = Plan {
        whole_target: false,
        members: vec![PlannedMember {
            member: "debug".to_owned(),
            selected: Some(selected),
        }],
    };
    let real = Plan {
        whole_target: false,
        members: vec![PlannedMember {
            member: "release".to_owned(),
            selected: None,
        }],
    };
    let whole = Plan {
        whole_target: true,
        members: vec![PlannedMember {
            member: MEMBER.to_owned(),
            selected: Some(fixture.local().selected(1)),
        }],
    };
    let empty = Plan {
        whole_target: false,
        members: Vec::new(),
    };
    // The profile layout, owned or receipt-recorded real.
    assert!(tool_notice_applies(true, Some(&owned)));
    assert!(tool_notice_applies(true, Some(&real)));
    // Unconverted, ephemeral and detached checkouts plan nothing.
    assert!(!tool_notice_applies(true, None));
    // A legacy whole-target checkout plans the whole target.
    assert!(!tool_notice_applies(true, Some(&whole)));
    // An explicit `--output` declaration and every modelled command are not
    // derived sets.
    assert!(!tool_notice_applies(false, Some(&owned)));
    // A plan with no member places nothing to report against.
    assert!(!tool_notice_applies(true, Some(&empty)));
}

/// S1 counterfactual 14: member names are matched before triple descent, so
/// a real or linked `target/debug` is never enumerated as a triple ancestor
/// and its payload is never listed.
#[test]
fn a_real_member_directory_is_never_treated_as_a_triple_ancestor() {
    let fixture = Fixture::new();
    placed_profile(&fixture, &["release"], &[]);
    let debug = fixture.checkout.join("target/debug");
    fs::create_dir(&debug).unwrap();
    fs::write(debug.join("profileapp"), b"payload").unwrap();
    fs::create_dir(debug.join("debug")).unwrap();
    fs::create_dir(debug.join("release")).unwrap();
    let members = profile_members(&fixture.checkout.join("target")).unwrap();
    assert!(members.contains(&"debug".to_owned()), "{members:?}");
    assert!(
        !members
            .iter()
            .any(|name| name == "debug/debug" || name == "debug/release"),
        "{members:?}"
    );
    // The derived set neither lists the payload as a member nor plans it.
    assert_eq!(derived_exec(&fixture), ["release"]);
    let declaration = exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap();
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    let facts = destination_facts(Some(&plan));
    assert!(!facts.contains("debug/debug"), "{facts}");
    assert!(!facts.contains("debug/release"), "{facts}");
}

#[test]
fn whole_target_run_maps_only_the_verified_bound_root() {
    let (fixture, receipt) = legacy_checkout();
    let root = receipt.selected().unwrap().root().to_path_buf();
    let target = fixture.checkout.join("target");
    let program = target.join("debug/app");
    let mapped = root.join("debug/app");
    fs::create_dir_all(mapped.parent().unwrap()).unwrap();
    fs::write(&mapped, b"program").unwrap();
    assert_eq!(
        program_target(&fixture.checkout, &root, &program),
        Some(mapped)
    );

    let foreign = fixture.root.join("foreign");
    fs::create_dir_all(foreign.join("debug")).unwrap();
    fs::write(foreign.join("debug/app"), b"foreign").unwrap();
    assert_eq!(program_target(&fixture.checkout, &foreign, &program), None);
    let marker = root.join(MARKER);
    let marker_bytes = fs::read(&marker).unwrap();
    fs::write(&marker, b"unverified").unwrap();
    assert_eq!(program_target(&fixture.checkout, &root, &program), None);
    fs::write(&marker, marker_bytes).unwrap();
    fs::remove_file(&target).unwrap();
    symlink(&foreign, &target).unwrap();
    assert_eq!(program_target(&fixture.checkout, &root, &program), None);
    fs::remove_file(&target).unwrap();
    fs::create_dir_all(target.join("debug")).unwrap();
    fs::write(&program, b"real").unwrap();
    assert_eq!(program_target(&fixture.checkout, &root, &program), None);
}

#[test]
fn profile_members_select_distinct_destinations_and_run_maps_only_owned_members() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    let receipt = Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    let (_, selected, _) = adopt_profile(&context, receipt).unwrap();
    let root = selected.root().to_path_buf();
    // Each member link selects its own approved subpath, never the root.
    for member in ["debug", "release"] {
        let link = context.link().join(member);
        assert_eq!(
            fs::read_link(&link).unwrap(),
            member_destination(&root, member),
            "{member}"
        );
        assert!(root.join(member).is_dir(), "{member}: no member directory");
    }
    // `run` maps a program beneath its own owned member only.
    let owned = root.join("debug").join("profileapp");
    fs::write(&owned, b"program").unwrap();
    let mapped = program_target(
        &fixture.checkout,
        &root,
        &context.link().join("debug").join("profileapp"),
    );
    assert_eq!(mapped.as_deref(), Some(owned.as_path()));
    // A real member keeps Cargo's path; it never borrows another member's
    // root.
    let real = fixture.checkout.join("target/triple/debug/app");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(&real, b"real").unwrap();
    assert_eq!(
        program_target(&fixture.checkout, &root, &real),
        None,
        "a real member program must keep Cargo's path"
    );
    // Non-normal components (`..`) must be rejected on profile members
    // just as on whole-target layouts, even when the target file exists.
    let escaped = root.join("outside-app");
    fs::write(&escaped, b"escaped").unwrap();
    for bad in [
        context.link().join("debug").join("..").join("outside-app"),
        context
            .link()
            .join("triple")
            .join("debug")
            .join("..")
            .join("..")
            .join("outside-app"),
        context
            .link()
            .join("debug")
            .join("..")
            .join("debug")
            .join("profileapp"),
    ] {
        assert_eq!(
            program_target(&fixture.checkout, &root, &bad),
            None,
            "non-normal path {} must be refused",
            bad.display()
        );
    }
    assert!(owned.is_file());
}

/// refuses the keyed plan with no nonce.
///
/// Counterfactual: skipping the reservation for every derived command (the
/// oxide#164 head `87d9ff1`) makes this test's binding refuse.
#[test]
fn an_external_subcommand_reserves_a_future_whole_target_root() {
    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let (context, _external) = committed_whole_target_external(&fixture);
    let external = crate::cli::parse_leased("oxfixture", &[]).unwrap();
    let declaration =
        lease_declaration(&external, Some(&fixture.control), &fixture.checkout, None).unwrap();
    assert!(
        declaration.is_empty(),
        "a whole-target checkout derived a member set"
    );
    // The decision main.rs makes: an empty derived set keeps the shipped
    // reservation path, a non-empty derived set skips it.
    assert!(needs_reservation(true, &declaration));
    assert!(!needs_reservation(
        true,
        &declaration_of(&["debug".to_owned()])
    ));
    assert!(needs_reservation(false, &declaration));
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
        Some("the registered volume is away"),
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
        "the external subcommand's future whole-target root has no reservation"
    );
    match bind_under_binding(&context, &store, 0, &reserved, Some(&plan), false) {
        Bind::Bound { outcome, .. } => assert_eq!(
            outcome.root.as_deref(),
            Some(fixture.local().root()),
            "the whole-target fallback did not bind the local root"
        ),
        Bind::Refuse(reason) => {
            let reason = reason.to_string();
            panic!("the external subcommand's future whole-target root refused: {reason}")
        }
        Bind::Wait => panic!("the external subcommand's future whole-target root waited"),
    }
}
