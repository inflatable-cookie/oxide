use super::{fake_cargo, private_env, Scratch, OXIDE};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

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

impl Fixture {
    fn install_blocking_cargo(&self) -> PathBuf {
        let other = self.scratch.path().join("other");
        fs::create_dir(&other).unwrap();
        fs::copy(self.checkout.join("Cargo.toml"), other.join("Cargo.toml")).unwrap();
        fs::write(
            self.bin.join("cargo"),
            r#"#!/bin/sh
            case "$1" in
              locate-project) echo "$PWD/Cargo.toml" ;;
              check)
                if [ "${CRASH_BUILD:-}" = 1 ]; then
                    kill -KILL "$PPID"
                elif [ -n "${BUILD_GATE:-}" ]; then
                    touch "$BUILD_GATE.ready"
                    while [ ! -f "$BUILD_GATE.done" ]; do sleep 0.02; done
                fi
                ;;
            esac
            "#,
        )
        .unwrap();
        other
    }

    fn status(&self) -> String {
        let output = self.command().arg("status").output().unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        String::from_utf8(output.stdout).unwrap()
    }

    fn wait_for(&self, name: &str) {
        eventually(|| self.scratch.path().join(name).exists(), name);
    }

    fn signal(&self, name: &str) {
        fs::write(self.scratch.path().join(name), "").unwrap();
    }
}

fn eventually(mut predicate: impl FnMut() -> bool, description: &str) {
    let until = Instant::now() + Duration::from_secs(12);
    while !predicate() {
        assert!(
            Instant::now() < until,
            "timed out waiting for {description}"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn idle_exec_yields_and_concurrent_nested_builds_share_one_permit() {
    let fixture = Fixture::new();
    let other = fixture.install_blocking_cargo();
    let mut outer = fixture
        .command()
        .env("GATES", fixture.scratch.path())
        .args([
            "exec",
            "--execution-budget",
            "45s",
            "--",
            "/bin/sh",
            "-ec",
            r#"
            touch "$GATES/outer.ready"
            while [ ! -f "$GATES/outer.build" ]; do sleep 0.02; done
            if "$1" check --admission-budget 500ms; then exit 99; else
                test "$?" -eq 124
            fi
            touch "$GATES/deadline.done"
            BUILD_GATE="$GATES/first" "$1" check --admission-budget 25s &
            first=$!
            while [ ! -f "$GATES/second.build" ]; do sleep 0.02; done
            BUILD_GATE="$GATES/second" "$1" exec -- "$1" check --admission-budget 15s &
            second=$!
            wait "$first"
            touch "$GATES/first.returned"
            wait "$second"
            touch "$GATES/second.returned"
            while [ ! -f "$GATES/outer.done" ]; do sleep 0.02; done
            "#,
            "outer",
            OXIDE,
        ])
        .spawn()
        .unwrap();
    let idle = "held by exec /bin/sh (slot custody only; no host build permit)";
    let borrowed = "held by exec /bin/sh (host build permit borrowed)";
    fixture.wait_for("outer.ready");
    eventually(|| fixture.status().contains(idle), "the yielded exec slot");

    let mut unrelated = fixture
        .command()
        .current_dir(&other)
        .env("BUILD_GATE", fixture.scratch.path().join("other"))
        .args([
            "check",
            "--admission-budget",
            "8s",
            "--execution-budget",
            "30s",
        ])
        .spawn()
        .unwrap();
    fixture.wait_for("other.ready");
    fixture.signal("outer.build");
    fixture.wait_for("deadline.done");
    let waiter = format!("recorded lease-ticket waiter PID {}", outer.id());
    eventually(
        || fixture.status().contains(&waiter),
        "the exec permit ticket",
    );
    assert!(!fixture.scratch.path().join("first.ready").exists());
    assert!(fixture.status().contains(idle));

    fixture.signal("other.done");
    assert!(unrelated.wait().unwrap().success());
    fixture.wait_for("first.ready");
    assert!(fixture.status().contains(borrowed));
    fixture.signal("second.build");
    fixture.wait_for("second.ready");
    fixture.signal("first.done");
    fixture.wait_for("first.returned");
    assert!(fixture.status().contains(borrowed));
    let blocked = fixture
        .command()
        .current_dir(&other)
        .args(["check", "--admission-budget", "750ms"])
        .output()
        .unwrap();
    assert_eq!(blocked.status.code(), Some(124), "{}", stderr(&blocked));

    fixture.signal("second.done");
    fixture.wait_for("second.returned");
    eventually(
        || fixture.status().contains(idle),
        "the returned build permit",
    );
    let admitted = fixture
        .command()
        .current_dir(&other)
        .args(["check", "--admission-budget", "5s"])
        .output()
        .unwrap();
    assert!(admitted.status.success(), "{}", stderr(&admitted));
    fixture.signal("outer.done");
    assert!(outer.wait().unwrap().success());
    fixture.assert_idle_slots(5);
}

#[test]
fn a_crashed_nested_build_returns_its_borrow_while_exec_keeps_the_slot() {
    let fixture = Fixture::new();
    let other = fixture.install_blocking_cargo();
    let mut outer = fixture
        .command()
        .env("GATES", fixture.scratch.path())
        .args([
            "exec",
            "--execution-budget",
            "25s",
            "--",
            "/bin/sh",
            "-ec",
            r#"
            if CRASH_BUILD=1 "$1" check --admission-budget 10s; then exit 99; else
                test "$?" -eq 137
            fi
            touch "$GATES/crash.done"
            while [ ! -f "$GATES/outer.done" ]; do sleep 0.02; done
            "#,
            "outer",
            OXIDE,
        ])
        .spawn()
        .unwrap();
    fixture.wait_for("crash.done");
    eventually(
        || {
            fixture
                .status()
                .contains("held by exec /bin/sh (slot custody only; no host build permit)")
        },
        "the crashed child's returned build permit",
    );
    let admitted = fixture
        .command()
        .current_dir(&other)
        .args(["check", "--admission-budget", "5s"])
        .output()
        .unwrap();
    assert!(admitted.status.success(), "{}", stderr(&admitted));
    fixture.signal("outer.done");
    assert!(outer.wait().unwrap().success());
    fixture.assert_idle_slots(3);
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
fn with_broker(
    fixture: &Fixture,
    remaining_ms: u64,
    run: impl FnOnce(&mut Command) -> Output,
) -> Output {
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
            .set_read_timeout(Some(Duration::from_secs(90)))
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
        .env("OXIDE_SESSION_BROKER", &path)
        .env("OXIDE_SESSION_TOKEN", TOKEN));
    server.join().unwrap();
    fs::remove_file(path).unwrap();
    fs::remove_dir(dir).unwrap();
    output
}

#[test]
fn exhausted_outer_window_refuses_before_opening_a_pool() {
    let fixture = Fixture::new();
    let output = with_broker(&fixture, 0, |command| {
        command
            .args(["exec", "--admission-budget", "30s", "--", "/usr/bin/true"])
            .output()
            .unwrap()
    });
    assert_eq!(output.status.code(), Some(124), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("stopped by the admission budget"),
        "{}",
        stderr(&output)
    );
    assert!(!fixture.pool.exists());
    assert!(!fixture.scratch.path().join("cargo.log").exists());
}

fn pass_budget_event_pipe(command: &mut Command, writer: std::io::PipeWriter) {
    let fd = writer.as_raw_fd();
    // SAFETY: the pipe is new and writer owns the open descriptor.
    assert_eq!(
        unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) },
        0
    );
    command.arg("--budget-events").arg(fd.to_string());
    // SAFETY: the closure keeps the descriptor open and uses only fcntl and
    // errno in the child. Clearing CLOEXEC here affects only this child.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(writer.as_raw_fd(), libc::F_SETFD, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[test]
fn execution_budget_uses_the_tighter_window() {
    const OUTER_MS: u64 = 60_000;
    let fixture = Fixture::new();
    // First-use pool creation and its Time Machine exclusion belong to setup.
    let setup = fixture
        .command()
        .args(["exec", "--", "/bin/sh", "-c", ":"])
        .output()
        .unwrap();
    assert!(setup.status.success(), "{}", stderr(&setup));

    for (inner_budget, tool, code, outcome) in [
        ("90s", "echo tool-started", 0, "child_exit"),
        (
            "2s",
            "echo tool-started; exec /bin/sleep 20",
            125,
            "execution_deadline",
        ),
    ] {
        let (reader, writer) = std::io::pipe().unwrap();
        let output = with_broker(&fixture, OUTER_MS, |command| {
            command.args([
                "exec",
                "--admission-budget",
                "90s",
                "--execution-budget",
                inner_budget,
            ]);
            pass_budget_event_pipe(command, writer);
            command
                .args(["--", "/bin/sh", "-c", tool])
                .output()
                .unwrap()
        });
        let error = stderr(&output);
        assert_eq!(output.status.code(), Some(code), "{error}");
        assert_eq!(output.stdout, b"tool-started\n");
        let events: Vec<Value> = BufReader::new(reader)
            .lines()
            .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
            .collect();
        let [started, finished] = events.as_slice() else {
            panic!("expected start and finish records: {events:?}\n{error}");
        };
        assert_eq!(started["event"], "admitted_child_started");
        assert_eq!(started["spawn_late"], false);
        let admission = started["admission_ms"].as_u64().unwrap();
        // Admission is reported from command start, which precedes Join.
        assert!((1..90_000).contains(&admission), "{started}");
        let execution = started["execution_ms"].as_u64().unwrap();
        if code == 0 {
            assert!((1..=OUTER_MS).contains(&execution), "{started}");
            assert!(
                execution < admission,
                "admission consumes the outer window: {started}"
            );
        } else {
            assert_eq!(execution, 2000, "{started}");
        }
        assert_eq!(finished["event"], "finished");
        assert_eq!(finished["outcome"], outcome);
        assert_eq!(finished["exit_code"], code);
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
