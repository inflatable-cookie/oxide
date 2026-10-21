#!/usr/bin/env python3
"""Bootstrap for `effigy proof:supervisor:qualify` (oxide#109).

The supervisor (`scripts/bounded-test.py`) is the candidate under test, not
an already-qualified guard, and this bootstrap does not supply the
qualification it evaluates. Its own bounds: Effigy heavy admission (before
this script starts), a clean checkout, a reserve reading before and after,
one fixed 300 s deadline on CLOCK_UPTIME_RAW covering its own finish, and
fixtures with finite self-exit fallbacks.

It runs the candidate with a 280 s limit and a 512 MiB monitored budget
around `scripts/test_supervisor.py`, with the candidate's owned root inside
this script's scratch. Pass requires: candidate exit 0, receipt phase
`monitor_complete`, closure established with command status 0 and no fault,
reserve sampling from before the gate with no gap over 1 s, the fixture
writers counted, custody at every signal, and every expected case passing.
On overrun it TERMs, then KILLs, only its own unreaped candidate child. On
pass it prints the receipt and removes its scratch; on failure it keeps it.
"""
from __future__ import annotations

import glob
import hashlib
import json
import os
import platform
import shutil
import signal
import subprocess
import sys
import tempfile
import time

NS = 1_000_000_000
WALL_NS = 300 * NS
CANDIDATE_LIMIT = 280
THRESHOLD = 22 * 1024 ** 3
GIB, MIB = 1024 ** 3, 1024 ** 2
EXPECTED_CASES = 43
SCRIPTS = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(SCRIPTS)
SOURCES = {
    "supervisor": os.path.join(SCRIPTS, "bounded-test.py"),
    "suite": os.path.join(SCRIPTS, "test_supervisor.py"),
    "bootstrap": os.path.abspath(__file__),
}
LIMITATIONS = [
    "Cooperative session scope: a process that calls setsid is seen only if tracked as a member earlier or while it holds an output pipe; arbitrary same-user writers are outside the promise.",
    "Only the pinned leader's group and the pinned monitor are ever signalled; survivors in other groups end the run UNCLOSED with scratch kept, never signalled.",
    "The census treats every unvalidated process on the host as uncertain, including zombies of unrelated parents, so such a process present after the stop yields a false UNCLOSED (fails closed).",
    "Host free space is a sampled abort threshold (0.25 s, 22 GiB = 20 GiB reserve + 2 GiB margin); no instantaneous guarantee. Owned bytes are walked every 2 s against a monitored budget, not a hard ceiling.",
    "Deadlines are enforced by a single-threaded custodian on CLOCK_UPTIME_RAW with bounded calls; no hard real-time scheduling claim.",
    "macOS only (kqueue NOTE_EXIT, sysctl kinfo_proc at self-checked offsets); Linux is not qualified.",
    "Effigy admission waits happen before this script starts and are not inside its 300 s.",
    "Synthetic fixtures model Cargo's setpgid groups; no real Cargo build or live application ran.",
]


def now_ns():
    return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)


START = now_ns()
DEADLINE = START + WALL_NS


def free_bytes(path):
    sample = os.statvfs(path)
    return sample.f_bavail * sample.f_frsize


def sha256(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def git(*args):
    return subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True, timeout=20, check=True).stdout


def check(receipt, results):
    problems = []
    outcome = receipt.get("outcome") or {}
    if receipt.get("phase") != "monitor_complete: pending_custodian":
        problems.append(f"receipt phase {receipt.get('phase')}")
    if (outcome.get("closure"), outcome.get("status"), outcome.get("fault"), outcome.get("deadline")) != (
            "closed", 0, None, False):
        problems.append(f"candidate outcome {outcome}")
    reserve = receipt["reserve"]
    gate = [r["at"] for r in receipt["custodian"] if r["event"] == "gate_open"]
    if not reserve["samples"] or not gate or reserve["first_ns"] >= gate[0]:
        problems.append("reserve sampling did not start before the gate opened")
    if reserve["max_gap_ns"] > NS:
        problems.append(f"reserve sample gap {reserve['max_gap_ns']} ns over 1 s")
    owned = receipt["owned_bytes"]
    if owned["maximum"] is None or not 5 * MIB <= owned["maximum"] <= owned["budget"]:
        problems.append(f"owned-byte peak {owned['maximum']} does not show the fixture writers within budget")
    for entry in receipt["custodian"]:
        if entry["event"] == "signal" and not (entry["held_unreaped"] and entry["pin"] in ("leader", "monitor")):
            problems.append(f"signal without custody: {entry}")
    outcomes = results.get("outcomes", [])
    failed = [case for case, verdict in outcomes if verdict != "pass"]
    if failed or len(outcomes) != EXPECTED_CASES:
        problems.append(f"cases: {len(outcomes)} of {EXPECTED_CASES} recorded, failed {failed}")
    return problems


def main():
    print("proof:supervisor:qualify: the supervisor is the candidate under test; this bootstrap (Effigy heavy"
          " admission, reserve readings, fixed 300 s deadline, finite fixture fallbacks) does not qualify it",
          flush=True)
    if sys.platform != "darwin":
        print("refused: macOS only", file=sys.stderr)
        return 2
    head = git("rev-parse", "HEAD").strip()
    if git("status", "--porcelain").strip():
        print("refused: the checkout is not clean, so HEAD does not name the tested source", file=sys.stderr)
        return 2
    initial = free_bytes("/private/tmp")
    print(f"bootstrap host free {initial / GIB:.1f} GiB (abort threshold 22 GiB)", flush=True)
    if initial < THRESHOLD:
        print("refused: host free space below 22 GiB", file=sys.stderr)
        return 2
    base = os.path.realpath(tempfile.mkdtemp(prefix="oxide-supervisor-qualify.", dir="/private/tmp"))
    env = {k: v for k, v in os.environ.items() if k not in ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_SUPERVISOR_INJECT")}
    env["OXIDE_SUPERVISOR_CASES"] = os.path.join(base, "cases.json")
    candidate = subprocess.Popen(
        [sys.executable, SOURCES["supervisor"], "--limit", str(CANDIDATE_LIMIT), "--scratch-budget-mib", "512",
         "--root-parent", base, "--", sys.executable, SOURCES["suite"]], env=env)
    problems = []
    try:
        code = candidate.wait(timeout=(DEADLINE - 15 * NS - now_ns()) / NS)
    except subprocess.TimeoutExpired:
        code = None
        problems.append("candidate overran the bootstrap deadline")
        for number in (signal.SIGTERM, signal.SIGKILL):  # our own unreaped child only
            candidate.send_signal(number)
            try:
                code = candidate.wait(timeout=3)
                break
            except subprocess.TimeoutExpired:
                continue
    if code != 0:
        problems.append(f"candidate exit {code}")
    final = free_bytes("/private/tmp")
    if final < THRESHOLD:
        problems.append(f"final host free {final} below 22 GiB")
    receipt, results = {}, {}
    try:
        [path] = glob.glob(os.path.join(base, "oxide-proof.*", "evidence", "receipt.json"))
        with open(path) as handle:
            receipt = json.load(handle)
        with open(env["OXIDE_SUPERVISOR_CASES"]) as handle:
            results = json.load(handle)
        problems += check(receipt, results)
    except (OSError, ValueError, KeyError) as error:
        problems.append(f"missing or malformed candidate evidence: {error!r}")
    reserve, owned = receipt.get("reserve", {}), receipt.get("owned_bytes", {})
    qualification = {
        "selector": "proof:supervisor:qualify",
        "result": "fail" if problems else "pass",
        "problems": problems,
        "head": head,
        "sha256": {name: sha256(path) for name, path in SOURCES.items()},
        "platform": {"system": platform.platform(), "machine": platform.machine(), "python": platform.python_version()},
        "candidate_exit": code,
        "candidate_phase": receipt.get("phase"),
        "candidate_outcome": receipt.get("outcome"),
        "reserve": {"samples": reserve.get("samples"), "minimum_bytes": reserve.get("minimum"),
                    "max_gap_ns": reserve.get("max_gap_ns")},
        "owned_bytes_peak": owned.get("maximum"),
        "candidate_signals": [r for r in receipt.get("custodian", []) if r["event"] == "signal"],
        "host_free_bytes": {"initial": initial, "final": final},
        "cases": results.get("cases"),
        "outcomes": results.get("outcomes"),
        "limitations": LIMITATIONS,
    }
    if now_ns() > DEADLINE:
        qualification["result"] = "fail"
        problems.append("bootstrap finished after its 300 s deadline")
    qualification["bootstrap_wall_ns"] = now_ns() - START
    print("qualification receipt:", json.dumps(qualification, indent=1), flush=True)
    if problems:
        print(f"FAIL; evidence kept in {base}", file=sys.stderr)
        return 1
    shutil.rmtree(base)
    return 0


if __name__ == "__main__":
    sys.exit(main())
