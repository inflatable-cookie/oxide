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
import shutil
import signal
import stat
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
        code = fixture("import os\n" + spawn_line(SLEEPER, setsid=True))
        self.assertEqual(self.guardian(code), 125)
        self.assertIn("EOF", self.outcome()["fault"])
        self.assertTrue(alive(self.child()))
        self.assert_kept_outside_outer_cleanup()

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
    loader = unittest.defaultTestLoader
    runner = unittest.TextTestRunner(verbosity=2, resultclass=Results)
    pure = unittest.TestSuite(loader.loadTestsFromTestCase(case)
                              for case in (PureGuard, PurePrecedence, PureCensus, PureCensusFinalAbsence,
                                           PureDiagnostics, PureAllowlist, PureTerminalCounts, PureProtocol,
                                           PureTraversal))
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
