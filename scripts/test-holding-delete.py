#!/usr/bin/env python3
"""Focused native proof for whole-tree holding eviction (oxide#190).

One fixed script and the whole command of the qualified shared supervisor
(`bounded-test.py --limit 300 --scratch-budget-mib 512 -- python3
scripts/test-holding-delete.py`): 300 s wall clock including staging,
fixtures and cleanup, 512 MiB monitored scratch, 30 s shutdown margin, one
staged binary, one fixture Cargo command at a time except the deliberately
concurrent case, and one Cargo job per fixture build.

`--plan` prints the exact command, cases, phases, disk allocation,
concurrency and limits with no effect. `--diagnostic` is the same guard,
root and limits in keep-going mode: it records every case it reaches, keeps
the first fault, continues only into cases whose prerequisites completed, and
writes a bounded `qualified: false` receipt. Only the bare invocation is the
counted proof.

The subject is this HEAD's debug `oxide`, staged with plain Cargo `--offline`
from the invoking Cargo home (target and build directories stay inside the
monitored scratch and are deleted before any fixture). Every fixture tree has
its own private `HOME`, `CARGO_HOME`, pool and crate, so nothing touches a
live pool, `~/.local/bin/oxide`, a mounted volume or host settings.

Cases:

- `evict-under-lock`: a 20,000-entry, ~20 MiB idle slot is evicted by another
  project's admission. The product's own debug-only observation records the
  pool-lock-held duration of that eviction; it must be well under 1 s while
  the tree is deleted outside the lock by the evicting command, which prints
  the deletion and leaves no holding row.
- `refused-entry-kept`: an evicted tree holding an entry the traversal refuses
  (a FIFO, an unknown entry type) stays in the holding area, keeps its
  recorded charge and keeps being reported; once the refused item is removed
  the next command's pass deletes it.
- `concurrent-admission`: while one command is deleting a 20,000-entry held
  tree, another project's command completes normally — the deletion holds no
  pool lock, so admission is not held behind it.
- `killed-deleter-takeover`: a deleter killed with SIGKILL leaves its entry
  recorded and charged; the next command claims it and finishes it.

Throughput is reported as the observed entries/second and MiB/second of the
outside-lock traversal plus the measured lock-held figure. The retired
per-entry journaled path is not executable here, so its measured figure from
the 2026-10-29 incident (about 60 entries/s, about 16.7 ms per entry, all
under the pool lock) is recorded as a labelled historical reference, never
as a live comparison. Tiny fixtures do not establish a real pool's rate, do
not measure removable media, and are not a benchmark or an alias for full QA.
"""

from __future__ import annotations

import hashlib
import json
import re
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILDER = "1.97.1"
EXPECTED_RUNNER_SHA256 = "bc24b238a2ec6f0a68719f59b14bce2d69df0e30c2a351366fdab07db4975924"
MIB = 1024 * 1024
WALL_LIMIT_S = 300
SCRATCH_BUDGET_MIB = 512
SHUTDOWN_MARGIN_S = 30
# A budget far below one planted fixture tree, so one admission must evict
# exactly the idle whole slot that holds it.
POOL_CAP = "8MiB"
SLOT_ENTRIES = 20_000
# The one large entry that makes the recorded figure cross a small budget.
SLOT_BYTES = 20 * MIB
# The concurrency case plants two `SLOT_ENTRIES`-entry trees of
# `CONCURRENT_ENTRY_BYTES` each, so both deletions last long enough to observe,
# and a 100 MiB cap still admits while one tree is pending.
CONCURRENT_ENTRY_BYTES = 1024
CONCURRENT_CAP = "100MiB"
STAGE_JOBS = 1
FIXTURE_JOBS = 1
LOCK_CEILING_MS = 1000
# Two commands that genuinely ran at the same time overlap for at least this
# long. An in-lock deletion would serialise them completely (zero overlap), so
# this is a real discriminator even though one fixture deletion is short.
# The falsifying in-lock control: the same new-slot admission is measured on a
# separate, pressure-free pool, and the admission taken during a live deletion
# must stay inside this multiple of it (plus this many seconds of slack). An
# in-lock deletion would delay it by the whole deletion, far outside the bound.
INLOCK_CONTROL_FACTOR = 3.0
INLOCK_CONTROL_SLACK_S = 2.0
# How long the deleter keeps custody before the competitor arrives, so the
# deletion is measurably in progress when the competitor takes the pool lock.
CONCURRENT_CUSTODY_S = 0.5
# The interval between two entry-count samples of the held tree. A strictly
# decreasing count between them is measured progress under the deleter's
# custody: nobody else can claim the entry while it holds the flock.
PROGRESS_INTERVAL_S = 0.3
MAX_RECEIPT_BYTES = 64 * 1024
SELECTOR = "rust:holding-delete"
COMMAND = [
    "python3",
    "scripts/bounded-test.py",
    "--limit",
    str(WALL_LIMIT_S),
    "--scratch-budget-mib",
    str(SCRATCH_BUDGET_MIB),
    "--",
    "python3",
    "scripts/test-holding-delete.py",
]
USAGE = """usage: test-holding-delete.py [--plan | --self-check | --diagnostic | --help]

Without an argument this runs the counted proof under the qualified shared
supervisor. --plan prints the fixed command, cases, phases, disk allocation,
concurrency and limits without any effect. --diagnostic runs the same guard,
root and bounds in keep-going mode and writes a bounded, unqualified receipt;
it is never a counted PASS. --self-check runs pure argument, plan, oracle and
constant checks in this process without staging, a child or a fixture.
"""
# The declared cases, in order, each with the result it has reached. A case is
# recorded as `unexecuted` until the run reaches it, so a run that stops early
# never lets an unreached case read as a pass.
DECLARED_CASES = (
    "evict-under-lock",
    "refused-entry-kept",
    "concurrent-admission",
    "killed-deleter-takeover",
)
CASES: list[dict] = [{"name": name, "result": "unexecuted"} for name in DECLARED_CASES]
RESULTS: list[dict] = []
PHASE_STARTED = time.perf_counter()
DIAGNOSTIC = False
FIRST_FAULT: str | None = None
# Every fault this run recorded, in order, bounded. The first one is latched
# separately for the receipt's headline; this list shows that a case fault was
# followed by a custody fault, or that only one happened.
FAULTS: list[str] = []
# Fixture children this process started and has not yet reaped. A case may not
# return while one is still running: its custody would be unknown.
CHILDREN: list[tuple[str, subprocess.Popen]] = []
# Whether the case being run started a fixture child at all. A descendant of a
# fixture command (Oxide's own Cargo group) is outside this proof's signalling
# authority, so once a child existed, a failed case cannot be followed by another
# case: descendant closure is the shared supervisor's census, never this script's
# assumption.
CASE_STARTED_CHILD = False


def fail(message: str) -> None:
    print(f"holding-delete proof failed: {message}", file=sys.stderr)
    sys.exit(1)


def record_fault(name: str, message: str) -> None:
    """Record a fault: the first one is latched, all of them are kept (bounded)."""
    global FIRST_FAULT
    print(f"holding-delete case {name} failed: {message}", file=sys.stderr)
    if FIRST_FAULT is None:
        FIRST_FAULT = f"{name}: {message}"
    if len(FAULTS) < 8:
        FAULTS.append(f"{name}: {message}")


def expect(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def say(message: str) -> None:
    print(f"[{time.perf_counter() - PHASE_STARTED:7.1f}s] {message}", flush=True)


def declare(names: tuple[str, ...] | list[str]) -> list[dict]:
    """Install a fresh case table; the collector and its regression share it."""
    global CASES
    CASES = [{"name": name, "result": "unexecuted"} for name in names]
    return CASES


def case(name: str) -> None:
    mark(name, "reached")
    say(f"case {name}")


def mark(name: str, result: str, **facts) -> None:
    for entry in CASES:
        if entry["name"] == name:
            entry["result"] = result
            entry.update(facts)


def passed(name: str, **_facts) -> None:
    """Announce a case's own oracle results; the collector records the result."""
    say(f"case {name}: passed")


def unexecuted(names: tuple[str, ...] | list[str]) -> None:
    for name in names:
        for entry in CASES:
            if entry["name"] == name and entry["result"] == "unexecuted":
                entry["result"] = "unexecuted"


def close_children() -> bool:
    """Close every fixture group this process still tracks; report closure."""
    closed = True
    for label, handle in list(CHILDREN):
        closed = discard(handle, label) and closed
    return closed and not CHILDREN


def child_faults() -> str | None:
    """A child this process started and did not reap is unknown custody."""
    if not CHILDREN:
        return None
    labels = ", ".join(label for label, _ in CHILDREN)
    return f"a fixture child was not reaped: {labels}"


def deadline_remaining() -> float | None:
    """Seconds left before the supervisor's stop point, or `None` when the
    supervisor exported no deadline. Never fails: it is a receipt field."""
    value = os.environ.get("OXIDE_PROOF_DEADLINE_NS")
    if not value:
        return None
    try:
        left = (int(value) - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9
    except ValueError:
        return None
    return round(left, 1)


def work_left() -> float:
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    left = (deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9
    left -= SHUTDOWN_MARGIN_S
    if left <= 5:
        fail("work deadline reached before the supervisor's shutdown margin")
    return left


def sha256_file(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def read_text(path: str) -> str:
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as handle:
            return handle.read()
    except FileNotFoundError:
        return ""


def read_json(path: str) -> dict:
    return json.loads(read_text(path))


def git(*args: str) -> str:
    result = subprocess.run(
        ["git", *args], cwd=REPO, check=True, capture_output=True, text=True
    )
    return result.stdout.strip()


def private_dir(path: str) -> str:
    os.makedirs(path, exist_ok=True)
    os.chmod(path, 0o700)
    return path


def write_private(path: str, data: str) -> None:
    private_dir(os.path.dirname(path))
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(data)
    os.chmod(path, 0o600)


def write_entries(
    root: str, count: int, big_bytes: int = 0, entry_bytes: int = 0
) -> int:
    """`count` entries in `root`, in flat batches.

    One entry carries `big_bytes` so the recorded figure crosses a small pool
    budget without needing every entry to. The rest are empty, which keeps both
    creation and deletion cheap while the tree still has `count` real entries.
    """
    private_dir(root)
    written = 0
    remaining = count
    bucket = 0
    while remaining:
        directory = private_dir(os.path.join(root, f"b{bucket:04d}"))
        for index in range(min(500, remaining)):
            path = os.path.join(directory, f"e{index:04d}")
            with open(path, "wb") as handle:
                if bucket == 0 and index == 0 and big_bytes:
                    handle.write(b"x" * big_bytes)
                    written += big_bytes
                elif entry_bytes:
                    handle.write(b"x" * entry_bytes)
                    written += entry_bytes
            remaining -= 1
        bucket += 1
    os.chmod(root, 0o700)
    return written


def make_fifo(path: str) -> None:
    os.mkfifo(path, 0o600)


# --------------------------------------------------------------------------
# Fixed plan, argument boundary and pure helpers. None of this has an effect.


def plan_document() -> dict:
    return {
        "task": "oxide#190",
        "selector": SELECTOR,
        "command": COMMAND,
        "counted_only_without_argument": True,
        "diagnostic": {
            "mode": "--diagnostic",
            "counted": False,
            "receipt": "bounded qualified:false beside the guard's evidence",
            "keep_going": "a recoverable oracle failure continues only when the "
            "failed case started no fixture child; a case that ran a command "
            "aborts the run (its descendants stay under the shared supervisor's "
            "census, which this script cannot conclude)",
        },
        "cases": [
            {
                "name": "evict-under-lock",
                "oracle": f"a {SLOT_ENTRIES}-entry idle slot is evicted by "
                f"another project's admission; the product's own observation of "
                f"the pool-lock-held part is under {LOCK_CEILING_MS} ms, the "
                "tree is deleted outside the lock by that command, and no "
                "holding row survives",
            },
            {
                "name": "refused-entry-kept",
                "oracle": "an evicted tree holding a FIFO is refused, stays in "
                "the holding area with its recorded charge and its row, is "
                "reported by status, and the next command after the item is "
                "removed finishes it",
            },
            {
                "name": "concurrent-admission",
                "oracle": "two entry-count samples of the held tree taken while "
                "only the deleter runs strictly decrease (measured progress under "
                "its non-blocking custody), then a new-slot admission (observed by "
                "the pool's own ledger row appearing) completes with the deleter "
                "still alive and that tree still non-empty, and that admission "
                "stays inside "
                f"{INLOCK_CONTROL_FACTOR}x the same admission measured on a "
                f"pressure-free pool plus {INLOCK_CONTROL_SLACK_S}s. An in-lock "
                "deletion would delay it by the whole deletion and fail the "
                "control; the control's own numbers are recorded.",
            },
            {
                "name": "killed-deleter-takeover",
                "oracle": "a SIGKILLed deleter leaves its row recorded and "
                "charged, and the next command finishes that entry",
            },
        ],
        "phases": [
            {"name": "stage", "allocation": "this HEAD's debug oxide, plain "
             "Cargo offline, target/build inside scratch, target deleted"},
            {"name": "evict", "allocation": f"one fixture root, one {SLOT_ENTRIES}-entry tree"},
            {"name": "refused", "allocation": "one fixture root, one small tree plus a FIFO"},
            {"name": "concurrent", "allocation": "one fixture root, two "
             f"{SLOT_ENTRIES}-entry trees of {CONCURRENT_ENTRY_BYTES} bytes per entry"},
            {"name": "takeover", "allocation": f"one fixture root, one {SLOT_ENTRIES}-entry tree"},
        ],
        "disk": {
            "monitored_scratch_mib": SCRATCH_BUDGET_MIB,
            "fixture_roots": 4,
            "per_fixture": f"{SLOT_ENTRIES} planted entries with one "
            f"{SLOT_BYTES // MIB} MiB entry (or {CONCURRENT_ENTRY_BYTES} bytes "
            "per entry in the concurrency case, two such trees at a "
            f"{CONCURRENT_CAP} cap), plus tiny std-only crate intermediates",
            "staged_target": "deleted before any fixture",
        },
        "concurrency": {
            "staged_binaries": 1,
            "fixture_commands": "one at a time except one deliberately "
            "concurrent admission in `concurrent-admission`",
            "stage_cargo_jobs": STAGE_JOBS,
            "fixture_cargo_jobs": FIXTURE_JOBS,
            "sessions": "none created: every fixture child and its descendants "
            "stay in the qualified shared supervisor's own session, which owns "
            "session closure and the census that decides it",
        },
        "limits": {
            "wall_limit_s": WALL_LIMIT_S,
            "scratch_budget_mib": SCRATCH_BUDGET_MIB,
            "shutdown_margin_s": SHUTDOWN_MARGIN_S,
            "lock_ceiling_ms": LOCK_CEILING_MS,
            "inlock_control_factor": INLOCK_CONTROL_FACTOR,
            "inlock_control_slack_s": INLOCK_CONTROL_SLACK_S,
            "concurrent_custody_s": CONCURRENT_CUSTODY_S,
            "progress_interval_s": PROGRESS_INTERVAL_S,
        },
        "observations": {
            "lock_held": "the product's debug-only OXIDE_TEST_HOLDING_LOCK_LOG "
            "line per eviction, armed only inside the supervisor's scratch root. "
            "The figure is the whole eviction transaction under the pool lock "
            "(victim and rank checks, the identity-verified rename and the one "
            "durable ledger write), timed from the start of that transaction, "
            "not an inner fragment of it",
            "throughput": "entries/second and MiB/second of the outside-lock "
            "traversal, measured from the deleted figures and elapsed time",
            "legacy_reference": "the retired per-entry journaled path is not "
            "executable here; its measured 2026-10-29 figure (about 60 "
            "entries/s, about 16.7 ms per entry, under the pool lock) is a "
            "labelled historical reference, not a live comparison",
        },
        "live_pool": False,
        "mounted_volume": False,
        "timing": "unmeasured; this plan claims no wall-clock figure",
    }


def parse_arguments(argv: list[str]) -> tuple[str, str | None]:
    if not argv:
        return ("proof", None)
    if len(argv) > 1:
        return ("refuse", f"at most one argument is accepted, got {argv!r}")
    mode = {
        "--plan": "plan",
        "--self-check": "self-check",
        "--diagnostic": "diagnostic",
        "--help": "help",
    }
    if argv[0] in mode:
        return (mode[argv[0]], None)
    return ("refuse", f"unrecognized argument {argv[0]!r}")


def ledger(pool: str) -> dict:
    return read_json(os.path.join(pool, "ledger.json"))


def holding_rows(pool: str) -> list[dict]:
    return ledger(pool).get("holding", [])


def slot_rows(pool: str) -> list[dict]:
    return ledger(pool)["slots"]


def slot_path(pool: str, slot: dict) -> str:
    slots_root = os.path.join(pool, "payload", "slots")
    if "dir" in slot:
        return os.path.join(slots_root, slot["dir"])
    return os.path.join(slots_root, str(slot["id"]))


DELETION_LINE = re.compile(
    r"deleted the held slot tree (?P<name>\S+) outside the pool lock "
    r"\((?P<entries>\d+) entries, (?P<size>[^)]*) in (?P<seconds>[\d.]+)s\)"
)


def deletion_figures(output: str) -> list[dict]:
    """Every deletion line the product printed, with its own figures."""
    figures = []
    for match in DELETION_LINE.finditer(output):
        seconds = float(match.group("seconds"))
        figures.append(
            {
                "name": match.group("name"),
                "entries": int(match.group("entries")),
                "size": match.group("size"),
                "seconds": seconds,
                "entries_per_s": round(int(match.group("entries")) / max(seconds, 0.001), 1),
            }
        )
    return figures


def lock_observations(path: str) -> list[dict]:
    """Every recorded lock-held figure, oldest first."""
    readings = []
    for line in read_text(path).splitlines():
        parts = line.split()
        if len(parts) != 2:
            continue
        try:
            readings.append({"slot": os.path.basename(parts[0]), "ms": int(parts[1])})
        except ValueError:
            continue
    return readings


def entry_count(root: str) -> int:
    total = 0
    for _, directories, files in os.walk(root, onerror=lambda _: None):
        total += len(directories) + len(files)
    return total


def tree_present(path: str) -> bool:
    try:
        return stat.S_ISDIR(os.lstat(path).st_mode)
    except FileNotFoundError:
        return False


def self_check() -> None:
    """The script's own oracles, without staging, a child or a pool.

    The only files it creates are inside one private `tempfile.mkdtemp`
    directory, which it removes before it returns.
    """
    expect(parse_arguments([]) == ("proof", None), "the bare invocation must run the proof")
    expect(parse_arguments(["--plan"]) == ("plan", None), "--plan must be effect-free")
    expect(parse_arguments(["--self-check"]) == ("self-check", None), "--self-check must parse")
    expect(
        parse_arguments(["--diagnostic"]) == ("diagnostic", None),
        "--diagnostic must parse",
    )
    expect(parse_arguments(["--help"]) == ("help", None), "--help must parse")
    for refused in (["--plan", "--plan"], ["proof"], ["--bogus"], [""]):
        mode, error = parse_arguments(refused)
        expect(mode == "refuse" and bool(error), f"{refused!r} must be refused")
    plan = plan_document()
    expect(plan["limits"]["wall_limit_s"] == 300, "the plan wall limit changed")
    expect(plan["limits"]["scratch_budget_mib"] == 512, "the plan scratch budget changed")
    expect(plan["limits"]["shutdown_margin_s"] == 30, "the plan margin changed")
    expect(plan["limits"]["lock_ceiling_ms"] == 1000, "the plan lock ceiling changed")
    expect(
        plan["limits"]["inlock_control_factor"] == 3.0
        and plan["limits"]["inlock_control_slack_s"] == 2.0,
        "the in-lock control bound changed",
    )
    expect(
        plan["limits"]["concurrent_custody_s"] == 0.5,
        "the concurrent custody lead changed",
    )
    expect(
        plan["limits"]["progress_interval_s"] == 0.3,
        "the progress sampling interval changed",
    )
    expect(len(plan["cases"]) == 4, "the plan case count changed")
    expect(plan["disk"]["fixture_roots"] == 4, "the plan fixture count changed")
    expect(plan["counted_only_without_argument"], "the counted-only rule changed")
    expect(not plan["diagnostic"]["counted"], "the diagnostic must never be counted")
    expect(
        "aborts the run" in plan["diagnostic"]["keep_going"],
        "the diagnostic keep-going rule changed",
    )
    expect(
        "not a live comparison" in plan["observations"]["legacy_reference"],
        "the historical reference must be labelled, not claimed as live",
    )
    expect(POOL_CAP == "8MiB", "the fixture pool budget changed")
    expect(SLOT_ENTRIES == 20_000, "the fixture entry count changed")
    expect(
        COMMAND[-1].endswith("scripts/test-holding-delete.py")
        and "--limit" in COMMAND
        and "--scratch-budget-mib" in COMMAND,
        "the fixed command changed",
    )
    expect(
        sha256_file(os.path.abspath(__file__)) != "",
        "the script digest is unreadable",
    )
    # The oracles on real fixture data, inside one private temporary directory
    # created and removed by this process: nothing fixed, nothing left behind.
    scratch = tempfile.mkdtemp(prefix="holding-delete-self-check.")
    try:
        self_check_oracles(scratch)
        collector = tempfile.mkdtemp(prefix="holding-delete-collector.")
        try:
            self_check_collector(collector)
        finally:
            shutil.rmtree(collector, ignore_errors=True)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    print("self-check: ok (pure argument, plan, oracle and constant checks; not evidence)")


def self_check_collector(scratch: str) -> None:
    """The collector's own branch: keep-going only while closure is provable.

    Fake case bodies, no child, no pool: the interesting thing is exactly which
    cases the collector records and whether it aborts, and that it treats a
    synchronously launched fixture command the same as an asynchronous one.
    """
    global DIAGNOSTIC, FIRST_FAULT

    def no_child(_root: str, _subject: str) -> dict:
        raise AssertionError("a case that launched nothing failed")

    def launched_then_failed(_root: str, _subject: str) -> dict:
        # Exactly what every fixture launcher does, before the launch.
        mark_fixture_child()
        raise AssertionError("a case that launched a command failed")

    def passing(_root: str, _subject: str) -> dict:
        return {"ok": True}

    def drive(keep_going: bool, bodies: list) -> tuple[dict, str | None, bool]:
        declare([name for name, _ in bodies])
        FIRST_FAULT = None
        del FAULTS[:]
        DIAGNOSTIC = keep_going
        try:
            facts = collect(bodies, scratch, keep_going)
            return facts, None, True
        except AssertionError as error:
            return {}, str(error), False
        finally:
            DIAGNOSTIC = False

    results = [
        ("no-child-failure", no_child),
        ("after-it", passing),
    ]
    _, _, completed = drive(True, results)
    expect(completed, "a diagnostic case with no child must still continue")
    expect(
        [entry["result"] for entry in CASES] == ["failed", "passed"],
        f"keep-going accounting changed: {CASES}",
    )
    expect(
        FIRST_FAULT is not None and "launched nothing" in FIRST_FAULT,
        f"the first fault was not latched: {FIRST_FAULT}",
    )

    launched = [
        ("launched-failure", launched_then_failed),
        ("after-it", passing),
    ]
    _, fault, completed = drive(True, launched)
    expect(
        not completed,
        "a diagnostic continued after a case that launched a fixture command",
    )
    expect(
        [entry["result"] for entry in CASES] == ["failed", "unexecuted"],
        f"abort accounting changed: {CASES}",
    )
    expect(
        any("descendant closure belongs" in fault for fault in FAULTS),
        f"the custody fault was not recorded: {FAULTS}",
    )

    counted = [
        ("launched-failure", launched_then_failed),
        ("after-it", passing),
    ]
    _, _, completed = drive(False, counted)
    expect(not completed, "counted mode did not stop at the first failure")
    expect(
        [entry["result"] for entry in CASES] == ["failed", "unexecuted"],
        f"counted accounting changed: {CASES}",
    )

    # Every fixture launch marks the case first: the wrapper and each direct
    # synchronous site are checked in the source, because that ordering is what
    # the collector branch above relies on.
    source = read_text(os.path.abspath(__file__))
    for helper in ("def check(", "def start_check(", "def run_fixture(", "def spawn_fixture("):
        start = source.index(helper)
        body = source[start : source.index("\ndef ", start + 1)]
        expect(
            "mark_fixture_child()" in body
            or "run_fixture(" in body
            or "spawn_fixture(" in body,
            f"{helper} does not mark its fixture launch",
        )
    status_site = source.index("[subject, \"status\"]")
    expect(
        "run_fixture(" in source[max(0, status_site - 400) : status_site],
        "the synchronous status command does not mark its launch",
    )
    declare(DECLARED_CASES)


def self_check_oracles(scratch: str) -> None:
    entries = os.path.join(scratch, "entries")
    write_entries(entries, 1000)
    expect(entry_count(entries) >= 1000, "the entry counter missed planted entries")
    fifo = os.path.join(entries, "pipe")
    make_fifo(fifo)
    expect(stat.S_ISFIFO(os.lstat(fifo).st_mode), "the FIFO oracle misfired")
    expect(tree_present(entries) and not tree_present(fifo), "the presence oracle misfired")
    figures = deletion_figures(
        "oxide: deleted the held slot tree abc-1-1-v1 outside the pool lock "
        "(20000 entries, "
        "78.1 MiB in 12.50s)\n"
    )
    expect(
        figures
        == [
            {
                "name": "abc-1-1-v1",
                "entries": 20000,
                "size": "78.1 MiB",
                "seconds": 12.5,
                "entries_per_s": 1600.0,
            }
        ],
        f"the deletion-line oracle changed: {figures}",
    )
    expect(deletion_figures("no deletion here") == [], "the deletion oracle invented a line")
    # The proof never creates a session and never signals a group: fixture
    # children and their descendants stay in the qualified supervisor's session.
    source = read_text(os.path.abspath(__file__))
    # The literals are assembled here so this check does not match itself.
    for forbidden in ("start_new" + "_session", "kill" + "pg", "os.set" + "sid"):
        expect(
            forbidden not in source,
            f"the proof must leave session custody to the supervisor ({forbidden})",
        )
    log = os.path.join(scratch, "lock.log")
    write_private(log, "/p/payload/slots/7 3\nnot a reading\n/p/x 12\n")
    readings = lock_observations(log)
    expect(
        readings == [{"slot": "7", "ms": 3}, {"slot": "x", "ms": 12}],
        f"the lock observation reader changed: {readings}",
    )


# --------------------------------------------------------------------------
# The proof. Everything below has effects and requires the supervisor.


def stage_subject(root: str) -> str:
    """Stage this HEAD's debug `oxide` with plain Cargo, offline."""
    stage = os.path.join(root, "stage")
    log = os.path.join(root, "stage.log")
    # A staging environment the caller's flags cannot steer: Cargo's encoded
    # forms (`CARGO_ENCODED_RUSTFLAGS` especially) override the plain variables
    # this script sets, so every encoded form is dropped along with the fixed
    # flags themselves.
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(
            ("CARGO_TARGET_", "CARGO_BUILD_", "CARGO_ENCODED_", "SCCACHE_", "OXIDE_", "XDG_")
        )
        and key
        not in (
            "CARGO",
            "RUSTC",
            "RUSTFLAGS",
            "RUSTDOCFLAGS",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
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
        }
    )
    with open(log, "w", encoding="utf-8") as handle:
        build = subprocess.run(
            ["cargo", "build", "-j", str(STAGE_JOBS), "--locked", "--offline", "--bin", "oxide"],
            cwd=REPO,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            timeout=work_left(),
        )
    if build.returncode:
        fail(f"staging exit {build.returncode}: {read_text(log)[-2000:]}")
    built = os.path.join(stage, "target", "debug", "oxide")
    expect(os.path.isfile(built), f"staging produced no binary at {built}")
    subject = os.path.join(root, "subject-oxide")
    shutil.copy2(built, subject)
    os.chmod(subject, 0o755)
    shutil.rmtree(stage)
    os.unlink(log)
    return subject


def invoking_rustup_home() -> str:
    explicit = os.environ.get("RUSTUP_HOME")
    if explicit:
        return explicit
    return os.path.join(os.path.expanduser("~"), ".rustup")


def fixture_env(root: str, pool: str, lock_log: str, cap: str = POOL_CAP) -> dict:
    home = private_dir(os.path.join(root, "home"))
    env = {
        "HOME": home,
        "OXIDE_TEST_HOME_FROM_ENV": "1",
        "TMPDIR": root,
        "TMP": root,
        "TEMP": root,
        "OXIDE_POOL": pool,
        "OXIDE_POOL_CAP": cap,
        "OXIDE_TEST_HOLDING_LOCK_LOG": lock_log,
        "CARGO_HOME": private_dir(os.path.join(root, "cargo-home")),
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "NO_COLOR": "1",
        "CARGO_TERM_COLOR": "never",
        "CARGO_NET_OFFLINE": "true",
        "RUSTUP_HOME": invoking_rustup_home(),
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_AUTO_INSTALL": "0",
    }
    for name in (
        "LANG",
        "LC_ALL",
        "TZ",
        "OXIDE_TASK_SCRATCH_ROOT",
        "OXIDE_PROOF_DEADLINE_NS",
        "OXIDE_PROOF_EVIDENCE",
    ):
        if name in os.environ:
            env[name] = os.environ[name]
    return env


def create_project(root: str, label: str) -> str:
    path = private_dir(os.path.join(root, "projects", label))
    write_private(
        os.path.join(path, "Cargo.toml"),
        f'[package]\nname = "oxide-holding-delete-{label}"\n'
        'version = "0.1.0"\nedition = "2021"\n',
    )
    write_private(os.path.join(path, "src", "lib.rs"), "pub fn fixture() {}\n")
    return path


def mark_fixture_child() -> None:
    """Record that the running case launched a fixture command.

    Called **before** every fixture launch, synchronous or asynchronous, so the
    collector never concludes that a failing case ran no command when it did —
    a command's descendants are outside this proof's signalling authority and
    their closure is the shared supervisor's census.
    """
    global CASE_STARTED_CHILD
    CASE_STARTED_CHILD = True


def run_fixture(argv: list[str], **kwargs) -> subprocess.CompletedProcess:
    """Run one fixture command synchronously, after marking the launch."""
    mark_fixture_child()
    return subprocess.run(argv, **kwargs)


def spawn_fixture(argv: list[str], **kwargs) -> subprocess.Popen:
    """Start one fixture command asynchronously, after marking the launch."""
    mark_fixture_child()
    return subprocess.Popen(argv, **kwargs)


def check(subject: str, workspace: str, env: dict, label: str) -> tuple[int, str, float]:
    work_left()
    started = time.perf_counter()
    result = run_fixture(
        [subject, "check", "--offline", "-j", str(FIXTURE_JOBS)],
        cwd=workspace,
        env=env,
        capture_output=True,
        text=True,
        timeout=work_left(),
    )
    elapsed = time.perf_counter() - started
    output = f"{result.stdout}{result.stderr}"
    say(f"{label}: exit {result.returncode} in {elapsed:.2f}s, {len(output)} bytes of output")
    return result.returncode, output, elapsed


def start_check(subject: str, workspace: str, env: dict, label: str) -> subprocess.Popen:
    work_left()
    handle = spawn_fixture(
        [subject, "check", "--offline", "-j", str(FIXTURE_JOBS)],
        cwd=workspace,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        # The child stays in the qualified supervisor's own session and process
        # group. This proof never creates a session and never signals a group:
        # session closure and the census that decides it belong to the shared
        # supervisor, and an escape it did not create would be its finding, not
        # this script's.
    )
    CHILDREN.append((label, handle))
    say(f"{label}: started (pid {handle.pid})")
    return handle


def finish(handle: subprocess.Popen, label: str) -> tuple[int, str]:
    """Collect one fixture child, keeping custody if it does not close.

    A child still running at the bounded wait stays **tracked**: dropping it
    would lose its custody, so the timeout propagates to the collector, which
    closes the whole group and can then report what happened.
    """
    work_left()
    output, _ = handle.communicate(timeout=work_left())
    CHILDREN[:] = [entry for entry in CHILDREN if entry[1] is not handle]
    say(f"{label}: exit {handle.returncode}")
    return handle.returncode, output


def discard(handle: subprocess.Popen, label: str) -> bool:
    """Kill and reap one fixture child by its own PID, then confirm it is gone.

    Only the child this proof started is signalled — never a process group, and
    never a pattern — because the child and its descendants live in the shared
    supervisor's session, whose closure the supervisor decides. Returns whether
    the child was positively reaped; an unreaped child is unknown custody and
    must end a diagnostic run. The handle stays tracked until it is reaped, so a
    timeout can never drop custody silently.
    """
    try:
        handle.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        handle.kill()
        try:
            handle.communicate(timeout=30)
        except subprocess.TimeoutExpired:
            pass
    if handle.returncode is None:
        # Still not reaped: keep it tracked and report unknown custody.
        say(f"{label}: still running after the discard wait (unknown custody)")
        return False
    CHILDREN[:] = [entry for entry in CHILDREN if entry[1] is not handle]
    say(f"{label}: discarded (exit {handle.returncode}, reaped)")
    return True


def wait_for_holding_row(pool: str, timeout: float = 20.0) -> dict | None:
    """Poll the ledger for a holding row. Observation only, no lock."""
    deadline = time.perf_counter() + timeout
    while time.perf_counter() < deadline:
        work_left()
        try:
            rows = holding_rows(pool)
        except (FileNotFoundError, ValueError):
            rows = []
        if rows:
            return rows[0]
        time.sleep(0.01)
    return None


def seed_big_slot(
    root: str,
    subject: str,
    label: str,
    entries: int,
    fifo: bool = False,
    cap: str = POOL_CAP,
    big_bytes: int = SLOT_BYTES,
    entry_bytes: int = 0,
) -> tuple[str, dict, dict, dict]:
    """One idle whole slot holding `entries` entries of payload."""
    pool = os.path.join(root, "pool")
    lock_log = os.path.join(root, "eviction-lock.log")
    env = fixture_env(root, pool, lock_log, cap)
    workspace = create_project(root, label)
    code, output, _ = check(subject, workspace, env, f"seed-{label}")
    expect(code == 0, f"seeding slot {label} exited {code}: {output[-3000:]}")
    slot = slot_rows(pool)[0]
    slot_dir = slot_path(pool, slot)
    planted = os.path.join(slot_dir, "build", "planted")
    written = write_entries(planted, entries, big_bytes, entry_bytes)
    # The release is what records the closed observation the eviction charges.
    code, output, _ = check(subject, workspace, env, f"release-{label}")
    expect(code == 0, f"releasing slot {label} exited {code}: {output[-3000:]}")
    slot = slot_rows(pool)[0]
    if fifo:
        # After the recording walk, so the fixture's own measurement is a real
        # one and the refused item is what the traversal meets.
        make_fifo(os.path.join(planted, "pipe"))
    expect(
        slot["state"] == "idle" and not slot.get("lease_id"),
        f"the seeded slot is not idle: {json.dumps(slot)[:400]}",
    )
    return pool, env, slot, {"lock_log": lock_log, "planted": planted, "bytes": written}


def prove_evict_under_lock(root: str, subject: str) -> dict:
    name = "evict-under-lock"
    case(name)
    pool, env, slot, facts = seed_big_slot(root, subject, "lock", SLOT_ENTRIES)
    planted = facts["planted"]
    expect(entry_count(planted) >= SLOT_ENTRIES, "the planted tree lost entries")
    victim = slot_path(pool, slot)
    before = len(slot_rows(pool))
    other = create_project(root, "lock-other")
    code, output, elapsed = check(subject, other, env, "evict-by-admission")
    expect(code == 0, f"the evicting admission exited {code}: {output[-3000:]}")
    expect(
        "deleted the held slot tree" in output,
        f"the evicting command did not report its deletion: {output[-2000:]}",
    )
    expect(len(slot_rows(pool)) == before, "the evicting admission changed the slot count")
    expect(not tree_present(victim), "the evicted slot directory is still in the slots root")
    expect(not holding_rows(pool), f"a holding row survived: {holding_rows(pool)}")
    readings = [
        row for row in lock_observations(facts["lock_log"]) if row["slot"] == str(slot["id"])
    ]
    expect(readings, "the product recorded no eviction lock-hold figure")
    lock_ms = readings[0]["ms"]
    expect(
        lock_ms < LOCK_CEILING_MS,
        f"the pool lock was held {lock_ms} ms, at or above {LOCK_CEILING_MS} ms",
    )
    printed = deletion_figures(output)
    expect(len(printed) == 1, f"expected one deletion line, got {printed}")
    facts_out = {
        "lock_held_ms": lock_ms,
        "lock_ceiling_ms": LOCK_CEILING_MS,
        "command_elapsed_s": round(elapsed, 3),
        "planted_entries": SLOT_ENTRIES,
        "printed_deletion": printed[0],
        "legacy_reference": "about 60 entries/s under the pool lock (2026-10-29 "
        "incident measurement, retired path; historical, not live)",
    }
    passed(name, **facts_out)
    return facts_out


def prove_refused_entry_kept(root: str, subject: str) -> dict:
    name = "refused-entry-kept"
    case(name)
    pool, env, slot, _facts = seed_big_slot(
        root, subject, "refused", 400, fifo=True, cap="1MiB", big_bytes=2 * MIB
    )
    victim = slot_path(pool, slot)
    other = create_project(root, "refused-other")
    code, output, _ = check(subject, other, env, "evict-with-fifo")
    expect(code == 0, f"the evicting admission exited {code}: {output[-3000:]}")
    expect(not tree_present(victim), "the evicted slot directory is still in the slots root")
    rows = holding_rows(pool)
    expect(len(rows) == 1, f"expected one recorded held tree, found {rows}")
    row = rows[0]
    expect(row["bytes"] > 0, f"the held row carries no charge: {row}")
    held = os.path.join(pool, "payload", ".oxide-holding", row["name"])
    expect(tree_present(held), f"the refused tree is not in the holding area: {held}")
    expect(
        stat.S_ISFIFO(os.lstat(os.path.join(held, "build", "planted", "pipe")).st_mode),
        "the refused item is not the planted FIFO",
    )
    # Recorded status reports it without a walk or a deletion. It is a fixture
    # command like any other, so its launch is marked before it starts.
    work_left()
    result = run_fixture(
        [subject, "status"], env=env, capture_output=True, text=True, timeout=work_left()
    )
    expect(result.returncode == 0, f"status exited {result.returncode}")
    text = f"{result.stdout}{result.stderr}"
    expect(
        f"slot {row['slot']} held for deletion" in text,
        f"status did not report the held tree: {text[-2000:]}",
    )
    expect(
        "recording" in text or "held for deletion" in text,
        "status did not describe the held tree",
    )
    # Remove the refused item through its own directory: the next command's
    # pass finishes the entry and drops the row.
    os.unlink(os.path.join(held, "build", "planted", "pipe"))
    third = create_project(root, "refused-takeover")
    code, output, _ = check(subject, third, env, "takeover-after-refusal")
    expect(code == 0, f"the takeover command exited {code}: {output[-3000:]}")
    expect(
        "deleted the held slot tree" in output,
        f"the takeover did not report its deletion: {output[-2000:]}",
    )
    expect(not holding_rows(pool), f"the row survived the takeover: {holding_rows(pool)}")
    expect(not tree_present(held), "the held tree survived the takeover")
    held_facts = {
        "recorded_charge": row["bytes"],
        "reported_by_recorded_status": True,
        "takeover_deleted": True,
    }
    passed(name, **held_facts)
    return held_facts


def measure_control_admission(root: str, subject: str) -> float:
    """One comparable new-slot admission on its own pressure-free pool.

    This is the falsifying control for the in-lock gate: the measured pool's
    pressure is untouched by it, and an admission taken while a deletion holds
    the pool lock would be delayed by that whole deletion, far outside the
    bound this control establishes.
    """
    control = private_dir(os.path.join(root, "control"))
    pool = os.path.join(control, "pool")
    env = fixture_env(control, pool, os.path.join(control, "control-lock.log"), CONCURRENT_CAP)
    first = create_project(control, "control-a")
    code, output, _ = check(subject, first, env, "control-seed")
    expect(code == 0, f"the control's seed admission exited {code}: {output[-2000:]}")
    second = create_project(control, "control-b")
    code, output, elapsed = check(subject, second, env, "control-admission")
    expect(code == 0, f"the control admission exited {code}: {output[-2000:]}")
    return elapsed


def admission_row_appeared(pool: str, before: set[str]) -> bool:
    """Whether a slot row the caller did not know about is now recorded."""
    try:
        rows = slot_rows(pool)
    except (FileNotFoundError, ValueError):
        return False
    return any(str(row["id"]) not in before for row in rows)


def prove_concurrent_admission(root: str, subject: str) -> dict:
    name = "concurrent-admission"
    case(name)
    # The control first: the same kind of admission on a pool with no deletion.
    control_elapsed = measure_control_admission(root, subject)
    # Two planted idle slots under a cap that one pending tree no longer
    # exhausts: the competitor must take the pool lock (its own eviction and
    # admission) while the first command is still deleting.
    pool, env, slot, _facts = seed_big_slot(
        root,
        subject,
        "concurrent-a",
        SLOT_ENTRIES,
        cap=CONCURRENT_CAP,
        big_bytes=0,
        entry_bytes=CONCURRENT_ENTRY_BYTES,
    )
    second = create_project(root, "concurrent-b")
    code, output, _ = check(subject, second, env, "seed-concurrent-b")
    expect(code == 0, f"seeding the second planted slot exited {code}: {output[-3000:]}")
    rows = slot_rows(pool)
    expect(len(rows) == 2, f"expected two seeded slots, found {len(rows)}")
    planted_b = os.path.join(slot_path(pool, rows[1]), "build", "planted")
    write_entries(planted_b, SLOT_ENTRIES, 0, CONCURRENT_ENTRY_BYTES)
    code, output, _ = check(subject, second, env, "record-concurrent-b")
    expect(code == 0, f"recording the second planted slot exited {code}: {output[-3000:]}")
    planted_total = entry_count(planted_b) + SLOT_ENTRIES
    deleter_workspace = create_project(root, "concurrent-deleter")
    competitor_workspace = create_project(root, "concurrent-competitor")
    deleter = start_check(subject, deleter_workspace, env, "deleter")
    row = wait_for_holding_row(pool)
    expect(row is not None, "the deleter never recorded a holding row")
    held = os.path.join(pool, "payload", ".oxide-holding", row["name"])
    # Let the deleter take custody and make measurable progress: from here the
    # entry is provably mid-deletion and another command cannot claim it.
    time.sleep(CONCURRENT_CUSTODY_S)
    expect(
        deleter.poll() is None,
        "the deleter finished before the competitor started; the case is vacuous",
    )
    first_sample = (time.perf_counter(), entry_count(held))
    time.sleep(PROGRESS_INTERVAL_S)
    second_sample = (time.perf_counter(), entry_count(held))
    expect(
        0 < second_sample[1] < first_sample[1] < planted_total,
        f"the deletion made no measurable progress while the deleter held "
        f"custody ({first_sample[1]} then {second_sample[1]} of {planted_total} "
        "entries left)",
    )
    remaining_before = second_sample[1]
    progressed = first_sample[1] - second_sample[1]
    progress_rate = progressed / max(second_sample[0] - first_sample[0], 0.001)
    slots_before = {str(entry["id"]) for entry in slot_rows(pool)}
    competitor = start_check(subject, competitor_workspace, env, "competitor")
    admission_started = time.perf_counter()
    admission_completed = None
    while admission_completed is None:
        work_left()
        if admission_row_appeared(pool, slots_before):
            admission_completed = time.perf_counter()
            break
        if competitor.poll() is not None:
            fail("the competitor exited without admitting a slot")
        if time.perf_counter() - admission_started > 120:
            fail("the competitor's admission did not complete inside its window")
        time.sleep(0.005)
    # The oracle: this admission completed while the held tree was positively
    # still being deleted, which an in-lock deletion could not allow.
    deleter_alive = deleter.poll() is None
    remaining_at_admission = entry_count(held) if tree_present(held) else 0
    admission_elapsed = admission_completed - admission_started
    expect(
        deleter_alive,
        "the deleting command had already exited when the competitor admitted",
    )
    expect(
        remaining_at_admission > 0 and tree_present(held),
        f"the held tree was already gone at the competitor's admission "
        f"({remaining_at_admission} entries left)",
    )
    bound = max(
        INLOCK_CONTROL_FACTOR * control_elapsed, control_elapsed + INLOCK_CONTROL_SLACK_S
    )
    expect(
        admission_elapsed <= bound,
        f"the admission took {admission_elapsed:.2f}s while a deletion was in "
        f"progress; the in-lock control bound is {bound:.2f}s "
        f"(control {control_elapsed:.2f}s)",
    )
    code, output = finish(competitor, "competitor")
    expect(code == 0, f"the concurrent admission exited {code}: {output[-3000:]}")
    code, delet_output = finish(deleter, "deleter")
    expect(code == 0, f"the deleting command exited {code}: {delet_output[-3000:]}")
    expect(not holding_rows(pool), "a holding row survived the deleting command")
    expect(
        not tree_present(slot_path(pool, slot)),
        "the evicted slot directory reappeared in the slots root",
    )
    concurrent_facts = {
        "pool_cap": CONCURRENT_CAP,
        "control_admission_s": round(control_elapsed, 3),
        "inlock_bound_s": round(bound, 3),
        "admission_during_deletion_s": round(admission_elapsed, 3),
        "progress_samples": [first_sample[1], second_sample[1]],
        "progress_entries": progressed,
        "progress_entries_per_s": round(progress_rate, 1),
        "remaining_before": remaining_before,
        "remaining_at_admission": remaining_at_admission,
        "deleter_alive_at_admission": deleter_alive,
        "planted_entries": planted_total,
    }
    passed(name, **concurrent_facts)
    return concurrent_facts


def prove_killed_deleter_takeover(root: str, subject: str) -> dict:
    name = "killed-deleter-takeover"
    case(name)
    pool, env, slot, _facts = seed_big_slot(root, subject, "killed", SLOT_ENTRIES)
    victim = slot_path(pool, slot)
    killer_workspace = create_project(root, "killed-deleter")
    deleter = start_check(subject, killer_workspace, env, "killed-deleter")
    row = wait_for_holding_row(pool)
    expect(row is not None, "the deleter never recorded a holding row")
    # Kill only the child this proof started, and confirm it was reaped before
    # trusting what it left behind. Its descendants stay in the shared
    # supervisor's session, which this proof never signals.
    deleter.kill()
    reaped = discard(deleter, "killed-deleter")
    expect(reaped, "the killed deleter could not be confirmed reaped")
    say(f"killed the deleter (exit {deleter.returncode})")
    work_left()
    rows = holding_rows(pool)
    expect(len(rows) == 1, f"the killed deleter left no recorded row: {rows}")
    expect(rows[0]["name"] == row["name"], "the recorded row changed")
    expect(rows[0]["bytes"] > 0, "the surviving row carries no charge")
    expect(not tree_present(victim), "the slot directory returned to the slots root")
    held = os.path.join(pool, "payload", ".oxide-holding", row["name"])
    present = tree_present(held)
    taker = create_project(root, "killed-takeover")
    code, output, _ = check(subject, taker, env, "takeover")
    expect(code == 0, f"the takeover command exited {code}: {output[-3000:]}")
    expect(
        "deleted the held slot tree" in output,
        f"the takeover did not report its deletion: {output[-2000:]}",
    )
    expect(not holding_rows(pool), f"the row survived the takeover: {holding_rows(pool)}")
    expect(not tree_present(held), "the held tree survived the takeover")
    killed_facts = {
        "killed_exit": deleter.returncode,
        "tree_present_after_kill": present,
        "next_command_finished_it": True,
    }
    passed(name, **killed_facts)
    return killed_facts


# --------------------------------------------------------------------------
# Receipt and dispatch.


def receipt_path(scratch: str, qualified: bool) -> str:
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE") or scratch
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    suffix = "counted" if qualified else "diagnostic"
    return os.path.join(evidence, f"holding-delete-{suffix}-{stamp}-{os.getpid()}.json")


def write_receipt(receipt: dict, scratch: str) -> str:
    path = receipt_path(scratch, receipt.get("qualified", False))
    private_dir(os.path.dirname(path))
    data = json.dumps(receipt, indent=2, sort_keys=True)
    if len(data) > MAX_RECEIPT_BYTES:
        data = json.dumps(
            {"qualified": receipt.get("qualified", False), "error": "receipt over bound"},
            indent=2,
        )
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(data)
    os.chmod(path, 0o600)
    return path


def run_proof(scratch: str, commit: str, tree: str, runner_sha: str, subject: str) -> None:
    fixture = private_dir(os.path.join(scratch, "fixtures"))
    run_cases()


def collect(
    cases: list[tuple[str, object]],
    root_parent: str,
    keep_going: bool,
) -> dict:
    """Run one case table under the reported collection rules.

    Counted mode (`keep_going` false) is fail-fast. Diagnostic mode keeps going
    only while descendant writer closure stays provable: a recoverable oracle
    failure in a case that launched **no** fixture command may continue, because
    nothing of that case is running. A case that launched one — synchronously or
    asynchronously, marked before the launch — aborts the run even after its
    leader is reaped: a command's descendants are outside this proof's signalling
    authority, so their closure is the qualified shared supervisor's census and
    this script cannot conclude it. An unclassified outcome always aborts. Every
    case not reached stays `unexecuted`; a case that reached an unclassified
    outcome is `aborted`; neither is ever a pass.
    """
    global CASE_STARTED_CHILD
    facts: dict = {}
    started = time.perf_counter()
    for index, (name, prove) in enumerate(cases):
        root = private_dir(os.path.join(root_parent, "case", f"{index}-{name}"))
        CASE_STARTED_CHILD = False
        case(name)
        try:
            outcome = prove(root, SUBJECT)
            custody = child_faults()
            expect(custody is None, custody or "")
            facts[name] = outcome if isinstance(outcome, dict) else {}
            mark(name, "passed", **facts[name])
        except AssertionError as error:
            record_fault(name, str(error))
            mark(name, "failed", error=str(error)[:400])
            started_child = CASE_STARTED_CHILD
            reaped = close_children()
            if not keep_going:
                unexecuted([other for other, _ in cases[index + 1 :]])
                raise
            if started_child or not reaped:
                record_fault(
                    "custody",
                    "a case that launched a fixture command failed: descendant "
                    "closure belongs to the shared supervisor's census, so this "
                    "diagnostic does not continue",
                )
                unexecuted([other for other, _ in cases[index + 1 :]])
                raise
        except BaseException as error:  # noqa: BLE001 - unclassified outcome
            fault = f"{name}: unclassified outcome {type(error).__name__}: {error}"
            record_fault(name, fault)
            mark(name, "aborted", error=fault[:400])
            close_children()
            unexecuted([other for other, _ in cases[index + 1 :]])
            raise
    facts["elapsed_s"] = round(time.perf_counter() - started, 3)
    return facts


def run_cases() -> None:
    """Run the declared proof cases."""
    declare(DECLARED_CASES)
    cases = [
        ("evict-under-lock", prove_evict_under_lock),
        ("refused-entry-kept", prove_refused_entry_kept),
        ("concurrent-admission", prove_concurrent_admission),
        ("killed-deleter-takeover", prove_killed_deleter_takeover),
    ]
    FACTS.update(
        collect(cases, os.environ["OXIDE_TASK_SCRATCH_ROOT"], DIAGNOSTIC)
    )


SUBJECT = ""
FACTS: dict = {}


def main(argv: list[str]) -> int:
    global DIAGNOSTIC, SUBJECT
    mode, error = parse_arguments(argv)
    if mode == "refuse":
        print(f"holding-delete: {error}", file=sys.stderr)
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
    DIAGNOSTIC = mode == "diagnostic"
    runner = os.path.join(REPO, "scripts", "bounded-test.py")
    runner_sha = sha256_file(runner)
    expect(
        runner_sha == EXPECTED_RUNNER_SHA256,
        f"supervisor {runner_sha} is not the qualified {EXPECTED_RUNNER_SHA256}",
    )
    dirty = git("status", "--porcelain")
    expect(dirty == "", f"the worktree is not clean: {dirty[:400]}")
    commit = git("rev-parse", "HEAD")
    tree = git("rev-parse", "HEAD^{tree}")
    branch = git("branch", "--show-current")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT") or fail(
        "OXIDE_TASK_SCRATCH_ROOT is not set"
    )
    scratch = os.path.abspath(scratch)
    expect(
        scratch.startswith("/private/tmp/") or scratch.startswith("/tmp/"),
        f"scratch root {scratch} is not a private temporary directory",
    )
    expect(os.path.isdir(scratch), f"scratch root {scratch} is not a directory")
    print(
        f"host: project={git('remote', 'get-url', 'origin')} commit={commit} "
        f"branch={branch} cpu={os.cpu_count()} load={os.getloadavg()}"
    )
    started = time.perf_counter()
    fault = None
    try:
        SUBJECT = stage_subject(scratch)
        say(f"staged subject {sha256_file(SUBJECT)}")
        run_cases()
    except AssertionError as error:
        fault = str(error)
        record_fault("proof", fault)
    except subprocess.TimeoutExpired as error:
        fault = f"a fixture command exceeded the remaining work window: {error}"
        record_fault("proof", fault)
    except BaseException as error:  # noqa: BLE001 - unclassified outcome
        fault = f"unclassified outcome {type(error).__name__}: {error}"
        record_fault("proof", fault)
    finally:
        custody_closed = close_children()
        if not custody_closed and fault is None:
            fault = "a fixture group could not be confirmed closed"
            record_fault("proof", fault)
    # Every case the run did not reach stays `unexecuted`, and a case that
    # reached an unclassified outcome is `aborted`: neither is a pass.
    if fault is not None:
        for entry in CASES:
            if entry["result"] == "reached":
                entry["result"] = "aborted"
    tally = {
        result: sum(1 for entry in CASES if entry["result"] == result)
        for result in ("passed", "failed", "aborted", "unexecuted", "reached")
    }
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE") or scratch
    receipt = {
        "task": "oxide#190",
        "selector": SELECTOR + (":diagnostic" if DIAGNOSTIC else ""),
        "qualified": bool(not DIAGNOSTIC and fault is None),
        "mode": "diagnostic" if DIAGNOSTIC else "counted",
        "activation": (
            "Uncounted diagnostic collection: it is never the counted evidence, "
            "and no native activation is claimed by it. The counted author "
            "native and the independent reviewer native are separate "
            "owner-gated activations."
            if DIAGNOSTIC
            else "Counted author native run under its own owner activation. The "
            "independent reviewer native is a separate activation and is not "
            "evidence here."
        ),
        "supervisor": {
            "path": "scripts/bounded-test.py",
            "sha256": runner_sha,
            "role": "the qualified shared supervisor; its own receipt, the "
            "caller's exit and the session closure stay separately "
            "authoritative",
        },
        "provenance": {
            "scratch_root": scratch,
            "evidence_dir": evidence,
            "branch": branch,
            "worktree_clean_at_start": True,
            "staging_env": "encoded Cargo forms dropped; RUSTFLAGS, RUSTDOCFLAGS, "
            "RUSTC, RUSTC_WRAPPER and RUSTC_WORKSPACE_WRAPPER fixed; target and "
            "build inside the monitored scratch and deleted after staging",
            "fixture_env": "private HOME, CARGO_HOME, OXIDE_POOL, OXIDE_POOL_CAP, "
            "TMPDIR/TMP/TEMP per fixture tree; the invoking RUSTUP_HOME passed "
            "explicitly with RUSTUP_TOOLCHAIN pinned",
        },
        "case_tally": tally,
        "custody": {
            "fixture_children": "every fixture command stays in the qualified "
            "shared supervisor's own session and process group; this proof "
            "creates no session and never signals a group",
            "reaping": "each child is waited for, or killed by its own PID and "
            "reaped, and only a positive reap is closure; an unreaped child is "
            "unknown custody and ends a diagnostic run, and it stays tracked",
            "descendants": "descendants of a fixture command (Oxide's own Cargo "
            "group) are outside this proof's signalling authority; their closure "
            "is the shared supervisor's census, and no closure of theirs is "
            "claimed here",
            "keep_going": "a recoverable oracle failure continues the diagnostic "
            "only when the failed case never started a fixture child; once a "
            "case that ran a command has failed, reaping its leader proves "
            "nothing about its descendants, so the run aborts and the rest stay "
            "unexecuted",
            "unreaped": [label for label, _ in CHILDREN],
        },
        "source_commit": commit,
        "source_tree": tree,
        "binary_sha256": sha256_file(SUBJECT) if SUBJECT and os.path.isfile(SUBJECT) else None,
        "script_sha256": sha256_file(os.path.abspath(__file__)),
        "guard_sha256": runner_sha,
        "command": COMMAND,
        "limit_s": WALL_LIMIT_S,
        "scratch_budget_mib": SCRATCH_BUDGET_MIB,
        "shutdown_margin_s": SHUTDOWN_MARGIN_S,
        "cases": CASES,
        "facts": FACTS,
        "first_fault": fault or FIRST_FAULT,
        "faults": FAULTS,
        "deadline_left_s": deadline_remaining(),
        "elapsed_s": round(time.perf_counter() - started, 3),
        "lock_metric": "the whole eviction transaction under the pool lock "
        "(victim and rank checks, identity-verified rename, one durable ledger "
        "write), from the product's debug-only observation",
        "unmeasured": [
            "a real project's slot rate",
            "removable media",
            "the retired per-entry journaled path, which is not executable here",
        ],
        "authority": "This receipt records only this helper's own case oracles. "
        "The guard's own receipt, the caller's exit and the session closure "
        "remain separately authoritative.",
        "live_pool": False,
        "mounted_volume": False,
        "result": "pass" if not DIAGNOSTIC and fault is None else "unqualified",
    }
    path = write_receipt(receipt, scratch)
    say(f"helper receipt {path}")
    print(json.dumps(receipt, indent=2, sort_keys=True), flush=True)
    if DIAGNOSTIC:
        # An uncounted collection is never a counted pass, even with no fault.
        return 0 if fault is None else 1
    return 0 if fault is None else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
