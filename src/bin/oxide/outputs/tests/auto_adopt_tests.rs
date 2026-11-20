use super::*;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// S4b: the automatic first conversion creates the profile layout through
/// the same creator an explicit adoption uses, so both publish one receipt
/// form and one root marker.
#[test]
fn automatic_first_conversion_creates_the_profile_layout() {
    let automatic = Fixture::new();
    let context = automatic.context();
    let (_committed, selected, converted) = adopt_profile_from(
        &context,
        Receipt::first_with(
            &automatic.checkout,
            &context.checkout_id,
            LayoutKind::Profile,
        ),
        Start::MissingOnly,
        Some(&automatic.local()),
    )
    .unwrap();
    assert!(converted);
    let target = automatic.checkout.join("target");
    assert!(fs::symlink_metadata(&target).unwrap().is_dir());
    assert!(!fs::symlink_metadata(&target)
        .unwrap()
        .file_type()
        .is_symlink());
    for member in ["debug", "release"] {
        let link = target.join(member);
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "{member} is not an owned link"
        );
        assert_eq!(
            fs::read_link(&link).unwrap(),
            member_destination(selected.root(), member)
        );
        // The first conversion creates the member's real directory before
        // it publishes the link, so no link ever dangles (oxide#162).
        let destination = selected.root().join(member);
        let metadata = fs::symlink_metadata(&destination).unwrap();
        assert!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "{member} is not a real member directory"
        );
        assert!(
            fs::metadata(&link).unwrap().is_dir(),
            "{member} is a dangling member link"
        );
    }
    assert!(selected.root().join(MARKER).is_file());
    let stored = read_receipt(
        &automatic.control,
        &automatic.checkout,
        &context.checkout_id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(stored.state, ReceiptState::Committed);
    // The explicit adoption of a twin checkout publishes the same form.
    let explicit = Fixture::new();
    let explicit_context = explicit.context();
    let (explicit_receipt, explicit_selected, _) = adopt_profile(
        &explicit_context,
        Receipt::first_with(
            &explicit.checkout,
            &explicit_context.checkout_id,
            LayoutKind::Profile,
        ),
    )
    .unwrap();
    assert_eq!(stored.version, explicit_receipt.version);
    assert_eq!(stored.layout, explicit_receipt.layout);
    assert_eq!(
        stored
            .links()
            .iter()
            .map(|placement| placement.member().to_owned())
            .collect::<Vec<_>>(),
        explicit_receipt
            .links()
            .iter()
            .map(|placement| placement.member().to_owned())
            .collect::<Vec<_>>()
    );
    assert_eq!(stored.generation, explicit_receipt.generation);
    assert_eq!(selected.kind, explicit_selected.kind);
    // Every placed member shares the root's one nonce, as explicit adoption
    // does, and the marker is the same recipe and format.
    let nonces = stored
        .links()
        .iter()
        .filter_map(|placement| placement.selected_ref())
        .map(|selected| selected.nonce().map(str::to_owned))
        .collect::<Vec<_>>();
    let explicit_nonces = explicit_receipt
        .links()
        .iter()
        .filter_map(|placement| placement.selected_ref())
        .map(|selected| selected.nonce().map(str::to_owned))
        .collect::<Vec<_>>();
    assert_eq!(nonces.len(), 2);
    assert!(nonces.iter().all(|nonce| nonce.is_some()));
    assert_eq!(nonces[0], nonces[1], "the pair shares one root nonce");
    // Explicit adoption publishes the same one-nonce pair (the nonce value
    // itself is random per conversion).
    assert_eq!(explicit_nonces.len(), 2);
    assert_eq!(explicit_nonces[0], explicit_nonces[1]);
    let marker = read_marker(selected.root()).unwrap();
    let explicit_marker = read_marker(explicit_selected.root()).unwrap();
    assert_eq!(marker.version, explicit_marker.version);
    assert_eq!(marker.recipe, explicit_marker.recipe);
    assert_eq!(marker.kind, explicit_marker.kind);
    // The next leased build keys exactly the derived member set.
    assert_eq!(derived_exec(&automatic), ["debug", "release"]);
}

/// S4b: the automatic conversion never converts or keeps anything that
/// already exists, and it never falls back to another root.
#[test]
fn automatic_first_conversion_refuses_an_ineligible_checkout() {
    // An empty real target is not a first conversion.
    let empty = Fixture::new();
    fs::create_dir(empty.checkout.join("target")).unwrap();
    let context = empty.context();
    let refused = adopt_profile_from(
        &context,
        Receipt::first_with(&empty.checkout, &context.checkout_id, LayoutKind::Profile),
        Start::MissingOnly,
        Some(&empty.local()),
    )
    .unwrap_err();
    assert!(refused.contains("auto-adopt refused"), "{refused}");
    assert!(
        fs::read_dir(empty.checkout.join("target"))
            .unwrap()
            .next()
            .is_none(),
        "a refused conversion wrote into the target"
    );
    assert!(
        read_receipt(&empty.control, &empty.checkout, &context.checkout_id)
            .unwrap()
            .is_none(),
        "a refused conversion wrote a receipt"
    );
    assert!(!empty.local().root().exists());
    // An owned whole-target link is left exactly as it is.
    let linked = Fixture::new();
    let linked_context = linked.context();
    transition(
        &linked_context,
        &linked.receipt(),
        &linked.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    let before = fs::read_link(linked.checkout.join("target")).unwrap();
    let refused = adopt_profile_from(
        &linked_context,
        Receipt::first_with(
            &linked.checkout,
            &linked_context.checkout_id,
            LayoutKind::Profile,
        ),
        Start::MissingOnly,
        Some(&linked.local()),
    )
    .unwrap_err();
    assert!(refused.contains("whole-target link"), "{refused}");
    assert_eq!(
        fs::read_link(linked.checkout.join("target")).unwrap(),
        before
    );
}

/// S4b: a whole-target receipt is untouched by a build and by a derived
/// `exec`; only an explicit adoption migrates it.
#[test]
fn a_whole_target_receipt_is_untouched_by_a_build() {
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
    let path = receipt_path(&fixture.control, &context.checkout_id);
    let before = fs::read(&path).unwrap();
    assert!(derived_exec(&fixture).is_empty());
    let declaration = declaration_of(&["debug".to_owned(), "release".to_owned()]);
    let plan = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &declaration,
        None,
    )
    .unwrap()
    .unwrap();
    assert!(plan.whole_target);
    assert!(destination_facts(Some(&plan)).starts_with("outputs-2 "));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(
        fs::read_link(fixture.checkout.join("target")).unwrap(),
        fixture.local().root()
    );
}

#[test]
fn auto_adoption_needs_every_fact_and_nothing_else() {
    for bits in 0..64u8 {
        let facts = AutoFacts {
            selected_permanent: bits & 1 != 0,
            same_selection: bits & 2 != 0,
            external_usable: bits & 4 != 0,
            permanent: bits & 8 != 0,
            receipt_absent: bits & 16 != 0,
            target_absent: bits & 32 != 0,
        };
        assert_eq!(auto_adopts(&facts), bits == 63, "{facts:?}");
    }
}

#[test]
fn fresh_auto_adoption_does_not_measure_neighboring_outputs() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let store = Store::for_tests(&fixture.root.join("pool"));
    let control = store.owner_control();
    let context = Context::new(Some(control.to_path_buf()), fixture.checkout.clone());
    crate::prune::ensure_ready(&store, &fixture.checkout).unwrap();
    let spec = fixture.local();
    let neighbors = crate::prune::seed_output_neighbors(control, &spec.selected(1), 708);
    assert!(context.receipt().unwrap().is_none());
    assert_eq!(context.target().unwrap(), TargetState::Missing);

    let started = Instant::now();
    let (result, (listings, measured)) = crate::prune::count_output_listings(|| {
        auto_adopt_fresh(&store, &context, &spec.selected(0), || {
            Ok(Some(spec.clone()))
        })
    });
    let elapsed = started.elapsed();
    result.unwrap();
    assert_eq!(measured, 0, "auto-adoption measured output payloads");
    assert!(
        listings <= 8,
        "auto-adoption listed the area {listings} times"
    );
    // A generous upper bound for the whole conversion, which includes
    // every admission-lock hold as well as work outside the lock.
    assert!(
        elapsed < Duration::from_secs(10),
        "auto-adoption took {elapsed:?}"
    );

    let receipt = context.receipt().unwrap().unwrap();
    assert_eq!(receipt.state, ReceiptState::Committed);
    for member in AUTO_ADOPT_MEMBERS {
        assert_eq!(
            fs::read_link(context.link().join(member)).unwrap(),
            spec.root().join(member),
        );
    }
    crate::prune::assert_output_neighbors(control, &neighbors);
    println!(
            "auto-adoption beside {} roots: {measured} measured, {listings} metadata listings, {elapsed:?} including admission-lock holds",
            neighbors.len()
        );
}

/// A command whose slot came from the internal pool, or from a registration
/// that was ephemeral when it selected, never converts, even when the drive
/// is permanent by the time it checks. The all-true facts are the control.
#[test]
fn an_ephemeral_selection_never_converts_after_a_switch_to_permanent() {
    let switched = AutoFacts {
        selected_permanent: false,
        ..all_facts()
    };
    assert!(!auto_adopts(&switched));
    assert!(auto_adopts(&all_facts()), "positive control");
}

/// A command whose slot came from volume A never converts for volume B,
/// even when B is the usable permanent registration now. The same volume
/// in force now is the positive control.
#[test]
fn a_selection_of_one_volume_never_converts_another_registration() {
    let external = |volume: &str, uuid: &str| Selected {
        kind: RootKind::External,
        root: PathBuf::from(format!("/Volumes/{volume}/oxide-outputs/app-0000")),
        volume_uuid: Some(uuid.to_owned()),
        nonce: None,
        generation: 0,
    };
    let captured = external("a", "uuid-a");
    let switched = external("b", "uuid-b");
    assert!(!selection_matches(&captured, &switched));
    assert!(!auto_adopts(&AutoFacts {
        same_selection: selection_matches(&captured, &switched),
        ..all_facts()
    }));
    assert!(selection_matches(&captured, &captured.clone()));
    assert!(auto_adopts(&AutoFacts {
        same_selection: selection_matches(&captured, &captured.clone()),
        ..all_facts()
    }));
}

/// A `detached` receipt is a receipt: a missing target beside it is never
/// a fresh checkout, and the receipt is read, not treated as absent.
#[test]
fn a_detached_receipt_blocks_auto_adoption_of_a_missing_target() {
    let fixture = Fixture::new();
    let mut receipt = fixture.receipt();
    receipt.state = ReceiptState::Detached;
    write_receipt(&fixture.control, &receipt).unwrap();
    let context = fixture.context();
    let receipt_absent = context.receipt().unwrap().is_none();
    assert!(!receipt_absent);
    assert!(!auto_adopts(&AutoFacts {
        selected_permanent: true,
        same_selection: true,
        external_usable: true,
        permanent: true,
        receipt_absent,
        target_absent: context.target().unwrap() == TargetState::Missing,
    }));
}

#[test]
fn an_automatic_conversion_converts_only_an_absent_target() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let committed = transition(
        &context,
        &fixture.receipt(),
        &fixture.local(),
        Start::MissingOnly,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    assert_eq!(committed.generation, 1);
    assert_eq!(
        fs::read_link(context.link()).unwrap(),
        fixture.local().root()
    );
}

/// Every present target (empty directory, file, dangling link) refuses
/// before any receipt or sibling is written.
#[test]
fn an_automatic_conversion_refuses_every_present_target_unchanged() {
    let fixture = Fixture::new();
    let context = fixture.context();
    let link = context.link();
    let dangling = fixture.root.join("absent-root");

    fs::create_dir(&link).unwrap();
    let refused = transition(
        &context,
        &fixture.receipt(),
        &fixture.local(),
        Start::MissingOnly,
        NonceAuthority::Unkeyed,
    )
    .unwrap_err();
    assert!(refused.contains("auto-adopt refused"), "{refused}");
    assert!(fs::symlink_metadata(&link).unwrap().is_dir());
    fs::remove_dir(&link).unwrap();

    fs::write(&link, b"kept").unwrap();
    assert!(transition(
        &context,
        &fixture.receipt(),
        &fixture.local(),
        Start::MissingOnly,
        NonceAuthority::Unkeyed
    )
    .is_err());
    assert_eq!(fs::read(&link).unwrap(), b"kept");
    fs::remove_file(&link).unwrap();

    symlink(&dangling, &link).unwrap();
    assert!(transition(
        &context,
        &fixture.receipt(),
        &fixture.local(),
        Start::MissingOnly,
        NonceAuthority::Unkeyed
    )
    .is_err());
    assert_eq!(fs::read_link(&link).unwrap(), dangling);
    assert!(
        read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .is_none()
    );
}

/// A staged link never replaces a file, directory or link that appeared
/// after the target was classified absent.
#[test]
fn a_no_replace_publish_keeps_whatever_appeared_at_the_link() {
    let fixture = Fixture::new();
    let sibling = fixture.root.join("staged");
    symlink(fixture.root.join("new-root"), &sibling).unwrap();
    let link = fixture.root.join("target");
    for creator in ["file", "directory", "link"] {
        match creator {
            "file" => fs::write(&link, b"kept").unwrap(),
            "directory" => fs::create_dir(&link).unwrap(),
            _ => symlink(fixture.root.join("foreign"), &link).unwrap(),
        }
        assert!(publish_link(&sibling, &link, true).is_err(), "{creator}");
        assert!(os_symlink_exists(&sibling), "{creator}: staged link lost");
        assert!(fs::symlink_metadata(&link).is_ok(), "{creator} removed");
        if creator == "directory" {
            fs::remove_dir(&link).unwrap();
        } else {
            fs::remove_file(&link).unwrap();
        }
    }
    // Control: with nothing at the link, the exclusive publish succeeds on
    // macOS; elsewhere it refuses (no replacing fallback).
    let published = publish_link(&sibling, &link, true);
    assert_eq!(published.is_ok(), cfg!(target_os = "macos"));
}

#[test]
fn a_changed_volume_or_root_is_not_the_captured_destination() {
    let captured = Selected {
        kind: RootKind::External,
        root: PathBuf::from("/Volumes/a/oxide-outputs/app-0000"),
        volume_uuid: Some("uuid-a".to_owned()),
        nonce: None,
        generation: 0,
    };
    assert!(captured.same_place(&captured));
    let other_volume = Selected {
        volume_uuid: Some("uuid-b".to_owned()),
        ..captured.clone()
    };
    let other_root = Selected {
        root: PathBuf::from("/Volumes/b/oxide-outputs/app-0000"),
        ..captured.clone()
    };
    assert!(!captured.same_place(&other_volume));
    assert!(!captured.same_place(&other_root));
}

/// The automatic conversion's destination is the one a later command keys:
/// the planned destination is the committed selection itself.
#[test]
fn an_automatic_conversion_keys_the_committed_destination() {
    let fixture = Fixture::new();
    let committed = transition(
        &fixture.context(),
        &fixture.receipt(),
        &fixture.local(),
        Start::MissingOnly,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    let planned = planned_destination(
        Some(&fixture.control),
        &fixture.checkout,
        &crate::layout::Declaration::empty(),
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(planned.selected(), committed.selected());
}

/// A metadata error on the target is an error, never a missing target.
#[test]
fn an_unreadable_checkout_is_an_error_not_an_absent_target() {
    // SAFETY: geteuid has no preconditions. Root reads through mode 000.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let fixture = Fixture::new();
    let id = fixture.context().checkout_id;
    fs::set_permissions(&fixture.checkout, fs::Permissions::from_mode(0o000)).unwrap();
    let result = target_state(&fixture.checkout, &id);
    fs::set_permissions(&fixture.checkout, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err(), "{result:?}");
}
