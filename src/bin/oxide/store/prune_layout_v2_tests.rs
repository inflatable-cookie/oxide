//! Regression tests for Cargo 1.100.0 layout v2 stale-unit pruning on fresh
//! `TMPDIR` fixtures.
//!
//! Verifies with real Cargo 1.100.0 builds that old unused unit directories in
//! a warm layout-v2 slot are pruned under the slot's exclusive Barge hold while
//! active/current units remain warm (`Fresh`) and usable on the next build, and
//! that layout v1 slots (via Cargo 1.100.0's opt-out) are untouched.

use super::tests::{lease, seed_fixture};

use crate::cargo_layout::support::{cargo_1_100, toolchain_installed, TOOLCHAIN};
use crate::cargo_prune::{finish_with, prepare_with, Limits};
use crate::identity::SlotKey;
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MANIFEST: &str = "\
[package]
name = \"probe\"
version = \"0.1.0\"
edition = \"2021\"

[features]
unused_variant = []
";

fn write_project(dir: &Path) {
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("Cargo.toml"), MANIFEST).unwrap();
    fs::write(
        dir.join("src/lib.rs"),
        "\
#[cfg(feature = \"unused_variant\")]
pub fn f() -> u32 { 99 }
#[cfg(not(feature = \"unused_variant\"))]
pub fn f() -> u32 { 7 }
",
    )
    .unwrap();
    fs::write(
        dir.join("src/main.rs"),
        "fn main() { println!(\"{}\", probe::f()); }\n",
    )
    .unwrap();
}

fn cargo_build(checkout: &Path, build: &Path, features: &[&str], opt_out_v1: bool) -> Output {
    let mut cmd = cargo_1_100(checkout);
    cmd.args(["build", "-v", "--offline"])
        .args(features)
        .env("CARGO_BUILD_BUILD_DIR", build)
        .env("CARGO_TERM_COLOR", "never")
        .env("CARGO_TARGET_DIR", checkout.join("target"));
    if opt_out_v1 {
        cmd.env("__CARGO_TEMPORARY_BUILD_DIR_NEW_LAYOUT_OPT_OUT", "1");
    } else {
        cmd.env_remove("__CARGO_TEMPORARY_BUILD_DIR_NEW_LAYOUT_OPT_OUT");
    }
    cmd.output().expect("cargo 1.100.0")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn set_times(path: &Path, atime: SystemTime, mtime: SystemTime) {
    let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
    let to_ts = |time: SystemTime| {
        let elapsed = time.duration_since(UNIX_EPOCH).unwrap();
        libc::timespec {
            tv_sec: libc::time_t::try_from(elapsed.as_secs()).unwrap(),
            tv_nsec: libc::c_long::try_from(elapsed.subsec_nanos()).unwrap(),
        }
    };
    let times = [to_ts(atime), to_ts(mtime)];
    // SAFETY: `c_path` is NUL-terminated and `times` holds two valid entries.
    assert_eq!(
        unsafe {
            libc::utimensat(
                libc::AT_FDCWD,
                c_path.as_ptr(),
                times.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        },
        0
    );
}

fn backdate_tree(path: &Path, atime: SystemTime, mtime: SystemTime) {
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        let meta = fs::symlink_metadata(entry.path()).unwrap();
        if meta.is_dir() {
            backdate_tree(&entry.path(), atime, mtime);
        } else if meta.is_file() {
            set_times(&entry.path(), atime, mtime);
        }
    }
    set_times(path, atime, mtime);
}

fn probe_unit_dirs(build_dir: &Path) -> Vec<PathBuf> {
    let mut units: Vec<PathBuf> = fs::read_dir(build_dir.join("debug/build/probe"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join("fingerprint").is_dir())
        .collect();
    units.sort();
    units
}

fn limits() -> Limits {
    Limits {
        max_age: Duration::from_secs(7 * 86_400),
        max_elapsed: Duration::from_secs(5),
        max_profiles: 16,
        max_packages: 4_096,
        max_units: 16_384,
        max_entries: 50_000,
        max_depth: 32,
        max_deletions: 256,
    }
}

#[test]
fn old_unused_v2_units_are_pruned_while_active_units_remain_warm_and_usable() {
    if !toolchain_installed() {
        eprintln!("skipped: Cargo {TOOLCHAIN} is not installed");
        return;
    }
    let (root, store, key) = seed_fixture();
    write_project(&key.checkout);

    // Build 1: build the `unused_variant` feature into the fresh layout-v2 slot.
    let first = lease(&store, &key, None).expect("first lease");
    let out1 = cargo_build(
        &key.checkout,
        &first.build_dir,
        &["--features", "unused_variant"],
        false,
    );
    assert!(out1.status.success(), "{}", text(&out1.stderr));
    let unused_units = probe_unit_dirs(&first.build_dir);
    assert_eq!(
        unused_units.len(),
        2,
        "lib and bin units for unused_variant"
    );
    let _ = finish_with(&first, store.control(), None, limits());
    store.release(first, None).unwrap();

    // Build 2: build the default feature in the same warm slot, creating the
    // active lib and bin unit directories alongside the unused ones.
    let second = lease(&store, &key, None).expect("second lease");
    assert!(second.reused);
    let session2 = prepare_with(&second.build_dir, limits());
    let out2 = cargo_build(&key.checkout, &second.build_dir, &[], false);
    assert!(out2.status.success(), "{}", text(&out2.stderr));
    let all_units = probe_unit_dirs(&second.build_dir);
    assert_eq!(all_units.len(), 4, "2 unused + 2 active unit directories");
    let active_units: Vec<PathBuf> = all_units
        .iter()
        .filter(|u| !unused_units.contains(u))
        .cloned()
        .collect();
    assert_eq!(active_units.len(), 2);
    let report2 = finish_with(&second, store.control(), session2, limits());
    assert_eq!(report2.pruned_units, 0, "all units are recent");
    store.release(second, None).unwrap();

    // Build 3: warm reuse of active units; unused units are untouched by Cargo
    // and pruned before slot release while holding the exclusive Barge hold.
    // A host indexer or reader that touches a stale unit between arming and
    // closeout makes it look active for that pass. Re-backdating (older on
    // each attempt, so the activity record a missed pass writes cannot keep
    // the units looking recent) and rebuilding still proves the invariant,
    // while a broken pruner fails every attempt.
    let third = lease(&store, &key, None).expect("third lease");
    assert!(third.reused);
    assert!(third.has_exclusive_hold());
    for attempt in 0..3 {
        // Age both the unused units AND the active units + sources to prove
        // an old unused unit is pruned while an equally old active unit
        // that Cargo reuses as Fresh is kept warm.
        let old = SystemTime::now() - Duration::from_secs(30 * 86_400 * (attempt + 1) as u64);
        for unit in &all_units {
            backdate_tree(unit, old, old);
        }
        backdate_tree(&key.checkout.join("src"), old, old);
        set_times(&key.checkout.join("Cargo.toml"), old, old);
        let session3 = prepare_with(&third.build_dir, limits()).expect("atime supported");
        let out3 = cargo_build(&key.checkout, &third.build_dir, &[], false);
        let stderr3 = text(&out3.stderr);
        assert!(out3.status.success(), "{stderr3}");
        assert!(stderr3.contains("Fresh probe"), "{stderr3}");
        assert!(!stderr3.contains("Compiling probe"), "{stderr3}");
        let report3 = finish_with(&third, store.control(), Some(session3), limits());
        if report3.pruned_units == 2 {
            break;
        }
        assert!(
            attempt < 2,
            "stale units survived 3 backdated warm builds: {all_units:?}"
        );
    }
    for unit in &unused_units {
        assert!(
            !unit.exists(),
            "stale unit {} must be pruned",
            unit.display()
        );
    }
    for unit in &active_units {
        assert!(
            unit.exists(),
            "active unit {} must stay warm",
            unit.display()
        );
    }
    store.release(third, None).unwrap();

    // Build 4: next build still sees active units as Fresh and produces working binary.
    let fourth = lease(&store, &key, None).expect("fourth lease");
    let out4 = cargo_build(&key.checkout, &fourth.build_dir, &[], false);
    let stderr4 = text(&out4.stderr);
    assert!(out4.status.success(), "{stderr4}");
    assert!(stderr4.contains("Fresh probe"), "{stderr4}");
    assert!(!stderr4.contains("Compiling probe"), "{stderr4}");
    let bin = std::process::Command::new(key.checkout.join("target/debug/probe"))
        .output()
        .expect("probe binary");
    assert_eq!(text(&bin.stdout).trim(), "7");
    store.release(fourth, None).unwrap();

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn real_cargo_1_100_v1_opt_out_slot_is_never_pruned() {
    if !toolchain_installed() {
        eprintln!("skipped: Cargo {TOOLCHAIN} is not installed");
        return;
    }
    let (root, store, _) = seed_fixture();
    let v1_key = SlotKey {
        checkout: root.join("v1-checkout"),
        identity: "v1-identity".into(),
        compatibility: "v1-compat".into(),
    };
    write_project(&v1_key.checkout);

    let first = lease(&store, &v1_key, None).expect("v1 first lease");
    let out1 = cargo_build(&v1_key.checkout, &first.build_dir, &[], true);
    assert!(out1.status.success(), "{}", text(&out1.stderr));
    assert!(first.build_dir.join("debug/deps").is_dir());
    assert!(first.build_dir.join("debug/.fingerprint").is_dir());
    store.release(first, None).unwrap();

    let old = SystemTime::now() - Duration::from_secs(30 * 86_400);
    backdate_tree(&store.payload, old, old);

    let second = lease(&store, &v1_key, None).expect("v1 second lease");
    let session = prepare_with(&second.build_dir, limits());
    let out2 = cargo_build(&v1_key.checkout, &second.build_dir, &[], true);
    assert!(out2.status.success(), "{}", text(&out2.stderr));
    let report = finish_with(&second, store.control(), session, limits());
    assert_eq!(report.pruned_units, 0);
    assert!(second.build_dir.join("debug/deps").is_dir());
    assert!(second.build_dir.join("debug/.fingerprint").is_dir());
    store.release(second, None).unwrap();

    fs::remove_dir_all(root).unwrap();
}
