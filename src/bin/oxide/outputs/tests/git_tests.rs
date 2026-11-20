use super::*;
use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn a_first_conversion_refuses_a_rule_that_would_not_cover_the_link() {
    // A trailing-slash rule matches directories only, so it never covers
    // the link. A missing target leaves Git answering "not ignored"; a
    // real empty target makes Git answer for a *directory* with that rule,
    // and that answer must not be read as eligibility either.
    for (label, rule, empty, reason) in [
        (
            "missing target, /target/",
            "/target/",
            false,
            "does not ignore",
        ),
        (
            "empty target, /target/",
            "/target/",
            true,
            "directories only",
        ),
        (
            "missing target, target/",
            "target/",
            false,
            "does not ignore",
        ),
        (
            "missing target, **/target/",
            "**/target/",
            false,
            "does not ignore",
        ),
        ("empty target, target/", "target/", true, "directories only"),
        (
            "empty target, **/target/",
            "**/target/",
            true,
            "directories only",
        ),
    ] {
        let fixture = Fixture::in_tree("", &format!("{rule}\n/.oxide-outputs/\n"));
        if empty {
            fs::create_dir(fixture.checkout.join("target")).unwrap();
        }
        let refused = transition(
            &fixture.context(),
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap_err();
        assert!(refused.contains(reason), "{label}: {refused}");
        assert!(refused.contains("not `target/`"), "{label}: {refused}");
        assert!(
            refused.ends_with("nothing was changed"),
            "{label}: {refused}"
        );
        // No receipt, no root and no link.
        let loaded = read_receipt(
            &fixture.control,
            &fixture.checkout,
            &fixture.context().checkout_id,
        )
        .unwrap();
        assert!(
            loaded.is_none(),
            "{label}: a refused conversion wrote a receipt"
        );
        assert!(
            !fixture.local().root().exists(),
            "{label}: a refused conversion created a root"
        );
        let link = fixture.checkout.join("target");
        if empty {
            // The real empty directory is exactly as it was.
            assert!(
                fs::symlink_metadata(&link).unwrap().is_dir(),
                "{label}: the empty target was removed or replaced"
            );
            assert!(
                fs::read_dir(&link).unwrap().next().is_none(),
                "{label}: the empty target changed"
            );
        } else {
            assert!(!os_symlink_exists(&link), "{label}: a target appeared");
        }
    }

    for (label, ignore) in [
        ("negated", "target\n!/target\n/.oxide-outputs/\n"),
        ("unignored", "/.oxide-outputs/\n"),
    ] {
        let fixture = Fixture::in_tree("", ignore);
        let refused = transition(
            &fixture.context(),
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap_err();
        let expected = if label == "negated" {
            "re-includes"
        } else {
            "does not ignore"
        };
        assert!(refused.contains(expected), "{label}: {refused}");
        assert!(
            refused.contains("nothing was changed"),
            "{label}: {refused}"
        );
        assert!(
            !fixture.local().root().exists(),
            "{label}: a refused conversion created a root"
        );
    }
}

#[test]
fn a_first_conversion_accepts_a_non_directory_ignore_rule() {
    for rule in ["/target", "target"] {
        let fixture = Fixture::in_tree("", &format!("{rule}\n/.oxide-outputs/\n"));
        let receipt = transition(
            &fixture.context(),
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        assert_eq!(receipt.state, ReceiptState::Committed, "{rule}");
        assert!(
            os_symlink_exists(&fixture.checkout.join("target")),
            "{rule}"
        );
        let repo = fixture.repo();
        let ignore = fs::read(repo.join(".gitignore")).unwrap();
        let listed = status_entries_naming(&fixture.checkout, "target");
        assert!(
            listed.is_empty(),
            "{rule}: Git status lists the owned link: {listed:?}"
        );
        // The check is non-vacuous: with the rule gone the published link
        // is detected at its exact path.
        fs::write(repo.join(".gitignore"), b"").unwrap();
        let exposed = status_entries_naming(&fixture.checkout, "target");
        assert!(
            exposed.iter().any(|entry| entry.get(3..) == Some("target")),
            "{rule}: the detector missed an exposed link: {exposed:?}"
        );
        fs::write(repo.join(".gitignore"), &ignore).unwrap();
    }
}

#[test]
fn a_nested_first_conversion_uses_the_exact_repository_relative_path() {
    let relative = "apps/farmyard/target";
    let allowed = Fixture::in_tree("apps/farmyard", "/apps/farmyard/target\n.oxide-outputs/\n");
    transition(
        &allowed.context(),
        &allowed.receipt(),
        &allowed.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    assert!(os_symlink_exists(&allowed.checkout.join("target")));
    let repo = allowed.repo();
    let ignore = fs::read(repo.join(".gitignore")).unwrap();
    let listed = status_entries_naming(&allowed.checkout, relative);
    assert!(
        listed.is_empty(),
        "Git status lists the nested link: {listed:?}"
    );
    // A wholly untracked workspace collapses to one entry such as `apps/`
    // in plain `git status`; every untracked file is listed here, so the
    // control detects the exact exposed nested link.
    fs::write(repo.join(".gitignore"), b"").unwrap();
    let exposed = status_entries_naming(&allowed.checkout, relative);
    assert!(
        exposed.iter().any(|entry| entry.get(3..) == Some(relative)),
        "the nested detector missed an exposed link: {exposed:?}"
    );
    fs::write(repo.join(".gitignore"), &ignore).unwrap();

    // The same rule with a real empty target makes Git answer for a
    // directory, which the winning rule's form then exposes.
    let empty = Fixture::in_tree("apps/farmyard", "/apps/farmyard/target/\n.oxide-outputs/\n");
    fs::create_dir(empty.checkout.join("target")).unwrap();
    let reason = transition(
        &empty.context(),
        &empty.receipt(),
        &empty.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap_err();
    assert!(reason.contains("directories only"), "{reason}");
    assert!(
        reason.contains(&format!("not `{relative}/`"))
            && reason.contains(&format!("`/{relative}`")),
        "{reason}"
    );
    assert!(!empty.local().root().exists());

    // A missing target with the trailing-slash form refuses as unignored.
    let refused = Fixture::in_tree("apps/farmyard", "/apps/farmyard/target/\n.oxide-outputs/\n");
    let reason = transition(
        &refused.context(),
        &refused.receipt(),
        &refused.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap_err();
    assert!(reason.contains("does not ignore"), "{reason}");
    assert!(
        reason.contains(&format!("not `{relative}/`"))
            && reason.contains(&format!("`/{relative}`")),
        "{reason}"
    );
    assert!(!refused.local().root().exists());
}

#[test]
fn a_tracked_or_nongit_checkout_refuses_a_first_conversion() {
    // Tracked in the index but absent from the working tree: a link there
    // would be a tracked path.
    let tracked = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    fs::write(tracked.checkout.join("target"), b"tracked").unwrap();
    let added = Command::new("git")
        .args(["add", "-f", "target"])
        .current_dir(&tracked.checkout)
        .status()
        .unwrap();
    assert!(added.success());
    fs::remove_file(tracked.checkout.join("target")).unwrap();
    let refused = transition(
        &tracked.context(),
        &tracked.receipt(),
        &tracked.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap_err();
    assert!(refused.contains("already tracks"), "{refused}");
    assert!(refused.contains("git rm --cached target"), "{refused}");
    assert!(!tracked.local().root().exists());

    let plain = Fixture::without_git();
    let refused = transition(
        &plain.context(),
        &plain.receipt(),
        &plain.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap_err();
    assert!(refused.contains("could not confirm"), "{refused}");
    assert!(!plain.local().root().exists());
    assert!(!os_symlink_exists(&plain.checkout.join("target")));
}

/// The automatic first conversion asks the same ignore prerequisite for
/// every member link it publishes (oxide#160) and treats every
/// non-`Eligible` answer as ineligibility (oxide#154): an expected refusal
/// must leave the build usable, so it cannot be a transition error. Each
/// class is the answer an explicit adoption refuses with, so the two
/// presentations cannot drift.
#[test]
fn automatic_adoption_is_ineligible_for_every_ignore_safety_refusal() {
    // Rules that genuinely cover `target/debug` are eligible, including
    // the directory-only forms the partner link cannot use.
    for rule in ["/target", "target", "target/", "/target/", "**/target/"] {
        let fixture = Fixture::in_tree("", &format!("{rule}\n/.oxide-outputs/\n"));
        for member in ["debug", "release"] {
            assert_eq!(
                first_link_git_answer(&fixture.context(), Some(member)).verdict,
                LinkGitVerdict::Eligible,
                "{rule} {member}: a covering rule is not eligible"
            );
        }
    }
    for (label, fixture) in [
        ("unignored", Fixture::in_tree("", "/.oxide-outputs/\n")),
        (
            "negated",
            Fixture::in_tree("", "target\n!/target\n/.oxide-outputs/\n"),
        ),
    ] {
        let answer = first_link_git_answer(&fixture.context(), Some("debug"));
        assert_ne!(
            answer.verdict,
            LinkGitVerdict::Eligible,
            "{label}: a refused conversion is eligible for automatic adoption"
        );
        assert!(answer.fix.is_some(), "{label}: no fix was derived");
    }
    // A tracked member link is never eligible.
    let tracked = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    create_tracked_member(&tracked.checkout);
    assert_eq!(
        first_link_git_answer(&tracked.context(), Some("debug")).verdict,
        LinkGitVerdict::Tracked
    );
    // A non-repository has no work tree to confirm: unknown, never
    // permission, and no fix is available because there is no root.
    let plain = Fixture::without_git();
    let answer = first_link_git_answer(&plain.context(), Some("debug"));
    assert!(
        matches!(answer.verdict, LinkGitVerdict::Unknown(_)),
        "{answer:?}"
    );
    assert!(answer.fix.is_none(), "{answer:?}");
    assert!(
        !plain.local().root().exists() && !os_symlink_exists(&plain.checkout.join("target")),
        "the eligibility answer had an effect"
    );
}

/// Track `target/debug` in the index while the working tree has neither
/// the file nor the `target` directory.
fn create_tracked_member(checkout: &Path) {
    fs::create_dir_all(checkout.join("target")).unwrap();
    fs::write(checkout.join("target").join("debug"), b"tracked").unwrap();
    let added = Command::new("git")
        .args(["add", "-f", "target/debug"])
        .current_dir(checkout)
        .status()
        .unwrap();
    assert!(added.success());
    fs::remove_file(checkout.join("target").join("debug")).unwrap();
    fs::remove_dir(checkout.join("target")).unwrap();
}

/// `oxide init` names the rule that covers the member links a first
/// conversion publishes and never edits the file (oxide#154): an
/// unignored path gets the exact non-slash rule, a nested workspace names
/// its repository-relative spelling, a tracked member is named, and a
/// covering rule (including a directory-only one, which genuinely covers
/// `target/<member>`) is silent.
#[test]
fn the_init_diagnostic_names_the_exact_non_directory_rule() {
    let ready = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    assert_eq!(gitignore_diagnostic(&ready.checkout), None);
    // A directory-only parent rule covers the member link, so it is not
    // diagnosed (oxide#160's profile layout).
    let directory_only = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
    assert_eq!(gitignore_diagnostic(&directory_only.checkout), None);

    let unignored = Fixture::in_tree("", "/.oxide-outputs/\n");
    let warning = gitignore_diagnostic(&unignored.checkout).expect("no diagnostic");
    assert!(warning.contains("`/target`"), "{warning}");
    assert!(
        warning.contains(&unignored.repo().join(".gitignore").display().to_string()),
        "{warning}"
    );
    assert!(
        warning.contains("leaves every .gitignore rule exactly as it is"),
        "{warning}"
    );
    assert_eq!(
        fs::read(unignored.repo().join(".gitignore")).unwrap(),
        b"/.oxide-outputs/\n",
        "the diagnostic edited the file"
    );

    let nested = Fixture::in_tree("apps/farmyard", "/.oxide-outputs/\n");
    let warning = gitignore_diagnostic(&nested.checkout).expect("no diagnostic");
    assert!(warning.contains("`/apps/farmyard/target`"), "{warning}");
    assert!(
        warning.contains("`apps/farmyard/target/debug`"),
        "{warning}"
    );

    let tracked = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    create_tracked_member(&tracked.checkout);
    let warning = gitignore_diagnostic(&tracked.checkout).expect("no diagnostic");
    assert!(warning.contains("already tracks"), "{warning}");
    assert!(
        warning.contains("git rm --cached target/debug"),
        "{warning}"
    );

    // Not a repository at all: init adopts it outside Git and there is
    // nothing for a first conversion to ignore.
    let plain = Fixture::without_git();
    assert_eq!(gitignore_diagnostic(&plain.checkout), None);
}

#[test]
fn the_verbose_ignore_record_parser_is_strict() {
    let rule = parse_ignore_rule(b".gitignore\x0010\x00/target\x00target\x00", "target").unwrap();
    assert_eq!(rule.source, ".gitignore");
    assert_eq!(rule.line, 10);
    assert_eq!(rule.pattern, "/target");
    // Another pathname, a missing terminator, extra records and empty
    // fields are all malformed, never a rule.
    assert!(parse_ignore_rule(b".gitignore\x001\x00/target\x00other\x00", "target").is_err());
    assert!(parse_ignore_rule(b".gitignore\x001\x00/target\x00target", "target").is_err());
    assert!(parse_ignore_rule(b".gitignore\x001\x00/target\x00target\x00x\x00", "target").is_err());
    assert!(parse_ignore_rule(b"\x001\x00/target\x00target\x00", "target").is_err());
    assert!(parse_ignore_rule(b".gitignore\x00x\x00/target\x00target\x00", "target").is_err());
}

fn probe_answer(code: Option<i32>, stdout: &[u8]) -> GitProbe {
    GitProbe {
        code,
        stdout: stdout.to_vec(),
    }
}

/// Round 2-A: the local-root ignore question never fails open and always
/// asks the hardened probe. A spawn error, an unusable `rev-parse` answer
/// and a signalled probe all refuse; only a positively non-work-tree
/// checkout makes an unknown check-ignore answer safe.
#[test]
fn the_local_root_ignore_check_fails_closed() {
    let asked: std::cell::RefCell<Vec<Vec<String>>> = std::cell::RefCell::new(Vec::new());
    let record = |args: &[&str]| {
        asked
            .borrow_mut()
            .push(args.iter().map(|arg| (*arg).to_owned()).collect());
    };
    let probe = |answer: fn(&[&str]) -> Result<GitProbe, String>| move |args: &[&str]| answer(args);

    // A spawn failure is never "not tracked".
    let refused = git_would_track_with(
        probe(|_| Err("cannot run git check-ignore: No such file".to_owned())),
        ".oxide-outputs/local",
    )
    .unwrap_err();
    assert!(refused.contains("cannot run git"), "{refused}");

    // check-ignore could not decide, and the checkout IS a work tree.
    let refused = git_would_track_with(
        probe(|args| match args.first() {
            Some(&"check-ignore") => Ok(probe_answer(Some(128), b"")),
            _ => Ok(probe_answer(Some(0), b"true\n")),
        }),
        ".oxide-outputs/local",
    )
    .unwrap_err();
    assert!(refused.contains("could not decide"), "{refused}");

    // The rev-parse probe fails to spawn, or dies: unknown, never safe.
    for answer in [
        (|_: &[&str]| -> Result<GitProbe, String> { Err("cannot run git rev-parse".to_owned()) })
            as fn(&[&str]) -> Result<GitProbe, String>,
        |_: &[&str]| -> Result<GitProbe, String> { Ok(probe_answer(None, b"")) },
    ] {
        let refused = git_would_track_with(
            |args| match args.first() {
                Some(&"check-ignore") => Ok(probe_answer(Some(128), b"")),
                _ => answer(args),
            },
            ".oxide-outputs/local",
        )
        .unwrap_err();
        assert!(
            refused.contains("cannot run git") || refused.contains("could not decide"),
            "{refused}"
        );
    }

    // Positively outside a work tree: nothing can be tracked.
    assert!(!git_would_track_with(
        |args| match args.first() {
            Some(&"check-ignore") => Ok(probe_answer(Some(128), b"")),
            _ => Ok(probe_answer(Some(0), b"false\n")),
        },
        ".oxide-outputs/local",
    )
    .unwrap());
    // `rev-parse` refusing outside a repository is that same answer.
    assert!(!git_would_track_with(
        |args| match args.first() {
            Some(&"check-ignore") => Ok(probe_answer(Some(128), b"")),
            _ => Ok(probe_answer(Some(128), b"")),
        },
        ".oxide-outputs/local",
    )
    .unwrap());

    // The probe is asked with the exact literal separator, and the two
    // real answers map to tracked/not tracked.
    asked.borrow_mut().clear();
    assert!(!git_would_track_with(
        |args| {
            record(args);
            Ok(probe_answer(Some(0), b""))
        },
        ".oxide-outputs/local",
    )
    .unwrap());
    assert!(git_would_track_with(
        |args| {
            record(args);
            Ok(probe_answer(Some(1), b""))
        },
        ".oxide-outputs/local",
    )
    .unwrap());
    let asked = asked.borrow();
    assert_eq!(asked.len(), 2);
    for args in asked.iter() {
        assert_eq!(
            args.as_slice(),
            ["check-ignore", "-q", "--", ".oxide-outputs/local"]
        );
    }
}

/// The same question end to end: a real work tree answers from its own
/// ignore rules, and a directory outside every work tree has no tracking
/// risk.
#[test]
fn the_local_root_ignore_check_reads_the_checkout_work_tree() {
    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    assert!(!git_would_track(&fixture.checkout, ".oxide-outputs/local").unwrap());
    assert!(!git_would_track(&fixture.checkout, "target/debug").unwrap());
    assert!(git_would_track(&fixture.checkout, "not-ignored").unwrap());
    let outside = fixture.root.join("outside");
    fs::create_dir(&outside).unwrap();
    assert!(!git_would_track(&outside, ".oxide-outputs/local").unwrap());
}
