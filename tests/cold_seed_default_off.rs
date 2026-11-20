//! Explicit cold-seed opt-out: `OXIDE_COLD_SEED=0` on a fresh private pool.
//!
//! Self-contained CLI regression. The fixture checkout, pool, HOME and Cargo
//! home are created here and removed on drop, so the opt-out claim cannot pass
//! dormant behind an external baseline binary or a selector that no longer
//! runs. The copy of this fixture that proves default-on qualification lives
//! in the seeding contract; this one proves only the opt-out.

use std::ffi::{CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const OXIDE: &str = env!("CARGO_BIN_EXE_oxide");

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

/// The invoking rustup home, so a private fixture HOME still resolves the
/// toolchain this test was compiled with instead of installing another one.
fn invoking_rustup_home() -> Option<PathBuf> {
    std::env::var_os("RUSTUP_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".rustup")))
        .filter(|path| path.is_dir())
}

/// The fixture's private identities. The pool lives under the scratch root
/// this run declares, which is what admits a pool on a non-boot volume.
struct Fixture {
    root: PathBuf,
    home: PathBuf,
    cargo_home: PathBuf,
    pool: PathBuf,
    checkout: PathBuf,
}

impl Fixture {
    fn new(scratch: &Scratch) -> Self {
        let fixture = Self {
            root: scratch.0.clone(),
            home: scratch.0.join("home"),
            cargo_home: scratch.0.join("cargo-home"),
            pool: scratch.0.join("pool"),
            checkout: scratch.0.join("checkout"),
        };
        std::fs::create_dir(&fixture.home).unwrap();
        std::fs::create_dir(&fixture.cargo_home).unwrap();
        std::fs::create_dir_all(fixture.checkout.join("src")).unwrap();
        std::fs::write(
            fixture.checkout.join("Cargo.toml"),
            "[package]\nname = \"seed-off-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n",
        )
        .unwrap();
        std::fs::write(
            fixture.checkout.join("src/main.rs"),
            "fn main() { println!(\"seed-off\"); }\n",
        )
        .unwrap();
        fixture
    }

    /// One `oxide` invocation against the fixture's private pool and identities.
    fn run(&self, cold_seed: Option<&str>, args: &[&str]) -> Output {
        let mut command = Command::new(OXIDE);
        command
            .current_dir(&self.checkout)
            .env("HOME", &self.home)
            .env("CARGO_HOME", &self.cargo_home)
            // Without this proof control Oxide resolves the account's real
            // home, and this fixture would reach the live state directory.
            .env("OXIDE_TEST_HOME_FROM_ENV", "1")
            .env("OXIDE_CONFIG", self.root.join("missing-config"))
            .env("OXIDE_POOL", &self.pool)
            .env("OXIDE_POOL_CREATE", "1")
            .env("OXIDE_POOL_CAP", "1GiB")
            .env("OXIDE_SCRATCH_ROOT", &self.root)
            .env("RUSTUP_AUTO_INSTALL", "0")
            .env("TMPDIR", &self.root)
            .env("TMP", &self.root)
            .env("TEMP", &self.root)
            .env_remove("OXIDE_COLD_SEED")
            .env_remove("OXIDE_TASK_SCRATCH_ROOT")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR")
            .args(args);
        if let Some(rustup_home) = invoking_rustup_home() {
            command.env("RUSTUP_HOME", rustup_home);
        }
        if let Some(value) = cold_seed {
            command.env("OXIDE_COLD_SEED", value);
        }
        command.output().expect("run oxide")
    }
}

/// The one slot `oxide` leased in the fixture pool.
fn leased_slot(pool: &Path, details: &str) -> PathBuf {
    let slots = pool.join("payload/slots");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&slots)
        .unwrap_or_else(|error| panic!("slot list {}: {error}\n{details}", slots.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| !name.starts_with('.'))
        })
        .collect();
    dirs.sort();
    assert_eq!(dirs.len(), 1, "expected one leased slot\n{details}");
    dirs.remove(0)
}

#[test]
fn cold_seed_opt_out_builds_a_fresh_pool_without_seeding() {
    let scratch = Scratch::new();
    let fixture = Fixture::new(&scratch);
    let output = fixture.run(Some("0"), &["build", "--offline"]);
    let details = evidence("oxide build --offline with OXIDE_COLD_SEED=0", &output);
    assert!(output.status.success(), "{details}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    // `seeding` covers both notices the seeding gate can print: the summary of
    // a completed seed and every skip reason.
    assert!(
        !stderr.contains("seeding"),
        "the explicit opt-out must suppress every seeding notice\n{details}"
    );
    // The pool exists because this run named OXIDE_POOL_CREATE=1, and the
    // build filled the one slot it leased; no part of that came from a seed.
    let managed = fixture.pool.join("oxide-managed");
    assert!(managed.is_dir(), "no pool control directory\n{details}");
    assert!(
        !managed.join("seeds").exists(),
        "a seed was planned\n{details}"
    );
    let artifacts: Vec<String> = std::fs::read_dir(&managed)
        .unwrap_or_else(|error| panic!("control list: {error}\n{details}"))
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("seed"))
        .collect();
    assert!(
        artifacts.is_empty(),
        "seeding artifacts: {artifacts:?}\n{details}"
    );
    let slot = leased_slot(&fixture.pool, &details);
    assert!(
        slot.join("build").is_dir(),
        "the slot has no build\n{details}"
    );
    // Oxide links the project's owned outputs back into its own target
    // directory, exactly as plain Cargo would place them.
    assert!(
        fixture
            .checkout
            .join("target/debug/seed-off-fixture")
            .is_file(),
        "the opted-out build produced no binary\n{details}"
    );
    assert!(!fixture.home.join(".rustup").exists(), "{details}");
}
