#!/usr/bin/env python3
"""Focused installed-binary hand-off proof for oxide#122.

One fixed script and whole command under the qualified shared supervisor
(`bounded-test.py`): the brief's 600 s wall limit and 128 MiB monitored scratch
budget including cleanup. It stages this clean HEAD's debug `oxide` itself with
plain Cargo and the pinned 1.97.1, offline, into its own scratch target, copies
the binary and deletes the target before the fixtures. Only private scratch
pools are created; no installed binary, host state or network is involved. The stage is kept
inside the scratch budget by single-threaded, single-codegen-unit, debug-free,
symbol-stripped, size-optimised codegen. An earlier internal-root run measured
about 58 MiB for staging and 37 s; it is not evidence of Builds performance.
A fallback to `/private/tmp` exercises the internal path, where the scratch
declaration is inert; it does not provide scratch-pool coverage.

Every fixture has a private HOME with receipts and a candidate stub, a private
`OXIDE_POOL` scratch pool with planted control files, a private CARGO_HOME,
and a PATH whose only `oxide` is the fixture candidate. Cases cover: newer
state and registration versions; malformed, older and non-numeric documents
that must not trigger; a valid candidate that is executed exactly once with
byte-exact argv and the caller's environment plus the internal marker; the
marker refusing and being inert without newer state; every candidate,
ownership, path and replacement refusal; the running binary itself; a
replacement between hashing and the last check; skipped commands; and a
command that reaches final release after effects with newer state, which must
never execute anything.

The trace layout binds two independent supervisor identities to their own
real sources and never equates them: the exported `OXIDE_PROOF_RUN_ID`
(session token) must equal the guard receipt's `run_id`, and the scratch
marker's `run_id` must equal that receipt's `scratch_marker.run_id` (the
monitor generates the two tokens independently). The diagnostic attributes
raw trace evidence per instrumented command and fails when the instrument
did not fire; missing ends stay unknown and trace totals and times are
observations, never acceptance gates.

HIGH premise dispositions (see the case bodies and helpers below):
1. _stage_subject captures invoking rustup and Cargo cache before a private HOME;
   fixture children use private HOME/CARGO_HOME, the pinned toolchain, no auto
   install, and fake Cargo (no fixture --locked invocation).
2. Fixture.env sets OXIDE_TEST_HOME_FROM_ENV=1 for every product child.
3. Fixture.env is an allowlist, scrubbing inherited OXIDE_, XDG_, CARGO_ and
   GIT_; staging scrubs those families before its explicit values.
4. No permanent-mode adoption is tested; fixture checkouts use scratch pools.
5. No Cargo external subcommand is invoked; the recording stub is an installed
   Oxide candidate, not a cargo-* external subcommand.
6. No disk images are attached or detached.
7. Staging and fixture children freeze RUSTFLAGS and omit encoded flags.
8. main derives the private declaration from the supervisor scratch root;
   Fixture.env supplies that root and an explicit 64 MiB cap to every child.
9. main uses tempfile.mkdtemp with an explicit supervisor scratch parent; no
   shell mktemp is used.

The uncounted diagnostic atomically publishes a bounded progress receipt at
phase and case boundaries and around child commands; a killed child has an
unknown exit. It records monotonic times, command exits, bounded captured output
and the retained merged Cargo staging log, including package-cache lock waits.
The invoking Cargo cache remains shared: candidate cache and checkout sizes
plus the historical staging peak do not establish a safe 128 MiB private seed.
A work-deadline stop is explicit. The receipt remains qualified: false; timing
observations are not acceptance gates.

Not a benchmark, not full QA and not real-drive or host-install qualification.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import hashlib
import json
import os
import re
import shlex
import shutil
import stat
import subprocess
import tempfile
import time

MIB = 1024 * 1024
RECEIPT_MAX_BYTES = 64 * 1024
TRACE_READ_BYTES = 64 * 1024
NOTICE = "oxide: handing off to the installed binary (state is newer than this one).\n"
REFUSAL = "oxide: oxide was upgraded during this command; re-run.\n"
# Control-state versions and the fences in the documents this proof plants.
RECORDED_ACCOUNTING_STATE_VERSION = 5
EVICTION_ACCOUNTING_STATE_VERSION = 7
SLOT_CEILING_STATE_VERSION = 8
HOLDING_STATE_VERSION = 9
STATE_VERSION = HOLDING_STATE_VERSION
NEWER_STATE_VERSION = STATE_VERSION + 1
WITNESS_STATE_VERSION = STATE_VERSION
LEDGER_VERSION = 1
REGISTRATION_VERSION = 2
CAPABILITIES_PROBE = "__oxide-build-capabilities"
SHUTDOWN_MARGIN_S = 20
CASE_PLAN = ('newer_state_hands_off_once', 'marker_present_refuses_even_with_newer_state', 'marker_is_inert_without_newer_state', 'non_numeric_older_and_malformed_documents_do_not_trigger', 'malformed_and_oversized_documents_keep_the_existing_refusal', 'no_pool_means_no_trigger', 'newer_registration_without_a_candidate_refuses', 'newer_state_without_receipts_or_candidate_refuses', 'a_receipt_that_does_not_cover_a_newer_registration_refuses', 'the_newest_receipt_is_the_only_candidate', 'a_replacement_before_the_last_check_refuses', 'plain_passthrough_skips_the_hand_off_check', 'usage_only_exec_skips_the_hand_off_check', 'launch_check_probe_skips_the_hand_off_check', 'the_running_binary_refuses_to_hand_off_to_itself', 'cargo_exit_75_is_propagated_and_is_not_a_trigger', 'a_final_release_error_never_replays_the_command')
CASES = []
DIAGNOSTIC = None
DIAGNOSTIC_PATH = None
DIAGNOSTIC_WRITER = None

# The platform toolchain stays reachable for `rustc -vV` and `cargo -V`
# probes; the fixture's own `cargo` and `oxide` stay first on PATH.
SUBJECT = ""
SCRATCH_ROOT = ""
TOOLCHAIN_BIN = os.path.dirname(shutil.which("rustc") or "")


class ProofFailure(SystemExit):
    def __init__(self, message: str):
        super().__init__(1)
        self.message = message


def fail(message: str) -> None:
    print(f"installed-handoff proof failed: {message}", file=sys.stderr)
    raise ProofFailure(message)


def expect(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def publish_diagnostic(outcome, writer=None):
    """Atomically replace this run's bounded, unqualified progress receipt."""
    if outcome is None:
        return
    encoded = (json.dumps(outcome, sort_keys=True, ensure_ascii=True) + "\n").encode("utf-8")
    if len(encoded) > 2 * MIB:
        raise RuntimeError("diagnostic receipt exceeds 2 MiB")
    writer = writer if writer is not None else DIAGNOSTIC_WRITER
    if writer is not None:
        writer(encoded)
        return
    if DIAGNOSTIC_PATH is None:
        return
    directory = os.path.dirname(DIAGNOSTIC_PATH)
    fd, temp = tempfile.mkstemp(prefix=".handoff-receipt.", dir=directory)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(encoded)
        os.replace(temp, DIAGNOSTIC_PATH)
    finally:
        if os.path.exists(temp):
            os.unlink(temp)


def phase(name):
    if DIAGNOSTIC is not None:
        now = time.monotonic_ns()
        if DIAGNOSTIC["phase"] is not None:
            DIAGNOSTIC["phases"][-1]["end_ns"] = now
        DIAGNOSTIC["phase"] = name
        DIAGNOSTIC["phases"].append({"name": name, "start_ns": now, "end_ns": None})
        publish_diagnostic(DIAGNOSTIC)
        print(f"phase {name}", flush=True)


def case(name: str) -> None:
    CASES.append(name)
    if DIAGNOSTIC is not None:
        if DIAGNOSTIC["active"] is not None:
            end_case(DIAGNOSTIC)
            DIAGNOSTIC["completed"].append(DIAGNOSTIC["active"])
            DIAGNOSTIC["states"][DIAGNOSTIC["active"]] = "completed"
        DIAGNOSTIC["active"] = name
        DIAGNOSTIC["case_times"][name] = {"start_ns": time.monotonic_ns(), "end_ns": None}
        DIAGNOSTIC["last_command"] = None
        DIAGNOSTIC["reached"].append(name)
        expect(name in CASE_PLAN and DIAGNOSTIC["states"][name] == "unexecuted", "case plan drift")
        DIAGNOSTIC["states"][name] = "active"
        publish_diagnostic(DIAGNOSTIC)
    print(f"case {name}", flush=True)


def work_left() -> float:
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    left = (deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 3:
        fail("work deadline reached before the supervisor's shutdown margin")
    return left


def end_case(outcome):
    name = outcome["active"]
    if name is not None and name in outcome["case_times"]:
        outcome["case_times"][name]["end_ns"] = time.monotonic_ns()


def bounded_output(value):
    if value is None:
        return None
    data = value if isinstance(value, bytes) else str(value).encode("utf-8", "replace")
    return data[:2048].decode("utf-8", "replace")


def read_receipt(evidence: str, owner_uid: int):
    """One bounded no-follow snapshot of the guard's published receipt at
    `<evidence>/receipt.json`. The monitor replaces that path atomically
    (`receipt.json.tmp` + rename) at each phase, so one open descriptor always
    reads exactly one whole publication; a snapshot whose identity changed
    between lstat and open, a symlink, a foreign owner, an oversized file, or
    a truncated or malformed document refuses. There is no retry: any refused
    receipt disables the trace."""
    path = os.path.join(evidence, "receipt.json")
    info = os.lstat(path)
    if (not stat.S_ISREG(info.st_mode) or info.st_uid != owner_uid
            or info.st_size > RECEIPT_MAX_BYTES):
        return None
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, "rb") as handle:
        opened = os.fstat(handle.fileno())
        if (opened.st_dev, opened.st_ino) != (info.st_dev, info.st_ino):
            return None
        raw = handle.read(RECEIPT_MAX_BYTES + 1)
    if len(raw) > RECEIPT_MAX_BYTES:
        return None
    return json.loads(raw)


def trace_layout(env, evidence, run_id):
    """Refuse anything except the owned supervisor and this private fixture.
    The two supervisor identities bind to their own sources and are never
    equated: the exported session token equals the guard receipt's `run_id`,
    the marker token equals the receipt's `scratch_marker.run_id`, the
    receipt names this verified root, scratch and evidence, and the marker's
    canonical root, UID, device and inode match the opened descriptor."""
    try:
        scratch = env["OXIDE_TASK_SCRATCH_ROOT"]
        fixture_root = env["OXIDE_SCRATCH_ROOT"]
        home = env["HOME"]
        root = os.path.dirname(scratch)
        marker_path = os.path.join(root, ".oxide-scratch.json")
        paths = (scratch, evidence, fixture_root, home, root)
        if any(not os.path.isabs(p) or os.path.realpath(p) != p for p in paths):
            return False
        if (os.path.basename(scratch) != "scratch" or evidence != os.path.join(root, "evidence")
                or os.path.commonpath((scratch, fixture_root)) != scratch
                or os.path.commonpath((fixture_root, home)) != fixture_root
                or env.get("OXIDE_TEST_HOME_FROM_ENV") != "1"
                or env.get("OXIDE_POOL") != os.path.join(os.path.dirname(home), "pool")):
            return False
        info = os.lstat(root)
        marker_info = os.lstat(marker_path)
        if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid() or info.st_mode & 0o077
                or not stat.S_ISREG(marker_info.st_mode) or marker_info.st_uid != info.st_uid
                or marker_info.st_mode & 0o077 or marker_info.st_size > 4096):
            return False
        fd = os.open(marker_path, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(fd, "rb") as handle:
            opened = os.fstat(handle.fileno())
            if (opened.st_dev, opened.st_ino) != (marker_info.st_dev, marker_info.st_ino):
                return False
            raw = handle.read(4097)
        if len(raw) > 4096:
            return False
        marker = json.loads(raw)
        session = run_id
        marker_run = marker.get("run_id")
        if (not isinstance(session, str) or re.fullmatch(r"[0-9a-f]{32}", session) is None
                or not isinstance(marker_run, str)
                or re.fullmatch(r"[0-9a-f]{32}", marker_run) is None
                or marker.get("root") != root
                or marker.get("owner_uid") != info.st_uid
                or marker.get("device") != info.st_dev
                or marker.get("inode") != info.st_ino
                or marker.get("version") != 1
                or marker.get("outcome") != "running" or marker.get("closure") != "none"
                or marker.get("concluded_unix", False) is not None):
            return False
        receipt = read_receipt(evidence, info.st_uid)
        scratch_marker = receipt.get("scratch_marker") if isinstance(receipt, dict) else None
        return (isinstance(scratch_marker, dict)
                and receipt.get("run_id") == session
                and receipt.get("root") == root
                and receipt.get("scratch") == scratch
                and receipt.get("evidence") == evidence
                and isinstance(receipt.get("phase"), str) and bool(receipt["phase"])
                and scratch_marker.get("run_id") == marker_run
                and scratch_marker.get("path") == marker_path)
    except (OSError, ValueError, KeyError, TypeError, AttributeError):
        return False


def parse_trace_events(path: str) -> dict:
    """Bounded read of one product trace file: `<name> start|end <nanos>`
    lines from the debug-only phase instrument. A refused, unreadable,
    oversized, empty or malformed file never counts as evidence."""
    record = {"state": "missing", "bytes": None, "events": 0, "first": None,
              "last": None, "phases": {}, "unpaired": []}
    try:
        with open(path, "rb") as handle:
            raw = handle.read(TRACE_READ_BYTES + 1)
    except OSError:
        record["state"] = "unreadable"
        return record
    record["bytes"] = len(raw)
    if len(raw) > TRACE_READ_BYTES:
        record["state"] = "oversized"
        return record
    if not raw:
        record["state"] = "empty"
        return record
    phases: dict = {}
    for line in raw.decode("ascii", "replace").splitlines():
        parts = line.split(" ")
        if len(parts) != 3 or parts[1] not in ("start", "end") or not parts[2].isdigit():
            record["state"] = "malformed"
            return record
        counts = phases.setdefault(parts[0], {"starts": 0, "ends": 0})
        counts["starts" if parts[1] == "start" else "ends"] += 1
        record["events"] += 1
        record["last"] = parts[0]
        if record["first"] is None:
            record["first"] = parts[0]
    record["state"] = "evidence"
    record["phases"] = phases
    record["unpaired"] = sorted(name for name, counts in phases.items()
                                if counts["starts"] != counts["ends"])
    return record


def attribute_traces(commands, *, evidence=None, session=None) -> dict:
    """Bind every traced command to its own raw trace evidence and to the
    guard receipt's two identities. A refused layout or absent raw evidence
    is an insufficient-instrumentation outcome, never a successful
    attribution beside passing cases. An unpaired start stays an unknown end;
    totals and times are observations, never acceptance gates."""
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "") if evidence is None else evidence
    session = os.environ.get("OXIDE_PROOF_RUN_ID", "") if session is None else session
    receipt = None
    if evidence and os.path.isdir(evidence):
        try:
            receipt = read_receipt(evidence, os.geteuid())
        except (OSError, ValueError):
            receipt = None
    receipt_run = receipt.get("run_id") if isinstance(receipt, dict) else None
    marker_run = None
    if isinstance(receipt, dict) and isinstance(receipt.get("scratch_marker"), dict):
        marker_run = receipt["scratch_marker"].get("run_id")
    rows, problems = [], []
    traced = [command for command in commands if isinstance(command.get("trace"), dict)]
    if not traced:
        problems.append("no instrumented commands were recorded")
    if receipt_run != session:
        problems.append(f"guard receipt run_id {receipt_run!r} does not bind the "
                        f"exported OXIDE_PROOF_RUN_ID {session!r}")
    for command in traced:
        trace = command["trace"]
        row = {"case": command.get("case"), "allocation": trace.get("state"),
               "file": trace.get("file"), "run": trace.get("run"), "bytes": None,
               "events": 0, "first": None, "last": None, "phases": {}, "unpaired": [],
               "evidence": None}
        if trace.get("state") == "allocated" and trace.get("file"):
            parsed = parse_trace_events(os.path.join(evidence, trace["file"]))
            row["bytes"] = parsed["bytes"]
            row["events"] = parsed["events"]
            row["first"], row["last"] = parsed["first"], parsed["last"]
            row["phases"], row["unpaired"] = parsed["phases"], parsed["unpaired"]
            row["evidence"] = parsed["state"]
            if trace.get("run") != session:
                problems.append(f"{command.get('case')}: trace allocation names run "
                                f"{trace.get('run')!r}, not the exported session token")
            if parsed["state"] != "evidence":
                problems.append(f"{command.get('case')}: no raw trace evidence ({parsed['state']})")
        else:
            problems.append(f"{command.get('case')}: trace not allocated ({trace.get('state')})")
        rows.append(row)
    return {"source": {"session": session, "receipt_run": receipt_run,
                       "marker_run": marker_run, "evidence": evidence},
            "expected_traced": len(rows),
            "allocated": sum(1 for row in rows if row["allocation"] == "allocated"),
            "with_evidence": sum(1 for row in rows if row["evidence"] == "evidence"),
            "unknown_ends": sum(1 for row in rows if row["unpaired"]),
            "commands": rows, "problems": problems[:8],
            "problems_omitted": max(0, len(problems) - 8),
            "outside": ("each command's window is its own first start to last end; earlier "
                        "and later steps stay outside it and unpaired starts keep unknown ends"),
            "sufficient": not problems}


def observed_run(argv, *, limit, _runner=None, _remaining=None, _merged_log=None, _fixture=None, _trace=False, **kwargs):
    """Observe the existing child call; the shared supervisor owns custody."""
    runner = subprocess.run if _runner is None else _runner
    remaining = work_left if _remaining is None else _remaining
    row = None
    if DIAGNOSTIC is not None:
        row = {"case": DIAGNOSTIC["active"], "command": [str(x)[:512] for x in argv[:16]],
               "start_ns": time.monotonic_ns(), "end_ns": None, "exit": None,
               "stdout": None, "stderr": None, "stop": None, "state": "preparing"}
        if _fixture is not None:
            row["fixture"] = _fixture[:128]
        if _merged_log is not None:
            row["merged_log"] = _merged_log
            row["package_cache_wait"] = {"state": "not yet observed", "line": None}
        DIAGNOSTIC["last_command"] = row
        if len(DIAGNOSTIC["commands"]) < 256:
            DIAGNOSTIC["commands"].append(row)
        else:
            DIAGNOSTIC["commands_omitted"] += 1
        publish_diagnostic(DIAGNOSTIC)
    try:
        if (row is not None and row in DIAGNOSTIC["commands"]
                and _fixture is not None and _trace and _runner is None):
            env = kwargs.get("env", {})
            evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
            run_id = os.environ.get("OXIDE_PROOF_RUN_ID", "")
            row["trace"] = {"state": "layout refused"}
            if trace_layout(env, evidence, run_id):
                try:
                    fd, path = tempfile.mkstemp(prefix="handoff-trace.", dir=evidence)
                    os.close(fd)
                    env = dict(env, OXIDE_HANDOFF_TRACE="1", OXIDE_HANDOFF_TRACE_FILE=path,
                               OXIDE_PROOF_EVIDENCE=evidence, OXIDE_PROOF_RUN_ID=run_id)
                    kwargs["env"] = env
                    row["trace"] = {"state": "allocated", "file": os.path.basename(path), "run": run_id}
                except OSError:
                    row["trace"] = {"state": "allocation failed"}
        if row is not None:
            row["state"] = "launching"
            publish_diagnostic(DIAGNOSTIC)
        left = remaining()
        result = runner(argv, timeout=min(limit, left) if limit is not None else left, **kwargs)
        if row is not None:
            row.update(exit=result.returncode, stdout=bounded_output(result.stdout),
                       stderr=bounded_output(result.stderr),
                       stderr_tail=bounded_output(result.stderr[-2048:] if result.stderr is not None else None),
                       state="finished")
        return result
    except BaseException as error:
        if row is not None:
            row["stop"] = getattr(error, "message", type(error).__name__)[:256]
            row["state"] = "not launched" if isinstance(error, ProofFailure) else "outcome unknown"
            row["stdout"] = bounded_output(getattr(error, "stdout", None))
            row["stderr"] = bounded_output(getattr(error, "stderr", None))
            stderr = getattr(error, "stderr", None)
            row["stderr_tail"] = bounded_output(stderr[-2048:] if stderr is not None else None)
        raise
    finally:
        if row is not None:
            row["end_ns"] = time.monotonic_ns()
            if _merged_log is not None:
                observe_staging_log(row, _merged_log)
            publish_diagnostic(DIAGNOSTIC)


def cache_wait_observation(prefix):
    needle = b"Blocking waiting for file lock on package cache"
    match = next((line for line in prefix.splitlines() if needle in line), None)
    return {"state": "observed" if match else "not observed in bounded prefix",
            "line": bounded_output(match), "prefix_bytes": len(prefix)}


def observe_staging_log(row, log):
    """Read a bounded prefix for Cargo's lock message and a bounded tail."""
    try:
        with open(log, "rb") as handle:
            prefix = handle.read(65536)
            handle.seek(0, os.SEEK_END)
            size = handle.tell()
            handle.seek(max(0, size - 2048))
            row["merged_log_tail"] = bounded_output(handle.read(2048))
        row["package_cache_wait"] = cache_wait_observation(prefix)
        row["merged_log_bytes"] = size
    except OSError as error:
        row["package_cache_wait"] = {"state": "unreadable", "line": None, "error": type(error).__name__}


def sha256(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def write(path: str, contents: str, mode: int) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(contents)
    os.chmod(path, mode)


RECORDED_ACCOUNTING_FENCE = "oxide.accounting/1"
EVICTION_ACCOUNTING_FENCE = "oxide.eviction-accounting/1"
SLOT_CEILING_FENCE = "oxide.slots/256"
HOLDING_FENCE = "oxide.holding/1"


def check_source_versions() -> None:
    source_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    def literal(path: str, name: str) -> int:
        with open(os.path.join(source_dir, path), encoding="utf-8") as handle:
            matches = re.findall(r"^pub const " + name + r": u32 = (\d+);$", handle.read(), re.MULTILINE)
        expect(len(matches) == 1, f"cannot read product constant {name}")
        return int(matches[0])
    expect(STATE_VERSION == literal("src/lease/mod.rs", "CONTROL_FENCE_VERSION"), "control-state fixture version drift")
    expect(SLOT_CEILING_STATE_VERSION == literal("src/lease/control.rs", "SLOT_CEILING_STATE_VERSION"), "slot ceiling fixture version drift")
    expect(LEDGER_VERSION == literal("src/lease/mod.rs", "LEASE_VERSION"), "ledger fixture version drift")


def current_state(version: int = STATE_VERSION) -> str:
    document = {
        "version": version,
        "next_id": 1,
        "next_use": 1,
        "tickets": [],
        "cleans": [],
        "records": [],
    }
    # Version 5 is the recorded-accounting fence (oxide#146) and must carry it;
    # the versions below it never do. Version 7 carries the eviction-accounting
    # fence; version 8 adds the raised-ceiling fence, and version 9 adds holding.
    if version >= RECORDED_ACCOUNTING_STATE_VERSION:
        document["recorded_accounting"] = RECORDED_ACCOUNTING_FENCE
    if version >= EVICTION_ACCOUNTING_STATE_VERSION:
        document["eviction_accounting"] = EVICTION_ACCOUNTING_FENCE
    if version >= SLOT_CEILING_STATE_VERSION:
        document["slot_ceiling"] = SLOT_CEILING_FENCE
    if version >= HOLDING_STATE_VERSION:
        document["holding"] = HOLDING_FENCE
    return json.dumps(document)


STUB_SOURCE = r"""
use std::io::Write;

fn main() {
    let record = std::env::var_os("OXIDE_HANDOFF_RECORD").expect("record path");
    let stem = record.to_string_lossy();
    let counts = format!("{stem}.count");
    let count = std::fs::read_to_string(&counts)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(0)
        + 1;
    std::fs::write(&counts, count.to_string()).unwrap();
    let mut argv = std::fs::File::create(format!("{stem}.argv")).unwrap();
    for argument in std::env::args_os() {
        argv.write_all(argument.as_encoded_bytes()).unwrap();
        argv.write_all(b"\0").unwrap();
    }
    let mut env = std::fs::File::create(format!("{stem}.env")).unwrap();
    for (name, value) in std::env::vars_os() {
        env.write_all(name.as_encoded_bytes()).unwrap();
        env.write_all(b"=").unwrap();
        env.write_all(value.as_encoded_bytes()).unwrap();
        env.write_all(b"\0").unwrap();
    }
    let code = std::env::var("OXIDE_HANDOFF_EXIT")
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(42);
    std::process::exit(code);
}
"""

STUB = ""


def compile_stub(root: str) -> str:
    """One tiny std-only recording binary, shared by hard link across cases.

    It records its own argv (including argv[0]) and environment, so the proof
    can show that a hand-off preserves both, and exits with
    OXIDE_HANDOFF_EXIT.
    """
    global STUB
    source = os.path.join(root, "handoff-stub.rs")
    write(source, STUB_SOURCE, 0o644)
    binary = os.path.join(root, "handoff-stub")
    env = {"PATH": os.environ.get("PATH", ""),
           "HOME": os.path.join(root, "stage-home"),
           "RUSTUP_HOME": os.environ.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup"),
           "RUSTUP_TOOLCHAIN": BUILDER, "RUSTUP_AUTO_INSTALL": "0",
           "RUSTFLAGS": "-C strip=symbols"}
    result = observed_run(
        ["rustc", "--edition", "2021", "-O", "-o", binary, source],
        env=env, capture_output=True,
        text=True,
        limit=60,
    )
    expect(result.returncode == 0, f"stub compile failed: {result.stderr[-2000:]}")
    os.unlink(source)
    STUB = binary
    return binary


def install_stub(destination: str) -> None:
    try:
        os.link(STUB, destination)
    except OSError:
        shutil.copy2(STUB, destination)
    os.chmod(destination, 0o755)


REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILDER = "1.97.1"


def stage_subject(root: str, capability: dict | None = None) -> str:
    """Shared staging entry point; CARGO_ENCODED_RUSTFLAGS is scrubbed below."""
    return _stage_subject(root, capability, isolated=False)


def _stage_subject(root: str, capability: dict | None = None, *, isolated: bool) -> str:
    """Stage this HEAD's `oxide` with plain Cargo, offline, in owned scratch.

    No Oxide pool is created or touched, and the staged target is deleted once
    the binary is copied out, so the proof's own writes stay tiny.
    """
    stage = os.path.join(root, "stage")
    log = (os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], "installed-handoff-stage.log")
           if DIAGNOSTIC is not None else os.path.join(root, "stage.log"))
    invoking_home = os.path.expanduser("~")
    rustup_home = os.environ.get("RUSTUP_HOME") or os.path.join(invoking_home, ".rustup")
    cargo_home = os.environ.get("CARGO_HOME") or os.path.join(invoking_home, ".cargo")
    private_home = os.path.join(root, "stage-home")
    if isolated:
        os.makedirs(private_home, mode=0o700, exist_ok=True)
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("CARGO_TARGET_", "CARGO_BUILD_", "SCCACHE_"))
        and not (isolated and key.startswith(("OXIDE_", "XDG_", "CARGO_", "GIT_")))
        and key not in ("CARGO", "CARGO_ENCODED_RUSTFLAGS", "RUSTC",
                        "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER")
    }
    if isolated:
        env.update({"HOME": private_home, "CARGO_HOME": cargo_home, "RUSTUP_HOME": rustup_home})
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
            "CARGO_PROFILE_DEV_PANIC": "abort",
            "CARGO_INCREMENTAL": "0",
            "RUSTFLAGS": "-C strip=symbols",
        }
    )
    with open(log, "w", encoding="utf-8") as handle:
        build = observed_run(
            ["cargo", "build", "-j", "1", "--locked", "--offline", "--bin", "oxide"],
            cwd=REPO,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            _merged_log=log if DIAGNOSTIC is not None else None,
            limit=None,
        )
    if DIAGNOSTIC is not None and DIAGNOSTIC["last_command"] is not None:
        # Cargo's stderr is merged into this file, so identify the merged
        # stream instead of implying that missing captured stderr was empty.
        with open(log, "rb") as handle:
            DIAGNOSTIC["last_command"]["stdout"] = bounded_output(handle.read(2048))
        DIAGNOSTIC["last_command"]["stderr"] = None
        DIAGNOSTIC["last_command"]["stderr_note"] = "merged into stdout"
    if build.returncode:
        with open(log, encoding="utf-8") as handle:
            fail(f"staging exit {build.returncode}: {handle.read()[-2000:]}")
    built = os.path.join(stage, "target", "debug", "oxide")
    expect(os.path.isfile(built), f"staging produced no binary at {built}")
    subject = os.path.join(root, "subject-oxide")
    shutil.copy2(built, subject)
    os.chmod(subject, 0o755)
    shutil.rmtree(stage)
    if DIAGNOSTIC is None:
        os.unlink(log)
    # The installer's hidden probe must report this build's own versions and
    # its compiled proof controls (debug staging); a future bump then fails
    # this proof loudly instead of mis-testing it.
    probe = observed_run(
        [subject, CAPABILITIES_PROBE],
        capture_output=True,
        text=True,
        limit=30,
    )
    expect(probe.returncode == 0, f"capability probe exit {probe.returncode}: {probe.stderr!r}")
    # The default stays exactly this proof's own declared versions, so every
    # existing caller is unchanged; a caller whose fixture needs a different
    # exact expectation passes it explicitly rather than accepting any value.
    expected = capability if capability is not None else {
        "control_state_version": STATE_VERSION,
        "registration_version": REGISTRATION_VERSION,
        "test_controls": True,
    }
    expect(
        json.loads(probe.stdout) == expected,
        f"capability probe {probe.stdout!r} disagrees with the declared expectation {expected!r}",
    )
    return subject


class Fixture:
    """One private HOME, install receipts and directory pool."""

    def __init__(self, root: str, create_pool: bool = True):
        self.root = root
        self.home = os.path.join(root, "home")
        self.pool = os.path.join(root, "pool")
        self.control = os.path.join(self.pool, "oxide-managed")
        self.bin = os.path.join(self.home, ".local/bin")
        self.receipts = os.path.join(self.home, ".local/state/oxide/install")
        self.cargo_home = os.path.join(root, "cargo-home")
        self.tools = os.path.join(root, "tools")
        for directory, mode in (
            (self.home, 0o700),
            (os.path.join(self.home, ".local"), 0o700),
            (self.bin, 0o700),
            (self.home + "/.local/state", 0o700),
            (os.path.join(self.home, ".local/state/oxide"), 0o700),
            (self.receipts, 0o700),
            (self.cargo_home, 0o700),
            (self.tools, 0o700),
        ):
            os.makedirs(directory, exist_ok=True)
            os.chmod(directory, mode)
        self.record = os.path.join(root, "record")
        # A recording Cargo that answers version probes and fails workspace
        # discovery: no fixture ever starts the real Cargo, so no toolchain
        # download or real build can leak into the proof.
        write(
            os.path.join(self.tools, "cargo"),
            "#!/bin/sh\n"
            + 'printf "%s\\n" "$@" >> ' + shlex.quote(f"{self.record}.cargo") + "\n"
            + 'if [ "$1" = "-V" ] || [ "$1" = "--version" ]; then\n'
            + "  printf 'cargo 1.97.1 (fixture)\\n'\n"
            + "  exit 0\n"
            + "fi\n"
            + "exit 1\n",
            0o755,
        )
        if create_pool:
            # Ask the product to create its own bound scratch pool, then plant
            # the hand-off control data in that genuine private pool.
            workspace = os.path.join(root, "setup-workspace")
            manifest = os.path.join(workspace, "Cargo.toml")
            write(manifest, '[package]\nname = "handoff-setup"\nversion = "0.1.0"\nedition = "2021"\n', 0o644)
            write(os.path.join(workspace, "src/lib.rs"), "pub fn value() {}\n", 0o644)
            cargo = os.path.join(self.tools, "cargo")
            with open(cargo, encoding="utf-8") as handle:
                original = handle.read()
            setup = ("#!/bin/sh\n"
                     "if [ \"$1\" = -V ] || [ \"$1\" = --version ]; then echo 'cargo 1.97.1 (fixture)'; exit 0; fi\n"
                     "if [ \"$1\" = locate-project ]; then echo " + shlex.quote(manifest) + "; exit 0; fi\n"
                     "exit 1\n")
            write(cargo, setup, 0o755)
            result = observed_run([SUBJECT, "check"], cwd=workspace, env=self.env(SUBJECT),
                                    capture_output=True, text=True, limit=30,
                                    _fixture=os.path.basename(self.root), _trace=True)
            write(cargo, original, 0o755)
            expect(result.returncode == 1 and os.path.isfile(os.path.join(self.pool, "ledger.json")),
                   f"fixture setup did not create its pool: {result.stderr[-2000:]}")
            expect(os.path.isfile(os.path.join(self.control, "scratch.json")),
                   f"fixture scratch creation failed: {result.stderr[-2000:]}")

    @property
    def destination(self) -> str:
        return os.path.join(self.bin, "oxide")

    def plant_state(self, version: int) -> None:
        write(os.path.join(self.control, "state.json"), current_state(version), 0o600)

    def plant_registration(self, version: int) -> None:
        write(
            os.path.join(self.control, "removable.json"),
            json.dumps({"version": version}),
            0o600,
        )

    def stub(self) -> None:
        """Install the compiled recording candidate as this fixture's oxide."""
        install_stub(self.destination)

    def receipt(
        self,
        name: str = "20260101T000000Z-aaaaaaaaaaaa.json",
        *,
        path: str | None = None,
        digest: str | None = None,
        size: int | None = None,
        state: int | None = NEWER_STATE_VERSION,
        registration: int | None = REGISTRATION_VERSION,
    ) -> None:
        installed = {
            "path": path if path is not None else self.destination,
            "sha256": digest if digest is not None else sha256(self.destination),
            "size": size if size is not None else os.path.getsize(self.destination),
        }
        document = {"installed": installed}
        if state is not None:
            document["control_state_version"] = state
        if registration is not None:
            document["registration_version"] = registration
        write(os.path.join(self.receipts, name), json.dumps(document), 0o644)

    def env(self, binary: str) -> dict:
        return {
            "HOME": self.home,
            "OXIDE_TEST_HOME_FROM_ENV": "1",
            "OXIDE_POOL": self.pool,
            "OXIDE_SCRATCH_ROOT": SCRATCH_ROOT,
            "OXIDE_POOL_CAP": "64MiB",
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTFLAGS": "-C strip=symbols",
            "CARGO_HOME": self.cargo_home,
            "PATH": ":".join(
                [
                    self.bin,
                    self.tools,
                    "/usr/bin",
                    "/bin",
                    "/usr/sbin",
                    "/sbin",
                    TOOLCHAIN_BIN,
                ]
            ),
            "OXIDE_TASK_SCRATCH_ROOT": os.environ["OXIDE_TASK_SCRATCH_ROOT"],
            "TMPDIR": os.environ["OXIDE_TASK_SCRATCH_ROOT"],
            "TMP": os.environ["OXIDE_TASK_SCRATCH_ROOT"],
            "TEMP": os.environ["OXIDE_TASK_SCRATCH_ROOT"],
            "OXIDE_HANDOFF_TOKEN": f"token-{os.path.basename(self.root)}",
            "OXIDE_HANDOFF_RECORD": self.record,
            "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", os.path.join(os.path.expanduser("~"), ".rustup")),
        }

    def control_listing(self) -> list:
        return sorted(os.listdir(self.control))

    def cargo_calls(self) -> list:
        path = f"{self.record}.cargo"
        if not os.path.exists(path):
            return []
        with open(path, encoding="utf-8") as handle:
            return handle.read().splitlines()

    def stub_runs(self) -> int:
        path = f"{self.record}.count"
        if not os.path.exists(path):
            return 0
        with open(path, encoding="utf-8") as handle:
            return int(handle.read() or "0")

    def stub_argv(self) -> list:
        with open(f"{self.record}.argv", "rb") as handle:
            entries = [entry for entry in handle.read().split(b"\0") if entry]
        return [entry.decode() for entry in entries]

    def stub_env(self) -> dict:
        with open(f"{self.record}.env", "rb") as handle:
            entries = [entry for entry in handle.read().split(b"\0") if entry]
        captured = {}
        for entry in entries:
            name, _, value = entry.partition(b"=")
            captured[name.decode()] = value.decode()
        return captured



def run(fixture: Fixture, binary: str, args: list, env_extra: dict | None = None, timeout: float = 30):
    env = fixture.env(binary)
    if env_extra:
        env.update(env_extra)
    return observed_run(
        [binary, *args],
        env=env,
        capture_output=True,
        text=True,
        limit=timeout,
        _fixture=os.path.basename(fixture.root),
    )


def expect_handoff_refusal(result, fixture: Fixture, name: str, before: list | None = None) -> None:
    expect(result.returncode == 75, f"{name}: exit {result.returncode}, stderr {result.stderr!r}")
    expect(result.stderr == REFUSAL, f"{name}: stderr {result.stderr!r} is not the pinned refusal")
    expect(result.stdout == "", f"{name}: stdout {result.stdout!r}")
    expect(fixture.stub_runs() == 0, f"{name}: the candidate ran")
    expect(fixture.cargo_calls() == [], f"{name}: Cargo ran")
    if before is not None:
        expect(fixture.control_listing() == before, f"{name}: control changed to {fixture.control_listing()}")


def newer_fixture(root: str, name: str) -> Fixture:
    fixture = Fixture(os.path.join(root, name))
    fixture.plant_state(NEWER_STATE_VERSION)
    return fixture


def check_handoff(root: str, binary: str) -> None:
    case("newer_state_hands_off_once")
    fixture = newer_fixture(root, "handoff")
    fixture.stub()
    fixture.receipt()
    argv = ["check", "--profile", "debug", "odd arg with spaces", "ünicode"]
    extra = {"OXIDE_HANDOFF_EXIT": "42"}
    before = fixture.control_listing()
    result = run(fixture, binary, argv, extra)
    expect(result.returncode == 42, f"hand-off exit {result.returncode}, stderr {result.stderr!r}")
    expect(result.stderr == NOTICE, f"hand-off stderr {result.stderr!r} is not one notice")
    expect(fixture.stub_runs() == 1, f"candidate ran {fixture.stub_runs()} times")
    expect(fixture.stub_argv() == [binary, *argv], f"argv {fixture.stub_argv()!r}")
    captured = fixture.stub_env()
    expected_env = dict(fixture.env(binary), **extra)
    expected_env["OXIDE_INTERNAL_HANDED_OFF"] = "1"
    expect(captured == expected_env, f"environment differs: {captured!r} vs {expected_env!r}")
    expect(fixture.control_listing() == before, f"control changed to {fixture.control_listing()}")
    expect(fixture.cargo_calls() == [], "Cargo ran during a hand-off")

    case("marker_present_refuses_even_with_newer_state")
    fixture = newer_fixture(root, "marker-refuses")
    fixture.stub()
    fixture.receipt()
    for value in ("1", "0", ""):
        result = run(fixture, binary, ["status"], {"OXIDE_INTERNAL_HANDED_OFF": value})
        expect_handoff_refusal(result, fixture, f"marker {value!r}")

    case("marker_is_inert_without_newer_state")
    fixture = Fixture(os.path.join(root, "marker-inert"))
    fixture.plant_state(STATE_VERSION)
    fixture.stub()
    fixture.receipt()
    result = run(fixture, binary, ["status"], {"OXIDE_INTERNAL_HANDED_OFF": "1"})
    expect(result.returncode == 0, f"marker inert exit {result.returncode}: {result.stderr!r}")
    expect(NOTICE not in result.stderr, "marker inert printed the hand-off notice")
    expect(REFUSAL not in result.stderr, "marker inert refused")
    expect(fixture.stub_runs() == 0, "marker inert ran the candidate")


def check_no_trigger(root: str, binary: str) -> None:
    case("non_numeric_older_and_malformed_documents_do_not_trigger")
    documents = {
        "malformed": "{",
        "array": "[]",
        "older": current_state(RECORDED_ACCOUNTING_STATE_VERSION),
        "string_version": json.dumps({"version": str(NEWER_STATE_VERSION)}),
        "negative": json.dumps({"version": -1}),
        "above_u32": json.dumps({"version": 2**32}),
        "float": json.dumps({"version": 3.5}),
        "missing_version": "{}",
    }
    for name, contents in documents.items():
        fixture = Fixture(os.path.join(root, f"document-{name}"))
        fixture.plant_state(STATE_VERSION)
        write(os.path.join(fixture.control, "state.json"), contents, 0o600)
        fixture.stub()
        fixture.receipt()
        result = run(fixture, binary, ["status"])
        expect(result.returncode == 0, f"{name}: exit {result.returncode}, stderr {result.stderr!r}")
        expect(NOTICE not in result.stderr, f"{name}: hand-off notice printed")
        expect(REFUSAL not in result.stderr, f"{name}: refused as newer")
        expect(fixture.stub_runs() == 0, f"{name}: candidate ran")

    case("malformed_and_oversized_documents_keep_the_existing_refusal")
    fixture = Fixture(os.path.join(root, "oversized"))
    write(
        os.path.join(fixture.control, "state.json"),
        json.dumps({"version": STATE_VERSION, "padding": "x" * (17 * MIB)}),
        0o600,
    )
    result = run(fixture, binary, ["status"])
    expect(result.returncode == 0, f"oversized exit {result.returncode}")
    expect(REFUSAL not in result.stderr and NOTICE not in result.stderr, "oversized document handed off")

    case("no_pool_means_no_trigger")
    fixture = Fixture(os.path.join(root, "no-pool"), create_pool=False)
    fixture.stub()
    fixture.receipt()
    result = run(fixture, binary, ["status"])
    expect(result.returncode == 0, f"no pool exit {result.returncode}: {result.stderr!r}")
    expect(NOTICE not in result.stderr and REFUSAL not in result.stderr, "no pool handed off")
    expect(not os.path.exists(fixture.pool), "status created the pool")
    expect(fixture.stub_runs() == 0, "no pool ran the candidate")


def check_candidate_refusals(root: str, binary: str) -> None:
    case("newer_registration_without_a_candidate_refuses")
    fixture = Fixture(os.path.join(root, "registration"))
    fixture.plant_state(STATE_VERSION)
    fixture.plant_registration(REGISTRATION_VERSION + 1)
    before = fixture.control_listing()
    expect_handoff_refusal(run(fixture, binary, ["status"]), fixture, "registration", before)

    case("newer_state_without_receipts_or_candidate_refuses")
    fixture = newer_fixture(root, "no-receipt")
    before = fixture.control_listing()
    expect_handoff_refusal(run(fixture, binary, ["status"]), fixture, "no receipt", before)
    fixture.stub()
    expect_handoff_refusal(run(fixture, binary, ["status"]), fixture, "no receipt with candidate", before)

    def refused(name, mutate, *, args=None, env_for=None):
        fixture = newer_fixture(root, f"refuse-{name}")
        fixture.stub()
        fixture.receipt()
        mutate(fixture)
        before = fixture.control_listing()
        extra = env_for(fixture) if env_for else None
        result = run(fixture, binary, args or ["status"], extra)
        expect_handoff_refusal(result, fixture, name, before)

    refused(
        "hash",
        lambda fixture: fixture.receipt(digest=hashlib.sha256(b"other").hexdigest()),
    )
    refused("size", lambda fixture: fixture.receipt(size=os.path.getsize(fixture.destination) + 1))
    refused("missing_capabilities", lambda fixture: fixture.receipt(state=None, registration=None))
    refused("low_state_capability", lambda fixture: fixture.receipt(state=STATE_VERSION))
    refused(
        "receipt_path",
        lambda fixture: fixture.receipt(path=os.path.join(fixture.root, "elsewhere/oxide")),
    )
    refused("not_executable", lambda fixture: os.chmod(fixture.destination, 0o644))
    refused(
        "symlinked_candidate",
        lambda fixture: (
            os.rename(fixture.destination, os.path.join(fixture.root, "real-oxide")),
            os.symlink(os.path.join(fixture.root, "real-oxide"), fixture.destination),
        ),
    )
    refused(
        "symlinked_bin_directory",
        lambda fixture: (
            os.rename(fixture.bin, os.path.join(fixture.root, "moved-bin")),
            os.symlink(os.path.join(fixture.root, "moved-bin"), fixture.bin),
        ),
    )
    refused("no_path_entry", lambda fixture: None, env_for=lambda fixture: {"PATH": "/usr/bin:/bin"})
    refused(
        "another_oxide_on_path",
        lambda fixture: write(os.path.join(fixture.root, "other/oxide"), "#!/bin/sh\nexit 0\n", 0o755),
        env_for=lambda fixture: {
            "PATH": ":".join(
                [
                    os.path.dirname(os.path.join(fixture.root, "other/oxide")),
                    fixture.bin,
                    fixture.tools,
                    "/usr/bin",
                    "/bin",
                    "/usr/sbin",
                    "/sbin",
                ]
            )
        },
    )

    case("a_receipt_that_does_not_cover_a_newer_registration_refuses")
    fixture = Fixture(os.path.join(root, "low-registration"))
    fixture.plant_state(STATE_VERSION)
    fixture.plant_registration(REGISTRATION_VERSION + 1)
    fixture.stub()
    fixture.receipt(state=STATE_VERSION, registration=REGISTRATION_VERSION)
    before = fixture.control_listing()
    expect_handoff_refusal(run(fixture, binary, ["status"]), fixture, "low registration", before)

    case("the_newest_receipt_is_the_only_candidate")
    fixture = newer_fixture(root, "newest-receipt")
    fixture.stub()
    fixture.receipt("20260101T000000Z-aaaaaaaaaaaa.json")
    write(
        os.path.join(fixture.receipts, "20260201T000000Z-bbbbbbbbbbbb.json"),
        json.dumps({"installed": {"path": "/elsewhere/oxide", "sha256": "0" * 64, "size": 1}}),
        0o644,
    )
    before = fixture.control_listing()
    expect_handoff_refusal(run(fixture, binary, ["status"]), fixture, "newest receipt", before)

    case("a_replacement_before_the_last_check_refuses")
    fixture = newer_fixture(root, "replacement")
    fixture.stub()
    fixture.receipt()
    replacement = os.path.join(fixture.root, "replacement")
    with open(STUB, "rb") as handle:
        replaced = handle.read() + b"\0"
    with open(replacement, "wb") as handle:
        handle.write(replaced)
    os.chmod(replacement, 0o755)
    before = fixture.control_listing()
    result = run(
        fixture,
        binary,
        ["status"],
        {"OXIDE_TEST_HANDOFF_REPLACE": replacement},
    )
    expect_handoff_refusal(result, fixture, "replacement", before)
    with open(fixture.destination, "rb") as handle:
        expect(handle.read() == replaced, "the replacement hook did not replace the candidate")


def check_skipped_commands(root: str, binary: str) -> None:
    case("plain_passthrough_skips_the_hand_off_check")
    fixture = newer_fixture(root, "skip-plain")
    fixture.stub()
    fixture.receipt()
    write(
        os.path.join(fixture.tools, "cargo"),
        "#!/bin/sh\n"
        + 'printf "%s\\n" "$@" > ' + shlex.quote(f"{fixture.record}.cargo") + "\n"
        + 'printf "cargo ran\\n"\nexit 0\n',
        0o755,
    )
    result = run(fixture, binary, ["fmt", "--all", "--", "--check"])
    expect(result.returncode == 0, f"plain exit {result.returncode}: {result.stderr!r}")
    expect(NOTICE not in result.stderr and REFUSAL not in result.stderr, "plain passthrough handed off")
    expect(fixture.stub_runs() == 0, "plain passthrough ran the candidate")
    expect(fixture.cargo_calls() == ["fmt", "--all", "--", "--check"], f"cargo calls {fixture.cargo_calls()}")

    case("usage_only_exec_skips_the_hand_off_check")
    fixture = newer_fixture(root, "skip-exec")
    for args, code in ((["exec", "--help"], 0), (["exec"], 1)):
        result = run(fixture, binary, args)
        expect(result.returncode == code, f"exec usage {args} exit {result.returncode}: {result.stderr!r}")
        expect(NOTICE not in result.stderr and REFUSAL not in result.stderr, f"exec usage {args} handed off")

    case("launch_check_probe_skips_the_hand_off_check")
    fixture = newer_fixture(root, "skip-launch")
    result = run(
        fixture,
        binary,
        ["status", "--launch-check"],
        {"OXIDE_TEST_LAUNCH_LIMIT_MS": "20000"},
    )
    expect(result.returncode == 0, f"launch check exit {result.returncode}: {result.stderr!r}")
    expect(NOTICE not in result.stderr and REFUSAL not in result.stderr, "launch check handed off")


def check_same_binary(root: str, binary: str) -> None:
    case("the_running_binary_refuses_to_hand_off_to_itself")
    fixture = newer_fixture(root, "same-inode")
    subject = os.path.join(fixture.root, "subject-oxide")
    try:
        os.link(binary, subject)
    except OSError:
        shutil.copy2(binary, subject)
    if os.path.lexists(fixture.destination):
        os.unlink(fixture.destination)
    try:
        os.link(subject, fixture.destination)
    except OSError:
        shutil.copy2(subject, fixture.destination)
        os.chmod(fixture.destination, 0o755)
    fixture.receipt()
    before = fixture.control_listing()
    result = run(fixture, subject, ["status"])
    expect_handoff_refusal(result, fixture, "same inode", before)


def check_final_release(root: str, binary: str) -> None:
    case("a_final_release_error_never_replays_the_command")
    fixture = Fixture(os.path.join(root, "final-release"), create_pool=False)
    # Let the first command create the pool properly (config, ledger and
    # control), so the real run starts from an existing current-version pool.
    workspace = os.path.join(fixture.root, "workspace")
    os.makedirs(workspace)
    write(
        os.path.join(workspace, "Cargo.toml"),
        '[package]\nname = "handoff-fixture"\nversion = "0.1.0"\nedition = "2021"\n',
        0o644,
    )
    write(os.path.join(workspace, "src/lib.rs"), "pub fn value() -> u32 { 1 }\n", 0o644)
    fixture.stub()
    fixture.receipt()
    state = os.path.join(fixture.control, "state.json")
    header = (
        "#!/bin/sh\n"
        + 'printf "%s\\n" "$@" >> ' + shlex.quote(f"{fixture.record}.cargo") + "\n"
        + 'if [ "$1" = "-V" ] || [ "$1" = "--version" ]; then\n'
        + "  printf 'cargo 1.97.1 (fixture)\\n'\n"
        + "  exit 0\n"
        + "fi\n"
        + 'if [ "$1" = "locate-project" ]; then\n'
        + "  printf '%s\\n' " + shlex.quote(os.path.join(workspace, "Cargo.toml")) + "\n"
        + "  exit 0\n"
        + "fi\n"
    )
    # A first command creates the current pool and fenced state; Cargo fails,
    # so nothing else happens.
    write(os.path.join(fixture.tools, "cargo"), header + "exit 1\n", 0o755)
    scaffold = observed_run(
        [binary, "check"],
        env=fixture.env(binary),
        cwd=workspace,
        capture_output=True,
        text=True,
        limit=60,
    )
    expect(scaffold.returncode == 1, f"scaffold exit {scaffold.returncode}: {scaffold.stderr!r}")
    with open(state, encoding="utf-8") as handle:
        expect(json.load(handle)["version"] == WITNESS_STATE_VERSION, "the pool did not start at the recorded-accounting state version")
    with open(os.path.join(fixture.pool, "ledger.json"), encoding="utf-8") as handle:
        expect(json.load(handle)["version"] == LEDGER_VERSION, "the pool ledger version drifted")
    expect(NOTICE not in scaffold.stderr, "the scaffold handed off")
    # The real command: a newer writer fences the state while Cargo runs.
    # The newer writer waits until the command has recorded its running
    # holder, so the fence lands after the command's last own save and the
    # final release is guaranteed to read it (no write-order race).
    poll = (
        "tries=0\n"
        "while [ $tries -lt 200 ]; do\n"
        "  if grep -q '\"phase\":\"running\"' " + shlex.quote(state) + " 2>/dev/null; then break; fi\n"
        "  sleep 0.05\n"
        "  tries=$((tries + 1))\n"
        "done\n"
    )
    write(
        os.path.join(fixture.tools, "cargo"),
        header + poll + "printf '%s' " + shlex.quote(current_state(NEWER_STATE_VERSION)) + " > " + shlex.quote(state) + "\nexit 0\n",
        0o755,
    )
    result = observed_run(
        [binary, "check"],
        env=fixture.env(binary),
        cwd=workspace,
        capture_output=True,
        text=True,
        limit=60,
    )
    expect(result.returncode == 1, f"final release exit {result.returncode}: {result.stderr!r}")
    expect(
        "unsupported version" in result.stderr,
        f"final release stderr {result.stderr!r} is not the existing refusal",
    )
    expect(NOTICE not in result.stderr, "final release printed the hand-off notice")
    expect(REFUSAL not in result.stderr, "final release used the hand-off refusal")
    expect(fixture.stub_runs() == 0, "final release executed the candidate")
    expect(fixture.cargo_calls() != [], "the command never reached Cargo, so it had no effects to replay")
    with open(state, encoding="utf-8") as handle:
        expect(json.load(handle)["version"] == NEWER_STATE_VERSION, "the newer state was not written")
    expect(
        os.path.exists(os.path.join(fixture.control, "admission.lock")),
        "the pool shows no command effects",
    )

def check_cargo_exit_75(root: str, binary: str) -> None:
    case("cargo_exit_75_is_propagated_and_is_not_a_trigger")
    fixture = Fixture(os.path.join(root, "cargo-75"), create_pool=False)
    workspace = os.path.join(fixture.root, "workspace")
    os.makedirs(workspace)
    write(
        os.path.join(workspace, "Cargo.toml"),
        '[package]\nname = "handoff-fixture"\nversion = "0.1.0"\nedition = "2021"\n',
        0o644,
    )
    write(os.path.join(workspace, "src/lib.rs"), "pub fn value() -> u32 { 1 }\n", 0o644)
    fixture.stub()
    fixture.receipt()
    write(
        os.path.join(fixture.tools, "cargo"),
        "#!/bin/sh\n"
        + 'if [ "$1" = "-V" ] || [ "$1" = "--version" ]; then\n'
        + "  printf 'cargo 1.97.1 (fixture)\\n'\n"
        + "  exit 0\n"
        + "fi\n"
        + 'if [ "$1" = "locate-project" ]; then\n'
        + "  printf '%s\\n' " + shlex.quote(os.path.join(workspace, "Cargo.toml")) + "\n"
        + "  exit 0\n"
        + "fi\n"
        + "exit 75\n",
        0o755,
    )
    # The first command creates the pool; the second starts from an existing
    # current-version pool. Both must return Cargo's 75 untouched.
    for label in ("creating", "existing"):
        result = observed_run(
            [binary, "check"],
            env=fixture.env(binary),
            cwd=workspace,
            capture_output=True,
            text=True,
            limit=60,
        )
        expect(result.returncode == 75, f"cargo 75 {label}: exit {result.returncode}: {result.stderr!r}")
        expect(NOTICE not in result.stderr and REFUSAL not in result.stderr, f"cargo 75 {label} handed off")
        expect(fixture.stub_runs() == 0, f"cargo 75 {label} ran the candidate")
    state = os.path.join(fixture.control, "state.json")
    with open(state, encoding="utf-8") as handle:
        expect(json.load(handle)["version"] == WITNESS_STATE_VERSION, "the pool state is not the recorded-accounting version")


# Each group has independent fixture roots. A failed oracle can leave a child
# descendant, so after any child launch the diagnostic aborts and leaves the
# supervisor to decide closure; it never treats a reaped leader as a census.
GROUPS = (
    ("handoff", check_handoff), ("no-trigger", check_no_trigger),
    ("candidate-refusals", check_candidate_refusals),
    ("skipped-commands", check_skipped_commands), ("same-binary", check_same_binary),
    ("cargo-exit", check_cargo_exit_75), ("final-release", check_final_release),
)


def collect(groups, invoke, outcome, launched):
    for index, (name, body) in enumerate(groups):
        try:
            invoke(body)
            if outcome["active"] is not None:
                end_case(outcome)
                outcome["completed"].append(outcome["active"])
                outcome["states"][outcome["active"]] = "completed"
                outcome["active"] = None
                publish_diagnostic(outcome)
            outcome["groups_completed"].append(name)
            publish_diagnostic(outcome)
        except SystemExit as error:
            end_case(outcome)
            message = getattr(error, "message", f"SystemExit {error.code}")
            outcome["failed"].append({"case": outcome["active"], "group": name,
                                      "error": str(message)[:256],
                                      "last_command": outcome["last_command"]})
            if outcome["active"] is not None:
                outcome["states"][outcome["active"]] = "failed"
            outcome["active"] = None
            publish_diagnostic(outcome)
            if launched():
                outcome["aborted"] = "writer closure unknown after failure"
                outcome["unexecuted_groups"] = [item[0] for item in groups[index + 1:]]
                publish_diagnostic(outcome)
                return
        except Exception as error:
            end_case(outcome)
            outcome["failed"].append({"case": outcome["active"], "group": name,
                                      "error": type(error).__name__,
                                      "last_command": outcome["last_command"]})
            if outcome["active"] is not None:
                outcome["states"][outcome["active"]] = "aborted"
            outcome["aborted"] = getattr(error, "message", type(error).__name__)[:256]
            outcome["unexecuted_groups"] = [item[0] for item in groups[index + 1:]]
            publish_diagnostic(outcome)
            return


def empty_outcome():
    return {"qualified": False, "states": {name: "unexecuted" for name in CASE_PLAN}, "reached": [], "completed": [], "failed": [],
            "active": None, "groups_completed": [], "unexecuted_groups": [],
            "aborted": None, "case_times": {}, "commands": [], "commands_omitted": 0,
            "last_command": None, "phase": None, "phases": [], "trace_attribution": None,
            "start_ns": time.monotonic_ns(), "end_ns": None}


def diagnostic_exit(outcome):
    """A clean collection is still unqualified; incomplete evidence is failure,
    and so is an insufficient trace-attribution result: a collection whose
    instrumented commands were all refused or produced no raw trace evidence
    never passes beside passing case oracles."""
    attribution = outcome.get("trace_attribution")
    instrumented = (isinstance(attribution, dict) and attribution.get("sufficient") is True
                    and not attribution.get("problems"))
    clean = (outcome["qualified"] is False and not outcome["failed"]
             and outcome["aborted"] is None and outcome["active"] is None
             and not outcome["unexecuted_groups"]
             and outcome["groups_completed"] == [name for name, _ in GROUPS]
             and outcome["reached"] == list(CASE_PLAN)
             and outcome["completed"] == list(CASE_PLAN)
             and outcome["states"] == {name: "completed" for name in CASE_PLAN})
    return 0 if clean and instrumented else 1


def self_check():
    global DIAGNOSTIC, DIAGNOSTIC_WRITER
    # Exercise receipt publication and interruption without a child or fixture.
    previous_diagnostic = DIAGNOSTIC
    previous_writer = DIAGNOSTIC_WRITER
    snapshots = []
    DIAGNOSTIC_WRITER = lambda encoded: snapshots.append(json.loads(encoded))
    DIAGNOSTIC = empty_outcome()
    try:
        timeouts = []
        def interrupted(argv, **kwargs):
            timeouts.append(kwargs["timeout"])
            saved = snapshots[-1]["last_command"]
            assert saved["command"] == ["cargo", "build"]
            assert saved["state"] == "launching" and saved["exit"] is None and saved["end_ns"] is None
            raise KeyboardInterrupt()
        try:
            observed_run(["cargo", "build"], limit=60, _runner=interrupted, _remaining=lambda: 7)
            assert False, "interruption was not propagated"
        except KeyboardInterrupt:
            pass
        assert timeouts == [7]
        saved = snapshots[-1]["last_command"]
        assert saved["exit"] is None and saved["state"] == "outcome unknown"
        assert saved["end_ns"] >= saved["start_ns"]
        assert len(json.dumps(snapshots[-1]).encode("utf-8")) < 2 * MIB
        count = len(snapshots)
        try:
            publish_diagnostic({"oversized": "x" * (2 * MIB)})
            assert False, "oversized receipt accepted"
        except RuntimeError:
            pass
        assert len(snapshots) == count
        wait = cache_wait_observation(b"    Blocking waiting for file lock on package cache\n")
        assert wait["state"] == "observed" and "package cache" in wait["line"]
        assert cache_wait_observation(b"")["state"] == "not observed in bounded prefix"
        DIAGNOSTIC = empty_outcome()
        DIAGNOSTIC["active"] = CASE_PLAN[0]
        fake = lambda argv, **kwargs: subprocess.CompletedProcess(argv, 42, "o" * 3000, "e" * 3000)
        result = observed_run(["fixture", "check"], limit=30, _runner=fake, _remaining=lambda: 10,
                              _fixture="f" * 200)
        row = DIAGNOSTIC["commands"][0]
        assert result.returncode == row["exit"] == 42
        assert row["case"] == CASE_PLAN[0] and row["command"] == ["fixture", "check"]
        assert row["fixture"] == "f" * 128
        assert row["start_ns"] <= row["end_ns"] and len(row["stderr"]) == len(row["stdout"]) == 2048
        def stopped():
            raise ProofFailure("work deadline")
        try:
            observed_run(["fixture", "status"], limit=30, _runner=fake, _remaining=stopped)
            assert False, "deadline did not stop launch"
        except ProofFailure:
            pass
        row = DIAGNOSTIC["commands"][1]
        assert row["stop"] == "work deadline" and row["exit"] is None and row["end_ns"] >= row["start_ns"]
        # A case failure carries its triggering command and elapsed window.
        DIAGNOSTIC = empty_outcome()
        old_cases = list(CASES)
        def failing_case(_):
            case(CASE_PLAN[0])
            observed_run(["fixture", "status"], limit=30, _runner=fake, _remaining=stopped)
        try:
            collect((("fixture", None),), failing_case, DIAGNOSTIC, lambda: True)
            failure = DIAGNOSTIC["failed"][0]
            timing = DIAGNOSTIC["case_times"][CASE_PLAN[0]]
            assert failure["error"] == "work deadline"
            assert failure["last_command"]["command"] == ["fixture", "status"]
            assert failure["last_command"]["exit"] is None
            assert timing["start_ns"] <= timing["end_ns"]
        finally:
            CASES[:] = old_cases
    finally:
        DIAGNOSTIC = previous_diagnostic
        DIAGNOSTIC_WRITER = previous_writer
    outcome = empty_outcome()
    calls = []
    def invoke(body):
        calls.append(body)
        if body == 1:
            raise SystemExit(1)
    collect((("one", 1), ("two", 2)), invoke, outcome, lambda: False)
    assert calls == [1, 2] and len(outcome["failed"]) == 1
    assert outcome["groups_completed"] == ["two"]
    outcome = empty_outcome()
    calls.clear()
    collect((("one", 1), ("two", 2)), invoke, outcome, lambda: True)
    assert calls == [1] and outcome["unexecuted_groups"] == ["two"]
    assert outcome["aborted"] and outcome["qualified"] is False
    assert diagnostic_exit(outcome) == 1
    clean = empty_outcome()
    clean["groups_completed"] = [name for name, _ in GROUPS]
    clean["reached"] = list(CASE_PLAN)
    clean["completed"] = list(CASE_PLAN)
    clean["states"] = {name: "completed" for name in CASE_PLAN}
    clean["trace_attribution"] = {"sufficient": True, "problems": []}
    assert diagnostic_exit(clean) == 0 and clean["qualified"] is False
    clean["trace_attribution"] = {"sufficient": False, "problems": ["layout refused"]}
    assert diagnostic_exit(clean) == 1, "insufficient instrumentation fails a clean collection"
    clean["trace_attribution"] = {"sufficient": True, "problems": []}
    clean["states"][CASE_PLAN[-1]] = "unexecuted"
    assert diagnostic_exit(clean) == 1
    clean["states"][CASE_PLAN[-1]] = "completed"
    clean["failed"].append({"case": CASE_PLAN[0]})
    assert diagnostic_exit(clean) == 1
    clean["failed"].clear()
    clean["aborted"] = "uncertain"
    assert diagnostic_exit(clean) == 1
    clean["aborted"] = None
    clean["qualified"] = True
    assert diagnostic_exit(clean) == 1
    assert parse_mode([]) == "native" and parse_mode(["--diagnostic"]) == "diagnostic"
    assert parse_mode(["--self-check"]) == "self-check" and parse_mode(["--plan"]) == "plan"
    assert parse_mode(["--diagnostic", "--plan"]) is None
    assert len({name for name, _ in GROUPS}) == len(GROUPS)
    assert len(set(CASE_PLAN)) == len(CASE_PLAN) == 17
    # Construct only an object in memory: no directories, pool, or child.
    global SCRATCH_ROOT
    old_root = SCRATCH_ROOT
    SCRATCH_ROOT = "/fixture/run"
    fixture = Fixture.__new__(Fixture)
    fixture.root = "/fixture/run/case"
    fixture.home = fixture.root + "/home"
    fixture.pool = fixture.root + "/pool"
    fixture.cargo_home = fixture.root + "/cargo-home"
    fixture.bin = fixture.home + "/.local/bin"
    fixture.tools = fixture.root + "/tools"
    fixture.record = fixture.root + "/record"
    old_scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
    os.environ["OXIDE_TASK_SCRATCH_ROOT"] = "/fixture/run"
    try:
        env = fixture.env("/fixture/binary")
        assert env["OXIDE_SCRATCH_ROOT"] == "/fixture/run"
        assert env["OXIDE_POOL"].startswith(env["OXIDE_SCRATCH_ROOT"] + "/")
        assert env["OXIDE_POOL_CAP"] == "64MiB"
        assert env["HOME"] == fixture.home and env["CARGO_HOME"] == fixture.cargo_home
        assert env["OXIDE_TEST_HOME_FROM_ENV"] == "1"
        assert env["RUSTUP_AUTO_INSTALL"] == "0" and env["RUSTUP_TOOLCHAIN"] == BUILDER
        assert "CARGO_ENCODED_RUSTFLAGS" not in env and not any(k.startswith(("XDG_", "GIT_")) for k in env)
    finally:
        SCRATCH_ROOT = old_root
        if old_scratch is None:
            os.environ.pop("OXIDE_TASK_SCRATCH_ROOT", None)
        else:
            os.environ["OXIDE_TASK_SCRATCH_ROOT"] = old_scratch
    # Synthetic supervisor layout: no product child or live account is read.
    # The qualified schema carries two independent tokens: the receipt's
    # session `run_id` and the marker's own `run_id`, bound to each other
    # through `scratch_marker.run_id` only.
    with tempfile.TemporaryDirectory(prefix="handoff-layout.", dir=os.environ["TMPDIR"]) as root:
        scratch = os.path.join(root, "scratch")
        evidence = os.path.join(root, "evidence")
        fixture_root = os.path.join(scratch, "installed-handoff.fixture")
        home = os.path.join(fixture_root, "case", "home")
        os.makedirs(home, mode=0o700)
        os.mkdir(evidence, 0o700)
        info = os.stat(root)
        session, marker_run = "9f13c2a47b8d0e5623ab4cd6ef901257", "5d8a02be71c49f3ea6d80b29c5147f36"
        marker_path = os.path.join(root, ".oxide-scratch.json")
        marker = {"version": 1, "run_id": marker_run, "root": root, "owner_uid": info.st_uid,
                  "device": info.st_dev, "inode": info.st_ino, "created_unix": 1700000000.0,
                  "concluded_unix": None, "outcome": "running", "closure": "none",
                  "kept_reason": "none", "nested_scope": "exclusive"}
        write(marker_path, json.dumps(marker), 0o600)
        receipt = {"run_id": session, "command": ["python3", "proof"], "limit_s": 600.0,
                   "root": root, "scratch": scratch, "evidence": evidence,
                   "scratch_owned": True, "start_ns": 1, "abort_threshold": 1,
                   "phase": "started: incomplete", "leader": None, "custodian": [],
                   "outcome": None, "census": None, "census_rounds": 0, "survivors": [],
                   "scratch_marker": {"path": marker_path, "state": "written",
                                      "run_id": marker_run, "reason": None}}
        receipt_path = os.path.join(evidence, "receipt.json")
        write(receipt_path, json.dumps(receipt), 0o644)
        env = {"HOME": home, "OXIDE_POOL": os.path.join(os.path.dirname(home), "pool"),
               "OXIDE_TASK_SCRATCH_ROOT": scratch, "OXIDE_SCRATCH_ROOT": fixture_root,
               "OXIDE_TEST_HOME_FROM_ENV": "1"}
        assert trace_layout(env, evidence, session), "two distinct valid tokens enable"
        assert not trace_layout(env, evidence, marker_run), "session slot refuses the marker token"
        assert not trace_layout(env, evidence, "a" * 32), "a third token refuses"
        assert not trace_layout(env, root, session), "a wrong evidence argument refuses"
        assert not trace_layout({}, evidence, session), "a missing environment refuses"

        def broken(**changes):
            document = json.loads(json.dumps(receipt))
            document.update(changes)
            write(receipt_path, json.dumps(document), 0o644)
            enabled = trace_layout(env, evidence, session)
            write(receipt_path, json.dumps(receipt), 0o644)
            return enabled

        assert not broken(run_id=marker_run), "swapped receipt session token refuses"
        assert not broken(scratch_marker={"path": marker_path, "state": "written",
                                          "run_id": session, "reason": None}), \
            "marker bound to the wrong token refuses"
        assert not broken(root=os.path.join(root, "other")), "wrong receipt root refuses"
        assert not broken(scratch=os.path.join(root, "scratch2")), "wrong receipt scratch refuses"
        assert not broken(evidence=os.path.join(root, "evidence2")), "wrong receipt evidence refuses"
        assert not broken(phase=None), "an unpublished receipt refuses"
        assert not broken(scratch_marker={"path": os.path.join(root, "elsewhere"),
                                          "state": "written", "run_id": marker_run,
                                          "reason": None}), "wrong marker path refuses"
        os.unlink(receipt_path)
        assert not trace_layout(env, evidence, session), "a missing receipt refuses"
        write(receipt_path, "{not json", 0o644)
        assert not trace_layout(env, evidence, session), "a malformed receipt refuses"
        write(receipt_path, "x" * (RECEIPT_MAX_BYTES + 1), 0o644)
        assert not trace_layout(env, evidence, session), "an oversized receipt refuses"
        link = os.path.join(evidence, "receipt-link")
        os.symlink(receipt_path, link)
        os.replace(link, receipt_path)
        assert not trace_layout(env, evidence, session), "a symlinked receipt refuses"
        os.unlink(receipt_path)
        write(receipt_path, json.dumps(receipt), 0o644)
        for field, value in (("device", info.st_dev + 1), ("inode", info.st_ino + 1),
                             ("owner_uid", info.st_uid + 1), ("root", os.path.join(root, "r")),
                             ("run_id", "short"), ("version", 2), ("outcome", "closed")):
            stale = dict(marker, **{field: value})
            write(marker_path, json.dumps(stale), 0o600)
            assert not trace_layout(env, evidence, session), f"marker {field} mismatch refuses"
            write(marker_path, json.dumps(marker), 0o600)

    # Trace attribution parses raw phase evidence and demands it (pure).
    with tempfile.TemporaryDirectory(prefix="handoff-attribution.", dir=os.environ["TMPDIR"]) as root:
        evidence = os.path.join(root, "evidence")
        os.mkdir(evidence, 0o700)
        session, marker_run = "9f13c2a47b8d0e5623ab4cd6ef901257", "5d8a02be71c49f3ea6d80b29c5147f36"
        write(os.path.join(evidence, "receipt.json"), json.dumps(
            {"run_id": session, "phase": "started: incomplete", "root": root,
             "scratch": os.path.join(root, "scratch"), "evidence": evidence,
             "scratch_marker": {"path": os.path.join(root, ".oxide-scratch.json"),
                                "state": "written", "run_id": marker_run, "reason": None}}), 0o644)
        write(os.path.join(evidence, "handoff-trace.a"), "invocation start 1\n"
              "store start 2\nscratch-create start 3\nscratch-create end 4\n"
              "store end 5\ninvocation end 6\n", 0o600)
        write(os.path.join(evidence, "handoff-trace.b"), "invocation start 7\n", 0o600)
        write(os.path.join(evidence, "handoff-trace.c"), "", 0o600)
        rows = [
            {"case": "kept", "trace": {"state": "allocated", "file": "handoff-trace.a", "run": session}},
            {"case": "unknown-end", "trace": {"state": "allocated", "file": "handoff-trace.b", "run": session}},
            {"case": "empty", "trace": {"state": "allocated", "file": "handoff-trace.c", "run": session}},
            {"case": "refused", "trace": {"state": "layout refused"}},
        ]
        attribution = attribute_traces(rows, evidence=evidence, session=session)
        assert attribution["expected_traced"] == 4 and attribution["allocated"] == 3
        assert attribution["with_evidence"] == 2 and attribution["unknown_ends"] == 1
        assert attribution["source"]["marker_run"] == marker_run
        assert not attribution["sufficient"], "an empty trace is insufficient instrumentation"
        assert attribution["problems"] == [
            "empty: no raw trace evidence (empty)",
            "refused: trace not allocated (layout refused)"]
        kept = attribution["commands"][0]
        assert kept["phases"] == {"invocation": {"starts": 1, "ends": 1},
                                  "store": {"starts": 1, "ends": 1},
                                  "scratch-create": {"starts": 1, "ends": 1}}
        assert kept["first"] == "invocation" and kept["last"] == "invocation" and kept["unpaired"] == []
        unknown = attribution["commands"][1]
        assert unknown["evidence"] == "evidence" and unknown["unpaired"] == ["invocation"] \
            and attribution["sufficient"] is False
        good = attribute_traces(rows[:1], evidence=evidence, session=session)
        assert good["sufficient"] and not good["problems"], "reached evidence suffices"
        assert not attribute_traces([], evidence=evidence, session=session)["sufficient"]
        assert not attribute_traces(rows[:1], evidence=evidence, session=marker_run)["sufficient"], \
            "a session slot with the marker token fails the receipt binding"
        assert not attribute_traces(rows[:1], evidence=os.path.join(root, "absent"),
                                    session=session)["sufficient"], "a missing guard receipt fails"
        write(os.path.join(evidence, "handoff-trace.d"), "store middle 9\n", 0o600)
        malformed = parse_trace_events(os.path.join(evidence, "handoff-trace.d"))
        assert malformed["state"] == "malformed" and malformed["events"] == 0
        assert parse_trace_events(os.path.join(root, "absent"))["state"] == "unreadable"
        still_good = attribute_traces(rows[:1], evidence=evidence, session=session)
        assert still_good["sufficient"]  # the malformed file is not among these rows
    check_source_versions()
    print("installed-handoff apparatus self-check passed")
    return 0


def parse_mode(arguments):
    modes = {(): "native", ("--plan",): "plan", ("--diagnostic",): "diagnostic",
             ("--self-check",): "self-check", ("--self-test",): "self-check"}
    return modes.get(tuple(arguments))


def main() -> int:
    global DIAGNOSTIC, DIAGNOSTIC_PATH, SUBJECT, SCRATCH_ROOT
    mode = parse_mode(sys.argv[1:])
    if mode == "plan":
        print("installed-handoff groups: " + ", ".join(name for name, _ in GROUPS))
        for name in CASE_PLAN:
            print("  " + name)
        print("supervisor bounds: 600 s, 128 MiB scratch")
        return 0
    if mode == "self-check":
        return self_check()
    expect(mode is not None, "unknown arguments")
    diagnostic = mode == "diagnostic"
    outcome = empty_outcome()
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
    expect(bool(evidence) and os.path.isdir(evidence), "not running under the shared supervisor")
    if diagnostic:
        DIAGNOSTIC = outcome
        DIAGNOSTIC_PATH = os.path.join(evidence, "installed-handoff-diagnostic.json")
        sys.stdout.reconfigure(line_buffering=True, write_through=True)
        publish_diagnostic(outcome)
    try:
        check_source_versions()
        scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
        expect(bool(scratch) and os.path.realpath(scratch) == scratch, f"scratch {scratch!r} is not canonical")
        for name in ("TMPDIR", "TMP", "TEMP"):
            expect(os.environ.get(name) == scratch, f"{name} differs from the supervisor scratch")
        root = tempfile.mkdtemp(prefix="installed-handoff.", dir=scratch)
        SCRATCH_ROOT = root
        phase("stage subject")
        binary = _stage_subject(root, isolated=True)
        SUBJECT = binary
        digest = sha256(binary)
        print(f"subject {binary} sha256 {digest}")
        phase("compile stub")
        compile_stub(root)
        if diagnostic:
            phase("cases")
            # Each group launches product children, so a failure after launch
            # is conservatively an unknown writer outcome.
            collect(GROUPS, lambda body: body(root, binary), outcome, lambda: True)
            outcome["trace_attribution"] = attribute_traces(outcome["commands"])
            return diagnostic_exit(outcome)
        for _, body in GROUPS:
            body(root, binary)
        with open(os.path.join(evidence, "installed-handoff.txt"), "w", encoding="utf-8") as handle:
            handle.write(f"subject {binary}\nsubject sha256 {digest}\n")
            handle.write(f"cases {len(CASES)}\n")
            for name in CASES:
                handle.write(f"ok {name}\n")
        print(f"installed-handoff: {len(CASES)} cases passed")
        return 0
    except BaseException as error:
        if diagnostic:
            outcome["aborted"] = getattr(error, "message", type(error).__name__)[:256]
            outcome["unexecuted_groups"] = [name for name, _ in GROUPS if name not in outcome["groups_completed"]]
        raise
    finally:
        if diagnostic:
            end_case(outcome)
            outcome["end_ns"] = time.monotonic_ns()
            if outcome["phase"] is not None:
                outcome["phases"][-1]["end_ns"] = outcome["end_ns"]
            if outcome.get("trace_attribution") is None:
                try:
                    outcome["trace_attribution"] = attribute_traces(outcome["commands"])
                except BaseException as error:  # never masks the original failure
                    outcome["trace_attribution"] = {"sufficient": False,
                                                    "problems": [f"attribution failed: {type(error).__name__}"]}
            publish_diagnostic(outcome)

if __name__ == "__main__":
    sys.exit(main())
