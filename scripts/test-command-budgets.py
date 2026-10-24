#!/usr/bin/env python3
"""Proof apparatus for opt-in command budgets (oxide#140).

One fixed script and the whole command of the qualified shared supervisor:

    python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 \\
        -- python3 scripts/test-command-budgets.py

`--plan` prints the exact command, cases, phases, disk allocation, child
lifetimes, concurrency and limits without any effect; an unrecognized argument
refuses before staging, subprocesses or helper loads; `--self-check` runs the
pure plan/argument/oracle checks in this process. Only the bare invocation is
the counted proof, and this source is HELD until ROOT activates a native run.

Apparatus contract
------------------
- One absolute work deadline: every child timeout is clipped to the guard's
  exported `OXIDE_PROOF_DEADLINE_NS` minus the reserved cleanup window, so no
  independent per-case timeout can outlive the guard.
- The staged candidate is this clean HEAD's debug `oxide`: plain Cargo, the
  invoking Cargo home, `--locked --offline`, pinned `RUSTUP_TOOLCHAIN`,
  no auto-install, one job, explicit target and build directories inside the
  monitored scratch, then copied out, hashed and identified; only the staging
  directories are removed. Fixtures never see a staged target.
- Every fixture runs in a private scratch tree with its own `HOME`,
  `CARGO_HOME`, `OXIDE_CONFIG`, `OXIDE_POOL` and temp, from a private tiny
  checkout `cwd`, so no case can act on this repository or a live pool. A
  missing control (scratch, deadline, evidence, explicit toolchain) refuses
  before any storage effect.
- Children this script starts are recorded, given bounded lifetimes, and
  released through their own release files or plain `terminate()`/`kill()` on
  the exact PIDs it started. It creates no new session or process group, runs
  no census and sends no broad signals. Direct leader reaping is not
  descendant closure: the qualified guard alone decides final session
  closure.
- A uniquely named bounded helper receipt is written beside the guard's own
  in `OXIDE_PROOF_EVIDENCE`. It records identities, reached/completed/failed/
  unexecuted cases, the primary fault and resource observations, and never
  claims guard closure.

Frozen acceptance map (frozen A2 section 10, committee-corrected)
----------------------------------------------------------------
1. Contention longer than the execution window -> `contention-longer-than-execution`.
2. No primary child on admission expiry (marker + typed event + ticket) ->
   `admission-expiry-no-start`.
3. Fast start and separate axes/nullable fields -> `fast-start-axes`.
4. Immediate spawn failure with typed evidence -> `spawn-failure-event`.
5. Slow child deadline -> `execution-deadline`.
6. Signal/closure precedence: caller SIGTERM, genuine child 125, TERM trap ->
   `precedence-caller-signal`, `precedence-child-125`, `precedence-term-trap`.
7. Cumulative build+program window that would fail a reset implementation, on
   the budget-only backend with no run-program space watchdog ->
   `cumulative-run-window`.
8. Nested call waits inside the outer window without resetting it ->
   `nested-outer-window`.
9. Typed event matrix: type/access/flag refusals, full pipe, broken reader,
   first-record protocol, contiguous sequence -> `events-matrix`,
   `events-broken-reader`.
10. Deterministic unknown closure/held custody (debug hook) ->
    `unknown-closure-held`.
11. Stopped preparation child is the admission deadline ->
    `preparation-stop-124`.
12. Late start reported and stopped -> `late-start`.
13. No mutation: refused inputs have no effect, and a successful budgeted
    command adds no settings file and no persisted budget field ->
    `no-mutation`.
14. Unbudgeted commands keep their behaviour -> `unbudgeted`.
"""

from __future__ import annotations

import hashlib
import json
import os
import select
import shutil
import signal
import subprocess
import sys
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPT = os.path.abspath(__file__)
BUILDER = "1.97.1"
# `bounded-test.py` at this HEAD. The owning documentation records the hash
# this proof source was reviewed against; the supervisor's own receipt, not
# this helper, remains authoritative for session closure.
EXPECTED_RUNNER_SHA256 = "961164e797c01432fa8234497627f75e56489ef6395ef66e48824cdb0e48c052"
WALL_LIMIT_S = 300
SCRATCH_BUDGET_MIB = 512
SHUTDOWN_MARGIN_S = 30
STAGE_JOBS = 1
FIXTURE_JOBS = 1
MAX_RECEIPT_BYTES = 64 * 1024
SELECTOR = "rust:command-budgets"
ADMISSION_EXIT = 124
EXECUTION_EXIT = 125
EVENT_BYTES = 512
COMMAND = [
    "python3",
    "scripts/bounded-test.py",
    "--limit",
    str(WALL_LIMIT_S),
    "--scratch-budget-mib",
    str(SCRATCH_BUDGET_MIB),
    "--",
    "python3",
    "scripts/test-command-budgets.py",
]
USAGE = """usage: test-command-budgets.py [--plan | --self-check | --self-test | --help]

Without an argument this runs the counted proof under the qualified shared
supervisor. --plan prints the fixed command, cases, phases, disk allocation,
child lifetimes, concurrency and limits without any effect. --self-check runs
the pure plan, argument, duration, event and deadline checks in this process
without staging or touching a fixture. --self-test executes the corrected case
bodies against faked process and filesystem seams (no child, image or pool) to
catch body-versus-plan drift, and exercises the whole-lifecycle fault/receipt
paths.
"""
# Case allocation in seconds, clipped further by the exported guard deadline.
# All estimates are unmeasured until a counted native run.
PHASE_ALLOCATION_S = {
    "stage": 90,
    "prepare": 15,
    "cases": 145,
    "cleanup": 20,
    "margin": SHUTDOWN_MARGIN_S,
}
CASES: list[str] = []
RESULTS: list[dict] = []
OWNED: list[subprocess.Popen] = []
UNCERTAIN: list[str] = []
PRIMARY_FAULT: str | None = None
PHASE_STARTED = time.perf_counter()


class Fault(Exception):
    """A proof assertion or fixture failure, carrying its own text."""


def fail(message: str) -> None:
    """Record the first fault, keep custody honest, and stop.

    `PRIMARY_FAULT` keeps the first message even if a later cleanup step also
    fails, so the receipt always records the original cause.
    """
    global PRIMARY_FAULT
    if PRIMARY_FAULT is None:
        PRIMARY_FAULT = message
    say(f"FAILED: {message}")
    for process in OWNED:
        release(process)
    raise Fault(message)


def expect(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def say(message: str) -> None:
    print(f"[{time.perf_counter() - PHASE_STARTED:7.1f}s] {message}", flush=True)


def case(name: str) -> None:
    CASES.append(name)
    say(f"case {name}")


def result(name: str, passed: bool, detail: str) -> None:
    RESULTS.append({"case": name, "result": "pass" if passed else "fail", "detail": detail})


# --------------------------------------------------------------------------
# Deadline arithmetic and the pure oracles checked by --self-check.


def clip(phase_seconds: float, left: float) -> float:
    """The one rule every child timeout uses: never outlive the guard."""
    if left <= 0:
        fail("the guard's work deadline is exhausted")
    return max(1.0, min(float(phase_seconds), left))


def work_left() -> float:
    guard = os.environ.get("OXIDE_PROOF_DEADLINE_NS")
    if not guard:
        fail("OXIDE_PROOF_DEADLINE_NS is not exported; this proof runs only under the guard")
    left = (int(guard) - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 5:
        fail("the guard's work deadline is inside its shutdown margin")
    return left


def timeout(phase_seconds: float) -> float:
    return clip(phase_seconds, work_left())


def remaining_quiet() -> float:
    """Seconds of guard work left, without raising: custody steps use this.

    A release or closeout must never call `fail()` (which itself releases owned
    children) or it could replace the primary fault with a recursive one.
    """
    guard = os.environ.get("OXIDE_PROOF_DEADLINE_NS")
    if not guard:
        return 0.0
    left = (int(guard) - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - SHUTDOWN_MARGIN_S
    return max(0.0, left)


def custody_bound() -> float:
    """A short wait for one owned direct child, inside the guard's leftover.

    The bound is half of what is left after the guard's own shutdown margin, so
    cumulative custody work cannot push the script past the guard's stop. It is
    a best-effort local release only: it says nothing about descendants, which
    remain outside this script's custody proof.
    """
    return min(2.0, max(0.05, remaining_quiet() / 2))


def parse_arguments(argv: list[str]) -> tuple[str, str | None]:
    """Fail-closed argument boundary: at most one known mode, no effects."""
    if not argv:
        return ("proof", None)
    if len(argv) > 1:
        return ("refuse", f"at most one argument is accepted, got {argv!r}")
    mode = {
        "--plan": "plan",
        "--self-check": "self-check",
        "--self-test": "self-test",
        "--help": "help",
    }
    if argv[0] in mode:
        return (mode[argv[0]], None)
    return ("refuse", f"unrecognized argument {argv[0]!r}")


def phase_allocation() -> dict:
    return dict(PHASE_ALLOCATION_S)


def allocation_fits() -> bool:
    return sum(PHASE_ALLOCATION_S.values()) <= WALL_LIMIT_S


def parse_records(raw: str) -> list[dict] | None:
    """Newline JSON records, or None when any line is not a JSON object."""
    records = []
    for line in raw.splitlines():
        if not line:
            continue
        if len(line.encode()) + 1 > EVENT_BYTES:
            return None
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            return None
        if not isinstance(entry, dict):
            return None
        records.append(entry)
    return records


def sequence_oracle(records: list[dict]) -> tuple[bool, str]:
    """Contiguous seq from 1 and exactly one terminal record when complete."""
    sequences = [entry.get("seq") for entry in records]
    if sequences != list(range(1, len(records) + 1)):
        return (False, f"sequence gap or non-contiguous seq: {sequences}")
    terminals = [entry for entry in records if entry.get("event") == "finished"]
    if len(terminals) > 1:
        return (False, f"{len(terminals)} terminal records")
    return (True, "contiguous")


def descriptor_refusal(described: str) -> str:
    """The expected refusal word for a channel that is not a writable pipe."""
    if "file" in described:
        return "not a pipe"
    if "read" in described:
        return "not writable"
    if "blocking" in described:
        return "not nonblocking"
    return "not a pipe"


def stage_identity(commit: str, tree: str, digest: str, script: str) -> dict:
    return {
        "source_commit": commit,
        "source_tree": tree,
        "binary_sha256": digest,
        "script_sha256": script,
        "supervisor_sha256": EXPECTED_RUNNER_SHA256,
        "toolchain": BUILDER,
    }


def plan_document() -> dict:
    return {
        "task": "oxide#140",
        "selector": SELECTOR,
        "command": COMMAND,
        "counted_only_without_argument": True,
        "modes": ["--plan", "--self-check", "--help"],
        "cases": [
            {"name": "fast-start-axes", "oracle": "both budgets: started then finished(child_exit 0), spawn_late false, both budget fields present; execution-only: admission_ms null; admission-only: execution_ms and execution_started_ms null"},
            {"name": "spawn-failure-event", "oracle": "missing program: exit 127, no started record, exactly one finished(spawn_failure)"},
            {"name": "admission-expiry-no-start", "oracle": "held slot + 1s admission: exit 124, no primary marker, no started record, finished(admission_deadline), contender ticket gone"},
            {"name": "contention-longer-than-execution", "oracle": "held slot released after ~3s + 60s admission/2s execution: starts after release, exit 0, execution_elapsed <=2s though wall >=3s"},
            {"name": "execution-deadline", "oracle": "60s sleeper + 1s execution: exit 125, finished(execution_deadline), budget named, group settled, slot reusable"},
            {"name": "precedence-child-124", "oracle": "child exits 124 itself under a 60s budget: child_exit 124, no budget line, no latched origin"},
            {"name": "precedence-caller-signal", "oracle": "SIGTERM before expiry: 143, released, no deadline origin"},
            {"name": "precedence-child-125", "oracle": "child exits 125 itself under a 60s budget: child_exit, no budget line"},
            {"name": "precedence-term-trap", "oracle": "TERM-trapping child under 1s: trap marker present, exit 125, finished(execution_deadline)"},
            {"name": "cumulative-run-window", "oracle": "3s build.rs + 60s program under one 5s execution window on a budget-only pool: exit 125 in run_program and the launched program ran 0.5s<=t<3.5s after the build hand-off (a reset window would let it run ~5s); closeout latency is not asserted"},
            {"name": "nested-outer-window", "oracle": "the outer exec starts first and takes the only slot; its tool starts an inner oxide that waits and ends on its own 1s admission budget with no build; the tool keeps running so the outer 2s window (not a natural exit) stops it: exit 125 with a typed exec_tool start and execution_deadline"},
            {"name": "events-matrix", "oracle": "regular file, read end and blocking pipe refuse before any effect; full pipe yields no typed record; contiguous sequence on a healthy stream"},
            {"name": "events-broken-reader", "oracle": "read end closed before start: exit 0, parent survives, one loss note, no typed record"},
            {"name": "unknown-closure-held", "oracle": "debug closure hook + 1s execution: exit 1, finished(unconfirmed_closure), drain not complete, lease held, status shows the held slot"},
            {"name": "preparation-stop-124", "oracle": "fake cargo sleeping in locate-project + 1ms admission: exit 124, no primary marker"},
            {"name": "late-start", "oracle": "debug spawn delay past a 2s admission: exit 124, started record with spawn_late and overshoot, slot released"},
            {"name": "no-mutation", "oracle": "a refused input leaves home/config/pool unchanged; a successful budgeted command adds no settings file, no new control-state fields and no budget lines in status"},
            {"name": "unbudgeted", "oracle": "exit 3 preserved, no budget line, no events required"},
        ],
        "phases": [
            {"name": "stage", "seconds": PHASE_ALLOCATION_S["stage"], "allocation": "one debug oxide, plain Cargo offline, target and build inside scratch, staging deleted before fixtures", "disk_mib": 220},
            {"name": "prepare", "seconds": PHASE_ALLOCATION_S["prepare"], "allocation": "private fixture roots and tiny std-only crates", "disk_mib": 20},
            {"name": "cases", "seconds": PHASE_ALLOCATION_S["cases"], "allocation": "sequential fixture commands; peak: one staged binary, one compiler job, at most two live fixture processes", "disk_mib": 260},
            {"name": "cleanup", "seconds": PHASE_ALLOCATION_S["cleanup"], "allocation": "owned direct children released, fixture roots removed", "disk_mib": 0},
            {"name": "margin", "seconds": PHASE_ALLOCATION_S["margin"], "allocation": "reserved for the guard's own shutdown", "disk_mib": 0},
        ],
        "disk": {
            "monitored_scratch_mib": SCRATCH_BUDGET_MIB,
            "estimated_peak_mib": 500,
            "note": "estimate only; the guard's monitoring remains authoritative",
        },
        "children": {
            "tracking": "exact PIDs this script started, recorded in OWNED",
            "lifetime": "bounded per case and clipped to the guard deadline; fixtures self-end on a release file or their own budget",
            "release": "release file first, then terminate() and bounded kill() on those exact PIDs",
            "sessions": "no new session or process group is created; no census and no broad signals",
            "closure": "direct leader reaping is not descendant closure; the guard decides session closure",
        },
        "concurrency": {
            "staged_binaries": 1,
            "stage_cargo_jobs": STAGE_JOBS,
            "fixture_cargo_jobs": FIXTURE_JOBS,
            "fixture_commands": "sequential, at most one candidate and one holder at a time",
        },
        "limits": {
            "wall_limit_s": WALL_LIMIT_S,
            "scratch_budget_mib": SCRATCH_BUDGET_MIB,
            "shutdown_margin_s": SHUTDOWN_MARGIN_S,
            "allocation_fits": allocation_fits(),
            "timing": "unmeasured",
        },
        "provenance": {
            "staging": "invoking Cargo home, offline, explicit target/build in scratch; not a claim that staging never reads it",
            "fixtures": "private HOME/CARGO_HOME/OXIDE_CONFIG/OXIDE_POOL/temp and a private tiny checkout cwd",
            "private_home": "OXIDE_TEST_HOME_FROM_ENV=1 with explicit private config and pool (oxide#143 convention)",
            "toolchain": "RUSTUP_TOOLCHAIN=1.97.1, RUSTUP_AUTO_INSTALL=0, RUSTUP_HOME captured before HOME is replaced",
        },
        "live_pool": False,
        "mounted_volume": False,
    }


# --------------------------------------------------------------------------
# Small filesystem and process helpers.


def sha256_file(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def private_dir(path: str) -> str:
    os.makedirs(path, exist_ok=True)
    os.chmod(path, 0o700)
    return path


def write_private(path: str, data: str) -> None:
    private_dir(os.path.dirname(path))
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(data)
    os.chmod(path, 0o600)


def write_executable(path: str, data: str) -> None:
    write_private(path, data)
    os.chmod(path, 0o700)


def read_text(path: str) -> str:
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as handle:
            return handle.read()
    except FileNotFoundError:
        return ""


def git(*args: str) -> str:
    result = subprocess.run(
        ["git", *args], cwd=REPO, check=True, capture_output=True, text=True, timeout=60
    )
    return result.stdout.strip()


def release(process: subprocess.Popen) -> None:
    """Best-effort custody for one owned direct child, with clipped waits.

    It never waits past the guard's remaining window and never claims that a
    descendant closed. An unresolved child is reported as unknown custody.
    """
    if process.poll() is not None:
        return
    try:
        process.terminate()
        process.wait(timeout=custody_bound())
    except (subprocess.TimeoutExpired, OSError):
        try:
            process.kill()
            process.wait(timeout=custody_bound())
        except (subprocess.TimeoutExpired, OSError) as error:
            note = f"owned pid {process.pid} did not settle ({error}); closure is unknown"
            UNCERTAIN.append(note)
            say(f"custody: {note}")


def start(argv: list[str], *, cwd: str, env: dict, **kwargs) -> subprocess.Popen:
    """Start one owned direct child (no new session or process group)."""
    process = subprocess.Popen(argv, cwd=cwd, env=env, **kwargs)
    OWNED.append(process)
    return process


def run(
    argv: list[str],
    *,
    cwd: str,
    env: dict,
    phase_seconds: float,
    pass_fds: tuple[int, ...] = (),
    stdin=None,
) -> subprocess.CompletedProcess:
    """Run one owned direct child with the guard-clipped timeout."""
    started = time.monotonic()
    process = start(
        argv,
        cwd=cwd,
        env=env,
        pass_fds=pass_fds,
        stdin=subprocess.DEVNULL if stdin is None else stdin,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout(phase_seconds))
    except subprocess.TimeoutExpired:
        # The primary fault is captured before any cleanup: cleanup arithmetic
        # must never replace it.
        fault_text = f"{argv[0]} exceeded its clipped timeout of {phase_seconds:.0f}s"
        process.kill()
        stdout, stderr = "", ""
        try:
            stdout, stderr = process.communicate(timeout=custody_bound())
        except (subprocess.TimeoutExpired, OSError) as error:
            note = f"owned pid {process.pid} outlived its clipped timeout ({error}); output is unknown"
            UNCERTAIN.append(note)
            say(f"custody: {note}")
        fail(fault_text)
    result = subprocess.CompletedProcess(argv, process.returncode, stdout, stderr)
    result.elapsed = time.monotonic() - started
    return result


# --------------------------------------------------------------------------
# Fixtures: private environment, tiny checkouts, bounded holders.


def invoking_rustup_home() -> str:
    explicit = os.environ.get("RUSTUP_HOME")
    if explicit:
        return explicit
    return os.path.join(os.path.expanduser("~"), ".rustup")


def require_controls() -> tuple[str, str]:
    """Refuse before any storage effect when a private control is missing."""
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE")
    expect(bool(scratch), "OXIDE_TASK_SCRATCH_ROOT is not set")
    expect(bool(evidence), "OXIDE_PROOF_EVIDENCE is not set")
    scratch = os.path.abspath(scratch)
    evidence = os.path.abspath(evidence)
    expect(
        scratch.startswith("/private/tmp/") or scratch.startswith("/tmp/"),
        f"scratch root {scratch} is not a private temporary directory",
    )
    expect(os.path.isdir(scratch), f"scratch root {scratch} is not a directory")
    expect(os.path.isdir(evidence), f"evidence directory {evidence} does not exist")
    return (scratch, evidence)


def fixture_env(root: str, label: str) -> dict:
    """A private allowlisted environment: no host Oxide, XDG or Cargo setting."""
    home = private_dir(os.path.join(root, f"home-{label}"))
    cargo_home = private_dir(os.path.join(root, f"cargo-home-{label}"))
    config = os.path.join(root, f"oxide-config-{label}.toml")
    write_private(config, "")
    pool = os.path.join(root, f"pool-{label}")
    env = {
        "HOME": home,
        "OXIDE_TEST_HOME_FROM_ENV": "1",
        "OXIDE_CONFIG": config,
        "OXIDE_POOL": pool,
        "OXIDE_POOL_BACKEND": "directory-budget",
        "OXIDE_POOL_CAP": "256MiB",
        "CARGO_HOME": cargo_home,
        "TMPDIR": root,
        "TMP": root,
        "TEMP": root,
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "NO_COLOR": "1",
        "CARGO_TERM_COLOR": "never",
        "CARGO_NET_OFFLINE": "true",
        "RUSTUP_HOME": invoking_rustup_home(),
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_AUTO_INSTALL": "0",
        "LANG": "C.UTF-8",
    }
    for name in ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_PROOF_DEADLINE_NS", "OXIDE_PROOF_EVIDENCE"):
        if name in os.environ:
            env[name] = os.environ[name]
    say(f"fixture {label}: private home, cargo home, config, pool and temp")
    return env


def create_crate(root: str, label: str) -> str:
    crate = private_dir(os.path.join(root, "checkouts", label))
    write_private(
        os.path.join(crate, "Cargo.toml"),
        f'[package]\nname = "oxide-budget-{label}"\nversion = "0.1.0"\nedition = "2021"\n',
    )
    write_private(os.path.join(crate, "src", "lib.rs"), "pub fn fixture() -> u8 { 1 }\n")
    return crate


def close_holder(process: subprocess.Popen, release_file: str) -> None:
    """Ask a holder to end through its own release file, then bound the wait."""
    try:
        write_private(release_file, "done\n")
    except OSError:
        pass
    try:
        process.wait(timeout=min(5.0, max(0.2, remaining_quiet())))
    except subprocess.TimeoutExpired:
        release(process)


def hold_slot(
    root: str, env: dict, checkout: str, label: str, seconds: float
) -> tuple[subprocess.Popen, str]:
    """Hold the only slot with a bounded exec fixture.

    The holder's tool loops on its own release file and also exits by itself
    after `seconds`, so cleanup never depends on a broad signal and the loop is
    never unbounded.
    """
    release_file = os.path.join(root, f"{label}.release")
    tool = os.path.join(root, f"{label}.sh")
    write_executable(
        tool,
        "#!/bin/sh\n"
        f"touch {os.path.join(root, label + '.held')}\n"
        f"limit=0\n"
        f"while [ ! -e {release_file} ]; do\n"
        "  sleep 0.05\n"
        "  limit=$((limit + 1))\n"
        f"  if [ $limit -gt {int(seconds / 0.05)} ]; then exit 0; fi\n"
        "done\n",
    )
    process = start(
        [SUBJECT, "exec", "--", tool],
        cwd=checkout,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )
    deadline = time.monotonic() + timeout(20)
    held = os.path.join(root, f"{label}.held")
    while not os.path.exists(held):
        if process.poll() is not None:
            fail(f"the {label} holder exited before taking its slot: {process.stderr.read()[-800:]}")
        if time.monotonic() > deadline:
            release(process)
            fail(f"the {label} holder never took its slot")
        time.sleep(0.02)
    return (process, release_file)


def event_pipe() -> tuple[int, int]:
    """A caller-created already-nonblocking pipe; both ends close-on-exec."""
    read_fd, write_fd = os.pipe()
    os.set_blocking(read_fd, False)
    os.set_blocking(write_fd, False)
    for fd in (read_fd, write_fd):
        os.set_inheritable(fd, False)
    return (read_fd, write_fd)


def drain(fd: int, *, deadline_s: float = 2.0) -> bytes:
    """Read what is ready, bounded; never depends on EOF for custody."""
    collected = b""
    deadline = time.monotonic() + deadline_s
    while time.monotonic() < deadline:
        ready, _, _ = select.select([fd], [], [], 0.05)
        if not ready:
            break
        try:
            chunk = os.read(fd, 65536)
        except BlockingIOError:
            continue
        if not chunk:
            break
        collected += chunk
    return collected


def read_until_record(fd: int, *, deadline_s: float = 10.0) -> bytes:
    """Deterministic first-record protocol: wait for readiness, then read.

    A nonblocking pipe may have no data yet; this waits on select until one
    complete line has arrived or the bound expires, instead of assuming a
    first read succeeds.
    """
    collected = b""
    deadline = time.monotonic() + deadline_s
    while time.monotonic() < deadline:
        ready, _, _ = select.select([fd], [], [], 0.05)
        if ready:
            try:
                chunk = os.read(fd, 65536)
            except BlockingIOError:
                continue
            if not chunk:
                break
            collected += chunk
            if b"\n" in collected:
                return collected
    return collected


def finish_events(raw: bytes) -> list[dict]:
    """Parse the typed stream; the completed process is never the oracle."""
    records = parse_records(raw.decode(errors="replace"))
    expect(records is not None, f"the event stream is not bounded newline JSON: {raw[:400]!r}")
    return records


def terminal(records: list[dict]) -> dict:
    terminals = [entry for entry in records if entry.get("event") == "finished"]
    expect(len(terminals) == 1, f"expected one terminal record, got {terminals}")
    return terminals[0]


def started_records(records: list[dict]) -> list[dict]:
    return [entry for entry in records if entry.get("event") == "admitted_child_started"]


# --------------------------------------------------------------------------
# Cases.


def case_fast_start_axes(root: str, subject: str) -> None:
    case("fast-start-axes")
    env = fixture_env(root, "fast")
    crate = create_crate(root, "fast")
    tool = os.path.join(root, "fast.sh")
    write_executable(tool, "#!/bin/sh\nexit 0\n")
    for label, budgets, expectations in (
        ("both", ["--admission-budget", "30s", "--execution-budget", "30s"], {"admission": 30000, "execution": 30000}),
        ("execution-only", ["--execution-budget", "30s"], {"admission": None, "execution": 30000}),
        ("admission-only", ["--admission-budget", "30s"], {"admission": 30000, "execution": None}),
    ):
        read_fd, write_fd = event_pipe()
        try:
            answer = run(
                [subject, "exec", *budgets, "--budget-events", str(write_fd), "--", tool],
                cwd=crate,
                env=env,
                phase_seconds=60,
                pass_fds=(write_fd,),
            )
            raw = read_until_record(read_fd)
        finally:
            os.close(write_fd)
            os.close(read_fd)
        expect(answer.returncode == 0, f"{label}: exit {answer.returncode}: {answer.stderr[-800:]}")
        records = finish_events(raw)
        ok, detail = sequence_oracle(records)
        expect(ok, f"{label}: {detail}")
        started = started_records(records)
        expect(len(started) == 1, f"{label}: expected one started record, got {started}")
        expect(started[0]["admission_ms"] == expectations["admission"], f"{label}: admission axis {started[0]}")
        expect(started[0]["execution_ms"] == expectations["execution"], f"{label}: execution axis {started[0]}")
        expect(started[0]["spawn_late"] is False, f"{label}: a fast start was reported late")
        if expectations["execution"] is None:
            expect(started[0]["execution_started_ms"] is None, f"{label}: claimed an execution clock")
        recorded = terminal(records)
        expect(recorded["outcome"] == "child_exit", f"{label}: outcome {recorded['outcome']}")
        expect(recorded["exit_code"] == 0, f"{label}: terminal exit {recorded['exit_code']}")
        expect("budget" not in answer.stderr.lower(), f"{label}: printed a budget line")
    result("fast-start-axes", True, "both/execution-only/admission-only axes, nullable fields and sequence")


def case_spawn_failure(root: str, subject: str) -> None:
    case("spawn-failure-event")
    env = fixture_env(root, "spawn-failure")
    crate = create_crate(root, "spawn-failure")
    read_fd, write_fd = event_pipe()
    try:
        answer = run(
            [subject, "exec", "--execution-budget", "5s", "--budget-events", str(write_fd), "--", os.path.join(root, "absent")],
            cwd=crate,
            env=env,
            phase_seconds=30,
            pass_fds=(write_fd,),
        )
        raw = drain(read_fd)
    finally:
        os.close(write_fd)
        os.close(read_fd)
    expect(answer.returncode == 127, f"a missing program exited {answer.returncode}, wanted 127")
    records = finish_events(raw)
    expect(not started_records(records), "a spawn failure emitted a started record")
    recorded = terminal(records)
    expect(recorded["outcome"] == "spawn_failure", f"outcome {recorded['outcome']}")
    expect(recorded["child_exit_code"] is None, "a spawn failure reported a child exit code")
    result("spawn-failure-event", True, "exit 127, no started record, finished(spawn_failure)")


def case_admission_expiry(root: str, subject: str) -> None:
    case("admission-expiry-no-start")
    env = fixture_env(root, "admission")
    crate = create_crate(root, "admission")
    env["OXIDE_MAX_ACTIVE"] = "1"
    marker = os.path.join(root, "admission.marker")
    tool = os.path.join(root, "admission.sh")
    write_executable(tool, f"#!/bin/sh\ntouch {marker}\nexit 0\n")
    holder, _release_file = hold_slot(root, env, crate, "admission", 30)
    tickets_before = ticket_count(env)
    read_fd, write_fd = event_pipe()
    try:
        answer = run(
            [subject, "exec", "--admission-budget", "1s", "--budget-events", str(write_fd), "--", tool],
            cwd=crate,
            env=env,
            phase_seconds=30,
            pass_fds=(write_fd,),
        )
        raw = drain(read_fd)
    finally:
        os.close(write_fd)
        os.close(read_fd)
        close_holder(holder, _release_file)
    expect(answer.returncode == ADMISSION_EXIT, f"admission expiry exited {answer.returncode}")
    expect(not os.path.exists(marker), "an admission-expired command started its child")
    expect("admission budget" in answer.stderr, f"the stop did not name the admission budget: {answer.stderr[-600:]}")
    records = finish_events(raw)
    expect(not started_records(records), "an admission-expired command emitted a started record")
    expect(terminal(records)["outcome"] == "admission_deadline", "the terminal outcome is not admission_deadline")
    expect(
        ticket_count(env) == tickets_before,
        "the contender's wait ticket survived its admission expiry",
    )
    status = run([subject, "status"], cwd=crate, env=env, phase_seconds=30)
    expect(status.returncode == 0, f"status exited {status.returncode}")
    result("admission-expiry-no-start", True, "124, no marker, no started record, ticket count back to baseline")


def case_contention(root: str, subject: str) -> None:
    case("contention-longer-than-execution")
    env = fixture_env(root, "contention")
    crate = create_crate(root, "contention")
    env["OXIDE_MAX_ACTIVE"] = "1"
    marker = os.path.join(root, "contention.marker")
    tool = os.path.join(root, "contention.sh")
    write_executable(tool, f"#!/bin/sh\ntouch {marker}\nsleep 0.2\n")
    holder, release_file = hold_slot(root, env, crate, "contention", 30)
    released_at = time.monotonic()
    process = start(
        [subject, "exec", "--admission-budget", "60s", "--execution-budget", "2s", "--", tool],
        cwd=crate,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        deadline = time.monotonic() + timeout(10)
        while time.monotonic() < deadline:
            if os.path.exists(marker):
                fail("the contender started while the only slot was held")
            time.sleep(0.02)
        waited = time.monotonic() - released_at
        write_private(release_file, "go\n")
        started = time.monotonic()
        stdout, stderr = process.communicate(timeout=timeout(60))
        elapsed = time.monotonic() - started
    except subprocess.TimeoutExpired:
        release(process)
        fail("the contender did not finish inside its clipped timeout")
    finally:
        close_holder(holder, release_file)
    expect(process.returncode == 0, f"the contender exited {process.returncode}: {stderr[-800:]}")
    expect(waited >= 2.5, f"the contender only waited {waited:.1f}s, not longer than its 2s execution window")
    expect(elapsed <= 6.0, f"the contender took {elapsed:.1f}s after release; execution elapsed must fit its own window")
    expect("admission budget" not in stderr and "execution budget" not in stderr, f"a budget stop appeared: {stderr[-600:]}")
    result("contention-longer-than-execution", True, f"waited {waited:.1f}s (over 2s) then succeeded in {elapsed:.1f}s")


def case_execution_deadline(root: str, subject: str) -> None:
    case("execution-deadline")
    env = fixture_env(root, "deadline")
    crate = create_crate(root, "deadline")
    tool = os.path.join(root, "deadline.sh")
    write_executable(tool, "#!/bin/sh\nexec sleep 60\n")
    read_fd, write_fd = event_pipe()
    try:
        answer = run(
            [subject, "exec", "--execution-budget", "1s", "--budget-events", str(write_fd), "--", tool],
            cwd=crate,
            env=env,
            phase_seconds=40,
            pass_fds=(write_fd,),
        )
        raw = drain(read_fd)
    finally:
        os.close(write_fd)
        os.close(read_fd)
    expect(answer.returncode == EXECUTION_EXIT, f"the deadline exited {answer.returncode}")
    expect("execution budget" in answer.stderr, f"the stop did not name the execution budget: {answer.stderr[-600:]}")
    expect(answer.elapsed <= 1 + 15, f"the stop took {answer.elapsed:.1f}s past the finite closeout bound")
    records = finish_events(raw)
    recorded = terminal(records)
    expect(recorded["outcome"] == "execution_deadline", f"outcome {recorded['outcome']}")
    expect(recorded["drain"] == "complete", f"a settled drain was reported {recorded['drain']}")
    follow_tool = os.path.join(root, "deadline-ok.sh")
    write_executable(follow_tool, "#!/bin/sh\nexit 0\n")
    follow = run([subject, "exec", "--", follow_tool], cwd=crate, env=env, phase_seconds=30)
    expect(follow.returncode == 0, f"the slot was not reusable after the deadline stop: {follow.returncode}")
    result("execution-deadline", True, "125, execution_deadline, settled drain, slot reusable")


def case_precedence(root: str, subject: str) -> None:
    env = fixture_env(root, "precedence")
    crate = create_crate(root, "precedence")
    case("precedence-child-124")
    self124 = os.path.join(root, "self124.sh")
    write_executable(self124, "#!/bin/sh\nexit 124\n")
    read_fd, write_fd = event_pipe()
    try:
        answer = run(
            [subject, "exec", "--execution-budget", "60s", "--budget-events", str(write_fd), "--", self124],
            cwd=crate,
            env=env,
            phase_seconds=30,
            pass_fds=(write_fd,),
        )
        raw = drain(read_fd)
    finally:
        os.close(write_fd)
        os.close(read_fd)
    expect(answer.returncode == 124, f"a genuine child 124 exited {answer.returncode}")
    expect("budget" not in answer.stderr.lower(), "a genuine child 124 printed a budget line")
    recorded = terminal(finish_events(raw))
    expect(recorded["outcome"] == "child_exit", f"a genuine 124 was typed {recorded['outcome']}")
    expect(recorded["stop_origin"] is None, f"a genuine 124 latched {recorded['stop_origin']}")
    result("precedence-child-124", True, "child_exit 124 without a budget line or latched origin")

    case("precedence-child-125")
    self125 = os.path.join(root, "self125.sh")
    write_executable(self125, "#!/bin/sh\nexit 125\n")
    result_run = run([subject, "exec", "--execution-budget", "60s", "--", self125], cwd=crate, env=env, phase_seconds=30)
    expect(result_run.returncode == 125, f"a genuine child 125 exited {result_run.returncode}")
    expect("budget" not in result_run.stderr.lower(), "a genuine child 125 printed a budget line")
    result("precedence-child-125", True, "child_exit 125 without a budget line")

    case("precedence-caller-signal")
    sleeper = os.path.join(root, "signal.sh")
    write_executable(sleeper, "#!/bin/sh\nexec sleep 60\n")
    process = start(
        [subject, "exec", "--execution-budget", "30s", "--", sleeper],
        cwd=crate,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        time.sleep(0.4)
        process.send_signal(signal.SIGTERM)
        _stdout, stderr = process.communicate(timeout=timeout(30))
    except subprocess.TimeoutExpired:
        release(process)
        fail("the caller-signal case did not finish inside its clipped timeout")
    expect(process.returncode == 143, f"a caller SIGTERM ended {process.returncode}, wanted 143")
    expect("execution budget" not in stderr, f"a caller signal printed a budget line: {stderr[-400:]}")
    result("precedence-caller-signal", True, "143, no deadline origin")

    case("precedence-term-trap")
    trapped = os.path.join(root, "trap.sh")
    trap_marker = os.path.join(root, "trap.marker")
    write_executable(
        trapped,
        "#!/bin/sh\n"
        f"trap 'touch {trap_marker}; exit 0' TERM\n"
        "while :; do sleep 0.05; done\n",
    )
    result_run = run(
        [subject, "exec", "--execution-budget", "1s", "--", trapped],
        cwd=crate,
        env=env,
        phase_seconds=40,
    )
    expect(result_run.returncode == EXECUTION_EXIT, f"the TERM-trapping child exited {result_run.returncode}")
    expect(os.path.exists(trap_marker), "the escalation never delivered TERM to the trapping child")
    result("precedence-term-trap", True, "TERM delivered to the trap, controlled stop still 125")


def case_cumulative_run(root: str, subject: str) -> None:
    case("cumulative-run-window")
    env = fixture_env(root, "cumulative")
    crate = private_dir(os.path.join(root, "checkouts", "cumulative"))
    write_private(
        os.path.join(crate, "Cargo.toml"),
        '[package]\nname = "oxide-budget-cumulative"\nversion = "0.1.0"\nedition = "2021"\nbuild = "build.rs"\n',
    )
    program_started_marker = os.path.join(root, "program.started")
    # The build consumes about 3 s of the single 5 s execution window, so the
    # launched program can only run for the remainder. A reset window would
    # give the program a fresh 5 s. The oracle is the program's own runtime,
    # not the command's total wall time: cooperative closeout latency is not
    # asserted here.
    write_private(
        os.path.join(crate, "build.rs"),
        "fn main() { std::thread::sleep(std::time::Duration::from_secs(3)); }\n",
    )
    write_private(
        os.path.join(crate, "src", "main.rs"),
        "fn main() {\n"
        f'    std::fs::write("{program_started_marker}", b"started").unwrap();\n'
        "    std::thread::sleep(std::time::Duration::from_secs(60));\n"
        "}\n",
    )
    read_fd, write_fd = event_pipe()
    stderr_path = os.path.join(root, "cumulative.stderr")
    started_at = time.monotonic()
    program_seen = None
    with open(stderr_path, "w", encoding="utf-8") as stderr_file:
        process = start(
            [
                subject,
                "run",
                "-j",
                str(FIXTURE_JOBS),
                "--execution-budget",
                "5s",
                "--budget-events",
                str(write_fd),
            ],
            cwd=crate,
            env=env,
            pass_fds=(write_fd,),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=stderr_file,
        )
        while True:
            if program_seen is None and os.path.exists(program_started_marker):
                program_seen = time.monotonic()
            if process.poll() is not None:
                break
            if time.monotonic() - started_at > timeout(45):
                fault = "the cumulative run outlived its clipped timeout"
                process.kill()
                fail(fault)
            time.sleep(0.02)
        exit_at = time.monotonic()
    raw = drain(read_fd)
    os.close(write_fd)
    os.close(read_fd)
    stderr = read_text(stderr_path)
    expect(process.returncode == EXECUTION_EXIT, f"the cumulative run exited {process.returncode}: {stderr[-600:]}")
    expect(program_seen is not None, "the build never handed off to the launched program")
    program_ran = exit_at - program_seen
    expect(
        0.5 <= program_ran < 3.5,
        f"the program ran {program_ran:.1f}s after a ~3s build under one 5s window: the window was reset",
    )
    records = finish_events(raw)
    phases = [entry.get("phase") for entry in started_records(records)]
    expect("primary_cargo" in phases and "run_program" in phases, f"phases {phases}")
    expect(terminal(records)["outcome"] == "execution_deadline", "the cumulative window did not end as execution_deadline")
    expect(
        terminal(records)["stop_origin"] == "execution",
        "the cumulative window did not report an execution-origin stop",
    )
    result(
        "cumulative-run-window",
        True,
        f"one 5s window: ~3s build then {program_ran:.1f}s program (a reset window would run ~5s)",
    )


def case_nested(root: str, subject: str) -> None:
    case("nested-outer-window")
    env = fixture_env(root, "nested")
    env["OXIDE_MAX_ACTIVE"] = "1"
    inner_crate = create_crate(root, "nested-inner")
    inner_log = os.path.join(root, "inner.log")
    # The outer command starts first and takes the only slot. Its tool starts an
    # inner oxide call, which waits for the slot the outer already holds and
    # ends on its own 1 s admission budget (a bounded lifetime of its own). The
    # tool then keeps running under its own cap, so the outer 2 s window — not a
    # natural tool exit — stops it. Outer group closure is not claimed to prove
    # anything about the inner group.
    tool = os.path.join(root, "nested-tool.sh")
    write_executable(
        tool,
        "#!/bin/sh\n"
        f"cd {inner_crate} && {subject} check -j {FIXTURE_JOBS} --admission-budget 1s > {inner_log} 2>&1\n"
        "limit=0\n"
        "while [ $limit -lt 600 ]; do sleep 0.05; limit=$((limit + 1)); done\n"
        "exit 0\n",
    )
    read_fd, write_fd = event_pipe()
    try:
        answer = run(
            [subject, "exec", "--execution-budget", "2s", "--budget-events", str(write_fd), "--", tool],
            cwd=inner_crate,
            env=env,
            phase_seconds=45,
            pass_fds=(write_fd,),
        )
        raw = drain(read_fd)
    finally:
        os.close(write_fd)
        os.close(read_fd)
    expect(answer.returncode == EXECUTION_EXIT, f"the outer command exited {answer.returncode}")
    expect("execution budget" in answer.stderr, f"the outer stop did not name the execution budget: {answer.stderr[-600:]}")
    records = finish_events(raw)
    phases = [entry.get("phase") for entry in started_records(records)]
    expect("exec_tool" in phases, f"the outer tool start was not reported: {phases}")
    recorded = terminal(records)
    expect(recorded["outcome"] == "execution_deadline", "the outer window did not report execution_deadline")
    expect(recorded["stop_origin"] == "execution", "the outer stop did not carry an execution origin")
    inner = read_text(inner_log)
    expect("waiting" in inner, f"the inner call never waited for the held slot: {inner[-600:]}")
    expect("admission budget" in inner or "124" in inner, f"the inner call did not end on its own budget: {inner[-600:]}")
    expect(not os.path.isdir(os.path.join(inner_crate, "target")), "the inner call started a build")
    result(
        "nested-outer-window",
        True,
        "outer started and took the slot; inner waited, ended on its own 1s budget with no build; the outer window stopped the running tool",
    )


def case_events(root: str, subject: str) -> None:
    case("events-matrix")
    env = fixture_env(root, "events")
    crate = create_crate(root, "events")
    tool = os.path.join(root, "events.sh")
    write_executable(tool, "#!/bin/sh\nexit 0\n")
    regular = os.path.join(root, "regular-file")
    write_private(regular, "not a pipe")
    regular_fd = os.open(regular, os.O_RDWR)
    read_fd, write_fd = os.pipe()
    blocking_read, blocking_write = os.pipe()
    for described, fd, expected in (
        ("regular file", regular_fd, descriptor_refusal("regular file")),
        ("read end", read_fd, descriptor_refusal("read end")),
        ("blocking pipe", blocking_write, descriptor_refusal("blocking pipe")),
    ):
        refused = run(
            [subject, "exec", "--execution-budget", "5s", "--budget-events", str(fd), "--", tool],
            cwd=crate,
            env=env,
            phase_seconds=20,
            pass_fds=(fd,),
        )
        expect(refused.returncode != 0, f"a {described} was accepted as an event channel")
        expect(expected in refused.stderr, f"the {described} refusal did not say {expected!r}: {refused.stderr[-400:]}")
    os.close(regular_fd)
    os.close(read_fd)
    os.close(write_fd)
    os.close(blocking_read)
    os.close(blocking_write)
    read_fd, write_fd = event_pipe()
    filler = b"x" * 4096
    while True:
        try:
            os.write(write_fd, filler)
        except BlockingIOError:
            break
    full = run(
        [subject, "exec", "--execution-budget", "5s", "--budget-events", str(write_fd), "--", tool],
        cwd=crate,
        env=env,
        phase_seconds=20,
        pass_fds=(write_fd,),
    )
    os.close(write_fd)
    drained = drain(read_fd, deadline_s=1.0)
    os.close(read_fd)
    expect(full.returncode == 0, f"a full event pipe changed the outcome: {full.returncode}")
    expect(b'"event"' not in drained, "a record was written after the stream was lost")
    result("events-matrix", True, "type/access/flag refusals, full pipe writes nothing")


def case_broken_reader(root: str, subject: str) -> None:
    case("events-broken-reader")
    env = fixture_env(root, "broken")
    crate = create_crate(root, "broken")
    tool = os.path.join(root, "broken.sh")
    write_executable(tool, "#!/bin/sh\nexit 0\n")
    read_fd, write_fd = event_pipe()
    # Close the read end before the candidate starts: every write fails with
    # EPIPE from the first record, which is the deterministic broken-reader
    # condition (no readiness race).
    os.close(read_fd)
    try:
        result_run = run(
            [subject, "exec", "--execution-budget", "5s", "--budget-events", str(write_fd), "--", tool],
            cwd=crate,
            env=env,
            phase_seconds=20,
            pass_fds=(write_fd,),
        )
    finally:
        os.close(write_fd)
    expect(result_run.returncode == 0, f"a broken reader changed the outcome: {result_run.returncode}")
    expect(result_run.stderr.count("event channel became unusable") <= 1, "the loss note repeated")
    result("events-broken-reader", True, "EPIPE ignored, parent survives, one loss note at most")


def case_unknown_closure(root: str, subject: str) -> None:
    case("unknown-closure-held")
    env = fixture_env(root, "closure")
    crate = create_crate(root, "closure")
    env["OXIDE_TEST_CLOSURE_STUCK"] = "1"
    tool = os.path.join(root, "closure.sh")
    write_executable(tool, "#!/bin/sh\nexec sleep 60\n")
    read_fd, write_fd = event_pipe()
    try:
        result_run = run(
            [subject, "exec", "--execution-budget", "1s", "--budget-events", str(write_fd), "--", tool],
            cwd=crate,
            env=env,
            phase_seconds=40,
            pass_fds=(write_fd,),
        )
        raw = drain(read_fd)
    finally:
        os.close(write_fd)
        os.close(read_fd)
    expect(result_run.returncode == 1, f"the injected unknown closure exited {result_run.returncode}")
    records = finish_events(raw)
    recorded = terminal(records)
    expect(recorded["outcome"] == "unconfirmed_closure", f"outcome {recorded['outcome']}")
    expect(recorded["drain"] != "complete", "unknown closure claimed a complete drain")
    expect(recorded["lease"] == "held", f"unknown closure reported lease {recorded['lease']}")
    status = run([subject, "status"], cwd=crate, env=env, phase_seconds=20)
    text = status.stdout + status.stderr
    expect("held" in text or "interrupted" in text, f"status did not show held custody: {text[-600:]}")
    result("unknown-closure-held", True, "exit 1, unconfirmed_closure, custody held and visible")


def case_preparation_stop(root: str, subject: str) -> None:
    case("preparation-stop-124")
    env = fixture_env(root, "preparation")
    crate = create_crate(root, "preparation")
    real_cargo = shutil.which("cargo")
    expect(bool(real_cargo), "the proof needs the invoking cargo on PATH")
    fake = private_dir(os.path.join(root, "fakebin"))
    fake_script = (
        "#!/bin/sh\n"
        'case " $* " in *locate-project*) sleep 5 ;; esac\n'
        f'exec {real_cargo} "$@"\n'
    )
    write_executable(os.path.join(fake, "cargo"), fake_script)
    env["PATH"] = f"{fake}:{env['PATH']}"
    marker = os.path.join(root, "prep.marker")
    tool = os.path.join(root, "prep.sh")
    write_executable(tool, f"#!/bin/sh\ntouch {marker}\n")
    result_run = run(
        [subject, "exec", "--admission-budget", "1ms", "--", tool],
        cwd=create_crate(root, "preparation"),
        env=env,
        phase_seconds=30,
    )
    expect(result_run.returncode == ADMISSION_EXIT, f"a stopped preparation child exited {result_run.returncode}")
    expect(not os.path.exists(marker), "a stopped preparation child started the primary program")
    result("preparation-stop-124", True, "124, no primary child")


def case_late_start(root: str, subject: str) -> None:
    case("late-start")
    env = fixture_env(root, "late")
    crate = create_crate(root, "late")
    env["OXIDE_TEST_SPAWN_DELAY_MS"] = "3000"
    tool = os.path.join(root, "late.sh")
    write_executable(tool, "#!/bin/sh\nexec sleep 60\n")
    read_fd, write_fd = event_pipe()
    try:
        result_run = run(
            [
                subject,
                "exec",
                "--admission-budget",
                "2s",
                "--execution-budget",
                "60s",
                "--budget-events",
                str(write_fd),
                "--",
                tool,
            ],
            cwd=crate,
            env=env,
            phase_seconds=45,
            pass_fds=(write_fd,),
        )
        raw = drain(read_fd)
    finally:
        os.close(write_fd)
        os.close(read_fd)
    expect(result_run.returncode == ADMISSION_EXIT, f"the late start exited {result_run.returncode}")
    records = finish_events(raw)
    started = started_records(records)
    expect(len(started) == 1, f"the late start emitted {len(started)} started records")
    expect(started[0]["spawn_late"] is True, f"the late start was not reported: {started[0]}")
    expect(started[0]["admission_expired_before_start"] is True, f"the expired window was not reported: {started[0]}")
    expect(isinstance(started[0]["admission_overshoot_ms"], int), f"no overshoot: {started[0]}")
    follow_tool = os.path.join(root, "late-ok.sh")
    write_executable(follow_tool, "#!/bin/sh\nexit 0\n")
    follow = run([subject, "exec", "--", follow_tool], cwd=crate, env=env, phase_seconds=30)
    expect(follow.returncode == 0, f"the slot was not released after a late start: {follow.returncode}")
    result("late-start", True, "124, spawn_late with overshoot, slot released")


def ticket_count(env: dict) -> int:
    """The recorded wait-ticket count, read from the pool's control state."""
    pool = env["OXIDE_POOL"]
    for root, _dirs, files in os.walk(pool):
        if "state.json" in files:
            try:
                document = json.loads(read_text(os.path.join(root, "state.json")))
            except json.JSONDecodeError:
                return -1
            tickets = document.get("tickets")
            return len(tickets) if isinstance(tickets, list) else 0
    return 0


def control_state(env: dict) -> tuple[str | None, list[str]]:
    pool = env["OXIDE_POOL"]
    for root, _dirs, files in os.walk(pool):
        if "state.json" in files:
            path = os.path.join(root, "state.json")
            try:
                document = json.loads(read_text(path))
            except json.JSONDecodeError:
                return (root, [])
            if isinstance(document, dict):
                return (root, sorted(document))
    return (None, [])


def case_no_mutation(root: str, subject: str) -> None:
    case("no-mutation")
    env = fixture_env(root, "mutation")
    crate = create_crate(root, "mutation")
    tool = os.path.join(root, "mutation.sh")
    write_executable(tool, "#!/bin/sh\nexit 0\n")
    home = env["HOME"]
    config = env["OXIDE_CONFIG"]
    regular = os.path.join(root, "mutation-regular")
    write_private(regular, "not a pipe")
    before_home = sorted(os.listdir(home))
    before_config = read_text(config)
    refused_fd = os.open(regular, os.O_RDWR)
    try:
        refused = run(
            [subject, "exec", "--execution-budget", "5s", "--budget-events", str(refused_fd), "--", tool],
            cwd=crate,
            env=env,
            phase_seconds=20,
            pass_fds=(refused_fd,),
        )
    finally:
        os.close(refused_fd)
    expect(refused.returncode != 0, "a refused input was accepted")
    expect(sorted(os.listdir(home)) == before_home, "a refused input changed the private HOME")
    expect(read_text(config) == before_config, "a refused input changed the private config")
    expect(not os.path.exists(env["OXIDE_POOL"]), "a refused input created a pool")

    run([subject, "exec", "--", tool], cwd=crate, env=env, phase_seconds=30)
    control, before_keys = control_state(env)
    expect(control is not None, "no recorded control state to compare")
    run(
        [subject, "exec", "--admission-budget", "30s", "--execution-budget", "30s", "--", tool],
        cwd=crate,
        env=env,
        phase_seconds=30,
    )
    _, after_keys = control_state(env)
    expect(before_keys == after_keys, f"a budgeted command changed recorded fields: {set(after_keys) ^ set(before_keys)}")
    for name in os.listdir(control):
        expect("budget" not in name, f"a budgeted command created {name} in the control directory")
    settings = os.path.join(home, ".config", "oxide")
    expect(not (os.path.isdir(settings) and os.listdir(settings)), "a budgeted command wrote settings")
    status = run([subject, "status"], cwd=crate, env=env, phase_seconds=20)
    expect(status.returncode == 0, f"status exited {status.returncode}")
    for word in ("admission budget", "execution budget", "budget-events"):
        expect(word not in status.stdout + status.stderr, f"status gained a budget-only line: {word}")
    result("no-mutation", True, "refused inputs inert; no settings, new state fields or budget lines")


def case_unbudgeted(root: str, subject: str) -> None:
    case("unbudgeted")
    env = fixture_env(root, "plain")
    crate = create_crate(root, "plain")
    tool = os.path.join(root, "plain.sh")
    write_executable(tool, "#!/bin/sh\nexit 3\n")
    result_run = run([subject, "exec", "--", tool], cwd=crate, env=env, phase_seconds=20)
    expect(result_run.returncode == 3, f"an unbudgeted exec exited {result_run.returncode}")
    expect("budget" not in result_run.stderr.lower(), "an unbudgeted command printed a budget line")
    result("unbudgeted", True, "exit 3 preserved, no budget output")


# --------------------------------------------------------------------------
# Staging and the counted run.


def stage_subject(scratch: str) -> str:
    """Stage this clean HEAD's debug oxide: plain Cargo, offline, in scratch."""
    stage = private_dir(os.path.join(scratch, "stage"))
    log = os.path.join(stage, "stage.log")
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("CARGO_TARGET_", "CARGO_BUILD_", "SCCACHE_", "OXIDE_"))
        and key
        not in (
            "CARGO",
            "RUSTC",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_ENCODED_RUSTFLAGS",
            "OXIDE_CONFIG",
            "OXIDE_POOL",
        )
    }
    env.update(
        {
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(stage, "target"),
            "CARGO_BUILD_BUILD_DIR": os.path.join(stage, "build"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_PROFILE_DEV_DEBUG_ASSERTIONS": "true",
            "CARGO_PROFILE_DEV_OPT_LEVEL": "s",
            "CARGO_PROFILE_DEV_CODEGEN_UNITS": "1",
            "CARGO_INCREMENTAL": "0",
            "RUSTFLAGS": "-C strip=symbols",
            "CARGO_NET_OFFLINE": "true",
        }
    )
    with open(log, "w", encoding="utf-8") as handle:
        build = subprocess.run(
            ["cargo", "build", "-j", str(STAGE_JOBS), "--locked", "--offline", "--bin", "oxide"],
            cwd=REPO,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            timeout=timeout(PHASE_ALLOCATION_S["stage"]),
        )
    if build.returncode:
        fail(f"staging exit {build.returncode}: {read_text(log)[-2000:]}")
    built = os.path.join(stage, "target", "debug", "oxide")
    expect(os.path.isfile(built), f"staging produced no binary at {built}")
    subject = os.path.join(scratch, "subject-oxide")
    shutil.copy2(built, subject)
    os.chmod(subject, 0o755)
    # The copied candidate must be byte-identical before the staging targets go.
    expect(
        sha256_file(subject) == sha256_file(built),
        "the copied candidate does not match the staged binary",
    )
    # Debug-control capability identity, checked statically: only a
    # debug_assertions build carries the closure hook name, so a release build
    # would be refused here instead of silently mis-running the proof.
    with open(subject, "rb") as handle:
        image = handle.read()
    expect(
        b"OXIDE_TEST_CLOSURE_STUCK" in image,
        "the staged candidate has no debug closure-control capability",
    )
    expect(
        b"OXIDE_TEST_SPAWN_DELAY_MS" in image,
        "the staged candidate has no debug spawn-delay capability",
    )
    say(
        "staged candidate capabilities: debug closure hook and spawn-delay hook present"
    )
    # Only the positively owned staging directories are removed.
    shutil.rmtree(os.path.join(stage, "target"), ignore_errors=True)
    shutil.rmtree(os.path.join(stage, "build"), ignore_errors=True)
    os.unlink(log)
    say(f"staged subject {subject} sha256 {sha256_file(subject)}")
    return subject


CAPABILITIES_PROBE = "__oxide-build-capabilities"


def probe_capabilities(subject: str, scratch: str) -> dict:
    """Effect-free capability handshake on the staged candidate.

    The candidate answers its built-in probe before any store work, so this runs
    before any private HOME/CARGO_HOME/pool fixture exists. A debug-only proof
    needs `test_controls: true`; a release candidate refuses here instead of
    silently mis-running the debug controls. Names found in an executable are
    not evidence: only this probe is.
    """
    home = private_dir(os.path.join(scratch, "capability-probe-home"))
    env = {
        "HOME": home,
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "TMPDIR": scratch,
        "TMP": scratch,
        "TEMP": scratch,
        "NO_COLOR": "1",
    }
    result = subprocess.run(
        [subject, CAPABILITIES_PROBE],
        cwd=scratch,
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=timeout(10),
    )
    expect(result.returncode == 0, f"the capability probe exited {result.returncode}: {result.stderr[-400:]}")
    try:
        document = json.loads(result.stdout)
    except json.JSONDecodeError:
        fail(f"the capability probe returned no JSON: {result.stdout[:300]!r}")
    expect(
        document.get("test_controls") is True,
        "the staged candidate does not honour the debug-only proof controls; a release build cannot run this proof",
    )
    return {
        "candidate_sha256": sha256_file(subject),
        "control_state_version": document.get("control_state_version"),
        "registration_version": document.get("registration_version"),
        "test_controls": True,
    }


def write_receipt(receipt: dict, scratch: str) -> str:
    """Write this helper's uniquely named, bounded receipt beside the guard's."""
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE")
    expect(bool(evidence), "OXIDE_PROOF_EVIDENCE is not set")
    evidence = os.path.abspath(evidence)
    expect(os.path.isdir(evidence), f"evidence directory {evidence} does not exist")
    parent = os.path.dirname(evidence)
    expect(
        evidence.startswith(scratch)
        or (
            os.path.basename(evidence) == "evidence"
            and os.path.basename(scratch) == "scratch"
            and os.path.dirname(scratch) == parent
        ),
        f"evidence directory {evidence} is not the supervisor's",
    )
    payload = json.dumps(receipt, indent=2, sort_keys=True).encode()
    expect(len(payload) <= MAX_RECEIPT_BYTES, f"the receipt is {len(payload)} bytes over its cap")
    stamp = f"{int(time.time() * 1000)}-{os.getpid()}"
    path = os.path.join(evidence, f"command-budgets-receipt.{stamp}.json")
    expect(not os.path.exists(path), f"receipt path {path} already exists")
    temporary = f"{path}.tmp"
    with open(temporary, "wb") as handle:
        handle.write(payload)
        handle.flush()
        os.fsync(handle.fileno())
    os.chmod(temporary, 0o600)
    os.replace(temporary, path)
    return path


def write_receipt_safely(document: dict, scratch: str) -> str | None:
    """Write the receipt without ever replacing the primary fault."""
    try:
        return write_receipt(document, scratch)
    except (Fault, OSError, ValueError) as error:
        say(f"receipt: the helper receipt could not be written ({error}); the primary fault is unchanged")
        return None


class Proof:
    """One counted run: staging, capability handshake, cases and receipt.

    The receipt is written for every outcome, including a staging, capability,
    assertion or receipt-write failure. Candidate identity is honestly unknown
    (null) when staging never produced one.
    """

    def __init__(self, scratch: str, runner_sha: str) -> None:
        self.scratch = scratch
        self.runner_sha = runner_sha
        self.subject: str | None = None
        self.digest: str | None = None
        self.capabilities: dict | None = None
        self.commit = ""
        self.tree = ""
        self.root: str | None = None

    def run(self) -> None:
        self.commit = git("rev-parse", "HEAD")
        self.tree = git("rev-parse", "HEAD^{tree}")
        self.subject = stage_subject(self.scratch)
        self.digest = sha256_file(self.subject)
        expect(len(self.digest) == 64, "the staged subject digest is malformed")
        # The effect-free capability handshake runs before any private HOME,
        # CARGO_HOME, config or pool fixture is created.
        self.capabilities = probe_capabilities(self.subject, self.scratch)
        self.root = private_dir(os.path.join(self.scratch, "fixture-budgets"))
        for entry in (
            case_fast_start_axes,
            case_spawn_failure,
            case_admission_expiry,
            case_contention,
            case_execution_deadline,
            case_precedence,
            case_cumulative_run,
            case_nested,
            case_events,
            case_broken_reader,
            case_unknown_closure,
            case_preparation_stop,
            case_late_start,
            case_no_mutation,
            case_unbudgeted,
        ):
            entry(self.root, self.subject)
            work_left()
        say(f"cases run: {', '.join(CASES)}")

    def document(self, fault: str | None) -> dict:
        planned = [entry["name"] for entry in plan_document()["cases"]]
        failed = [entry for entry in RESULTS if entry["result"] != "pass"]
        if fault is not None:
            failed = failed + [{"case": "primary", "result": "fail", "detail": fault}]
        return {
            "task": "oxide#140",
            "kind": "command-budgets helper receipt",
            "selector": SELECTOR,
            "identity": {
                "source_commit": self.commit or None,
                "source_tree": self.tree or None,
                "candidate_sha256": self.digest,
                "script_sha256": sha256_file(SCRIPT),
                "supervisor_sha256": self.runner_sha,
                "toolchain": BUILDER,
                "capabilities": self.capabilities,
            },
            "command": COMMAND,
            "reached": list(CASES),
            "completed": [entry["case"] for entry in RESULTS if entry["result"] == "pass"],
            "failed": failed,
            "unexecuted": [name for name in planned if name not in CASES],
            "cases": RESULTS,
            "primary_fault": fault,
            "uncertain_custody": list(UNCERTAIN),
            "allocation": phase_allocation(),
            "limits": {
                "wall_limit_s": WALL_LIMIT_S,
                "scratch_budget_mib": SCRATCH_BUDGET_MIB,
                "shutdown_margin_s": SHUTDOWN_MARGIN_S,
                "timing": "unmeasured",
            },
            "children": {
                "tracked": len(OWNED),
                "release": "release files, then terminate() and clipped kill() on exact owned PIDs",
                "sessions": "no new session or process group; no census or broad signals",
                "closure": "direct leader reaping is not descendant closure; the guard decides closure",
            },
            "authority": "This helper receipt records only the script's own case oracles, its own "
            "primary fault and its own custody uncertainty. The guard's own receipt, the actual "
            "caller exit and the session closure remain separately authoritative.",
            "live_pool": False,
            "mounted_volume": False,
            "result": "pass" if fault is None else "fail",
        }


SUBJECT = ""
SUBJECT = ""


# --------------------------------------------------------------------------
# Modes.

def self_check() -> None:
    """Pure checks of the argument boundary, plan, arithmetic and oracles."""
    expect(parse_arguments([]) == ("proof", None), "the bare invocation must run the proof")
    expect(parse_arguments(["--plan"]) == ("plan", None), "--plan must be effect-free")
    expect(parse_arguments(["--self-check"]) == ("self-check", None), "--self-check must parse")
    expect(parse_arguments(["--help"]) == ("help", None), "--help must parse")
    for refused in (["--plan", "--plan"], ["proof"], ["--bogus"], [""]):
        mode, error = parse_arguments(refused)
        expect(mode == "refuse" and bool(error), f"{refused!r} must be refused")
    plan = plan_document()
    expect(plan["limits"]["wall_limit_s"] == 300, "the wall limit changed")
    expect(plan["limits"]["scratch_budget_mib"] == 512, "the scratch budget changed")
    expect(plan["limits"]["shutdown_margin_s"] == 30, "the shutdown margin changed")
    expect(plan["limits"]["allocation_fits"] is True, "the phase allocation does not fit 300 s")
    expect(allocation_fits(), "the phase allocation must fit the wall limit")
    expect(len(plan["cases"]) == 18, "the plan case count changed")
    expect("unmeasured" in json.dumps(plan["limits"]), "the plan must not claim measured timing")
    expect(plan["children"]["sessions"].startswith("no new session"), "sessions must stay uncreated")
    expect(plan["provenance"]["private_home"].startswith("OXIDE_TEST_HOME_FROM_ENV=1"), "the private home control changed")
    expect(COMMAND[-1].endswith("scripts/test-command-budgets.py") and "--limit" in COMMAND and "--scratch-budget-mib" in COMMAND, "the fixed command changed")
    # Timeout clipping: a smaller phase uses the phase, a larger one the guard.
    expect(clip(10, 100) == 10.0, "clip must keep a smaller phase bound")
    expect(clip(500, 100) == 100.0, "clip must never exceed the guard's remaining time")
    # Sequence oracle.
    ok, _ = sequence_oracle(
        [
            {"seq": 1, "event": "admitted_child_started"},
            {"seq": 2, "event": "finished"},
        ]
    )
    expect(ok, "contiguous sequences must pass")
    broken, _ = sequence_oracle([{"seq": 1, "event": "admitted_child_started"}, {"seq": 3, "event": "finished"}])
    expect(not broken, "a sequence gap must fail")
    twice, _ = sequence_oracle([{"seq": 1, "event": "finished"}, {"seq": 2, "event": "finished"}])
    expect(not twice, "two terminal records must fail")
    expect(parse_records("not json\n") is None, "malformed records must be unknown")
    expect(parse_records('{"seq":1}\n{"seq":2}\n') is not None, "newline JSON must parse")
    # Descriptor oracles used by the matrix case.
    expect(descriptor_refusal("regular file") == "not a pipe", "regular-file oracle changed")
    expect(descriptor_refusal("read end") == "not writable", "read-end oracle changed")
    expect(descriptor_refusal("blocking pipe") == "not nonblocking", "blocking-pipe oracle changed")
    # Apparatus invariants added after the independent review.
    names = [entry["name"] for entry in plan["cases"]]
    expect("precedence-child-124" in names, "the genuine-124 control is missing")
    cumulative = [entry for entry in plan["cases"] if entry["name"] == "cumulative-run-window"][0]
    expect("reset" in cumulative["oracle"], "the cumulative oracle must falsify a clock reset")
    expect("closeout latency is not asserted" in cumulative["oracle"], "the cumulative oracle must not gate closeout latency")
    nested = [entry for entry in plan["cases"] if entry["name"] == "nested-outer-window"][0]
    expect("outer 2s window" in nested["oracle"], "the nested oracle must attribute the stop to the outer window")
    expect(remaining_quiet() >= 0.0, "the quiet remaining clock must not raise")
    expect(custody_bound() >= 0.05, "the custody bound must stay positive")
    import importlib.util

    spec = importlib.util.spec_from_file_location("budget_proof_shape", SCRIPT)
    shape = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(shape)
    clean = shape.Proof("/tmp/scratch", "s" * 64).document(None)
    expect(clean["primary_fault"] is None and clean["result"] == "pass", "a clean receipt changed")
    failed_receipt = shape.Proof("/tmp/scratch", "s" * 64).document("boom")
    expect(
        failed_receipt["primary_fault"] == "boom" and failed_receipt["result"] == "fail",
        "a failed receipt lost its fault",
    )
    # Receipt identity shape.
    identity = stage_identity("c" * 40, "t" * 40, "d" * 64, "s" * 64)
    expect(set(identity) == {"source_commit", "source_tree", "binary_sha256", "script_sha256", "supervisor_sha256", "toolchain"}, "the receipt identity changed")
    expect(sha256_file(SCRIPT) != "", "the script digest is unreadable")
    expect(len(EXPECTED_RUNNER_SHA256) == 64, "the supervisor pin is malformed")
    print("self-check: ok (pure argument, plan, allocation, sequence, descriptor and recipe checks; not evidence)")


def self_test() -> None:
    """Execute the corrected case bodies and lifecycle paths with faked seams.

    No child is spawned, no candidate is staged and no pool is created: `run`
    and `start` return scripted responses and a fake clock advances one second
    per read, while the real event-pipe helpers deliver the records. Each
    positive control must pass and each mutated variant must fail its own
    oracle, so a body that stopped asserting cannot pass this test.
    """
    import importlib.util
    import tempfile

    scratch = tempfile.mkdtemp(prefix="budget-selftest-", dir="/private/tmp")

    class FakeTime:
        """One second per monotonic read; real clock for everything else."""

        CLOCK_UPTIME_RAW = time.CLOCK_UPTIME_RAW

        def __init__(self):
            self.seconds = 0.0

        def monotonic(self):
            self.seconds += 1.0
            return self.seconds

        def perf_counter(self):
            return self.seconds

        def sleep(self, _seconds):
            return None

        def clock_gettime_ns(self, _clock):
            return int(self.seconds * 1e9)

    def completed(argv, code, stdout="", stderr="", elapsed=1.0):
        result = subprocess.CompletedProcess(argv, code, stdout, stderr)
        result.elapsed = elapsed
        return result

    def stream(entries):
        return b"".join(json.dumps(entry).encode() + b"\n" for entry in entries)

    def load():
        spec = importlib.util.spec_from_file_location("budget_proof_body", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        module.work_left = lambda: 30.0
        module.time = FakeTime()
        module.fixture_env = lambda root, label: {
            "OXIDE_POOL": os.path.join(root, f"pool-{label}"),
            "HOME": os.path.join(root, f"home-{label}"),
            "OXIDE_CONFIG": os.path.join(root, f"config-{label}.toml"),
        }
        module.create_crate = lambda root, label: module.private_dir(
            os.path.join(root, "checkouts", label)
        )
        module.hold_slot = lambda *args, **kwargs: (None, os.path.join(scratch, "unused"))
        module.close_holder = lambda process, release_file: None
        module.ticket_count = lambda env: 0
        module.control_state = lambda env: (os.path.join(scratch, "control"), ["version"])
        module.CASES.clear()
        module.RESULTS.clear()
        return module

    def flag(argv, name):
        return argv[argv.index(name) + 1] if name in argv else None

    def millis(text):
        return None if text is None else int(text.rstrip("s")) * 1000

    def make_run(module, *, late, late_start, nested_code):
        def fake_run(argv, *, cwd, env, phase_seconds, pass_fds=(), stdin=None):
            if "--budget-events" not in argv:
                # Follow-up controls take no event channel; they are plain
                # successes with no typed records.
                return completed(argv, 0, "", "")
            fd = int(flag(argv, "--budget-events"))
            program = argv[argv.index("--") + 1] if "--" in argv else ""
            if late_start:
                os.write(fd, stream([
                    {"seq": 1, "event": "admitted_child_started", "phase": "exec_tool",
                     "spawn_late": late, "admission_expired_before_start": late,
                     "admission_overshoot_ms": 1000 if late else None},
                    {"seq": 2, "event": "finished", "outcome": "admission_deadline",
                     "stop_origin": "admission" if late else None, "exit_code": 124},
                ]))
                return completed(argv, 124, "", "oxide: stopped by the admission budget (2000ms)\n")
            if program.endswith("nested-tool.sh"):
                root = os.path.dirname(os.path.dirname(cwd))
                with open(os.path.join(root, "inner.log"), "w", encoding="utf-8") as handle:
                    handle.write("oxide: waiting for a build slot (1 of 1 host-wide in use)\n"
                                 "oxide: stopped by the admission budget (1000ms) before a usable child start\n")
                os.write(fd, stream([
                    {"seq": 1, "event": "admitted_child_started", "phase": "exec_tool", "spawn_late": False},
                    {"seq": 2, "event": "finished", "outcome": "execution_deadline",
                     "stop_origin": "execution", "exit_code": 125},
                ]))
                return completed(argv, nested_code, "", "oxide: stopped by the execution budget (2000ms)\n")
            os.write(fd, stream([
                {"seq": 1, "event": "admitted_child_started",
                 "admission_ms": millis(flag(argv, "--admission-budget")),
                 "execution_ms": millis(flag(argv, "--execution-budget")),
                 "execution_started_ms": 0 if flag(argv, "--execution-budget") else None,
                 "spawn_late": False},
                {"seq": 2, "event": "finished", "outcome": "child_exit", "exit_code": 0,
                 "stop_origin": None},
            ]))
            return completed(argv, 0, "", "")

        return fake_run

    def fault_of(module, step):
        try:
            step()
            return None
        except module.Fault as error:
            return str(error)

    # ---- fast start: three axes pass; a nulled execution axis must fail ----
    module = load()
    module.run = make_run(module, late=False, late_start=False, nested_code=125)
    expect(fault_of(module, lambda: module.case_fast_start_axes(scratch, "/bin/true")) is None,
           "fast-start should pass")
    expect(module.RESULTS and module.RESULTS[0]["result"] == "pass", "fast-start did not record a pass")

    broken = load()

    def nulled_execution(argv, **kwargs):
        fd = int(flag(argv, "--budget-events"))
        os.write(fd, stream([
            {"seq": 1, "event": "admitted_child_started", "admission_ms": 30000,
             "execution_ms": None, "execution_started_ms": 0, "spawn_late": False},
            {"seq": 2, "event": "finished", "outcome": "child_exit", "exit_code": 0},
        ]))
        return completed(argv, 0, "", "")

    broken.run = nulled_execution
    fault = fault_of(broken, lambda: broken.case_fast_start_axes(scratch, "/bin/true"))
    expect(fault is not None and "execution axis" in fault, f"a nulled axis passed: {fault}")

    # ---- late start: a non-late start must fail the late oracle ----
    late_ok = load()
    late_ok.run = make_run(late_ok, late=True, late_start=True, nested_code=125)
    expect(fault_of(late_ok, lambda: late_ok.case_late_start(scratch, "/bin/true")) is None,
           "late-start should pass")
    late_bad = load()
    late_bad.run = make_run(late_bad, late=False, late_start=True, nested_code=125)
    fault = fault_of(late_bad, lambda: late_bad.case_late_start(scratch, "/bin/true"))
    expect(fault is not None and "late start" in fault, f"a non-late start passed: {fault}")

    # ---- nested: a natural outer exit must fail ----
    nested_ok = load()
    nested_ok.run = make_run(nested_ok, late=False, late_start=False, nested_code=125)
    expect(fault_of(nested_ok, lambda: nested_ok.case_nested(scratch, "/bin/true")) is None,
           "nested should pass")
    nested_bad = load()
    nested_bad.run = make_run(nested_bad, late=False, late_start=False, nested_code=0)
    fault = fault_of(nested_bad, lambda: nested_bad.case_nested(scratch, "/bin/true"))
    expect(fault is not None and "the outer command exited" in fault, f"a natural outer exit passed: {fault}")

    # ---- cumulative: the one-window variant passes, a reset fails ----
    def make_start(marker_poll, exit_poll):
        class FakeProcess:
            def __init__(self, argv, cwd):
                self.argv = argv
                self.cwd = cwd
                self.polls = 0
                self.returncode = None

            def poll(self):
                self.polls += 1
                root = os.path.dirname(os.path.dirname(self.cwd))
                if self.polls == marker_poll:
                    with open(os.path.join(root, "program.started"), "w", encoding="utf-8") as handle:
                        handle.write("started")
                if self.polls >= exit_poll:
                    os.write(int(flag(self.argv, "--budget-events")), stream([
                        {"seq": 1, "event": "admitted_child_started", "phase": "primary_cargo"},
                        {"seq": 2, "event": "admitted_child_started", "phase": "run_program"},
                        {"seq": 3, "event": "finished", "outcome": "execution_deadline",
                         "stop_origin": "execution", "exit_code": 125},
                    ]))
                    self.returncode = 125
                return self.returncode

            def kill(self):
                self.returncode = 137

        return lambda argv, **kwargs: FakeProcess(argv, kwargs["cwd"])

    one_window = load()
    one_window.start = make_start(marker_poll=2, exit_poll=4)
    expect(fault_of(one_window, lambda: one_window.case_cumulative_run(scratch, "/bin/true")) is None,
           "the one-window cumulative variant should pass")
    reset = load()
    reset.start = make_start(marker_poll=1, exit_poll=6)
    fault = fault_of(reset, lambda: reset.case_cumulative_run(scratch, "/bin/true"))
    expect(fault is not None and "window was reset" in fault, f"a reset window passed: {fault}")

    # ---- whole-lifecycle fault and receipt paths (no children) ----
    lifecycle = load()
    lifecycle.git = lambda *args: "f" * 40
    lifecycle.stage_subject = lambda scratch_arg: lifecycle.fail("stage boom")
    proof = lifecycle.Proof(scratch, "s" * 64)
    fault = lifecycle.capture_fault(proof.run)
    document = proof.document(fault)
    expect(fault == "stage boom", f"a staging fault was lost: {fault}")
    expect(document["identity"]["candidate_sha256"] is None, "an unstaged receipt claimed a candidate")
    expect(document["result"] == "fail" and document["primary_fault"] == "stage boom", "staging receipt wrong")

    refused = load()
    refused.git = lambda *args: "f" * 40
    refused.stage_subject = lambda scratch_arg: os.path.join(scratch, "fake-subject")
    refused.write_private(os.path.join(scratch, "fake-subject"), "#!/bin/sh\nexit 0\n")
    refused.os.chmod(os.path.join(scratch, "fake-subject"), 0o700)
    refused.probe_capabilities = lambda subject, scratch_arg: refused.fail("release candidate has no debug controls")
    refused_proof = refused.Proof(scratch, "s" * 64)
    refused_fault = refused.capture_fault(refused_proof.run)
    refused_document = refused_proof.document(refused_fault)
    expect("debug controls" in (refused_fault or ""), f"the capability refusal was lost: {refused_fault}")
    expect(
        refused_document["identity"]["candidate_sha256"] is not None
        and refused_document["identity"]["capabilities"] is None,
        "a refused capability should keep the candidate hash and leave capabilities null",
    )

    interrupted = load()
    interrupted.git = lambda *args: "f" * 40
    interrupted.stage_subject = lambda scratch_arg: (_ for _ in ()).throw(KeyboardInterrupt)
    expect(interrupted.capture_fault(interrupted.Proof(scratch, "s" * 64).run) == "interrupted (KeyboardInterrupt)",
           "an interrupt did not become a receipt fault")

    reached = lifecycle.Proof(scratch, "s" * 64).document(None)
    expect(reached["reached"] == list(lifecycle.CASES), "reached must come from CASES")
    expect("cumulative-run-window" in reached["unexecuted"], "unexecuted must come from the plan")

    failing_receipt = load()
    failing_receipt.PRIMARY_FAULT = "primary boom"
    failing_receipt.write_receipt = lambda document, scratch_arg: (_ for _ in ()).throw(OSError("disk full"))
    expect(failing_receipt.write_receipt_safely({"result": "fail"}, scratch) is None,
           "a receipt-write failure must be reported as not written")
    expect(failing_receipt.PRIMARY_FAULT == "primary boom", "a receipt-write failure replaced the primary fault")
    print("self-test: ok (faked bodies for fast-start/late/cumulative/nested plus lifecycle fault paths)")


def capture_fault(step) -> str | None:
    """Run one lifecycle step and return its primary fault, or None.

    Faults, interrupts and unexpected errors all produce a receipt-worthy
    string; the first recorded fault text wins so a later cleanup failure can
    never replace it.
    """
    try:
        step()
    except Fault as error:
        return PRIMARY_FAULT or str(error) or "assertion failed"
    except KeyboardInterrupt:
        return PRIMARY_FAULT or "interrupted (KeyboardInterrupt)"
    except BaseException as error:  # noqa: BLE001 - every outcome gets a receipt
        return PRIMARY_FAULT or f"{type(error).__name__}: {error}"
    return None


def main(argv: list[str]) -> int:
    mode, error = parse_arguments(argv)
    if mode == "refuse":
        print(f"command-budgets: {error}", file=sys.stderr)
        print(USAGE, end="", file=sys.stderr)
        return 2
    if mode == "help":
        print(USAGE, end="")
        return 0
    if mode == "plan":
        print(json.dumps(plan_document(), indent=2, sort_keys=True))
        return 0
    if mode == "self-check":
        self_check()
        return 0
    if mode == "self-test":
        self_test()
        return 0
    runner = os.path.join(REPO, "scripts", "bounded-test.py")
    runner_sha = sha256_file(runner)
    expect(
        runner_sha == EXPECTED_RUNNER_SHA256,
        f"supervisor {runner_sha} is not the pinned {EXPECTED_RUNNER_SHA256}",
    )
    dirty = git("status", "--porcelain")
    expect(dirty == "", f"the worktree is not clean, so the staged source would not be HEAD: {dirty[:400]}")
    scratch, _evidence = require_controls()
    work_left()
    say(
        f"host: project={git('remote', 'get-url', 'origin')} commit={git('rev-parse', 'HEAD')} "
        f"branch={git('branch', '--show-current')} cpu={os.cpu_count()} load={os.getloadavg()}"
    )
    proof = Proof(scratch, runner_sha)
    fault = capture_fault(proof.run)
    for process in OWNED:
        release(process)
    document = proof.document(fault)
    path = write_receipt_safely(document, scratch)
    say(f"helper receipt {path if path else 'NOT WRITTEN'} (result {document['result']})")
    if fault is None and path is not None:
        print(json.dumps(document, indent=2, sort_keys=True), flush=True)
    elif fault is not None:
        say(f"primary fault: {fault}")
    return 0 if fault is None else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
