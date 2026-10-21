#!/usr/bin/env python3
"""Synthetic qualification suite for the shared proof supervisor (oxide#109).

Runs inside the candidate supervisor's own session under
`proof:supervisor:qualify`. Pure cases (run first) drive the guard,
precedence, census and control-protocol logic with fake inputs: nothing is
spawned. Real cases run `bounded-test.py` as a separate process around tiny
Python fixtures, with test-only injected faults (OXIDE_SUPERVISOR_INJECT) and
injected readings; the disk is never filled.

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

import importlib.util
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
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


# Pure cases --------------------------------------------------------------------


class PureGuard(unittest.TestCase):
    def guard(self):
        return bounded.Guard(start_ns=1000 * NS)

    def test_low_reading_latches_and_never_clears(self):
        guard = self.guard()
        guard.reading({"kind": "reserve", "value": 22 * GIB - 1, "at": 1000 * NS})
        guard.reading({"kind": "reserve", "value": 500 * GIB, "at": 1000 * NS + 1})
        self.assertIn("below", guard.check(1000 * NS + 2))

    def test_exact_threshold_passes(self):
        guard = self.guard()
        guard.reading({"kind": "reserve", "value": 22 * GIB, "at": 1000 * NS})
        guard.reading({"kind": "scratch", "value": 0, "complete": True, "at": 1000 * NS})
        self.assertIsNone(guard.check(1000 * NS + NS // 2))

    def test_measurement_error_faults(self):
        guard = self.guard()
        guard.reading({"kind": "reserve", "error": "injected statvfs failure", "at": 1000 * NS})
        self.assertIn("measurement failed", guard.fault)

    def test_stale_reserve_and_scratch(self):
        guard = self.guard()
        guard.reading({"kind": "reserve", "value": 500 * GIB, "at": 1000 * NS})
        guard.reading({"kind": "scratch", "value": 0, "complete": True, "at": 1000 * NS})
        self.assertIsNone(guard.check(1001 * NS))
        self.assertIn("reserve readings stale", guard.check(1001 * NS + 1))
        guard = self.guard()
        for at in range(1000, 1012):
            guard.reading({"kind": "reserve", "value": 500 * GIB, "at": at * NS})
        guard.reading({"kind": "scratch", "value": 0, "complete": True, "at": 1000 * NS})
        self.assertIn("scratch readings stale", guard.check(1010 * NS + 1))

    def test_incomplete_scratch_reading_does_not_refresh(self):
        guard = self.guard()
        guard.reading({"kind": "scratch", "value": 0, "complete": False, "at": 1009 * NS})
        guard.reading({"kind": "reserve", "value": 500 * GIB, "at": 1010 * NS})
        self.assertIn("scratch readings stale", guard.check(1010 * NS + 1))

    def test_future_stamped_reading_latches(self):
        # A reading stamped after the custodian's own clock reading is never
        # fresh, so it cannot keep a stopped sampler looking alive.
        guard = self.guard()
        guard.reading({"kind": "reserve", "value": 500 * GIB, "at": 2000 * NS}, now=1000 * NS)
        self.assertIn("future", guard.fault)
        self.assertIsNone(guard.reserve_at)

    def test_over_budget(self):
        guard = self.guard()
        guard.reading({"kind": "scratch", "value": 513 * 1024 * 1024, "complete": True, "at": 1000 * NS})
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


def census(table, sid=100):
    return bounded.Census(sid, snapshot=table.snapshot, getsid=table.getsid, reread=table.reread,
                          pause=lambda _seconds: None)


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

    def test_unpinned_target_is_never_signalled(self):
        custodian = self.custodian()
        custodian.leader_status = 0  # reaped: the pin is gone
        records = []
        custodian.record = lambda kind, **fields: records.append(fields)
        custodian.signal_pinned(4242, signal.SIGKILL, group=True)
        custodian.signal_pinned(5555, signal.SIGKILL, group=True)
        self.assertEqual([r["result"] for r in records], ["refused: not a pinned child"] * 2)
        self.assertEqual([r["held_unreaped"] for r in records], [False, False])


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


class Real(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp(prefix=self.id().rsplit(".", 1)[-1] + ".", dir=nested_root())).resolve()
        self.started = now_ns()

    def guardian(self, code, inject=(), owned=False, limit=35, signal_after=None, signal_number=None, command=None):
        env = dict(os.environ, CASE_DIR=str(self.dir), FIXTURE_DEADLINE_NS=str(self.started + FALLBACK_S * NS),
                   OXIDE_SUPERVISOR_INJECT=",".join(inject))
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
                                  for s in signals]})
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
        code = fixture("import os\n" + spawn_line(SLEEPER, setsid=True))
        self.assertEqual(self.guardian(code), 125)
        self.assertIn("EOF", self.outcome()["fault"])
        self.assertTrue(alive(self.child()))
        self.assert_kept_outside_outer_cleanup()

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

    def exception_case(self, inject, body, fault):
        """A TERM-ignoring leader with a TERM-ignoring child in its group: the
        injected exception must still reach the bounded pinned stop (TERM,
        then KILL of the group), WNOHANG reap, census and keep."""
        self.assertEqual(self.guardian(fixture(body), inject=[inject], owned=True), 125)
        outcome = self.outcome()
        self.assertEqual(outcome["closure"], "UNCLOSED")
        self.assertIn(fault, outcome["fault"])
        self.assertEqual(self.receipt["phase"], "monitor_complete: pending_custodian")
        records = self.receipt["custodian"]
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
        self.exception_case("stop-exception", self.GROUP + "time.sleep(1)\n", "injected exception during the stop")

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
    loader = unittest.defaultTestLoader
    runner = unittest.TextTestRunner(verbosity=2, resultclass=Results)
    pure = unittest.TestSuite(loader.loadTestsFromTestCase(case)
                              for case in (PureGuard, PurePrecedence, PureCensus, PureProtocol))
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
