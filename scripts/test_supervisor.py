#!/usr/bin/env python3
"""Synthetic qualification suite for the shared proof supervisor (oxide#109).

Runs inside the candidate supervisor's own session under
`proof:supervisor:qualify`. Pure cases (run first) drive the guard,
precedence, census, control-protocol and owned-byte traversal logic with fake
inputs or with owned fixtures: nothing is spawned. Real cases run
`bounded-test.py` as a separate process around tiny Python fixtures, with
test-only injected faults (OXIDE_SUPERVISOR_INJECT) and injected readings; the
disk is never filled.

Every fixture process first resets and unblocks SIGALRM and arms an interval
timer to FIXTURE_DEADLINE_NS, an absolute CLOCK_UPTIME_RAW deadline in the
environment, so a late descendant exits at once. Ordering: case verdict
(45 s; the detach class uses its own 75 s outer window, because its nested guard
has a 40 s stop and a 58 s exit) < fixture fallback (60 s) < qualification
(600 s). A case asserts its
verdict before the fallback; fallback expiry never counts as closure.

Real case directories go in the outer supervisor's evidence directory
(OXIDE_PROOF_EVIDENCE), outside its cleanup, and are never deleted: an
UNCLOSED case's live fixtures and receipts are kept for the owner.
"""
from __future__ import annotations

import ast
import contextlib
import errno
import gc
import importlib.util
import inspect
import io
import json
import os
import plistlib
import re
import secrets
import shutil
import signal
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import threading
import time
import textwrap
import types
import unittest
from contextlib import contextmanager
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
SUPERVISOR = SCRIPTS / "bounded-test.py"
BOOTSTRAP = SCRIPTS / "qualify-supervisor.py"
SCRATCH = Path(tempfile.mkdtemp(prefix="suite.", dir=os.environ.get("OXIDE_TASK_SCRATCH_ROOT") or None)).resolve()
OUTER_EVIDENCE = os.environ.get("OXIDE_PROOF_EVIDENCE")
NS = 1_000_000_000
GIB = 1024 ** 3
VERDICT_S, FALLBACK_S = 45, 60
CASES = []

spec = importlib.util.spec_from_file_location("bounded_test", SUPERVISOR)
bounded = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bounded)


def now_ns():
    return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)


# Every fixture starts with this prologue (secondary protection only).
PROLOGUE = """
import os, signal, time
signal.signal(signal.SIGALRM, signal.SIG_DFL)
signal.pthread_sigmask(signal.SIG_UNBLOCK, {signal.SIGALRM})
_left = (int(os.environ["FIXTURE_DEADLINE_NS"]) - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9
if _left <= 0:
    os.kill(os.getpid(), signal.SIGALRM)
signal.setitimer(signal.ITIMER_REAL, _left)
"""


def fixture(body):
    return PROLOGUE + body


SLEEPER = fixture("time.sleep(3600)\n")
TERM_IGNORING = fixture("signal.signal(signal.SIGTERM, signal.SIG_IGN)\ntime.sleep(3600)\n")


def spawn_line(code, setsid=False):
    pre = "os.setsid()" if setsid else "os.setpgid(0, 0)"
    return (f"import subprocess, sys\nchild = subprocess.Popen([sys.executable, '-c', {code!r}],"
            f" preexec_fn=lambda: {pre})\nopen(os.environ['CASE_DIR'] + '/child.pid', 'w').write(str(child.pid))\n")


# Closure-oracle helpers (test-only) ----------------------------------------------
#
# The supervisor keeps two independent closure vetoes: a certain empty census
# after the leader was reaped, then an output-EOF witness for both streams. The
# oracles below judge observed facts instead of one fault string, so a case can
# prove its own property (an escaped live writer, or a held pipe) without
# claiming which veto the guard reached first. They never decide closure.

CENSUS_FAULT = "closure not established: no certain empty census after the leader was reaped"
EOF_FAULT = "closure not established: output EOF witness missing"
SETTLE_REFUSALS = (CENSUS_FAULT, EOF_FAULT)
FD_SOCKET_NAME = "fdsock"
FD_ACK_TIMEOUT_S = 10
NONCE_TIMEOUT_S = 10

# A Cargo-shaped group inside the leader's own cooperative session (oxide#195).
# Oxide starts Cargo with `process_group(0)`: Cargo leads a new process group in
# the leader's session, so the pinned leader-group KILL never reaches it. The
# cargo fixture below leads such a group and starts a "test binary" in that same
# group; both ignore SIGTERM, so only the supervisor's proved per-identity
# session stop can close them. The leader keeps the session alive to its
# deadline, which is the oxide#182 shape.
CARGO_TEST_BINARY = fixture("signal.signal(signal.SIGTERM, signal.SIG_IGN)\ntime.sleep(3600)\n")
CARGO_SHAPED = fixture(
    "import os, subprocess, sys, time\n"
    "signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
    f"test = subprocess.Popen([sys.executable, '-c', {CARGO_TEST_BINARY!r}])\n"
    "open(os.environ['CASE_DIR'] + '/test.pid', 'w').write(str(test.pid))\n"
    "open(os.environ['CASE_DIR'] + '/cargo.ready', 'w').write(str(os.getpid()))\n"
    "time.sleep(3600)\n")
CARGO_LEADER = fixture(
    "import os, subprocess, sys, time\n"
    f"cargo = subprocess.Popen([sys.executable, '-c', {CARGO_SHAPED!r}],"
    " preexec_fn=lambda: os.setpgid(0, 0))\n"
    "open(os.environ['CASE_DIR'] + '/cargo.pid', 'w').write(str(cargo.pid))\n"
    "deadline = time.monotonic() + 20\n"
    "while not os.path.exists(os.environ['CASE_DIR'] + '/cargo.ready') and time.monotonic() < deadline:\n"
    "    time.sleep(0.02)\n"
    "time.sleep(3600)\n")

# A transient same-session member: it leads its own group in the leader's session
# (the Cargo shape), outlives the leader's deadline KILL, and then exits on its own
# a few seconds later. The census must record its exact row and the run must close
# truthfully, never claiming UNCLOSED for a survivor that was gone in time.
TRANSIENT_MEMBER = fixture(
    "signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
    "parent = os.getppid()\n"
    "until = time.monotonic() + 25\n"
    "while os.getppid() == parent and time.monotonic() < until:\n"
    "    time.sleep(0.02)\n"
    "time.sleep(3.0)\n"
    "raise SystemExit(0)\n")
TRANSIENT_LEADER = fixture(
    "import subprocess, sys\n"
    f"member = subprocess.Popen([sys.executable, '-c', {TRANSIENT_MEMBER!r}],"
    " preexec_fn=lambda: os.setpgid(0, 0))\n"
    "open(os.environ['CASE_DIR'] + '/member.pid', 'w').write(str(member.pid))\n"
    "time.sleep(3600)\n")

# An escaped writer: it leaves the nested session with setsid, keeps the nested
# leader's stdout/stderr write descriptors open (the pipe therefore never
# reaches EOF) and answers one fresh nonce after the verdict, which shows it was
# alive then rather than inferred from an old PID. Its own finite
# fixture-deadline alarm still bounds it.
ESCAPED_WRITER = fixture("""
import os, time
case = os.environ["CASE_DIR"]
with open(os.path.join(case, "escaped.ready.tmp"), "w") as handle:
    handle.write(str(os.getpid()))
os.replace(os.path.join(case, "escaped.ready.tmp"), os.path.join(case, "escaped.ready"))
while True:
    try:
        with open(os.path.join(case, "nonce")) as handle:
            token = handle.read().strip()
    except OSError:
        token = ""
    if token:
        with open(os.path.join(case, "nonce.reply.tmp"), "w") as handle:
            handle.write(token)
        os.replace(os.path.join(case, "nonce.reply.tmp"), os.path.join(case, "nonce.reply"))
        break
    time.sleep(0.02)
""")

# Sends duplicates of its own actual stdout/stderr write descriptors to the
# already-running suite process and exits only after the receiver's ACK.
FD_HANDOFF = fixture("""
import os, socket
cwd = os.getcwd()
socket_dir, name = os.environ["FD_SOCKET_DIR"], os.environ["FD_SOCKET_NAME"]
streams = [item for item in os.environ["FD_STREAMS"].split(",") if item]
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.settimeout(float(os.environ["FD_ACK_TIMEOUT_S"]))
try:
    os.chdir(socket_dir)
    try:
        sock.connect(name)
    finally:
        os.chdir(cwd)
    socket.send_fds(sock, [b"d"], [1 if item == "stdout" else 2 for item in streams])
    ack = sock.recv(16)
    if ack != b"ack":
        raise SystemExit(3)
    with open(os.environ["CASE_DIR"] + "/handoff.done", "w") as handle:
        handle.write(",".join(streams))
finally:
    sock.close()
""")


class FdReceiver:
    """One short AF_UNIX listener inside the case's owned directory. It runs in
    the suite process, which is never a member of the nested session, so holding
    a received writer descriptor cannot disturb that session's census. The
    socket is bound by a relative name after chdir (macOS sun_path is short) and
    the caller's working directory is restored either way."""

    def __init__(self, directory):
        self.directory = os.path.realpath(directory)
        self.received = []
        cwd = os.getcwd()
        try:
            os.chdir(self.directory)
            self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            self.listener.bind(FD_SOCKET_NAME)
            self.listener.listen(1)
        finally:
            os.chdir(cwd)

    @property
    def path(self):
        return os.path.join(self.directory, FD_SOCKET_NAME)

    def accept(self, count, timeout):
        # Receive exactly `count` descriptors, refuse a truncated or short
        # control message, take custody (close-on-exec) and only then ACK.
        self.listener.settimeout(timeout)
        connection, _ = self.listener.accept()
        fds = []
        try:
            connection.settimeout(timeout)
            message, fds, flags, _address = socket.recv_fds(connection, 1, count + 2)
            if flags & socket.MSG_CTRUNC:
                raise AssertionError("descriptor control message was truncated")
            if not message or len(fds) != count:
                raise AssertionError(f"descriptor handoff delivered {len(fds)} of {count} descriptors")
            for fd in fds:
                os.set_inheritable(fd, False)
            self.received.extend(fds)
            connection.sendall(b"ack")
        except BaseException:
            for fd in fds:
                try:
                    os.close(fd)
                except OSError:
                    pass
            raise
        finally:
            connection.close()
        return list(self.received)

    def close(self):
        for fd in self.received:
            try:
                os.close(fd)
            except OSError:
                pass
        self.received = []
        try:
            self.listener.close()
        except OSError:
            pass


def escaped_writer_problems(facts):
    # The retained native escaped-writer proof, judged from observed facts.
    problems = []
    if facts.get("leader_status") != 0:
        problems.append(f"the nested leader did not exit 0: {facts.get('leader_status')}")
    if facts.get("exit") != 125:
        problems.append(f"the nested guardian exit {facts.get('exit')} is not 125")
    if facts.get("closure") != "UNCLOSED":
        problems.append(f"closure {facts.get('closure')} is not UNCLOSED")
    if facts.get("fault") not in SETTLE_REFUSALS:
        problems.append(f"the fault {facts.get('fault')!r} is not one of the two settle refusals")
    if facts.get("deadline"):
        problems.append("the run hit its deadline")
    if not facts.get("ready"):
        problems.append("the escaped writer never signalled readiness before the verdict")
    if not facts.get("nonce_ok"):
        problems.append("the escaped writer did not answer the fresh post-verdict nonce")
    child, suite = facts.get("child_pid"), facts.get("suite_pid")
    if child is None or suite is None:
        problems.append("the escaped child or suite identity was not observed")
    for target in facts.get("signal_targets") or []:
        if target in (child, suite):
            problems.append(f"a signal targeted the escaped writer or the suite: {target}")
    for target in facts.get("session_targets") or []:
        # oxide#195 (ruling c096c8fc): no non-held session identity is ever
        # signalled, so no such record may exist at all.
        problems.append(f"a non-held session signal record exists: {target}")
    for pin in facts.get("signal_pins") or []:
        if pin not in ("leader", "monitor"):
            problems.append(f"a signal targeted an unexpected pin: {pin}")
    if not facts.get("retained"):
        problems.append("the case resources were not retained")
    return problems


def eof_veto_problems(facts):
    # A held actual writer descriptor must produce exactly the EOF veto after a
    # real certain empty census, with the leader's own status kept.
    problems = []
    if facts.get("leader_status") != 0:
        problems.append(f"the nested leader did not exit 0: {facts.get('leader_status')}")
    if facts.get("exit") != 125:
        problems.append(f"the nested guardian exit {facts.get('exit')} is not 125")
    if facts.get("closure") != "UNCLOSED":
        problems.append(f"closure {facts.get('closure')} is not UNCLOSED")
    if facts.get("deadline"):
        problems.append("the run hit its deadline")
    if facts.get("fault") != EOF_FAULT:
        problems.append(f"the EOF veto did not latch: {facts.get('fault')!r}")
    census = facts.get("census") or {}
    if not (census.get("certain") is True and census.get("members") == 0 and census.get("uncertain") == 0):
        problems.append(f"the census was not certainly empty, so this is not an EOF-only proof: {census}")
    if facts.get("held") != facts.get("expected_held"):
        problems.append(f"held streams {facts.get('held')} are not the expected {facts.get('expected_held')}")
    for target in facts.get("signal_targets") or []:
        if target == facts.get("suite_pid"):
            problems.append("a signal targeted the suite")
    if not facts.get("retained"):
        problems.append("the case resources were not retained")
    return problems


def released_streams_problems(facts):
    # The paired control: releasing both descriptors must close normally.
    problems = []
    if facts.get("exit") != 0:
        problems.append(f"the paired control exit {facts.get('exit')} is not 0")
    if facts.get("closure") != "closed":
        problems.append(f"the paired control closure {facts.get('closure')} is not closed")
    if facts.get("fault") is not None:
        problems.append(f"the paired control latched a fault: {facts.get('fault')!r}")
    if facts.get("held"):
        problems.append(f"the paired control still held descriptors: {facts.get('held')}")
    return problems


def wait_for(path, timeout):
    # Bounded wait for one owned fixture artifact; never an OS-liveness probe.
    deadline = time.monotonic() + timeout
    while not os.path.exists(path):
        if time.monotonic() >= deadline:
            raise AssertionError(f"{path} did not appear within {timeout} s")
        time.sleep(0.02)


class _ReleasedReceiver:
    # Takes custody of the descriptors through the real receiver and releases
    # them at once, so the paired control can prove normal closure.
    def __init__(self, receiver):
        self.receiver = receiver
        self.released = False
        self.held_names = []

    def accept(self, count, timeout):
        fds = self.receiver.accept(count, timeout)
        for fd in fds:
            os.close(fd)
        self.receiver.received = []
        self.released = True
        return []

    def still_held(self):
        return [] if self.released else ["pending"]


# Pure cases --------------------------------------------------------------------


class PureGuard(unittest.TestCase):
    def guard(self):
        # Steady-state cases: sampling began with the run, so the freshness window
        # is measured from the first reading exactly as before this correction.
        guard = bounded.Guard(start_ns=1000 * NS)
        guard.sampling_ns = 1000 * NS
        return guard

    def test_low_reading_latches_and_never_clears(self):
        guard = self.guard()
        guard.reading({"kind": "reserve", "value": 22 * GIB - 1, "at": 1000 * NS, "completed": 1000 * NS})
        guard.reading({"kind": "reserve", "value": 500 * GIB, "at": 1000 * NS + 1, "completed": 1000 * NS + 1})
        self.assertIn("below", guard.check(1000 * NS + 2))

    def test_exact_threshold_passes(self):
        guard = self.guard()
        guard.reading({"kind": "reserve", "value": 22 * GIB, "at": 1000 * NS, "completed": 1000 * NS})
        guard.reading({"kind": "scratch", "value": 0, "complete": True, "at": 1000 * NS, "completed": 1000 * NS})
        self.assertIsNone(guard.check(1000 * NS + NS // 2))

    def test_measurement_error_faults(self):
        guard = self.guard()
        guard.reading({"kind": "reserve", "error": "injected statvfs failure", "at": 1000 * NS})
        self.assertIn("measurement failed", guard.fault)

    def test_stale_reserve_and_scratch(self):
        guard = self.guard()
        guard.reading({"kind": "reserve", "value": 500 * GIB, "at": 1000 * NS, "completed": 1000 * NS})
        guard.reading({"kind": "scratch", "value": 0, "complete": True, "at": 1000 * NS, "completed": 1000 * NS})
        self.assertIsNone(guard.check(1001 * NS))
        self.assertIn("reserve readings stale", guard.check(1001 * NS + 1))
        guard = self.guard()
        for at in range(1000, 1012):
            guard.reading({"kind": "reserve", "value": 500 * GIB, "at": at * NS, "completed": at * NS})
        guard.reading({"kind": "scratch", "value": 0, "complete": True, "at": 1000 * NS, "completed": 1000 * NS})
        self.assertIn("scratch readings stale", guard.check(1010 * NS + 1))

    def test_incomplete_scratch_reading_does_not_refresh(self):
        guard = self.guard()
        guard.reading({"kind": "scratch", "value": 0, "complete": False, "at": 1009 * NS, "completed": 1009 * NS})
        guard.reading({"kind": "reserve", "value": 500 * GIB, "at": 1010 * NS, "completed": 1010 * NS})
        self.assertIn("scratch readings stale", guard.check(1010 * NS + 1))

    def test_future_stamped_reading_latches(self):
        # A reading stamped after the custodian's own clock reading is never
        # fresh, so it cannot keep a stopped sampler looking alive.
        guard = self.guard()
        guard.reading({"kind": "reserve", "value": 500 * GIB, "at": 2000 * NS, "completed": 2000 * NS}, now=1000 * NS)
        self.assertIn("future", guard.fault)
        self.assertIsNone(guard.reserve_at)

    # Review 6320463419: freshness counts from completion, not publication.

    def test_delayed_reserve_publication_does_not_refresh(self):
        guard = self.guard()
        guard.reading({"kind": "reserve", "value": 500 * GIB, "completed": 1000 * NS, "at": 1000 * NS + NS // 2},
                      now=1000 * NS + NS // 2)
        guard.reading({"kind": "scratch", "value": 0, "complete": True, "completed": 1000 * NS, "at": 1000 * NS})
        self.assertEqual(guard.reserve_at, 1000 * NS)
        self.assertIsNone(guard.check(1001 * NS))
        self.assertIn("reserve readings stale", guard.check(1001 * NS + 1))  # not 1001.5 s

    def test_delayed_scratch_publication_does_not_refresh(self):
        guard = self.guard()
        guard.reading({"kind": "scratch", "value": 0, "complete": True, "completed": 1000 * NS, "at": 1004 * NS},
                      now=1004 * NS)
        for at in range(1000, 1012):
            guard.reading({"kind": "reserve", "value": 500 * GIB, "completed": at * NS, "at": at * NS})
        self.assertEqual(guard.scratch_at, 1000 * NS)
        self.assertIsNone(guard.check(1010 * NS))
        self.assertIn("scratch readings stale", guard.check(1010 * NS + 1))  # not 1014 s

    def test_completion_after_send_latches(self):
        guard = self.guard()
        guard.reading({"kind": "reserve", "value": 500 * GIB, "completed": 1001 * NS, "at": 1000 * NS},
                      now=1002 * NS)
        self.assertIn("completed after it was sent", guard.fault)
        self.assertIsNone(guard.reserve_at)

    def test_over_budget(self):
        guard = self.guard()
        guard.reading({"kind": "scratch", "value": 513 * 1024 * 1024, "complete": True, "at": 1000 * NS, "completed": 1000 * NS})
        self.assertIn("over the", guard.fault)


class PureFreshnessClock(unittest.TestCase):
    """Decision 50acd33b: freshness measures the steady-state sampling window
    from when sampling actually began; setup is bounded by its own deadline."""

    START = 1000 * NS

    def test_a_bounded_setup_longer_than_one_second_does_not_latch(self):
        # (a) A cold setup (the bounded startup sweep can take >1 s) inside its own
        # deadline launches and closes: no reading exists yet, and nothing is stale.
        guard = bounded.Guard(self.START)
        self.assertIsNone(guard.check(self.START + 2 * bounded.NS),
                          "a bounded setup must not be a freshness fault")
        # Sampling begins (the monitor's ready) with its first completed readings.
        guard.sampling_ns = self.START + 2 * bounded.NS
        guard.reading({"kind": "reserve", "value": 500 * bounded.GIB,
                       "at": self.START + 2 * bounded.NS, "completed": self.START + 2 * bounded.NS})
        guard.reading({"kind": "scratch", "value": 0, "complete": True,
                       "at": self.START + 2 * bounded.NS, "completed": self.START + 2 * bounded.NS})
        self.assertIsNone(guard.check(self.START + 2 * bounded.NS + bounded.NS // 2))
        self.assertIsNone(guard.fault)

    def test_the_custodian_start_counterfactual_would_have_refused_that_setup(self):
        # (b) The old rule measured an absent reading from custodian start_ns; it
        # would call this healthy 2 s setup stale, so the two rules are
        # discriminating rather than both permissive.
        def old_rule(guard, now):
            for at, stale in ((guard.reserve_at, bounded.RESERVE_STALE_NS),
                              (guard.scratch_at, bounded.SCRATCH_STALE_NS)):
                if now - (guard.start_ns if at is None else at) > stale:
                    return True
            return False

        guard = bounded.Guard(self.START)
        now = self.START + 2 * bounded.NS
        self.assertTrue(old_rule(guard, now), "the superseded start-based rule must fail this case")
        self.assertIsNone(guard.check(now), "the corrected sampling-based rule must not")

    def test_no_sample_within_one_second_after_sampling_began_still_latches(self):
        # (c) Once sampling began, an absent reserve reading is stale after 1 s.
        guard = bounded.Guard(self.START)
        guard.sampling_ns = self.START + 2 * bounded.NS
        self.assertIsNone(guard.check(self.START + 2 * bounded.NS + bounded.NS))
        self.assertIn("reserve readings stale",
                      guard.check(self.START + 2 * bounded.NS + bounded.NS + 1))
        # The owned-byte window is unchanged at 10 s; keep the reserve reading
        # fresh so only the scratch rule is under test.
        guard = bounded.Guard(self.START)
        guard.sampling_ns = self.START
        guard.reading({"kind": "reserve", "value": 500 * bounded.GIB,
                       "at": self.START + 10_900_000_000, "completed": self.START + 10_900_000_000})
        self.assertIsNone(guard.check(self.START + 10 * bounded.NS))
        self.assertIn("scratch readings stale", guard.check(self.START + 11 * bounded.NS))

    def test_a_setup_stall_beyond_its_deadline_still_launches_nothing(self):
        # (d) The setup deadline is unchanged and independent of freshness: past it
        # the launch is abandoned with one bounded reason, whatever the readings.
        custodian = bounded.Custodian(["true"], limit_s=300)
        custodian.start = self.START
        custodian.absolute = self.START + 300 * bounded.NS
        custodian.stop_at = custodian.absolute - bounded.STOP_RESERVE_S * bounded.NS
        custodian.exit_at = custodian.absolute - bounded.EXIT_MARGIN_S * bounded.NS
        custodian.setup_at = min(self.START + bounded.SETUP_S * bounded.NS, custodian.stop_at)
        self.assertFalse(custodian.setup_expired(self.START + bounded.SETUP_S * bounded.NS - 1))
        self.assertTrue(custodian.setup_expired(self.START + bounded.SETUP_S * bounded.NS))
        # Freshness state cannot move the setup deadline.
        before = custodian.setup_at
        custodian.guard = bounded.Guard(self.START)
        custodian.guard.sampling_ns = self.START
        self.assertEqual(custodian.setup_at, before)
        custodian.monitor = 1234
        custodian.not_launched = "monitor setup failed or slow"
        custodian.signal_pinned = lambda pid, number, group: None
        custodian.reap = lambda pid: 0
        custodian.pump = lambda timeout: None
        captured = []
        with patched(bounded, "report", lambda text, fd=2: captured.append(text)):
            with patched(bounded, "now_ns", lambda: self.START + 11 * bounded.NS):
                self.assertEqual(custodian.abandon_launch(), 125)
        self.assertEqual(len(captured), 1)
        self.assertIn("not launched", captured[0])

    def test_ready_starts_the_freshness_clock_at_the_custodian(self):
        # Wiring: only a valid `ready` starts sampling, and a refused one does not.
        custodian = bounded.Custodian(["true"], limit_s=300)
        custodian.start = self.START
        custodian.guard = bounded.Guard(self.START)
        read_end, custodian.cmd_w = os.pipe()
        self.addCleanup(os.close, read_end)
        self.addCleanup(os.close, custodian.cmd_w)
        custodian.run_id = "r"
        custodian.leader = 4242
        ready = {"run": "r", "seq": custodian.monitor_seq + 1, "leader": None, "type": "ready",
                 "at": self.START + 2 * bounded.NS}
        custodian.accept(ready)
        self.assertIn("ready before setup", custodian.guard.fault or "")
        self.assertIsNone(custodian.guard.sampling_ns, "a refused ready must not start the clock")
        custodian.guard.fault = None
        custodian.paths = {"owned": False}
        custodian.accept({"run": "r", "seq": custodian.monitor_seq + 1, "leader": None, "type": "ready",
                          "at": self.START + 2 * bounded.NS})
        self.assertIn("ready before completed readings", custodian.guard.fault or "")
        self.assertIsNone(custodian.guard.sampling_ns)
        custodian.guard.fault = None
        custodian.guard.reading({"kind": "reserve", "value": 500 * bounded.GIB,
                                 "at": self.START + 2 * bounded.NS, "completed": self.START + 2 * bounded.NS})
        custodian.guard.reading({"kind": "scratch", "value": 0, "complete": True,
                                 "at": self.START + 2 * bounded.NS, "completed": self.START + 2 * bounded.NS})
        with patched(bounded, "now_ns", lambda: self.START + 2 * bounded.NS):
            custodian.accept({"run": "r", "seq": custodian.monitor_seq + 1, "leader": None,
                              "type": "ready", "at": self.START + 2 * bounded.NS})
        self.assertTrue(custodian.ready)
        self.assertEqual(custodian.guard.sampling_ns, self.START + 2 * bounded.NS)


class PurePrecedence(unittest.TestCase):
    def test_order(self):
        code = bounded.exit_code
        self.assertEqual(code(True, "fault", False, 0), 124)
        self.assertEqual(code(False, "fault", True, 0), 125)
        self.assertEqual(code(False, None, False, 0), 125)
        self.assertEqual(code(False, None, True, None), 125)
        self.assertEqual(code(False, None, True, 0), 0)
        self.assertEqual(code(False, None, True, 3 << 8), 3)
        self.assertEqual(code(False, None, True, 127 << 8), 127)
        self.assertEqual(code(False, None, True, 9), 137)


def row(pid, start=1, **extra):
    return dict({"pid": pid, "start": start, "ppid": 1, "pgid": pid, "uid": 501, "zombie": False}, **extra)


def _fake_libc(entries, errors=None):
    """Deterministic fake libc for the fixed sysctl reader: no host calls.

    entries maps sysctl name to (value bytes, reported length); errors maps
    name to an errno for a failing call."""
    class FakeLibc:
        def __init__(self):
            self.values = dict(entries)
            self.errors = dict(errors or {})
            self.call_errno = 0

        def sysctlbyname(self, name, oldp, oldlenp, _newp, _newlen):
            key = bytes(name).decode("ascii")
            if key in self.errors:
                self.call_errno = self.errors[key]
                return -1
            value, reported = self.values[key]
            clipped = value[:len(oldp)]
            oldp[:len(clipped)] = clipped
            oldlenp.contents.value = reported
            return 0

        def get_errno(self):
            return self.call_errno

    return FakeLibc()


class FakeTable:
    def __init__(self, rows, sessions, reread=None, groups=None):
        self.rows, self.sessions, self.rereads = rows, sessions, reread or {}
        self.groups = groups or {}
        self.reads = 0
        self.reads_by_pid = {}
        self.session_reads = {}

    def snapshot(self):
        return list(self.rows)

    def getsid(self, pid):
        value = self.sessions[pid]
        if isinstance(value, list):  # per-PID sequence; the last entry repeats
            index = self.session_reads.get(pid, 0)
            self.session_reads[pid] = index + 1
            value = value[min(index, len(value) - 1)]
        if isinstance(value, BaseException):
            raise value
        return value

    def getpgid(self, pid):
        value = self.groups.get(pid, pid)
        if isinstance(value, BaseException):
            raise value
        return value

    def reread(self, pid):
        self.reads += 1
        value = self.rereads.get(pid, "rows")
        if isinstance(value, list):  # per-PID sequence; the last entry repeats
            index = self.reads_by_pid.get(pid, 0)
            self.reads_by_pid[pid] = index + 1
            value = value[min(index, len(value) - 1)]
        if callable(value):
            value = value(self.reads)
        if isinstance(value, BaseException):
            raise value
        if value != "rows":
            return value
        return next((r for r in self.rows if r["pid"] == pid), None)


def fixture_work():
    return {"snapshot": 0, "replacement": 0, "reads": 1, "pending": 0,
            "status": "complete", "rows": [], "omitted": 0}


def census(table, sid=100, terminal_allowed=False, ancestors=(), prime=False):
    # Legacy confirmation scripts begin immediately after getsid; model the
    # new preceding identity read separately without consuming the script.
    primed = set()
    def snapshot():
        primed.clear()
        return table.snapshot()
    def reread(pid):
        if prime and pid not in primed:
            primed.add(pid)
            return next((r for r in table.rows if r["pid"] == pid), None)
        return table.reread(pid)
    return bounded.Census(sid, snapshot=snapshot, getsid=table.getsid, reread=reread,
                          pause=lambda _seconds: None, terminal_allowed=terminal_allowed,
                          ancestors=ancestors, getpgid=table.getpgid)


class PureCensus(unittest.TestCase):
    def test_work_ledger_validates_snapshot_before_admission(self):
        work = bounded.CensusWork()
        with self.assertRaises(bounded.CensusWorkStopped):
            work.seed([row(1), row(1, start=2)])
        self.assertEqual((work.status, work.snapshot_count, work.entries), ("invalid", 0, {}))

    def test_work_ledger_reopens_without_losing_omitted_obligations(self):
        work = bounded.CensusWork()
        work.seed([row(1)])
        identity, _ = work.next()
        work.resolve(identity, "foreign")
        for pid in range(2, 24):
            work.replacement(row(pid))
        work.replacement(row(1))
        summary = work.summary()
        self.assertEqual((summary["snapshot"], summary["replacement"], summary["pending"]), (1, 22, 23))
        self.assertEqual((len(summary["rows"]), summary["omitted"]), (20, 3))
        self.assertEqual(work.queue[-1], identity)

    def test_work_ledger_caps_replacements_without_discarding_prior_work(self):
        work = bounded.CensusWork()
        work.seed([row(1)])
        for start in range(128):
            work.replacement(row(2, start=start))
        with self.assertRaises(bounded.CensusWorkStopped):
            work.replacement(row(2, start=128))
        self.assertEqual((work.status, work.replacement_count, len(work.entries)), ("identity_cap", 128, 129))
        self.assertEqual((work.summary()["status"], work.summary()["pending"]), ("identity_cap", 129))

    def test_work_ledger_charges_failures_and_rejects_late_returns(self):
        ticks = [0]
        work = bounded.CensusWork(clock=lambda: ticks[0])
        def fail():
            raise OSError("unreadable")
        with self.assertRaises(OSError):
            work.call(fail)
        self.assertEqual(work.reads, 1)
        def late():
            ticks[0] = 2 * bounded.NS
        with self.assertRaises(bounded.CensusWorkStopped):
            work.call(late)
        self.assertEqual((work.status, work.reads), ("deadline", 2))

    def test_each_replacement_entry_classifies_the_new_identity(self):
        for path in ("first", "shared", "final", "ordinary"):
            with self.subTest(path=path):
                old, new = row(501, start=1), row(501, start=2)
                if path == "first":
                    reads = [old, new, new, new]
                elif path == "shared":
                    reads = [old, old, new, new, new]
                elif path == "final":
                    reads = [old] * 21 + [new, new, new]
                else:
                    reads = [old, new, new, new]
                sessions = [7, 100] if path == "ordinary" else [ProcessLookupError(3, "gone"), 100]
                table = FakeTable([old], {501: sessions}, reread={501: reads})
                check = census(table)
                result = check.round()
                self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 1, 0))
                self.assertIn((501, 2), check.tracked)
                self.assertEqual(result["replacement_work"]["replacement"], 1)

    def test_churn_hits_identity_cap_without_certain_empty(self):
        table = FakeTable([row(502, start=1)], {502: 7},
                          reread={502: lambda reads: row(502, start=reads + 1)})
        result = census(table).round()
        self.assertFalse(result["certain"])
        self.assertGreaterEqual(result["uncertain"], 1)
        self.assertEqual(result["replacement_work"]["status"], "identity_cap")
        self.assertEqual(result["replacement_work"]["replacement"], 128)
        self.assertGreater(result["replacement_work"]["omitted"], 0)
        self.assertEqual(result["terminal_count_status"], "invalid")

    def test_duplicate_snapshot_does_not_discard_tracking(self):
        table = FakeTable([row(503), row(503, start=9)], {503: 7})
        check = census(table)
        check.tracked[(999, 7)] = row(999, start=7)
        result = check.round()
        self.assertEqual(result["replacement_work"]["status"], "invalid")
        self.assertFalse(result["certain"])
        self.assertIn((999, 7), check.tracked)

    def test_reappearing_resolved_identity_is_reopened(self):
        a, b = row(504, start=1), row(504, start=2)
        table = FakeTable([a], {504: [7, 7, 100]}, reread={504: [a, b, b, a, a, a]})
        check = census(table)
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 1, 0))
        self.assertIn((504, 1), check.tracked)
        self.assertEqual(result["replacement_work"]["reads"], 10)
        self.assertEqual(result["replacement_work"]["replacement"], 1)
        self.assertIn({"pid": 504, "start": 1, "state": "member"}, result["replacement_work"]["rows"])

    def test_snapshot_limit_and_malformed_rows_preserve_tracking(self):
        for rows in ([row(i + 1) for i in range(4097)], [row(1), dict(row(2), start=True)]):
            with self.subTest(size=len(rows)):
                table = FakeTable(rows, {})
                check = census(table)
                check.tracked[(9999, 7)] = row(9999, start=7)
                result = check.round()
                self.assertFalse(result["certain"])
                self.assertIn(result["replacement_work"]["status"], ("identity_cap", "invalid"))
                self.assertIn((9999, 7), check.tracked)
                self.assertEqual(result["terminal_count_status"], "invalid")

    def test_read_cap_preserves_unresolved_member(self):
        table = FakeTable([row(505, start=7)] + [row(600 + i) for i in range(24)], {505: 7})
        check = census(table)
        check.tracked[(505, 7)] = row(505, start=7)
        with patched(bounded, "CENSUS_READ_CAP", 2):
            result = check.round()
        self.assertEqual(result["replacement_work"]["status"], "read_cap")
        self.assertFalse(result["certain"])
        self.assertIn((505, 7), check.tracked)
        self.assertEqual(result["replacement_work"]["reads"], 2)
        self.assertEqual(result["replacement_work"]["omitted"], 5)

    def test_late_identity_reader_invalidates_round_and_retains_tracking(self):
        ticks = [0]
        real_work = bounded.CensusWork
        def work():
            return real_work(clock=lambda: ticks[0])
        table = FakeTable([row(506, start=7)] + [row(700 + i) for i in range(24)], {506: 7},
                          reread={506: lambda _n: (ticks.__setitem__(0, 2 * NS), row(506, start=7))[1]})
        check = census(table)
        check.tracked[(506, 7)] = row(506, start=7)
        with patched(bounded, "CensusWork", work):
            result = check.round()
        self.assertFalse(result["certain"])
        self.assertEqual(result["replacement_work"]["status"], "deadline")
        self.assertEqual(result["terminal_count_status"], "invalid")
        self.assertEqual(result["replacement_work"]["omitted"], 5)
        self.assertIn((506, 7), check.tracked)

    def test_late_pause_invalidates_round(self):
        ticks = [0]
        real_work = bounded.CensusWork
        table = FakeTable([row(508, start=7)], {508: ProcessLookupError(3, "gone")})
        check = census(table)
        check.pause = lambda _seconds: ticks.__setitem__(0, 2 * NS)
        with patched(bounded, "CensusWork", lambda: real_work(clock=lambda: ticks[0])):
            result = check.round()
        self.assertFalse(result["certain"])
        self.assertEqual(result["replacement_work"]["status"], "deadline")
        self.assertEqual(result["terminal_count_status"], "invalid")

    def test_parent_revalidation_contradiction_taints_pending_terminal(self):
        zombie, live = row(700, start=11, zombie=True), row(700, start=11)
        sibling = row(701, start=12, ppid=700)
        table = FakeTable([zombie, sibling], {700: ProcessLookupError(3, "gone"), 701: 7},
                          reread={700: [zombie, zombie, live, zombie], 701: sibling})
        check = census(table, terminal_allowed=True, ancestors=[(700, 11)])
        check.tracked[(700, 11)] = zombie
        result = check.round()
        self.assertFalse(result["certain"])
        self.assertEqual(result["terminal_count_status"], "invalid")
        self.assertIsNone(result["terminal_nonwriters"])
        self.assertIn((700, 11), check.tracked)

    def test_terminal_witness_requires_snapshot_and_initial_read(self):
        zombie, live = row(507, start=7, zombie=True), row(507, start=7)
        table = FakeTable([zombie], {507: ProcessLookupError(3, "gone")},
                          reread={507: [live] + [zombie] * 21})
        result = census(table, terminal_allowed=True).round()
        self.assertFalse(result["certain"])
        self.assertEqual(result["terminal_count_status"], "invalid")
        self.assertNotIn("terminal_witnesses", result)

    def test_member_and_foreign(self):
        table = FakeTable([row(101), row(200)], {101: 100, 200: 7})
        result = census(table).round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 1, 0))

    def test_changed_identity_is_classified_independently(self):
        table = FakeTable([row(101, start=1)], {101: 7}, reread={101: row(101, start=2)})
        result = census(table).round()
        self.assertEqual((result["certain"], result["uncertain"]), (True, 0))
        self.assertIn({"pid": 101, "start": 2, "state": "foreign"}, result["replacement_work"]["rows"])

    def test_first_seen_unreadable_is_uncertain_not_foreign(self):
        table = FakeTable([row(300)], {300: PermissionError(1, "denied")})
        self.assertEqual(census(table).round()["uncertain"], 1)

    def test_zombie_is_uncertain_until_gone(self):
        table = FakeTable([row(102, zombie=True)], {102: ProcessLookupError(3, "no such process")})
        check = census(table)
        self.assertEqual(check.round()["uncertain"], 1)
        table.rows = []
        self.assertTrue(check.round()["certain"])

    def test_gone_between_snapshot_and_getsid(self):
        table = FakeTable([row(103)], {103: ProcessLookupError(3, "gone")}, reread={103: None})
        result = census(table).round()
        self.assertEqual((result["certain"], result["members"]), (True, 0))

    def test_gone_needs_successful_confirmation(self):
        # Review 6312725112 finding 4: ESRCH then a failing KERN_PROC_PID
        # re-read is missing metadata, not absence.
        table = FakeTable([row(107)], {107: ProcessLookupError(3, "gone")}, reread={107: PermissionError(1, "denied")})
        result = census(table).round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (False, 0, 1))
        self.assertIn("reread", result["uncertain_rows"][0]["reason"])

    def test_zombie_confirmed_gone_within_round(self):
        # Present at the first re-read, absent at a later successful one.
        table = FakeTable([row(108, zombie=True)], {108: ProcessLookupError(3, "gone")},
                          reread={108: lambda reads: row(108, zombie=True) if reads < 3 else None})
        result = census(table).round()
        self.assertEqual((result["certain"], result["uncertain"]), (True, 0))

    def test_member_stays_member_after_setsid_or_reparenting(self):
        table = FakeTable([row(104, ppid=101)], {104: 100})
        check = census(table)
        self.assertEqual(check.round()["members"], 1)
        table.rows, table.sessions = [row(104, ppid=1, uid=0)], {104: 104}  # escaped and reparented
        self.assertEqual(check.round()["members"], 1)
        table.rows = []
        self.assertEqual(check.round()["members"], 0)

    def test_start_time_never_excludes(self):
        table = FakeTable([row(105, start=-5)], {105: 100})  # clock stepped backwards
        self.assertEqual(census(table).round()["members"], 1)

    def test_recycled_pid_is_a_new_identity(self):
        table = FakeTable([row(106, start=1)], {106: 100})
        check = census(table)
        check.round()
        table.rows, table.sessions = [row(106, start=9)], {106: 7}
        self.assertEqual(check.round()["members"], 0)

    def test_failed_snapshot_is_uncertain(self):
        def broken():
            raise OSError(12, "injected")

        result = bounded.Census(100, snapshot=broken).round()
        self.assertFalse(result["certain"])


class PureAncestors(unittest.TestCase):
    """Oxide#197 (design ruling ea5a3dc9): only structural launcher facts may
    exclude a post-leader census row, and nothing else is relaxed."""

    LAUNCHER = (700, 11)  # a recorded ancestor (this suite, under the outer guard)

    @staticmethod
    def chain(levels):
        """Snapshot rows for a real ppid chain of (pid, start, ppid) levels."""
        return [row(pid, start=start, ppid=ppid) for pid, start, ppid in levels]

    def launcher_rows(self, *extra):
        """A snapshot with the recorded launcher and any additional rows."""
        return [row(self.LAUNCHER[0], start=self.LAUNCHER[1], ppid=600)] + list(extra)

    def test_every_ancestor_to_pid_1_is_recorded(self):
        rows = self.chain([(700, 11, 600), (600, 12, 500), (500, 13, 1), (1, 14, 0)])
        record = bounded.ancestor_identities(snapshot=lambda: rows, parent=lambda: 700)
        self.assertEqual((record["state"], record["stop"]), ("recorded", "pid-1"))
        self.assertEqual(record["count"], 4)
        self.assertEqual([(i["pid"], i["start"], i["level"]) for i in record["identities"]],
                         [(700, 11, 1), (600, 12, 2), (500, 13, 3), (1, 14, 4)])

    def test_an_ancestor_is_reverified_before_it_is_excluded(self):
        # The identity is re-read through the safe reader after the snapshot,
        # so exclusion needs that read to still name the same identity.
        table = FakeTable([row(700, start=11)], {700: ProcessLookupError(errno.ESRCH, "gone")})
        check = census(table, ancestors=[self.LAUNCHER])
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))
        self.assertEqual(table.reads, 1, "the ancestor identity was not re-verified exactly once")
        self.assertNotIn((700, 11), check.tracked)
        self.assertEqual(result["ancestor_exclusions"]["by_rule"]["ancestor"], 1)

    def test_an_ancestor_is_not_a_member_even_in_the_leader_session(self):
        table = FakeTable([row(700, start=11)], {700: 100})
        result = census(table, ancestors=[self.LAUNCHER]).round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))

    def test_an_ancestor_identity_changing_mid_census_is_uncertain(self):
        # The snapshot names the recorded identity, but the re-verification read
        # shows a different process: nothing may be excluded.
        table = FakeTable([row(700, start=11)], {700: PermissionError(1, "denied")},
                          reread={700: row(700, start=12)})
        result = census(table, ancestors=[self.LAUNCHER]).round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (False, 0, 1))
        self.assertEqual(result["ancestor_exclusions"]["count"], 0)
        # The same rule applies to the parent identity of a launcher sibling.
        sibling = row(900, start=12, ppid=700)
        parent_changed = FakeTable(self.launcher_rows(sibling), {700: 7, 900: PermissionError(1, "denied")},
                                   reread={700: row(700, start=12), 900: sibling})
        result = census(parent_changed, ancestors=[self.LAUNCHER]).round()
        # The replacement launcher is bound foreign; the child remains unknown.
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertIn({"pid": 700, "start": 12, "state": "foreign"}, result["replacement_work"]["rows"])
        self.assertEqual(result["ancestor_exclusions"]["by_rule"]["parent"], 0)

    def test_a_recycled_pid_with_a_different_start_is_not_excluded(self):
        table = FakeTable([row(700, start=99)], {700: ProcessLookupError(errno.ESRCH, "gone")},
                          reread={700: row(700, start=99)})
        result = census(table, ancestors=[self.LAUNCHER]).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual(result["uncertain_rows"][0]["pid"], 700)
        member = FakeTable([row(700, start=99)], {700: 100})
        self.assertEqual(census(member, ancestors=[self.LAUNCHER]).round()["members"], 1)

    def test_a_non_ancestor_unreadable_row_stays_unknown(self):
        table = FakeTable([row(700, start=11), row(701, start=5)],
                          {700: ProcessLookupError(errno.ESRCH, "gone"),
                           701: PermissionError(1, "denied")})
        result = census(table, ancestors=[self.LAUNCHER]).round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (False, 0, 1))
        self.assertEqual(result["uncertain_rows"][0]["pid"], 701)

    def test_an_unreadable_chain_excludes_nothing(self):
        def broken():
            raise OSError(12, "injected")
        record = bounded.ancestor_identities(snapshot=broken, parent=lambda: 700)
        self.assertEqual((record["state"], record["count"], record["identities"]),
                         ("unreadable", 0, []))
        table = FakeTable([row(700, start=11)], {700: ProcessLookupError(errno.ESRCH, "gone")},
                          reread={700: row(700, start=11)})
        result = census(table, ancestors=record["identities"]).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))

    def test_the_depth_bound_holds(self):
        rows = self.chain([(pid, pid, pid - 1) for pid in range(1000, 800, -1)])
        record = bounded.ancestor_identities(snapshot=lambda: rows, parent=lambda: 1000)
        self.assertEqual(bounded.ANCESTOR_MAX_LEVELS, 64)
        self.assertEqual((record["count"], record["stop"]), (64, "depth-bound"))
        self.assertEqual(len(record["identities"]), 64)
        self.assertEqual((record["identities"][-1]["pid"], record["identities"][-1]["level"]),
                         (1000 - 63, 64))
        small = bounded.ancestor_identities(snapshot=lambda: rows, parent=lambda: 1000, max_levels=3)
        self.assertEqual((small["count"], small["stop"]), (3, "depth-bound"))

    def test_the_walk_never_guesses_a_missing_level(self):
        rows = self.chain([(700, 11, 600)])  # 600 is not in the snapshot
        record = bounded.ancestor_identities(snapshot=lambda: rows, parent=lambda: 700)
        self.assertEqual((record["count"], record["stop"]), (1, "not-in-snapshot"))
        self.assertEqual(record["identities"], [{"pid": 700, "start": 11, "level": 1}])

    def test_a_malformed_record_excludes_nothing(self):
        table = FakeTable([row(700, start=11)], {700: 100})
        for bad in ([(700,)], [(700, "11")], [(True, 11)], [(0, 11)], [("700", 11)], [None], "700"):
            result = census(table, ancestors=bad).round()
            self.assertEqual(result["members"], 1, bad)
            self.assertEqual(result["uncertain"], 0, bad)

    # No wall, boot or uptime fact may exclude a row (ruling ea5a3dc9).

    def test_there_is_no_clock_based_exclusion_surface(self):
        self.assertFalse(hasattr(bounded, "boot_clock_evidence"))
        self.assertFalse(hasattr(bounded, "CLOCK_AGREEMENT_US"))
        parameters = inspect.signature(bounded.Census.__init__).parameters
        self.assertNotIn("start_token", parameters)
        self.assertNotIn("boot_evidence", parameters)
        self.assertNotIn("clock", parameters)

    def early_row_table(self):
        """A row whose start token is far earlier than any supervisor start."""
        return FakeTable([row(800, start=1, ppid=600), row(903, start=2, ppid=800)],
                         {800: 7, 903: PermissionError(1, "denied")})

    def assert_no_clock_exclusion(self):
        table = self.early_row_table()
        result = census(table, ancestors=[self.LAUNCHER]).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual(result["ancestor_exclusions"]["count"], 0)
        self.assertEqual(result["ancestor_exclusions"]["rows"], [])

    def test_a_masked_backward_wall_step_never_excludes_a_row(self):
        # A backward step that leaves the wall and monotonic readings looking
        # consistent would have hidden an earlier-looking creation. No such fact
        # exists in the census any more, so contradictory wall facts cannot
        # change the classification.
        saved = (time.time_ns, time.clock_gettime_ns)
        self.addCleanup(lambda: (setattr(time, "time_ns", saved[0]),
                                 setattr(time, "clock_gettime_ns", saved[1])))
        time.time_ns = lambda: 60 * 10 ** 9            # wall clock stepped far back
        time.clock_gettime_ns = lambda _clock: 400 * 10 ** 9   # "uptime" far ahead
        self.assert_no_clock_exclusion()

    def test_a_back_and_forth_wall_step_never_excludes_a_row(self):
        saved = (time.time_ns, time.clock_gettime_ns)
        self.addCleanup(lambda: (setattr(time, "time_ns", saved[0]),
                                 setattr(time, "clock_gettime_ns", saved[1])))
        values = iter([10 ** 18, 10 ** 9, 10 ** 18, 10 ** 9])
        time.time_ns = lambda: next(values, 10 ** 18)
        time.clock_gettime_ns = lambda _clock: 0
        for _ in range(2):
            self.assert_no_clock_exclusion()

    def test_a_forward_wall_step_never_excludes_a_row(self):
        saved = (time.time_ns, time.clock_gettime_ns)
        self.addCleanup(lambda: (setattr(time, "time_ns", saved[0]),
                                 setattr(time, "clock_gettime_ns", saved[1])))
        time.time_ns = lambda: 2 * 10 ** 18            # wall clock jumped forward
        time.clock_gettime_ns = lambda _clock: 1
        self.assert_no_clock_exclusion()

    def test_a_tracked_descendant_with_an_earlier_start_token_stays_counted(self):
        # A member whose start token looks earlier than the supervisor's own is
        # still a member, in the same round and across rounds.
        member = row(500, start=1, ppid=400)
        table = FakeTable([member], {500: 100})
        check = census(table, ancestors=[self.LAUNCHER])
        first = check.round()
        self.assertEqual((first["certain"], first["members"], first["uncertain"]), (True, 1, 0))
        second = check.round()
        self.assertEqual((second["certain"], second["members"], second["uncertain"]), (True, 1, 0))
        self.assertEqual(second["ancestor_exclusions"]["count"], 0, second["ancestor_exclusions"])

    # The launcher-sibling rule needs a positive parent and positive isolation.

    def test_a_launcher_sibling_is_excluded_only_when_outside_the_session_and_group(self):
        sibling = row(900, start=12, ppid=self.LAUNCHER[0])
        outside = FakeTable(self.launcher_rows(sibling), {700: 7, 900: 7}, groups={900: 900})
        result = census(outside, ancestors=[self.LAUNCHER]).round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))
        self.assertEqual(result["ancestor_exclusions"]["rows"],
                         [{"pid": 700, "start": 11, "rule": "ancestor"},
                          {"pid": 900, "start": 12, "rule": "parent"}], result["ancestor_exclusions"])
        # In the leader's session it is a member, never excluded.
        in_session = FakeTable(self.launcher_rows(sibling), {700: 7, 900: 100})
        result = census(in_session, ancestors=[self.LAUNCHER]).round()
        self.assertEqual((result["members"], result["uncertain"]), (1, 0))
        self.assertEqual(result["ancestor_exclusions"]["by_rule"]["parent"], 0)
        # A leader group with a foreign session is not a positive decision, so
        # the row keeps main's classification: foreign, never excluded.
        in_group = FakeTable(self.launcher_rows(sibling), {700: 7, 900: 7}, groups={900: 100})
        result = census(in_group, ancestors=[self.LAUNCHER]).round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))
        self.assertEqual(result["ancestor_exclusions"]["by_rule"]["parent"], 0)
        # An unreadable session keeps main's uncertain result; an unreadable
        # group with a readable foreign session keeps main's foreign result.
        unreadable_session = FakeTable(self.launcher_rows(sibling),
                                       {700: 7, 900: ProcessLookupError(errno.ESRCH, "gone")},
                                       groups={900: 900})
        result = census(unreadable_session, ancestors=[self.LAUNCHER]).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual(result["ancestor_exclusions"]["by_rule"]["parent"], 0)
        unreadable_group = FakeTable(self.launcher_rows(sibling), {700: 7, 900: 7},
                                     groups={900: OSError(1, "denied")})
        result = census(unreadable_group, ancestors=[self.LAUNCHER]).round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))
        self.assertEqual(result["ancestor_exclusions"]["by_rule"]["parent"], 0)

    def test_a_tracked_member_with_a_recorded_ancestor_parent_is_never_excluded(self):
        # Review finding: the parent rule must not precede, or remove, tracked
        # attribution. A row this census already accepted as a member keeps its
        # member status even when its parent is a recorded ancestor.
        sibling = row(900, start=12, ppid=self.LAUNCHER[0])
        table = FakeTable(self.launcher_rows(sibling), {700: 7, 900: 100})
        check = census(table, ancestors=[self.LAUNCHER])
        first = check.round()
        self.assertEqual((first["certain"], first["members"], first["uncertain"]), (True, 1, 0))
        self.assertIn((900, 12), check.tracked)
        self.assertEqual(first["ancestor_exclusions"]["by_rule"]["parent"], 0)
        # The tracked veto holds on the next round even if the row's own session
        # becomes unreadable: a member is never reclassified by a launcher rule.
        table.sessions = {700: 7, 900: ProcessLookupError(errno.ESRCH, "gone")}
        second = check.round()
        self.assertEqual((second["members"], second["uncertain"]), (1, 0))
        self.assertIn((900, 12), check.tracked)
        self.assertEqual(second["ancestor_exclusions"]["by_rule"]["parent"], 0)

    def test_a_pid_reused_between_the_sibling_reads_is_never_excluded(self):
        # Review finding: the session/group facts must belong to the snapshot
        # row's own identity. A replacement identity inside the observation
        # window (the second sibling read) is never excluded as a sibling.
        sibling = row(900, start=12, ppid=self.LAUNCHER[0])
        table = FakeTable(self.launcher_rows(sibling), {700: 7, 900: 100},
                          reread={900: [sibling, row(900, start=99, ppid=700)]})
        result = census(table, ancestors=[self.LAUNCHER]).round()
        # A fresh bound session finds the replacement itself as a member.
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 1, 0))
        self.assertIn({"pid": 900, "start": 99, "state": "member"}, result["replacement_work"]["rows"])
        self.assertEqual(result["ancestor_exclusions"]["by_rule"]["parent"], 0)
        self.assertEqual(result["ancestor_exclusions"]["rows"],
                         [{"pid": 700, "start": 11, "rule": "ancestor"}])

    def test_a_changed_ancestor_child_identity_never_falls_through_to_its_parent(self):
        # Review finding: a failed ancestor re-verification stays unknown. The
        # child 600 is a recorded ancestor PID with a changed start token and a
        # stable recorded parent (700), which must not turn it into a sibling.
        rows = [row(700, start=11, ppid=500), row(600, start=99, ppid=700)]
        table = FakeTable(rows, {700: 7, 600: PermissionError(1, "denied")},
                          reread={700: row(700, start=11, ppid=500),
                                  600: row(600, start=99, ppid=700)})
        result = census(table, ancestors=[self.LAUNCHER, (600, 12)]).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual(result["uncertain_rows"][0]["pid"], 600)
        self.assertEqual(result["ancestor_exclusions"]["by_rule"],
                         {"ancestor": 1, "parent": 0}, result["ancestor_exclusions"])
        self.assertEqual(result["ancestor_exclusions"]["rows"],
                         [{"pid": 700, "start": 11, "rule": "ancestor"}])

    def test_a_foreign_ancestor_child_with_an_undecided_observation_matches_main(self):
        # Review c4032d7b: an UNDECIDED launcher observation excludes no row and
        # falls through to main's ordinary getsid/reread classification. A stable
        # replacement at a recorded ancestor PID with a readable foreign session
        # is therefore foreign, exactly as it is without the launcher rules.
        replaced = row(700, start=99, ppid=600)
        table = FakeTable([replaced], {700: 7}, reread={700: replaced})
        result = census(table, ancestors=[self.LAUNCHER]).round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))
        self.assertEqual(result["ancestor_exclusions"]["count"], 0)
        # Counterfactual: the superseded path turned the same row uncertain, so
        # this case fails under it and only the fall-through satisfies it.
        def superseded_launcher_path(round_result):
            return dict(round_result, certain=False, uncertain=round_result["uncertain"] + 1)
        superseded = superseded_launcher_path(result)
        self.assertNotEqual((superseded["certain"], superseded["uncertain"]),
                            (result["certain"], result["uncertain"]),
                            "the old ambiguous path must fail this classification")
        # A positively supervised session at that PID is still carried as a member.
        supervised = FakeTable([row(700, start=99, ppid=600)], {700: 100},
                               reread={700: row(700, start=99, ppid=600)})
        result = census(supervised, ancestors=[self.LAUNCHER]).round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 1, 0))

    def test_a_transient_failed_sibling_reverification_takes_main_classification(self):
        # Review c4032d7b: a transiently failed re-verification is undecided, so
        # it excludes nothing and the readable foreign observations classify the
        # row exactly as main does.
        sibling = row(900, start=12, ppid=self.LAUNCHER[0])
        table = FakeTable(self.launcher_rows(sibling), {700: 7, 900: 7}, groups={900: 900},
                          reread={900: [OSError(5, "transient"), sibling, sibling, sibling]})
        result = census(table, ancestors=[self.LAUNCHER]).round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))
        self.assertEqual(result["ancestor_exclusions"]["by_rule"]["parent"], 0)

    def test_a_held_suite_zombie_classifies_exactly_as_main_does(self):
        # Review c4032d7b: a zombie's launcher rules must never change main's
        # classification. Here the row's parent is the recorded launcher but its
        # group read is unreadable, so the launcher decision is undecided: the
        # terminal-allowlist and strict outcomes must match the same table with
        # no ancestor record at all.
        zombie = row(900, start=12, ppid=self.LAUNCHER[0], zombie=True)
        def table():
            return FakeTable([row(700, start=11, ppid=600), zombie],
                             {700: 7, 900: ProcessLookupError(errno.ESRCH, "gone")},
                             reread={900: zombie}, groups={900: OSError(1, "denied")})
        for terminal_allowed in (True, False):
            with_ancestors = census(table(), ancestors=[self.LAUNCHER],
                                    terminal_allowed=terminal_allowed).round()
            without = census(table(), ancestors=(), terminal_allowed=terminal_allowed).round()
            self.assertEqual((with_ancestors["certain"], with_ancestors["members"],
                              with_ancestors["uncertain"]),
                             (without["certain"], without["members"], without["uncertain"]),
                             terminal_allowed)
            self.assertEqual(with_ancestors["terminal_count_status"],
                             without["terminal_count_status"], terminal_allowed)
            self.assertEqual(with_ancestors["terminal_nonwriters"],
                             without["terminal_nonwriters"], terminal_allowed)
            self.assertEqual(with_ancestors["ancestor_exclusions"]["by_rule"]["parent"], 0)

    def test_an_unreadable_sibling_session_or_group_keeps_main_classification(self):
        sibling = row(900, start=12, ppid=self.LAUNCHER[0])
        for sessions, groups in (
                ({700: 7, 900: ProcessLookupError(errno.ESRCH, "gone")}, {900: 900}),
                ({700: 7, 900: 7}, {900: OSError(1, "denied")}),
                ({700: 7, 900: 7}, {900: None})):
            table = FakeTable(self.launcher_rows(sibling), sessions, groups=groups)
            result = census(table, ancestors=[self.LAUNCHER]).round()
            # Main's result: an unreadable session is uncertain; an unreadable
            # group with a readable foreign session is foreign.
            expected = (False, 1) if 900 in sessions and isinstance(
                sessions.get(900), BaseException) else (True, 0)
            self.assertEqual((result["certain"], result["uncertain"]), expected, (sessions, groups))
            self.assertEqual(result["ancestor_exclusions"]["by_rule"]["parent"], 0)

    def test_an_inside_ancestor_pid_observation_is_carried_as_a_member(self):
        # Review finding: a positively observed leader-session member identity
        # must not be discarded before classification, or a repeated session read
        # could report it foreign and skip it. The first session read is the
        # bound observation (the leader's session); every later read is foreign.
        replaced = row(700, start=99, ppid=600)
        table = FakeTable([replaced], {700: [100, 7, 7]}, reread={700: replaced})
        check = census(table, ancestors=[self.LAUNCHER])
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 1, 0))
        self.assertIn((700, 99), check.tracked)
        # Carried evidence stays a member on the next round, whatever it now reads.
        table.sessions = {700: 7}
        follow = check.round()
        self.assertEqual((follow["members"], follow["uncertain"]), (1, 0))

    def test_an_inside_sibling_observation_is_carried_as_a_member(self):
        # The same rule for the launcher-sibling branch: a positive in-session
        # observation is carried as a member instead of being re-read.
        sibling = row(900, start=12, ppid=self.LAUNCHER[0])
        table = FakeTable(self.launcher_rows(sibling), {700: 7, 900: [100, 7, 7]})
        check = census(table, ancestors=[self.LAUNCHER])
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 1, 0))
        self.assertIn((900, 12), check.tracked)
        self.assertEqual(result["ancestor_exclusions"]["by_rule"]["parent"], 0)
        table.sessions = {700: 7, 900: 7}
        follow = check.round()
        self.assertEqual((follow["members"], follow["uncertain"]), (1, 0))

    def test_a_sibling_spawned_mid_run_is_excluded_and_recorded(self):
        # Round 1: the launcher alone. Round 2: an unrelated sibling appears
        # mid-run. The same census must stay certain and record the evidence.
        launcher, sibling = row(700, start=11, ppid=600), row(904, start=40, ppid=700)
        table = FakeTable([launcher], {700: 7})
        check = census(table, ancestors=[self.LAUNCHER])
        first = check.round()
        self.assertEqual((first["certain"], first["ancestor_exclusions"]["count"]), (True, 1))
        table.rows = [launcher, sibling]
        table.sessions = {700: 7, 904: 7}
        table.groups = {904: 904}
        second = check.round()
        self.assertEqual((second["certain"], second["uncertain"]), (True, 0))
        self.assertEqual(second["ancestor_exclusions"]["rows"],
                         [{"pid": 700, "start": 11, "rule": "ancestor"},
                          {"pid": 904, "start": 40, "rule": "parent"}])

    def test_a_reused_parent_pid_is_not_a_parent_exclusion(self):
        # The parent PID is a recorded launcher PID, but its current start token
        # is different, so the parent is a different process.
        rows = [row(700, start=99, ppid=600), row(900, start=12, ppid=700)]
        table = FakeTable(rows, {700: 7, 900: PermissionError(1, "denied")},
                          reread={700: row(700, start=99), 900: row(900, start=12, ppid=700)})
        result = census(table, ancestors=[self.LAUNCHER]).round()
        # The changed identity at a recorded ancestor PID falls through to main's
        # classification (foreign here), and only the child's unreadable session
        # stays unknown: no exclusion and no launcher-side classification.
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual(result["ancestor_exclusions"]["by_rule"]["parent"], 0)
        self.assertEqual(result["ancestor_exclusions"]["rows"], [], result["ancestor_exclusions"])

    def test_an_unknown_parent_is_never_excluded(self):
        stranger = row(901, start=10, ppid=999)
        table = FakeTable([stranger], {901: PermissionError(1, "denied")})
        result = census(table, ancestors=[self.LAUNCHER]).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual(result["ancestor_exclusions"]["count"], 0)

    def test_a_ppid_1_orphan_is_never_excluded(self):
        orphan = row(902, start=10, ppid=1)
        table = FakeTable([orphan], {902: PermissionError(1, "denied")})
        result = census(table, ancestors=[self.LAUNCHER, (1, 14)]).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual(result["ancestor_exclusions"]["count"], 0)

    def test_the_bounded_evidence_keeps_a_capped_number_of_rows(self):
        # 30 excludable siblings: every decision is counted, rows are capped.
        siblings = [row(900 + n, start=20 + n, ppid=700) for n in range(30)]
        sessions = {700: 7}
        sessions.update({900 + n: 7 for n in range(30)})
        table = FakeTable([row(700, start=11, ppid=600)] + siblings, sessions)
        result = census(table, ancestors=[self.LAUNCHER]).round()
        evidence = result["ancestor_exclusions"]
        self.assertEqual(evidence["count"], 31)  # the launcher row plus 30 siblings
        self.assertEqual(len(evidence["rows"]), bounded.EXCLUSION_MAX_ROWS)
        self.assertEqual(evidence["omitted"], 11, evidence)
        self.assertEqual(evidence["rows"][0], {"pid": 700, "start": 11, "rule": "ancestor"})
        self.assertEqual(evidence["by_rule"], {"ancestor": 1, "parent": 30})

    def test_the_receipt_evidence_merges_without_raising(self):
        receipt = {}
        report = {"rows": [{"pid": 900 + n, "start": n, "rule": "parent"} for n in range(25)],
                  "count": 25, "by_rule": {"ancestor": 0, "parent": 25}}
        bounded.merge_exclusions(receipt, report)
        record = receipt["ancestor_exclusions"]
        self.assertEqual((record["count"], len(record["rows"]), record["omitted"]),
                         (25, bounded.EXCLUSION_MAX_ROWS, 5))
        self.assertEqual(record["by_rule"]["parent"], 25)
        self.assertEqual(set(record), {"rows", "count", "omitted", "by_rule", "rounds"})
        # A repeat of the same identities adds to the counts but not the rows.
        bounded.merge_exclusions(receipt, report)
        self.assertEqual((record["count"], len(record["rows"])), (50, bounded.EXCLUSION_MAX_ROWS))
        self.assertEqual(record["omitted"], 30, record)  # count == rows + omitted
        # A malformed or absent report changes nothing and never raises.
        for bad in (None, {}, {"rows": "x", "count": "1", "by_rule": [], "clock": 7}, 5):
            bounded.merge_exclusions(receipt, bad)
        self.assertEqual((record["count"], record["omitted"]), (50, 30))
        bounded.merge_exclusions(None, report)
        bounded.merge_exclusions({}, {"rows": [None, {"pid": True, "start": 1, "rule": "parent"},
                                               {"pid": 1, "start": 1, "rule": "other"}], "count": 3})

    def test_the_monitor_hands_the_recorded_chain_to_the_census(self):
        seen = []

        class Recording(bounded.Census):
            def __init__(self, sid, terminal_allowed=False, ancestors=(), getpgid=None):
                seen.append((sid, tuple(ancestors)))

            def round(self):
                self.round_deadline = bounded.now_ns() + 2 * NS
                return {"certain": True, "members": 0, "uncertain": 0, "member_rows": [],
                        "uncertain_rows": [], "terminal_count_status": "valid",
                        "terminal_nonwriters": 0, "replacement_work": fixture_work(),
                        "ancestor_exclusions": {
                            "rows": [{"pid": 900, "start": 5, "rule": "parent"}],
                            "count": 1, "by_rule": {"ancestor": 0, "parent": 1}}}

        identities = [{"pid": 700, "start": 11, "level": 1}, {"pid": 1, "start": 14, "level": 2}]
        ctx = {"run_id": "r", "inject": [], "terminal_mode": {"mode": "strict"},
               "ancestors": {"state": "recorded", "count": 2, "stop": "pid-1",
                             "identities": identities}}
        ctl_r, ctl_w = os.pipe()
        cmd_r, cmd_w = os.pipe()
        out_r, out_w = os.pipe()
        err_r, err_w = os.pipe()
        for fd in (ctl_r, cmd_r, out_r, err_r, ctl_w, cmd_w, out_w, err_w):
            self.addCleanup(os.close, fd)
        monitor = bounded.Monitor(ctx, ctl_w, cmd_r, out_r, err_r)
        monitor.receipt = {"census": None, "census_rounds": 0, "settlement": monitor.settlement}
        monitor.leader = [4242, 77]
        monitor.census_stop = OneShotStop()
        saved = bounded.Census
        bounded.Census = Recording
        try:
            monitor.census_loop(after=7)
        finally:
            bounded.Census = saved
        self.assertEqual(seen, [(4242, tuple(identities))])
        # The round's bounded positive evidence is merged into the receipt.
        merged = monitor.receipt["ancestor_exclusions"]
        self.assertEqual((merged["count"], merged["rounds"]), (1, 1), merged)
        self.assertEqual(merged["rows"], [{"pid": 900, "start": 5, "rule": "parent"}], merged)
        self.assertEqual(merged["by_rule"]["parent"], 1, merged)


class PureCensusFinalAbsence(unittest.TestCase):
    """Oxide#114: the round's existing final read applies the Gone rule."""

    def test_final_none_after_matching_exhaustion_untracked(self):
        base = row(109, start=5)
        table = FakeTable([base], {109: ProcessLookupError(3, "gone")},
                           reread={109: lambda reads: row(109, start=5) if reads <= 20 else None})
        check = census(table, prime=True)
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))
        self.assertEqual(table.reads, 21)
        self.assertNotIn((109, 5), check.tracked)
        diag = result.get("diagnostics")
        self.assertIsNotNone(diag)
        self.assertEqual(diag["attempted"], 1)
        self.assertEqual(diag["rows"][0]["confirm"], "matching_exhausted")
        self.assertEqual(diag["rows"][0]["final"], "absent")

    def test_final_replacement_after_matching_exhaustion_untracked(self):
        table = FakeTable([row(110, start=5)], {110: ProcessLookupError(3, "gone")},
                           reread={110: lambda reads: row(110, start=5) if reads <= 20 else row(110, start=9)})
        check = census(table, prime=True)
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (False, 0, 1))
        self.assertGreater(table.reads, 21)
        self.assertNotIn((110, 5), check.tracked)
        self.assertNotIn((110, 9), check.tracked)
        self.assertEqual(result["diagnostics"]["rows"][0]["final"], "replaced")

    def test_final_replacement_is_classified_in_same_round(self):
        # The replacement has a bound leader session; the old implementation
        # drops it and incorrectly reports a certain empty deciding census.
        table = FakeTable([row(110, start=5)],
                          {110: [ProcessLookupError(3, "gone"), 100]},
                          reread={110: lambda reads: row(110, start=5) if reads <= 21 else row(110, start=9)})
        check = census(table)
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 1, 0))
        self.assertIn((110, 9), check.tracked)

    def test_final_none_tracked_drops_only_old(self):
        table = FakeTable([row(111, start=5)], {111: 100})
        check = census(table, prime=True)
        self.assertEqual(check.round()["members"], 1)
        table.sessions = {111: ProcessLookupError(3, "gone")}
        table.reads = 0
        table.rereads = {111: lambda reads: row(111, start=5) if reads <= 20 else None}
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))
        self.assertNotIn((111, 5), check.tracked)

    def test_final_replacement_tracked_not_foreign_by_inference(self):
        table = FakeTable([row(112, start=5)], {112: 100})
        check = census(table, prime=True)
        check.round()
        table.sessions = {112: ProcessLookupError(3, "gone")}
        table.reads = 0
        table.rereads = {112: lambda reads: row(112, start=5) if reads <= 20 else row(112, start=9)}
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (False, 0, 1))
        self.assertNotIn((112, 5), check.tracked)
        self.assertNotIn((112, 9), check.tracked)
        # The late replacement cannot complete confirmation in this round.
        self.assertEqual(result["replacement_work"]["replacement"], 1)
        # A later round may bind it foreign independently.
        table.rows = [row(112, start=9)]
        table.sessions = {112: 7}
        table.rereads = {}
        table.reads = 0
        follow = check.round()
        self.assertEqual((follow["certain"], follow["members"], follow["uncertain"]), (True, 0, 0))

    def test_persistent_matching_untracked_still_uncertain(self):
        table = FakeTable([row(113, start=5)], {113: ProcessLookupError(3, "gone")})
        check = census(table, prime=True)
        result = check.round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual(table.reads, 21)
        self.assertEqual(result["diagnostics"]["rows"][0]["final"], "same_identity")

    def test_persistent_matching_tracked_stays_member(self):
        table = FakeTable([row(114, start=5)], {114: 100})
        check = census(table, prime=True)
        check.round()
        table.sessions = {114: ProcessLookupError(3, "gone")}
        table.reads = 0
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 1, 0))

    def test_final_read_failure_still_blocks(self):
        def failing(_reads):
            raise PermissionError(13, "denied")
        table = FakeTable([row(115, start=5)], {115: ProcessLookupError(3, "gone")}, reread={115: failing})
        result = census(table, prime=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual(result["diagnostics"]["rows"][0]["final"], "read_failed")

    def test_confirm_read_failure_then_final_absence_gone(self):
        def flaky(reads):
            if reads == 1:
                raise PermissionError(13, "denied")
            return None
        table = FakeTable([row(116, start=5)], {116: ProcessLookupError(3, "gone")}, reread={116: flaky})
        check = census(table, prime=True)
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))
        self.assertEqual(table.reads, 2)
        self.assertEqual(result["diagnostics"]["rows"][0]["confirm"], "read_failed")
        self.assertEqual(result["diagnostics"]["rows"][0]["final"], "absent")

    def test_confirm_read_failure_then_final_failure_blocks(self):
        def failing(_reads):
            raise PermissionError(13, "denied")
        table = FakeTable([row(117, start=5)], {117: ProcessLookupError(3, "gone")}, reread={117: failing})
        result = census(table, prime=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual(table.reads, 2)

    def test_non_esrch_still_blocks(self):
        table = FakeTable([row(118, start=5)], {118: PermissionError(1, "denied")})
        result = census(table, prime=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertNotIn("diagnostics", result)

    def test_tracked_changes_still_member_until_positive_absence(self):
        table = FakeTable([row(119, start=5, ppid=101)], {119: 100})
        check = census(table, prime=True)
        self.assertEqual(check.round()["members"], 1)
        table.rows = [row(119, start=5, ppid=1, uid=0)]
        table.sessions = {119: 119}
        table.reads = 0
        self.assertEqual(check.round()["members"], 1)
        table.sessions = {119: ProcessLookupError(3, "gone")}
        table.reads = 0
        table.rereads = {119: lambda reads: row(119, start=5, ppid=1, uid=0) if reads <= 20 else None}
        result = check.round()
        self.assertEqual((result["certain"], result["members"]), (True, 0))


class PureDiagnostics(unittest.TestCase):
    def test_enums_and_no_raw_text(self):
        table = FakeTable([row(120, start=5)], {120: ProcessLookupError(3, "gone")})
        result = census(table).round()
        diag = result["diagnostics"]
        self.assertEqual((diag["attempted"], diag["retained"]), (1, 1))
        self.assertEqual(diag["omitted"], 0)
        self.assertFalse(diag["saturated"])
        entry = diag["rows"][0]
        self.assertIn(entry["confirm"], bounded.CONFIRM_VALUES)
        self.assertIn(entry["final"], bounded.FINAL_VALUES)
        self.assertIn(entry["error"], bounded.DIAG_ERROR_VALUES)
        self.assertEqual(entry["confirm_tries"], 20)
        blob = json.dumps(diag, ensure_ascii=True, separators=(',', ':')).encode("utf-8")
        self.assertLessEqual(len(blob), bounded.DIAG_MAX_BYTES)
        text = blob.decode("utf-8")
        self.assertNotIn("denied", text)
        self.assertNotIn("ProcessLookup", text)

    def test_max_width_rows_under_ceiling(self):
        big = {"pid": bounded.U64MAX, "start": bounded.U64MAX, "ppid": bounded.U64MAX,
               "pgid": bounded.U64MAX, "uid": bounded.U64MAX, "zombie": True}
        rows = [bounded._diag_row(big, 20, "matching_exhausted", "same_identity", "other", 4095)
                for _ in range(20)]
        obj = bounded._diag_object(20, rows, False)
        self.assertEqual((obj["retained"], obj["omitted"]), (20, 0))
        blob = json.dumps(obj, ensure_ascii=True, separators=(',', ':')).encode("utf-8")
        self.assertLessEqual(len(blob), bounded.DIAG_MAX_BYTES)

    def test_overflow_omitted_count(self):
        rows = [bounded._diag_row(row(200 + i, start=i), 20, "matching_exhausted", "same_identity",
                                   "not_found", 3) for i in range(25)]
        obj = bounded._diag_object(25, rows, False)
        self.assertEqual((obj["attempted"], obj["retained"], obj["omitted"]), (25, 20, 5))
        self.assertLessEqual(len(json.dumps(obj, ensure_ascii=True, separators=(',', ':')).encode("utf-8")),
                             bounded.DIAG_MAX_BYTES)

    def test_byte_ceiling_drops_rows_without_changing_classification(self):
        table = FakeTable([row(121, start=5)], {121: ProcessLookupError(3, "gone")})
        plain = census(table).round()
        saved = bounded.DIAG_MAX_BYTES
        bounded.DIAG_MAX_BYTES = 120
        try:
            table2 = FakeTable([row(121, start=5)], {121: ProcessLookupError(3, "gone")})
            trimmed = census(table2).round()
        finally:
            bounded.DIAG_MAX_BYTES = saved
        self.assertEqual((plain["certain"], plain["members"], plain["uncertain"]),
                         (trimmed["certain"], trimmed["members"], trimmed["uncertain"]))
        diag = trimmed.get("diagnostics")
        if diag is not None:
            blob = json.dumps(diag, ensure_ascii=True, separators=(',', ':')).encode("utf-8")
            self.assertLessEqual(len(blob), 120)
            self.assertGreaterEqual(diag["omitted"], 0)

    def test_serialization_failure_omits_without_changing_result(self):
        table = FakeTable([row(122, start=5)], {122: ProcessLookupError(3, "gone")})
        expected = census(table).round()
        real_dumps = json.dumps

        def broken(*_args, **_kwargs):
            raise ValueError("injected")
        table2 = FakeTable([row(122, start=5)], {122: ProcessLookupError(3, "gone")})
        check = census(table2)
        saved = bounded.json.dumps
        bounded.json.dumps = broken
        try:
            result = check.round()
        finally:
            bounded.json.dumps = saved
        self.assertEqual((result["certain"], result["members"], result["uncertain"]),
                         (expected["certain"], expected["members"], expected["uncertain"]))
        self.assertNotIn("diagnostics", result)
        self.assertTrue(real_dumps is not None)

    def test_invalid_numerics_omitted(self):
        table = FakeTable([row(105, start=-5)], {105: ProcessLookupError(3, "gone")},
                           reread={105: None})
        result = census(table, prime=True).round()
        self.assertEqual((result["certain"], result["members"]), (True, 0))
        entry = result["diagnostics"]["rows"][0]
        self.assertNotIn("start", entry)
        self.assertEqual(entry["confirm"], "absent")

    def test_no_diagnostics_without_esrch(self):
        table = FakeTable([row(101), row(200)], {101: 100, 200: 7})
        self.assertNotIn("diagnostics", census(table).round())


def zombie_row(pid, start):
    return row(pid, start=start, zombie=True)


class PureAllowlist(unittest.TestCase):
    """One-shot exact-kernel allowlist decision (no live kernel change)."""

    def reader(self, values=(), errors=()):
        table, failures = dict(values), dict(errors)
        def read(name, _size):
            if name in failures:
                raise failures[name]
            return table[name]
        return read

    def exact_triple(self):
        return {
            "kern.uuid": bytes.fromhex("8D3E13A001FC381D9DA37F0AF537CB32"),
            "kern.osversion": b"25G83\x00",
            "kern.version": (b"Darwin Kernel Version 25.6.0: Fri Jul 31 19:19:08 PDT 2026; "
                               b"root:xnu-12377.161.14~5/RELEASE_ARM64_T6050\x00"),
        }

    def test_exact_match_allowlisted(self):
        mode = bounded._decide_terminal_mode(self.reader(self.exact_triple()))
        self.assertEqual(mode, {"mode": "allowlisted", "reason": "allowlisted",
                                "evidence": bounded.ALLOW_EVIDENCE_ID})

    def test_ascii_uuid_form_allowlisted(self):
        values = self.exact_triple()
        values["kern.uuid"] = b"8D3E13A0-01FC-381D-9DA3-7F0AF537CB32\x00"
        self.assertEqual(bounded._decide_terminal_mode(self.reader(values))["mode"], "allowlisted")

    def test_build_mismatch_strict(self):
        values = self.exact_triple()
        values["kern.osversion"] = b"25G84\x00"
        self.assertEqual(bounded._decide_terminal_mode(self.reader(values)),
                         {"mode": "strict", "reason": "identity_mismatch",
                          "evidence": bounded.ALLOW_EVIDENCE_ID})

    def test_uuid_wrong_length_malformed(self):
        values = self.exact_triple()
        values["kern.uuid"] = b"short"
        self.assertEqual(bounded._decide_terminal_mode(self.reader(values))["reason"],
                         "identity_malformed")

    def test_uuid_wrong_value_mismatch(self):
        values = self.exact_triple()
        values["kern.uuid"] = bytes(16)
        self.assertEqual(bounded._decide_terminal_mode(self.reader(values))["reason"],
                         "identity_mismatch")

    def test_read_error_strict(self):
        self.assertEqual(bounded._decide_terminal_mode(
            self.reader({}, {"kern.uuid": OSError(2, "no such entry")}))["reason"],
            "identity_read_error")

    def test_oversize_malformed(self):
        values = self.exact_triple()
        del values["kern.osversion"]
        self.assertEqual(bounded._decide_terminal_mode(
            self.reader(values, {"kern.osversion": OSError(errno.ENOMEM, "too big")}))["reason"],
            "identity_malformed")

    def test_version_contradiction_malformed(self):
        values = self.exact_triple()
        values["kern.version"] = (b"root:xnu-12377.161.14~5/RELEASE_ARM64_T6050 "
                                    b"root:xnu-9.9.9/RELEASE_ARM64_T9999\x00")
        self.assertEqual(bounded._decide_terminal_mode(self.reader(values))["reason"],
                         "identity_malformed")

    def test_version_without_token_mismatch(self):
        values = self.exact_triple()
        values["kern.version"] = b"Darwin Kernel Version 25.6.0\x00"
        self.assertEqual(bounded._decide_terminal_mode(self.reader(values))["reason"],
                         "identity_mismatch")

    def test_version_interior_nul_malformed(self):
        values = self.exact_triple()
        values["kern.version"] = b"junk\x00root:xnu-12377.161.14~5/RELEASE_ARM64_T6050\x00"
        self.assertEqual(bounded._decide_terminal_mode(self.reader(values))["reason"],
                         "identity_malformed")

    def test_version_missing_nul_malformed(self):
        values = self.exact_triple()
        values["kern.version"] = b"root:xnu-12377.161.14~5/RELEASE_ARM64_T6050"
        self.assertEqual(bounded._decide_terminal_mode(self.reader(values))["reason"],
                         "identity_malformed")

    def test_fixed_reader_exact_bytes(self):
        lib = _fake_libc({"kern.osversion": (b"25G83\x00", 6)})
        reader = bounded._fixed_sysctl_reader(lib.sysctlbyname, lib.get_errno)
        self.assertEqual(reader("kern.osversion", 64), b"25G83\x00")

    def test_fixed_reader_rejects_reported_oversize(self):
        # Success with a reported length above capacity must raise, never
        # silently slice the buffer.
        lib = _fake_libc({"kern.osversion": (b"25G83\x00", 65)})
        reader = bounded._fixed_sysctl_reader(lib.sysctlbyname, lib.get_errno)
        with self.assertRaises(OSError) as raised:
            reader("kern.osversion", 64)
        self.assertEqual(raised.exception.errno, errno.ENOMEM)

    def test_fixed_reader_propagates_error(self):
        lib = _fake_libc({})
        lib.errors = {"kern.uuid": 2}
        reader = bounded._fixed_sysctl_reader(lib.sysctlbyname, lib.get_errno)
        with self.assertRaises(OSError) as raised:
            reader("kern.uuid", 64)
        self.assertEqual(raised.exception.errno, 2)

    def test_decide_through_fixed_reader(self):
        lib = _fake_libc({
            "kern.uuid": (bytes.fromhex("8D3E13A001FC381D9DA37F0AF537CB32"), 16),
            "kern.osversion": (b"25G83\x00", 6),
            "kern.version": (b"root:xnu-12377.161.14~5/RELEASE_ARM64_T6050\x00", 44),
        })
        mode = bounded._decide_terminal_mode(bounded._fixed_sysctl_reader(
            lib.sysctlbyname, lib.get_errno))
        self.assertEqual(mode["mode"], "allowlisted")


class PureTerminalCounts(unittest.TestCase):
    """Mandatory receipt terminal_count_status / terminal_nonwriters.

    Terminal counts positively verified still-present SZOMB identities
    accepted by the ESRCH-only terminal rule, never Gone rows."""

    def test_strict_clean_round_valid_zero(self):
        table = FakeTable([row(101), row(200)], {101: 100, 200: 7})
        result = census(table, prime=True).round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 1, 0))
        self.assertEqual(result["terminal_count_status"], "valid")
        self.assertEqual(result["terminal_nonwriters"], 0)
        self.assertIs(type(result["terminal_nonwriters"]), int)
        self.assertNotIn("terminal_witnesses", result)

    def test_gone_never_counts_even_allowlisted(self):
        for allowed in (False, True):
            table = FakeTable([row(103)], {103: ProcessLookupError(3, "gone")}, reread={103: None})
            result = census(table, terminal_allowed=allowed, prime=True).round()
            self.assertEqual((result["certain"], result["uncertain"]), (True, 0))
            self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                             ("valid", 0))
            self.assertNotIn("terminal_witnesses", result)
            self.assertEqual(table.reads, 1)

    def test_terminal_zombie_allowlisted(self):
        table = FakeTable([zombie_row(140, 5)], {140: ProcessLookupError(3, "gone")})
        check = census(table, terminal_allowed=True, prime=True)
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("valid", 1))
        self.assertEqual(table.reads, 21)
        witness = result["terminal_witnesses"]
        self.assertEqual(len(witness), 1)
        self.assertEqual(witness[0], {"pid": 140, "start": 5, "snapshot_zombie": True,
                                      "first_zombie": True, "tracked": False})

    def test_terminal_zombie_strict_blocks_but_count_known_zero(self):
        # Strict mode can never accept terminal, so the count is
        # accountably known 0 even though the zombie row still blocks.
        table = FakeTable([zombie_row(141, 5)], {141: ProcessLookupError(3, "gone")})
        result = census(table, prime=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("valid", 0))
        self.assertNotIn("terminal_witnesses", result)

    def test_live_snapshot_unrelated_valid_zero(self):
        # A live snapshot was never a terminal candidate: its uncertainty
        # is unrelated to terminal accounting, so the accountable count
        # stays valid 0 while the round still blocks.
        table = FakeTable([row(142, start=5)], {142: ProcessLookupError(3, "gone")})
        result = census(table, terminal_allowed=True, prime=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("valid", 0))
        self.assertNotIn("terminal_witnesses", result)

    def test_first_read_failure_no_terminal(self):
        def flaky(reads):
            if reads == 1:
                raise PermissionError(13, "denied")
            return zombie_row(143, 5)
        table = FakeTable([zombie_row(143, 5)], {143: ProcessLookupError(3, "gone")},
                           reread={143: flaky})
        result = census(table, terminal_allowed=True, prime=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("invalid", None))

    def test_final_failure_no_terminal(self):
        table = FakeTable([zombie_row(144, 5)], {144: ProcessLookupError(3, "gone")},
                           reread={144: lambda reads: zombie_row(144, 5) if reads <= 20
                           else (_ for _ in ()).throw(PermissionError(13, "denied"))})
        result = census(table, terminal_allowed=True, prime=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("invalid", None))

    def test_tracked_member_terminal_recomputed(self):
        table = FakeTable([row(145, start=5)], {145: 100})
        check = census(table, terminal_allowed=True, prime=True)
        self.assertEqual(check.round()["members"], 1)
        table.rows = [zombie_row(145, 5)]
        table.sessions = {145: ProcessLookupError(3, "gone")}
        table.reads = 0
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("valid", 1))
        self.assertIn((145, 5), check.tracked)
        self.assertTrue(result["terminal_witnesses"][0]["tracked"])
        table.reads = 0
        again = check.round()  # recomputed, no exemption cache
        self.assertEqual((again["terminal_count_status"], again["terminal_nonwriters"]),
                         ("valid", 1))

    def test_non_esrch_unrelated_valid_zero(self):
        # A non-ESRCH failure can never qualify as terminal, but it is
        # unrelated to terminal accounting: valid 0 with blocking uncertainty.
        table = FakeTable([zombie_row(146, 5)], {146: PermissionError(1, "denied")})
        result = census(table, terminal_allowed=True, prime=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("valid", 0))
        self.assertNotIn("terminal_witnesses", result)

    def test_intermediate_failure_disqualifies(self):
        # Read 1 matches SZOMB, read 2 fails, final matches: the actually
        # performed contradiction disqualifies terminal even though the
        # final read is same-SZOMB.
        def flaky(reads):
            if reads == 2:
                raise OSError(errno.EIO, "injected I/O failure")
            return zombie_row(148, 5)
        table = FakeTable([zombie_row(148, 5)], {148: ProcessLookupError(3, "gone")},
                           reread={148: flaky})
        result = census(table, terminal_allowed=True, prime=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("invalid", None))
        self.assertNotIn("terminal_witnesses", result)

    def test_intermediate_live_disqualifies(self):
        # An actually performed intermediate live observation of the same
        # identity contradicts terminal even though every other read matches.
        def mixed(reads):
            if reads == 2:
                return row(149, start=5)
            return zombie_row(149, 5)
        table = FakeTable([zombie_row(149, 5)], {149: ProcessLookupError(3, "gone")},
                           reread={149: mixed})
        result = census(table, terminal_allowed=True, prime=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("invalid", None))

    def test_final_live_disqualifies(self):
        table = FakeTable([zombie_row(150, 5)], {150: ProcessLookupError(3, "gone")},
                           reread={150: lambda reads: zombie_row(150, 5) if reads <= 20
                           else row(150, start=5)})
        result = census(table, terminal_allowed=True, prime=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("invalid", None))
        self.assertNotIn("terminal_witnesses", result)

    def test_tracked_member_failed_reads_forces_uncertain(self):
        # Integration: an invalid terminal count forces uncertain>=1 and
        # certain False through the unchanged IPC fields, even though the
        # tracked row stays a member.
        def flaky(reads):
            if reads == 2:
                raise OSError(errno.EPERM, "injected failure")
            return zombie_row(151, 5)
        table = FakeTable([row(151, start=5)], {151: 100})
        check = census(table, terminal_allowed=True, prime=True)
        self.assertEqual(check.round()["members"], 1)
        table.rows = [zombie_row(151, 5)]
        table.sessions = {151: ProcessLookupError(3, "gone")}
        table.reads, table.rereads = 0, {151: flaky}
        result = check.round()
        self.assertEqual(result["members"], 1)  # tracked attribution kept
        self.assertEqual(result["uncertain"], 1)
        self.assertFalse(result["certain"])
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("invalid", None))
        self.assertNotIn("terminal_witnesses", result)

    def test_non_esrch_errno_not_gone(self):
        # Only an actual errno-ESRCH failure takes the Gone branch; any
        # other ProcessLookupError errno stays on the non-ESRCH path.
        table = FakeTable([zombie_row(152, 5)], {152: ProcessLookupError(1, "denied")})
        result = census(table, terminal_allowed=True, prime=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertIn("getsid", result["uncertain_rows"][0]["reason"])
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("valid", 0))

    def test_replacement_is_gone_not_terminal(self):
        table = FakeTable([zombie_row(147, 5)], {147: ProcessLookupError(3, "gone")},
                           reread={147: lambda reads: zombie_row(147, 5) if reads == 1
                           else zombie_row(147, 9)})
        result = census(table, terminal_allowed=True, prime=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("invalid", None))
        self.assertNotIn("terminal_witnesses", result)

    def test_combined_budget_truncation_keeps_count(self):
        pids = list(range(150, 165))
        rows = [zombie_row(pid, pid) for pid in pids]
        sessions = {pid: ProcessLookupError(3, "gone") for pid in pids}
        table = FakeTable(rows, sessions)
        result = census(table, terminal_allowed=True, prime=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (True, 0))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("valid", 15))
        self.assertEqual(len(result["terminal_witnesses"]), 5)  # 15 diag + 15 witness > 20
        self.assertEqual(len(result["diagnostics"]["rows"]), 15)

    def test_unaccountable_round_invalid_null(self):
        # A zombie ESRCH row whose first confirmation read fails cannot be
        # adjudicated: the terminal count is unaccountable (null) with
        # blocking uncertainty, serialized as JSON null with its status.
        def flaky(reads):
            if reads == 1:
                raise PermissionError(13, "denied")
            return zombie_row(132, 5)
        table = FakeTable([zombie_row(132, start=5)], {132: ProcessLookupError(3, "gone")},
                           reread={132: flaky})
        result = census(table, terminal_allowed=True, prime=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertGreaterEqual(result["uncertain"], 1)
        self.assertEqual(result["terminal_count_status"], "invalid")
        self.assertIsNone(result["terminal_nonwriters"])
        self.assertNotIn("terminal_witnesses", result)
        blob = json.dumps(result, ensure_ascii=True, separators=(',', ':'))
        self.assertIn('"terminal_nonwriters":null', blob)

    def test_snapshot_failure_invalid_null(self):
        def broken():
            raise OSError(12, "injected")
        result = bounded.Census(100, snapshot=broken).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]), ("invalid", None))

    def test_helper_rejects_bool_and_overflow(self):
        self.assertEqual(bounded._terminal_fields(True, False, False), ("invalid", None))
        self.assertEqual(bounded._terminal_fields(0, False, False), ("valid", 0))
        self.assertEqual(bounded._terminal_fields(bounded.U64MAX, False, False), ("valid", bounded.U64MAX))
        self.assertEqual(bounded._terminal_fields(bounded.U64MAX + 1, False, False), ("overflow", None))
        self.assertEqual(bounded._terminal_fields(0, True, False), ("invalid", None))
        self.assertEqual(bounded._terminal_fields(3, True, False), ("invalid", None))
        self.assertEqual(bounded._terminal_fields(-1, False, False), ("invalid", None))

    def test_ipc_schema_unchanged(self):
        self.assertEqual(set(bounded.FIELDS["census"]), {"after", "members", "uncertain", "certain", "replacement_work"})
        self.assertNotIn("terminal_count_status", bounded.FIELDS["census"])
        self.assertNotIn("terminal_nonwriters", bounded.FIELDS["census"])


class PureProtocol(unittest.TestCase):
    def custodian(self):
        custodian = bounded.Custodian(["true"])
        custodian.run_id = "r"
        custodian.guard = bounded.Guard(0)
        read_end, custodian.cmd_w = os.pipe()
        self.addCleanup(os.close, read_end)
        self.addCleanup(os.close, custodian.cmd_w)
        custodian.leader = 4242
        return custodian

    def message(self, custodian, kind, leader=None, **fields):
        if kind == "census":
            fields.setdefault("replacement_work", fixture_work())
        return {"run": "r", "seq": custodian.monitor_seq + 1, "leader": leader, "type": kind, "at": 1, **fields}

    def test_wrong_run_and_sequence_gap(self):
        custodian = self.custodian()
        custodian.accept(dict(self.message(custodian, "ready"), run="other"))
        self.assertIn("protocol", custodian.guard.fault)
        custodian = self.custodian()
        custodian.accept(dict(self.message(custodian, "ready"), seq=5))
        self.assertIn("protocol", custodian.guard.fault)

    def test_leader_identity_binding(self):
        custodian = self.custodian()
        custodian.accept(self.message(custodian, "output_eof", leader=[4242, 77], stream="stdout"))
        self.assertIsNone(custodian.guard.fault)
        custodian.accept(self.message(custodian, "output_eof", leader=None, stream="stderr"))
        self.assertIn("dropped", custodian.guard.fault)
        custodian = self.custodian()
        custodian.accept(self.message(custodian, "output_eof", leader=[4242, 77], stream="stdout"))
        custodian.accept(self.message(custodian, "output_eof", leader=[4242, 78], stream="stderr"))
        self.assertIn("not the pinned leader", custodian.guard.fault)
        custodian = self.custodian()
        custodian.accept(self.message(custodian, "output_eof", leader=[999, 1], stream="stdout"))
        self.assertIn("not the pinned leader", custodian.guard.fault)

    def test_ready_needs_received_readings(self):
        custodian = self.custodian()
        custodian.paths = {"owned": False}  # setup done; readings missing
        custodian.accept(self.message(custodian, "ready"))
        self.assertIn("ready before completed readings", custodian.guard.fault)

    def test_census_before_reap_is_ignored(self):
        custodian = self.custodian()
        custodian.reaped_seq = 10
        custodian.accept(self.message(custodian, "census", after=9, certain=True, members=0, uncertain=0))
        self.assertFalse(custodian.census_ok)
        custodian.accept(self.message(custodian, "census", after=10, certain=True, members=0, uncertain=0))
        self.assertTrue(custodian.census_ok)

    def test_uncertain_census_latches_over_later_empty(self):
        # Review 6312725112 finding 3: an uncertain census after the stop
        # latches; a later certain empty census never restores closure.
        custodian = self.custodian()
        custodian.reaped_seq = 10
        custodian.eof = {"stdout", "stderr"}
        custodian.accept(self.message(custodian, "census", after=10, certain=False, members=0, uncertain=1))
        custodian.accept(self.message(custodian, "census", after=10, certain=True, members=0, uncertain=0))
        self.assertIn("census uncertain", custodian.guard.fault)
        self.assertFalse(custodian.census_ok)
        self.assertFalse(custodian.closure(True))

    def test_malformed_fields_latch_instead_of_raising(self):
        # Review 6318598212: missing or ill-typed fields latch a protocol fault.
        cases = [
            {"type": "census"},  # every census field missing
            {"type": "census", "after": 1, "members": 0, "uncertain": 0, "certain": "yes"},
            {"type": "census", "after": True, "members": 0, "uncertain": 0, "certain": True},
            {"type": "output_eof", "stream": "bogus"},
            {"type": "output_eof"},
            {"type": "admission_waiting"},
            {"type": "admission_waiting", "waited": 1},
            {"type": "reading", "kind": "reserve", "value": "big"},
            {"type": "reading", "kind": "scratch", "value": 1},
            {"type": "reading", "kind": "other", "value": 1},
            {"type": "ready", "leader": "abc"},
            {"type": "ready", "at": "now"},
            {"type": None},
        ]
        for fields in cases:
            custodian = self.custodian()
            custodian.reaped_seq = 0
            custodian.accept(dict(self.message(custodian, "ready"), **fields))
            self.assertIn("protocol: malformed", custodian.guard.fault or "", fields)
            self.assertFalse(custodian.census_ok)
            self.assertEqual(custodian.eof, set())
        custodian = self.custodian()
        custodian.accept(["not", "an", "object"])
        self.assertIn("protocol: malformed", custodian.guard.fault)

    def test_report_never_blocks_on_a_full_pipe(self):
        read_end, write_end = os.pipe()
        self.addCleanup(os.close, read_end)
        self.addCleanup(os.close, write_end)
        os.set_blocking(write_end, False)
        try:
            while True:
                os.write(write_end, bytes(65536))
        except BlockingIOError:
            pass
        os.set_blocking(write_end, True)  # a blocking write would now hang
        start = now_ns()
        bounded.report("x" * 10000, fd=write_end)
        self.assertLess(now_ns() - start, NS)

    # Review 6319610400: the exception path latches first and is filesystem-free.

    def raised(self, error):
        def innermost():
            raise error
        try:
            innermost()
        except BaseException as caught:
            return caught

    def test_exception_latches_without_source_lookup(self):
        import builtins, linecache, traceback
        calls = []

        def forbidden(name):
            def call(*_args, **_kwargs):
                calls.append(name)
                raise AssertionError(f"{name} called")
            return call

        targets = [(linecache, n) for n in ("getline", "getlines", "checkcache", "lazycache", "updatecache")]
        targets += [(traceback, n) for n in ("extract_tb", "format_exception", "format_tb", "print_exc")]
        targets += [(traceback.StackSummary, "extract"), (builtins, "open"), (os, "stat"), (os, "lstat")]
        error = self.raised(ValueError("boom"))
        custodian = self.custodian()
        records = []
        custodian.record = lambda kind, **fields: records.append((kind, fields))
        saved = [(owner, name, getattr(owner, name)) for owner, name in targets]
        try:
            for owner, name in targets:
                setattr(owner, name, forbidden(name))
            custodian.exception(error)
        finally:
            for owner, name, value in saved:
                setattr(owner, name, value)
        self.assertEqual(calls, [])
        self.assertEqual(custodian.guard.fault, "custodian exception: ValueError")
        self.assertEqual(records[0][1]["type"], "ValueError")
        self.assertTrue(records[0][1]["where"].startswith("innermost:"), records)

    def test_exception_latches_when_diagnostics_raise(self):
        custodian = self.custodian()

        def broken(*_args, **_kwargs):
            raise RuntimeError("diagnostics failed")

        custodian.record = broken
        custodian.exception(self.raised(OSError(9, "bad")))  # must not raise
        self.assertEqual(custodian.guard.fault, "custodian exception: OSError")
        custodian.exception(self.raised(KeyError("second")))
        self.assertEqual(custodian.guard.fault, "custodian exception: OSError")  # first fault kept

    def test_hostile_exception_text_is_never_converted(self):
        touched = []

        class HostileName(str):
            def __str__(self):
                touched.append("name str")
                raise AssertionError

            def __format__(self, spec):
                touched.append("name format")
                raise AssertionError

            def __radd__(self, other):
                touched.append("name radd")
                raise AssertionError

        class Meta(type):
            @property
            def __name__(cls):
                touched.append("metaclass name")
                raise AssertionError

        class Hostile(Exception, metaclass=Meta):
            def __str__(self):
                touched.append("str")
                raise AssertionError

            def __repr__(self):
                touched.append("repr")
                raise AssertionError

            def __format__(self, spec):
                touched.append("format")
                raise AssertionError

        type.__dict__["__name__"].__set__(Hostile, HostileName("Hostile"))
        custodian = self.custodian()
        records = []
        custodian.record = lambda kind, **fields: records.append(fields)
        custodian.exception(self.raised(Hostile()))
        self.assertEqual(touched, [])
        self.assertEqual(custodian.guard.fault, "custodian exception: Hostile")
        self.assertIs(type(custodian.guard.fault), str)
        self.assertEqual(json.loads(json.dumps(records))[0]["type"], "Hostile")

    def test_custodian_setup_is_filesystem_free(self):
        # Review 6320463419: blocking setup runs in the monitor, inside the
        # setup deadline; the custodian's own setup makes no filesystem call.
        import builtins
        calls = []

        def forbidden(name):
            def call(*_args, **_kwargs):
                calls.append(name)
                raise AssertionError(f"{name} called")
            return call

        targets = [(os.path, n) for n in ("realpath", "isdir", "exists", "lexists")]
        targets += [(os, n) for n in ("mkdir", "makedirs", "open", "stat", "lstat", "statvfs", "scandir", "listdir")]
        targets += [(tempfile, "mkdtemp"), (builtins, "open"), (bounded.Kinfo, "check"), (bounded.Kinfo, "sysctl")]
        saved = [(owner, name, getattr(owner, name)) for owner, name in targets]
        handlers = {n: signal.getsignal(n) for n in (signal.SIGINT, signal.SIGTERM, signal.SIGCHLD)}
        custodian = bounded.Custodian(["true"], root_parent="/private/tmp/does-not-matter")
        try:
            for owner, name in targets:
                setattr(owner, name, forbidden(name))
            custodian.setup()
        finally:
            for owner, name, value in saved:
                setattr(owner, name, value)
            for number, handler in handlers.items():
                signal.signal(number, handler)
        self.assertEqual(calls, [])
        self.assertIsNone(custodian.paths)
        self.assertNotIn("root", custodian.ctx)
        self.assertEqual(custodian.absolute - custodian.start, int(custodian.limit_s * NS))

    def test_ready_before_setup_faults(self):
        custodian = self.custodian()
        custodian.paths = None
        custodian.guard.reserve_at = custodian.guard.scratch_at = 1
        custodian.accept(self.message(custodian, "ready"))
        self.assertIn("ready before setup", custodian.guard.fault)

    def test_unpinned_target_is_never_signalled(self):
        custodian = self.custodian()
        custodian.leader_status = 0  # reaped: the pin is gone
        records = []
        custodian.record = lambda kind, **fields: records.append(fields)
        custodian.signal_pinned(4242, signal.SIGKILL, group=True)
        custodian.signal_pinned(5555, signal.SIGKILL, group=True)
        self.assertEqual([r["result"] for r in records], ["refused: not a pinned child"] * 2)
        self.assertEqual([r["held_unreaped"] for r in records], [False, False])


# Admission grants (oxide#192) ---------------------------------------------------


class PureAdmission(unittest.TestCase):
    """The bounded admission-report protocol: a completed wait grants bounded
    time, a waiting report never does, and each command's wait is paired only
    with that command's own completion (oxide#192)."""

    RUN = "r"

    def pipe(self):
        read_end, write_end = os.pipe()
        os.set_blocking(read_end, False)
        self.addCleanup(os.close, read_end)
        return read_end, write_end

    def drain(self, read_end):
        lines, buffer = [], b""
        while True:
            try:
                chunk = os.read(read_end, 65536)
            except BlockingIOError:
                break
            if not chunk:
                break
            buffer += chunk
        for line in buffer.splitlines():
            lines.append(json.loads(line))
        return lines

    def custodian(self, limit=300, start=1000 * NS):
        custodian = bounded.Custodian(["true"], limit_s=limit)
        custodian.run_id = self.RUN
        custodian.guard = bounded.Guard(start)
        custodian.cmd_r, custodian.cmd_w = self.pipe()
        custodian.leader = 4242
        custodian.start = start
        custodian.absolute = start + int(limit * NS)
        custodian.stop_at = custodian.absolute - bounded.STOP_RESERVE_S * NS
        custodian.exit_at = custodian.absolute - bounded.EXIT_MARGIN_S * NS
        return custodian

    def message(self, custodian, waited_ns, offset_s=1, command="c1", kind="admission"):
        # The send stamp is the patched clock's, so the future-stamp rule is
        # exercised separately from the grant arithmetic.
        at = custodian.start + int(offset_s * NS)
        return {"run": self.RUN, "seq": custodian.monitor_seq + 1, "leader": None, "type": kind,
                "at": at, "command": command,
                "waited_ns": waited_ns if kind == "admission" else 0,
                "reason": "queue" if kind == "admission_waiting" else None,
                "completed": at, "marker": 1}

    def clock(self, custodian, offset_s):
        """Drive the custodian's own CLOCK_UPTIME_RAW seam at a fixed offset."""
        return patched(bounded, "now_ns", lambda: custodian.start + int(offset_s * NS))

    def waiting(self, custodian, offset_s, command="c1"):
        message = self.message(custodian, 0, offset_s, command, "admission_waiting")
        message.pop("waited_ns")
        return message

    def pairs(self, custodian):
        return custodian.admission_pairs_summary()

    # Grants -----------------------------------------------------------------

    def test_a_reported_wait_moves_every_deadline_by_exactly_that_wait(self):
        custodian = self.custodian()
        with self.clock(custodian, 150):
            custodian.accept(self.message(custodian, 120 * NS, offset_s=150))
        self.assertIsNone(custodian.guard.fault)
        self.assertEqual(custodian.admission_granted, 120 * NS)
        self.assertEqual(custodian.absolute, custodian.start + 420 * NS)
        self.assertEqual(custodian.stop_at, custodian.start + 400 * NS)
        self.assertEqual(custodian.exit_at, custodian.start + 418 * NS)
        self.assertFalse(custodian.deadline_hit)
        os.close(custodian.cmd_w)
        records = self.drain(custodian.cmd_r)
        grants = [r["record"] for r in records if r["record"]["event"] == "admission"]
        self.assertEqual([(g["command"], g["waited_ns"], g["grant_ns"], g["clamped"]) for g in grants],
                         [("c1", 120 * NS, 120 * NS, False)])

    def test_a_wait_never_exceeds_this_runs_own_elapsed_time(self):
        custodian = self.custodian()
        with self.clock(custodian, 50):
            custodian.accept(self.message(custodian, 10_000 * NS, offset_s=50))
        self.assertEqual(custodian.admission_granted, 50 * NS)
        self.assertTrue(custodian.admission_clamped)
        self.assertEqual(custodian.absolute, custodian.start + 350 * NS)

    def test_the_total_grant_never_passes_the_fixed_cap(self):
        custodian = self.custodian()
        custodian.record = lambda kind, **fields: None  # one grant record is already proved
        elapsed = 0
        for _ in range(10):  # every report is fully backed by elapsed time
            elapsed += 200
            with self.clock(custodian, elapsed):
                custodian.accept(self.message(custodian, 200 * NS, offset_s=elapsed, command=f"c{elapsed}"))
        self.assertEqual(custodian.admission_granted, bounded.ADMISSION_CAP_NS)
        self.assertTrue(custodian.admission_capped)
        self.assertEqual(custodian.absolute, custodian.start + 300 * NS + bounded.ADMISSION_CAP_NS)
        with self.clock(custodian, elapsed + 1):
            custodian.accept(self.message(custodian, 1 * NS, offset_s=elapsed + 1, command="last"))
        self.assertEqual(custodian.admission_granted, bounded.ADMISSION_CAP_NS)
        self.assertEqual(custodian.admission_counted, 11)
        self.assertEqual(custodian.exit_at, custodian.absolute - bounded.EXIT_MARGIN_S * NS)

    def test_nothing_is_ever_shortened(self):
        custodian = self.custodian()
        before = (custodian.absolute, custodian.stop_at, custodian.exit_at)
        with self.clock(custodian, 10):
            custodian.accept(self.message(custodian, 0, offset_s=10))
        self.assertEqual((custodian.absolute, custodian.stop_at, custodian.exit_at), before)
        self.assertEqual(custodian.admission_granted, 0)

    def test_a_grant_after_the_stop_decision_changes_nothing(self):
        custodian = self.custodian()
        custodian.stopped_at = custodian.start + 2 * NS
        before = (custodian.absolute, custodian.stop_at, custodian.exit_at)
        with self.clock(custodian, 3):
            custodian.accept(self.message(custodian, 60 * NS, offset_s=3))
        self.assertEqual((custodian.absolute, custodian.stop_at, custodian.exit_at), before)
        self.assertEqual(custodian.admission_granted, 0)
        self.assertEqual(custodian.admission_ignored, 1)
        self.assertEqual(custodian.admission_counted, 0)
        self.assertEqual(custodian.admission_late_reason, "stopped")
        self.assertEqual(custodian.admission_late_ns, custodian.start + 3 * NS)

    def test_a_grant_after_a_deadline_hit_changes_nothing(self):
        custodian = self.custodian()
        custodian.deadline_hit = True
        before = custodian.absolute
        with self.clock(custodian, 3):
            custodian.accept(self.message(custodian, 60 * NS, offset_s=3))
        self.assertEqual(custodian.absolute, before)
        self.assertEqual(custodian.admission_granted, 0)
        self.assertEqual(custodian.admission_ignored, 1)
        self.assertEqual(custodian.admission_late_reason, "deadline")

    def test_a_late_grant_is_recorded_with_its_reason_and_decision_facts(self):
        custodian = self.custodian()
        custodian.deadline_hit = True
        recorded = []
        custodian.record = lambda kind, **fields: recorded.append(fields)
        with self.clock(custodian, 300):
            custodian.accept(self.message(custodian, 126_900_000_000, offset_s=300))
        self.assertEqual(len(recorded), 1)
        self.assertEqual((recorded[0]["reason"], recorded[0]["deadline_hit"], recorded[0]["stopped"]),
                         ("deadline", True, False))
        self.assertEqual(recorded[0]["waited_ns"], 126_900_000_000)
        self.assertEqual(recorded[0]["at"], custodian.start + 300 * NS)
        custodian.cmd_r, custodian.cmd_w = self.pipe()
        custodian.observe_admission(True)
        os.close(custodian.cmd_w)
        decision = [record["record"] for record in self.drain(custodian.cmd_r)
                    if record["record"]["event"] == "admission_decision"][0]
        self.assertEqual(decision["ignored"], 1)
        self.assertEqual(decision["late_reason"], "deadline")
        self.assertEqual(decision["late_ns"], custodian.start + 300 * NS)
        self.assertEqual(decision["granted_ns"], 0)
        self.assertEqual(decision["counted"], 0)
        self.assertEqual(decision["absolute_ns"], custodian.start + 300 * NS)

    def test_a_future_stamped_grant_latches_and_grants_nothing(self):
        custodian = self.custodian()
        future = now_ns() + 60 * NS
        custodian.accept(self.message(custodian, 60 * NS, offset_s=1) | {"at": future})
        self.assertIn("future", custodian.guard.fault or "")
        self.assertEqual(custodian.admission_granted, 0)

    def test_grant_records_are_capped_and_never_fault(self):
        custodian = self.custodian()
        recorded = []
        custodian.record = lambda kind, **fields: recorded.append(fields)
        elapsed = 0
        for _ in range(bounded.ADMISSION_RECORDS + 5):
            elapsed += 1
            with self.clock(custodian, elapsed):
                custodian.accept(self.message(custodian, NS, offset_s=elapsed, command=f"c{elapsed}"))
        self.assertIsNone(custodian.guard.fault)
        self.assertEqual(len(recorded), bounded.ADMISSION_RECORDS)
        self.assertEqual(custodian.admission_granted, (bounded.ADMISSION_RECORDS + 5) * NS)

    def test_the_decision_record_is_best_effort_and_never_latches(self):
        custodian = self.custodian()
        with self.clock(custodian, 90):
            custodian.accept(self.message(custodian, 60 * NS, offset_s=90))
        custodian.cmd_w = -1  # a failed write is dropped, never a fault
        custodian.observe_admission(True)
        self.assertIsNone(custodian.guard.fault)
        custodian = self.custodian()
        custodian.cmd_w = None
        custodian.observe_admission(False)  # nothing to write to, nothing raised
        self.assertIsNone(custodian.guard.fault)

    # The stop boundary ------------------------------------------------------

    def test_a_marker_pending_at_the_stop_is_drained_before_the_deadline(self):
        custodian = self.custodian(limit=300)
        state = {"t": custodian.stop_at}
        pending = [self.message(custodian, 60 * NS, offset_s=280)]
        calls = []

        def pump(timeout):
            calls.append(timeout)
            state["t"] += 1
            if pending:
                custodian.accept(pending.pop())
            elif len(calls) == 2:  # no marker left: run out the (moved) stop
                state["t"] = custodian.stop_at

        custodian.pump = pump
        with patched(bounded, "now_ns", lambda: state["t"]):
            custodian.watch()
        self.assertIn(0, calls, "the stop instant did not drain the control pipe")
        self.assertTrue(custodian.deadline_hit)
        self.assertEqual(custodian.admission_granted, 60 * NS)
        self.assertEqual(custodian.admission_ignored, 0)
        self.assertEqual(custodian.stop_at, custodian.start + 340 * NS)

    def test_a_grant_that_cannot_move_the_stop_cannot_spin_the_watch_loop(self):
        custodian = self.custodian(limit=300)
        state = {"t": custodian.stop_at}
        pending = [self.message(custodian, 0, offset_s=280)]
        calls = []

        def pump(timeout):
            calls.append(timeout)
            state["t"] += 1
            if pending:
                custodian.accept(pending.pop())

        custodian.pump = pump
        with patched(bounded, "now_ns", lambda: state["t"]):
            custodian.watch()
        self.assertEqual(calls, [0])
        self.assertTrue(custodian.deadline_hit)
        self.assertEqual(custodian.admission_granted, 0)

    # Waiting reports and per-command pairing --------------------------------

    def test_a_waiting_report_never_moves_a_deadline_and_is_recorded(self):
        custodian = self.custodian()
        recorded = []
        custodian.record = lambda kind, **fields: recorded.append(fields)
        before = (custodian.absolute, custodian.stop_at, custodian.exit_at)
        with self.clock(custodian, 5):
            custodian.accept(self.waiting(custodian, 5))
        self.assertEqual((custodian.absolute, custodian.stop_at, custodian.exit_at), before)
        self.assertEqual((custodian.admission_waiting_reports, custodian.admission_waiting_reason),
                         (1, "queue"))
        self.assertEqual(custodian.admission_waiting_ns, custodian.start + 5 * NS)
        self.assertEqual(recorded[0]["after_stop"], False)
        self.assertEqual(recorded[0]["command"], "c1")
        custodian.stopped_at = custodian.start + 6 * NS
        with self.clock(custodian, 7):
            custodian.accept(self.waiting(custodian, 7, command="c2"))
        self.assertEqual((custodian.absolute, custodian.stop_at, custodian.exit_at), before)
        self.assertEqual(custodian.admission_waiting_reports, 2)
        self.assertEqual(custodian.admission_waiting_reason, "queue", "the first reason is kept")
        self.assertEqual(recorded[1]["after_stop"], True)
        self.assertEqual(custodian.admission_granted, 0)
        self.assertEqual(custodian.admission_counted, 0)

    def test_each_command_pairs_its_own_waiting_and_completion(self):
        # Review finding: only a command's own terminal report ends its wait, and
        # an unpaired wait is never called finished or unfinished. A zero-duration
        # or late grant ends the wait; a waiting report for one command never pairs
        # with the completion of another.
        custodian = self.custodian()
        custodian.record = lambda kind, **fields: None
        with self.clock(custodian, 10):
            custodian.accept(self.waiting(custodian, 10, command="ended"))
        with self.clock(custodian, 20):
            custodian.accept(self.message(custodian, 25 * NS, offset_s=20, command="ended"))
        with self.clock(custodian, 30):
            custodian.accept(self.waiting(custodian, 30, command="zero"))
        with self.clock(custodian, 40):
            custodian.accept(self.message(custodian, 0, offset_s=40, command="zero"))
        with self.clock(custodian, 50):
            custodian.accept(self.waiting(custodian, 50, command="lateonly"))
        custodian.deadline_hit = True
        with self.clock(custodian, 51):
            custodian.accept(self.message(custodian, 5 * NS, offset_s=51, command="lateonly"))
        # Nothing later was observed, but a completion can still have been missed
        # between the last poll and the sampler's stop, so the wait stays unpaired
        # and unclassified rather than proven unfinished.
        with self.clock(custodian, 60):
            custodian.accept(self.waiting(custodian, 60, command="lost"))
        pairs = self.pairs(custodian)
        self.assertEqual(pairs["commands"], 4)
        self.assertEqual(pairs["ended"], 3)
        self.assertEqual(pairs["unpaired_count"], 1)
        self.assertEqual([entry["command"] for entry in pairs["unpaired"]], ["lost"])
        self.assertFalse(pairs["unpaired"][0]["later_observation"])
        self.assertEqual(pairs["possibly_overwritten_count"], 0)
        self.assertEqual(pairs["unpaired"][0]["reason"], "queue")
        self.assertEqual(pairs["unpaired"][0]["waiting_ns"], custodian.start + 60 * NS)

    def test_a_completion_overwritten_before_a_poll_is_not_a_proven_unfinished_wait(self):
        # Review finding: the completion file has one fixed name, so two
        # completions inside one 0.25 s poll leave only the later inode visible.
        # An unpaired wait is therefore an unknown (possibly overwritten, rejected
        # or missed), never a proven unfinished wait - including one that was the
        # run's last observation.
        custodian = self.custodian()
        custodian.record = lambda kind, **fields: None
        with self.clock(custodian, 10):
            custodian.accept(self.waiting(custodian, 10, command="first"))
        # The later command's completion replaces the fixed-name file before a
        # poll: only this grant is observed.
        with self.clock(custodian, 20):
            custodian.accept(self.message(custodian, 8 * NS, offset_s=20, command="second"))
        pairs = self.pairs(custodian)
        self.assertEqual(pairs["commands"], 1)
        self.assertEqual(pairs["ended"], 0)
        self.assertEqual(pairs["unpaired_count"], 1)
        self.assertEqual(pairs["possibly_overwritten_count"], 1)
        self.assertTrue(pairs["unpaired"][0]["later_observation"])
        self.assertEqual(pairs["unpaired"][0]["command"], "first")
        self.assertEqual(pairs["unpaired"][0]["waiting_ns"], custodian.start + 10 * NS)

    def test_a_rejected_read_after_a_wait_is_also_a_later_observation(self):
        # A rejected read still observed the fixed-name file: it may have
        # replaced the completion, so the earlier wait stays unclassified even
        # though no later report was accepted.
        custodian = self.custodian()
        custodian.record = lambda kind, **fields: None
        with self.clock(custodian, 10):
            custodian.accept(self.waiting(custodian, 10, command="first"))
        with self.clock(custodian, 11):
            custodian.accept({"run": self.RUN, "seq": custodian.monitor_seq + 1, "leader": None,
                              "type": "admission_rejected", "at": custodian.start + 11 * NS,
                              "kind": "admission", "completed": custodian.start + 11 * NS,
                              "reason": "unexpected keys"})
        pairs = self.pairs(custodian)
        self.assertEqual((pairs["unpaired_count"], pairs["possibly_overwritten_count"]), (1, 1))
        self.assertTrue(pairs["unpaired"][0]["later_observation"])
        self.assertEqual(pairs["rejected_reports"], 1)
        self.assertIsNone(custodian.guard.fault, "a rejected report is cooperative evidence, never a fault")

    def test_an_entirely_missed_completion_is_unclassified_not_unfinished(self):
        # No later observation exists, but a completion written between the last
        # poll and the sampler's stop can be missed entirely. Missing terminal
        # evidence is unknown: nothing here may be called a proven unfinished
        # wait, and the decision record says so.
        custodian = self.custodian()
        custodian.record = lambda kind, **fields: None
        with self.clock(custodian, 10):
            custodian.accept(self.waiting(custodian, 10, command="only"))
        pairs = self.pairs(custodian)
        self.assertEqual(pairs["unpaired_count"], 1)
        self.assertFalse(pairs["unpaired"][0]["later_observation"])
        self.assertNotIn("unfinished", pairs)

    def test_every_launch_refusal_prints_one_bounded_reason(self):
        # Papercut bc0d56cd: a 125 launch refusal must never be silent.
        custodian = self.custodian()
        custodian.monitor = 1234
        custodian.not_launched = "monitor setup failed or slow"
        custodian.signal_pinned = lambda pid, number, group: None
        custodian.reap = lambda pid: 0
        custodian.pump = lambda timeout: None
        captured = []
        with patched(bounded, "report", lambda text, fd=2: captured.append(text)):
            with patched(bounded, "now_ns", lambda: custodian.start):
                self.assertEqual(custodian.abandon_launch(), 125)
        self.assertEqual(len(captured), 1)
        self.assertIn("not launched", captured[0])
        self.assertIn("monitor setup failed or slow", captured[0])
        self.assertLessEqual(len(captured[0]), 4096)

    def test_the_monitor_reports_an_unexpected_failure_instead_of_exiting_silently(self):
        # An exception in the monitor used to exit silently, which reached the
        # custodian as an unexplained 125.
        captured, codes = [], []

        class BrokenMonitor:
            def __init__(self, *_args, **_kwargs):
                pass

            def run(self):
                raise RuntimeError("injected monitor failure")

        with patched(bounded, "Monitor", BrokenMonitor), \
                patched(bounded, "report", lambda text, fd=2: captured.append(text)), \
                patched(bounded.os, "_exit", lambda code: codes.append(code)):
            bounded.monitor_main({}, None, None, None, None)
        self.assertEqual(codes, [1])
        self.assertEqual(len(captured), 1)
        self.assertIn("monitor failed before completion", captured[0])
        self.assertIn("RuntimeError", captured[0])
        self.assertNotIn("Traceback", captured[0])

    def test_reading_freshness_is_not_latched_after_the_closure_decision(self):
        # Review finding: a completed, closed run was reported as exit 125 when
        # a reserve sample stalled after the decision, because the monitor stops
        # sampling as it publishes `monitor_complete`. The shutdown is not a
        # freshness window; a stale reading before the decision still latches.
        custodian = self.custodian()
        custodian.guard = bounded.Guard(custodian.start)
        custodian.guard.reading({"kind": "reserve", "value": 500 * bounded.GIB,
                                 "at": custodian.start, "completed": custodian.start})
        custodian.guard.reading({"kind": "scratch", "value": 0, "complete": True,
                                 "at": custodian.start, "completed": custodian.start})
        stale = custodian.start + 5 * bounded.NS
        with patched(bounded, "now_ns", lambda: stale):
            custodian.sampling_ended = False
            custodian.check_readings()
        self.assertIn("stale", custodian.guard.fault or "", "a stale reading before the decision must latch")
        custodian.guard.fault = None
        with patched(bounded, "now_ns", lambda: stale):
            custodian.sampling_ended = True
            custodian.check_readings()
        self.assertIsNone(custodian.guard.fault, "the shutdown must not latch freshness")

    def test_finish_ends_sampling_before_waiting_for_the_monitor(self):
        custodian = self.custodian()
        custodian.record = lambda kind, **fields: None
        custodian.tell = lambda kind, **fields: None
        custodian.pump = lambda timeout: None
        custodian.reap = lambda pid: 0
        custodian.signal_pinned = lambda pid, number, group: None  # never signal a live host PID
        custodian.monitor = 1234
        custodian.monitor_complete = True
        custodian.leader_status = 0
        custodian.census_ok = True
        custodian.eof = {"stdout", "stderr"}
        custodian.guard = bounded.Guard(custodian.start)
        custodian.guard.reading({"kind": "reserve", "value": 500 * bounded.GIB,
                                 "at": custodian.start, "completed": custodian.start})
        with patched(bounded, "now_ns", lambda: custodian.start):
            self.assertEqual(custodian.finish(True), 0)
        self.assertTrue(custodian.sampling_ended)
        self.assertIsNone(custodian.guard.fault)

    def test_a_completion_without_a_waiting_report_is_never_unfinished(self):
        custodian = self.custodian()
        custodian.record = lambda kind, **fields: None
        with self.clock(custodian, 10):
            custodian.accept(self.message(custodian, 8 * NS, offset_s=10, command="only-grant"))
        self.assertEqual(self.pairs(custodian)["commands"], 0, "only waiting reports are counted")
        self.assertEqual(self.pairs(custodian)["unpaired_count"], 0)

    def test_the_pairing_table_is_bounded_and_overflow_is_unclassified(self):
        custodian = self.custodian()
        custodian.record = lambda kind, **fields: None
        for index in range(bounded.ADMISSION_COMMANDS + 3):
            with self.clock(custodian, index):
                custodian.accept(self.waiting(custodian, index, command=f"c{index}"))
        self.assertEqual(custodian.admission_unclassified, 3)
        self.assertEqual(len(custodian.admission_pairs), bounded.ADMISSION_COMMANDS)
        pairs = self.pairs(custodian)
        # An overflow report still advances the ordinal, so the last *stored*
        # waiting report is not provably last: every unpaired wait here is a
        # possible overwrite, an unknown, and none is a proven unfinished wait.
        self.assertEqual(pairs["unpaired_count"], bounded.ADMISSION_COMMANDS)
        self.assertEqual(pairs["possibly_overwritten_count"], bounded.ADMISSION_COMMANDS)
        self.assertEqual(len(pairs["unpaired"]), bounded.ADMISSION_UNPAIRED)
        self.assertTrue(all(entry["later_observation"] for entry in pairs["unpaired"]))

    def test_the_decision_record_carries_the_waiting_and_pairing_facts(self):
        custodian = self.custodian()
        custodian.record = lambda kind, **fields: None
        with self.clock(custodian, 9):
            custodian.accept(self.waiting(custodian, 9, command="c9"))
        custodian.cmd_r, custodian.cmd_w = self.pipe()
        custodian.observe_admission(True)
        os.close(custodian.cmd_w)
        decision = [record["record"] for record in self.drain(custodian.cmd_r)
                    if record["record"]["event"] == "admission_decision"][0]
        self.assertEqual((decision["waiting_reports"], decision["waiting_reason"]), (1, "queue"))
        self.assertEqual(decision["waiting_ns"], custodian.start + 9 * NS)
        self.assertEqual(decision["admission_waiting"]["commands"], 1)
        self.assertEqual(decision["admission_waiting"]["ended"], 0)
        self.assertEqual(decision["admission_waiting"]["unpaired_count"], 1)
        self.assertFalse(decision["admission_waiting"]["unpaired"][0]["later_observation"])
        self.assertEqual(decision["unclassified"], 0)
        self.assertEqual(decision["granted_ns"], 0)
        self.assertEqual(decision["absolute_ns"], custodian.start + 300 * NS)

    # Documents --------------------------------------------------------------

    def test_malformed_marker_matrix(self):
        valid = {"schema": 2, "source": "oxide", "run": self.RUN, "command": "4242-0", "waited_ns": 5}
        self.assertIsNone(bounded.admission_malformed(valid, self.RUN))
        self.assertIsNone(bounded.admission_malformed(dict(valid, waited_ns=bounded.U64MAX), self.RUN))
        cases = [
            ([], "not an object"), ("marker", "not an object"), (1, "not an object"),
            ({}, "unexpected keys"),
            ({key: valid[key] for key in ("schema", "source", "run", "command")}, "unexpected keys"),
            (dict(valid, extra=1), "unexpected keys"),
            (dict(valid, schema=1), "unsupported schema"), (dict(valid, schema=3), "unsupported schema"),
            (dict(valid, schema="2"), "unsupported schema"), (dict(valid, schema=True), "unsupported schema"),
            (dict(valid, source=""), "ill-typed source"), (dict(valid, source="x" * 33), "ill-typed source"),
            (dict(valid, run="other"), "stale run"), (dict(valid, run=None), "stale run"),
            (dict(valid, command=""), "ill-typed command"), (dict(valid, command="x" * 33), "ill-typed command"),
            (dict(valid, command=1), "ill-typed command"),
            (dict(valid, waited_ns=-1), "ill-typed waited_ns"), (dict(valid, waited_ns=True), "ill-typed waited_ns"),
            (dict(valid, waited_ns=1.5), "ill-typed waited_ns"), (dict(valid, waited_ns=bounded.U64MAX + 1),
                                                                  "ill-typed waited_ns"),
        ]
        for marker, expected in cases:
            self.assertEqual(bounded.admission_malformed(marker, self.RUN), expected, marker)

    def test_waiting_report_validation_matrix(self):
        valid = {"schema": 2, "source": "oxide", "run": self.RUN, "command": "4242-0", "reason": "queue"}
        self.assertIsNone(bounded.admission_waiting_malformed(valid, self.RUN))
        self.assertIsNone(bounded.admission_waiting_malformed(dict(valid, reason="host limit"), self.RUN))
        cases = [
            ([], "not an object"), ({}, "unexpected keys"),
            ({key: valid[key] for key in ("schema", "source", "run", "command")}, "unexpected keys"),
            (dict(valid, waited_ns=1), "unexpected keys"),
            (dict(valid, schema=1), "unsupported schema"), (dict(valid, schema="2"), "unsupported schema"),
            (dict(valid, source=""), "ill-typed source"),
            (dict(valid, run="other"), "stale run"),
            (dict(valid, command=""), "ill-typed command"), (dict(valid, command=1), "ill-typed command"),
            (dict(valid, reason=""), "ill-typed reason"), (dict(valid, reason="x" * 33), "ill-typed reason"),
            (dict(valid, reason=1), "ill-typed reason"),
        ]
        for marker, expected in cases:
            self.assertEqual(bounded.admission_waiting_malformed(marker, self.RUN), expected, marker)

    # The monitor's readers --------------------------------------------------

    def monitor(self, directory):
        monitor = bounded.Monitor({"inject": set(), "run_id": self.RUN, "evidence": str(directory),
                                   "admission_path": os.path.join(str(directory), bounded.ADMISSION_NAME),
                                   "waiting_path": os.path.join(str(directory), bounded.WAITING_NAME)},
                                  None, None, None, None)
        monitor.cmd_r, monitor.ctl_w = self.pipe()
        monitor.receipt = {"admission": bounded.admission_record(),
                           "admission_waiting": bounded.admission_waiting_record()}
        return monitor

    def fixture(self):
        directory = Path(tempfile.mkdtemp(prefix="admission.", dir=str(SCRATCH)))
        self.addCleanup(shutil.rmtree, str(directory), True)
        return directory

    def publish(self, directory, payload, name=None):
        path = directory / (name or bounded.ADMISSION_NAME)
        with open(path, "wb") as handle:
            handle.write(payload if isinstance(payload, bytes) else json.dumps(payload).encode())
        return path

    def test_a_valid_marker_is_reported_once_with_its_own_completion_stamp(self):
        directory = self.fixture()
        monitor = self.monitor(directory)
        path = self.publish(directory, {"schema": 2, "source": "oxide", "run": self.RUN,
                                        "command": "4242-0", "waited_ns": 90 * NS})
        monitor.read_admission(str(path))
        os.close(monitor.ctl_w)
        sent = self.drain(monitor.cmd_r)
        self.assertEqual([m["type"] for m in sent], ["admission"])
        self.assertEqual((sent[0]["waited_ns"], sent[0]["command"]), (90 * NS, "4242-0"))
        self.assertIsNone(bounded.malformed(sent[0]))
        self.assertEqual(monitor.receipt["admission"]["observed"], 1)
        self.assertEqual(monitor.receipt["admission"]["reported_wait_ns"], 90 * NS)
        self.assertLessEqual(monitor.receipt["admission"]["first_ns"], monitor.receipt["admission"]["last_ns"])

    def test_a_stale_run_marker_is_rejected_without_a_message(self):
        # Review finding: a marker left by an earlier run must never be paired
        # with this one, whatever it claims.
        directory = self.fixture()
        monitor = self.monitor(directory)
        stale = self.publish(directory, {"schema": 2, "source": "oxide", "run": "another-run",
                                         "command": "4242-0", "waited_ns": 90 * NS})
        monitor.read_admission(str(stale))
        stale_waiting = self.publish(directory, {"schema": 2, "source": "oxide", "run": "another-run",
                                                 "command": "4242-0", "reason": "queue"},
                                     name=bounded.WAITING_NAME)
        monitor.read_admission(str(stale_waiting), None, "admission_waiting")
        sent = self.drain(monitor.cmd_r)
        self.assertEqual([m["type"] for m in sent], ["admission_rejected", "admission_rejected"])
        self.assertEqual(monitor.receipt["admission"]["ignored"], 1)
        self.assertIn("stale run", monitor.receipt["admission"]["rejections"])
        self.assertEqual(monitor.receipt["admission_waiting"]["ignored"], 1)
        self.assertIn("stale run", monitor.receipt["admission_waiting"]["rejections"])
        self.assertEqual(monitor.receipt["admission_waiting"]["observed"], 0)

    def test_an_ignored_marker_is_recorded_and_never_sent(self):
        directory = self.fixture()
        monitor = self.monitor(directory)
        oversized = self.publish(directory, b"{" + b" " * bounded.ADMISSION_MAX_BYTES)
        monitor.read_admission(str(oversized))
        malformed = self.publish(directory, b"not json")
        monitor.read_admission(str(malformed))
        wrong = self.publish(directory, {"schema": 1, "source": "oxide", "run": self.RUN,
                                         "command": "4242-0", "waited_ns": 1})
        monitor.read_admission(str(wrong))
        link = directory / "link"
        os.symlink(str(malformed), str(link))
        monitor.read_admission(str(link))
        # Every readable-but-invalid document is reported as a rejection (an
        # observation of the fixed-name file), never as an accepted marker; the
        # symlink and other non-document paths are recorded only.
        sent = self.drain(monitor.cmd_r)
        self.assertEqual([m["type"] for m in sent], ["admission_rejected"] * 3)
        record = monitor.receipt["admission"]
        self.assertEqual(record["observed"], 0)
        self.assertEqual(record["ignored"], 4)
        self.assertEqual(len(record["rejections"]), 4)
        self.assertEqual(record["reported_wait_ns"], 0)

    def test_the_rejection_list_is_bounded(self):
        directory = self.fixture()
        monitor = self.monitor(directory)
        bad = self.publish(directory, b"not json")
        for _ in range(bounded.ADMISSION_REJECTIONS + 3):
            monitor.read_admission(str(bad))
        record = monitor.receipt["admission"]
        self.assertEqual(record["ignored"], bounded.ADMISSION_REJECTIONS + 3)
        self.assertEqual(len(record["rejections"]), bounded.ADMISSION_REJECTIONS)
        sent = self.drain(monitor.cmd_r)
        self.assertEqual([m["type"] for m in sent], ["admission_rejected"] * (bounded.ADMISSION_REJECTIONS + 3))

    def test_an_absent_marker_is_not_an_event(self):
        directory = self.fixture()
        monitor = self.monitor(directory)
        monitor.read_admission(str(directory / bounded.ADMISSION_NAME))
        record = monitor.receipt["admission"]
        self.assertEqual((record["observed"], record["ignored"], record["errors"]), (0, 1, 0))

    def test_the_waiting_reader_reports_its_own_message_and_record(self):
        directory = self.fixture()
        monitor = self.monitor(directory)
        path = self.publish(directory, {"schema": 2, "source": "oxide", "run": self.RUN,
                                        "command": "4242-3", "reason": "queue"},
                            name=bounded.WAITING_NAME)
        monitor.read_admission(str(path), None, "admission_waiting")
        os.close(monitor.ctl_w)
        sent = self.drain(monitor.cmd_r)
        self.assertEqual([m["type"] for m in sent], ["admission_waiting"])
        self.assertEqual((sent[0]["reason"], sent[0]["command"]), ("queue", "4242-3"))
        self.assertIsNone(bounded.malformed(sent[0]))
        waiting = monitor.receipt["admission_waiting"]
        self.assertEqual((waiting["observed"], waiting["reason"], waiting["ignored"]), (1, "queue", 0))
        self.assertEqual(monitor.receipt["admission"]["observed"], 0)
        self.assertEqual(monitor.receipt["admission"]["reported_wait_ns"], 0)

    def test_the_dedup_identity_is_the_file_actually_opened(self):
        # Review finding: a writer publishes by atomic rename. If the file is
        # replaced between the sampler's `lstat` and the read, dedup must bind to
        # the opened inode, not to the sampled one, or the replacement would be
        # reported now and again on the next poll (and could grant twice).
        directory = self.fixture()
        monitor = self.monitor(directory)
        path = self.publish(directory, {"schema": 2, "source": "oxide", "run": self.RUN,
                                        "command": "first", "waited_ns": 5 * NS})
        sampled = os.lstat(str(path))
        sampled_identity = (sampled.st_dev, sampled.st_ino)
        # Replace the file atomically before the read: the sampler's stat names
        # the old inode, the open sees the new one.
        self.publish(directory, {"schema": 2, "source": "oxide", "run": self.RUN,
                                 "command": "second", "waited_ns": 7 * NS},
                     name=bounded.ADMISSION_NAME + ".replacement")
        os.replace(str(directory / (bounded.ADMISSION_NAME + ".replacement")), str(path))
        consumed = monitor.read_admission(str(path), sampled_identity)
        info = os.lstat(str(path))
        self.assertEqual(consumed, (info.st_dev, info.st_ino),
                         "the read must report the identity it actually opened")
        self.assertNotEqual(consumed, sampled_identity)
        # Reporting the same opened identity again is a no-op.
        self.assertEqual(monitor.read_admission(str(path), consumed), consumed)
        os.close(monitor.ctl_w)
        sent = self.drain(monitor.cmd_r)
        self.assertEqual([m["type"] for m in sent], ["admission"])
        self.assertEqual(sent[0]["command"], "second")
        self.assertEqual(monitor.receipt["admission"]["observed"], 1)
        self.assertEqual(monitor.receipt["admission"]["identity"], list(consumed))

    def test_a_duplicate_completion_cannot_grant_the_same_wait_twice(self):
        # Belt and braces for the same race: even if a completion reaches the
        # custodian twice, one command grants its wait only once.
        custodian = self.custodian()
        custodian.record = lambda kind, **fields: None
        with self.clock(custodian, 10):
            custodian.accept(self.waiting(custodian, 10, command="dup"))
        # 15 s of reported wait, fully backed by the 20 s elapsed time.
        with self.clock(custodian, 20):
            custodian.accept(self.message(custodian, 15 * NS, offset_s=20, command="dup"))
        with self.clock(custodian, 21):
            custodian.accept(self.message(custodian, 15 * NS, offset_s=21, command="dup"))
        # A duplicate that arrives after a deadline is still a duplicate: it is
        # neither granted nor recorded as a late grant.
        custodian.deadline_hit = True
        with self.clock(custodian, 22):
            custodian.accept(self.message(custodian, 15 * NS, offset_s=22, command="dup"))
        self.assertEqual(custodian.admission_ignored, 0, "a duplicate is not a late grant")
        custodian.deadline_hit = False
        self.assertEqual(custodian.admission_granted, 15 * NS, "the second completion must not grant again")
        self.assertEqual(custodian.admission_counted, 1)
        self.assertEqual(custodian.admission_duplicates, 2, "both duplicates are counted, neither granted")
        custodian.cmd_r, custodian.cmd_w = self.pipe()
        custodian.observe_admission(True)
        os.close(custodian.cmd_w)
        decision = [record["record"] for record in self.drain(custodian.cmd_r)
                    if record["record"]["event"] == "admission_decision"][0]
        self.assertEqual(decision["duplicates"], 2)

    def test_an_overflowed_completion_never_grants(self):
        # Review finding: at table capacity admission_pair returns None, so the
        # custodian cannot tell a first terminal from a duplicate. An overflowed
        # command therefore grants nothing at all (a missed extension is safe, a
        # double grant is not), and the reports stay counted as unclassified.
        custodian = self.custodian()
        custodian.record = lambda kind, **fields: None
        for index in range(bounded.ADMISSION_COMMANDS):
            with self.clock(custodian, index):
                custodian.accept(self.waiting(custodian, index, command=f"c{index}"))
        before = custodian.absolute
        for offset in (100, 101):
            with self.clock(custodian, offset):
                custodian.accept(self.message(custodian, 15 * NS, offset_s=offset, command="overflow"))
        self.assertEqual(custodian.absolute, before, "an overflowed completion must not move the deadline")
        self.assertEqual(custodian.admission_granted, 0)
        self.assertEqual(custodian.admission_counted, 0)
        self.assertEqual(custodian.admission_duplicates, 0)
        self.assertGreaterEqual(custodian.admission_unclassified, 2)

    def test_a_rejected_waiting_report_is_recorded_without_a_message(self):
        directory = self.fixture()
        monitor = self.monitor(directory)
        bad = self.publish(directory, {"schema": 2, "source": "oxide", "run": self.RUN,
                                       "command": "4242-3", "reason": "queue", "waited_ns": 4},
                           name=bounded.WAITING_NAME)
        monitor.read_admission(str(bad), None, "admission_waiting")
        link = directory / "waiting-link"
        os.symlink(str(bad), str(link))
        monitor.read_admission(str(link), None, "admission_waiting")
        # The invalid document is reported as a rejection; the symlink is only
        # recorded.
        sent = self.drain(monitor.cmd_r)
        self.assertEqual([m["type"] for m in sent], ["admission_rejected"])
        waiting = monitor.receipt["admission_waiting"]
        self.assertEqual((waiting["observed"], waiting["ignored"]), (0, 2))
        self.assertEqual(waiting["reason"], None)
        self.assertEqual(monitor.receipt["admission"]["ignored"], 0, "a waiting rejection is its own record")

    def test_the_receipt_objects_are_bounded_and_typed(self):
        record = bounded.admission_record()
        self.assertEqual(set(record), {"schema", "marker", "observed", "reported_wait_ns", "first_ns",
                                       "last_ns", "identity", "ignored", "rejections", "errors"})
        self.assertEqual(record["marker"], bounded.ADMISSION_NAME)
        waiting = bounded.admission_waiting_record()
        self.assertEqual(set(waiting), {"schema", "marker", "observed", "first_ns", "last_ns", "reason",
                                        "identity", "ignored", "rejections", "errors"})
        self.assertEqual(waiting["marker"], bounded.WAITING_NAME)
        for entry in (record, waiting):
            entry["rejections"] = ["reason"] * bounded.ADMISSION_REJECTIONS
            self.assertLess(len(json.dumps(entry)), 1024)
        self.assertLess(bounded.ADMISSION_MAX_BYTES, 4096)


# Owned-byte traversal (oxide#142) ----------------------------------------------

try:
    LIMIT = os.pathconf(str(SCRATCH), "PC_PATH_MAX")  # bytes, stated independently of the walk
except (OSError, ValueError):
    LIMIT = 1024
NAME = "\u00e9" * 20  # 20 characters, 40 bytes: the byte and character rules differ


def oracle_bytes(root, union=True):
    """Independent descriptor-relative total: `st_blocks * 512` per distinct
    (device, inode) under `root`, with no symlink followed. `union=False`
    counts every name instead, so a hard-link case can show what union removed."""
    seen, total = set(), 0

    def note(meta):
        nonlocal total
        if union:
            identity = (meta.st_dev, meta.st_ino)
            if identity in seen:
                return False
            seen.add(identity)
        total += meta.st_blocks * 512
        return True

    def walk(fd):
        for name in os.listdir(fd):
            meta = os.stat(name, dir_fd=fd, follow_symlinks=False)
            if not note(meta) or not stat.S_ISDIR(meta.st_mode):
                continue
            child = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            try:
                walk(child)
            finally:
                os.close(child)

    if isinstance(root, int):  # an already-open directory, however deep
        fd = os.dup(root)
    else:
        fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        note(os.fstat(fd))
        walk(fd)
    finally:
        os.close(fd)
    return total


def long_fixture(base, referent):
    """Create owned paths at and past `LIMIT` using only short
    descriptor-relative names, exactly as the #141 product fixture does: a
    regular file one byte past the limit, a hard-link pair, and a directory
    that is itself unnameable holding a file and a symlink to `referent`,
    which names a directory outside the fixture. Returns the absolute names
    that no path lookup can reach."""
    os.mkdir(base)
    names, fds = [], []
    fd = os.open(base, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    fds.append(fd)

    def descend():
        nonlocal fd
        os.mkdir(NAME, dir_fd=fd)
        child = os.open(NAME, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
        fds.append(child)
        names.append(NAME)
        fd = child

    def create(where, name, size):
        handle = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600, dir_fd=where)
        try:
            os.write(handle, bytes(size))
        finally:
            os.close(handle)

    try:
        while len(os.fsencode(os.path.join(base, *names, NAME))) < LIMIT:
            descend()  # every level stays nameable by path
        prefix = len(os.fsencode(os.path.join(base, *names))) + 1
        leaf, link = "f" * (LIMIT - prefix + 1), "h" * (LIMIT - prefix + 1)
        leaf_path = os.path.join(base, *names, leaf)  # one byte past the limit
        create(fd, leaf, 4096)
        create(fds[0], "short", 8192)  # nameable by path, at the fixture base
        os.link("short", link, src_dir_fd=fds[0], dst_dir_fd=fd)
        link_path = os.path.join(base, *names, link)
        anchor, mark = os.path.join(base, *names), len(names)  # still nameable
        while len(os.fsencode(os.path.join(base, *names))) < LIMIT + 64:
            descend()  # the directory itself is past the limit now
        deepest = os.path.join(base, *names)
        create(fd, "i" * 40, 2048)
        os.symlink(referent, "s" * 40, dir_fd=fd)
    finally:
        for handle in reversed(fds):
            os.close(handle)
    return {"leaf": leaf_path, "link": link_path, "deepest": deepest,
            "anchor": anchor, "suffix": tuple(names[mark:])}


def seam_fixture(base, referent):
    """A nameable anchor holding one overlong regular file and one entry past
    the limit, for the seam controls: that entry is a symlink to `referent`, so
    a walk that follows it would count an outside tree."""
    os.mkdir(base)
    names, fds = [], []
    fd = os.open(base, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    fds.append(fd)
    try:
        while len(os.fsencode(os.path.join(base, *names, NAME))) < LIMIT:
            os.mkdir(NAME, dir_fd=fd)
            child = os.open(NAME, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            fds.append(child)
            names.append(NAME)
            fd = child
        anchor = os.path.join(base, *names)
        prefix = len(os.fsencode(anchor)) + 1
        leaf, tail = "f" * (LIMIT - prefix + 1), "l" * (LIMIT - prefix + 1)
        handle = os.open(leaf, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600, dir_fd=fd)
        try:
            os.write(handle, bytes(4096))
        finally:
            os.close(handle)
        os.symlink(referent, tail, dir_fd=fd)
    finally:
        for handle in reversed(fds):
            os.close(handle)
    return {"anchor": anchor, "leaf": os.path.join(anchor, leaf),
            "tail": tail, "link": os.path.join(anchor, tail)}


class PureSurvivors(unittest.TestCase):
    """Oxide#195 under Tom's 2026-10-31 ruling (Queue decision c096c8fc): a
    same-user process that is not held unreaped by the runner is never
    signalled, because its PID could be recycled between the identity read and
    the signal. The monitor records the survivor's exact identity in the
    receipt and the census still decides UNCLOSED; no announcement and no
    non-held signal exists on this path."""

    def monitor(self, survivors=None):
        ctl_r, ctl_w = os.pipe()
        cmd_r, cmd_w = os.pipe()
        out_r, out_w = os.pipe()
        err_r, err_w = os.pipe()
        for fd in (ctl_r, cmd_r, out_r, err_r, ctl_w, cmd_w, out_w, err_w):
            self.addCleanup(os.close, fd)
        monitor = bounded.Monitor({"run_id": "r", "inject": [],
                                   "terminal_mode": {"mode": "strict"}},
                                  ctl_w, cmd_r, out_r, err_r)
        monitor.leader = [4242, 77]
        monitor.receipt = {"survivors": [] if survivors is None else survivors}
        os.set_blocking(ctl_r, False)
        return monitor, ctl_r

    def sent(self, ctl_r):
        blob = b""
        while True:
            try:
                chunk = os.read(ctl_r, 1 << 20)
            except BlockingIOError:
                break
            if not chunk:
                break
            blob += chunk
        return blob

    def members(self, *rows):
        return {"certain": False, "members": len(rows), "uncertain": 0,
                "member_rows": list(rows)}

    def test_mismatched_deciding_message_is_not_bound(self):
        monitor, _ = self.monitor()
        monitor.receipt.update({"census_rounds": 1, "custodian_census": {
            "seq": 4, "after": 2, "certain": True, "members": 0,
            "uncertain": 0, "replacement_work": fixture_work()}})
        monitor.census_messages = {4: {"after": 2, "certain": True, "members": 1,
                                      "uncertain": 0, "replacement_work": fixture_work()}}
        monitor.bind_deciding_census()
        self.assertIsNone(monitor.receipt["deciding_census"])
        self.assertIsNone(monitor.receipt["deciding_census_seq"])

    def test_a_survivor_row_carries_only_a_bounded_identity(self):
        live = row(900, start=5, uid=501)
        self.assertEqual(bounded.survivor_row(live),
                         {"pid": 900, "start": 5, "ppid": 1, "pgid": 900, "uid": 501,
                          "zombie": False})
        self.assertEqual(set(bounded.survivor_row(live)), set(bounded.SURVIVOR_KEYS))
        # Extra or textual fields are dropped, never copied into the receipt.
        self.assertEqual(set(bounded.survivor_row(dict(live, command="/bin/x", note="text"))),
                         set(bounded.SURVIVOR_KEYS))
        for broken in (None, "row", {}, {"pid": 900}, dict(live, pid=0),
                       dict(live, start=0), dict(live, pid=-1), dict(live, start="5"),
                       dict(live, pid=True), dict(live, zombie="no"), dict(live, zombie=1),
                       dict(live, uid=None), dict(live, pgid="1")):
            with self.subTest(broken=broken):
                self.assertIsNone(bounded.survivor_row(broken))

    def test_the_monitor_records_each_identity_once_within_the_bound(self):
        monitor, _ctl_r = self.monitor()
        monitor.record_survivors(self.members(row(900, start=5), row(900, start=5),
                                              {"pid": 901}, "not-a-row", None))
        self.assertEqual(monitor.receipt["survivors"],
                         [{"pid": 900, "start": 5, "ppid": 1, "pgid": 900, "uid": 501,
                           "zombie": False}])
        monitor.record_survivors(self.members(row(900, start=5)))  # same identity: no duplicate
        self.assertEqual(len(monitor.receipt["survivors"]), 1)
        # The first observation of one identity wins; a new identity is a new row.
        monitor.record_survivors(self.members(row(900, start=5, zombie=True), row(901, start=6)))
        self.assertEqual([(item["pid"], item["start"], item["zombie"])
                          for item in monitor.receipt["survivors"]],
                         [(900, 5, False), (901, 6, False)])
        monitor, _ctl_r = self.monitor()
        rows = [row(1000 + index, start=index + 1) for index in range(bounded.SURVIVOR_MAX + 6)]
        monitor.record_survivors(self.members(*rows))
        self.assertEqual(len(monitor.receipt["survivors"]), bounded.SURVIVOR_MAX)
        self.assertEqual([item["pid"] for item in monitor.receipt["survivors"]],
                         [1000 + index for index in range(bounded.SURVIVOR_MAX)])

    def test_recording_a_survivor_sends_and_signals_nothing(self):
        monitor, ctl_r = self.monitor()
        calls = []

        def refuse(*args):
            calls.append(args)
            raise AssertionError("the survivor path must never signal")

        with patched(os, "kill", refuse), patched(os, "killpg", refuse):
            monitor.record_survivors(self.members(row(900, start=5), row(901, start=6)))
        self.assertEqual(calls, [])
        self.assertEqual(self.sent(ctl_r), b"", "the survivor path must never send a message")
        self.assertEqual(len(monitor.receipt["survivors"]), 2)
        # No closure claim either: the census/receipt decision is untouched.
        self.assertNotIn("census", monitor.receipt)

    def test_no_supervisor_path_signals_an_unheld_process(self):
        # The structural regression for the ruling: every kill/killpg call site
        # in the supervisor must be one of the custody-bound functions, so a
        # checked-then-signalled non-child PID cannot be reintroduced.
        source = Path(bounded.__file__).read_text()
        tree = ast.parse(source)
        def callee(node):
            if isinstance(node, ast.Attribute):
                return {f"{getattr(node.value, 'id', '?')}.{node.attr}"}
            if isinstance(node, ast.IfExp):  # `(os.killpg if group else os.kill)(...)`
                return callee(node.body) | callee(node.orelse)
            return set()

        sites = {}
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef):
                for inner in ast.walk(node):
                    if not isinstance(inner, ast.Call):
                        continue
                    for name in callee(inner.func) & {"os.kill", "os.killpg"}:
                        sites.setdefault(node.name, set()).add(name)
        self.assertEqual(sites, {
            # a direct child this process still holds unreaped (pid cannot be reused)
            "close_own_child": {"os.kill"},
            # the pinned leader/monitor, checked with held_unreaped before the call
            "signal_pinned": {"os.kill", "os.killpg"},
            # the pinned detach child, identity- and parentage-checked
            "signal_detach_child": {"os.kill"},
            # a signal to its own PID inside the fork child (injections)
            "launch_leader": {"os.kill"},
            # a liveness probe of the pinned group, never a signal
            "stop_leader": {"os.killpg"},
        }, sites)
        self.assertNotIn("session_stop", bounded.FIELDS)
        for forbidden in ("session_signal", "session_member_refused", "session_stop_attempt"):
            self.assertNotIn(forbidden, source)

    def test_the_summary_line_states_the_residual_without_claiming_a_signal(self):
        self.assertIsNone(bounded.survivors_line([]))
        self.assertIsNone(bounded.survivors_line(None))
        self.assertIsNone(bounded.survivors_line([], "closed"))
        one = bounded.survivors_line([{"pid": 900, "start": 5}], "UNCLOSED")
        self.assertIn("1 same-session survivor(s) not signalled", one)
        self.assertIn("900/5", one)
        self.assertIn("UNCLOSED", one)
        self.assertIn("scratch is kept", one)
        self.assertNotIn("limit", one)
        many = bounded.survivors_line([{"pid": 900 + index, "start": 5}
                                       for index in range(10)], "UNCLOSED")
        self.assertIn("900/5", many)
        self.assertIn("+2 more", many)
        self.assertEqual(many.count("/5"), 8)

    def test_a_transient_survivor_then_a_certain_empty_census_reports_closed(self):
        # Exact-head review: a survivor observed after the stop may exit by
        # itself, so a later certain-empty census closes the run. The line must
        # never claim UNCLOSED or a kept scratch for a closed run, and the
        # summary must take the run's own final closure rather than the rows.
        transient = bounded.survivors_line([{"pid": 900, "start": 5}], "closed")
        self.assertIn("900/5", transient)
        self.assertIn("gone by the deciding census", transient)
        self.assertIn("the run closed", transient)
        self.assertNotIn("UNCLOSED", transient)
        self.assertNotIn("kept", transient)
        unknown = bounded.survivors_line([{"pid": 900, "start": 5}])
        self.assertIn("the deciding census fixes the outcome", unknown)
        self.assertNotIn("UNCLOSED", unknown)
        self.assertNotIn("closed;", unknown)
        # The summary composes the line from the receipt's own outcome, so the
        # transient case closes truthfully end to end.
        source = " ".join(inspect.getsource(bounded.Monitor.summary).split())
        self.assertIn('survivors_line(receipt.get("survivors"), outcome.get("closure"),'
                      ' join.get("joined"))', source)
        self.assertNotIn('survivors_line(receipt.get("survivors"), "UNCLOSED")', source)
        # And the persistent-survivor regression still holds: the same rows with
        # an UNCLOSED outcome keep the honest residual.
        persistent = bounded.survivors_line([{"pid": 900, "start": 5}], "UNCLOSED")
        self.assertIn("the run stays UNCLOSED and its scratch is kept", persistent)
        self.assertNotIn("the run closed", persistent)

    def test_a_transient_survivor_records_a_row_and_does_not_decide_closure(self):
        # The monitor records the row and makes no closure claim of its own: the
        # receipt decision stays with the census, so a later empty round can win.
        monitor, _ctl_r = self.monitor()
        monitor.record_survivors(self.members(row(900, start=5)))
        self.assertEqual([item["pid"] for item in monitor.receipt["survivors"]], [900])
        self.assertNotIn("outcome", monitor.receipt)
        self.assertNotIn("census", monitor.receipt)
        # A second, certainly empty round adds nothing and closes nothing here.
        monitor.record_survivors({"certain": True, "members": 0, "uncertain": 0, "member_rows": []})
        self.assertEqual(len(monitor.receipt["survivors"]), 1)

    @contextlib.contextmanager
    def settled_census(self):
        """A certain-empty census stand-in, for scripted decisions."""

        class SettledCensus:
            def __init__(self, sid, terminal_allowed=False, ancestors=()):
                self.stats = {}

            def round(self):
                self.round_deadline = bounded.now_ns() + 2 * NS
                return {"certain": True, "members": 0, "uncertain": 0, "member_rows": [],
                        "uncertain_rows": [], "terminal_count_status": "valid",
                        "terminal_nonwriters": 0, "replacement_work": fixture_work()}

        saved = bounded.Census
        bounded.Census = SettledCensus
        try:
            yield
        finally:
            bounded.Census = saved

    def publishable_monitor(self):
        monitor, ctl_r = self.monitor()
        evidence = Path(tempfile.mkdtemp(prefix="survivors.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(evidence), True)
        monitor.ctx = {"run_id": "r", "evidence": str(evidence)}
        monitor.receipt.update({"phase": None, "custodian": [], "census": None, "census_rounds": 0})
        return monitor, evidence, ctl_r

    def test_a_recorded_survivor_reaches_the_published_receipt(self):
        # Review finding: a row that never reaches the receipt on disk is not
        # evidence. Recording plus publication must leave the row in the file.
        monitor, evidence, _ctl_r = self.publishable_monitor()
        monitor.record_survivors(self.members(row(900, start=5)))
        monitor.publish("unclosed: scratch_kept")
        published = json.loads((evidence / "receipt.json").read_text())
        self.assertEqual([(item["pid"], item["start"], item["zombie"])
                          for item in published["survivors"]], [(900, 5, False)])
        self.assertEqual(published["phase"], "unclosed: scratch_kept")

    def test_the_round_records_its_survivors_before_it_reports_them(self):
        # The custodian can decide on the census message and the monitor can
        # publish immediately after it, so the row must already be recorded when
        # the message is sent. Driving the real loop pins the ordering.
        monitor, _evidence, _ctl_r = self.publishable_monitor()
        monitor.census_stop = OneShotStop()
        seen = []

        class SurvivorCensus(FakeCensus):
            def round(self):
                self.round_deadline = bounded.now_ns() + 2 * NS
                return {"certain": False, "members": 1, "uncertain": 0,
                        "member_rows": [row(900, start=5)], "uncertain_rows": [],
                        "terminal_count_status": "valid", "terminal_nonwriters": 0, "replacement_work": fixture_work()}

        original_send = monitor.send

        def send(message_type, **fields):
            if message_type == "census":
                seen.append(len(monitor.receipt["survivors"]))
            return original_send(message_type, **fields)

        monitor.send = send
        saved = bounded.Census
        bounded.Census = SurvivorCensus
        try:
            monitor.census_loop(7)
        finally:
            bounded.Census = saved
        self.assertEqual(seen, [1], "the census message was sent before the survivor was recorded")
        self.assertEqual([item["pid"] for item in monitor.receipt["survivors"]], [900])

    def test_the_census_thread_is_joined_before_the_final_publication(self):
        # A late append after publication would silently drop evidence: a prompt
        # join must land the row first and record that the list is complete.
        monitor, evidence, _ctl_r = self.publishable_monitor()
        monitor.census_stop = OneShotStop()

        def late_round():
            time.sleep(0.05)
            monitor.record_survivors(self.members(row(900, start=5)))

        monitor.census_thread = threading.Thread(target=late_round)
        monitor.census_thread.start()
        record = monitor.join_census(5)
        self.assertEqual(record["joined"], True)
        self.assertNotIn("frozen", record)
        self.assertEqual(record["rows"], 1)
        monitor.publish("unclosed: scratch_kept")
        published = json.loads((evidence / "receipt.json").read_text())
        self.assertEqual([item["pid"] for item in published["survivors"]], [900])
        self.assertEqual(published["census_join"]["joined"], True)
        # Idempotent and never blocking: an absent thread and a spent bound are
        # both non-verdicts, never exceptions.
        self.assertTrue(monitor.join_census(5)["joined"])
        self.assertTrue(bounded.Monitor.join_census(self.monitor()[0], 0)["joined"])
        self.assertTrue(bounded.Monitor.join_census(self.monitor()[0], -5)["joined"])

    def test_an_expired_join_freezes_the_published_list_and_says_so(self):
        # Reviewer finding: a bounded join may expire. The published receipt must
        # then be an honest, explicitly incomplete prefix: no row may land after
        # the publication, the receipt records `joined: false`/`frozen: true`, and
        # the summary says later rows may be missing instead of implying a
        # complete record. This is the discriminating timeout-path regression.
        monitor, evidence, _ctl_r = self.publishable_monitor()
        monitor.census_stop = OneShotStop()
        monitor.record_survivors(self.members(row(900, start=5)))  # prefix before the stall
        release = threading.Event()
        appended = []

        def stalled_round():
            release.wait(5)
            monitor.record_survivors(self.members(row(901, start=6)))
            appended.append(True)

        monitor.census_thread = threading.Thread(target=stalled_round)
        monitor.census_thread.start()
        record = monitor.join_census(0.05)
        self.assertEqual(record["joined"], False)
        self.assertIs(record["frozen"], True)
        self.assertEqual(record["rows"], 1)
        self.assertIs(monitor.census_frozen, True)
        try:
            monitor.publish("unclosed: scratch_kept")
            published = json.loads((evidence / "receipt.json").read_text())
            self.assertEqual([item["pid"] for item in published["survivors"]], [900])
            self.assertEqual(published["census_join"]["joined"], False)
            self.assertIs(published["census_join"]["frozen"], True)
            line = bounded.survivors_line(published["survivors"], "UNCLOSED",
                                          published["census_join"].get("joined"))
            self.assertIn("may be missing", line)
            self.assertIn("the run stays UNCLOSED and its scratch is kept", line)
            self.assertNotIn("the run closed", line)
            # The stalled round finishes late: its row must never appear behind
            # the published receipt, and the receipt stays the frozen prefix.
            release.set()
            monitor.census_thread.join(5)
            self.assertFalse(monitor.census_thread.is_alive())
            self.assertEqual(appended, [True])
            self.assertEqual([item["pid"] for item in monitor.receipt["survivors"]], [900])
            on_disk = json.loads((evidence / "receipt.json").read_text())
            self.assertEqual([item["pid"] for item in on_disk["survivors"]], [900])
            # A later join call never rewrites the published record, even though
            # the thread has now finished: the frozen prefix stays the record.
            again = monitor.join_census(5)
            self.assertIs(again["joined"], False)
            self.assertIs(again["frozen"], True)
            self.assertEqual(monitor.receipt["census_join"]["rows"], 1)
            # A completed join adds no caveat, and a completed closed run still
            # reports the truth without one.
            self.assertNotIn("may be missing",
                             bounded.survivors_line([{"pid": 900, "start": 5}], "closed", True))
            false_closed = bounded.survivors_line([{"pid": 900, "start": 5}], "closed", False)
            self.assertNotIn("so the run closed", false_closed)
            self.assertIn("the run is not closed", false_closed)
            self.assertIn("may be missing",
                          bounded.survivors_line([{"pid": 900, "start": 5}], "UNCLOSED", False))
        finally:
            release.set()

    def test_the_join_record_is_published_and_never_decides_closure(self):
        # The join record is evidence about the record, not a verdict: a monitor
        # that produced it makes no closure claim of its own.
        monitor, evidence, _ctl_r = self.publishable_monitor()
        monitor.census_thread = None
        record = monitor.join_census(1)
        self.assertEqual(record, {"joined": True, "started": False, "bound_s": 1.0,
                                  "waited_ns": record["waited_ns"], "rows": 0,
                                  "deciding": None, "final": None})
        monitor.publish("started: incomplete")
        published = json.loads((evidence / "receipt.json").read_text())
        self.assertEqual(published["census_join"]["joined"], True)
        self.assertNotIn("outcome", published)
        self.assertNotIn("frozen", published["census_join"])

    def custodian_stub(self):
        custodian = bounded.Custodian(["true"])
        custodian.run_id = "r"
        read_end, custodian.cmd_w = os.pipe()
        self.addCleanup(os.close, read_end)
        self.addCleanup(os.close, custodian.cmd_w)
        os.set_blocking(read_end, False)
        custodian.guard = bounded.Guard(0)
        custodian.leader = 4242
        custodian.leader_start = 77
        return custodian, read_end

    def test_an_expired_join_turns_a_cleanup_decision_into_an_honest_non_closure(self):
        # Supervisor invariant: an incomplete join NEVER yields certain closure.
        # The run keeps its root, the persisted verdict becomes UNCLOSED with the
        # reason, and the custodian latches the same reason so it cannot exit as a
        # pass. This fails on the pre-fix source, where the join answer was
        # ignored and the closed outcome stood.
        monitor, evidence, ctl_r = self.publishable_monitor()
        monitor.receipt["outcome"] = {"closure": "closed", "status": 0, "fault": None,
                                      "deadline": False, "launched": True}
        self.assertIn('if decision == "cleanup" and not settled:',
                      " ".join(inspect.getsource(bounded.Monitor.run).split()))
        join = {"joined": False, "frozen": True, "rows": 1, "bound_s": 0.05, "waited_ns": 1}
        reason = monitor.refuse_incomplete_closure(join)
        self.assertIs(monitor.receipt["census_join"]["refused"], True)
        # The refusal is final: a later join (or a second refusal pass) cannot make
        # the record look complete again.
        self.assertIs(monitor.join_census(5)["joined"], False)
        outcome = monitor.receipt["outcome"]
        self.assertEqual(outcome["closure"], "UNCLOSED")
        self.assertEqual(outcome["fault"], "closure not established: " + reason)
        self.assertIs(outcome["deadline"], False)
        monitor.publish("census_join_incomplete: scratch_kept")
        published = json.loads((evidence / "receipt.json").read_text())
        self.assertEqual(published["outcome"]["closure"], "UNCLOSED")
        self.assertEqual(published["phase"], "census_join_incomplete: scratch_kept")
        # The message is well formed and the custodian turns it into a latched
        # fault: closure is refused and the exit is not a pass.
        blob = os.read(ctl_r, 1 << 16).decode()
        lines = [line for line in blob.splitlines() if '"census_join_incomplete"' in line]
        self.assertEqual(len(lines), 1, blob)
        message = json.loads(lines[0])
        self.assertIsNone(bounded.malformed(message))
        custodian, _cmd = self.custodian_stub()
        custodian.monitor_seq = 0
        custodian.accept(message)
        self.assertIn("closure not established", custodian.guard.fault or "")
        self.assertTrue(custodian.census_uncertain)
        self.assertFalse(custodian.closure(True))
        self.assertEqual(bounded.exit_code(False, custodian.guard.fault, True, 0), 125)
        self.assertEqual(bounded.exit_code(True, custodian.guard.fault, True, 0), 124)
        # A completed join takes none of this path: the closed outcome stands and
        # no message is sent.
        clean, clean_evidence, clean_ctl = self.publishable_monitor()
        clean.receipt["outcome"] = {"closure": "closed", "status": 0, "fault": None,
                                    "deadline": False, "launched": True}
        clean.census_thread = None
        self.assertIs(clean.join_census(1)["joined"], True)
        clean.publish("closure_established: cleanup_pending")
        kept = json.loads((clean_evidence / "receipt.json").read_text())
        self.assertEqual(kept["outcome"]["closure"], "closed")
        self.assertNotIn("refused", kept["census_join"])
        with self.assertRaises(BlockingIOError):  # nothing was sent: no message
            os.read(clean_ctl, 1 << 16)

    def test_a_late_census_round_cannot_be_omitted_or_pass_through_run(self):
        # Reviewer finding: exercise the whole run path, including finalisation.
        # A cleanup decision arrives while the census thread is still inside a
        # round that will later observe a member. The bounded join expires, the
        # decision is refused, the late row can never reach the published receipt,
        # the owned scratch is kept, and nothing may pass.
        sandbox = Path(tempfile.mkdtemp(prefix="late-census.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(sandbox), True)
        evidence = sandbox / "evidence"
        ctx = {"run_id": "late-census", "command": ["/bin/true"], "limit_s": 300,
               "adopted": None, "evidence_arg": str(evidence), "root_parent": str(sandbox),
               "budget": 512 * 1024 * 1024,
               "start_ns": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW), "inject": []}
        ctl_r, ctl_w = os.pipe()
        cmd_r, cmd_w = os.pipe()
        for fd in (ctl_r, cmd_w):
            self.addCleanup(os.close, fd)
        # The monitor's drain threads get immediate EOF (its own read fds).
        monitor = bounded.Monitor(ctx, ctl_w, cmd_r, os.open(os.devnull, os.O_RDONLY),
                                  os.open(os.devnull, os.O_RDONLY))
        sent, lines = [], []
        release = threading.Event()

        def send(message_type, **fields):
            sent.append((message_type, dict(fields)))

        class LateCensus:
            def __init__(self, sid, terminal_allowed=False, ancestors=()):
                self.stats = {}

            def round(self):
                self.round_deadline = bounded.now_ns() + 2 * NS
                release.wait(15)  # still inside a round when the join expires
                return {"certain": False, "members": 1, "uncertain": 0,
                        "member_rows": [row(900, start=5)], "uncertain_rows": [],
                        "terminal_count_status": "valid", "terminal_nonwriters": 0, "replacement_work": fixture_work()}

        monitor.send = send
        try:
            with patched(bounded, "report", lines.append), patched(bounded, "Census", LateCensus):
                for number, command in enumerate((
                        {"type": "leader", "pid": os.getpid()},
                        {"type": "stopped", "sent": 0},
                        {"type": "cleanup", "outcome": {"closure": "closed", "status": 0,
                                                        "fault": None, "deadline": False,
                                                        "launched": True}}), start=1):
                    os.write(cmd_w, (json.dumps(dict(command, run="late-census", seq=number))
                                     + "\n").encode())
                monitor.run()
        finally:
            release.set()
            if monitor.census_thread is not None:
                monitor.census_thread.join(15)
        persisted = json.loads((evidence / "receipt.json").read_text())
        # No pass: the refused decision is UNCLOSED, with the reason on the record.
        self.assertEqual(persisted["phase"], "monitor_complete: pending_custodian")
        self.assertEqual(persisted["outcome"]["closure"], "UNCLOSED")
        self.assertIn("census thread did not finish before publication",
                      persisted["outcome"]["fault"])
        join = persisted["census_join"]
        self.assertIs(join["joined"], False)
        self.assertIs(join["frozen"], True)
        self.assertIs(join["refused"], True)
        self.assertIs(join["started"], True)
        # The late round observed a member, but it cannot land behind the receipt.
        self.assertEqual(persisted["survivors"], [])
        scratch = Path(ctx["scratch"])
        self.assertTrue(scratch.is_dir(), "the refused run removed its owned scratch")
        self.assertEqual(sorted(path.name for path in scratch.iterdir()), [])
        # The summary names the incomplete census and never claims closure, even
        # with an empty survivor prefix.
        self.assertTrue(any("no same-session survivor row was recorded" in line for line in lines),
                        lines)
        self.assertFalse(any("so the run closed" in line for line in lines), lines)
        self.assertFalse(any("closure_established" in line for line in lines), lines)
        # No cleanup was attempted: no cleanup phase was ever published.
        self.assertNotIn("cleanup: owned scratch not fully removed", " ".join(lines))
        kinds = [kind for kind, _fields in sent]
        self.assertIn("census_join_incomplete", kinds)
        self.assertIn("monitor_complete", kinds)
        # The custodian turns that message into a latched fault: not a pass.
        custodian, _cmd = self.custodian_stub()
        custodian.run_id = "late-census"
        custodian.monitor_seq = 0
        custodian.leader = monitor.leader[0]
        custodian.leader_start = monitor.leader[1]
        message = dict(next(fields for kind, fields in sent if kind == "census_join_incomplete"),
                       type="census_join_incomplete", run="late-census", seq=1,
                       at=bounded.now_ns(), leader=monitor.leader)
        self.assertIsNone(bounded.malformed(message))
        custodian.accept(message)
        self.assertIn("closure not established", custodian.guard.fault or "")
        self.assertEqual(bounded.exit_code(False, custodian.guard.fault, True, 0), 125)

    def test_a_round_that_returns_during_the_join_binds_the_deciding_census(self):
        # Reviewer finding: a stalled round may return between the stop and the
        # final publication. It must never be mistaken for the deciding census,
        # and a final round that reports a member must refuse closure while its
        # row is kept as evidence. Releasing the round during the join (not after
        # run() returns) is the interleaving this exercises.
        sandbox = Path(tempfile.mkdtemp(prefix="mid-join.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(sandbox), True)
        evidence = sandbox / "evidence"
        ctx = {"run_id": "mid-join", "command": ["/bin/true"], "limit_s": 300,
               "adopted": None, "evidence_arg": str(evidence), "root_parent": str(sandbox),
               "budget": 512 * 1024 * 1024,
               "start_ns": time.clock_gettime_ns(time.CLOCK_UPUPTIME_RAW)
               if False else time.clock_gettime_ns(time.CLOCK_UPTIME_RAW), "inject": []}
        ctl_r, ctl_w = os.pipe()
        cmd_r, cmd_w = os.pipe()
        for fd in (ctl_r, cmd_w):
            self.addCleanup(os.close, fd)
        # The monitor's drain threads get immediate EOF (its own read fds).
        monitor = bounded.Monitor(ctx, ctl_w, cmd_r, os.open(os.devnull, os.O_RDONLY),
                                  os.open(os.devnull, os.O_RDONLY))
        sent, lines, written = [], [], []
        release = threading.Event()

        class InterleavedCensus:
            def __init__(self, sid, terminal_allowed=False, ancestors=()):
                self.stats = {}
                self.rounds = 0

            def round(self):
                self.round_deadline = bounded.now_ns() + 2 * NS
                self.rounds += 1
                if self.rounds == 1:
                    return {"certain": True, "members": 0, "uncertain": 0, "member_rows": [],
                            "uncertain_rows": [], "terminal_count_status": "valid",
                            "terminal_nonwriters": 0, "replacement_work": fixture_work()}
                release.wait(15)  # still inside the round when the stop is bound
                return {"certain": False, "members": 1, "uncertain": 0,
                        "member_rows": [row(900, start=5)], "uncertain_rows": [],
                        "terminal_count_status": "valid", "terminal_nonwriters": 0, "replacement_work": fixture_work()}

        real_send = monitor.send
        census_seqs = []

        def decide(seq):
            os.write(cmd_w, (json.dumps({
                "type": "cleanup", "run": "mid-join",
                "census": {"seq": seq, "after": 2, "certain": True, "members": 0,
                           "uncertain": 0, "replacement_work": fixture_work()},
                "outcome": {"closure": "closed", "status": 0, "fault": None,
                            "deadline": False, "launched": True}}) + "\n").encode())

        def send(message_type, **fields):
            if monitor.stop.is_set():  # the run is over: never write to a closing pipe
                return monitor.seq
            seq = real_send(message_type, **fields)
            sent.append((message_type, dict(fields)))
            if message_type == "census" and not written:
                # The decision lands after the next round has started, so that
                # round is still in flight when the deciding census is bound.
                written.append(True)
                census_seqs.append(seq)
                threading.Timer(0.35, lambda: decide(seq)).start()
            return seq

        def releaser():
            # Release only once the deciding census is bound: that is inside the
            # bounded join, before the final publication.
            until = time.monotonic() + 15
            while not monitor.census_bound and time.monotonic() < until:
                time.sleep(0.005)
            release.set()

        monitor.send = send
        helper = threading.Thread(target=releaser)
        try:
            with patched(bounded, "report", lines.append), \
                    patched(bounded, "Census", InterleavedCensus):
                for number, command in enumerate((
                        {"type": "leader", "pid": os.getpid()},
                        {"type": "stopped", "sent": 0}), start=1):
                    os.write(cmd_w, (json.dumps(dict(command, run="mid-join", seq=number))
                                     + "\n").encode())
                helper.start()
                monitor.run()
        finally:
            release.set()
            helper.join(15)
            if monitor.census_thread is not None:
                monitor.census_thread.join(15)
        persisted = json.loads((evidence / "receipt.json").read_text())
        # The deciding census is the certainly empty round, and it never changes.
        self.assertEqual(persisted["deciding_census"],
                         {"certain": True, "members": 0, "uncertain": 0, "after": 2,
                          "replacement_work": fixture_work()})
        self.assertEqual(persisted["census_rounds"], 1)
        self.assertEqual(persisted["census_rounds_at_stop"], 1)
        self.assertEqual(persisted["census"], {"certain": True, "members": 0, "uncertain": 0,
                                               "member_rows": [], "uncertain_rows": [],
                                               "terminal_count_status": "valid",
                                               "terminal_nonwriters": 0,
                                               "replacement_work": fixture_work()})
        # The late round is a post-decision observation, clearly labelled.
        self.assertEqual(persisted["census_after_stop"],
                         {"rounds": 1, "last": {"certain": False, "members": 1,
                                                "uncertain": 0, "replacement_work": fixture_work()}})
        join = persisted["census_join"]
        self.assertIs(join["joined"], True)
        self.assertIs(join["started"], True)
        self.assertEqual(join["deciding"], {"certain": True, "members": 0, "uncertain": 0})
        self.assertEqual(join["final"], {"certain": False, "members": 1, "uncertain": 0})
        # The deciding census is the message the decision named, resolved against
        # the monitor's own record of what it sent.
        self.assertEqual(persisted["deciding_census_seq"], census_seqs[0])
        self.assertEqual(persisted["deciding_census"],
                         {"certain": True, "members": 0, "uncertain": 0, "after": 2,
                          "replacement_work": fixture_work()})
        self.assertEqual(persisted["custodian_census"]["seq"], census_seqs[0])
        # Closure is refused on the FINAL census, not the stale deciding one, and
        # the late row is kept as evidence instead of being dropped.
        self.assertEqual(persisted["outcome"]["closure"], "UNCLOSED")
        self.assertIn("final census round was not certainly empty",
                      persisted["outcome"]["fault"])
        self.assertEqual([item["pid"] for item in persisted["survivors"]], [900])
        self.assertTrue(Path(ctx["scratch"]).is_dir(), "the refused run removed its scratch")
        self.assertFalse(any("so the run closed" in line for line in lines), lines)
        self.assertTrue(any("the run stays UNCLOSED and its scratch is kept" in line
                            for line in lines), lines)
        kinds = [kind for kind, _fields in sent]
        self.assertIn("census_join_incomplete", kinds)
        custodian, _cmd = self.custodian_stub()
        custodian.run_id = "mid-join"
        custodian.monitor_seq = 0
        custodian.leader = monitor.leader[0]
        custodian.leader_start = monitor.leader[1]
        message = dict(next(fields for kind, fields in sent if kind == "census_join_incomplete"),
                       type="census_join_incomplete", run="mid-join", seq=1,
                       at=bounded.now_ns(), leader=monitor.leader)
        self.assertIsNone(bounded.malformed(message))
        custodian.accept(message)
        self.assertIn("closure not established", custodian.guard.fault or "")
        self.assertEqual(bounded.exit_code(False, custodian.guard.fault, True, 0), 125)

    def test_the_deciding_census_is_the_message_the_decision_named(self):
        # Reviewer finding: the labelled deciding census must be the census
        # MESSAGE the custodian's decision named, not merely the last round the
        # monitor happened to complete. Here a member round completes (and is
        # sent) BEFORE the decision arrives, while the decision names the earlier
        # certainly empty message: the receipt must bind the deciding census to
        # that message, keep the member round visible as the census at the stop,
        # and refuse closure on the final round.
        sandbox = Path(tempfile.mkdtemp(prefix="named-census.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(sandbox), True)
        evidence = sandbox / "evidence"
        ctx = {"run_id": "named-census", "command": ["/bin/true"], "limit_s": 300,
               "adopted": None, "evidence_arg": str(evidence), "root_parent": str(sandbox),
               "budget": 512 * 1024 * 1024,
               "start_ns": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW), "inject": []}
        ctl_r, ctl_w = os.pipe()
        cmd_r, cmd_w = os.pipe()
        for fd in (ctl_r, cmd_w):
            self.addCleanup(os.close, fd)
        # The monitor's drain threads get immediate EOF (its own read fds).
        monitor = bounded.Monitor(ctx, ctl_w, cmd_r, os.open(os.devnull, os.O_RDONLY),
                                  os.open(os.devnull, os.O_RDONLY))
        sent, lines, census_seqs = [], [], []
        empty = {"certain": True, "members": 0, "uncertain": 0, "member_rows": [],
                 "uncertain_rows": [], "terminal_count_status": "valid",
                 "terminal_nonwriters": 0}
        member = {"certain": False, "members": 1, "uncertain": 0,
                  "member_rows": [row(900, start=5)], "uncertain_rows": [],
                  "terminal_count_status": "valid", "terminal_nonwriters": 0}
        empty["replacement_work"] = fixture_work()
        member["replacement_work"] = fixture_work()
        state = {"rounds": 0}

        class NamedCensus:
            def __init__(self, sid, terminal_allowed=False, ancestors=()):
                self.stats = {}

            def round(self):
                self.round_deadline = bounded.now_ns() + 2 * NS
                state["rounds"] += 1
                if state["rounds"] == 1:
                    return dict(empty)
                if state["rounds"] == 2:
                    return dict(member)  # completes and is sent before the decision
                # Round 3: the decision is written now, naming round 1's message,
                # so the bind happens while this round is in flight.
                os.write(cmd_w, (json.dumps({
                    "type": "cleanup", "run": "named-census",
                    "census": {"seq": census_seqs[0], "after": 2, "certain": True,
                               "members": 0, "uncertain": 0, "replacement_work": fixture_work()},
                    "outcome": {"closure": "closed", "status": 0, "fault": None,
                                "deadline": False, "launched": True}}) + "\n").encode())
                # Stay in flight until the deciding census is bound, so this round
                # is unambiguously post-decision.
                until = time.monotonic() + 15
                while not monitor.census_bound and time.monotonic() < until:
                    time.sleep(0.005)
                return dict(member)

        real_send = monitor.send

        def send(message_type, **fields):
            if monitor.stop.is_set():
                return monitor.seq
            seq = real_send(message_type, **fields)
            sent.append((message_type, dict(fields)))
            if message_type == "census":
                census_seqs.append(seq)
            return seq

        monitor.send = send
        try:
            with patched(bounded, "report", lines.append), patched(bounded, "Census", NamedCensus):
                for number, command in enumerate((
                        {"type": "leader", "pid": os.getpid()},
                        {"type": "stopped", "sent": 0}), start=1):
                    os.write(cmd_w, (json.dumps(dict(command, run="named-census", seq=number))
                                     + "\n").encode())
                monitor.run()
        finally:
            release = getattr(monitor, "census_thread", None)
            if release is not None:
                release.join(15)
        persisted = json.loads((evidence / "receipt.json").read_text())
        # The decision named message 1, which is certainly empty: that is the
        # deciding census, and it is bound by the monitor's own message record.
        self.assertEqual(persisted["custodian_census"]["seq"], census_seqs[0])
        self.assertEqual(persisted["deciding_census_seq"], census_seqs[0])
        self.assertEqual(persisted["deciding_census"],
                         {"certain": True, "members": 0, "uncertain": 0, "after": 2,
                          "replacement_work": fixture_work()})
        # The last round completed before the bind is the member round - clearly
        # distinguished from the deciding census rather than relabelled as it.
        self.assertEqual(persisted["census_rounds"], 2)
        self.assertEqual(persisted["census"]["members"], 1)
        self.assertNotEqual(persisted["deciding_census"], {"certain": False, "members": 1,
                                                           "uncertain": 0})
        # Round 3 (final) reports a member: closure is refused, on the record.
        self.assertEqual(persisted["census_after_stop"]["rounds"], 1)
        self.assertEqual(persisted["outcome"]["closure"], "UNCLOSED")
        self.assertIn("final census round was not certainly empty", persisted["outcome"]["fault"])
        self.assertEqual(persisted["census_join"]["deciding"],
                         {"certain": True, "members": 0, "uncertain": 0})
        self.assertTrue(Path(ctx["scratch"]).is_dir())
        self.assertFalse(any("so the run closed" in line for line in lines), lines)

    def test_a_decision_naming_no_sent_census_message_cannot_close(self):
        # The other side of the binding: a decision that names nothing, or a
        # message this monitor never sent, refuses closure even when the census
        # thread finished and the final round is certainly empty.
        for named in (None, {"seq": 999999, "after": 2, "certain": True, "members": 0,
                             "uncertain": 0}, {"seq": True, "certain": True, "members": 0,
                                              "uncertain": 0}, "not-an-object"):
            with self.subTest(named=named):
                sandbox = Path(tempfile.mkdtemp(prefix="unnamed-census.",
                                                dir=str(SCRATCH))).resolve()
                self.addCleanup(shutil.rmtree, str(sandbox), True)
                evidence = sandbox / "evidence"
                ctx = {"run_id": "unnamed-census", "command": ["/bin/true"], "limit_s": 300,
                       "adopted": None, "evidence_arg": str(evidence),
                       "root_parent": str(sandbox), "budget": 512 * 1024 * 1024,
                       "start_ns": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW), "inject": []}
                ctl_r, ctl_w = os.pipe()
                cmd_r, cmd_w = os.pipe()
                for fd in (ctl_r, cmd_w):
                    self.addCleanup(os.close, fd)
                monitor = bounded.Monitor(ctx, ctl_w, cmd_r,
                                          os.open(os.devnull, os.O_RDONLY),
                                          os.open(os.devnull, os.O_RDONLY))
                sent = []
                real_send = monitor.send

                def send(message_type, **fields):
                    if monitor.stop.is_set():
                        return monitor.seq
                    seq = real_send(message_type, **fields)
                    sent.append(message_type)
                    if message_type == "census" and not state.get("decided"):
                        state["decided"] = True
                        os.write(cmd_w, (json.dumps({
                            "type": "cleanup", "census": named,
                            "outcome": {"closure": "closed", "status": 0, "fault": None,
                                        "deadline": False, "launched": True}}) + "\n").encode())
                    return seq

                state = {}
                monitor.send = send
                try:
                    with self.settled_census():
                        for number, command in enumerate(({"type": "leader", "pid": os.getpid()},
                                                          {"type": "stopped", "sent": 0}), start=1):
                            os.write(cmd_w, (json.dumps(dict(command, run="unnamed-census",
                                                             seq=number)) + "\n").encode())
                        monitor.run()
                finally:
                    if monitor.census_thread is not None:
                        monitor.census_thread.join(15)
                persisted = json.loads((evidence / "receipt.json").read_text())
                self.assertEqual(persisted["outcome"]["closure"], "UNCLOSED", named)
                self.assertIn("census message", persisted["outcome"]["fault"], named)
                self.assertIsNone(persisted["deciding_census"], named)
                self.assertTrue(Path(ctx["scratch"]).is_dir())

    def test_every_publication_path_joins_the_census_first(self):
        # Structural guard for the reviewer's "all publication paths" point: the
        # join must precede the first publication in the run and in finalisation.
        run = inspect.getsource(bounded.Monitor.run).split("\n", 1)[1]
        joined = run.find("join_census(")
        decision = run.find('if decision == "cleanup"')
        self.assertGreaterEqual(joined, 0)
        self.assertGreaterEqual(decision, 0)
        self.assertLess(joined, decision, "run can publish a decision before it joins the census")
        finish = inspect.getsource(bounded.Monitor.finish_monitor).split("\n", 1)[1]
        self.assertGreaterEqual(finish.find("join_census("), 0)
        self.assertLess(finish.find("join_census("), finish.find("publish("),
                        "finalisation publishes before it joins the census")
        self.assertTrue(hasattr(bounded.Monitor, "refuse_incomplete_closure"))
        self.assertIn('if decision == "cleanup" and not settled:',
                      " ".join(inspect.getsource(bounded.Monitor.run).split()))

    def test_an_empty_frozen_prefix_is_reported_and_never_closed(self):
        empty = bounded.survivors_line([], "UNCLOSED", False)
        self.assertIn("no same-session survivor row was recorded", empty)
        self.assertIn("cannot show the run closed", empty)
        self.assertIsNone(bounded.survivors_line([], "UNCLOSED", True))
        self.assertIsNone(bounded.survivors_line([], None))
        # A contradictory pair (closed with an incomplete join) is never reported
        # as a closed run, with or without rows.
        for rows in ([], [{"pid": 900, "start": 5}]):
            line = bounded.survivors_line(rows, "closed", False)
            self.assertNotIn("so the run closed", line)
            self.assertTrue("the run is not closed" in line or "cannot show the run closed" in line,
                            line)

    def test_the_qualification_oracle_requires_the_survivor_record_and_no_signal(self):
        rows = survivor_case_rows("/x")
        self.assertEqual(self.survivor_oracle(rows), [], rows)
        self.assertTrue(any("recorded no case outcome" in p
                            for p in self.survivor_oracle([])))
        closed = [dict(rows[0], outcome={"closure": "closed", "deadline": True})]
        self.assertTrue(any("claimed closure" in p for p in self.survivor_oracle(closed)))
        early = [dict(rows[0], outcome={"closure": "UNCLOSED", "deadline": False})]
        self.assertTrue(any("did not reach the deadline" in p for p in self.survivor_oracle(early)))
        dropped = [dict(rows[0], survivors=[])]
        self.assertTrue(any("survivor row(s)" in p for p in self.survivor_oracle(dropped)))
        anonymous = [dict(rows[0], survivors=[{"pid": 0, "start": 0}])]
        self.assertTrue(any("without an identity" in p for p in self.survivor_oracle(anonymous)))
        signalled = [dict(rows[0], signal_targets=[4100])]
        self.assertTrue(any("signalled a non-held survivor" in p
                            for p in self.survivor_oracle(signalled)))
        no_census = [dict(rows[0], census=0)]
        self.assertTrue(any("no census member" in p for p in self.survivor_oracle(no_census)))

    def test_the_signal_custody_oracle_rejects_any_non_held_signal(self):
        receipt = {"phase": "monitor_complete: pending_custodian", "custodian": [
            {"event": "signal", "target": 9, "pin": "leader", "held_unreaped": True},
            {"event": "detach_child_signal", "pid": 4100, "start": 77}],
            "outcome": {"closure": "closed", "status": 0, "fault": None, "deadline": False}}
        self.assertEqual(self.custody_oracle(receipt), [])
        for mutation in ({"pin": "session-member"}, {"held_unreaped": False}):
            with self.subTest(mutation=mutation):
                broken = dict(receipt)
                broken["custodian"] = [dict(receipt["custodian"][0], **mutation)]
                self.assertTrue(any("without custody" in p for p in self.custody_oracle(broken)))
        extra = dict(receipt)
        extra["custodian"] = receipt["custodian"] + [
            {"event": "session_signal", "target": 4100, "start": 77, "killpg": False,
             "held_unreaped": False, "pin": "session-member", "signal": "SIGKILL"}]
        self.assertTrue(any("non-held session signal" in p for p in self.custody_oracle(extra)))
        anonymous = dict(receipt)
        anonymous["custodian"] = [{"event": "detach_child_signal", "pid": 0, "start": 0}]
        self.assertTrue(any("held identity" in p for p in self.custody_oracle(anonymous)))

    def custody_oracle(self, receipt):
        results = {"outcomes": [["zero", "pass"]], "cases": [{"case": "zero", "dir": "/x"}]}
        plan = dict(qualify.check_plan(receipt, results, {"source": "selected-volume"},
                                       None, KernelFactsForOracle(), None))
        return plan["signal-custody"]()

    def survivor_oracle(self, rows):
        receipt = {"phase": "monitor_complete: pending_custodian", "custodian": [],
                   "outcome": {"closure": "UNCLOSED", "status": -15, "fault": CENSUS_FAULT,
                               "deadline": True}}
        results = {"outcomes": [["zero", "pass"]], "cases": [{"case": "zero", "dir": "/x"}] + rows}
        plan = dict(qualify.check_plan(receipt, results, {"source": "selected-volume"},
                                       None, KernelFactsForOracle(), None))
        return plan["survivor-paths"]()


class TraversalSeam:
    """Test-only seams over `os.open`, `os.stat` and `os.close`, restored in
    `finally`. The monitor has no test hook of its own: these wrap the real
    calls, count their hits (so a seam that never fires fails its case), record
    every name opened (so an outside traversal is visible) and track every
    descriptor returned from an open against every close (so a leak is
    visible)."""

    def __init__(self, vanish_at_open=None, report_directory=None):
        self.vanish_at_open = vanish_at_open  # answer ENOENT for this component
        self.report_directory = report_directory  # report this component as a directory
        self.hits = {"vanish": 0, "relabel": 0}
        self.opened, self.live, self.saved = [], set(), None

    def __enter__(self):
        self.saved = (os.open, os.stat, os.close)
        real_open, real_stat, real_close = self.saved

        def open_(path, flags, mode=0o777, *, dir_fd=None):
            self.opened.append((path, dir_fd))
            if path == self.vanish_at_open:
                self.hits["vanish"] += 1
                raise FileNotFoundError(errno.ENOENT, os.strerror(errno.ENOENT), str(path))
            fd = real_open(path, flags, mode, dir_fd=dir_fd)
            self.live.add(fd)
            return fd

        def stat_(path, *, dir_fd=None, follow_symlinks=True):
            if path == self.report_directory:
                self.hits["relabel"] += 1
                meta = real_stat(path, dir_fd=dir_fd, follow_symlinks=False)
                # The entry's own identity, reported as a different kind: the
                # contradiction the no-follow open has to refuse.
                return types.SimpleNamespace(st_mode=stat.S_IFDIR | 0o755, st_dev=meta.st_dev,
                                             st_ino=meta.st_ino, st_blocks=meta.st_blocks)
            return real_stat(path, dir_fd=dir_fd, follow_symlinks=follow_symlinks)

        def close_(fd):
            self.live.discard(fd)
            return real_close(fd)

        os.open, os.stat, os.close = open_, stat_, close_
        return self

    def __exit__(self, *_exception):
        os.open, os.stat, os.close = self.saved
        return False

    def outside(self, marker="referent"):
        return [call for call in self.opened if marker in str(call[0])]


class PureTraversal(unittest.TestCase):
    """The monitor's owned-byte walk over owned paths whose absolute name
    cannot exist (oxide#142). It spawns nothing: the fixture lives under the
    suite's scratch and is removed by descriptor-relative names."""

    def setUp(self):
        self.dir = Path(tempfile.mkdtemp(prefix="tree.", dir=str(SCRATCH)))
        self.referent = self.dir / "referent"
        self.referent.mkdir()
        (self.referent / "big.bin").write_bytes(bytes(1024 * 1024))
        self.base = str(self.dir / "fixture")
        self.paths = long_fixture(self.base, str(self.referent))
        self.expected = oracle_bytes(self.base)

    def tearDown(self):
        shutil.rmtree(str(self.dir), ignore_errors=True)

    def test_a_path_past_the_limit_is_counted(self):
        with self.assertRaises(OSError) as caught:
            os.lstat(self.paths["leaf"])
        self.assertEqual(caught.exception.errno, errno.ENAMETOOLONG)
        # A byte length, not a character length: this name is past the limit in
        # bytes and well short of it in characters, so a character rule would
        # have looked it up by path and failed on it.
        self.assertGreaterEqual(len(os.fsencode(self.paths["leaf"])), LIMIT)
        self.assertLess(len(self.paths["leaf"]), LIMIT)
        self.assertEqual(bounded.tree_bytes(self.base), self.expected)
        self.assertGreaterEqual(self.expected, 4096)  # the overlong file is in it

    def test_a_directory_past_the_limit_is_read_and_a_hard_link_counted_once(self):
        with self.assertRaises(OSError) as caught:
            os.scandir(self.paths["deepest"]).close()
        self.assertEqual(caught.exception.errno, errno.ENAMETOOLONG)
        self.assertEqual(bounded.tree_bytes(self.base), self.expected)
        # The pair joins a nameable name to one past the limit, so the union,
        # not the path, is what keeps its inode in the total once.
        with self.assertRaises(OSError) as caught:
            os.lstat(self.paths["link"])
        self.assertEqual(caught.exception.errno, errno.ENAMETOOLONG)
        shared = os.stat(os.path.join(self.base, "short")).st_blocks * 512
        self.assertNotEqual(shared, 0)
        self.assertEqual(oracle_bytes(self.base, union=False) - self.expected, shared)

    def test_a_symlink_past_the_limit_is_never_followed(self):
        # The entry's name would be a directory if the referent were followed:
        # the walk counts the link itself and never opens another tree.
        referent = (self.referent / "big.bin").stat()
        self.assertLess(self.expected, referent.st_size, "the fixture is smaller than the referent")
        self.assertEqual(bounded.tree_bytes(self.base), self.expected)

    def test_an_unreadable_overlong_directory_is_a_measurement_fault(self):
        if os.geteuid() == 0:
            self.skipTest("root ignores directory permission bits")
        fd = bounded.open_relative(self.paths["anchor"], self.paths["suffix"])
        self.addCleanup(os.close, fd)
        os.fchmod(fd, 0o000)
        self.addCleanup(os.fchmod, fd, 0o700)
        with self.assertRaises(OSError) as caught:
            bounded.tree_bytes(self.base)
        self.assertIn(caught.exception.errno, (errno.EACCES, errno.EPERM))
        os.fchmod(fd, 0o700)  # a fault is never a smaller total, and never a stuck fixture
        self.assertEqual(bounded.tree_bytes(self.base), self.expected)

    def test_a_missing_root_counts_nothing(self):
        self.assertEqual(bounded.tree_bytes(str(self.dir / "absent")), 0)

    def test_an_ordinary_tree_still_matches_the_descriptor_oracle(self):
        plain = self.dir / "plain"
        (plain / "a" / "b").mkdir(parents=True)
        (plain / "a" / "b" / "file.bin").write_bytes(bytes(4096))
        (plain / "a" / "link").symlink_to(self.referent)
        os.link(str(plain / "a" / "b" / "file.bin"), str(plain / "a" / "twin.bin"))
        self.assertEqual(bounded.tree_bytes(str(plain)), oracle_bytes(str(plain)))

    def test_a_directory_that_vanishes_at_its_final_open_is_skipped(self):
        # The walk that counted everything, kept as this case's own control.
        with TraversalSeam() as control:
            self.assertEqual(bounded.tree_bytes(self.base), self.expected)
        self.assertEqual(control.hits, {"vanish": 0, "relabel": 0})
        self.assertEqual(control.live, set(), "a descriptor was left open")
        # The branch under the anchor disappears between its no-follow stat
        # and its no-follow open: the pathname read this replaces skipped that
        # ENOENT, so the injection must too, and only that one.
        branch = self.paths["suffix"][0]
        branch_fd = bounded.open_relative(self.paths["anchor"], (branch,))
        try:
            branch_own = os.fstat(branch_fd).st_blocks * 512
            children = oracle_bytes(branch_fd) - branch_own
        finally:
            os.close(branch_fd)
        self.assertGreater(children, 0, "the vanishing branch has no children to lose")
        with TraversalSeam(vanish_at_open=branch) as seam:
            total = bounded.tree_bytes(self.base)
        self.assertEqual(seam.hits["vanish"], 1)
        self.assertEqual(total, self.expected - children)
        self.assertEqual(seam.live, set(), "a descriptor was left open")
        self.assertEqual(seam.outside(), [], "the walk left the fixture")
        # The walk continued: both remaining overlong siblings are still in it.
        anchor_fd = bounded.open_relative(self.paths["anchor"], ())
        try:
            leaf_fd = os.open(os.path.basename(self.paths["leaf"]), os.O_RDONLY, dir_fd=anchor_fd)
            try:
                leaf_blocks = os.fstat(leaf_fd).st_blocks * 512
            finally:
                os.close(leaf_fd)
        finally:
            os.close(anchor_fd)
        pair_blocks = os.stat(os.path.join(self.base, "short")).st_blocks * 512
        self.assertGreaterEqual(total, leaf_blocks + pair_blocks)

    def test_a_directory_reported_where_a_symlink_sits_errors_without_following(self):
        base = str(self.dir / "seam")
        paths = seam_fixture(base, str(self.referent))
        expected = oracle_bytes(base)  # the link itself, never the referent
        self.assertLess(expected, (self.referent / "big.bin").stat().st_size)
        with TraversalSeam() as control:  # uninjected: link counted, nothing followed
            self.assertEqual(bounded.tree_bytes(base), expected)
        self.assertEqual(control.hits, {"vanish": 0, "relabel": 0})
        self.assertEqual(control.live, set(), "a descriptor was left open")
        # The observed kind says directory while the entry at that name is a
        # symlink: the no-follow open must refuse it, not read the referent.
        with TraversalSeam(report_directory=paths["tail"]) as seam:
            with self.assertRaises(OSError) as caught:
                bounded.tree_bytes(base)
        self.assertEqual(seam.hits["relabel"], 1)
        self.assertIn(caught.exception.errno, (errno.ELOOP, errno.ENOTDIR),
                      "a kind contradiction must fault, not become vanished")
        self.assertNotEqual(caught.exception.errno, errno.ENOENT)
        self.assertEqual(seam.live, set(), "a descriptor was left open")
        self.assertEqual(seam.outside(), [], "the walk opened the referent")

    def test_seams_whose_target_never_appears_change_nothing(self):
        # A seam that never fires must not make its case pass by accident: the
        # untouched walk still counts everything here, and the two injected
        # cases above assert their own hit count.
        base = str(self.dir / "seam")
        seam_fixture(base, str(self.referent))
        expected = oracle_bytes(base)
        with TraversalSeam(vanish_at_open="never-this-component",
                           report_directory="never-this-component") as seam:
            self.assertEqual(bounded.tree_bytes(base), expected)
        self.assertEqual(seam.hits, {"vanish": 0, "relabel": 0})
        self.assertEqual(seam.live, set(), "a descriptor was left open")
        self.assertGreater(len(seam.opened), 0, "the seam never observed an open")


# Default scratch parent (oxide#145) --------------------------------------------

BOOT, DRIVE = 1000, 2000
DRIVE_UUID = "a6ac3ed7-33ea-46b4-8082-7254cf291ed2"
OTHER_UUID = "11111111-2222-3333-4444-555555555555"
CONFIG = "/Users/owner/Library/Application Support/oxide/config/config.toml"


class FakeProbe:
    """Injected default-selection facts: no kernel read, no real lookup."""

    def __init__(self, nodes, documents=None, uuids=None, home="/Users/owner", euid=501,
                 realpaths=None):
        self.nodes = dict(nodes)  # path -> (device, mode, uid)
        self.documents = dict(documents or {})
        self.uuids = dict(uuids or {})
        self.home_dir = home
        self.uid = euid
        self.realpaths = dict(realpaths or {})

    def _node(self, path):
        try:
            return self.nodes[path]
        except KeyError:
            raise FileNotFoundError(errno.ENOENT, "No such file or directory", path)

    def stat(self, path):
        device, mode, uid = self._node(path)
        return types.SimpleNamespace(st_dev=device, st_mode=mode, st_uid=uid, st_size=0)

    def lstat(self, path):
        return self.stat(path)

    def realpath(self, path):
        return self.realpaths.get(path, path)

    def volume_uuid(self, path):
        self._node(path)
        return self.uuids.get(path)

    def euid(self):
        return self.uid

    def home(self):
        return self.home_dir

    def read(self, path, limit):
        if path not in self.documents:
            self._node(path)
        data = self.documents[path]
        if len(data) > limit:
            raise bounded.SelectionRefused(f"{path} is larger than {limit} bytes")
        return data


def registration_document(mount="/Volumes/Builds", uuid=DRIVE_UUID):
    return json.dumps({"version": 2, "mount_point": mount, "volume_uuid": uuid,
                       "container_uuid": OTHER_UUID, "reserve_bytes": 1024, "cap_bytes": 2048,
                       "quota_bytes": None, "capacity_bytes": 4096, "registered_ms": 17,
                       "mode": "permanent"}, indent=2).encode()


def default_world(pool="/Users/owner/Dev/oxide", registration=True, config=True, drive=True,
                  drive_device=DRIVE, drive_uuid=DRIVE_UUID, drive_mode=0o755, volumes_mode=0o755,
                  pool_device=BOOT, documents=None):
    """One fake machine: the boot volume, the drive and the pool's registration."""
    nodes = {"/": (BOOT, stat.S_IFDIR | 0o755, 0),
             "/Volumes": (BOOT, stat.S_IFDIR | volumes_mode, 0),
             "/private/tmp": (BOOT, stat.S_IFDIR | 0o777, 0),
             "/Users": (BOOT, stat.S_IFDIR | 0o755, 0),
             "/Users/owner": (BOOT, stat.S_IFDIR | 0o700, 501),
             "/Users/owner/Dev": (BOOT, stat.S_IFDIR | 0o755, 501),
             "/Users/owner/Dev/oxide": (pool_device, stat.S_IFDIR | 0o700, 501),
             pool: (pool_device, stat.S_IFDIR | 0o700, 501)}
    uuids = {"/": OTHER_UUID, "/Volumes": OTHER_UUID}
    if drive:
        nodes["/Volumes/Builds"] = (drive_device, stat.S_IFDIR | drive_mode, 501)
        uuids["/Volumes/Builds"] = drive_uuid
    documents = dict(documents or {})
    if config:
        documents[CONFIG] = b'[pool]\npath = "~/Dev/oxide"\nbudget = "100GB"\n'
    if registration:
        documents[os.path.join(pool, "oxide-managed", "removable.json")] = registration_document()
    return FakeProbe(nodes, documents, uuids)


class PureDefaultScratch(unittest.TestCase):
    """The default scratch parent: the selected volume when eligible, one
    bounded warned fallback otherwise, and the caller's own contexts kept."""

    def select(self, world, env=None):
        return bounded.select_default_parent(dict(env or {}), world)

    # Selected volume

    def test_registered_drive_is_the_default(self):
        facts = self.select(default_world())
        self.assertEqual(facts["source"], "selected-volume")
        self.assertEqual(facts["root_parent"], "/Volumes/Builds/scratch/oxide")
        self.assertEqual(facts["mount"], "/Volumes/Builds")
        self.assertEqual(facts["volume_uuid"], DRIVE_UUID)
        self.assertEqual(facts["device"], DRIVE)
        self.assertEqual(facts["pool"], "/Users/owner/Dev/oxide")
        self.assertEqual(facts["pool_source"], "account config")
        self.assertEqual(facts["registration"], "/Users/owner/Dev/oxide/oxide-managed/removable.json")
        self.assertIsNone(facts["reason"])

    def test_selection_reports_the_owned_subtree_without_creating_it(self):
        # Selection is effect-free: it names `<mount>/scratch/oxide`, never the
        # drive root, and neither the reported subtree nor its components exist
        # afterwards. The fake world has no such nodes, so any status read of
        # them would raise instead of silently passing.
        world = default_world()
        facts = self.select(world)
        self.assertEqual(facts["root_parent"], os.path.join("/Volumes/Builds", "scratch", "oxide"))
        self.assertNotIn("/Volumes/Builds/scratch", world.nodes, "selection invented the subtree")
        self.assertNotIn(facts["root_parent"], world.nodes)
        self.assertEqual(bounded.scratch_parent(facts["mount"]), facts["root_parent"])

    def test_pool_on_the_drive_uses_its_mount_root(self):
        world = default_world(pool="/Volumes/Builds/oxide-pool", registration=False, pool_device=DRIVE)
        facts = self.select(world, {"OXIDE_POOL": "/Volumes/Builds/oxide-pool"})
        self.assertEqual(facts["source"], "selected-volume")
        self.assertEqual(facts["root_parent"], "/Volumes/Builds/scratch/oxide")
        self.assertEqual(facts["mount"], "/Volumes/Builds")
        self.assertEqual(facts["registration"], None)
        self.assertEqual(facts["pool_source"], "OXIDE_POOL")

    def test_registration_beats_an_internal_pool(self):
        facts = self.select(default_world(pool_device=BOOT))
        self.assertEqual(facts["source"], "selected-volume")
        self.assertEqual(facts["root_parent"], "/Volumes/Builds/scratch/oxide")
        self.assertEqual(facts["mount"], "/Volumes/Builds")

    # Fallback with a bounded warning

    def test_internal_pool_without_registration_falls_back(self):
        facts = self.select(default_world(registration=False))
        self.assertEqual(facts["source"], "fallback")
        self.assertEqual(facts["root_parent"], bounded.FALLBACK_PARENT)
        self.assertIn("boot volume", facts["reason"])
        warning = bounded.fallback_warning(facts)
        self.assertLessEqual(len(warning.encode()), 512, "the warning is not bounded")
        self.assertEqual(warning.count("\n"), 1, "the warning is more than one line")
        self.assertIn(bounded.FALLBACK_PARENT, warning)
        self.assertIn(facts["reason"][:40], warning)

    def test_absent_drive_falls_back_without_creating_a_mount_path(self):
        facts = self.select(default_world(drive=False))
        self.assertEqual(facts["source"], "fallback")
        self.assertIn("FileNotFoundError", facts["reason"])
        self.assertIn("/Volumes/Builds", facts["reason"])
        self.assertIsNone(facts["mount"])

    def test_stale_mount_point_on_the_boot_volume_is_refused(self):
        facts = self.select(default_world(drive_device=BOOT))
        self.assertEqual(facts["source"], "fallback")
        self.assertIn("boot volume", facts["reason"])

    def test_a_different_volume_under_the_registered_name_is_refused(self):
        facts = self.select(default_world(drive_uuid=OTHER_UUID))
        self.assertEqual(facts["source"], "fallback")
        self.assertIn(DRIVE_UUID, facts["reason"])
        self.assertIn(OTHER_UUID, facts["reason"])

    def test_symlinked_mount_point_is_refused(self):
        world = default_world()
        world.nodes["/Volumes/Builds"] = (DRIVE, stat.S_IFLNK | 0o777, 501)
        facts = self.select(world)
        self.assertEqual(facts["source"], "fallback")
        self.assertIn("symlink", facts["reason"])

    def test_uncanonical_mount_point_is_refused(self):
        world = default_world()
        world.realpaths["/Volumes/Builds"] = "/private/tmp/Builds"
        facts = self.select(world)
        self.assertEqual(facts["source"], "fallback")
        self.assertIn("not canonical", facts["reason"])

    def test_unreadable_volume_identity_is_refused(self):
        facts = self.select(default_world(drive_uuid=None))
        self.assertEqual(facts["source"], "fallback")
        self.assertIn("no readable UUID", facts["reason"])

    def test_writable_mount_parent_is_refused(self):
        facts = self.select(default_world(volumes_mode=0o777))
        self.assertEqual(facts["source"], "fallback")
        self.assertIn("mount parent", facts["reason"])

    # Registration and config refusals

    def test_unreadable_registration_falls_back(self):
        for document, marks in ((b"{", "not JSON"), (b"[]", "not an object"),
                                (json.dumps({"version": 2}).encode(), "no recorded mount point"),
                                (b"x" * (64 * 1024 + 1), "larger than")):
            with self.subTest(document=document[:16]):
                world = default_world()
                world.documents["/Users/owner/Dev/oxide/oxide-managed/removable.json"] = document
                facts = self.select(world)
                self.assertEqual(facts["source"], "fallback")
                self.assertIn(marks, facts["reason"])

    def test_config_pool_path_forms(self):
        world = default_world(registration=False)
        for text, expected in ((b'[other]\npath = "/nowhere"\n', "/Users/owner/Library/Caches/oxide/build-pool"),
                               (b'[pool]\npath = \'~/Dev/oxide\'\n', "/Users/owner/Dev/oxide"),
                               (b'# note\n[pool]\npath = "~/Dev/oxide"\n', "/Users/owner/Dev/oxide")):
            with self.subTest(text=text):
                world.documents[CONFIG] = text
                facts = self.select(world)
                self.assertEqual(facts["pool"], expected)
        for text, marks in ((b'[pool]\npath = "~/Dev/oxi\\u0078de"\n', "unsupported escape"),
                            (b'[pool]\npath = 12\n', "unsupported pool path value"),
                            (b'[pool\npath = "~/Dev/oxide"\n', "unsupported table header"),
                            (b'[pool]\npath = "~/a"\npath = "~/b"\n', "repeated pool path")):
            with self.subTest(text=text):
                world.documents[CONFIG] = text
                facts = self.select(world)
                self.assertEqual(facts["source"], "fallback")
                self.assertIn(marks, facts["reason"])

    def test_an_absent_config_uses_the_built_in_pool(self):
        world = default_world(registration=False, config=False)
        facts = self.select(world)
        self.assertEqual(facts["pool"], "/Users/owner/Library/Caches/oxide/build-pool")
        self.assertEqual(facts["pool_source"], "built-in default pool")
        self.assertEqual(facts["source"], "fallback")  # that pool does not exist here

    # Caller contexts keep their own contracts

    def test_explicit_root_parent_and_adopted_scratch_are_not_defaults(self):
        self.assertIsNone(bounded.parse(["command"])[1]["root_parent"])
        self.assertEqual(bounded.parse(["--root-parent", "/x", "command"])[1]["root_parent"], "/x")
        self.assertEqual(
            bounded.parse(["--root-parent", "/x", "--evidence", "/e", "--limit", "40", "command"])[1],
            {"limit": 40.0, "budget": bounded.SCRATCH_BUDGET, "evidence": "/e", "root_parent": "/x"})
        self.assertIsNone(bounded.Custodian(["command"]).root_parent)

    def test_bootstrap_still_exercises_the_default(self):
        # The qualification must not mask default selection with --root-parent;
        # it finds the candidate receipt through the suite layout record instead.
        text = BOOTSTRAP.read_text()
        self.assertNotIn('"--root-parent"', text, "the bootstrap would mask default selection")
        self.assertNotIn('"--evidence"', text, "external evidence would sit outside the monitored root")
        self.assertIn('"OXIDE_SUPERVISOR_LAYOUT"', text, "the suite layout record is not wired")
        self.assertIn("prepare_scratch_parent", text, "the bootstrap does not prepare the owned parent")
        self.assertIn("sweep_concluded_roots", text, "the bootstrap does not sweep before its own base")

    def test_the_agents_unit_recipe_uses_the_owned_validation_parent(self):
        # The documented unit-suite recipe must derive its validation parent
        # from the pool selection's mount, not from the supervisor's nested
        # root_parent, not from the bare mount and not from a hardcoded drive
        # path; it must create that parent before one fresh run directory.
        text = (Path(__file__).resolve().parent.parent / "AGENTS.md").read_text()
        self.assertIn("validation_parent", text, "the recipe does not use the validation parent")
        self.assertNotIn("prepare_scratch_parent", text,
                         "the recipe would create the supervisor's marked subtree")
        # The mount path rule is about the RECIPE, so it is checked on the recipe's
        # own shell block: the file elsewhere records historical evidence paths
        # (an archived packet under a drive) that no recipe may reuse.
        blocks = re.findall(r"```sh\n(.*?)```", text, re.S)
        self.assertTrue(blocks, "the unit recipe has no shell block")
        recipe = next(block for block in blocks if "validation_parent" in block)
        self.assertIn("validation_parent", recipe)
        self.assertNotIn("/Volumes/Builds", recipe, "the recipe hardcodes a mount path")
        self.assertIn('mkdir -p "$scratch_base"', recipe)
        self.assertIn('mktemp -d "$scratch_base/run.XXXXXX"', recipe)

    # The owned leaf

    def leaf_world(self, **facts):
        sandbox = Path(tempfile.mkdtemp(prefix="leaf.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(sandbox), True)
        parent, leaf = str(sandbox), str(sandbox / "leaf")
        mode = facts.get("mode", stat.S_IFDIR | 0o700)
        nodes = {parent: (BOOT, stat.S_IFDIR | 0o700, 501),
                 leaf: (facts.get("device", BOOT), mode, facts.get("uid", 501))}
        probe = FakeProbe(nodes, euid=501, realpaths={leaf: facts.get("realpath", leaf)})

        def make_dir(prefix, dir):
            os.mkdir(leaf, 0o700)
            return leaf

        return parent, leaf, probe, make_dir

    def test_owned_leaf_is_private_canonical_and_on_the_parent_device(self):
        parent, leaf, probe, make_dir = self.leaf_world()
        self.assertEqual(bounded.create_owned_root(parent, probe, make_dir), leaf)
        self.assertTrue(os.path.isdir(leaf))
        info = os.lstat(leaf)
        self.assertEqual(info.st_uid, os.geteuid())
        self.assertEqual(stat.S_IMODE(info.st_mode), 0o700)

    def test_unusable_owned_leaf_is_refused_and_removed(self):
        cases = {"not private": {"mode": stat.S_IFDIR | 0o755},
                 "not private to this user": {"uid": 0},
                 "not on the parent's device": {"device": DRIVE},
                 "not canonical under": {"realpath": "/private/tmp/leaf"},
                 "not a real directory": {"mode": stat.S_IFLNK | 0o777}}
        for marks, facts in cases.items():
            with self.subTest(marks):
                parent, leaf, probe, make_dir = self.leaf_world(**facts)
                with self.assertRaises(bounded.SelectionRefused) as caught:
                    bounded.create_owned_root(parent, probe, make_dir)
                self.assertIn(marks, str(caught.exception))
                self.assertFalse(os.path.lexists(leaf), "the refused leaf was kept")

    def test_a_symlinked_root_parent_is_refused(self):
        parent, leaf, probe, make_dir = self.leaf_world()
        probe.nodes[parent] = (BOOT, stat.S_IFLNK | 0o777, 501)
        with self.assertRaises(bounded.SelectionRefused) as caught:
            bounded.create_owned_root(parent, probe, make_dir)
        self.assertIn("not a real directory", str(caught.exception))
        self.assertFalse(os.path.lexists(leaf))


class FixtureProbe:
    """Real filesystem facts over one owned sandbox, with injected device,
    ownership, canonical-path and read errors so each refusal is provable
    without touching any real volume."""

    def __init__(self, euid=None, devices=None, owners=None, realpaths=None, errors=None):
        self.uid = os.geteuid() if euid is None else euid
        self.devices = dict(devices or {})
        self.owners = dict(owners or {})
        self.realpaths = dict(realpaths or {})
        self.errors = dict(errors or {})

    def _info(self, path):
        if path in self.errors:
            raise self.errors[path]
        return os.lstat(path)

    @staticmethod
    def _node(info, device, owner):
        return types.SimpleNamespace(st_dev=device, st_ino=info.st_ino, st_mode=info.st_mode,
                                     st_uid=owner, st_size=info.st_size)

    def lstat(self, path):
        info = self._info(path)
        return self._node(info, self.devices.get(path, info.st_dev), self.owners.get(path, info.st_uid))

    def stat(self, path):
        return self.lstat(path)

    def realpath(self, path):
        return self.realpaths.get(path, os.path.realpath(path))

    def volume_uuid(self, path):
        return None

    def euid(self):
        return self.uid

    def home(self):
        return "/Users/owner"

    def read(self, path, limit):
        info = self._info(path)
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
            raise bounded.SelectionRefused(f"{path} is not a regular file")
        if info.st_size > limit:
            raise bounded.SelectionRefused(f"{path} is larger than {limit} bytes")
        with open(path, "rb") as handle:
            return handle.read(limit + 1)


class PureScratchPreparation(unittest.TestCase):
    """The selected volume's scratch pair: a SHARED, pre-existing ancestor
    `<mount>/scratch` (this user, selected device, no group/other WRITE; 0755 is
    normal) and Oxide's PRIVATE leaf `<mount>/scratch/oxide` (exactly 0700).
    Only the leaf is ever created; the ancestor is never created, chmodded,
    adopted or removed."""

    def world(self, ancestor_mode=0o755, leaf_mode=None):
        mount = Path(tempfile.mkdtemp(prefix="prepare.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(mount), True)
        selection = {"source": "selected-volume", "mount": str(mount),
                     "root_parent": bounded.scratch_parent(str(mount)),
                     "device": os.stat(str(mount)).st_dev, "reason": None}
        scratch = os.path.join(str(mount), "scratch")
        if ancestor_mode is not None:
            os.mkdir(scratch)
            os.chmod(scratch, ancestor_mode)
        if leaf_mode is not None:
            os.mkdir(os.path.join(scratch, "oxide"))
            os.chmod(os.path.join(scratch, "oxide"), leaf_mode)
        return str(mount), selection

    def test_a_0755_shared_ancestor_is_accepted_and_only_the_leaf_is_created(self):
        mount, selection = self.world(0o755)
        scratch = os.path.join(mount, "scratch")
        before = os.lstat(scratch)
        parent = bounded.prepare_scratch_parent(selection, FixtureProbe())
        self.assertEqual(parent, os.path.join(scratch, "oxide"))
        after = os.lstat(scratch)
        self.assertEqual((after.st_ino, stat.S_IMODE(after.st_mode)), (before.st_ino, 0o755),
                         "the shared ancestor was replaced or chmodded")
        leaf = os.lstat(parent)
        self.assertEqual(stat.S_IMODE(leaf.st_mode), 0o700)
        self.assertEqual(leaf.st_uid, os.geteuid())
        self.assertEqual(os.listdir(scratch), ["oxide"], "preparation created another entry")
        self.assertEqual(os.listdir(parent), [], "preparation created a run resource")

    def test_a_0700_shared_ancestor_is_also_accepted(self):
        mount, selection = self.world(0o700)
        parent = bounded.prepare_scratch_parent(selection, FixtureProbe())
        self.assertEqual(stat.S_IMODE(os.lstat(os.path.join(mount, "scratch")).st_mode), 0o700)
        self.assertEqual(stat.S_IMODE(os.lstat(parent).st_mode), 0o700)

    def test_sticky_does_not_substitute_for_non_writability(self):
        mount, selection = self.world(0o1777)  # sticky AND group/other writable
        with self.assertRaises(bounded.SelectionRefused) as caught:
            bounded.prepare_scratch_parent(selection, FixtureProbe())
        self.assertIn("group/other writable", str(caught.exception))
        self.assertIn("sticky", str(caught.exception))
        self.assertFalse(os.path.lexists(selection["root_parent"]), "a refused ancestor still got a leaf")
        self.assertEqual(stat.S_IMODE(os.lstat(os.path.join(mount, "scratch")).st_mode), 0o1777,
                         "the refused shared ancestor was chmodded")
        mount, selection = self.world(0o1755)  # sticky, readable, not writable
        parent = bounded.prepare_scratch_parent(selection, FixtureProbe())
        self.assertEqual(stat.S_IMODE(os.lstat(parent).st_mode), 0o700)

    def test_unsafe_shared_ancestors_refuse_before_any_leaf_effect(self):
        cases = {
            "is a symlink": dict(build="symlink"),
            "is not a directory": dict(build="file"),
            "is not owned by this user": dict(build="dir", owners=0),
            "is not on the selected device": dict(build="dir", device=1),
            "is not canonical": dict(build="dir", realpaths="/private/tmp/shared-scratch"),
            "is unreadable": dict(build="dir", error=errno.EACCES),
            "is group/other writable": dict(build="mode", mode=0o775),
            "is group/other writable, other": dict(build="mode", mode=0o703),
        }
        for marks, spec in cases.items():
            with self.subTest(marks):
                mount, selection = self.world(ancestor_mode=None)
                target = os.path.join(mount, "scratch")
                if spec["build"] == "symlink":
                    os.mkdir(os.path.join(mount, "elsewhere"), 0o755)
                    os.symlink(os.path.join(mount, "elsewhere"), target)
                elif spec["build"] == "file":
                    Path(target).write_bytes(b"")
                else:
                    os.mkdir(target)
                    os.chmod(target, spec.get("mode", 0o755))
                before = (os.lstat(target).st_ino, stat.S_IMODE(os.lstat(target).st_mode))
                probe = FixtureProbe(
                    owners={target: spec["owners"]} if "owners" in spec else None,
                    devices={target: spec["device"]} if "device" in spec else None,
                    realpaths={target: spec["realpaths"]} if "realpaths" in spec else None,
                    errors={target: PermissionError(spec["error"], "denied", target)}
                    if "error" in spec else None)
                with self.assertRaises(bounded.SelectionRefused) as caught:
                    bounded.prepare_scratch_parent(selection, probe)
                self.assertIn(marks.split(",")[0], str(caught.exception))
                self.assertTrue(os.path.lexists(target), "the refused ancestor was removed")
                self.assertEqual((os.lstat(target).st_ino, stat.S_IMODE(os.lstat(target).st_mode)),
                                 before, "the refused ancestor was mutated")
                self.assertFalse(os.path.lexists(selection["root_parent"]),
                                 "a refused ancestor still got a leaf")

    def test_a_missing_shared_ancestor_refuses_and_creates_nothing(self):
        mount, selection = self.world(ancestor_mode=None)
        calls = []

        def make_dir(path, mode):
            calls.append(path)
            os.mkdir(path, mode)

        with self.assertRaises(bounded.SelectionRefused) as caught:
            bounded.prepare_scratch_parent(selection, FixtureProbe(), make_dir)
        self.assertIn("is absent", str(caught.exception))
        self.assertIn("never creates it", str(caught.exception))
        self.assertEqual(calls, [], "the shared ancestor was created")
        self.assertEqual(os.listdir(mount), [], "a refused preparation created an entry")

    def test_the_private_leaf_must_be_exactly_0700_and_is_never_repaired(self):
        cases = {
            "0755": 0o755,          # no group/other write, still not 0700
            "0750": 0o750,
            "01700": 0o1700,
        }
        for marks, mode in cases.items():
            with self.subTest(marks):
                mount, selection = self.world(0o755, leaf_mode=mode)
                leaf = selection["root_parent"]
                with self.assertRaises(bounded.SelectionRefused) as caught:
                    bounded.prepare_scratch_parent(selection, FixtureProbe())
                self.assertIn("is not private", str(caught.exception))
                self.assertIn("0o700 required", str(caught.exception))
                self.assertEqual(stat.S_IMODE(os.lstat(leaf).st_mode), mode, "a refused leaf was chmodded")
        for marks, probe in (("is not owned by this user", FixtureProbe()),
                             ("is a symlink", FixtureProbe()),
                             ("is not on the selected device", FixtureProbe(devices={}))):
            with self.subTest(marks):
                mount, selection = self.world(0o755)
                leaf = selection["root_parent"]
                if marks == "is not owned by this user":
                    probe = FixtureProbe(owners={leaf: 0})
                elif marks == "is a symlink":
                    os.mkdir(os.path.join(mount, "leaf-referent"), 0o700)
                    os.symlink(os.path.join(mount, "leaf-referent"), leaf)
                else:
                    probe = FixtureProbe(devices={leaf: 1})
                with self.assertRaises(bounded.SelectionRefused) as caught:
                    bounded.prepare_scratch_parent(selection, probe)
                self.assertIn(marks, str(caught.exception))
                self.assertTrue(os.path.lexists(leaf), "the refused leaf was removed")

    def test_an_absent_leaf_is_created_0700_and_reverified(self):
        mount, selection = self.world(0o755)
        parent = bounded.prepare_scratch_parent(selection, FixtureProbe())
        info = os.lstat(parent)
        self.assertEqual((stat.S_IMODE(info.st_mode), info.st_uid), (0o700, os.geteuid()))
        self.assertEqual(info.st_dev, os.stat(mount).st_dev)
        # A post-create identity change refuses rather than being trusted.
        original = os.lstat(parent)
        forged = types.SimpleNamespace(st_dev=original.st_dev, st_ino=original.st_ino + 1,
                                       st_mode=original.st_mode, st_uid=original.st_uid,
                                       st_size=original.st_size)
        mount, selection = self.world(0o755)

        class MovingProbe(FixtureProbe):
            calls = 0

            def lstat(self, path):
                if path == selection["root_parent"]:
                    MovingProbe.calls += 1
                    if MovingProbe.calls > 2:  # validated read, then a changed identity
                        return forged
                return super().lstat(path)

        with self.assertRaises(bounded.SelectionRefused) as caught:
            bounded.prepare_scratch_parent(selection, MovingProbe())
        self.assertIn("changed while it was being prepared", str(caught.exception))

    def test_a_concurrent_creator_is_validated_not_adopted(self):
        mount, selection = self.world(0o755)
        leaf = selection["root_parent"]

        def safe(path, mode):
            os.mkdir(path, 0o700)
            raise FileExistsError(path)  # another run won the race

        self.assertEqual(bounded.prepare_scratch_parent(selection, FixtureProbe(), safe), leaf)

        def unsafe(path, mode):
            os.mkdir(path, 0o755)
            raise FileExistsError(path)

        os.rmdir(leaf)
        with self.assertRaises(bounded.SelectionRefused) as caught:
            bounded.prepare_scratch_parent(selection, FixtureProbe(), unsafe)
        self.assertIn("is not private", str(caught.exception))
        self.assertEqual(stat.S_IMODE(os.lstat(leaf).st_mode), 0o755, "a foreign leaf was chmodded")

    def test_readiness_uses_the_same_policy_without_creating_anything(self):
        def ready_world():
            mount, selection = self.world(0o755)
            return mount, selection

        mount, selection = ready_world()
        result = bounded.readiness(selection, FixtureProbe())
        self.assertTrue(result["ready"])
        self.assertEqual(result["leaf"]["state"], "absent; would create 0700")
        self.assertIn("absent", result["leaf"]["state"])
        self.assertNotIn("exists", result["leaf"]["state"])
        self.assertFalse(os.path.lexists(selection["root_parent"]), "readiness created the leaf")
        self.assertEqual(sorted(os.listdir(os.path.join(mount, "scratch"))), [], "readiness created an entry")

        mount, selection = ready_world()
        bounded.prepare_scratch_parent(selection, FixtureProbe())
        again = bounded.readiness(selection, FixtureProbe())
        self.assertTrue(again["ready"])
        self.assertEqual(again["leaf"]["state"], "exists")
        self.assertEqual(again["leaf"]["mode"], "0o700")

        for label, world in (("group writable", 0o775), ("missing ancestor", None),
                             ("sticky writable", 0o1777)):
            with self.subTest(label):
                mount, selection = world_args = ready_world() if world is not None else \
                    (None, None)
                if world is None:
                    mount, selection = self.world(ancestor_mode=None)
                else:
                    mount, selection = self.world(world)
                result = bounded.readiness(selection, FixtureProbe())
                self.assertFalse(result["ready"], result)
                self.assertTrue(result["reason"])
                self.assertFalse(os.path.lexists(selection["root_parent"]),
                                 "a refused readiness still created the leaf")
                if world is None:
                    self.assertIn("absent", result["reason"])
                    self.assertEqual(os.listdir(mount), [], "readiness created the ancestor")
                with self.assertRaises(bounded.SelectionRefused):
                    bounded.prepare_scratch_parent(selection, FixtureProbe())

    def test_readiness_refuses_without_a_selected_volume(self):
        fallback = {"source": "fallback", "mount": None, "root_parent": bounded.FALLBACK_PARENT,
                    "reason": "the volume holding the pool is the boot volume"}
        result = bounded.readiness(fallback, FixtureProbe())
        self.assertFalse(result["ready"])
        self.assertIn("boot volume", result["reason"])

    def test_contract_counterfactuals(self):
        # (a) the superseded both-private-parent rule would refuse the shared
        # ancestor the owner's contract now accepts; the accepted leaf is exact
        # 0700, so privacy lives at the leaf.
        mount, selection = self.world(0o755)
        parent = bounded.prepare_scratch_parent(selection, FixtureProbe())
        self.assertEqual(stat.S_IMODE(os.lstat(os.path.join(mount, "scratch")).st_mode), 0o755)
        self.assertEqual(stat.S_IMODE(os.lstat(parent).st_mode), 0o700)
        # (b) allowing a writable shared ancestor is refused, sticky or not.
        for mode in (0o775, 0o757, 0o1777):
            with self.subTest(mode=oct(mode)):
                mount, selection = self.world(mode)
                with self.assertRaises(bounded.SelectionRefused):
                    bounded.prepare_scratch_parent(selection, FixtureProbe())
                self.assertFalse(os.path.lexists(selection["root_parent"]))
        # (c) weakening the leaf below exact 0700 is refused even when no
        # group/other WRITE bit is set.
        mount, selection = self.world(0o755, leaf_mode=0o750)
        with self.assertRaises(bounded.SelectionRefused) as caught:
            bounded.prepare_scratch_parent(selection, FixtureProbe())
        self.assertIn("0o700 required", str(caught.exception))

    def test_the_mount_and_the_reported_parent_are_revalidated(self):
        mount, selection = self.world(0o755)
        link = os.path.join(mount, "link")
        os.symlink(mount, link)
        root_parent = bounded.scratch_parent(link)
        with self.assertRaises(bounded.SelectionRefused) as caught:
            bounded.prepare_scratch_parent({"source": "selected-volume", "mount": link,
                                            "root_parent": root_parent, "device": 1}, FixtureProbe())
        self.assertIn("not a real directory", str(caught.exception))
        with self.assertRaises(bounded.SelectionRefused) as caught:
            bounded.prepare_scratch_parent({"source": "selected-volume", "mount": mount,
                                            "root_parent": mount, "device": os.stat(mount).st_dev},
                                           FixtureProbe())
        self.assertIn("is not", str(caught.exception))
        with self.assertRaises(bounded.SelectionRefused) as caught:
            bounded.prepare_scratch_parent({"source": "fallback", "mount": None,
                                            "root_parent": bounded.FALLBACK_PARENT, "device": None},
                                           FixtureProbe())
        self.assertIn("no selected volume", str(caught.exception))

    def test_the_owned_validation_parent_derives_from_the_mount(self):
        # The unit-validation temp parent is a sibling of the supervisor's
        # marked subtree: `<mount>/scratch/oxide-validation`, never the nested
        # root_parent with a second `scratch` appended and never the drive root.
        selected = {"source": "selected-volume", "mount": "/Volumes/Builds",
                    "root_parent": "/Volumes/Builds/scratch/oxide", "device": 1}
        parent = bounded.validation_parent(selected)
        self.assertEqual(parent, "/Volumes/Builds/scratch/oxide-validation")
        self.assertEqual(parent.count("/scratch/"), 1, "a duplicate nested scratch component")
        self.assertNotIn("oxide-validation/scratch", parent)
        mount, sandbox_selection = self.world(ancestor_mode=None)
        sandbox_parent = bounded.validation_parent(sandbox_selection)
        self.assertEqual(sandbox_parent, os.path.join(mount, "scratch", "oxide-validation"))
        self.assertEqual(sandbox_parent[len(mount):].count("/scratch/"), 1)
        self.assertFalse(os.path.lexists(sandbox_parent), "naming the validation parent created it")
        self.assertFalse(os.path.lexists(os.path.join(mount, "scratch")),
                         "naming the validation parent created its parent")
        fallback = {"source": "fallback", "mount": None, "root_parent": bounded.FALLBACK_PARENT,
                    "reason": "no eligible selected volume"}
        self.assertEqual(bounded.validation_parent(fallback), bounded.FALLBACK_PARENT)
        self.assertEqual(bounded.validation_parent({"source": "fallback", "mount": None}), bounded.FALLBACK_PARENT)

    def test_a_creation_refusal_keeps_the_warned_fallback_contract(self):
        mount, selection = self.world(0o755)

        def refuse(path, mode):
            raise PermissionError(errno.EACCES, "denied", path)

        with self.assertRaises(bounded.SelectionRefused) as caught:
            bounded.prepare_scratch_parent(selection, FixtureProbe(), refuse)
        self.assertIn("cannot be created", str(caught.exception))
        self.assertEqual(os.listdir(os.path.join(mount, "scratch")), [])


OLD = dict(concluded_unix=1000.0, outcome="closed", closure="positive", kept_reason="evidence")
NOW = 1000.0 + bounded.EXPIRY_S + 1




class PureMarker(unittest.TestCase):
    """The owner marker: strict schema, atomic private write, genuine
    conclusion only, and no liveness claim."""

    def root(self, name="oxide-proof.aaa"):
        sandbox = Path(tempfile.mkdtemp(prefix="marker.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(sandbox), True)
        root = os.path.join(str(sandbox), name)
        os.mkdir(root, 0o700)
        return root

    def put(self, root, payload, mode=0o600):
        path = os.path.join(root, bounded.MARKER_NAME)
        with open(path, "wb") as handle:
            handle.write(payload if isinstance(payload, bytes) else json.dumps(payload).encode())
        os.chmod(path, mode)
        return path

    def document(self, root, **changes):
        document = bounded.marker_document(bounded.new_run_id(), root, FixtureProbe(), created_unix=100.0)
        document.update(changes)
        return document

    def test_the_initial_marker_round_trips_privately(self):
        root = self.root()
        document = self.document(root)
        path = bounded.write_marker(root, document, FixtureProbe())
        self.assertEqual(path, os.path.join(root, bounded.MARKER_NAME))
        self.assertEqual(bounded.read_marker(root, FixtureProbe()), document)
        self.assertEqual(stat.S_IMODE(os.lstat(path).st_mode), 0o600)
        self.assertEqual(sorted(os.listdir(root)), [bounded.MARKER_NAME], "a temporary file was left")
        self.assertIsNone(document["concluded_unix"])
        self.assertEqual(document["outcome"], "running")
        self.assertEqual(set(document), set(bounded.MARKER_KEYS))

    def test_every_schema_mutation_fails_closed(self):
        def mutations(root):
            document = self.document(root)
            return {
                "schema": [dict(document, extra=1),
                           {key: value for key, value in document.items() if key != "inode"}],
                "version": [dict(document, version=2), dict(document, version=True)],
                "run-id": [dict(document, run_id="zz"),
                           dict(document, run_id=document["run_id"].upper())],
                "identity": [dict(document, root="/private/tmp/elsewhere"), dict(document, device="1"),
                             dict(document, inode=0)],
                "owner": [dict(document, owner_uid=0), dict(document, owner_uid=True)],
                "created": [dict(document, created_unix=-1), dict(document, created_unix=float("nan")),
                            dict(document, created_unix=float("inf"))],
                "concluded": [dict(document, concluded_unix="x"), dict(document, concluded_unix=-2)],
                "state": [dict(document, outcome="weird"), dict(document, closure="weird"),
                          dict(document, kept_reason="weird"),
                          dict(document, concluded_unix=1000.0, outcome="unclosed", closure="positive"),
                          dict(document, concluded_unix=1000.0, outcome="closed", closure="none"),
                          dict(document, closure="positive")],
            }
        for code in ("schema", "version", "run-id", "identity", "owner", "created", "concluded",
                     "state"):
            for index in range(len(mutations(self.root())[code])):
                with self.subTest(code=code, index=index):
                    root = self.root()
                    mutation = mutations(root)[code][index]
                    self.put(root, mutation)
                    with self.assertRaises(bounded.MarkerInvalid) as caught:
                        bounded.read_marker(root, FixtureProbe())
                    self.assertEqual(caught.exception.code, code)
        root = self.root()
        for code, payload in (("not-json", b"{"), ("oversized", b"x" * (bounded.MARKER_MAX_BYTES + 1))):
            with self.subTest(code=code):
                self.put(root, payload)
                with self.assertRaises(bounded.MarkerInvalid) as caught:
                    bounded.read_marker(root, FixtureProbe())
                self.assertEqual(caught.exception.code, code)

    def test_an_absent_foreign_or_linked_marker_is_never_trusted(self):
        root = self.root()
        with self.assertRaises(bounded.MarkerInvalid) as caught:
            bounded.read_marker(root, FixtureProbe())
        self.assertEqual(caught.exception.code, "absent")
        document = self.document(root)
        path = self.put(root, document)
        for owner, code in ((0, "foreign"),):
            with self.subTest(code=code):
                with self.assertRaises(bounded.MarkerInvalid) as caught:
                    bounded.read_marker(root, FixtureProbe(owners={path: owner}))
                self.assertEqual(caught.exception.code, code)
        os.chmod(path, 0o644)
        with self.assertRaises(bounded.MarkerInvalid) as caught:
            bounded.read_marker(root, FixtureProbe())
        self.assertEqual(caught.exception.code, "not-private")
        self.put(root, document)
        os.unlink(path)
        os.symlink(os.path.join(root, "elsewhere"), path)
        with self.assertRaises(bounded.MarkerInvalid) as caught:
            bounded.read_marker(root, FixtureProbe())
        self.assertEqual(caught.exception.code, "not-regular")

    def test_writes_refuse_an_unsafe_root_and_leave_nothing_behind(self):
        root = self.root()
        document = self.document(root)
        with self.assertRaises(bounded.MarkerInvalid) as caught:
            bounded.write_marker(root, dict(document, extra=1), FixtureProbe())
        self.assertEqual(caught.exception.code, "schema")
        self.assertEqual(os.listdir(root), [], "a refused write left a file")
        foreign = FixtureProbe(owners={root: 0})
        with self.assertRaises(bounded.MarkerInvalid) as caught:
            bounded.write_marker(root, document, foreign)
        self.assertEqual(caught.exception.code, "root-owner")
        link = os.path.join(str(Path(root).parent), "link")
        os.symlink(root, link)
        with self.assertRaises(bounded.MarkerInvalid) as caught:
            bounded.write_marker(link, self.document(link), FixtureProbe())
        self.assertIn(caught.exception.code, ("root", "identity"))

    def test_only_a_genuine_conclusion_marks_a_root_concluded(self):
        root = self.root()
        run_id = bounded.new_run_id()
        bounded.write_marker(root, self.document(root, run_id=run_id), FixtureProbe())
        concluded = bounded.record_conclusion(root, run_id, FixtureProbe(), "closed", "positive",
                                              "evidence", wall=500.0)
        self.assertEqual(concluded["concluded_unix"], 500.0)
        self.assertEqual((concluded["outcome"], concluded["closure"], concluded["kept_reason"]),
                         ("closed", "positive", "evidence"))
        self.assertEqual(bounded.read_marker(root, FixtureProbe()), concluded)
        for outcome, closure in (("unclosed", "positive"), ("closed", "none"), ("running", "none")):
            with self.subTest(outcome=outcome, closure=closure):
                with self.assertRaises(bounded.MarkerInvalid):
                    bounded.record_conclusion(root, run_id, FixtureProbe(), outcome, closure, "evidence")
        with self.assertRaises(bounded.MarkerInvalid) as caught:
            bounded.record_conclusion(root, bounded.new_run_id(), FixtureProbe(), "closed", "positive",
                                      "evidence")
        self.assertEqual(caught.exception.code, "identity")
        self.assertEqual(bounded.read_marker(root, FixtureProbe())["outcome"], "closed",
                         "a refused conclusion changed the marker")

    def test_a_hosting_root_is_never_concluded_from_outer_closure(self):
        root = self.root()
        probe = FixtureProbe()
        run_id = bounded.new_run_id()
        bounded.write_marker(root, self.document(root, run_id=run_id), probe)
        self.assertEqual(bounded.read_marker(root, probe)["nested_scope"], "exclusive")
        protected = bounded.protect_nested_scope(root, probe)
        self.assertEqual(protected["nested_scope"], "hosting")
        identity = bounded.marker_identity(root, probe)
        # Re-protecting is idempotent and rewrites nothing.
        again = bounded.protect_nested_scope(root, probe)
        self.assertEqual(again, protected)
        self.assertEqual(bounded.marker_identity(root, probe), identity)
        # The run's own outer closure cannot conclude a hosting root.
        with self.assertRaises(bounded.MarkerInvalid) as caught:
            bounded.record_conclusion(root, run_id, probe, "closed", "positive", "evidence")
        self.assertEqual(caught.exception.code, "nested-unconcluded")
        unchanged = bounded.read_marker(root, probe)
        self.assertEqual((unchanged["outcome"], unchanged["concluded_unix"],
                          unchanged["kept_reason"]), ("running", None, "none"))
        # A truthful non-conclusion is allowed, and only the owning authority
        # can record the positive conclusion of every owned resource.
        retained = bounded.record_conclusion(root, run_id, probe, "retained", "none", "nested-unclosed")
        self.assertIsNone(retained["concluded_unix"])
        self.assertEqual(retained["kept_reason"], "nested-unclosed")
        with self.assertRaises(bounded.MarkerInvalid):
            bounded.protect_nested_scope(root, probe)  # never downgrades a recorded state

    def test_a_new_root_is_never_adopted_and_a_race_never_loses_protection(self):
        root = self.root()
        probe = FixtureProbe()
        with self.assertRaises(bounded.MarkerInvalid) as caught:
            bounded.protect_nested_scope(root, probe)  # unmarked directories are never adopted
        self.assertEqual(caught.exception.code, "absent")
        self.assertEqual(os.listdir(root), [], "an unmarked root gained a marker")
        run_id = bounded.new_run_id()
        bounded.write_marker(root, self.document(root, run_id=run_id), probe)
        with self.assertRaises(bounded.MarkerInvalid) as caught:
            bounded.write_marker(root, self.document(root, run_id=run_id), probe)  # initial only
        self.assertEqual(caught.exception.code, "exists")
        # A conclusion that read the marker before protection cannot overwrite
        # the protection: the marker identity changed, so it refuses.
        stale, stale_identity = bounded.read_marker_with_identity(root, probe)
        bounded.protect_nested_scope(root, probe)
        original = bounded.read_marker_with_identity

        def stale_read(path, probe_):
            if path == root:
                return stale, stale_identity
            return original(path, probe_)

        with patched(bounded, "read_marker_with_identity", stale_read):
            with self.assertRaises(bounded.MarkerInvalid) as caught:
                bounded.record_conclusion(root, run_id, probe, "closed", "positive", "evidence",
                                          nested_concluded=True)
        self.assertEqual(caught.exception.code, "changed")
        final = bounded.read_marker(root, probe)
        self.assertEqual((final["nested_scope"], final["outcome"], final["concluded_unix"]),
                         ("hosting", "running", None))

    def test_the_marker_records_a_conclusion_not_a_process_state(self):
        # No marker field names a PID, a signal or a liveness fact: expiry reads
        # only the recorded conclusion, and this surely-alive process does not
        # change or refresh an existing conclusion.
        self.assertFalse({"pid", "start", "alive", "signal", "session"} & set(bounded.MARKER_KEYS))
        root = self.root()
        run_id = bounded.new_run_id()
        bounded.write_marker(root, self.document(root, run_id=run_id), FixtureProbe())
        concluded = bounded.record_conclusion(root, run_id, FixtureProbe(), "closed", "positive",
                                              "evidence", wall=500.0)
        self.assertEqual(os.getpid() > 0, True)
        self.assertEqual(bounded.read_marker(root, FixtureProbe()), concluded)


class PureSweep(unittest.TestCase):
    """The one bounded startup sweep: only real owner-marked roots positively
    concluded more than seven days ago, never anything else, and never by
    following a link."""

    def parent(self):
        sandbox = Path(tempfile.mkdtemp(prefix="sweep.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(sandbox), True)
        return str(sandbox), os.stat(str(sandbox)).st_dev

    def root(self, parent, name="oxide-proof.aaa", payload=2, **state):
        path = os.path.join(parent, name)
        os.mkdir(path, 0o700)
        document = bounded.marker_document(bounded.new_run_id(), path, FixtureProbe(), created_unix=10.0)
        document.update(OLD, **state)
        bounded.write_marker(path, document, FixtureProbe())
        for index in range(payload):
            folder = os.path.join(path, f"evidence-{index}", "nested")
            os.makedirs(folder)
            Path(folder, "receipt.json").write_text("{}")
        return path, document

    def sweep(self, parent, device, **options):
        return bounded.sweep_concluded_roots(parent, device, options.pop("probe", FixtureProbe()),
                                             now=options.pop("now", NOW), **options)

    def rows(self, summary):
        return {row["name"]: row["result"] for row in summary["rows"]}

    def test_a_positively_concluded_root_older_than_seven_days_is_removed(self):
        parent, device = self.parent()
        path, _ = self.root(parent)
        summary = self.sweep(parent, device)
        self.assertEqual(summary["examined"], 1)
        self.assertEqual(summary["removed_total"], 1)
        self.assertEqual(summary["skipped_total"], 0)
        self.assertEqual(self.rows(summary), {os.path.basename(path): "removed"})
        self.assertIsNone(summary["stop"])
        self.assertGreater(summary["steps"], 0)
        self.assertGreater(summary["elapsed_ns"], 0)
        self.assertFalse(os.path.lexists(path), "the eligible root was not removed whole")
        self.assertEqual(os.listdir(parent), [])

    def test_the_seven_day_edge_and_young_conclusions_are_kept(self):
        for label, concluded in (("exactly seven days", NOW - bounded.EXPIRY_S),
                                 ("young", NOW - 1)):
            with self.subTest(label):
                parent, device = self.parent()
                path, _ = self.root(parent, concluded_unix=concluded)
                summary = self.sweep(parent, device)
                self.assertEqual(summary["removed_total"], 0)
                self.assertEqual(self.rows(summary), {os.path.basename(path): "young"})
                self.assertTrue(os.path.isdir(path))

    def test_kept_reason_decides_eligibility_not_age(self):
        for reason in bounded.MARKER_EXPIRABLE_REASONS:
            with self.subTest(reason=reason):
                parent, device = self.parent()
                path, _ = self.root(parent, kept_reason=reason)
                self.assertEqual(self.sweep(parent, device)["removed_total"], 1)
                self.assertFalse(os.path.lexists(path))
        for reason in bounded.MARKER_REASONS:
            if reason in bounded.MARKER_EXPIRABLE_REASONS:
                continue
            with self.subTest(reason=reason):
                parent, device = self.parent()
                path, _ = self.root(parent, kept_reason=reason)
                summary = self.sweep(parent, device)
                self.assertEqual(summary["removed_total"], 0)
                self.assertEqual(self.rows(summary), {os.path.basename(path): "kept-" + reason})
                self.assertTrue(os.path.isdir(path))

    def test_old_but_unconcluded_and_nested_unknown_roots_are_kept(self):
        parent, device = self.parent()
        roots = {
            "oxide-proof.running": dict(concluded_unix=None, outcome="running", closure="none",
                                        kept_reason="none"),
            "oxide-proof.unclosed": dict(concluded_unix=None, outcome="unclosed", closure="none",
                                         kept_reason="unclosed"),
            "oxide-proof.nested": dict(concluded_unix=None, outcome="unclosed", closure="none",
                                       kept_reason="nested-unclosed"),
            "oxide-proof.failed": dict(concluded_unix=None, outcome="failed", closure="none",
                                       kept_reason="qualify-failed"),
        }
        paths = {name: self.root(parent, name=name, **state)[0] for name, state in roots.items()}
        summary = self.sweep(parent, device)
        self.assertEqual(summary["removed_total"], 0)
        self.assertEqual(self.rows(summary), {name: "unconcluded" for name in roots})
        for path in paths.values():
            self.assertTrue(os.path.isdir(path))
            self.assertTrue(os.path.lexists(os.path.join(path, bounded.MARKER_NAME)))

    def test_unmarked_foreign_and_linked_entries_are_never_touched(self):
        parent, device = self.parent()
        outside = os.path.join(str(Path(parent).parent), "outside")  # never created
        unmarked = os.path.join(parent, "oxide-proof.unmarked")
        os.mkdir(unmarked, 0o700)
        Path(unmarked, "keep.bin").write_bytes(b"x")
        os.utime(unmarked, (1, 1))  # old, but age alone proves nothing
        Path(parent, "oxide-proof.file").write_bytes(b"")
        target = os.path.join(parent, "target")
        os.mkdir(target, 0o700)
        Path(target, "keep.bin").write_bytes(b"y")
        os.symlink(target, os.path.join(parent, "oxide-proof.link"))
        foreign, _ = self.root(parent, name="oxide-proof.foreign")
        other = os.path.join(parent, "random-name")
        os.mkdir(other, 0o700)
        probe = FixtureProbe(owners={foreign: 0})
        summary = self.sweep(parent, device, probe=probe)
        self.assertEqual(summary["removed_total"], 0)
        self.assertEqual(self.rows(summary), {"oxide-proof.unmarked": "marker-absent",
                                              "oxide-proof.file": "not-a-directory",
                                              "target": "not-a-run-root",
                                              "oxide-proof.link": "symlink",
                                              "oxide-proof.foreign": "foreign-owner",
                                              "random-name": "not-a-run-root"})
        self.assertEqual(Path(unmarked, "keep.bin").read_bytes(), b"x")
        self.assertEqual(Path(target, "keep.bin").read_bytes(), b"y")
        self.assertTrue(os.path.isdir(foreign))
        self.assertTrue(os.path.isdir(other))
        self.assertFalse(os.path.lexists(outside))

    def test_a_corrupt_future_or_mismatched_marker_keeps_its_root(self):
        cases = {
            "marker-schema": dict(extra_field=1),
            "marker-not-json": None,
            "marker-oversized": None,
            "marker-foreign": dict(marker_owner=0),
            "marker-not-private": dict(mode=0o644),
            "marker-not-regular": dict(link=True),
            "marker-identity": dict(device=12345),
            "future-clock": dict(concluded_unix=NOW + 60),
        }
        for expected, spec in cases.items():
            with self.subTest(expected):
                parent, device = self.parent()
                path = os.path.join(parent, "oxide-proof.aaa")
                os.mkdir(path, 0o700)
                document = bounded.marker_document(bounded.new_run_id(), path, FixtureProbe(),
                                                   created_unix=10.0)
                document.update(OLD)
                marker = os.path.join(path, bounded.MARKER_NAME)
                probe = FixtureProbe()
                if spec is None and expected == "marker-not-json":
                    Path(marker).write_bytes(b"{")
                    os.chmod(marker, 0o600)
                elif spec is None:
                    Path(marker).write_bytes(b"x" * (bounded.MARKER_MAX_BYTES + 1))
                    os.chmod(marker, 0o600)
                elif spec.get("link"):
                    os.symlink(os.path.join(path, "elsewhere"), marker)
                else:
                    if "extra_field" in spec:
                        document["extra"] = 1
                    if "marker_owner" in spec:
                        probe = FixtureProbe(owners={marker: spec["marker_owner"]})
                    if "mode" in spec:
                        Path(marker).write_text(json.dumps(document))
                        os.chmod(marker, spec["mode"])
                    if "device" in spec:
                        document["device"] = spec["device"]
                    if "concluded_unix" in spec:
                        document["concluded_unix"] = spec["concluded_unix"]
                    if not os.path.lexists(marker):
                        Path(marker).write_text(json.dumps(document))
                        os.chmod(marker, 0o600)
                summary = self.sweep(parent, device, probe=probe)
                self.assertEqual(summary["removed_total"], 0)
                self.assertEqual(self.rows(summary), {"oxide-proof.aaa": expected})
                self.assertTrue(os.path.isdir(path), "a refused marker's root was removed")
                self.assertTrue(os.path.lexists(marker), "a refused marker was removed")

    def test_a_link_or_foreign_entry_inside_a_concluded_root_stops_it(self):
        parent, device = self.parent()
        path, _ = self.root(parent, name="oxide-proof.link")
        target = os.path.join(parent, "target")
        os.mkdir(target, 0o700)
        Path(target, "keep.bin").write_bytes(b"y")
        os.symlink(target, os.path.join(path, "escape"))
        summary = self.sweep(parent, device)
        self.assertEqual(summary["removed_total"], 0)
        self.assertEqual(self.rows(summary), {"oxide-proof.link": "unsafe-symlink",
                                              "target": "not-a-run-root"})
        self.assertTrue(os.path.isdir(path), "the root was removed despite its link")
        self.assertTrue(os.path.lexists(os.path.join(path, bounded.MARKER_NAME)))
        self.assertEqual(Path(target, "keep.bin").read_bytes(), b"y", "a link was followed")

        parent, device = self.parent()
        path, _ = self.root(parent, name="oxide-proof.foreign-entry")
        entry = os.path.join(path, "foreign.bin")
        Path(entry).write_bytes(b"z")
        original = bounded.entry_stat

        def foreign(name, dir_fd):
            info = original(name, dir_fd)
            return types.SimpleNamespace(st_dev=info.st_dev, st_ino=info.st_ino, st_mode=info.st_mode,
                                         st_uid=0 if name == "foreign.bin" else info.st_uid,
                                         st_size=info.st_size)

        with patched(bounded, "entry_stat", foreign):
            summary = self.sweep(parent, device)
        self.assertEqual(self.rows(summary), {"oxide-proof.foreign-entry": "unsafe-foreign-owner"})
        self.assertTrue(os.path.isdir(path))
        self.assertEqual(Path(entry).read_bytes(), b"z")

    def test_budgets_stop_the_work_and_a_later_run_finishes_it(self):
        parent, device = self.parent()
        path, _ = self.root(parent, payload=6)
        marker = os.path.join(path, bounded.MARKER_NAME)
        summary = self.sweep(parent, device, entries=4)
        self.assertEqual(summary["stop"], "entries")
        self.assertEqual(summary["removed_total"], 0)
        self.assertEqual(self.rows(summary), {"oxide-proof.aaa": "partial-entries"})
        self.assertTrue(os.path.isdir(path), "an exhausted budget removed a root")
        self.assertTrue(os.path.lexists(marker), "a partial deletion removed the marker")
        summary = self.sweep(parent, device)
        self.assertEqual(summary["removed_total"], 1)
        self.assertFalse(os.path.lexists(path))

        parent, device = self.parent()
        path, _ = self.root(parent, payload=2)
        ticks = []

        def clock():
            ticks.append(1)
            return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) + len(ticks) * 2 * NS

        summary = self.sweep(parent, device, clock=clock)
        self.assertEqual(summary["stop"], "time")
        self.assertEqual(summary["removed_total"], 0)
        self.assertTrue(os.path.isdir(path))

    def test_no_cross_root_deletion_and_a_changing_marker_is_refused(self):
        parent, device = self.parent()
        first, _ = self.root(parent, name="oxide-proof.first")
        second, _ = self.root(parent, name="oxide-proof.second")
        kept = self.root(parent, name="oxide-proof.kept", payload=1,
                         concluded_unix=None, outcome="running", closure="none", kept_reason="none")[0]
        summary = self.sweep(parent, device)
        self.assertEqual(summary["removed_total"], 2)
        self.assertFalse(os.path.lexists(first))
        self.assertFalse(os.path.lexists(second))
        self.assertTrue(os.path.isdir(kept))

        parent, device = self.parent()
        path, _ = self.root(parent, payload=2)
        original = bounded.read_marker
        seen = []

        def changing(root, probe):
            document = original(root, probe)
            seen.append(root)
            if len([item for item in seen if item == root]) > 1:
                return dict(document, run_id=bounded.new_run_id())
            return document

        with patched(bounded, "read_marker", changing):
            summary = self.sweep(parent, device)
        self.assertEqual(self.rows(summary), {"oxide-proof.aaa": "changed-during-sweep"})
        self.assertTrue(os.path.isdir(path))
        self.assertTrue(os.path.lexists(os.path.join(path, bounded.MARKER_NAME)))
        self.assertTrue(os.path.isdir(os.path.join(path, "evidence-0")), "a refused root was emptied")

        original_facts = bounded.entry_facts
        calls = []

        def changing_root(entry, device_, probe, budget, dir_fd=None):
            calls.append(entry)
            if entry == path and calls.count(entry) > 1:
                return "not-canonical"
            return original_facts(entry, device_, probe, budget, dir_fd)

        with patched(bounded, "entry_facts", changing_root):
            summary = self.sweep(parent, device)
        self.assertEqual(self.rows(summary), {"oxide-proof.aaa": "changed-during-sweep"})
        self.assertTrue(os.path.isdir(os.path.join(path, "evidence-0")))

    def test_the_pre_correction_marker_shape_can_never_expire(self):
        # Explicit counterfactual: the old lifecycle's closed/positive/evidence
        # marker (no nested_scope) over retained UNKNOWN resources must not be
        # removable, at any age.
        parent, device = self.parent()
        path = os.path.join(parent, "oxide-proof.legacy")
        os.mkdir(path, 0o700)
        document = bounded.marker_document(bounded.new_run_id(), path, FixtureProbe(), created_unix=10.0)
        document.update(OLD)
        document.pop("nested_scope")
        marker = os.path.join(path, bounded.MARKER_NAME)
        Path(marker).write_text(json.dumps(document))
        os.chmod(marker, 0o600)
        Path(path, "retained-unknown").write_text("UNKNOWN")
        summary = self.sweep(parent, device)
        self.assertEqual(summary["removed_total"], 0)
        self.assertEqual(self.rows(summary), {"oxide-proof.legacy": "marker-schema"})
        self.assertTrue(os.path.isdir(path))
        self.assertEqual(Path(path, "retained-unknown").read_text(), "UNKNOWN")

    def test_a_hosting_root_with_retained_unknown_is_never_eligible(self):
        parent, device = self.parent()
        path = os.path.join(parent, "oxide-proof.hosting")
        os.mkdir(path, 0o700)
        probe = FixtureProbe()
        run_id = bounded.new_run_id()
        bounded.write_marker(path, bounded.marker_document(run_id, path, probe, created_unix=10.0), probe)
        bounded.protect_nested_scope(path, probe)
        bounded.record_conclusion(path, run_id, probe, "retained", "none", "nested-unclosed")
        summary = self.sweep(parent, device)
        self.assertEqual(summary["removed_total"], 0)
        self.assertEqual(self.rows(summary), {"oxide-proof.hosting": "unconcluded"})
        self.assertTrue(os.path.isdir(path))

    def test_peer_io_faults_are_bounded_skips_not_this_runs_fallback(self):
        # A peer's filesystem fault is reported for that peer and never diverts
        # this run: the selected parent still hosts this run's own new root.
        def world():
            parent, device = self.parent()
            faulty, _ = self.root(parent, name="oxide-proof.aaa")
            healthy, _ = self.root(parent, name="oxide-proof.bbb")
            inodes = {os.lstat(faulty).st_ino, os.lstat(os.path.join(faulty, "evidence-0")).st_ino}
            return parent, device, faulty, healthy, inodes

        def under_faulty(name, faulty, inodes, dir_fd=None):
            if isinstance(name, str) and os.path.abspath(name).startswith(os.path.abspath(faulty) + os.sep):
                return True
            if isinstance(name, int):
                dir_fd = name
            if dir_fd is not None:
                try:
                    return os.fstat(dir_fd).st_ino in inodes
                except OSError:
                    return False
            return False

        def run(parent, device, label):
            original = getattr(os, label)

            def wrapped(*args, **kwargs):
                name, dir_fd = (args[0] if args else None), kwargs.get("dir_fd")
                if under_faulty(name, faulty, inodes, dir_fd):
                    raise PermissionError(errno.EACCES, "denied", name)
                return original(*args, **kwargs)

            with patched(os, label, wrapped):
                return self.sweep(parent, device)

        for label in ("unlink", "rmdir", "open", "listdir"):
            with self.subTest(label=label):
                parent, device, faulty, healthy, inodes = world()
                summary = run(parent, device, label)
                self.assertEqual(summary["removed_total"] + summary["skipped_total"], 2)
                self.assertTrue(os.path.isdir(faulty), "a faulty peer was treated as removed")
                self.assertTrue(os.path.lexists(os.path.join(faulty, bounded.MARKER_NAME)))
                self.assertEqual(self.rows(summary)[os.path.basename(faulty)].startswith("io-")
                                 or self.rows(summary)[os.path.basename(faulty)].startswith("unsafe-io-"),
                                 True, summary)
                self.assertFalse(os.path.lexists(healthy), "a healthy peer was not swept")
                root = bounded.create_owned_root(parent, FixtureProbe())
                self.assertTrue(root.startswith(parent + os.sep))
                shutil.rmtree(root, ignore_errors=True)
                shutil.rmtree(faulty, ignore_errors=True)

        with self.subTest(label="disappearing-child"):
            parent, device, faulty, healthy, inodes = world()
            original = bounded.entry_stat

            def vanishing(name, dir_fd):
                if isinstance(name, str) and name.startswith("evidence-") and os.fstat(dir_fd).st_ino in inodes:
                    raise FileNotFoundError(errno.ENOENT, "gone", name)
                return original(name, dir_fd)

            with patched(bounded, "entry_stat", vanishing):
                summary = self.sweep(parent, device)
            self.assertTrue(os.path.isdir(faulty))
            self.assertTrue(os.path.lexists(os.path.join(faulty, bounded.MARKER_NAME)))
            self.assertEqual(summary["removed_total"], 1)
            self.assertFalse(os.path.lexists(healthy))

    def test_a_foreign_or_linked_parent_stops_the_sweep_without_diverting_the_run(self):
        parent, device = self.parent()
        link = os.path.join(str(Path(parent).parent), os.path.basename(parent) + "-link")
        os.symlink(parent, link)
        self.addCleanup(os.unlink, link)
        summary = self.sweep(link, device)
        self.assertEqual(summary["stop"], "parent-symlink")
        self.assertEqual(summary["removed_total"], 0)
        summary = self.sweep(parent, device, probe=FixtureProbe(owners={parent: 0}))
        self.assertEqual(summary["stop"], "parent-foreign-owner")
        self.assertEqual(summary["removed_total"], 0)
        summary = self.sweep(parent, device + 1)
        self.assertEqual(summary["stop"], "parent-other-device")
        self.assertEqual(summary["removed_total"], 0)
        absent = os.path.join(parent, "absent")
        summary = self.sweep(absent, device)
        self.assertEqual(summary["stop"], "parent-absent")
        self.assertEqual(summary["removed_total"], 0)

    def test_the_sweep_reports_within_fixed_bounds(self):
        parent, device = self.parent()
        for index in range(bounded.SWEEP_ROWS + 3):
            self.root(parent, name=f"oxide-proof.r{index:02d}", payload=0,
                      concluded_unix=None, outcome="running", closure="none", kept_reason="none")
        summary = self.sweep(parent, device)
        self.assertEqual(len(summary["rows"]), bounded.SWEEP_ROWS)
        self.assertEqual(summary["omitted"], 3)
        self.assertEqual(summary["entry_budget"], bounded.SWEEP_ENTRY_BUDGET)
        self.assertEqual(summary["time_budget_ns"], bounded.SWEEP_TIME_BUDGET_NS)
        self.assertLessEqual(bounded.SWEEP_TIME_BUDGET_NS, bounded.SETUP_S * NS)



class PureExistingParent(unittest.TestCase):
    """The pre-existing private parent after the operator's same-volume
    relocation: a safe 0700 current-user parent that already holds a nonempty,
    unmarked `legacy-retained/` directory is accepted, and that entry is left
    entirely alone while a marked, positively concluded peer is swept."""

    def test_prepare_and_sweep_accept_the_existing_parent_and_skip_legacy(self):
        mount = Path(tempfile.mkdtemp(prefix="existing.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(mount), True)
        device = os.stat(str(mount)).st_dev
        scratch = os.path.join(str(mount), "scratch")
        parent = os.path.join(scratch, "oxide")
        os.mkdir(scratch, 0o700)
        os.mkdir(parent, 0o700)
        legacy = os.path.join(parent, "legacy-retained")
        os.mkdir(legacy, 0o700)
        sentinels = {}
        for name in ("oxide-proof.aaa", "oxide-supervisor-qualify.bbb", "oxide-validation.ccc"):
            inner = os.path.join(legacy, name)
            for leaf in ("receipt.json", os.path.join("nested", "sentinel")):
                path = os.path.join(inner, "evidence", leaf)
                os.makedirs(os.path.dirname(path), exist_ok=True)
                Path(path).write_text("UNKNOWN")
                sentinels[path] = "UNKNOWN"
        peer = os.path.join(parent, "oxide-proof.peer")
        os.mkdir(peer, 0o700)
        probe = bounded.DefaultProbe()
        run_id = bounded.new_run_id()
        bounded.write_marker(peer, bounded.marker_document(run_id, peer, probe, created_unix=10.0), probe)
        bounded.record_conclusion(peer, run_id, probe, "closed", "positive", "evidence", wall=1000.0)
        Path(peer, "payload.bin").write_bytes(b"x")
        before = (os.lstat(legacy).st_ino, os.lstat(scratch).st_ino, os.lstat(parent).st_ino)
        selection = {"source": "selected-volume", "mount": str(mount), "root_parent": parent,
                     "device": device, "reason": None}
        listing, opens = [], []
        original_listdir, original_open = os.listdir, os.open

        def listdir(*args, **kwargs):
            listing.append(args[0] if args else None)
            return original_listdir(*args, **kwargs)

        def opening(*args, **kwargs):
            opens.append(args[0] if args else None)
            return original_open(*args, **kwargs)

        with patched(os, "listdir", listdir), patched(os, "open", opening):
            prepared = bounded.prepare_scratch_parent(selection, probe)
            summary = bounded.sweep_concluded_roots(parent, device, probe, now=NOW)
        # The existing safe parent is accepted unchanged, not re-created.
        self.assertEqual(prepared, parent)
        self.assertEqual((os.lstat(legacy).st_ino, os.lstat(scratch).st_ino, os.lstat(parent).st_ino),
                         before)
        # The marked, concluded peer was processed within the budgets.
        self.assertEqual(summary["stop"], None)
        self.assertEqual((summary["examined"], summary["removed_total"], summary["skipped_total"]),
                         (1, 1, 1))
        self.assertFalse(os.path.lexists(peer))
        # The unmarked legacy entry is untouched and never enumerated.
        self.assertEqual(os.lstat(legacy).st_ino, before[0])
        self.assertTrue(os.path.isdir(legacy))
        for path, content in sentinels.items():
            self.assertEqual(Path(path).read_text(), content, path)
        self.assertFalse(os.path.lexists(os.path.join(legacy, bounded.MARKER_NAME)))
        for root, _directories, files in os.walk(legacy):
            self.assertNotIn(bounded.MARKER_NAME, files, "a legacy root gained a marker")
        for entry in listing + opens:
            if isinstance(entry, str):
                self.assertNotIn("legacy-retained", entry,
                                 "the sweep enumerated into the unmarked legacy entry")
        # A NEW separately marked run root still lands safely in that parent.
        root = bounded.create_owned_root(parent, probe)
        self.assertTrue(root.startswith(parent + os.sep))
        bounded.write_marker(root, bounded.marker_document(bounded.new_run_id(), root, probe), probe)
        self.assertEqual(bounded.read_marker(root, probe)["nested_scope"], "exclusive")
        shutil.rmtree(root, ignore_errors=True)


class PureProtectionAndLifecycle(unittest.TestCase):
    """The nested-window protection that stops an intentional nested UNKNOWN
    from outliving an expirable marker, and the bootstrap's separation of the
    suite result from the custody of the owned resources."""

    def world(self):
        sandbox = Path(tempfile.mkdtemp(prefix="protect.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(sandbox), True)
        guard = qualify.candidate()
        probe = guard.DefaultProbe()

        def make(name, payload=True):
            root = os.path.join(str(sandbox), name)
            os.mkdir(root, 0o700)
            os.makedirs(os.path.join(root, "evidence"))
            run_id = guard.new_run_id()
            if payload:
                guard.write_marker(root, guard.marker_document(run_id, root, probe, created_unix=10.0),
                                   probe)
            return root, run_id

        return sandbox, guard, probe, make

    def test_the_suite_protects_the_outer_root_before_any_case(self):
        module = sys.modules[__name__]
        sandbox, guard, _probe, make = self.world()
        root, run_id = make("oxide-proof.aaa")
        evidence = os.path.join(root, "evidence")
        with patched(module, "OUTER_EVIDENCE", evidence):
            record = module.protect_outer_root()
        self.assertEqual(record, {"state": "protected", "root": root, "run_id": run_id})
        self.assertEqual(guard.read_marker(root, guard.DefaultProbe())["nested_scope"], "hosting")
        # An unmarked root is reported and never adopted.
        plain, _ = make("oxide-proof.plain", payload=False)
        with patched(module, "OUTER_EVIDENCE", os.path.join(plain, "evidence")):
            self.assertEqual(module.protect_outer_root()["state"], "unmarked")
        self.assertEqual(os.listdir(plain), ["evidence"])
        # A marker that cannot be protected refuses instead of running cases.
        corrupt, _ = make("oxide-proof.corrupt")
        Path(corrupt, bounded.MARKER_NAME).write_text("{}")
        os.chmod(os.path.join(corrupt, bounded.MARKER_NAME), 0o600)
        with patched(module, "OUTER_EVIDENCE", os.path.join(corrupt, "evidence")):
            self.assertEqual(module.protect_outer_root()["state"], "failed")
        with patched(module, "OUTER_EVIDENCE", None):
            self.assertEqual(module.protect_outer_root()["state"], "no-outer-root")
        # The protection call is before every case, and it is recorded.
        source = Path(module.__file__).read_text()
        protect_at = source.index("PROTECTION.update(protect_outer_root())")
        self.assertLess(protect_at, source.index("runner.run(pure)"))
        self.assertLess(protect_at, source.index("loadTestsFromTestCase(Real)"))
        self.assertIn('"protection": dict(PROTECTION)', source)

    def test_a_passing_qualification_with_retained_unknown_stays_pass_and_unconcluded(self):
        sandbox, guard, probe, make = self.world()
        root, run_id = make("oxide-proof.candidate")
        guard.protect_nested_scope(root, probe)
        protection = {"state": "protected", "root": root, "run_id": run_id}
        problems, concluded = qualify.finish_candidate(guard, probe, {"root": root}, ["case/dir"],
                                                       protection)
        self.assertEqual(problems, [])
        self.assertFalse(concluded)
        document = guard.read_marker(root, probe)
        self.assertEqual((document["outcome"], document["closure"], document["kept_reason"],
                          document["concluded_unix"]), ("retained", "none", "nested-unclosed", None))
        summary = guard.sweep_concluded_roots(str(sandbox), os.stat(sandbox).st_dev, probe, now=NOW)
        self.assertEqual(summary["removed_total"], 0)
        base, base_run = make("oxide-supervisor-qualify.base")
        self.assertEqual(qualify.finish_base(guard, probe, base, base_run, True, True, False, protection), [])
        base_document = guard.read_marker(base, probe)
        self.assertEqual((base_document["outcome"], base_document["kept_reason"],
                          base_document["concluded_unix"]),
                         ("retained", "nested-unclosed", None))
        self.assertEqual(guard.sweep_concluded_roots(str(sandbox), os.stat(sandbox).st_dev, probe,
                                                     now=NOW)["removed_total"], 0)

    def test_a_fully_concluded_qualification_records_positive_markers(self):
        sandbox, guard, probe, make = self.world()
        root, run_id = make("oxide-proof.candidate")
        guard.protect_nested_scope(root, probe)
        protection = {"state": "protected", "root": root, "run_id": run_id}
        problems, concluded = qualify.finish_candidate(guard, probe, {"root": root}, [], protection)
        self.assertEqual(problems, [])
        self.assertTrue(concluded)
        document = guard.read_marker(root, probe)
        self.assertEqual((document["outcome"], document["closure"], document["kept_reason"]),
                         ("closed", "positive", "evidence"))
        base, base_run = make("oxide-supervisor-qualify.base")
        self.assertEqual(qualify.finish_base(guard, probe, base, base_run, True, False, True, protection), [])
        base_document = guard.read_marker(base, probe)
        self.assertEqual((base_document["outcome"], base_document["closure"],
                          base_document["kept_reason"]), ("closed", "positive", "qualify-base"))
        # Exactly this now-concluded shape is what the sweep may expire later.
        later = time.time() + guard.EXPIRY_S + 60
        summary = guard.sweep_concluded_roots(str(sandbox), os.stat(sandbox).st_dev, probe, now=later)
        self.assertEqual(summary["removed_total"], 2)

    def test_a_failed_qualification_keeps_the_base_unconcluded(self):
        sandbox, guard, probe, make = self.world()
        base, base_run = make("oxide-supervisor-qualify.base")
        self.assertEqual(qualify.finish_base(guard, probe, base, base_run, False, False, False, None), [])
        document = guard.read_marker(base, probe)
        self.assertEqual((document["outcome"], document["kept_reason"], document["concluded_unix"]),
                         ("failed", "qualify-failed", None))
        self.assertEqual(guard.sweep_concluded_roots(str(sandbox), os.stat(sandbox).st_dev, probe,
                                                     now=NOW)["removed_total"], 0)

    def test_the_candidate_is_never_concluded_from_outer_closure(self):
        _sandbox, guard, probe, make = self.world()
        root, run_id = make("oxide-proof.candidate")
        guard.protect_nested_scope(root, probe)
        problems, concluded = qualify.finish_candidate(
            guard, probe, {"root": root}, [], {"state": "protected", "root": root,
                                               "run_id": "b" * 32})
        self.assertTrue(problems)
        self.assertFalse(concluded)
        self.assertEqual(guard.read_marker(root, probe)["concluded_unix"], None)
        # A hosting root concluded from its outer closure is refused, not trusted.
        fabricated, fabricated_run = make("oxide-proof.fabricated")
        guard.protect_nested_scope(fabricated, probe)
        Path(os.path.join(fabricated, bounded.MARKER_NAME)).write_text(json.dumps(
            dict(guard.read_marker(fabricated, probe), outcome="closed", closure="positive",
                 kept_reason="evidence", concluded_unix=1000.0)))
        os.chmod(os.path.join(fabricated, bounded.MARKER_NAME), 0o600)
        problems, concluded = qualify.finish_candidate(
            guard, probe, {"root": fabricated}, [], {"state": "protected", "root": fabricated,
                                                     "run_id": fabricated_run})
        self.assertTrue(any("outer closure" in problem for problem in problems), problems)
        self.assertFalse(concluded)
        # An unmarked root is left alone and never eligible.
        plain, _ = make("oxide-proof.plain", payload=False)
        self.assertEqual(qualify.finish_candidate(guard, probe, {"root": plain}, [], None), ([], False))
        self.assertEqual(os.listdir(plain), ["evidence"])

    def test_marker_transition_failures_are_reported_not_swallowed(self):
        _sandbox, guard, probe, make = self.world()
        root, run_id = make("oxide-proof.candidate")
        guard.protect_nested_scope(root, probe)
        protection = {"state": "protected", "root": root, "run_id": run_id}
        base, base_run = make("oxide-supervisor-qualify.base")

        def failing(*_args, **_kwargs):
            raise guard.MarkerInvalid("changed")

        with patched(guard, "record_conclusion", failing):
            problems, concluded = qualify.finish_candidate(guard, probe, {"root": root}, [], protection)
            self.assertTrue(any("positive conclusion" in problem for problem in problems), problems)
            self.assertFalse(concluded)
            problems, _retained = qualify.finish_candidate(guard, probe, {"root": root}, ["dir"], protection)
            self.assertTrue(any("retention" in problem for problem in problems), problems)
            base_problems = qualify.finish_base(guard, probe, base, base_run, True, False, True, protection)
            self.assertTrue(any("base could not record" in problem for problem in base_problems),
                            base_problems)

        def unreadable(*_args, **_kwargs):
            raise OSError(errno.EIO, "read error")

        with patched(guard, "record_conclusion", unreadable):
            problems, _concluded = qualify.finish_candidate(guard, probe, {"root": root}, [], protection)
            self.assertTrue(problems)


# Qualification layout (oxide#145) ------------------------------------------------

qspec = importlib.util.spec_from_file_location("qualify_supervisor", BOOTSTRAP)
qualify = importlib.util.module_from_spec(qspec)
qspec.loader.exec_module(qualify)

VOLUME = "/Volumes/Builds"
DRIVE_DEVICE = 2000


class TopologyFacts:
    """Injected device, canonical-path, free-space and document facts for the
    qualification's pass checks; real documents may also be read."""

    def __init__(self, nodes, documents=None, boot=1, realpaths=None, free=None):
        self.nodes, self.documents, self.boot = dict(nodes), dict(documents or {}), boot
        self.realpaths, self.free_bytes = dict(realpaths or {}), dict(free or {})

    def device(self, path):
        try:
            return self.nodes[path]
        except KeyError:
            raise FileNotFoundError(errno.ENOENT, "No such file or directory", path)

    def realpath(self, path):
        return self.realpaths.get(path, path)

    def boot_device(self):
        return self.boot

    def free(self, path):
        try:
            return self.free_bytes[path]
        except KeyError:
            raise FileNotFoundError(errno.ENOENT, "No such file or directory", path)

    def document(self, path):
        if path in self.documents:
            return self.documents[path]
        with open(path) as handle:
            return json.load(handle)


class QualifyEntry(unittest.TestCase):
    """The bootstrap's `qualify()` entry seam (oxide#151 diagnostic2).

    The real function body runs against in-memory mocks: platform, git,
    selection, the module candidate loader and the preparation boundary are all
    faked, and every host, filesystem, directory, sweep, marker and child seam
    raises if it is reached. The regression proves the module loader is resolved
    before anything else and that execution reaches the preparation boundary, so
    a local binding that shadows the loader (the diagnostic2 UnboundLocalError)
    fails here without any effect, and a negative counterfactual re-inserting
    that binding is shown to fail the same entry."""

    class Reached(Exception):
        pass

    def run_entry(self, source=None):
        calls, forbidden = [], []

        def forbid(name):
            def fail(*_args, **_kwargs):
                forbidden.append(name)
                raise AssertionError(f"{name} must not be reached before the preparation boundary")
            return fail

        class Guard:
            SelectionRefused = type("SelectionRefused", (Exception,), {})

            @staticmethod
            def DefaultProbe():
                # Constructing the guard's probe is in-memory only; every fact it
                # could read is forbidden, so no host effect can occur here.
                calls.append(("probe", None))

                class Probe:
                    lstat = staticmethod(forbid("probe.lstat"))
                    stat = staticmethod(forbid("probe.stat"))
                    realpath = staticmethod(forbid("probe.realpath"))
                    volume_uuid = staticmethod(forbid("probe.volume_uuid"))
                    euid = staticmethod(forbid("probe.euid"))
                    home = staticmethod(forbid("probe.home"))
                    read = staticmethod(forbid("probe.read"))

                return Probe()

            @staticmethod
            def prepare_scratch_parent(expected, _probe):
                calls.append(("prepare", expected.get("root_parent")))
                raise QualifyEntry.Reached("reached the preparation boundary")

            @staticmethod
            def sweep_concluded_roots(*_args, **_kwargs):
                forbidden.append("sweep")
                raise AssertionError("a sweep was attempted")

            @staticmethod
            def write_marker(*_args, **_kwargs):
                forbidden.append("marker")
                raise AssertionError("a marker write was attempted")

        def loader():
            calls.append(("candidate-loader", None))
            return Guard

        selection = {"source": "selected-volume", "root_parent": "/Volumes/Builds/scratch/oxide",
                     "mount": "/Volumes/Builds", "device": 1, "reason": None, "pool": "/x",
                     "pool_source": "account config", "registration": "/x/oxide-managed/removable.json",
                     "volume_uuid": "a" * 8}
        fakes = {
            "signal": types.SimpleNamespace(SIGINT=2, SIGTERM=15,
                                           signal=lambda *_a, **_k: calls.append(("signal", None))),
            "git": lambda *args: "deadbeef\n" if tuple(args[:2]) == ("rev-parse", "HEAD") else "",
            "selection": lambda: dict(selection),
            "candidate": loader,
            "subprocess": types.SimpleNamespace(Popen=forbid("subprocess.Popen")),
            "tempfile": types.SimpleNamespace(mkdtemp=forbid("tempfile.mkdtemp")),
            "reserve_readings": forbid("reserve_readings"),
            "hold": forbid("hold"),
        }
        saved = {name: getattr(qualify, name) for name in fakes}
        raised, result = None, None
        stdout = io.StringIO()
        try:
            for name, value in fakes.items():
                setattr(qualify, name, value)
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(io.StringIO()):
                if source is None:
                    try:
                        qualify.qualify()
                        result = "returned"
                    except BaseException as error:  # the entry's own outcome, recorded not raised
                        raised = error
                else:
                    namespace = dict(qualify.__dict__)
                    exec(compile(source, "<qualify-entry-counterfactual>", "exec"), namespace)
                    try:
                        namespace["qualify"]()
                        result = "returned"
                    except BaseException as error:
                        raised = error
        finally:
            for name, value in saved.items():
                setattr(qualify, name, value)
        return {"calls": [name for name, _ in calls], "forbidden": forbidden, "raised": raised,
                "result": result, "stdout": stdout.getvalue()}

    def test_the_entry_resolves_the_candidate_loader_before_any_effect(self):
        observed = self.run_entry()
        self.assertIsInstance(observed["raised"], QualifyEntry.Reached,
                              (observed["raised"], observed["forbidden"], observed["stdout"]))
        self.assertEqual(observed["calls"], ["signal", "signal", "candidate-loader", "probe", "prepare"])
        self.assertEqual(observed["forbidden"], [],
                         "a host, filesystem, directory, sweep, marker or child effect was attempted")
        # No local binding in the entry may shadow a module helper it calls.
        body = ast.parse(inspect.getsource(qualify.qualify)).body[0]
        bound = {node.id for node in ast.walk(body)
                 if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Store)}
        called = {node.func.id for node in ast.walk(body)
                  if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)}
        shadows = sorted(name for name in bound & called if callable(getattr(qualify, name, None)))
        self.assertEqual(shadows, [], f"a local binding shadows these module helpers: {shadows}")

    def test_the_counterfactual_local_binding_fails_this_entry(self):
        # Restore the diagnostic2 shadow in memory only: the real body with its
        # Popen local named `candidate` again. Nothing tracked is mutated.
        source = textwrap.dedent(inspect.getsource(qualify.qualify))
        rebroken = source.replace("candidate_process", "candidate")
        self.assertNotEqual(rebroken, source, "the counterfactual did not reintroduce the local binding")
        self.assertIn("candidate = subprocess.Popen(", rebroken)
        observed = self.run_entry(source=rebroken)
        self.assertIsInstance(observed["raised"], UnboundLocalError, observed["raised"])
        self.assertEqual(observed["calls"], ["signal", "signal"],
                         "the shadowed loader still ran, so the counterfactual proved nothing")
        self.assertEqual(observed["forbidden"], [], "the broken entry attempted an effect")


def typed_int(value):
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def detach_case_rows(case):
    """The three real detach case rows a qualification receipt must carry, in
    the shape the suite publishes them (oxide#191): each inside the monitored
    root and carrying its own recorded detach outcome."""
    return [
        {"case": "test_success_detaches_exactly_the_requested_volume", "dir": case, "exit": 0,
         "detach": {"state": "detached", "qualified": True, "post_state": "detached"}},
        {"case": "test_an_unbound_identity_is_refused_before_any_detach", "dir": case, "exit": 125,
         "detach": {"state": "refused", "qualified": False, "post_state": "unknown"}},
        {"case": "test_a_failed_attempt_keeps_the_mount_and_is_unqualified", "dir": case, "exit": 125,
         "detach": {"state": "failed", "qualified": False, "post_state": "still_mounted"}},
        {"case": "test_an_unrequested_attachment_keeps_the_owned_root", "dir": case, "exit": 125,
         "cleanup": {"state": "kept", "reason": "a mounted image the proof never requested"}},
        {"case": "test_a_stuck_detach_child_is_closed_by_the_custodian", "dir": case, "exit": 125,
         "detach": {"state": "timed_out", "qualified": False, "reaped": False, "custodian_closed": True}},
    ]


def survivor_case_rows(case):
    """The real cooperative-session survivor case row a qualification receipt
    must carry, in the shape the suite publishes it (oxide#195): a Cargo-shaped
    same-session group that reached the deadline, is recorded in the receipt's
    bounded `survivors` list and is never signalled, with the run honestly
    UNCLOSED."""
    return [
        {"case": "test_cargo_shaped_group_survivor_is_recorded_and_never_signalled",
         "dir": case, "exit": 124, "census": 2,
         "outcome": {"closure": "UNCLOSED", "status": -15, "fault": CENSUS_FAULT,
                     "deadline": True},
         "survivors": [{"pid": 4100, "start": 77, "ppid": 1, "pgid": 4100, "uid": 501,
                        "zombie": False},
                       {"pid": 4101, "start": 78, "ppid": 4100, "pgid": 4100, "uid": 501,
                        "zombie": False}],
         "session_signals": []},
    ]


class KernelFactsForOracle:
    """The detach-path oracle needs no kernel facts; this makes that explicit."""


class PureQualificationTopology(unittest.TestCase):
    """The actual new-default qualification: nested case resources, evidence
    and reserve all on the selected volume inside the monitored owned root,
    with a separately labelled boot reserve."""

    def layout(self):
        """A real owned-root tree inside the suite's scratch, with the
        selected volume's device facts injected (no drive is touched)."""
        sandbox = Path(tempfile.mkdtemp(prefix="qualify.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(sandbox), True)
        parent = str(sandbox)
        root = os.path.join(parent, "oxide-proof.abc123")
        evidence, scratch = os.path.join(root, "evidence"), os.path.join(root, "scratch")
        for path in (evidence, scratch):
            os.makedirs(path)
        case = os.path.join(evidence, "nested", "test_exit_zero_closed.1")
        os.makedirs(os.path.join(case, "evidence"))
        (Path(case) / "evidence" / "receipt.json").write_text("{}")
        run_id = self.protect(root)
        registration = os.path.join(parent, "registration.json")
        with open(registration, "w") as handle:
            json.dump({"version": 2, "mount_point": VOLUME, "volume_uuid": DRIVE_UUID}, handle)
        facts = TopologyFacts({parent: DRIVE_DEVICE, root: DRIVE_DEVICE, evidence: DRIVE_DEVICE,
                               scratch: DRIVE_DEVICE, "/private/tmp": 1,
                               os.path.join(parent, "pool"): DRIVE_DEVICE},
                              documents={os.path.join(parent, "topology.json"):
                                         {"scratch": scratch, "evidence": evidence,
                                          "protection": {"state": "protected", "root": root,
                                                         "run_id": run_id}}},
                              boot=1, free={parent: 500 * 1024 ** 3, VOLUME: 500 * 1024 ** 3,
                                          "/private/tmp": 30 * 1024 ** 3})
        return sandbox, parent, root, evidence, scratch, case, registration, facts

    def protect(self, root):
        """Create the root's real owner marker and protect it as hosting through
        the production seams, exactly as the suite does before nested cases."""
        run_id = bounded.new_run_id()
        probe = bounded.DefaultProbe()
        bounded.write_marker(root, bounded.marker_document(run_id, root, probe), probe)
        bounded.protect_nested_scope(root, probe)
        self.marker_run_id = run_id
        return run_id

    def receipt(self, parent, root, evidence, scratch, registration, case):
        return {
            "phase": "monitor_complete: pending_custodian",
            "outcome": {"closure": "closed", "status": 0, "fault": None, "deadline": False},
            "root": root, "scratch": scratch, "evidence": evidence,
            "scratch_default": {"source": "selected-volume", "root_parent": parent, "pool":
                                os.path.join(parent, "pool"), "pool_source": "account config",
                                "registration": registration, "mount": VOLUME,
                                "volume_uuid": DRIVE_UUID, "reason": None},
            "reserve": {"samples": 4, "minimum": 400 * 1024 ** 3, "max_gap_ns": 10, "first_ns": 1,
                        "last_ns": 2, "path": root, "device": DRIVE_DEVICE},
            "owned_bytes": {"maximum": 8 * 1024 ** 2, "budget": 512 * 1024 ** 2},
            "custodian": [{"event": "gate_open", "at": 5}],
            "scratch_sweep": {"parent": parent, "examined": 1, "removed_total": 0,
                              "skipped_total": 1, "steps": 5, "stop": None,
                              "entry_budget": bounded.SWEEP_ENTRY_BUDGET,
                              "time_budget_ns": bounded.SWEEP_TIME_BUDGET_NS,
                              "expiry_s": bounded.EXPIRY_S, "rows": [], "omitted": 0,
                              "elapsed_ns": 10},
            "scratch_marker": {"path": os.path.join(root, bounded.MARKER_NAME),
                               "state": "nested-protected", "reason": "nested-unconcluded",
                               "run_id": getattr(self, "marker_run_id", "a" * 32)},
        }

    def control_records(self, parent, root, kept):
        return [
            {"case": "bootstrap_deadline_blocked_stdout", "exit": 124, "wall_s": 2.197},
            {"case": "direct_runner_setup_blocked", "exit": 125, "wall_s": 1.139, "kept": kept},
            {"case": "fixture_deadline", "exit": [-14, -14]},
        ]

    def kept_world(self, parent, root):
        kept = os.path.join(root, "evidence", "nested", "test_direct_runner_setup_blocked_in_filesystem.062")
        os.makedirs(kept)
        facts = TopologyFacts({parent: DRIVE_DEVICE, root: DRIVE_DEVICE, kept: DRIVE_DEVICE},
                              documents={}, boot=1, free={})
        return kept, facts

    def test_the_suites_three_controls_are_validated_by_shape(self):
        sandbox, parent, root, evidence, scratch, case, registration, facts = self.layout()
        expected = {"source": "selected-volume", "root_parent": parent}
        record, _ = qualify.topology(str(sandbox), expected, facts)
        kept, guard_facts = self.kept_world(parent, root)
        ordinary = {"case": "zero", "dir": case, "exit": 0,
                    "phase": "monitor_complete: pending_custodian", "retained_unclosed": False,
                    "outcome": {}, "census": 0, "wall_s": 1.0, "signals": []}
        results = {"cases": [ordinary] + self.control_records(parent, root, kept)}
        self.assertEqual(qualify.nested_cases(results, record, guard_facts), [])
        # A control that claims a receipt although setup was refused is wrong.
        os.makedirs(os.path.join(kept, "evidence"))
        open(os.path.join(kept, "evidence", "receipt.json"), "w").write("{}")
        problems = qualify.nested_cases(results, record, guard_facts)
        self.assertTrue(any("claims a receipt" in p for p in problems), problems)

    def test_control_record_mutations_are_refused(self):
        sandbox, parent, root, evidence, scratch, case, registration, facts = self.layout()
        expected = {"source": "selected-volume", "root_parent": parent}
        record, _ = qualify.topology(str(sandbox), expected, facts)
        kept, guard_facts = self.kept_world(parent, root)

        def check(records, marks, guard=guard_facts):
            problems = qualify.nested_cases({"cases": records}, record, guard)
            self.assertTrue(any(marks in p for p in problems), (marks, problems))

        controls = self.control_records(parent, root, kept)
        # An ordinary case with no dir, or with no receipt, is never exempt.
        check([{"case": "zero", "exit": 0}], "ran outside the monitored root")
        bare = os.path.join(evidence, "nested", "no-receipt.1")
        os.makedirs(bare)
        check([{"case": "zero", "dir": bare, "exit": 0}], "evidence missing after cleanup")
        # An unknown control, an unmodelled field and a wrong exit are refused.
        check([{"case": "unknown_control", "exit": 0}], "ran outside the monitored root")
        check([dict(controls[0], dir=case)], "unmodelled field")
        check([dict(controls[1], dir=kept)], "unmodelled field")
        check([dict(controls[2], exit=[0, 0])], "is not the recorded")
        check([dict(controls[1], wall_s=1.0) and {"case": "direct_runner_setup_blocked", "exit": 125,
                                                  "kept": kept}], "missing a field")
        # A retained path that escapes, is not canonical or is on another
        # device is refused rather than treated as kept containment.
        check([{"case": "direct_runner_setup_blocked", "exit": 125, "wall_s": 1.139,
                "kept": "/private/tmp/elsewhere"}], "outside the monitored root")
        check([{"case": "direct_runner_setup_blocked", "exit": 125, "wall_s": 1.139, "kept": kept}],
              "not canonical", TopologyFacts({parent: DRIVE_DEVICE, root: DRIVE_DEVICE, kept: DRIVE_DEVICE},
                                             documents={}, boot=1, realpaths={kept: "/private/tmp/x"}))
        check([{"case": "direct_runner_setup_blocked", "exit": 125, "wall_s": 1.139, "kept": kept}],
              "not on the monitored root's device",
              TopologyFacts({parent: DRIVE_DEVICE, root: DRIVE_DEVICE, kept: 1}, documents={}, boot=1))

    def test_qualification_requires_both_labelled_replacement_receipts(self):
        cases, outcomes = [], []
        for suffix, mode, state in (("member", "census-replacement-member", "member"),
                                    ("foreign", "census-replacement-foreign", "foreign")):
            name = f"test_synthetic_replacement_{suffix}_reaches_real_receipt"
            work = dict(fixture_work(), snapshot=1, replacement=1,
                        rows=[{"pid": 1, "start": 1, "state": "gone"},
                              {"pid": 2_147_483_647, "start": 2, "state": state}])
            cases.append({"case": name, "signal_targets": [], "replacement_fixture":
                          {"source": "synthetic", "mode": mode, "work": work}})
            outcomes.append(["__main__.Real." + name, "pass"])
        results = {"outcomes": outcomes, "cases": cases}
        plan = dict(qualify.check_plan({}, results, {}, None, None, None))
        with patched(qualify, "EXPECTED_CASES", 2):
            self.assertEqual(plan["cases"](), [])
            broken = dict(results, cases=cases[:1])
            bad = dict(qualify.check_plan({}, broken, {}, None, None, None))["cases"]()
            self.assertTrue(any("lacks one passed fixture" in issue for issue in bad))
            signalled = dict(cases[0], signal_targets=[2_147_483_647])
            bad = dict(qualify.check_plan({}, dict(results, cases=[signalled, cases[1]]), {}, None, None, None))["cases"]()
            self.assertTrue(any("signalled" in issue for issue in bad))

    def test_nested_cases_and_evidence_stay_inside_the_monitored_root(self):
        sandbox, parent, root, evidence, scratch, case, registration, facts = self.layout()
        expected = {"source": "selected-volume", "root_parent": parent}
        record, problems = qualify.topology(str(sandbox), expected, facts)
        self.assertEqual(problems, [])
        self.assertEqual(record["root"], root)
        self.assertEqual(qualify.nested_cases({"cases": [{"case": "zero", "dir": case}]}, record), [])
        receipt = self.receipt(parent, root, evidence, scratch, registration, case)
        results = {"outcomes": [["zero", "pass"]],
                   "cases": ([{"case": "zero", "dir": case}] + detach_case_rows(case)
                             + survivor_case_rows(case))}
        problems = qualify.check(receipt, results, expected, record, facts)
        self.assertEqual([p for p in problems if "cases:" not in p], [], problems)

    def test_a_case_outside_the_monitored_root_fails(self):
        sandbox, parent, root, evidence, scratch, case, registration, facts = self.layout()
        expected = {"source": "selected-volume", "root_parent": parent}
        record, _ = qualify.topology(str(sandbox), expected, facts)
        outside = "/private/tmp/nested/elsewhere.1"
        problems = qualify.nested_cases({"cases": [{"case": "zero", "dir": outside}]}, record)
        self.assertTrue(any("outside the monitored root" in p for p in problems), problems)
        self.assertTrue(any("no nested case directory" in p for p in
                            qualify.nested_cases({"cases": []}, record)))

    def test_receipt_evidence_must_be_the_suite_recorded_layout(self):
        sandbox, parent, root, evidence, scratch, case, registration, facts = self.layout()
        expected = {"source": "selected-volume", "root_parent": parent}
        record, _ = qualify.topology(str(sandbox), expected, facts)
        receipt = self.receipt(parent, root, evidence, scratch, registration, case)
        receipt["evidence"] = "/private/tmp/evidence"
        problems = qualify.default_scratch(receipt, expected, record, facts)
        self.assertTrue(any("is not the suite's recorded" in p for p in problems), problems)

    def test_topology_refusals(self):
        sandbox, parent, root, evidence, scratch, case, registration, facts = self.layout()
        expected = {"source": "selected-volume", "root_parent": parent}
        cases = {
            "not directly on the selected volume": dict(record={"scratch": os.path.join(parent, "sub",
                                                                                     "oxide-proof.x", "scratch"),
                                                               "evidence": os.path.join(parent, "sub",
                                                                                        "oxide-proof.x",
                                                                                        "evidence")},
                                                        nodes={}),
            "not members of one owned root": dict(record={"scratch": scratch, "evidence": parent},
                                                  nodes={}),
            "not canonical": dict(record={"scratch": scratch, "evidence": evidence},
                                  realpaths={scratch: "/private/tmp/elsewhere"}),
            "not on the selected volume's device": dict(record={"scratch": scratch, "evidence": evidence},
                                                       nodes={root: 1}),
        }
        for marks, override in cases.items():
            with self.subTest(marks):
                probe = TopologyFacts(dict(facts.nodes, **override.get("nodes", {})),
                                      documents={os.path.join(str(sandbox), "topology.json"):
                                                 override["record"]},
                                      boot=facts.boot, realpaths=override.get("realpaths"), free=facts.free_bytes)
                _, problems = qualify.topology(str(sandbox), expected, probe)
                self.assertTrue(any(marks in p for p in problems), problems)
        for label, document in (("missing", None), ("not an object", [1]),
                                ("no scratch and evidence", {"root": root})):
            with self.subTest(label):
                probe = TopologyFacts(facts.nodes, documents={}, boot=facts.boot, free=facts.free_bytes)
                if document is not None:
                    probe.documents[os.path.join(str(sandbox), "topology.json")] = document
                record, problems = qualify.topology(str(sandbox), expected, probe)
                self.assertIsNone(record)
                self.assertTrue(problems)

    def test_default_scratch_facts_are_rechecked(self):
        sandbox, parent, root, evidence, scratch, case, registration, facts = self.layout()
        expected = {"source": "selected-volume", "root_parent": parent}
        record, _ = qualify.topology(str(sandbox), expected, facts)
        receipt = self.receipt(parent, root, evidence, scratch, registration, case)
        self.assertEqual(qualify.default_scratch(receipt, expected, record, facts), [])
        booted = TopologyFacts(facts.nodes, documents={}, boot=DRIVE_DEVICE, free=facts.free_bytes)
        self.assertTrue(any("boot volume" in p for p in qualify.default_scratch(receipt, expected, record, booted)))
        reserve = dict(receipt, reserve=dict(receipt["reserve"], device=1))
        self.assertTrue(any("reserve is not sampled" in p for p in
                            qualify.default_scratch(reserve, expected, record, facts)))
        payload_root = os.path.join(parent, "pool", "oxide-proof.abc")
        payload_nodes = dict(facts.nodes)
        payload_nodes[payload_root] = DRIVE_DEVICE
        payload_facts = TopologyFacts(payload_nodes, documents={}, boot=1, free=facts.free_bytes)
        in_pool = dict(receipt, root=payload_root)
        record_in_pool = dict(record, root=payload_root)
        self.assertTrue(any("inside the pool payload" in p for p in
                            qualify.default_scratch(in_pool, expected, record_in_pool, payload_facts)))
        fallback = dict(receipt, scratch_default=dict(receipt["scratch_default"], source="fallback"))
        self.assertTrue(any("did not use the selected volume" in p for p in
                            qualify.default_scratch(fallback, expected, record, facts)))

    def nested_layout(self):
        """A real owned tree whose parent is the owned subtree of a mount."""
        sandbox = Path(tempfile.mkdtemp(prefix="nested.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(sandbox), True)
        mount = str(sandbox)
        parent = os.path.join(mount, "scratch", "oxide")
        os.makedirs(parent, 0o700)
        base = os.path.join(parent, "oxide-supervisor-qualify.bbbb")
        os.makedirs(base, 0o700)
        root = os.path.join(parent, "oxide-proof.aaa")
        evidence, scratch = os.path.join(root, "evidence"), os.path.join(root, "scratch")
        os.makedirs(evidence)
        os.makedirs(scratch)
        case = os.path.join(evidence, "nested", "test_zero.1")
        os.makedirs(os.path.join(case, "evidence"))
        (Path(case) / "evidence" / "receipt.json").write_text("{}")
        run_id = self.protect(root)
        registration = os.path.join(mount, "registration.json")
        with open(registration, "w") as handle:
            json.dump({"version": 2, "mount_point": mount, "volume_uuid": DRIVE_UUID}, handle)
        with open(os.path.join(base, "topology.json"), "w") as handle:
            json.dump({"scratch": scratch, "evidence": evidence,
                       "protection": {"state": "protected", "root": root, "run_id": run_id}}, handle)
        facts = TopologyFacts({mount: DRIVE_DEVICE, parent: DRIVE_DEVICE, base: DRIVE_DEVICE,
                               root: DRIVE_DEVICE, evidence: DRIVE_DEVICE, scratch: DRIVE_DEVICE,
                               case: DRIVE_DEVICE, "/private/tmp": 1},
                              documents={}, boot=1,
                              free={parent: 500 * 1024 ** 3, "/private/tmp": 30 * 1024 ** 3})
        expected = {"source": "selected-volume", "root_parent": parent, "mount": mount,
                    "registration": registration, "pool": os.path.join(mount, "pool"),
                    "volume_uuid": DRIVE_UUID, "device": DRIVE_DEVICE}
        return (mount, parent, base, root, evidence, scratch, case, registration, facts, expected)

    def detach_oracle(self, receipt, rows):
        results = {"outcomes": [["zero", "pass"]], "cases": [{"case": "zero", "dir": "/x"}] + rows}
        plan = dict(qualify.check_plan(receipt, results, {"source": "selected-volume"},
                                       None, KernelFactsForOracle(), None))
        return plan["detach-paths"]()

    def monitor_stub(self, receipt=None):
        monitor = bounded.Monitor.__new__(bounded.Monitor)
        cmd_r, cmd_w = os.pipe()
        self.addCleanup(os.close, cmd_r)
        self.addCleanup(os.close, cmd_w)
        monitor.cmd_r = cmd_r
        monitor.lock = threading.Lock()
        monitor.receipt = receipt if receipt is not None else {"custodian": []}
        monitor.published = []
        monitor.publish = lambda phase: monitor.published.append(
            (phase, [r["event"] for r in monitor.receipt["custodian"]]))
        monitor.summary = lambda: None
        monitor.send = lambda *a, **k: monitor.published.append(("sent", None))
        monitor.stop = threading.Event()
        return monitor, cmd_w

    def test_an_unclosed_decision_completes_and_persists_through_run(self):
        # The REAL `Monitor.run` with the real setup, publish and send: a keep
        # (UNCLOSED) decision must still write the final receipt and announce
        # completion. This is the path an uninitialized `opted_in` used to kill.
        sandbox = Path(tempfile.mkdtemp(prefix="unclosed-run.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(sandbox), True)
        evidence = sandbox / "evidence"
        ctx = {"run_id": "unclosed-run", "command": ["/bin/true"], "limit_s": 300,
               "adopted": str(sandbox), "evidence_arg": str(evidence), "root_parent": None,
               "budget": 512 * 1024 * 1024, "start_ns": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW),
               "inject": []}
        ctl_r, ctl_w = os.pipe()
        cmd_r, cmd_w = os.pipe()
        for fd in (ctl_r, cmd_w):
            self.addCleanup(os.close, fd)
        # The monitor's drain threads get immediate EOF (its own read fds).
        monitor = bounded.Monitor(ctx, ctl_w, cmd_r, os.open(os.devnull, os.O_RDONLY),
                                  os.open(os.devnull, os.O_RDONLY))
        messages = []

        def send(message_type, **fields):
            messages.append(message_type)

        monitor.send = send
        os.write(cmd_w, (json.dumps({"type": "keep", "outcome": {"closure": "UNCLOSED",
                                                                "status": 7}}) + "\n").encode())
        monitor.run()
        phases = []
        with open(os.path.join(str(evidence), "receipt.json")) as handle:
            persisted = json.load(handle)
        self.assertEqual(persisted["phase"], "monitor_complete: pending_custodian", persisted["phase"])
        self.assertEqual(persisted["outcome"]["closure"], "UNCLOSED")
        self.assertIn("monitor_complete", messages, messages)
        self.assertTrue(monitor.stop.is_set())
        # Oxide#197: the real setup recorded this run's own ancestor chain in the
        # receipt, as positive identities, bounded, starting at our own parent.
        ancestors = persisted["ancestors"]
        self.assertEqual(set(ancestors), {"state", "count", "identities", "stop"}, ancestors)
        self.assertIn(ancestors["state"], ("recorded", "partial"), ancestors)
        self.assertEqual(ancestors["count"], len(ancestors["identities"]), ancestors)
        self.assertTrue(0 < ancestors["count"] <= bounded.ANCESTOR_MAX_LEVELS, ancestors)
        self.assertEqual([i["level"] for i in ancestors["identities"]],
                         list(range(1, ancestors["count"] + 1)), ancestors)
        first = ancestors["identities"][0]
        self.assertEqual(first["pid"], os.getppid(), ancestors)
        parent = bounded.Kinfo.one(first["pid"])
        self.assertIsNotNone(parent, ancestors)
        self.assertEqual(parent["start"], first["start"], ancestors)
        # No wall, boot or uptime fact is recorded for exclusion (ruling
        # ea5a3dc9): the chain and the bounded structural evidence only.
        self.assertNotIn("self", ancestors)
        evidence = persisted["ancestor_exclusions"]
        self.assertEqual(set(evidence), {"rows", "count", "omitted", "by_rule", "rounds"})
        self.assertEqual(evidence["count"], evidence["omitted"] + len(evidence["rows"]), evidence)
        self.assertEqual(len(evidence["rows"]), 0, evidence)
        self.assertEqual(set(evidence["by_rule"]), {"ancestor", "parent"})

    def test_the_drain_happens_before_the_final_publication(self):
        monitor, cmd_w = self.monitor_stub()
        os.write(cmd_w, (json.dumps({"type": "record", "record": {"event": "detach_child_pinned"}}) + "\n").encode())
        ctx = {"start_ns": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW), "limit_s": 300}
        monitor.finish_monitor(ctx, "monitor_complete: pending_custodian", drain=True)
        # The published receipt already carries the record, and completion follows.
        self.assertEqual(monitor.published[0], ("monitor_complete: pending_custodian",
                                                ["detach_child_pinned"]), monitor.published)
        self.assertEqual(monitor.published[-1], ("sent", None))
        self.assertTrue(monitor.stop.is_set())

    def test_a_request_free_completion_drains_nothing_and_never_waits(self):
        monitor, cmd_w = self.monitor_stub()
        os.write(cmd_w, (json.dumps({"type": "record", "record": {"event": "detach_child_pinned"}}) + "\n").encode())
        ctx = {"start_ns": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW), "limit_s": 300}
        started = time.monotonic()
        monitor.finish_monitor(ctx, "monitor_complete: pending_custodian", drain=False)
        self.assertLess(time.monotonic() - started, 0.05, "a request-free completion waited")
        self.assertEqual(monitor.receipt["custodian"], [], "a request-free run read custodian records")
        self.assertEqual(monitor.published[0], ("monitor_complete: pending_custodian", []))

    def test_a_near_deadline_drain_reads_nothing(self):
        monitor, cmd_w = self.monitor_stub()
        os.write(cmd_w, (json.dumps({"type": "record", "record": {"event": "detach_child_pinned"}}) + "\n").encode())
        # The custodian's wait window is already spent.
        ctx = {"start_ns": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) - 300 * bounded.NS,
               "limit_s": 300}
        started = time.monotonic()
        monitor.finish_monitor(ctx, "monitor_complete: pending_custodian", drain=True)
        self.assertLess(time.monotonic() - started, 0.05, "an exhausted window still waited")
        self.assertEqual(monitor.receipt["custodian"], [])
        self.assertTrue(monitor.stop.is_set())

    def test_await_command_keeps_records_it_meets(self):
        monitor, cmd_w = self.monitor_stub()
        os.write(cmd_w, (json.dumps({"type": "record", "record": {"event": "detach_child_signal"}}) + "\n").encode())
        os.write(cmd_w, (json.dumps({"type": "detach_child_ack", "closed": True}) + "\n").encode())
        answer = monitor.await_command("detach_child_ack", 0.5)
        self.assertEqual(answer["type"], "detach_child_ack")
        self.assertEqual([r["event"] for r in monitor.receipt["custodian"]], ["detach_child_signal"])

    def test_post_decision_records_are_drained_into_the_receipt(self):
        # The custodian sends its detach pin/signal/observation records after the
        # monitor stopped reading commands; the monitor drains them at completion.
        monitor = bounded.Monitor.__new__(bounded.Monitor)
        cmd_r, cmd_w = os.pipe()
        self.addCleanup(os.close, cmd_r)
        self.addCleanup(os.close, cmd_w)
        monitor.cmd_r = cmd_r
        monitor.lock = threading.Lock()
        monitor.receipt = {"custodian": []}
        for record in ({"event": "detach_child_pinned", "pid": 7},
                       {"event": "detach_child_observed", "closed": True}):
            os.write(cmd_w, (json.dumps({"type": "record", "record": record}) + "\n").encode())
        os.write(cmd_w, (json.dumps({"type": "detach_go", "allowed": True}) + "\n").encode())
        monitor.drain_records(0.3)
        self.assertEqual([r["event"] for r in monitor.receipt["custodian"]],
                         ["detach_child_pinned", "detach_child_observed"])
        # The drain is opt-in at completion (never unconditional in send()).
        self.assertNotIn("drain_records", inspect.getsource(bounded.Monitor.send))
        self.assertIn("drain=", inspect.getsource(bounded.Monitor.finish_monitor))

    def test_the_custodian_timeout_injection_keeps_deadline_semantics(self):
        absolute = 1000
        self.assertTrue(bounded.custodian_deadline_hit(absolute, absolute, ()))
        self.assertFalse(bounded.custodian_deadline_hit(absolute - 1, absolute, ()))
        self.assertTrue(bounded.custodian_deadline_hit(
            absolute - 1, absolute, ("custodian-timeout-after-monitor",)))
        self.assertFalse(bounded.custodian_deadline_hit(absolute - 1, absolute, ("other",)))

    def test_the_receipt_path_checks_the_recorded_detach_and_cleanup_objects(self):
        receipt = {"phase": "monitor_complete: pending_custodian",
                   "outcome": {"closure": "closed", "status": 0, "fault": None, "deadline": False}}
        rows = detach_case_rows("/x")
        self.assertEqual(self.detach_oracle(receipt, rows), [], rows)

        # The request-free keep must be recorded under `cleanup`, not `detach`.
        swapped = [dict(row) for row in rows]
        for index, row in enumerate(swapped):
            if row["case"].endswith("unrequested_attachment_keeps_the_owned_root"):
                swapped[index] = {"case": row["case"], "dir": "/x", "exit": 125,
                                  "detach": row["cleanup"]}
        problems = self.detach_oracle(receipt, swapped)
        self.assertTrue(any("recorded a detach object for a request-free run" in p for p in problems),
                        problems)
        self.assertTrue(any("recorded no pre-cleanup keep" in p for p in problems), problems)

        # A cleanup record that is not a keep is a problem too.
        not_kept = [dict(row) for row in rows]
        for index, row in enumerate(not_kept):
            if row["case"].endswith("unrequested_attachment_keeps_the_owned_root"):
                not_kept[index] = dict(row, cleanup={"state": "ok", "reason": None})
        self.assertTrue(any("not kept" in p for p in self.detach_oracle(receipt, not_kept)),
                        not_kept)

        # Missing rows and mislabelled stuck-child outcomes are named.
        without_stuck = [row for row in rows if "stuck_detach_child" not in row["case"]]
        self.assertTrue(any("recorded no detach outcome" in p
                            for p in self.detach_oracle(receipt, without_stuck)))
        stuck_open = [dict(row) for row in rows]
        for index, row in enumerate(stuck_open):
            if "stuck_detach_child" in row["case"]:
                stuck_open[index] = dict(row, detach=dict(row["detach"], custodian_closed=False))
        self.assertTrue(any("custodian_closed" in p for p in self.detach_oracle(receipt, stuck_open)),
                        stuck_open)

        # An outer run that published a detach request of its own is a problem.
        self.assertTrue(any("published a detach request" in p
                            for p in self.detach_oracle(dict(receipt, detach={}), rows)))

    def test_the_owned_subtree_routing_and_marker_are_verified(self):
        mount, parent, base, root, evidence, scratch, case, registration, facts, expected = \
            self.nested_layout()
        record, problems = qualify.topology(base, expected, facts)
        self.assertEqual(problems, [], problems)
        receipt = self.receipt(parent, root, evidence, scratch, registration, case)
        receipt["scratch_default"].update(mount=mount, volume_uuid=DRIVE_UUID)
        results = {"outcomes": [["zero", "pass"]],
                   "cases": ([{"case": "zero", "dir": case}] + detach_case_rows(case)
                             + survivor_case_rows(case))}
        problems = qualify.check(receipt, results, expected, record, facts, base)
        self.assertEqual([p for p in problems if "cases:" not in p], [], problems)
        # oxide#191: the receipt must exercise the detach success, refusal and
        # failure paths, and an absent or mislabelled one is a named problem.
        stripped = dict(results, cases=[{"case": "zero", "dir": case}])
        self.assertTrue(any("recorded no detach outcome" in p for p in
                            qualify.check(receipt, stripped, expected, record, facts, base)))
        rows = detach_case_rows(case)
        wrong = dict(results, cases=[{"case": "zero", "dir": case}] + [
            dict(rows[0], detach={"state": "refused", "qualified": True, "post_state": "detached"})]
            + rows[1:])
        self.assertTrue(any("detach case test_success_detaches_exactly_the_requested_volume" in p
                            and "not detached/True" in p
                            for p in qualify.check(receipt, wrong, expected, record, facts, base)))
        # The drive root is not the contract: neither the bare mount as parent
        # nor a root created directly on it is accepted as the new default.
        drive_expected = dict(expected, root_parent=mount)
        self.assertTrue(any("is not" in p for p in
                            qualify.default_scratch(receipt, drive_expected, record, facts)))
        at_root = os.path.join(mount, "oxide-proof.direct")
        direct = dict(record, root=at_root, scratch=os.path.join(at_root, "scratch"),
                      evidence=os.path.join(at_root, "evidence"))
        direct_receipt = dict(receipt, root=at_root, scratch=direct["scratch"],
                              evidence=direct["evidence"])
        self.assertTrue(any("at the drive root" in p for p in
                            qualify.default_scratch(direct_receipt, expected, direct, facts)))

    def test_the_candidate_sweep_and_marker_conclusion_are_required(self):
        mount, parent, base, root, evidence, scratch, case, registration, facts, expected = \
            self.nested_layout()
        receipt = self.receipt(parent, root, evidence, scratch, registration, case)
        receipt["scratch_default"].update(mount=mount, volume_uuid=DRIVE_UUID)
        self.assertEqual(qualify.scratch_sweep(receipt, expected, base, facts), [])
        missing = dict(receipt)
        del missing["scratch_sweep"]
        self.assertTrue(any("no bounded startup sweep" in p for p in
                            qualify.scratch_sweep(missing, expected, base, facts)))
        wrong_parent = dict(receipt, scratch_sweep=dict(receipt["scratch_sweep"], parent=VOLUME))
        self.assertTrue(any("is not the owned parent" in p for p in
                            qualify.scratch_sweep(wrong_parent, expected, base, facts)))
        bad_stop = dict(receipt, scratch_sweep=dict(receipt["scratch_sweep"], stop="fault"))
        self.assertTrue(any("unrecognised reason" in p for p in
                            qualify.scratch_sweep(bad_stop, expected, base, facts)))
        bad_counts = dict(receipt, scratch_sweep=dict(receipt["scratch_sweep"], examined=-1))
        self.assertTrue(any("no valid examined" in p for p in
                            qualify.scratch_sweep(bad_counts, expected, base, facts)))
        deferred = dict(receipt, scratch_marker={"state": "concluded", "path": None})
        self.assertTrue(any("did not defer" in p for p in
                            qualify.protected_root(deferred, None)))
        self.assertTrue(any("no nested-window protection" in p for p in
                            qualify.protected_root(receipt, None)))
        no_root = dict(receipt, scratch_marker={"state": "nested-protected", "path": None,
                                                "reason": "nested-unconcluded"})
        self.assertTrue(any("has no root" in p for p in
                            qualify.protected_root(no_root, {"protection": {"state": "protected"}})))
        self.assertTrue(any("is gone" in p for p in
                            qualify.scratch_sweep(receipt, expected, os.path.join(parent, "absent"),
                                                  facts)))

    def test_volume_identity_matches_across_uuid_spellings(self):
        # The product writes the registered UUID uppercase (volume.rs) and the
        # kernel reports ATTR_VOL_UUID lowercase, exactly as the live host
        # registration does. The same identity must match: this failed at
        # 40b3a3aa, which compared the raw recorded text.
        sandbox, parent, root, evidence, scratch, case, registration, facts = self.layout()
        expected = {"source": "selected-volume", "root_parent": parent}
        record, _ = qualify.topology(str(sandbox), expected, facts)
        for registered, observed in ((DRIVE_UUID.upper(), DRIVE_UUID),
                                     (DRIVE_UUID, DRIVE_UUID.upper()),
                                     (DRIVE_UUID.upper(), DRIVE_UUID.upper())):
            with self.subTest(registered=registered, observed=observed):
                with open(registration, "w") as handle:
                    json.dump({"version": 2, "mount_point": VOLUME, "volume_uuid": registered}, handle)
                receipt = self.receipt(parent, root, evidence, scratch, registration, case)
                receipt["scratch_default"]["volume_uuid"] = observed
                self.assertEqual(qualify.default_scratch(receipt, expected, record, facts), [])
        self.assertEqual(qualify.canonical_uuid(DRIVE_UUID.upper()), DRIVE_UUID)
        self.assertEqual(qualify.canonical_uuid(DRIVE_UUID), DRIVE_UUID)

    def test_missing_malformed_or_different_volume_identity_is_refused(self):
        sandbox, parent, root, evidence, scratch, case, registration, facts = self.layout()
        expected = {"source": "selected-volume", "root_parent": parent}
        record, _ = qualify.topology(str(sandbox), expected, facts)
        other = "b6ac3ed7-33ea-46b4-8082-7254cf291ed2"
        for label, registered, observed, marks in (
                ("mismatched spelling-normalised", other.upper(), DRIVE_UUID, "is not the recorded"),
                ("missing registered", None, DRIVE_UUID, "missing or malformed"),
                ("non-string registered", 12345, DRIVE_UUID, "missing or malformed"),
                ("short registered", DRIVE_UUID[:-1], DRIVE_UUID, "missing or malformed"),
                ("non-hex registered", "zzzzzzzz-33ea-46b4-8082-7254cf291ed2", DRIVE_UUID,
                 "missing or malformed"),
                ("missing observed", DRIVE_UUID.upper(), None, "missing or malformed"),
                ("non-string observed", DRIVE_UUID.upper(), 42, "missing or malformed"),
                ("both missing", None, None, "missing or malformed")):
            with self.subTest(label):
                document = {"version": 2, "mount_point": VOLUME}
                if registered is not None:
                    document["volume_uuid"] = registered
                with open(registration, "w") as handle:
                    json.dump(document, handle)
                receipt = self.receipt(parent, root, evidence, scratch, registration, case)
                receipt["scratch_default"]["volume_uuid"] = observed
                problems = qualify.default_scratch(receipt, expected, record, facts)
                self.assertTrue(any(marks in problem for problem in problems), problems)

    def test_reserve_readings_identify_the_scratch_volume_and_the_boot_volume_separately(self):
        sandbox, parent, root, evidence, scratch, case, registration, facts = self.layout()
        readings = qualify.reserve_readings(parent, "/private/tmp", facts)
        self.assertEqual(readings["scratch_volume"]["path"], parent)
        self.assertEqual(readings["scratch_volume"]["free"], 500 * 1024 ** 3)
        self.assertEqual(readings["boot_volume"]["path"], "/private/tmp")
        self.assertEqual(readings["boot_volume"]["free"], 30 * 1024 ** 3)
        self.assertEqual(readings["scratch_volume"]["threshold"], qualify.THRESHOLD)
        self.assertEqual(readings["boot_volume"]["threshold"], qualify.THRESHOLD)



class PureClosureDecision(unittest.TestCase):
    """Drive the guard's actual settle()/closure() acceptance with fake events:
    the EOF veto needs both DISTINCT stream witnesses after a certain empty
    census, in either order of refusal, and first fault is preserved. Nothing
    here substitutes for the native fixtures; it pins the decision path they
    depend on."""

    def custodian(self):
        read_end, write_end = os.pipe()
        self.addCleanup(os.close, read_end)
        cmd_r, cmd_w = os.pipe()
        self.addCleanup(os.close, cmd_r)
        self.addCleanup(os.close, cmd_w)
        custodian = bounded.Custodian(["command"], limit_s=60.0)
        custodian.run_id = "r"
        custodian.start = 1000 * NS
        custodian.absolute = custodian.start + 60 * NS
        custodian.stop_at = custodian.absolute - bounded.STOP_RESERVE_S * NS
        custodian.exit_at = custodian.absolute - bounded.EXIT_MARGIN_S * NS
        custodian.guard = bounded.Guard(start_ns=custodian.start)
        custodian.leader = 4242
        custodian.leader_status = 0
        custodian.reaped_seq = 3
        custodian.cmd_w = cmd_w
        custodian.monitor = 4243
        custodian.monitor_complete = True
        custodian.monitor_reaped = True
        return custodian

    def message(self, custodian, kind, **fields):
        if kind == "census":
            fields.setdefault("replacement_work", fixture_work())
        return {"run": custodian.run_id, "seq": custodian.monitor_seq + 1, "leader": [4242, 77],
                "type": kind, "at": 1, **fields}

    def settle(self, custodian, census_ok, uncertain, eof):
        custodian.census_ok, custodian.census_uncertain = census_ok, uncertain
        custodian.eof = set(eof)
        custodian.exit_at = 1  # the grace window is already past: a bounded decision
        custodian.settle(True)
        return custodian.guard.fault

    def test_both_distinct_witnesses_may_close_without_fault(self):
        custodian = self.custodian()
        custodian.accept(self.message(custodian, "output_eof", stream="stdout"))
        custodian.accept(self.message(custodian, "output_eof", stream="stderr"))
        self.assertEqual(custodian.eof, {"stdout", "stderr"})
        self.assertIsNone(self.settle(custodian, True, False, custodian.eof))
        self.assertTrue(custodian.closure(True))

    def test_one_or_no_witness_refuses(self):
        for eof in (set(), {"stdout"}, {"stderr"}):
            with self.subTest(eof=sorted(eof)):
                custodian = self.custodian()
                self.assertEqual(self.settle(custodian, True, False, eof), EOF_FAULT)
                self.assertFalse(custodian.closure(True))

    def test_duplicate_stdout_witness_never_counts_as_two(self):
        custodian = self.custodian()
        custodian.accept(self.message(custodian, "output_eof", stream="stdout"))
        custodian.accept(self.message(custodian, "output_eof", stream="stdout"))
        self.assertEqual(custodian.eof, {"stdout"})
        self.assertEqual(self.settle(custodian, True, False, custodian.eof), EOF_FAULT)

    def test_uncertain_census_refuses_before_any_eof_question(self):
        custodian = self.custodian()
        custodian.accept(self.message(custodian, "census", after=3, certain=False, members=0, uncertain=1))
        custodian.accept(self.message(custodian, "census", after=3, certain=True, members=0, uncertain=0))
        custodian.accept(self.message(custodian, "output_eof", stream="stdout"))
        custodian.accept(self.message(custodian, "output_eof", stream="stderr"))
        self.assertIn("census uncertain", custodian.guard.fault)
        self.assertFalse(custodian.closure(True))
        self.assertEqual(self.settle(custodian, False, True, {"stdout", "stderr"}),
                         custodian.guard.fault, "an earlier fault must be preserved")
        self.assertFalse(custodian.closure(True))

    def test_missing_witness_is_named_after_a_certain_census_only(self):
        custodian = self.custodian()
        self.assertEqual(self.settle(custodian, True, False, {"stdout"}), EOF_FAULT)
        custodian = self.custodian()
        self.assertEqual(self.settle(custodian, False, False, set()), CENSUS_FAULT)

    def test_pre_stop_census_does_not_qualify(self):
        custodian = self.custodian()
        custodian.accept(self.message(custodian, "census", after=2, certain=True, members=0, uncertain=0))
        self.assertFalse(custodian.census_ok)
        self.assertFalse(custodian.closure(True))

    def test_fd_transport_api_is_available(self):
        # The held-stream fixtures need real descriptor transport; assert the
        # runtime API rather than assuming it.
        self.assertTrue(callable(getattr(socket, "send_fds", None)))
        self.assertTrue(callable(getattr(socket, "recv_fds", None)))
        self.assertTrue(hasattr(socket, "MSG_CTRUNC"))

    # Negative mutations: each must falsify the oracle it belongs to.

    def test_escaped_writer_oracle_rejects_its_mutations(self):
        good = {"leader_status": 0, "exit": 125, "closure": "UNCLOSED", "fault": CENSUS_FAULT,
                "deadline": False, "ready": True, "nonce_ok": True, "child_pid": 5000,
                "suite_pid": 4000, "signal_targets": [4242], "signal_pins": ["leader", "leader"],
                "retained": True}
        self.assertEqual(escaped_writer_problems(good), [])
        also_eof = dict(good, fault=EOF_FAULT)
        self.assertEqual(escaped_writer_problems(also_eof), [])
        for label, mutation in (("absent child", {"child_pid": None}), ("no readiness", {"ready": False}),
                                ("no escape", {"signal_targets": [5000]}), ("stale nonce", {"nonce_ok": False}),
                                ("no response", {"nonce_ok": False, "error": "missing nonce.reply"}),
                                ("deadline", {"deadline": True}), ("resource fault", {"fault": "reserve readings stale"}),
                                ("unrelated fault", {"fault": "monitor did not complete"}),
                                ("not retained", {"retained": False}),
                                ("suite signalled", {"signal_targets": [4000]}),
                                ("signal outside the pins", {"signal_pins": ["foreign"]}),
                                ("leader failed", {"leader_status": 1}), ("guardian exit", {"exit": 124}),
                                ("closed", {"closure": "closed"})):
            with self.subTest(label):
                self.assertTrue(escaped_writer_problems(dict(good, **mutation)), label)

    def test_eof_veto_oracle_rejects_its_mutations(self):
        good = {"leader_status": 0, "exit": 125, "closure": "UNCLOSED", "fault": EOF_FAULT,
                "deadline": False, "census": {"certain": True, "members": 0, "uncertain": 0},
                "held": ["stdout"], "expected_held": ["stdout"], "suite_pid": 4000,
                "signal_targets": [4242], "retained": True}
        self.assertEqual(eof_veto_problems(good), [])
        for label, mutation in (("wrong pipe", {"held": ["stderr"], "expected_held": ["stdout"]}),
                                ("no hold", {"held": []}), ("uncertain census", {"census": {"certain": False, "members": 0,
                                                                                            "uncertain": 1}}),
                                ("nonempty census", {"census": {"certain": True, "members": 1, "uncertain": 0}}),
                                ("missing census", {"census": None}), ("census fault", {"fault": CENSUS_FAULT}),
                                ("deadline", {"deadline": True}), ("not retained", {"retained": False}),
                                ("leader failed", {"leader_status": 1}), ("suite signalled", {"signal_targets": [4000]})):
            with self.subTest(label):
                self.assertTrue(eof_veto_problems(dict(good, **mutation)), label)

    def test_released_streams_oracle_rejects_an_always_unclosed_oracle(self):
        self.assertEqual(released_streams_problems({"exit": 0, "closure": "closed", "fault": None, "held": []}), [])
        for label, mutation in (("held anyway", {"held": ["stdout"]}), ("unclosed", {"closure": "UNCLOSED"}),
                                ("fault", {"fault": EOF_FAULT}), ("exit", {"exit": 125})):
            with self.subTest(label):
                self.assertTrue(released_streams_problems({"exit": 0, "closure": "closed", "fault": None,
                                                           "held": [], **mutation}), label)


# Settlement observation (oxide#147) --------------------------------------------
#
# The supervisor records observation-only evidence of the 10 s settlement window
# so a run that ends with no completed post-reap census round can be told apart
# from one whose round was certain and empty. These helpers judge a record that
# the REAL monitor/custodian paths produced; the mutations below prove each
# check is load-bearing rather than a restatement of the constants.

SETTLE_KEYS = {
    "schema", "clock", "stopped_sent_ns", "stopped_received_ns", "stopped_seq",
    "receive_latency_ns", "leader_bound", "thread_start_ns", "thread_start_latency_ns",
    "census_built_ns", "first_round_ns", "first_round_latency_ns", "stop_requested_ns",
    "loop_exit_ns", "rounds_started", "rounds_completed", "omitted", "phase", "rounds",
}
SETTLE_ROUND_KEYS = {
    "index", "phase", "start_ns", "scanned_ns", "snapshot_ns", "scan_ns", "rows",
    "esrch_rows", "zombie_rows", "confirm_reads", "pause_calls", "lock_wait_ns",
    "lock_acquired_ns", "lock_released_ns", "sent_ns", "end_ns", "certain", "members",
    "uncertain",
}
SETTLE_ROUND_STAMPS = ("start_ns", "lock_wait_ns", "lock_acquired_ns", "lock_released_ns",
                       "sent_ns", "end_ns")


def _stamp(value):
    return value is None or (type(value) is int and 0 <= value <= bounded.U64MAX)


def _count(value):
    return value is None or (type(value) is int and 0 <= value <= bounded.U64MAX)


def settlement_stamps(record):
    """Every stamp in the record, in the order the real path writes them."""
    stamps = [record.get("stopped_sent_ns"), record.get("stopped_received_ns"),
              record.get("thread_start_ns"), record.get("first_round_ns")]
    for entry in record.get("rounds") or []:
        if isinstance(entry, dict):
            stamps += [entry.get(name) for name in SETTLE_ROUND_STAMPS]
    return [value for value in stamps if value is not None]


def settlement_problems(record):
    """Why an observed settlement record is not usable evidence, or []."""
    if not isinstance(record, dict):
        return ["not an object"]
    problems = []
    if set(record) != SETTLE_KEYS:
        problems.append(f"schema keys differ: {sorted(set(record) ^ SETTLE_KEYS)}")
    if record.get("schema") != 1 or record.get("clock") != "CLOCK_UPTIME_RAW":
        problems.append("unknown schema or clock")
    if record.get("phase") not in bounded.SETTLE_PHASES:
        problems.append(f"unknown phase {record.get('phase')!r}")
    if not isinstance(record.get("leader_bound"), (bool, type(None))):
        problems.append("ill-typed leader_bound")
    if not _count(record.get("stopped_seq")):
        problems.append("ill-typed stopped_seq")
    for name in ("stopped_sent_ns", "stopped_received_ns", "thread_start_ns", "census_built_ns",
                 "first_round_ns", "receive_latency_ns", "thread_start_latency_ns",
                 "first_round_latency_ns", "stop_requested_ns", "loop_exit_ns"):
        if not _stamp(record.get(name)):
            problems.append(f"ill-typed {name}")
    for name in ("rounds_started", "rounds_completed", "omitted"):
        if type(record.get(name)) is not int or record[name] < 0:
            problems.append(f"bad counter {name}")
    rounds = record.get("rounds")
    if not isinstance(rounds, list) or len(rounds) > bounded.SETTLE_MAX_ROUNDS:
        return problems + ["round list missing or over the cap"]
    if type(record.get("rounds_started")) is int and type(record.get("omitted")) is int and \
            record["rounds_started"] != len(rounds) + record["omitted"]:
        problems.append("rounds_started does not equal kept plus omitted")
    completed = 0
    for index, entry in enumerate(rounds):
        if not isinstance(entry, dict) or set(entry) != SETTLE_ROUND_KEYS:
            problems.append(f"round {index} schema")
            continue
        if entry.get("phase") not in bounded.SETTLE_ROUND_STEPS:
            problems.append(f"round {index} unknown phase {entry.get('phase')!r}")
        if entry.get("index") != index:
            problems.append(f"round {index} wrong index")
        if not all(_stamp(entry.get(name)) for name in SETTLE_ROUND_STAMPS):
            problems.append(f"round {index} ill-typed stamp")
            continue
        if not all(_count(entry.get(name)) for name in ("snapshot_ns", "scan_ns", "rows", "esrch_rows",
                                                        "zombie_rows", "confirm_reads", "pause_calls",
                                                        "members", "uncertain")):
            problems.append(f"round {index} ill-typed count")
        if not isinstance(entry.get("certain"), (bool, type(None))):
            problems.append(f"round {index} ill-typed certain")
        stamps = [entry[name] for name in SETTLE_ROUND_STAMPS if entry[name] is not None]
        if any(later < earlier for earlier, later in zip(stamps, stamps[1:])):
            problems.append(f"round {index} stamps not non-decreasing: {stamps}")
        if entry.get("phase") == "complete":
            completed += 1
            if len(stamps) != len(SETTLE_ROUND_STAMPS):
                problems.append(f"round {index} complete with a missing stamp")
            if entry.get("scanned_ns") is None or entry.get("certain") is None:
                problems.append(f"round {index} complete without its phases")
            elif entry["scanned_ns"] < entry["start_ns"]:
                problems.append(f"round {index} scanned before it started")
    if type(record.get("rounds_completed")) is int and record.get("omitted") == 0 \
            and completed != record["rounds_completed"]:
        problems.append("rounds_completed does not match the complete records")
    chain = [value for value in (record.get("stopped_sent_ns"), record.get("stopped_received_ns"),
                                 record.get("thread_start_ns"), record.get("first_round_ns"),
                                 rounds[0].get("start_ns") if rounds else None) if type(value) is int]
    if any(later < earlier for earlier, later in zip(chain, chain[1:])):
        problems.append(f"settlement stamps out of order: {chain}")
    every = settlement_stamps(record)
    if len(every) > 2 and len(set(every)) == 1:
        problems.append("every stamp is one value: a frozen clock, not an observation")
    try:
        blob = json.dumps(record, ensure_ascii=True, separators=(",", ":")).encode("utf-8")
        if len(blob) > bounded.SETTLE_MAX_BYTES:
            problems.append(f"settlement over the byte ceiling: {len(blob)}")
    except BaseException:
        problems.append("settlement is not serializable")
    return problems


class FakeCensus:
    """Stands in for the host snapshot in the pure settlement cases: it reads
    no host process and spawns nothing."""

    rounds = 0

    def __init__(self, sid, terminal_allowed=False, ancestors=(), getpgid=None):
        self.sid, self.terminal_allowed = sid, terminal_allowed
        self.ancestors = tuple(ancestors)
        self.stats = {"rows": 3, "esrch_rows": 1, "zombie_rows": 1, "confirm_reads": 2,
                      "pause_calls": 1, "terminal": 0, "snapshot_ns": 5, "scan_ns": 7,
                      "snapshot_error": False}

    def round(self):
        self.round_deadline = bounded.now_ns() + 2 * NS
        FakeCensus.rounds += 1
        return {"certain": True, "members": 0, "uncertain": 0, "member_rows": [],
                "uncertain_rows": [], "terminal_count_status": "valid", "terminal_nonwriters": 0, "replacement_work": fixture_work()}


class OneShotStop:
    """`census_stop` stand-in whose first wait ends the real loop after the
    round it follows, so the loop body runs exactly once."""

    def __init__(self, stopped=False):
        self.stopped = stopped

    def is_set(self):
        return self.stopped

    def wait(self, _timeout):
        self.stopped = True
        return True

    def set(self):
        self.stopped = True


DETACH_UUID = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"


class DetachFacts:
    """Substituted detach seams: the kernel and `hdiutil` facts the runner reads,
    plus the one detach attempt. `mount_point` and `attachments` take either a
    fixed answer or a function of `attempted`, so a case states exactly what was
    true before and after its attempt and a refusal cannot pass by accident."""

    def __init__(self, mount="/Volumes/OxDetach", device="/dev/disk7", uuid=DETACH_UUID,
                 mount_point=None, attachments=None, attempt=None, stats=None, nodes=()):
        self.mount, self.device, self.uuid = mount, device, uuid
        self.mount_point_value = mount_point
        self.attachments_value = attachments
        self.attempt = attempt or {"exit": 0, "timed_out": False, "error": None, "output": ""}
        self.stats = stats or {}
        self.nodes = set(nodes)
        self.attempted = False
        self.detached = []

    def realpath(self, path):
        return os.path.realpath(path)

    def lstat(self, path):
        if path in self.nodes:  # a device node this case declares present
            return types.SimpleNamespace(st_mode=stat.S_IFCHR, st_rdev=42)
        return os.lstat(path)

    def stat(self, path):
        return self.stats[path] if path in self.stats else os.stat(path)

    def bound_stats(self):
        return {"/dev/disk7s1": types.SimpleNamespace(st_rdev=42),
                "/Volumes/OxDetach": types.SimpleNamespace(st_dev=42)}

    def rows(self, mounted):
        return ([(self.device, None, None), (self.device + "s1", self.mount, self.uuid)]
                if mounted else [])

    def mount_point(self, path):
        if callable(self.mount_point_value):
            return self.mount_point_value(self.attempted)
        if self.mount_point_value is not None:
            return self.mount_point_value
        # Unstated: mounted before the attempt and, by default, detached by a
        # successful one.
        success = self.attempt.get("exit") == 0 and not self.attempt.get("timed_out")
        return not (self.attempted and success)

    def volume_uuid(self, path):
        return self.uuid

    def attachments(self, backing, timeout=None):
        if callable(self.attachments_value):
            return self.attachments_value(self.attempted)
        if self.attachments_value is not None:
            return list(self.attachments_value)
        return self.rows(self.mount_point("/Volumes/OxDetach"))

    def detach(self, mount, timeout, still_allowed=None):
        self.detached.append((mount, timeout))
        self.attempted = True
        self.polled = still_allowed
        attempt = {"reaped": True}
        attempt.update(self.attempt)
        return attempt


class PureDetach(unittest.TestCase):
    """Oxide#191: the opt-in post-census detach a proof requests. Pure: no image,
    no mount, no child, no supervisor. It proves the request is bounded and
    never partly trusted, that only a positively re-derived identity is detached,
    that exactly one ordinary attempt can happen, and that every refusal, failure,
    timeout or ambiguity keeps the mount and is unqualified."""

    def layout(self):
        sandbox = Path(tempfile.mkdtemp(prefix="detach.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(sandbox), True)
        root = os.path.join(str(sandbox), "oxide-proof.aaa")
        evidence = os.path.join(root, "evidence")
        os.makedirs(os.path.join(root, "scratch"), 0o700)
        os.makedirs(evidence, 0o700)
        backing = os.path.join(root, "scratch", "volume.sparseimage")
        with open(backing, "wb") as handle:
            handle.write(b"sparse image placeholder")
        ctx = {"owned": True, "root": root, "scratch": os.path.join(root, "scratch"),
               "evidence": evidence, "reserve_path": root, "inject": [],
               "start_ns": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW), "limit_s": 300}
        return ctx, backing

    def facts(self, **changes):
        facts = DetachFacts(**changes)
        facts.stats.update(facts.bound_stats())
        return facts

    def request(self, ctx, backing, **changes):
        document = {"version": 1, "backing": backing, "mount": "/Volumes/OxDetach",
                    "device": "/dev/disk7", "volume_uuid": DETACH_UUID.upper(),
                    "note": "self-check"}
        document.update(changes)
        path = os.path.join(ctx["evidence"], bounded.DETACH_REQUEST_NAME)
        with open(path, "w") as handle:
            json.dump(document, handle)
        return path

    def test_no_request_is_the_unchanged_runner(self):
        ctx, _backing = self.layout()
        facts = self.facts()
        self.assertIsNone(bounded.post_census_detach(ctx, facts))
        self.assertEqual(facts.detached, [], "a detach ran without a request")

    def test_a_bound_request_detaches_exactly_once(self):
        ctx, backing = self.layout()
        facts = self.facts()
        self.request(ctx, backing)
        record = bounded.post_census_detach(ctx, facts)
        self.assertEqual(facts.detached[0][0], "/Volumes/OxDetach")
        self.assertLessEqual(facts.detached[0][1], bounded.DETACH_TIMEOUT_S)
        self.assertEqual(len(facts.detached), 1, "more than one detach attempt")
        self.assertEqual((record["state"], record["qualified"], record["post_state"]),
                         ("detached", True, "detached"))
        self.assertEqual((record["attempted"], record["exit"], record["timed_out"]), (True, 0, False))
        self.assertIsNone(record["reason"])
        self.assertEqual(record["backing"], backing, "the record does not name the requested backing")
        self.assertIsNotNone(record["reserve_before"])
        self.assertIsNone(record["reserve_after"] if record["state"] != "detached" else None)
        self.assertEqual(record["deadline_ns"], ctx["start_ns"] + 300 * bounded.NS)

    def test_a_wrong_uuid_device_or_mount_refuses_without_detaching(self):
        for changes, expected in (({"volume_uuid": "11111111-2222-3333-4444-555555555555"}, "uuid"),
                                  ({"device": "/dev/disk9"}, "device"),
                                  ({"mount": "/Volumes/OxOther"}, "mount"),
                                  ({"backing": "/private/etc/passwd"}, "outside"),
                                  ({"backing": "/Volumes"}, "regular file"),
                                  ({"version": 2}, "version"),
                                  ({"device": "/dev/disk7s1s9"}, "device"),
                                  ({"mount": "/Volumes/../Volumes/OxDetach"}, "canonical"),
                                  ({"extra": "unexpected"}, "unknown fields")):
            ctx, backing = self.layout()
            facts = self.facts()
            changes = dict(changes)
            self.request(ctx, changes.pop("backing", backing), **changes)
            record = bounded.post_census_detach(ctx, facts)
            self.assertEqual(facts.detached, [], f"{changes} was detached")
            self.assertEqual((record["state"], record["qualified"], record["attempted"]),
                             ("refused", False, False), f"{changes}: {record}")
            self.assertIn(expected, (record["reason"] or "").lower(), f"{changes}: {record['reason']}")

    def test_a_mount_that_is_not_this_image_or_not_mounted_refuses(self):
        for changes, expected in (({"mount_point": False}, "mount point"),
                                  ({"attachments": lambda attempted: []}, "not attached")):
            ctx, backing = self.layout()
            facts = self.facts(**changes)
            self.request(ctx, backing)
            record = bounded.post_census_detach(ctx, facts)
            self.assertEqual(facts.detached, [], f"{changes} was detached")
            self.assertEqual(record["state"], "refused", record)
            self.assertIn(expected, record["reason"], record["reason"])

    def test_a_symlinked_backing_or_request_is_refused(self):
        ctx, backing = self.layout()
        link = os.path.join(ctx["scratch"], "link.sparseimage")
        os.symlink(backing, link)
        facts = self.facts()
        self.request(ctx, link)
        record = bounded.post_census_detach(ctx, facts)
        self.assertEqual(facts.detached, [], "a symlinked backing was detached")
        self.assertIn("symlink", record["reason"])

        ctx, backing = self.layout()
        target = os.path.join(ctx["evidence"], "elsewhere.json")
        with open(target, "w") as handle:
            json.dump({"version": 1, "backing": backing, "mount": "/Volumes/OxDetach",
                       "device": "/dev/disk7", "volume_uuid": DETACH_UUID}, handle)
        os.symlink(target, os.path.join(ctx["evidence"], bounded.DETACH_REQUEST_NAME))
        record = bounded.post_census_detach(ctx, self.facts())
        self.assertEqual(record["state"], "refused")
        self.assertIn("regular file", record["reason"])

    def test_an_oversize_or_malformed_request_is_refused(self):
        for payload, expected in ((b"{" + b" " * bounded.DETACH_REQUEST_MAX_BYTES + b"}", "bytes"),
                                  (b"not json", "JSON"), (b"[]", "object"),
                                  (b'{"version": 1, "mount": "\\u0000"}', "bounded single-line")):
            ctx, _backing = self.layout()
            with open(os.path.join(ctx["evidence"], bounded.DETACH_REQUEST_NAME), "wb") as handle:
                handle.write(payload)
            facts = self.facts()
            record = bounded.post_census_detach(ctx, facts)
            self.assertEqual((record["state"], record["attempted"]), ("refused", False), record)
            self.assertEqual(facts.detached, [], f"{payload} was detached")
            self.assertIn(expected, record["reason"], record["reason"])

    def test_a_caller_owned_root_detaches_nothing(self):
        ctx, backing = self.layout()
        ctx["owned"] = False
        self.request(ctx, backing)
        facts = self.facts()
        record = bounded.post_census_detach(ctx, facts)
        self.assertEqual(record["state"], "refused")
        self.assertEqual(facts.detached, [], "a caller-owned root detached an image")
        self.assertIn("belongs to the caller", record["reason"])

    def test_a_deadline_reserve_refuses_before_any_attempt(self):
        ctx, backing = self.layout()
        ctx["limit_s"] = 1  # the custodian's wait window has already closed
        facts = self.facts()
        self.request(ctx, backing)
        record = bounded.post_census_detach(ctx, facts)
        self.assertEqual(facts.detached, [], "a detach ran past the custodian's wait window")
        self.assertEqual((record["state"], record["qualified"]), ("refused", False))
        self.assertIn("wait window", record["reason"])

    def test_a_failed_or_timed_out_attempt_keeps_the_mount(self):
        for attempt, state in (({"exit": 1, "timed_out": False, "error": None, "output": "busy"}, "failed"),
                               ({"exit": None, "timed_out": True, "error": None, "output": ""}, "timed_out")):
            ctx, backing = self.layout()
            facts = self.facts(attempt=attempt, mount_point=True)
            self.request(ctx, backing)
            record = bounded.post_census_detach(ctx, facts)
            self.assertEqual((record["state"], record["qualified"], record["post_state"]),
                             (state, False, "still_mounted"), record)
            self.assertIsNotNone(record["reason"])

    def test_a_zero_exit_that_leaves_the_mount_is_ambiguous(self):
        ctx, backing = self.layout()
        facts = self.facts(mount_point=True)
        self.request(ctx, backing)
        record = bounded.post_census_detach(ctx, facts)
        self.assertEqual((record["state"], record["post_state"], record["qualified"]),
                         ("failed", "still_mounted", False), record)

    def test_a_detach_that_leaves_the_device_attached_is_not_detached(self):
        ctx, backing = self.layout()
        facts = self.facts(
            mount_point=lambda attempted: not attempted,
            attachments=lambda attempted: [("/dev/disk7", None, None),
                                           ("/dev/disk7s1", "/Volumes/OxDetach", DETACH_UUID)])
        self.request(ctx, backing)
        record = bounded.post_census_detach(ctx, facts)
        self.assertEqual((record["state"], record["post_state"], record["qualified"]),
                         ("failed", "still_attached", False), record)

    def test_unreadable_attachment_facts_refuse(self):
        ctx, backing = self.layout()
        facts = self.facts(attachments=lambda attempted: None)
        self.request(ctx, backing)
        record = bounded.post_census_detach(ctx, facts)
        self.assertEqual(facts.detached, [], "unreadable attachment facts still detached")
        self.assertIn("attachment facts are unreadable", record["reason"])

    def test_an_unknown_mount_post_state_is_unqualified(self):
        ctx, backing = self.layout()
        facts = self.facts(mount_point=lambda attempted: None if attempted else True)
        self.request(ctx, backing)
        record = bounded.post_census_detach(ctx, facts)
        self.assertEqual((record["state"], record["post_state"], record["qualified"]),
                         ("failed", "unknown", False), record)

    def test_a_null_or_non_object_request_is_refused(self):
        for payload in (b"null", b"[]", b'"text"', b"7", b"true"):
            ctx, _backing = self.layout()
            with open(os.path.join(ctx["evidence"], bounded.DETACH_REQUEST_NAME), "wb") as handle:
                handle.write(payload)
            facts = self.facts()
            record = bounded.post_census_detach(ctx, facts)
            self.assertIsNotNone(record, f"{payload} was read as an absent request")
            self.assertEqual((record["state"], record["attempted"], record["qualified"]),
                             ("refused", False, False), record)
            self.assertIn("not an object", record["reason"])
            self.assertEqual(facts.detached, [], f"{payload} was detached")

    def test_the_kept_output_is_bounded(self):
        big = b"x" * (4 * bounded.DETACH_OUTPUT_MAX_BYTES)
        tail = b""
        for index in range(0, len(big), 1024):
            tail = bounded._bounded_tail(tail, big[index:index + 1024])
            self.assertLessEqual(len(tail), bounded.DETACH_OUTPUT_MAX_BYTES)
        self.assertEqual(tail, big[-bounded.DETACH_OUTPUT_MAX_BYTES:])
        self.assertLessEqual(len(bounded.bounded_run.__doc__), 4096)

    def test_the_authorization_gate_is_the_custodians_verdict(self):
        now, exit_at = 1000 * bounded.NS, 1060 * bounded.NS
        allowed, reason, budget = bounded.detach_authorization(None, False, now, exit_at)
        self.assertTrue(allowed, reason)
        self.assertGreater(budget, 0)
        self.assertLessEqual(budget, 60 * bounded.NS)
        for fault, deadline, when, text in (("a fault", False, now, "fault is latched"),
                                            (None, True, now, "deadline"),
                                            (None, False, exit_at, "wait budget")):
            allowed, reason, budget = bounded.detach_authorization(fault, deadline, when, exit_at)
            self.assertFalse(allowed, (fault, deadline, when))
            self.assertEqual(budget, 0)
            self.assertIn(text, reason)

    def test_the_detach_budget_ends_before_the_custodian_wait_ends(self):
        ctx, _backing = self.layout()
        start = ctx["start_ns"]
        self.assertEqual(bounded.detach_budget(ctx, start), 300 * bounded.NS - bounded.EXIT_MARGIN_S * bounded.NS
                         - bounded.DETACH_GATE_MARGIN_NS)
        late = start + (300 - bounded.EXIT_MARGIN_S) * bounded.NS
        self.assertEqual(bounded.detach_budget(ctx, late), 0)
        # A spent budget refuses before any attempt, never past the custodian's wait.
        ctx, backing = self.layout()
        ctx["start_ns"] = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) - int(300 * bounded.NS)
        facts = self.facts()
        self.request(ctx, backing)
        record = bounded.post_census_detach(ctx, facts)
        self.assertEqual(facts.detached, [], "a detach ran past the custodian's wait window")
        self.assertEqual(record["state"], "refused")
        self.assertIn("wait window", record["reason"])

    def test_the_attempt_is_clamped_by_the_authorized_budget(self):
        ctx, backing = self.layout()
        self.request(ctx, backing)
        granted = []
        record = bounded.post_census_detach(
            ctx, self.facts(), authorize=lambda budget, wait: (granted.append((budget, wait))
                                                              or (True, None, 4 * bounded.NS)))
        self.assertTrue(record["qualified"], record)
        # The granted four-second window clamps the whole remaining sequence.
        self.assertLessEqual(record["timeout_s"], 4.0)
        self.assertGreaterEqual(record["timeout_s"], 1.0)
        self.assertEqual(granted[0][0] > 0, True)

    def test_a_withdrawn_authorization_detaches_nothing(self):
        ctx, backing = self.layout()
        facts = self.facts()
        self.request(ctx, backing)
        record = bounded.post_census_detach(
            ctx, facts, authorize=lambda budget, wait: (False, "a fault is latched (injected)", 0))
        self.assertEqual(facts.detached, [], "a detach ran without the custodian's authorization")
        self.assertEqual((record["state"], record["attempted"], record["qualified"]),
                         ("refused", False, False), record)
        self.assertIn("a fault is latched", record["reason"])

    def test_the_monitor_takes_no_detach_without_a_usable_answer(self):
        def monitor(answer, close=False):
            monitor = bounded.Monitor.__new__(bounded.Monitor)
            _ctl_r, _ctl_w = os.pipe()
            cmd_r, cmd_w = os.pipe()
            self.addCleanup(os.close, _ctl_r)
            self.addCleanup(os.close, _ctl_w)
            self.addCleanup(os.close, cmd_r)
            monitor.cmd_r = cmd_r
            monitor.sent = []
            monitor.send = lambda message_type, **fields: monitor.sent.append((message_type, fields))
            if answer is not None:
                os.write(cmd_w, (json.dumps(answer) + "\n").encode())
            if close:
                os.close(cmd_w)
            else:
                self.addCleanup(os.close, cmd_w)
            return monitor

        monitor_ = monitor({"type": "detach_go", "allowed": True, "budget_ns": 5 * bounded.NS})
        self.assertEqual(monitor_.authorize_detach(10 * bounded.NS), (True, None, 5 * bounded.NS))
        self.assertEqual(monitor_.sent[0][0], "detach_ready")
        self.assertEqual(monitor_.sent[0][1], {"budget_ns": 10 * bounded.NS})
        for answer, expected in (({"type": "detach_go", "allowed": False, "reason": "the deadline"},
                                  "the deadline"),
                                 ({"type": "detach_go", "allowed": True, "budget_ns": "lots"},
                                  "no usable budget"),
                                 ({"type": "other"}, "in time"),
                                 ("not an object", "in time")):
            monitor_ = monitor(answer)
            self.assertFalse(monitor_.authorize_detach(200_000_000)[0], (answer, expected))
        monitor_ = monitor(None, close=True)
        self.assertFalse(monitor_.authorize_detach(5 * bounded.NS)[0])

    def declare_custody(self, ctx, *backings):
        path = os.path.join(ctx["evidence"], bounded.CUSTODY_NAME)
        with open(path, "w") as handle:
            json.dump({"version": bounded.CUSTODY_VERSION, "backings": list(backings)}, handle)
        return path

    def test_without_a_custody_indication_cleanup_performs_no_scan(self):
        ctx, _backing = self.layout()
        calls = []
        self.assertIsNone(bounded.cleanup_safety(ctx, self.facts(), facts=lambda *a: calls.append(a) or []))
        self.assertEqual(calls, [], "a run without an indication still scanned attachments")

    def test_a_declared_mounted_image_keeps_the_owned_root(self):
        ctx, backing = self.layout()
        self.declare_custody(ctx, backing)
        rows = [("/dev/disk7", "/Volumes/OxDetach", None)]  # (device, mount, uuid)
        record = bounded.cleanup_safety(ctx, self.facts(), facts=lambda *a: rows)
        self.assertEqual(record["state"], "kept")
        self.assertEqual(record["unrequested"], [backing])
        self.assertIn("mounted", record["reason"])

    def test_attached_but_unmounted_is_not_positively_detached(self):
        # The reviewer's second case: no mount point, but the device node still
        # exists, so the image is attached. It must keep the root.
        ctx, backing = self.layout()
        self.declare_custody(ctx, backing)
        facts = self.facts(nodes=("/dev/disk7s1",))
        rows = [("/dev/disk7s1", None, None)]
        record = bounded.cleanup_safety(ctx, facts, facts=lambda *a: rows)
        self.assertEqual(record["state"], "kept")
        self.assertIn("attached without a mount point", record["reason"])

    def test_stale_metadata_with_no_device_node_keeps_nothing(self):
        ctx, backing = self.layout()
        self.declare_custody(ctx, backing)
        rows = [("/dev/disk99s1", None, None)]  # no such node on this host
        self.assertEqual(bounded.cleanup_safety(ctx, self.facts(), facts=lambda *a: rows)["state"], "ok")

    def test_uncertain_custody_state_keeps_the_owned_root(self):
        # Unreadable attachment facts are uncertain, so the root is kept.
        ctx, backing = self.layout()
        self.declare_custody(ctx, backing)
        self.assertIn("unreadable", bounded.cleanup_safety(
            ctx, self.facts(), facts=lambda *a: None)["reason"])
        # An unreadable device-node state is uncertain too.
        ctx, backing = self.layout()
        self.declare_custody(ctx, backing)
        probe = self.facts()
        real_lstat = os.lstat

        def lstat(path):
            if path == "/dev/disk7":
                raise PermissionError("denied")
            return real_lstat(path)

        probe.lstat = lstat
        record = bounded.cleanup_safety(ctx, probe, facts=lambda *a: [("/dev/disk7", None, None)])
        self.assertEqual(record["state"], "kept")
        self.assertIn("unreadable", record["reason"])

    def test_the_post_attempt_gate_withdraws_success(self):
        ctx, backing = self.layout()
        self.request(ctx, backing)
        answers = []

        def still_allowed():
            answers.append(1)
            return False, "a fault latched after the attempt"

        record = bounded.post_census_detach(ctx, self.facts(), still_allowed=still_allowed)
        self.assertEqual((record["state"], record["qualified"]), ("withdrawn", False), record)
        self.assertIn("after the attempt", record["reason"])
        self.assertTrue(answers)

    def test_an_unreaped_detach_child_is_not_a_detach(self):
        ctx, backing = self.layout()
        self.request(ctx, backing)
        record = bounded.post_census_detach(ctx, self.facts(), still_allowed=None)
        self.assertTrue(record["qualified"], record)  # baseline for the injected case
        ctx, backing = self.layout()
        ctx["inject"] = ["detach-reap-timeout"]
        self.request(ctx, backing)
        record = bounded.post_census_detach(ctx, self.facts())
        self.assertEqual((record["state"], record["qualified"], record["reap_timed_out"]),
                         ("failed", False, True), record)

    def test_an_opted_in_but_clear_root_reports_ok_and_is_the_only_case_gated(self):
        ctx, backing = self.layout()
        self.declare_custody(ctx, backing)
        record = bounded.cleanup_safety(ctx, self.facts(), facts=lambda *a: [])
        self.assertEqual(record["state"], "ok")
        self.assertEqual(record["unrequested"], [backing])
        # Not opted in: no indication means no record at all, so the monitor never
        # gated, scanned or changed this run's cleanup path.
        ctx, _backing = self.layout()
        self.assertIsNone(bounded.cleanup_safety(ctx, self.facts(), facts=lambda *a: []))

    def test_an_attached_volume_with_an_unexpected_uuid_still_keeps(self):
        # A UUID mismatch is not evidence of a detach: any live entity keeps the
        # root, so a failed parse or a wrong UUID can never delete a backing.
        ctx, backing = self.layout()
        self.declare_custody(ctx, backing)
        rows = [("/dev/disk7s1", None, "ffffffff-1111-2222-3333-444444444444")]
        record = bounded.cleanup_safety(ctx, self.facts(nodes=("/dev/disk7s1",)), facts=lambda *a: rows)
        self.assertEqual(record["state"], "kept")
        self.assertIn("attached without a mount point", record["reason"])

    def test_an_unreaped_detach_child_is_never_claimed_as_reaped(self):
        ctx, backing = self.layout()
        ctx["inject"] = ["detach-reap-timeout"]
        self.request(ctx, backing)
        record = bounded.post_census_detach(ctx, self.facts())
        self.assertFalse(record["reaped"])
        self.assertFalse(record["qualified"])

    def test_a_spent_window_spawns_nothing(self):
        spawned = []
        result = bounded.bounded_run(["/bin/true"], 0.0, 64,
                                     spawn=lambda *a, **k: spawned.append(1) or None)
        self.assertEqual(spawned, [], "a spent window still spawned a child")
        self.assertEqual(result["error"], "no time left in the window")
        self.assertFalse(result["reaped"])

    def test_no_wait_exceeds_the_granted_window(self):
        ctx, backing = self.layout()
        self.request(ctx, backing)
        record = bounded.post_census_detach(ctx, self.facts(), authorize=lambda b, w: (True, None, b))
        self.assertTrue(record["qualified"], record)
        self.assertLessEqual(record["timeout_s"] * bounded.NS, record["budget_ns"] + bounded.NS // 100)
        # A budget that cannot hold the attempt refuses instead of spawning one.
        ctx, backing = self.layout()
        # One second short of the minimum: the window cannot hold a bounded attempt.
        ctx["start_ns"] = (time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
                           - (300 * bounded.NS - bounded.DETACH_MIN_BUDGET_NS - bounded.NS))
        facts = self.facts()
        self.request(ctx, backing)
        record = bounded.post_census_detach(ctx, facts)
        self.assertEqual(facts.detached, [], "an attempt ran below the minimum budget")
        self.assertEqual(record["state"], "refused")
        self.assertIn("wait window", record["reason"])

    def test_a_recycled_or_reparented_pid_is_never_signalled(self):
        # The row is the REAL `Kinfo.parse` shape: pid, start, zombie (boolean),
        # uid, ppid, pgid — there is no `stat` key at all.
        real = bounded.Kinfo.one(os.getpid())
        self.assertIsNotNone(real)
        self.assertEqual(sorted(real), ["pgid", "pid", "ppid", "start", "uid", "zombie"])
        self.assertIsInstance(real["zombie"], bool)
        self.assertNotIn("stat", real)
        self.assertTrue(bounded.detach_child_identity(real, os.getpid(), real["start"], os.getppid()))
        for row, pid, start, parent in ((dict(real, start=real["start"] + 1), real["pid"], real["start"],
                                         os.getppid()),                                  # recycled PID
                                        (dict(real, ppid=1), real["pid"], real["start"], os.getppid()),
                                        (dict(real, zombie=True), real["pid"], real["start"], os.getppid()),
                                        (None, real["pid"], real["start"], os.getppid()),  # gone
                                        (dict(real, pid=real["pid"] + 1), real["pid"], real["start"],
                                         os.getppid())):                                   # other process
            self.assertFalse(bounded.detach_child_identity(row, pid, start, parent), row)

    def test_observation_reads_the_real_row_shape(self):
        # `observe_detach_child` must accept the real row shape (no `stat`) and
        # treat a zombie row as closed.
        class FakeKinfo:
            rows = []

            @staticmethod
            def one(pid):
                return FakeKinfo.rows[0] if FakeKinfo.rows else None

        real = bounded.Kinfo.one(os.getpid())
        custodian = bounded.Custodian.__new__(bounded.Custodian)
        custodian.detach_child = [real["pid"], real["start"]]
        custodian.absolute = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) + 2 * bounded.NS
        custodian.pump = lambda _timeout: None
        recorded = []
        custodian.record = lambda kind, **fields: recorded.append((kind, fields))
        saved = bounded.Kinfo
        try:
            bounded.Kinfo = FakeKinfo
            FakeKinfo.rows = [dict(real, zombie=False)]
            self.assertFalse(custodian.observe_detach_child(bounded.NS // 10))
            FakeKinfo.rows = [dict(real, zombie=True)]
            self.assertTrue(custodian.observe_detach_child(bounded.NS // 10))
            FakeKinfo.rows = []
            self.assertTrue(custodian.observe_detach_child(bounded.NS // 10))
        finally:
            bounded.Kinfo = saved
        self.assertTrue(all(kind == "detach_child_observed" for kind, _f in recorded), recorded)

    def test_an_unreadable_identity_is_never_read_as_gone(self):
        # Real row, then the same pid unreadable: the safe read reports a problem
        # and the caller can never mistake it for absence.
        real = bounded.Kinfo.one(os.getpid())
        row, problem = bounded.process_row(os.getpid())
        self.assertIsNone(problem)
        self.assertEqual(row["pid"], real["pid"])

        class BrokenKinfo:
            @staticmethod
            def one(pid):
                raise OSError("sysctl read failed")

        class MalformedKinfo:
            @staticmethod
            def one(pid):
                return {"pid": pid, "start": None}

        saved = bounded.Kinfo
        try:
            bounded.Kinfo = BrokenKinfo
            self.assertEqual(bounded.process_row(4242), (None, "OSError"))
            bounded.Kinfo = MalformedKinfo
            self.assertEqual(bounded.process_row(4242), (None, "malformed process row"))
        finally:
            bounded.Kinfo = saved
        self.assertEqual(bounded.process_row(0), (None, "no usable pid"))

    def custodian_stub(self):
        custodian = bounded.Custodian.__new__(bounded.Custodian)
        custodian.guard = bounded.Guard(time.clock_gettime_ns(time.CLOCK_UPTIME_RAW))
        custodian.run_id = "run"
        custodian.monitor_seq = 0
        custodian.monitor = os.getpid()
        custodian.leader = None
        custodian.leader_bound = False
        custodian.detach_child = None
        custodian.records, custodian.told = [], []
        custodian.record = lambda kind, **fields: custodian.records.append((kind, fields))
        custodian.tell = lambda kind, **fields: custodian.told.append((kind, fields))
        custodian.signal_detach_child = lambda: custodian.records.append(("detach_child_signal", {}))
        custodian.observe_detach_child = lambda limit: True
        return custodian

    def test_the_handoff_refuses_an_unreadable_identity_without_signing(self):
        real = bounded.Kinfo.one(os.getpid())
        message = {"run": "run", "seq": 1, "at": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW),
                   "type": "detach_child", "leader": None, "pid": real["pid"], "start": real["start"]}

        class BrokenKinfo:
            @staticmethod
            def one(pid):
                raise OSError("sysctl read failed")

        custodian = self.custodian_stub()
        saved = bounded.Kinfo
        try:
            bounded.Kinfo = BrokenKinfo
            custodian.accept(dict(message))
        finally:
            bounded.Kinfo = saved
        self.assertIsNone(custodian.detach_child, "an unreadable identity was pinned")
        self.assertNotIn("detach_child_signal", [kind for kind, _f in custodian.records])
        self.assertEqual([kind for kind, _f in custodian.records], ["detach_child_refused"])
        ack = custodian.told[-1]
        self.assertEqual(ack[0], "detach_child_ack")
        self.assertIs(ack[1]["custodian_closes"], False)
        self.assertIs(ack[1]["closed"], False)
        self.assertIn("could not be read", ack[1]["reason"])

    def test_the_handoff_pins_signals_and_acknowledges_a_live_child(self):
        # The pinned child must still be this (stubbed) monitor's own child.
        real = dict(bounded.Kinfo.one(os.getpid()), pid=os.getpid() + 1, start=1234,
                    ppid=os.getpid(), zombie=False)
        message = {"run": "run", "seq": 1, "at": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW),
                   "type": "detach_child", "leader": None, "pid": real["pid"], "start": real["start"]}
        custodian = self.custodian_stub()

        class FakeKinfo:
            @staticmethod
            def one(pid):
                return dict(real) if pid == real["pid"] else None

        saved = bounded.Kinfo
        try:
            bounded.Kinfo = FakeKinfo
            custodian.accept(dict(message))
        finally:
            bounded.Kinfo = saved
        self.assertEqual(custodian.detach_child, [real["pid"], real["start"]])
        self.assertEqual([kind for kind, _f in custodian.records],
                         ["detach_child_pinned", "detach_child_signal"])
        ack = custodian.told[-1]
        self.assertIs(ack[1]["custodian_closes"], True)
        self.assertIs(ack[1]["closed"], True)

    def test_an_unreadable_observation_is_uncertain_not_closed(self):
        real = bounded.Kinfo.one(os.getpid())

        class BrokenKinfo:
            @staticmethod
            def one(pid):
                raise OSError("sysctl read failed")

        custodian = bounded.Custodian.__new__(bounded.Custodian)
        custodian.detach_child = [real["pid"], real["start"]]
        custodian.absolute = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) + 2 * bounded.NS
        custodian.pump = lambda _timeout: None
        recorded = []
        custodian.record = lambda kind, **fields: recorded.append((kind, fields))
        saved = bounded.Kinfo
        try:
            bounded.Kinfo = BrokenKinfo
            self.assertFalse(custodian.observe_detach_child(bounded.NS // 50),
                             "an unreadable observation was reported closed")
        finally:
            bounded.Kinfo = saved
        kind, fields = recorded[-1]
        self.assertEqual(kind, "detach_child_observed")
        self.assertIs(fields["closed"], False)
        self.assertEqual(fields["uncertain"], "OSError")

    def test_the_monitor_closes_or_hands_off_an_identity_less_child(self):
        # Behaviour is covered end-to-end by the two injected real cases; this
        # pins the shape: the safe reader, the bounded own-child close, and the
        # parentage-only handoff that carries no start time.
        source = inspect.getsource(bounded.Monitor.run)
        self.assertIn("process_row(getattr(handle", source)
        self.assertIn("close_own_child(handle, own_deadline)", source)
        self.assertIn("left_ns * 3", source)  # part of the window is kept for the handoff
        self.assertIn("start=None", source)
        self.assertIn("detach_child_identity_unreadable: closing_own_child", source)
        self.assertIn("detach_child_closed_by_monitor: scratch_kept", source)
        self.assertNotIn("Kinfo.one", source)

    def test_the_child_descriptor_is_closed_once_by_one_owner(self):
        # The reviewed hazard: closing the raw descriptor while its buffered
        # object still owned it, across a wait, could close a descriptor another
        # thread had reused in the interval.
        read_fd, write_fd = os.pipe()
        stream = os.fdopen(read_fd, "rb", buffering=4096)
        self.addCleanup(lambda: (os.close(read_fd) if False else None))

        class FakeChild:
            pid = 0

            def __init__(self):
                self.stdout = stream
                self.returncode = 0
                self.waits = []

            def poll(self):
                return 0

            def kill(self):
                pass

            def wait(self, timeout=None):
                self.waits.append(timeout)
                # While the parent waits, another thread may take the freed number.
                return 0

        child = FakeChild()
        old_fd = stream.fileno()
        os.write(write_fd, b"out")
        os.close(write_fd)
        result = bounded.bounded_run(["/bin/true"], 2, 64, spawn=lambda *a, **k: child)
        self.assertEqual(result["exit"], 0, result)
        self.assertIsNone(child.stdout, "the detached buffer was kept")
        # The buffer was detached (so it can never finalise the descriptor) and the
        # descriptor itself is closed by its single owner.
        with self.assertRaises(ValueError):
            stream.closed
        with self.assertRaises(OSError):
            os.fstat(old_fd)
        # The descriptor number can be reused at once and survives collection.
        keep = os.open(os.devnull, os.O_RDONLY)
        self.addCleanup(os.close, keep)
        gc.collect()
        os.fstat(keep)  # raises if the stale stream closed a reused descriptor

    def test_the_own_child_close_is_bounded_and_never_raises(self):
        class FakeChild:
            pid = 0  # no signal is attempted for pid 0

            def __init__(self, statuses):
                self.statuses = list(statuses)

            def wait(self, timeout=None):
                if not self.statuses:
                    return None
                value = self.statuses.pop(0)
                if isinstance(value, BaseException):
                    raise value
                return value

        window = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) + 2 * bounded.NS
        self.assertEqual(bounded.close_own_child(FakeChild([7]), window), (True, 7))
        self.assertEqual(bounded.close_own_child(FakeChild([OSError("wait failed")]), window), (False, None))
        expired = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) - bounded.NS
        self.assertEqual(bounded.close_own_child(FakeChild([None]), expired), (False, None))

    def test_a_parentage_handoff_needs_no_start_time(self):
        message = {"run": "run", "seq": 1, "at": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW),
                   "type": "detach_child", "leader": None, "pid": 4242, "start": None}
        self.assertIsNone(bounded.malformed(message))

    def test_the_receiver_accepts_a_parentage_only_handoff(self):
        # End-to-end through `Custodian.accept`: no start time at all, a row that
        # shows this monitor as parent, so it pins by parentage, signals, observes
        # and acknowledges closure.
        real = bounded.Kinfo.one(os.getpid())
        child_pid = os.getpid() + 1
        message = {"run": "run", "seq": 1, "at": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW),
                   "type": "detach_child", "leader": None, "pid": child_pid, "start": None}
        custodian = self.custodian_stub()

        class FakeKinfo:
            @staticmethod
            def one(pid):
                if pid != child_pid:
                    return None
                return dict(real, pid=child_pid, start=real["start"], ppid=os.getpid(), zombie=False)

        saved = bounded.Kinfo
        try:
            bounded.Kinfo = FakeKinfo
            custodian.accept(dict(message))
        finally:
            bounded.Kinfo = saved
        self.assertEqual(custodian.detach_child, [child_pid, real["start"]])
        self.assertEqual([kind for kind, _f in custodian.records],
                         ["detach_child_pinned", "detach_child_signal"])
        self.assertEqual(custodian.records[0][1]["identity"], "parentage")
        ack = custodian.told[-1]
        self.assertIs(ack[1]["custodian_closes"], True)
        self.assertIs(ack[1]["closed"], True)

    def test_a_positively_absent_child_is_acknowledged_without_signal(self):
        child_pid = os.getpid() + 1
        message = {"run": "run", "seq": 1, "at": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW),
                   "type": "detach_child", "leader": None, "pid": child_pid, "start": None}
        custodian = self.custodian_stub()

        class AbsentKinfo:
            @staticmethod
            def one(pid):
                return None

        saved = bounded.Kinfo
        try:
            bounded.Kinfo = AbsentKinfo
            custodian.accept(dict(message))
        finally:
            bounded.Kinfo = saved
        self.assertIsNone(custodian.detach_child)
        self.assertEqual([kind for kind, _f in custodian.records], ["detach_child_refused"])
        self.assertEqual(custodian.records[0][1]["observation"], "positively absent")
        ack = custodian.told[-1]
        self.assertIs(ack[1]["custodian_closes"], False)
        self.assertIs(ack[1]["closed"], True)
        self.assertIn("positively gone", ack[1]["reason"])

    def test_the_unreadable_injection_is_scoped_to_one_actor(self):
        saved_role, saved_env = bounded._ROLE[0], os.environ.get("OXIDE_SUPERVISOR_INJECT")
        try:
            os.environ["OXIDE_SUPERVISOR_INJECT"] = "detach-identity-unreadable"
            bounded._ROLE[0] = "monitor"
            self.assertTrue(bounded.injected_unreadable())
            bounded._ROLE[0] = "custodian"
            self.assertFalse(bounded.injected_unreadable(),
                             "the monitor's injection also blinded the custodian")
            os.environ["OXIDE_SUPERVISOR_INJECT"] = "detach-custodian-identity-unreadable"
            bounded._ROLE[0] = "monitor"
            self.assertFalse(bounded.injected_unreadable())
            bounded._ROLE[0] = "custodian"
            self.assertTrue(bounded.injected_unreadable())
            os.environ["OXIDE_SUPERVISOR_INJECT"] = ""
            self.assertFalse(bounded.injected_unreadable())
        finally:
            bounded._ROLE[0] = saved_role
            if saved_env is None:
                os.environ.pop("OXIDE_SUPERVISOR_INJECT", None)
            else:
                os.environ["OXIDE_SUPERVISOR_INJECT"] = saved_env

    def test_a_parentage_mismatch_is_acknowledged_without_signalling(self):
        real = bounded.Kinfo.one(os.getpid())
        child_pid = os.getpid() + 1
        message = {"run": "run", "seq": 1, "at": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW),
                   "type": "detach_child", "leader": None, "pid": child_pid, "start": None}
        custodian = self.custodian_stub()

        class FakeKinfo:
            @staticmethod
            def one(pid):
                # Same PID, but reparented: it is no longer this monitor's child.
                return dict(real, pid=child_pid, start=real["start"], ppid=1, zombie=False)

        saved = bounded.Kinfo
        try:
            bounded.Kinfo = FakeKinfo
            custodian.accept(dict(message))
        finally:
            bounded.Kinfo = saved
        self.assertIsNone(custodian.detach_child)
        self.assertEqual([kind for kind, _f in custodian.records], ["detach_child_refused"])
        self.assertEqual(custodian.records[0][1]["observation"], "parentage no longer matches")
        ack = custodian.told[-1]
        self.assertIs(ack[1]["custodian_closes"], False)
        self.assertIs(ack[1]["closed"], True)
        self.assertNotIn("detach_child_signal", [kind for kind, _f in custodian.records])

    def test_an_executor_exception_keeps_the_live_handle(self):
        class BrokenStdout:
            def detach(self):
                raise OSError("the pipe vanished")

            def fileno(self):
                raise OSError("the pipe vanished")

        class FakeChild:
            returncode = None
            stdout = BrokenStdout()
            killed = False

            def poll(self):
                return None

            def kill(self):
                self.killed = True

            def wait(self, timeout=None):
                raise subprocess.TimeoutExpired("child", timeout or 0)

        child = FakeChild()
        result = bounded.bounded_run(["/bin/true"], 2, 64, spawn=lambda *a, **k: child)
        self.assertIs(result["handle"], child, "the live child handle was lost")
        self.assertFalse(result["reaped"])
        self.assertEqual(result["error"], "OSError")
        self.assertTrue(result["reap_timed_out"])
        self.assertIn('self.send("detach_child", pid=int(row["pid"]), start=int(row["start"]))',
                      inspect.getsource(bounded.Monitor.run))

    def test_the_post_state_read_has_no_forced_minimum(self):
        ctx, backing = self.layout()
        self.request(ctx, backing)
        seen = []

        def facts_call(backing_, timeout=None):
            seen.append(timeout)
            return [("/dev/disk7", None, None), ("/dev/disk7s1", "/Volumes/OxDetach", DETACH_UUID)]

        facts = self.facts()
        facts.attachments = facts_call
        record = bounded.post_census_detach(ctx, facts)
        self.assertTrue(record["attempted"])
        # Whatever the value, it is what the window allowed: never above the cap
        # and never a forced one-second floor imposed by the runner.
        self.assertTrue(all(value is None or value <= bounded.DETACH_INFO_TIMEOUT_S for value in seen), seen)

    def test_a_malformed_custody_indication_keeps_the_owned_root(self):
        for payload in (b"null", b"[]", b'{"version": 1}', b'{"version": 1, "backings": []}',
                        b'{"version": 2, "backings": ["/tmp/x"]}',
                        b'{"version": 1, "backings": ["relative"]}'):
            ctx, _backing = self.layout()
            with open(os.path.join(ctx["evidence"], bounded.CUSTODY_NAME), "wb") as handle:
                handle.write(payload)
            record = bounded.cleanup_safety(ctx, self.facts(), facts=lambda *a: [])
            self.assertEqual(record["state"], "kept", (payload, record))
            self.assertIn("custody indication", record["reason"], (payload, record["reason"]))

    def test_a_backing_outside_the_owned_layout_is_refused(self):
        ctx, _backing = self.layout()
        path = os.path.join(ctx["evidence"], bounded.CUSTODY_NAME)
        with open(path, "w") as handle:
            json.dump({"version": 1, "backings": ["/private/etc/passwd"]}, handle)
        record = bounded.cleanup_safety(ctx, self.facts(), facts=lambda *a: [])
        self.assertEqual(record["state"], "kept")
        self.assertIn("outside this run's owned root", record["reason"])

    def test_a_diagnostic_receipt_is_never_qualified(self):
        rows = [{"check": "receipt-phase", "result": "pass", "problems": []},
                {"check": "detach-paths", "result": "pass", "problems": []}]
        receipt = qualify.diagnostic_receipt([], rows)
        self.assertIs(receipt["qualified"], False)
        self.assertEqual(receipt["result"], "diagnostic")
        self.assertEqual((receipt["checks_passed"], receipt["checks_failed"]), (2, 0))
        self.assertIn("never a counted native PASS", receipt["claims"])
        refused = qualify.diagnostic_receipt(["boom"], [])
        self.assertIs(refused["qualified"], False)
        self.assertEqual(refused["problems"], ["boom"])

    def test_an_unmounted_but_still_attached_image_is_not_a_detach(self):
        # The reviewer's case: the mount point can be gone while the image (or one
        # of its slices) is still attached. Any entity of that backing means the
        # attempt did not detach it.
        ctx, backing = self.layout()
        facts = self.facts(
            mount_point=lambda attempted: not attempted,
            attachments=lambda attempted: [("/dev/disk7s1", None, None)] if attempted else
            [("/dev/disk7", None, None), ("/dev/disk7s1", "/Volumes/OxDetach", DETACH_UUID)])
        self.request(ctx, backing)
        record = bounded.post_census_detach(ctx, facts)
        self.assertEqual((record["state"], record["post_state"], record["qualified"]),
                         ("failed", "still_attached", False), record)

    def test_the_apfs_container_and_synthesized_disk_shape(self):
        # A real APFS attach: entities are a container whole disk, a synthesized
        # whole disk and the mounted slice. The request must name the MOUNTED
        # entity; the container's whole-disk name is not that mount's device, and
        # any entity left behind after the attempt is not a detach.
        container = ("/dev/disk10", None, None)
        synthesized = ("/dev/disk11", None, None)
        mounted = ("/dev/disk11s1", "/Volumes/OxDetach", DETACH_UUID)

        def shape(requested):
            def attachments(attempted):
                if attempted:
                    return [] if requested == mounted[0] else [container, synthesized]
                return [container, synthesized, mounted]
            return attachments

        ctx, backing = self.layout()
        mounted_facts = self.facts(attachments=shape(mounted[0]))
        mounted_facts.stats["/dev/disk11s1"] = types.SimpleNamespace(st_rdev=42)
        self.request(ctx, backing, device=mounted[0])
        record = bounded.post_census_detach(ctx, mounted_facts)
        self.assertEqual((record["state"], record["qualified"]), ("detached", True), record)

        # The container's whole-disk name is attached from the backing, but it is
        # not the device the mount is bound to.
        ctx, backing = self.layout()
        container_facts = self.facts(attachments=shape(container[0]))
        self.request(ctx, backing, device=container[0])
        record = bounded.post_census_detach(ctx, container_facts)
        self.assertEqual((record["state"], record["qualified"]), ("refused", False), record)
        self.assertIn("not this image's own device", record["reason"])

        # Naming the mounted device but leaving the container/synthesized entities
        # attached is still not a detach.
        ctx, backing = self.layout()
        leftover = self.facts(attachments=shape("other"))
        leftover.stats["/dev/disk11s1"] = types.SimpleNamespace(st_rdev=42)
        self.request(ctx, backing, device=mounted[0])
        record = bounded.post_census_detach(ctx, leftover)
        self.assertEqual((record["state"], record["post_state"], record["qualified"]),
                         ("failed", "still_attached", False), record)

    def test_the_attachment_parser_fails_closed_on_malformed_facts(self):
        # The parser's own seam: a document either yields every relevant entity
        # exactly, or it is unreadable (None). Never a smaller or empty set.
        backing = "/fixture/volume.sparseimage"

        def attempt(document):
            import plistlib as _plistlib
            payload = _plistlib.dumps(document) if not isinstance(document, bytes) else document
            return {"exit": 0, "timed_out": False, "truncated": False,
                    "output": payload.decode("utf-8", "replace") if isinstance(payload, bytes) else payload}

        # A real `hdiutil info` plist omits `mount-point` on an unmounted entity and
        # cannot carry nulls at all.
        good = {"images": [{"image-path": backing,
                            "system-entities": [{"dev-entry": "/dev/disk10"},
                                                {"dev-entry": "/dev/disk10s1",
                                                 "mount-point": "/Volumes/OxDetach",
                                                 "volume-uuid": DETACH_UUID}]}]}
        self.assertEqual(bounded.image_attachments(backing, run=lambda *a: attempt(good)),
                         [("/dev/disk10", None, None),
                          ("/dev/disk10s1", "/Volumes/OxDetach", DETACH_UUID)])
        self.assertEqual(len(bounded.image_attachments(None, run=lambda *a: attempt(good))), 2)

        # Every malformed shape is unreadable, including the declared backing's
        # mounted entity losing its dev-entry.
        for bad in (
            {"images": "nope"},
            {"not-images": []},
            {"images": ["not an object"]},
            {"images": [{"system-entities": []}]},                                  # no image path
            {"images": [{"image-path": "", "system-entities": []}]},
            {"images": [{"image-path": backing, "system-entities": "nope"}]},
            {"images": [{"image-path": backing, "system-entities": ["nope"]}]},
            {"images": [{"image-path": backing, "system-entities": [{"mount-point": "/Volumes/OxDetach"}]}]},
            {"images": [{"image-path": backing,
                         "system-entities": [{"dev-entry": "", "mount-point": "/Volumes/OxDetach"}]}]},
            {"images": [{"image-path": backing,
                         "system-entities": [{"dev-entry": "/dev/disk10s1", "mount-point": 7}]}]},
            {"images": [{"image-path": backing,
                         "system-entities": [{"dev-entry": "/dev/disk10s1", "volume-uuid": 7}]}]},
            {"images": [{"image-path": backing, "system-entities": []}]},           # matched, no entities
        ):
            self.assertIsNone(bounded.image_attachments(backing, run=lambda *a: attempt(bad)), bad)
            self.assertIsNone(bounded.image_attachments(None, run=lambda *a: attempt(bad)), bad)

        # A malformed UNRELATED image is still unreadable, and an unreadable child
        # or output is too.
        unrelated = {"images": [{"image-path": "/other.sparseimage", "system-entities": "nope"}]}
        self.assertIsNone(bounded.image_attachments(backing, run=lambda *a: attempt(unrelated)))
        for broken in ({"exit": 3, "timed_out": False, "truncated": False, "output": ""},
                       {"exit": 0, "timed_out": True, "truncated": False, "output": ""},
                       {"exit": 0, "timed_out": False, "truncated": True, "output": ""},
                       {"exit": 0, "timed_out": False, "truncated": False, "output": "not a plist"}):
            self.assertIsNone(bounded.image_attachments(backing, run=lambda *a, b=broken: b), broken)

    @contextlib.contextmanager
    def settled_census(self):
        """A certain-empty census stand-in for a scripted decision."""

        class SettledCensus:
            def __init__(self, sid, terminal_allowed=False, ancestors=()):
                self.stats = {}

            def round(self):
                self.round_deadline = bounded.now_ns() + 2 * NS
                return {"certain": True, "members": 0, "uncertain": 0, "member_rows": [],
                        "uncertain_rows": [], "terminal_count_status": "valid",
                        "terminal_nonwriters": 0, "replacement_work": fixture_work()}

        saved = bounded.Census
        bounded.Census = SettledCensus
        try:
            yield
        finally:
            bounded.Census = saved

    def start_census_then(self, monitor, cmd_w, run_id, decision, sent):
        """Write a leader/stopped pair and `decision` once the first census round is
        reported. The decision names that census message, as a real custodian does,
        so the scripted run is as coherent as a real one (oxide#195)."""
        written = []
        real = monitor.send

        def send(message_type, **fields):
            if monitor.stop.is_set():  # the run is over: never write to a closing pipe
                return monitor.seq
            seq = real(message_type, **fields)
            sent.append(message_type)
            if message_type == "census" and not written:
                written.append(True)
                payload = dict(decision)
                payload["census"] = {"seq": seq, "after": 2, "certain": True,
                                     "members": 0, "uncertain": 0, "replacement_work": fixture_work()}
                os.write(cmd_w, (json.dumps(payload) + "\n").encode())
            return seq

        for number, command in enumerate(({"type": "leader", "pid": os.getpid()},
                                          {"type": "stopped", "sent": 0}), start=1):
            os.write(cmd_w, (json.dumps(dict(command, run=run_id, seq=number)) + "\n").encode())
        return send

    def test_an_absent_child_whose_reap_fails_is_not_recorded_as_reaped(self):
        # P2: the absent-child branch wrote reaped/reaped_by as a success even when
        # the reap helper failed. Driven through the REAL `Monitor.run`.
        sandbox = Path(tempfile.mkdtemp(prefix="absent-child.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(sandbox), True)
        evidence = sandbox / "evidence"
        evidence.mkdir(parents=True)
        # A short limit keeps the (correct) hold bounded inside this test.
        ctx = {"run_id": "absent-child", "command": ["/bin/true"], "limit_s": 2,
               "adopted": None, "evidence_arg": str(evidence), "root_parent": str(sandbox),
               "budget": 512 * 1024 * 1024,
               "start_ns": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW), "inject": []}
        ctl_r, ctl_w = os.pipe()
        cmd_r, cmd_w = os.pipe()
        for fd in (ctl_r, cmd_w):
            self.addCleanup(os.close, fd)
        # The monitor's drain threads get immediate EOF (its own read fds).
        monitor = bounded.Monitor(ctx, ctl_w, cmd_r, os.open(os.devnull, os.O_RDONLY),
                                  os.open(os.devnull, os.O_RDONLY))
        sent = []
        monitor.send = self.start_census_then(
            monitor, cmd_w, "absent-child",
            {"type": "cleanup", "outcome": {"closure": "closed", "status": 0}}, sent)

        class Handle:
            pid = 0

            def wait(self, timeout=None):
                return None

        record = {"state": "failed", "qualified": False, "attempted": True, "exit": None,
                  "timed_out": False, "withdrawn": None, "error": None, "post_state": "still_mounted",
                  "reason": "fixture", "backing": None, "mount": None, "device": None,
                  "volume_uuid": None, "timeout_s": None, "budget_ns": None, "output": "",
                  "output_truncated": False, "reap_timed_out": False, "reaped": False,
                  "late_gate": None, "_handle": Handle()}
        saved = (bounded.post_census_detach, bounded.process_row, bounded.reap_own_child,
                 bounded.cleanup_safety)
        bounded.post_census_detach = lambda *a, **k: dict(record)
        bounded.process_row = lambda pid: (None, None)          # positively absent
        bounded.reap_own_child = lambda handle, window_end: (False, None)  # the reap fails
        bounded.cleanup_safety = lambda ctx, **k: None
        try:
            with self.settled_census():
                monitor.run()
        finally:
            (bounded.post_census_detach, bounded.process_row, bounded.reap_own_child,
             bounded.cleanup_safety) = saved
        with open(evidence / "receipt.json") as handle:
            persisted = json.load(handle)
        detach = persisted["detach"]
        self.assertEqual((detach["reaped"], detach["reaped_by"], detach["reap_timed_out"]),
                         (False, None, True), detach)
        self.assertEqual(persisted["phase"], "detach_child_unaccounted: monitor_held",
                         persisted["phase"])
        self.assertNotIn("monitor_complete", sent, sent)

    def test_a_detach_receipt_is_recorded_before_a_custody_keep_returns(self):
        # Ordering, driven through the REAL `Monitor.run`: a run with BOTH a detach
        # request and a custody indication whose second backing is live must record
        # the detach object (and settle its child) before the custody keep returns.
        sandbox = Path(tempfile.mkdtemp(prefix="both-paths.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(sandbox), True)
        evidence = sandbox / "evidence"
        evidence.mkdir(parents=True)
        # The declared backings live in the evidence directory, which the detach
        # request's containment rule accepts alongside the owned root.
        first = str(evidence / "first.sparseimage")
        second = str(evidence / "second.sparseimage")
        for path_ in (first, second):
            with open(path_, "wb") as handle:
                handle.write(b"backing")
        with open(evidence / "detach-request.json", "w") as handle:
            json.dump({"version": 1, "backing": first, "mount": "/Volumes/OxDetach",
                       "device": "/dev/disk7s1", "volume_uuid": DETACH_UUID}, handle)
        with open(evidence / "image-custody.json", "w") as handle:
            json.dump({"version": 1, "backings": [first, second]}, handle)

        def router(argv, *_args, **_kwargs):
            if "info" not in argv:
                router.calls["detach"] = router.calls.get("detach", 0) + 1
                return {"exit": 0, "timed_out": False, "truncated": False, "output": "",
                        "error": None, "withdrawn": None, "reap_timed_out": False,
                        "reaped": True, "handle": None}
            # First read: the requested backing is attached and mounted. Later
            # reads for the requested backing: nothing left (detached). The second
            # declared backing stays mounted throughout.
            target = router.calls.get("info", 0)
            router.calls["info"] = target + 1
            if target == 0:
                images = [{"image-path": first, "system-entities": [
                    {"dev-entry": "/dev/disk7s1", "mount-point": "/Volumes/OxDetach",
                     "volume-uuid": DETACH_UUID}]}]
            else:
                images = [{"image-path": second, "system-entities": [
                    {"dev-entry": "/dev/disk9s1", "mount-point": "/Volumes/OxSecond"}]}]
            payload = plistlib.dumps({"images": images}).decode()
            return {"exit": 0, "timed_out": False, "truncated": False, "output": payload,
                    "reaped": True, "handle": None}

        router.calls = {}
        saved_run, saved_mount = bounded.bounded_run, bounded.DefaultProbe.mount_point
        saved_stat, saved_uuid = bounded.DefaultProbe.stat, bounded.DefaultProbe.volume_uuid
        bounded.bounded_run = router
        # The requested node and volume exist for this fixture.
        bounded.DefaultProbe.stat = lambda self, path: types.SimpleNamespace(st_dev=42, st_rdev=42)
        bounded.DefaultProbe.volume_uuid = lambda self, path: DETACH_UUID
        # The mount is a mount point until the requested image is detached.
        bounded.DefaultProbe.mount_point = (
            lambda self, path: not router.calls.get("detach"))
        ctx = {"run_id": "both-paths", "command": ["/bin/true"], "limit_s": 300,
               "adopted": None, "evidence_arg": str(evidence), "root_parent": str(sandbox),
               "budget": 512 * 1024 * 1024,
               "start_ns": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW), "inject": []}
        ctl_r, ctl_w = os.pipe()
        cmd_r, cmd_w = os.pipe()
        for fd in (ctl_r, cmd_w):
            self.addCleanup(os.close, fd)
        # The monitor's drain threads get immediate EOF (its own read fds).
        monitor = bounded.Monitor(ctx, ctl_w, cmd_r, os.open(os.devnull, os.O_RDONLY),
                                  os.open(os.devnull, os.O_RDONLY))
        sent = []

        base = self.start_census_then(
            monitor, cmd_w, "both-paths",
            {"type": "cleanup", "outcome": {"closure": "closed", "status": 0}}, sent)

        def send(message_type, **fields):
            if message_type == "detach_ready":
                # Answer every authorization request, as the custodian would.
                os.write(cmd_w, (json.dumps({"type": "detach_go", "allowed": True,
                                             "budget_ns": 30 * bounded.NS}) + "\n").encode())
            base(message_type, **fields)

        monitor.send = send
        try:
            with self.settled_census():
                monitor.run()
        finally:
            bounded.bounded_run = saved_run
            bounded.DefaultProbe.mount_point = saved_mount
            bounded.DefaultProbe.stat = saved_stat
            bounded.DefaultProbe.volume_uuid = saved_uuid
        with open(evidence / "receipt.json") as handle:
            persisted = json.load(handle)
        self.assertIn("detach", persisted, "the detach receipt was lost to the custody keep")
        self.assertEqual(persisted["detach"]["qualified"], True, persisted["detach"])
        self.assertEqual(persisted["cleanup"]["state"], "kept", persisted["cleanup"])
        self.assertIn("cleanup_kept", sent, sent)
        self.assertEqual(persisted["phase"], "monitor_complete: pending_custodian", persisted["phase"])

    def test_a_second_declared_backing_is_checked_after_a_successful_detach(self):
        # A proof may declare more than one backing. Detaching the requested image
        # must not skip the custody check for the other one.
        ctx, first = self.layout()
        second = os.path.join(ctx["scratch"], "second.sparseimage")
        with open(second, "wb") as handle:
            handle.write(b"second backing")
        self.declare_custody(ctx, first, second)
        rows = {first: [], second: [("/dev/disk9s1", "/Volumes/OxSecond", None)]}
        record = bounded.cleanup_safety(
            ctx, self.facts(), facts=lambda backing, timeout=None: rows[os.path.realpath(backing)])
        self.assertEqual(record["state"], "kept", record)
        self.assertEqual(sorted(record["unrequested"]), sorted([first, second]))
        self.assertIn("/Volumes/OxSecond", record["reason"])

    def test_the_custody_scan_shares_the_remaining_custodian_window(self):
        # The final custody scan must budget every declared backing inside what is
        # left of the custodian's window, not restart a fresh per-backing allowance.
        ctx, first = self.layout()
        second = os.path.join(ctx["scratch"], "second.sparseimage")
        with open(second, "wb") as handle:
            handle.write(b"second backing")
        self.declare_custody(ctx, first, second)
        # ~0.6 s of window left, two backings.
        ctx["start_ns"] = (time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
                           - 300 * bounded.NS + int(0.6 * bounded.NS)
                           + bounded.EXIT_MARGIN_S * bounded.NS + bounded.DETACH_GATE_MARGIN_NS)
        timeouts = []

        def facts(backing, timeout=None):
            timeouts.append(timeout)
            return []

        record = bounded.cleanup_safety(ctx, self.facts(), facts=facts)
        self.assertEqual(record["state"], "ok", record)
        self.assertEqual(len(timeouts), 2, timeouts)
        # Every grant stays inside the window that was left when it started (the
        # remainder is also shared with the backings still to read), so no read can
        # be given a fresh full allowance.
        self.assertTrue(all(value is not None and value < 0.7 for value in timeouts), timeouts)
        self.assertLessEqual(timeouts[0], 0.35, timeouts)

    def test_the_deadline_is_rechecked_before_every_backing_read(self):
        # A first read that consumes the window must not buy a later backing a
        # fresh allowance: the deadline is rechecked before each read.
        ctx, first = self.layout()
        second = os.path.join(ctx["scratch"], "second.sparseimage")
        with open(second, "wb") as handle:
            handle.write(b"second backing")
        self.declare_custody(ctx, first, second)
        # ~0.3 s of window left; the first read consumes it.
        ctx["start_ns"] = (time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
                           - 300 * bounded.NS + int(0.3 * bounded.NS)
                           + bounded.EXIT_MARGIN_S * bounded.NS + bounded.DETACH_GATE_MARGIN_NS)
        reads = []

        def facts(backing, timeout=None):
            reads.append((backing, timeout))
            time.sleep(0.35)  # this read uses up the window
            return []

        record = bounded.cleanup_safety(ctx, self.facts(), facts=facts)
        self.assertEqual(record["state"], "kept", record)
        self.assertEqual(len(reads), 1, reads)
        self.assertLessEqual(reads[0][1], 0.35, reads)
        self.assertIn("no custodian wait window is left", record["reason"])

    def test_a_sub_50ms_remainder_is_not_rounded_up(self):
        # No floor may exceed a smaller positive remainder. The clock is fixed so
        # the boundary is deterministic: 30 ms shared across two backings grants at
        # most 15 ms each, never a 50 ms floor.
        ctx, first = self.layout()
        second = os.path.join(ctx["scratch"], "second.sparseimage")
        with open(second, "wb") as handle:
            handle.write(b"second backing")
        grants = []

        def facts(backing, timeout=None):
            grants.append(timeout)
            return []

        fixed = 1_000 * bounded.NS
        saved_now = bounded.now_ns
        bounded.now_ns = lambda: fixed
        try:
            problems = bounded.custody_problems(
                [first, second], self.facts(), facts, timeout=bounded.DETACH_INFO_TIMEOUT_S,
                deadline_ns=fixed + 30_000_000)
        finally:
            bounded.now_ns = saved_now
        self.assertEqual(problems, [], problems)
        self.assertEqual(len(grants), 2, grants)
        # Each grant is bounded by the 30 ms remainder (the first is shared down to
        # half), and none is widened to the old 50 ms floor.
        self.assertTrue(all(value is not None and value <= 0.03 for value in grants), grants)
        self.assertEqual(grants[0], 0.015, grants)
        self.assertTrue(all(value < 0.05 for value in grants), grants)

    def test_an_exhausted_window_scans_nothing(self):
        ctx, backing = self.layout()
        self.declare_custody(ctx, backing)
        ctx["start_ns"] = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) - 300 * bounded.NS
        calls = []
        record = bounded.cleanup_safety(
            ctx, self.facts(), facts=lambda backing_, timeout=None: calls.append(timeout) or [])
        self.assertEqual(record["state"], "kept", record)
        self.assertIn("no custodian wait window", record["reason"])
        self.assertEqual(calls, [], "a spent window still read the custody indication")

    def test_an_unreaped_attachment_read_is_kept_as_a_child(self):
        backing = "/fixture/volume.sparseimage"

        class FakeChild:
            pid = 0

            def __init__(self, statuses):
                self.statuses = list(statuses)

            def wait(self, timeout=None):
                if not self.statuses:
                    return None
                value = self.statuses.pop(0)
                if isinstance(value, BaseException):
                    raise value
                return value

        handle = FakeChild([4242])
        bounded.INFO_ORPHANS.clear()
        try:
            self.assertIsNone(bounded.image_attachments(backing, run=lambda *a: {
                "exit": 0, "timed_out": False, "truncated": False, "output": "",
                "reaped": False, "handle": handle}))
            self.assertEqual(bounded.INFO_ORPHANS, [handle], "an unreaped read child was dropped")
        finally:
            bounded.INFO_ORPHANS.clear()

    def monitor_stub(self):
        monitor = bounded.Monitor.__new__(bounded.Monitor)
        cmd_r, cmd_w = os.pipe()
        self.addCleanup(os.close, cmd_r)
        self.addCleanup(os.close, cmd_w)
        monitor.cmd_r = cmd_r
        monitor.lock = threading.Lock()
        monitor.receipt = {"custodian": []}
        monitor.published = []
        monitor.publish = lambda phase: monitor.published.append(
            (phase, [r["event"] for r in monitor.receipt["custodian"]]))
        monitor.summary = lambda: None
        monitor.send = lambda *a, **k: monitor.published.append(("sent", None))
        monitor.stop = threading.Event()
        return monitor, cmd_w

    def test_an_orphan_read_is_closed_before_completion_or_the_monitor_holds(self):
        class FakeChild:
            pid = 0

            def __init__(self, reaped):
                self.reaped = reaped

            def wait(self, timeout=None):
                return 7 if self.reaped else None

        ctx = {"start_ns": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW), "limit_s": 300}
        monitor, _cmd_w = self.monitor_stub()
        bounded.INFO_ORPHANS.clear()
        try:
            bounded.INFO_ORPHANS.append(FakeChild(True))
            monitor.finish_monitor(ctx, "monitor_complete: pending_custodian", drain=False)
            self.assertEqual(bounded.INFO_ORPHANS, [], "a closed orphan was kept")
            self.assertEqual([phase for phase, _r in monitor.published][0],
                             "monitor_complete: pending_custodian")
            self.assertTrue(monitor.stop.is_set())

            # Unreaped: the monitor must not claim completion.
            monitor, _cmd_w = self.monitor_stub()
            ctx_late = {"start_ns": time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) - 300 * bounded.NS,
                        "limit_s": 300}
            bounded.INFO_ORPHANS.append(FakeChild(False))
            monitor.finish_monitor(ctx_late, "monitor_complete: pending_custodian", drain=False)
            self.assertEqual([phase for phase, _r in monitor.published],
                             ["orphan_read_unaccounted: monitor_held"], monitor.published)
            self.assertFalse(monitor.stop.is_set())
        finally:
            bounded.INFO_ORPHANS.clear()

    def test_a_malformed_attachment_keeps_the_request_free_root(self):
        # Parser to the opt-in custody check: the declared backing's mounted entity
        # loses its dev-entry, so nothing about it is provably clear.
        ctx, backing = self.layout()
        self.declare_custody(ctx, backing)
        document = {"images": [{"image-path": backing,
                                "system-entities": [{"mount-point": "/Volumes/OxDetach"}]}]}
        payload = plistlib.dumps(document).decode()

        def fake_run(*_args, **_kwargs):
            return {"exit": 0, "timed_out": False, "truncated": False, "output": payload}

        saved = bounded.bounded_run
        try:
            bounded.bounded_run = fake_run
            record = bounded.cleanup_safety(ctx, self.facts())
        finally:
            bounded.bounded_run = saved
        self.assertEqual(record["state"], "kept", record)
        self.assertIn("unreadable", record["reason"])
        # And with no custody indication the default path still performs no scan.
        ctx, _backing = self.layout()
        try:
            bounded.bounded_run = fake_run
            self.assertIsNone(bounded.cleanup_safety(ctx, self.facts()))
        finally:
            bounded.bounded_run = saved

    def test_a_malformed_attachment_is_an_unknown_detach_post_state(self):
        # Parser to the detach POST-STATE only: the pre-attempt binding is valid,
        # so the one attempt really begins, and then the post-state read is
        # malformed. Unreadable facts can never read as "detached", whatever the
        # exit code says, and the backing is retained.
        ctx, backing = self.layout()
        self.request(ctx, backing)
        good = {"images": [{"image-path": backing,
                            "system-entities": [{"dev-entry": "/dev/disk7"},
                                                {"dev-entry": "/dev/disk7s1",
                                                 "mount-point": "/Volumes/OxDetach",
                                                 "volume-uuid": DETACH_UUID}]}]}
        # Genuinely malformed: a non-string mount point is not a readable fact.
        malformed = {"images": [{"image-path": backing,
                                 "system-entities": [{"dev-entry": "/dev/disk7s1", "mount-point": 7}]}]}
        calls = []

        def fake_run(*_args, **_kwargs):
            calls.append(len(calls))
            payload = plistlib.dumps(good if len(calls) == 1 else malformed).decode()
            return {"exit": 0, "timed_out": False, "truncated": False, "output": payload}

        facts = self.facts(mount_point=lambda attempted: not attempted)
        # Use the REAL parser for both reads (binding and post-state).
        facts.attachments = lambda target, timeout=None: bounded.image_attachments(target, timeout)
        facts.stats["/dev/disk7s1"] = types.SimpleNamespace(st_rdev=42)
        saved = bounded.bounded_run
        try:
            bounded.bounded_run = fake_run
            record = bounded.post_census_detach(ctx, facts)
        finally:
            bounded.bounded_run = saved
        # The request bound and exactly one attempt really ran…
        self.assertTrue(record["attempted"], record)
        self.assertEqual(len(facts.detached), 1, facts.detached)
        self.assertGreaterEqual(len(calls), 2, "the post-state read did not happen")
        # …and its post-state is unknown, not detached, with the backing retained.
        self.assertEqual((record["post_state"], record["qualified"]), ("unknown", False), record)
        self.assertEqual(record["state"], "failed", record)
        self.assertTrue(os.path.isfile(backing), "the backing was removed")

    def test_the_fixture_child_imports_every_module_it_uses(self):
        # The fixture body is a fresh python child: a module it reaches through a
        # dotted name must be imported there, or it dies with NameError before any
        # mount exists. This parses the body, proves every such root is imported,
        # and executes the import line so the binding is real.
        body = DETACH_CASE
        tree = ast.parse(body)
        imported, assigned = set(), set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                imported.update(alias.name.split(".")[0] for alias in node.names)
            elif isinstance(node, ast.ImportFrom):
                imported.add((node.module or "").split(".")[0])
            elif isinstance(node, ast.Name) and isinstance(node.ctx, ast.Store):
                assigned.add(node.id)
            elif isinstance(node, (ast.FunctionDef, ast.ClassDef)):
                assigned.add(node.name)
        roots = {node.value.id for node in ast.walk(tree)
                 if isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name)} - assigned
        missing = sorted(roots - imported)
        self.assertEqual(missing, [], f"the fixture uses unimported modules: {missing}")
        self.assertIn("re", imported)
        # Execute every module-level import the child runs (the prologue's and the
        # fixture's own) and prove the modules it reaches are really bound.
        imports = [node for node in tree.body if isinstance(node, ast.Import)]
        self.assertTrue(imports)
        namespace = {}
        for node in imports:
            exec(compile(ast.Module(body=[node], type_ignores=[]), "<fixture-imports>", "exec"),
                 namespace)
        for name in sorted(roots):
            self.assertIn(name, namespace, f"{name} is used but not bound by an import")
        self.assertTrue(callable(namespace["re"].fullmatch), "the import line does not bind re")

    def test_the_fixture_accepts_the_two_disk_attachment(self):
        # The qualification fixture must accept an image with more than one
        # whole-disk entity and name the mounted one.
        source = inspect.getsource(bounded)  # keep the import used
        fixture_source = DETACH_CASE
        self.assertIn("mounted = [entity for entity in entities", fixture_source)
        self.assertIn('device = mounted[0]["dev-entry"]', fixture_source)
        self.assertNotIn("if len(wholes) != 1", fixture_source)
        self.assertNotIn('"device": wholes[0]', fixture_source)

    def test_the_child_runner_polls_and_stops_on_withdrawal(self):
        # A fake process over a real pipe: no child, so this stays pure. While the
        # child still runs, a withdrawn authority must stop it; an allowed one must
        # not. `bounded_run` owns and closes the read end.

        class FakeChild:
            returncode = None

            class _Raw:
                def __init__(self, fd):
                    self.fd = fd
                    self.closed = False

                def fileno(self):
                    return self.fd

                def close(self):
                    self.closed = True

            class _Stdout:
                def __init__(self, fd):
                    self.raw = FakeChild._Raw(fd)

                def detach(self):
                    return self.raw

                def fileno(self):
                    return self.raw.fd

            def __init__(self, fd):
                self.stdout = self._Stdout(fd)
                self.killed = False

            def poll(self):
                return self.returncode

            def kill(self):
                self.killed = True
                self.returncode = -9

            def wait(self, timeout=None):
                return self.returncode

        def case(still_allowed, exit_code):
            read_fd, write_fd = os.pipe()
            child = FakeChild(read_fd)

            def spawn(argv, **kwargs):
                os.write(write_fd, b"partial output")
                if exit_code is not None:  # a child that finished: EOF, no polling
                    child.returncode = exit_code
                    os.close(write_fd)
                return child

            result = bounded.bounded_run(["/bin/true"], 5, 64, still_allowed=still_allowed, spawn=spawn)
            if exit_code is None:  # still "running" when it was stopped: close our end
                os.close(write_fd)
            return child, result

        asked = []

        def withdrawn():
            asked.append(1)
            return False, "a fault is latched mid-attempt"

        child, result = case(withdrawn, None)
        self.assertTrue(asked, "authority was never re-checked while the child ran")
        self.assertTrue(child.killed, "withdrawn authority did not kill the running child")
        self.assertEqual(result["withdrawn"], "a fault is latched mid-attempt")
        self.assertFalse(result["timed_out"])

        child, result = case(lambda: (True, None), 0)
        self.assertFalse(child.killed, "an allowed, finished child was killed")
        self.assertIsNone(result["withdrawn"])
        self.assertEqual(result["exit"], 0)

    def test_success_requires_a_zero_exit_and_a_detached_post_state(self):
        for attempt, mount_point in (({"exit": None, "timed_out": False, "error": None, "output": ""}, False),
                                     ({"exit": 0, "timed_out": False, "error": "ENOENT", "output": ""}, False)):
            ctx, backing = self.layout()
            self.request(ctx, backing)
            record = bounded.post_census_detach(ctx, self.facts(attempt=attempt, mount_point=mount_point))
            self.assertFalse(record["qualified"], record)
            self.assertNotEqual(record["state"], "detached", record)

    def test_the_marker_records_a_failed_detach_as_unconcluded(self):
        self.assertIn("detach", bounded.MARKER_REASONS)
        self.assertNotIn("detach", bounded.MARKER_EXPIRABLE_REASONS)
        sandbox = Path(tempfile.mkdtemp(prefix="detach-marker.", dir=str(SCRATCH))).resolve()
        self.addCleanup(shutil.rmtree, str(sandbox), True)
        root = os.path.join(str(sandbox), "oxide-proof.bbb")
        os.mkdir(root, 0o700)
        document = bounded.marker_document(bounded.new_run_id(), root, FixtureProbe())
        bounded.write_marker(root, document, FixtureProbe())
        updated = bounded.record_conclusion(root, document["run_id"], FixtureProbe(), "failed", "none",
                                            "detach")
        self.assertEqual((updated["outcome"], updated["closure"], updated["kept_reason"]),
                         ("failed", "none", "detach"))
        self.assertIsNone(updated["concluded_unix"])


class PureSettlement(unittest.TestCase):
    """Oxide#147: the settlement diagnostics the real monitor and custodian
    write, and the oracles that must reject a missing, reordered, duplicated,
    over-cap or frozen assertion instead of mirroring the constants."""

    def monitor(self, leader=(4242, 77)):
        ctl_r, ctl_w = os.pipe()
        cmd_r, cmd_w = os.pipe()
        out_r, out_w = os.pipe()
        err_r, err_w = os.pipe()
        for fd in (ctl_r, cmd_r, out_r, err_r, ctl_w, cmd_w, out_w, err_w):
            self.addCleanup(os.close, fd)
        monitor = bounded.Monitor({"run_id": "r", "inject": [],
                                   "terminal_mode": {"mode": "strict"}},
                                  ctl_w, cmd_r, out_r, err_r)
        # The receipt setup() builds holds this same object, so a publish
        # serializes the diagnostics without a second write path.
        monitor.receipt = {"census": None, "census_rounds": 0, "settlement": monitor.settlement}
        monitor.leader = None if leader is None else list(leader)
        monitor.census_stop = OneShotStop()
        return monitor, ctl_r

    def run_loop(self, monitor, after=7):
        saved = bounded.Census
        bounded.Census = FakeCensus
        try:
            monitor.census_loop(after)
        finally:
            bounded.Census = saved

    def custodian(self):
        read_end, cmd_w = os.pipe()
        self.addCleanup(os.close, read_end)
        self.addCleanup(os.close, cmd_w)
        custodian = bounded.Custodian(["true"])
        custodian.run_id = "r"
        custodian.cmd_w = cmd_w
        custodian.guard = bounded.Guard(0)
        custodian.leader = 4242
        custodian.leader_status = 0
        custodian.reaped_seq = 3
        custodian.exit_at = 1  # the grace window is already past: a bounded decision
        return custodian, read_end

    def records(self, custodian, read_end):
        blob = os.read(read_end, 1 << 20).decode()
        out = []
        for line in blob.splitlines():
            message = json.loads(line)
            if message.get("type") == "record":
                out.append(message["record"])
        return out

    def message(self, custodian, kind, **fields):
        if kind == "census":
            fields.setdefault("replacement_work", fixture_work())
        return {"run": custodian.run_id, "seq": custodian.monitor_seq + 1, "leader": [4242, 77],
                "type": kind, "at": 1, **fields}

    # The real census thread and receipt path.

    def test_synthetic_replacement_uses_real_census_and_message_path(self):
        for mode, expected in (("census-replacement-member", "member"),
                               ("census-replacement-foreign", "foreign")):
            with self.subTest(mode=mode):
                monitor, ctl_r = self.monitor()
                monitor.inject = {mode}
                with patched(bounded.Kinfo, "all", lambda: []), patched(bounded.Kinfo, "one", lambda _pid: None):
                    monitor.census_loop(after=7)
                evidence = monitor.receipt["replacement_fixture"]
                self.assertEqual((evidence["source"], evidence["mode"]), ("synthetic", mode))
                self.assertIn({"pid": 2_147_483_647, "start": 2, "state": expected}, evidence["work"]["rows"])
                message = json.loads(os.read(ctl_r, 1 << 16).decode())
                self.assertEqual(message["replacement_work"], evidence["work"])
                self.assertIsNone(bounded.malformed(message))

    def test_a_late_census_message_invalidates_final_round(self):
        monitor, ctl_r = self.monitor()
        ticks = [bounded.now_ns()]
        original = monitor.send
        def late_send(kind, **fields):
            seq = original(kind, **fields)
            if kind == "census" and fields["certain"]:
                ticks[0] += 3 * NS
            return seq
        monitor.send = late_send
        with patched(bounded, "Census", FakeCensus), patched(bounded, "now_ns", lambda: ticks[0]):
            monitor.census_loop(after=7)
        messages = [json.loads(line) for line in os.read(ctl_r, 1 << 16).decode().splitlines()]
        self.assertEqual([m["certain"] for m in messages], [True, False])
        self.assertEqual(messages[-1]["replacement_work"]["status"], "deadline")
        self.assertFalse(monitor.census_last["certain"])
        self.assertEqual(monitor.census_last["terminal_count_status"], "invalid")
        self.assertFalse(bounded.census_settled({"joined": True}, monitor.census_last,
                                                monitor.census_messages[messages[0]["seq"]])[0])

    def test_census_loop_records_the_real_round_and_keeps_the_message(self):
        monitor, ctl_r = self.monitor()
        monitor.note_stopped({"seq": 7, "sent": 1000 * NS}, 1000 * NS + 250_000)
        self.run_loop(monitor)
        record = monitor.settlement
        self.assertEqual(settlement_problems(record), [])
        self.assertEqual((record["rounds_started"], record["rounds_completed"]), (1, 1))
        self.assertEqual((record["phase"], record["stopped_seq"], record["leader_bound"]),
                         ("loop-exit", 7, True))
        self.assertEqual(record["receive_latency_ns"], 250_000)
        entry = record["rounds"][0]
        self.assertEqual((entry["rows"], entry["confirm_reads"], entry["pause_calls"],
                          entry["snapshot_ns"], entry["scan_ns"]), (3, 2, 1, 5, 7))
        self.assertEqual((entry["certain"], entry["members"], entry["uncertain"]), (True, 0, 0))
        self.assertEqual(entry["phase"], "complete")
        self.assertIs(monitor.receipt["settlement"], record)
        self.assertEqual(monitor.receipt["census_rounds"], 1)
        blob = os.read(ctl_r, 1 << 20).decode().splitlines()
        self.assertEqual(len(blob), 1)
        message = json.loads(blob[0])
        self.assertEqual(set(message), {"run", "leader", "seq", "at", "type", "after", "members",
                                        "uncertain", "certain", "replacement_work"})
        self.assertEqual((message["after"], message["certain"]), (7, True))

    def test_missing_completion_is_distinguishable_from_an_empty_census(self):
        monitor, _ctl_r = self.monitor()
        monitor.note_stopped({"seq": 9, "sent": 100}, 200)
        monitor.census_stop = OneShotStop(stopped=True)  # the loop exits before a round
        self.run_loop(monitor, after=9)
        stalled = monitor.settlement
        self.assertEqual((stalled["rounds_started"], stalled["rounds_completed"]), (0, 0))
        self.assertIsNone(stalled["first_round_ns"])
        self.assertIsNotNone(stalled["thread_start_ns"])
        self.assertIsNone(monitor.receipt["census"])
        self.assertEqual(settlement_problems(stalled), [])
        monitor, _ctl_r = self.monitor()
        monitor.note_stopped({"seq": 9, "sent": 100}, 200)
        self.run_loop(monitor, after=9)
        finished = monitor.settlement
        self.assertEqual((finished["rounds_started"], finished["rounds_completed"]), (1, 1))
        self.assertEqual(monitor.receipt["census"]["certain"], True)
        self.assertEqual(monitor.receipt["census"]["members"], 0)
        self.assertNotEqual(stalled["rounds_completed"], finished["rounds_completed"])

    def test_a_frozen_clock_is_rejected_not_mirrored(self):
        monitor, _ctl_r = self.monitor()
        saved = bounded.now_ns
        bounded.now_ns = lambda: 1000 * NS
        try:
            monitor.note_stopped({"seq": 7, "sent": 1000 * NS}, 1000 * NS)
            self.run_loop(monitor)
        finally:
            bounded.now_ns = saved
        problems = settlement_problems(monitor.settlement)
        self.assertTrue(problems)
        self.assertTrue(any("frozen clock" in problem for problem in problems), problems)

    def test_reordered_or_duplicated_stamps_are_rejected(self):
        monitor, _ctl_r = self.monitor()
        monitor.note_stopped({"seq": 7, "sent": 1000 * NS}, 1000 * NS + 250_000)
        self.run_loop(monitor)
        self.assertEqual(settlement_problems(monitor.settlement), [])
        entry = monitor.settlement["rounds"][0]
        good = dict(entry)
        for label, mutation, needle in (
                ("sent before the lock", {"sent_ns": good["lock_wait_ns"] - 1}, "non-decreasing"),
                ("complete without a stamp", {"end_ns": None}, "missing stamp"),
                ("duplicate under a new name", {"end_ns_copy": good["end_ns"]}, "schema"),
                ("unknown phase", {"phase": "finished"}, "unknown phase"),
                ("scanned before it started", {"scanned_ns": good["start_ns"] - 1}, "scanned"),
                ("bool as a count", {"rows": True}, "ill-typed count"),
                ("wrong index", {"index": 9}, "wrong index")):
            with self.subTest(label):
                entry.clear()
                entry.update(good)
                entry.update(mutation)
                problems = settlement_problems(monitor.settlement)
                self.assertTrue(any(needle in problem for problem in problems), problems)

    def test_record_level_mutations_are_rejected(self):
        monitor, _ctl_r = self.monitor()
        monitor.note_stopped({"seq": 7, "sent": 1000 * NS}, 1000 * NS + 250_000)
        self.run_loop(monitor)
        record = monitor.settlement
        self.assertEqual(settlement_problems(record), [])
        saved = dict(record)
        for label, mutation, needle in (
                ("extra field", {"settled_ns": 1}, "schema keys"),
                ("unknown clock", {"clock": "mach_absolute"}, "unknown schema"),
                ("unknown phase", {"phase": "idle"}, "unknown phase"),
                ("completed without a record", {"rounds_completed": 2}, "rounds_completed"),
                ("started without kept or omitted", {"rounds_started": 3}, "does not equal"),
                ("receive before the send", {"stopped_received_ns": 1000 * NS - 1}, "out of order"),
                ("string stamp", {"thread_start_ns": "now"}, "ill-typed"),
                ("bool stamp", {"first_round_ns": True}, "ill-typed")):
            with self.subTest(label):
                record.clear()
                record.update(saved)
                record.update(mutation)
                problems = settlement_problems(record)
                self.assertTrue(any(needle in problem for problem in problems), problems)

    def test_round_records_are_capped_and_the_remainder_counted(self):
        monitor, _ctl_r = self.monitor()
        entries = [monitor.note_round(index * NS) for index in range(bounded.SETTLE_MAX_ROUNDS + 5)]
        record = monitor.settlement
        self.assertEqual(len(record["rounds"]), bounded.SETTLE_MAX_ROUNDS)
        self.assertEqual((record["omitted"], record["rounds_started"]),
                         (5, bounded.SETTLE_MAX_ROUNDS + 5))
        self.assertIsNone(entries[-1])
        monitor.note(entries[-1], phase="complete", sent_ns=1)  # a capped round takes nothing
        self.assertEqual(record["rounds"][-1]["phase"], "start")
        self.assertEqual(settlement_problems(record), [])

    def test_worst_case_record_stays_under_the_byte_ceiling(self):
        monitor, _ctl_r = self.monitor()
        base = NS
        monitor.note(stopped_sent_ns=base - 3, stopped_received_ns=base - 2,
                     thread_start_ns=base - 1, census_built_ns=base)
        for index in range(bounded.SETTLE_MAX_ROUNDS):
            start = base + index * 10
            entry = monitor.note_round(start)
            monitor.note(entry, phase="complete", scanned_ns=start + 1, snapshot_ns=bounded.U64MAX,
                         scan_ns=bounded.U64MAX, rows=bounded.U64MAX, esrch_rows=bounded.U64MAX,
                         zombie_rows=bounded.U64MAX, confirm_reads=bounded.U64MAX,
                         pause_calls=bounded.U64MAX, lock_wait_ns=start + 2,
                         lock_acquired_ns=start + 3, lock_released_ns=start + 4,
                         sent_ns=start + 5, end_ns=bounded.U64MAX, certain=True,
                         members=bounded.U64MAX, uncertain=bounded.U64MAX)
        monitor.note(rounds_completed=bounded.SETTLE_MAX_ROUNDS, phase="loop-exit")
        record = monitor.settlement
        blob = json.dumps(record, ensure_ascii=True, separators=(",", ":")).encode("utf-8")
        self.assertLessEqual(len(blob), bounded.SETTLE_MAX_BYTES)
        self.assertEqual(settlement_problems(record), [])
        record["rounds"][0]["rows"] = bounded.U64MAX + 1  # over the fixed integer width
        self.assertTrue(any("ill-typed count" in problem for problem in settlement_problems(record)))

    def test_missing_or_ill_typed_custodian_clock_is_unknown(self):
        monitor, _ctl_r = self.monitor()
        for command, expected in (
                ({"seq": 5}, None), ({"seq": 5, "sent": "1000"}, None),
                ({"seq": 5, "sent": True}, None), ({"seq": 5, "sent": -1}, None),
                ({"seq": 5, "sent": 2000}, None),  # later than the receive: never a latency
                ({"seq": 5, "sent": 1000}, 500)):
            with self.subTest(command=command):
                monitor.settlement.clear()
                monitor.settlement.update(bounded.settlement_record())
                monitor.note_stopped(command, 1500)
                self.assertEqual(monitor.settlement["receive_latency_ns"], expected)
                self.assertEqual(monitor.settlement["stopped_received_ns"], 1500)
                self.assertEqual(monitor.settlement["stopped_seq"], 5)
                self.assertEqual(monitor.settlement["phase"], "received")
        monitor.settlement.clear()
        monitor.settlement.update(bounded.settlement_record())
        monitor.note_stopped(["not", "an", "object"], 1500)  # never raises
        self.assertIsNone(monitor.settlement["receive_latency_ns"])

    # The real census round's observation-only counters.

    def test_census_counters_follow_the_real_reads_and_leave_classification_alone(self):
        table = FakeTable([row(108, zombie=True)], {108: ProcessLookupError(3, "gone")},
                          reread={108: lambda reads: row(108, zombie=True)})
        check = census(table)
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (False, 0, 1))
        self.assertNotIn("stats", result)
        stats = check.stats
        self.assertEqual((stats["rows"], stats["zombie_rows"], stats["esrch_rows"]), (1, 1, 1))
        self.assertEqual((stats["confirm_reads"], stats["pause_calls"]), (bounded.CONFIRM_TRIES,
                                                                          bounded.CONFIRM_TRIES - 1))
        self.assertGreaterEqual(stats["scan_ns"], 0)
        self.assertGreaterEqual(stats["snapshot_ns"], 0)
        table.rows, table.sessions, table.rereads = [], {}, {}
        check.round()
        self.assertEqual((check.stats["rows"], check.stats["confirm_reads"], check.stats["pause_calls"]),
                         (0, 0, 0))  # counters reset per round

    def test_failed_snapshot_is_recorded_without_changing_the_result(self):
        def broken():
            raise OSError(12, "injected")

        check = bounded.Census(100, snapshot=broken)
        result = check.round()
        self.assertFalse(result["certain"])
        self.assertTrue(check.stats["snapshot_error"])
        self.assertEqual(check.stats["rows"], 0)
        self.assertNotIn("stats", result)

    # The real custodian decision-time snapshot.

    def test_custodian_snapshot_names_the_decision_time_facts(self):
        custodian, read_end = self.custodian()
        for kind, fields in (("output_eof", {"stream": "stdout"}), ("output_eof", {"stream": "stderr"}),
                             ("census", {"after": 2, "members": 4, "uncertain": 0, "certain": True}),
                             ("census", {"after": 3, "members": 0, "uncertain": 0, "certain": True})):
            custodian.accept(self.message(custodian, kind, **fields))
        self.assertTrue(custodian.census_ok)
        custodian.settle(True)
        settlement = [record for record in self.records(custodian, read_end)
                      if record["event"] == "settlement"]
        self.assertEqual(len(settlement), 1)
        snapshot = settlement[0]
        self.assertEqual((snapshot["census_messages"], snapshot["census_eligible"]), (2, 1))
        self.assertTrue(snapshot["census_ok"])
        self.assertFalse(snapshot["census_uncertain"])
        self.assertIsNone(snapshot["fault"])
        self.assertIsNotNone(snapshot["stopped_sent_ns"])
        self.assertEqual(snapshot["census_end_ns"], 1)  # bounded by exit_at, not a new clock
        self.assertEqual(snapshot["wait_ns"], 0)
        # Only the post-reap message is timed; the pre-reap one is counted but
        # never mistaken for settlement evidence.
        self.assertEqual(snapshot["first_census_ns"], snapshot["last_census_ns"])
        self.assertEqual(snapshot["eof"], ["stderr", "stdout"])

    def test_custodian_snapshot_records_an_absent_census_as_zero_messages(self):
        custodian, read_end = self.custodian()
        custodian.accept(self.message(custodian, "output_eof", stream="stdout"))
        custodian.accept(self.message(custodian, "output_eof", stream="stderr"))
        custodian.settle(True)
        snapshot = [record for record in self.records(custodian, read_end)
                    if record["event"] == "settlement"][0]
        self.assertEqual(snapshot["census_messages"], 0)
        self.assertIsNone(snapshot["first_census_ns"])
        self.assertIsNone(snapshot["last_census_ns"])
        self.assertFalse(snapshot["census_ok"])
        self.assertEqual(snapshot["fault"], CENSUS_FAULT)

    def test_a_failed_observation_write_can_never_latch_or_reopen_a_closure(self):
        custodian, _read_end = self.custodian()
        fd = os.open(os.devnull, os.O_WRONLY)
        os.close(fd)
        custodian.cmd_w = fd  # every write fails with EBADF
        custodian.census_ok, custodian.eof = True, {"stdout", "stderr"}
        custodian.observe_settlement(True, None)
        self.assertIsNone(custodian.guard.fault)
        self.assertTrue(custodian.closure(True))
        custodian.observe_settlement(True, custodian.exit_at)
        self.assertIsNone(custodian.guard.fault)


# Oxide#147 correction: serial reference scheduling at 8849f99 ----------------
#
# Production now confirms ESRCH identities in shared passes (one CONFIRM_PAUSE
# per pass, one reread per pending identity per pass). The reference below is a
# verbatim copy of `Census.confirm_gone_info` and `Census.round` from
# scripts/bounded-test.py at 8849f99, before that correction: one identity at a
# time with its own pause before every further confirmation read. Production
# never selects this class; only the differential cases instantiate it, to show
# the corrected scheduling returns exactly the same round data and still grants
# every identity exactly its existing observations and spacing.
#
# The module names the copied bodies use are bound here once, so a mutation of
# a production constant or helper cannot silently move the reference with it.

CONFIRM_TRIES = bounded.CONFIRM_TRIES
CONFIRM_PAUSE = bounded.CONFIRM_PAUSE
U64MAX = bounded.U64MAX
TERMINAL_STATUS_VALUES = bounded.TERMINAL_STATUS_VALUES
DIAG_MAX_BYTES = bounded.DIAG_MAX_BYTES
DIAG_MAX_ROWS = bounded.DIAG_MAX_ROWS
_diag_row = bounded._diag_row
_diag_classify = bounded._diag_classify
_terminal_fields = bounded._terminal_fields
_same_complete_zombie = bounded._same_complete_zombie
_terminal_witness_row = bounded._terminal_witness_row
_fit_witnesses = bounded._fit_witnesses
_ERRNO = bounded._ERRNO
_diag_object = bounded._diag_object


class SerialCensus(bounded.Census):
    """Test-only serial reference: the scheduling in use at 8849f99.

    It binds the shared terminal predicate itself, so a counterfactual that
    patches the production method cannot silently move the reference too."""

    terminal_eligible = bounded.Census.terminal_eligible

    def confirm_gone_info(self, row):
        """Confirm absence, reporting how. Returns (gone, tries, outcome,
        error, first_row, first_error, all_clean).

        `gone` follows the existing Gone rule (a successful read lacking the
        identity); `tries` is the bounded 1..20 re-read count; `outcome` is
        one of absent/replaced/matching_exhausted/read_failed; `error` is
        the reread exception or None. `first_row`/`first_error` carry the
        FIRST existing confirmation read (a dict, None when absent, or the
        exception). `all_clean` reports whether every actually performed
        non-terminal read showed the same complete SZOMB identity (a later
        Gone still takes priority and ends the confirmation). The terminal
        rule reuses all of this with no added reads. Same reads, pauses
        and budget as confirm_gone; no early short-circuit."""
        tries, first_row, first_error, all_clean = 0, "unset", None, True
        for attempt in range(CONFIRM_TRIES):
            if attempt:
                self.pause(CONFIRM_PAUSE)
                try:
                    self.stats["pause_calls"] += 1  # observation only
                except BaseException:
                    pass
            tries += 1
            try:
                self.stats["confirm_reads"] += 1  # observation only
            except BaseException:
                pass
            try:
                again = self.reread(row["pid"])
            except Exception as error:
                if attempt == 0:
                    first_error = error
                return (False, tries, "read_failed", error, first_row, first_error, False)
            if attempt == 0:
                first_row, first_error = again, None
            if again is None:
                return (True, tries, "absent", None, first_row, first_error, all_clean)
            if again["start"] != row["start"]:
                return (True, tries, "replaced", None, first_row, first_error, all_clean)
            if not _same_complete_zombie(row, again):
                all_clean = False
        return (False, tries, "matching_exhausted", None, first_row, first_error, all_clean)

    def round(self):
        self.reset_stats()
        begun = now_ns()
        try:
            rows = self.snapshot()
        except Exception as error:
            try:
                self.stats["snapshot_ns"] = now_ns() - begun
                self.stats["snapshot_error"] = True
            except BaseException:
                pass
            return {"certain": False, "members": len(self.tracked), "uncertain": 1,
                    "error": f"snapshot: {error}", "member_rows": [], "uncertain_rows": [],
                    "terminal_count_status": "invalid", "terminal_nonwriters": None}
        try:
            self.stats["snapshot_ns"] = now_ns() - begun  # observation only
            self.stats["rows"] = len(rows)
            self.stats["zombie_rows"] = sum(1 for row in rows if row.get("zombie") is True)
        except BaseException:
            pass
        present = {(row["pid"], row["start"]) for row in rows}
        for identity in list(self.tracked):
            if identity not in present:
                del self.tracked[identity]
        members, uncertain = [], []
        diag_attempted, diag_saturated, diag_rows = 0, False, []
        terminal_count, terminal_witnesses, terminal_tainted = 0, [], False
        for row in rows:
            identity = (row["pid"], row["start"])
            failure, getsid_error = None, None
            try:
                session = self.getsid(row["pid"])
            except ProcessLookupError as error:
                try:
                    actual_esrch = _ERRNO.__get__(error) == errno.ESRCH
                except BaseException:
                    actual_esrch = False
                if actual_esrch:
                    session, failure, getsid_error = None, "gone", error
                else:
                    session, failure = None, f"getsid: {error}"
            except OSError as error:
                session, failure = None, f"getsid: {error}"
            if failure == "gone":
                if diag_attempted >= U64MAX:
                    diag_saturated = True
                else:
                    diag_attempted += 1
                gone, tries, confirm_outcome, _cerr, first_row, first_error, all_clean = self.confirm_gone_info(row)
                if gone:
                    self.tracked.pop(identity, None)
                    try:
                        diag_rows.append(_diag_row(row, tries, confirm_outcome, "not_read", "none", None))
                    except BaseException:
                        pass
                    continue
                try:
                    again = self.reread(row["pid"])
                except Exception as error:
                    again = None
                    failure = f"reread: {error}" if failure in (None, "gone") else failure
                    try:
                        category, number = _diag_classify(error)
                        diag_rows.append(_diag_row(row, tries, confirm_outcome, "read_failed", category, number))
                    except BaseException:
                        pass
                    try:
                        if self.terminal_allowed is True and row.get("zombie") is True:
                            terminal_tainted = True
                    except BaseException:
                        pass
                    stable = False
                else:
                    if again is None:
                        self.tracked.pop(identity, None)
                        try:
                            diag_rows.append(_diag_row(row, tries, confirm_outcome, "absent", "none", None))
                        except BaseException:
                            pass
                        continue
                    if again["start"] != row["start"]:
                        self.tracked.pop(identity, None)
                        try:
                            diag_rows.append(_diag_row(row, tries, confirm_outcome, "replaced", "none", None))
                        except BaseException:
                            pass
                        continue
                    stable = True
                    try:
                        category, number = _diag_classify(getsid_error)
                        diag_rows.append(_diag_row(row, tries, confirm_outcome, "same_identity", category, number))
                    except BaseException:
                        pass
                    try:
                        terminal = (confirm_outcome == "matching_exhausted" and all_clean
                                    and self.terminal_eligible(row, first_row, first_error)
                                    and _same_complete_zombie(row, again))
                    except BaseException:
                        terminal = False
                    if not terminal:
                        # An actual-ESRCH row with a SZOMB snapshot that is
                        # neither Gone nor terminal-accepted stays
                        # unaccounted and taints the terminal count, but only
                        # when terminal acceptance is possible at all. In
                        # strict mode the count is accountably zero; rows that
                        # were never terminal candidates (no SZOMB snapshot)
                        # leave an accountable count alone.
                        try:
                            if self.terminal_allowed is True and row.get("zombie") is True:
                                terminal_tainted = True
                        except BaseException:
                            pass
                    if terminal:
                        try:
                            witness = _terminal_witness_row(row, identity in self.tracked)
                        except BaseException:
                            witness = None
                        if witness is not None:
                            terminal_witnesses.append(witness)
                        terminal_count += 1
                        continue  # kept tracked attribution; neither member nor uncertain
                    failure = "getsid ESRCH, absence not confirmed"
                if failure is None and stable and session == self.sid:
                    self.tracked[identity] = row
                    members.append(row)
                elif identity in self.tracked:
                    members.append(row)
                elif failure is None and stable:
                    continue  # positively foreign
                else:
                    uncertain.append(dict(row, reason=failure or "identity changed across getsid"))
                continue
            try:
                again = self.reread(row["pid"])
            except Exception as error:
                again, failure = None, f"reread: {error}" if failure in (None, "gone") else failure
            stable = again is not None and again["start"] == row["start"]
            if failure == "gone":
                failure = "getsid ESRCH, absence not confirmed"
            if failure is None and stable and session == self.sid:
                self.tracked[identity] = row
                members.append(row)
            elif identity in self.tracked:
                members.append(row)
            elif failure is None and stable:
                continue  # positively foreign
            else:
                uncertain.append(dict(row, reason=failure or "identity changed across getsid"))
        try:
            self.stats["scan_ns"] = now_ns() - begun - self.stats["snapshot_ns"]  # observation only
            self.stats["esrch_rows"] = diag_attempted
            self.stats["terminal"] = terminal_count
        except BaseException:
            pass
        try:
            status, value = _terminal_fields(terminal_count, terminal_tainted, False)
        except BaseException:
            status, value = "invalid", None
        if status not in TERMINAL_STATUS_VALUES:
            status, value = "invalid", None
        if status in ("invalid", "overflow") and not uncertain:
            # An unaccountable terminal count must block closure through
            # the unchanged IPC fields, even if no row was uncertain.
            uncertain.append({"reason": "terminal count not countable"})
        result = {"certain": not uncertain, "members": len(members), "uncertain": len(uncertain),
                "member_rows": members[:20], "uncertain_rows": uncertain[:20]}
        result["terminal_count_status"] = status
        result["terminal_nonwriters"] = value if (
            status == "valid" and type(value) is int and 0 <= value <= U64MAX) else None
        diag_compact_len, diag_kept = 0, 0
        if diag_attempted:
            try:
                diagnostics = _diag_object(diag_attempted, diag_rows, diag_saturated)
            except BaseException:
                diagnostics = None
            if diagnostics is not None:
                try:
                    check = json.dumps(diagnostics, ensure_ascii=True, separators=(',', ':')).encode("utf-8")
                    if len(check) <= DIAG_MAX_BYTES and len(diagnostics.get("rows", [])) <= DIAG_MAX_ROWS:
                        result["diagnostics"] = diagnostics
                        diag_compact_len, diag_kept = len(check), len(diagnostics.get("rows", []))
                except BaseException:
                    pass
        if terminal_witnesses:
            try:
                fitted = _fit_witnesses(terminal_witnesses, diag_kept, diag_compact_len)
            except BaseException:
                fitted = []
            if fitted:
                result["terminal_witnesses"] = fitted
        return result

@contextmanager
def patched(target, name, value):
    """Temporarily replace one production seam, restoring it afterwards."""
    saved = getattr(target, name)
    setattr(target, name, value)
    try:
        yield
    finally:
        setattr(target, name, saved)


def owned_witness_report(witnesses, starts):
    """Coverage of a (possibly clipped) terminal-witness list against the
    identities this fixture positively holds.

    A shared host census can legitimately retain ambient allowlisted terminal
    rows, so those are accepted as observations and never treated as foreign
    writers. They can never stand in for an owned identity: an owned identity
    must be witnessed with its exact (PID, start). The witness list is bounded
    by a shared byte budget, so the report names which owned identities were
    witnessed and which were clipped away rather than claiming all of them
    from a subset."""
    rows = [entry for entry in witnesses or [] if isinstance(entry, dict)]
    owned, ambient, problems = {}, [], []
    for entry in rows:
        pid = entry.get("pid")
        if pid in starts:
            owned[pid] = entry.get("start")
            if entry.get("start") != starts[pid]:
                problems.append(f"owned pid {pid} witnessed with start {entry.get('start')} "
                                f"instead of {starts[pid]}")
        else:
            ambient.append(pid)
    omitted = sorted(set(starts) - set(owned))
    if not owned:
        problems.append(f"no owned identity witnessed ({len(starts)} held, "
                        f"{len(ambient)} ambient retained, {len(omitted)} omitted)")
    return {"problems": problems, "owned": owned, "ambient": ambient, "omitted": omitted}


def owned_witness_problems(witnesses, starts):
    """Just the problems from owned_witness_report."""
    return owned_witness_report(witnesses, starts)["problems"]


def ordering_problems(result):
    """Why a round's published lists are not in snapshot order, or [].

    The applied check on the corrected source, and the oracle the shuffled
    counterfactual below must falsify: rows and diagnostic entries are ordered
    by PID just as the snapshot was, before any clipping."""
    problems = []
    for name in ("member_rows", "uncertain_rows"):
        pids = [entry.get("pid") for entry in result.get(name) or [] if isinstance(entry, dict)]
        if pids != sorted(pids):
            problems.append(f"{name} not in snapshot order")
    diagnostics = result.get("diagnostics")
    if diagnostics:
        pids = [entry.get("pid") for entry in diagnostics.get("rows") or [] if isinstance(entry, dict)]
        if pids != sorted(pids):
            problems.append("diagnostic rows not in snapshot order")
        if diagnostics.get("omitted", 0) != diagnostics.get("attempted", 0) - len(diagnostics["rows"]):
            problems.append("diagnostic clipping does not add up")
    return problems


class ScriptedTable:
    """Deterministic per-identity census fixture for the differential cases.

    Each PID carries its snapshot row, its getsid result and its own script of
    KERN_PROC_PID reread outcomes. The n-th reread of a PID consumes that PID's
    n-th script entry and the last entry repeats, so behaviour depends only on
    how many times THAT identity was read - never on a shared global read
    count - and the identical script can drive the production round and the
    serial reference independently. Every call is logged in one ordered list.
    """

    def __init__(self, rows, sessions, scripts):
        self.rows = list(rows)
        self.sessions = dict(sessions)
        self.scripts = {pid: list(items) for pid, items in scripts.items()}
        self.reads = {}
        self.log = []

    def snapshot(self):
        self.log.append(("snapshot", None, None))
        return list(self.rows)

    def getsid(self, pid):
        self.log.append(("getsid", pid, None))
        value = self.sessions[pid]
        if isinstance(value, BaseException):
            raise value
        return value

    def reread(self, pid):
        self.reads[pid] = self.reads.get(pid, 0) + 1
        items = self.scripts.get(pid) or [None]
        value = items[min(self.reads[pid] - 1, len(items) - 1)]
        self.log.append(("reread", pid, self.reads[pid]))
        if isinstance(value, BaseException):
            raise value
        return value

    def pause(self, seconds):
        self.log.append(("pause", None, seconds))

    def census(self, terminal_allowed=False):
        primed = set()
        def snapshot():
            primed.clear()
            return self.snapshot()
        def reread(pid):
            if pid not in primed:
                primed.add(pid)
                return next((r for r in self.rows if r["pid"] == pid), None)
            return self.reread(pid)
        return bounded.Census(100, snapshot=snapshot, getsid=self.getsid,
                              reread=reread, pause=self.pause,
                              terminal_allowed=terminal_allowed)

    def reference(self, terminal_allowed=False):
        return SerialCensus(100, snapshot=self.snapshot, getsid=self.getsid,
                            reread=self.reread, pause=self.pause,
                            terminal_allowed=terminal_allowed)

    def rereads(self, pid):
        return [entry for entry in self.log if entry[0] == "reread" and entry[1] == pid]

    def pauses(self):
        return [entry for entry in self.log if entry[0] == "pause"]

    def pause_between(self, pid, first, second):
        """Whether at least one pause sits between two numbered reads of pid."""
        seen, between = 0, 0
        for kind, who, _value in self.log:
            if kind == "reread" and who == pid:
                seen += 1
                if seen == second:
                    return between >= 1
            elif kind == "pause" and first <= seen < second:
                between += 1
        return False

    def first_read_follows_getsid(self):
        """Whether every confirmed identity's first reread follows its own
        getsid immediately, with no other call in between."""
        problems = []
        for position, entry in enumerate(self.log):
            if entry[0] != "getsid":
                continue
            following = self.log[position + 1] if position + 1 < len(self.log) else None
            if following is None or following[0] != "reread" or following[1] != entry[1]:
                problems.append(entry[1])
        return problems


ESRCH = ProcessLookupError(3, "gone")
EPERM = PermissionError(1, "denied")


def dead_row(pid, start=7):
    return row(pid, start=start, zombie=True)


def scenario(esrch=(), members=(), foreign=(), unclear=()):
    """A ScriptedTable from per-identity confirmation scripts.

    `esrch` is a sequence of (pid, script) pairs: getsid raises an actual ESRCH
    and the script drives that identity's confirmation history."""
    rows, sessions, scripts = [], {}, {}
    for pid in members:
        rows.append(row(pid, start=11)); sessions[pid] = 100; scripts[pid] = [row(pid, start=11)]
    for pid in foreign:
        rows.append(row(pid, start=12)); sessions[pid] = 7; scripts[pid] = [row(pid, start=12)]
    for pid in unclear:
        rows.append(row(pid, start=13)); sessions[pid] = EPERM; scripts[pid] = [row(pid, start=13)]
    for pid, script in esrch:
        rows.append(dead_row(pid)); sessions[pid] = ESRCH; scripts[pid] = script
    return ScriptedTable(rows, sessions, scripts)


# Confirmation histories the serial algorithm at 8849f99 already handled, and
# the exact per-identity reread counts it produced: the 20 confirmation reads
# plus the one separate final read wherever the identity reached it.
PERSISTENT = [dead_row(900)]
GONE_AT_3 = [dead_row(901), dead_row(901), None]
REPLACED_AT_7 = [dead_row(902)] * 6 + [row(902, start=8)]
FAIL_AT_1 = [EPERM]
FAIL_AT_5 = [dead_row(903)] * 4 + [EPERM]
CONTRADICTION = [dead_row(904), row(904, start=7)]  # same start token, never SZOMB
NONZOMBIE = [row(905, start=7)]                     # actual ESRCH, never SZOMB
FINAL_FAILS = [dead_row(970)] * 20 + [EPERM]        # 20 confirmations, then read 21 fails
FINAL_FAILS_NONZOMBIE = [row(971, start=7)] * 20 + [EPERM]


def without_replacement_work(result):
    """Compare the historical fields to the serial oracle; the new work
    summary is independently checked by the replacement-work regressions."""
    return {key: value for key, value in result.items() if key != "replacement_work"}


class PureInterleavedConfirmation(unittest.TestCase):
    """Oxide#147 correction: shared confirmation passes must return exactly the
    round data the serial algorithm at 8849f99 returned, while every identity
    keeps its existing observations and spacing and one shared pause per pass
    replaces one pause per identity."""

    def compare(self, script, allowed, **kwargs):
        """Run the corrected round and the serial reference over two
        independently scripted fixtures, and assert identical round data."""
        produced_table = scenario(esrch=script, **kwargs)
        reference_table = scenario(esrch=script, **kwargs)
        produced = produced_table.census(allowed)
        result = produced.round()
        self.assertEqual(without_replacement_work(result), reference_table.reference(allowed).round(),
                         "corrected round data must equal the serial reference")
        return produced_table, produced, result

    def test_persistent_zombies_share_one_pause_per_pass(self):
        for count in (0, 1, 4, 12, 15):
            for allowed in (False, True):
                with self.subTest(zombies=count, allowlisted=allowed):
                    pids = list(range(910, 910 + count))
                    script = [(pid, [dead_row(pid)]) for pid in pids]
                    table, produced, result = self.compare(
                        script, allowed, members=[800], foreign=[810], unclear=[820])
                    for pid in pids:
                        self.assertEqual(len(table.rereads(pid)), 21,
                                         "20 confirmation reads plus one separate final read")
                    self.assertEqual(produced.stats["pause_calls"], 19 if count else 0)
                    if count:
                        # The serial reference pays one pause per identity per
                        # pass; the corrected round pays one pause per pass.
                        serial = scenario(esrch=script, members=[800], foreign=[810],
                                          unclear=[820]).reference(allowed)
                        self.assertEqual(serial.round(), without_replacement_work(result))
                        self.assertEqual(serial.stats["pause_calls"], 19 * count)
                        self.assertEqual(serial.stats["confirm_reads"], produced.stats["confirm_reads"])
                    self.assertEqual(table.first_read_follows_getsid(), [])

    def test_every_candidate_resolving_at_the_first_read_never_pauses(self):
        for allowed in (False, True):
            with self.subTest(allowlisted=allowed):
                table, produced, _result = self.compare(
                    [(921, [None]), (922, [None])], allowed)
                self.assertEqual(produced.stats["pause_calls"], 0)
                self.assertEqual((len(table.rereads(921)), len(table.rereads(922))), (1, 1),
                                 "a Gone identity is never read again and gets no final read")

    def test_mixed_resolutions_match_the_serial_reference(self):
        for allowed in (False, True):
            with self.subTest(allowlisted=allowed):
                table, produced, result = self.compare(
                    [(900, PERSISTENT), (901, GONE_AT_3), (902, [dead_row(902)] * 6 + [None]),
                     (903, FAIL_AT_5), (904, CONTRADICTION), (905, NONZOMBIE)],
                    allowed, members=[800], foreign=[810], unclear=[820])
                self.assertEqual({pid: len(table.rereads(pid)) for pid in (900, 901, 902, 903, 904, 905)},
                                 {900: 21, 901: 3, 902: 7, 903: 6, 904: 21, 905: 21})
                self.assertEqual([entry["pid"] for entry in result["member_rows"]], [800])
                self.assertEqual(result["members"], 1)
                self.assertEqual(produced.stats["pause_calls"], 19)
                self.assertNotIn("stats", result)
                # 800 is the only member; 810 is foreign and 820 unreadable,
                # and the four unresolved ESRCH identities block certainty.
                self.assertFalse(result["certain"])
                for pid in (900, 901, 902, 903, 904, 905):
                    confirmations = min(len(table.rereads(pid)), 20)
                    for index in range(2, confirmations + 1):
                        self.assertTrue(table.pause_between(pid, index - 1, index),
                                        f"pid {pid} confirmation reads {index - 1}->{index} must be a pause apart")
                self.assertEqual(table.first_read_follows_getsid(), [])

    def test_shared_pass_replacement_does_not_inherit_old_disposition(self):
        table = scenario(esrch=[(902, REPLACED_AT_7)])
        result = table.census().round()
        self.assertFalse(result["certain"])
        self.assertEqual(result["uncertain"], 1)
        self.assertEqual(result["replacement_work"]["replacement"], 1)
        self.assertIn({"pid": 902, "start": 8, "state": "uncertain"}, result["replacement_work"]["rows"])

    def test_tracked_attribution_is_unchanged(self):
        for allowed, expected_members in ((False, 1), (True, 0)):
            with self.subTest(allowlisted=allowed):
                table = scenario(esrch=[(950, [dead_row(950)])])
                produced = table.census(allowed)
                produced.tracked[(950, 7)] = dead_row(950)
                result = produced.round()
                reference = scenario(esrch=[(950, [dead_row(950)])]).reference(allowed)
                reference.tracked[(950, 7)] = dead_row(950)
                self.assertEqual(without_replacement_work(result), reference.round())
                self.assertEqual(result["members"], expected_members,
                                 "strict keeps the tracked identity as a member; terminal keeps it tracked")
                self.assertEqual(result["terminal_nonwriters"], 1 if allowed else 0)
                self.assertEqual((produced.tracked.get((950, 7)) is not None),
                                 (reference.tracked.get((950, 7)) is not None))

    def test_one_identity_resolving_early_leaves_the_others_independent(self):
        both, _produced, both_result = self.compare([(900, PERSISTENT), (901, GONE_AT_3)], False)
        self.assertEqual((len(both.rereads(900)), len(both.rereads(901))), (21, 3))
        solo, _produced, solo_result = self.compare([(900, PERSISTENT)], False)
        self.assertEqual(len(solo.rereads(900)), 21)
        self.assertEqual((solo_result["uncertain"], both_result["uncertain"]), (1, 1),
                         "a Gone identity is never uncertain and adds no uncertainty")
        self.assertEqual((solo_result["diagnostics"]["attempted"],
                          both_result["diagnostics"]["attempted"]), (1, 2),
                         "the extra Gone identity is the only difference")

    def test_a_first_read_failure_keeps_the_existing_final_read(self):
        for allowed in (False, True):
            with self.subTest(allowlisted=allowed):
                table, produced, result = self.compare([(930, FAIL_AT_1)], allowed)
                self.assertEqual(len(table.rereads(930)), 2, "one failed confirmation and the final read")
                self.assertIn("reread", result["uncertain_rows"][0]["reason"])
                self.assertEqual(produced.stats["pause_calls"], 0)

    def test_more_than_twenty_rows_keep_snapshot_order_and_clipping(self):
        # Persistent identities that can never be terminal, so the uncertain
        # list itself overflows 20 and must be clipped in snapshot order.
        esrch = [(940 + index, [row(940 + index, start=7)]) for index in range(26)]
        _table, _produced, result = self.compare(esrch, True, members=[800], foreign=[810], unclear=[820])
        self.assertEqual(ordering_problems(result), [])
        self.assertEqual([entry["pid"] for entry in result["uncertain_rows"]],
                         [820] + list(range(940, 959)),
                         "the unreadable row keeps its snapshot position and the list clips at 20")
        self.assertEqual(result["uncertain"], 27)
        diagnostics = result["diagnostics"]
        self.assertEqual([entry["pid"] for entry in diagnostics["rows"]], list(range(940, 960)),
                         "diagnostic rows keep snapshot order and clip at 20")
        self.assertEqual((diagnostics["attempted"], diagnostics["omitted"]), (26, 6))
        self.assertEqual(result["terminal_count_status"], "invalid",
                         "unrelated uncertainty makes the terminal count unaccountable")
        self.assertIsNone(result["terminal_nonwriters"])

        # The same shape with persistent zombies stays terminal-counted rather
        # than uncertain, still in snapshot order.
        zombies = [(960 + index, [dead_row(960 + index)]) for index in range(12)]
        _table, _produced, zombie_result = self.compare(zombies, True)
        self.assertEqual(ordering_problems(zombie_result), [])
        self.assertTrue(zombie_result["certain"])
        self.assertEqual((zombie_result["uncertain"], zombie_result["terminal_nonwriters"]), (0, 12))
        # Witness rows share one byte budget with the diagnostics, so the
        # kept prefix is a subset of the snapshot order and never reordered.
        witnesses = [entry["pid"] for entry in zombie_result["terminal_witnesses"]
                     if isinstance(entry, dict)]
        self.assertEqual(witnesses, list(range(960, 960 + len(witnesses))))
        self.assertGreaterEqual(len(witnesses), 8)

    def test_tracked_final_read_failure_keeps_serial_attribution(self):
        # oxide#147 correction review: a failed FINAL read must follow the
        # serial tail at 8849f99, where a tracked identity keeps its retained
        # member attribution and only an untracked one becomes uncertain.
        for allowed in (False, True):
            for script, pid in ((FINAL_FAILS, 970), (FINAL_FAILS_NONZOMBIE, 971)):
                with self.subTest(allowlisted=allowed, script=pid):
                    table = scenario(esrch=[(pid, script)])
                    produced = table.census(allowed)
                    produced.tracked[(pid, 7)] = (dead_row(pid) if pid == 970
                                                  else row(pid, start=7))
                    result = produced.round()
                    reference = scenario(esrch=[(pid, script)]).reference(allowed)
                    reference.tracked[(pid, 7)] = (dead_row(pid) if pid == 970
                                                   else row(pid, start=7))
                    expected = reference.round()
                    self.assertEqual(without_replacement_work(result), expected,
                                     "a tracked final-read failure must keep the serial result")
                    self.assertEqual([entry["pid"] for entry in result["member_rows"]], [pid])
                    self.assertEqual(len(table.rereads(pid)), 21)
                    self.assertEqual(produced.stats["pause_calls"], 19)
                    self.assertIn((pid, 7), produced.tracked)
                    self.assertEqual(produced.tracked.keys(), reference.tracked.keys())
                    self.assertIn("read_failed", result["diagnostics"]["rows"][0]["final"])
                    if allowed:
                        # Every ESRCH identity here has a SZOMB snapshot row, so in
                        # allowlisted mode only the terminal count becomes
                        # unaccountable and adds the synthetic uncertainty row,
                        # exactly as the serial did.
                        self.assertEqual([entry.get("reason") for entry in result["uncertain_rows"]],
                                         ["terminal count not countable"])
                    else:
                        self.assertEqual(result["uncertain_rows"], [])
                        self.assertEqual(result["certain"], True)

        # Untracked, the same failure is uncertain, exactly as before.
        table = scenario(esrch=[(970, FINAL_FAILS)])
        self.assertEqual(without_replacement_work(table.census(True).round()),
                         scenario(esrch=[(970, FINAL_FAILS)]).reference(True).round())
        self.assertEqual(len(table.rereads(970)), 21)

        # The currently incorrect unconditional-uncertain branch must fail.
        tracked_reference = scenario(esrch=[(970, FINAL_FAILS)]).reference(True)
        tracked_reference.tracked[(970, 7)] = dead_row(970)
        expected_tracked = tracked_reference.round()
        with patched(bounded.Census, "retains_member_attribution", lambda self, identity: False):
            wrong = scenario(esrch=[(970, FINAL_FAILS)])
            wrong_census = wrong.census(True)
            wrong_census.tracked[(970, 7)] = dead_row(970)
            wrong_result = wrong_census.round()
        self.assertNotEqual(wrong_result, expected_tracked,
                            "dropping retained attribution on a failed final read must be detected")
        self.assertEqual(wrong_result["uncertain_rows"][0]["pid"], 970)

    def test_owned_witness_oracle_accepts_ambient_but_requires_own(self):
        starts = {5001: 11, 5002: 12}
        own = [{"pid": 5001, "start": 11, "tracked": False},
               {"pid": 5002, "start": 12, "tracked": False}]
        ambient = [{"pid": 9001, "start": 77, "tracked": False}]
        self.assertEqual(owned_witness_problems(own + ambient, starts), [])
        self.assertEqual(owned_witness_problems([{"pid": 5001, "start": 11}], starts), [])
        self.assertTrue(owned_witness_problems(ambient, starts),
                        "ambient witnesses alone must never stand in for an owned identity")
        self.assertTrue(owned_witness_problems([], starts),
                        "an empty retained list cannot prove an owned witness")
        self.assertTrue(owned_witness_problems([{"pid": 5001, "start": 99}] + ambient, starts),
                        "an owned PID with the wrong start token must be rejected")
        # A clipped list that lost every owned identity reports that plainly.
        report = owned_witness_report([{"pid": 9001, "start": 77}], starts)
        self.assertEqual((report["owned"], report["omitted"]), ({}, [5001, 5002]))
        self.assertTrue(report["problems"])

    def test_snapshot_order_oracles_reject_a_shuffled_list(self):
        esrch = [(940 + index, [None] if index % 2 == 0 else [row(940 + index, start=7)])
                 for index in range(8)]  # persistent but never SZOMB: stays uncertain
        result = scenario(esrch=esrch).census(True).round()
        self.assertEqual(ordering_problems(result), [])
        shuffled = dict(result, uncertain_rows=list(reversed(result["uncertain_rows"])))
        self.assertTrue(ordering_problems(shuffled))
        diagnostics = dict(result["diagnostics"], rows=list(reversed(result["diagnostics"]["rows"])))
        self.assertTrue(ordering_problems(dict(result, diagnostics=diagnostics)))

    def live_run(self, allowed=True):
        """One production round over a persistent zombie; returns its observable
        facts so a mutation can be judged by the same checks."""
        table = scenario(esrch=[(900, PERSISTENT)])
        produced = table.census(allowed)
        result = produced.round()
        return {"result": without_replacement_work(result),
                "reference": scenario(esrch=[(900, PERSISTENT)]).reference(allowed).round(),
                "reads": len(table.rereads(900)),
                "pauses": produced.stats["pause_calls"],
                "pause_values": [entry[2] for entry in table.pauses()],
                "witnesses": result.get("terminal_witnesses") or []}

    def test_counterfactuals_fail_the_differential(self):
        baseline = self.live_run()
        self.assertEqual(baseline["result"], baseline["reference"])
        self.assertEqual((baseline["reads"], baseline["pauses"], len(baseline["witnesses"])), (21, 19, 1))
        self.assertTrue(all(value == bounded.CONFIRM_PAUSE for value in baseline["pause_values"]))

        # Fewer tries: the identity loses confirmation reads and its final read.
        with patched(bounded, "CONFIRM_TRIES", bounded.CONFIRM_TRIES - 1):
            observed = self.live_run()
        self.assertNotEqual(observed["reads"], baseline["reads"], "fewer tries must be detected")

        # A zero pause constant: the per-identity spacing evidence disappears.
        with patched(bounded, "CONFIRM_PAUSE", 0.0):
            observed = self.live_run()
        self.assertNotEqual(observed["pause_values"], baseline["pause_values"],
                            "a changed pause constant must be detected")

        # A no-op batch that never reads must not look like a confirmation.
        with patched(bounded.Census, "confirmation_attempt", lambda self, row_, state: True):
            observed = self.live_run()
        self.assertEqual(observed["reads"], 0, "a no-op batch must be detected")

        # A dropped final read leaves 20 instead of 21 rereads and changes the
        # Gone classification, so it must be detected.
        table = scenario(esrch=[(900, PERSISTENT)])
        produced = table.census(True)
        real_reread = produced.reread
        produced.reread = lambda pid: None if len(table.rereads(pid)) >= 20 else real_reread(pid)
        dropped = produced.round()
        self.assertNotEqual(dropped, scenario(esrch=[(900, PERSISTENT)]).reference(True).round(),
                            "a dropped final read must be detected")
        self.assertEqual(len(table.rereads(900)), 20)

        # A lost terminal witness changes the receipt evidence.
        with patched(bounded, "_terminal_witness_row", lambda row_, tracked: None):
            observed = self.live_run()
        self.assertEqual(observed["witnesses"], [], "a lost witness must be detected")

        # A matching early exit: treating a matching read as a resolution must
        # be detected, because a match never short-circuits.
        real_attempt = bounded.Census.confirmation_attempt
        def early_exit(self, row_, state):
            matched = real_attempt(self, row_, state)
            if matched:
                state["outcome"] = "matching_exhausted"
            return False
        with patched(bounded.Census, "confirmation_attempt", early_exit):
            observed = self.live_run()
        self.assertEqual(observed["reads"], 2, "a matching early exit must be detected")
        self.assertNotEqual(observed["result"], observed["reference"],
                            "a matching early exit must change the round data")

        # Relaxed terminal eligibility changes the strict classification.
        with patched(bounded.Census, "terminal_eligible", lambda self, row_, first, error: True):
            observed = self.live_run(allowed=False)
        self.assertNotEqual(observed["result"], observed["reference"],
                            "relaxed terminal eligibility must be detected")

# Real cases --------------------------------------------------------------------



def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


def nested_root():
    """Outside the outer supervisor's cleanup; never deleted here."""
    if not OUTER_EVIDENCE:
        return SCRATCH
    root = Path(OUTER_EVIDENCE) / "nested"
    root.mkdir(exist_ok=True)
    return root.resolve()


PROTECTION = {}


def protect_outer_root():
    """Durably mark the outer candidate root as `hosting` BEFORE any nested
    supervisor can exist (oxide#151). A hosting root can never be concluded
    from its own outer closure, so an intentionally retained nested UNCLOSED
    fixture cannot leave an expirable root behind. An unmarked root is never
    adopted and is never eligible either; a marker that exists but cannot be
    protected aborts this suite before any real case runs, which leaves no
    nested UNKNOWN resource for a later conclusion to cover."""
    if not OUTER_EVIDENCE:
        return {"state": "no-outer-root"}
    root = os.path.dirname(os.path.realpath(OUTER_EVIDENCE))
    try:
        document = bounded.protect_nested_scope(root, bounded.DefaultProbe())
    except bounded.MarkerInvalid as invalid:
        if invalid.code == "absent":
            return {"state": "unmarked", "root": root}
        return {"state": "failed", "root": root, "reason": "marker-" + invalid.code}
    except OSError as error:
        return {"state": "failed", "root": root, "reason": type(error).__name__}
    return {"state": "protected", "root": root, "run_id": document["run_id"]}


def record_layout():
    """Record the layout this suite ran in for the qualification bootstrap
    (oxide#145, oxide#151): the candidate's owned root, scratch and evidence,
    and the nested-window protection it established first. Nested real-case
    directories are created in OXIDE_PROOF_EVIDENCE, so recording and verifying
    this record is what proves the nested resources stayed inside the
    candidate's monitored root on the selected volume."""
    path = os.environ.get("OXIDE_SUPERVISOR_LAYOUT")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
    if not path or not scratch or not OUTER_EVIDENCE:
        return
    record = {"scratch": os.path.realpath(scratch), "evidence": os.path.realpath(OUTER_EVIDENCE),
              "suite_pid": os.getpid(), "protection": dict(PROTECTION)}
    with open(path, "w") as handle:
        json.dump(record, handle)


class Real(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp(prefix=self.id().rsplit(".", 1)[-1] + ".", dir=nested_root())).resolve()
        self.started = now_ns()

    def guardian(self, code, inject=(), owned=False, limit=35, signal_after=None, signal_number=None,
                 command=None, extra_env=None, receiver=None, receiver_count=0):
        env = dict(os.environ, CASE_DIR=str(self.dir), FIXTURE_DEADLINE_NS=str(self.started + FALLBACK_S * NS),
                   OXIDE_SUPERVISOR_INJECT=",".join(inject))
        env.update(extra_env or {})
        args = [sys.executable, str(SUPERVISOR), "--limit", str(limit), "--evidence", str(self.dir / "evidence")]
        if owned:
            env.pop("OXIDE_TASK_SCRATCH_ROOT", None)
            args += ["--root-parent", str(self.dir)]
        else:
            env["OXIDE_TASK_SCRATCH_ROOT"] = str(self.dir)
        with open(self.dir / "guardian.log", "wb") as log:
            guardian = subprocess.Popen(args + ["--"] + (command or [sys.executable, "-c", code]), env=env,
                                        stdout=subprocess.DEVNULL, stderr=log)
            try:
                if receiver is not None:  # actual descriptor handoff, then ACK, then the wait
                    receiver.accept(receiver_count, FD_ACK_TIMEOUT_S)
                if signal_after is not None:
                    deadline = time.monotonic() + 10
                    while not (self.dir / signal_after).exists() and time.monotonic() < deadline:
                        time.sleep(0.05)
                    guardian.send_signal(signal_number)
                self.code = guardian.wait(timeout=VERDICT_S - (now_ns() - self.started) / NS)
            finally:
                if guardian.poll() is None:  # the candidate overran the case verdict
                    guardian.kill()
                    guardian.wait(5)
        self.verdict_ns = now_ns()
        self.assertLess(self.verdict_ns, self.started + FALLBACK_S * NS, "verdict after the fixture fallback")
        self.receipt = json.loads((self.dir / "evidence" / "receipt.json").read_text())
        signals = [r for r in self.receipt["custodian"] if r["event"] == "signal"]
        closure = (self.receipt["outcome"] or {}).get("closure")
        CASES.append({"case": self.id().rsplit(".", 1)[-1], "exit": self.code, "phase": self.receipt["phase"],
                      "dir": str(self.dir), "retained_unclosed": closure != "closed",
                      "outcome": self.receipt["outcome"], "census": (self.receipt["census"] or {}).get("members"),
                      "wall_s": round((self.verdict_ns - self.started) / NS, 3),
                      "signals": [[s["signal"], s["pin"], s["killpg"], s["held_unreaped"], s["result"]]
                                  for s in signals],
                      "signal_targets": [s["target"] for s in signals],
                      "survivors": self.receipt.get("survivors"),
                      "session_signals": [[record.get("target"), record.get("start")]
                                          for record in self.receipt["custodian"]
                                          if str(record.get("event", "")).startswith("session_")],
                      "held_streams": sorted(getattr(receiver, "held_names", []))})
        for entry in signals:  # custody at every signal
            self.assertTrue(entry["held_unreaped"], entry)
            self.assertIn(entry["pin"], ("leader", "monitor"))
        return self.code

    def outcome(self):
        return self.receipt["outcome"] or {}

    def child(self):
        return int((self.dir / "child.pid").read_text())

    def assert_not_launched(self):
        self.assertFalse((self.dir / "ran").exists(), "the command ran")

    def assert_kept_outside_outer_cleanup(self):
        # Review 6312725112 finding 1: an UNCLOSED case's resources are not
        # inside the outer supervisor's cleanup root.
        outer = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
        if OUTER_EVIDENCE and outer:
            self.assertFalse(str(self.dir).startswith(os.path.realpath(outer) + os.sep), self.dir)
        self.assertTrue((self.dir / "evidence" / "receipt.json").exists())

    # Normal paths

    def test_exit_zero_closed(self):
        self.assertEqual(self.guardian(fixture("open(os.environ['CASE_DIR'] + '/ran', 'w')\n")), 0)
        self.assertEqual(self.outcome()["closure"], "closed")
        self.assertEqual(self.receipt["phase"], "monitor_complete: pending_custodian")

    def replacement_fixture(self, mode, expected_state):
        # The seam supplies a labelled synthetic A -> B observation alongside
        # the actual kernel snapshot; all real rows still use the normal readers.
        self.assertEqual(self.guardian(fixture("pass\n"), inject=[mode]), 0)
        self.assertEqual(self.outcome()["closure"], "closed")
        observed = self.receipt.get("replacement_fixture")
        self.assertEqual((observed["source"], observed["mode"]), ("synthetic", mode))
        self.assertTrue(observed["certain"])
        self.assertEqual(observed["uncertain"], 0)
        work = observed["work"]
        self.assertEqual((work["status"], work["replacement"], work["pending"]), ("complete", 1, 0))
        self.assertIn({"pid": 2_147_483_647, "start": 2, "state": expected_state}, work["rows"])
        signalled = [r["target"] for r in self.receipt["custodian"] if r["event"] == "signal"]
        self.assertNotIn(2_147_483_647, signalled)
        CASES[-1]["replacement_fixture"] = observed

    def test_synthetic_replacement_member_reaches_real_receipt(self):
        self.replacement_fixture("census-replacement-member", "member")

    def test_synthetic_replacement_foreign_reaches_real_receipt(self):
        self.replacement_fixture("census-replacement-foreign", "foreign")

    def test_an_unrelated_launcher_sibling_does_not_block_closure(self):
        # Oxide#197 (design ruling ea5a3dc9). This suite is the nested
        # supervisor's launcher, so it is one of the ancestors that supervisor
        # records. An unrelated sibling of the nested run, spawned mid-run and
        # still alive during the post-leader census, is positively outside the
        # nested leader's session and process group, so the launcher-sibling
        # rule must exclude it and the nested run must still close. The sibling
        # exits on a release file written after the nested run ends, so it is
        # live (and readable) for every census round.
        sibling = []
        release = self.dir / "sibling.release"
        code = ("import os, time\n"
                "open(os.environ['CASE_DIR'] + '/sibling', 'w').write('1')\n"
                "while not os.path.exists(os.environ['CASE_DIR'] + '/sibling.release'):\n"
                "    time.sleep(0.02)\n")

        def spawn_mid_run():
            receipt = self.dir / "evidence" / "receipt.json"
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                try:
                    phase = json.loads(receipt.read_text()).get("phase") or ""
                except Exception:
                    phase = ""
                if phase.startswith("started"):
                    break
                time.sleep(0.02)
            sibling.append(subprocess.Popen(
                [sys.executable, "-c", code],
                env=dict(os.environ, CASE_DIR=str(self.dir)),
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))

        thread = threading.Thread(target=spawn_mid_run, daemon=True)
        thread.start()
        try:
            code_status = self.guardian(fixture("open(os.environ['CASE_DIR'] + '/ran', 'w')\n"))
        finally:
            thread.join(15)
            release.write_text("release")
            for child in sibling:
                try:
                    child.wait(10)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(5)
        self.assertFalse(thread.is_alive(), "the sibling was never spawned")
        self.assertTrue((self.dir / "sibling").exists(), "the sibling never ran")
        self.assertEqual(code_status, 0)
        self.assertEqual(self.outcome()["closure"], "closed")
        self.assertTrue((self.dir / "ran").exists())
        # The exclusion is positive, bounded evidence in the receipt.
        evidence = self.receipt["ancestor_exclusions"]
        self.assertGreaterEqual(evidence["by_rule"]["parent"], 1, evidence)
        self.assertEqual(evidence["count"], evidence["omitted"] + len(evidence["rows"]), evidence)
    @staticmethod
    def report_source(kind, command, waited_ns=None, reason=None):
        """Fixture source that builds one schema-2 report at run time.

        The document must be built inside the fixture, never here: the nested
        guardian a real case runs under exports ITS OWN run identity, and this
        suite process only sees the outer supervisor's. A document built here
        would name the wrong run and the nested guardian would reject it as
        `stale run`."""
        extra = (f"'reason': {reason or 'queue'!r}" if kind == "waiting"
                 else f"'waited_ns': {int(waited_ns)}")
        return ("{'schema': 2, 'source': 'fixture', "
                f"'run': os.environ['OXIDE_PROOF_RUN_ID'], 'command': {command!r}, {extra}}}")

    def test_admission_marker_extends_the_work_window(self):
        # oxide#192: a reported slot-queue wait moves the supervisor's deadline
        # by that wait, so execution keeps its own window instead of being
        # killed inside it. Without the grant the leader is stopped at
        # limit - 20 s, well before it finishes.
        document = self.report_source("grant", "fixture-1", waited_ns=10 * 1000000000)
        code = fixture(
            "import json\n"
            "path = os.path.join(os.environ['OXIDE_PROOF_EVIDENCE'], " + repr(bounded.ADMISSION_NAME) + ")\n"
            "time.sleep(10)\n"
            "with open(path + '.tmp', 'w') as handle:\n"
            "    json.dump(" + document + ", handle)\n"
            "os.replace(path + '.tmp', path)\n"
            "time.sleep(12)\n"
            "open(os.environ['CASE_DIR'] + '/ran', 'w')\n")
        self.assertEqual(self.guardian(code, limit=36), 0)
        self.assertEqual(self.outcome()["closure"], "closed")
        self.assertIsNone(self.outcome()["fault"])
        self.assertFalse(self.outcome()["deadline"])
        self.assertTrue((self.dir / "ran").exists(), "the command did not keep its work window")
        self.assertEqual(self.receipt["admission"]["observed"], 1)
        self.assertEqual(self.receipt["admission"]["ignored"], 0)
        self.assertIsNotNone(self.receipt["admission"]["first_ns"])
        decisions = [r for r in self.receipt["custodian"] if r["event"] == "admission_decision"]
        self.assertEqual(len(decisions), 1)
        self.assertEqual(decisions[0]["counted"], 1)
        self.assertGreaterEqual(decisions[0]["granted_ns"], 9 * NS)
        self.assertLessEqual(decisions[0]["granted_ns"], 11 * NS)
        self.assertFalse(decisions[0]["capped"])
        self.assertEqual(decisions[0]["admission_waiting"]["commands"], 0)

    def test_a_late_admission_marker_is_classified_not_ignored(self):
        # Review finding: slot admission can reach the supervisor after its stop
        # decision. The wait is reported, nothing is granted, and the deadline
        # classification stands instead of becoming an unexplained timeout.
        # The fixture ignores TERM, writes its marker after the stop instant and
        # stays in the group long enough for the sampler to read it while the
        # custodian is still in its bounded stop.
        waiting = self.report_source("waiting", "fixture-late")
        grant = self.report_source("grant", "fixture-late", waited_ns=13 * 1000000000)
        code = fixture(
            "import json\n"
            "signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
            "evidence = os.environ['OXIDE_PROOF_EVIDENCE']\n"
            "path = os.path.join(evidence, " + repr(bounded.ADMISSION_NAME) + ")\n"
            "waiting = os.path.join(evidence, " + repr(bounded.WAITING_NAME) + ")\n"
            "time.sleep(5)\n"
            "with open(waiting + '.tmp', 'w') as handle:\n"
            "    json.dump(" + waiting + ", handle)\n"
            "os.replace(waiting + '.tmp', waiting)\n"
            "time.sleep(8)\n"
            "with open(path + '.tmp', 'w') as handle:\n"
            "    json.dump(" + grant + ", handle)\n"
            "os.replace(path + '.tmp', path)\n"
            "open(os.environ['CASE_DIR'] + '/ran', 'w')\n"
            "time.sleep(1.5)\n")
        self.assertEqual(self.guardian(code, limit=32), 124)
        self.assertTrue(self.outcome()["deadline"], "a late marker must not look like an ordinary exit")
        self.assertEqual(self.outcome()["closure"], "closed")
        self.assertIsNone(self.outcome()["fault"])
        self.assertTrue((self.dir / "ran").exists(), "the fixture never reached its late write")
        self.assertEqual(self.receipt["admission"]["observed"], 1)
        late = [record for record in self.receipt["custodian"] if record["event"] == "admission_late"]
        self.assertEqual(len(late), 1)
        self.assertEqual(late[0]["reason"], "deadline")
        self.assertEqual(late[0]["waited_ns"], 13 * NS)
        self.assertGreaterEqual(late[0]["at"], self.receipt["start_ns"] + 12 * NS)
        decisions = [r for r in self.receipt["custodian"] if r["event"] == "admission_decision"]
        self.assertEqual(len(decisions), 1)
        self.assertEqual(decisions[0]["counted"], 0)
        self.assertEqual(decisions[0]["ignored"], 1)
        self.assertEqual(decisions[0]["granted_ns"], 0)
        self.assertEqual(decisions[0]["late_reason"], "deadline")
        self.assertEqual(decisions[0]["absolute_ns"], self.receipt["start_ns"] + 32 * NS)
        pairs = decisions[0]["admission_waiting"]
        self.assertEqual((pairs["commands"], pairs["ended"], pairs["unpaired_count"]), (1, 1, 0),
                         "a late grant ends its own command's wait, never leaving it unpaired")
        self.assertEqual(self.receipt["admission_waiting"]["observed"], 1)
        self.assertEqual(self.receipt["admission_waiting"]["reason"], "queue")

    def test_a_wait_that_never_ends_is_reported_and_classified(self):
        # Review finding: a wait ending at the supervisor stop could leave no
        # admission marker and no separately classified queue wait. A waiting
        # report is published when the wait starts, so the run is classified as
        # a deadline with a reported, never-granted queue wait.
        document = self.report_source("waiting", "fixture-1")
        code = fixture(
            "import json\n"
            "path = os.path.join(os.environ['OXIDE_PROOF_EVIDENCE'], "
            + repr(bounded.WAITING_NAME) + ")\n"
            "with open(path + '.tmp', 'w') as handle:\n"
            "    json.dump(" + document + ", handle)\n"
            "os.replace(path + '.tmp', path)\n"
            "time.sleep(30)\n")
        self.assertEqual(self.guardian(code, limit=32), 124)
        self.assertTrue(self.outcome()["deadline"])
        self.assertEqual(self.outcome()["closure"], "closed")
        self.assertIsNone(self.outcome()["fault"])
        self.assertEqual(self.receipt["admission"]["observed"], 0, "no completed wait was reported")
        waiting = self.receipt["admission_waiting"]
        self.assertEqual((waiting["observed"], waiting["reason"], waiting["ignored"]), (1, "queue", 0))
        reports = [record for record in self.receipt["custodian"] if record["event"] == "admission_waiting"]
        self.assertEqual(len(reports), 1)
        self.assertEqual((reports[0]["reason"], reports[0]["after_stop"]), ("queue", False))
        decisions = [r for r in self.receipt["custodian"] if r["event"] == "admission_decision"]
        self.assertEqual(len(decisions), 1)
        self.assertEqual((decisions[0]["waiting_reports"], decisions[0]["waiting_reason"]), (1, "queue"))
        self.assertEqual(decisions[0]["granted_ns"], 0)
        self.assertEqual(decisions[0]["absolute_ns"], self.receipt["start_ns"] + 32 * NS)
        pairs = decisions[0]["admission_waiting"]
        self.assertEqual((pairs["commands"], pairs["ended"], pairs["unpaired_count"]), (1, 0, 1))
        self.assertEqual(pairs["unpaired"][0]["command"], "fixture-1")
        self.assertEqual(pairs["unpaired"][0]["reason"], "queue")
        self.assertFalse(pairs["unpaired"][0]["later_observation"])

    def test_a_stale_wait_and_a_zero_duration_grant_are_not_proven_unfinished(self):
        # Review finding: a stale marker left by an earlier run, and a wait whose
        # own completion arrived with a zero duration, must not be reported as a
        # proven unfinished wait. The stale file is rejected by run identity and
        # the zero-duration grant ends its command's wait.
        evidence = self.dir / "evidence"
        evidence.mkdir()
        (evidence / bounded.WAITING_NAME).write_text(json.dumps(
            {"schema": 2, "source": "fixture", "run": "stale-run", "command": "fixture-stale",
             "reason": "queue"}))
        waiting = self.report_source("waiting", "fixture-1")
        grant = self.report_source("grant", "fixture-1", waited_ns=0)
        code = fixture(
            "import json\n"
            "evidence = os.environ['OXIDE_PROOF_EVIDENCE']\n"
            "waiting = os.path.join(evidence, " + repr(bounded.WAITING_NAME) + ")\n"
            "grant = os.path.join(evidence, " + repr(bounded.ADMISSION_NAME) + ")\n"
            "time.sleep(1)\n"
            "with open(waiting + '.tmp', 'w') as handle:\n"
            "    json.dump(" + waiting + ", handle)\n"
            "os.replace(waiting + '.tmp', waiting)\n"
            "with open(grant + '.tmp', 'w') as handle:\n"
            "    json.dump(" + grant + ", handle)\n"
            "os.replace(grant + '.tmp', grant)\n"
            "open(os.environ['CASE_DIR'] + '/ran', 'w')\n"
            "time.sleep(30)\n")
        self.assertEqual(self.guardian(code, limit=31), 124)
        self.assertTrue(self.outcome()["deadline"])
        self.assertEqual(self.outcome()["closure"], "closed")
        self.assertIsNone(self.outcome()["fault"])
        self.assertTrue((self.dir / "ran").exists())
        self.assertGreaterEqual(self.receipt["admission_waiting"]["ignored"], 1,
                                "the stale waiting file was not rejected")
        self.assertIn("stale run", self.receipt["admission_waiting"]["rejections"])
        decisions = [r for r in self.receipt["custodian"] if r["event"] == "admission_decision"]
        self.assertEqual(len(decisions), 1)
        self.assertEqual(decisions[0]["granted_ns"], 0, "a zero-duration grant moves no clock")
        pairs = decisions[0]["admission_waiting"]
        self.assertEqual((pairs["commands"], pairs["ended"], pairs["unpaired_count"]), (1, 1, 0),
                         "the zero-duration grant ends its command's wait")
        self.assertEqual(pairs["possibly_overwritten_count"], 0)
        self.assertEqual(decisions[0]["unclassified"], 0)

    def test_a_malformed_admission_marker_changes_nothing(self):
        # Cooperative evidence only: a rejected marker is recorded and the run
        # keeps its unextended deadline.
        code = fixture(
            "import json\n"
            "path = os.path.join(os.environ['OXIDE_PROOF_EVIDENCE'], " + repr(bounded.ADMISSION_NAME) + ")\n"
            "with open(path, 'w') as handle:\n"
            "    json.dump({'schema': 2, 'source': 'fixture', 'waited_ns': 10 * 1000000000}, handle)\n"
            "time.sleep(1)\n"
            "open(os.environ['CASE_DIR'] + '/ran', 'w')\n")
        # The limit must leave the fixture a real execution window: 32 s put the
        # stop only 12 s in, so a loaded host could end the run before the
        # fixture finished and the failure would say nothing about the marker.
        code = self.guardian(code, limit=45)
        # Read the marker's no-effect invariant from the receipt first: a guard
        # shutdown fault must never be confused with an admission effect, and an
        # admission effect must never hide behind a closed exit.
        self.assertEqual(self.outcome()["closure"], "closed")
        self.assertIsNone(self.outcome()["fault"])
        self.assertFalse(self.outcome()["deadline"])
        self.assertTrue((self.dir / "ran").exists())
        self.assertEqual(self.receipt["admission"]["observed"], 0)
        self.assertGreaterEqual(self.receipt["admission"]["ignored"], 1)
        self.assertIn("unexpected keys", self.receipt["admission"]["rejections"])
        decisions = [r for r in self.receipt["custodian"] if r["event"] == "admission_decision"]
        self.assertEqual(decisions[0]["counted"], 0)
        self.assertEqual(decisions[0]["granted_ns"], 0)
        self.assertEqual(code, 0, "the malformed marker had no effect (closed, no fault, no grant),"
                                  " so a non-zero guard exit is a guard-side fault, not this case's")

    def test_a_slow_setup_inside_its_deadline_launches_and_closes(self):
        # Oxide#197 (inherited from #192, decision 7bb1089d): the end-to-end
        # regression for the sampling-based freshness clock. The injected cold
        # setup takes longer than the 1 s reserve window but stays inside the 10 s
        # setup deadline, so no reading exists when the superseded custodian-start
        # basis would have declared the reserve stale. The corrected clock starts
        # when sampling starts, so the run must launch through the gate, execute
        # its child and close. The separate controls live in PureFreshnessClock:
        # no reading within 1 s AFTER sampling began still latches, and a setup
        # stall beyond its deadline launches nothing.
        code = fixture("open(os.environ['CASE_DIR'] + '/ran', 'w').write('1')\n")
        self.assertEqual(self.guardian(code, inject=["setup-delay"], limit=45), 0)
        self.assertEqual(self.outcome()["closure"], "closed")
        self.assertIsNone(self.outcome()["fault"])
        self.assertFalse(self.outcome()["deadline"])
        self.assertTrue((self.dir / "ran").exists(), "the child never launched")
        reserve = self.receipt["reserve"]
        first_gap = reserve["first_ns"] - self.receipt["start_ns"]
        # The fixture really is discriminating: the first completed reserve reading
        # arrived after the superseded rule's 1 s window measured from custodian
        # start, which is exactly when the old basis latched a stale reserve fault,
        # while the corrected basis measures from when sampling began.
        self.assertGreater(first_gap, bounded.RESERVE_STALE_NS,
                           "the injected setup did not exceed the superseded window")
        self.assertLess(first_gap, bounded.SETUP_S * bounded.NS,
                        "the injected setup left its own deadline")
        self.assertGreaterEqual(first_gap, int(bounded.SETUP_DELAY_S * 0.9 * bounded.NS))
        self.assertTrue(reserve["samples"] >= 1, reserve)

    def test_owned_scratch_cleanup(self):
        self.assertEqual(self.guardian(fixture("open(os.environ['TMPDIR'] + '/w', 'wb').write(bytes(65536))\n"),
                                       owned=True), 0)
        roots = list(self.dir.glob("oxide-proof.*"))
        self.assertEqual(len(roots), 1)
        self.assertFalse((roots[0] / "scratch").exists())
        self.assertEqual(self.receipt["phase"], "monitor_complete: pending_custodian")

    def test_owned_paths_past_the_limit_are_counted_from_the_first_reading(self):
        # The case directory is the monitored root, so a path at and past the
        # platform limit exists before the first scratch reading. Before
        # oxide#142 that reading faulted with ENAMETOOLONG, so the run ended
        # below the launch gate and this command never ran.
        referent = Path(tempfile.mkdtemp(prefix="referent.", dir=str(SCRATCH)))
        self.addCleanup(shutil.rmtree, str(referent), True)
        (referent / "big.bin").write_bytes(bytes(1024 * 1024))
        fixture_root = str(self.dir / "fixture")
        long_fixture(fixture_root, str(referent / "big.bin"))
        self.addCleanup(shutil.rmtree, fixture_root, True)
        expected = oracle_bytes(fixture_root)
        self.assertEqual(self.guardian(fixture("open(os.environ['CASE_DIR'] + '/ran', 'w')\n")), 0)
        self.assertEqual(self.outcome()["closure"], "closed")
        self.assertIsNone(self.outcome()["fault"])
        self.assertTrue((self.dir / "ran").exists(), "the command ran")
        self.assertGreaterEqual(self.receipt["owned_bytes"]["samples"], 1)
        self.assertGreaterEqual(self.receipt["owned_bytes"]["maximum"], expected,
                                "the overlong fixture was never counted")
        self.assertEqual(self.receipt["phase"], "monitor_complete: pending_custodian")

    def test_nonzero_exit(self):
        self.assertEqual(self.guardian(fixture("raise SystemExit(3)\n")), 3)
        self.assertEqual(self.outcome()["closure"], "closed")

    def test_exec_failure_127(self):
        self.assertEqual(self.guardian(None, command=[str(self.dir / "missing-command")]), 127)
        self.assertEqual(self.outcome()["status"], 127)

    def test_cascading_group_closes_at_deadline(self):
        code = fixture("import os\n" + spawn_line(SLEEPER) +
                       "signal.signal(signal.SIGTERM, lambda *_: os.killpg(child.pid, signal.SIGTERM))\nchild.wait()\n")
        self.assertEqual(self.guardian(code), 124)
        self.assertEqual(self.outcome()["closure"], "closed")
        self.assertFalse(alive(self.child()))

    def test_separate_group_survivor_unclosed_never_signalled(self):
        code = fixture("import os\n" + spawn_line(TERM_IGNORING))
        self.assertEqual(self.guardian(code), 125)
        self.assertEqual(self.outcome()["closure"], "UNCLOSED")
        self.assertTrue(alive(self.child()), "survivor must still be alive at the verdict")
        targets = {r["target"] for r in self.receipt["custodian"] if r["event"] == "signal"}
        self.assertNotIn(self.child(), targets)
        self.assertGreaterEqual(self.receipt["census"]["members"], 1)
        self.assert_kept_outside_outer_cleanup()

    def test_cargo_shaped_group_survivor_is_recorded_and_never_signalled(self):
        # oxide#195 regression, the oxide#182 shape under Tom's 2026-10-31 ruling
        # (c096c8fc): a Cargo-shaped process group leads its own group in the
        # leader's cooperative session and the leader reaches the deadline. The
        # pinned leader-group KILL cannot reach it, and the supervisor must NOT
        # signal it either - it is not this run's child, so it is not held
        # unreaped and a checked-then-signalled PID could be recycled. The run is
        # honestly UNCLOSED, the survivor's exact identity is recorded, and the
        # process is still alive at the verdict.
        self.assertEqual(self.guardian(CARGO_LEADER), 124)
        outcome = self.outcome()
        self.assertEqual(outcome["closure"], "UNCLOSED")
        self.assertTrue(outcome["deadline"])
        self.assertIn(CENSUS_FAULT, outcome["fault"])
        census = self.receipt["census"]
        self.assertGreaterEqual(census["members"], 2, census)
        cargo = int((self.dir / "cargo.pid").read_text())
        test = int((self.dir / "test.pid").read_text())
        # No signal may name either survivor, and no non-held session signal exists.
        signalled = {record["target"] for record in self.receipt["custodian"]
                     if record["event"] == "signal"}
        self.assertNotIn(cargo, signalled)
        self.assertNotIn(test, signalled)
        self.assertEqual([record for record in self.receipt["custodian"]
                          if str(record.get("event", "")).startswith("session_")], [])
        # The exact identity is recorded for the owner, and it is still running.
        recorded = {(row["pid"], row["start"]) for row in self.receipt["survivors"]}
        member_pids = {row.get("pid") for row in (census.get("member_rows") or [])}
        for pid in (cargo, test):
            row = bounded.Kinfo.one(pid)
            self.assertIsNotNone(row, f"survivor {pid} left the process table")
            self.assertIs(row.get("zombie"), False, f"survivor {pid} is not live")
            self.assertIn((pid, row["start"]), recorded,
                          "the survivor's exact identity was not recorded")
            self.assertIn(pid, member_pids, "the survivor was not the census's own member row")
        self.assert_kept_outside_outer_cleanup()

    def test_a_transient_member_records_a_row_and_the_run_closes(self):
        # Reviewer finding: the transient case must be behavioural. A member that
        # leads its own group in the leader's session outlives the deadline KILL
        # and then exits by itself, so the census records its exact row and a
        # later certainly-empty round closes the run. The summary must say the
        # run closed, never that it stays UNCLOSED or kept a scratch, and no
        # non-held identity may be signalled.
        self.assertEqual(self.guardian(TRANSIENT_LEADER), 124)
        outcome = self.outcome()
        self.assertEqual(outcome["closure"], "closed")
        self.assertIsNone(outcome["fault"])
        self.assertTrue(outcome["deadline"])
        census = self.receipt["census"]
        self.assertEqual((census["certain"], census["members"], census["uncertain"]),
                         (True, 0, 0), census)
        member = int((self.dir / "member.pid").read_text())
        recorded = [(item["pid"], item["start"], item["zombie"])
                    for item in self.receipt["survivors"]]
        self.assertTrue(recorded, "the transient member was never recorded")
        self.assertIn(member, [pid for pid, _start, _zombie in recorded])
        for pid, start, zombie in recorded:
            self.assertGreater(start, 0)
            self.assertIs(zombie, False)
        signalled = {record["target"] for record in self.receipt["custodian"]
                     if record["event"] == "signal"}
        self.assertNotIn(member, signalled)
        self.assertIsNone(bounded.Kinfo.one(member), "the transient member was still live")
        log = (self.dir / "guardian.log").read_text()
        survivor_line = [line for line in log.splitlines()
                         if "same-session survivor(s) not signalled" in line]
        self.assertEqual(len(survivor_line), 1, log)
        self.assertIn("gone by the deciding census, so the run closed", survivor_line[0])
        self.assertNotIn("UNCLOSED", survivor_line[0])
        self.assertNotIn("scratch is kept", survivor_line[0])
        self.assertFalse((self.dir / "oxide-proof.").exists())

    def test_escaped_pipe_holder_unclosed(self):
        # The escaped writer leaves the nested session, keeps the leader's
        # stdout/stderr write descriptors open, and proves it is still alive
        # after the verdict by answering a fresh nonce. Which closure veto
        # latched is not claimed: both settle refusals are accepted, and every
        # other outcome is refused.
        code = fixture("import os\n" + spawn_line(ESCAPED_WRITER, setsid=True))
        exit_code = self.guardian(code)
        wait_for(os.path.join(str(self.dir), "escaped.ready"), NONCE_TIMEOUT_S)
        child_pid = int((self.dir / "child.pid").read_text())
        token = secrets.token_hex(16)
        (self.dir / "nonce").write_text(token)
        wait_for(os.path.join(str(self.dir), "nonce.reply"), NONCE_TIMEOUT_S)
        reply = (self.dir / "nonce.reply").read_text().strip()
        signals = [r for r in self.receipt["custodian"] if r["event"] == "signal"]
        facts = {"leader_status": self.outcome().get("status"), "exit": exit_code,
                 "closure": self.outcome().get("closure"), "fault": self.outcome().get("fault"),
                 "deadline": self.outcome().get("deadline"), "ready": True,
                 "nonce_ok": reply == token, "child_pid": child_pid, "suite_pid": os.getpid(),
                 "signal_targets": [r["target"] for r in signals], "signal_pins": [r["pin"] for r in signals],
                 "session_targets": [r.get("target") for r in self.receipt["custodian"]
                                     if str(r.get("event", "")).startswith("session_")],
                 "retained": (self.dir / "evidence" / "receipt.json").exists()}
        CASES[-1]["nonce_ok"] = facts["nonce_ok"]
        self.assertEqual(escaped_writer_problems(facts), [], facts)
        self.assert_kept_outside_outer_cleanup()

    def held_stream_case(self, streams):
        # The nested leader's own command sends duplicates of its actual
        # stdout/stderr write descriptors to this suite process, which is not a
        # member of the nested session; the suite holds them through the
        # verdict and closes them in cleanups. A released pair must close.
        receiver = FdReceiver(str(self.dir))
        receiver.held_names = list(streams)
        self.addCleanup(receiver.close)
        extra = {"FD_SOCKET_DIR": receiver.directory, "FD_SOCKET_NAME": FD_SOCKET_NAME,
                 "FD_STREAMS": ",".join(streams), "FD_ACK_TIMEOUT_S": str(FD_ACK_TIMEOUT_S)}
        exit_code = self.guardian(FD_HANDOFF, extra_env=extra, receiver=receiver,
                                  receiver_count=len(streams))
        self.assertTrue((self.dir / "handoff.done").exists(), "the ACK never reached the fixture")
        held = [name for name in streams] if receiver.received else []
        signals = [r for r in self.receipt["custodian"] if r["event"] == "signal"]
        facts = {"leader_status": self.outcome().get("status"), "exit": exit_code,
                 "closure": self.outcome().get("closure"), "fault": self.outcome().get("fault"),
                 "deadline": self.outcome().get("deadline"), "census": self.receipt.get("census"),
                 "held": held, "expected_held": list(streams), "suite_pid": os.getpid(),
                 "signal_targets": [r["target"] for r in signals],
                 "retained": (self.dir / "evidence" / "receipt.json").exists()}
        return facts

    def test_held_stdout_descriptor_keeps_the_eof_veto(self):
        facts = self.held_stream_case(["stdout"])
        self.assertEqual(eof_veto_problems(facts), [], facts)
        self.assert_kept_outside_outer_cleanup()

    def test_held_stderr_descriptor_keeps_the_eof_veto(self):
        facts = self.held_stream_case(["stderr"])
        self.assertEqual(eof_veto_problems(facts), [], facts)
        self.assert_kept_outside_outer_cleanup()

    def test_released_descriptors_close_normally(self):
        # The paired control: both descriptors transferred and both released by
        # the suite before the verdict, so the same fixture and handoff must
        # close with status 0 and no fault. This refuses an always-UNCLOSED oracle.
        receiver = FdReceiver(str(self.dir))
        self.addCleanup(receiver.close)
        extra = {"FD_SOCKET_DIR": receiver.directory, "FD_SOCKET_NAME": FD_SOCKET_NAME,
                 "FD_STREAMS": "stdout,stderr", "FD_ACK_TIMEOUT_S": str(FD_ACK_TIMEOUT_S)}
        stream = _ReleasedReceiver(receiver)
        exit_code = self.guardian(FD_HANDOFF, extra_env=extra, receiver=stream, receiver_count=2)
        facts = {"exit": exit_code, "closure": self.outcome().get("closure"),
                 "fault": self.outcome().get("fault"), "held": stream.still_held()}
        self.assertEqual(released_streams_problems(facts), [], facts)



    def test_terminal_zombie_witness_closed(self):
        # Positive TERMINALNONWRITER fixture: this suite process forks one
        # direct child that exits at once and stays an unreaped zombie (no
        # extra setsid/setpgid) while a nested guardian runs a trivial
        # command. The nested terminal census must witness the exact held
        # (PID, start) as terminal and close with status 0 and no fault;
        # the suite reaps its own child in finally so nothing leaks.
        held = os.fork()
        if held == 0:
            os._exit(0)
        try:
            deadline = time.monotonic() + 5
            start_row = None
            while time.monotonic() < deadline:
                start_row = bounded.Kinfo.one(held)
                if start_row is not None and start_row.get("zombie") is True:
                    break
                time.sleep(0.01)
            self.assertIsNotNone(start_row, "held child left the process table")
            self.assertTrue(start_row.get("zombie"), "held child is not yet a zombie")
            held_start = start_row["start"]
            self.assertEqual(
                self.guardian(fixture("open(os.environ['CASE_DIR'] + '/ran', 'w')\n")), 0)
            self.assertEqual(self.outcome()["closure"], "closed")
            self.assertIsNone(self.outcome()["fault"])
            nested = self.receipt["census"]
            self.assertEqual(self.receipt.get("terminal_mode", {}).get("mode"), "allowlisted")
            self.assertGreaterEqual(nested["terminal_nonwriters"], 1)
            self.assertEqual(nested["terminal_count_status"], "valid")
            witnesses = nested.get("terminal_witnesses", [])
            self.assertTrue(
                any(w.get("pid") == held and w.get("start") == held_start for w in witnesses),
                f"exact held (PID, start) witness missing for {held}")
        finally:
            try:
                os.waitpid(held, 0)
            except OSError:
                pass

    def test_twelve_held_zombies_share_one_confirmation_budget(self):
        # Oxide#147 correction, Real coverage: this suite process holds twelve
        # real exited/unreaped children while a nested guardian runs a trivial
        # command. The nested round must stay inside the unchanged 10 s grace
        # with at most 19 SHARED confirmation pauses (the per-identity budget,
        # not twelve times it), still witness the held identities as terminal
        # nonwriters, and close with status 0 and no fault. The children are
        # reaped in finally, so the fixture leaves nothing behind; a failure
        # keeps the nested case directory under the ordinary unknown-resource
        # retention and is never relabelled as closed.
        held = []
        try:
            for _ in range(12):
                pid = os.fork()
                if pid == 0:
                    os._exit(0)
                held.append(pid)
            deadline = time.monotonic() + 5
            starts = {}
            while time.monotonic() < deadline and len(starts) < 12:
                for pid in held:
                    struck = bounded.Kinfo.one(pid)
                    if struck is not None and struck.get("zombie") is True:
                        starts[pid] = struck["start"]
                if len(starts) < 12:
                    time.sleep(0.01)
            self.assertEqual(len(starts), 12, "not every held child became an unreaped zombie")
            self.assertEqual(
                self.guardian(fixture("open(os.environ['CASE_DIR'] + '/ran', 'w')\n")), 0)
            outcome = self.outcome()
            self.assertEqual(outcome["closure"], "closed")
            self.assertIsNone(outcome["fault"])
            self.assertFalse(outcome["deadline"])
            self.assertGreaterEqual(self.receipt["census_rounds"], 1)
            settlement = self.receipt.get("settlement") or {}
            self.assertGreaterEqual(settlement.get("rounds_completed", 0), 1,
                                    "a completed post-reap round is required, not a missing one")
            rounds = settlement.get("rounds") or []
            self.assertTrue(rounds, "the round record must be present")
            self.assertLessEqual(rounds[0].get("pause_calls"), bounded.CONFIRM_TRIES - 1,
                                 "shared pauses must stay within one per-identity read budget")
            nested = self.receipt["census"]
            self.assertEqual(self.receipt.get("terminal_mode", {}).get("mode"), "allowlisted")
            self.assertEqual(nested["terminal_count_status"], "valid")
            self.assertGreaterEqual(nested["terminal_nonwriters"], 12)
            # The global host census may legitimately retain ambient allowlisted
            # terminal rows. They are observations, never foreign writers, and
            # they can never stand in for an identity this fixture holds. The
            # witness list is byte-budget clipped, so report exactly which owned
            # identities were witnessed and which were clipped away; a clipped
            # list with no owned witness fails rather than being relaxed.
            report = owned_witness_report(nested.get("terminal_witnesses"), starts)
            self.assertEqual(report["problems"], [])
            self.assertTrue(report["owned"], "a nonvacuous exact owned witness is required")
            print("[oxide#147] held=%d witnessed=%s omitted=%s ambient=%d"
                  % (len(starts), sorted(report["owned"]), report["omitted"], len(report["ambient"])))
        finally:
            for pid in held:
                try:
                    os.waitpid(pid, 0)
                except OSError:
                    pass

    def test_live_real_child_injected_error_blocks(self):
        # Contrast: a genuinely LIVE owned child whose session lookup is
        # explicitly INJECTED to fail (not a naturally failing getsid) can
        # never qualify as terminal (not ESRCH, zombie False). Uses the
        # existing Census DI callbacks with real Kinfo.one against the real
        # child PID; the injected error is reported, not natural.
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        try:
            live = bounded.Kinfo.one(child.pid)
            self.assertIsNotNone(live, "owned child left the process table")
            self.assertFalse(live.get("zombie"), "owned child is unexpectedly a zombie")
            live_rows = [dict(live)]
            def injected_getsid(_pid):
                raise PermissionError(1, "injected session error")
            check = bounded.Census(100, snapshot=lambda: list(live_rows),
                                   getsid=injected_getsid, reread=bounded.Kinfo.one,
                                   pause=lambda _seconds: None, terminal_allowed=True)
            result = check.round()
            # Blocking uncertainty with an accountable count: valid 0, since
            # a live row with an explicitly injected non-ESRCH failure was
            # never a terminal candidate.
            self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
            self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                             ("valid", 0))
            self.assertNotIn("terminal_witnesses", result)
        finally:
            child.kill()
            child.wait(timeout=10)

    def test_output_saturation(self):
        code = fixture("block = b'x' * 65536\nfor _ in range(48 * 16): os.write(1, block)\n"
                       "for _ in range(10 * 16): os.write(2, block)\n")
        self.assertEqual(self.guardian(code), 0)
        out, err = self.receipt["output"]["stdout"], self.receipt["output"]["stderr"]
        self.assertEqual((out["bytes"], err["bytes"]), (48 << 20, 10 << 20))
        self.assertEqual((out["persisted"], err["persisted"]), (8 << 20, 8 << 20))
        self.assertGreater(out["discarded"], 0)

    # Faults and signals

    def test_latched_fault_then_empty_census_keeps(self):
        self.assertEqual(self.guardian(SLEEPER, inject=["reserve-low-after"]), 125)
        self.assertIn("below", self.outcome()["fault"])
        self.assertEqual(self.receipt["phase"], "monitor_complete: pending_custodian")
        self.assertEqual(self.receipt["census"]["members"], 0)
        self.assertEqual(self.outcome()["closure"], "UNCLOSED")

    def test_sigterm_to_guardian(self):
        code = fixture("open(os.environ['CASE_DIR'] + '/ran', 'w')\n"
                       "signal.signal(signal.SIGTERM, signal.SIG_IGN)\ntime.sleep(3600)\n")
        self.assertEqual(self.guardian(code, signal_after="ran", signal_number=signal.SIGTERM), 125)
        self.assertIn("cancelled", self.outcome()["fault"])
        sent = [(r["signal"], r["result"]) for r in self.receipt["custodian"] if r["event"] == "signal"]
        self.assertEqual(sent, [("SIGTERM", "sent"), ("SIGKILL", "sent")])

    def test_sigint_to_guardian(self):
        code = fixture("open(os.environ['CASE_DIR'] + '/ran', 'w')\ntime.sleep(3600)\n")
        self.assertEqual(self.guardian(code, signal_after="ran", signal_number=signal.SIGINT), 125)
        self.assertIn("cancelled", self.outcome()["fault"])

    def gate_case(self, inject):
        self.assertEqual(self.guardian(fixture("open(os.environ['CASE_DIR'] + '/ran', 'w')\n"), inject=[inject]), 125)
        self.assert_not_launched()
        kills = [r for r in self.receipt["custodian"] if r["event"] == "signal"]
        self.assertEqual([(k["signal"], k["killpg"], k["pin"]) for k in kills], [("SIGKILL", False, "leader")])
        self.assertEqual(self.outcome()["closure"], "UNCLOSED")

    def test_gate_ack_eof(self):
        self.gate_case("ack-eof")

    def test_gate_ack_wrong_byte(self):
        self.gate_case("ack-wrong")

    def test_cancel_before_ack(self):
        self.gate_case("cancel-before-ack")

    def test_no_detector(self):
        self.gate_case("no-detector")

    def test_cancel_after_ack_before_gate(self):
        # Review 6312725112 finding 5: a real SIGTERM after detector
        # registration, before the gate write.
        self.gate_case("cancel-after-ack")
        self.assertIn("signal", self.outcome()["fault"])

    def test_cancel_pending_at_gate(self):
        # A SIGTERM arriving while the gate decision holds INT and TERM blocked.
        self.gate_case("cancel-at-gate")
        self.assertIn("signal", self.outcome()["fault"])

    def test_setup_stall_launches_nothing(self):
        self.assertEqual(self.guardian(fixture("open(os.environ['CASE_DIR'] + '/ran', 'w')\n"),
                                       inject=["setup-stall"]), 125)
        self.assert_not_launched()
        self.assertEqual(self.receipt["phase"], "started: incomplete")

    def test_monitor_death(self):
        self.assertEqual(self.guardian(SLEEPER, inject=["monitor-die"]), 125)
        self.assertNotEqual(self.receipt["phase"], "monitor_complete: pending_custodian")

    def test_monitor_stall_goes_stale(self):
        self.assertNotEqual(self.guardian(fixture("pass\n"), inject=["monitor-stall"]), 0)
        self.assertNotEqual(self.receipt["phase"], "monitor_complete: pending_custodian")

    def test_census_uncertain_is_unclosed(self):
        self.assertEqual(self.guardian(fixture("pass\n"), inject=["census-uncertain"]), 125)
        self.assertEqual(self.outcome()["closure"], "UNCLOSED")

    def test_uncertain_then_empty_census_keeps_scratch(self):
        # Review 6312725112 finding 3: only the first census is uncertain;
        # later certain empty censuses must not restore closure or cleanup.
        self.assertEqual(self.guardian(fixture("pass\n"), inject=["census-uncertain-first"], owned=True), 125)
        self.assertEqual(self.outcome()["closure"], "UNCLOSED")
        self.assertIn("census uncertain", self.outcome()["fault"])
        [root] = list(self.dir.glob("oxide-proof.*"))
        self.assertTrue((root / "scratch").is_dir(), "scratch removed after an uncertain census")
        self.assertEqual(self.receipt["phase"], "monitor_complete: pending_custodian")

    def test_cleanup_interrupted(self):
        self.assertEqual(self.guardian(fixture("pass\n"), inject=["cleanup-interrupt"], owned=True), 125)
        self.assertEqual(self.receipt["phase"], "cleanup_partial_or_unknown")

    def test_monitor_complete_then_custodian_timeout(self):
        self.assertEqual(self.guardian(fixture("pass\n"), inject=["custodian-timeout-after-monitor"]), 124)
        self.assertEqual(self.receipt["phase"], "monitor_complete: pending_custodian")

    # Post-gate exceptions (review 6318598212)

    def exception_case(self, inject, body, fault, where=None):
        """A TERM-ignoring leader with a TERM-ignoring child in its group: the
        injected exception must still reach the bounded pinned stop (TERM,
        then KILL of the group), WNOHANG reap, census and keep."""
        self.assertEqual(self.guardian(fixture(body), inject=[inject], owned=True), 125)
        outcome = self.outcome()
        self.assertEqual(outcome["closure"], "UNCLOSED")
        self.assertIn(fault, outcome["fault"])
        self.assertEqual(self.receipt["phase"], "monitor_complete: pending_custodian")
        records = self.receipt["custodian"]
        if where:  # the fault carries the class name only; the record carries the location
            self.assertTrue(any(r["event"] == "exception" and r["where"].startswith(where) for r in records),
                            [r for r in records if r["event"] == "exception"])
        sent = [(r["signal"], r["killpg"], r["result"]) for r in records if r["event"] == "signal"]
        self.assertIn(("SIGKILL", True, "sent"), sent)
        self.assertFalse(alive(self.child()), "the group member survived the pinned KILL")
        reaped = [r for r in records if r["event"] == "leader_reaped"]
        self.assertTrue(reaped and reaped[0]["status"] is not None, "leader not reaped")
        # Sampling stays fresh through shutdown: the last reserve sample is
        # within the guard's 1 s staleness window of the monitor's final
        # publication. (Shutdown can finish inside one 0.25 s interval, so a
        # sample after the reap itself is not required; native 74f1b65.)
        age = self.receipt["published_ns"] - self.receipt["reserve"]["last_ns"]
        self.assertGreaterEqual(age, 0, "reserve sample stamped after the final publication")
        self.assertLess(age, bounded.RESERVE_STALE_NS, "reserve sampling went stale before the shutdown ended")
        [root] = list(self.dir.glob("oxide-proof.*"))
        self.assertTrue((root / "scratch").is_dir(), "scratch removed after an exception")

    GROUP = ("import subprocess, sys\nsignal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
             "child = subprocess.Popen([sys.executable, '-c', {child!r}])\n"
             "open(os.environ['CASE_DIR'] + '/child.pid', 'w').write(str(child.pid))\n").format(child=TERM_IGNORING)

    def test_event_loop_exception_stops_pinned_group(self):
        self.exception_case("pump-exception", self.GROUP + "time.sleep(3600)\n", "custodian exception: RuntimeError")

    def test_kqueue_exception_stops_pinned_group(self):
        self.exception_case("kqueue-exception", self.GROUP + "time.sleep(3600)\n", "custodian exception: OSError")

    def test_malformed_message_stops_pinned_group(self):
        self.exception_case("malformed-message", self.GROUP + "time.sleep(3600)\n", "protocol: malformed missing")

    def test_exception_during_stop_still_kills(self):
        # The leader exits; the exception lands after TERM, and KILL still follows.
        self.exception_case("stop-exception", self.GROUP + "time.sleep(1)\n", "custodian exception: RuntimeError",
                            where="stop_leader:")

    # Freshness from completion and direct setup bound (review 6320463419)

    def delay_stall_case(self, inject, fault, stale_ns):
        self.assertEqual(self.guardian(SLEEPER, inject=[inject]), 125)
        self.assertIn(fault, self.outcome()["fault"])
        kind = "reserve" if inject.startswith("reserve") else "owned_bytes"
        completed = self.receipt[kind]["last_ns"]  # the stalled sampler's last completion
        stop = [r["at"] for r in self.receipt["custodian"] if r["event"] == "stop"][0]
        latency = stop - completed
        CASES[-1]["stale_latency_ns"] = latency
        # The fault lands at the original bound measured from completion, not
        # from the delayed publication (which would add the injected delay).
        self.assertGreaterEqual(latency, stale_ns)
        self.assertLess(latency, stale_ns + NS * 3 // 10, latency)

    def test_reserve_delayed_publication_then_stall(self):
        self.delay_stall_case("reserve-delay-stall", "reserve readings stale", bounded.RESERVE_STALE_NS)

    def test_scratch_delayed_publication_then_stall(self):
        self.delay_stall_case("scratch-delay-stall", "scratch readings stale", bounded.SCRATCH_STALE_NS)

    def test_direct_runner_setup_blocked_in_filesystem(self):
        # A real blocking filesystem call in setup: the evidence directory's
        # receipt.json.tmp is a FIFO, so the first receipt open blocks with
        # no reader. The direct invocation (no bootstrap) must stop at the
        # setup deadline, launch nothing and claim no receipt.
        (self.dir / "evidence").mkdir()
        os.mkfifo(self.dir / "evidence" / "receipt.json.tmp")
        env = dict(os.environ, CASE_DIR=str(self.dir), FIXTURE_DEADLINE_NS=str(self.started + FALLBACK_S * NS),
                   OXIDE_TASK_SCRATCH_ROOT=str(self.dir), OXIDE_SUPERVISOR_INJECT="")
        command = [sys.executable, "-c", fixture("open(os.environ['CASE_DIR'] + '/ran', 'w')\n")]
        with open(self.dir / "guardian.log", "wb") as log:
            guardian = subprocess.Popen([sys.executable, str(SUPERVISOR), "--limit", "35", "--evidence",
                                         str(self.dir / "evidence"), "--"] + command, env=env,
                                        stdout=subprocess.DEVNULL, stderr=log)
            try:
                self.code = guardian.wait(timeout=VERDICT_S - (now_ns() - self.started) / NS)
            finally:
                if guardian.poll() is None:
                    guardian.kill()
                    guardian.wait(5)
        wall = (now_ns() - self.started) / NS
        CASES.append({"case": "direct_runner_setup_blocked", "exit": self.code, "wall_s": round(wall, 3),
                      "kept": str(self.dir)})
        self.assertEqual(self.code, 125)
        # Bounded inside the setup window. No reading arrives while setup is
        # blocked, so the 1 s reserve staleness guard usually stops it first
        # (native f3bde11: 1.13 s); the 10 s setup deadline is the backstop.
        self.assertLess(wall, bounded.SETUP_S + 3)
        self.assert_not_launched()
        self.assertFalse((self.dir / "evidence" / "receipt.json").exists(), "a receipt was claimed")

    # Bootstrap finalization

    def test_bootstrap_deadline_bounds_blocked_stdout(self):
        # Review 6312725112 finding 2: a worker blocked writing to a stdout
        # nobody reads cannot extend the bootstrap's absolute deadline.
        code = PROLOGUE + f"""
import importlib.util, sys
spec = importlib.util.spec_from_file_location("qualify", {str(BOOTSTRAP)!r})
qualify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qualify)

def work():
    # Timers do not survive fork: re-arm the fallback in the worker.
    signal.setitimer(signal.ITIMER_REAL, max(0.01, (int(os.environ["FIXTURE_DEADLINE_NS"])
                                                    - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9))
    open(os.environ["CASE_DIR"] + "/worker.pid", "w").write(str(os.getpid()))
    os.write(1, bytes(1 << 20))  # blocks: the pipe is never read
    return 0

start = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
sys.exit(qualify.bounded(work, start + 2 * 10**9, start + 3 * 10**9))
"""
        env = dict(os.environ, CASE_DIR=str(self.dir), FIXTURE_DEADLINE_NS=str(self.started + FALLBACK_S * NS))
        with open(self.dir / "actor.log", "wb") as log:
            actor = subprocess.Popen([sys.executable, "-c", code], env=env, stdout=subprocess.PIPE, stderr=log)
            try:
                self.code = actor.wait(timeout=15)
            finally:
                if actor.poll() is None:
                    actor.kill()
                    actor.wait(5)
                actor.stdout.close()
        wall = (now_ns() - self.started) / NS
        self.assertLess(now_ns(), self.started + FALLBACK_S * NS, "verdict after the fixture fallback")
        CASES.append({"case": "bootstrap_deadline_blocked_stdout", "exit": self.code, "wall_s": round(wall, 3)})
        self.assertEqual(self.code, 124)
        self.assertLess(wall, 6)
        self.assertIn(b"KILLed (reaped True)", (self.dir / "actor.log").read_bytes())
        self.assertFalse(alive(int((self.dir / "worker.pid").read_text())))

    # Fixture fallback

    def test_fixture_deadline_holds_across_exec_and_late_grandchild(self):
        env = dict(os.environ, FIXTURE_DEADLINE_NS=str(now_ns() + 2 * NS))
        inner = PROLOGUE + "time.sleep(3600)\n"
        outer = PROLOGUE + f"import sys\nos.execv(sys.executable, [sys.executable, '-c', {inner!r}])\n"

        def inherited():
            signal.signal(signal.SIGALRM, signal.SIG_IGN)
            signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGALRM})

        start = time.monotonic()
        result = subprocess.run([sys.executable, "-c", outer], env=env, preexec_fn=inherited, timeout=20)
        self.assertEqual(result.returncode, -signal.SIGALRM)
        self.assertLess(time.monotonic() - start, 10)
        late = subprocess.run([sys.executable, "-c", inner], env=env, preexec_fn=inherited, timeout=20)
        self.assertEqual(late.returncode, -signal.SIGALRM)
        CASES.append({"case": "fixture_deadline", "exit": [result.returncode, late.returncode]})


# Post-census detach (oxide#191): the runner detaches, at most once, the exact
# image a proof publishes a request for -- and only after its own certain empty
# census. These are the only cases here that attach a real volume; each uses a
# tiny 8 MiB sparse image in the case's own evidence directory, attaches it with
# `-owners on`, and (for the two refusal cases) removes its own fixture after the
# nested guard has exited.
DETACH_CASE = fixture("""
import json, os, plistlib, re, subprocess, sys
case = os.environ["CASE_DIR"]
evidence = os.environ["OXIDE_PROOF_EVIDENCE"]
token = os.environ["CASE_TOKEN"]
# "request": the image lives in the evidence directory and a request is
# published. "unrequested": the image lives in the guard's own scratch and no
# request is published, so the fail-closed pre-cleanup scan must keep the root.
mode = os.environ.get("CASE_DETACH_MODE", "request")
mount = "/Volumes/OxD" + token[:8]
image = os.path.join(evidence if mode == "request" else os.environ["OXIDE_TASK_SCRATCH_ROOT"],
                     "detach.sparseimage")
def run(*argv, timeout=90):
    return subprocess.run(list(argv), stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, timeout=timeout)
created = run("/usr/bin/hdiutil", "create", "-size", "8m", "-fs", "APFS", "-volname", "OxD" + token[:8],
              "-type", "SPARSE", image)
if created.returncode != 0:
    print("image create failed: " + created.stderr.decode(errors="replace"), file=sys.stderr)
    raise SystemExit(3)
# The opt-in custody indication is published ATOMICALLY BEFORE the attach, so an
# ambiguous attach, a failed parse or a wrong UUID below still leaves the guard
# able to keep this root instead of cleaning a live backing.
_custody = os.path.join(evidence, "image-custody.json")
with open(_custody + ".tmp", "w") as handle:
    json.dump({"version": 1, "backings": [os.path.realpath(image)]}, handle)
os.replace(_custody + ".tmp", _custody)
if not os.path.isfile(_custody):
    print("custody indication was not published before attach", file=sys.stderr)
    raise SystemExit(3)
attached = run("/usr/bin/hdiutil", "attach", "-owners", "on", "-nobrowse", "-noverify", "-plist", image)
if attached.returncode != 0:
    print("attach failed: " + attached.stderr.decode(errors="replace"), file=sys.stderr)
    raise SystemExit(3)
entities = plistlib.loads(attached.stdout)["system-entities"]
wholes = [entity["dev-entry"] for entity in entities
          if re.fullmatch(r"/dev/disk\d+", entity.get("dev-entry", ""))]
mounts = [entity["mount-point"] for entity in entities if entity.get("mount-point")]
# An APFS image attaches as a container whole disk PLUS a synthesized whole disk,
# with the volume mounted from a slice of one of them. Any number of whole-disk
# entities is fine; exactly one mounted entity is not.
mounted = [entity for entity in entities if entity.get("mount-point") == mount]
if not wholes or mounts != [mount] or len(mounted) != 1 or not mounted[0].get("dev-entry"):
    print("unexpected attachment " + repr((wholes, mounts)), file=sys.stderr)
    raise SystemExit(3)
# The detach request names the MOUNTED entity's own device: a whole-disk name can
# be the container, which is not what the mount is bound to.
device = mounted[0]["dev-entry"]
uuid = plistlib.loads(run("/usr/sbin/diskutil", "info", "-plist", mount).stdout)["VolumeUUID"]
if mode == "request":
    request = {"version": 1, "backing": os.path.realpath(image), "mount": mount, "device": device,
               "volume_uuid": uuid, "note": "supervisor suite detach case"}
    request.update(json.loads(os.environ.get("CASE_DETACH_OVERRIDE") or "{}"))
    temporary = os.path.join(evidence, "detach-request.json.tmp")
    with open(temporary, "w") as handle:
        json.dump(request, handle)
    os.replace(temporary, os.path.join(evidence, "detach-request.json"))

with open(os.path.join(case, "volume.json"), "w") as handle:
    json.dump({"mount": mount, "device": device, "uuid": uuid, "image": image,
               "wholes": wholes}, handle)
print("detach fixture ready: " + mount + " " + device + " wholes " + repr(wholes))
""")


# The nested detach guard runs `--limit 60`: its own stop is at 40 s and its
# exit at 58 s. This class's outer window must cover both plus a bounded settle:
# killing it at the Real class's 45 s verdict would decide nothing and would
# leave its mount and custody unknown. A timeout therefore retains the resources
# and fails as unknown, never as a detach outcome.
DETACH_CASE_VERDICT_S = 75


class DetachReal(unittest.TestCase):
    """The three real post-census detach cases. Each one owns its image, keeps
    its evidence, and removes its own fixture mount after the nested guard has
    exited (a successful case finds it already gone)."""

    def setUp(self):
        self.dir = Path(tempfile.mkdtemp(prefix="detach_case.", dir=nested_root())).resolve()
        self.started = now_ns()
        self.token = secrets.token_hex(4)

    def guard(self, override=None, inject=(), mode="request"):
        extra = {"CASE_TOKEN": self.token, "CASE_DETACH_MODE": mode}
        if override is not None:
            extra["CASE_DETACH_OVERRIDE"] = json.dumps(override)
        env = dict(os.environ, CASE_DIR=str(self.dir), CASE_TOKEN=self.token,
                   FIXTURE_DEADLINE_NS=str(self.started + FALLBACK_S * NS),
                   OXIDE_SUPERVISOR_INJECT=",".join(inject))
        # An inherited root would make the nested guard adopt it, and an adopted
        # root owns no image to detach: this case needs the owned `--root-parent`.
        env.pop("OXIDE_TASK_SCRATCH_ROOT", None)
        env.update(extra)
        args = [sys.executable, str(SUPERVISOR), "--limit", "60", "--root-parent", str(self.dir),
                "--evidence", str(self.dir / "evidence"), "--", sys.executable, "-c", DETACH_CASE]
        with open(self.dir / "guardian.log", "wb") as log:
            guardian = subprocess.Popen(args, env=env, stdout=subprocess.DEVNULL, stderr=log)
            try:
                self.code = guardian.wait(timeout=DETACH_CASE_VERDICT_S - (now_ns() - self.started) / NS)
            except subprocess.TimeoutExpired:
                if guardian.poll() is None:
                    guardian.kill()
                    guardian.wait(5)
                self.fail("the nested detach guard did not settle inside its outer window"
                          f" ({DETACH_CASE_VERDICT_S} s); its mount, backing and evidence are retained"
                          " and no detach outcome is inferred from a forced kill")
            finally:
                if guardian.poll() is None:
                    guardian.kill()
                    guardian.wait(5)
        self.receipt = json.loads((self.dir / "evidence" / "receipt.json").read_text())
        volume_path = self.dir / "volume.json"
        if not volume_path.exists():
            # The fixture publishes its own attachment facts; without them no
            # detach oracle can be evaluated, and saying so is truthful (a
            # missing volume identity is not a detach outcome).
            self.fail(f"the detach fixture never published its attachment facts (guard exit {self.code},"
                      f" outcome {self.receipt.get('outcome')}); no detach oracle can be evaluated")
        self.volume = json.loads(volume_path.read_text())
        CASES.append({"case": self._testMethodName, "exit": self.code, "phase": self.receipt["phase"],
                      "dir": str(self.dir), "detach": self.receipt.get("detach"),
                      # The request-free keep is recorded under `cleanup`, which the
                      # qualification oracle verifies; without it the row cannot be
                      # checked at all.
                      "cleanup": self.receipt.get("cleanup"),
                      "wall_s": round((now_ns() - self.started) / NS, 3), "kept": str(self.dir)})

    def remove_fixture_mount(self):
        # This case's own image, after the nested guard exited: fixture hygiene,
        # never a closure claim about the guard's session.
        subprocess.run(["/usr/bin/hdiutil", "detach", self.volume["mount"]], stdin=subprocess.DEVNULL,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60)
        self.assertFalse(os.path.exists(self.volume["mount"]), "the fixture mount survived its own detach")

    def test_success_detaches_exactly_the_requested_volume(self):
        self.guard()
        detach = self.receipt["detach"]
        self.assertEqual((detach["state"], detach["qualified"], detach["post_state"]),
                         ("detached", True, "detached"), detach)
        self.assertEqual((detach["backing"], detach["mount"], detach["device"]),
                         (self.volume["image"], self.volume["mount"], self.volume["device"]))
        self.assertTrue(detach["attempted"])
        self.assertEqual(self.code, 0, self.receipt["outcome"])
        self.assertFalse(os.path.exists(self.volume["mount"]), "the requested mount was not detached")
        self.assertFalse(os.path.exists(self.receipt["scratch"]), "owned scratch was kept")
        self.assertTrue(os.path.isfile(self.volume["image"]), "the evidence backing was removed")

    def test_an_unbound_identity_is_refused_before_any_detach(self):
        self.guard(override={"volume_uuid": "11111111-2222-3333-4444-555555555555"})
        detach = self.receipt["detach"]
        self.assertEqual((detach["state"], detach["attempted"], detach["qualified"]),
                         ("refused", False, False), detach)
        self.assertIn("UUID", detach["reason"], detach["reason"])
        self.assertEqual(self.code, 125)
        self.assertTrue(os.path.exists(self.volume["mount"]), "a refused request still detached")
        self.assertTrue(os.path.exists(self.receipt["scratch"]), "the refused root was cleaned")
        self.remove_fixture_mount()

    def test_an_unrequested_attachment_keeps_the_owned_root(self):
        # A proof that failed before publishing its request may still have its
        # image attached inside the guard's own scratch. Cleaning that root would
        # delete a backing under a live mount, so the fail-closed scan keeps it.
        self.guard(mode="unrequested")
        # No request: no detach attempt and no `detach` object at all. The
        # separate fail-closed cleanup check is what keeps the root.
        self.assertIsNone(self.receipt.get("detach"), "a request-free run recorded a detach object")
        kept = self.receipt["cleanup"]
        self.assertEqual(kept["state"], "kept")
        # Stable semantics, not a transcript phrase: the declared backing is named
        # and the reason says nothing was positively detached.
        self.assertEqual(kept["unrequested"], [self.volume["image"]])
        self.assertIn("never positively detached", kept["reason"])
        self.assertEqual(self.code, 125)
        self.assertTrue(os.path.exists(self.receipt["scratch"]), "the root holding the image was cleaned")
        self.assertTrue(os.path.isfile(self.volume["image"]), "the backing under the live mount was removed")
        self.assertTrue(os.path.exists(self.volume["mount"]), "the unrequested attachment was detached")
        self.remove_fixture_mount()

    def test_a_stuck_detach_child_is_closed_by_the_custodian(self):
        # A REAL injected stuck child: the detach "child" is a live `sleep` the
        # monitor cannot reap. The custodian must pin it by identity, signal it and
        # observe it gone, and nothing may be cleaned up.
        self.guard(inject=["detach-stuck-child"])
        detach = self.receipt["detach"]
        self.assertEqual((detach["qualified"], detach["reaped"]), (False, False), detach)
        self.assertEqual(self.code, 125)
        # `guard()` loaded the PERSISTED receipt, so this is an integrated
        # receipt-path check: the monitor handed the child over by PID AND start
        # time, never reaped it afterwards, and the custodian's pin, signal and
        # observation reached the file because the drain runs before the final
        # publication.
        self.assertEqual((detach.get("custodian_owned"), detach.get("custodian_closed")), (True, True), detach)
        records = [r for r in self.receipt["custodian"] if r["event"].startswith("detach_child_")]
        pinned = [r for r in records if r["event"] == "detach_child_pinned"]
        self.assertTrue(pinned, records)
        self.assertEqual(pinned[0]["observation"], "live", records)
        self.assertTrue(typed_int(pinned[0]["start"]), pinned)
        self.assertTrue([r for r in records if r["event"] == "detach_child_signal"], records)
        observed = [r for r in records if r["event"] == "detach_child_observed"]
        self.assertTrue(observed and observed[0]["closed"] is True, records)
        pid = pinned[0]["pid"]
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)
        self.assertTrue(os.path.exists(self.receipt["scratch"]), "the root was cleaned under a live child")
        self.remove_fixture_mount()

    def test_an_unreadable_identity_closes_the_child_in_the_monitor(self):
        # A REAL live detach child plus an unreadable identity lookup: the monitor
        # cannot name it by start, so it stops and reaps its own direct child
        # inside the custodian's window and reports that closure truthfully.
        self.guard(inject=["detach-stuck-child", "detach-identity-unreadable"])
        detach = self.receipt["detach"]
        self.assertEqual(self.code, 125)
        self.assertIsNotNone(detach.get("identity_problem"), detach)
        self.assertEqual((detach.get("reaped"), detach.get("reaped_by")), (True, "monitor"), detach)
        self.assertEqual(detach["qualified"], False)
        # Durable facts only: the monitor's own close is recorded in the detach
        # object, whatever phase the monitor published last.
        self.assertEqual(detach.get("reap_timed_out"), False, detach)
        records = [r for r in self.receipt["custodian"] if r["event"].startswith("detach_child_")]
        self.assertFalse([r for r in records if r["event"] == "detach_child_signal"],
                         "an identity-less child was handed to the custodian anyway")
        self.assertFalse([r for r in records if r["event"] == "detach_child_pinned"], records)
        self.assertTrue(os.path.exists(self.receipt["scratch"]), "the root was cleaned")
        self.remove_fixture_mount()

    def test_an_unreadable_identity_can_be_handed_off_by_parentage(self):
        # The monitor cannot close it itself here (and only ITS lookups are forced
        # unreadable), so it hands the child over by PARENTAGE while still holding
        # it unreaped; the custodian's own lookup is readable, so it signals and
        # positively observes it.
        self.guard(inject=["detach-stuck-child", "detach-identity-unreadable",
                           "detach-handoff-unreadable"])
        detach = self.receipt["detach"]
        self.assertEqual(self.code, 125)
        self.assertIsNotNone(detach.get("identity_problem"), detach)
        self.assertEqual((detach.get("custodian_owned"), detach.get("custodian_closed")), (True, True),
                         detach)
        records = [r for r in self.receipt["custodian"] if r["event"].startswith("detach_child_")]
        pinned = [r for r in records if r["event"] == "detach_child_pinned"]
        self.assertTrue(pinned and pinned[0].get("identity") == "parentage", records)
        observed = [r for r in records if r["event"] == "detach_child_observed"]
        self.assertTrue(observed and observed[0]["closed"] is True, records)
        with self.assertRaises(ProcessLookupError):
            os.kill(pinned[0]["pid"], 0)
        self.assertTrue(os.path.exists(self.receipt["scratch"]), "the root was cleaned")
        self.remove_fixture_mount()

    def test_a_custodian_side_unreadable_lookup_refuses_without_signalling(self):
        # Only the custodian's lookups are forced unreadable: the monitor's handoff
        # is well formed, but the receiver cannot verify it, so it refuses, records
        # that, and acknowledges without signalling; the run stays unqualified and
        # the root is kept.
        self.guard(inject=["detach-stuck-child", "detach-identity-unreadable",
                           "detach-handoff-unreadable", "detach-custodian-identity-unreadable"])
        detach = self.receipt["detach"]
        self.assertEqual(self.code, 125)
        self.assertEqual((detach.get("custodian_owned"), detach.get("custodian_closed")), (False, False),
                         detach)
        records = [r for r in self.receipt["custodian"] if r["event"].startswith("detach_child_")]
        refused = [r for r in records if r["event"] == "detach_child_refused"]
        self.assertTrue(refused and refused[0]["observation"].startswith("identity unreadable"),
                        records)
        self.assertFalse([r for r in records if r["event"] == "detach_child_signal"], records)
        self.assertTrue(os.path.exists(self.receipt["scratch"]), "the root was cleaned")
        self.remove_fixture_mount()

    def test_a_failed_attempt_keeps_the_mount_and_is_unqualified(self):
        self.guard(inject=["detach-fail"])
        detach = self.receipt["detach"]
        self.assertEqual((detach["state"], detach["qualified"], detach["post_state"]),
                         ("failed", False, "still_mounted"), detach)
        self.assertEqual(self.code, 125)
        # The published outcome is decided before the detach message latches its
        # fault, so the failure is durable in the detach object and the exit code.
        self.assertEqual((detach["state"], detach["qualified"], detach["post_state"]),
                         ("failed", False, "still_mounted"), detach)
        self.assertTrue(os.path.exists(self.volume["mount"]), "a failed detach removed the mount")
        self.assertTrue(os.path.exists(self.receipt["scratch"]), "the kept root was cleaned")
        self.remove_fixture_mount()



class Results(unittest.TextTestResult):
    outcomes = []

    def addSuccess(self, test):
        super().addSuccess(test)
        Results.outcomes.append([test.id(), "pass"])

    def addFailure(self, test, err):
        super().addFailure(test, err)
        Results.outcomes.append([test.id(), "fail"])

    def addError(self, test, err):
        super().addError(test, err)
        Results.outcomes.append([test.id(), "error"])


def write(name, size):
    with open(SCRATCH / name, "wb") as handle:
        handle.write(os.urandom(size))


def main():
    # Secondary bound; the supervisor's deadline is primary. It is the 560 s
    # candidate limit less its 30 s margin (oxide#192 amendment: was 350 s; 250 s before #191).
    signal.alarm(530)
    PROTECTION.update(protect_outer_root())
    if PROTECTION.get("state") == "failed":
        # No nested case may run when the outer root could not be protected:
        # that is the one way an intentional nested UNKNOWN could outlive an
        # expirable marker.
        print(f"suite: refused before any case: the outer root could not be protected: {PROTECTION}",
              file=sys.stderr)
        return 1
    record_layout()
    loader = unittest.defaultTestLoader
    runner = unittest.TextTestRunner(verbosity=2, resultclass=Results)
    pure = unittest.TestSuite(loader.loadTestsFromTestCase(case)
                              for case in (PureGuard, PureFreshnessClock, PurePrecedence, PureCensus,
                                           PureAncestors,
                                           PureCensusFinalAbsence,
                                           PureDiagnostics, PureAllowlist, PureTerminalCounts, PureProtocol,
                                           PureAdmission,
                                           PureTraversal, PureDefaultScratch, PureScratchPreparation,
                                           PureMarker, PureSweep, PureExistingParent,
                                           PureProtectionAndLifecycle, QualifyEntry,
                                           PureSurvivors,
                                           PureQualificationTopology, PureClosureDecision,
                                           PureSettlement, PureInterleavedConfirmation, PureDetach))
    ok = runner.run(pure).wasSuccessful()
    if sys.argv[1:] != ["--pure"]:
        # Writers before and after a simulated staging step, in the outer
        # supervisor's scratch so its monitor counts them.
        write("before.bin", 1024 * 1024)
        staged = str(SCRATCH / "staged.bin")
        subprocess.run([sys.executable, "-c", f"open({staged!r}, 'wb').write(bytes(4 * 1024 * 1024))"],
                       check=True, timeout=30)
        time.sleep(2.5)
        ok = runner.run(unittest.TestSuite([loader.loadTestsFromTestCase(Real),
                                              loader.loadTestsFromTestCase(DetachReal)])).wasSuccessful() and ok
        write("after.bin", 1024 * 1024)
        path = os.environ.get("OXIDE_SUPERVISOR_CASES")
        if path:
            with open(path, "w") as handle:
                json.dump({"outcomes": Results.outcomes, "cases": CASES}, handle, indent=1)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
