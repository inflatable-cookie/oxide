//! The macOS scratch-declaration proofs, isolated in their own processes.

use super::*;
use std::ffi::{CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::process::Command;

fn fixture() -> PathBuf {
    let named =
        std::env::var_os("OXIDE_TEST_INTERNAL_ROOT").expect("OXIDE_TEST_INTERNAL_ROOT is required");
    let root = fs::canonicalize(&named).expect("internal root");
    assert_eq!(Path::new(&named), root);
    let supervised =
        fs::canonicalize(std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").expect("supervised root"))
            .expect("supervised root");
    assert!(
        root.starts_with(supervised),
        "internal root must be supervised"
    );
    let metadata = fs::metadata(&root).unwrap();
    assert!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::getuid() }
            && metadata.mode() & 0o077 == 0
    );
    let mut name = CString::new(root.join("scratch-unit.XXXXXX").as_os_str().as_bytes())
        .unwrap()
        .into_bytes_with_nul();
    assert!(!unsafe { libc::mkdtemp(name.as_mut_ptr().cast()) }.is_null());
    PathBuf::from(OsStr::from_bytes(
        CStr::from_bytes_until_nul(&name).unwrap().to_bytes(),
    ))
}

fn isolated(test: &str, root: &Path) {
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("OXIDE_S3_UNIT_CHILD", "1")
        .env(
            "OXIDE_TEST_INTERNAL_ROOT",
            std::env::var_os("OXIDE_TEST_INTERNAL_ROOT").unwrap(),
        )
        .env("HOME", root.join("home"))
        .env("OXIDE_TEST_HOME_FROM_ENV", "1")
        .env("OXIDE_CONFIG", root.join("config.toml"))
        .env("OXIDE_POOL", root.join("pool"))
        .env("OXIDE_POOL_CAP", "64MiB")
        .env("OXIDE_POOL_CREATE", "1")
        .env("OXIDE_SCRATCH_ROOT", root)
        .env("OXIDE_S3_ROOT", root)
        .env("TMPDIR", root)
        .env("TMP", root)
        .env("TEMP", root)
        .status()
        .expect("unit child");
    assert!(status.success(), "isolated unit child failed");
}

#[test]
fn scratch_declaration_is_inert_on_boot_volume() {
    if std::env::var_os("OXIDE_S3_UNIT_CHILD").is_none() {
        let root = fixture();
        isolated(
            "store::scratch_internal_tests::scratch_declaration_is_inert_on_boot_volume",
            &root,
        );
        fs::remove_dir_all(root).unwrap();
        return;
    }
    let _internal = crate::config::internal_fixture_scope();
    let root = PathBuf::from(std::env::var_os("OXIDE_S3_ROOT").unwrap());
    fs::create_dir(root.join("home")).unwrap();
    let declared = root.join("pool");
    let control = root.join("control-pool");
    assert!(!declared.exists() && !control.exists());
    let first = Store::open_or_create().expect("declared internal pool");
    assert_eq!(first.pool, declared);
    assert!(!declared.join("oxide-managed/scratch.json").exists());
    assert!(!root
        .join("home/Library/Application Support/oxide/state/scratch-pools")
        .exists());
    // This child process is isolated from the unit runner's environment.
    unsafe {
        std::env::remove_var("OXIDE_SCRATCH_ROOT");
        std::env::set_var("OXIDE_POOL", &control);
    }
    let second = Store::open_or_create().expect("ordinary internal pool");
    assert_eq!(second.pool, control);
    assert!(!control.join("oxide-managed/scratch.json").exists());
    assert!(!root
        .join("home/Library/Application Support/oxide/state/scratch-pools")
        .exists());
    let mut first_config: serde_json::Value =
        serde_json::from_slice(&fs::read(declared.join("config.json")).unwrap()).unwrap();
    let mut second_config: serde_json::Value =
        serde_json::from_slice(&fs::read(control.join("config.json")).unwrap()).unwrap();
    let left = first_config.as_object_mut().unwrap();
    let right = second_config.as_object_mut().unwrap();
    let expected: std::collections::BTreeSet<&str> =
        ["version", "backend", "store_id", "cap_bytes", "image_bytes"]
            .into_iter()
            .collect();
    assert_eq!(
        left.keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    assert_eq!(
        right
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    // Only store_id is generated per initialization; a new field must be assessed above.
    let left_id = left
        .remove("store_id")
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    let right_id = right
        .remove("store_id")
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    for id in [&left_id, &right_id] {
        assert!(oxide_storage::seed_source::SourceCache::new(id).is_ok());
        assert!(
            id.len() == 32
                && id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }
    assert_ne!(left_id, right_id);
    assert_eq!(left, right);
    fn entries(root: &Path, path: &Path, id: &str, result: &mut Vec<String>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let relative = entry
                .path()
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace(id, "<store-id>");
            result.push(relative);
            if entry.file_type().unwrap().is_dir() {
                entries(root, &entry.path(), id, result);
            }
        }
    }
    let mut left_entries = Vec::new();
    let mut right_entries = Vec::new();
    entries(&declared, &declared, &left_id, &mut left_entries);
    entries(&control, &control, &right_id, &mut right_entries);
    left_entries.sort();
    right_entries.sort();
    assert_eq!(left_entries, right_entries);
}

#[test]
fn internal_git_refusal_is_unchanged_by_scratch_declaration() {
    if std::env::var_os("OXIDE_S3_UNIT_CHILD").is_none() {
        let root = fixture();
        isolated("store::scratch_internal_tests::internal_git_refusal_is_unchanged_by_scratch_declaration", &root);
        fs::remove_dir_all(root).unwrap();
        return;
    }
    let _internal = crate::config::internal_fixture_scope();
    let root = PathBuf::from(std::env::var_os("OXIDE_S3_ROOT").unwrap());
    fs::create_dir(root.join(".git")).unwrap();
    let settings = super::settings().unwrap();
    let with = crate::scratch_io::evaluate(&settings, false).unwrap_err();
    unsafe { std::env::remove_var("OXIDE_SCRATCH_ROOT") };
    let without = crate::scratch_io::evaluate(&settings, false).unwrap_err();
    assert_eq!(with, without);
    assert!(with.contains("Git work tree"));
    assert!(!settings.pool.exists());
}
