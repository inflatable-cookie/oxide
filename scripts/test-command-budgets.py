#!/usr/bin/env python3
"""Focused proof for opt-in command budgets (oxide#140).

Source only until its owner activation: this script is the whole command

    python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 \\
        -- python3 scripts/test-command-budgets.py

and it writes only inside the supervisor-owned scratch root. It never creates
an image, a mount or a live pool, and every fixture child ends by itself when
the candidate dies (release file plus a short natural exit).

Cases and oracles (see docs/knowledge/contracts/workflow.md#command-time-budgets-oxide140):

  1 contention longer than the execution window still succeeds in its own
  2 admission expiry starts no primary child
  3 a fast start and a plain spawn failure
  4 an execution deadline stops a slow child
  5 precedence: caller signal, a genuine 124/125 child, TERM then KILL
  6 a bounded `run` uses one cumulative build+program window
  7 the event channel matrix (type, access, flag, full, broken, gap, sticky loss)
  8 an admission-only start (nullable execution axis) and a stopped
    preparation child reported as the admission deadline
  9 a late start forced through the debug spawn-delay control
 10 no settings or ledger mutation
 11 unbudgeted commands keep their old behaviour
"""

import json
import os
import shutil
import signal
import subprocess
import time
from pathlib import Path

EXIT_OK = 0
ADMISSION_EXIT = 124
EXECUTION_EXIT = 125


def fail(message):
    print(f"FAIL: {message}", flush=True)
    raise SystemExit(1)


def run(command, env, timeout=90, check=False):
    started = time.monotonic()
    result = subprocess.run(
        command,
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    result.elapsed = time.monotonic() - started
    if check and result.returncode != EXIT_OK:
        fail(f"{command} exited {result.returncode}: {result.stderr[-2000:]}")
    return result


def write_executable(path, text):
    path.write_text(text)
    path.chmod(0o755)


def fixture_pool(root, name):
    pool = root / f"pool-{name}"
    pool.mkdir(parents=True)
    return pool


def base_env(root, name, **extra):
    home = root / f"home-{name}"
    cargo_home = root / f"cargo-{name}"
    home.mkdir(exist_ok=True)
    cargo_home.mkdir(exist_ok=True)
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": str(home),
        "CARGO_HOME": str(cargo_home),
        "OXIDE_POOL": str(fixture_pool(root, name)),
        "OXIDE_POOL_BACKEND": "directory-budget",
        "OXIDE_POOL_CAP": "256MiB",
        "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", ""),
        "TMPDIR": str(root),
        "LANG": "C.UTF-8",
    }
    env.update(extra)
    return env


def stage_oxide(root):
    """Stage this HEAD's debug oxide offline with the pinned toolchain."""
    staged = root / "target"
    env = dict(os.environ)
    env.update({"CARGO_TARGET_DIR": str(staged), "CARGO_NET_OFFLINE": "true"})
    run(
        [
            "cargo",
            "build",
            "--locked",
            "--offline",
            "--bin",
            "oxide",
            "-j",
            "1",
            "--config",
            "profile.dev.debug=0",
            "--config",
            "profile.dev.incremental=false",
        ],
        env,
        timeout=180,
        check=True,
    )
    binary = staged / "debug" / "oxide"
    if not binary.is_file():
        fail("the staged oxide binary is missing")
    return binary


def event_reader(fd):
    """Create a nonblocking pipe and hand back (read_fd, write_fd)."""
    read_fd, write_fd = os.pipe()
    os.set_blocking(read_fd, False)
    os.set_blocking(write_fd, False)
    return read_fd, write_fd


def records(raw):
    out = []
    for line in raw.splitlines():
        if not line:
            continue
        try:
            out.append(json.loads(line))
        except json.JSONDecodeError:
            return None
    return out


def hold_slot(root, env, release, label):
    """Hold the only slot with a paused `exec` fixture until `release` exists."""
    tool = root / f"hold-{label}.sh"
    write_executable(tool, f"#!/bin/sh\ntouch {root}/{label}.held\nwhile [ ! -e {release} ]; do sleep 0.1; done\n")
    process = subprocess.Popen(
        [str(BINARY), "exec", "--", str(tool)],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )
    deadline = time.monotonic() + 60
    while not (root / f"{label}.held").exists():
        if process.poll() is not None:
            fail(f"the {label} holder exited early: {process.stderr.read()[-1000:]}")
        if time.monotonic() > deadline:
            fail(f"the {label} holder never took its slot")
        time.sleep(0.05)
    return process


def case_contention(root):
    """Waiting longer than the execution budget still succeeds inside it."""
    env = base_env(root, "contention", OXIDE_MAX_ACTIVE="1")
    marker = root / "contention.started"
    tool = root / "builder.sh"
    write_executable(tool, f"#!/bin/sh\ntouch {marker}\nsleep 0.2\n")
    release = root / "contention.release"
    holder = hold_slot(root, env, release, "contention")
    try:
        process = subprocess.Popen(
            [
                str(BINARY),
                "exec",
                "--admission-budget",
                "60s",
                "--execution-budget",
                "2s",
                "--",
                str(tool),
            ],
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        time.sleep(4)
        if marker.exists():
            fail("the contender started while the only slot was held")
        release.write_text("go\n")
        stdout, stderr = process.communicate(timeout=60)
    finally:
        holder.terminate()
        holder.wait(timeout=30)
    if process.returncode != EXIT_OK:
        fail(f"contention case exited {process.returncode}: {stderr[-1500:]}")
    if not marker.exists():
        fail("the contender never started after the release")
    if "admission" in stderr or "execution budget" in stderr:
        fail(f"the contention case reported a budget stop: {stderr[-1500:]}")
    print("ok contention: waited past the execution budget, child succeeded", flush=True)


def case_admission_expiry(root):
    """An expired admission window starts no primary child."""
    env = base_env(root, "admission", OXIDE_MAX_ACTIVE="1")
    marker = root / "admission.started"
    tool = root / "never.sh"
    write_executable(tool, f"#!/bin/sh\ntouch {marker}\n")
    release = root / "admission.release"
    holder = hold_slot(root, env, release, "admission")
    try:
        result = run(
            [
                str(BINARY),
                "exec",
                "--admission-budget",
                "1s",
                "--",
                str(tool),
            ],
            env,
            timeout=60,
        )
    finally:
        holder.terminate()
        holder.wait(timeout=30)
    if result.returncode != ADMISSION_EXIT:
        fail(f"admission expiry exited {result.returncode}, wanted {ADMISSION_EXIT}")
    if marker.exists():
        fail("an admission-expired command started its child")
    if "admission budget" not in result.stderr:
        fail(f"admission expiry did not name the budget: {result.stderr[-1000:]}")
    print("ok admission expiry: no child started, exit 124", flush=True)


def case_fast_start_and_spawn_failure(root):
    env = base_env(root, "fast")
    tool = root / "fast.sh"
    write_executable(tool, "#!/bin/sh\nexit 0\n")
    read_fd, write_fd = event_reader()
    with os.fdopen(read_fd, "r") as reader:
        result = subprocess.run(
            [
                str(BINARY),
                "exec",
                "--admission-budget",
                "30s",
                "--execution-budget",
                "30s",
                "--budget-events",
                str(write_fd),
                "--",
                str(tool),
            ],
            env=env,
            pass_fds=(write_fd,),
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=90,
        )
        os.close(write_fd)
        raw = reader.read()
    if result.returncode != EXIT_OK:
        fail(f"the fast start exited {result.returncode}: {result.stderr[-1000:]}")
    parsed = records(raw)
    if parsed is None:
        fail(f"the event stream is not newline JSON: {raw[:400]}")
    kinds = [entry.get("event") for entry in parsed]
    if kinds[:2] != ["admitted_child_started", "finished"]:
        fail(f"unexpected event order: {kinds}")
    if parsed[1].get("outcome") != "child_exit" or parsed[1].get("exit_code") != 0:
        fail(f"unexpected terminal record: {parsed[1]}")
    if parsed[0].get("spawn_late") is not False:
        fail("a fast start was reported late")
    if parsed[0].get("admission_expired_before_start") is not False:
        fail("a fast start claimed an expired admission window")
    if parsed[0].get("admission_ms") != 30_000 or parsed[0].get("execution_ms") != 30_000:
        fail(f"the started record lost a caller budget: {parsed[0]}")
    if parsed[0].get("admission_overshoot_ms") is not None:
        fail("a fast start reported an admission overshoot")
    # Independently optional axes: an execution-only command reports a null
    # admission budget, and an admission-only one a null execution budget.
    read_fd, write_fd = event_reader()
    with os.fdopen(read_fd, "r") as reader:
        execution_only = subprocess.run(
            [str(BINARY), "exec", "--execution-budget", "30s", "--budget-events", str(write_fd), "--", str(tool)],
            env=env,
            pass_fds=(write_fd,),
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=90,
        )
        os.close(write_fd)
        raw = reader.read()
    if execution_only.returncode != EXIT_OK:
        fail(f"the execution-only command exited {execution_only.returncode}")
    started = (records(raw) or [{}])[0]
    if started.get("admission_ms") is not None or started.get("execution_ms") != 30_000:
        fail(f"the execution-only axis is wrong: {started}")
    print("ok fast start: started then finished(child_exit), nullable axes", flush=True)

    missing = run(
        [str(BINARY), "exec", "--execution-budget", "5s", "--", str(root / "absent")],
        env,
        timeout=60,
    )
    if missing.returncode != 127:
        fail(f"a missing program exited {missing.returncode}, wanted 127")
    print("ok spawn failure: exit 127, no started record", flush=True)


def case_execution_deadline(root):
    env = base_env(root, "deadline")
    tool = root / "slow.sh"
    write_executable(tool, "#!/bin/sh\nexec sleep 60\n")
    read_fd, write_fd = event_reader()
    with os.fdopen(read_fd, "r") as reader:
        result = subprocess.run(
            [
                str(BINARY),
                "exec",
                "--execution-budget",
                "1s",
                "--budget-events",
                str(write_fd),
                "--",
                str(tool),
            ],
            env=env,
            pass_fds=(write_fd,),
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=120,
        )
        os.close(write_fd)
        raw = reader.read()
    if result.returncode != EXECUTION_EXIT:
        fail(f"the execution deadline exited {result.returncode}, wanted {EXECUTION_EXIT}")
    if "execution budget" not in result.stderr:
        fail(f"the stop did not name the execution budget: {result.stderr[-1000:]}")
    if result.elapsed > 1 + 15:
        fail(f"the stop took {result.elapsed:.1f}s, past the finite closeout bound")
    parsed = records(raw) or []
    terminal = [entry for entry in parsed if entry.get("event") == "finished"]
    if len(terminal) != 1 or terminal[0].get("outcome") != "execution_deadline":
        fail(f"unexpected terminal record: {terminal}")
    print("ok execution deadline: 125, named budget, finite closeout", flush=True)


def case_precedence(root):
    env = base_env(root, "precedence")
    # A child that exits 125 by itself under a generous budget is a child exit.
    reader = root / "self125.sh"
    write_executable(reader, "#!/bin/sh\nexit 125\n")
    result = run(
        [str(BINARY), "exec", "--execution-budget", "60s", "--", str(reader)],
        env,
        timeout=60,
    )
    if result.returncode != 125 or "budget" in result.stderr:
        fail(f"genuine child 125 was treated as a budget stop: {result.returncode} {result.stderr[-500:]}")
    # A caller signal before expiry keeps its own status.
    sleeper = root / "sleep.sh"
    write_executable(sleeper, "#!/bin/sh\nexec sleep 60\n")
    process = subprocess.Popen(
        [str(BINARY), "exec", "--execution-budget", "30s", "--", str(sleeper)],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )
    time.sleep(0.4)
    process.send_signal(signal.SIGTERM)
    process.communicate(timeout=60)
    if process.returncode != 143:
        fail(f"a caller SIGTERM ended {process.returncode}, wanted 143")
    print("ok precedence: child 125 and caller 143 are not budget stops", flush=True)


def case_cumulative_run(root):
    env = base_env(root, "run")
    crate = root / "run-fixture"
    (crate / "src").mkdir(parents=True)
    write_executable(
        crate / "Cargo.toml",
        '[package]\nname = "run-fixture"\nversion = "0.1.0"\nedition = "2021"\n',
    )
    write_executable(
        crate / "src" / "main.rs",
        'fn main() { std::thread::sleep(std::time::Duration::from_secs(60)); }\n',
    )
    result = run(
        [
            str(BINARY),
            "run",
            "--execution-budget",
            "20s",
            "--manifest-path",
            str(crate / "Cargo.toml"),
        ],
        env,
        timeout=180,
    )
    if result.returncode != EXECUTION_EXIT:
        fail(f"the cumulative run exited {result.returncode}, wanted {EXECUTION_EXIT}")
    # One window: the program cannot add a second 60 s sleep to the build time.
    if result.elapsed > 20 + 20:
        fail(f"the cumulative run took {result.elapsed:.1f}s: the clock was reset")
    print("ok cumulative run: one window through build and program", flush=True)


def case_events(root):
    env = base_env(root, "events")
    tool = root / "events.sh"
    write_executable(tool, "#!/bin/sh\nexit 0\n")
    # A regular file, a read end and a blocking pipe are refused before any effect.
    regular = root / "not-a-pipe"
    regular.write_text("x")
    fd = os.open(regular, os.O_RDWR)
    try:
        refused = run(
            [str(BINARY), "exec", "--execution-budget", "5s", "--budget-events", str(fd), "--", str(tool)],
            env,
            timeout=60,
        )
        if refused.returncode == EXIT_OK:
            fail("a regular file was accepted as an event channel")
        if "not a pipe" not in refused.stderr:
            fail(f"the regular-file refusal did not name the type: {refused.stderr[-800:]}")
    finally:
        os.close(fd)
    read_fd, write_fd = os.pipe()
    try:
        refused = run(
            [str(BINARY), "exec", "--execution-budget", "5s", "--budget-events", str(read_fd), "--", str(tool)],
            env,
            timeout=60,
        )
        if refused.returncode == EXIT_OK:
            fail("a read end was accepted as an event channel")
        refused = run(
            [str(BINARY), "exec", "--execution-budget", "5s", "--budget-events", str(write_fd), "--", str(tool)],
            env,
            timeout=60,
        )
        if refused.returncode == EXIT_OK:
            fail("a blocking pipe was accepted as an event channel")
    finally:
        os.close(read_fd)
        os.close(write_fd)
    # A reader that closes after the first record loses the stream without
    # changing the command's own status or timing.
    read_fd, write_fd = event_reader()
    process = subprocess.Popen(
        [str(BINARY), "exec", "--execution-budget", "10s", "--budget-events", str(write_fd), "--", str(tool)],
        env=env,
        pass_fds=(write_fd,),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )
    os.close(write_fd)
    os.read(read_fd, 64)
    os.close(read_fd)
    _, stderr = process.communicate(timeout=60)
    if process.returncode != EXIT_OK:
        fail(f"a broken event reader changed the outcome: {process.returncode} {stderr[-500:]}")
    if stderr.count("event channel became unusable") > 1:
        fail("the loss note was printed more than once")
    # Sticky loss: when the stream is full before the command starts, nothing
    # is ever written, and a later terminal record cannot appear.
    read_fd, write_fd = event_reader()
    filler = b"x" * 4096
    while True:
        try:
            os.write(write_fd, filler)
        except BlockingIOError:
            break
    result = subprocess.run(
        [str(BINARY), "exec", "--execution-budget", "10s", "--budget-events", str(write_fd), "--", str(tool)],
        env=env,
        pass_fds=(write_fd,),
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=90,
    )
    os.close(write_fd)
    drained = b""
    while True:
        try:
            chunk = os.read(read_fd, 65536)
        except BlockingIOError:
            break
        if not chunk:
            break
        drained += chunk
    os.close(read_fd)
    if result.returncode != EXIT_OK:
        fail(f"a full event pipe changed the command outcome: {result.returncode}")
    if b'"event"' in drained:
        fail("a record was written after the stream was lost")
    print("ok events: type/access/flag refusals, broken reader survives, sticky loss", flush=True)


def state_shape(control):
    """The recorded control state's key set, or None when there is no state."""
    path = Path(control) / "state.json"
    if not path.is_file():
        return None
    try:
        document = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return "unreadable"
    return sorted(document) if isinstance(document, dict) else "not-an-object"


def case_admission_only_and_preparation_stop(root):
    """An admission-only start is typed, and a stopped preparation child is 124."""
    env = base_env(root, "admission-only")
    tool = root / "admission-only.sh"
    write_executable(tool, "#!/bin/sh\nexit 0\n")
    read_fd, write_fd = event_reader()
    with os.fdopen(read_fd, "r") as reader:
        result = subprocess.run(
            [
                str(BINARY),
                "exec",
                "--admission-budget",
                "30s",
                "--budget-events",
                str(write_fd),
                "--",
                str(tool),
            ],
            env=env,
            pass_fds=(write_fd,),
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=90,
        )
        os.close(write_fd)
        raw = reader.read()
    if result.returncode != EXIT_OK:
        fail(f"the admission-only command exited {result.returncode}: {result.stderr[-800:]}")
    started = (records(raw) or [{}])[0]
    if started.get("admission_ms") != 30_000 or started.get("execution_ms") is not None:
        fail(f"the admission-only started record is wrong: {started}")
    if started.get("execution_started_ms") is not None:
        fail(f"an admission-only start claimed an execution clock: {started}")
    if started.get("spawn_late") is not False:
        fail(f"an admission-only fast start was reported late: {started}")
    print("ok admission-only: started record with a null execution axis", flush=True)

    # A preparation child that outlives a tiny admission window is the
    # admission deadline, never an internal error: the fake cargo below sleeps
    # inside `cargo locate-project`, so the bounded helper stops it.
    real_cargo = shutil.which("cargo")
    if not real_cargo:
        fail("the proof needs the real cargo on PATH for the fake")
    fake = root / "fakebin"
    fake.mkdir(exist_ok=True)
    write_executable(
        fake / "cargo",
        f"#!/bin/sh\ncase \" $* \" in *locate-project*) sleep 5 ;; esac\nexec {real_cargo} \"$@\"\n",
    )
    slow_env = dict(env)
    slow_env["PATH"] = f"{fake}:{env['PATH']}"
    marker = root / "prep.started"
    prep_tool = root / "prep.sh"
    write_executable(prep_tool, f"#!/bin/sh\ntouch {marker}\n")
    result = run(
        [str(BINARY), "exec", "--admission-budget", "1ms", "--", str(prep_tool)],
        slow_env,
        timeout=60,
    )
    if result.returncode != ADMISSION_EXIT:
        fail(f"a stopped preparation child exited {result.returncode}, wanted {ADMISSION_EXIT}")
    if marker.exists():
        fail("a stopped preparation child still started the primary program")
    print("ok preparation stop: exit 124, no primary child", flush=True)


def case_late_start(root):
    """A start after the admission deadline is reported and stopped at once."""
    env = base_env(root, "late", OXIDE_TEST_SPAWN_DELAY_MS="3000")
    tool = root / "late.sh"
    write_executable(tool, "#!/bin/sh\nexec sleep 60\n")
    read_fd, write_fd = event_reader()
    with os.fdopen(read_fd, "r") as reader:
        result = subprocess.run(
            [
                str(BINARY),
                "exec",
                "--admission-budget",
                "2s",
                "--execution-budget",
                "60s",
                "--budget-events",
                str(write_fd),
                "--",
                str(tool),
            ],
            env=env,
            pass_fds=(write_fd,),
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=120,
        )
        os.close(write_fd)
        raw = reader.read()
    if result.returncode != ADMISSION_EXIT:
        fail(f"the late start exited {result.returncode}, wanted {ADMISSION_EXIT}")
    if "admission budget" not in result.stderr:
        fail(f"the late start did not name the admission budget: {result.stderr[-800:]}")
    parsed = records(raw) or []
    started = [entry for entry in parsed if entry.get("event") == "admitted_child_started"]
    if not started:
        fail(f"the late start emitted no started record: {parsed}")
    record = started[0]
    if record.get("spawn") is not True or record.get("spawn_late") is not True:
        fail(f"the late start was not reported truthfully: {record}")
    if record.get("admission_expired_before_start") is not True:
        fail(f"the late start did not report the expired window: {record}")
    if not isinstance(record.get("admission_overshoot_ms"), int):
        fail(f"the late start reported no overshoot: {record}")
    if record.get("execution_started_ms") is not None and record.get("execution_ms") is None:
        fail(f"a late start claimed an execution clock without one: {record}")
    # The group was stopped and released: the next command gets the slot.
    follow = run([str(BINARY), "exec", "--", str(root / "admission-only.sh")], env, timeout=90)
    if follow.returncode != EXIT_OK:
        fail(f"the slot was not released after a late start: {follow.returncode}")
    print("ok late start: spawn_late, admission overshoot, stopped and released", flush=True)


def state_keys(pool):
    """The recorded control state's top-level keys (empty when none exists)."""
    for path in sorted(Path(pool).rglob("state.json")):
        try:
            document = json.loads(path.read_text())
        except (OSError, json.JSONDecodeError):
            continue
        if isinstance(document, dict):
            return path.parent, sorted(document)
    return None, []


def case_no_mutation(root):
    """Budgets add no settings file, persisted field or report line."""
    env = base_env(root, "mutation")
    tool = root / "mutation.sh"
    write_executable(tool, "#!/bin/sh\nexit 0\n")
    pool = Path(env["OXIDE_POOL"])
    home = Path(env["HOME"])
    config = home / ".config" / "oxide"
    # Warm the pool with an unbudgeted command so the comparison is about the
    # budget, not about first-use state.
    run([str(BINARY), "exec", "--", str(tool)], env, timeout=90, check=True)
    control, before_keys = state_keys(pool)
    if control is None:
        fail("the fixture pool recorded no control state to compare")
    run(
        [str(BINARY), "exec", "--admission-budget", "30s", "--execution-budget", "30s", "--", str(tool)],
        env,
        timeout=90,
        check=True,
    )
    _, after_keys = state_keys(pool)
    if before_keys != after_keys:
        fail(f"a budgeted command changed the recorded fields: {set(after_keys) ^ set(before_keys)}")
    for name in os.listdir(control):
        if "budget" in name:
            fail(f"a budgeted command created {name} in the control directory")
    if config.exists() and any(config.iterdir()):
        fail("a budgeted command wrote a settings file")
    status = run([str(BINARY), "status"], env, timeout=90)
    if status.returncode != EXIT_OK:
        fail(f"status exited {status.returncode}: {status.stderr[-800:]}")
    for word in ("admission budget", "execution budget", "budget-events"):
        if word in status.stdout or word in status.stderr:
            fail(f"status gained a budget-only line: {word}")
    print("ok no mutation: no settings, no new state fields, no budget report lines", flush=True)


def case_unbudgeted(root):
    env = base_env(root, "plain")
    tool = root / "plain.sh"
    write_executable(tool, "#!/bin/sh\nexit 3\n")
    result = run([str(BINARY), "exec", "--", str(tool)], env, timeout=60)
    if result.returncode != 3:
        fail(f"an unbudgeted exec exited {result.returncode}, wanted 3")
    if "budget" in result.stderr:
        fail("an unbudgeted command printed a budget line")
    print("ok unbudgeted: unchanged status and output", flush=True)


def case_nested(root):
    """A nested oxide call waits inside, and consumes, the outer window."""
    env = base_env(root, "nested", OXIDE_MAX_ACTIVE="1")
    crate = root / "nested-fixture"
    (crate / "src").mkdir(parents=True, exist_ok=True)
    (crate / "Cargo.toml").write_text(
        '[package]\nname = "nested-fixture"\nversion = "0.1.0"\nedition = "2021"\n'
    )
    (crate / "src" / "lib.rs").write_text("pub fn value() -> u8 { 1 }\n")
    marker = root / "nested.started"
    release = root / "nested.release"
    holder = hold_slot(root, env, release, "nested")
    tool = root / "nested-tool.sh"
    write_executable(
        tool,
        "#!/bin/sh\n"
        f"touch {marker}\n"
        f"{BINARY} check --admission-budget 60s --manifest-path {crate}/Cargo.toml\n"
        "exit 0\n",
    )
    try:
        result = run(
            [str(BINARY), "exec", "--execution-budget", "3s", "--", str(tool)],
            env,
            timeout=90,
        )
    finally:
        holder.terminate()
        holder.wait(timeout=30)
    if result.returncode != EXECUTION_EXIT:
        fail(f"the nested case exited {result.returncode}, wanted {EXECUTION_EXIT}")
    if not marker.exists():
        fail("the outer tool never started")
    if "execution budget" not in result.stderr:
        fail(f"the outer stop did not name the execution budget: {result.stderr[-800:]}")
    # The inner call was still waiting on the only slot when the outer window
    # closed: it started no compiler of its own.
    if (crate / "target").exists():
        fail("the inner command started a build after the outer window closed")
    print("ok nested: the outer window kept running while the inner call waited", flush=True)


def case_unknown_closure(root):
    """The debug-only hook proves the held-custody path without a false claim."""
    env = base_env(root, "closure", OXIDE_TEST_CLOSURE_STUCK="1")
    tool = root / "closure.sh"
    write_executable(tool, "#!/bin/sh\nexec sleep 60\n")
    read_fd, write_fd = event_reader()
    with os.fdopen(read_fd, "r") as reader:
        result = subprocess.run(
            [str(BINARY), "exec", "--execution-budget", "1s", "--budget-events", str(write_fd), "--", str(tool)],
            env=env,
            pass_fds=(write_fd,),
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=120,
        )
        os.close(write_fd)
        raw = reader.read()
    if result.returncode != 1:
        fail(f"the injected unknown closure exited {result.returncode}, wanted 1")
    terminal = [entry for entry in (records(raw) or []) if entry.get("event") == "finished"]
    if len(terminal) != 1:
        fail(f"the unknown-closure case emitted {len(terminal)} terminal records")
    record = terminal[0]
    if record.get("outcome") != "unconfirmed_closure":
        fail(f"unexpected unknown-closure outcome: {record}")
    if record.get("drain") == "complete" or record.get("lease") != "held":
        fail(f"unknown closure claimed completion or release: {record}")
    held = run([str(BINARY), "status"], env, timeout=90)
    if "held" not in held.stdout and "interrupted" not in held.stdout:
        fail(f"status did not show the held slot: {held.stdout[-800:]}")
    print("ok unknown closure: exit 1, typed unconfirmed_closure, custody held", flush=True)


def main():
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
    if not scratch:
        fail("run this script under scripts/bounded-test.py (OXIDE_TASK_SCRATCH_ROOT is unset)")
    root = Path(scratch).resolve()
    work = root / "command-budgets"
    work.mkdir(parents=True, exist_ok=True)
    global BINARY
    BINARY = stage_oxide(work)
    print(f"staged {BINARY}", flush=True)

    case_fast_start_and_spawn_failure(work)
    case_admission_expiry(work)
    case_contention(work)
    case_execution_deadline(work)
    case_precedence(work)
    case_cumulative_run(work)
    case_events(work)
    case_admission_only_and_preparation_stop(work)
    case_late_start(work)
    case_nested(work)
    case_unknown_closure(work)
    case_no_mutation(work)
    case_unbudgeted(work)
    shutil.rmtree(work / "target", ignore_errors=True)
    print("test-command-budgets: all cases passed", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
