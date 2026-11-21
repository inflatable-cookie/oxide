//! End-to-end CLI regression test for Cargo 1.100.0 layout v2 stale-unit
//! pruning through the compiled `oxide` binary on a fresh `TMPDIR` fixture.

#![cfg(unix)]

use std::ffi::{CStr, CString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const OXIDE: &str = env!("CARGO_BIN_EXE_oxide");
const TOOLCHAIN: &str = "1.100.0";
const TOOLCHAIN_LINE: &str = "cargo 1.100.0 (3acc79c0c ";

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let base = std::env::var_os("OXIDE_TEST_INTERNAL_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let template = base.join("stale-unit-prune.XXXXXX");
        let mut name = CString::new(template.as_os_str().as_bytes())
            .unwrap()
            .into_bytes_with_nul();
        // SAFETY: `name` is writable, NUL-terminated and ends in XXXXXX.
        assert!(!unsafe { libc::mkdtemp(name.as_mut_ptr().cast()) }.is_null());
        let name = CStr::from_bytes_until_nul(&name).unwrap();
        Self(fs::canonicalize(PathBuf::from(std::ffi::OsStr::from_bytes(name.to_bytes()))).unwrap())
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn toolchain_installed() -> bool {
    Command::new("rustup")
        .args(["run", TOOLCHAIN, "cargo", "-V"])
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).starts_with(TOOLCHAIN_LINE)
        })
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

fn run_oxide(scratch: &Path, checkout: &Path, pool: &Path, args: &[&str]) -> Output {
    let home = scratch.join("home");
    fs::create_dir_all(&home).unwrap();
    let mut command = Command::new(OXIDE);
    command
        .current_dir(checkout)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("OXIDE_TEST_HOME_FROM_ENV", "1")
        .env("OXIDE_CONFIG", scratch.join("config.toml"))
        .env("OXIDE_POOL", pool)
        .env("OXIDE_POOL_CREATE", "1")
        .env("OXIDE_POOL_CAP", "1GiB")
        .env("OXIDE_SCRATCH_ROOT", scratch)
        .env("OXIDE_COLD_SEED", "0")
        .env("RUSTUP_TOOLCHAIN", TOOLCHAIN)
        .env("TMPDIR", scratch)
        .env("TMP", scratch)
        .env("TEMP", scratch)
        .args(args);
    for name in ["RUSTUP_HOME", "CARGO_HOME", "LANG"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.output().expect("run oxide")
}

fn find_slot_build_dir(pool: &Path) -> PathBuf {
    let slots = pool.join("payload/slots");
    let dirs: Vec<PathBuf> = fs::read_dir(&slots)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| !n.starts_with('.'))
        })
        .collect();
    assert_eq!(dirs.len(), 1, "expected one slot directory in {dirs:?}");
    dirs[0].join("build")
}

fn probe_units(build_dir: &Path) -> Vec<PathBuf> {
    let mut units: Vec<PathBuf> = fs::read_dir(build_dir.join("debug/build/probe"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join("fingerprint").is_dir())
        .collect();
    units.sort();
    units
}

#[test]
fn oxide_cli_prunes_stale_v2_units_and_keeps_active_units_warm() {
    if !toolchain_installed() {
        eprintln!("skipped: Cargo {TOOLCHAIN} is not installed");
        return;
    }
    let scratch = Scratch::new();
    let pool = scratch.path().join("pool");
    let checkout = scratch.path().join("checkout");
    fs::create_dir_all(checkout.join("src")).unwrap();
    fs::write(
        checkout.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nunused_feat = []\n",
    )
    .unwrap();
    fs::write(
        checkout.join("src/lib.rs"),
        "#[cfg(feature = \"unused_feat\")]\npub fn val() -> u32 { 42 }\n#[cfg(not(feature = \"unused_feat\"))]\npub fn val() -> u32 { 7 }\n",
    )
    .unwrap();
    fs::write(
        checkout.join("src/main.rs"),
        "fn main() { println!(\"{}\", probe::val()); }\n",
    )
    .unwrap();

    // 1. Build unused feature variant.
    let out1 = run_oxide(
        scratch.path(),
        &checkout,
        &pool,
        &["build", "--offline", "--features", "unused_feat"],
    );
    assert!(
        out1.status.success(),
        "{}",
        String::from_utf8_lossy(&out1.stderr)
    );
    let build_dir = find_slot_build_dir(&pool);
    let unused_units = probe_units(&build_dir);
    assert_eq!(unused_units.len(), 2);

    // 2. Build default feature in the same warm slot.
    let out2 = run_oxide(scratch.path(), &checkout, &pool, &["build", "--offline"]);
    assert!(
        out2.status.success(),
        "{}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let all_units = probe_units(&build_dir);
    assert_eq!(all_units.len(), 4);
    let active_units: Vec<PathBuf> = all_units
        .iter()
        .filter(|unit| !unused_units.contains(unit))
        .cloned()
        .collect();
    assert_eq!(active_units.len(), 2);

    // 4. Warm build via `oxide` reuses active units as Fresh and prunes old unused ones.
    // A host indexer or reader that touches a stale unit between backdating
    // and closeout makes it look active for that pass. Re-backdating (older
    // on each attempt, so the activity record a missed pass writes cannot
    // keep the units looking recent) and rebuilding still proves the
    // invariant, while a broken pruner fails every attempt.
    for attempt in 0..3 {
        // 3. Age all units and checkout sources to prove an old unused unit
        // is pruned while an equally old active unit Cargo reuses as Fresh
        // is kept warm.
        let old = SystemTime::now() - Duration::from_secs(30 * 86_400 * (attempt + 1) as u64);
        for unit in &all_units {
            backdate_tree(unit, old, old);
        }
        backdate_tree(&checkout.join("src"), old, old);
        set_times(&checkout.join("Cargo.toml"), old, old);
        let out3 = run_oxide(
            scratch.path(),
            &checkout,
            &pool,
            &["build", "-v", "--offline"],
        );
        let stderr3 = String::from_utf8_lossy(&out3.stderr);
        assert!(out3.status.success(), "{stderr3}");
        assert!(stderr3.contains("Fresh probe"), "{stderr3}");
        assert!(!stderr3.contains("Compiling probe"), "{stderr3}");
        if unused_units.iter().all(|unit| !unit.exists()) {
            break;
        }
        assert!(
            attempt < 2,
            "stale units survived 3 backdated warm builds: {unused_units:?}"
        );
    }
    for unit in &active_units {
        assert!(unit.exists(), "active unit {} must remain", unit.display());
    }

    // 5. Next `oxide build` still sees active units as Fresh.
    let out4 = run_oxide(
        scratch.path(),
        &checkout,
        &pool,
        &["build", "-v", "--offline"],
    );
    let stderr4 = String::from_utf8_lossy(&out4.stderr);
    assert!(out4.status.success(), "{stderr4}");
    assert!(stderr4.contains("Fresh probe"), "{stderr4}");
    assert!(!stderr4.contains("Compiling probe"), "{stderr4}");
}
