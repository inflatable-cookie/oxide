//! Unit tests for the descriptor-relative deletion seam (oxide#155).
//!
//! Every race here is deterministic: either the entry is planted while the
//! caller already holds the listing (the plain falsification control), or the
//! debug hook creates it at the exact boundary the seam documents — after the
//! caller's listing, before the removal.

use super::*;
use crate::test_fs::arm_race;
use std::ffi::OsStr;
use std::fs;
use std::os::fd::AsFd;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(1);

/// The seam's caller context for these tests: no capture journal of its own.
fn context<'a>() -> CaptureContext<'a> {
    CaptureContext {
        chain: Vec::new(),
        journal: None,
    }
}

struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        // Ordinary unit fixtures live on the recipe's ordinary test temp
        // (`TMPDIR`, on the selected volume), never inside
        // `OXIDE_TASK_SCRATCH_ROOT`: the shared proof supervisor measures that
        // root while tests run, and these fixtures deliberately hold
        // owner-unreadable directories (0100/0300/0000) to prove the repair and
        // refusal paths. A fixture the supervisor cannot walk would fault its
        // own measurement, so the unreadable windows stay outside it.
        let base = std::env::temp_dir();
        let parent = base.join("owned-delete");
        fs::create_dir_all(&parent).expect("fixture parent");
        loop {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("{label}-{:x}-{id:x}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Self { path },
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create fixture: {error}"),
            }
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.path.join(name);
        fs::create_dir(&path).expect("fixture dir");
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn open(path: &Path) -> fs::File {
    fs::File::open(path).expect("fixture directory")
}

fn name(text: &str) -> std::ffi::CString {
    std::ffi::CString::new(text).expect("name")
}

/// The state the bug is about: the caller listed this directory, then Finder
/// metadata appeared. A plain `rmdir` of exactly that state fails `ENOTEMPTY`,
/// and the seam completes it. This is the old no-relist parent as a control, so
/// the completed cases below cannot pass for an unrelated reason.
#[test]
fn a_plain_removal_fails_on_the_raced_state_the_seam_completes() {
    let fx = Fixture::new("plain-falsification");
    let parent = open(fx.path());
    let tree = fx.dir("slot");
    let dir = open(&tree);
    let name = name("slot");
    // The listing is already held (the file descriptor above predates this
    // write), so this is content the caller never saw.
    fs::write(tree.join(".DS_Store"), vec![b'd'; 6_148]).expect("metadata");

    // SAFETY: the descriptor and C string are live for the call.
    let status = unsafe {
        libc::unlinkat(
            parent.as_fd().as_raw_fd(),
            name.as_ptr(),
            libc::AT_REMOVEDIR,
        )
    };
    assert_ne!(
        status, 0,
        "a plain removal no longer fails on the raced state: the control is vacuous"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ENOTEMPTY),
        "a plain removal no longer fails on the raced state: the control is vacuous"
    );
    assert!(tree.is_dir());

    match rmdir_retrying_finder_metadata(
        parent.as_fd(),
        &name,
        dir.as_fd(),
        OsStr::new("slot"),
        &mut context(),
        &mut || Ok(()),
    ) {
        Rmdir::Removed => {}
        other => panic!("the seam did not complete the same raced state: {other:?}"),
    }
    assert!(!tree.exists(), "the raced directory is removed");
}

/// Both tolerated forms, created by the debug hook after the caller's listing
/// and before the removal, complete the deletion.
#[test]
fn finder_metadata_recreated_before_the_removal_is_drained() {
    let fx = Fixture::new("seam-metadata");
    let parent = open(fx.path());
    let tree = fx.dir("slot");
    let dir = open(&tree);
    let _race = arm_race(OsStr::new("slot"), "metadata", false);

    match rmdir_retrying_finder_metadata(
        parent.as_fd(),
        &name("slot"),
        dir.as_fd(),
        OsStr::new("slot"),
        &mut context(),
        &mut || Ok(()),
    ) {
        Rmdir::Removed => {}
        other => panic!("Finder metadata kept the directory: {other:?}"),
    }
    assert!(!tree.exists(), "the whole directory is gone");
}

/// An enumeration that stops with an error is never read as an ended
/// directory (oxide#155). The re-list has already collected a tolerated
/// metadata entry when the failure is injected, and a foreign entry is still
/// unread: with the former end-of-directory conflation the partial list would
/// have been verified and its metadata entry removed. The fixed seam refuses the
/// whole round before any verify, capture or unlink, so the metadata entry, the
/// foreign entry and the directory are exactly as the writer left them.
#[test]
fn an_incomplete_listing_refuses_before_any_effect() {
    let fx = Fixture::new("seam-enumeration");
    let parent = open(fx.path());
    let tree = fx.dir("slot");
    let dir = open(&tree);
    // The tolerated entries are read first in this fixture's directory order;
    // the assertions below hold whichever order the filesystem returns.
    fs::write(tree.join(".DS_Store"), vec![b'd'; 6_148]).expect("metadata");
    fs::write(tree.join("._member"), vec![b'a'; 16]).expect("metadata");
    fs::write(tree.join("zzz-foreign.txt"), b"foreign").expect("foreign");
    let _fault = crate::test_fs::readdir_fault_after(1, libc::EIO);

    match rmdir_retrying_finder_metadata(
        parent.as_fd(),
        &name("slot"),
        dir.as_fd(),
        OsStr::new("slot"),
        &mut context(),
        &mut || Ok(()),
    ) {
        Rmdir::Failed { error } => assert_eq!(error.raw_os_error(), Some(libc::EIO)),
        other => panic!("an incomplete listing must fail the round: {other:?}"),
    }
    assert!(tree.is_dir(), "nothing was removed");
    assert_eq!(
        fs::read(tree.join(".DS_Store")).expect("metadata kept"),
        vec![b'd'; 6_148]
    );
    assert_eq!(
        fs::read(tree.join("._member")).expect("metadata kept"),
        vec![b'a'; 16]
    );
    assert_eq!(
        fs::read(tree.join("zzz-foreign.txt")).expect("foreign kept"),
        b"foreign"
    );
}

/// A recreated entry that is not tolerated Finder metadata refuses by name and
/// is left exactly where the writer put it.
#[test]
fn a_foreign_entry_is_refused_by_name_and_untouched() {
    let fx = Fixture::new("seam-foreign");
    let parent = open(fx.path());
    let tree = fx.dir("slot");
    let dir = open(&tree);
    let _race = arm_race(OsStr::new("slot"), "foreign", false);

    match rmdir_retrying_finder_metadata(
        parent.as_fd(),
        &name("slot"),
        dir.as_fd(),
        OsStr::new("slot"),
        &mut context(),
        &mut || Ok(()),
    ) {
        Rmdir::Refused { entry } => assert_eq!(entry, "foreign.txt"),
        other => panic!("a foreign entry must refuse: {other:?}"),
    }
    assert!(tree.is_dir(), "the directory with foreign content is kept");
    assert_eq!(
        fs::read(tree.join("foreign.txt")).expect("foreign entry kept"),
        vec![b'd'; 16]
    );
}

/// A metadata-named symlink is never followed: it is refused by name, the link
/// survives and so does the directory it names.
#[test]
fn a_metadata_named_link_is_refused_and_never_followed() {
    let fx = Fixture::new("seam-link");
    let parent = open(fx.path());
    let tree = fx.dir("slot");
    let dir = open(&tree);
    let _race = arm_race(OsStr::new("slot"), "link", false);

    match rmdir_retrying_finder_metadata(
        parent.as_fd(),
        &name("slot"),
        dir.as_fd(),
        OsStr::new("slot"),
        &mut context(),
        &mut || Ok(()),
    ) {
        Rmdir::Refused { entry } => assert_eq!(entry, ".DS_Store"),
        other => panic!("a metadata-named link must refuse: {other:?}"),
    }
    assert!(
        fs::symlink_metadata(tree.join(".DS_Store"))
            .expect("link kept")
            .file_type()
            .is_symlink(),
        "the link itself is untouched"
    );
    assert!(tree.is_dir(), "the directory it names is untouched too");
}

/// A metadata-named directory is not a tolerated file, and it is not removed.
#[test]
fn a_metadata_named_directory_is_refused() {
    let fx = Fixture::new("seam-directory");
    let parent = open(fx.path());
    let tree = fx.dir("slot");
    let dir = open(&tree);
    let _race = arm_race(OsStr::new("slot"), "directory", false);

    match rmdir_retrying_finder_metadata(
        parent.as_fd(),
        &name("slot"),
        dir.as_fd(),
        OsStr::new("slot"),
        &mut context(),
        &mut || Ok(()),
    ) {
        Rmdir::Refused { entry } => assert_eq!(entry, ".DS_Store"),
        other => panic!("a metadata-named directory must refuse: {other:?}"),
    }
    assert!(tree.join(".DS_Store").is_dir(), "the directory is kept");
}

/// A writer that recreates metadata on every attempt uses the fixed bound and
/// then reports exhaustion; nothing becomes a success.
#[test]
fn repeated_recreation_exhausts_the_fixed_bound() {
    let fx = Fixture::new("seam-exhaust");
    let parent = open(fx.path());
    let tree = fx.dir("slot");
    let dir = open(&tree);
    let _race = arm_race(OsStr::new("slot"), "metadata", true);

    match rmdir_retrying_finder_metadata(
        parent.as_fd(),
        &name("slot"),
        dir.as_fd(),
        OsStr::new("slot"),
        &mut context(),
        &mut || Ok(()),
    ) {
        Rmdir::Exhausted { entries } => {
            // The hook recreates both tolerated forms each round: one drain of
            // two entries per attempt and no more.
            assert_eq!(
                entries,
                u64::from(FINDER_RACE_ATTEMPTS) * 2,
                "one drain per attempt, no more"
            );
        }
        other => panic!("a recreating writer must exhaust the bound: {other:?}"),
    }
    assert!(tree.is_dir(), "the directory that was not removed is kept");
}

/// More names than one bounded pass is not this race: the round refuses without
/// deleting anything.
#[test]
fn more_names_than_one_pass_is_refused_without_deleting() {
    let fx = Fixture::new("seam-many");
    let parent = open(fx.path());
    let tree = fx.dir("slot");
    // macOS removes a directory that holds nothing but AppleDouble sidecars, so
    // one `.DS_Store` keeps the removal failing and the count over the bound.
    fs::write(tree.join(".DS_Store"), b"x").expect("metadata");
    for index in 0..FINDER_RACE_MAX_NAMES {
        fs::write(tree.join(format!("._sidecar{index}")), b"x").expect("metadata");
    }
    let dir = open(&tree);
    match rmdir_draining_finder_metadata(
        parent.as_fd(),
        &name("slot"),
        dir.as_fd(),
        OsStr::new("slot"),
        &mut context(),
        &mut || Ok(()),
    ) {
        Rmdir::RefusedMany { entries } => {
            assert!(entries > FINDER_RACE_MAX_NAMES as u64);
        }
        other => panic!("more names than one pass must refuse: {other:?}"),
    }
    assert_eq!(
        fs::read_dir(&tree).expect("still there").count(),
        FINDER_RACE_MAX_NAMES + 1,
        "nothing was deleted"
    );
}

#[test]
fn the_metadata_rule_rejects_near_misses_and_other_kinds() {
    assert!(is_finder_metadata(OsStr::new(".DS_Store")));
    assert!(is_finder_metadata(OsStr::new("._member")));
    assert!(!is_finder_metadata(OsStr::new("._")));
    assert!(!is_finder_metadata(OsStr::new(".ds_store")));
    assert!(!is_finder_metadata(OsStr::new(".DS_Store.bak")));
    assert!(!is_finder_metadata(OsStr::new("DS_Store")));

    let fx = Fixture::new("seam-only-metadata");
    let tree = fx.dir("slot");
    assert!(
        only_finder_metadata(open(&tree).as_fd()).expect("empty lists"),
        "an empty directory holds nothing but metadata"
    );
    fs::write(tree.join(".DS_Store"), b"x").expect("metadata");
    assert!(only_finder_metadata(open(&tree).as_fd()).expect("lists"));

    let link = fx.dir("links");
    symlink(".", link.join(".DS_Store")).expect("link");
    assert!(
        !only_finder_metadata(open(&link).as_fd()).expect("lists"),
        "a metadata name on a link is not metadata"
    );

    let foreign = fx.dir("foreign");
    fs::write(foreign.join("stray"), b"x").expect("stray");
    assert!(!only_finder_metadata(open(&foreign).as_fd()).expect("lists"));

    let named_dir = fx.dir("named-dir");
    fs::create_dir(named_dir.join(".DS_Store")).expect("named dir");
    assert!(!only_finder_metadata(open(&named_dir).as_fd()).expect("lists"));
}

/// The held descriptor is the authority: when the name is rebound to a
/// different directory, the seam refuses instead of draining the held tree or
/// removing the replacement.
#[test]
fn a_rebound_name_is_neither_removed_nor_drained() {
    let fx = Fixture::new("seam-rebound");
    let parent = open(fx.path());
    let held_path = fx.dir("slot");
    let dir = open(&held_path);
    fs::rename(&held_path, fx.path().join("slot-moved")).expect("rename");
    let replacement = fx.dir("slot");
    fs::write(replacement.join(".DS_Store"), b"x").expect("replacement metadata");

    match rmdir_draining_finder_metadata(
        parent.as_fd(),
        &name("slot"),
        dir.as_fd(),
        OsStr::new("slot"),
        &mut context(),
        &mut || Ok(()),
    ) {
        Rmdir::Replaced { entry } => assert_eq!(entry, "slot"),
        other => panic!("a rebound name must be refused: {other:?}"),
    }
    assert!(
        replacement.join(".DS_Store").is_file(),
        "the replacement and its content are untouched"
    );
    assert!(held_path.is_dir() || fx.path().join("slot-moved").is_dir());
}

/// A metadata entry that vanishes between the listing and its own unlink is not
/// our deletion and does not fail the round.
#[test]
fn an_entry_that_vanishes_before_its_unlink_does_not_fail_the_round() {
    let fx = Fixture::new("seam-vanish");
    let parent = open(fx.path());
    let tree = fx.dir("slot");
    fs::write(tree.join(".DS_Store"), b"x").expect("metadata");
    let dir = open(&tree);
    // The round sees one tolerated entry; it is gone before the retry removes
    // the now-empty directory, which is exactly the vanish path.
    fs::remove_file(tree.join(".DS_Store")).expect("vanish");
    match rmdir_retrying_finder_metadata(
        parent.as_fd(),
        &name("slot"),
        dir.as_fd(),
        OsStr::new("slot"),
        &mut context(),
        &mut || Ok(()),
    ) {
        Rmdir::Removed => {}
        other => panic!("an emptied directory is removed: {other:?}"),
    }
    assert!(!tree.exists());
}

/// Successive same-user replacements at the original name before a capture
/// remove **zero** foreign objects: the first capture verifies the object it
/// actually holds, finds it is not the observed one, puts it back and stops.
#[test]
fn replacements_before_a_capture_remove_no_foreign_object_and_stop_at_the_first() {
    let fx = Fixture::new("seam-capture-mismatch");
    let parent = open(fx.path());
    let tree = fx.dir("slot");
    fs::write(tree.join(".DS_Store"), b"first").expect("metadata");
    fs::write(tree.join("._z"), b"second").expect("metadata");
    let dir = open(&tree);
    let (tree_path,) = (tree.clone(),);
    let _swap = crate::test_fs::before_capture(1, move || {
        // A same-user racer replaces both names after the observation pass.
        fs::remove_file(tree_path.join(".DS_Store")).expect("remove first");
        fs::write(tree_path.join(".DS_Store"), b"racer one").expect("racer one");
        fs::remove_file(tree_path.join("._z")).expect("remove second");
        fs::write(tree_path.join("._z"), b"racer two").expect("racer two");
    });
    match rmdir_draining_finder_metadata(
        parent.as_fd(),
        &name("slot"),
        dir.as_fd(),
        OsStr::new("slot"),
        &mut context(),
        &mut || Ok(()),
    ) {
        Rmdir::Mismatch {
            original,
            restored,
            kind,
            ..
        } => {
            assert_eq!(original, ".DS_Store");
            assert!(restored, "the captured object was not put back");
            assert!(
                kind.contains("observed") || kind.contains("type"),
                "the mismatch names its reason: {kind}"
            );
        }
        other => panic!("a pre-capture replacement must stop as a mismatch: {other:?}"),
    }
    // Zero foreign objects were removed: both racer files are still there.
    assert_eq!(
        fs::read(tree.join(".DS_Store")).expect("racer one kept"),
        b"racer one"
    );
    assert_eq!(
        fs::read(tree.join("._z")).expect("racer two kept"),
        b"racer two"
    );
    assert!(tree.is_dir());
}

/// The same race against the *previous* check-then-unlink approach loses a
/// racer's object, which is why the capture exists. This is the falsifying
/// counterfactual, not product code.
#[test]
fn the_previous_check_then_unlink_would_lose_the_racers_object() {
    let fx = Fixture::new("seam-old-approach");
    let parent = open(fx.path());
    let tree = fx.dir("slot");
    fs::write(tree.join(".DS_Store"), b"observed").expect("metadata");
    let dir = open(&tree);
    // The old order: observe, then a racer replaces the name, then unlink by
    // that name.
    let observed = observe_entry(dir.as_fd(), &name(".DS_Store")).expect("observed");
    fs::remove_file(tree.join(".DS_Store")).expect("remove");
    fs::write(tree.join(".DS_Store"), b"racer").expect("racer");
    let current = observe_entry(dir.as_fd(), &name(".DS_Store")).expect("current");
    assert_ne!(
        observed.inode, current.inode,
        "the racer replaced the entry"
    );
    let status = unsafe { libc::unlinkat(dir.as_fd().as_raw_fd(), name(".DS_Store").as_ptr(), 0) };
    assert_eq!(status, 0);
    assert!(
        !tree.join(".DS_Store").exists(),
        "the old approach removed the racer's object by name"
    );
    let _ = parent;
}

/// A symlink swapped in before the capture is not followed: it is captured,
/// verified as the wrong type, put back, and its referent is never touched.
#[test]
fn a_symlink_swapped_before_the_capture_is_not_followed() {
    let fx = Fixture::new("seam-capture-link");
    let outside = fx.dir("outside");
    let referent = outside.join("referent");
    fs::write(&referent, b"keep").expect("referent");
    let parent = open(fx.path());
    let tree = fx.dir("slot");
    fs::write(tree.join(".DS_Store"), b"observed").expect("metadata");
    let dir = open(&tree);
    let (tree_path, referent_path) = (tree.clone(), referent.clone());
    let _swap = crate::test_fs::before_capture(1, move || {
        fs::remove_file(tree_path.join(".DS_Store")).expect("remove");
        symlink(&referent_path, tree_path.join(".DS_Store")).expect("swap in a link");
    });
    match rmdir_draining_finder_metadata(
        parent.as_fd(),
        &name("slot"),
        dir.as_fd(),
        OsStr::new("slot"),
        &mut context(),
        &mut || Ok(()),
    ) {
        Rmdir::Mismatch { restored, .. } => assert!(restored),
        other => panic!("a swapped link must stop as a mismatch: {other:?}"),
    }
    assert_eq!(fs::read(&referent).expect("referent kept"), b"keep");
    assert!(
        fs::symlink_metadata(tree.join(".DS_Store"))
            .expect("link kept")
            .file_type()
            .is_symlink(),
        "the captured link was put back, not removed"
    );
}

/// A directory swapped in before the capture is not recursed into: it is
/// verified as the wrong type, put back and left entire.
#[test]
fn a_directory_swapped_before_the_capture_is_not_recursed() {
    let fx = Fixture::new("seam-capture-dir");
    let parent = open(fx.path());
    let tree = fx.dir("slot");
    fs::write(tree.join(".DS_Store"), b"observed").expect("metadata");
    let dir = open(&tree);
    let (tree_path,) = (tree.clone(),);
    let _swap = crate::test_fs::before_capture(1, move || {
        fs::remove_file(tree_path.join(".DS_Store")).expect("remove");
        fs::create_dir(tree_path.join(".DS_Store")).expect("swap in a directory");
        fs::write(tree_path.join(".DS_Store/inside"), b"content").expect("inside");
    });
    match rmdir_draining_finder_metadata(
        parent.as_fd(),
        &name("slot"),
        dir.as_fd(),
        OsStr::new("slot"),
        &mut context(),
        &mut || Ok(()),
    ) {
        Rmdir::Mismatch { restored, .. } => assert!(restored),
        other => panic!("a swapped directory must stop as a mismatch: {other:?}"),
    }
    assert_eq!(
        fs::read(tree.join(".DS_Store/inside")).expect("directory content kept"),
        b"content"
    );
}

/// A put-back collision keeps **both** names: the captured object stays under
/// its private name and the racer keeps the original one.
#[test]
fn a_put_back_collision_keeps_both_names() {
    let fx = Fixture::new("seam-put-back-collision");
    let tree = fx.dir("slot");
    fs::write(tree.join(".DS_Store"), b"observed").expect("metadata");
    let dir = open(&tree);
    let observed = observe_entry(dir.as_fd(), &name(".DS_Store")).expect("observed");
    let capture = capture_entry(dir.as_fd(), &name(".DS_Store")).expect("capture");
    // A racer takes the original name again, so the exclusive put-back fails.
    fs::write(tree.join(".DS_Store"), b"racer").expect("racer");
    assert!(
        put_back(dir.as_fd(), &capture).is_err(),
        "put-back must not replace a name"
    );
    let private = tree.join(OsStr::from_bytes(capture.private.to_bytes()));
    assert_eq!(
        fs::read(&private).expect("captured object kept"),
        b"observed"
    );
    assert_eq!(
        fs::read(tree.join(".DS_Store")).expect("racer kept"),
        b"racer"
    );
    assert_ne!(
        observed.inode,
        observe_entry(dir.as_fd(), &name(".DS_Store"))
            .expect("current")
            .inode
    );
}

/// A caller's own bounded check runs after *every* recovered metadata unlink: a
/// failure there stops the drain at that entry, keeps the rest of the metadata
/// and the directory, and never reports a removal.
#[test]
fn a_caller_stop_after_one_unlink_keeps_the_rest_of_the_drain() {
    let fx = Fixture::new("seam-mid-drain-stop");
    let parent = open(fx.path());
    let tree = fx.dir("slot");
    fs::write(tree.join(".DS_Store"), b"x").expect("metadata");
    fs::write(tree.join("._member"), b"y").expect("metadata");
    fs::write(tree.join("._sidecar"), b"z").expect("metadata");
    let dir = open(&tree);
    let mut checks = 0;
    let mut between = || {
        checks += 1;
        if checks == 1 {
            Err(DrainStop::Stopped(
                "the registered volume's reserve was reached".to_owned(),
            ))
        } else {
            Ok(())
        }
    };
    match rmdir_draining_finder_metadata(
        parent.as_fd(),
        &name("slot"),
        dir.as_fd(),
        OsStr::new("slot"),
        &mut context(),
        &mut between,
    ) {
        Rmdir::Stopped { reason } => {
            assert!(reason.contains("reserve"), "{reason}");
        }
        other => panic!("a caller stop must be reported: {other:?}"),
    }
    assert_eq!(
        checks, 1,
        "the check runs once per unlinked entry, not per batch"
    );
    assert_eq!(
        fs::read_dir(&tree).expect("kept").count(),
        2,
        "only the entry the check covered was unlinked"
    );
    assert!(tree.is_dir(), "the stopped directory is kept");
}
