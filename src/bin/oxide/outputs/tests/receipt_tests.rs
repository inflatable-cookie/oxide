use super::*;
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt};

#[test]
fn receipts_reject_unknown_versions_fields_and_generations() {
    let fixture = Fixture::new();
    let mut receipt = fixture.receipt();
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt.link_mut().selected = Some(fixture.local().selected(1));
    receipt.roots = vec![fixture.local().selected(1)];
    let id = fixture.context().checkout_id.clone();
    write_receipt(&fixture.control, &receipt).unwrap();
    assert_eq!(
        read_receipt(&fixture.control, &fixture.checkout, &id)
            .unwrap()
            .unwrap(),
        receipt
    );

    let path = receipt_path(&fixture.control, &id);
    let mut unknown = serde_json::to_value(&receipt).unwrap();
    unknown["version"] = serde_json::Value::String("oxide.outputs.transition/9".to_owned());
    fs::write(&path, serde_json::to_vec(&unknown).unwrap()).unwrap();
    let refused = read_receipt(&fixture.control, &fixture.checkout, &id).unwrap_err();
    assert!(refused.contains("unsupported version"), "{refused}");

    let mut extra = serde_json::to_value(&receipt).unwrap();
    extra["surprise"] = serde_json::Value::Bool(true);
    fs::write(&path, serde_json::to_vec(&extra).unwrap()).unwrap();
    assert!(read_receipt(&fixture.control, &fixture.checkout, &id).is_err());

    let mut ahead = receipt.clone();
    ahead.roots = vec![fixture.local().selected(3)];
    ahead.link_mut().selected = None;
    ahead.state = ReceiptState::Detached;
    fs::write(&path, serde_json::to_vec(&ahead).unwrap()).unwrap();
    assert!(read_receipt(&fixture.control, &fixture.checkout, &id).is_err());

    let other = Fixture::new();
    let refused = read_receipt(&fixture.control, &other.checkout, &id).unwrap_err();
    assert!(refused.contains("another checkout"), "{refused}");
    let other_id = identity::digest(&other.checkout.to_string_lossy());
    assert!(read_receipt(&fixture.control, &other.checkout, &other_id)
        .unwrap()
        .is_none());
}

/// A document that uses a successor-only surface under the version-1 label
/// is contradictory, never "old": both readers refuse it.
#[test]
fn a_contradictory_version_one_receipt_is_refused_by_both_readers() {
    let fixture = Fixture::new();
    let root = fixture.local().root().to_path_buf();
    let checkout = fixture.checkout.clone();
    let id = context_id(&fixture);
    let base = version_one_receipt(&fixture, "committed", Some(&root));
    let mut cases: Vec<(&str, serde_json::Value)> = Vec::new();
    for (label, key, value) in [
        ("layout", "layout", serde_json::json!("profile")),
        (
            "links",
            "links",
            serde_json::json!([{"member": "target", "selected": {"kind": "local", "root": root, "generation": 1}}]),
        ),
        (
            "reservation",
            "reserved",
            serde_json::json!({
                "id": "5".repeat(32),
                "nonce": "6".repeat(32),
                "kind": "local",
                "root": root,
                "generation": 2,
                "created_ms": 1,
            }),
        ),
        (
            "prune record",
            "pruned",
            serde_json::json!({"intent": "7".repeat(32), "outcome": "deleted", "at_ms": 1}),
        ),
        ("unknown field", "surprise", serde_json::json!(true)),
    ] {
        let mut document = base.clone();
        document[key] = value;
        cases.push((label, document));
    }
    for (label, mut document) in [
        ("pruned state", base.clone()),
        ("pruning state", base.clone()),
    ] {
        let state = if label == "pruned state" {
            "pruned"
        } else {
            "pruning"
        };
        document["state"] = serde_json::json!(state);
        cases.push((label, document));
    }
    let mut nonce = base.clone();
    nonce["selected"]["nonce"] = serde_json::json!("8".repeat(32));
    cases.push(("fabricated root nonce", nonce));
    let mut unknown = base.clone();
    unknown["version"] = serde_json::json!("oxide.outputs.transition/9");
    cases.push(("unknown version", unknown));
    for (label, document) in cases {
        write_receipt_document(&fixture, &document);
        let refused = read_receipt(&fixture.control, &checkout, &id).unwrap_err();
        assert!(refused.contains("is invalid"), "{label}: {refused}");
        let refused =
            read_receipt_with(LegacyReceiptReading::Read, &fixture.control, &checkout, &id)
                .unwrap_err();
        assert!(refused.contains("is invalid"), "{label}: {refused}");
    }
}

/// The real run's terminal image for an actually pruned version-1 root is
/// the current receipt shape; it is deterministic, pinned by the caller's
/// digest, and published only through the existing terminal transaction.

/// A committed profile receipt for `debug` and `release` on one root.

#[test]
fn profile_receipts_place_members_independently_but_always_select_one() {
    let fixture = Fixture::new();
    let id = identity::digest(&fixture.checkout.to_string_lossy());
    let mut profile = profile_receipt(&fixture);
    profile.place_member("release").selected = None;
    profile
        .validate(&fixture.checkout, &id)
        .expect("a profile receipt with one unselected member is coherent");
    let mut unselected = profile.clone();
    unselected.place_member("debug").selected = None;
    assert!(
        unselected.validate(&fixture.checkout, &id).is_err(),
        "a profile receipt that selects nothing was accepted"
    );
    // A whole-target receipt still requires its one placement selected.
    let mut whole = fixture.receipt();
    whole.state = ReceiptState::Committed;
    whole.generation = 1;
    whole.link_mut().selected = Some(fixture.local().selected(1));
    whole.validate(&fixture.checkout, &id).unwrap();
    whole.link_mut().selected = None;
    assert!(
        whole.validate(&fixture.checkout, &id).is_err(),
        "a whole-target receipt with no selection was accepted"
    );
}

#[test]
fn profile_destinations_separate_linked_and_real_members() {
    let fixture = Fixture::new();
    let mut receipt = Receipt::first_with(
        &fixture.checkout,
        &identity::digest(&fixture.checkout.to_string_lossy()),
        LayoutKind::Profile,
    );
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt.place_member("debug").selected = Some(
        fixture
            .local()
            .selected(1)
            .with_nonce(Some("77777777777777777777777777777777".to_owned())),
    );
    write_receipt(&fixture.control, &receipt).unwrap();
    // `release` is a real, nonempty profile directory: it stays local.
    fs::create_dir(fixture.checkout.join("target")).unwrap();
    fs::create_dir(fixture.checkout.join("target/release")).unwrap();
    fs::write(fixture.checkout.join("target/release/keep"), b"x").unwrap();
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
    let facts = destination_facts(Some(&plan));
    assert!(facts.contains(" debug"), "{facts}");
    assert!(facts.contains("real release"), "{facts}");
    // A layout in which `release` is linked as well keys differently, so a
    // linked and a real member never share a warm slot.
    fs::remove_file(fixture.checkout.join("target/release/keep")).unwrap();
    fs::remove_dir(fixture.checkout.join("target/release")).unwrap();
    let linked = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    assert_ne!(destination_facts(Some(&linked)), facts);
    // A profile checkout refuses a command that declares no members.
    let refused = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &crate::layout::Declaration::empty(),
        None,
    )
    .unwrap_err();
    assert!(refused.contains("declares none"), "{refused}");
}

/// P5: the profile members are discovered with one bounded second
/// component. Only a positively identified plain triple contributes its own
/// `debug`/`release` directories; a linked triple directory, a JSON target
/// specification, a custom profile and any other root entry are never
/// classified as members.
#[test]
fn profile_members_discover_only_bounded_triple_pairs() {
    let fixture = Fixture::new();
    let root = fixture.root.join("volume/oxide-outputs/checkout-0000");
    for directory in [
        "debug",
        "release",
        "aarch64-apple-darwin/debug",
        "aarch64-apple-darwin/release",
        // A custom profile under a real triple is not a v1 member.
        "aarch64-apple-darwin/bench",
        // Three components are outside the bounded traversal.
        "aarch64-apple-darwin/debug/incremental",
        // A JSON target specification is not a plain triple.
        "x86_64-unknown-linux-gnu.json/debug",
        // A profile name no v1 layout links.
        "custom-profile",
    ] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    // A plain triple that is a SYMLINK is not positively identified: it is
    // never descended into.
    fs::create_dir_all(fixture.root.join("elsewhere/riscv64-unknown-elf/debug")).unwrap();
    symlink(
        fixture.root.join("elsewhere"),
        root.join("riscv64-unknown-elf"),
    )
    .unwrap();
    fs::write(root.join("CACHEDIR.TAG"), b"signature").unwrap();
    assert_eq!(
        profile_members(&root).unwrap(),
        vec![
            "aarch64-apple-darwin/debug".to_owned(),
            "aarch64-apple-darwin/release".to_owned(),
            "debug".to_owned(),
            "release".to_owned(),
        ]
    );
}

#[test]
fn a_profile_receipt_round_trips_in_its_own_version() {
    let fixture = Fixture::new();
    let mut receipt = profile_receipt(&fixture);
    receipt.reserved = None;
    let bytes = serde_json::to_vec(&receipt).unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["version"], PROFILE_RECEIPT_VERSION);
    assert_eq!(doc["layout"], "profile");
    assert_eq!(doc["links"][0]["member"], "debug");
    assert_eq!(doc["links"][1]["member"], "release");
    let back: Receipt = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(back, receipt);
    assert_eq!(back.layout(), LayoutKind::Profile);
    assert_eq!(
        back.selected_for("debug").map(Selected::root),
        Some(fixture.local().root())
    );
    // A version-3 receipt without its layout, or with a per-placement
    // reservation, is refused rather than guessed.
    let mut missing = doc.clone();
    missing.as_object_mut().unwrap().remove("layout");
    assert!(serde_json::from_value::<Receipt>(missing).is_err());
    let mut misplaced = doc;
    misplaced["links"][0]["reserved"] = serde_json::json!({
        "id": "33333333333333333333333333333333",
        "nonce": "44444444444444444444444444444444",
        "kind": "local",
        "root": "/nowhere",
        "generation": 2,
        "created_ms": 1,
    });
    assert!(serde_json::from_value::<Receipt>(misplaced).is_err());
}

#[test]
fn a_legacy_receipt_keeps_its_published_shape_and_reservation() {
    let fixture = Fixture::new();
    let mut receipt = Receipt::first(
        &fixture.checkout,
        &identity::digest(&fixture.checkout.to_string_lossy()),
    );
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt.link_mut().selected = Some(fixture.local().selected(1));
    receipt.reserved = Some(Reservation {
        id: "55555555555555555555555555555555".to_owned(),
        nonce: "66666666666666666666666666666666".to_owned(),
        kind: RootKind::Local,
        root: fixture.local().root().to_path_buf(),
        volume_uuid: None,
        generation: 2,
        created_ms: 1,
    });
    let doc = serde_json::to_value(&receipt).unwrap();
    assert_eq!(doc["version"], RECEIPT_VERSION);
    assert!(doc.get("layout").is_none());
    assert!(doc.get("reserved").is_none());
    assert_eq!(
        doc["links"][0]["reserved"]["id"],
        "55555555555555555555555555555555"
    );
    let back: Receipt = serde_json::from_value(doc).unwrap();
    assert_eq!(back, receipt);
    assert_eq!(back.layout(), LayoutKind::WholeTarget);
}

#[test]
fn a_profile_receipt_refuses_unsorted_overlapping_or_foreign_members() {
    let fixture = Fixture::new();
    let id = identity::digest(&fixture.checkout.to_string_lossy());
    let mut receipt = profile_receipt(&fixture);
    receipt.links.reverse();
    assert!(receipt.validate(&fixture.checkout, &id).is_err());
    let mut receipt = profile_receipt(&fixture);
    let duplicate = receipt.links[0].clone();
    receipt.links.insert(1, duplicate);
    assert!(receipt.validate(&fixture.checkout, &id).is_err());
    let mut receipt = profile_receipt(&fixture);
    receipt.place_member("debug/debug");
    assert!(receipt.validate(&fixture.checkout, &id).is_err());
    let mut receipt = profile_receipt(&fixture);
    receipt.links.push(LinkPlacement {
        member: "../../escape".to_owned(),
        selected: None,
        prepared: None,
    });
    receipt
        .links
        .sort_by(|left, right| left.member.cmp(&right.member));
    assert!(receipt.validate(&fixture.checkout, &id).is_err());
}

#[test]
fn a_prepared_profile_placement_keeps_the_others_committed() {
    let fixture = Fixture::new();
    let id = identity::digest(&fixture.checkout.to_string_lossy());
    let mut receipt = profile_receipt(&fixture);
    let link = link_path(&fixture.checkout, "release");
    receipt.generation = 1;
    receipt.state = ReceiptState::Prepared;
    receipt.place_member("release").prepared = Some(Prepared {
        generation: 2,
        to: fixture.local().selected(2),
        link: link.clone(),
    });
    receipt.validate(&fixture.checkout, &id).unwrap();
    assert_eq!(receipt.prepared_ref().map(|p| p.link.clone()), Some(link));
    // A prepared transition on a link no placement owns is unusable.
    let mut orphan = receipt.clone();
    orphan.place_member("release").prepared = Some(Prepared {
        generation: 2,
        to: fixture.local().selected(2),
        link: fixture.checkout.join("target/elsewhere"),
    });
    assert!(orphan.validate(&fixture.checkout, &id).is_err());
}

#[test]
fn a_receipt_write_publishes_registry_rows_and_prepared_keeps_selection() {
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
    let selected = committed.selected().unwrap().clone();
    let rows = crate::prune::test_rows(&fixture.control).unwrap();
    assert!(
        rows.iter().any(|(root, selected_flag, pending)| {
            root == &selected.root && *selected_flag && !*pending
        }),
        "{rows:?}"
    );
    let mut prepared = committed.clone();
    prepared.state = ReceiptState::Prepared;
    prepared.link_mut().prepared = Some(Prepared {
        generation: committed.generation + 1,
        to: RootSpec::Local {
            root: fixture.root.join("next-root"),
        }
        .selected(committed.generation + 1),
        link: context.link(),
    });
    write_receipt(&fixture.control, &prepared).unwrap();
    let rows = crate::prune::test_rows(&fixture.control).unwrap();
    assert!(
        rows.iter()
            .any(|(root, selected_flag, _)| { root == &selected.root && *selected_flag }),
        "the prepared transition dropped the committed selection: {rows:?}"
    );
    assert!(
        rows.iter().any(|(root, selected_flag, pending)| {
            root == &fixture.root.join("next-root") && !*selected_flag && *pending
        }),
        "the proposed root is not pending: {rows:?}"
    );
    // A rolled-back preparation clears the pending reference; the
    // committed root stays selected.
    let mut rolled = committed.clone();
    rolled.link_mut().prepared = None;
    write_receipt(&fixture.control, &rolled).unwrap();
    let rows = crate::prune::test_rows(&fixture.control).unwrap();
    assert!(
        rows.iter().all(|(_, _, pending)| !*pending),
        "a rolled-back preparation left a pending reference: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|(root, selected_flag, _)| { root == &selected.root && *selected_flag }),
        "the rollback dropped the committed selection: {rows:?}"
    );
}

#[test]
fn a_reserved_root_nonce_is_created_exactly_and_cancels_through_its_journal() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let RootSpec::Local { root: future } = fixture.local() else {
        panic!("the fixture's local spec is not local");
    };
    let mut reserved = fixture.receipt();
    let reservation = reservation_for(&future, 1);
    *reserved.reserved_mut() = Some(reservation.clone());
    write_receipt(&fixture.control, &reserved).unwrap();
    crate::prune::validate_reservation(&fixture.control, &reservation).unwrap();
    let rows = crate::prune::test_rows(&fixture.control).unwrap();
    assert!(
        rows.iter()
            .any(|(root, selected, pending)| root == &future && !*selected && *pending),
        "{rows:?}"
    );
    let wanted = fixture
        .local()
        .selected(1)
        .with_nonce(Some(reservation.nonce.clone()));
    reservation_matches(&context, &reserved, &wanted).unwrap();
    let created = transition(
        &context,
        &reserved,
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    assert_eq!(
        read_marker(&future).unwrap().nonce,
        Some(reservation.nonce.clone())
    );
    assert!(
        created.reserved().is_none(),
        "the commit kept its consumed reservation"
    );
    assert!(crate::prune::validate_reservation(&fixture.control, &reservation).is_err());

    // A second, unused reservation is cancelled through its journal; its
    // stale starter refuses and the abandoned row is gone.
    let later = fixture.root.join("later-root");
    let mut held = created.clone();
    let unused = reservation_for(&later, created.generation + 1);
    *held.reserved_mut() = Some(unused.clone());
    write_receipt(&fixture.control, &held).unwrap();
    let store = Store::for_tests(&fixture.root.join("pool"));
    cancel_reservation(&store, &fixture.control, &context, &mut held).unwrap();
    assert!(held.reserved().is_none());
    assert!(crate::prune::validate_reservation(&fixture.control, &unused).is_err());
    let stale = RootSpec::Local {
        root: later.clone(),
    }
    .selected(created.generation + 1)
    .with_nonce(Some(unused.nonce.clone()));
    let refused = reservation_matches(&context, &held, &stale).unwrap_err();
    assert!(refused.contains("cancelled or replaced"), "{refused}");
    let rows = crate::prune::test_rows(&fixture.control).unwrap();
    assert!(rows.iter().all(|(root, ..)| root != &later), "{rows:?}");
    // Dropping a live reservation without its journal is refused.
    let mut dropped = held.clone();
    let orphan = reservation_for(&later, created.generation + 1);
    *dropped.reserved_mut() = Some(orphan);
    write_receipt(&fixture.control, &dropped).unwrap();
    *dropped.reserved_mut() = None;
    let refused = write_receipt(&fixture.control, &dropped).unwrap_err();
    assert!(refused.contains("cancellation journal"), "{refused}");
}

/// The first sorted member can retain an older root while a later member
/// commits the root-level reservation. Exercise actual replay/publication.
#[test]
fn profile_reservation_mixed_roots_finish_and_replay() {
    for (old_member, new_member) in [
        ("aarch64-apple-darwin/release", "debug"),
        ("debug", "release"),
        ("release", "debug"),
        (MEMBER, MEMBER),
    ] {
        let fixture = Fixture::new();
        let context = fixture.context();
        let mut receipt = if new_member == MEMBER {
            fixture.receipt()
        } else {
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile)
        };
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        let old = RootSpec::Local {
            root: fixture.root.join("older-root"),
        }
        .selected(1)
        .with_nonce(Some("a".repeat(32)));
        receipt.place_member(old_member).selected = Some(old.clone());
        receipt.place_member(new_member).selected = Some(old.clone());
        receipt.roots.push(old);
        let future = fixture.local();
        let reserved = publish_reservation(&fixture, &mut receipt, future.root());
        let selected = future.selected(2).with_nonce(Some(reserved.nonce.clone()));
        ensure_root_with(&context, &future, 2, Some(&reserved.nonce)).unwrap();
        ensure_member_directory(future.root(), new_member).unwrap();
        let link = link_path(&fixture.checkout, new_member);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(member_destination(future.root(), new_member), &link).unwrap();
        receipt.state = ReceiptState::Prepared;
        receipt.place_member(new_member).prepared = Some(Prepared {
            generation: 2,
            to: selected.clone(),
            link,
        });
        write_receipt(&fixture.control, &receipt).unwrap();
        let committed = replay(&context, receipt).unwrap();
        assert!(committed.reserved().is_none());
        assert_eq!(committed.selected_for(new_member), Some(&selected));
        assert_eq!(
            read_marker(future.root()).unwrap().nonce.as_deref(),
            Some(reserved.nonce.as_str())
        );
        assert!(crate::prune::validate_reservation(&fixture.control, &reserved).is_err());
        let rows = crate::prune::test_rows(&fixture.control).unwrap();
        assert!(rows
            .iter()
            .any(|(root, selected, pending)| root == future.root() && *selected && !*pending));
        let before = authority_tree(&fixture.control);
        let replayed = replay(&context, committed.clone()).unwrap();
        assert_eq!(replayed, committed);
        assert_eq!(authority_tree(&fixture.control), before);
        let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored, committed);
    }
}

#[test]
fn profile_reservation_prepared_reference_consumes_pending_row() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let mut receipt =
        Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt
        .place_member("aarch64-apple-darwin/release")
        .selected = Some(
        RootSpec::Local {
            root: fixture.root.join("old-root"),
        }
        .selected(1),
    );
    let future = fixture.local();
    let reserved = publish_reservation(&fixture, &mut receipt, future.root());
    receipt.state = ReceiptState::Prepared;
    receipt.place_member("debug").prepared = Some(Prepared {
        generation: 2,
        to: future.selected(2).with_nonce(Some(reserved.nonce.clone())),
        link: link_path(&fixture.checkout, "debug"),
    });
    *receipt.reserved_mut() = None;
    write_receipt(&fixture.control, &receipt).unwrap();
    assert!(crate::prune::validate_reservation(&fixture.control, &reserved).is_err());
    let rows = crate::prune::test_rows(&fixture.control).unwrap();
    assert!(rows
        .iter()
        .any(|(root, selected, pending)| root == future.root() && !*selected && *pending));
}

#[test]
fn profile_reservation_drop_history_and_cancellation_controls() {
    let fixture = Fixture::new();
    let mut receipt = fixture.receipt();
    let future = fixture.local();
    let reserved = publish_reservation(&fixture, &mut receipt, future.root());
    // History alone must never excuse a genuinely dropped reservation.
    receipt
        .roots
        .push(future.selected(1).with_nonce(Some(reserved.nonce.clone())));
    *receipt.reserved_mut() = None;
    let before = authority_tree(&fixture.control);
    let error = write_receipt(&fixture.control, &receipt).unwrap_err();
    assert!(
        error.contains("without its cancellation journal"),
        "{error}"
    );
    assert_eq!(authority_tree(&fixture.control), before);
    crate::prune::validate_reservation(&fixture.control, &reserved).unwrap();
    // Cancellation in progress still prevents republication.
    crate::prune::begin_cancel(&fixture.control, &reserved).unwrap();
    *receipt.reserved_mut() = Some(reserved);
    let before = authority_tree(&fixture.control);
    assert!(write_receipt(&fixture.control, &receipt)
        .unwrap_err()
        .contains("being cancelled"));
    assert_eq!(authority_tree(&fixture.control), before);
}

/// The future-root shape this seam can build without a registered volume:
/// the checkout's own local root, held by a durable reservation on the same
/// fixture receipt the existing reservation test uses. The only variable
/// these tests add is the nonce AUTHORITY.

/// `Pruned` selects nothing, exactly as `Detached` does: a slot keyed for
/// a local target is not "converted", and an owned key is a real
/// mismatch. Historical receipts carrying a `pruned` object also still
/// deserialize cleanly.
#[test]
fn a_pruned_receipt_selects_nothing_in_every_reader() {
    let fixture = Fixture::new();
    let mut receipt = fixture.receipt();
    receipt.state = ReceiptState::Pruned;
    assert!(receipt.selects_nothing());
    assert!(keyed_state_matches(Some(&receipt), None).is_ok());
    let plan = whole_target_plan(Some(fixture.local().selected(1)));
    assert!(keyed_state_matches(Some(&receipt), Some(&plan)).is_err());

    // A historical transition/2 or transition/3 receipt with a `pruned`
    // record still deserializes read-only/cleanly.
    let mut doc = serde_json::to_value(&receipt).unwrap();
    doc["pruned"] = serde_json::json!({
        "intent": "0123456789abcdef0123456789abcdef",
        "outcome": "deleted",
        "at_ms": 1
    });
    let parsed: Receipt = serde_json::from_value(doc).unwrap();
    assert_eq!(parsed.state, ReceiptState::Pruned);
    assert!(parsed.selects_nothing());
}

/// A member directory is created component by component beneath the
/// root's own descriptor: a symlinked intermediate is refused and nothing
/// is created outside the root, where `create_dir_all` used to walk
/// through it.

/// Round 2-A: an owned directory is created owner-only, and an entry that
/// already exists (including a creator that raced the create) is accepted
/// only while it really is a directory. A symlink or a file is refused,
/// never written through.
#[test]
fn an_owned_directory_accepts_only_a_real_directory() {
    let fixture = Fixture::new();
    let created = fixture.root.join(".oxide-outputs");
    create_owned_directory(&created).unwrap();
    assert_eq!(
        fs::metadata(&created).unwrap().mode() & 0o777,
        0o700,
        "the created directory is not owner-only"
    );
    // An existing real directory is accepted as it is.
    create_owned_directory(&created).unwrap();

    let real = fixture.root.join("real");
    fs::create_dir(&real).unwrap();
    let link = fixture.root.join("link");
    symlink(&real, &link).unwrap();
    let file = fixture.root.join("file");
    fs::write(&file, b"kept").unwrap();

    for path in [&link, &file] {
        let refused = create_owned_directory(path).unwrap_err();
        assert!(
            refused.contains("is not a real directory"),
            "{}: {refused}",
            path.display()
        );
        assert!(fs::symlink_metadata(path).is_ok(), "the entry was removed");
    }
    assert_eq!(fs::read(&file).unwrap(), b"kept");
    assert_eq!(fs::read_link(&link).unwrap(), real);

    // The race arm accepts exactly the same shapes as the first
    // inspection, so a creator that appeared between them cannot redirect a
    // later write.
    let real_metadata = fs::symlink_metadata(&real).unwrap();
    accept_owned_directory(&real, &real_metadata).unwrap();
    for path in [&link, &file] {
        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(accept_owned_directory(path, &metadata).is_err());
    }
}

/// Round 2-A: a root marker is published with a no-replace rename, exactly
/// like a migration successor. A marker that appeared after the caller's
/// inspection is kept byte for byte instead of being replaced.
#[test]
fn a_root_marker_is_published_without_replacing_anything() {
    let fixture = Fixture::new();
    let temp = fixture.root.join(".oxide-output.json.tmp");
    let marker = fixture.root.join(MARKER);

    fs::write(&temp, b"new").unwrap();
    fs::write(&marker, b"incumbent").unwrap();
    let refused = publish_marker_file(&temp, &marker).unwrap_err();
    assert!(refused.contains("cannot publish"), "{refused}");
    assert_eq!(fs::read(&marker).unwrap(), b"incumbent");
    assert!(!temp.exists(), "the temporary marker was kept");

    // With nothing at the path the same publish succeeds on macOS; the
    // no-replace rename is unavailable elsewhere.
    fs::remove_file(&marker).unwrap();
    fs::write(&temp, b"new").unwrap();
    assert_eq!(
        publish_marker_file(&temp, &marker).is_ok(),
        cfg!(target_os = "macos")
    );
    if cfg!(target_os = "macos") {
        assert_eq!(fs::read(&marker).unwrap(), b"new");
        assert!(!temp.exists());
    }
}

/// Round 2-A: reading a marker or hashing its bytes never follows a
/// symlink. The checks (regular file, byte cap) apply to the opened
/// descriptor, so a swapped path cannot redirect the read.
#[test]
fn a_marker_read_or_digest_never_follows_a_symlink() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let spec = fixture.local();
    let marker = ensure_root(&context, &spec, 1).unwrap();
    let valid = fs::read(spec.root().join(MARKER)).unwrap();

    // A marker path that is a symlink to a VALID marker document: a plain
    // open would follow it and digest another root's identity.
    let root = fixture.root.join("root");
    fs::create_dir(&root).unwrap();
    let elsewhere = fixture.root.join("elsewhere.json");
    fs::write(&elsewhere, &valid).unwrap();
    symlink(&elsewhere, root.join(MARKER)).unwrap();
    assert!(read_marker(&root).is_err());
    let refused = marker_digest(&root).unwrap_err();
    assert!(refused.contains("cannot read"), "{refused}");

    // A directory at the marker path is refused as well, not read.
    fs::remove_file(root.join(MARKER)).unwrap();
    fs::create_dir(root.join(MARKER)).unwrap();
    assert!(read_marker(&root).is_err());
    assert!(marker_digest(&root).is_err());

    // The real marker this root publishes still reads and digests.
    assert_eq!(read_marker(spec.root()).unwrap(), marker);
    assert_eq!(marker_digest(spec.root()).unwrap().len(), 64);
}

/// The owner-marker refusal names the ROOT that carries the marker and the
/// checkout the marker records. The two nouns are never swapped, so a reader
/// can tell which side of the comparison disagreed.
#[test]
fn an_owner_marker_refusal_names_the_root_that_carries_it() {
    let fixture = Fixture::without_git();
    let other = Fixture::without_git();
    let root = fixture.root.join("owned-root");
    create_owned_directory(&root).unwrap();
    let marker = Marker {
        version: MARKER_VERSION.to_owned(),
        checkout: other.checkout.clone(),
        checkout_id: context_id(&other),
        label: "other".to_owned(),
        recipe: RECIPE.to_owned(),
        kind: RootKind::Local,
        volume_uuid: None,
        nonce: Some("0123456789abcdef0123456789abcdef".to_owned()),
        checkout_volume_uuid: None,
        generation: 1,
        created_ms: 1,
    };
    let path = root.join(MARKER);
    fs::write(&path, serde_json::to_vec_pretty(&marker).unwrap()).unwrap();
    let refused = owned_root(&fixture.checkout, &context_id(&fixture), &root).unwrap_err();
    assert!(
        refused.starts_with(&format!("{} belongs to", root.display())),
        "{refused}"
    );
    assert!(
        refused.contains(&other.checkout.display().to_string()),
        "{refused}"
    );

    // An unusable marker is reported against the root too, never against the
    // checkout that asked.
    let mut unusable = marker.clone();
    unusable.recipe = "an-unknown-recipe".to_owned();
    fs::write(&path, serde_json::to_vec_pretty(&unusable).unwrap()).unwrap();
    let refused = owned_root(&fixture.checkout, &context_id(&fixture), &root).unwrap_err();
    assert!(
        refused.starts_with(&format!("{} carries owner recipe", root.display())),
        "{refused}"
    );
}

/// A recorded root nonce is the same lowercase 128-bit hex a fresh nonce is;
/// a short, uppercase or non-hex value is refused by the receipt itself.
#[test]
fn a_receipt_root_nonce_must_be_lowercase_hex() {
    let fixture = Fixture::new();
    let id = context_id(&fixture);
    let mut receipt = fixture.receipt();
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    let mut selected = fixture.local().selected(1);
    selected.nonce = Some("0123456789abcdef0123456789abcdef".to_owned());
    receipt.link_mut().selected = Some(selected.clone());
    receipt.roots = vec![selected.clone()];
    receipt.validate(&fixture.checkout, &id).unwrap();

    for bad in [
        "9".repeat(31),
        "9".repeat(31) + "A",
        "9".repeat(31) + "g",
        format!("{}0", "9".repeat(32)),
    ] {
        let mut refused_receipt = receipt.clone();
        let mut refused_selected = selected.clone();
        refused_selected.nonce = Some(bad.clone());
        refused_receipt.link_mut().selected = Some(refused_selected);
        let refused = refused_receipt
            .validate(&fixture.checkout, &id)
            .unwrap_err();
        assert!(refused.contains("unusable root nonce"), "{bad}: {refused}");
    }
}
