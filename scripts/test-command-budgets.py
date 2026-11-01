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
  no auto-install, 4 Cargo jobs with 16 codegen units, explicit target and
  build directories inside the monitored scratch, then copied out, hashed and
  identified; only the staging directories are removed. Fixtures never see a
  staged target: they keep one Cargo job, run sequentially and never overlap
  the staging build. Cargo jobs and codegen units are configured build
  parallelism, not a CPU or thread hard cap, and they declare no admission
  demand.
- Lockfile premise: the only `--locked` invocation in this proof is that
  staging build of this repository, whose committed `Cargo.lock` is present at
  the proof's own HEAD. Fixture crates are std-only and are never invoked with
  `--locked`; Cargo writes their lock beside each fixture manifest in its
  private checkout, so a fixture checkout without a lock is normal, not an
  error.
- Human budget results are recognised only by the product's exact
  `report_stop`/`LOSS_NOTE` grammar. Ordinary Cargo, path or project output
  that contains the word `budget` (the fixture crates are named
  `oxide-budget-<label>`) is not a stop or a channel loss, and a failure
  records a bounded escaped stderr excerpt plus its typed fields.
- Every fixture runs in a private scratch tree with its own `HOME`,
  `CARGO_HOME`, `OXIDE_CONFIG`, `OXIDE_POOL` and temp, from a private tiny
  checkout `cwd`, so no case can act on this repository or a live pool. A
  missing control (scratch, deadline, evidence, explicit toolchain) refuses
  before any storage effect.
- Scratch placement (scratch-pools.md S4): the run root is the supervisor's
  `OXIDE_TASK_SCRATCH_ROOT` on the selected volume. The script creates one
  private `scratch-pools` directory beneath it, declares that directory as
  `OXIDE_SCRATCH_ROOT` for every fixture, and places each fixture pool strictly
  inside it with an explicit `OXIDE_POOL_CAP`. The `/private/tmp` fallback is
  refused before any effect, because the product refuses boot-volume scratch
  and that run would not be scratch-pool coverage.
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

import ast
import hashlib
import json
import os
import re
import select
import shutil
import signal
import subprocess
import sys
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPT = os.path.abspath(__file__)
BUILDER = "1.97.1"
# `bounded-test.py` at the current main (the oxide#191/#192/#197 bytes). The
# pure self-check pins this against the file on disk; the supervisor's own
# receipt, not this helper, remains authoritative for session closure.
EXPECTED_RUNNER_SHA256 = "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b"
WALL_LIMIT_S = 300
SCRATCH_BUDGET_MIB = 512
# Fixture pools live beneath one private directory of the run root, which is
# the OXIDE_SCRATCH_ROOT every fixture declares (scratch-pools.md S4).
POOL_SCRATCH_LEAF = "scratch-pools"
FIXTURE_POOL_CAP = "256MiB"
# Set by Proof.prepare() before any case runs; the pure check passes its own.
POOL_SCRATCH_ROOT: str | None = None
# The product's two pool-creation notices: an ordinary creation and a scratch
# pool creation (src/bin/oxide/store.rs create_pool, scratch_io.rs create).
POOL_CREATION_NOTICES = (
    "oxide: creating the build pool at",
    "oxide: creating a scratch build pool on volume ",
)
SHUTDOWN_MARGIN_S = 30
# Staging is one debug build of this repository. Four Cargo jobs with 16 codegen
# units cut its wall time while the staged artifact keeps the same frozen
# profile (opt-level `s`, debug info off, debug assertions on, incremental off,
# symbol-stripped), and no case inspects debug info or symbols. Cargo jobs and
# codegen units are configured build parallelism, not a CPU or thread hard cap,
# and they promise no admission or capacity fit: the guard's own admission and
# monitoring stay authoritative. Fixtures keep one job and run sequentially,
# and no fixture runs while staging builds.
STAGE_JOBS = 4
STAGE_CODEGEN_UNITS = 16
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
USAGE = """usage: test-command-budgets.py [--plan | --self-check | --self-test | --diagnostic | --help]

Without an argument this runs the counted proof under the qualified shared
supervisor in fail-fast mode. --plan prints the fixed command, cases, phases,
disk allocation, child lifetimes, concurrency and limits without any effect.
--self-check runs the pure plan, argument, duration, event and deadline checks
in this process without staging or touching a fixture. --self-test executes the
corrected case bodies against faked process and filesystem seams (no child,
image or pool) to catch body-versus-plan drift, and exercises the
whole-lifecycle fault/receipt paths. --diagnostic is an uncounted keep-going
collection under the same guard: it records recoverable oracle failures and
continues only with dependency-independent cases, while an unknown child or
writer, a custody uncertainty, a prerequisite failure or an exhausted deadline
stops it. Its receipt is distinctly qualified=false and is never a counted
PASS; only ROOT activates such a run separately.
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
# Bounded first-fault evidence: only the most recent observations are kept.
FAULT_DETAIL: list[dict] = []
MAX_FAULT_DETAIL = 3
MAX_EXCERPT_CHARS = 400
PHASE_STARTED = time.perf_counter()

# The product's own human budget diagnostics, recognised by their exact
# grammar (`src/bin/oxide/budget.rs`: report_stop and LOSS_NOTE). Ordinary
# Cargo, project or path output may contain the word "budget" — the fixture
# crates are named `oxide-budget-<label>` — so a lowercase substring test is
# not a budget diagnostic and must never decide an oracle.
STOP_ADMISSION_RE = re.compile(
    r"^oxide: stopped by the admission budget \(\d+ms\) before a usable child start$"
)
STOP_EXECUTION_RE = re.compile(r"^oxide: stopped by the execution budget \(\d+ms\)$")
LOSS_NOTE_RE = re.compile(
    r"^oxide: the budget event channel became unusable; typed results are unavailable for this command$"
)
DIAGNOSTIC_TEXT = {
    "admission_stop": "oxide: stopped by the admission budget (<N>ms) before a usable child start",
    "execution_stop": "oxide: stopped by the execution budget (<N>ms)",
    "event_loss": "oxide: the budget event channel became unusable; typed results are unavailable for this command",
}


def pool_creation_notice(text: str | None) -> bool:
    """Whether stderr carries an ordinary or a scratch pool-creation notice.

    A scratch pool prints only the scratch notice, so an oracle that matched the
    ordinary notice alone would fail every scratch fixture as "no pool created".
    """
    if not text:
        return False
    return any(line.startswith(POOL_CREATION_NOTICES) for line in text.splitlines())


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


class Abort(Fault):
    """An outcome that keep-going diagnostics must not continue past.

    An unknown child, a custody uncertainty, a staging or capability
    prerequisite failure, or an exhausted guard deadline is not a recoverable
    oracle failure: a diagnostic stops and reports it instead of continuing.
    """


def abort(message: str) -> None:
    """Record and raise a non-continuable fault (the first text wins)."""
    global PRIMARY_FAULT
    if PRIMARY_FAULT is None:
        PRIMARY_FAULT = message
    say(f"ABORT: {message}")
    for process in OWNED:
        release(process)
    raise Abort(message)


def custody_guard(stage: str) -> None:
    """Stop when any owned child's custody is currently unknown.

    This is shared safety state, not a per-case delta: a case that returns
    normally can still have left an unresolved owned child (`close_holder` or
    `release` record it), and direct leader reaping is not descendant closure.
    Both the counted run and the diagnostic call this before dispatch and after
    every outcome, so no later case can start on unknown custody.
    """
    if UNCERTAIN:
        abort(f"unknown child custody {stage}: {UNCERTAIN[-1]}")


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
        abort("the guard's work deadline is exhausted")
    return max(1.0, min(float(phase_seconds), left))


def work_left() -> float:
    guard = os.environ.get("OXIDE_PROOF_DEADLINE_NS")
    if not guard:
        abort("OXIDE_PROOF_DEADLINE_NS is not exported; this proof runs only under the guard")
    left = (int(guard) - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 5:
        abort("the guard's work deadline is inside its shutdown margin")
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


BUDGET_FLAGS = ("--admission-budget", "--execution-budget", "--budget-events")
RESERVED = (*BUDGET_FLAGS, "--profile", "--manifest-path", "-p", "--package", "--release", "-r")


def budget_prefix_ok(argv: list[str]) -> bool:
    """Whether every budget flag sits inside the product's prefix.

    The product's scan stops at the first non-reserved argument, so a budget
    after `-j`, a program path or a subcommand would never parse. Proof argv
    must satisfy this before it is worth running.
    """
    index = 0
    while index < len(argv):
        text = argv[index]
        if any(text == flag or text.startswith(flag + "=") for flag in BUDGET_FLAGS):
            index += 1
            if "=" not in text:
                index += 1
            continue
        if any(text == flag or text.startswith(flag + "=") for flag in RESERVED):
            index += 1
            if "=" not in text and text in ("--profile", "--manifest-path", "-p", "--package"):
                index += 1
            continue
        break
    saw_budget = False
    for text in argv[:index]:
        if any(text == flag or text.startswith(flag + "=") for flag in BUDGET_FLAGS):
            saw_budget = True
    for text in argv[index:]:
        if any(text == flag or text.startswith(flag + "=") for flag in BUDGET_FLAGS):
            return False
    return True if saw_budget or not any(flag in argv for flag in BUDGET_FLAGS) else False


def body_signature_drift(path: str) -> list[tuple[int, str]]:
    """Keywords a case body passes to `run` that its real signature lacks.

    This is the body-versus-API check: a mock with a permissive signature can
    hide such a mismatch from a runtime test, but the real call would fail.
    """
    import inspect as _inspect

    declared = set(_inspect.signature(run).parameters)
    tree = ast.parse(open(path, encoding="utf-8").read())
    drift: list[tuple[int, str]] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.FunctionDef) or not node.name.startswith("case_"):
            continue
        for call in ast.walk(node):
            if not isinstance(call, ast.Call):
                continue
            if not isinstance(call.func, ast.Name) or call.func.id != "run":
                continue
            for keyword in call.keywords:
                if keyword.arg is not None and keyword.arg not in declared:
                    drift.append((call.lineno, keyword.arg))
    return drift


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
        "--diagnostic": "diagnostic",
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


# The nine items of the fixed proof-authoring premise checklist
# (docs/knowledge/contracts/workflow.md#proof-authoring-premise-checklist). Each
# disposition names the module-level source it depends on; the pure self-check
# requires every name to exist and pins the fixture environment values, so a
# disposition cannot outlive the code that satisfies it.
PREMISE_CHECKLIST = {
    "1_private_home_and_rustup": {
        "disposition": (
            "invoking_rustup_home() is read before any private HOME exists and passed to "
            "fixture_environment() with RUSTUP_TOOLCHAIN pinned to 1.97.1 and "
            "RUSTUP_AUTO_INSTALL=0. The only --locked call is the staging build of this "
            "repository (LOCKED_FLAG, its committed Cargo.lock); fixture crates are "
            "std-only and never pass --locked, so no fixture lock is seeded and nothing "
            "is downloaded or installed"
        ),
        "source": ["invoking_rustup_home", "fixture_environment", "LOCKED_FLAG", "BUILDER"],
    },
    "2_account_home_control": {
        "disposition": "fixture_environment() sets OXIDE_TEST_HOME_FROM_ENV=1 beside the private HOME",
        "source": ["fixture_environment"],
    },
    "3_environment_isolation": {
        "disposition": (
            "stage_subject() drops every inherited OXIDE_ (including OXIDE_SCRATCH_ROOT), "
            "CARGO_TARGET_/CARGO_BUILD_/SCCACHE_ and encoded compiler key (STAGING_SCRUBBED); "
            "fixture_environment() is an allowlist of private HOME, CARGO_HOME, config, "
            "scratch pool, pool cap, temp and the toolchain home, so no host pool, config or "
            "account provenance is inherited"
        ),
        "source": ["STAGING_SCRUBBED", "fixture_environment"],
    },
    "4_automatic_adoption": {
        "disposition": (
            "not applicable: every fixture is a leased command on a private scratch pool, and "
            "no case places or reads permanent outputs, so no checkout is automatically adopted"
        ),
        "source": [],
    },
    "5_cargo_external_argv": {
        "disposition": (
            "not applicable: no case invokes an external Cargo subcommand; fixtures run "
            "`oxide exec`/`run` and the staged candidate's own commands only"
        ),
        "source": [],
    },
    "6_owned_images": {
        "disposition": (
            "not applicable: no sparse image is created or attached; the scratch pools are "
            "directories on the selected scratch volume, which may be removable"
        ),
        "source": [],
    },
    "7_compiler_flags": {
        "disposition": (
            "staging freezes RUSTFLAGS through STAGING_ENV_REQUIRED and drops every encoded "
            "form (CARGO_ENCODED_RUSTFLAGS); fixture_environment() passes no RUSTFLAGS at all, "
            "so an inherited value cannot change the staged candidate or a fixture"
        ),
        "source": ["STAGING_ENV_REQUIRED", "STAGING_SCRUBBED", "fixture_environment"],
    },
    "8_temporary_storage": {
        "disposition": (
            "require_controls() refuses the /private/tmp fallback and a "
            "non-canonical, foreign-owned or symlinked root before any effect; Proof.prepare() "
            "creates one private scratch-pools directory beneath OXIDE_TASK_SCRATCH_ROOT; "
            "fixture_environment() declares it as OXIDE_SCRATCH_ROOT with an explicit cap and "
            "places every pool strictly inside it; TMPDIR is the fixture root on the same volume. "
            "The product's DirectoryBudget reserve watchdog is the same class as the internal pool's"
        ),
        "source": ["require_controls", "POOL_SCRATCH_LEAF", "POOL_SCRATCH_ROOT", "fixture_environment", "FIXTURE_POOL_CAP"],
    },
    "9_macos_mktemp": {
        "disposition": (
            "no shell mktemp: every fixture root is a private_dir() beneath the supervisor run "
            "root, so macOS TMPDIR handling never selects a location. The only tempfile.mkdtemp "
            "is the pure self-test's, with an explicit dir"
        ),
        "source": ["private_dir"],
    },
}


def plan_document() -> dict:
    return {
        "task": "oxide#140",
        "selector": SELECTOR,
        "command": COMMAND,
        "counted_only_without_argument": True,
        "premise_checklist": {key: entry["disposition"] for key, entry in PREMISE_CHECKLIST.items()},
        "scratch": {
            "placement": "supervisor-selected volume: OXIDE_TASK_SCRATCH_ROOT/scratch-pools is the declared OXIDE_SCRATCH_ROOT",
            "pool": "one scratch pool per fixture, strictly inside that root, named pool-<case>",
            "cap": FIXTURE_POOL_CAP + " explicit per fixture (scratch pools have no implicit budget)",
            "fallback": "a /private/tmp fallback refuses before any effect; it is not scratch-pool coverage",
            "not_a_cap": "the per-pool cap is the product's soft budget for that fixture; the guard's 512 MiB monitored scratch budget governs the whole run",
        },
        "modes": ["--plan", "--self-check", "--self-test", "--diagnostic", "--help"],
        "cases": [
            {"name": "fast-start-axes", "oracle": "both budgets: started then finished(child_exit 0), spawn_late false, both budget fields present; execution-only: admission_ms null; admission-only: execution_ms and execution_started_ms null; no budget diagnostic (exact report_stop/LOSS_NOTE grammar) in any axis"},
            {"name": "spawn-failure-event", "oracle": "missing program: exit 127, no started record, exactly one finished(spawn_failure)"},
            {"name": "admission-expiry-no-start", "oracle": "held slot + 1s admission: exit 124, no primary marker, no started record, finished(admission_deadline), contender ticket gone"},
            {"name": "contention-longer-than-execution", "oracle": "held slot released after ~3s + 60s admission/2s execution: starts after release, exit 0, execution_elapsed <=2s though wall >=3s"},
            {"name": "execution-deadline", "oracle": "60s sleeper + 1s execution: exit 125, finished(execution_deadline), budget named, group settled, slot reusable"},
            {"name": "precedence-child-124", "oracle": "child exits 124 itself under a 60s budget: child_exit 124, no budget diagnostic, no latched origin"},
            {"name": "precedence-caller-signal", "oracle": "SIGTERM before expiry: 143, released, no deadline origin"},
            {"name": "precedence-child-125", "oracle": "child exits 125 itself under a 60s budget: child_exit, no budget diagnostic"},
            {"name": "precedence-term-trap", "oracle": "TERM-trapping child under 1s: trap marker present, exit 125, finished(execution_deadline)"},
            {"name": "cumulative-run-window", "oracle": "3s build.rs then a 60s program under one 5s execution window on a budget-only pool: exit 125 in run_program, execution stop origin, and the launched program wrote 2..20 heartbeats of its own 200ms counter (~10 in the remaining window; a reset window would write ~25); the candidate exit and closeout latency are not asserted"},
            {"name": "nested-outer-window", "oracle": "the outer exec starts first and takes the only slot; its tool starts an inner oxide that waits and ends on its own 1s admission budget (grammar-checked, no Cargo compile lines); the tool keeps running so the outer 2s window (not a natural exit) stops it: exit 125 with a typed exec_tool start and execution_deadline"},
            {"name": "events-matrix", "oracle": "regular file, read end and blocking pipe refuse before any effect; full pipe yields no typed record; contiguous sequence on a healthy stream"},
            {"name": "events-broken-reader", "oracle": "read end closed before start: exit 0, parent survives, one loss note, no typed record"},
            {"name": "unknown-closure-held", "oracle": "debug closure hook + 1s execution: exit 1, finished(unconfirmed_closure), drain not complete, lease held, status shows the held slot"},
            {"name": "preparation-stop-124", "oracle": "fake cargo sleeping in locate-project + 1ms admission: exit 124, no primary marker"},
            {"name": "late-start", "oracle": "one delay-free seed launch of the same private pool, checkout and tool argv with the spawn-delay hook absent (exit 0, an ordinary or scratch pool-creation notice, recorded control state), then the timed launch under a 6s window with a 7s debug delay after the last admission check: it must reuse the prepared state (no second pool-creation notice, same control root) and report exactly one started exec_tool record with spawn_late and admission_expired_before_start true, a positive overshoot and admission_elapsed_ms past 6000, terminal admission_deadline with an admission origin, exit 124, the exact stop line, and a delay-free follow-up exec that reuses the released slot; a no-started pre-spawn expiry or a second pool creation is an unexercised fixture FAIL"},
            {"name": "no-mutation", "oracle": "a refused input leaves home/config/pool unchanged; a successful budgeted command adds no settings file, no new control-state fields and no budget lines in status"},
            {"name": "unbudgeted", "oracle": "exit 3 preserved, no budget diagnostic, no events required"},
        ],
        "phases": [
            {"name": "stage", "seconds": PHASE_ALLOCATION_S["stage"], "allocation": "one debug oxide, plain Cargo offline, 4 Cargo jobs with 16 codegen units, target and build inside scratch, staging deleted before fixtures", "disk_mib": 220},
            {"name": "prepare", "seconds": PHASE_ALLOCATION_S["prepare"], "allocation": "private fixture roots and tiny std-only crates", "disk_mib": 20},
            {"name": "cases", "seconds": PHASE_ALLOCATION_S["cases"], "allocation": "sequential fixture commands after staging finishes; peak: one staged binary, one compiler job, at most two live fixture processes", "disk_mib": 260},
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
            "stage_codegen_units": STAGE_CODEGEN_UNITS,
            "fixture_cargo_jobs": FIXTURE_JOBS,
            "fixture_commands": "sequential after staging finishes, at most one candidate and one holder at a time",
            "not_a_cap": "Cargo jobs and codegen units are configured build parallelism, not a CPU or thread hard cap; they declare no admission demand and promise no capacity fit",
        },
        "diagnostic": {
            "qualified": False,
            "default_mode": "fail-fast",
            "continues": "recoverable oracle failures in dependency-independent cases",
            "aborts": "unknown child or writer outcome, custody uncertainty, a staging or capability prerequisite failure, or an exhausted guard deadline",
            "dependencies": {name: [] for name in DIAGNOSTIC_DEPENDENCIES},
            "independence": "every case labels its own private HOME, CARGO_HOME, config and pool paths, so no case depends on another case's fixture state",
            "receipt": "distinct helper receipt with qualified=false, the failures, bounded evidence, completed functions, truthful skips and any abort reason",
            "exit": "0 only when the collection completed with no failure and no abort; 1 otherwise; never a counted PASS",
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
            "fixtures": "private HOME/CARGO_HOME/OXIDE_CONFIG, a scratch pool (OXIDE_SCRATCH_ROOT scratch-pools with OXIDE_POOL_CAP) and temp on the selected volume, and a private tiny checkout cwd",
            "private_home": "OXIDE_TEST_HOME_FROM_ENV=1 with explicit private config and pool (oxide#143 convention)",
            "toolchain": "RUSTUP_TOOLCHAIN=1.97.1, RUSTUP_AUTO_INSTALL=0, RUSTUP_HOME captured before HOME is replaced",
            "lockfile": "the only --locked call is the staging build of this repository, whose committed Cargo.lock is present; fixture crates are std-only, are never invoked with --locked, and Cargo writes their lock beside each fixture manifest in its private checkout",
            "diagnostics": "human budget results are recognised only by the exact report_stop and LOSS_NOTE grammar; innocent project/path/Cargo text containing 'budget' is not a diagnostic",
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
    stdout=None,
    stderr=None,
) -> subprocess.CompletedProcess:
    """Run one owned direct child with the guard-clipped timeout.

    `stdout`/`stderr` are the real API for a case that must send a stream to a
    file (the cumulative case polls nothing, but keeps the candidate's stderr
    out of a pipe); when omitted both are captured. A redirected stream is not
    captured, so the completed process reports `None` for it.
    """
    started = time.monotonic()
    process = start(
        argv,
        cwd=cwd,
        env=env,
        pass_fds=pass_fds,
        stdin=subprocess.DEVNULL if stdin is None else stdin,
        stdout=subprocess.PIPE if stdout is None else stdout,
        stderr=subprocess.PIPE if stderr is None else stderr,
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
        abort(fault_text)
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
    is_directory = os.path.isdir(scratch)
    owner = os.stat(scratch).st_uid if is_directory else -1
    reason = scratch_root_refusal(
        scratch,
        os.path.realpath(scratch),
        is_directory,
        os.path.islink(scratch),
        owner,
        os.geteuid(),
    )
    expect(reason is None, reason or "")
    expect(os.path.isdir(evidence), f"evidence directory {evidence} does not exist")
    return (scratch, evidence)


def scratch_root_refusal(
    scratch: str, canonical: str, is_directory: bool, is_link: bool, owner: int, user: int
) -> str | None:
    """The first reason a declared scratch root is unusable, or None (pure).

    The /private/tmp fallback is the internal boot volume, which the product
    refuses as scratch (scratch-pools.md row 8); refusing it here keeps a
    fallback run from reporting as scratch-pool coverage.
    """
    if canonical != scratch:
        return f"scratch root {scratch} is not canonical"
    if scratch.startswith(("/private/tmp/", "/tmp/")):
        return f"scratch root {scratch} is the /private/tmp fallback; scratch-pool coverage needs the selected volume"
    if not is_directory or is_link:
        return f"scratch root {scratch} is not a directory"
    if owner != user:
        return f"scratch root {scratch} is not owned by this user"
    return None


def fixture_environment(root: str, label: str, pool_scratch: str) -> dict:
    """The pure allowlisted environment of one fixture: no effect, no directory.

    Its pool lies strictly inside `pool_scratch`, which is declared as the
    OXIDE_SCRATCH_ROOT with an explicit cap, so the pool is a scratch pool on
    the supervisor-selected volume. Temp is the fixture root, on that volume.
    """
    env = {
        "HOME": os.path.join(root, f"home-{label}"),
        "OXIDE_TEST_HOME_FROM_ENV": "1",
        "OXIDE_CONFIG": os.path.join(root, f"oxide-config-{label}.toml"),
        "OXIDE_SCRATCH_ROOT": pool_scratch,
        "OXIDE_POOL": os.path.join(pool_scratch, f"pool-{label}"),
        "OXIDE_POOL_BACKEND": "directory-budget",
        "OXIDE_POOL_CAP": FIXTURE_POOL_CAP,
        "CARGO_HOME": os.path.join(root, f"cargo-home-{label}"),
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
    return env


def fixture_env(root: str, label: str) -> dict:
    """A private allowlisted environment: no host Oxide, XDG or Cargo setting."""
    expect(POOL_SCRATCH_ROOT is not None, "the scratch-pool root is not prepared")
    private_dir(os.path.join(root, f"home-{label}"))
    private_dir(os.path.join(root, f"cargo-home-{label}"))
    env = fixture_environment(root, label, POOL_SCRATCH_ROOT)
    write_private(env["OXIDE_CONFIG"], "")
    say(f"fixture {label}: private home, cargo home, config, scratch pool and temp")
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
    root: str, env: dict, checkout: str, label: str, seconds: float, subject: str
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
        [subject, "exec", "--", tool],
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
            abort(f"the {label} holder exited before taking its slot: {process.stderr.read()[-800:]}")
        if time.monotonic() > deadline:
            release(process)
            abort(f"the {label} holder never took its slot")
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


def budget_diagnostics(text: str | None) -> list[str]:
    """The kinds of product budget diagnostic present, by exact grammar.

    Only the three product lines count: an unexplained stop, a later loss note
    or an innocent mention of "budget" (a crate name, a path, a Cargo line, a
    user's own tool output) is not a budget result. Incomplete or truncated
    lines therefore never become positive evidence.
    """
    kinds: list[str] = []
    for line in (text or "").splitlines():
        stripped = line.strip()
        if STOP_ADMISSION_RE.match(stripped):
            kinds.append("admission_stop")
        elif STOP_EXECUTION_RE.match(stripped):
            kinds.append("execution_stop")
        elif LOSS_NOTE_RE.match(stripped):
            kinds.append("event_loss")
    return kinds


def stderr_excerpt(text: str | None, limit: int = MAX_EXCERPT_CHARS) -> str:
    """A bounded escaped excerpt; truncation is explicit, never silent."""
    if not text:
        return "<no stderr captured>"
    escaped = text.encode("unicode_escape").decode("ascii")
    if len(escaped) <= limit:
        return escaped
    return f"{escaped[:limit]}... <truncated; {len(escaped)} escaped chars total>"


def expect_no_budget_diagnostic(text: str | None, label: str) -> None:
    """No product stop/loss diagnostic; innocent output may mention budget."""
    found = budget_diagnostics(text)
    expect(not found, f"{label}: printed a budget diagnostic {found}: {stderr_excerpt(text)}")


def expect_budget_diagnostic(text: str | None, kind: str, label: str) -> None:
    found = budget_diagnostics(text)
    expect(
        kind in found,
        f"{label}: expected the {kind} diagnostic {DIAGNOSTIC_TEXT[kind]!r}, saw {found}: {stderr_excerpt(text)}",
    )


def typed_summary(records: list[dict] | None) -> dict:
    """A bounded view of the typed records for a failure receipt."""
    if not records:
        return {"records": 0}
    keys = ("seq", "event", "phase", "outcome", "stop_origin", "exit_code", "drain", "lease")
    return {
        "records": len(records),
        "shown": [{key: entry.get(key) for key in keys if key in entry} for entry in records[:8]],
        "omitted": max(0, len(records) - 8),
    }


def record_detail(
    label: str,
    stderr: str | None,
    records: list[dict] | None = None,
    **fields,
) -> None:
    """Keep bounded first-fault evidence for the helper receipt.

    The label names the exact command and oracle; the stderr excerpt is escaped
    and length-bounded, and any typed records are summarised rather than copied
    whole. Only the most recent entries are kept, so the receipt stays small
    while a failure still carries the facts that separate a grammar bug from
    product behaviour.
    """
    entry: dict = {"label": label, "stderr": stderr_excerpt(stderr)}
    if records is not None:
        entry["typed"] = typed_summary(records)
    if fields:
        entry["fields"] = {key: fields[key] for key in sorted(fields)}
    FAULT_DETAIL.append(entry)
    del FAULT_DETAIL[:-MAX_FAULT_DETAIL]


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
            # A fast child writes two records back to back: wait for the first
            # line deterministically, then take whatever else is already there
            # instead of leaving the terminal record unread in the pipe.
            raw += drain(read_fd)
        finally:
            os.close(write_fd)
            os.close(read_fd)
        record_detail(
            f"fast-start/{label}",
            answer.stderr,
            returncode=answer.returncode,
            elapsed=round(getattr(answer, "elapsed", 0.0), 3),
        )
        expect(answer.returncode == 0, f"{label}: exit {answer.returncode}: {stderr_excerpt(answer.stderr)}")
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
        record_detail(f"fast-start/{label}", answer.stderr, records)
        expect_no_budget_diagnostic(answer.stderr, label)
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
    record_detail("spawn-failure", answer.stderr, returncode=answer.returncode, drained=len(raw))
    expect(answer.returncode == 127, f"a missing program exited {answer.returncode}, wanted 127")
    records = finish_events(raw)
    record_detail("spawn-failure", answer.stderr, records)
    expect(not started_records(records), "a spawn failure emitted a started record")
    recorded = terminal(records)
    expect(recorded["outcome"] == "spawn_failure", f"outcome {recorded['outcome']}")
    expect(recorded["child_exit_code"] is None, "a spawn failure reported a child exit code")
    expect_no_budget_diagnostic(answer.stderr, "spawn-failure")
    result("spawn-failure-event", True, "exit 127, no started record, finished(spawn_failure)")


def case_admission_expiry(root: str, subject: str) -> None:
    case("admission-expiry-no-start")
    env = fixture_env(root, "admission")
    crate = create_crate(root, "admission")
    env["OXIDE_MAX_ACTIVE"] = "1"
    marker = os.path.join(root, "admission.marker")
    tool = os.path.join(root, "admission.sh")
    write_executable(tool, f"#!/bin/sh\ntouch {marker}\nexit 0\n")
    holder, _release_file = hold_slot(root, env, crate, "admission", 30, subject)
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
    record_detail("admission-expiry", answer.stderr, returncode=answer.returncode, drained=len(raw))
    expect(answer.returncode == ADMISSION_EXIT, f"admission expiry exited {answer.returncode}")
    expect(not os.path.exists(marker), "an admission-expired command started its child")
    records = finish_events(raw)
    record_detail("admission-expiry", answer.stderr, records)
    expect_budget_diagnostic(answer.stderr, "admission_stop", "admission expiry")
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
    holder, release_file = hold_slot(root, env, crate, "contention", 30, subject)
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
    expect(process.returncode == 0, f"the contender exited {process.returncode}: {stderr_excerpt(stderr)}")
    expect(waited >= 2.5, f"the contender only waited {waited:.1f}s, not longer than its 2s execution window")
    expect(elapsed <= 6.0, f"the contender took {elapsed:.1f}s after release; execution elapsed must fit its own window")
    record_detail("contention", stderr, returncode=process.returncode, waited=round(waited, 3), elapsed=round(elapsed, 3))
    expect_no_budget_diagnostic(stderr, "contention")
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
    record_detail("execution-deadline", answer.stderr, returncode=answer.returncode, elapsed=round(answer.elapsed, 3))
    expect(answer.returncode == EXECUTION_EXIT, f"the deadline exited {answer.returncode}")
    records = finish_events(raw)
    record_detail("execution-deadline", answer.stderr, records)
    expect_budget_diagnostic(answer.stderr, "execution_stop", "execution deadline")
    expect(answer.elapsed <= 1 + 15, f"the stop took {answer.elapsed:.1f}s past the finite closeout bound")
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
    record_detail("precedence-child-124", answer.stderr, returncode=answer.returncode)
    expect_no_budget_diagnostic(answer.stderr, "genuine child 124")
    recorded = terminal(finish_events(raw))
    expect(recorded["outcome"] == "child_exit", f"a genuine 124 was typed {recorded['outcome']}")
    expect(recorded["stop_origin"] is None, f"a genuine 124 latched {recorded['stop_origin']}")
    result("precedence-child-124", True, "child_exit 124 without a budget line or latched origin")

    case("precedence-child-125")
    self125 = os.path.join(root, "self125.sh")
    write_executable(self125, "#!/bin/sh\nexit 125\n")
    result_run = run([subject, "exec", "--execution-budget", "60s", "--", self125], cwd=crate, env=env, phase_seconds=30)
    expect(result_run.returncode == 125, f"a genuine child 125 exited {result_run.returncode}")
    record_detail("precedence-child-125", result_run.stderr, returncode=result_run.returncode)
    expect_no_budget_diagnostic(result_run.stderr, "genuine child 125")
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
    record_detail("precedence-caller-signal", stderr, returncode=process.returncode)
    expect_no_budget_diagnostic(stderr, "caller signal")
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
    record_detail("precedence-term-trap", result_run.stderr, returncode=result_run.returncode)
    expect_budget_diagnostic(result_run.stderr, "execution_stop", "TERM-trap stop")
    result("precedence-term-trap", True, "TERM delivered to the trap, controlled stop still 125")


def case_cumulative_run(root: str, subject: str) -> None:
    case("cumulative-run-window")
    env = fixture_env(root, "cumulative")
    crate = private_dir(os.path.join(root, "checkouts", "cumulative"))
    write_private(
        os.path.join(crate, "Cargo.toml"),
        '[package]\nname = "oxide-budget-cumulative"\nversion = "0.1.0"\nedition = "2021"\nbuild = "build.rs"\n',
    )
    counter = os.path.join(root, "program-heartbeats")
    # The launched program records its own running time as a heartbeat counter:
    # one line every 200 ms. The proof never derives a runtime from the
    # candidate's exit or from closeout/scheduling latency.
    write_private(
        os.path.join(crate, "build.rs"),
        "fn main() { std::thread::sleep(std::time::Duration::from_secs(3)); }\n",
    )
    write_private(
        os.path.join(crate, "src", "main.rs"),
        "use std::io::Write;\n"
        "fn main() {\n"
        f'    let mut file = std::fs::OpenOptions::new().create(true).append(true).open("{counter}").unwrap();\n'
        "    for index in 0..600 {\n"
        '        writeln!(file, "{index}").unwrap();\n'
        "        file.flush().ok();\n"
        "        std::thread::sleep(std::time::Duration::from_millis(200));\n"
        "    }\n"
        "}\n",
    )
    read_fd, write_fd = event_pipe()
    stderr_path = os.path.join(root, "cumulative.stderr")
    with open(stderr_path, "w", encoding="utf-8") as stderr_file:
        answer = run(
            [
                subject,
                "run",
                "--execution-budget",
                "5s",
                "--budget-events",
                str(write_fd),
                "-j",
                str(FIXTURE_JOBS),
            ],
            cwd=crate,
            env=env,
            phase_seconds=45,
            pass_fds=(write_fd,),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=stderr_file,
        )
        raw = drain(read_fd)
    os.close(write_fd)
    os.close(read_fd)
    record_detail(
        "cumulative-run-window",
        read_text(stderr_path),
        returncode=answer.returncode,
        elapsed=round(answer.elapsed, 3),
    )
    expect(
        answer.returncode == EXECUTION_EXIT,
        f"the cumulative run exited {answer.returncode}: {stderr_excerpt(read_text(stderr_path))}",
    )
    records = finish_events(raw)
    record_detail("cumulative-run-window", read_text(stderr_path), records)
    expect_budget_diagnostic(read_text(stderr_path), "execution_stop", "cumulative run")
    phases = [entry.get("phase") for entry in started_records(records)]
    expect("primary_cargo" in phases and "run_program" in phases, f"phases {phases}")
    expect(terminal(records)["outcome"] == "execution_deadline", "the cumulative window did not end as execution_deadline")
    expect(terminal(records)["stop_origin"] == "execution", "the cumulative window did not report an execution-origin stop")
    beats = len([line for line in read_text(counter).splitlines() if line.strip()])
    # The build consumes about 3 s of the single 5 s window, so the program can
    # only beat for about 2 s (~10 beats). A window reset at the build hand-off
    # would give it a fresh ~5 s (~25 beats). The band is deliberately wide so
    # scheduling jitter cannot decide the result, and closeout latency is not
    # part of it.
    expect(2 <= beats <= 20, f"the program produced {beats} heartbeat(s) under one 5s window: the window was reset or never armed")
    result(
        "cumulative-run-window",
        True,
        f"one 5s window: ~3s build then {beats} program heartbeat(s) (~2s); a reset window would give ~25",
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
        f"cd {inner_crate} && {subject} check --admission-budget 1s -j {FIXTURE_JOBS} > {inner_log} 2>&1\n"
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
    expect(
        answer.returncode == EXECUTION_EXIT,
        f"the outer command exited {answer.returncode}: {stderr_excerpt(answer.stderr)}",
    )
    records = finish_events(raw)
    record_detail("nested-outer", answer.stderr, records, returncode=answer.returncode)
    expect_budget_diagnostic(answer.stderr, "execution_stop", "outer stop")
    phases = [entry.get("phase") for entry in started_records(records)]
    expect("exec_tool" in phases, f"the outer tool start was not reported: {phases}")
    recorded = terminal(records)
    expect(recorded["outcome"] == "execution_deadline", "the outer window did not report execution_deadline")
    expect(recorded["stop_origin"] == "execution", "the outer stop did not carry an execution origin")
    inner = read_text(inner_log)
    record_detail("nested-inner", inner, fields={"inner_log_bytes": len(inner)})
    expect("oxide: waiting for" in inner, f"the inner call never waited for the held slot: {stderr_excerpt(inner)}")
    expect_budget_diagnostic(inner, "admission_stop", "inner call")
    # `oxide check` places build output in the pool, so a checkout-local
    # `target/` never appears even when a build happens; assert the absence of
    # Cargo's own build lines instead of a directory that proves nothing.
    expect(
        "Compiling " not in inner and "Finished " not in inner,
        f"the inner call compiled a crate: {stderr_excerpt(inner)}",
    )
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
    # Fill any remainder byte by byte: a partially filled pipe could still
    # accept a whole record, which would make the full-channel premise
    # non-deterministic.
    while True:
        try:
            os.write(write_fd, b"x")
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
    record_detail("events-full", full.stderr, returncode=full.returncode, drained=len(drained))
    expect(full.returncode == 0, f"a full event pipe changed the outcome: {full.returncode}")
    expect(b'"event"' not in drained, "a record was written after the stream was lost")
    expect_budget_diagnostic(full.stderr, "event_loss", "full event pipe")
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
    record_detail("events-broken-reader", result_run.stderr, returncode=result_run.returncode)
    losses = budget_diagnostics(result_run.stderr).count("event_loss")
    expect(losses == 1, f"the loss note count is {losses}, wanted exactly one: {stderr_excerpt(result_run.stderr)}")
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
    record_detail("unknown-closure", result_run.stderr, returncode=result_run.returncode)
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
    record_detail("preparation-stop", result_run.stderr, returncode=result_run.returncode)
    expect_budget_diagnostic(result_run.stderr, "admission_stop", "preparation stop")
    result("preparation-stop-124", True, "124, no primary child")


def case_late_start(root: str, subject: str) -> None:
    case("late-start")
    # The debug hook sleeps after the last admission check and before the
    # launch, so the window must still be open at that check and must close
    # during the sleep. Cold preparation (pool creation, workspace metadata,
    # the slot key and the freshness gate) consumed the whole window in
    # diagnostic1 and diagnostic2, so this case pre-establishes that work with
    # ONE delay-free seed launch of the same command shape on the same private
    # pool, checkout and tool argv, and checks the prepared state positively.
    # The timed launch must reuse it: a second pool-creation notice, a different
    # control root or any pre-spawn expiry fails as an unexercised fixture. The
    # budget prefix only governs the clock: `SlotKey::resolve_in` takes the
    # checkout, profile recipe, target and destination facts, which the seed and
    # the timed launch share, so both resolve the same prepared slot key.
    admission_ms = 6000
    delay_ms = 7000
    mode = os.path.join(root, "late.run")
    tool = os.path.join(root, "late.sh")
    write_executable(
        tool,
        "#!/bin/sh\n"
        f"if [ -e {mode} ]; then exec sleep 60; fi\n"
        "exit 0\n",
    )
    expect(not os.path.exists(mode), "the late-start fixture started in run mode")
    crate = create_crate(root, "late")
    base_env = fixture_env(root, "late")
    expect(
        "OXIDE_TEST_SPAWN_DELAY_MS" not in base_env,
        "the fixture environment must not carry the spawn-delay hook before the timed launch",
    )
    seed = run([subject, "exec", "--", tool], cwd=crate, env=base_env, phase_seconds=30)
    record_detail(
        "late-start/seed",
        seed.stderr,
        returncode=seed.returncode,
        elapsed=round(seed.elapsed, 3),
    )
    expect(seed.returncode == 0, f"the seed launch failed ({seed.returncode}): {stderr_excerpt(seed.stderr)}")
    expect(
        pool_creation_notice(seed.stderr),
        f"the seed did not create the fixture pool: {stderr_excerpt(seed.stderr)}",
    )
    control, _keys_before = control_state(base_env)
    expect(control is not None, "the seed left no recorded control state to reuse")
    timed_env = dict(base_env)
    timed_env["OXIDE_TEST_SPAWN_DELAY_MS"] = str(delay_ms)
    write_private(mode, "run\n")
    read_fd, write_fd = event_pipe()
    try:
        result_run = run(
            [
                subject,
                "exec",
                "--admission-budget",
                f"{admission_ms}ms",
                "--execution-budget",
                "60s",
                "--budget-events",
                str(write_fd),
                "--",
                tool,
            ],
            cwd=crate,
            env=timed_env,
            phase_seconds=45,
            pass_fds=(write_fd,),
        )
        raw = drain(read_fd)
    finally:
        os.close(write_fd)
        os.close(read_fd)
    expect(result_run.returncode == ADMISSION_EXIT, f"the late start exited {result_run.returncode}")
    records = finish_events(raw)
    record_detail(
        "late-start",
        result_run.stderr,
        records,
        returncode=result_run.returncode,
        elapsed=round(result_run.elapsed, 3),
        admission_ms=admission_ms,
        spawn_delay_ms=delay_ms,
    )
    expect(
        not pool_creation_notice(result_run.stderr),
        "the timed launch created another pool instead of reusing the prepared one: "
        f"{stderr_excerpt(result_run.stderr)}",
    )
    control_after, _keys_after = control_state(base_env)
    expect(
        control_after == control,
        f"the timed launch used a different control root: {control_after!r} != {control!r}",
    )
    started = started_records(records)
    recorded = terminal(records)
    if not started:
        # Distinguish the two shapes honestly: a pre-spawn expiry (the last
        # admission check saw the closed window, so nothing was launched and
        # this case exercised nothing) is not a late start.
        expect(
            recorded["outcome"] == "admission_deadline",
            f"the late start emitted no started record but reported {recorded['outcome']}: {recorded}",
        )
        expect_budget_diagnostic(result_run.stderr, "admission_stop", "late start")
        fail(
            "the late start reached pre-spawn admission expiry instead of the in-flight window: "
            f"preparation consumed the {admission_ms}ms budget before the {delay_ms}ms sleep, so no "
            "late start was observed (an unexercised fixture, not a product-stop claim)"
        )
    expect(len(started) == 1, f"the late start emitted {len(started)} started records")
    expect(started[0]["phase"] == "exec_tool", f"the late start was not the tool phase: {started[0]}")
    expect(started[0]["spawn_late"] is True, f"the late start was not reported: {started[0]}")
    expect(
        started[0]["admission_expired_before_start"] is True,
        f"the expired window was not reported: {started[0]}",
    )
    expect(started[0]["admission_ms"] == admission_ms, f"the admission axis is {started[0]}")
    overshoot = started[0]["admission_overshoot_ms"]
    expect(isinstance(overshoot, int) and overshoot >= 1, f"no positive admission overshoot: {started[0]}")
    observed = started[0]["admission_elapsed_ms"]
    expect(
        isinstance(observed, int) and observed > admission_ms,
        f"the observed start was not late against the clock: {started[0]}",
    )
    expect(recorded["outcome"] == "admission_deadline", f"the terminal outcome is {recorded['outcome']}")
    expect(recorded["stop_origin"] == "admission", f"the stop origin is {recorded['stop_origin']}")
    expect_budget_diagnostic(result_run.stderr, "admission_stop", "late start")
    # The delay-free follow-up runs the same tool in seed mode again: the same
    # slot must be reusable after the late start released it.
    try:
        os.unlink(mode)
    except OSError:
        pass
    follow = run([subject, "exec", "--", tool], cwd=crate, env=base_env, phase_seconds=30)
    expect(follow.returncode == 0, f"the slot was not released after a late start: {follow.returncode}")
    result(
        "late-start",
        True,
        f"seed prepared the pool, timed launch started {observed}ms into a {admission_ms}ms window "
        f"(overshoot {overshoot}ms) as admission_deadline, delay-free follow-up reused the slot",
    )


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
    record_detail("no-mutation/refused", refused.stderr, returncode=refused.returncode)
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
    record_detail("unbudgeted", result_run.stderr, returncode=result_run.returncode)
    expect_no_budget_diagnostic(result_run.stderr, "unbudgeted command")
    result("unbudgeted", True, "exit 3 preserved, no budget output")


# --------------------------------------------------------------------------
# Staging and the counted run.


# The staging profile is frozen apart from the build-parallelism constants
# above: opt-level, debug info, debug assertions, incremental, strip flags, the
# pinned toolchain, the offline lock and the scrubbed ambient variables.
STAGING_ENV_REQUIRED = {
    "RUSTUP_TOOLCHAIN": BUILDER,
    "RUSTUP_AUTO_INSTALL": "0",
    "RUSTC_WRAPPER": "",
    "RUSTC_WORKSPACE_WRAPPER": "",
    "CARGO_PROFILE_DEV_DEBUG": "0",
    "CARGO_PROFILE_DEV_DEBUG_ASSERTIONS": "true",
    "CARGO_PROFILE_DEV_OPT_LEVEL": "s",
    "CARGO_PROFILE_DEV_CODEGEN_UNITS": str(STAGE_CODEGEN_UNITS),
    "CARGO_INCREMENTAL": "0",
    "RUSTFLAGS": "-C strip=symbols",
    "CARGO_NET_OFFLINE": "true",
}
STAGING_SCRUBBED = (
    "CARGO",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_ENCODED_RUSTFLAGS",
    "OXIDE_CONFIG",
    "OXIDE_POOL",
)


# The staging build of this repository is the proof's only `--locked`
# invocation; fixture crates are std-only and never pass it. Defining the flag
# once keeps that single-invocation premise checkable in source.
LOCKED_FLAG = "--locked"


def staging_phase_bound() -> float:
    """The staging phase bound before it is clipped to the guard's deadline."""
    return float(PHASE_ALLOCATION_S["stage"])


def staging_recipe(stage: str) -> tuple[list[str], dict]:
    """The production staging recipe: the argv and environment it really uses.

    Callers pass the owned stage directory so target and build stay inside the
    monitored scratch. The child bound is applied by `timeout()` at the call
    site, clipped to the guard's own remaining time.
    """
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("CARGO_TARGET_", "CARGO_BUILD_", "SCCACHE_", "OXIDE_"))
        and key not in STAGING_SCRUBBED
    }
    env.update(STAGING_ENV_REQUIRED)
    env["CARGO_TARGET_DIR"] = os.path.join(stage, "target")
    env["CARGO_BUILD_BUILD_DIR"] = os.path.join(stage, "build")
    argv = ["cargo", "build", "-j", str(STAGE_JOBS), LOCKED_FLAG, "--offline", "--bin", "oxide"]
    return argv, env


def staging_recipe_problems(argv: list[str], env: dict, clipped_s: float, stage: str) -> list[str]:
    """Every deviation from the frozen staging recipe; empty means conformant.

    The pure checks use this on the production `staging_recipe()` output and on
    deliberately mutated copies, so a regression in jobs, codegen units,
    profile flags, a leaked encoded flag, a mis-scoped target directory or a
    stretched child bound is reported as a concrete problem instead of being
    silently accepted.
    """
    problems: list[str] = []
    expected_argv = ["cargo", "build", "-j", str(STAGE_JOBS), LOCKED_FLAG, "--offline", "--bin", "oxide"]
    if argv != expected_argv:
        problems.append(f"staging argv is {argv!r}, not {expected_argv!r}")
    if STAGE_JOBS != 4:
        problems.append(f"staging must configure 4 Cargo jobs, not {STAGE_JOBS}")
    if STAGE_CODEGEN_UNITS != 16:
        problems.append(f"staging must configure 16 codegen units, not {STAGE_CODEGEN_UNITS}")
    if FIXTURE_JOBS != 1:
        problems.append(f"fixtures must keep one Cargo job, not {FIXTURE_JOBS}")
    for key, value in STAGING_ENV_REQUIRED.items():
        if env.get(key) != value:
            problems.append(f"staging env {key} is {env.get(key)!r}, not {value!r}")
    if env.get("CARGO_ENCODED_RUSTFLAGS"):
        problems.append("staging leaked CARGO_ENCODED_RUSTFLAGS")
    for key in ("CARGO_TARGET_DIR", "CARGO_BUILD_BUILD_DIR"):
        if not str(env.get(key, "")).startswith(stage):
            problems.append(f"staging env {key} is not inside the owned stage directory")
    if not 0 < clipped_s <= staging_phase_bound():
        problems.append(f"staging child bound {clipped_s} is outside the {staging_phase_bound()} s phase")
    return problems


def stage_subject(scratch: str) -> str:
    """Stage this clean HEAD's debug oxide: plain Cargo, offline, in scratch."""
    stage = private_dir(os.path.join(scratch, "stage"))
    log = os.path.join(stage, "stage.log")
    argv, env = staging_recipe(stage)
    stage_timeout = timeout(staging_phase_bound())
    problems = staging_recipe_problems(argv, env, stage_timeout, stage)
    expect(not problems, "the staging recipe drifted: " + "; ".join(problems))
    with open(log, "w", encoding="utf-8") as handle:
        build = subprocess.run(
            argv,
            cwd=REPO,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            timeout=stage_timeout,
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
    # Capability evidence comes only from the effect-free probe in
    # probe_capabilities(); names inside an executable are not evidence.
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


CASE_FUNCTIONS = (
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
)
# A diagnostic continues only with cases that are independent of the failed
# one. Every case labels its own private HOME, CARGO_HOME, config, pool and
# checkout paths, so no case reads or writes another case's fixture state; the
# staged candidate and the scratch root are immutable prerequisites, and a
# failure there aborts instead of continuing. The empty lists are therefore
# deliberate declarations, not an unexamined assumption.
DIAGNOSTIC_DEPENDENCIES: dict[str, list[str]] = {entry.__name__: [] for entry in CASE_FUNCTIONS}


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

    def prepare(self) -> None:
        """Prerequisites shared by the counted run and the diagnostic.

        A failure here is not a recoverable oracle failure: there is no
        identified candidate to test, so a diagnostic aborts.
        """
        self.commit = git("rev-parse", "HEAD")
        self.tree = git("rev-parse", "HEAD^{tree}")
        self.subject = stage_subject(self.scratch)
        self.digest = sha256_file(self.subject)
        expect(len(self.digest) == 64, "the staged subject digest is malformed")
        # The effect-free capability handshake runs before any private HOME,
        # CARGO_HOME, config or pool fixture is created.
        self.capabilities = probe_capabilities(self.subject, self.scratch)
        self.root = private_dir(os.path.join(self.scratch, "fixture-budgets"))
        # Every fixture pool is a scratch pool beneath this one declared root.
        global POOL_SCRATCH_ROOT
        POOL_SCRATCH_ROOT = private_dir(os.path.join(self.scratch, POOL_SCRATCH_LEAF))

    def run(self) -> None:
        self.prepare()
        for entry in CASE_FUNCTIONS:
            custody_guard(f"before {entry.__name__}")
            entry(self.root, self.subject)
            custody_guard(f"after {entry.__name__}")
            work_left()
        say(f"cases run: {', '.join(CASES)}")

    def run_diagnostic(self, summary: dict) -> None:
        """Keep-going collection: record recoverable failures, then continue.

        Only dependency-independent cases continue; an unknown child or writer,
        custody uncertainty, a prerequisite failure or an exhausted deadline
        aborts. The summary is filled in place so an abort still has truthful
        facts, and it is never a counted PASS.
        """
        global PRIMARY_FAULT
        self.prepare()
        pending = list(CASE_FUNCTIONS)
        while pending:
            name = pending[0].__name__
            fault_before = PRIMARY_FAULT
            # Custody is shared state: pre-existing uncertainty stops dispatch.
            try:
                custody_guard("before dispatch")
            except Abort as error:
                summary["aborted"] = f"{name}: {error}"
                summary["skipped"] = [entry.__name__ for entry in pending]
                raise
            function = pending.pop(0)
            try:
                function(self.root, self.subject)
            except Abort as error:
                summary["aborted"] = f"{name}: {PRIMARY_FAULT or error}"
                summary["skipped"] = [entry.__name__ for entry in pending]
                PRIMARY_FAULT = fault_before
                raise
            except Fault as error:
                text = PRIMARY_FAULT or str(error) or "assertion failed"
                summary["failures"].append(
                    {
                        "case": name,
                        "fault": text,
                        "labels": list(CASES),
                        "evidence": list(FAULT_DETAIL),
                    }
                )
                say(f"diagnostic: {name} failed ({text}); continuing with independent cases")
                PRIMARY_FAULT = fault_before
                try:
                    custody_guard("after a recoverable failure")
                except Abort as custody_error:
                    summary["aborted"] = f"{name}: {custody_error}"
                    summary["skipped"] = [entry.__name__ for entry in pending]
                    raise
                summary["continued_after_failure"].append(name)
                continue
            # A nominally successful case can still have left unknown custody.
            try:
                custody_guard("after a successful case")
            except Abort as custody_error:
                summary["aborted"] = f"{name}: {custody_error}"
                summary["skipped"] = [entry.__name__ for entry in pending]
                raise
            summary["completed"].append(name)
        say(f"diagnostic cases run: {', '.join(CASES)}")

    def document(self, fault: str | None, diagnostic: dict | None = None) -> dict:
        planned = [entry["name"] for entry in plan_document()["cases"]]
        recorded = fault
        if diagnostic is not None and diagnostic.get("failures"):
            # The earliest recorded oracle failure stays the primary fault even
            # when a later custody abort ends the collection; the abort itself
            # remains visible under diagnostic.aborted.
            recorded = diagnostic["failures"][0]["fault"]
        # The live evidence ring can be overwritten by later cases, so a
        # diagnostic keeps the first failure's evidence snapshot as primary.
        evidence = list(FAULT_DETAIL)
        if diagnostic is not None and diagnostic.get("failures"):
            first_evidence = diagnostic["failures"][0].get("evidence") or []
            if first_evidence:
                evidence = list(first_evidence)
        failed = [entry for entry in RESULTS if entry["result"] != "pass"]
        if recorded is not None:
            failed = failed + [{"case": "primary", "result": "fail", "detail": recorded}]
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
            "first_fault_evidence": evidence,
            "primary_fault": recorded,
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
            "result": ("pass" if fault is None else "fail") if diagnostic is None else "diagnostic",
            **({"mode": "diagnostic", "qualified": False, "diagnostic": diagnostic} if diagnostic is not None else {}),
        }




# --------------------------------------------------------------------------
# Modes.

def self_check() -> None:
    """Pure checks of the argument boundary, plan, arithmetic and oracles."""
    global STAGE_JOBS, STAGE_CODEGEN_UNITS, FIXTURE_JOBS
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
    expect("not asserted" in cumulative["oracle"], "the cumulative oracle must not gate closeout latency")
    nested = [entry for entry in plan["cases"] if entry["name"] == "nested-outer-window"][0]
    expect("outer 2s window" in nested["oracle"], "the nested oracle must attribute the stop to the outer window")
    late = [entry for entry in plan["cases"] if entry["name"] == "late-start"][0]
    expect(
        "pre-spawn expiry" in late["oracle"]
        and "seed launch" in late["oracle"]
        and "6s" in late["oracle"]
        and "7s" in late["oracle"]
        and "delay-free follow-up" in late["oracle"],
        "the late-start seed premise or its pre-spawn distinction changed",
    )
    expect(remaining_quiet() >= 0.0, "the quiet remaining clock must not raise")
    expect(custody_bound() >= 0.05, "the custody bound must stay positive")
    # Human budget diagnostics are recognised by exact grammar, never by the
    # lowercase substring that ordinary Cargo/path output can contain.
    innocent = (
        "   Compiling oxide-budget-both v0.1.0 (/private/tmp/oxide-budget-both)\n"
        "    Finished `dev` profile [unoptimized + debuginfo] target(s)\n"
        "note: the budget profile is an ordinary package name\n"
    )
    expect(budget_diagnostics(innocent) == [], "innocent output containing 'budget' was read as a diagnostic")
    expect(budget_diagnostics("") == [], "empty stderr reported a diagnostic")
    expect(budget_diagnostics(None) == [], "missing stderr reported a diagnostic")
    expect(
        budget_diagnostics("oxide: stopped by the admission budget (1000ms) before a usable child start\n")
        == ["admission_stop"],
        "the admission stop grammar is not recognised",
    )
    expect(
        budget_diagnostics("oxide: stopped by the execution budget (5000ms)\n") == ["execution_stop"],
        "the execution stop grammar is not recognised",
    )
    expect(
        budget_diagnostics(DIAGNOSTIC_TEXT["event_loss"] + "\n") == ["event_loss"],
        "the loss note grammar is not recognised",
    )
    expect(
        budget_diagnostics("prefix oxide: stopped by the execution budget (5000ms)\n") == [],
        "an embedded stop line was recognised from a non-exact line",
    )
    expect(
        budget_diagnostics("oxide: the budget event channel became unusable\n") == [],
        "a truncated loss note became positive evidence",
    )
    expect(
        budget_diagnostics("oxide: stopped by the execution budget (x ms)\n") == [],
        "a malformed duration became positive evidence",
    )
    expect("truncated" in stderr_excerpt("x" * (MAX_EXCERPT_CHARS * 2)), "a long excerpt was not marked truncated")
    expect(stderr_excerpt(None) == "<no stderr captured>", "a missing stderr was not reported honestly")
    FAULT_DETAIL.clear()
    for index in range(5):
        record_detail(f"probe-{index}", "note " * 200)
    expect(len(FAULT_DETAIL) == MAX_FAULT_DETAIL, "the first-fault evidence is not bounded")
    expect(MAX_FAULT_DETAIL <= 3, "the first-fault evidence bound grew")
    expect(len(FAULT_DETAIL[-1]["stderr"]) <= MAX_EXCERPT_CHARS + 80, "a recorded excerpt exceeded its bound")
    FAULT_DETAIL.clear()
    # Lockfile premise: one staged build of this repository, whose committed lock
    # is present; no fixture is invoked with --locked.
    proof_source = open(SCRIPT, encoding="utf-8").read()
    expect(os.path.isfile(os.path.join(REPO, "Cargo.lock")), "the staged repository has no Cargo.lock for --locked")
    locked_flag = "--" + "locked"
    locked = [
        node.value
        for node in ast.walk(ast.parse(proof_source))
        if isinstance(node, ast.Constant) and node.value == locked_flag
    ]
    expect(len(locked) == 1, "the proof gained a --locked call its lockfile premise does not cover")
    expect(
        plan["provenance"]["lockfile"].startswith("the only --locked call is the staging build")
        and "beside each fixture manifest" in plan["provenance"]["lockfile"],
        "the lockfile premise changed",
    )
    # Staging parallelism and its frozen surroundings: the production
    # `staging_recipe()` output is the subject, not a copy of what the test
    # expects, and each mutation below must surface as a concrete problem.
    stage_dir = "/private/tmp/oxide-proof-selfcheck/stage"
    stage_argv, stage_env = staging_recipe(stage_dir)
    stage_bound = clip(staging_phase_bound(), 1000.0)
    expect(stage_argv == ["cargo", "build", "-j", "4", LOCKED_FLAG, "--offline", "--bin", "oxide"], "the staging argv is no longer 4 jobs with the locked/offline flags")
    expect(stage_env["CARGO_PROFILE_DEV_CODEGEN_UNITS"] == "16", "the staging codegen units are not 16")
    expect(FIXTURE_JOBS == 1, "fixtures must keep one Cargo job")
    expect(clip(staging_phase_bound(), 12.5) == 12.5, "the staging bound must still clip to the guard's remaining time")
    expect(
        staging_recipe_problems(stage_argv, stage_env, stage_bound, stage_dir) == [],
        "the production staging recipe drifted: "
        + "; ".join(staging_recipe_problems(stage_argv, stage_env, stage_bound, stage_dir)),
    )
    expect(plan["concurrency"]["stage_cargo_jobs"] == 4, "the plan does not report 4 staging Cargo jobs")
    expect(plan["concurrency"]["stage_codegen_units"] == 16, "the plan does not report 16 staging codegen units")
    expect(plan["concurrency"]["fixture_cargo_jobs"] == 1, "the plan does not report one fixture Cargo job")
    expect("not a CPU or thread hard cap" in plan["concurrency"]["not_a_cap"], "the plan must deny a CPU or thread hard cap")
    saved_parallelism = (STAGE_JOBS, STAGE_CODEGEN_UNITS, FIXTURE_JOBS)
    try:
        STAGE_JOBS = 1
        mutated_argv, mutated_env = staging_recipe(stage_dir)
        expect(
            any("4 Cargo jobs" in problem for problem in staging_recipe_problems(mutated_argv, mutated_env, stage_bound, stage_dir)),
            "a one-job staging mutation did not fail its oracle",
        )
        STAGE_JOBS = saved_parallelism[0]
        STAGE_CODEGEN_UNITS = 1
        mutated_argv, mutated_env = staging_recipe(stage_dir)
        expect(
            any("16 codegen units" in problem for problem in staging_recipe_problems(mutated_argv, mutated_env, stage_bound, stage_dir)),
            "a one-codegen-unit staging mutation did not fail its oracle",
        )
        STAGE_CODEGEN_UNITS = saved_parallelism[1]
        FIXTURE_JOBS = 2
        expect(
            any("fixtures must keep one Cargo job" in problem for problem in staging_recipe_problems(stage_argv, stage_env, stage_bound, stage_dir)),
            "a two-job fixture mutation did not fail its oracle",
        )
    finally:
        STAGE_JOBS, STAGE_CODEGEN_UNITS, FIXTURE_JOBS = saved_parallelism
    drifted_env = dict(stage_env)
    drifted_env["CARGO_PROFILE_DEV_OPT_LEVEL"] = "0"
    expect(
        any("CARGO_PROFILE_DEV_OPT_LEVEL" in problem for problem in staging_recipe_problems(stage_argv, drifted_env, stage_bound, stage_dir)),
        "an opt-level drift was accepted",
    )
    stripped_env = dict(stage_env)
    stripped_env.pop("RUSTFLAGS")
    expect(
        any("RUSTFLAGS" in problem for problem in staging_recipe_problems(stage_argv, stripped_env, stage_bound, stage_dir)),
        "a missing strip flag was accepted",
    )
    leaked_env = dict(stage_env)
    leaked_env["CARGO_ENCODED_RUSTFLAGS"] = "-Cdebuginfo=2"
    expect(
        any("CARGO_ENCODED_RUSTFLAGS" in problem for problem in staging_recipe_problems(stage_argv, leaked_env, stage_bound, stage_dir)),
        "a leaked CARGO_ENCODED_RUSTFLAGS was accepted",
    )
    escaped_env = dict(stage_env)
    escaped_env["CARGO_BUILD_BUILD_DIR"] = "/private/elsewhere/build"
    expect(
        any("CARGO_BUILD_BUILD_DIR" in problem for problem in staging_recipe_problems(stage_argv, escaped_env, stage_bound, stage_dir)),
        "a staging build directory outside the owned stage was accepted",
    )
    expect(
        any(
            "staging child bound" in problem
            for problem in staging_recipe_problems(stage_argv, stage_env, stage_bound + 1.0, stage_dir)
        ),
        "a stretched staging child bound was accepted",
    )
    expect("grammar" in plan["provenance"]["diagnostics"], "the diagnostic-grammar premise changed")
    # Diagnostic mode is declared, unqualified and dependency-ordered.
    expect(parse_arguments(["--diagnostic"]) == ("diagnostic", None), "--diagnostic must parse")
    expect(plan["diagnostic"]["qualified"] is False, "the diagnostic plan must never be qualified")
    expect(plan["diagnostic"]["default_mode"] == "fail-fast", "the default mode must stay fail-fast")
    expect("--diagnostic" in plan["modes"], "the diagnostic mode is missing from the plan")
    expect(
        len(plan["diagnostic"]["dependencies"]) == len(CASE_FUNCTIONS),
        "the diagnostic dependency map does not cover every case",
    )
    expect(
        all(not values for values in plan["diagnostic"]["dependencies"].values()),
        "a diagnostic case declares a dependency that the independence premise denies",
    )
    expect(
        "private HOME" in plan["diagnostic"]["independence"],
        "the diagnostic independence premise changed",
    )
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
    # Current-main oracle pins (S4 sweep): the supervisor pin must name the bytes
    # on disk, so a runner change without a proof update fails here, pure.
    runner_on_disk = sha256_file(os.path.join(REPO, "scripts", "bounded-test.py"))
    expect(
        runner_on_disk == EXPECTED_RUNNER_SHA256,
        f"the supervisor pin is stale: scripts/bounded-test.py is {runner_on_disk}",
    )
    # Pool-creation notices: both product forms are recognised, nothing else is.
    expect(
        pool_creation_notice("oxide: creating a scratch build pool on volume 1234; not for ordinary project builds\n"),
        "the scratch pool-creation notice is not recognised",
    )
    expect(
        pool_creation_notice("oxide: creating the build pool at /x (256 MB soft budget, directory-budget-v1, budget-only)\n"),
        "the ordinary pool-creation notice is not recognised",
    )
    expect(not pool_creation_notice("oxide: waiting for a build slot\n"), "a waiting line was read as pool creation")
    expect(not pool_creation_notice(None), "missing stderr was read as pool creation")
    # Scratch root judgement (S4): canonical, owned, directory, never the fallback.
    fake_root = "/Volumes/Builds/scratch/oxide/run.example/scratch"
    expect(scratch_root_refusal(fake_root, fake_root, True, False, 501, 501) is None, "a selected-volume root was refused")
    expect(
        "fallback" in (scratch_root_refusal("/private/tmp/x", "/private/tmp/x", True, False, 501, 501) or ""),
        "the /private/tmp fallback was not refused",
    )
    expect(scratch_root_refusal(fake_root, "/Volumes/Other/x", True, False, 501, 501) is not None, "a non-canonical root was accepted")
    expect(scratch_root_refusal(fake_root, fake_root, True, False, 0, 501) is not None, "a foreign-owned root was accepted")
    expect(scratch_root_refusal(fake_root, fake_root, True, True, 501, 501) is not None, "a symlinked root was accepted")
    # Fixture environment contract (S4), pure: every fixture pool is a scratch
    # pool strictly inside one declared root, with an explicit cap, reaching no
    # /private/tmp value and inheriting no compiler flags.
    pool_root = os.path.join(fake_root, POOL_SCRATCH_LEAF)
    fixture_root = os.path.join(fake_root, "fixture-budgets")
    fixture_values = fixture_environment(fixture_root, "admission", pool_root)
    expect(fixture_values["OXIDE_SCRATCH_ROOT"] == pool_root, "the fixture does not declare the scratch-pool root")
    expect(
        fixture_values["OXIDE_POOL"].startswith(pool_root + os.sep),
        "the fixture pool escapes the declared scratch-pool root",
    )
    expect(fixture_values["OXIDE_POOL_CAP"] == FIXTURE_POOL_CAP, "the fixture pool cap is not the explicit cap")
    expect(fixture_values["OXIDE_POOL_BACKEND"] == "directory-budget", "the fixture backend changed")
    expect(fixture_values["OXIDE_TEST_HOME_FROM_ENV"] == "1", "the account-home control is missing")
    expect(fixture_values["TMPDIR"] == fixture_root == fixture_values["TMP"] == fixture_values["TEMP"], "temp is not the fixture root")
    for key in ("HOME", "CARGO_HOME", "OXIDE_CONFIG", "OXIDE_SCRATCH_ROOT", "OXIDE_POOL", "TMPDIR"):
        expect(
            not fixture_values[key].startswith(("/private/tmp", "/tmp")),
            f"fixture {key} names the internal /private/tmp root",
        )
    expect(
        "RUSTFLAGS" not in fixture_values and "CARGO_ENCODED_RUSTFLAGS" not in fixture_values,
        "a fixture inherits compiler flags",
    )
    # The premise checklist: nine items, each naming source that exists.
    expect(len(PREMISE_CHECKLIST) == 9, "the premise checklist does not have nine items")
    for key, entry in PREMISE_CHECKLIST.items():
        for name in entry["source"]:
            expect(name in globals(), f"premise {key} names {name}, which is not in the script")
    expect(
        plan["premise_checklist"] == {key: entry["disposition"] for key, entry in PREMISE_CHECKLIST.items()},
        "the plan does not carry the premise dispositions",
    )
    print("self-check: ok (pure argument, plan, allocation, sequence, descriptor and recipe checks; not evidence)")


def self_test() -> None:
    """Execute the corrected case bodies and lifecycle paths with faked seams.

    No child is spawned, no candidate is staged and no pool is created. The
    fakes enforce the same public budget-prefix order as the product, record
    typed event streams, write the heartbeat counter the cumulative case reads,
    and provide holders only for a non-empty subject. Each mutation below must
    fail its own oracle, so a body that stopped asserting, that placed a budget
    after `-j`, or that started an empty holder cannot pass.
    """
    import importlib.util
    import tempfile

    scratch = tempfile.mkdtemp(prefix="budget-selftest-", dir="/private/tmp")

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
        module.fixture_env = lambda root, label: {
            "OXIDE_POOL": os.path.join(root, f"pool-{label}"),
            "HOME": os.path.join(root, f"home-{label}"),
            "OXIDE_CONFIG": os.path.join(root, f"config-{label}.toml"),
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        }
        module.create_crate = lambda root, label: module.private_dir(
            os.path.join(root, "checkouts", label)
        )
        module.ticket_count = lambda env: 0
        module.control_state = lambda env: (os.path.join(scratch, "control"), ["version"])
        module.CASES.clear()
        module.RESULTS.clear()
        return module

    def flag(argv, name):
        return argv[argv.index(name) + 1] if name in argv else None

    def millis(text):
        """Parse the product's ms|s budget grammar for the faked axis fields."""
        if text is None:
            return None
        if text.endswith("ms"):
            return int(text[:-2])
        if text.endswith("s"):
            return int(text[:-1]) * 1000
        return int(text)

    def make_start(module, beats):
        """Fake `run`: refuses bad prefix order, writes events and heartbeat."""

        def fake_run(argv, *, cwd, env, phase_seconds, pass_fds=(), stdin=None, stdout=None, stderr=None):
            if not module.budget_prefix_ok(argv[2:]):
                return completed(argv, 2, "", "error: unexpected argument after the option prefix\n")
            if "--budget-events" not in argv:
                return completed(argv, 0, "", "")
            fd = int(flag(argv, "--budget-events"))
            program = argv[argv.index("--") + 1] if "--" in argv else ""
            if "run" in argv:
                root = os.path.dirname(os.path.dirname(cwd))
                with open(os.path.join(root, "program-heartbeats"), "w", encoding="utf-8") as handle:
                    handle.write("".join(f"{index}\n" for index in range(beats)))
                os.write(fd, stream([
                    {"seq": 1, "event": "admitted_child_started", "phase": "primary_cargo"},
                    {"seq": 2, "event": "admitted_child_started", "phase": "run_program"},
                    {"seq": 3, "event": "finished", "outcome": "execution_deadline",
                     "stop_origin": "execution", "exit_code": 125},
                ]))
                note = "oxide: stopped by the execution budget (5000ms)\n"
                # The cumulative case redirects stderr to a file, so the fake
                # must write there rather than return a captured string.
                if hasattr(stderr, "write"):
                    stderr.write(note)
                    stderr.flush()
                    return completed(argv, 125, "", "")
                return completed(argv, 125, "", note)
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
                return completed(argv, 125, "", "oxide: stopped by the execution budget (2000ms)\n")
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

    def with_stderr(fake, suffix):
        """Append human output to a fake command's stderr."""

        def wrapped(argv, **kwargs):
            answer = fake(argv, **kwargs)
            answer.stderr = answer.stderr + suffix
            return answer

        return wrapped

    def plain_run(text, code):
        """A fake command with no typed stream, for cases that need none."""

        def fake(argv, *, cwd, env, phase_seconds, pass_fds=(), stdin=None, stdout=None, stderr=None):
            return completed(argv, code, "", text)

        return fake

    def holder_start(module):
        """Fake start for holders: refuses an empty program, creates `.held`."""

        def fake_start(argv, *, cwd, env, **kwargs):
            expect(bool(argv[0]), "a holder was started with no candidate subject")
            program = argv[argv.index("--") + 1] if "--" in argv else ""

            class Holder:
                returncode = None
                pid = os.getpid()

                def poll(self):
                    return self.returncode

                def terminate(self):
                    self.returncode = 0

                def wait(self, timeout=None):
                    self.returncode = 0
                    return 0

                def kill(self):
                    self.returncode = -9

            label = os.path.basename(program).split(".")[0]
            root = os.path.dirname(os.path.dirname(cwd))
            with open(os.path.join(root, f"{label}.held"), "w", encoding="utf-8") as handle:
                handle.write("held")
            return Holder()

        return fake_start

    # ---- the real helper signature is the contract every case call meets ----
    import inspect

    contract = load()
    parameters = set(inspect.signature(contract.run).parameters)
    expect({"stdout", "stderr"} <= parameters, "run() must expose its output streams")
    expect(contract.body_signature_drift(SCRIPT) == [], "a case passes a keyword run() does not accept")
    contract.run.__defaults__  # noqa: B018 - the signature above is the check
    mutated = os.path.join(scratch, "mutated-proof.py")
    original = open(SCRIPT, encoding="utf-8").read()
    with open(mutated, "w", encoding="utf-8") as handle:
        handle.write(original.replace("stderr=stderr_file,", "stderr_file=stderr_file,", 1))
    expect(
        bool(contract.body_signature_drift(mutated)),
        "the drift sweep missed an undeclared output-stream keyword",
    )

    # ---- fast start: three axes pass; a nulled axis must fail ----
    module = load()
    module.run = make_start(module, beats=0)
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

    # ---- the human diagnostic oracle: innocent output containing "budget"
    # passes; each exact product stop/loss line fails the same negative oracle.
    innocent = (
        "   Compiling oxide-budget-both v0.1.0 (/private/tmp/oxide-budget-both)\n"
        "note: the budget profile is an ordinary package name\n"
    )
    tolerated = load()
    tolerated.run = with_stderr(make_start(tolerated, beats=0), innocent)
    expect(
        fault_of(tolerated, lambda: tolerated.case_fast_start_axes(scratch, "/bin/true")) is None,
        "innocent output containing 'budget' failed the fast-start oracle",
    )
    for label, line in (
        ("admission", "oxide: stopped by the admission budget (30000ms) before a usable child start\n"),
        ("execution", "oxide: stopped by the execution budget (30000ms)\n"),
        ("loss", "oxide: the budget event channel became unusable; typed results are unavailable for this command\n"),
    ):
        noisy = load()
        noisy.run = with_stderr(make_start(noisy, beats=0), line)
        fault = fault_of(noisy, lambda: noisy.case_fast_start_axes(scratch, "/bin/true"))
        expect(
            fault is not None and "budget diagnostic" in fault,
            f"a real {label} diagnostic passed the fast-start negative oracle: {fault}",
        )

    # ---- budget prefix order: the fake refuses late flags, and a mutation of
    # the actual body argv is caught by that same rule ----
    expect(load().budget_prefix_ok(["--admission-budget", "1s", "-j", "1"]), "a correct prefix was rejected")
    expect(not load().budget_prefix_ok(["-j", "1", "--admission-budget", "1s"]), "a late budget passed the prefix oracle")

    # ---- late start: a delay-free seed establishes the prepared state, the
    # timed launch must observe a real in-flight late start, and each broken
    # premise fails with its own message. ----
    admission_note = "oxide: stopped by the admission budget (6000ms) before a usable child start\n"
    pool_note = "oxide: creating the build pool at /private/tmp/fixture/pool-late (budget-only)\n"
    started_late = {
        "seq": 1, "event": "admitted_child_started", "phase": "exec_tool",
        "spawn_late": True, "admission_expired_before_start": True,
        "admission_ms": 6000, "execution_ms": 60000,
        "admission_elapsed_ms": 8500, "admission_overshoot_ms": 2500,
    }
    finished_admission = {
        "seq": 2, "event": "finished", "outcome": "admission_deadline",
        "stop_origin": "admission", "drain": "not_reached", "lease": "released", "exit_code": 124,
    }

    def late_fake(
        module_ref,
        *,
        calls,
        seed_code=0,
        seed_note=pool_note,
        timed_note=admission_note,
        records=(started_late, finished_admission),
        timed_code=124,
        follow_code=0,
        timed_creates_pool=False,
    ):
        """Fake the seed, timed and follow-up launches of the real case body."""

        def fake(argv, *, cwd, env, phase_seconds, pass_fds=(), stdin=None, stdout=None, stderr=None):
            tool = argv[-1]
            mode_path = os.path.join(os.path.dirname(tool), "late.run")
            calls.append(
                {
                    "argv": list(argv),
                    "cwd": cwd,
                    "tool": tool,
                    "mode": os.path.exists(mode_path),
                    "hook": env.get("OXIDE_TEST_SPAWN_DELAY_MS"),
                }
            )
            if "--budget-events" not in argv:
                if len(calls) == 1:
                    return completed(argv, seed_code, "", seed_note)
                return completed(argv, follow_code, "", "")
            if not module_ref.budget_prefix_ok(argv[2:]):
                return completed(argv, 2, "", "error: unexpected argument after the option prefix\n")
            fd = int(flag(argv, "--budget-events"))
            os.write(fd, stream(records))
            note = (pool_note if timed_creates_pool else "") + timed_note
            return completed(argv, timed_code, "", note)

        return fake

    def late_scratch(tag):
        """A distinct owned root per faked late-start invocation."""
        return os.path.join(scratch, f"late-{tag}")

    late_ok = load()
    calls = []
    late_ok.run = late_fake(late_ok, calls=calls)
    expect(
        fault_of(late_ok, lambda: late_ok.case_late_start(late_scratch("ok"), "/bin/true")) is None,
        "an observed late start with a delay-free seed should pass",
    )
    expect(len(calls) == 3, f"the late-start case did not run seed, timed launch and follow-up: {len(calls)}")
    expect(
        calls[0]["hook"] is None and calls[2]["hook"] is None,
        f"the seed or the follow-up carried the spawn-delay hook: {calls}",
    )
    expect(calls[1]["hook"] == "7000", f"the timed launch did not carry the delay: {calls[1]}")
    expect(
        calls[0]["mode"] is False and calls[1]["mode"] is True and calls[2]["mode"] is False,
        f"the fixture mode file was not controlled per launch: {calls}",
    )
    expect(
        {entry["cwd"] for entry in calls} == {calls[0]["cwd"]}
        and {entry["tool"] for entry in calls} == {calls[0]["tool"]},
        f"the seed, timed launch and follow-up used different fixture identities: {calls}",
    )

    seed_broken = load()
    seed_broken.run = late_fake(seed_broken, calls=[], seed_code=2, seed_note="")
    fault = fault_of(seed_broken, lambda: seed_broken.case_late_start(late_scratch("seed"), "/bin/true"))
    expect(fault is not None and "seed launch failed" in fault, f"a failed seed passed: {fault}")

    no_pool = load()
    no_pool.run = late_fake(no_pool, calls=[], seed_note="")
    fault = fault_of(no_pool, lambda: no_pool.case_late_start(late_scratch("nopool"), "/bin/true"))
    expect(fault is not None and "did not create the fixture pool" in fault, f"an unseeded pool passed: {fault}")

    second_pool = load()
    second_pool.run = late_fake(second_pool, calls=[], timed_creates_pool=True)
    fault = fault_of(second_pool, lambda: second_pool.case_late_start(late_scratch("warm"), "/bin/true"))
    expect(fault is not None and "created another pool" in fault, f"a cold timed launch passed: {fault}")

    hook_leak = load()
    leaked = hook_leak.fixture_env
    hook_leak.fixture_env = lambda root, label: dict(leaked(root, label), OXIDE_TEST_SPAWN_DELAY_MS="7000")
    hook_leak.run = late_fake(hook_leak, calls=[])
    fault = fault_of(hook_leak, lambda: hook_leak.case_late_start(late_scratch("hook"), "/bin/true"))
    expect(
        fault is not None and "must not carry the spawn-delay hook" in fault,
        f"a leaked spawn-delay hook passed the seed premise: {fault}",
    )

    late_pre = load()
    late_pre.run = late_fake(
        late_pre,
        calls=[],
        records=(
            {"seq": 1, "event": "finished", "outcome": "admission_deadline",
             "stop_origin": "admission", "drain": "not_reached", "lease": "released", "exit_code": 124},
        ),
    )
    fault = fault_of(late_pre, lambda: late_pre.case_late_start(late_scratch("pre"), "/bin/true"))
    expect(
        fault is not None and "pre-spawn admission expiry" in fault,
        f"a pre-spawn expiry was not distinguished from a late start: {fault}",
    )

    late_early = load()
    on_time = dict(started_late, spawn_late=False, admission_expired_before_start=False,
                   admission_elapsed_ms=500, admission_overshoot_ms=None)
    late_early.run = late_fake(
        late_early,
        calls=[],
        records=(
            on_time,
            {"seq": 2, "event": "finished", "outcome": "child_exit", "stop_origin": None,
             "drain": "complete", "lease": "released", "exit_code": 0},
        ),
    )
    fault = fault_of(late_early, lambda: late_early.case_late_start(late_scratch("early"), "/bin/true"))
    expect(fault is not None and "was not reported" in fault, f"an on-time start passed as late: {fault}")

    late_overshoot = load()
    late_overshoot.run = late_fake(
        late_overshoot,
        calls=[],
        records=(dict(started_late, admission_overshoot_ms=0), finished_admission),
    )
    fault = fault_of(late_overshoot, lambda: late_overshoot.case_late_start(late_scratch("overshoot"), "/bin/true"))
    expect(fault is not None and "no positive admission overshoot" in fault, f"a zero overshoot passed: {fault}")

    late_bad = load()
    late_bad.run = late_fake(
        late_bad,
        calls=[],
        timed_code=0,
        timed_note="",
        records=(
            dict(started_late, spawn_late=False, admission_expired_before_start=False,
                 admission_elapsed_ms=500, admission_overshoot_ms=None),
            {"seq": 2, "event": "finished", "outcome": "child_exit", "stop_origin": None,
             "drain": "complete", "lease": "released", "exit_code": 0},
        ),
    )
    fault = fault_of(late_bad, lambda: late_bad.case_late_start(late_scratch("bad"), "/bin/true"))
    expect(fault is not None and "late start exited" in fault, f"a non-late start passed: {fault}")

    # ---- nested: a natural outer exit must fail ----
    nested_ok = load()
    nested_ok.run = make_start(nested_ok, beats=0)
    expect(fault_of(nested_ok, lambda: nested_ok.case_nested(scratch, "/bin/true")) is None,
           "nested should pass")
    nested_bad = load()

    def natural_exit(argv, **kwargs):
        if not nested_bad.budget_prefix_ok(argv[2:]):
            return completed(argv, 2, "", "error: unexpected argument after the option prefix\n")
        if "--budget-events" not in argv:
            return completed(argv, 0, "", "")
        fd = int(flag(argv, "--budget-events"))
        os.write(fd, stream([
            {"seq": 1, "event": "admitted_child_started", "phase": "exec_tool", "spawn_late": False},
            {"seq": 2, "event": "finished", "outcome": "child_exit", "stop_origin": None, "exit_code": 0},
        ]))
        return completed(argv, 0, "", "")

    nested_bad.run = natural_exit
    fault = fault_of(nested_bad, lambda: nested_bad.case_nested(scratch, "/bin/true"))
    expect(fault is not None and "the outer command exited" in fault, f"a natural outer exit passed: {fault}")

    # ---- cumulative: ~10 heartbeats pass, ~25 (a reset window) fail ----
    one_window = load()
    one_window.run = make_start(one_window, beats=10)
    expect(fault_of(one_window, lambda: one_window.case_cumulative_run(scratch, "/bin/true")) is None,
           "the one-window heartbeat count should pass")
    reset = load()
    reset.run = make_start(reset, beats=25)
    fault = fault_of(reset, lambda: reset.case_cumulative_run(scratch, "/bin/true"))
    expect(fault is not None and "window was reset" in fault, f"a reset window passed: {fault}")

    # ---- a positive stop oracle needs the exact grammar, and the actual
    # execution-deadline / preparation / unbudgeted bodies are driven here.
    def deadline_run(note):
        def fake(argv, *, cwd, env, phase_seconds, pass_fds=(), stdin=None, stdout=None, stderr=None):
            if "--budget-events" not in argv:
                return completed(argv, 0, "", "")
            fd = int(flag(argv, "--budget-events"))
            os.write(fd, stream([
                {"seq": 1, "event": "admitted_child_started", "phase": "exec_tool"},
                {"seq": 2, "event": "finished", "outcome": "execution_deadline",
                 "stop_origin": "execution", "drain": "complete", "lease": "released",
                 "exit_code": 125},
            ]))
            return completed(argv, 125, "", note)

        return fake

    deadline_ok = load()
    deadline_ok.run = deadline_run("oxide: stopped by the execution budget (1000ms)\n")
    expect(
        fault_of(deadline_ok, lambda: deadline_ok.case_execution_deadline(scratch, "/bin/true")) is None,
        "the execution-deadline body should pass with its exact stop diagnostic",
    )
    deadline_bad = load()
    deadline_bad.run = deadline_run("note: the oxide-budget stop was reported elsewhere\n")
    fault = fault_of(deadline_bad, lambda: deadline_bad.case_execution_deadline(scratch, "/bin/true"))
    expect(fault is not None and "execution_stop" in fault, f"a missing execution diagnostic passed: {fault}")

    preparation_ok = load()
    preparation_ok.run = plain_run("oxide: stopped by the admission budget (1ms) before a usable child start\n", 124)
    expect(
        fault_of(preparation_ok, lambda: preparation_ok.case_preparation_stop(scratch, "/bin/true")) is None,
        "the preparation-stop body should pass with its exact admission diagnostic",
    )
    preparation_bad = load()
    preparation_bad.run = plain_run("   Compiling oxide-budget-preparation v0.1.0\n", 124)
    fault = fault_of(preparation_bad, lambda: preparation_bad.case_preparation_stop(scratch, "/bin/true"))
    expect(fault is not None and "admission_stop" in fault, f"an innocent line satisfied the preparation oracle: {fault}")

    plain_ok = load()
    plain_ok.run = plain_run("   Compiling oxide-budget-plain v0.1.0 (/private/tmp/oxide-budget-plain)\n", 3)
    expect(
        fault_of(plain_ok, lambda: plain_ok.case_unbudgeted(scratch, "/bin/true")) is None,
        "innocent output failed the unbudgeted oracle",
    )
    plain_bad = load()
    plain_bad.run = plain_run("oxide: stopped by the admission budget (1000ms) before a usable child start\n", 3)
    fault = fault_of(plain_bad, lambda: plain_bad.case_unbudgeted(scratch, "/bin/true"))
    expect(fault is not None and "budget diagnostic" in fault, f"a stop diagnostic passed the unbudgeted oracle: {fault}")

    # ---- the receipt carries bounded first-fault evidence, including on a
    # failure, and the excerpt is bounded rather than an unbounded stream copy.
    evidence = load()
    evidence.FAULT_DETAIL.clear()
    for index in range(5):
        evidence.record_detail(f"probe-{index}", "line of stderr output\n" * 60, [{"seq": 1, "event": "finished"}])
    expect(len(evidence.FAULT_DETAIL) == evidence.MAX_FAULT_DETAIL, "the receipt evidence is not bounded")
    expect(
        len(evidence.FAULT_DETAIL[-1]["stderr"]) <= evidence.MAX_EXCERPT_CHARS + 80,
        "a receipt excerpt exceeded its bound",
    )
    evidence_receipt = evidence.Proof(scratch, "s" * 64).document("boom")
    expect(
        bool(evidence_receipt["first_fault_evidence"]) and evidence_receipt["result"] == "fail",
        "a failure receipt lost its bounded first-fault evidence",
    )
    expect(
        evidence_receipt["first_fault_evidence"][-1]["typed"]["records"] == 1,
        "the receipt evidence lost its typed summary",
    )

    # ---- diagnostic keep-going: a recoverable failure continues only into
    # independent cases; an Abort stops with truthful skips; the receipt is
    # distinctly unqualified. No child runs here.
    diag = load()
    diag.Proof.prepare = lambda self: None
    diag.CASES.clear()
    diag.RESULTS.clear()
    diag.UNCERTAIN.clear()
    diag.PRIMARY_FAULT = None
    calls = []

    def diag_first(root, subject):
        calls.append("first")
        diag.case("first")
        diag.record_detail("first", "the first command's stderr\n", [{"seq": 1, "event": "finished"}])
        diag.fail("first oracle boom")

    def diag_second(root, subject):
        calls.append("second")
        diag.case("second")

    def diag_third(root, subject):
        calls.append("third")
        diag.case("third")
        diag.abort("infrastructure boom")

    def diag_fourth(root, subject):
        calls.append("fourth")
        diag.case("fourth")

    diag.CASE_FUNCTIONS = (diag_first, diag_second, diag_third, diag_fourth)
    diag.DIAGNOSTIC_DEPENDENCIES = {entry.__name__: [] for entry in diag.CASE_FUNCTIONS}
    summary = diag.diagnostic_summary()
    fault = diag.capture_fault(lambda: diag.Proof(scratch, "s" * 64).run_diagnostic(summary))
    expect(calls == ["first", "second", "third"], f"diagnostic call order changed: {calls}")
    expect(fault is not None and "infrastructure boom" in fault, f"an abort was lost: {fault}")
    expect(
        summary["failures"] and summary["failures"][0]["case"] == "diag_first",
        f"a recoverable failure was not recorded: {summary['failures']}",
    )
    expect(summary["completed"] == ["diag_second"], f"independent cases did not continue: {summary['completed']}")
    expect(summary["skipped"] == ["diag_fourth"], f"skips are not truthful: {summary['skipped']}")
    expect(
        summary["continued_after_failure"] == ["diag_first"],
        f"the diagnostic did not record its continuation: {summary['continued_after_failure']}",
    )
    diag_receipt = diag.Proof(scratch, "s" * 64).document(fault, diagnostic=summary)
    expect(
        diag_receipt["mode"] == "diagnostic" and diag_receipt["qualified"] is False
        and diag_receipt["result"] == "diagnostic",
        "the diagnostic receipt is not distinctly unqualified",
    )
    expect(
        diag_receipt["primary_fault"] == summary["failures"][0]["fault"],
        "the diagnostic receipt lost its first recorded fault",
    )
    expect(
        "infrastructure boom" in (diag_receipt["diagnostic"].get("aborted") or ""),
        "the diagnostic receipt lost the later abort",
    )
    failure_evidence = diag_receipt["diagnostic"]["failures"][0]["evidence"]
    expect(
        failure_evidence and failure_evidence[-1]["label"] == "first"
        and "stderr" in failure_evidence[-1],
        f"the diagnostic receipt lost its failure evidence: {failure_evidence}",
    )

    # An unknown-child custody outcome after a recoverable failure aborts too.
    uncertain = load()
    uncertain.Proof.prepare = lambda self: None
    uncertain.CASES.clear()
    uncertain.RESULTS.clear()
    uncertain.UNCERTAIN.clear()
    uncertain.PRIMARY_FAULT = None

    def uncertain_case(root, subject):
        uncertain.case("uncertain")
        uncertain.UNCERTAIN.append("owned pid 1 did not settle")
        uncertain.fail("oracle boom with unknown custody")

    uncertain.CASE_FUNCTIONS = (uncertain_case,)
    uncertain.DIAGNOSTIC_DEPENDENCIES = {"uncertain_case": []}
    uncertain_summary = uncertain.diagnostic_summary()
    uncertain_fault = uncertain.capture_fault(
        lambda: uncertain.Proof(scratch, "s" * 64).run_diagnostic(uncertain_summary)
    )
    expect(
        uncertain_fault is not None and "unknown child custody" in uncertain_fault,
        f"an unknown-child outcome did not abort the diagnostic: {uncertain_fault}",
    )
    expect(uncertain_summary["aborted"], "the unknown-child abort was not recorded")

    # ---- custody is shared safety state: ANY current uncertainty stops
    # dispatch before and after every outcome, in both modes, including when a
    # case returns green. A mutation that removes the check must be visible.
    def custody_cases(module, order):
        def green_then_unknown(root, subject):
            order.append("green")
            module.case("green-unknown")
            module.UNCERTAIN.append("owned pid 4242 did not settle")

        def never_again(root, subject):
            order.append("next")
            module.case("next-independent")

        return (green_then_unknown, never_again)

    guarded = load()
    guarded.Proof.prepare = lambda self: None
    order = []
    guarded.CASE_FUNCTIONS = custody_cases(guarded, order)
    guarded.DIAGNOSTIC_DEPENDENCIES = {entry.__name__: [] for entry in guarded.CASE_FUNCTIONS}
    green_summary = guarded.diagnostic_summary()
    green_fault = guarded.capture_fault(lambda: guarded.Proof(scratch, "s" * 64).run_diagnostic(green_summary))
    expect(order == ["green"], f"a case started after green-case custody uncertainty: {order}")
    expect(
        green_fault is not None and "unknown child custody" in green_fault,
        f"a green case's custody uncertainty did not abort: {green_fault}",
    )
    expect(green_summary["completed"] == [], "an uncertain green case was reported completed")
    expect(green_summary["skipped"] == ["never_again"], f"skips are not truthful: {green_summary['skipped']}")

    # The mutation removing the post-success check lets the next case run, so
    # the test above is meaningful rather than vacuous.
    unguarded = load()
    unguarded.Proof.prepare = lambda self: None
    unguarded.custody_guard = lambda stage: None
    mutated_order = []
    unguarded.CASE_FUNCTIONS = custody_cases(unguarded, mutated_order)
    unguarded.DIAGNOSTIC_DEPENDENCIES = {entry.__name__: [] for entry in unguarded.CASE_FUNCTIONS}
    mutated_summary = unguarded.diagnostic_summary()
    unguarded.capture_fault(lambda: unguarded.Proof(scratch, "s" * 64).run_diagnostic(mutated_summary))
    expect(
        mutated_order == ["green", "next"],
        f"the post-success custody mutation was not observable: {mutated_order}",
    )

    # Pre-existing uncertainty stops dispatch before the first case runs.
    preexisting = load()
    preexisting.Proof.prepare = lambda self: None
    preexisting.UNCERTAIN.append("owned pid 4343 was already unknown")
    preexisting_order = []
    preexisting.CASE_FUNCTIONS = (
        lambda root, subject: preexisting_order.append("first"),
    )
    preexisting.DIAGNOSTIC_DEPENDENCIES = {"<lambda>": []}
    preexisting_summary = preexisting.diagnostic_summary()
    preexisting_fault = preexisting.capture_fault(
        lambda: preexisting.Proof(scratch, "s" * 64).run_diagnostic(preexisting_summary)
    )
    expect(preexisting_order == [], f"a case ran with pre-existing custody uncertainty: {preexisting_order}")
    expect(
        preexisting_fault is not None and "unknown child custody" in preexisting_fault,
        f"pre-existing uncertainty did not abort: {preexisting_fault}",
    )

    # Counted mode uses the same boundary: green + uncertainty aborts and no
    # later case starts; a clean dispatch still completes.
    counted = load()
    counted.Proof.prepare = lambda self: None
    counted_order = []
    counted.CASE_FUNCTIONS = custody_cases(counted, counted_order)
    counted_fault = counted.capture_fault(lambda: counted.Proof(scratch, "s" * 64).run())
    expect(counted_order == ["green"], f"counted mode started a case after uncertainty: {counted_order}")
    expect(
        counted_fault is not None and "unknown child custody" in counted_fault,
        f"counted mode continued on unknown custody: {counted_fault}",
    )
    clean_counted = load()
    clean_counted.Proof.prepare = lambda self: None
    clean_counted.CASE_FUNCTIONS = (
        lambda root, subject: clean_counted.case("one"),
        lambda root, subject: clean_counted.case("two"),
    )
    expect(
        clean_counted.capture_fault(lambda: clean_counted.Proof(scratch, "s" * 64).run()) is None,
        "a clean counted dispatch changed",
    )

    # An earlier recorded oracle stays the primary fault and keeps its own
    # bounded evidence after a later green case aborts on unknown custody.
    sequence = load()
    sequence.Proof.prepare = lambda self: None

    def oracle_case(root, subject):
        sequence.case("early-oracle")
        sequence.record_detail("early-oracle", "earlier stderr line\n", [{"seq": 1, "event": "finished"}])
        sequence.fail("earlier oracle boom")

    def later_unknown(root, subject):
        sequence.CASE_FUNCTIONS  # keep the closure explicit
        sequence.case("later-green")
        sequence.UNCERTAIN.append("owned pid 4545 did not settle")

    sequence.CASE_FUNCTIONS = (oracle_case, later_unknown)
    sequence.DIAGNOSTIC_DEPENDENCIES = {entry.__name__: [] for entry in sequence.CASE_FUNCTIONS}
    sequence_summary = sequence.diagnostic_summary()
    sequence_fault = sequence.capture_fault(
        lambda: sequence.Proof(scratch, "s" * 64).run_diagnostic(sequence_summary)
    )
    expect(
        sequence_fault is not None and "unknown child custody" in sequence_fault,
        f"the later custody abort was not visible: {sequence_fault}",
    )
    sequence_receipt = sequence.Proof(scratch, "s" * 64).document(sequence_fault, diagnostic=sequence_summary)
    expect(
        sequence_receipt["primary_fault"] == "earlier oracle boom",
        f"the earlier oracle stopped being primary: {sequence_receipt['primary_fault']}",
    )
    expect(
        sequence_receipt["first_fault_evidence"]
        and sequence_receipt["first_fault_evidence"][-1]["label"] == "early-oracle",
        "the earlier oracle's bounded evidence was overwritten by the later abort",
    )
    expect(
        "unknown child custody" in (sequence_receipt["diagnostic"].get("aborted") or ""),
        "the receipt hid the later custody abort",
    )

    # ---- holder: an empty subject must fail, a real one must acquire ----
    holder = load()
    holder.start = holder_start(holder)
    holder.close_holder = lambda process, release_file: None
    try:
        holder.hold_slot(scratch, {}, scratch, "empty", 1, "")
        empty_fault = None
    except Exception as error:  # noqa: BLE001 - any refusal is the expected outcome
        empty_fault = str(error)
    expect(empty_fault is not None, "an empty holder subject was accepted")
    acquired, _release = holder.hold_slot(scratch, {}, holder.create_crate(scratch, "held"), "held", 1, "/bin/true")
    expect(acquired is not None, "a real holder did not acquire")

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

    # A clean run without a receipt is a failure, and a receipt-write failure
    # never replaces the primary fault.
    expect(load().final_status(None, "receipt.json") == 0, "a clean run with a receipt must pass")
    expect(load().final_status(None, None) == 1, "a clean run without a receipt must fail")
    expect(load().final_status("boom", "receipt.json") == 1, "a fault must fail even with a receipt")
    failing_receipt = load()
    failing_receipt.PRIMARY_FAULT = "primary boom"
    failing_receipt.write_receipt = lambda document, scratch_arg: (_ for _ in ()).throw(OSError("disk full"))
    expect(failing_receipt.write_receipt_safely({"result": "fail"}, scratch) is None,
           "a receipt-write failure must be reported as not written")
    expect(failing_receipt.PRIMARY_FAULT == "primary boom", "a receipt-write failure replaced the primary fault")
    print(
        "self-test: ok (faked bodies for fast-start/late/cumulative/nested/deadline/preparation/"
        "unbudgeted/holders, human-diagnostic grammar counterfactuals, bounded fault evidence, "
        "diagnostic keep-going/abort/receipt counterfactuals, shared custody boundaries "
        "(pre-existing, green, post-failure, counted/default) and lifecycle fault paths)"
    )


def final_status(fault: str | None, receipt_path: str | None) -> int:
    """0 only for a clean run that also wrote its helper receipt."""
    if fault is not None:
        return 1
    if receipt_path is None:
        return 1
    return 0


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


def start_proof() -> tuple[Proof, str]:
    """Shared prerequisites for the counted run and the diagnostic."""
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
    return (Proof(scratch, runner_sha), scratch)


def diagnostic_summary() -> dict:
    """The starting summary of one uncounted keep-going collection."""
    return {
        "qualified": False,
        "mode": "diagnostic",
        "failures": [],
        "completed": [],
        "skipped": [],
        "aborted": None,
        "continued_after_failure": [],
        "dependencies": {name: list(values) for name, values in DIAGNOSTIC_DEPENDENCIES.items()},
        "authority": "diagnostic only: a recoverable oracle failure is recorded, not a counted PASS; the guard remains closure authority",
    }


def diagnostic_mode() -> int:
    """Uncounted keep-going collection under the same guard and custody.

    Recoverable oracle failures are recorded and independent cases continue; an
    abort is written into the receipt. This mode never reports a counted PASS,
    and ROOT activates it separately from the fail-fast proof.
    """
    say("diagnostic: keep-going collection; not a counted PASS, receipts are qualified=false")
    proof, scratch = start_proof()
    summary = diagnostic_summary()
    fault = capture_fault(lambda: proof.run_diagnostic(summary))
    document = proof.document(fault, diagnostic=summary)
    path = write_receipt_safely(document, scratch)
    if fault is None and path is None:
        fault = "the diagnostic receipt was not written"
        document = proof.document(fault, diagnostic=summary)
        path = write_receipt_safely(document, scratch)
    say(f"diagnostic receipt {path if path else 'NOT WRITTEN'} (result diagnostic, qualified=false)")
    if summary["aborted"]:
        say(f"diagnostic aborted: {summary['aborted']}")
    for entry in summary["failures"]:
        say(f"diagnostic failure: {entry['case']}: {entry['fault']}")
    clean = fault is None and not summary["failures"] and bool(path)
    return 0 if clean else 1


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
    if mode == "diagnostic":
        return diagnostic_mode()
    proof, scratch = start_proof()
    fault = capture_fault(proof.run)
    for process in OWNED:
        release(process)
    if fault is None:
        # A missing helper receipt is a failed proof, not a quiet success.
        document = proof.document(None)
        path = write_receipt_safely(document, scratch)
        if path is None:
            fault = "the helper receipt was not written"
            document = proof.document(fault)
    else:
        document = proof.document(fault)
        path = write_receipt_safely(document, scratch)
    say(f"helper receipt {path if path else 'NOT WRITTEN'} (result {document['result']})")
    if fault is None and path is not None:
        print(json.dumps(document, indent=2, sort_keys=True), flush=True)
    elif fault is not None:
        say(f"primary fault: {fault}")
    return final_status(fault, path)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
