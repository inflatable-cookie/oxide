//! Cargo's build-directory layout, read from what Cargo wrote.
//!
//! Layout v1 (Cargo 1.95.0 to 1.99.1, and 1.97.1) keeps `<profile>/deps`,
//! `<profile>/.fingerprint` and `<profile>/incremental`. Cargo 1.100.0 makes
//! layout v2 the default: per-unit `<profile>/build/<pkg>/<meta>/{out,fingerprint,run}`,
//! with only `incremental/` and `build/` left at the profile level. Cargo's
//! private opt-out (`__CARGO_TEMPORARY_BUILD_DIR_NEW_LAYOUT_OPT_OUT`) writes
//! v1 on 1.100.0 too, so a directory's shape is the evidence and a Cargo
//! version is only the default.
//!
//! This module reads names and directory shapes. It opens no fingerprint,
//! dep-info or artifact file, and it writes nothing. Owning contracts:
//! [workflow](../../../docs/knowledge/contracts/workflow.md#cargo-version-matrix)
//! and [cold-slot seeding](../../../docs/knowledge/contracts/cold-slot-seeding.md).

use oxide_storage::inventory::{inventory_directory, Category, InventoryLimits};
use std::fs;
use std::path::{Path, PathBuf};

/// `cargo -V` prefixes of verified releases whose default layout is v2.
/// Each entry must also be in `VERIFIED_CARGO`.
const V2_CARGO: [&str; 1] = ["cargo 1.100.0 (3acc79c0c "];

/// Profile directory names Cargo writes beneath a target directory.
const PROFILES: [&str; 2] = ["debug", "release"];
/// Top-level names that are never a triple directory.
const NOT_TRIPLES: [&str; 3] = ["debug", "release", "doc"];
/// Directory entries read from one directory before the scan stops.
const MAX_ENTRIES: usize = 256;
/// Limits for one inventory call. The stray scan splits its own budget.
const MAX_INVENTORY_ENTRIES: u64 = 50_000;
const MAX_INVENTORY_DEPTH: usize = 24;
const MAX_INVENTORY_OUTPUT: usize = 4096;
const MAX_INVENTORY_MS: u64 = 2_000;

/// The layout a verified Cargo release writes by default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Layout {
    V1,
    V2,
}

/// The default layout of a verified Cargo, from its `cargo -V` line.
/// Opt-outs are not seen here; [`holds_v2_build`] reads what was written.
pub(crate) fn default_for(cargo_version: &str) -> Layout {
    if V2_CARGO
        .iter()
        .any(|prefix| cargo_version.starts_with(prefix))
    {
        Layout::V2
    } else {
        Layout::V1
    }
}

/// Profile directories under `target`: `debug` and `release`, and the same
/// under each triple directory. Only directories that exist are returned.
pub(crate) fn profile_members(target: &Path) -> Vec<PathBuf> {
    let mut roots = vec![target.to_path_buf()];
    roots.extend(child_dirs(target).into_iter().filter(|dir| {
        dir.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| !NOT_TRIPLES.contains(&name))
    }));
    let mut members = Vec::new();
    for root in roots {
        for profile in PROFILES {
            let member = root.join(profile);
            if is_directory(&member) {
                members.push(member);
            }
        }
    }
    members
}

/// Whether `path` is a real directory, without following a symlink: a linked
/// profile member or fingerprint is not layout Cargo wrote here.
fn is_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
}

/// Whether a profile member holds layout v2 units: `build/<pkg>/<meta>/fingerprint`.
/// Layout v1 never has that shape, so v1 members never qualify.
pub(crate) fn holds_v2_build(member: &Path) -> bool {
    child_dirs(&member.join("build"))
        .into_iter()
        .any(|package| {
            child_dirs(&package)
                .into_iter()
                .any(|unit| is_directory(&unit.join("fingerprint")))
        })
}

/// Stray intermediates under one checkout's `target/`.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct Stray {
    /// Files under `deps/` and `incremental/`, which both layouts use.
    pub(crate) files: u64,
    pub(crate) bytes: u64,
    /// Files under `build/` of profile members that hold layout v2 units.
    pub(crate) build_files: u64,
    pub(crate) build_bytes: u64,
    /// Whether every scan that ran covered its whole tree.
    pub(crate) complete: bool,
}

/// One call's share of the scan budget. It never truncates to zero: a zero
/// millisecond limit would stop a call before it read anything (many members
/// under a small budget used to do exactly that).
fn per_call_ms(budget_ms: u64, calls: u64) -> u64 {
    (budget_ms.min(MAX_INVENTORY_MS) / calls.max(1)).max(1)
}

/// Count stray intermediates. `budget_ms` bounds the whole scan. Layout v2
/// `build/` is counted only beneath members that hold v2 units, so v1
/// output keeps its existing meaning.
pub(crate) fn scan_stray(target: &Path, budget_ms: u64) -> Result<Stray, String> {
    let members: Vec<PathBuf> = profile_members(target)
        .into_iter()
        .filter(|member| holds_v2_build(member))
        .collect();
    let calls = u64::try_from(members.len() + 1).unwrap_or(u64::MAX);
    let per_call = per_call_ms(budget_ms, calls);
    let limits = InventoryLimits {
        max_entries: MAX_INVENTORY_ENTRIES,
        max_depth: MAX_INVENTORY_DEPTH,
        max_output_bytes: MAX_INVENTORY_OUTPUT,
        max_elapsed_ms: per_call,
    };
    let report = inventory_directory(target, limits).map_err(|error| error.to_string())?;
    let mut stray = Stray {
        complete: report.coverage.complete,
        ..Stray::default()
    };
    for entry in &report.categories {
        if matches!(entry.category, Category::Deps | Category::Incremental) {
            stray.files += entry.accounting.files;
            stray.bytes += entry.accounting.allocated_bytes;
        }
    }
    for member in &members {
        let build = inventory_directory(&member.join("build"), limits)
            .map_err(|error| error.to_string())?;
        stray.complete &= build.coverage.complete;
        for entry in &build.categories {
            stray.build_files += entry.accounting.files;
            stray.build_bytes += entry.accounting.allocated_bytes;
        }
    }
    Ok(stray)
}

/// Immediate subdirectories, at most [`MAX_ENTRIES`] entries read. Unreadable
/// entries are skipped: a missing directory is simply not a layout.
fn child_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .take(MAX_ENTRIES)
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_directory(path))
        .collect()
}

/// Test support shared with the freshness gate's real-Cargo tests.
#[cfg(test)]
pub(crate) mod support {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// The Cargo this module's real-output tests run.
    pub(crate) const TOOLCHAIN: &str = "1.100.0";
    /// The `cargo -V` line of [`TOOLCHAIN`].
    pub(crate) const TOOLCHAIN_LINE: &str = "cargo 1.100.0 (3acc79c0c ";

    /// A fresh directory under `TMPDIR`, removed on drop. Only this path.
    pub(crate) struct Fixture(pub(crate) PathBuf);

    impl Fixture {
        pub(crate) fn new(label: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos());
            let path = std::env::temp_dir().join(format!(
                "oxide-cargo-{label}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("fixture directory");
            Self(path)
        }

        /// Create an empty file, and its parents, beneath the fixture.
        pub(crate) fn file(&self, relative: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
            fs::write(path, b"x").expect("fixture file");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only this fixture's own directory. A failure here must not mask a test failure.
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A Cargo command under the pinned 1.100.0 toolchain, with storage and
    /// wrapper variables from the caller's environment removed.
    pub(crate) fn cargo_1_100(dir: &Path) -> Command {
        let mut command = Command::new("cargo");
        command
            .current_dir(dir)
            .env("RUSTUP_TOOLCHAIN", TOOLCHAIN)
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS");
        command
    }

    /// Whether the pinned toolchain is installed and reports its verified line.
    pub(crate) fn toolchain_installed() -> bool {
        Command::new("rustup")
            .args(["run", TOOLCHAIN, "cargo", "-V"])
            .output()
            .is_ok_and(|output| {
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).starts_with(TOOLCHAIN_LINE)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::support::{cargo_1_100, toolchain_installed, Fixture, TOOLCHAIN};
    use super::*;

    #[test]
    fn default_layout_is_v2_only_for_the_verified_1_100_line() {
        assert_eq!(
            default_for("cargo 1.100.0 (3acc79c0c 2026-11-05)"),
            Layout::V2
        );
        for version in [
            "cargo 1.97.1 (c980f4866 2026-06-30)",
            "cargo 1.99.1 (5f94df478 2026-08-27)",
            "cargo 1.100.0 (5f94df478 2026-08-27)",
        ] {
            assert_eq!(default_for(version), Layout::V1, "{version}");
        }
    }

    #[test]
    fn shapes_separate_v1_from_v2_and_ignore_unrelated_directories() {
        let v1 = Fixture::new("v1");
        v1.file("debug/deps/app-1.d");
        v1.file("debug/.fingerprint/app-1/lib-app");
        assert!(!holds_v2_build(&v1.0.join("debug")));

        let v2 = Fixture::new("v2");
        v2.file("release/build/app/0123456789abcdef/fingerprint/lib-app");
        v2.file("release/build/app/0123456789abcdef/out/libapp-0123456789abcdef.rlib");
        v2.file("release/build/unrelated/notes.txt");
        assert!(holds_v2_build(&v2.0.join("release")));

        // A build directory without a fingerprint is script output, not v2.
        let script = Fixture::new("script");
        script.file("debug/build/app-1/output");
        assert!(!holds_v2_build(&script.0.join("debug")));
    }

    #[test]
    fn profile_members_include_triple_directories() {
        let target = Fixture::new("members");
        target.file("debug/keep");
        target.file("x86_64-test/release/keep");
        target.file("doc/keep");
        let members = profile_members(&target.0);
        assert!(members.contains(&target.0.join("debug")));
        assert!(members.contains(&target.0.join("x86_64-test/release")));
        assert!(!members.iter().any(|member| member.ends_with("doc/debug")));
    }

    /// Regression: layout shapes were read with symlink-following `is_dir()`, so
    /// a linked profile member, triple directory or fingerprint counted as
    /// Cargo's layout and the scan walked outside the tree it was given.
    #[test]
    fn symlinked_members_and_fingerprints_are_not_layout() {
        use std::os::unix::fs::symlink;

        let target = Fixture::new("linked");
        let outside = Fixture::new("linked-outside");
        outside.file("elsewhere/debug/deps/app-1.d");
        outside.file("linked-profile/release/keep");
        outside.file("unit/fingerprint/lib-app");

        // `debug` is a link to another tree's debug directory.
        symlink(outside.0.join("elsewhere/debug"), target.0.join("debug")).unwrap();
        // A triple directory that is a link.
        symlink(
            outside.0.join("linked-profile"),
            target.0.join("x86_64-linked"),
        )
        .unwrap();
        // A v2 unit whose fingerprint directory is a link.
        fs::create_dir_all(target.0.join("release/build/app/meta")).unwrap();
        symlink(
            outside.0.join("unit/fingerprint"),
            target.0.join("release/build/app/meta/fingerprint"),
        )
        .unwrap();

        let members = profile_members(&target.0);
        assert!(!members.contains(&target.0.join("debug")), "{members:?}");
        assert!(
            !members
                .iter()
                .any(|member| member.to_string_lossy().contains("x86_64-linked")),
            "{members:?}"
        );
        assert!(members.contains(&target.0.join("release")), "{members:?}");
        assert!(!holds_v2_build(&target.0.join("release")));
    }

    /// Regression: many members under a small remaining budget truncated a
    /// call's share to `0 ms`, which stopped the scan before it read anything.
    #[test]
    fn a_small_budget_never_collapses_a_call_to_zero_ms() {
        assert_eq!(per_call_ms(0, 8), 1);
        assert_eq!(per_call_ms(1, 4), 1);
        assert_eq!(per_call_ms(1_000, 1), 1_000);
        assert_eq!(per_call_ms(9_000, 3), MAX_INVENTORY_MS / 3);
    }

    #[test]
    fn stray_counts_v2_build_only_beneath_v2_members() {
        let v2 = Fixture::new("stray-v2");
        v2.file("release/build/app/0123456789abcdef/fingerprint/lib-app");
        v2.file("release/build/app/0123456789abcdef/out/libapp.rlib");
        let stray = scan_stray(&v2.0, 1_000).expect("scan");
        assert!(stray.build_files >= 2, "{stray:?}");
        assert!(stray.complete);

        let v1 = Fixture::new("stray-v1");
        v1.file("debug/deps/app-1.d");
        v1.file("debug/build/app-1/output");
        let stray = scan_stray(&v1.0, 1_000).expect("scan");
        assert_eq!(stray.build_files, 0, "v1 build/ keeps its old meaning");
        assert!(stray.files >= 1);
    }

    /// Real Cargo 1.100.0 output, both profiles, from a plain `target/`.
    /// Skipped with a message when the toolchain is not installed.
    #[test]
    fn real_cargo_1_100_writes_v2_intermediates_that_stray_scan_finds() {
        if !toolchain_installed() {
            eprintln!("skipped: Cargo {TOOLCHAIN} is not installed");
            return;
        }
        let fixture = Fixture::new("real");
        fs::write(
            fixture.0.join("Cargo.toml"),
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("manifest");
        fixture.file("src/lib.rs");
        fs::write(fixture.0.join("src/lib.rs"), "pub fn f() -> u32 { 1 }\n").expect("lib");
        for profile_flag in [&[][..], &["--release"][..]] {
            let status = cargo_1_100(&fixture.0)
                .arg("build")
                .args(profile_flag)
                .status()
                .expect("cargo 1.100.0");
            assert!(status.success(), "cargo build {profile_flag:?}");
        }
        let target = fixture.0.join("target");
        for member in ["debug", "release"] {
            assert!(
                holds_v2_build(&target.join(member)),
                "{member} must hold v2 units"
            );
            assert!(
                !target.join(member).join("deps").exists(),
                "{member} must not hold v1 deps/"
            );
        }
        let stray = scan_stray(&target, 2_000).expect("scan");
        assert!(
            stray.build_files > 0,
            "a release build must not read none: {stray:?}"
        );
        assert!(stray.complete);
    }
}
