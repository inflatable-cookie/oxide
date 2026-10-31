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
  **whole eviction-deciding pool-lock interval** — from that command's lock
  acquisition to the eviction's row write, so it includes the byte-pressure
  decision and its recorded-total walks, the victim selection, the intent write,
  the identity-verified rename and the row write; it must be well under 1 s while
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
  recorded and charged; the next command's pre-admission pass claims it and
  finishes it. That command is a warm reuse of the fixture's own idle slot, so
  its own capacity decision cannot retire the tree the oracle requires to
  survive (the fixture is over its cap on purpose), and the case also proves the
  pool's slot membership is unchanged by the takeover.

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
EXPECTED_RUNNER_SHA256 = "77e87c6edfcef99ccabcfb5033e82079854b2cba20b3a65992b828c604694453"
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
# A held tree stays charged until it is positively removed, so a byte-pressure
# admission that evicts cannot admit in the same attempt: the command deletes the
# pending tree outside the lock and retries once. The pending-tree cases therefore
# plant one large victim tree and one small competitor tree under a cap that only
# the pair exceeds.
# The fixture is **asymmetrical**, because the in-lock control only discriminates
# when the pending deletion outlasts its bound: the bound is
# `max(3 x control, control + 2 s)`, so the victim must delete for noticeably more
# than two seconds even though a 20,000-entry tree finishes in about one. The
# victim therefore carries `PENDING_VICTIM_ENTRIES` cheap entries (well above the
# brief's 20,000-entry floor) and the competitor's own reusable slot stays small,
# which keeps the whole fixture inside the 512 MiB monitored scratch.
#
# The cap sits strictly between the victim's own charge and the pair's sum, so both
# seeds record without pressure and only the evicting admission must retire the
# victim. Every half of that arithmetic is asserted in the pure self-check.
PENDING_VICTIM_ENTRIES = 70_000
PENDING_REUSABLE_ENTRIES = 8_000
PENDING_ENTRY_BYTES = 1024
PENDING_CAP = "290MiB"
# Each 1 KiB entry occupies one filesystem block, so a tree's recorded figure is
# its entry count times this.
PENDING_BLOCK_BYTES = 4096
PENDING_TREES = 2
# Per-tree allowance for the bucket directories and the tiny crate's own output,
# and the staging allowance, used only by the pure self-check's sizing arithmetic.
PENDING_TREE_OVERHEAD_BYTES = 1024 * 1024
PENDING_STAGING_BYTES = 64 * MIB
# Conservative floor for the deletion rate this fixture is sized against: the
# measured figure on an idle internal volume is about 20,000 unlinks/s, so sizing
# against 15,000 keeps the premise honest on a slower run.
PROGRESS_FLOOR_UNLINKS_PER_S = 15_000
# Entries one planted bucket holds (`write_entries` batches 500 per directory).
PENDING_BUCKET_ENTRIES = 500
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
# Progress is measured by bounded sampling rather than one fixed sleep: a 78 MiB
# tree on a fast internal volume can vanish inside a long interval. Sampling every
# `PROGRESS_SAMPLE_S` stops at the first proven decrease, while most of the tree
# is still there, which is also the point at which the competitor starts.
PROGRESS_SAMPLE_S = 0.02
PROGRESS_BUDGET_S = 60.0
# Buckets that must be gone before a rate is estimated at all: with one bucket per
# 20 ms tick a single-tick window overstates the rate roughly fivefold, and three
# buckets make the window span several real bucket intervals.
PROGRESS_MIN_BUCKETS = 3
MAX_RECEIPT_BYTES = 64 * 1024
SELECTOR = "rust:holding-delete"
# The nine items of the fixed proof-authoring premise checklist
# (docs/knowledge/contracts/workflow.md#proof-authoring-premise-checklist), each
# with the executable source location that satisfies it or the concrete reason it
# does not apply. The pure self-check binds this map to the source it names, so a
# disposition cannot drift into a claim no case body supports, and `--plan`
# prints it before any activation.
PREMISE_CHECKLIST = {
    "1_private_home_and_rustup": (
        "invoking_rustup_home() (explicit RUSTUP_HOME, else the invoking HOME/.rustup, "
        "read before any private HOME exists) is passed to stage_subject() and "
        "fixture_env() with RUSTUP_TOOLCHAIN pinned to " + BUILDER + " and "
        "RUSTUP_AUTO_INSTALL=0. The only `--locked` call is staging, which builds "
        "this repository against its committed Cargo.lock; the fixture crate "
        "create_project() writes is dependency-free and no fixture invocation passes "
        "`--locked` (they use `--offline` and the fixture's own Cargo-generated lock "
        "inside the fixture tree), so no mode needs a pre-seeded lock and no toolchain "
        "is installed or downloaded"
    ),
    "2_account_home_control": (
        "fixture_env() sets OXIDE_TEST_HOME_FROM_ENV=1 beside the private HOME, so the "
        "product honours that HOME instead of the account home"
    ),
    "3_environment_isolation": (
        "stage_subject() drops every CARGO_TARGET_/CARGO_BUILD_/CARGO_ENCODED_/SCCACHE_/"
        "OXIDE_/XDG_ key plus CARGO/RUSTC/RUSTFLAGS/RUSTDOCFLAGS/RUSTC_WRAPPER/"
        "RUSTC_WORKSPACE_WRAPPER, and fixture_env() builds the child environment from a "
        "fixed allow-list (private HOME, CARGO_HOME, OXIDE_POOL, OXIDE_POOL_CAP, "
        "TMPDIR/TMP/TEMP, the toolchain home, the bounded test controls) so no host "
        "pool, config or account provenance is inherited"
    ),
    "4_automatic_adoption": (
        "not applicable: every fixture is a leased command on a private "
        "directory-budget pool and none places or reads permanent outputs, so no "
        "checkout can be automatically adopted and no case needs one created"
    ),
    "5_cargo_external_argv": (
        "not applicable: no case invokes an external Cargo subcommand; the fixtures "
        "run `oxide run`/`oxide test` and the staged candidate's own commands only"
    ),
    "6_owned_images": (
        "not applicable: the fixture is an internal directory-budget pool, with no "
        "sparse image, attach, mount or removable device anywhere in the script"
    ),
    "7_compiler_flags": (
        "stage_subject() freezes RUSTFLAGS=-C strip=symbols with assertion-on, "
        "debug-free profile settings and drops every encoded CARGO_ENCODED_* form, so "
        "an inherited encoded value cannot override the staged candidate"
    ),
    "8_temporary_storage": (
        "the selector's fixed COMMAND runs the qualified shared supervisor with "
        "--root-parent /private/tmp (the owner-accepted internal-placement exception "
        "bd357a4c, which the fixture's genuine internal directory-budget pool needs), "
        "every case root comes from an explicit path under that parent, and "
        "fixture_env() gives each fixture tree its own TMPDIR/TMP/TEMP inside it"
    ),
    "9_macos_mktemp": (
        "the script never runs a shell `mktemp`; self_check() creates its two "
        "temporary directories with `tempfile.mkdtemp(prefix=...)`, which honours the "
        "supervisor's TMPDIR, and removes both before it returns"
    ),
}
NA_PREMISES = ("4_automatic_adoption", "5_cargo_external_argv", "6_owned_images")
COMMAND = [
    "python3",
    "scripts/bounded-test.py",
    "--root-parent",
    "/private/tmp",
    "--limit",
    str(WALL_LIMIT_S),
    "--scratch-budget-mib",
    str(SCRATCH_BUDGET_MIB),
    "--",
    "python3",
    "scripts/test-holding-delete.py",
]
# The root the fixture's genuine internal directory-budget pool needs (the
# owner-accepted internal-placement exception bd357a4c). The fixed command, the
# plan, the receipt and `effigy.toml`'s selector all carry it, and the pure
# self-check binds the three records together.
ROOT_PARENT = "/private/tmp"
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
                f"the whole eviction-deciding pool-lock interval — from that "
                f"command's lock acquisition to the eviction's row write, "
                f"including the byte-pressure decision, the victim selection, "
                f"the rename and the row write — is under {LOCK_CEILING_MS} ms, the "
                "tree is deleted outside the lock by that command, and no "
                "holding row survives",
            },
            {
                "name": "refused-entry-kept",
                "oracle": "an evicted tree holding a FIFO is refused, stays in "
                "the holding area with its recorded charge and its row; the "
                "admission that evicted it refuses with a bounded capacity "
                "result because those bytes are still charged; recorded status "
                "reports the held tree; and the next command after the item is "
                "removed finishes it",
            },
            {
                "name": "concurrent-admission",
                "oracle": "two recorded idle trees exceed the pool cap while one "
                "of them alone fits under it, so the eviction admission must "
                "retire the older tree, refuse while its row is charged, and "
                "delete that row in its own outside-the-lock pass; two "
                "entry-count samples of that held tree taken while only the "
                "evicting command runs strictly decrease (measured progress under "
                "its non-blocking custody), then the competitor's own warm reuse "
                "of its separate idle slot (observed by that slot's ledger row "
                "becoming held) completes with the evicting command still alive "
                "and the held tree still non-empty, and stays inside "
                f"{INLOCK_CONTROL_FACTOR}x the same reuse measured on a "
                f"pressure-free pool plus {INLOCK_CONTROL_SLACK_S}s. An in-lock "
                "deletion would delay it by the whole deletion and fail the "
                "control; the control's own numbers are recorded.",
            },
            {
                "name": "killed-deleter-takeover",
                "oracle": "a SIGKILLed deleter leaves its row recorded and "
                "charged; the killed command is confirmed reaped and its partly "
                "deleted tree survives; the next command, a warm reuse of the "
                "fixture's own idle slot (no capacity decision), reports and "
                "finishes that entry, leaves no holding row, keeps the pool's "
                "slot membership unchanged and preserves that slot's tree",
            },
        ],
        "phases": [
            {"name": "stage", "allocation": "this HEAD's debug oxide, plain "
             "Cargo offline, target/build inside scratch, target deleted"},
            {"name": "evict", "allocation": f"one fixture root, one {SLOT_ENTRIES}-entry tree"},
            {"name": "refused", "allocation": "one fixture root, one small tree plus a FIFO"},
            {"name": "concurrent", "allocation": "one fixture root, one "
             f"{PENDING_VICTIM_ENTRIES}-entry victim tree and one "
             f"{PENDING_REUSABLE_ENTRIES}-entry competitor tree of "
             f"{PENDING_ENTRY_BYTES} bytes per entry under a {PENDING_CAP} cap "
             "(the victim alone under the cap, the pair over it)"},
            {"name": "takeover", "allocation": f"one fixture root, one {SLOT_ENTRIES}-entry tree"},
        ],
        "disk": {
            "monitored_scratch_mib": SCRATCH_BUDGET_MIB,
            "fixture_roots": 4,
            "per_fixture": f"{SLOT_ENTRIES} planted entries with one "
            f"{SLOT_BYTES // MIB} MiB entry, or a "
            f"{PENDING_VICTIM_ENTRIES}-entry victim tree beside a "
            f"{PENDING_REUSABLE_ENTRIES}-entry competitor tree for the "
            "eviction/competitor cases, plus tiny std-only crate intermediates",
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
            "progress_sample_s": PROGRESS_SAMPLE_S,
            "progress_budget_s": PROGRESS_BUDGET_S,
        },
        "pending_row_premise": "the fixture is asymmetrical: a victim tree well "
        "above the brief's 20,000-entry floor (sized so its deletion outlasts the "
        "bound's fixed slack even at a conservative unlink-rate floor) and a small "
        "competitor tree, under a cap the victim alone stays below and only the "
        "pair exceeds, so both seeds record without pressure and exactly one "
        "eviction is forced; the cap window, the scratch peak and the "
        "discriminating window (including a counterfactual where the old "
        "20,000-entry victim fails it) are asserted in the pure self-check",
        "progress_observation": f"bounded sampling every {PROGRESS_SAMPLE_S}s "
        "of one cheap `listdir` count of the held tree's planted bucket "
        "directories (never a full 20,000-entry walk), estimated only once at "
        f"least {PROGRESS_MIN_BUCKETS} buckets are gone and then as the average "
        "rate over the whole first-to-last window, so one 20 ms tick cannot "
        "satisfy the rate; the competitor starts while most buckets remain, and "
        "the case also requires the estimated remaining deletion time to exceed "
        "the in-lock bound it enforces, which is what makes that control "
        "falsifying",
        "boundaries": "the competitor's admission window is recorded as exact "
        "offsets, with the held bucket count and the recorded holding row read at "
        "both the start and the completion instant",
        "observations": {
            "lock_held": "the product's debug-only OXIDE_TEST_HOLDING_LOCK_LOG "
            "line per eviction, armed only inside the supervisor's scratch root. "
            "The figure is the whole eviction-deciding pool-lock interval, timed "
            "from that open's lock acquisition to the eviction's row write: the "
            "fence, the ledger read, reconciliation, admission bookkeeping, the "
            "byte-pressure decision and the recorded-total walks that feed it, "
            "the victim selection, the intent write, the identity-verified "
            "rename and the row write. It is the lock hold the design promises "
            "is under a second, not an inner fragment, and a command that "
            "evicts more than once under one lock records a figure that also "
            "covers the evictions before it",
            "throughput": "entries/second and MiB/second of the outside-lock "
            "traversal, measured from the deleted figures and elapsed time",
            "legacy_reference": "the retired per-entry journaled path is not "
            "executable here; its measured 2026-10-29 figure (about 60 "
            "entries/s, about 16.7 ms per entry, under the pool lock) is a "
            "labelled historical reference, not a live comparison",
        },
        "premise_checklist": {
            "source": "docs/knowledge/contracts/workflow.md#proof-authoring-premise-checklist",
            "items": PREMISE_CHECKLIST,
            "not_applicable": list(NA_PREMISES),
            "note": "recorded before any activation; the required uncounted "
            "diagnostic, the counted author native and the reviewer native remain "
            "separately owner-gated activations, so this plan claims none of them",
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
        plan["limits"]["progress_sample_s"] == 0.02
        and plan["limits"]["progress_budget_s"] == 60.0,
        "the bounded progress sampling changed",
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
    # The figure is the whole eviction-deciding lock interval, so the plan must
    # say that instead of narrowing it to the rename transaction.
    expect(
        "whole eviction-deciding pool-lock interval" in plan["observations"]["lock_held"]
        and "byte-pressure decision" in plan["observations"]["lock_held"],
        "the plan must state that the timed interval is the whole "
        "eviction-deciding lock hold, byte-pressure decision included",
    )
    expect(
        list(PREMISE_CHECKLIST) == [
            "1_private_home_and_rustup",
            "2_account_home_control",
            "3_environment_isolation",
            "4_automatic_adoption",
            "5_cargo_external_argv",
            "6_owned_images",
            "7_compiler_flags",
            "8_temporary_storage",
            "9_macos_mktemp",
        ],
        "the premise checklist must record all nine items in order",
    )
    source = read_text(os.path.abspath(__file__))
    selector_source = read_text(os.path.join(REPO, "effigy.toml"))
    for item, disposition in PREMISE_CHECKLIST.items():
        expect(bool(disposition.strip()), f"premise {item} has no disposition")
        if item in NA_PREMISES:
            expect(
                disposition.startswith("not applicable:"),
                f"premise {item} is declared inapplicable without a reason",
            )
        else:
            # Every applicable disposition names a helper the script really defines.
            named = re.findall(r"\b([a-z_][a-z0-9_]*)\(\)", disposition)
            expect(bool(named), f"premise {item} names no executable location")
            for helper in named:
                expect(
                    f"def {helper}(" in source,
                    f"premise {item} names {helper}(), which this script does not define",
                )
    expect(
        plan["premise_checklist"]["items"] == PREMISE_CHECKLIST
        and plan["premise_checklist"]["not_applicable"] == list(NA_PREMISES),
        "the plan must print the recorded premise dispositions",
    )
    # Item 1's lock claim, bound to the two places it can drift: the fixture
    # manifest (dependency-free, so Cargo can generate its own lock offline) and
    # the one `--locked` call (staging, against this repository's committed lock).
    # Each region is located by a line-anchored definition, so this block's own
    # text can never satisfy the check it states.
    def body_of(name: str) -> str:
        match = re.search(rf"^def {name}\(", source, re.MULTILINE)
        expect(match is not None, f"the script must define {name}")
        return re.split(r"^def ", source[match.end():], maxsplit=1, flags=re.MULTILINE)[0]

    fixture_body = body_of("create_project")
    expect(
        "[dependencies]" not in fixture_body and "Cargo.lock" not in fixture_body,
        "the fixture manifest must stay dependency-free and ship no lock of its own, "
        "or premise item 1's disposition is false",
    )
    staging_body = body_of("stage_subject")
    marker = re.search(r"^# The proof\. Everything below has effects", source, re.MULTILINE)
    expect(marker is not None, "the effectful region marker is missing")
    effectful = source[marker.start():]
    expect(
        effectful.count('"--locked"') == 1 and '"--locked"' in staging_body,
        "the only --locked Cargo call must be staging against the committed lock, "
        "which is what premise item 1 records",
    )
    # The killed-deleter case's later command must stay a warm reuse of the
    # fixture's own idle slot: the fixture is over its cap on purpose, so a new
    # admission would legitimately evict the tree that case's oracle requires to
    # survive, and the case would then prove the fixture's arithmetic instead of
    # the takeover. Bound to the case body text so the correction cannot regress.
    killed_body = body_of("prove_killed_deleter_takeover")
    expect(
        'check(subject, reusable_ws, env, "takeover")' in killed_body,
        "the killed-deleter case must run its takeover as a warm reuse of the "
        "fixture's own idle slot",
    )
    expect(
        "killed-takeover" not in killed_body,
        "the killed-deleter case must not create a new project for the takeover: "
        "a new admission takes a capacity decision the fixture's cap forces onto "
        "the tree the oracle requires to survive",
    )
    expect(
        "len(members_after) <= len(members_before)" in killed_body,
        "the killed-deleter case must prove the takeover added no slot",
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
        "--root-parent" in COMMAND
        and COMMAND[COMMAND.index("--root-parent") + 1] == ROOT_PARENT,
        "the fixed command must carry the internal-placement root its fixture pool "
        "needs, which is what premise item 8 records",
    )

    def selector_command(name: str) -> str:
        block = re.search(
            rf'^\[tasks\."{re.escape(name)}"\]\n((?:[^\[].*\n)*)',
            selector_source,
            re.MULTILINE,
        )
        expect(block is not None, f"effigy.toml must define {name}")
        run = re.search(r'^run = "(.*)"$', block.group(1), re.MULTILINE)
        expect(run is not None, f"{name} must declare a run command")
        return run.group(1)

    expect(
        selector_command("rust:holding-delete") == " ".join(COMMAND),
        "the counted selector must run exactly the fixed command the plan and "
        "receipt name, or the receipt records a command nobody runs",
    )
    expect(
        selector_command("rust:holding-delete:diagnostic")
        == " ".join(COMMAND) + " --diagnostic",
        "the diagnostic selector must be the same command with --diagnostic appended",
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
    # Every constant the case bodies and the plan use must exist: a deleted
    # constant that is still referenced aborted a diagnostic run with NameError
    # before any case (the apparatus regression this check exists for).
    source = read_text(os.path.abspath(__file__))
    module = globals()
    referenced = set()
    for name, value in list(module.items()):
        if callable(value) and getattr(value, "__module__", None) == "__main__":
            code = getattr(value, "__code__", None)
            if code is not None:
                referenced.update(code.co_names)
    # Attribute lookups (`os.X`, `signal.X`, `stat.S_IFDIR`) also appear as
    # names; only a bare module-level constant of this file can be undefined
    # here, so the check is restricted to the file's own upper-case definitions.
    declared = set()
    for line in read_text(os.path.abspath(__file__)).splitlines():
        match = re.match(r"^([A-Z][A-Z0-9_]*)(?::[^=]*)?\s*=", line)
        if match:
            declared.add(match.group(1))
    missing = sorted(name for name in declared if name not in module)
    expect(
        not missing,
        f"the proof references undefined constants: {missing}",
    )
    # The concurrency case's own apparatus resolves: its cap, entry count and
    # control pool are the pending-tree shape, not a deleted constant.
    expect(
        PENDING_VICTIM_ENTRIES > 0
        and PENDING_REUSABLE_ENTRIES > 0
        and PENDING_ENTRY_BYTES > 0,
        "pending shape changed",
    )
    expect(
        PENDING_CAP.endswith("MiB"),
        f"the concurrency cap is not a size: {PENDING_CAP}",
    )
    # The control admission really resolves the concurrency fixture's cap (its
    # globals are looked up by name at call time, so the code names are the
    # authority here), and no retired concurrency constant survives.
    control_names = set(measure_reuse_admission.__code__.co_names)
    expect(
        "check" in control_names,
        f"the control admission does not run a fixture command: {sorted(control_names)}",
    )
    # The fixture's arithmetic premise, which two failed runs got wrong: the
    # victim alone must stay under the cap (so both seeds record without
    # pressure) while the pair exceeds it (so exactly one eviction is forced).
    expect(
        PENDING_VICTIM_ENTRIES >= 20_000,
        f"the victim is below the brief's 20,000-entry floor: {PENDING_VICTIM_ENTRIES}",
    )
    cap = parse_size(PENDING_CAP)
    victim_charge = PENDING_VICTIM_ENTRIES * PENDING_BLOCK_BYTES + PENDING_TREE_OVERHEAD_BYTES
    reusable_charge = (
        PENDING_REUSABLE_ENTRIES * PENDING_BLOCK_BYTES + PENDING_TREE_OVERHEAD_BYTES
    )
    expect(
        victim_charge < cap,
        f"the victim alone would trip the cap: {victim_charge} >= {cap}",
    )
    expect(
        victim_charge + reusable_charge > cap,
        f"the fixture never forces eviction: {victim_charge} + {reusable_charge} <= {cap}",
    )
    # Scratch: the pair plus staging stays inside the monitored 512 MiB budget,
    # even with the run's earlier fixtures and the second case's own pair held.
    expect(
        victim_charge + reusable_charge + PENDING_STAGING_BYTES
        < SCRATCH_BUDGET_MIB * MIB,
        f"the fixture does not fit the scratch budget: "
        f"{victim_charge + reusable_charge + PENDING_STAGING_BYTES}",
    )
    # Discriminating window: the deletion must outlast the bound's dominant term
    # (the fixed slack) even at the conservative rate floor, with several buckets
    # left *after* the measurement point. The old 20,000-entry victim fails this
    # check at the measured 20,000 unlinks/s; the new one passes with margin.
    for entries, expected in (
        (PENDING_VICTIM_ENTRIES, True),
        (20_000, False),
    ):
        buckets_left = entries // PENDING_BUCKET_ENTRIES - PROGRESS_MIN_BUCKETS
        remaining_s = buckets_left * PENDING_BUCKET_ENTRIES / PROGRESS_FLOOR_UNLINKS_PER_S
        dominating_bound = INLOCK_CONTROL_SLACK_S
        expect(
            (remaining_s > dominating_bound) is expected,
            f"the {entries}-entry victim's discriminating window is wrong: "
            f"{remaining_s:.2f}s against {dominating_bound:.2f}s",
        )
    expect(
        parse_size("20MiB") == 20 * 1024 * 1024,
        "the size parser changed",
    )
    # Both cases must use the deterministic eviction fixture and must observe
    # the product's own row before acting on it; the concurrency case must
    # observe the competitor's admission in the pool's own ledger rather than
    # assuming it. Looked up through the functions' own code names, because a
    # source substring could match this check itself.
    for fn in (prove_concurrent_admission, prove_killed_deleter_takeover):
        names = set(fn.__code__.co_names)
        expect(
            "seed_eviction_fixture" in names,
            f"{fn.__name__} does not use the deterministic eviction fixture",
        )
        expect(
            "start_eviction" in names,
            f"{fn.__name__} does not start the observed eviction",
        )
    expect(
        "wait_for_holding_row" in set(start_eviction.__code__.co_names),
        "the eviction starter does not positively observe the published row",
    )
    expect(
        "slot_state" in set(prove_concurrent_admission.__code__.co_names),
        "the concurrency case does not observe the competitor's admission in the ledger",
    )
    # Progress is measured by bounded sampling, and the in-lock control is
    # falsifying by construction: the case must estimate how much deletion work
    # remains and require it to exceed the bound it enforces.
    for fn in (prove_concurrent_admission, prove_killed_deleter_takeover):
        expect(
            "observe_progress" in set(fn.__code__.co_names),
            f"{fn.__name__} does not measure progress by bounded sampling",
        )
    concurrency_names = set(prove_concurrent_admission.__code__.co_names)
    expect(
        "max" in concurrency_names and "INLOCK_CONTROL_FACTOR" in concurrency_names,
        "the concurrency case does not build the in-lock bound it enforces",
    )
    # The oracle observes progress with one cheap `listdir` of planted buckets,
    # never by walking the whole tree per sample, and it reads the deletion state
    # at both ends of the admission window rather than inferring activity.
    expect(
        "planted_buckets" in concurrency_names
        and "entry_count" not in concurrency_names,
        f"the concurrency case does not use the cheap bucket counter: "
        f"{sorted(concurrency_names)}",
    )
    expect(
        "planted_buckets" in set(observe_progress.__code__.co_names),
        "the progress sampler does not use the cheap bucket counter",
    )
    # The estimator itself, on synthetic timestamped windows.
    #
    # 1. A one-tick window (one bucket in one 20 ms interval) is not an estimate:
    #    it is rejected outright, so it can never satisfy a caller.
    one_tick = [(0.0, 40), (PROGRESS_SAMPLE_S, 39)]
    expect(
        estimate_progress(one_tick) is None,
        "a one-tick window was accepted as a rate estimate",
    )
    # 2. Two buckets removed is still below the minimum.
    two_buckets = [(0.0, 40), (0.20, 39), (0.40, 38)]
    expect(
        estimate_progress(two_buckets) is None,
        f"a two-bucket window was accepted ({estimate_progress(two_buckets)!r})",
    )
    # 3. A sustained multi-bucket window gives the average rate over the whole
    #    window and the remaining time that rate implies.
    sustained = [(0.0, 40), (0.20, 38), (0.40, 36), (0.60, 34)]
    estimate = estimate_progress(sustained)
    expect(estimate is not None, "a sustained multi-bucket window was rejected")
    expect(
        estimate["progressed_buckets"] == 6
        and abs(estimate["span_s"] - 0.60) < 1e-9
        and abs(estimate["rate_per_s"] - 10.0) < 1e-9,
        f"the sustained window's progress, span or rate is wrong: {estimate}",
    )
    expect(
        abs(estimate["remaining_s"] - 3.4) < 1e-9,
        f"the remaining time is not remaining/rate: {estimate}",
    )
    # 4. The counterfactual: the old last-two-sample estimator divides one bucket
    #    by one 20 ms interval and so *falsely* concludes that too little deletion
    #    remained to distinguish an in-lock deletion; the corrected estimate, over
    #    the same tree state, is above the bound the real case enforces.
    bound = 2.27
    old_rate = (39.0 - 40.0) / -PROGRESS_SAMPLE_S
    expect(
        abs(old_rate - 50.0) < 1e-9,
        f"the counterfactual's old rate changed: {old_rate}",
    )
    old_remaining_s = 39.0 / old_rate
    expect(
        old_remaining_s < bound,
        f"the old estimator would not have failed on this tree: {old_remaining_s}",
    )
    expect(
        estimate["remaining_s"] > bound,
        f"the corrected estimate is not discriminating: {estimate}",
    )
    expect(
        estimate["remaining_s"] > 3.0 * old_remaining_s,
        f"the corrected estimate is not materially larger than the old one: "
        f"{estimate['remaining_s']} vs {old_remaining_s}",
    )
    # Both cases must use the estimator, and neither may derive a rate from the
    # last two samples itself.
    for fn in (prove_concurrent_admission, prove_killed_deleter_takeover):
        names = set(fn.__code__.co_names)
        expect(
            "observe_progress" in names,
            f"{fn.__name__} does not measure progress by bounded sampling",
        )
        expect(
            "PROGRESS_MIN_BUCKETS" in names or "estimate_progress" in names,
            f"{fn.__name__} does not require a sound multi-bucket estimate",
        )
    for evidence in ("holding_rows", "slot_state"):
        expect(
            evidence in concurrency_names,
            f"the concurrency oracle does not read {evidence} at the boundaries",
        )
    for key in (
        "buckets_at_admission_start",
        "buckets_" + "at_admission_completion",
        "admission_" + "started_offset_s",
        "admission_" + "completed_offset_s",
    ):
        expect(key in source, f"the receipt does not record the boundary {key}")
    premise = "expected_" + "remaining_s"
    expect(
        premise in source,
        "the concurrency case does not state the falsifying in-lock premise",
    )
    retired = "CONCURRENT" + "_CAP"
    expect(retired not in source, "a retired concurrency constant is still referenced")

    # The proof never creates a session and never signals a group: fixture
    # children and their descendants stay in the qualified supervisor's session.
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
    # Every held tree stays charged until it is positively removed, so an
    # admission that evicted a tree whose deletion refuses cannot admit: it
    # reports the capacity refusal instead of deciding as if those bytes were
    # free. The eviction itself still happened.
    expect(
        code != 0,
        f"an admission over budget with an undeletable held tree exited {code}: {output[-2000:]}",
    )
    expect(
        "full" in output or "cannot lease a slot" in output,
        f"the refusal does not name the capacity result: {output[-2000:]}",
    )
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


def estimate_progress(
    samples: list[tuple[float, int]],
    min_progress: int = PROGRESS_MIN_BUCKETS,
) -> dict | None:
    """The rate and remaining time a timestamped bucket window supports.

    The bucket counter is **quantized**: a bucket disappears only when its last
    entry is unlinked, so two samples one 20 ms interval apart can only ever show
    one bucket and dividing that by the interval overstates the rate by the ratio
    of the interval to a bucket's real duration. The estimate is therefore taken
    over the whole window — first to last sample — and only once at least
    `min_progress` buckets have been removed, which makes the window span several
    bucket intervals. Fewer than that is not an estimate at all (`None`), so a
    one-tick reading can never satisfy a caller.
    """
    if len(samples) < 2:
        return None
    initial = samples[0][1]
    remaining = samples[-1][1]
    progressed = initial - remaining
    span = samples[-1][0] - samples[0][0]
    if progressed < min_progress or span <= 0.0:
        return None
    rate = progressed / span
    return {
        "initial_buckets": initial,
        "remaining_buckets": remaining,
        "progressed_buckets": progressed,
        "span_s": span,
        "rate_per_s": rate,
        "remaining_s": (remaining / rate) if rate > 0.0 else float("inf"),
    }


def observe_progress(
    held: str, budget_s: float = PROGRESS_BUDGET_S
) -> tuple[list[tuple[float, int]], dict | None]:
    """Sample the cheap bucket counter until a sound rate estimate exists.

    One `listdir` per sample, never a 20,000-entry walk. Sampling stops as soon as
    `estimate_progress` accepts the window (several buckets removed, average rate
    over the whole window), when the tree is empty, or at the budget. The caller
    must treat `None` as a failed premise, never as progress.
    """
    deadline = time.perf_counter() + budget_s
    samples: list[tuple[float, int]] = []
    while time.perf_counter() < deadline:
        work_left()
        count = planted_buckets(held) if tree_present(held) else 0
        samples.append((time.perf_counter(), count))
        if count == 0:
            return samples, None
        estimate = estimate_progress(samples)
        if estimate is not None:
            return samples, estimate
        time.sleep(PROGRESS_SAMPLE_S)
    return samples, estimate_progress(samples)


def planted_buckets(held: str) -> int:
    """A cheap monotone progress counter for a held tree.

    The fixture writes its entries in `b0000`..`b0039` buckets of 500 files, and
    the deletion empties those buckets one at a time, so the number of bucket
    directories still present is a faithful progress signal that costs one
    `listdir` of at most a few dozen names. Walking all 20,000 entries per sample
    (as `entry_count` does) costs on the order of the deletion itself and makes any
    sampling window meaningless.
    """
    planted = os.path.join(held, "build", "planted")
    try:
        return len(os.listdir(planted))
    except OSError:
        return 0


def parse_size(text: str) -> int:
    """A size the fixture passes to `OXIDE_POOL_CAP` (`20MiB`, `512KiB`, `3G`)."""
    units = {"": 1, "B": 1, "K": 1024, "KB": 1024, "KIB": 1024}
    units.update({"M": 1024**2, "MB": 1024**2, "MIB": 1024**2})
    units.update({"G": 1024**3, "GB": 1024**3, "GIB": 1024**3})
    body = text.strip()
    digits = ""
    while body and (body[0].isdigit() or body[0] == "."):
        digits += body[0]
        body = body[1:]
    expect(bool(digits), f"not a size: {text}")
    expect(body.upper() in units, f"unknown size unit: {text}")
    return int(float(digits) * units[body.upper()])


def seed_recorded_tree(
    root: str,
    subject: str,
    label: str,
    workspace: str,
    env: dict,
    entries: int,
) -> tuple[dict, str]:
    """One idle whole slot with `entries` recorded entries.

    The recording walk runs on the release, after the payload is planted, so the
    closed observation the eviction charges is the planted figure.
    """
    pool = os.path.join(root, "pool")
    code, output, _ = check(subject, workspace, env, f"seed-{label}")
    expect(code == 0, f"seeding {label} exited {code}: {output[-2000:]}")
    mine = [row for row in slot_rows(pool) if row["workspace"] == workspace]
    expect(len(mine) == 1, f"expected one slot for {label}, found {mine}")
    planted = os.path.join(slot_path(pool, mine[0]), "build", "planted")
    write_entries(planted, entries, 0, PENDING_ENTRY_BYTES)
    code, output, _ = check(subject, workspace, env, f"record-{label}")
    expect(code == 0, f"recording {label} exited {code}: {output[-2000:]}")
    mine = [row for row in slot_rows(pool) if row["workspace"] == workspace]
    expect(len(mine) == 1, f"expected one recorded slot for {label}, found {mine}")
    slot = mine[0]
    expect(
        slot["state"] == "idle" and not slot.get("lease_id"),
        f"the recorded slot is not idle: {json.dumps(slot)[:300]}",
    )
    expect("recorded" in slot, f"the slot recorded no observation: {slot}")
    return slot, planted


def seed_eviction_fixture(
    root: str, subject: str, label: str
) -> tuple[str, dict, dict, dict, str, str]:
    """Two recorded idle trees whose total exceeds the cap.

    `victim` is the older idle slot (the rank-LRU eviction target) and `reusable`
    is the newer one, owned by its own project so a later warm reuse needs no
    capacity decision at all.
    """
    pool = os.path.join(root, "pool")
    env = fixture_env(root, pool, os.path.join(root, "eviction-lock.log"), PENDING_CAP)
    victim_workspace = create_project(root, f"{label}-victim")
    victim, _planted = seed_recorded_tree(
        root,
        subject,
        f"{label}-victim",
        victim_workspace,
        env,
        PENDING_VICTIM_ENTRIES,
    )
    reusable_workspace = create_project(root, "competitor")
    reusable, _ = seed_recorded_tree(
        root,
        subject,
        f"{label}-reusable",
        reusable_workspace,
        env,
        PENDING_REUSABLE_ENTRIES,
    )
    expect(
        victim["id"] != reusable["id"],
        "the fixture seeded one slot instead of two",
    )
    expect(
        int(victim["rank"]) < int(reusable["rank"]),
        f"the reusable slot is not newer than the victim: {victim} {reusable}",
    )
    return pool, env, victim, reusable, victim_workspace, reusable_workspace


def start_eviction(
    root: str,
    subject: str,
    pool: str,
    env: dict,
    victim: dict,
    label: str,
) -> tuple[subprocess.Popen, dict, str]:
    """Start the admission that must evict `victim`, and observe its row.

    The admission's *own* publish pass starts as soon as the refusal is decided,
    so the row exists for the whole deletion and is observed positively here
    rather than raced for.
    """
    workspace = create_project(root, f"{label}-evictor")
    evictor = start_check(subject, workspace, env, f"{label}-evictor")
    row = wait_for_holding_row(pool)
    expect(row is not None, "the evicting command never recorded a holding row")
    expect(
        row["slot"] == victim["id"],
        f"the fixture's oldest idle tree was not the victim: {row} vs {victim}",
    )
    held = os.path.join(pool, "payload", ".oxide-holding", row["name"])
    expect(tree_present(held), f"the held tree is missing: {held}")
    expect(row["bytes"] > 0, f"the held row carries no charge: {row}")
    return evictor, row, held


def measure_reuse_admission(
    subject: str,
    workspace: str,
    env: dict,
    label: str,
) -> float:
    """One warm-reuse admission of the competitor, on its own idle slot.

    This is the falsifying control for the in-lock gate: the same admission shape
    is measured while no deletion runs, and again beside a live deletion. A reuse
    needs no capacity decision, so it is measurable even while a charged holding
    row sits above the pool's byte figure — which is exactly the side-by-side the
    gate needs. An in-lock deletion would delay it by the whole deletion.
    """
    code, output, elapsed = check(subject, workspace, env, label)
    expect(code == 0, f"the control admission exited {code}: {output[-2000:]}")
    return elapsed


def slot_state(pool: str, slot_id: str) -> str | None:
    """One recorded slot's state, read from the pool's own ledger."""
    try:
        rows = slot_rows(pool)
    except (FileNotFoundError, ValueError):
        return None
    for row in rows:
        if str(row["id"]) == str(slot_id):
            return row.get("state")
    return None


def prove_concurrent_admission(root: str, subject: str) -> dict:
    name = "concurrent-admission"
    case(name)
    pool, env, victim, reusable, _victim_ws, competitor_ws = seed_eviction_fixture(
        root, subject, "concurrent"
    )
    reusable_slot = str(reusable["id"])
    # The control first: the competitor's own warm-reuse admission, measured while
    # no deletion runs (the fixture's total is over the cap, but a reuse needs no
    # capacity decision). It leaves the competitor's slot idle again.
    control_elapsed = measure_reuse_admission(
        subject, competitor_ws, env, "control-reuse"
    )
    expect(
        slot_state(pool, reusable_slot) == "idle",
        "the control admission did not return the competitor's slot to idle",
    )
    # The eviction: the admission retires the older idle tree and its own publish
    # pass deletes it outside the pool lock. The row is observed positively while
    # that deletion runs.
    evictor, row, held = start_eviction(
        root, subject, pool, env, victim, "concurrent"
    )
    expect(
        str(row["slot"]) == str(victim["id"]),
        "the oldest idle tree was not the victim",
    )
    # Progress under the evicting command's own non-blocking custody, measured with
    # the cheap bucket counter (one `listdir` per sample, never a 20,000-entry
    # walk) and estimated over a multi-bucket window, so a single 20 ms tick can
    # never satisfy the rate.
    samples, estimate = observe_progress(held)
    expect(
        estimate is not None,
        f"the deletion's bucket samples do not support a sound rate estimate "
        f"(fewer than {PROGRESS_MIN_BUCKETS} buckets removed): {samples[-8:]}",
    )
    initial_buckets = estimate["initial_buckets"]
    remaining_before = estimate["remaining_buckets"]
    progressed = estimate["progressed_buckets"]
    observed_span = estimate["span_s"]
    progress_rate = estimate["rate_per_s"]
    expect(
        0 < remaining_before < initial_buckets,
        f"the deletion's bucket samples are not a strict decrease: {samples[-8:]}",
    )
    expect(
        remaining_before * 2 >= initial_buckets,
        f"the deletion was already {initial_buckets - remaining_before} of "
        f"{initial_buckets} buckets in before the first sample: the window is too "
        "short for the in-lock control to be discriminating",
    )
    expect(
        evictor.poll() is None,
        "the evicting command finished before the competitor started; the case is vacuous",
    )
    # The falsifying premise of the in-lock control: enough deletion work must
    # remain that an admission serialised behind the pool lock would exceed the
    # bound this case enforces. Buckets are removed one at a time, so the
    # remaining bucket count over the observed bucket rate is that time.
    bound = max(
        INLOCK_CONTROL_FACTOR * control_elapsed, control_elapsed + INLOCK_CONTROL_SLACK_S
    )
    expected_remaining_s = estimate["remaining_s"]
    expect(
        expected_remaining_s > bound,
        f"only {expected_remaining_s:.2f}s of deletion remained against a "
        f"{bound:.2f}s bound, so an in-lock deletion could not be distinguished",
    )
    # The competitor's admission, observed through the pool's own slot row: its
    # idle slot becomes held. Boundaries are recorded exactly: the admission
    # starts here and completes at the ledger transition below.
    expect(
        slot_state(pool, reusable_slot) == "idle",
        "the competitor's slot was not idle before its admission",
    )
    buckets_at_admission_start = planted_buckets(held)
    expect(
        buckets_at_admission_start > 0,
        "the deletion was already over when the competitor's admission started",
    )
    competitor = start_check(subject, competitor_ws, env, "competitor")
    admission_started = time.perf_counter()
    admission_completed = None
    while admission_completed is None:
        work_left()
        if slot_state(pool, reusable_slot) == "held":
            admission_completed = time.perf_counter()
            break
        if competitor.poll() is not None:
            fail("the competitor exited without taking its slot")
        if time.perf_counter() - admission_started > 120:
            fail("the competitor's admission did not complete inside its window")
        time.sleep(0.002)
    # The oracle, read at the completion instant: the competitor's admission ended
    # while the evicting command was still alive, its holding row was still
    # recorded and the held tree still had buckets to delete. All three are read
    # immediately, so none of them is inferred from a process lifetime.
    evictor_alive = evictor.poll() is None
    row_still_recorded = any(entry["name"] == row["name"] for entry in holding_rows(pool))
    buckets_at_completion = planted_buckets(held)
    admission_elapsed = admission_completed - admission_started
    expect(
        evictor_alive,
        "the deleting command had already exited when the competitor admitted",
    )
    expect(
        row_still_recorded,
        "the held row was already concluded at the competitor's admission",
    )
    expect(
        buckets_at_completion > 0 and tree_present(held),
        f"the held tree was already empty at the competitor's admission "
        f"({buckets_at_completion} buckets left)",
    )
    expect(
        buckets_at_completion <= buckets_at_admission_start,
        "the bucket counter is not monotonic across the admission",
    )
    expect(
        admission_elapsed <= bound,
        f"the admission took {admission_elapsed:.2f}s while a deletion was in "
        f"progress; the in-lock control bound is {bound:.2f}s "
        f"(control {control_elapsed:.2f}s)",
    )
    code, output = finish(competitor, "competitor")
    expect(code == 0, f"the concurrent admission exited {code}: {output[-3000:]}")
    code, evict_output = finish(evictor, "evictor")
    expect(code == 0, f"the evicting command exited {code}: {evict_output[-3000:]}")
    expect(not holding_rows(pool), "a holding row survived the evicting command")
    expect(not tree_present(held), "the held tree survived the evicting command")
    expect(
        tree_present(slot_path(pool, reusable)),
        "the competitor's own tree was deleted",
    )
    concurrent_facts = {
        "pool_cap": PENDING_CAP,
        "victim_slot": str(victim["id"]),
        "competitor_slot": reusable_slot,
        "pending_row_bytes": row["bytes"],
        "control_admission_s": round(control_elapsed, 3),
        "inlock_bound_s": round(bound, 3),
        # Exact boundaries: the admission window, and the deletion state read at
        # both ends of it.
        "admission_started_offset_s": round(admission_started - samples[0][0], 3),
        "admission_completed_offset_s": round(admission_completed - samples[0][0], 3),
        "admission_during_deletion_s": round(admission_elapsed, 3),
        "deletion_remaining_estimate_s": round(expected_remaining_s, 3),
        "progress_samples": [count for _, count in samples],
        "progress_sample_span_s": round(observed_span, 3),
        "progress_buckets": progressed,
        "progress_buckets_per_s": round(progress_rate, 3),
        "initial_buckets": initial_buckets,
        "buckets_remaining_before": remaining_before,
        "buckets_at_admission_start": buckets_at_admission_start,
        "buckets_at_admission_completion": buckets_at_completion,
        "evictor_alive_at_admission": evictor_alive,
        "holding_row_recorded_at_admission": row_still_recorded,
        "competitor_slot_held_at_admission": True,
    }
    passed(name, **concurrent_facts)
    return concurrent_facts


def prove_killed_deleter_takeover(root: str, subject: str) -> dict:
    name = "killed-deleter-takeover"
    case(name)
    pool, env, victim, reusable, _victim_ws, reusable_ws = seed_eviction_fixture(
        root, subject, "killed"
    )
    # The evicting admission retires the older idle tree, and its own publish pass
    # is what deletes it. Kill that command inside the deletion: the row and the
    # partly deleted tree survive, which is exactly the state a later command
    # resumes.
    evictor, row, held = start_eviction(root, subject, pool, env, victim, "killed")
    samples, estimate = observe_progress(held)
    expect(
        estimate is not None,
        f"the evicting command's deletion has no sound rate estimate "
        f"(fewer than {PROGRESS_MIN_BUCKETS} buckets removed): {samples[-8:]}",
    )
    evictor.kill()
    reaped = discard(evictor, "killed-evictor")
    expect(reaped, "the killed evicting command could not be confirmed reaped")
    say(f"killed the evicting command (exit {evictor.returncode})")
    rows = holding_rows(pool)
    expect(len(rows) == 1, f"the killed command left no recorded row: {rows}")
    expect(rows[0]["name"] == row["name"], "the recorded row changed")
    expect(rows[0]["bytes"] > 0, "the surviving row carries no charge")
    expect(tree_present(held), "the held tree is gone")
    remaining_after_kill = planted_buckets(held)
    expect(remaining_after_kill > 0, "the held tree was already empty")
    # A later command resumes it: its own pre-admission pass claims the row
    # outside the pool lock and finishes the deletion, before any capacity
    # decision of its own.
    #
    # The command is a **warm reuse** of the fixture's own idle slot, not a new
    # project. The fixture is over its cap on purpose (the victim's recorded
    # charge plus the reusable slot exceeds `PENDING_CAP`), so a *new* admission
    # would legitimately retire an idle whole slot — the reusable one — and the
    # "other tree survives" half of this oracle would be testing the fixture's
    # own arithmetic instead of the takeover. A reuse takes no capacity decision,
    # so the only thing this command can do to a slot is finish the abandoned
    # holding entry.
    # Membership is compared by count and by this slot's own identity rather
    # than as an exact list: the killed command's held slot may be recovered by
    # the same opens that drain the entry, and recovery keeps a slot's identity
    # whatever state it settles in. What must not happen is a *new* slot (which a
    # capacity decision would have to make room for) or a lost reusable slot.
    members_before = [(row["id"], row["incarnation"]) for row in slot_rows(pool)]
    expect(
        (reusable["id"], reusable["incarnation"]) in members_before,
        f"the fixture's reusable slot is not in the pool: {members_before}",
    )
    reusable_tree = slot_path(pool, reusable)
    expect(tree_present(reusable_tree), "the fixture's other tree is missing before the takeover")
    code, output, _ = check(subject, reusable_ws, env, "takeover")
    expect(code == 0, f"the takeover command exited {code}: {output[-3000:]}")
    expect(
        "deleted the held slot tree" in output,
        f"the takeover did not report its deletion: {output[-2000:]}",
    )
    expect(not holding_rows(pool), f"the row survived the takeover: {holding_rows(pool)}")
    expect(not tree_present(held), "the held tree survived the takeover")
    # The reuse re-took the very slot it already owned: no new slot, no eviction
    # of the tree this oracle requires, and the command returned it to idle.
    members_after = [(row["id"], row["incarnation"]) for row in slot_rows(pool)]
    expect(
        (reusable["id"], reusable["incarnation"]) in members_after,
        f"the reuse lost its own slot: {members_after}",
    )
    expect(
        len(members_after) <= len(members_before),
        f"the takeover added a slot, so it took a capacity decision: "
        f"{members_before} -> {members_after}",
    )
    expect(
        slot_state(pool, str(reusable["id"])) == "idle",
        "the takeover did not return the reused slot to idle",
    )
    expect(
        tree_present(reusable_tree),
        "the fixture's other tree was deleted by the takeover",
    )
    killed_facts = {
        "killed_exit": evictor.returncode,
        "pending_row_bytes": row["bytes"],
        "remaining_after_kill": remaining_after_kill,
        "next_command_finished_it": True,
        "takeover_reused_its_own_slot": True,
        "slot_count_before_takeover": len(members_before),
        "slot_count_after_takeover": len(members_after),
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
        "lock_metric": "the whole eviction-deciding pool-lock interval, from the "
        "command's lock acquisition to the eviction's row write (byte-pressure "
        "decision, victim selection, intent write, identity-verified rename, row "
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
