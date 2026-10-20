#!/usr/bin/env python3
"""Bootstrap for `effigy proof:supervisor:qualify` (oxide#109).

The supervisor (`scripts/bounded-test.py`) is the candidate under test, not
an already-qualified guard. This short fixed script gives the bootstrap
bounds around it: Effigy heavy admission (before this script starts), a
healthy initial and final host-reserve reading, one fixed 300 s wall limit
of its own, and tiny disposable fixtures. It does not supply the
qualification it is evaluating.

The candidate runs the synthetic suite (`scripts/test_supervisor.py`) as its
supervised command with a 280 s limit and a 512 MiB monitored scratch budget,
owning its scratch. The suite's fault-injection cases run before its tiny
writers. On success this prints a receipt binding the exact sources, the
platform, every case and the limitations, and removes its own scratch; on any
failure the scratch is kept. If the candidate overruns, this script TERMs
then KILLs only the candidate's own session group and fails.
"""
from __future__ import annotations

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

WALL = 300
CANDIDATE_LIMIT = 280
THRESHOLD = 22 * 1024 ** 3
GIB = 1024 ** 3
MIB = 1024 ** 2
STARTED = time.monotonic()
SCRIPTS = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(SCRIPTS)
SOURCES = {
    "supervisor": os.path.join(SCRIPTS, "bounded-test.py"),
    "suite": os.path.join(SCRIPTS, "test_supervisor.py"),
    "bootstrap": os.path.abspath(__file__),
}
LIMITATIONS = [
    "Cooperative session scope only: a process that calls setsid leaves the session and is seen only if it still holds an output pipe; arbitrary same-user writers are outside the promise.",
    "Host free space is a sampled abort threshold (every 0.25 s, 22 GiB = 20 GiB reserve + 2 GiB margin), not an instantaneous guarantee; overshoot between samples is possible.",
    "Scratch bytes are walked every 2 s against a monitored budget, not a hard ceiling; only the supervisor's own scratch is counted.",
    "A group other than the leader's is re-verified through a live member just before each signal; a PID recycled in the gap between that check and killpg is a residual race.",
    "macOS only (kqueue NOTE_EXIT); Linux is not qualified.",
    "Effigy admission waits happen before this script starts and are not inside its 300 s.",
    "Synthetic fixtures model Cargo's setpgid groups; no real Cargo build or live application ran.",
]


def elapsed():
    return time.monotonic() - STARTED


def free_bytes(path):
    sample = os.statvfs(path)
    return sample.f_bavail * sample.f_frsize


def sha256(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def git(*args):
    return subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True, timeout=20, check=True).stdout


def check_receipt(receipt, cases):
    """Reasons the candidate's own receipt does not qualify, or []."""
    problems = []
    closure, reserve, usage = receipt.get("closure") or {}, receipt["reserve"], receipt["scratch_usage"]
    if receipt["exit_code"] != 0 or receipt["primary"]["kind"] != "exit" or receipt["status"] != 0:
        problems.append(f"candidate exit {receipt['exit_code']} ({receipt['primary']})")
    if closure.get("state") != "closed":
        problems.append(f"closure {closure.get('state')}: {closure.get('errors')}")
    if not (receipt["scratch_owned"] and receipt["scratch_removed"]) or receipt["cleanup_errors"]:
        problems.append(f"owned scratch not removed cleanly: {receipt['cleanup_errors']}")
    if receipt.get("late_fault"):
        problems.append(f"fault during closure: {receipt['late_fault']}")
    if not reserve["samples"] or reserve["first_at_s"] > receipt["spawned_at_s"]:
        problems.append("host free-space sampling did not start before the command")
    if reserve["max_gap_s"] > 1.0 or reserve["last_at_s"] + 1.0 < receipt["wall_s"]:
        problems.append("host free-space sampling did not cover the session through cleanup")
    if usage["maximum"] is None or usage["maximum"] < 5 * MIB or usage["maximum"] > usage["budget_bytes"]:
        problems.append(f"scratch peak {usage['maximum']} does not show the fixture writers within budget")
    failed = [case for case, outcome in cases if outcome != "pass"]
    real = [case for case, _ in cases if ".RealChildCases." in case]
    if failed or not cases or not real:
        problems.append(f"suite cases failed or missing: {failed or 'no cases'}")
    return problems


def run_candidate(base, env):
    command = [
        sys.executable, SOURCES["supervisor"], "--limit", str(CANDIDATE_LIMIT), "--scratch-budget-mib", "512",
        "--receipt", os.path.join(base, "receipt.json"), "--", sys.executable, SOURCES["suite"],
    ]
    candidate = subprocess.Popen(command, start_new_session=True, env=env)
    try:
        return candidate.wait(timeout=WALL - 10 - elapsed()), None
    except subprocess.TimeoutExpired:
        pass
    # Bootstrap stop: the candidate's own group only, then fail.
    for number, grace in ((signal.SIGTERM, 3), (signal.SIGKILL, 3)):
        try:
            os.killpg(candidate.pid, number)
        except ProcessLookupError:
            break
        try:
            return candidate.wait(timeout=grace), "candidate overran the bootstrap wall limit"
        except subprocess.TimeoutExpired:
            continue
    return None, "candidate overran the bootstrap wall limit and did not exit after KILL"


def main():
    print("proof:supervisor:qualify: the supervisor is the candidate under test; this bootstrap"
          " (Effigy heavy admission, reserve readings, fixed 300 s wall limit, tiny fixtures)"
          " does not qualify it", flush=True)
    if sys.platform != "darwin":
        print("refused: qualification is macOS only", file=sys.stderr)
        return 2
    head = git("rev-parse", "HEAD").strip()
    if git("status", "--porcelain").strip():
        print("refused: the checkout is not clean, so HEAD does not name the tested source", file=sys.stderr)
        return 2
    initial = free_bytes("/private/tmp")
    print(f"bootstrap host free {initial / GIB:.1f} GiB (abort threshold 22 GiB)", flush=True)
    if initial < THRESHOLD:
        print("refused: host free space is below 22 GiB", file=sys.stderr)
        return 2
    base = os.path.realpath(tempfile.mkdtemp(prefix="oxide-supervisor-qualify.", dir="/private/tmp"))
    env = {key: value for key, value in os.environ.items() if key != "OXIDE_TASK_SCRATCH_ROOT"}
    env["OXIDE_SUPERVISOR_CASES"] = os.path.join(base, "cases.json")
    code, stop = run_candidate(base, env)
    final = free_bytes("/private/tmp")
    problems = [stop] if stop else []
    receipt = cases = None
    try:
        with open(os.path.join(base, "receipt.json")) as handle:
            receipt = json.load(handle)
        with open(env["OXIDE_SUPERVISOR_CASES"]) as handle:
            cases = json.load(handle)
    except (OSError, ValueError) as error:
        problems.append(f"missing candidate evidence: {error}")
    if receipt is not None and cases is not None:
        problems += check_receipt(receipt, cases)
    if code != 0:
        problems.append(f"candidate process exit {code}")
    if final < THRESHOLD:
        problems.append(f"final host free {final} is below 22 GiB")
    qualification = {
        "selector": "proof:supervisor:qualify",
        "result": "fail" if problems else "pass",
        "problems": problems,
        "head": head,
        "sha256": {name: sha256(path) for name, path in SOURCES.items()},
        "platform": {"system": platform.platform(), "machine": platform.machine(), "python": platform.python_version()},
        "bootstrap_wall_s": round(elapsed(), 3),
        "host_free_bytes": {"initial": initial, "final": final},
        "candidate": receipt and {
            key: receipt.get(key)
            for key in ("exit_code", "primary", "closure", "wall_s", "limit_s", "cleanup_reserved_s",
                        "spawned_at_s", "closed_at_s", "reserve", "scratch_usage", "scratch_removed", "output")
        },
        "cases": cases,
        "limitations": LIMITATIONS,
    }
    print("qualification receipt:", json.dumps(qualification, indent=2), flush=True)
    if problems:
        print(f"FAIL; evidence kept in {base}", file=sys.stderr)
        return 1
    shutil.rmtree(base)
    return 0


if __name__ == "__main__":
    sys.exit(main())
