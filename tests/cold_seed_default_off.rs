//! The qualification hold is invisible when the opt-in is unset.

use std::ffi::{CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let template = std::env::temp_dir().join("oxide-seed-off.XXXXXX");
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

#[test]
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
    // A separately staged merge-base binary turns this into a direct
    // base-versus-branch comparison; ordinary suite runs need no second build.
    if let Some(base) = std::env::var_os("OXIDE_SEED_BASELINE_BINARY") {
        let base = run(&base, None);
        assert_eq!(base.status.code(), off.status.code());
        assert_eq!(base.stdout, off.stdout);
        assert_eq!(base.stderr, off.stderr);
    }
    assert!(!pool.exists());
    assert!(std::fs::read_dir(&home).unwrap().next().is_none());
    assert!(std::fs::read_dir(&cargo_home).unwrap().next().is_none());
    assert!(!home.join(".rustup").exists());
}
