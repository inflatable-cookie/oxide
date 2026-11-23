use super::{fake_cargo, private_env, Scratch, OXIDE};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::Duration;

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct Fixture {
    scratch: Scratch,
    checkout: PathBuf,
    bin: PathBuf,
    pool: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let scratch = Scratch::new();
        let checkout = scratch.path().join("app");
        fs::create_dir(&checkout).unwrap();
        let manifest = checkout.join("Cargo.toml");
        fs::write(
            &manifest,
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let (bin, _) = fake_cargo(scratch.path(), &manifest);
        let pool = scratch.path().join("pool");
        Self {
            scratch,
            checkout,
            bin,
            pool,
        }
    }

    fn command(&self) -> Command {
        let mut command = private_env(self.scratch.path(), &self.bin, &self.pool);
        command
            .current_dir(&self.checkout)
            .env("OXIDE_TASK_SCRATCH_ROOT", self.scratch.path())
            .env("OXIDE_MAX_ACTIVE", "1");
        command
    }

    fn assert_idle_slots(&self, count: usize) {
        let control = self.pool.join(oxide_storage::lease::MANAGED_NAME);
        let state: Value =
            serde_json::from_slice(&fs::read(control.join("state.json")).unwrap()).unwrap();
        let records = state["records"].as_array().unwrap();
        assert_eq!(records.len(), count, "{records:?}");
        assert!(
            records.iter().all(|record| record["phase"] == "idle"),
            "{records:?}"
        );
        assert_eq!(state["tickets"].as_array().unwrap().len(), 0);
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn nested_execs_take_spare_slots_at_a_host_limit_of_one() {
    let fixture = Fixture::new();
    let tool = fixture.scratch.path().join("nested.sh");
    fs::write(
        &tool,
        r#"
        printf '%s\n' "$CARGO_BUILD_BUILD_DIR" "$OXIDE_SESSION_BROKER"
        if [ "$2" -gt 0 ]; then
            "$1" exec -- /bin/sh -e "$0" "$1" "$(( $2 - 1 ))"
        fi
        "#,
    )
    .unwrap();
    let output = fixture
        .command()
        .args(["exec", "--execution-budget", "15s", "--", "/bin/sh", "-e"])
        .arg(tool)
        .args([OXIDE, "2"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        !stderr(&output).contains("waiting for a build slot"),
        "{}",
        stderr(&output)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines.len(), 6, "{stdout}");
    let build_dirs = [lines[0], lines[2], lines[4]];
    assert!(build_dirs
        .iter()
        .all(|path| Path::new(path).starts_with(&fixture.pool)));
    assert_ne!(build_dirs[0], build_dirs[1]);
    assert_ne!(build_dirs[0], build_dirs[2]);
    assert_ne!(build_dirs[1], build_dirs[2]);
    for endpoint in [lines[1], lines[3], lines[5]] {
        let dir = Path::new(endpoint).parent().unwrap();
        assert!(
            dir.starts_with(fixture.scratch.path()) && !dir.exists(),
            "{endpoint}"
        );
    }
    fixture.assert_idle_slots(3);
}

#[test]
fn a_joined_slot_leaves_the_other_host_permit_available() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("OXIDE_MAX_ACTIVE", "2")
        .args([
            "exec",
            "--execution-budget",
            "15s",
            "--",
            "/bin/sh",
            "-ec",
            r#"
        "$1" exec -- /bin/sh -ec '
            touch member-ready
            while [ ! -f member-done ]; do sleep 0.02; done
        ' &
        member=$!
        trap 'touch member-done; wait "$member"' EXIT
        while [ ! -f member-ready ]; do sleep 0.02; done
        env -u OXIDE_SESSION_BROKER -u OXIDE_SESSION_TOKEN \
            "$1" exec --admission-budget 3s -- /usr/bin/true
        "#,
            "outer",
            OXIDE,
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    fixture.assert_idle_slots(3);
}

/// Supply a fixed remaining window without a competing outer signal. The
/// conversation also proves the process releases on the original connection.
fn with_broker(remaining_ms: u64, run: impl FnOnce(&mut Command) -> Output) -> (Fixture, Output) {
    let fixture = Fixture::new();
    let dir = fixture.scratch.path().join("broker");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.join("s");
    let listener = UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        let start = std::time::Instant::now();
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && start.elapsed() < Duration::from_secs(5) =>
                {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("broker accept: {error}"),
            }
        };
        // Darwin accepted sockets inherit the listener's nonblocking mode.
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let join: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(join["body"]["type"], "join");
        assert!(join["body"]["token"].as_str() == Some(TOKEN));
        writeln!(
            reader.get_mut(),
            "{{\"version\":1,\"body\":{{\"type\":\"joined\",\"session_id\":\"test\",\"member_id\":1,\"permit_mode\":\"inherited\",\"remaining_execution_ms\":{remaining_ms}}}}}"
        ).unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let release: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(release["body"]["type"], "release");
        assert_eq!(release["body"]["member_id"], 1);
        writeln!(
            reader.get_mut(),
            r#"{{"version":1,"body":{{"type":"released"}}}}"#
        )
        .unwrap();
    });
    let output = run(fixture
        .command()
        .env("OXIDE_SESSION_BROKER", path)
        .env("OXIDE_SESSION_TOKEN", TOKEN));
    server.join().unwrap();
    (fixture, output)
}

#[test]
fn exhausted_outer_window_refuses_before_opening_a_pool() {
    let (fixture, output) = with_broker(0, |command| {
        command
            .args(["exec", "--admission-budget", "30s", "--", "/usr/bin/true"])
            .output()
            .unwrap()
    });
    assert_eq!(output.status.code(), Some(124), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("admission budget (0ms)"),
        "{}",
        stderr(&output)
    );
    assert!(!fixture.pool.exists());
    assert!(!fixture.scratch.path().join("cargo.log").exists());
}

#[test]
fn execution_budget_uses_the_tighter_window() {
    // Allow scratch-pool creation under contention so this exercises execution.
    for (outer_ms, inner_budget, limit_ms) in [(8000, "30s", 8000), (30_000, "500ms", 500)] {
        let (fixture, output) = with_broker(outer_ms, |command| {
            command
                .args([
                    "exec",
                    "--admission-budget",
                    "30s",
                    "--execution-budget",
                    inner_budget,
                    "--",
                    "/bin/sh",
                    "-c",
                    "echo tool-started; exec /bin/sleep 20",
                ])
                .output()
                .unwrap()
        });
        let error = stderr(&output);
        assert_eq!(output.status.code(), Some(125), "{error}");
        assert_eq!(output.stdout, b"tool-started\n");
        let limit = error
            .lines()
            .find_map(|line| line.strip_prefix("oxide: stopped by the execution budget ("))
            .and_then(|rest| rest.strip_suffix("ms)"))
            .and_then(|ms| ms.parse::<u64>().ok())
            .expect("effective execution budget");
        assert!((1..=limit_ms).contains(&limit), "{error}");
        fixture.assert_idle_slots(1);
        assert!(
            fs::read_dir(fixture.scratch.path()).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("ox.")
            }),
            "the leased child's broker directory survived timeout"
        );
    }
}

#[test]
fn unreachable_broker_refuses_leases_and_leaves_nonjoining_commands_alone() {
    let fixture = Fixture::new();
    let mut command = fixture.command();
    command
        .env(
            "OXIDE_SESSION_BROKER",
            fixture.scratch.path().join("missing"),
        )
        .env("OXIDE_SESSION_TOKEN", TOKEN);
    let output = command
        .args(["exec", "--", "/usr/bin/true"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("cannot join the outer Oxide session"),
        "{}",
        stderr(&output)
    );

    for args in [
        &["--help"][..],
        &["exec", "--help"],
        &["fmt"],
        &["deny"],
        &["status"],
        &["pool", "describe", "--json"],
        &["outputs", "plan"],
    ] {
        let output = fixture
            .command()
            .env(
                "OXIDE_SESSION_BROKER",
                fixture.scratch.path().join("missing"),
            )
            .env("OXIDE_SESSION_TOKEN", TOKEN)
            .args(args)
            .output()
            .unwrap();
        assert!(
            !stderr(&output).contains("session"),
            "{args:?}: {}",
            stderr(&output)
        );
    }
    assert!(!fixture.pool.exists());
}
