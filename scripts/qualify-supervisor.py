#!/usr/bin/env python3
"""Bootstrap for `effigy proof:supervisor:qualify` (oxide#109).

The supervisor (`scripts/bounded-test.py`) is the candidate under test, not
an already-qualified guard, and this bootstrap does not supply the
qualification it evaluates. Its own bounds: Effigy heavy admission (before
this script starts), a clean checkout, a reserve reading before and after,
one fixed 300 s deadline on CLOCK_UPTIME_RAW, and fixtures with finite
self-exit fallbacks.

The deadline is enforced by `bounded`, a minimal outer actor: the process
forks the whole bootstrap as a worker child and holds it unreaped, making
only WNOHANG reaps, bounded sleeps and a KILL of that pinned child. All
filesystem, hashing and stdout work happens in the worker, so a blocked
stdout reader or slow read cannot extend the 300 s: at the deadline the
worker is KILLed and the exit is 124 with whatever output was written.

The worker runs the candidate with a 280 s limit and a 512 MiB monitored
budget around `scripts/test_supervisor.py`, with the candidate's owned root
inside this script's scratch. Nested case directories live in the
candidate's evidence directory (OXIDE_PROOF_EVIDENCE), which the candidate's
cleanup never removes. Pass requires: candidate exit 0, receipt phase
`monitor_complete`, closure established with command status 0 and no fault,
reserve sampling from before the gate with no gap over 1 s, the fixture
writers counted, custody at every signal, every expected case passing, and
every nested case directory still present after the candidate's cleanup.
On overrun it TERMs, then KILLs, only its own unreaped candidate child.
Nothing is ever deleted: nested UNCLOSED cases leave live fixture processes
whose only bound is their fallback, which is not closure, so the whole
scratch base is kept and its path printed for the owner.
"""
from __future__ import annotations

import glob
import hashlib
import json
import os
import platform
import signal
import subprocess
import sys
import tempfile
import time
import traceback

NS = 1_000_000_000
WALL_NS = 300 * NS
CANDIDATE_LIMIT = 280
THRESHOLD = 22 * 1024 ** 3
GIB, MIB = 1024 ** 3, 1024 ** 2
EXPECTED_CASES = 120
ACTOR_MARGIN_NS = 2 * NS  # the actor's own KILL, reap and exit
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
    "Nested candidate sessions are outside the outer session; their UNCLOSED cases are settled by no guardian. Their resources are kept in the outer evidence directory and the scratch base is never deleted; their fixture processes end only at the finite fallback, which is not closure.",
    "The outer actor KILLs only its pinned worker at the deadline; a candidate still running then is bounded by its own 280 s limit, not by the actor.",
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


def hold(pid, until_ns, forward):
    """Wait for the pinned child `pid` until `until_ns`; return its status or
    None. Signals recorded in `forward` by handlers are sent here, before each
    reap attempt, so none can reach the PID after this loop reaped it."""
    while True:
        while forward:
            try:
                os.kill(pid, forward.pop(0))
            except OSError:
                pass  # an unreaped zombie: nothing to deliver
        done, status = os.waitpid(pid, os.WNOHANG)
        if done == pid:
            return status
        if now_ns() >= until_ns:
            return None
        time.sleep(max(0, min(until_ns - now_ns(), 50_000_000)) / NS)


def code_of(status):
    return os.WEXITSTATUS(status) if os.WIFEXITED(status) else 128 + os.WTERMSIG(status)


def bounded(work, deadline_ns, finish_ns):
    """Minimal outer deadline actor. Runs `work` in a forked child held
    unreaped (so its PID cannot be reused) and makes only WNOHANG reaps,
    bounded sleeps and signals to that pinned child: no file or blocking
    stdio. INT and TERM are forwarded to the worker. At `deadline_ns` the
    worker is KILLed, reaped with WNOHANG until `finish_ns`, and the exit is
    124. Otherwise the worker's own exit code is returned."""
    signal.signal(signal.SIGCHLD, signal.SIG_DFL)
    pid = os.fork()
    if pid == 0:
        code = 1
        try:
            code = work()
        except BaseException:
            traceback.print_exc()
        finally:
            for stream in (sys.stdout, sys.stderr):
                try:
                    stream.flush()
                except (OSError, ValueError):
                    pass
            os._exit(code)
    forward = []
    for number in (signal.SIGINT, signal.SIGTERM):
        signal.signal(number, lambda n, _f: forward.append(n))
    status = hold(pid, deadline_ns, forward)
    if status is not None:
        return code_of(status)
    forward[:] = [signal.SIGKILL]  # still unreaped: the pin holds
    reaped = hold(pid, finish_ns, forward) is not None
    try:  # best effort; never blocks
        os.set_blocking(2, False)
        os.write(2, f"bootstrap: worker overran the fixed deadline; KILLed (reaped {reaped}); exit 124,"
                    " nothing deleted\n".encode())
    except OSError:
        pass
    return 124


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
    for case in results.get("cases", []):  # nested evidence survives the candidate's cleanup
        if "dir" in case and not os.path.exists(os.path.join(case["dir"], "evidence", "receipt.json")):
            problems.append(f"nested case evidence missing after cleanup: {case['case']} {case['dir']}")
    return problems


def main():
    return bounded(qualify, DEADLINE - ACTOR_MARGIN_NS, DEADLINE - NS // 2)


def qualify():
    forward = []  # INT and TERM go to the candidate, which cancels itself
    for number in (signal.SIGINT, signal.SIGTERM):
        signal.signal(number, lambda n, _f: forward.append(n))
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
    print(f"scratch base {base} (kept: never deleted by this bootstrap)", flush=True)
    env = {k: v for k, v in os.environ.items() if k not in ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_SUPERVISOR_INJECT")}
    env["OXIDE_SUPERVISOR_CASES"] = os.path.join(base, "cases.json")
    candidate = subprocess.Popen(
        [sys.executable, SOURCES["supervisor"], "--limit", str(CANDIDATE_LIMIT), "--scratch-budget-mib", "512",
         "--root-parent", base, "--", sys.executable, SOURCES["suite"]], env=env)
    problems = []
    # Reaped only by `hold`, never by Popen, so the candidate stays pinned.
    status = hold(candidate.pid, DEADLINE - 15 * NS, forward)
    if status is None:
        problems.append("candidate overran the bootstrap deadline")
        for number in (signal.SIGTERM, signal.SIGKILL):  # our own unreaped child only
            forward.append(number)
            status = hold(candidate.pid, now_ns() + 3 * NS, forward)
            if status is not None:
                break
    code = None if status is None else code_of(status)
    candidate.returncode = code
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
        "kept": {"base": base, "nested_unclosed": [case["dir"] for case in results.get("cases", [])
                                                   if case.get("retained_unclosed")]},
        "outcomes": results.get("outcomes"),
        "limitations": LIMITATIONS,
    }
    qualification["worker_wall_ns_before_print"] = now_ns() - START
    print("qualification receipt:", json.dumps(qualification, indent=1), flush=True)
    print(f"{'FAIL' if problems else 'pass'}; evidence kept in {base}", file=sys.stderr, flush=True)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
