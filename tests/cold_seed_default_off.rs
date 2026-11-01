//! The qualification hold is invisible when the opt-in is unset.

use std::ffi::{CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        // Under the Rust recipe the child's TMPDIR is the selected pool's
        // slot scratch, so an internal pool fixture must use the internal root.
        let base = std::env::var_os("OXIDE_TEST_INTERNAL_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let template = base.join("seed.XXXXXX");
        let mut name = CString::new(template.as_os_str().as_bytes())
            .unwrap()
            .into_bytes_with_nul();
        // SAFETY: name is writable, NUL-terminated and ends in XXXXXX.
        assert!(!unsafe { libc::mkdtemp(name.as_mut_ptr().cast()) }.is_null());
        let name = CStr::from_bytes_until_nul(&name).unwrap();
        Self(PathBuf::from(OsStr::from_bytes(name.to_bytes())))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

/// The oxide binary staged from the merge-base source. `effigy rust:seed-default-off`
/// exports `OXIDE_SEED_BASELINE_BINARY`; a missing or non-file value fails the
/// test loudly so the default-off identity claim cannot pass dormant.
fn baseline_binary() -> std::ffi::OsString {
    let base = std::env::var_os("OXIDE_SEED_BASELINE_BINARY")
        .expect("OXIDE_SEED_BASELINE_BINARY must name the staged merge-base oxide binary");
    assert!(
        std::path::Path::new(&base).is_file(),
        "staged merge-base oxide binary is absent: {}",
        std::path::Path::new(&base).display()
    );
    base
}

#[test]
#[ignore = "needs OXIDE_SEED_BASELINE_BINARY; run via effigy rust:seed-default-off"]
fn status_off_matches_ignored_opt_in_and_held_adds_only_its_notice() {
    let real_home = std::env::var_os("HOME").expect("invoking HOME");
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(real_home).join(".rustup"));
    let scratch = Scratch::new();
    let home = scratch.0.join("home");
    let cargo_home = scratch.0.join("cargo-home");
    let pool = scratch.0.join("pool");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&cargo_home).unwrap();
    let run = |binary: &OsStr, opt: Option<&str>| {
        let mut command = Command::new(binary);
        command
            .arg("status")
            .env("HOME", &home)
            .env("CARGO_HOME", &cargo_home)
            .env("RUSTUP_HOME", &rustup_home)
            .env("RUSTUP_AUTO_INSTALL", "0")
            .env("RUSTUP_TOOLCHAIN", "1.97.1")
            .env("OXIDE_POOL", &pool)
            .env("OXIDE_CONFIG", scratch.0.join("missing-config"))
            .env_remove("OXIDE_COLD_SEED");
        if let Some(value) = opt {
            command.env("OXIDE_COLD_SEED", value);
        }
        command.output().expect("oxide status")
    };
    let candidate = OsStr::new(env!("CARGO_BIN_EXE_oxide"));
    let off = run(candidate, None);
    let ignored = run(candidate, Some("0"));
    let held = run(candidate, Some("1"));
    assert!(off.status.success() && ignored.status.success() && held.status.success());
    assert_eq!(off.stdout, ignored.stdout);
    assert_eq!(off.stderr, ignored.stderr);
    assert_eq!(off.stderr, held.stderr);
    let notice = b"seeding skipped: seeding not yet qualified on this Cargo; enabled after the qualification proof\n";
    assert_eq!(
        held.stdout.strip_prefix(notice),
        Some(off.stdout.as_slice())
    );
    // The merge-base binary is the required comparison: an absent input fails
    // here instead of letting the identity claim pass dormant.
    let base = run(&baseline_binary(), None);
    assert_eq!(base.status.code(), off.status.code());
    assert_eq!(base.stdout, off.stdout);
    assert_eq!(base.stderr, off.stderr);
    assert!(!pool.exists());
    assert!(std::fs::read_dir(&home).unwrap().next().is_none());
    assert!(std::fs::read_dir(&cargo_home).unwrap().next().is_none());
    assert!(!home.join(".rustup").exists());
}

#[test]
#[ignore = "needs OXIDE_SEED_BASELINE_BINARY; run via effigy rust:seed-default-off"]
fn tiny_build_matches_staged_base_when_seed_is_off() {
    let base = baseline_binary();
    let real_home = std::env::var_os("HOME").expect("invoking HOME");
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(real_home).join(".rustup"));
    let scratch = Scratch::new();
    let home = scratch.0.join("home");
    let cargo_home = scratch.0.join("cargo-home");
    let pool = scratch.0.join("pool");
    let checkout = scratch.0.join("checkout");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&cargo_home).unwrap();
    std::fs::create_dir_all(checkout.join("src")).unwrap();
    std::fs::write(
        checkout.join("Cargo.toml"),
        "[package]\nname = 'seed-off-fixture'\nversion = '0.1.0'\nedition = '2021'\n",
    )
    .unwrap();
    std::fs::write(
        checkout.join("src/main.rs"),
        "fn main() { println!(\"seed-off\"); }\n",
    )
    .unwrap();
    let run = |binary: &OsStr| {
        let output = Command::new(binary)
            .args(["build", "--offline"])
            .current_dir(&checkout)
            .env("HOME", &home)
            .env("CARGO_HOME", &cargo_home)
            .env("RUSTUP_HOME", &rustup_home)
            .env("RUSTUP_AUTO_INSTALL", "0")
            .env("RUSTUP_TOOLCHAIN", "1.97.1")
            .env("OXIDE_POOL", &pool)
            .env("OXIDE_CONFIG", scratch.0.join("missing-config"))
            .env_remove("OXIDE_COLD_SEED")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR")
            .output()
            .expect("run tiny offline build");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("seeding"));
        let bytes = std::fs::read(checkout.join("target/debug/seed-off-fixture")).unwrap();
        assert!(!pool.join("oxide-managed/seeds").exists());
        assert!(!home.join(".rustup").exists());
        (output.stdout, bytes)
    };
    let old = run(&base);
    std::fs::remove_dir_all(&pool).unwrap();
    std::fs::remove_dir_all(checkout.join("target")).unwrap();
    let new = run(OsStr::new(env!("CARGO_BIN_EXE_oxide")));
    assert_eq!(old.0, new.0);
    assert_eq!(old.1, new.1);
}

#[test]
fn scratch_declaration_is_inert_for_internal_status() {
    let scratch = Scratch::new();
    let home = scratch.0.join("home");
    let cargo_home = scratch.0.join("cargo-home");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&cargo_home).unwrap();
    let run = |declared: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oxide"));
        cmd.arg("status")
            .env("HOME", &home)
            .env("CARGO_HOME", &cargo_home)
            .env("OXIDE_TEST_HOME_FROM_ENV", "1")
            .env("OXIDE_POOL", scratch.0.join("pool"))
            .env("OXIDE_CONFIG", scratch.0.join("missing-config"))
            .env_remove("OXIDE_SCRATCH_ROOT");
        if declared {
            cmd.env("OXIDE_SCRATCH_ROOT", "");
        }
        cmd.output().expect("oxide status")
    };
    let ordinary = run(false);
    let declared = run(true);
    assert!(ordinary.status.success() && declared.status.success());
    assert_eq!(ordinary.stdout, declared.stdout);
    assert_eq!(ordinary.stderr, declared.stderr);
    assert!(!scratch.0.join("pool").exists());
}
