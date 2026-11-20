//! Scratch-pool status: a declared scratch root and a recorded-but-absent
//! scratch volume are both read-only for `oxide status`.
//!
//! Relocated out of the cold-seed file (oxide#358): these cases are about
//! scratch selection, not seeding, and each fixture is independent of it.

use std::ffi::{CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::Command;

const OXIDE: &str = env!("CARGO_BIN_EXE_oxide");

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        // Under the Rust recipe the child's TMPDIR is the selected pool's
        // slot scratch, so an internal pool fixture must use the internal root.
        let base = std::env::var_os("OXIDE_TEST_INTERNAL_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let template = base.join("scratch-status.XXXXXX");
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
        // A panic here would replace the test's own failure during unwinding.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn scratch_drop_tolerates_a_directory_already_removed() {
    let scratch = Scratch::new();
    std::fs::remove_dir_all(&scratch.0).expect("remove the fixture early");
    drop(scratch);
}

#[test]
fn scratch_declaration_is_inert_for_internal_status() {
    let scratch = Scratch::new();
    let home = scratch.0.join("home");
    let cargo_home = scratch.0.join("cargo-home");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&cargo_home).unwrap();
    let run = |declared: bool| {
        let mut cmd = Command::new(OXIDE);
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
    let run = || {
        Command::new(OXIDE)
            .arg("status")
            .current_dir(&scratch.0)
            .env("HOME", &home)
            .env("CARGO_HOME", &cargo_home)
            .env("OXIDE_TEST_HOME_FROM_ENV", "1")
            .env("OXIDE_POOL", &pool)
            .env("OXIDE_CONFIG", scratch.0.join("missing-config"))
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
    assert!(!mount.exists());
}
