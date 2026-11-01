//! Explicit seeding opt-out preserves merge-base identity after default-on qualification.

use std::ffi::{CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::{Command, Output};

fn evidence(label: &str, output: &Output) -> String {
    let head = |bytes: &[u8]| String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).into_owned();
    format!(
        "{label}: status={}\nstdout (first 4096 bytes):\n{}\nstderr (first 4096 bytes):\n{}",
        output.status,
        head(&output.stdout),
        head(&output.stderr)
    )
}

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
fn status_qualified_modes_match_baseline_without_side_effects() {
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
    let default = run(candidate, None);
    let off = run(candidate, Some("0"));
    let on = run(candidate, Some("1"));
    let base = run(&baseline_binary(), None);
    let details = format!(
        "{}\n{}\n{}\n{}",
        evidence("candidate unset", &default),
        evidence("candidate 0", &off),
        evidence("candidate 1", &on),
        evidence("baseline unset", &base)
    );
    assert!(
        default.status.success()
            && off.status.success()
            && on.status.success()
            && base.status.success(),
        "{details}"
    );
    // No recorded seed exists in this read-only fixture. Qualified status has
    // no hold notice, and opt-out suppresses recorded seed information.
    let old_notice = "seeding skipped: seeding not yet qualified on this Cargo; enabled after the qualification proof";
    for output in [&default, &off, &on, &base] {
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains(old_notice),
            "{details}"
        );
    }
    assert!(
        !String::from_utf8_lossy(&off.stdout).contains("recorded seed:"),
        "{details}"
    );
    assert_eq!(default.stdout, off.stdout, "{details}");
    assert_eq!(default.stderr, off.stderr, "{details}");
    assert_eq!(default.stdout, on.stdout, "{details}");
    assert_eq!(default.stderr, on.stderr, "{details}");
    assert_eq!(base.status.code(), off.status.code(), "{details}");
    assert_eq!(base.stdout, off.stdout, "{details}");
    assert_eq!(base.stderr, off.stderr, "{details}");
    assert!(!pool.exists(), "{details}");
    assert!(
        std::fs::read_dir(&home)
            .unwrap_or_else(|error| panic!("home read: {error}\n{details}"))
            .next()
            .is_none(),
        "{details}"
    );
    assert!(
        std::fs::read_dir(&cargo_home)
            .unwrap_or_else(|error| panic!("cargo home read: {error}\n{details}"))
            .next()
            .is_none(),
        "{details}"
    );
    assert!(!home.join(".rustup").exists(), "{details}");
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
    let run = |binary: &OsStr, prior: Option<&Output>| {
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
            .env("OXIDE_COLD_SEED", "0")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR")
            .output()
            .expect("run tiny offline build");
        let details = match prior {
            Some(previous) => format!(
                "{}\n{}",
                evidence("baseline", previous),
                evidence("candidate", &output)
            ),
            None => evidence("baseline", &output),
        };
        assert!(output.status.success(), "{details}");
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("seeding"),
            "{details}"
        );
        let bytes = std::fs::read(checkout.join("target/debug/seed-off-fixture"))
            .unwrap_or_else(|error| panic!("fixture binary read failed: {error}\n{details}"));
        assert!(!pool.join("oxide-managed/seeds").exists(), "{details}");
        assert!(!home.join(".rustup").exists(), "{details}");
        (output, bytes)
    };
    let old = run(&base, None);
    std::fs::remove_dir_all(&pool)
        .unwrap_or_else(|error| panic!("pool cleanup: {error}\n{}", evidence("baseline", &old.0)));
    std::fs::remove_dir_all(checkout.join("target")).unwrap_or_else(|error| {
        panic!("target cleanup: {error}\n{}", evidence("baseline", &old.0))
    });
    let new = run(OsStr::new(env!("CARGO_BIN_EXE_oxide")), Some(&old.0));
    let details = format!(
        "{}\n{}",
        evidence("baseline", &old.0),
        evidence("candidate", &new.0)
    );
    assert_eq!(old.0.stdout, new.0.stdout, "{details}");
    assert_eq!(old.1, new.1, "{details}");
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

#[test]
fn recorded_absent_scratch_status_is_read_only() {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let scratch = Scratch::new();
    let home = scratch.0.join("home");
    let cargo_home = scratch.0.join("cargo-home");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&cargo_home).unwrap();
    let mount = PathBuf::from("/Volumes").join(scratch.0.file_name().unwrap());
    assert!(!mount.exists());
    let root = mount.join("root");
    let pool = root.join("pool");
    let state = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/oxide/state")
    } else {
        home.join(".local/state/oxide")
    };
    let records = state.join("scratch-pools");
    std::fs::create_dir_all(&records).unwrap();
    let hash = Sha256::digest(pool.as_os_str().as_bytes());
    let name = hash.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let record_path = records.join(format!("{name}.json"));
    let walk_log = scratch.0.join("walk-log");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_oxide"))
            .arg("status")
            .current_dir(&scratch.0)
            .env("HOME", &home)
            .env("CARGO_HOME", &cargo_home)
            .env("OXIDE_TEST_HOME_FROM_ENV", "1")
            .env("OXIDE_POOL", &pool)
            .env("OXIDE_CONFIG", scratch.0.join("missing-config"))
            .env("OXIDE_TEST_WALK_LOG", &walk_log)
            .env_remove("OXIDE_SCRATCH_ROOT")
            .output()
            .unwrap()
    };
    let none = run();
    assert!(none.status.success());
    assert!(!String::from_utf8_lossy(&none.stdout).contains("scratch pool"));
    assert_eq!(std::fs::read_dir(&records).unwrap().count(), 0);
    let bytes = serde_json::to_vec(&serde_json::json!({"schema":"oxide.scratch-pool-record/1", "pool":pool, "volume_uuid":"test-uuid", "mount_point":mount, "scratch_root":root, "created_ms":1})).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&record_path)
        .unwrap();
    file.write_all(&bytes).unwrap();
    drop(file);
    let recorded = run();
    assert!(recorded.status.success());
    assert!(String::from_utf8_lossy(&recorded.stdout)
        .contains("unavailable (scratch volume test-uuid absent)"));
    assert_eq!(std::fs::read(&record_path).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(&records).unwrap().count(), 1);
    assert!(!walk_log.exists());
    assert!(!mount.exists());
}
