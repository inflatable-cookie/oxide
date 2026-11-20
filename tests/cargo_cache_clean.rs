//! Integration tests for `oxide clean --cargo-cache` under Cargo 1.100.0 and 1.97.1.
//!
//! Every fixture lives under `OXIDE_TEST_INTERNAL_ROOT` / `TMPDIR` with an
//! isolated `HOME`, `CARGO_HOME`, `OXIDE_CONFIG` and `OXIDE_POOL`.

use std::ffi::{CStr, CString};
use std::fs::{self, OpenOptions};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const OXIDE: &str = env!("CARGO_BIN_EXE_oxide");

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let base = std::env::var_os("OXIDE_TEST_INTERNAL_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let template = base.join("cargo-cache-clean.XXXXXX");
        let mut name = CString::new(template.as_os_str().as_bytes())
            .unwrap()
            .into_bytes_with_nul();
        // SAFETY: `name` is writable, NUL-terminated and ends in `XXXXXX`.
        assert!(!unsafe { libc::mkdtemp(name.as_mut_ptr().cast()) }.is_null());
        let name = CStr::from_bytes_until_nul(&name).unwrap();
        Self(PathBuf::from(std::ffi::OsStr::from_bytes(name.to_bytes())))
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

fn installed_rustup_home() -> PathBuf {
    let invoking_home = std::env::var_os("HOME").expect("invoking HOME");
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(invoking_home).join(".rustup"));
    fs::canonicalize(rustup_home).expect("installed RUSTUP_HOME")
}

fn toolchain_installed_matching(toolchain: &str, version_prefix: &str) -> bool {
    Command::new("rustup")
        .args(["run", toolchain, "cargo", "-V"])
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).starts_with(version_prefix)
        })
}

fn oxide_command(
    scratch: &Path,
    bin_override: Option<&Path>,
    cargo_home: &Path,
    cwd: &Path,
) -> Command {
    let home = scratch.join("home");
    fs::create_dir_all(&home).unwrap();
    let pool = scratch.join("absent-pool");
    let path = match bin_override {
        Some(bin) => format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
        None => std::env::var("PATH").unwrap_or_default(),
    };
    let mut command = Command::new(OXIDE);
    command
        .current_dir(cwd)
        .env_clear()
        .env("PATH", path)
        .env("HOME", &home)
        .env("OXIDE_TEST_HOME_FROM_ENV", "1")
        .env("OXIDE_CONFIG", scratch.join("config.toml"))
        .env("OXIDE_POOL", &pool)
        .env("CARGO_HOME", cargo_home)
        .env("RUSTUP_HOME", installed_rustup_home())
        .env("RUSTUP_AUTO_INSTALL", "0")
        .env("TMPDIR", scratch)
        .env("TMP", scratch)
        .env("TEMP", scratch);
    if let Some(lang) = std::env::var_os("LANG") {
        command.env("LANG", lang);
    }
    command
}

fn create_adopted_workspace(
    scratch: &Path,
    name: &str,
    channel: &str,
    manifest_deps: &str,
    lock_body: &str,
    cargo_home: &Path,
) -> PathBuf {
    let app = scratch.join(name);
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        app.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{manifest_deps}"
        ),
    )
    .unwrap();
    fs::write(app.join("src/lib.rs"), "pub fn marker() {}\n").unwrap();
    fs::write(
        app.join("Cargo.lock"),
        format!("version = 4\n\n{lock_body}"),
    )
    .unwrap();
    fs::write(
        app.join("rust-toolchain.toml"),
        format!("[toolchain]\nchannel = \"{channel}\"\n"),
    )
    .unwrap();
    let init = oxide_command(scratch, None, cargo_home, &app)
        .arg("init")
        .output()
        .expect("run oxide init");
    assert!(
        init.status.success(),
        "oxide init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    app
}

fn verify_runs_and_skips_for_channel(channel: &str) {
    let scratch = Scratch::new();
    let cargo_home = scratch.path().join("cargo-home");
    fs::create_dir_all(&cargo_home).unwrap();
    let app = create_adopted_workspace(
        scratch.path(),
        "app",
        channel,
        "",
        "[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n",
        &cargo_home,
    );

    // First run initializes Cargo's cache db with `last_auto_gc = now`, so
    // Cargo skips on the same-second frequency gate (exit 75).
    let first = oxide_command(scratch.path(), None, &cargo_home, &app)
        .args(["clean", "--cargo-cache"])
        .output()
        .expect("first clean --cargo-cache");
    let first_stderr = String::from_utf8_lossy(&first.stderr);
    assert_eq!(first.status.code(), Some(75), "{channel}: {first_stderr}");
    assert!(
        first_stderr.contains("Cargo skipped cleanup: its frequency gate"),
        "{channel}: {first_stderr}"
    );

    // Advance past Cargo's one-second `last_auto_gc` timestamp granularity.
    std::thread::sleep(Duration::from_millis(1200));

    let second = oxide_command(scratch.path(), None, &cargo_home, &app)
        .args(["clean", "--cargo-cache"])
        .output()
        .expect("second clean --cargo-cache");
    let second_stderr = String::from_utf8_lossy(&second.stderr);
    assert!(second.status.success(), "{channel}: {second_stderr}");
    assert!(
        second_stderr.contains("oxide: Cargo cleanup ran"),
        "{channel}: {second_stderr}"
    );
    assert!(
        second_stderr.contains("closed 0 connection(s) and forwarded nothing"),
        "{channel}: {second_stderr}"
    );

    // Holding Cargo's mutate lock forces the lock-skip trace and exit 75.
    let mutate_lock = cargo_home.join(".package-cache-mutate");
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&mutate_lock)
        .expect("open .package-cache-mutate");
    // SAFETY: `lock_file` is an open descriptor owned by this test.
    let locked = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(locked, 0, "acquire exclusive mutate lock");
    let third = oxide_command(scratch.path(), None, &cargo_home, &app)
        .args(["clean", "--cargo-cache"])
        .output()
        .expect("locked clean --cargo-cache");
    drop(lock_file);
    let third_stderr = String::from_utf8_lossy(&third.stderr);
    assert_eq!(third.status.code(), Some(75), "{channel}: {third_stderr}");
    assert!(
        third_stderr.contains("Cargo skipped cleanup: another Cargo holds the package cache lock"),
        "{channel}: {third_stderr}"
    );
}

#[test]
fn cargo_cache_clean_runs_and_skips_under_cargo_1_100_0() {
    verify_runs_and_skips_for_channel("1.100.0");
}

#[test]
fn cargo_cache_clean_preserves_cargo_1_97_1() {
    if !toolchain_installed_matching("1.97.1", "cargo 1.97.1 (c980f4866 ") {
        eprintln!("skipped: Cargo 1.97.1 is not installed");
        return;
    }
    verify_runs_and_skips_for_channel("1.97.1");
}

#[test]
fn cargo_cache_clean_contains_uncached_fetch_with_closed_proxy_on_1_100_0() {
    let scratch = Scratch::new();
    let cargo_home = scratch.path().join("cargo-home");
    fs::create_dir_all(&cargo_home).unwrap();
    let lock_body = "\
[[package]]
name = \"app\"
version = \"0.1.0\"
dependencies = [
 \"cfg-if\",
]

[[package]]
name = \"cfg-if\"
version = \"1.0.0\"
source = \"registry+https://github.com/rust-lang/crates.io-index\"
checksum = \"baf1de4339761588bc0619e3cbc0120ee582ebb74b53b4efbf79117bd2da40fd\"
";
    let app = create_adopted_workspace(
        scratch.path(),
        "app",
        "1.100.0",
        "cfg-if = \"1.0.0\"\n",
        lock_body,
        &cargo_home,
    );

    // Without the debug hook, offline resolution refuses before Cargo touches the proxy.
    let refused = oxide_command(scratch.path(), None, &cargo_home, &app)
        .args(["clean", "--cargo-cache"])
        .output()
        .expect("offline refusal clean --cargo-cache");
    let refused_stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{refused_stderr}");
    assert!(
        refused_stderr.contains("cannot resolve this workspace offline from Cargo.lock"),
        "{refused_stderr}"
    );

    // In debug builds, skipping offline resolution lets `cargo fetch` reach the
    // closed loopback proxy, which intercepts and refuses every HTTP attempt.
    if cfg!(debug_assertions) {
        let proxied = oxide_command(scratch.path(), None, &cargo_home, &app)
            .env("OXIDE_TEST_SKIP_OFFLINE_RESOLUTION", "1")
            .args(["clean", "--cargo-cache"])
            .output()
            .expect("closed proxy clean --cargo-cache");
        let proxied_stderr = String::from_utf8_lossy(&proxied.stderr);
        assert!(!proxied.status.success(), "{proxied_stderr}");
        assert!(
            proxied_stderr.contains("Cargo cleanup failed: cargo fetch exited"),
            "{proxied_stderr}"
        );
        assert!(
            !proxied_stderr.contains("closed 0 connection(s)")
                && proxied_stderr.contains("connection(s) and forwarded nothing"),
            "{proxied_stderr}"
        );
        assert!(
            !cargo_home.join("registry/cache").exists(),
            "closed proxy must prevent any crate download"
        );
    }
}

#[test]
fn cargo_cache_clean_refuses_unqualified_cargo_version() {
    let scratch = Scratch::new();
    let cargo_home = scratch.path().join("cargo-home");
    fs::create_dir_all(&cargo_home).unwrap();
    let app = scratch.path().join("app");
    fs::create_dir_all(app.join("src")).unwrap();
    let manifest = app.join("Cargo.toml");
    fs::write(
        &manifest,
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(app.join("src/lib.rs"), "").unwrap();

    let bin = scratch.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let fake_cargo = bin.join("cargo");
    fs::write(
        &fake_cargo,
        "#!/bin/sh\nif [ \"$1\" = \"-V\" ]; then echo 'cargo 1.98.1 (797e8a9bc 2026-08-10)'; exit 0; fi\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&fake_cargo, fs::Permissions::from_mode(0o755)).unwrap();

    let output = oxide_command(scratch.path(), Some(&bin), &cargo_home, &app)
        .args(["clean", "--cargo-cache"])
        .output()
        .expect("unqualified cargo clean --cargo-cache");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("is not a qualified Cargo release (1.97.1 or 1.100.0)"),
        "{stderr}"
    );
}
