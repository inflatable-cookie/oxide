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
(45 s) < fixture fallback (60 s) < qualification (300 s). A case asserts its
verdict before the fallback; fallback expiry never counts as closure.

Real case directories go in the outer supervisor's evidence directory
(OXIDE_PROOF_EVIDENCE), outside its cleanup, and are never deleted: an
UNCLOSED case's live fixtures and receipts are kept for the owner.
"""
from __future__ import annotations

import errno
import importlib.util
import json
import os
import secrets
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time
import types
import unittest
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
        return bounded.Guard(start_ns=1000 * NS)

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
    def __init__(self, rows, sessions, reread=None):
        self.rows, self.sessions, self.rereads = rows, sessions, reread or {}
        self.reads = 0

    def snapshot(self):
        return list(self.rows)

    def getsid(self, pid):
        value = self.sessions[pid]
        if isinstance(value, BaseException):
            raise value
        return value

    def reread(self, pid):
        self.reads += 1
        value = self.rereads.get(pid, "rows")
        if callable(value):
            value = value(self.reads)
        if isinstance(value, BaseException):
            raise value
        if value != "rows":
            return value
        return next((r for r in self.rows if r["pid"] == pid), None)


def census(table, sid=100, terminal_allowed=False):
    return bounded.Census(sid, snapshot=table.snapshot, getsid=table.getsid, reread=table.reread,
                          pause=lambda _seconds: None, terminal_allowed=terminal_allowed)


class PureCensus(unittest.TestCase):
    def test_member_and_foreign(self):
        table = FakeTable([row(101), row(200)], {101: 100, 200: 7})
        result = census(table).round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 1, 0))

    def test_identity_change_across_getsid_is_uncertain(self):
        table = FakeTable([row(101, start=1)], {101: 7}, reread={101: row(101, start=2)})
        result = census(table).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))

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


class PureCensusFinalAbsence(unittest.TestCase):
    """Oxide#114: the round's existing final read applies the Gone rule."""

    def test_final_none_after_matching_exhaustion_untracked(self):
        base = row(109, start=5)
        table = FakeTable([base], {109: ProcessLookupError(3, "gone")},
                           reread={109: lambda reads: row(109, start=5) if reads <= 20 else None})
        check = census(table)
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
        check = census(table)
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))
        self.assertEqual(table.reads, 21)
        self.assertNotIn((110, 5), check.tracked)
        self.assertNotIn((110, 9), check.tracked)
        self.assertEqual(result["diagnostics"]["rows"][0]["final"], "replaced")

    def test_final_none_tracked_drops_only_old(self):
        table = FakeTable([row(111, start=5)], {111: 100})
        check = census(table)
        self.assertEqual(check.round()["members"], 1)
        table.sessions = {111: ProcessLookupError(3, "gone")}
        table.reads = 0
        table.rereads = {111: lambda reads: row(111, start=5) if reads <= 20 else None}
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))
        self.assertNotIn((111, 5), check.tracked)

    def test_final_replacement_tracked_not_foreign_by_inference(self):
        table = FakeTable([row(112, start=5)], {112: 100})
        check = census(table)
        check.round()
        table.sessions = {112: ProcessLookupError(3, "gone")}
        table.reads = 0
        table.rereads = {112: lambda reads: row(112, start=5) if reads <= 20 else row(112, start=9)}
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))
        self.assertNotIn((112, 5), check.tracked)
        self.assertNotIn((112, 9), check.tracked)
        # The replacement identity is a new row next round, foreign by validation, not by inference.
        table.rows = [row(112, start=9)]
        table.sessions = {112: 7}
        table.rereads = {}
        table.reads = 0
        follow = check.round()
        self.assertEqual((follow["certain"], follow["members"], follow["uncertain"]), (True, 0, 0))

    def test_persistent_matching_untracked_still_uncertain(self):
        table = FakeTable([row(113, start=5)], {113: ProcessLookupError(3, "gone")})
        check = census(table)
        result = check.round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual(table.reads, 21)
        self.assertEqual(result["diagnostics"]["rows"][0]["final"], "same_identity")

    def test_persistent_matching_tracked_stays_member(self):
        table = FakeTable([row(114, start=5)], {114: 100})
        check = census(table)
        check.round()
        table.sessions = {114: ProcessLookupError(3, "gone")}
        table.reads = 0
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 1, 0))

    def test_final_read_failure_still_blocks(self):
        def failing(_reads):
            raise PermissionError(13, "denied")
        table = FakeTable([row(115, start=5)], {115: ProcessLookupError(3, "gone")}, reread={115: failing})
        result = census(table).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual(result["diagnostics"]["rows"][0]["final"], "read_failed")

    def test_confirm_read_failure_then_final_absence_gone(self):
        def flaky(reads):
            if reads == 1:
                raise PermissionError(13, "denied")
            return None
        table = FakeTable([row(116, start=5)], {116: ProcessLookupError(3, "gone")}, reread={116: flaky})
        check = census(table)
        result = check.round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 0, 0))
        self.assertEqual(table.reads, 2)
        self.assertEqual(result["diagnostics"]["rows"][0]["confirm"], "read_failed")
        self.assertEqual(result["diagnostics"]["rows"][0]["final"], "absent")

    def test_confirm_read_failure_then_final_failure_blocks(self):
        def failing(_reads):
            raise PermissionError(13, "denied")
        table = FakeTable([row(117, start=5)], {117: ProcessLookupError(3, "gone")}, reread={117: failing})
        result = census(table).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual(table.reads, 2)

    def test_non_esrch_still_blocks(self):
        table = FakeTable([row(118, start=5)], {118: PermissionError(1, "denied")})
        result = census(table).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertNotIn("diagnostics", result)

    def test_tracked_changes_still_member_until_positive_absence(self):
        table = FakeTable([row(119, start=5, ppid=101)], {119: 100})
        check = census(table)
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
        result = census(table).round()
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
        result = census(table).round()
        self.assertEqual((result["certain"], result["members"], result["uncertain"]), (True, 1, 0))
        self.assertEqual(result["terminal_count_status"], "valid")
        self.assertEqual(result["terminal_nonwriters"], 0)
        self.assertIs(type(result["terminal_nonwriters"]), int)
        self.assertNotIn("terminal_witnesses", result)

    def test_gone_never_counts_even_allowlisted(self):
        for allowed in (False, True):
            table = FakeTable([row(103)], {103: ProcessLookupError(3, "gone")}, reread={103: None})
            result = census(table, terminal_allowed=allowed).round()
            self.assertEqual((result["certain"], result["uncertain"]), (True, 0))
            self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                             ("valid", 0))
            self.assertNotIn("terminal_witnesses", result)
            self.assertEqual(table.reads, 1)

    def test_terminal_zombie_allowlisted(self):
        table = FakeTable([zombie_row(140, 5)], {140: ProcessLookupError(3, "gone")})
        check = census(table, terminal_allowed=True)
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
        result = census(table).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("valid", 0))
        self.assertNotIn("terminal_witnesses", result)

    def test_live_snapshot_unrelated_valid_zero(self):
        # A live snapshot was never a terminal candidate: its uncertainty
        # is unrelated to terminal accounting, so the accountable count
        # stays valid 0 while the round still blocks.
        table = FakeTable([row(142, start=5)], {142: ProcessLookupError(3, "gone")})
        result = census(table, terminal_allowed=True).round()
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
        result = census(table, terminal_allowed=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("invalid", None))

    def test_final_failure_no_terminal(self):
        table = FakeTable([zombie_row(144, 5)], {144: ProcessLookupError(3, "gone")},
                           reread={144: lambda reads: zombie_row(144, 5) if reads <= 20
                           else (_ for _ in ()).throw(PermissionError(13, "denied"))})
        result = census(table, terminal_allowed=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("invalid", None))

    def test_tracked_member_terminal_recomputed(self):
        table = FakeTable([row(145, start=5)], {145: 100})
        check = census(table, terminal_allowed=True)
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
        result = census(table, terminal_allowed=True).round()
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
        result = census(table, terminal_allowed=True).round()
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
        result = census(table, terminal_allowed=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("invalid", None))

    def test_final_live_disqualifies(self):
        table = FakeTable([zombie_row(150, 5)], {150: ProcessLookupError(3, "gone")},
                           reread={150: lambda reads: zombie_row(150, 5) if reads <= 20
                           else row(150, start=5)})
        result = census(table, terminal_allowed=True).round()
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
        check = census(table, terminal_allowed=True)
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
        result = census(table, terminal_allowed=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (False, 1))
        self.assertIn("getsid", result["uncertain_rows"][0]["reason"])
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("valid", 0))

    def test_replacement_is_gone_not_terminal(self):
        table = FakeTable([zombie_row(147, 5)], {147: ProcessLookupError(3, "gone")},
                           reread={147: lambda reads: zombie_row(147, 5) if reads == 1
                           else zombie_row(147, 9)})
        result = census(table, terminal_allowed=True).round()
        self.assertEqual((result["certain"], result["uncertain"]), (True, 0))
        self.assertEqual((result["terminal_count_status"], result["terminal_nonwriters"]),
                         ("valid", 0))
        self.assertNotIn("terminal_witnesses", result)

    def test_combined_budget_truncation_keeps_count(self):
        pids = list(range(150, 165))
        rows = [zombie_row(pid, pid) for pid in pids]
        sessions = {pid: ProcessLookupError(3, "gone") for pid in pids}
        table = FakeTable(rows, sessions)
        result = census(table, terminal_allowed=True).round()
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
        result = census(table, terminal_allowed=True).round()
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
        self.assertEqual(set(bounded.FIELDS["census"]), {"after", "members", "uncertain", "certain"})
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
        self.assertEqual(facts["root_parent"], "/Volumes/Builds")
        self.assertEqual(facts["mount"], "/Volumes/Builds")
        self.assertEqual(facts["volume_uuid"], DRIVE_UUID)
        self.assertEqual(facts["device"], DRIVE)
        self.assertEqual(facts["pool"], "/Users/owner/Dev/oxide")
        self.assertEqual(facts["pool_source"], "account config")
        self.assertEqual(facts["registration"], "/Users/owner/Dev/oxide/oxide-managed/removable.json")
        self.assertIsNone(facts["reason"])

    def test_pool_on_the_drive_uses_its_mount_root(self):
        world = default_world(pool="/Volumes/Builds/oxide-pool", registration=False, pool_device=DRIVE)
        facts = self.select(world, {"OXIDE_POOL": "/Volumes/Builds/oxide-pool"})
        self.assertEqual(facts["source"], "selected-volume")
        self.assertEqual(facts["root_parent"], "/Volumes/Builds")
        self.assertEqual(facts["registration"], None)
        self.assertEqual(facts["pool_source"], "OXIDE_POOL")

    def test_registration_beats_an_internal_pool(self):
        facts = self.select(default_world(pool_device=BOOT))
        self.assertEqual(facts["source"], "selected-volume")
        self.assertEqual(facts["root_parent"], "/Volumes/Builds")

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
        # it finds the candidate receipt through --evidence instead.
        text = BOOTSTRAP.read_text()
        self.assertNotIn('"--root-parent"', text, "the bootstrap would mask default selection")
        self.assertNotIn('"--evidence"', text, "external evidence would sit outside the monitored root")
        self.assertIn('"OXIDE_SUPERVISOR_LAYOUT"', text, "the suite layout record is not wired")

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
        registration = os.path.join(parent, "registration.json")
        with open(registration, "w") as handle:
            json.dump({"version": 2, "mount_point": VOLUME, "volume_uuid": DRIVE_UUID}, handle)
        facts = TopologyFacts({parent: DRIVE_DEVICE, root: DRIVE_DEVICE, evidence: DRIVE_DEVICE,
                               scratch: DRIVE_DEVICE, "/private/tmp": 1,
                               os.path.join(parent, "pool"): DRIVE_DEVICE},
                              documents={os.path.join(parent, "topology.json"):
                                         {"scratch": scratch, "evidence": evidence}},
                              boot=1, free={parent: 500 * 1024 ** 3, VOLUME: 500 * 1024 ** 3,
                                          "/private/tmp": 30 * 1024 ** 3})
        return sandbox, parent, root, evidence, scratch, case, registration, facts

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

    def test_nested_cases_and_evidence_stay_inside_the_monitored_root(self):
        sandbox, parent, root, evidence, scratch, case, registration, facts = self.layout()
        expected = {"source": "selected-volume", "root_parent": parent}
        record, problems = qualify.topology(str(sandbox), expected, facts)
        self.assertEqual(problems, [])
        self.assertEqual(record["root"], root)
        self.assertEqual(qualify.nested_cases({"cases": [{"case": "zero", "dir": case}]}, record), [])
        receipt = self.receipt(parent, root, evidence, scratch, registration, case)
        results = {"outcomes": [["zero", "pass"]], "cases": [{"case": "zero", "dir": case}]}
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

    def __init__(self, sid, terminal_allowed=False):
        self.sid, self.terminal_allowed = sid, terminal_allowed
        self.stats = {"rows": 3, "esrch_rows": 1, "zombie_rows": 1, "confirm_reads": 2,
                      "pause_calls": 1, "terminal": 0, "snapshot_ns": 5, "scan_ns": 7,
                      "snapshot_error": False}

    def round(self):
        FakeCensus.rounds += 1
        return {"certain": True, "members": 0, "uncertain": 0, "member_rows": [],
                "uncertain_rows": [], "terminal_count_status": "valid", "terminal_nonwriters": 0}


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
        return {"run": custodian.run_id, "seq": custodian.monitor_seq + 1, "leader": [4242, 77],
                "type": kind, "at": 1, **fields}

    # The real census thread and receipt path.

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
                                        "uncertain", "certain"})
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


def record_layout():
    """Record the layout this suite ran in for the qualification bootstrap
    (oxide#145): the candidate's owned root, scratch and evidence. Nested
    real-case directories are created in OXIDE_PROOF_EVIDENCE, so recording
    and verifying this record is what proves the nested resources stayed
    inside the candidate's monitored root on the selected volume."""
    path = os.environ.get("OXIDE_SUPERVISOR_LAYOUT")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
    if not path or not scratch or not OUTER_EVIDENCE:
        return
    record = {"scratch": os.path.realpath(scratch), "evidence": os.path.realpath(OUTER_EVIDENCE),
              "suite_pid": os.getpid()}
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
    signal.alarm(250)  # secondary bound; the supervisor's deadline is primary
    record_layout()
    loader = unittest.defaultTestLoader
    runner = unittest.TextTestRunner(verbosity=2, resultclass=Results)
    pure = unittest.TestSuite(loader.loadTestsFromTestCase(case)
                              for case in (PureGuard, PurePrecedence, PureCensus, PureCensusFinalAbsence,
                                           PureDiagnostics, PureAllowlist, PureTerminalCounts, PureProtocol,
                                           PureTraversal, PureDefaultScratch, PureQualificationTopology,
                                           PureClosureDecision, PureSettlement))
    ok = runner.run(pure).wasSuccessful()
    if sys.argv[1:] != ["--pure"]:
        # Writers before and after a simulated staging step, in the outer
        # supervisor's scratch so its monitor counts them.
        write("before.bin", 1024 * 1024)
        staged = str(SCRATCH / "staged.bin")
        subprocess.run([sys.executable, "-c", f"open({staged!r}, 'wb').write(bytes(4 * 1024 * 1024))"],
                       check=True, timeout=30)
        time.sleep(2.5)
        ok = runner.run(loader.loadTestsFromTestCase(Real)).wasSuccessful() and ok
        write("after.bin", 1024 * 1024)
        path = os.environ.get("OXIDE_SUPERVISOR_CASES")
        if path:
            with open(path, "w") as handle:
                json.dump({"outcomes": Results.outcomes, "cases": CASES}, handle, indent=1)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
