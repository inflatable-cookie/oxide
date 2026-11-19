//! oxide#182: the four CLI papercuts at the process boundary.
//!
//! These are CLI regressions, not native proofs: each case runs the built
//! `oxide` binary with a private home, config and pool and a recording fake
//! `cargo`, so no live pool, real Cargo build, account root or install is
//! touched. The help and refusal cases also prove nothing was created; the
//! creation case creates a temporary `directory-budget` pool inside its own
//! scratch root and removes it.

use serde_json::{json, Value};
use sha2::Digest;
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
    private_env_for(Path::new(OXIDE), scratch, bin, pool)
}

fn private_env_for(executable: &Path, scratch: &Path, bin: &Path, pool: &Path) -> Command {
    let home = scratch.join("home");
    fs::create_dir_all(&home).unwrap();
    let mut command = Command::new(executable);
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
        .env("OXIDE_TEST_HOME_FROM_ENV", "1")
        .env("OXIDE_CONFIG", scratch.join("config.toml"))
        .env("OXIDE_POOL", pool)
        .env("OXIDE_POOL_BACKEND", "directory-budget")
        .env("OXIDE_POOL_CAP", "2MiB")
        .env("CARGO_HOME", scratch.join("cargo-home"))
        .env("OXIDE_SCRATCH_ROOT", scratch)
        .env("TMPDIR", scratch)
        .env("TMP", scratch)
        .env("TEMP", scratch);
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
        // The request is forwarded as the caller spelled it.
        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            format!("build {spelling}\n"),
            "oxide build {spelling} did not forward the help request unchanged"
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
    // Oxide's own budget options are not Cargo's, so they are dropped from a
    // forwarded help request; the rest of the request is kept.
    let _ = fs::remove_file(&log);
    let output = private_env(scratch.path(), &bin, &pool)
        .current_dir(manifest.parent().unwrap())
        .args(["build", "--admission-budget", "30s", "--help"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "oxide build --admission-budget 30s --help: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&log).unwrap(), "build --help\n");

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

    // `clean` is Oxide-owned, but its help is still Cargo's: it must not run
    // its own workspace discovery or planning. Counterfactual: without the
    // Class::Clean arm the fake Cargo would have seen `locate-project` first
    // and clean would have refused `--help`.
    let _ = fs::remove_file(&log);
    let output = private_env(scratch.path(), &bin, &pool)
        .current_dir(&manifest.parent().unwrap())
        .args(["clean", "--help"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "oxide clean --help: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&log).unwrap(), "clean --help\n");
    assert!(!pool.exists(), "oxide clean --help created the pool");

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

/// oxide#326: `oxide nextest run --help` must ask Cargo for that whole
/// request. Counterfactual: the front door kept only the command name and
/// `--help`, so `oxide nextest run --help` printed top-level help and every
/// argument before `--help` was lost.
#[test]
fn external_subcommand_help_keeps_every_argument() {
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
    for request in [
        &["nextest", "run", "--help"][..],
        &["nextest", "run", "-h"],
        &["nextest", "--cargo-profile", "debug", "--help"],
    ] {
        let _ = fs::remove_file(&log);
        let output = private_env(scratch.path(), &bin, &pool)
            .current_dir(manifest.parent().unwrap())
            .args(request)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "oxide {request:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            format!("{}\n", request.join(" ")),
            "oxide {request:?} did not forward the whole help request"
        );
        assert!(
            !pool.exists(),
            "help created the pool at {}",
            pool.display()
        );
    }
}

/// One recording fake Cargo for `oxide run`: the workspace lookup, `metadata`,
/// and one built binary — the fixture script named by `FAKE_CARGO_PROGRAM` —
/// as Cargo's JSON reports it. `FAKE_CARGO_SIGNAL_PARENT` makes it signal this
/// command before it exits, standing for a Ctrl-C that arrives while Cargo
/// runs.
fn fake_run_cargo(scratch: &Path, manifest: &Path) -> (PathBuf, PathBuf) {
    let bin = scratch.join("run-bin");
    fs::create_dir_all(&bin).unwrap();
    let log = scratch.join("run-cargo.log");
    let metadata = scratch.join("metadata.json");
    fs::write(
        &metadata,
        serde_json::to_vec(&json!({"packages": [{
            "name": "app",
            "version": "2.3.4-beta.7+build.9",
            "manifest_path": manifest.display().to_string(),
            "authors": ["A One <a@example.com>", "B Two <b@example.com>"],
            "license": "MIT",
            "readme": "README.md",
        }]}))
        .unwrap(),
    )
    .unwrap();
    let cargo = bin.join("cargo");
    fs::write(
        &cargo,
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> "{log}"
case "$1" in
  locate-project) echo "{manifest}"; exit 0 ;;
  metadata) cat "{metadata}"; exit 0 ;;
esac
if [ -n "$FAKE_CARGO_SIGNAL_PARENT" ]; then kill -INT "$PPID"; fi
printf '{{"reason":"compiler-artifact","target":{{"name":"demo","kind":["bin"]}},"executable":"%s","manifest_path":"{manifest}"}}\n' "$FAKE_CARGO_PROGRAM"
exit 0
"#,
            log = log.display(),
            manifest = manifest.display(),
            metadata = metadata.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
    (bin, log)
}

/// The `oxide run` fixture: a manifest, a fake Cargo, an existing private pool,
/// and the built program's script, which records the environment it was given in
/// `program.env` inside the scratch and writes nothing to stdout.
fn run_fixture(scratch: &Scratch) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let app = scratch.path().join("app");
    let manifest = app.join("Cargo.toml");
    fs::create_dir_all(&app).unwrap();
    fs::write(
        &manifest,
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let program = app.join("program.sh");
    fs::write(&program, "#!/bin/sh\nenv | sort > \"$FAKE_PROGRAM_OUT\"\n").unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    let (bin, log) = fake_run_cargo(scratch.path(), &manifest);
    let pool = scratch.path().join("fixture-pool");
    created_pool(scratch, &bin, &manifest, &pool);
    (manifest, program, bin, log)
}

/// The fixture's `oxide run` command, with the built program named by the fake
/// Cargo and the marker file it records its environment in.
fn run_command(
    scratch: &Scratch,
    bin: &Path,
    pool: &Path,
    manifest: &Path,
    program: &Path,
) -> Command {
    let mut command = private_env(scratch.path(), bin, pool);
    command
        .current_dir(manifest.parent().unwrap())
        .env("FAKE_CARGO_PROGRAM", program)
        .env("FAKE_PROGRAM_OUT", scratch.path().join("program.env"));
    command
}

fn environment_file(path: &Path) -> std::collections::HashMap<String, String> {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

/// oxide#326: a program under `oxide run` reads the environment `cargo run`
/// would give it. Counterfactual: only `CARGO_MANIFEST_DIR` was set, so
/// `CARGO`, `CARGO_MANIFEST_PATH`, every `CARGO_PKG_*` fact and the dylib
/// search path were missing (or inherited from the caller).
#[test]
fn run_gives_the_program_cargos_run_environment() {
    assert_run_environment(&["run", "--bin", "demo"]);
}

#[test]
fn bounded_run_gives_the_program_cargos_run_environment() {
    assert_run_environment(&["run", "--execution-budget", "30s", "--bin", "demo"]);
}

fn assert_run_environment(args: &[&str]) {
    let scratch = Scratch::new();
    let (manifest, program, bin, _log) = run_fixture(&scratch);
    let pool = scratch.path().join("fixture-pool");
    let output = run_command(&scratch, &bin, &pool, &manifest, &program)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "oxide run: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let environment = environment_file(&scratch.path().join("program.env"));
    let app = manifest.parent().unwrap();
    assert_eq!(
        environment.get("CARGO_MANIFEST_DIR").map(String::as_str),
        Some(app.to_str().unwrap())
    );
    assert_eq!(
        environment.get("CARGO_MANIFEST_PATH").map(String::as_str),
        Some(manifest.to_str().unwrap())
    );
    // The Cargo this Oxide runs, resolved on the fixture's `PATH`.
    assert_eq!(
        environment.get("CARGO").map(String::as_str),
        Some(bin.join("cargo").to_str().unwrap())
    );
    for (name, value) in [
        ("CARGO_PKG_NAME", "app"),
        ("CARGO_PKG_VERSION", "2.3.4-beta.7+build.9"),
        ("CARGO_PKG_VERSION_MAJOR", "2"),
        ("CARGO_PKG_VERSION_MINOR", "3"),
        ("CARGO_PKG_VERSION_PATCH", "4"),
        ("CARGO_PKG_VERSION_PRE", "beta.7"),
        // Cargo joins the authors with a colon.
        (
            "CARGO_PKG_AUTHORS",
            "A One <a@example.com>:B Two <b@example.com>",
        ),
        ("CARGO_PKG_LICENSE", "MIT"),
        ("CARGO_PKG_README", "README.md"),
        // A fact the package does not have is an empty string, as Cargo sets it.
        ("CARGO_PKG_DESCRIPTION", ""),
        ("CARGO_PKG_RUST_VERSION", ""),
    ] {
        assert_eq!(
            environment.get(name).map(String::as_str),
            Some(value),
            "{name}"
        );
    }
    // The loader searches beside the program, then the toolchain, then the
    // caller's path: the program's own directory and its `deps` come first.
    // A script fixture runs as `/bin/sh`, and macOS strips `DYLD_*` variables
    // from the environment of a platform binary, so macOS observes this only
    // through a compiled program (`identity`'s search-path tests pin the
    // content).
    #[cfg(not(target_os = "macos"))]
    {
        let search: Vec<&str> = environment
            .get("LD_LIBRARY_PATH")
            .map(|value| value.split(':').collect())
            .unwrap_or_default();
        assert_eq!(
            search.first().copied(),
            app.join("target/debug").to_str(),
            "LD_LIBRARY_PATH: {search:?}"
        );
        assert_eq!(
            search.get(1).copied(),
            app.join("target/debug/deps").to_str(),
            "LD_LIBRARY_PATH: {search:?}"
        );
    }
}

/// A closed stdout must not panic while the lease is held: forwarding stops
/// and the program still runs. Counterfactual: `println!` panicked on the
/// closed pipe, the command unwound and the program never started.
#[test]
fn run_keeps_draining_after_a_closed_stdout() {
    use std::os::unix::io::FromRawFd;
    let scratch = Scratch::new();
    let (manifest, program, bin, _log) = run_fixture(&scratch);
    let pool = scratch.path().join("fixture-pool");
    let mut fds = [0; 2];
    // SAFETY: `pipe` writes two owned descriptors into `fds`.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    // SAFETY: the read end is closed exactly once, so every write to the pipe
    // fails with EPIPE; the write end is given to the child.
    unsafe { libc::close(fds[0]) };
    let closed = unsafe { std::process::Stdio::from_raw_fd(fds[1]) };
    let output = run_command(&scratch, &bin, &pool, &manifest, &program)
        // The caller's own json format is what Oxide forwards and prints.
        .args(["run", "--bin", "demo", "--message-format=json"])
        .stdout(closed)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(output.status.success(), "oxide run: {stderr}");
    assert!(
        scratch.path().join("program.env").is_file(),
        "the closed stdout stopped the program from running: {stderr}"
    );
}

/// A signal that arrives while Cargo runs ends the command and never launches
/// the program. Counterfactual: the release-then-exec hand-off ran the program
/// anyway and returned its exit code.
#[test]
fn a_signal_after_cargo_never_launches_the_program() {
    let scratch = Scratch::new();
    let (manifest, program, bin, _log) = run_fixture(&scratch);
    let pool = scratch.path().join("fixture-pool");
    let output = run_command(&scratch, &bin, &pool, &manifest, &program)
        .env("FAKE_CARGO_SIGNAL_PARENT", "1")
        .args(["run", "--bin", "demo"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(130),
        "the interrupted run did not return the signal code: {stderr}"
    );
    assert!(
        !scratch.path().join("program.env").exists(),
        "the program ran after the signal: {stderr}"
    );
}

/// A release binary carries no test hooks: `OXIDE_TEST_LAUNCH_SOURCE` cannot
/// make it copy and execute a file named in the environment. This release-only
/// test runs when the suite itself is a release build; in a debug build the
/// hooks are the integration tests' own controls.
#[cfg(all(not(debug_assertions), target_os = "macos"))]
#[test]
fn a_release_binary_ignores_the_launch_test_hooks() {
    let scratch = Scratch::new();
    let app = scratch.path().join("app");
    fs::create_dir_all(&app).unwrap();
    let manifest = app.join("Cargo.toml");
    fs::write(
        &manifest,
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let (bin, _log) = fake_cargo(scratch.path(), &manifest);
    let pool = scratch.path().join("fixture-pool");
    created_pool(&scratch, &bin, &manifest, &pool);
    let missing = scratch.path().join("absent-probe");
    let output = private_env(scratch.path(), &bin, &pool)
        .current_dir(&app)
        .env("OXIDE_TEST_LAUNCH_SOURCE", &missing)
        .env("OXIDE_TEST_LAUNCH_LIMIT_MS", "900")
        .args(["status", "--launch-check"])
        .output()
        .unwrap();
    // A debug build answers `oxide: could not read <source>` here.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("absent-probe") && !stderr.contains("could not read"),
        "a release binary honoured a test hook: {stderr}"
    );
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

    // The same annotation on an EXISTING ordinary directory (Found::Unknown,
    // not Found::Absent) must still get the hint, not the generic open error
    // the path otherwise reaches.
    let annotated = scratch.path().join("existing (from OXIDE_POOL)");
    fs::create_dir_all(&annotated).unwrap();
    fs::write(annotated.join("keep.txt"), "keep").unwrap();
    let output = private_env(scratch.path(), &bin, &annotated)
        .current_dir(&manifest.parent().unwrap())
        .arg("check")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("pasted from `oxide status`"),
        "an existing non-pool path lost the annotation hint: {stderr}"
    );
    assert!(
        !stderr.contains("cannot open the pool"),
        "the generic open error ran before the annotation hint: {stderr}"
    );
    assert_eq!(fs::read(annotated.join("keep.txt")).unwrap(), b"keep");
    assert!(!annotated.join("oxide-managed").exists());

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

/// Create one real `directory-budget` pool through the product itself. The
/// caller must already have written its fake Cargo and manifest, and the pool
/// path must be inside this fixture's task scratch root (the item-4 rule).
fn created_pool(scratch: &Scratch, bin: &Path, manifest: &Path, pool: &Path) -> () {
    let output = private_env(scratch.path(), bin, pool)
        .current_dir(manifest.parent().unwrap())
        .env("OXIDE_TASK_SCRATCH_ROOT", scratch.path())
        .arg("check")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "pool creation via oxide check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        pool.join("config.json").is_file() && pool.join("oxide-managed").is_dir(),
        "the product did not create the pool at {}",
        pool.display()
    );
}

/// One private owner-only file, the shape the output control reader accepts.
fn write_private(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// The product's checkout identity: `sha256(canonical checkout path text)`.
fn checkout_id(checkout: &Path) -> String {
    let digest = sha2::Sha256::digest(checkout.to_string_lossy().as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn registration_at(control: &Path, mount: &Path) {
    write_private(
        &control.join("removable.json"),
        serde_json::to_vec(&json!({
            "version": 2,
            "mount_point": mount,
            "volume_uuid": "11111111-2222-3333-4444-555555555555",
            "container_uuid": "66666666-7777-8888-9999-000000000000",
            "reserve_bytes": 1,
            "cap_bytes": 1_073_741_824u64,
            "quota_bytes": null,
            "capacity_bytes": null,
            "registered_ms": 1,
            "mode": "permanent",
        }))
        .unwrap()
        .as_slice(),
    );
}

/// A committed whole-target fixture written in the product's own document
/// shapes: one owned local root with a root/2 marker, the legacy `target`
/// symlink, and a transition/2 whole-target receipt. No fact `oxide outputs
/// plan` reads is guessed beyond these published document fields.
fn whole_target_fixture(scratch: &Scratch, bin: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let app = scratch.path().join("app");
    let manifest = app.join("Cargo.toml");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        &manifest,
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(app.join("src/lib.rs"), "pub fn v() -> u32 { 1 }\n").unwrap();
    let checkout = fs::canonicalize(&app).unwrap();
    let pool = scratch.path().join("fixture-pool");
    created_pool(scratch, bin, &manifest, &pool);
    let control = pool.join("oxide-managed");
    // Model positive absence. A present directory with a different UUID is
    // a wrong-volume refusal, never the local fallback this fixture needs.
    let mount = scratch.path().join("absent-mount");
    registration_at(&control, &mount);
    let id = checkout_id(&checkout);
    let root = scratch.path().join("owned-root");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join(".oxide-output.json"),
        serde_json::to_vec(&json!({
            "version": "oxide.outputs.root/2",
            "checkout": checkout,
            "checkout_id": id,
            "label": "app",
            "recipe": "target-debug-v1",
            "kind": "local",
            "nonce": "0123456789abcdef0123456789abcdef",
            "generation": 1,
            "created_ms": 1,
        }))
        .unwrap(),
    )
    .unwrap();
    let selected = json!({"kind": "local", "root": root, "generation": 1});
    std::os::unix::fs::symlink(&root, checkout.join("target")).unwrap();
    write_private(
        &control.join("outputs").join(format!("{id}.json")),
        serde_json::to_vec(&json!({
            "version": "oxide.outputs.transition/2",
            "checkout": checkout,
            "checkout_id": id,
            "generation": 1,
            "state": "committed",
            "links": [{"member": "target", "selected": selected}],
            "updated_ms": 1,
        }))
        .unwrap()
        .as_slice(),
    );
    (checkout, pool, root)
}

/// oxide#182 item 2, at the CLI boundary: `oxide outputs plan` reports the
/// migration `adopt` would perform for a committed whole-target checkout —
/// action, the same classified member set, the root/1→root/2 marker upgrade
/// with its reserved nonce where it applies, the same root at the next
/// generation, and `data_move: false` — and changes no receipt or marker byte.
#[test]
fn outputs_plan_reports_the_whole_target_migration() {
    let scratch = Scratch::new();
    let manifest = scratch.path().join("app/Cargo.toml");
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    fs::write(
        &manifest,
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let (bin, _log) = fake_cargo(scratch.path(), &manifest);
    let (checkout, pool, root) = whole_target_fixture(&scratch, &bin);
    let control = pool.join("oxide-managed");
    let receipt = control
        .join("outputs")
        .join(format!("{}.json", checkout_id(&checkout)));
    let marker = root.join(".oxide-output.json");
    let receipt_before = fs::read(&receipt).unwrap();
    let marker_before = fs::read(&marker).unwrap();
    let ledger_before = fs::read(pool.join("ledger.json")).unwrap();

    let plan = |checkout: &Path, pool: &Path| -> Value {
        let output = private_env(scratch.path(), &bin, pool)
            .current_dir(checkout)
            .args(["outputs", "plan"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "oxide outputs plan failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stdout.len() < 64 * 1024,
            "the plan exceeded its byte bound: {} bytes",
            output.stdout.len()
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };

    let report = plan(&checkout, &pool);
    assert_eq!(report["version"], "oxide.outputs.plan/2", "{report}");
    assert_eq!(report["layout"], "whole-target", "{report}");
    assert_eq!(report["action"], "migrate", "{report}");
    assert_eq!(report["data_move"], json!(false), "{report}");
    assert_eq!(
        report["migration_members"],
        json!(["debug", "release"]),
        "{report}"
    );
    assert!(report.get("marker_upgrade").is_none(), "{report}");
    assert_eq!(
        report["proposed_root"]["root"],
        json!(root.to_string_lossy())
    );
    assert_eq!(report["proposed_root"]["generation"], json!(2));
    assert_eq!(
        fs::read(&marker).unwrap(),
        marker_before,
        "the plan rewrote the marker"
    );
    assert_eq!(
        fs::read(&receipt).unwrap(),
        receipt_before,
        "the plan rewrote the receipt"
    );

    // A root/1 marker is upgraded by `adopt`; the plan says so without
    // reserving the nonce, and writes nothing.
    let mut one: Value = serde_json::from_slice(&marker_before).unwrap();
    one["version"] = json!("oxide.outputs.root/1");
    one.as_object_mut().unwrap().remove("nonce");
    one.as_object_mut().unwrap().remove("checkout_volume_uuid");
    fs::write(&marker, serde_json::to_vec(&one).unwrap()).unwrap();
    let marker_one = fs::read(&marker).unwrap();
    let report = plan(&checkout, &pool);
    assert_eq!(report["action"], "migrate", "{report}");
    assert_eq!(
        report["marker_upgrade"],
        json!({
            "from": "oxide.outputs.root/1",
            "to": "oxide.outputs.root/2",
            "nonce": "reserve",
        }),
        "{report}"
    );
    assert_eq!(
        fs::read(&marker).unwrap(),
        marker_one,
        "the plan rewrote the marker"
    );
    assert_eq!(
        fs::read(&receipt).unwrap(),
        receipt_before,
        "the plan rewrote the receipt"
    );
    assert_eq!(fs::read(pool.join("ledger.json")).unwrap(), ledger_before);
}

/// Every path (directories included) and file byte under one tree, so an
/// effect-free command can be compared exactly.
fn tree_snapshot(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
    fn walk(root: &Path, path: &Path, out: &mut Vec<(String, Option<Vec<u8>>)>) {
        let mut entries: Vec<_> = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let child = entry.path();
            let relative = child
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                out.push((relative, None));
                walk(root, &child, out);
            } else if kind.is_file() {
                out.push((relative, Some(fs::read(&child).unwrap())));
            } else {
                out.push((relative, Some(Vec::new())));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn the_first_writer_waits_for_admission_before_upgrading_v9() {
    use oxide_storage::lease::control::{
        HOLDING_STATE_VERSION, WRITER_GENERATION, WRITER_STATE_VERSION,
    };
    use std::os::fd::AsRawFd;

    let scratch = Scratch::new();
    let app = scratch.path().join("app");
    let manifest = app.join("Cargo.toml");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        &manifest,
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(app.join("src/lib.rs"), "").unwrap();
    let (bin, log) = fake_cargo(scratch.path(), &manifest);
    let pool = scratch.path().join("fixture-pool");
    created_pool(&scratch, &bin, &manifest, &pool);
    let control = pool.join("oxide-managed");
    let state_path = control.join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    state["version"] = json!(HOLDING_STATE_VERSION);
    state
        .as_object_mut()
        .unwrap()
        .remove("minimum_writer_generation");
    write_private(&state_path, &serde_json::to_vec(&state).unwrap());

    let lock = fs::File::open(control.join("admission.lock")).unwrap();
    // SAFETY: the descriptor belongs to this fixture only.
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    // A refused open records a timing observation. Every other byte and path
    // must stay unchanged while the predecessor holds admission.
    let history = "oxide-managed/admission-cost.json";
    let sample_count = || {
        serde_json::from_slice::<Vec<Value>>(&fs::read(pool.join(history)).unwrap())
            .unwrap()
            .len()
    };
    let pool_without_history = || {
        tree_snapshot(&pool)
            .into_iter()
            .filter(|(path, _)| path != history)
            .collect::<Vec<_>>()
    };
    let before = pool_without_history();
    let samples_before = sample_count();
    let waiting = private_env(scratch.path(), &bin, &pool)
        .current_dir(&app)
        .args(["check", "--admission-budget=2s"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&waiting.stderr);
    assert_eq!(waiting.status.code(), Some(124), "{stderr}");
    assert!(
        stderr.contains("stopped by the admission budget"),
        "{stderr}"
    );
    assert_eq!(sample_count(), samples_before + 1);
    assert_eq!(pool_without_history(), before);
    drop(lock);

    let upgraded = private_env(scratch.path(), &bin, &pool)
        .current_dir(&app)
        .arg("check")
        .output()
        .unwrap();
    assert!(
        upgraded.status.success(),
        "{}",
        String::from_utf8_lossy(&upgraded.stderr)
    );
    let fenced: Value = serde_json::from_slice(&fs::read(state_path).unwrap()).unwrap();
    assert_eq!(fenced["version"], json!(WRITER_STATE_VERSION));
    assert_eq!(
        fenced["minimum_writer_generation"],
        json!(WRITER_GENERATION)
    );

    // The covering suite can also exercise an available version-9 executable.
    // Status runs the same schema preflight with a read-only fallback.
    if let Some(predecessor) = std::env::var_os("OXIDE_TEST_V9_PREDECESSOR") {
        let before = tree_snapshot(&pool);
        let cargo_before = fs::read(&log).unwrap();
        let old = private_env_for(Path::new(&predecessor), scratch.path(), &bin, &pool)
            .env_remove("OXIDE_TEST_HOME_FROM_ENV")
            .env("OXIDE_INTERNAL_HANDED_OFF", "1")
            .current_dir(app)
            .arg("status")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&old.stderr);
        assert_eq!(old.status.code(), Some(75), "{stderr}");
        assert!(
            stderr.contains("oxide was upgraded during this command; re-run."),
            "{stderr}"
        );
        assert_eq!(tree_snapshot(&pool), before);
        assert_eq!(fs::read(log).unwrap(), cargo_before);
    }
}

#[cfg(debug_assertions)]
#[test]
fn a_waiting_writer_refuses_a_newer_generation_without_touching_the_pool() {
    use oxide_storage::lease::{control::WRITER_GENERATION, LeaseError};
    use std::os::fd::AsRawFd;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let scratch = Scratch::new();
    let app = scratch.path().join("app");
    let manifest = app.join("Cargo.toml");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        &manifest,
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(app.join("src/lib.rs"), "").unwrap();
    let (bin, _) = fake_cargo(scratch.path(), &manifest);
    let pool = scratch.path().join("fixture-pool");
    created_pool(&scratch, &bin, &manifest, &pool);
    let gate = scratch.path().join("writer-gate");
    let mut child = private_env(scratch.path(), &bin, &pool)
        .current_dir(&app)
        .env("OXIDE_TASK_SCRATCH_ROOT", scratch.path())
        .env("OXIDE_TEST_PAUSE", format!("reserved:{}", gate.display()))
        .arg("check")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let marker = gate.with_extension(format!("reserved-{}", child.id()));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() && child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !marker.exists() {
        let _ = child.kill();
        let output = child.wait_with_output().unwrap();
        panic!(
            "the command did not reach its admission wait: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let control = pool.join("oxide-managed");
    let admission = fs::File::open(control.join("admission.lock")).unwrap();
    // SAFETY: the descriptor belongs to this fixture only.
    assert_eq!(
        unsafe { libc::flock(admission.as_raw_fd(), libc::LOCK_EX) },
        0
    );
    let path = control.join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(state["minimum_writer_generation"], json!(WRITER_GENERATION));
    state["minimum_writer_generation"] = json!(WRITER_GENERATION + 1);
    write_private(&path, &serde_json::to_vec(&state).unwrap());
    let fenced = tree_snapshot(&pool);
    fs::write(gate, "").unwrap();
    drop(admission);
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(i32::from(LeaseError::WriterUpdated.exit_code())),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("oxide was updated while this command waited; rerun it"));
    assert_eq!(tree_snapshot(&pool), fenced);
    let status = private_env(scratch.path(), &bin, &pool)
        .current_dir(&app)
        .env("OXIDE_TASK_SCRATCH_ROOT", scratch.path())
        .arg("status")
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(tree_snapshot(&pool), fenced);
}

/// oxide#182 item 3, at the CLI boundary: `oxide init` readiness names the
/// pool a build would select from the recorded registration, consistent with
/// `status`, and reads no pool state: every recorded pool byte is unchanged.
#[test]
fn init_readiness_names_the_selected_pool() {
    let scratch = Scratch::new();
    let app = scratch.path().join("project");
    let manifest = app.join("Cargo.toml");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        &manifest,
        "[package]\nname = \"project\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(app.join("src/lib.rs"), "pub fn v() -> u32 { 1 }\n").unwrap();
    let (bin, _log) = fake_cargo(scratch.path(), &manifest);
    let pool = scratch.path().join("fixture-pool");
    created_pool(&scratch, &bin, &manifest, &pool);
    let mount = scratch.path().join("volume-mount");
    fs::create_dir_all(&mount).unwrap();
    registration_at(&pool.join("oxide-managed"), &mount);
    let recorded = tree_snapshot(&pool);

    let output = private_env(scratch.path(), &bin, &pool)
        .current_dir(&app)
        .arg("init")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "oxide init failed: {}{stdout}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(&format!(
            "  selected pool: removable {} (registered, mode: permanent",
            mount.display()
        )),
        "readiness did not name the recorded removable selection: {stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "  configured internal pool: {}, use not measured (readiness reads no pool state)",
            pool.display()
        )),
        "readiness did not name the configured internal pool: {stdout}"
    );
    assert_eq!(
        tree_snapshot(&pool),
        recorded,
        "readiness created, rewrote or removed pool state"
    );
    assert!(app.join("AGENTS.md").is_file(), "init wrote no guidance");
}

/// oxide#182, second review: `init` and `new` help must run before the
/// pre-effect hand-off check, which reads pool control state. The pool's
/// recorded state version is raised so that check would refuse; a non-help
/// command (`status`) proves the planted state is live and refused, then both
/// help requests forward exactly Cargo's help and change nothing.
#[test]
fn init_and_new_help_skip_the_handoff_check() {
    let scratch = Scratch::new();
    let app = scratch.path().join("app");
    let manifest = app.join("Cargo.toml");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        &manifest,
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(app.join("src/lib.rs"), "pub fn v() -> u32 { 1 }\n").unwrap();
    let (bin, log) = fake_cargo(scratch.path(), &manifest);
    let pool = scratch.path().join("fixture-pool");
    created_pool(&scratch, &bin, &manifest, &pool);

    // Plant a newer control state. Any command past the early help branch
    // reaches the hand-off check and refuses with its pinned line.
    let state = pool.join("oxide-managed/state.json");
    let mut document: Value = fs::read(&state)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| json!({}));
    document["version"] = json!(99);
    write_private(&state, serde_json::to_vec(&document).unwrap().as_slice());
    let refused = private_env(scratch.path(), &bin, &pool)
        .current_dir(&app)
        .arg("status")
        .output()
        .unwrap();
    assert_eq!(
        refused.status.code(),
        Some(75),
        "the planted newer state did not reach the hand-off check: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("oxide was upgraded during this command; re-run."),
        "the hand-off control printed another refusal"
    );

    let before = tree_snapshot(&pool);
    for command in ["init", "new"] {
        let _ = fs::remove_file(&log);
        let output = private_env(scratch.path(), &bin, &pool)
            .current_dir(&app)
            .args([command, "--help"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "oxide {command} --help ran the hand-off check: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            format!("{command} --help\n"),
            "oxide {command} --help did not forward exactly Cargo's help"
        );
        assert_eq!(
            tree_snapshot(&pool),
            before,
            "oxide {command} --help changed pool state"
        );
    }
    assert!(!app.join("AGENTS.md").exists(), "help wrote guidance");
}

/// Preview must not reconcile pool slots, sweep retained trash or publish a
/// missing output declaration, even when the registry ultimately refuses.
#[test]
fn output_prune_dry_run_preserves_pool_trash_and_partial_output_accounts() {
    use oxide_storage::lease::control::{OutputsInit, State, OWNED_OUTPUTS_FENCE};
    use oxide_storage::storage::{AreaConfig, Storage};

    let scratch = Scratch::new();
    let app = scratch.path().join("app");
    fs::create_dir(&app).unwrap();
    let app = fs::canonicalize(app).unwrap();
    let manifest = app.join("Cargo.toml");
    fs::write(
        &manifest,
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let (bin, _) = fake_cargo(scratch.path(), &manifest);
    let pool = scratch.path().join("fixture-pool");
    created_pool(&scratch, &bin, &manifest, &pool);
    let control = pool.join("oxide-managed");
    let storage = Storage::open(&control).unwrap();
    let pool_area = storage
        .areas()
        .unwrap()
        .into_iter()
        .find(|area| area.key().label() == "oxide/pool")
        .unwrap();
    let retained = pool_area.create_root("retained-trash").unwrap();
    fs::write(retained.path.join("keep"), b"retained owned bytes").unwrap();
    let trash = retained.path.parent().unwrap().join(".trash");
    fs::create_dir_all(&trash).unwrap();
    // Simulate a crash after Barge's rename, using its actual marked root.
    let retained_path = trash.join("retained-preview-fixture");
    fs::rename(&retained.path, &retained_path).unwrap();

    let run = || {
        private_env(scratch.path(), &bin, &pool)
            .current_dir(&app)
            .args(["outputs", "prune", "--dry-run"])
            .output()
            .unwrap()
    };
    let registry_path = control.join("outputs/registry.json");
    if registry_path.exists() {
        fs::remove_file(&registry_path).unwrap();
    }
    let before = tree_snapshot(&pool);
    let refused = run();
    assert!(!refused.status.success(), "a missing registry was accepted");
    assert_eq!(
        tree_snapshot(&pool),
        before,
        "a refusing preview changed pool state or trash"
    );

    let transaction = "a".repeat(32);
    let state_path = control.join("state.json");
    let mut state: State = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    state.version = state.version.max(4);
    state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
    state.outputs_init = Some(OutputsInit::Ready {
        transaction: transaction.clone(),
        ready_ms: 1,
    });
    write_private(&state_path, &serde_json::to_vec(&state).unwrap());

    let parent = scratch.path().join("outputs");
    let location: String = sha2::Sha256::digest(parent.as_os_str().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let output_storage = Storage::open(&control.join("output-storage").join(location)).unwrap();
    let area = output_storage
        .declare(
            &AreaConfig::new("outputs", scratch.path(), "outputs")
                .folder("outputs")
                .floor_bytes(0),
        )
        .unwrap();
    let root = area.create_root("output").unwrap();
    fs::write(root.path.join("keep"), b"output bytes").unwrap();
    write_private(
        &registry_path,
        &serde_json::to_vec(&json!({
            "version": "oxide.outputs.registry/2", "schema": 2, "revision": 1,
            "complete": true, "created_by": transaction,
            "rows": [{"checkout_id": checkout_id(&app), "checkout": app,
                "receipt": control.join("outputs/fixture.json"), "member": "debug",
                "root": root.path, "barge_id": root.id, "root_kind": "local",
                "recipe": "quick", "generation": 1, "selected": false, "pending": false}]
        }))
        .unwrap(),
    );

    let before = tree_snapshot(&pool);
    let present = run();
    assert!(
        present.status.success(),
        "{}",
        String::from_utf8_lossy(&present.stderr)
    );
    assert_eq!(
        tree_snapshot(&pool),
        before,
        "preview changed a complete account"
    );
    let preview: Value = serde_json::from_slice(&present.stdout).unwrap();
    assert_eq!(preview["roots"][0]["id"], root.id);
    // Fault the fresh account through both partial states without forging a
    // root marker or replacing the real Barge parser.
    let areas = output_storage.state_dir().join("areas");
    fs::remove_file(areas.join("oxide.outputs.json")).unwrap();
    for missing_directory in [false, true] {
        if missing_directory {
            fs::remove_dir(&areas).unwrap();
        }
        let before = tree_snapshot(&pool);
        let preview = run();
        assert!(
            preview.status.success(),
            "{}",
            String::from_utf8_lossy(&preview.stderr)
        );
        assert_eq!(
            tree_snapshot(&pool),
            before,
            "preview published missing area state"
        );
        assert_eq!(fs::read(root.path.join("keep")).unwrap(), b"output bytes");
        assert_eq!(
            fs::read(retained_path.join("keep")).unwrap(),
            b"retained owned bytes"
        );
    }
    // An entirely absent account must stay absent, rather than acquiring an
    // empty areas directory during the supposedly passive open.
    let account = output_storage.state_dir();
    fs::remove_dir_all(account).unwrap();
    let before = tree_snapshot(&pool);
    let preview = run();
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    assert!(!account.exists(), "preview recreated the absent account");
    assert_eq!(
        tree_snapshot(&pool),
        before,
        "preview changed an absent account"
    );
    assert_eq!(fs::read(root.path.join("keep")).unwrap(), b"output bytes");
    assert_eq!(
        fs::read(retained_path.join("keep")).unwrap(),
        b"retained owned bytes"
    );
}
