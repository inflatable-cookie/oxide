//! oxide#182: the four CLI papercuts at the process boundary.
//!
//! These are CLI regressions, not native proofs: each case runs the built
//! `oxide` binary with a private home, config and pool and a recording fake
//! `cargo`, so no live pool, real Cargo build, account root or install is
//! touched. The help and refusal cases also prove nothing was created; the
//! creation case creates a temporary `directory-budget` pool inside its own
//! scratch root and removes it.

use std::ffi::{CStr, CString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const OXIDE: &str = env!("CARGO_BIN_EXE_oxide");

/// A fresh owner-only directory under the internal test root, removed on
/// drop. Only this test's own files are written inside it.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let base = std::env::var_os("OXIDE_TEST_INTERNAL_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let template = base.join("cli-papercuts.XXXXXX");
        let mut name = CString::new(template.as_os_str().as_bytes())
            .unwrap()
            .into_bytes_with_nul();
        // SAFETY: name is writable, NUL-terminated and ends in XXXXXX.
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

/// One recording fake `cargo`. It logs its arguments and answers `oxide`'s
/// workspace lookup so a leased command reaches its primary child; it never
/// compiles anything.
fn fake_cargo(scratch: &Path, manifest: &Path) -> (PathBuf, PathBuf) {
    let bin = scratch.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let log = scratch.join("cargo.log");
    let cargo = bin.join("cargo");
    fs::write(
        &cargo,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\ncase \"$1\" in\n  locate-project) echo \"{}\"; exit 0 ;;\nesac\nexit 0\n",
            log.display(),
            manifest.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    (bin, log)
}

/// A private environment: a fresh home, an absent config file and an absent
/// pool outside any task scratch root, plus the fake Cargo first on `PATH`.
fn private_env(scratch: &Path, bin: &Path, pool: &Path) -> Command {
    let home = scratch.join("home");
    fs::create_dir_all(&home).unwrap();
    let mut command = Command::new(OXIDE);
    command
        .env_clear()
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("HOME", &home)
        .env("OXIDE_CONFIG", scratch.join("config.toml"))
        .env("OXIDE_POOL", pool)
        .env("OXIDE_POOL_BACKEND", "directory-budget")
        .env("OXIDE_POOL_CAP", "2MiB")
        .env("CARGO_HOME", scratch.join("cargo-home"))
        .env("TMPDIR", scratch);
    for name in [
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
        "CARGO_NET_OFFLINE",
        "LANG",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
}

/// `oxide <cmd> --help` / `-h` runs `cargo <cmd> --help`: no pool is opened,
/// created, leased or waited for. Counterfactual: before oxide#182 the help
/// request went through the lease, so this pool would have been refused or
/// created and the fake Cargo would have seen the whole argument list.
#[test]
fn help_is_cargos_help_without_a_slot_or_pool() {
    let scratch = Scratch::new();
    let manifest = scratch.path().join("app/Cargo.toml");
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    fs::write(
        &manifest,
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let (bin, log) = fake_cargo(scratch.path(), &manifest);
    let pool = scratch.path().join("outside-pool");
    for spelling in ["--help", "-h"] {
        let _ = fs::remove_file(&log);
        let output = private_env(scratch.path(), &bin, &pool)
            .current_dir(&manifest.parent().unwrap())
            .args(["build", spelling])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "oxide build {spelling}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            "build --help\n",
            "oxide build {spelling} did not forward exactly Cargo's help request"
        );
        assert!(
            !pool.exists(),
            "help created the pool at {}",
            pool.display()
        );
        assert!(
            !scratch.path().join("home/Library").exists(),
            "help touched the private home"
        );
    }
    // `oxide help <cmd>` is the existing plain passthrough: Cargo's help with
    // no pool either.
    let _ = fs::remove_file(&log);
    let output = private_env(scratch.path(), &bin, &pool)
        .current_dir(&manifest.parent().unwrap())
        .args(["help", "build"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "oxide help build: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&log).unwrap(), "help build\n");
    assert!(!pool.exists(), "oxide help build created the pool");

    // Help after `--` belongs to the program, not to Cargo: it must not take
    // the help shortcut, so the command keeps the leased path and refuses at
    // this absent explicit pool before any program starts.
    let _ = fs::remove_file(&log);
    let output = private_env(scratch.path(), &bin, &pool)
        .current_dir(&manifest.parent().unwrap())
        .args(["build", "--", "--help"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("OXIDE_POOL names") && stderr.contains("OXIDE_POOL_CREATE=1"),
        "help after `--` took the Cargo-help shortcut: {stderr}"
    );
    if let Ok(log) = fs::read_to_string(&log) {
        assert!(
            !log.lines().any(|line| line.starts_with("build ")),
            "the program's own help argument was forwarded to Cargo: {log}"
        );
    }
}

/// An absent pool named by `OXIDE_POOL` refuses by name instead of being
/// created silently, and no Cargo or pool work happens first.
#[test]
fn an_explicit_pool_outside_the_task_scratch_root_refuses() {
    let scratch = Scratch::new();
    let manifest = scratch.path().join("app/Cargo.toml");
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    fs::write(
        &manifest,
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let (bin, log) = fake_cargo(scratch.path(), &manifest);
    let pool = scratch.path().join("outside-pool");
    let output = private_env(scratch.path(), &bin, &pool)
        .current_dir(&manifest.parent().unwrap())
        .arg("check")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(&pool.display().to_string()),
        "the refusal did not name the path: {stderr}"
    );
    assert!(
        stderr.contains("OXIDE_POOL_CREATE=1"),
        "the refusal did not name the opt-in: {stderr}"
    );
    assert!(!pool.exists(), "the refusal created the pool");
    assert!(!log.exists(), "Cargo ran before the refusal");

    // The pasted `oxide status` annotation is not part of a path.
    let pasted = scratch.path().join("pasted-pool (from OXIDE_POOL)");
    let output = private_env(scratch.path(), &bin, &pasted)
        .current_dir(&manifest.parent().unwrap())
        .arg("check")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("pasted from `oxide status`"), "{stderr}");
    assert!(stderr.contains("OXIDE_POOL is"), "{stderr}");
    assert!(!pasted.exists() && !scratch.path().join("pasted-pool").exists());

    // A path that exists but holds no Oxide pool is also named, with the
    // open refusal's own cause, and nothing is created inside it.
    let empty = scratch.path().join("empty-pool");
    fs::create_dir_all(&empty).unwrap();
    fs::write(empty.join("unrelated.txt"), "keep").unwrap();
    let output = private_env(scratch.path(), &bin, &empty)
        .current_dir(&manifest.parent().unwrap())
        .arg("check")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!("OXIDE_POOL names {}:", empty.display())),
        "the open refusal did not name the path: {stderr}"
    );
    assert_eq!(fs::read(empty.join("unrelated.txt")).unwrap(), b"keep");
    assert!(!empty.join("oxide-managed").exists());
}

/// Creation inside the caller's own task scratch root still works: the
/// fixture's pool is created and the leased command reaches Cargo.
#[test]
fn a_pool_inside_the_task_scratch_root_is_still_created() {
    let scratch = Scratch::new();
    let manifest = scratch.path().join("app/Cargo.toml");
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    fs::write(
        &manifest,
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let (bin, log) = fake_cargo(scratch.path(), &manifest);
    let pool = scratch.path().join("fixture-pool");
    let output = private_env(scratch.path(), &bin, &pool)
        .current_dir(&manifest.parent().unwrap())
        // The same directory the fixture writes in is its task scratch root.
        .env("OXIDE_TASK_SCRATCH_ROOT", scratch.path())
        .arg("check")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        pool.join("config.json").is_file() && pool.join("oxide-managed").is_dir(),
        "the scratch-root pool was not created at {}",
        pool.display()
    );
    assert!(
        fs::read_to_string(&log)
            .unwrap()
            .lines()
            .any(|line| line == "check"),
        "the leased command never reached Cargo: {:?}",
        fs::read_to_string(&log).unwrap()
    );
}
