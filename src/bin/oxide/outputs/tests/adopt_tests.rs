use super::super::plan::*;
use super::*;
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[test]
fn a_version_one_receipt_reads_for_prune_only_and_is_never_rewritten() {
    let fixture = Fixture::new();
    let root = fixture.local().root().to_path_buf();
    write_receipt_document(
        &fixture,
        &version_one_receipt(&fixture, "committed", Some(&root)),
    );
    let before = receipt_proof(&fixture);
    let checkout = fixture.checkout.clone();
    let id = context_id(&fixture);

    // The pre-oxide#152 refusal is the control: every ordinary reader
    // still rejects the same bytes as an unsupported version.
    let refused = read_receipt(&fixture.control, &checkout, &id).unwrap_err();
    assert!(refused.contains("unsupported version"), "{refused}");
    assert!(refused.contains(LEGACY_RECEIPT_VERSION), "{refused}");

    let receipt = read_receipt_with(LegacyReceiptReading::Read, &fixture.control, &checkout, &id)
        .unwrap()
        .expect("the version-1 receipt was not read");
    assert_eq!(receipt.version, RECEIPT_VERSION);
    assert_eq!(receipt.layout(), LayoutKind::WholeTarget);
    assert_eq!(receipt.state, ReceiptState::Committed);
    assert_eq!(receipt.checkout(), checkout.as_path());
    assert_eq!(receipt.checkout_id(), id);
    assert_eq!(receipt.roots.len(), 1);
    assert_eq!(receipt.roots[0].root(), root.as_path());
    assert_eq!(receipt.roots[0].generation(), 1);
    assert!(receipt
        .selected()
        .is_some_and(|selected| selected.root() == root.as_path()));
    assert!(receipt
        .selected()
        .is_some_and(|selected| selected.nonce().is_none()));
    assert!(receipt.reserved().is_none());
    assert_eq!(receipt.links.len(), 1);
    assert_eq!(receipt.links[0].member(), MEMBER);
    assert!(receipt.prepared_ref().is_none());
    // Reading is not a migration: the published bytes, inode, length,
    // mode and mtime are exactly as oxide#117 left them.
    assert_eq!(receipt_proof(&fixture), before);

    // A valid noncommitted version-1 state is read, not malformed.
    write_receipt_document(&fixture, &version_one_receipt(&fixture, "detached", None));
    let detached = read_receipt_with(LegacyReceiptReading::Read, &fixture.control, &checkout, &id)
        .unwrap()
        .expect("the detached version-1 receipt was not read");
    assert_eq!(detached.state, ReceiptState::Detached);
    assert!(detached.current().is_none());
    let mut prepared = version_one_receipt(&fixture, "prepared", Some(&root));
    prepared["prepared"] = serde_json::json!({
        "generation": 2,
        "to": {"kind": "local", "root": root, "generation": 2},
        "link": fixture.checkout.join("target").to_string_lossy(),
    });
    prepared["generation"] = serde_json::json!(1);
    write_receipt_document(&fixture, &prepared);
    let transition =
        read_receipt_with(LegacyReceiptReading::Read, &fixture.control, &checkout, &id)
            .unwrap()
            .expect("the prepared version-1 receipt was not read");
    assert_eq!(transition.state, ReceiptState::Prepared);
    assert!(transition.prepared_ref().is_some());
    assert!(transition.current().is_some());
}

/// Legacy whole-target receipts (`transition/1` and `transition/2`) are
/// recognized read-only by `status` and `plan`, refused without mutation by
/// direct `adopt`, and switched to profile member links via `detach` +
/// `adopt`.
#[test]
fn legacy_whole_target_receipts_switch_via_detach_then_adopt() {
    for v1 in [true, false] {
        let (fixture, legacy) = legacy_checkout();
        let context = fixture.context();
        let id = context.checkout_id.clone();
        let store = Store::for_tests(&fixture.root.join("pool"));
        let control = store.owner_control().to_path_buf();
        register_at(&fixture, &control, DriveMode::Permanent);
        let root = legacy.selected().unwrap().root().to_path_buf();
        if v1 {
            make_root_one_marker(&fixture, &legacy);
            write_receipt_document_at(
                &fixture,
                &control,
                &version_one_receipt(&fixture, "committed", Some(&root)),
            );
        } else {
            write_receipt_bytes(&control, &id, &legacy);
        }
        fs::create_dir_all(root.join("debug")).unwrap();
        fs::write(root.join("debug").join("old-binary"), b"old").unwrap();
        let before = receipt_proof_at(&control, &id);

        if v1 {
            let refused = read_receipt(&control, &fixture.checkout, &id).unwrap_err();
            assert!(refused.contains("unsupported version"), "{refused}");
            assert!(refused.contains("oxide outputs detach"), "{refused}");
            let bind = match bind_for_command(&store, &fixture.checkout, 0, None, None, false) {
                Err(reason) => reason,
                Ok(_) => panic!("binding accepted a version-1 receipt"),
            };
            assert!(bind.contains("oxide outputs detach"), "{bind}");
        }

        // `status` and `plan` read both v1 and v2 read-only without rewriting.
        let status_ctx = Context::new(Some(control.clone()), fixture.checkout.clone());
        let status = output_status_lines(&status_ctx).unwrap();
        assert!(
            status
                .iter()
                .any(|line| line.contains("oxide outputs detach")),
            "v1={v1}: {status:?}"
        );
        let report = plan_report(&status_ctx).unwrap();
        assert_eq!(report.layout, "whole-target", "v1={v1}");
        assert_eq!(report.action, "refuse", "v1={v1}");
        assert!(
            report.reason.as_deref().is_some_and(
                |r| r.contains("oxide outputs detach") && r.contains("oxide outputs adopt")
            ),
            "v1={v1}: {:?}",
            report.reason
        );
        assert_eq!(receipt_proof_at(&control, &id), before);

        // Direct `adopt` refuses without changing receipt, target, or registry.
        let refused = adopt_at(&status_ctx, || Ok(Some(store.handle_for_tests()))).unwrap_err();
        assert!(
            refused.contains("oxide outputs detach"),
            "v1={v1}: {refused}"
        );
        assert_eq!(receipt_proof_at(&control, &id), before);
        assert!(fs::symlink_metadata(context.link())
            .unwrap()
            .file_type()
            .is_symlink());

        // Switching via `detach` + `adopt` works cleanly and keeps the legacy root.
        assert_eq!(
            detach_at(&status_ctx, || Ok(Some(store.handle_for_tests()))).unwrap(),
            0
        );
        assert!(fs::symlink_metadata(context.link()).unwrap().is_dir());
        assert_eq!(
            fs::read(root.join("debug").join("old-binary")).unwrap(),
            b"old"
        );
        let detached = read_receipt(&control, &fixture.checkout, &id)
            .unwrap()
            .unwrap();
        assert_eq!(detached.state, ReceiptState::Detached);
        assert_eq!(detached.layout(), LayoutKind::WholeTarget);

        assert_eq!(
            adopt_at(&status_ctx, || Ok(Some(store.handle_for_tests()))).unwrap(),
            0
        );
        let profile = read_receipt(&control, &fixture.checkout, &id)
            .unwrap()
            .unwrap();
        assert_eq!(profile.version, PROFILE_RECEIPT_VERSION);
        assert_eq!(profile.layout(), LayoutKind::Profile);
        assert_eq!(profile.state, ReceiptState::Committed);
        assert!(profile.generation > legacy.generation);
        assert!(profile.roots.iter().any(|known| known.root == root));
        for member in ["debug", "release"] {
            assert!(
                fs::symlink_metadata(context.link().join(member))
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "v1={v1}: {member}"
            );
        }
    }
}

/// Contradictory version-1 receipts (successor-only fields or unknown
/// versions) are refused by both ordinary and read-only readers, and their
/// bytes never change.
#[test]
fn contradictory_version_one_receipts_are_refused() {
    let fixture = Fixture::new();
    let root = fixture.local().root().to_path_buf();
    let id = context_id(&fixture);

    // Successor-only surfaces and unknown fields are contradictory, not
    // merely old.
    let mut layout = version_one_receipt(&fixture, "committed", Some(&root));
    layout["layout"] = serde_json::json!("profile");
    let mut reserved = version_one_receipt(&fixture, "committed", Some(&root));
    reserved["reserved"] = serde_json::json!({
        "id": "reservation",
        "nonce": "a".repeat(32),
        "kind": "local",
        "root": root,
        "generation": 2,
        "created_ms": 1,
    });
    let mut nonce = version_one_receipt(&fixture, "committed", Some(&root));
    nonce["selected"]["nonce"] = serde_json::json!("a".repeat(32));
    let mut unknown = version_one_receipt(&fixture, "committed", Some(&root));
    unknown["surprise"] = serde_json::json!(true);
    let mut other = version_one_receipt(&fixture, "committed", Some(&root));
    other["checkout"] = serde_json::json!("/somewhere/another-checkout");
    for document in [&layout, &reserved, &nonce, &unknown] {
        write_receipt_document(&fixture, document);
        let before = receipt_proof(&fixture);
        let refused = read_receipt_for_prune(&fixture.control, &fixture.checkout, &id).unwrap_err();
        assert!(
            refused.contains("is invalid"),
            "{document} was read: {refused}"
        );
        assert_eq!(receipt_proof(&fixture), before, "{document}");
    }
    // An unreadable identity is refused by the same checkout check every
    // reader applies, and nothing is rewritten.
    write_receipt_document(&fixture, &other);
    let before = receipt_proof(&fixture);
    let refused = read_receipt_for_prune(&fixture.control, &fixture.checkout, &id).unwrap_err();
    assert!(refused.contains("another checkout"), "{refused}");
    assert_eq!(receipt_proof(&fixture), before);
}

#[test]
fn first_profile_plan_agrees_with_adopt_for_member_classes() {
    for class in [
        "nonempty",
        "empty",
        "missing",
        "foreign-file",
        "foreign-symlink",
        "all-keep",
    ] {
        let fixture = Fixture::new();
        let (store, context) = fresh_pool(&fixture);
        let target = context.link();
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep.txt"), b"warm").unwrap();
        let debug = target.join("debug");
        match class {
            "nonempty" | "all-keep" => {
                fs::create_dir(&debug).unwrap();
                fs::write(debug.join("keep"), b"member").unwrap();
                if class == "all-keep" {
                    let release = target.join("release");
                    fs::create_dir(&release).unwrap();
                    fs::write(release.join("keep"), b"release").unwrap();
                }
            }
            "empty" => fs::create_dir(&debug).unwrap(),
            "foreign-file" => fs::write(&debug, b"foreign").unwrap(),
            "foreign-symlink" => {
                let outside = fixture.root.join("foreign");
                fs::create_dir(&outside).unwrap();
                symlink(outside, &debug).unwrap();
            }
            "missing" => {}
            _ => unreachable!(),
        }
        let report = plan_report(&context).unwrap();
        assert_eq!(report.layout, "profile", "{class}");
        assert!(!report.data_move);
        let member = report.members.iter().find(|m| m.member == "debug").unwrap();
        let foreign = class.starts_with("foreign");
        let expected = if foreign {
            "refuse"
        } else if class == "nonempty" || class == "all-keep" {
            "keep"
        } else {
            "convert"
        };
        assert_eq!(member.adoption, Some(expected), "{class}");
        assert_eq!(
            report.action == "refuse",
            foreign || class == "all-keep",
            "{class}"
        );
        if class == "all-keep" {
            assert_eq!(report.action, "refuse");
            assert!(report.members.iter().all(|m| m.adoption == Some("keep")));
        }
        if foreign {
            assert!(report.reason.as_ref().unwrap().contains("debug"));
        }
        assert!(
            !registry_document(&store).exists(),
            "plan wrote registry: {class}"
        );
        assert!(
            !receipt_path(store.control(), &context.checkout_id).exists(),
            "plan wrote receipt: {class}"
        );
        assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"warm");

        let result = adopt_at(&context, || Ok(Some(store.handle_for_tests())));
        assert_eq!(
            result.is_err(),
            foreign || class == "all-keep",
            "{class}: {result:?}"
        );
        if class == "all-keep" {
            assert!(result.unwrap_err().contains("adoption committed no root"));
        }
        if class == "nonempty" || class == "all-keep" {
            assert_eq!(fs::read(debug.join("keep")).unwrap(), b"member");
            assert!(!fs::symlink_metadata(&debug)
                .unwrap()
                .file_type()
                .is_symlink());
        } else if !foreign {
            assert!(
                fs::symlink_metadata(&debug)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "{class}"
            );
        } else if class == "foreign-file" {
            assert_eq!(fs::read(&debug).unwrap(), b"foreign");
        } else {
            assert!(fs::symlink_metadata(&debug)
                .unwrap()
                .file_type()
                .is_symlink());
        }
        if class == "all-keep" {
            let release = target.join("release");
            assert_eq!(fs::read(release.join("keep")).unwrap(), b"release");
            assert!(!fs::symlink_metadata(release)
                .unwrap()
                .file_type()
                .is_symlink());
            assert!(!receipt_path(store.control(), &context.checkout_id).exists());
        }
        assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"warm");
    }
}

/// A committed whole-target receipt for this fixture's checkout.

#[test]
fn a_fresh_pool_initializes_the_registry_before_its_first_profile_adoption() {
    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let (store, context) = fresh_pool(&fixture);
    // What a completed checkout-local build leaves behind: a real nonempty
    // target with no owned profile member inside it.
    fs::create_dir(context.link()).unwrap();
    fs::write(context.link().join("CACHEDIR.TAG"), b"warm").unwrap();
    assert!(adoption_publishes_a_transition(&context, store.control()).unwrap());

    let status = adopt_at(&context, || Ok(Some(store.handle_for_tests()))).unwrap();
    assert_eq!(status, 0);
    assert!(registry_document(&store).is_file());
    assert!(matches!(
        store.outputs_initialization().unwrap(),
        Some(crate::store::OutputsInit::Ready { .. })
    ));
    let receipt = read_receipt(store.control(), &fixture.checkout, &context.checkout_id)
        .unwrap()
        .expect("the adoption published a receipt");
    assert_eq!(receipt.layout(), LayoutKind::Profile);
    assert_eq!(receipt.state, ReceiptState::Committed);
    // Both members are ownet links into one root; the real nonempty
    // directory underneath and its content were never moved.
    let root = receipt
        .selected()
        .expect("a selected root")
        .root()
        .to_path_buf();
    for member in ["debug", "release"] {
        let link = context.link().join(member);
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "{member} stayed a real directory"
        );
        assert_eq!(
            fs::read_link(&link).unwrap(),
            member_destination(&root, member)
        );
        assert!(receipt.selected_for(member).is_some(), "{member}");
    }
    assert_eq!(
        fs::read(context.link().join("CACHEDIR.TAG")).unwrap(),
        b"warm"
    );
    assert!(fs::symlink_metadata(context.link())
        .unwrap()
        .file_type()
        .is_dir());
}

/// The same fresh pool through the empty-target route: `adopt` converts a
/// real empty target's members, and that transition needs the registry
/// initialized first too. The automatic first conversion keeps its own
/// establishment before its publication lock.
#[test]
fn a_fresh_pool_initializes_the_registry_for_an_empty_target_adoption() {
    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let (store, context) = fresh_pool(&fixture);
    fs::create_dir(context.link()).unwrap();
    assert!(adoption_publishes_a_transition(&context, store.control()).unwrap());
    let status = adopt_at(&context, || Ok(Some(store.handle_for_tests()))).unwrap();
    assert_eq!(status, 0);
    assert!(registry_document(&store).is_file());
    let receipt = read_receipt(store.control(), &fixture.checkout, &context.checkout_id)
        .unwrap()
        .expect("the adoption published a receipt");
    assert_eq!(receipt.layout(), LayoutKind::Profile);
    assert!(receipt.selected_for("debug").is_some());
    assert!(receipt.selected_for("release").is_some());
}

/// A pool whose state records a ready registry but whose registry document
/// is gone is genuinely legacy: it is never rebuilt from receipts, markers
/// or a fresh initialization, and the existing upgrade refusal stands with
/// the receipt, the target and the pool unchanged.
#[test]
fn a_legacy_pool_without_its_registry_still_refuses_a_first_adoption() {
    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let store = Store::for_tests(&fixture.root.join("pool"));
    register_at(&fixture, store.control(), DriveMode::Permanent);
    let context = Context::new(
        Some(store.control().to_path_buf()),
        fixture.checkout.clone(),
    );
    // What a pool that had already initialized looks like once its registry
    // document disappears: the state says ready, the document is absent.
    store
        .initialize_outputs(&"c".repeat(32), Some(&fixture.checkout), &|_, _| Ok(()))
        .unwrap();
    assert!(!registry_document(&store).exists());
    // A committed receipt makes it a legacy pool rather than a fresh one.
    let mut legacy_profile =
        Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    legacy_profile.state = ReceiptState::Committed;
    legacy_profile.generation = 1;
    legacy_profile.links = vec![LinkPlacement {
        member: "debug".to_owned(),
        selected: Some(fixture.local().selected(1)),
        prepared: None,
    }];
    legacy_profile.roots = vec![fixture.local().selected(1)];
    write_receipt_bytes(store.control(), &context.checkout_id, &legacy_profile);
    let receipt_path = receipt_path(store.control(), &context.checkout_id);
    let before = fs::read(&receipt_path).unwrap();
    fs::create_dir(context.link()).unwrap();
    fs::write(context.link().join("keep"), b"warm").unwrap();
    assert!(adoption_publishes_a_transition(&context, store.control()).unwrap());

    let refused = adopt_at(&context, || Ok(Some(store.handle_for_tests()))).unwrap_err();
    assert!(
        refused.contains("records a ready registry; refusing to rebuild it"),
        "{refused}"
    );
    assert!(!registry_document(&store).exists());
    assert_eq!(fs::read(&receipt_path).unwrap(), before);
    assert!(fs::symlink_metadata(context.link().join("debug")).is_err());
    assert!(fs::symlink_metadata(context.link().join("release")).is_err());
}

/// A refused target initializes nothing: the adoption refuses with its
/// existing text, and the fresh pool still has no registry, no receipt and
/// no link. The same holds for a member the conversion refuses before it
/// publishes anything; a member refusal after an earlier conversion is not
/// an effect-free refusal, and the earlier transition is real work.
#[test]
fn a_refused_target_initializes_no_registry() {
    // A foreign target: a link to somewhere Oxide does not own.
    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let (store, context) = fresh_pool(&fixture);
    let elsewhere = fixture.root.join("elsewhere");
    fs::create_dir_all(elsewhere.join("debug")).unwrap();
    symlink(&elsewhere, context.link()).unwrap();
    assert!(!adoption_publishes_a_transition(&context, store.control()).unwrap());
    let refused = adopt_at(&context, || Ok(Some(store.handle_for_tests()))).unwrap_err();
    assert!(refused.contains("Oxide never replaces it"), "{refused}");
    assert!(!registry_document(&store).exists());
    assert!(!store.control().join("outputs").exists());
    assert!(fs::symlink_metadata(&elsewhere.join("debug"))
        .unwrap()
        .file_type()
        .is_dir());

    // A real nonempty target whose first member is foreign: the conversion
    // refuses it before publishing anything, so nothing is initialized.
    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let (store, context) = fresh_pool(&fixture);
    let elsewhere = fixture.root.join("elsewhere");
    fs::create_dir(context.link()).unwrap();
    fs::write(context.link().join("keep"), b"warm").unwrap();
    fs::create_dir(&elsewhere).unwrap();
    symlink(&elsewhere, context.link().join("debug")).unwrap();
    assert!(!adoption_publishes_a_transition(&context, store.control()).unwrap());
    let refused = adopt_at(&context, || Ok(Some(store.handle_for_tests()))).unwrap_err();
    assert!(
        refused.contains("it is never followed or replaced"),
        "{refused}"
    );
    assert!(!registry_document(&store).exists());
    assert!(fs::symlink_metadata(context.link().join("release")).is_err());
}

/// Every arm the profile conversion dispatches on decides readiness the same
/// way (oxide#217), so no arm can publish a transition on a fresh pool
/// without the registry, and no refusal that publishes nothing initializes
/// one.
#[test]
fn adoption_readiness_follows_every_conversion_arm() {
    let no_receipt = |prepare: &dyn Fn(&Context, &Fixture)| -> bool {
        let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
        let (store, context) = fresh_pool(&fixture);
        prepare(&context, &fixture);
        adoption_publishes_a_transition(&context, store.control()).unwrap()
    };
    let populated = |context: &Context, _: &Fixture| {
        fs::create_dir(context.link()).unwrap();
        fs::write(context.link().join("keep"), b"warm").unwrap();
    };
    let nonempty_member = |context: &Context, member: &str| {
        fs::create_dir(context.link().join(member)).unwrap();
        fs::write(context.link().join(member).join("keep"), b"warm").unwrap();
    };

    // A missing or empty real target always places every member.
    assert!(no_receipt(&|_, _| {}));
    assert!(no_receipt(&|context, _| {
        fs::create_dir(context.link()).unwrap();
    }));
    // A real nonempty target publishes only a member that is not already a
    // real nonempty directory.
    assert!(!no_receipt(&|context, fixture| {
        populated(context, fixture);
        nonempty_member(context, "debug");
        nonempty_member(context, "release");
    }));
    assert!(no_receipt(&|context, fixture| {
        populated(context, fixture);
        nonempty_member(context, "debug");
    }));
    assert!(no_receipt(&|context, fixture| {
        populated(context, fixture);
        nonempty_member(context, "release");
    }));
    // An empty real member directory is converted by an explicit adoption,
    // so it publishes too.
    assert!(no_receipt(&|context, fixture| {
        populated(context, fixture);
        fs::create_dir(context.link().join("debug")).unwrap();
        nonempty_member(context, "release");
    }));
    // An owned member link and a dangling one are relinked: a first
    // conversion has no committed selection, so both publish.
    assert!(no_receipt(&|context, fixture| {
        populated(context, fixture);
        let spec = fixture.local();
        ensure_root(context, &spec, 1).unwrap();
        ensure_member_directory(spec.root(), "debug").unwrap();
        symlink(
            member_destination(spec.root(), "debug"),
            context.link().join("debug"),
        )
        .unwrap();
        nonempty_member(context, "release");
    }));
    assert!(no_receipt(&|context, fixture| {
        populated(context, fixture);
        symlink(
            fixture.root.join("absent-root/debug"),
            context.link().join("debug"),
        )
        .unwrap();
        nonempty_member(context, "release");
    }));
    // A foreign member the conversion meets first refuses before any
    // transition, so nothing is initialized for it...
    assert!(!no_receipt(&|context, fixture| {
        populated(context, fixture);
        symlink(&fixture.root, context.link().join("debug")).unwrap();
        fs::create_dir(context.link().join("release")).unwrap();
    }));
    // ...while a foreign member after an earlier publishing member cannot
    // un-publish that work, so the registry is needed.
    assert!(no_receipt(&|context, fixture| {
        populated(context, fixture);
        symlink(&fixture.root, context.link().join("release")).unwrap();
    }));

    // A whole-target owned or dangling link and a foreign target refuse
    // with nothing to publish.
    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let (store, context) = fresh_pool(&fixture);
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    symlink(spec.root(), context.link()).unwrap();
    assert!(!adoption_publishes_a_transition(&context, store.control()).unwrap());
    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let (store, context) = fresh_pool(&fixture);
    symlink(fixture.root.join("absent-root"), context.link()).unwrap();
    assert!(!adoption_publishes_a_transition(&context, store.control()).unwrap());
    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let (store, context) = fresh_pool(&fixture);
    symlink(&fixture.root, context.link()).unwrap();
    assert!(!adoption_publishes_a_transition(&context, store.control()).unwrap());

    // A committed whole-target receipt refuses without initializing or
    // writing; a detached or profile receipt may write.
    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let store = Store::for_tests(&fixture.root.join("pool"));
    register_at(&fixture, store.control(), DriveMode::Permanent);
    let context = Context::new(
        Some(store.control().to_path_buf()),
        fixture.checkout.clone(),
    );
    write_receipt_bytes(
        store.control(),
        &context.checkout_id,
        &committed_receipt(&fixture),
    );
    assert!(!adoption_publishes_a_transition(&context, store.control()).unwrap());
    let mut detached = committed_receipt(&fixture);
    detached.state = ReceiptState::Detached;
    detached.link_mut().selected = None;
    write_receipt_bytes(store.control(), &context.checkout_id, &detached);
    assert!(adoption_publishes_a_transition(&context, store.control()).unwrap());
}

#[test]
fn adoption_selects_the_profile_layout_and_keeps_real_members() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    let local = fixture.local();
    // A real nonempty member directory is never converted.
    fs::create_dir(context.link()).unwrap();
    fs::create_dir(context.link().join("release")).unwrap();
    fs::write(context.link().join("release/keep"), b"x").unwrap();
    let receipt = Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    let (committed, selected, converted) = adopt_profile(&context, receipt).unwrap();
    assert!(converted);
    assert_eq!(committed.layout(), LayoutKind::Profile);
    assert_eq!(selected.root(), local.root());
    // target stays a real directory; debug is an owned link into the root
    // and release is still the real directory we left there.
    assert!(fs::symlink_metadata(context.link()).unwrap().is_dir());
    assert!(fs::symlink_metadata(context.link())
        .unwrap()
        .file_type()
        .is_dir());
    let debug = fs::symlink_metadata(context.link().join("debug")).unwrap();
    assert!(debug.file_type().is_symlink());
    assert_eq!(
        fs::read_link(context.link().join("debug")).unwrap(),
        member_destination(local.root(), "debug")
    );
    let release = fs::symlink_metadata(context.link().join("release")).unwrap();
    assert!(release.is_dir() && !release.file_type().is_symlink());
    assert_eq!(fs::read(context.link().join("release/keep")).unwrap(), b"x");
    // Both owned members share one immutable root nonce.
    let debug_nonce = committed
        .selected_for("debug")
        .unwrap()
        .nonce()
        .unwrap()
        .to_owned();
    assert_eq!(debug_nonce.len(), 32);
    assert_eq!(committed.selected_for("release").map(Selected::nonce), None);
    // Adoption is idempotent.
    let (again, _, converted) = adopt_profile(&context, committed.clone()).unwrap();
    assert!(!converted);
    assert_eq!(again.generation, committed.generation);
    // Detach restores a real empty member directory and keeps the root.
    let (detached, changed) = detach_profile(&context, &fixture.control, committed).unwrap();
    assert!(changed);
    assert_eq!(detached.state, ReceiptState::Detached);
    let debug = fs::symlink_metadata(context.link().join("debug")).unwrap();
    assert!(debug.is_dir() && !debug.file_type().is_symlink());
    assert!(local.root().is_dir());
    // ... and re-adoption converts that empty directory again.
    let (readopted, _, converted) = adopt_profile(&context, detached).unwrap();
    assert!(converted);
    assert!(fs::symlink_metadata(context.link().join("debug"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(readopted.selected().map(Selected::root), Some(local.root()));
}

#[test]
fn adoption_relinks_to_the_desired_place_instead_of_trusting_the_old_receipt() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture
        .context()
        .with_pool_fallback(Some("fixture registered drive absent"));
    // An older receipt selects an EXTERNAL root whose member links are
    // live. With no registration the desired place is now the local
    // fallback, so adoption must relink even though the links still match
    // the receipt.
    // A registered permanent drive that is NOT usable now (its mount point
    // is absent), so the desired place is the local fallback while the old
    // receipt still selects the external root.
    let registration = serde_json::json!({
        "version": 2,
        "mount_point": fixture.root.join("volume"),
        "volume_uuid": "11111111-2222-3333-4444-555555555555",
        "container_uuid": "22222222-3333-4444-5555-666666666666",
        "reserve_bytes": 16 * 1024 * 1024,
        "cap_bytes": 64 * 1024 * 1024,
        "quota_bytes": null,
        "capacity_bytes": null,
        "registered_ms": 1,
        "mode": "permanent"
    });
    let registration_path = fixture.control.join("removable.json");
    fs::write(
        &registration_path,
        serde_json::to_vec_pretty(&registration).unwrap(),
    )
    .unwrap();
    fs::set_permissions(&registration_path, fs::Permissions::from_mode(0o600)).unwrap();
    let external = fixture.root.join("volume/oxide-outputs/checkout-0000");
    fs::create_dir_all(external.join("debug")).unwrap();
    fs::create_dir_all(external.join("release")).unwrap();
    let marker = Marker {
        version: MARKER_VERSION.to_owned(),
        checkout: fixture.checkout.clone(),
        checkout_id: context.checkout_id.clone(),
        label: "checkout".to_owned(),
        recipe: RECIPE.to_owned(),
        kind: RootKind::External,
        volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
        nonce: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned()),
        checkout_volume_uuid: None,
        generation: 1,
        created_ms: 1,
    };
    fs::write(
        external.join(MARKER),
        serde_json::to_vec_pretty(&marker).unwrap(),
    )
    .unwrap();
    fs::create_dir_all(fixture.checkout.join("target")).unwrap();
    for member in ["debug", "release"] {
        symlink(
            external.join(member),
            fixture.checkout.join("target").join(member),
        )
        .unwrap();
    }
    let mut receipt =
        Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    let selected = Selected {
        kind: RootKind::External,
        root: external.clone(),
        volume_uuid: marker.volume_uuid.clone(),
        nonce: marker.nonce.clone(),
        generation: 1,
    };
    receipt.place_member("debug").selected = Some(selected.clone());
    receipt.place_member("release").selected = Some(selected);
    let (adopted, desired, converted) = adopt_profile(&context, receipt.clone()).unwrap();
    assert!(converted, "a stale external placement was not relinked");
    assert_eq!(desired.kind, RootKind::Local);
    for member in ["debug", "release"] {
        assert_eq!(
            fs::read_link(fixture.checkout.join("target").join(member)).unwrap(),
            member_destination(desired.root(), member),
            "{member}"
        );
    }
    // A second adoption to the same desired place is a no-op.
    let (again, _, converted) = adopt_profile(&context, adopted.clone()).unwrap();
    assert!(
        !converted,
        "a matching desired placement was relinked again"
    );
    assert_eq!(again.generation, adopted.generation);
}

#[test]
fn a_member_link_is_owned_only_through_its_root_marker() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    fs::create_dir_all(spec.root().join("debug")).unwrap();
    fs::create_dir_all(fixture.checkout.join("target")).unwrap();
    let link = fixture.checkout.join("target/debug");
    symlink(spec.root().join("debug"), &link).unwrap();
    assert_eq!(
        link_state(&link, &fixture.checkout, &context.checkout_id).unwrap(),
        TargetState::OwnedLink(spec.root().join("debug"))
    );
    // A member-looking path whose root carries no marker is foreign: the
    // classifier never grants ownership by following an ancestor.
    let foreign = fixture.root.join("foreign/debug");
    fs::create_dir_all(&foreign).unwrap();
    let other = fixture.checkout.join("target/release");
    symlink(&foreign, &other).unwrap();
    assert_eq!(
        link_state(&other, &fixture.checkout, &context.checkout_id).unwrap(),
        TargetState::Foreign
    );
}

/// The exact accepted-proof counterexample (oxide#133 case 15): after a
/// `--target <host triple>` build, `target/<triple>/debug` selects
/// `R/<triple>/debug` and must be classified as owned through the marker at
/// `R`, never as foreign.
#[test]
fn a_host_triple_member_link_is_owned_through_the_marker_at_its_root() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    fs::create_dir_all(fixture.checkout.join("target")).unwrap();
    for member in [
        "debug",
        "release",
        "aarch64-apple-darwin/debug",
        "aarch64-apple-darwin/release",
        "x86_64-unknown-linux-gnu/debug",
    ] {
        let destination = member_destination(spec.root(), member);
        fs::create_dir_all(&destination).unwrap();
        let link = fixture.checkout.join("target").join(member);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&destination, &link).unwrap();
        assert_eq!(
            link_state(&link, &fixture.checkout, &context.checkout_id).unwrap(),
            TargetState::OwnedLink(destination),
            "{member}"
        );
    }
    // The same shape under a root that carries no marker stays foreign: the
    // member suffix is a candidate, never a grant.
    let unmarked = fixture.root.join("unmarked");
    fs::create_dir_all(unmarked.join("aarch64-apple-darwin/debug")).unwrap();
    let unmarked_link = fixture.checkout.join("target/aarch64-apple-darwin/release");
    let _ = fs::remove_file(&unmarked_link);
    symlink(unmarked.join("aarch64-apple-darwin/debug"), &unmarked_link).unwrap();
    assert_eq!(
        link_state(&unmarked_link, &fixture.checkout, &context.checkout_id).unwrap(),
        TargetState::Foreign
    );
    // A root of another checkout's marker is equally not this checkout's.
    let other = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let other_context = other.context();
    let other_spec = other.local();
    ensure_root(&other_context, &other_spec, 1).unwrap();
    fs::create_dir_all(other_spec.root().join("aarch64-apple-darwin/release")).unwrap();
    let foreign_link = fixture
        .checkout
        .join("target/x86_64-unknown-linux-gnu/release");
    fs::create_dir_all(foreign_link.parent().unwrap()).unwrap();
    symlink(
        other_spec.root().join("aarch64-apple-darwin/release"),
        &foreign_link,
    )
    .unwrap();
    assert_eq!(
        link_state(&foreign_link, &fixture.checkout, &context.checkout_id).unwrap(),
        TargetState::Foreign
    );
    // A root whose marker exists but whose member destination does not is
    // dangling, not owned: detach must not act on it.
    let dangling = fixture
        .checkout
        .join("target/aarch64-unknown-linux-gnu/debug");
    fs::create_dir_all(dangling.parent().unwrap()).unwrap();
    symlink(
        member_destination(spec.root(), "aarch64-unknown-linux-gnu/debug"),
        &dangling,
    )
    .unwrap();
    assert_eq!(
        link_state(&dangling, &fixture.checkout, &context.checkout_id).unwrap(),
        TargetState::Dangling(member_destination(
            spec.root(),
            "aarch64-unknown-linux-gnu/debug"
        ))
    );
}

/// A committed profile receipt that places the host-triple member beside
/// `debug` and `release`, with every link live and the root marked.
fn host_triple_profile(fixture: &Fixture) -> (Context, Receipt, PathBuf) {
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    fs::create_dir_all(fixture.checkout.join("target")).unwrap();
    let selected = spec.selected(1);
    let mut receipt =
        Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    for member in ["debug", "release", "aarch64-apple-darwin/debug"] {
        receipt.place_member(member).selected = Some(selected.clone());
        let destination = member_destination(spec.root(), member);
        fs::create_dir_all(&destination).unwrap();
        let link = fixture.checkout.join("target").join(member);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&destination, &link).unwrap();
    }
    write_receipt(&fixture.control, &receipt).unwrap();
    (context, receipt, spec.root().to_path_buf())
}

/// Detach recognises the host-triple and the plain members, converts each
/// owned link to a real empty directory, and keeps the root, its marker and
/// its payload byte for byte. Re-adoption restores every placement.
#[test]
fn detach_recognizes_host_triple_member_links_and_preserves_every_root_byte() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let (context, receipt, root) = host_triple_profile(&fixture);
    let marker = fs::read(root.join(MARKER)).unwrap();
    fs::write(root.join("debug/profileapp"), b"payload").unwrap();
    fs::write(
        root.join("aarch64-apple-darwin/debug/profileapp"),
        b"payload",
    )
    .unwrap();
    let root_metadata = fs::symlink_metadata(&root).unwrap();
    let (detached, changed) = detach_profile(&context, &fixture.control, receipt).unwrap();
    assert!(changed);
    assert_eq!(detached.state, ReceiptState::Detached);
    for member in ["debug", "release", "aarch64-apple-darwin/debug"] {
        let link = fixture.checkout.join("target").join(member);
        let metadata = fs::symlink_metadata(&link).unwrap();
        assert!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "{member} is not a real directory after detach"
        );
        assert!(
            fs::read_dir(&link).unwrap().next().is_none(),
            "{member} is not empty after detach"
        );
        // The placement survives the detach as authority for re-adoption.
        assert!(
            detached
                .placement(member)
                .is_some_and(|placement| placement.selected.is_none()),
            "{member} lost its placement"
        );
    }
    assert!(fs::symlink_metadata(context.link()).unwrap().is_dir());
    // No root, marker or payload byte moved.
    assert_eq!(fs::read(root.join(MARKER)).unwrap(), marker);
    assert_eq!(fs::read(root.join("debug/profileapp")).unwrap(), b"payload");
    assert_eq!(
        fs::read(root.join("aarch64-apple-darwin/debug/profileapp")).unwrap(),
        b"payload"
    );
    assert_eq!(
        fs::symlink_metadata(&root).unwrap().ino(),
        root_metadata.ino()
    );
    // Re-adoption converts every recorded placement again, the triple one
    // included, and the triple link selects its own destination.
    let (readopted, _, converted) = adopt_profile(&context, detached).unwrap();
    assert!(converted);
    assert_eq!(readopted.state, ReceiptState::Committed);
    for member in ["debug", "release", "aarch64-apple-darwin/debug"] {
        let link = fixture.checkout.join("target").join(member);
        assert_eq!(
            fs::read_link(&link).unwrap(),
            member_destination(&root, member),
            "{member}"
        );
    }
}

/// Every failure mode around a host-triple member refuses with no mutation:
/// a wrong destination, an unmarked referent, a dangling destination and a
/// relative (malformed) link each leave the link, the receipt and the other
/// members exactly as they were.
#[test]
fn detach_refuses_unowned_host_triple_links_without_mutating_anything() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let (context, receipt, root) = host_triple_profile(&fixture);
    let triple_link = fixture.checkout.join("target/aarch64-apple-darwin/debug");
    let receipt_before = fs::read(receipt_path(&fixture.control, &context.checkout_id)).unwrap();
    let check_untouched = |label: &str, link: &Path, text: &Path| {
        let error = detach_profile(&context, &fixture.control, receipt.clone())
            .expect_err(&format!("{label}: detach accepted an unowned member"));
        assert!(
            error.contains("target/aarch64-apple-darwin/debug"),
            "{label}: the refusal did not name the member: {error}"
        );
        assert_eq!(fs::read_link(link).unwrap(), text, "{label}");
        assert_eq!(
            fs::read(receipt_path(&fixture.control, &context.checkout_id)).unwrap(),
            receipt_before,
            "{label}: the refusal rewrote the receipt"
        );
        for member in ["debug", "release"] {
            let path = fixture.checkout.join("target").join(member);
            assert_eq!(
                fs::read_link(&path).unwrap(),
                member_destination(&root, member),
                "{label}: {member} was changed"
            );
        }
    };
    // A wrong destination: the link names `R/debug` while the placement
    // promises `R/<triple>/debug`.
    fs::remove_file(&triple_link).unwrap();
    let wrong = member_destination(&root, "debug");
    symlink(&wrong, &triple_link).unwrap();
    check_untouched("wrong destination", &triple_link, &wrong);
    // An unmarked referent of the right shape.
    fs::remove_file(&triple_link).unwrap();
    let unmarked = root.join("unmarked/aarch64-apple-darwin/debug");
    fs::create_dir_all(&unmarked).unwrap();
    symlink(&unmarked, &triple_link).unwrap();
    check_untouched("unmarked referent", &triple_link, &unmarked);
    // A destination that does not exist although the root is ours.
    fs::remove_file(&triple_link).unwrap();
    let vanished = member_destination(&root, "aarch64-apple-darwin/release");
    symlink(&vanished, &triple_link).unwrap();
    check_untouched("dangling destination", &triple_link, &vanished);
    // A relative, malformed link text is never treated as owned.
    fs::remove_file(&triple_link).unwrap();
    let relative = PathBuf::from("../../.oxide-outputs/local/aarch64-apple-darwin/debug");
    symlink(&relative, &triple_link).unwrap();
    check_untouched("relative link", &triple_link, &relative);
}

#[test]
fn adoption_refuses_a_whole_target_link_without_touching_it() {
    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let context = fixture.context();
    let legacy = transition(
        &context,
        &fixture.receipt(),
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    let target = context.link();
    assert!(fs::symlink_metadata(&target)
        .unwrap()
        .file_type()
        .is_symlink());
    let receipt = Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    let refused = adopt_profile(&context, receipt).unwrap_err();
    assert!(refused.contains("whole-target"), "{refused}");
    assert_eq!(fs::read_link(&target).unwrap(), fixture.local().root());
    assert_eq!(legacy.state, ReceiptState::Committed);
    assert!(!target.join("debug").exists());
}

/// `oxide outputs adopt` keeps its **error precedence**: drive mode first,
/// then the control directory, then the pool. A pool provider that fails
/// cannot replace the drive-mode refusal.
#[test]
fn adopt_keeps_its_error_precedence() {
    let fixture = OutputFixture::new("adopt-precedence");
    let failing = || -> Result<Option<Store>, String> {
        Err("the removable registration is unreadable".to_owned())
    };
    let refused = adopt_at(&fixture.context(), failing).unwrap_err();
    assert!(
        refused.starts_with("outputs adopt needs permanent drive mode (mode is unregistered)"),
        "the drive-mode refusal wins: {refused}"
    );
}

/// The leased command path keeps the shipped texts and the shipped durable
/// effect: `detach` and `cancel-reservation` refuse an absent receipt after
/// taking their barrier (one clean mark each), `adopt` refuses before the
/// barrier, no receipt is written and only the shipped fence is.
#[test]
fn the_command_path_answers_with_the_shipped_texts() {
    let fixture = OutputFixture::new("command-path");
    let detach = detach_at(&fixture.context(), || {
        Ok(Some(fixture.store.handle_for_tests()))
    })
    .unwrap_err();
    let cancel = cancel_at(&fixture.context(), || {
        Ok(Some(fixture.store.handle_for_tests()))
    })
    .unwrap_err();
    let adopt = adopt_at(&fixture.context(), || Ok(None)).unwrap_err();
    assert_eq!(
        detach,
        "this checkout has no owned-output receipt; nothing to detach"
    );
    assert_eq!(
        cancel,
        "this checkout has no owned-output receipt; nothing to cancel"
    );
    assert!(
        adopt.starts_with("outputs adopt needs permanent drive mode (mode is unregistered)"),
        "{adopt}"
    );
    assert_eq!(
        next_id(&fixture),
        Some(3),
        "one clean mark per barriered verb"
    );
    let state = fixture.state_bytes().expect("the shipped fence write");
    let text = String::from_utf8_lossy(&state);
    assert!(
        text.contains("\"recorded_accounting\":\"oxide.accounting/1\"")
            || text.contains("\"slot_ceiling\""),
        "{text}"
    );
    assert!(text.contains("\"cleans\":[]"), "{text}");
    assert!(text.contains("\"records\":[]"), "{text}");
    assert!(
        !fixture
            .control()
            .join("outputs")
            .join(format!("{}.json", fixture.checkout_id()))
            .exists(),
        "no receipt was written"
    );
    // The binding refuses a `Prepared` receipt with the shipped text and
    // leaves both the receipt and the tree exactly as they were.
    fixture.write(&fixture.prepared());
    let receipt_path = receipt_path(&fixture.control(), &fixture.checkout_id());
    let receipt_bytes = fs::read(&receipt_path).unwrap();
    let prepared_digest = durable_digest(&fixture);
    let refused = match bind_for_command(&fixture.store, &fixture.checkout, 0, None, None, false) {
        Err(reason) => reason,
        Ok(_) => panic!("the binding refuses a prepared transition"),
    };
    assert_eq!(
            refused,
            "outputs: an interrupted owned-output transition is pending; restore or set a permanent drive registration, then run `oxide outputs adopt` to replay it before building"
        );
    assert_eq!(fs::read(&receipt_path).unwrap(), receipt_bytes);
    assert_eq!(durable_digest(&fixture), prepared_digest);
}

#[test]
fn barge_pins_selected_members_and_detach_releases_only_their_pin() {
    // Close-on-exec descriptors are still inherited between fork and exec.
    // Isolate this released-lock assertion from other fixtures' Git forks.
    const NAME: &str = concat!(
        module_path!(),
        "::barge_pins_selected_members_and_detach_releases_only_their_pin"
    );
    if std::env::var("OXIDE_OUTPUT_PIN_FIXTURE_CHILD").as_deref() != Ok("1") {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture"])
            .env("OXIDE_OUTPUT_PIN_FIXTURE_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "private pin fixture failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let context = fixture.context();
    let receipt = Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    let (committed, selected, _) = adopt_profile(&context, receipt).unwrap();
    assert!(fs::symlink_metadata(context.link()).unwrap().is_dir());
    assert!(fs::symlink_metadata(context.link().join("debug"))
        .unwrap()
        .file_type()
        .is_symlink());
    let (area, id) = crate::prune::test_output_storage(&fixture.control, &selected);
    assert!(area
        .list_without_bytes()
        .unwrap()
        .roots
        .iter()
        .any(|root| root.id == id && root.pinned));
    assert!(area.evict(&id).unwrap_err().to_string().contains("pinned"));
    let foreign = fixture.root.join("foreign");
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("keep"), b"foreign bytes").unwrap();
    symlink(&foreign, selected.root().join("foreign-link")).unwrap();
    let copied_marker = fs::read(selected.root().join(MARKER)).unwrap();
    let holds = area.hold(&id, barge::Mode::Shared).unwrap();
    let (detached, changed) = detach_profile(&context, &fixture.control, committed).unwrap();
    assert!(changed);
    assert!(fs::symlink_metadata(context.link().join("debug"))
        .unwrap()
        .is_dir());
    assert!(area.evict(&id).unwrap_err().to_string().contains("held"));
    drop(holds);
    assert!(area.evict(&id).unwrap().deleted);
    assert!(!selected.root().exists());
    assert_eq!(fs::read(foreign.join("keep")).unwrap(), b"foreign bytes");
    fs::create_dir(selected.root()).unwrap();
    fs::write(selected.root().join("stray"), b"unowned").unwrap();
    fs::write(selected.root().join(MARKER), copied_marker).unwrap();
    let (_, successor, _) = adopt_profile(&context, detached).unwrap();
    assert_ne!(successor.root(), selected.root());
    assert_eq!(fs::read(selected.root().join("stray")).unwrap(), b"unowned");
    let (next_area, next_id) = crate::prune::test_output_storage(&fixture.control, &successor);
    assert_ne!(id, next_id);
    assert!(next_area
        .evict(&next_id)
        .unwrap_err()
        .to_string()
        .contains("pinned"));
}
#[test]
fn output_location_distinguishes_absence_mismatch_and_pressure() {
    use std::sync::Arc;
    let fixture = Fixture::new();
    let mount = fixture.root.join("fake-volume");
    fs::create_dir(&mount).unwrap();
    let uuid = "1E8E7F10-0000-4000-8000-000000000001";
    let os = barge::FakeOs::with_volume(&mount, uuid, 0);
    let storage = oxide_storage::storage::Storage::open_with(
        fixture.root.join("barge-state"),
        Arc::new(os.clone()),
        Arc::new(barge::ManualClock::new(1000)),
    )
    .unwrap();
    let area = storage
        .declare(
            &oxide_storage::storage::AreaConfig::new("outputs", &mount, "outputs")
                .uuid(Some(uuid.to_owned()))
                .folder("oxide-outputs")
                .reserve_bytes(1024)
                .floor_bytes(512),
        )
        .unwrap();
    assert!(
        external_location(&area).unwrap(),
        "low free space never changes output volumes"
    );
    os.set_uuid(&mount, None);
    assert!(!external_location(&area).unwrap());
    os.set_uuid(&mount, Some("1E8E7F10-0000-4000-8000-000000000002"));
    assert!(external_location(&area).is_err());
    os.set_uuid(&mount, Some(uuid));
    assert!(external_location(&area).unwrap());
}

/// A detach makes its opt-out durable before it removes the owned link.
/// The crash window therefore leaves a `Detached` receipt over a live
/// owned link: the next detach finishes the conversion, and a binding in
/// that window refuses (naming the verb that finishes it) instead of
/// silently re-linking the root the checkout opted out of.
#[test]
fn a_whole_target_detach_writes_its_receipt_before_the_link_goes() {
    let (fixture, receipt) = legacy_checkout();
    let root = receipt.selected().unwrap().root().to_path_buf();
    let store = Store::for_tests(&fixture.root.join("pool"));
    let context = fixture.context();
    context.inject_crash("detach-receipt");
    let stopped = detach_at(&context, || Ok(Some(store.handle_for_tests())));
    assert!(stopped.is_err(), "the injected detach stop did not stop");
    // The opt-out is durable although the link is still in place.
    let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    assert!(stored.selects_nothing());
    assert_eq!(stored.state, ReceiptState::Detached);
    assert_eq!(fs::read_link(context.link()).unwrap(), root);
    // The recorded opt-out is never silently re-linked: the link no
    // committed receipt selects is not followed, and the refusal names
    // the detach that finishes it.
    match bind_under_binding(&context, &store, 0, &stored, None, false) {
        Bind::Refuse(error) => assert!(
            error.message().contains("`oxide outputs detach`"),
            "{error}"
        ),
        _ => panic!("a leftover detach link was bound"),
    }
    // The next detach finishes the crash window and keeps every root.
    detach_at(&context, || Ok(Some(store.handle_for_tests()))).unwrap();
    let target = fs::symlink_metadata(context.link()).unwrap();
    assert!(target.is_dir() && !target.file_type().is_symlink());
    assert!(fs::read_dir(context.link()).unwrap().next().is_none());
    assert!(root.is_dir(), "the detach deleted the root");
    let done = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    assert!(done.selects_nothing());
    assert!(done.roots.iter().any(|known| known.root == root));
}

/// The other side of the crash window — and a manual `rm -rf target` — is
/// a `Detached` receipt over an absent target. A binding creates the real
/// directory the detach promises and selects no root: no link is created.
#[test]
fn a_detached_receipt_creates_its_missing_real_target() {
    let fixture = Fixture::new();
    let mut receipt = fixture.receipt();
    receipt.state = ReceiptState::Detached;
    write_receipt(&fixture.control, &receipt).unwrap();
    let context = fixture.context();
    assert!(matches!(context.target().unwrap(), TargetState::Missing));
    let store = Store::for_tests(&fixture.root.join("pool"));
    match bind_under_binding(&context, &store, 0, &receipt, None, false) {
        Bind::Bound { outcome, .. } => {
            assert!(outcome.root.is_none());
            assert_eq!(outcome.note.as_deref(), Some("outputs detached"));
        }
        Bind::Refuse(error) => panic!("a detached checkout refused locally: {error}"),
        Bind::Wait => panic!("a detached checkout waited"),
    }
    let target = fs::symlink_metadata(context.link()).unwrap();
    assert!(target.is_dir() && !target.file_type().is_symlink());
}

/// A profile detach writes the same durable receipt first, so a crash
/// leaves owned member links that a binding refuses (they are never read
/// as the real directories the opt-out names) and the next detach
/// converts.
#[test]
fn a_profile_detach_writes_its_receipt_before_its_member_links() {
    let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    let (context, _, root) = host_triple_profile(&fixture);
    let store = Store::for_tests(&fixture.root.join("pool"));
    context.inject_crash("detach-receipt");
    let stopped = detach_at(&context, || Ok(Some(store.handle_for_tests())));
    assert!(stopped.is_err(), "the injected detach stop did not stop");
    let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
        .unwrap()
        .unwrap();
    assert!(stored.selects_nothing());
    assert_eq!(stored.layout(), LayoutKind::Profile);
    // Every member is still the committed link in this window.
    for member in ["debug", "release", "aarch64-apple-darwin/debug"] {
        assert!(
            fs::symlink_metadata(fixture.checkout.join("target").join(member))
                .unwrap()
                .file_type()
                .is_symlink(),
            "{member} was converted before the receipt was durable"
        );
    }
    match bind_profile(&context, &store, 0, &stored, None, false) {
        Bind::Refuse(error) => assert!(
            error.message().contains("`oxide outputs detach`"),
            "{error}"
        ),
        _ => panic!("a leftover member link was bound"),
    }
    detach_at(&context, || Ok(Some(store.handle_for_tests()))).unwrap();
    for member in ["debug", "release", "aarch64-apple-darwin/debug"] {
        let metadata = fs::symlink_metadata(fixture.checkout.join("target").join(member)).unwrap();
        assert!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "{member} is not a real directory after the finished detach"
        );
    }
    assert!(root.is_dir(), "the detach deleted the root");
}

/// Round 2-A: a refused `adopt` (e.g. a committed whole-target receipt)
/// releases the checkout's clean barrier.
#[test]
fn a_refused_adopt_releases_the_adopt_barrier() {
    let (fixture, legacy) = legacy_checkout();
    let store = pool_store(&fixture.root.join("pool"));
    register_at(&fixture, store.owner_control(), DriveMode::Permanent);
    let context = Context::new(
        Some(store.owner_control().to_path_buf()),
        fixture.checkout.clone(),
    );
    write_receipt(store.owner_control(), &legacy).unwrap();
    let refused = adopt_at(&context, || Ok(Some(store.handle_for_tests()))).unwrap_err();
    assert!(refused.contains("oxide outputs detach"), "{refused}");
    assert!(
        crate::store::active_holders(store.owner_control(), &context.workspace)
            .unwrap()
            .is_empty(),
        "the refused adopt left its clean barrier recorded"
    );
    let again = adopt_at(&context, || Ok(Some(store.handle_for_tests()))).unwrap_err();
    assert!(again.contains("oxide outputs detach"), "{again}");
}

/// Round 2-A: `oxide outputs plan` opens the recorded Barge account
/// read-only. It never prepares the account tree or takes its state lock,
/// so the plan's effect-free promise holds at the account boundary too.
#[test]
fn a_plan_never_prepares_or_locks_the_barge_account() {
    let fixture = Fixture::new();
    let (mount, state) = register_live_volume(&fixture);
    let context = Context::new(Some(fixture.control.clone()), fixture.checkout.clone());
    let before = tree_snapshot(&state);

    let report = plan_report(&context).unwrap();

    assert_eq!(report.mode, "permanent");
    assert!(
        report
            .proposed_root
            .as_ref()
            .is_some_and(|root| root.root.starts_with(&mount)),
        "the plan did not reach the registered volume: {:?}",
        report.proposed_root
    );
    assert_eq!(
        tree_snapshot(&state),
        before,
        "the plan prepared or locked the Barge account"
    );
}

/// Round 2-A: the usage text and the unrecorded-link refusal describe the
/// current profile layout, and the retired `resolve-intent` verb is gone
/// from the dispatch as well as from the text.
#[test]
fn the_usage_and_refusals_describe_the_current_profile_layout() {
    assert!(USAGE.contains("profile member"));
    assert!(!USAGE.contains("version-1 whole-target receipt"));
    assert!(!USAGE.contains("owns one `<checkout>/target` symlink"));
    let words = [OsString::from("resolve-intent"), OsString::from("0123")];
    assert_eq!(run(&words).unwrap_err(), USAGE);

    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    fs::remove_dir(context.link()).ok();
    symlink(spec.root(), context.link()).unwrap();
    let receipt = Receipt::first_with(
        &fixture.checkout,
        &identity::digest(&fixture.checkout.to_string_lossy()),
        LayoutKind::Profile,
    );
    let refused = adopt_profile(&context, receipt).unwrap_err();
    assert!(!refused.contains("refused by this build"), "{refused}");
    assert!(refused.contains("oxide outputs detach"), "{refused}");
    assert!(refused.contains("never adopted"), "{refused}");
}
