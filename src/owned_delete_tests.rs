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

struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
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

    match rmdir_retrying_finder_metadata(parent.as_fd(), &name, dir.as_fd(), OsStr::new("slot")) {
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
    ) {
        Rmdir::Removed => {}
        other => panic!("Finder metadata kept the directory: {other:?}"),
    }
    assert!(!tree.exists(), "the whole directory is gone");
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
    ) {
        Rmdir::Removed => {}
        other => panic!("an emptied directory is removed: {other:?}"),
    }
    assert!(!tree.exists());
}

/// The directory binding is revalidated for *every* entry, not once per round:
/// a name rebound just before an entry's unlink stops the drain and leaves the
/// replacement untouched.
#[test]
fn a_rebind_during_the_drain_stops_it_before_the_next_unlink() {
    let fx = Fixture::new("seam-drain-rebind");
    let parent = open(fx.path());
    let tree = fx.dir("slot");
    fs::write(tree.join(".DS_Store"), b"x").expect("metadata");
    fs::write(tree.join("._member"), b"y").expect("metadata");
    let dir = open(&tree);
    let held = fx.path().join("slot-moved");
    let replacement = fx.path().join("slot-replacement");
    let (parent_path, replacement_path) = (fx.path().to_path_buf(), replacement.clone());
    let _rebind = crate::test_fs::before_unlink(move || {
        fs::rename(parent_path.join("slot"), parent_path.join("slot-moved")).expect("rename away");
        fs::create_dir(&parent_path.join("slot")).expect("replacement directory");
    });
    match rmdir_draining_finder_metadata(
        parent.as_fd(),
        &name("slot"),
        dir.as_fd(),
        OsStr::new("slot"),
    ) {
        Rmdir::Replaced { entry } => assert_eq!(entry, "slot"),
        other => panic!("a rebound name must stop the drain: {other:?}"),
    }
    assert!(held.is_dir(), "the held directory is still there");
    assert!(
        fs::read_dir(fx.path().join("slot"))
            .expect("replacement kept")
            .count()
            == 0,
        "the replacement is untouched"
    );
    assert!(
        !replacement_path.exists(),
        "the control did not create an extra directory"
    );
}

/// Permission repair never follows a link: a metadata-shaped replacement that
/// is a symlink to an unreadable directory outside the tree must leave the
/// referent's mode exactly as it was, and the deletion must refuse.
#[test]
fn unreadable_directory_repair_never_follows_a_link() {
    let fx = Fixture::new("seam-repair-link");
    let outside = fx.dir("outside");
    let referent = outside.join("referent");
    fs::create_dir(&referent).expect("referent");
    fs::set_permissions(&referent, fs::Permissions::from_mode(0o000)).expect("referrant mode");
    let before = fs::symlink_metadata(&referent)
        .expect("referent metadata")
        .permissions()
        .mode();

    let parent = open(fx.path());
    let tree = fx.dir("slot");
    let locked = tree.join("locked");
    fs::create_dir(&locked).expect("locked");
    // The verified entry is replaced by a link to the unreadable referent
    // before the seam reaches it, so a following repair would chmod it.
    fs::remove_dir(&locked).expect("remove");
    symlink(&referent, &locked).expect("link");
    let dir = open(&tree);

    // The recursion refuses a link (it is unlinked, never followed), so the
    // referent's mode is what proves nothing followed it.
    let _ = rmdir_draining_finder_metadata(
        parent.as_fd(),
        &name("slot"),
        dir.as_fd(),
        OsStr::new("slot"),
    );
    let after = fs::symlink_metadata(&referent)
        .expect("referent metadata")
        .permissions()
        .mode();
    assert_eq!(before & 0o777, after & 0o777, "the referent's mode changed");
    // The fixture's own removal needs the referent readable again.
    fs::set_permissions(&referent, fs::Permissions::from_mode(0o700)).expect("restore");
}
