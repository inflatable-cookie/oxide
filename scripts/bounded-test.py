#!/usr/bin/env python3
"""Shared proof supervisor: pinned-leader cooperative closure (oxide#109).

    bounded-test.py [--limit S] [--scratch-budget-mib N] [--evidence DIR] [--root-parent DIR] [--] command ...

Implements docs/knowledge/contracts/proof-supervisor-correction.md (revision
4, reviewed at 29f0950e). Three processes:

- The custodian (this process) is single-threaded. It owns the absolute
  deadline, both pins (the monitor and the proof leader, held unreaped) and
  every signal. After setup it makes only bounded calls: fork, kevent and
  select with a zero or remaining-budget timeout, waitpid WNOHANG, kill or
  killpg on a pinned child, and non-blocking pipe I/O. It never touches files
  or stdio after setup and never signals a number it does not hold unreaped.
- The monitor (first child) owns all I/O, including all blocking setup (the
  kinfo_proc check, path resolution, scratch and evidence creation) inside
  the custodian's setup deadline: the receipt phases, output draining,
  reserve and scratch sampling (each reading carries its completion stamp),
  the session census and cleanup. It reports over a control pipe. Its
  owned-byte walk counts `st_blocks * 512` per distinct (device, inode) and
  never follows a symlink; an entry whose absolute path name is at or past
  the platform limit is reached from its nearest nameable directory by one
  short descriptor-relative component at a time (oxide#142).
- The leader (second child) calls setsid, acknowledges, and execs the command
  only after the custodian holds its NOTE_EXIT detector and opens the gate.

Closure is established only with no latched fault, the leader reaped, a
census started after the leader was reaped that is certain with no members
and no uncertain rows, and EOF on both output pipes. An uncertain census
after the stop latches a fault that no later census clears. Anything else is
UNCLOSED and scratch is kept. The command gets OXIDE_PROOF_EVIDENCE, the
evidence directory, which cleanup never removes. Groups other than the
pinned leader's are never signalled.

An exception in any phase after setup, or a monitor message with a missing
or ill-typed field, latches a fault; the bounded pinned stop, WNOHANG reap,
census and keep still run, each phase under its own guard. An uncaught
error at the top level gets one bounded, non-blocking line, no traceback.

One clock: CLOCK_UPTIME_RAW integer nanoseconds. Exit: 124 deadline; 125
fault (including SIGINT or SIGTERM to the custodian), refusal or UNCLOSED;
otherwise the command's status (128+N for signal N). The receipt has no success phase: success is phase `monitor_complete`
plus custodian exit 0, recorded by the caller.

Admission reports (oxide#192). A supervised child that waited for admission may
publish one bounded marker, `admission.json`, in the evidence directory, and one
`admission-waiting.json` when it starts waiting. Schema 2 documents name this
run (`OXIDE_PROOF_RUN_ID`) and one bounded command token, so a stale file from an
earlier run is rejected instead of paired with this one, and each waiting report
is paired only with its own command's completed marker. The monitor reads only a
regular file without following a link, at most 512 bytes with an exact key set,
and reports each once per file identity. The custodian grants at most the
completed wait, never more than the run's own elapsed wall clock and never more
than a fixed 600 s total, to its deadline; a waiting report grants nothing, and
a command's wait counts as unfinished only when no terminal report for that same
command arrived and its waiting report was the last accepted admission report (a
zero-duration grant, a capped grant or a late grant all end it; any later report
makes the fixed-name completion file a possible overwrite, so such a wait is
`possibly_overwritten` and unclassified rather than proven unfinished). Nothing is shortened, the exported OXIDE_PROOF_DEADLINE_NS is not rewritten
and classification is unchanged: a rejected or missing report removes nothing
but the grant, and the receipt reports the wait, the pairing and the grant
separately. One
bounded drain reads what has already arrived before the deadline is declared, so
a marker published just before the stop can still grant; a marker that reaches
the custodian after the stop decision or a deadline is recorded as late with its
reason and moves nothing, so a run that stopped as admission completed is
classified honestly rather than as an unexplained timeout. The grant is
cooperative evidence about a child's own admission, not a claim about any other
writer.

Scope: the cooperative session. A process that calls setsid leaves it and is
seen only while tracked or holding an output pipe; arbitrary same-user
writers are outside the promise. Host free space is a sampled abort
threshold; the owned-byte budget is monitored, not a hard ceiling. macOS
only.

Post-census image detach (oxide#191, decision ee263cea): a proof whose own
test image must be detached publishes ONE bounded atomic `detach-request.json`
under OXIDE_PROOF_EVIDENCE naming that image's backing, mount, device and volume
UUID. Only on the `cleanup` decision (a certain empty census after the leader was
reaped, with no fault) does the monitor re-derive every binding from kernel and
`hdiutil` facts and make at most one ORDINARY detach of exactly that image,
inside the same absolute deadline and under the still-running reserve and
owned-byte samplers, before the owned-scratch cleanup. A refusal, failure,
timeout, identity change or ambiguous post-state is never retried, keeps the
mount, backing and evidence, is recorded in the receipt's `detach` object and
ends the run unqualified (exit 125). No request means nothing changes.

Default scratch (oxide#145, oxide#151): with no caller `OXIDE_TASK_SCRATCH_ROOT`
and no `--root-parent`, the owned root is created under the owned subtree
`<mount>/scratch/oxide` of the volume the effect-free pool selection names,
outside the pool payload; a selected volume must not be the boot volume, and an
absent, unreadable, malformed or unsafe selection falls back to /private/tmp
with one bounded warning. Selection itself creates nothing: the blocking setup
creates and validates the subtree components (0700, no symlink, this user,
canonical, the selected device) and then one `oxide-proof.*` root. Reserve
sampling reads the filesystem that actually holds the scratch, and the receipt
records the selection source, the prepared parent, the reserve path and device,
the bounded sweep summary and the owner-marker state.

Owner markers and bounded self-expiry (oxide#151): every root in that subtree
carries one private `.oxide-scratch.json` written before any run resource and
updated only at a genuine conclusion. Before its own root exists, one bounded
startup sweep may remove only owner-marked roots whose marker records a positive
conclusion of all their owned resources strictly older than seven days. The
entry and time budgets are fixed constants checked between filesystem steps;
there is no background reaper, and unmarked, corrupt, foreign, unconcluded or
nested-UNCLOSED roots are never touched.

OXIDE_SUPERVISOR_INJECT (comma list from INJECTIONS) is a test-only
fault-injection hook used by scripts/test_supervisor.py.
"""
from __future__ import annotations

import ctypes
import errno
import json
import math
import os
import plistlib
import pwd
import re
import secrets
import select
import shutil
import signal
import stat
import struct
import subprocess
import sys
import tempfile
import threading
import time

NS = 1_000_000_000
_MS = 1_000_000
GIB = 1024 ** 3
MIB = 1024 ** 2
LIMIT_S = 300
STOP_RESERVE_S = 20
EXIT_MARGIN_S = 2
SETUP_S = 10
TERM_GRACE_S = 5
CENSUS_GRACE_S = 10
THRESHOLD = 22 * GIB  # 20 GiB reserve + 2 GiB margin
RESERVE_INTERVAL, RESERVE_STALE_NS = 0.25, 1 * NS
SCRATCH_INTERVAL, SCRATCH_STALE_NS = 2.0, 10 * NS
SCRATCH_BUDGET = 512 * MIB
PERSIST = 8 * MIB
FORWARD = 32 * MIB
CENSUS_INTERVAL = 0.2
CONFIRM_TRIES, CONFIRM_PAUSE = 20, 0.05  # gone confirmation within one round, at most 1 s
DIAG_MAX_BYTES = 16384  # compact UTF-8 JSON ceiling for the optional receipt census diagnostics
DIAG_MAX_ROWS = 20  # at most 20 diagnostic summary rows; the rest count as omitted
# Observation-only settlement diagnostics (oxide#147). Fixed schema, one record
# per census round up to a cap, explicit `None` for a phase never reached. They
# never change classification, counts, faults, EOF, closure, deadlines or
# cleanup, and never appear in a control message.
SETTLE_MAX_ROUNDS = 20  # detailed round records kept; further rounds count as omitted
SETTLE_MAX_BYTES = 16384  # structural compact-JSON ceiling for the whole object
# The declared phase enums: the monitor writes only these values and the pure
# oracle rejects anything else, so an unknown phase is never evidence.
SETTLE_PHASES = ("none", "received", "thread-start", "round-start", "scanned", "lock-wait",
                 "locked", "sent", "round-complete", "loop-exit")
SETTLE_ROUND_STEPS = ("start", "scanned", "lock-wait", "locked", "sent", "complete")
_SETTLE_TOP = object()  # sentinel: a note without a round record targets the settlement object
TERMINAL_STATUS_VALUES = ("valid", "invalid", "overflow")
TERMINAL_MODE_REASONS = ("allowlisted", "identity_read_error", "identity_malformed", "identity_mismatch")
# One-shot exact-kernel allowlist for TERMINALNONWRITER acceptance (owner activation
# 3c6f543a v2 / Tom accept 9f497184; static trace c8677574 with disclosed gaps).
ALLOW_KERNEL_UUID_STR = "8D3E13A0-01FC-381D-9DA3-7F0AF537CB32"
ALLOW_KERNEL_UUID_BIN = bytes.fromhex("8D3E13A001FC381D9DA37F0AF537CB32")
ALLOW_KERNEL_BUILD = b"25G83\x00"
ALLOW_KERNEL_VERSION_TOKEN = "root:xnu-12377.161.14~5/RELEASE_ARM64_T6050"
ALLOW_EVIDENCE_ID = "kdk25G83-8D3E13A0-c8677574-TomAccept9f497184"
U64MAX = 2 ** 64 - 1
CONFIRM_VALUES = ("absent", "replaced", "matching_exhausted", "read_failed")
FINAL_VALUES = ("not_read", "absent", "replaced", "same_identity", "read_failed")
DIAG_ERROR_VALUES = ("none", "permission", "not_found", "io", "other")
MAX_LINE = 64 * 1024
# Default scratch parent (oxide#145): a private area on the volume the passive
# pool selection names, outside its managed payload, else the warned fallback.
FALLBACK_PARENT = "/private/tmp"
VOLUMES = "/Volumes"
DEFAULT_POOL_LEAF = "build-pool"
CONFIG_MAX_BYTES = 64 * 1024
REGISTRATION_MAX_BYTES = 64 * 1024
SCRATCH_LEAF = "oxide-proof."
REGISTRATION = ("oxide-managed", "removable.json")

# Selected-volume scratch layout (oxide#151). Proof scratch lives in one owned
# subtree of the selected volume, `<mount>/scratch/oxide`, instead of the drive
# root; selection only reports it and the supervisor's blocking setup creates
# and validates it, keeping the warned /private/tmp fallback on a refusal.
SCRATCH_SUBDIR = "scratch"
SCRATCH_OWNER = "oxide"
QUALIFY_LEAF = "oxide-supervisor-qualify."
VALIDATION_OWNER = "oxide-validation"
RUN_ROOT_LEAVES = (SCRATCH_LEAF, QUALIFY_LEAF)

# Owner marker and bounded self-expiry (oxide#151, Tom 2026-10-25). Every run
# root in the owned subtree carries one private marker file, written before any
# child or run resource and updated only at a genuine conclusion. A root is
# eligible for the bounded startup sweep only when its marker records a positive
# conclusion of all its owned resources strictly older than seven days. Nothing
# else is ever removed, no marker is a liveness probe, and an absent, foreign or
# corrupt marker fails closed.
MARKER_NAME = ".oxide-scratch.json"
MARKER_VERSION = 1
MARKER_MAX_BYTES = 4096
MARKER_KEYS = frozenset({"version", "run_id", "root", "owner_uid", "device", "inode",
                         "created_unix", "concluded_unix", "outcome", "closure", "kept_reason",
                         "nested_scope"})
MARKER_OUTCOMES = ("running", "closed", "unclosed", "failed", "retained")
# A root's own outer closure is a whole-owned-resource conclusion only when the
# root is exclusive. A root that may host resources created by another authority
# is `hosting` from before those resources exist, and only that authority can
# conclude it (oxide#151): the run's own closure is never enough.
MARKER_SCOPES = ("exclusive", "hosting")
MARKER_CLOSURES = ("positive", "none")
MARKER_REASONS = ("none", "evidence", "unclosed", "exception", "nested-unclosed",
                  "qualify-base", "qualify-failed", "detach", "live-image")
MARKER_EXPIRABLE_REASONS = ("evidence", "qualify-base")
EXPIRY_S = 7 * 24 * 60 * 60
SWEEP_TIME_BUDGET_NS = 1 * NS  # of the 10 s setup budget, checked between filesystem steps
SWEEP_ENTRY_BUDGET = 4096  # examined entries plus recursive steps, across all roots
SWEEP_ROWS = 20  # bounded result rows in the receipt; the rest count as omitted
INJECTIONS = {
    "setup-stall", "no-detector", "ack-eof", "ack-wrong", "cancel-before-ack", "monitor-die",
    "monitor-stall", "cleanup-interrupt", "custodian-timeout-after-monitor", "census-uncertain",
    "census-uncertain-first", "reserve-low-after", "cancel-after-ack", "cancel-at-gate",
    "pump-exception", "kqueue-exception", "malformed-message", "stop-exception",
    "reserve-delay-stall", "scratch-delay-stall",
    "detach-fail", "detach-timeout", "detach-ambiguous", "detach-withdraw",
    "detach-reap-timeout", "detach-stuck-child",
    "detach-identity-unreadable", "detach-handoff-unreadable",
    "detach-custodian-identity-unreadable",
}
RESERVE_DELAY_S, SCRATCH_DELAY_S = 0.5, 4.0  # injected publication delays before a sampler stall
INJECT_AFTER_GATE_NS = 1_500_000_000  # post-gate injections wait for the fixture to start
EXCEPTION_RECORDS = 8
# Admission marker (oxide#192): a supervised child that waited for admission
# publishes one bounded marker in the evidence directory. The custodian grants
# at most the reported wait, never more than the run's own elapsed wall clock
# and never more than ADMISSION_CAP_NS in total, so the work clock is not
# consumed by a queue wait while the run stays hard-bounded. A marker is
# cooperative evidence only: nothing about a rejected one faults the run.
ADMISSION_NAME = "admission.json"
# A waiting report (oxide#192): the child has started an admission wait and no
# grant has ended it. It never moves a deadline; it exists so a wait that never
# ends before the stop is reported and classified instead of leaving no trace.
WAITING_NAME = "admission-waiting.json"
ADMISSION_SCHEMA = 2
ADMISSION_MAX_BYTES = 512
ADMISSION_POLL_S = 0.25
ADMISSION_CAP_NS = 600 * NS
ADMISSION_RECORDS = 32  # per-report custodian records kept; the rest only move the counters
ADMISSION_REJECTIONS = 8  # bounded rejection reasons kept in the receipt
ADMISSION_COMMANDS = 64  # bounded pairing table: one entry per supervised command token
ADMISSION_UNFINISHED = 8  # bounded unfinished list carried by the decision record
ADMISSION_KEYS = frozenset({"schema", "source", "run", "command", "waited_ns"})
WAITING_KEYS = frozenset({"schema", "source", "run", "command", "reason"})
FIELDS = {  # required fields per monitor message type; bool is never accepted as int
    "reading": {"kind": str},
    "ready": {}, "monitor_complete": {}, "cleanup_failed": {},
    "output_eof": {"stream": str},
    "census": {"after": int, "members": int, "uncertain": int, "certain": bool},
    "admission": {"waited_ns": int, "command": str, "completed": int, "marker": int},
    "admission_waiting": {"reason": str, "command": str, "completed": int, "marker": int},
    "admission_rejected": {"kind": str, "completed": int, "reason": str},
    "setup": {"root": str, "scratch": str, "evidence": str, "owned": bool},
    "detach": {"attempted": bool, "qualified": bool, "post_state": str},
    "detach_ready": {"budget_ns": int},
    "detach_child": {"pid": int},  # `start` is optional: a parentage-only handoff has none
    "cleanup_kept": {"reason": str},
}


def now_ns():
    return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)


def settlement_record():
    """The monitor's observation-only settlement diagnostics (oxide#147).

    Fixed shape: every stamp is an int CLOCK_UPTIME_RAW value or `None` when
    the step was never reached, so a run that ends with no completed census
    round is distinguishable from one whose round was certain and empty. The
    round list is capped at SETTLE_MAX_ROUNDS and the remaining rounds are
    counted in `omitted`. Nothing here is read by classification, the fault,
    the EOF witness, the deadline or the cleanup decision."""
    return {
        "schema": 1, "clock": "CLOCK_UPTIME_RAW",
        "stopped_sent_ns": None, "stopped_received_ns": None, "stopped_seq": None,
        "receive_latency_ns": None, "leader_bound": None,
        "thread_start_ns": None, "thread_start_latency_ns": None, "census_built_ns": None,
        "first_round_ns": None, "first_round_latency_ns": None,
        "stop_requested_ns": None, "loop_exit_ns": None,
        "rounds_started": 0, "rounds_completed": 0, "omitted": 0,
        "phase": "none", "rounds": [],
    }


def token_ok(value):
    """A bounded, printable, non-empty token: the only text a report may carry."""
    return isinstance(value, str) and 0 < len(value) <= 32 and value.isprintable()


def admission_common_malformed(marker, keys, run_id):
    """Why a report's shared fields are not usable, or None.

    Fixed schema, exact keys, bounded types: schema 2, one printable source
    token, this run's own identity and one printable command token. A report
    from another run is stale and is never paired with this one."""
    if not isinstance(marker, dict):
        return "not an object"
    if set(marker) != keys:
        return "unexpected keys"
    if not typed(marker["schema"], int) or marker["schema"] != ADMISSION_SCHEMA:
        return "unsupported schema"
    if not token_ok(marker["source"]):
        return "ill-typed source"
    if marker["run"] != run_id:
        return "stale run"
    if not token_ok(marker["command"]):
        return "ill-typed command"
    return None


def admission_malformed(marker, run_id):
    """Why a completed-wait marker is not usable, or None.

    The shared fields, then a non-negative reported wait inside u64. A marker
    is cooperative evidence for the custodian's bounded grant; anything else is
    reported in the receipt and never latches a fault."""
    problem = admission_common_malformed(marker, ADMISSION_KEYS, run_id)
    if problem:
        return problem
    waited = marker["waited_ns"]
    if not typed(waited, int) or not 0 <= waited <= U64MAX:
        return "ill-typed waited_ns"
    return None


def admission_waiting_malformed(marker, run_id):
    """Why a waiting report is not usable, or None. The shared fields, then its
    own reason token instead of a wait. A waiting report never grants time:
    anything wrong with it is reported in the receipt and never latches a
    fault."""
    problem = admission_common_malformed(marker, WAITING_KEYS, run_id)
    if problem:
        return problem
    if not token_ok(marker["reason"]):
        return "ill-typed reason"
    return None


def admission_record():
    """The monitor's admission-marker observations (oxide#192).

    Fixed shape: every stamp is an int CLOCK_UPTIME_RAW value or `None` when
    no marker was observed, every count is a non-bool int, and the rejection
    reasons are a bounded list. This is the observation side of the grant;
    the custodian's own decision record carries what it actually granted."""
    return {"schema": 1, "marker": ADMISSION_NAME, "observed": 0, "reported_wait_ns": 0,
            "first_ns": None, "last_ns": None, "identity": None, "ignored": 0,
            "rejections": [], "errors": 0}


def admission_waiting_record():
    """The monitor's waiting-report observations (oxide#192).

    Fixed shape, same bounds: a stamp is an int CLOCK_UPTIME_RAW value or
    `None`, counts are non-bool ints, the first reported reason is kept as an
    opaque token and rejections are a bounded list. A waiting report grants
    nothing; the custodian records its own decision-side facts separately."""
    return {"schema": 1, "marker": WAITING_NAME, "observed": 0, "first_ns": None,
            "last_ns": None, "reason": None, "identity": None, "ignored": 0,
            "rejections": [], "errors": 0}


def report(text, fd=2):
    """Best effort and never blocking: writes at most PIPE_BUF bytes, and only
    when `fd` is writable now (a pipe then has at least PIPE_BUF free)."""
    try:
        if select.select([], [fd], [], 0)[1]:
            os.write(fd, text.encode()[: select.PIPE_BUF])
    except (OSError, ValueError):
        pass


_TYPE_NAME = type.__dict__["__name__"]
_TRACEBACK = BaseException.__dict__["__traceback__"]
_ERRNO = OSError.__dict__["errno"]


def type_name(error):
    """The exception's class name from the C-level type slot, as an exact
    str: no str/repr/format of the exception, no metaclass or subclass hook,
    no traceback or linecache, no I/O."""
    try:
        return str.__str__(_TYPE_NAME.__get__(type(error)))[:64]
    except BaseException:
        return "?"


def where_of(error):
    """Innermost frame's function and line from the traceback object's own
    fields: bounded walk, no source lookup or I/O."""
    try:
        tb = _TRACEBACK.__get__(error)
        for _ in range(64):
            if tb.tb_next is None:
                break
            tb = tb.tb_next
        return str.__str__(tb.tb_frame.f_code.co_name)[:64] + ":" + str(int(tb.tb_lineno))
    except BaseException:
        return "?"


def errno_name(error):
    try:
        value = _ERRNO.__get__(error)
        return errno.errorcode.get(value, "?") if type(value) is int else "?"
    except BaseException:
        return "?"


def typed(value, kind):
    return isinstance(value, bool) if kind is bool else isinstance(value, kind) and not isinstance(value, bool)


def malformed(message):
    """Why a monitor message is malformed, or None. Checked before any use."""
    if not isinstance(message, dict):
        return "not an object"
    kind = message.get("type")
    if kind not in FIELDS:
        return f"unknown type {kind!r}"
    leader = message.get("leader")
    if leader is not None and not (isinstance(leader, list) and len(leader) == 2 and typed(leader[0], int)
                                   and (leader[1] is None or typed(leader[1], int))):
        return "ill-typed leader"
    required = dict(FIELDS[kind], run=str, seq=int, at=int)
    if kind == "reading":
        if "error" in message:
            required["error"] = str
        else:
            required["value"] = int
            required["completed"] = int  # when the measurement completed, not when it was sent
        if message.get("kind") == "scratch":
            required["complete"] = bool
        elif message.get("kind") != "reserve":
            return "unknown reading kind"
    if kind == "output_eof" and message.get("stream") not in ("stdout", "stderr"):
        return "unknown stream"
    for name, kind_ in required.items():
        if name not in message:
            return f"missing {name}"
        if not typed(message[name], kind_):
            return f"ill-typed {name}"
    return None


def capability():
    """Missing primitives, from attribute checks only (no I/O); refused before
    any proof work, with no fallback. The kinfo_proc layout is checked by the
    monitor during its bounded setup."""
    missing = []
    if sys.platform != "darwin":
        missing.append("macOS")
    if not hasattr(time, "CLOCK_UPTIME_RAW"):
        missing.append("CLOCK_UPTIME_RAW")
    if not hasattr(select, "kqueue"):
        missing.append("kqueue")
    return missing


# Census (monitor side) ---------------------------------------------------------


class Kinfo:
    """struct kinfo_proc via sysctl, at offsets checked against this process."""

    SIZE, PID, STAT, UID, PPID, PGID = 648, 40, 36, 420, 560, 564
    _libc = None

    @classmethod
    def sysctl(cls, mib):
        if cls._libc is None:
            cls._libc = ctypes.CDLL(None, use_errno=True)
        array = (ctypes.c_int * len(mib))(*mib)
        for _ in range(3):
            size = ctypes.c_size_t(0)
            if cls._libc.sysctl(array, len(mib), None, ctypes.byref(size), None, 0):
                raise OSError(ctypes.get_errno(), "sysctl size")
            buffer = ctypes.create_string_buffer(size.value + 64 * cls.SIZE)
            size = ctypes.c_size_t(len(buffer))
            if cls._libc.sysctl(array, len(mib), buffer, ctypes.byref(size), None, 0) == 0:
                return buffer.raw[: size.value]
            if ctypes.get_errno() != errno.ENOMEM:
                raise OSError(ctypes.get_errno(), "sysctl read")
        raise OSError(errno.ENOMEM, "process table kept growing")

    @classmethod
    def parse(cls, raw):
        if len(raw) % cls.SIZE:
            raise ValueError("kinfo_proc size mismatch")
        rows = []
        for offset in range(0, len(raw), cls.SIZE):
            sec, usec = struct.unpack_from("<qi", raw, offset)
            rows.append({
                "pid": struct.unpack_from("<i", raw, offset + cls.PID)[0],
                "start": sec * 1_000_000 + usec,  # identity token only, never ordered
                "zombie": raw[offset + cls.STAT] == 5,
                "uid": struct.unpack_from("<I", raw, offset + cls.UID)[0],
                "ppid": struct.unpack_from("<i", raw, offset + cls.PPID)[0],
                "pgid": struct.unpack_from("<i", raw, offset + cls.PGID)[0],
            })
        return rows

    @classmethod
    def all(cls):
        return [row for row in cls.parse(cls.sysctl([1, 14, 0])) if row["pid"] > 0]

    @classmethod
    def one(cls, pid):
        rows = cls.parse(cls.sysctl([1, 14, 1, pid]))
        return rows[0] if rows and rows[0]["pid"] == pid else None

    @classmethod
    def check(cls):
        row = cls.one(os.getpid())
        if row is None or (row["ppid"], row["pgid"], row["uid"]) != (os.getppid(), os.getpgid(0), os.getuid()):
            raise ValueError("layout does not match this process")


def _diag_u64(value):
    """Valid unsigned 64-bit diagnostic integer, else None (omitted, never sanitized)."""
    return value if type(value) is int and 0 <= value <= U64MAX else None


def _diag_small(value, low, high):
    return value if type(value) is int and low <= value <= high else None


def _diag_classify(error):
    """Map an exception to (enumerated category, bounded errno or None).

    Reads only the C-level errno slot; never converts the exception to
    text. Unknown or unbounded values become omitted, never widened."""
    if error is None:
        return ("none", None)
    try:
        code = _ERRNO.__get__(error)
    except BaseException:
        code = None
    if type(code) is not int:
        return ("other", None)
    bounded = code if 0 <= code <= 4095 else None
    try:
        if code == errno.ESRCH:
            return ("not_found", bounded)
        if code == errno.EACCES or code == errno.EPERM:
            return ("permission", bounded)
        if code == errno.EIO:
            return ("io", bounded)
    except BaseException:
        return ("other", bounded)
    return ("other", bounded)


def _diag_row(row, tries, confirm, final, error, err_no):
    """One bounded diagnostic summary row with fixed enums only.

    Allowed keys only: initial pid/start/ppid/pgid/uid/zombie plus
    confirm_tries/confirm/final/error and an optional bounded errno.
    Invalid numerics are omitted (absent), never sanitized."""
    out = {}
    source = row if isinstance(row, dict) else {}
    for key in ("pid", "start", "ppid", "pgid", "uid"):
        try:
            valid = _diag_u64(source.get(key))
        except BaseException:
            valid = None
        if valid is not None:
            out[key] = valid
    try:
        zombie = source.get("zombie")
    except BaseException:
        zombie = None
    if zombie is True:
        out["zombie"] = True
    elif zombie is False:
        out["zombie"] = False
    valid_tries = _diag_small(tries, 0, CONFIRM_TRIES)
    if valid_tries is not None:
        out["confirm_tries"] = valid_tries
    if confirm in CONFIRM_VALUES:
        out["confirm"] = confirm
    if final in FINAL_VALUES:
        out["final"] = final
    if error in DIAG_ERROR_VALUES:
        out["error"] = error
    if type(err_no) is int and 0 <= err_no <= 4095:
        out["errno"] = err_no
    return out


def _diag_attempt(row, tries, outcome, final, error=None, category="none"):
    """One receipt-only diagnostic row, or None.

    Classifying an optional error and building the row are wrapped together,
    so a failure omits that single row exactly as before and can never change
    a classification, a count, a fault or the round's result."""
    try:
        number = None
        if error is not None:
            category, number = _diag_classify(error)
        return _diag_row(row, tries, outcome, final, category, number)
    except BaseException:
        return None


def _diag_object(attempted, rows, saturated):
    """Build the optional diagnostics object under the byte ceiling.

    Drops complete rows from the end first with an explicit omitted count;
    if even the minimal object cannot fit, returns None (omit entirely).
    Never raises; serialization failure also returns None."""
    try:
        if type(attempted) is not int or attempted < 0:
            attempted = U64MAX
            saturated = True
        elif attempted > U64MAX:
            attempted = U64MAX
            saturated = True
        kept = list(rows[:DIAG_MAX_ROWS])
        omitted = len(rows) - len(kept)
        if omitted < 0:
            omitted = 0
        if omitted > U64MAX:
            omitted = U64MAX
            saturated = True
        retained = len(kept)
        if retained > U64MAX:
            retained = U64MAX
            saturated = True
        obj = {"attempted": attempted, "retained": retained, "omitted": omitted,
               "saturated": bool(saturated), "rows": kept}
        compact = json.dumps(obj, ensure_ascii=True, separators=(',', ':')).encode("utf-8")
        if len(compact) <= DIAG_MAX_BYTES:
            return obj
        while kept:
            kept = kept[:-1]
            omitted = len(rows) - len(kept)
            if omitted < 0:
                omitted = 0
            if omitted > U64MAX:
                omitted = U64MAX
                saturated = True
            retained = len(kept)
            obj = {"attempted": attempted, "retained": retained, "omitted": omitted,
                   "saturated": bool(saturated), "rows": kept}
            compact = json.dumps(obj, ensure_ascii=True, separators=(',', ':')).encode("utf-8")
            if len(compact) <= DIAG_MAX_BYTES:
                return obj
        minimal = {"attempted": attempted, "retained": 0, "omitted": len(rows) if len(rows) <= U64MAX else U64MAX,
                   "saturated": bool(saturated or len(rows) > U64MAX), "rows": []}
        compact = json.dumps(minimal, ensure_ascii=True, separators=(',', ':')).encode("utf-8")
        return minimal if len(compact) <= DIAG_MAX_BYTES else None
    except BaseException:
        return None


def _same_complete_zombie(ref, cand):
    """Whether two observations show the same complete SZOMB identity.

    Requires both to be dicts with exact non-bool bounded ints (positive
    PID, start in range) equal across both, and exact SZOMB state in
    both. Pure predicate, never raises."""
    try:
        if not isinstance(ref, dict) or not isinstance(cand, dict):
            return False
        ref_pid, ref_start = ref.get("pid"), ref.get("start")
        cand_pid, cand_start = cand.get("pid"), cand.get("start")
        if not (type(ref_pid) is int and 1 <= ref_pid <= U64MAX):
            return False
        if not (type(ref_start) is int and 0 <= ref_start <= U64MAX):
            return False
        if not (type(cand_pid) is int and 1 <= cand_pid <= U64MAX):
            return False
        if not (type(cand_start) is int and 0 <= cand_start <= U64MAX):
            return False
        return (ref_pid == cand_pid and ref_start == cand_start
                and ref.get("zombie") is True and cand.get("zombie") is True)
    except BaseException:
        return False


def _terminal_fields(terminal_count, unaccountable, failed):
    """Mandatory receipt-only terminal count fields.

    Returns (status, value) with status in valid/invalid/overflow and
    value an exact non-bool int 0..U64MAX when valid, else None (JSON
    null). Valid counts this round's positively verified still-present
    SZOMB identities accepted by the terminal rule, never Gone rows.
    Unaccountable terminal candidacy (an actual-ESRCH row that could not
    be fully adjudicated as terminal or Gone), a failure without a
    countable set, or a malformed count is invalid with null, never a raw
    bad value, false zero or lower bound; genuine count overflow is
    likewise null. Invalid and overflow always coincide with uncertain>=1
    and certain False in the unchanged IPC fields (enforced by the
    caller). Uncertainty of unrelated rows (never terminal candidates)
    does not itself invalidate an accountable count. Optional
    diagnostics saturation never feeds this helper and must not change
    authoritative counts or classification. Never raises."""
    try:
        if failed:
            return ("invalid", None)
        if type(terminal_count) is int and terminal_count > U64MAX:
            return ("overflow", None)
        if unaccountable:
            return ("invalid", None)
        if type(terminal_count) is int and 0 <= terminal_count <= U64MAX:
            return ("valid", terminal_count)
        return ("invalid", None)
    except BaseException:
        return ("invalid", None)


def _fixed_sysctl_reader(sysctlbyname=None, get_errno=None):
    """One fixed-buffer sysctlbyname read per name. No size probe, no retry.

    The libc entry points are injectable for deterministic tests; production
    passes the loaded libc. A reported length above the fixed capacity is
    rejected, never silently sliced."""
    if sysctlbyname is None:
        _libc = ctypes.CDLL(None, use_errno=True)
        sysctlbyname, get_errno = _libc.sysctlbyname, ctypes.get_errno
    def read(name, size):
        buf = ctypes.create_string_buffer(size)
        length = ctypes.c_size_t(size)
        if sysctlbyname(name.encode("ascii"), buf, ctypes.pointer(length), None, 0) != 0:
            raise OSError(get_errno(), "sysctlbyname " + name)
        if length.value > size:
            raise OSError(errno.ENOMEM, "sysctlbyname " + name + " exceeds fixed buffer")
        return bytes(buf.raw[:length.value])
    return read


def _is_oversize(error):
    """Whether a sysctlbyname failure means the value exceeds the fixed buffer."""
    try:
        return _ERRNO.__get__(error) == errno.ENOMEM
    except BaseException:
        return False


def _ascii_nul_text(raw):
    """Complete ASCII NUL-terminated string body, else None.

    Exactly one NUL, in terminal position; interior NULs are malformed."""
    try:
        if not raw or raw[-1:] != b"\x00":
            return None
        body = raw[:-1]
        if not body or b"\x00" in body:
            return None
        return body.decode("ascii")
    except BaseException:
        return None


def _decide_terminal_mode(reader=None):
    """Decide terminal acceptance mode once. Never raises.

    Uses at most 3 fixed-buffer sysctlbyname reads (kern.uuid 64B,
    kern.osversion 64B, kern.version 1024B) via the loaded libc; no size
    probes, retries, subprocess, files, hashes or new actors. Returns
    {mode, reason, evidence} with mode allowlisted/strict, reason one of
    allowlisted/identity_read_error/identity_malformed/identity_mismatch,
    and the fixed evidence ID. Any failure, oversize, malformed value or
    mismatch freezes to STRICT."""
    try:
        if reader is None:
            reader = _fixed_sysctl_reader()
        try:
            uuid_raw = reader("kern.uuid", 64)
            if len(uuid_raw) > 64:
                return {"mode": "strict", "reason": "identity_malformed", "evidence": ALLOW_EVIDENCE_ID}
        except Exception as error:
            reason = "identity_malformed" if _is_oversize(error) else "identity_read_error"
            return {"mode": "strict", "reason": reason, "evidence": ALLOW_EVIDENCE_ID}
        if len(uuid_raw) == 16:
            if uuid_raw != ALLOW_KERNEL_UUID_BIN:
                return {"mode": "strict", "reason": "identity_mismatch", "evidence": ALLOW_EVIDENCE_ID}
        elif len(uuid_raw) == 37 and uuid_raw[-1:] == b"\x00":
            try:
                uuid_text = uuid_raw[:-1].decode("ascii")
            except BaseException:
                return {"mode": "strict", "reason": "identity_malformed", "evidence": ALLOW_EVIDENCE_ID}
            if "\x00" in uuid_text:
                return {"mode": "strict", "reason": "identity_malformed", "evidence": ALLOW_EVIDENCE_ID}
            if uuid_text == ALLOW_KERNEL_UUID_STR:
                pass
            elif len(uuid_text) == 36:
                return {"mode": "strict", "reason": "identity_mismatch", "evidence": ALLOW_EVIDENCE_ID}
            else:
                return {"mode": "strict", "reason": "identity_malformed", "evidence": ALLOW_EVIDENCE_ID}
        else:
            return {"mode": "strict", "reason": "identity_malformed", "evidence": ALLOW_EVIDENCE_ID}
        try:
            build_raw = reader("kern.osversion", 64)
            if len(build_raw) > 64:
                return {"mode": "strict", "reason": "identity_malformed", "evidence": ALLOW_EVIDENCE_ID}
        except Exception as error:
            reason = "identity_malformed" if _is_oversize(error) else "identity_read_error"
            return {"mode": "strict", "reason": reason, "evidence": ALLOW_EVIDENCE_ID}
        if build_raw == ALLOW_KERNEL_BUILD:
            pass
        elif _ascii_nul_text(build_raw) is not None:
            return {"mode": "strict", "reason": "identity_mismatch", "evidence": ALLOW_EVIDENCE_ID}
        else:
            return {"mode": "strict", "reason": "identity_malformed", "evidence": ALLOW_EVIDENCE_ID}
        try:
            version_raw = reader("kern.version", 1024)
            if len(version_raw) > 1024:
                return {"mode": "strict", "reason": "identity_malformed", "evidence": ALLOW_EVIDENCE_ID}
        except Exception as error:
            reason = "identity_malformed" if _is_oversize(error) else "identity_read_error"
            return {"mode": "strict", "reason": reason, "evidence": ALLOW_EVIDENCE_ID}
        if not version_raw or version_raw[-1:] != b"\x00" or version_raw.count(b"\x00") != 1:
            return {"mode": "strict", "reason": "identity_malformed", "evidence": ALLOW_EVIDENCE_ID}
        try:
            version_text = version_raw[:-1].decode("ascii")
        except BaseException:
            return {"mode": "strict", "reason": "identity_malformed", "evidence": ALLOW_EVIDENCE_ID}
        tokens = version_text.split()
        if not tokens:
            return {"mode": "strict", "reason": "identity_malformed", "evidence": ALLOW_EVIDENCE_ID}
        exact = sum(1 for token in tokens if token == ALLOW_KERNEL_VERSION_TOKEN)
        others = [token for token in tokens
                  if token != ALLOW_KERNEL_VERSION_TOKEN and ("xnu-" in token or "RELEASE_" in token)]
        if exact == 1 and not others:
            return {"mode": "allowlisted", "reason": "allowlisted", "evidence": ALLOW_EVIDENCE_ID}
        if exact >= 1:
            return {"mode": "strict", "reason": "identity_malformed", "evidence": ALLOW_EVIDENCE_ID}
        return {"mode": "strict", "reason": "identity_mismatch", "evidence": ALLOW_EVIDENCE_ID}
    except BaseException:
        return {"mode": "strict", "reason": "identity_read_error", "evidence": ALLOW_EVIDENCE_ID}


def _terminal_witness_row(row, was_tracked):
    """Optional witness row for a terminal-accepted identity.

    Fixed keys only: pid/start plus both qualifying SZOMB states and
    prior tracking, all fixed enums/bools and bounded ints. Returns None
    when the identity is not exactly witnessable (never sanitized)."""
    try:
        pid = _diag_u64(row.get("pid")) if isinstance(row, dict) else None
        start = _diag_u64(row.get("start")) if isinstance(row, dict) else None
        if pid is None or start is None:
            return None
        snapshot_zombie = row.get("zombie") if isinstance(row, dict) else None
        if snapshot_zombie is not True:
            return None
        return {"pid": pid, "start": start, "snapshot_zombie": True,
                "first_zombie": True, "tracked": bool(was_tracked)}
    except BaseException:
        return None


def _fit_witnesses(witnesses, diag_kept_count, diag_compact_len):
    """Fit optional witness rows into the remainder of the combined budget.

    The combined cap is at most 20 rows and 16384 compact bytes shared
    with diagnostics; witnesses take only the remainder after the
    diagnostics object built under its own rule. Drops whole witness
    rows from the end first (loss is missing qualification evidence
    only); never changes classification or counts. Never raises."""
    try:
        budget_rows = DIAG_MAX_ROWS - diag_kept_count
        if budget_rows < 0:
            budget_rows = 0
        kept = list(witnesses[:budget_rows])
        blob = json.dumps({"terminal_witnesses": kept},
                          ensure_ascii=True, separators=(',', ':')).encode("utf-8")
        while kept and diag_compact_len + len(blob) > DIAG_MAX_BYTES:
            kept = kept[:-1]
            blob = json.dumps({"terminal_witnesses": kept},
                              ensure_ascii=True, separators=(',', ':')).encode("utf-8")
        return kept
    except BaseException:
        return []


class Census:
    """Classifies every process against session `sid` (the leader's PID).

    Member: getsid reads `sid` with the PID and start token unchanged across
    the call (KERN_PROC_PID re-read). A member stays tracked by (PID, start)
    until a snapshot lacks that identity, whatever its later session, UID or
    parent. Positively foreign: a validated other session, never a member.
    Gone: getsid ESRCH and a later successful KERN_PROC_PID read lacks the
    identity; a zombie still present is re-read within the round, at most
    CONFIRM_TRIES times. When that confirmation does not establish absence,
    the round's existing final KERN_PROC_PID read for the same PID is still
    checked under the same rule: a successful read showing None or a
    different start token establishes Gone and drops only the old tracked
    (PID, start) identity. A replacement identity is never declared foreign
    or member by inference. A failed read confirms nothing.

    Confirmation scheduling (oxide#147 correction): the FIRST confirmation
    read for an actual-ESRCH row runs immediately after that row's own getsid
    and before the next row's, so the snapshot/first-read pairing terminal
    eligibility depends on stays adjacent in time. Every identity still
    pending then takes part in shared confirmation passes: each pass is ONE
    `CONFIRM_PAUSE` followed by ONE reread of each pending identity in
    snapshot order, for at most CONFIRM_TRIES - 1 passes. An identity
    therefore still gets exactly its existing 1..CONFIRM_TRIES confirmation
    reads with at least one full pause between successive reads, but a single
    pause serves every identity in the pass, so the round's own wait no longer
    grows with the number of zombie rows. Per-identity tries, first-read
    pairing, contradiction tracking, absent/replaced priority, read-failure
    termination and the separate final read are all unchanged, a match never
    short-circuits, and members, uncertain rows, diagnostics and terminal
    witnesses are still assembled in snapshot order from per-row records
    rather than in resolution order.

    Terminal (allowlisted kernels only): an actual getsid ESRCH (errno-
    checked, not any ProcessLookupError) whose snapshot row and FIRST
    existing confirmation read show the same complete SZOMB identity
    (exact non-bool bounded ints, positive PID, exact SZOMB state in
    both) is accepted as a terminal nonwriter without claiming
    disappearance, provided every actually performed read in the round
    (all confirmation reads plus the existing final read, same 20+final
    bounds, no early short-circuit) shows the same complete SZOMB
    identity with no failure. Terminal is neither Gone (which keeps
    priority) nor Foreign nor a blocking member; tracked attribution is
    kept and eligibility is recomputed every round with no exemption
    cache. A nonqualifying first read is never salvaged by later reads;
    any failed, missing, malformed, identity/state or non-zombie
    observation in the round disqualifies terminal and taints the round's
    terminal count as unaccountable. Rows that were never terminal
    candidates (no SZOMB snapshot, non-ESRCH failures) leave an
    accountable count alone. A successful getsid keeps the old
    member/foreign/tracked paths and other session errors can never
    qualify. In strict mode terminal acceptance never fires and every
    ESRCH row keeps the old uncertain handling, while the terminal count
    stays the accountably known valid 0.

    Everything else (other getsid errors, identity change, failed or
    missing metadata, unconfirmed zombies, a failed snapshot) is uncertain
    and blocks closure, first-seen rows included. Start time is never used
    to exclude a process.

    The round also builds an optional, monitor-receipt-only diagnostics
    object summarizing ESRCH rows (confirm attempts/outcome and the final
    read result with an enumerated error category). It never leaves this
    round's return value: the census IPC message keeps only
    after/members/uncertain/certain, and the diagnostics never change
    classification, membership, authoritative counts, first fault, closure
    or cleanup, nor consume extra reads, time or retry budget.

    Every round also carries mandatory receipt-only terminal fields:
    `terminal_count_status` (valid/invalid/overflow) and
    `terminal_nonwriters` (exact non-bool int 0..U64MAX when valid, else
    JSON null). The value counts this round's positively verified
    still-present SZOMB identities accepted by the terminal rule, never
    Gone rows; a strictly completed round with none is valid 0. Any
    uncertain row or a snapshot/census failure without a countable set
    is invalid with null (never a raw bad value, false zero or lower
    bound); overflow is likewise null. Invalid and
    overflow always coincide with uncertain>=1 and certain False in the
    unchanged IPC fields. Output-witness loss never touches these fields
    and they never change classification."""

    def __init__(self, sid, snapshot=Kinfo.all, getsid=os.getsid, reread=Kinfo.one, pause=time.sleep,
                 terminal_allowed=False):
        self.sid, self.snapshot, self.getsid, self.reread, self.pause = sid, snapshot, getsid, reread, pause
        self.terminal_allowed = terminal_allowed if terminal_allowed is True else False
        self.tracked = {}
        # Observation-only per-round counters (oxide#147). They are reset at the
        # start of every round, are never added to the round's returned object
        # and never influence a classification, count, read, pause or budget.
        self.stats = {}

    def reset_stats(self):
        """Start this round's observation-only counters. Never raises."""
        try:
            self.stats = {"rows": 0, "esrch_rows": 0, "zombie_rows": 0, "confirm_reads": 0,
                          "pause_calls": 0, "terminal": 0, "snapshot_ns": 0, "scan_ns": 0,
                          "snapshot_error": False}
        except BaseException:
            self.stats = {}

    def confirmation_attempt(self, row, state):
        """Exactly one confirmation read for one pending ESRCH identity.

        Applies the same priority and bookkeeping as one attempt of the
        serial loop it replaces: a successful read lacking the identity is
        Gone (`absent`) and a successful read naming a different start token
        is Gone (`replaced`); a failed read resolves the identity as
        `read_failed` with `all_clean` false; anything else keeps it pending
        and updates `all_clean` and the FIRST-read pairing the terminal rule
        needs. `state` carries tries/outcome/error/first_row/first_error/
        clean/gone across passes, so an identity's read count, first-read
        pairing, contradiction tracking and resolution stay identical however
        many other identities share its pass. Never pauses, never
        short-circuits a match, and counts only the read it makes."""
        state["tries"] += 1
        try:
            self.stats["confirm_reads"] += 1  # observation only
        except BaseException:
            pass
        first = state["first_row"] == "unset"
        try:
            again = self.reread(row["pid"])
        except Exception as error:
            if first:
                state["first_error"] = error
            state.update(outcome="read_failed", error=error, clean=False)
            return False
        if first:
            state["first_row"], state["first_error"] = again, None
        if again is None:
            state.update(outcome="absent", gone=True)
            return False
        if again["start"] != row["start"]:
            state.update(outcome="replaced", gone=True)
            return False
        if not _same_complete_zombie(row, again):
            state["clean"] = False
        return True

    def retains_member_attribution(self, identity):
        """Whether an unresolved ESRCH identity keeps its retained member
        attribution instead of being reported uncertain.

        This is the serial tail rule at 8849f99: a tracked identity that is
        neither positively Gone nor terminal-accepted stays a member. It is a
        separate seam so the differential can falsify exactly this branch."""
        return identity in self.tracked

    def terminal_eligible(self, row, first_row, first_error):
        """Whether an actual-ESRCH row qualifies as terminal nonwriter.

        Requires allowlisted mode plus the snapshot row and the FIRST
        existing confirmation read showing the same complete SZOMB
        identity (exact non-bool bounded ints, positive PID, exact SZOMB
        state in both). Pure predicate: no reads, no writes, never
        raises. Later-read contradictions are tracked separately by the
        caller; this predicate covers only the snapshot/first-read pair."""
        try:
            if self.terminal_allowed is not True:
                return False
            if first_error is not None:
                return False
            return _same_complete_zombie(row, first_row)
        except BaseException:
            return False

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
        diag_attempted, diag_saturated = 0, False
        terminal_count, terminal_witnesses, terminal_tainted = 0, [], False
        # One outcome record per snapshot row. Confirmation reads are
        # interleaved across identities, so every published list (members,
        # uncertain, diagnostics, terminal witnesses) is assembled from these
        # records in SNAPSHOT order at the end of the round, never in the
        # order in which a pass happened to resolve an identity.
        records = [None] * len(rows)
        pending = []  # (index, row, identity, state) still being confirmed, snapshot order
        for index, row in enumerate(rows):
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
                state = {"tries": 0, "outcome": None, "error": None, "first_row": "unset",
                         "first_error": None, "clean": True, "gone": False, "getsid_error": getsid_error}
                # The FIRST confirmation read for this identity runs here,
                # immediately after its own getsid and before the next row's,
                # so the snapshot/first-read pairing terminal eligibility
                # depends on keeps its existing timing adjacency.
                if self.confirmation_attempt(row, state):
                    pending.append((index, row, identity, state))
                    continue
                if state["gone"]:
                    self.tracked.pop(identity, None)
                    records[index] = {"diag": _diag_attempt(row, state["tries"], state["outcome"], "not_read")}
                    continue
                # A failed first read resolves the confirmation exactly as the
                # serial loop did: the identity keeps the existing final read.
                records[index] = {"esrch": True, "row": row, "identity": identity, "state": state}
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
                records[index] = {"member": row}
            elif identity in self.tracked:
                records[index] = {"member": row}
            elif failure is None and stable:
                records[index] = {"skip": True}  # positively foreign
            else:
                records[index] = {"uncertain": dict(row, reason=failure or "identity changed across getsid")}
        # Further confirmation passes: each is ONE shared CONFIRM_PAUSE and
        # then ONE reread of every still-pending identity in snapshot order.
        # One pause therefore serves every identity in the pass, while any two
        # successive confirmation reads for one identity stay at least one
        # full CONFIRM_PAUSE apart. Per-identity tries, all_clean, first-read
        # pairing, absent/replaced priority and read-failure termination are
        # unchanged, and a match never short-circuits.
        further = 0
        while pending and further < CONFIRM_TRIES - 1:
            further += 1
            self.pause(CONFIRM_PAUSE)
            try:
                self.stats["pause_calls"] += 1  # observation only: this round's shared pauses
            except BaseException:
                pass
            survivors = []
            for index, row, identity, state in pending:
                if self.confirmation_attempt(row, state):
                    survivors.append((index, row, identity, state))
                elif state["gone"]:
                    self.tracked.pop(identity, None)
                    records[index] = {"diag": _diag_attempt(row, state["tries"], state["outcome"], "not_read")}
                else:
                    records[index] = {"esrch": True, "row": row, "identity": identity, "state": state}
            pending = survivors
        for index, row, identity, state in pending:  # 20 reads done, absence never shown
            state["outcome"] = "matching_exhausted"
            records[index] = {"esrch": True, "row": row, "identity": identity, "state": state}
        # The existing final KERN_PROC_PID read and classification, once per
        # surviving ESRCH identity, in snapshot order: 20 confirmation reads
        # plus this distinct final read for a persistent row, while a Gone
        # during confirmation skips it and a read failure keeps the old
        # final-absence rule. No pause is added before it.
        for record in records:
            if not record or not record.get("esrch"):
                continue
            row, identity, state = record["row"], record["identity"], record["state"]
            tries, confirm_outcome = state["tries"], state["outcome"]
            failure = "getsid ESRCH, absence not confirmed"
            try:
                again = self.reread(row["pid"])
            except Exception as error:
                # The serial algorithm also reached its ordinary tail here, so
                # a tracked identity keeps its retained attribution as a member
                # and only an untracked one becomes uncertain. Marking every
                # failed final read uncertain would change that result.
                again = None
                failure = f"reread: {error}"
                record["diag"] = _diag_attempt(row, tries, confirm_outcome, "read_failed", error=error)
                try:
                    if self.terminal_allowed is True and row.get("zombie") is True:
                        terminal_tainted = True
                except BaseException:
                    pass
                if self.retains_member_attribution(identity):
                    record["member"] = row  # retained attribution, exactly as the serial tail
                else:
                    record["uncertain"] = dict(row, reason=failure)
                continue
            if again is None:
                self.tracked.pop(identity, None)
                record["diag"] = _diag_attempt(row, tries, confirm_outcome, "absent")
                continue
            if again["start"] != row["start"]:
                self.tracked.pop(identity, None)
                record["diag"] = _diag_attempt(row, tries, confirm_outcome, "replaced")
                continue
            record["diag"] = _diag_attempt(row, tries, confirm_outcome, "same_identity",
                                           error=state["getsid_error"])
            try:
                terminal = (confirm_outcome == "matching_exhausted" and state["clean"]
                            and self.terminal_eligible(row, state["first_row"], state["first_error"])
                            and _same_complete_zombie(row, again))
            except BaseException:
                terminal = False
            if not terminal:
                # An actual-ESRCH row with a SZOMB snapshot that is neither
                # Gone nor terminal-accepted stays unaccounted and taints the
                # terminal count, but only when terminal acceptance is
                # possible at all. In strict mode the count is accountably
                # zero; rows that were never terminal candidates (no SZOMB
                # snapshot) leave an accountable count alone.
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
                record["skip"] = True  # kept tracked attribution; neither member nor uncertain
                continue
            if self.retains_member_attribution(identity):
                record["member"] = row  # tracked attribution kept
            else:
                record["uncertain"] = dict(row, reason=failure)
        diag_rows = []
        for record in records:  # SNAPSHOT order, including more than 20 rows
            if not record:
                continue
            if record.get("member") is not None:
                members.append(record["member"])
            elif record.get("uncertain") is not None:
                uncertain.append(record["uncertain"])
            if record.get("diag") is not None:
                diag_rows.append(record["diag"])
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


# Guard and precedence (custodian side, pure) -------------------------------------


class Guard:
    """Newest completed readings and the latched first fault."""

    def __init__(self, start_ns, threshold=THRESHOLD, budget=SCRATCH_BUDGET):
        self.start_ns, self.threshold, self.budget = start_ns, threshold, budget
        self.reserve_at = self.scratch_at = None
        self.fault = None

    def latch(self, fault):
        if self.fault is None:
            self.fault = fault

    def reading(self, message, now=None):
        """Freshness counts from `completed`, the moment the measurement
        finished, never from when it was sent: a delayed publication cannot
        refresh an old observation. Same system-wide clock on both sides, so
        a completion after the send stamp, or a send stamp after `now`, is
        refused rather than trusted."""
        kind = message["kind"]
        if now is not None and message["at"] > now:
            self.latch(f"{kind} reading stamped in the future")
        elif message.get("error"):
            self.latch(f"{kind} measurement failed: {message['error']}")
        elif message["completed"] > message["at"]:
            self.latch(f"{kind} reading completed after it was sent")
        elif kind == "reserve":
            self.reserve_at = max(self.reserve_at or 0, message["completed"])
            if message["value"] < self.threshold:
                self.latch(f"host free {message['value']} bytes below the {self.threshold} byte abort threshold")
        elif message.get("complete"):
            self.scratch_at = max(self.scratch_at or 0, message["completed"])
            if message["value"] > self.budget:
                self.latch(f"owned bytes {message['value']} over the {self.budget} byte budget")

    def check(self, now):
        for kind, at, stale in (("reserve", self.reserve_at, RESERVE_STALE_NS),
                                ("scratch", self.scratch_at, SCRATCH_STALE_NS)):
            if now - (self.start_ns if at is None else at) > stale:
                self.latch(f"{kind} readings stale: none in {stale / NS:g} s")
        return self.fault


def custodian_deadline_hit(now, absolute, inject):
    """The custodian's deadline flag. The injected `custodian-timeout-after-monitor`
    stands in for the deadline actor firing right after the decision, so it keeps
    deadline semantics (exit 124); a real monitor that has not completed by the
    exit margin is a fault, and only the absolute deadline is a real deadline."""
    return now >= absolute or "custodian-timeout-after-monitor" in inject


def exit_code(deadline_hit, fault, closed, status):
    """124 deadline; 125 fault or UNCLOSED; else the command's own status."""
    if deadline_hit:
        return 124
    if fault is not None or not closed or status is None:
        return 125
    if os.WIFEXITED(status):
        return os.WEXITSTATUS(status)
    if os.WIFSIGNALED(status):
        return 128 + os.WTERMSIG(status)
    return 125


# Monitor -------------------------------------------------------------------------


PATH_LIMIT_FALLBACK = 1024  # a smaller limit than the platform's only costs descriptor walks


def path_limit(root):
    """Byte length of the longest absolute path name, its terminating NUL
    included, that this platform can open below `root`; a name shorter than
    this is nameable by path. An unavailable answer takes the conservative
    smaller limit: descriptor-relative opens work at any length."""
    try:
        value = os.pathconf(root, "PC_PATH_MAX")
    except (OSError, ValueError):
        value = 0
    return value if isinstance(value, int) and value > 0 else PATH_LIMIT_FALLBACK


def open_relative(anchor, names):
    """Open the directory `anchor` and then each of the short component
    `names` below it, closing every descriptor as it is replaced. No symlink
    is followed at any step, so a substitution is an error, not a walk into
    another tree. `names` is empty for the anchor itself."""
    fd = os.open(anchor, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for name in names:
            child = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = child
    except BaseException:
        os.close(fd)
        raise
    return fd


def tree_bytes(root):
    """Allocated bytes under `root`: `st_blocks * 512` for every distinct
    (device, inode), with no symlink followed.

    A path name at or past the platform limit cannot be opened by absolute
    path at all, so a frame below the deepest nameable directory is reached
    from that directory one short descriptor-relative component at a time.
    No frame keeps a descriptor between iterations: the walk opens one
    directory, reads its names, closes it and moves on, so descriptor use
    stays constant whatever the depth or path length and every error closes
    what it opened. `PC_PATH_MAX` is only the choice between the two lookup
    forms: depth, entries and bytes are unbounded by it, and this is not an
    atomic snapshot.

    An entry that is gone is skipped exactly as before: at its stat, at the
    final no-follow open of a directory, or at that directory's own read. Every
    other error raises, so the sampler reports a measurement fault and never a
    smaller, unbounded or partly counted total; in particular a name whose
    observed kind or identity contradicts what the open finds is a fault, and
    only `FileNotFoundError` is ever treated as a vanished entry.
    """
    limit = path_limit(root)
    total, seen = 0, set()
    stack = [(None, (root,))]  # (nameable anchor, (absolute path,)) or (anchor, components)
    while stack:
        anchor, names = stack.pop()
        fd = None
        try:
            if anchor is None:
                try:
                    meta = os.lstat(names[0])
                except FileNotFoundError:
                    continue
            else:
                # One short component at a time from the nearest nameable
                # directory: the accumulated absolute name is never looked up.
                try:
                    fd = open_relative(anchor, names[:-1])
                except FileNotFoundError:
                    continue  # the entry's own ancestor went away: still skipped
                try:
                    meta = os.stat(names[-1], dir_fd=fd, follow_symlinks=False)
                except FileNotFoundError:
                    continue
            identity = (meta.st_dev, meta.st_ino)
            if identity in seen:
                continue
            seen.add(identity)
            total += meta.st_blocks * 512
            if not stat.S_ISDIR(meta.st_mode):
                continue
            if anchor is None:
                try:
                    children = os.listdir(names[0])
                except FileNotFoundError:
                    continue
                prefix = len(os.fsencode(names[0])) + 1  # bytes, not characters
                for name in children:
                    if prefix + len(os.fsencode(name)) < limit:
                        stack.append((None, (os.path.join(names[0], name),)))
                    else:
                        stack.append((names[0], (name,)))
            else:
                # The final component is opened no-follow too, and the same
                # vanished tolerance the pathname read had applies: a directory
                # that disappeared between its stat and this open is skipped.
                try:
                    inner = os.open(names[-1], os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
                except FileNotFoundError:
                    continue
                try:
                    children = os.listdir(inner)
                except FileNotFoundError:
                    continue
                finally:
                    os.close(inner)
                for name in children:  # already past the limit: stays relative
                    stack.append((anchor, names + (name,)))
        finally:
            if fd is not None:
                os.close(fd)
    return total


class SelectionRefused(Exception):
    """The default scratch parent cannot be used; the caller falls back."""


class DefaultProbe:
    """The facts the default selection reads: kernel metadata, two bounded
    documents and the account home. Tests substitute their own."""

    def stat(self, path):
        return os.stat(path)

    def lstat(self, path):
        return os.lstat(path)

    def realpath(self, path):
        return os.path.realpath(path)

    def volume_uuid(self, path):
        return volume_uuid(path)

    def euid(self):
        return os.geteuid()

    def home(self):
        """The account home, never `$HOME` (the product's rule)."""
        try:
            return pwd.getpwuid(self.euid()).pw_dir
        except KeyError:
            raise SelectionRefused("the account home is unavailable")

    def read(self, path, limit):
        info = self.lstat(path)
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
            raise SelectionRefused(f"{path} is not a regular file")
        if info.st_size > limit:
            raise SelectionRefused(f"{path} is larger than {limit} bytes")
        with open(path, "rb") as handle:
            return handle.read(limit + 1)

    def mount_point(self, path):
        """Whether `path` is currently a mount point (a kernel fact, not a
        remembered path)."""
        return os.path.ismount(path)

    def attachments(self, backing, timeout=None):
        """[(device, mount-point, volume-uuid)] of the attached image whose
        path is exactly `backing`, or None when that is unreadable."""
        return image_attachments(backing, DETACH_INFO_TIMEOUT_S if timeout is None else timeout)

    def detach(self, mount, timeout, still_allowed=None):
        """One bounded ORDINARY detach attempt; never a forced one."""
        return detach_image(mount, timeout, still_allowed)


_ATTR_BIT_MAP_COUNT = 5
_ATTR_VOL_UUID = 0x00040000  # sys/attr.h
_LIBC = [None]


class _AttrList(ctypes.Structure):
    """struct attrlist (sys/attr.h), for one getattrlist volume read."""

    _fields_ = [("bitmapcount", ctypes.c_uint16), ("reserved", ctypes.c_uint16),
                ("commonattr", ctypes.c_uint32), ("volattr", ctypes.c_uint32),
                ("dirattr", ctypes.c_uint32), ("fileattr", ctypes.c_uint32),
                ("forkattr", ctypes.c_uint32)]


def volume_uuid(path):
    """The `ATTR_VOL_UUID` of the volume holding `path`, lowercase hyphenated,
    or None when it cannot be read. The product's own identity primitive: a
    non-root directory answers its containing volume's UUID, so a stale
    directory on the internal disk is never mistaken for the drive."""
    if sys.platform != "darwin":
        return None
    try:
        if _LIBC[0] is None:
            _LIBC[0] = ctypes.CDLL(None, use_errno=True)
        attrs = _AttrList(_ATTR_BIT_MAP_COUNT, 0, 0, _ATTR_VOL_UUID, 0, 0, 0)
        buffer = ctypes.create_string_buffer(32)
        code = _LIBC[0].getattrlist(os.fsencode(path), ctypes.byref(attrs), buffer,
                                    ctypes.c_size_t(len(buffer)), 0)
        if code != 0:
            return None
        raw = buffer.raw
    except (OSError, AttributeError, ValueError):
        return None
    length = struct.unpack_from("<I", raw, 0)[0]
    if length != 20:  # the 4-byte length field plus exactly one 16-byte UUID
        return None
    data = raw[4:20]
    return "-".join((data[0:4].hex(), data[4:6].hex(), data[6:8].hex(), data[8:10].hex(), data[10:16].hex()))


#
# A proof that created its own attachable image publishes ONE bounded atomic
# request under OXIDE_PROOF_EVIDENCE after that image exists. Only then, and only
# on the monitor's `cleanup` decision (a positive descendant census after the
# leader was reaped), may this runner attempt exactly one ORDINARY detach of that
# exact image -- inside the same absolute deadline and under the same sampled
# reserve and owned-byte bounds -- before the owned scratch cleanup. A static
# command-line mount path could not identify a volume minted by the child, so the
# child names it and the runner re-derives every binding from kernel and
# `hdiutil` facts. No request means the runner's behaviour is unchanged and the
# receipt gains no detach object -- except for one fail-closed safety check: an
# owned root that still holds an attached image it was never asked about is kept,
# never cleaned.
#
# The custodian, which alone holds the fault, deadline and closure state, must
# also AUTHORIZE the single attempt: the monitor asks over the control pipe and
# the custodian answers with its own verdict and the wait budget that remains
# before it would declare the monitor incomplete. A refusal, a failed or
# timed-out attempt, an ambiguous post-state or an unreadable identity never
# detaches, is never retried and never removes a backing: the mount, backing and
# evidence are kept and the run is unqualified.
DETACH_REQUEST_NAME = "detach-request.json"
DETACH_REQUEST_VERSION = 1
DETACH_REQUEST_MAX_BYTES = 4096
DETACH_REQUEST_KEYS = frozenset({"version", "backing", "mount", "device", "volume_uuid", "note"})
DETACH_TEXT_LIMITS = {"backing": 1024, "mount": 256, "device": 64, "volume_uuid": 64, "note": 256}
DETACH_DEVICE_RE = re.compile(r"^/dev/disk[0-9]{1,4}(s[0-9]{1,3})?$")  # a whole disk or one mounted slice
DETACH_UUID_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
DETACH_INFO_TIMEOUT_S = 10
DETACH_INFO_MAX_BYTES = 1 * MIB  # a bounded `hdiutil info -plist` read, never an unbounded buffer
DETACH_TIMEOUT_S = 30
DETACH_GATE_MARGIN_NS = 1 * NS
DETACH_GATE_WAIT_S = 5.0
DETACH_OUTPUT_MAX_BYTES = 4096
DETACH_POLL_S = 0.2  # how often authority is re-checked while the detach child runs
# The opt-in image-custody indication (oxide#191). A proof that creates an
# attachable image publishes this BEFORE attaching it, so a request-free run can
# still be kept when the image is mounted or attached-but-unmounted at cleanup.
# No indication means no scan at all: the default runner path is unchanged and
# gains no new failure mode.
CUSTODY_NAME = "image-custody.json"
CUSTODY_VERSION = 1
CUSTODY_MAX_BYTES = 2048
CUSTODY_MAX_BACKINGS = 8
DETACH_MIN_BUDGET_NS = 8 * NS  # below this the whole sequence cannot fit the custodian's window
DRAIN_WAIT_S = 0.5  # one short bounded drain of post-decision records, inside the window
INFO_ORPHANS = []  # attachment reads whose child was not reaped; custody is never dropped
DETACH_POST_RESERVE_S = 5.0  # kept for the post-state read after the child
DETACH_STATES = ("refused", "detached", "failed", "timed_out", "withdrawn", "unknown")
DETACH_POST_STATES = ("detached", "still_mounted", "still_attached", "unknown")
HDIUTIL = "/usr/bin/hdiutil"
_ABSENT = object()  # a request file that does not exist, distinct from one that is not an object


def _bounded_tail(tail, chunk, cap=DETACH_OUTPUT_MAX_BYTES):
    """The last `cap` bytes of `tail + chunk`, so peak memory is one cap plus one
    read, whatever a child writes."""
    combined = tail + chunk
    return combined[-cap:] if len(combined) > cap else combined


def bounded_run(argv, timeout, cap=DETACH_OUTPUT_MAX_BYTES, still_allowed=None, spawn=None):
    """One bounded child: at most `timeout` seconds and at most `cap` bytes kept,
    however much it writes. `still_allowed()` is asked at `DETACH_POLL_S`
    intervals while the child runs, so authority withdrawn mid-attempt stops it
    instead of waiting out the bound; a False answer KILLs and reaps the child
    here and is reported as `withdrawn`. Returns the attempt's own facts (never
    raises for a spawn failure). `spawn` substitutes the process factory."""
    if timeout <= 0:  # a spent window never spawns a child at all
        return {"exit": None, "timed_out": False, "error": "no time left in the window", "output": "",
                "truncated": False, "withdrawn": None, "reap_timed_out": False, "reaped": False,
                "handle": None}
    spawn = spawn or subprocess.Popen
    try:
        child = spawn(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                      stderr=subprocess.STDOUT, close_fds=True)
    except OSError as error:
        return {"exit": None, "timed_out": False, "error": errno_name(error), "output": "",
                "truncated": False, "withdrawn": None, "reap_timed_out": False, "reaped": False,
                "handle": None}
    deadline = time.monotonic() + timeout
    tail, seen, timed_out, withdrawn, reap_timed_out, reaped = b"", 0, False, None, False, False
    poll_at = time.monotonic() + DETACH_POLL_S
    try:
        # One owner for the descriptor: detaching the buffer gives this function
        # the raw stream, so the descriptor is closed exactly once here and never
        # left to a buffered object's finalizer while another thread may have
        # reused the number.
        raw = child.stdout.detach()
        fd = raw.fileno()
    except BaseException as error:
        return _unreaped_record(child, error, deadline)
    try:
        try:
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    timed_out = True
                    break
                if still_allowed is not None and time.monotonic() >= poll_at:
                    allowed, reason = still_allowed()
                    poll_at = time.monotonic() + DETACH_POLL_S
                    if not allowed:
                        withdrawn = reason or "authority withdrawn"
                        break
                try:
                    ready, _, _ = select.select(
                        [fd], [], [], max(0.0, min(0.25, remaining,
                                                   max(0.0, poll_at - time.monotonic()))))
                except OSError:
                    ready = [fd]  # readable in the failing sense: the read reports it
                if not ready:
                    if child.poll() is not None:
                        continue  # drain what is left, then EOF ends the loop
                    continue
                try:
                    chunk = os.read(fd, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                seen += len(chunk)
                tail = _bounded_tail(tail, chunk, cap)
        finally:
            # Close the descriptor through its single owner BEFORE any wait, then
            # drop the detached buffer so nothing can close it a second time.
            try:
                raw.close()
            except OSError:
                pass
            try:
                child.stdout = None
            except BaseException:
                pass
            # The reap is bounded by what is left of the SAME deadline: no extra
            # fixed wait and no round that can run past it. Any exception here
            # still hands the live child back through the outer handler.
            if timed_out or withdrawn is not None:
                try:
                    child.kill()
                except OSError:
                    pass
            for _round in (1, 2, 3):  # three bounded rounds, each inside the deadline
                remaining = max(0.0, deadline - time.monotonic())
                try:
                    reaped = child.wait(timeout=remaining) is not None
                except subprocess.TimeoutExpired:
                    reaped = False
                if reaped:
                    break
                if remaining <= 0:
                    break  # the window is spent: the child stays unaccounted
                reap_timed_out = True
                try:
                    child.kill()
                except OSError:
                    pass
    except BaseException as error:
        return _unreaped_record(child, error, deadline)
    status = child.returncode
    return {"exit": status, "timed_out": timed_out, "error": None,
            "output": tail.decode("utf-8", "replace"), "truncated": seen > len(tail),
            "withdrawn": withdrawn, "reap_timed_out": reap_timed_out, "reaped": reaped,
            "handle": None if reaped else child}


def detach_text(document, name):
    """One bounded, single-line request field, or None when it is not one."""
    if not isinstance(document, dict):
        return None
    value = document.get(name)
    if not isinstance(value, str) or not value:
        return None
    if len(value.encode("utf-8", "replace")) > DETACH_TEXT_LIMITS[name]:
        return None
    if any(ord(character) < 32 or ord(character) == 127 for character in value):
        return None
    return value


def read_detach_request(evidence, probe):
    """(document, problem) for this run's request, or (_ABSENT, None) when the
    proof published none. Bounded, symlink-refusing and never partly trusted; a
    document that is present but not an object (including JSON `null`) is a
    refusal, never an absent request."""
    path = os.path.join(evidence, DETACH_REQUEST_NAME)
    try:
        info = probe.lstat(path)
    except FileNotFoundError:
        return _ABSENT, None
    except OSError as error:
        return None, f"request {errno_name(error)}"
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
        return None, "request is not a regular file"
    if info.st_size > DETACH_REQUEST_MAX_BYTES:
        return None, f"request is over {DETACH_REQUEST_MAX_BYTES} bytes"
    if os.path.dirname(os.path.realpath(path)) != os.path.realpath(evidence):
        return None, "request is not directly inside this run's evidence directory"
    try:
        with open(path, "rb") as handle:
            raw = handle.read(DETACH_REQUEST_MAX_BYTES + 1)
    except OSError as error:
        return None, f"request {errno_name(error)}"
    if len(raw) > DETACH_REQUEST_MAX_BYTES:
        return None, f"request is over {DETACH_REQUEST_MAX_BYTES} bytes"
    try:
        return json.loads(raw.decode("utf-8")), None
    except (UnicodeDecodeError, ValueError):
        return None, "request is not a JSON document"


def image_attachments(backing=None, timeout=DETACH_INFO_TIMEOUT_S, run=None):
    """[(device, mount-point, volume-uuid)] for the attached image whose path is
    exactly `backing` (or, with no backing, `[(image-path, device, mount-point,
    volume-uuid)]` for every attachment), or None when the facts are unreadable.

    Fail closed: one bounded read-only `hdiutil info`, and ANY malformed or
    incomplete relevant structure -- a document without an images list, an image
    that is not an object or has no path, a non-list entity list, an entity that
    is not an object or has no device, a non-string mount or UUID, or a matching
    image with no entities at all -- is unreadable facts (None), never a smaller
    or empty attachment set. Callers treat None as uncertain: a detach post-state
    is unknown and a custody check keeps the root. `run` substitutes the child
    runner (the parser's own seam)."""
    attempt = (run or bounded_run)([HDIUTIL, "info", "-plist"], timeout, DETACH_INFO_MAX_BYTES)
    if attempt.get("handle") is not None:
        # A read whose child could not be reaped is unreadable AND its custody is
        # kept: the caller must close or hand it over before completing.
        INFO_ORPHANS.append(attempt["handle"])
        return None
    if attempt["exit"] != 0 or attempt["timed_out"] or attempt["truncated"] or not attempt["output"]:
        return None
    try:
        document = plistlib.loads(attempt["output"].encode("utf-8"))
    except Exception:
        return None
    if not isinstance(document, dict):
        return None
    images = document.get("images")
    if not isinstance(images, list):
        return None
    canonical = None if backing is None else os.path.realpath(backing)
    # Validate EVERY image and entity before filtering for the declared backing: a
    # malformed unrelated entry could be the one that belongs to it, so it makes
    # the whole answer unreadable rather than "this backing is not attached".
    parsed, paths = [], []
    for image in images:
        if not isinstance(image, dict):
            return None
        path = image.get("image-path")
        if not isinstance(path, str) or not path:
            return None
        paths.append(os.path.realpath(path))
        entities = image.get("system-entities", [])
        if not isinstance(entities, list) or not entities:
            # An attached image with no entities (or a non-list) is not a fact that
            # proves anything is detached or clear.
            return None
        for entity in entities:
            if not isinstance(entity, dict):
                return None
            device = entity.get("dev-entry")
            if not isinstance(device, str) or not device:
                return None
            mount = entity.get("mount-point")
            if mount is not None and (not isinstance(mount, str) or not mount):
                return None
            identifier = entity.get("volume-uuid")
            if identifier is not None and (not isinstance(identifier, str) or not identifier):
                return None
            parsed.append((path, device, mount, identifier.lower() if identifier else None))
    if canonical is None:
        return [(device, mount, identifier) for _path, device, mount, identifier in parsed]
    mine = [row for row in parsed if os.path.realpath(row[0]) == canonical]
    if canonical in paths and not mine:
        return None  # the declared backing is attached with no entities at all
    return [(device, mount, identifier) for _path, device, mount, identifier in mine]


def read_custody(ctx, probe):
    """(backings, problem) for this run's opt-in image-custody indication, or
    (None, None) when the proof published none. Bounded, symlink-refusing and
    never partly trusted: a present but unreadable indication is a problem, never
    an absent one."""
    path = os.path.join(ctx["evidence"], CUSTODY_NAME)
    try:
        info = probe.lstat(path)
    except FileNotFoundError:
        return None, None
    except OSError as error:
        return None, f"custody indication {errno_name(error)}"
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
        return None, "custody indication is not a regular file"
    if info.st_size > CUSTODY_MAX_BYTES:
        return None, f"custody indication is over {CUSTODY_MAX_BYTES} bytes"
    try:
        with open(path, "rb") as handle:
            raw = handle.read(CUSTODY_MAX_BYTES + 1)
    except OSError as error:
        return None, f"custody indication {errno_name(error)}"
    if len(raw) > CUSTODY_MAX_BYTES:
        return None, f"custody indication is over {CUSTODY_MAX_BYTES} bytes"
    try:
        document = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, ValueError):
        return None, "custody indication is not a JSON document"
    if not isinstance(document, dict) or set(document) != {"version", "backings"}:
        return None, "custody indication is not an object with exactly version and backings"
    if not typed(document.get("version"), int) or document.get("version") != CUSTODY_VERSION:
        return None, f"custody indication version is not {CUSTODY_VERSION}"
    backings = document.get("backings")
    if not isinstance(backings, list) or not backings or len(backings) > CUSTODY_MAX_BACKINGS:
        return None, f"custody indication names no bounded non-empty backing list (max {CUSTODY_MAX_BACKINGS})"
    root, evidence = probe.realpath(ctx["root"]), probe.realpath(ctx["evidence"])
    for backing in backings:
        if not isinstance(backing, str) or not backing or len(backing) > DETACH_TEXT_LIMITS["backing"]:
            return None, "custody indication names an unbounded backing"
        if any(ord(character) < 32 for character in backing):
            return None, "custody indication names a backing with control characters"
        if not os.path.isabs(backing) or os.path.normpath(backing) != backing:
            return None, "custody indication names a non-canonical backing"
        if probe.realpath(backing) != backing:
            return None, "custody indication names a backing that resolves through a symlink"
        if not (backing.startswith(root + os.sep) or backing.startswith(evidence + os.sep)):
            return None, "custody indication names a backing outside this run's owned root and evidence"
    return [str(backing) for backing in backings], None


def custody_problems(paths, probe, facts=None, timeout=DETACH_INFO_TIMEOUT_S, deadline_ns=None):
    """Why a decayed custody record must keep the root, from attachment facts and
    device nodes: a mounted entity is live, an entity without a mount point is
    live when its /dev node still exists (attached but unmounted) and stale
    metadata only when that node is positively gone; unreadable facts are
    uncertain and keep. An empty result means the backings are positively
    detached. With an absolute `deadline_ns`, the deadline is rechecked before
    EVERY backing read and each read gets only what is left, so earlier reads can
    never buy a later one a fresh allowance."""
    read = facts or image_attachments
    problems = []
    for index, backing in enumerate(paths):
        budget = timeout
        if deadline_ns is not None:
            remaining = (deadline_ns - now_ns()) / NS
            if remaining <= 0.0:
                problems.append(f"{backing}: no custodian wait window is left to read its facts")
                break
            # The deadline is rechecked here AND the remainder is shared with the
            # backings still to read, so neither a slow earlier read nor several
            # reads can outlive the custodian's window.
            # No floor: a floor larger than a small positive remainder would grant a
            # read more time than the custodian's window still allows.
            budget = min(timeout, remaining / max(1, len(paths) - index))
        rows = read(backing, budget)
        if rows is None:
            problems.append(f"{backing}: attachment facts are unreadable")
            continue
        for device, mount, _uuid in rows:
            if mount is not None:
                problems.append(f"{backing}: {device} is mounted at {mount}")
                break
            try:
                probe.lstat(device)
            except FileNotFoundError:
                continue  # stale metadata: the device node is positively gone
            except OSError as error:
                problems.append(f"{backing}: {device} state is unreadable ({errno_name(error)})")
                break
            problems.append(f"{backing}: {device} is attached without a mount point")
            break
    return problems


def cleanup_safety(ctx, probe=None, facts=None):
    """The opt-in, fail-closed pre-cleanup check. It runs ONLY when the proof
    published an image-custody indication, so a run without one performs no scan
    and keeps the default path unchanged. With an indication, an owned root is
    never cleaned while one of those backings is mounted or attached-but-unmounted
    (a proof that failed before publishing its detach request can leave exactly
    that); a positively absent device node is stale metadata and keeps nothing.
    Returns a bounded `cleanup` record when the root must be kept, else None."""
    probe = probe or DefaultProbe()
    try:
        backings, problem = read_custody(ctx, probe)
    except BaseException as error:
        backings, problem = None, f"custody indication unreadable: {type_name(error)}"
    if backings is None and problem is None:
        return None  # not opted in: no scan and no gate, so the default path is unchanged
    if problem is not None:
        return {"state": "kept", "reason": problem, "unrequested": [], "at_ns": now_ns()}
    # Budget the whole scan inside what is left of the custodian's own window: the
    # declared backings SHARE it, so no backing restarts a fresh allowance and the
    # last read cannot outlive the monitor.
    start_ns, limit_s = ctx.get("start_ns"), ctx.get("limit_s")
    deadline_ns = None
    if typed(start_ns, int) and typed(limit_s, (int, float)):
        deadline_ns = (start_ns + int(limit_s * NS) - EXIT_MARGIN_S * NS
                       - DETACH_GATE_MARGIN_NS)
        if (deadline_ns - now_ns()) <= 0:
            return {"state": "kept", "at_ns": now_ns(), "unrequested": list(backings),
                    "reason": ("no custodian wait window is left to read the custody indication;"
                               " the root, its backings and the evidence are kept")}
    try:
        problems = custody_problems(backings, probe, facts, DETACH_INFO_TIMEOUT_S, deadline_ns)
    except BaseException as error:
        problems = [f"attachment facts failed: {type_name(error)}"]
    if not problems:
        return {"state": "ok", "reason": None, "unrequested": list(backings), "at_ns": now_ns()}
    return {"state": "kept", "at_ns": now_ns(),
            "reason": ("this run's owned root still holds an image it declared in its custody"
                       " indication and never positively detached: " + "; ".join(problems[:4])),
            "unrequested": backings[:CUSTODY_MAX_BACKINGS]}


def close_own_child(handle, window_end_ns):
    """Stop and reap a direct child this process still holds unreaped, bounded by
    the custodian's window. While the parent holds the child unreaped, its PID
    cannot be recycled, so signalling it needs no identity lookup; SIGKILL cannot
    be ignored. Returns (reaped, status) and never raises."""
    try:
        pid = int(getattr(handle, "pid", 0) or 0)
    except BaseException:
        return False, None
    if pid > 0:
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except BaseException:
            pass
    try:
        while True:
            remaining = max(0.0, (window_end_ns - now_ns()) / NS)
            try:
                status = handle.wait(timeout=min(0.05, remaining))
            except BaseException:
                return False, None
            if status is not None:
                return True, status
            if remaining <= 0.0:
                return False, None
    except BaseException:
        return False, None


def reap_own_child(handle, window_end_ns):
    """Reap a direct child this process may already have lost, WITHOUT signalling:
    a positively absent child's PID can be reused, so nothing is ever killed here.
    Bounded by the custodian's window; never raises. Returns (reaped, status)."""
    try:
        while True:
            remaining = max(0.0, (window_end_ns - now_ns()) / NS)
            try:
                status = handle.wait(timeout=min(0.05, remaining))
            except ChildProcessError:  # already reaped or otherwise not ours now
                return True, None
            except BaseException:
                return False, None
            if status is not None:
                return True, status
            if remaining <= 0.0:
                return False, None
    except BaseException:
        return False, None


def _unreaped_record(child, error, deadline):
    """A failure record that never loses a live child: one last bounded wait, and
    the handle is handed back whenever the child is not positively reaped."""
    reaped = False
    try:
        reaped = child.wait(timeout=max(0.0, deadline - time.monotonic())) is not None
    except BaseException:
        reaped = False
    try:
        if child.stdout is not None:  # still owning its descriptor: close it once
            child.stdout.close()
    except BaseException:
        pass
    try:
        child.stdout = None
    except BaseException:
        pass
    return {"exit": child.returncode if reaped else None, "timed_out": False,
            "error": type_name(error), "output": "", "truncated": False, "withdrawn": None,
            "reap_timed_out": not reaped, "reaped": reaped, "handle": None if reaped else child}


def _stuck_child_attempt():
    """A REAL unruly detach child for the injected stuck-child case: started,
    left running and left unreaped on purpose, so the custodian must identify,
    signal and observe it after the monitor's own timeout."""
    try:
        child = subprocess.Popen(["/bin/sh", "-c", "sleep 30"], stdin=subprocess.DEVNULL,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, close_fds=True)
    except OSError as error:
        return {"exit": None, "timed_out": False, "error": errno_name(error), "output": "",
                "truncated": False, "withdrawn": None, "reaped": False, "handle": None}
    return {"exit": None, "timed_out": True, "error": None, "output": "", "truncated": False,
            "withdrawn": None, "reaped": False, "handle": child}


def detach_image(mount, timeout, still_allowed=None):
    """One bounded ORDINARY detach of `mount`. Returns the attempt's own facts;
    the caller judges the post-state separately and never trusts a zero exit."""
    attempt = bounded_run([HDIUTIL, "detach", mount], timeout, DETACH_OUTPUT_MAX_BYTES,
                          still_allowed=still_allowed)
    return {"exit": attempt["exit"], "timed_out": attempt["timed_out"], "error": attempt["error"],
            "output": attempt["output"], "truncated": attempt["truncated"],
            "withdrawn": attempt["withdrawn"], "reap_timed_out": attempt["reap_timed_out"],
            "reaped": attempt["reaped"], "handle": attempt.get("handle")}


def _free_bytes(path):
    try:
        reading = os.statvfs(path)
        return reading.f_bavail * reading.f_frsize
    except OSError:
        return None


def detach_budget(ctx, now=None):
    """Nanoseconds left for one detach attempt: the custodian's own wait window
    (EXIT_MARGIN_S before the absolute deadline) less one gate margin, so a
    bounded attempt always finishes and reports before the custodian would
    declare the monitor incomplete."""
    now = now_ns() if now is None else now
    exit_at = ctx["start_ns"] + int(ctx["limit_s"] * NS) - EXIT_MARGIN_S * NS
    return max(0, exit_at - now - DETACH_GATE_MARGIN_NS)


_ROLE = [None]  # "monitor" or "custodian", set by each actor's constructor


def injections():
    return set(filter(None, os.environ.get("OXIDE_SUPERVISOR_INJECT", "").split(",")))


def injected_unreadable():
    """Test-only: which actor's process lookups are forced unreadable. Scoped to
    ONE actor, so the other actor's identity facts stay readable and each
    handoff outcome can be exercised on its own."""
    active = injections()
    if _ROLE[0] == "monitor" and "detach-identity-unreadable" in active:
        return True
    return _ROLE[0] == "custodian" and "detach-custodian-identity-unreadable" in active


def process_row(pid):
    """(row, problem) from `Kinfo.one(pid)`, never raising. `problem is None`
    means the facts were readable (and `row is None` that the process is
    positively absent); a failed size/read or a malformed row is an unreadable
    observation and is never silently turned into "gone" or into an identity."""
    if not typed(pid, int) or pid <= 0:
        return None, "no usable pid"
    if injected_unreadable():
        return None, "injected unreadable identity"
    try:
        row = Kinfo.one(pid)
    except BaseException as error:
        return None, f"{type_name(error)}"
    if row is None:
        return None, None
    if not (typed(row.get("pid"), int) and typed(row.get("start"), int)
            and isinstance(row.get("zombie"), bool) and typed(row.get("ppid"), int)):
        return None, "malformed process row"
    return row, None


def detach_child_identity(row, pid, start, monitor_pid):
    """Whether an observed kinfo_proc row is still the exact child a monitor handed
    over: same PID, same start time, still that monitor's child and not a zombie.
    A recycled bare PID fails at least one of those, so it is never signalled.
    The row is the real `Kinfo.parse` shape, whose liveness field is the boolean
    `zombie` (there is no `stat` key)."""
    return bool(row is not None and row.get("pid") == pid and row.get("start") == start
                and row.get("ppid") == monitor_pid and row.get("zombie") is False)


def detach_authorization(fault, deadline_hit, now, exit_at):
    """The custodian's verdict for one post-census detach attempt, and the wait
    budget it may spend. Pure: the custodian holds the only authority over
    latched faults and the deadline, and never touches the filesystem here."""
    budget = max(0, exit_at - now - DETACH_GATE_MARGIN_NS)
    if fault is not None:
        return False, f"a fault is latched ({fault})", 0
    if deadline_hit:
        return False, "the deadline was reached before the detach", 0
    if budget <= 0:
        return False, "no custodian wait budget is left for one bounded detach", 0
    return True, None, budget


def detach_request_problems(ctx, document, probe, info_timeout=None):
    """Why this request must not be honoured; empty only when every binding is
    positively established from the owned layout, the kernel and `hdiutil`."""
    problems = []
    if not isinstance(document, dict):
        return ["request is not an object"]
    if set(document) - DETACH_REQUEST_KEYS:
        problems.append("request names unknown fields")
    if not typed(document.get("version"), int) or document.get("version") != DETACH_REQUEST_VERSION:
        problems.append(f"request version is not {DETACH_REQUEST_VERSION}")
    text = {}
    for name in ("backing", "mount", "device", "volume_uuid"):
        value = detach_text(document, name)
        if value is None:
            problems.append(f"request {name} is not a bounded single-line value")
        else:
            text[name] = value
    if "note" in document and detach_text(document, "note") is None:
        problems.append("request note is not a bounded single-line value")
    if not ctx.get("owned"):
        problems.append("this run's scratch root belongs to the caller, so it owns no image to detach")
    if len(text) != 4:
        return problems
    backing, mount, device = text["backing"], text["mount"], text["device"]
    identifier = text["volume_uuid"].lower()
    root, evidence = probe.realpath(ctx["root"]), probe.realpath(ctx["evidence"])
    if not os.path.isabs(backing) or os.path.normpath(backing) != backing:
        problems.append("backing is not an absolute canonical path")
    elif probe.realpath(backing) != backing:
        problems.append("backing resolves through a symlink")
    else:
        try:
            info = probe.lstat(backing)
        except OSError as error:
            problems.append(f"backing is unreadable: {errno_name(error)}")
        else:
            if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
                problems.append("backing is not a regular file")
            elif not (backing.startswith(root + os.sep) or backing.startswith(evidence + os.sep)):
                problems.append("backing is outside this run's owned root and evidence directory")
    if not os.path.isabs(mount) or os.path.normpath(mount) != mount:
        problems.append("mount is not an absolute canonical path")
    elif os.path.dirname(mount) != VOLUMES or not os.path.basename(mount):
        problems.append(f"mount is not one volume directly under {VOLUMES}")
    elif probe.realpath(mount) != mount:
        problems.append("mount resolves through a symlink")
    elif not probe.mount_point(mount):
        problems.append("mount is not a mount point")
    if not DETACH_DEVICE_RE.match(device):
        problems.append("device is not one /dev/disk node")
    if not DETACH_UUID_RE.match(identifier):
        problems.append("volume_uuid is not a UUID")
    if problems:
        return problems
    rows = probe.attachments(backing, info_timeout)
    if rows is None:
        return ["the attachment facts are unreadable"]
    if not [row for row in rows if row[0] == device]:
        problems.append("the requested device is not attached from the requested backing")
    mounted = [row for row in rows if row[1] == mount]
    if len(mounted) != 1:
        problems.append(f"{mount} is mounted {len(mounted)} times from the requested backing")
    else:
        mount_device_node, _attached_mount, attached_uuid = mounted[0]
        if mount_device_node != device and not mount_device_node.startswith(device + "s"):
            problems.append("the mount point is not this image's own device")
        if attached_uuid is not None and attached_uuid != identifier:
            problems.append("the attachment's volume UUID is not the requested UUID")
        try:
            node, mounted_device = probe.stat(mount_device_node).st_rdev, probe.stat(mount).st_dev
        except OSError as error:
            problems.append(f"the mount or device identity is unreadable: {errno_name(error)}")
        else:
            if node != mounted_device:
                problems.append("the mount's device is not the attachment's own /dev node")
    if probe.volume_uuid(mount) != identifier:
        problems.append("the mount's volume UUID is not the requested UUID")
    return problems


def detach_record(ctx, document):
    record = {"request": os.path.join(os.path.realpath(ctx["evidence"]), DETACH_REQUEST_NAME),
              "state": "refused", "qualified": False, "attempted": False, "exit": None,
              "timed_out": False, "withdrawn": None, "error": None, "post_state": "unknown",
              "reason": None, "backing": None, "mount": None, "device": None, "volume_uuid": None,
              "timeout_s": None, "budget_ns": None, "output": "", "output_truncated": False,
              "reap_timed_out": False, "reaped": False, "late_gate": None,
              # Only the cheap reserve read is sampled inside the custodial window;
              # no owned-byte walk runs here (the samplers already record the peak).
              "reserve_before": None, "reserve_after": None,
              "deadline_ns": ctx["start_ns"] + int(ctx["limit_s"] * NS), "at_ns": now_ns()}
    if isinstance(document, dict):
        for name in ("backing", "mount", "device", "volume_uuid"):
            record[name] = detach_text(document, name)
    return record


def post_census_detach(ctx, probe=None, authorize=None, still_allowed=None):
    """This run's one post-census detach, or None when its proof asked for none.
    Only the `cleanup` branch calls it, so it can only run after a positive
    descendant census. The WHOLE sequence -- request read, identity re-derivation,
    the custodian's authorization, the single ordinary detach and the post-state
    read -- is bounded by one absolute deadline taken from the custodian's own
    wait window, and each phase is clamped to what is left of it. `authorize`
    is the one-time gate; `still_allowed` is asked again while the child runs, so
    authority withdrawn mid-attempt kills it. Never raises: an unknown outcome
    stays unknown, is kept and is unqualified."""
    probe = probe or DefaultProbe()
    record = None
    try:
        document, problem = read_detach_request(ctx["evidence"], probe)
    except BaseException as error:
        document, problem = None, f"request read failed: {type_name(error)}"
    if document is _ABSENT and problem is None:
        return None
    try:
        record = detach_record(ctx, document)
        record["reason"] = problem
        budget_ns = detach_budget(ctx)
        record["budget_ns"] = budget_ns
        if problem is not None:
            return record
        if budget_ns < DETACH_MIN_BUDGET_NS:
            record["reason"] = "not enough of the custodian's wait window left for one bounded detach"
            return record
        deadline = time.monotonic() + budget_ns / NS

        def left():
            return max(0.0, deadline - time.monotonic())

        problems = detach_request_problems(ctx, document, probe,
                                           max(0.0, min(DETACH_INFO_TIMEOUT_S, left() * 0.25)))
        if problems:
            record["reason"] = "; ".join(problems)
            return record
        if authorize is not None:
            wait = max(0.0, min(DETACH_GATE_WAIT_S, left() - DETACH_POST_RESERVE_S - 0.5))
            if wait <= 0.0:
                record["reason"] = "not enough of the custodian's wait window left to authorize a detach"
                return record
            allowed, reason, granted = authorize(max(0, int((left() - DETACH_POST_RESERVE_S) * NS)), wait)
            if not allowed:
                record["reason"] = f"the guard withdrew the authorization: {reason}"
                return record
            if granted:
                deadline = min(deadline, time.monotonic() + granted / NS)
        post_reserve = max(0.0, min(DETACH_POST_RESERVE_S, 0.25 * left()))
        timeout = min(DETACH_TIMEOUT_S, left() - post_reserve)
        if timeout <= 0.0:
            record["reason"] = "not enough of the custodian's wait window left for the attempt and its post-state"
            return record
        record["timeout_s"] = round(timeout, 3)
        record["reserve_before"] = _free_bytes(ctx["reserve_path"])
        record["at_ns"] = now_ns()
        inject = set(ctx.get("inject") or ())
        if "detach-fail" in inject:
            attempt = {"exit": 1, "timed_out": False, "error": None, "output": "injected failing detach",
                       "truncated": False, "withdrawn": None, "reaped": True}
        elif "detach-timeout" in inject:
            attempt = {"exit": None, "timed_out": True, "error": None, "output": "injected timeout",
                       "truncated": False, "withdrawn": None}
        elif "detach-ambiguous" in inject:
            attempt = {"exit": 0, "timed_out": False, "error": None, "output": "injected no-op",
                       "truncated": False, "withdrawn": None, "reaped": True}
        elif "detach-withdraw" in inject:
            attempt = {"exit": None, "timed_out": False, "error": None, "output": "",
                       "truncated": False, "withdrawn": "injected withdrawal", "reaped": True}
        elif "detach-stuck-child" in inject:
            attempt = _stuck_child_attempt()
        elif "detach-reap-timeout" in inject:
            attempt = {"exit": None, "timed_out": False, "error": None, "output": "",
                       "truncated": False, "withdrawn": None, "reap_timed_out": True}
        else:
            attempt = probe.detach(record["mount"], timeout, still_allowed)
        record.update(attempted=True, exit=attempt.get("exit"), timed_out=bool(attempt.get("timed_out")),
                      withdrawn=attempt.get("withdrawn"), reap_timed_out=bool(attempt.get("reap_timed_out")),
                      reaped=bool(attempt.get("reaped")), error=attempt.get("error"),
                      _handle=attempt.get("handle"),
                      output=_bounded_tail(b"", (attempt.get("output") or "").encode(
                          "utf-8", "replace")).decode("utf-8", "replace"),
                      output_truncated=bool(attempt.get("truncated")))
        record["at_ns"] = now_ns()
        if still_allowed is not None:
            allowed, reason = still_allowed()
            if not allowed:
                record["late_gate"] = reason or "authority withdrawn"
        try:
            mounted = probe.mount_point(record["mount"])
        except OSError:
            mounted = None
        attached = None
        if mounted is False:
            attached = probe.attachments(record["backing"],
                                         max(0.0, min(DETACH_INFO_TIMEOUT_S, left())))
        if mounted is None:
            post_state = "unknown"
        elif mounted:
            post_state = "still_mounted"
        elif attached is None:
            post_state = "unknown"
        elif attached:
            # ANY entity of that backing still attached -- an unmounted volume, a
            # slice of the requested whole disk or a second attachment -- is not a
            # detach, whatever the requested device's own row says.
            post_state = "still_attached"
        else:
            post_state = "detached"
        record["post_state"] = post_state
        record["reserve_after"] = _free_bytes(ctx["reserve_path"])
        if record["withdrawn"] is not None or record["late_gate"] is not None:
            record["state"] = "withdrawn"
        elif record["timed_out"]:
            record["state"] = "timed_out"
        elif (record["reap_timed_out"] or not record["reaped"] or record["exit"] != 0
              or record["error"] is not None or post_state != "detached"):
            record["state"] = "failed"
        else:
            record["state"], record["qualified"], record["reason"] = "detached", True, None
            return record
        if record["withdrawn"] is not None:
            record["reason"] = f"authority withdrawn mid-attempt ({record['withdrawn']}); post-state {post_state}"
        elif record["late_gate"] is not None:
            record["reason"] = f"authority withdrawn after the attempt ({record['late_gate']}); post-state {post_state}"
        elif record["reap_timed_out"] or not record["reaped"]:
            record["reason"] = (f"the detach child was not positively reaped inside the window"
                                f" (reaped {record['reaped']}); post-state {post_state}")
        else:
            record["reason"] = (f"detach exit {record['exit']}" if not record["timed_out"]
                                else "detach timed out") + f"; post-state {post_state}"
            if record["error"] is not None:
                record["reason"] += f"; spawn error {record['error']}"
        return record
    except BaseException as error:
        if record is None:
            record = detach_record(ctx, document if isinstance(document, dict) else {})
        # Keep `_handle`: a live child is never discarded on an error path. The
        # monitor owns closing it (or handing it to the custodian) and pops it
        # before the record is stored.
        record["state"], record["qualified"] = "unknown", False
        record["reason"] = record["reason"] or f"detach attempt failed: {type_name(error)}"
        return record

def config_pool_path(text):
    """`[pool] path` from a user config, or None when it sets none.

    Only that one entry is read: other lines and tables are ignored. A pool
    table or pool path this narrow reader does not understand refuses, so an
    unknown selection is never guessed from a partly understood file."""
    table, value = None, None
    for number, line in enumerate(text.splitlines(), 1):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if stripped.startswith("["):
            if not stripped.endswith("]"):
                raise SelectionRefused(f"config line {number}: unsupported table header")
            table = stripped[1:-1].strip()
            continue
        if table != "pool":
            continue
        key, separator, rest = stripped.partition("=")
        if not separator:
            raise SelectionRefused(f"config line {number}: unsupported entry in the pool table")
        if key.strip() != "path":
            continue
        rest = rest.strip()
        if value is not None:
            raise SelectionRefused(f"config line {number}: repeated pool path")
        if len(rest) < 2 or rest[0] != rest[-1] or rest[0] not in "'\"":
            raise SelectionRefused(f"config line {number}: unsupported pool path value")
        body = rest[1:-1]
        if (rest[0] == "\"" and "\\" in body) or (rest[0] == "'" and "'" in body):
            raise SelectionRefused(f"config line {number}: unsupported escape in the pool path")
        value = body
    return value


def configured_pool(env, probe):
    """The configured pool path and the fact that named it, effect-free and
    in the product's precedence: `OXIDE_POOL`, then the user config file's
    `[pool] path`, then the built-in default pool. `~` is the account home."""
    explicit = env.get("OXIDE_POOL")
    if explicit:
        return explicit, "OXIDE_POOL"
    home = probe.home()
    config = env.get("OXIDE_CONFIG") or os.path.join(
        home, "Library", "Application Support", "oxide", "config", "config.toml")
    source = "OXIDE_CONFIG" if env.get("OXIDE_CONFIG") else "account config"
    default = os.path.join(home, "Library", "Caches", "oxide", DEFAULT_POOL_LEAF)
    try:
        text = probe.read(config, CONFIG_MAX_BYTES)
    except FileNotFoundError:
        return default, "built-in default pool"
    except SelectionRefused as refusal:
        raise SelectionRefused(f"config {config}: {refusal}")
    except OSError as error:
        raise SelectionRefused(f"config {config}: {type_name(error)}")
    try:
        text = text.decode("utf-8")
    except UnicodeDecodeError:
        raise SelectionRefused(f"config {config}: not UTF-8")
    value = config_pool_path(text)
    if value is None:
        return default, "built-in default pool"
    if not value.startswith("~"):
        return value, source
    if value == "~" or value.startswith("~/"):
        return home + value[1:], source
    raise SelectionRefused(f"config {config}: unsupported home form {value!r}")


def registered_mount(path, probe):
    """The recorded passive selection of the pool's removable registration as
    (mount point, volume UUID), or None when nothing is registered. A present
    but unreadable, oversized or malformed document refuses: an unknown
    selection is never guessed."""
    try:
        raw = probe.read(path, REGISTRATION_MAX_BYTES)
    except FileNotFoundError:
        return None
    except SelectionRefused as refusal:
        raise SelectionRefused(f"registration {path}: {refusal}")
    except OSError as error:
        raise SelectionRefused(f"registration {path}: {type_name(error)}")
    try:
        document = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, ValueError):
        raise SelectionRefused(f"registration {path}: not JSON")
    if not isinstance(document, dict):
        raise SelectionRefused(f"registration {path}: not an object")
    mount, identifier = document.get("mount_point"), document.get("volume_uuid")
    if not isinstance(mount, str) or not isinstance(identifier, str):
        raise SelectionRefused(f"registration {path}: no recorded mount point and volume UUID")
    return mount, identifier.lower()


def checked_volume(path, probe, boot, parent=None, expected_uuid=None):
    """The mount root and volume UUID of `path` when it is a positively
    existing, eligible scratch volume: an absolute canonical real directory,
    directly under `parent` when one is required, on a device other than the
    boot volume's, with a readable volume UUID that is the recorded one when
    `expected_uuid` is given."""
    if not os.path.isabs(path) or os.path.normpath(path) != path:
        raise SelectionRefused(f"selected path {path!r} is not an absolute canonical path")
    if parent is not None:
        if os.path.dirname(path) != parent:
            raise SelectionRefused(f"selected mount {path} is not directly under {parent}")
        guard = probe.lstat(parent)
        if (stat.S_ISLNK(guard.st_mode) or not stat.S_ISDIR(guard.st_mode)
                or guard.st_uid != 0 or guard.st_mode & 0o022):
            raise SelectionRefused(f"{parent} is not a root-owned, non-writable mount parent")
    info = probe.lstat(path)
    if stat.S_ISLNK(info.st_mode):
        raise SelectionRefused(f"selected mount {path} is a symlink")
    if not stat.S_ISDIR(info.st_mode):
        raise SelectionRefused(f"selected mount {path} is not a directory")
    if probe.realpath(path) != path:
        raise SelectionRefused(f"selected mount {path} is not canonical")
    if info.st_dev == boot:
        raise SelectionRefused(f"the volume holding {path} is the boot volume")
    identifier = probe.volume_uuid(path)
    if identifier is None:
        raise SelectionRefused(f"the volume holding {path} has no readable UUID")
    if expected_uuid is not None and identifier != expected_uuid:
        raise SelectionRefused(f"the volume holding {path} is {identifier}, not the registered {expected_uuid}")
    return path, identifier


def mount_root(path, probe):
    """The highest ancestor of `path` on its own device: its mount root."""
    current = probe.realpath(path)
    while True:
        parent = os.path.dirname(current)
        if parent == current or probe.stat(parent).st_dev != probe.stat(current).st_dev:
            return current
        current = parent


def scratch_parent(mount):
    """The owned scratch parent of a selected volume: one private subtree, never
    the drive root. Naming it creates nothing."""
    return os.path.join(mount, SCRATCH_SUBDIR, SCRATCH_OWNER)


def validation_parent(selection):
    """The parent of a private unit-validation temp root (oxide#151, after the
    operator recipe on main `debffbb`): `<mount>/scratch/oxide-validation` on a
    selected volume, a sibling of the supervisor's marked scratch subtree and
    therefore never inside its expiry area; on the fallback it is the fallback
    parent itself. It derives from the mount, never from the already-nested
    `root_parent`, so no second `scratch` component is ever appended. Naming it
    creates nothing."""
    mount = selection.get("mount")
    if isinstance(mount, str):
        return os.path.join(mount, SCRATCH_SUBDIR, VALIDATION_OWNER)
    return selection.get("root_parent") or FALLBACK_PARENT


def select_default_parent(env, probe=None):
    """The default root parent for proof scratch (oxide#145, oxide#151).

    When no caller scratch or root parent is supplied, scratch belongs to a
    private area on the volume the effect-free pool selection names: the
    registered removable volume when the registration and a cheap presence
    and identity check pass, otherwise the configured pool's own volume. A
    volume is eligible only when it is not the boot volume.

    `root_parent` is the owned subtree `<mount>/scratch/oxide` of that volume,
    not the mount root, and `mount` keeps the actual mount. Anything absent,
    unreadable, malformed or unsafe falls back to /private/tmp with a bounded
    warning. Nothing is created here: creating and validating the subtree is
    the caller's blocking setup, so plans, probes and refusal paths stay
    effect-free."""
    probe = probe if probe is not None else DefaultProbe()
    facts = {"source": "fallback", "root_parent": FALLBACK_PARENT, "reason": None,
             "pool": None, "pool_source": None, "registration": None, "mount": None,
             "volume_uuid": None, "device": None}
    try:
        boot = probe.stat("/").st_dev
        facts["pool"], facts["pool_source"] = configured_pool(env, probe)
        registration = os.path.join(facts["pool"], *REGISTRATION)
        recorded = registered_mount(registration, probe)
        if recorded is None:
            volume = mount_root(facts["pool"], probe)
            mount, identifier = checked_volume(volume, probe, boot)
        else:
            facts["registration"] = registration
            mount, identifier = checked_volume(recorded[0], probe, boot, VOLUMES, recorded[1])
        facts.update(source="selected-volume", root_parent=scratch_parent(mount), mount=mount,
                     volume_uuid=identifier, device=probe.stat(mount).st_dev)
    except SelectionRefused as refusal:
        facts["reason"] = str(refusal)
    except OSError as error:
        name = getattr(error, "filename", None)
        facts["reason"] = (f"selection facts unavailable: {type_name(error)}"
                           + (f" at {name}" if isinstance(name, str) else ""))
    return facts


def fallback_warning(selection):
    """One bounded line naming where scratch went and why; `report` caps it."""
    reason = selection.get("reason") or "no eligible selected volume"
    return (f"bounded-test.py: warning: proof scratch defaults to {FALLBACK_PARENT}, not the selected volume:"
            f" {reason}")[:400] + "\n"


def create_owned_root(parent, probe, make_dir=None):
    """Create this run's owned scratch root directly under `parent` and verify
    it before use: a real canonical directory, private to this user and on the
    parent's own device. A refusal removes the directory it just created."""
    make_dir = make_dir or tempfile.mkdtemp
    info = probe.lstat(parent)
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise SelectionRefused(f"root parent {parent} is not a real directory")
    root = make_dir(prefix=SCRATCH_LEAF, dir=parent)
    try:
        info = probe.lstat(root)
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            raise SelectionRefused(f"scratch root {root} is not a real directory")
        if info.st_uid != probe.euid() or info.st_mode & 0o077:
            raise SelectionRefused(f"scratch root {root} is not private to this user")
        if probe.stat(root).st_dev != probe.stat(parent).st_dev:
            raise SelectionRefused(f"scratch root {root} is not on the parent's device")
        if probe.realpath(root) != root or os.path.dirname(root) != parent:
            raise SelectionRefused(f"scratch root {root} is not canonical under {parent}")
    except BaseException:
        try:
            shutil.rmtree(root, ignore_errors=True)
        except BaseException:
            pass
        raise
    return root


# Owned selected-volume scratch parent ------------------------------------------
#
# The selected volume's proof scratch sits INSIDE a shared scratch folder that
# other projects also use, so the two components have different contracts
# (Supervisor2160fee3, 2026-10-25):
#   - `<mount>/scratch` is SHARED. It must already exist; Oxide never creates,
#     chmods, adopts, renames or removes it. It must be a real canonical
#     directory of this user on the selected device with no group or other
#     WRITE bit (0755 passes). A sticky bit is not a substitute for that.
#   - `<mount>/scratch/oxide` is Oxide's PRIVATE leaf. It must be exactly 0700
#     and this user's; only this leaf is ever created (0700) when absent.
# On the Builds drive macOS ignores ownership (Owners Disabled), so these
# checks are consistency and tamper guards for a cooperative same-user host,
# not a multi-user privacy guarantee.

def checked_ancestor(path, device, probe):
    """A required, pre-existing shared scratch ancestor. It may be readable by
    group/other (0755 is the normal case for a shared folder) but never
    writable by them, and it is never created or repaired here."""
    info = probe.lstat(path)
    if stat.S_ISLNK(info.st_mode):
        raise SelectionRefused(f"shared scratch ancestor {path} is a symlink")
    if not stat.S_ISDIR(info.st_mode):
        raise SelectionRefused(f"shared scratch ancestor {path} is not a directory")
    if probe.realpath(path) != path:
        raise SelectionRefused(f"shared scratch ancestor {path} is not canonical")
    if probe.stat(path).st_dev != device:
        raise SelectionRefused(f"shared scratch ancestor {path} is not on the selected device")
    if info.st_uid != probe.euid():
        raise SelectionRefused(f"shared scratch ancestor {path} is not owned by this user")
    if info.st_mode & 0o022:
        raise SelectionRefused(f"shared scratch ancestor {path} is group/other writable"
                               " (a sticky bit does not make a writable ancestor safe)")
    return info


def checked_leaf(path, device, probe):
    """Oxide's private scratch leaf: exactly 0700, this user, real, canonical and
    on the selected device. Anything else refuses rather than being repaired."""
    info = probe.lstat(path)
    if stat.S_ISLNK(info.st_mode):
        raise SelectionRefused(f"scratch leaf {path} is a symlink")
    if not stat.S_ISDIR(info.st_mode):
        raise SelectionRefused(f"scratch leaf {path} is not a directory")
    if probe.realpath(path) != path:
        raise SelectionRefused(f"scratch leaf {path} is not canonical")
    if probe.stat(path).st_dev != device:
        raise SelectionRefused(f"scratch leaf {path} is not on the selected device")
    if info.st_uid != probe.euid():
        raise SelectionRefused(f"scratch leaf {path} is not owned by this user")
    if stat.S_IMODE(info.st_mode) != 0o700:
        raise SelectionRefused(f"scratch leaf {path} is not private"
                               f" (mode {oct(stat.S_IMODE(info.st_mode))}, 0o700 required)")
    return info


def prepare_scratch_parent(selection, probe, make_dir=None):
    """Make the selected volume's private scratch leaf usable, or refuse.

    This ONE seam is the authority for both the real setup and the effect-free
    readiness check. Selection stays effect-free; this runs in the supervisor's
    blocking setup, before any mkdtemp, reserve reading or run resource.

    The shared ancestor `<mount>/scratch` must already exist and pass
    `checked_ancestor`; it is never created, chmodded, adopted, renamed or
    removed. Only the private leaf `<mount>/scratch/oxide` is created, with
    0700, when it is absent, and it is re-validated after creation (including
    its device and inode identity and a concurrent creator's result). A refusal
    leaves the caller its warned `/private/tmp` fallback."""
    make_dir = make_dir or os.mkdir
    mount, device, parent = selection.get("mount"), selection.get("device"), selection.get("root_parent")
    if selection.get("source") != "selected-volume" or not isinstance(mount, str):
        raise SelectionRefused("no selected volume to prepare")
    expected = scratch_parent(mount)
    if not isinstance(parent, str) or parent != expected:
        raise SelectionRefused(f"scratch parent {parent!r} is not {expected}")
    try:
        info = probe.lstat(mount)
    except OSError as error:
        raise SelectionRefused(f"selected mount {mount} is unreadable: {type_name(error)}")
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise SelectionRefused(f"selected mount {mount} is not a real directory")
    if probe.realpath(mount) != mount:
        raise SelectionRefused(f"selected mount {mount} is not canonical")
    if probe.stat(mount).st_dev != device:
        raise SelectionRefused(f"selected mount {mount} is not on the selected device")
    ancestor = os.path.join(mount, SCRATCH_SUBDIR)
    try:
        checked_ancestor(ancestor, device, probe)
    except FileNotFoundError:
        raise SelectionRefused(f"shared scratch ancestor {ancestor} is absent; Oxide never creates it")
    except OSError as error:
        raise SelectionRefused(f"shared scratch ancestor {ancestor} is unreadable: {type_name(error)}")
    try:
        checked_leaf(parent, device, probe)
        return parent
    except FileNotFoundError:
        pass
    except OSError as error:
        raise SelectionRefused(f"scratch leaf {parent} is unreadable: {type_name(error)}")
    try:
        make_dir(parent, 0o700)
    except FileExistsError:
        pass  # a concurrent run created it; validate it instead of adopting it
    except OSError as error:
        raise SelectionRefused(f"scratch leaf {parent} cannot be created: {type_name(error)}")
    try:
        created = checked_leaf(parent, device, probe)
        again = probe.lstat(parent)
    except OSError as error:
        raise SelectionRefused(f"scratch leaf {parent} is unreadable: {type_name(error)}")
    if (again.st_dev, again.st_ino) != (created.st_dev, created.st_ino):
        raise SelectionRefused(f"scratch leaf {parent} changed while it was being prepared")
    return parent


def readiness(selection, probe=None):
    """Effect-free, metadata-only readiness for the selected host.

    It answers whether `prepare_scratch_parent` would succeed, describing an
    absent leaf as `absent; would create 0700` instead of claiming it exists,
    and it performs no mkdir, chmod, sweep, marker write, child launch or
    reserve reading. The result is point-in-time: the real setup re-runs the
    same checks, so this is never a reservation."""
    probe = probe if probe is not None else DefaultProbe()
    result = {"source": selection.get("source"), "mount": selection.get("mount"),
              "root_parent": selection.get("root_parent"), "device": selection.get("device"),
              "ready": False, "leaf": None, "reason": None}
    if selection.get("source") != "selected-volume":
        result["reason"] = selection.get("reason") or "no eligible selected volume"
        return result
    mount, device = selection.get("mount"), selection.get("device")
    ancestor = os.path.join(mount, SCRATCH_SUBDIR)
    leaf = scratch_parent(mount)

    def facts(path):
        info = probe.lstat(path)
        return {"path": path, "mode": oct(stat.S_IMODE(info.st_mode)), "uid": info.st_uid,
                "device": info.st_dev, "inode": info.st_ino}

    try:
        result["ancestor"] = facts(ancestor)
        checked_ancestor(ancestor, device, probe)
    except FileNotFoundError:
        result["ancestor"] = {"path": ancestor, "state": "absent"}
        result["reason"] = f"shared scratch ancestor {ancestor} is absent; Oxide never creates it"
        return result
    except SelectionRefused as refusal:
        try:
            result["ancestor"] = facts(ancestor)
        except OSError:
            result["ancestor"] = {"path": ancestor, "state": "unreadable"}
        result["reason"] = str(refusal)
        return result
    except OSError as error:
        result["ancestor"] = {"path": ancestor, "state": "unreadable"}
        result["reason"] = f"shared scratch ancestor {ancestor} is unreadable: {type_name(error)}"
        return result
    try:
        result["leaf"] = dict(facts(leaf), state="exists")
        checked_leaf(leaf, device, probe)
        result["ready"] = True
        return result
    except FileNotFoundError:
        result["leaf"] = {"path": leaf, "state": "absent; would create 0700"}
        result["ready"] = True
        return result
    except SelectionRefused as refusal:
        try:
            result["leaf"] = dict(facts(leaf), state="exists")
        except OSError:
            result["leaf"] = {"path": leaf, "state": "unreadable"}
        result["reason"] = str(refusal)
        return result
    except OSError as error:
        result["leaf"] = {"path": leaf, "state": "unreadable"}
        result["reason"] = f"scratch leaf {leaf} is unreadable: {type_name(error)}"
        return result


# Owner marker and bounded self-expiry ------------------------------------------

class MarkerInvalid(Exception):
    """A marker that cannot be trusted; its root is never eligible."""

    def __init__(self, code):
        self.code = code
        super().__init__(code)


class UnsafeEntry(Exception):
    """An entry inside a root this sweep refuses to descend or delete."""

    def __init__(self, code):
        self.code = code
        super().__init__(code)


class SweepStop(Exception):
    """The sweep's fixed entry or time budget stopped the work."""

    def __init__(self, code):
        self.code = code
        super().__init__(code)


def new_run_id():
    return os.urandom(16).hex()


def marker_number(value):
    return (isinstance(value, (int, float)) and not isinstance(value, bool)
            and math.isfinite(value) and value >= 0)


def marker_int(value):
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def marker_document(run_id, root, probe, created_unix=None):
    """The initial marker of a freshly created owned root: identity facts only,
    no conclusion."""
    info = probe.lstat(root)
    return {"version": MARKER_VERSION, "run_id": run_id, "root": root,
            "owner_uid": probe.euid(), "device": info.st_dev, "inode": info.st_ino,
            "created_unix": time.time() if created_unix is None else created_unix,
            "concluded_unix": None, "outcome": "running", "closure": "none",
            "kept_reason": "none", "nested_scope": "exclusive"}


def read_marker(root, probe):
    """The validated marker of `root`, or MarkerInvalid."""
    document, _identity = read_marker_with_identity(root, probe)
    return document


def read_marker_with_identity(root, probe):
    """The validated marker of `root` and the identity of the file it was read
    from. Strict by design: an absent, symlinked, foreign-owned, non-private,
    oversized or unreadable marker, an unknown version, a field set that is not
    exactly the schema, a wrong type or an unknown state value all fail
    closed."""
    path = os.path.join(root, MARKER_NAME)
    try:
        info = probe.lstat(path)
    except FileNotFoundError:
        raise MarkerInvalid("absent")
    except OSError:
        raise MarkerInvalid("unreadable")
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
        raise MarkerInvalid("not-regular")
    if info.st_uid != probe.euid():
        raise MarkerInvalid("foreign")
    if info.st_mode & 0o077:
        raise MarkerInvalid("not-private")
    if info.st_size > MARKER_MAX_BYTES:
        raise MarkerInvalid("oversized")
    try:
        raw = probe.read(path, MARKER_MAX_BYTES)
    except SelectionRefused:
        raise MarkerInvalid("not-regular")
    except OSError:
        raise MarkerInvalid("unreadable")
    if len(raw) > MARKER_MAX_BYTES:
        raise MarkerInvalid("oversized")
    try:
        document = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, ValueError):
        raise MarkerInvalid("not-json")
    if not isinstance(document, dict) or set(document) != MARKER_KEYS:
        raise MarkerInvalid("schema")
    if not marker_int(document["version"]) or document["version"] != MARKER_VERSION:
        raise MarkerInvalid("version")
    run_id = document["run_id"]
    if (not isinstance(run_id, str) or len(run_id) != 32
            or any(character not in "0123456789abcdef" for character in run_id)):
        raise MarkerInvalid("run-id")
    if document["root"] != root:
        raise MarkerInvalid("identity")
    if not marker_int(document["owner_uid"]) or document["owner_uid"] != probe.euid():
        raise MarkerInvalid("owner")
    if (not marker_int(document["device"]) or not marker_int(document["inode"])
            or document["inode"] == 0):
        raise MarkerInvalid("identity")
    if not marker_number(document["created_unix"]):
        raise MarkerInvalid("created")
    if document["concluded_unix"] is not None and not marker_number(document["concluded_unix"]):
        raise MarkerInvalid("concluded")
    if (document["outcome"] not in MARKER_OUTCOMES or document["closure"] not in MARKER_CLOSURES
            or document["kept_reason"] not in MARKER_REASONS
            or document["nested_scope"] not in MARKER_SCOPES):
        raise MarkerInvalid("state")
    concluded = document["concluded_unix"] is not None
    if concluded and (document["outcome"] != "closed" or document["closure"] != "positive"):
        raise MarkerInvalid("state")
    if not concluded and document["closure"] != "none":
        raise MarkerInvalid("state")
    return document, (info.st_dev, info.st_ino)


def marker_identity(root, probe):
    """The (device, inode) identity of a root's marker file, so a transition can
    prove it is rewriting the marker it read."""
    info = probe.lstat(os.path.join(root, MARKER_NAME))
    return (info.st_dev, info.st_ino)


def write_marker(root, document, probe, expected=None):
    """Write the marker privately and atomically inside `root`: one private
    temporary file in the same directory, then one replace. The root itself is
    rechecked first, so a swapped, symlinked or foreign directory is never
    written. The payload is bounded and the schema is validated before use.

    `expected` makes the transition race-safe: the initial marker requires that
    no marker exists yet (an unmarked directory is never adopted) and a later
    transition requires the marker it was derived from to be the one still in
    place. A marker that changed in between refuses instead of overwriting the
    other authority's state, so protection and conclusion cannot race."""
    marker = os.path.join(root, MARKER_NAME)
    try:
        info = probe.lstat(root)
    except OSError:
        raise MarkerInvalid("root")
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise MarkerInvalid("root")
    if info.st_uid != probe.euid():
        raise MarkerInvalid("root-owner")
    if probe.realpath(root) != root:
        raise MarkerInvalid("root")
    if not isinstance(document, dict) or set(document) != MARKER_KEYS:
        raise MarkerInvalid("schema")
    if document["nested_scope"] not in MARKER_SCOPES:
        raise MarkerInvalid("state")
    payload = json.dumps(document, sort_keys=True).encode()
    if len(payload) > MARKER_MAX_BYTES:
        raise MarkerInvalid("oversized")

    def unchanged():
        try:
            current = probe.lstat(marker)
        except FileNotFoundError:
            return expected is None
        except OSError:
            return False
        return expected is not None and (current.st_dev, current.st_ino) == expected

    if not unchanged():
        raise MarkerInvalid("exists" if expected is None else "changed")
    temporary = os.path.join(root, MARKER_NAME + ".tmp")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
    try:
        os.fchmod(descriptor, 0o600)
        os.write(descriptor, payload)
    finally:
        os.close(descriptor)
    if not unchanged():  # rechecked at the replace instant: no lost update
        try:
            os.unlink(temporary)
        except OSError:
            pass
        raise MarkerInvalid("exists" if expected is None else "changed")
    os.replace(temporary, marker)
    return marker


def record_conclusion(root, run_id, probe, outcome, closure, kept_reason, wall=None,
                      nested_concluded=False):
    """Record a genuine conclusion (or a truthful non-conclusion) on an existing
    marker. Only the authority holding the actual facts calls this: the marker
    is never updated from age, a disappearing PID, a helper's success or a
    finished task. `closure == "positive"` is the only state that can ever make
    a root eligible for expiry, and it requires the matching conclusion.

    A `hosting` root is never concluded from the run's own outer closure: the
    authority that owns the nested window must pass `nested_concluded=True` and
    is responsible for having positively concluded every owned resource. The
    marker it read must still be the marker in place, so a protection write
    cannot be overwritten by a conclusion that started before it."""
    if outcome not in MARKER_OUTCOMES or closure not in MARKER_CLOSURES \
            or kept_reason not in MARKER_REASONS:
        raise MarkerInvalid("state")
    if outcome == "running" or (closure == "positive") != (outcome == "closed"):
        raise MarkerInvalid("state")  # a conclusion is terminal and never back to running
    document, identity = read_marker_with_identity(root, probe)
    if document["run_id"] != run_id:
        raise MarkerInvalid("identity")
    if closure == "positive" and document["nested_scope"] == "hosting" and not nested_concluded:
        raise MarkerInvalid("nested-unconcluded")
    updated = dict(document, concluded_unix=(time.time() if wall is None else wall)
                   if closure == "positive" else None,
                   outcome=outcome, closure=closure, kept_reason=kept_reason)
    write_marker(root, updated, probe, expected=identity)
    return updated


def protect_nested_scope(root, probe):
    """Durably mark an existing run root as hosting resources created by another
    authority, before any of them exists. A hosting root can never be concluded
    from its own outer closure, so an unexpected nested UNCLOSED fixture cannot
    leave an expirable root.

    An unmarked directory is never adopted (`MarkerInvalid("absent")`), an
    already concluded marker is never downgraded (`"concluded"`), and the
    marker read must still be the marker in place (`"changed"`), so this cannot
    race with a conclusion."""
    document, identity = read_marker_with_identity(root, probe)
    if document["concluded_unix"] is not None or document["outcome"] != "running":
        raise MarkerInvalid("concluded")
    if document["nested_scope"] == "hosting":
        return document
    updated = dict(document, nested_scope="hosting")
    write_marker(root, updated, probe, expected=identity)
    return updated


class SweepBudget:
    """The startup sweep's fixed entry and time bounds, checked between
    filesystem steps; exhaustion leaves the unreached work for a later run."""

    def __init__(self, entries=SWEEP_ENTRY_BUDGET, deadline_ns=None, clock=None):
        self.clock = clock or now_ns
        self.entries = entries
        self.started_ns = self.clock()
        self.deadline_ns = (self.started_ns + SWEEP_TIME_BUDGET_NS
                            if deadline_ns is None else deadline_ns)
        self.used = 0
        self.exhausted = None

    def step(self):
        self.used += 1
        if self.used > self.entries:
            self.exhausted = "entries"
            raise SweepStop("entries")
        if self.clock() >= self.deadline_ns:
            self.exhausted = "time"
            raise SweepStop("time")


def entry_stat(name, dir_fd):
    """The facts of one directory entry by descriptor, never following a link.
    A separate seam so a synthetic probe can stand in for an entry it cannot
    create (such as a foreign-owned file) in the pure cases."""
    return os.stat(name, dir_fd=dir_fd, follow_symlinks=False)


def entry_facts(path, device, probe, budget, dir_fd=None):
    """Why an entry is not a run root this sweep may touch, or None. Never
    follows a link: `lstat` sees the entry itself, and the device and ownership
    checks are the same ones the owned root already passed."""
    budget.step()
    if dir_fd is None:
        info = probe.lstat(path)
    else:
        info = entry_stat(path, dir_fd)
    if stat.S_ISLNK(info.st_mode):
        return "symlink"
    if not stat.S_ISDIR(info.st_mode):
        return "not-a-directory"
    if info.st_uid != probe.euid():
        return "foreign-owner"
    if info.st_dev != device:
        return "other-device"
    if dir_fd is None and probe.realpath(path) != path:
        return "not-canonical"
    return None


def empty_directory(fd, probe, budget, device):
    """Remove every entry of an already-open directory except the marker, which
    the eligible root's own deletion removes last. Every entry is lstat-checked;
    a symlink, a foreign owner, another device or a link that is not a real
    directory stops the deletion instead of following anything. A filesystem
    error on a peer's own entries stops that peer as a bounded skip; it never
    becomes this run's selection refusal."""
    try:
        empty_directory_entries(fd, probe, budget, device)
    except OSError as error:
        raise UnsafeEntry("io-" + errno_name(error))


def empty_directory_entries(fd, probe, budget, device):
    for name in sorted(os.listdir(fd)):
        if name == MARKER_NAME:
            continue
        budget.step()
        info = entry_stat(name, fd)
        if stat.S_ISLNK(info.st_mode):
            raise UnsafeEntry("symlink")
        if info.st_uid != probe.euid():
            raise UnsafeEntry("foreign-owner")
        if not (stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode)):
            raise UnsafeEntry("not-a-regular-entry")
        if info.st_dev != device:
            raise UnsafeEntry("other-device")
        if not stat.S_ISDIR(info.st_mode):
            os.unlink(name, dir_fd=fd)
            continue
        inner = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
        try:
            empty_directory_entries(inner, probe, budget, device)
        finally:
            os.close(inner)
        os.rmdir(name, dir_fd=fd)


def remove_marked_root(root, marker, marker_identity, probe, budget):
    """Delete one positively concluded root: its own entries first, then the
    marker, then the root itself. The marker stays until last, so a partial
    bounded deletion never turns a damaged root into an unmarked one and never
    makes an unknown root eligible. Every destructive step is preceded by an
    identity recheck of the root and its marker."""
    fd = None
    try:
        try:
            fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            opened = os.fstat(fd)
        except FileNotFoundError:
            return False, "vanished"
        except OSError as error:
            return False, "io-" + errno_name(error)
        if opened.st_dev != marker["device"] or opened.st_ino != marker["inode"]:
            return False, "changed-during-sweep"
        try:
            empty_directory(fd, probe, budget, marker["device"])
        except SweepStop as stop:
            return False, "partial-" + stop.code
        except UnsafeEntry as unsafe:
            return False, "unsafe-" + unsafe.code
        try:
            current = entry_stat(MARKER_NAME, fd)
        except FileNotFoundError:
            return False, "marker-vanished"
        except OSError as error:
            return False, "io-" + errno_name(error)
        if (not stat.S_ISREG(current.st_mode) or current.st_uid != probe.euid()
                or (current.st_dev, current.st_ino) != marker_identity):
            return False, "marker-changed"
        try:
            os.unlink(MARKER_NAME, dir_fd=fd)
        except OSError as error:
            return False, "io-" + errno_name(error)
        os.close(fd)
        fd = None
        try:
            after = os.lstat(root)
        except FileNotFoundError:
            return True, None
        if after.st_dev != marker["device"] or after.st_ino != marker["inode"]:
            return False, "changed-during-sweep"
        try:
            os.rmdir(root)
        except FileNotFoundError:
            return True, None
        except OSError:
            return False, "rmdir"
        return True, None
    finally:
        if fd is not None:
            os.close(fd)


def sweep_concluded_roots(parent, device, probe, now=None, entries=SWEEP_ENTRY_BUDGET,
                          deadline_ns=None, clock=None):
    """One bounded startup sweep of the owned selected-volume scratch parent.

    It removes only a real owner-marked run root whose marker records a positive
    conclusion of all its owned resources strictly older than seven days by the
    wall clock. Everything else is skipped and reported within fixed bounds: an
    unmarked, foreign, symlinked, non-directory, other-device, non-canonical,
    corrupt, future-dated, unconcluded, non-positive or non-expirable root is
    never touched, and neither is anything outside this one subtree. The entry
    and time budgets are checked between filesystem steps; exhaustion leaves the
    unreached work for a later run, and there is no background reaper."""
    clock = clock or now_ns
    wall = time.time() if now is None else now
    budget = SweepBudget(entries, deadline_ns, clock)
    summary = {"parent": parent, "examined": 0, "steps": 0, "removed_total": 0,
               "skipped_total": 0, "rows": [], "omitted": 0, "stop": None,
               "entry_budget": entries, "time_budget_ns": SWEEP_TIME_BUDGET_NS,
               "expiry_s": EXPIRY_S, "elapsed_ns": 0}

    def row(name, result):
        if len(summary["rows"]) < SWEEP_ROWS:
            summary["rows"].append({"name": name, "result": result})
        else:
            summary["omitted"] += 1

    try:
        problem = entry_facts(parent, device, probe, budget)
        if problem is not None:
            summary["stop"] = "parent-" + problem
            summary["elapsed_ns"] = clock() - budget.started_ns
            return summary
        names = sorted(os.listdir(parent))
    except FileNotFoundError:
        summary["stop"] = "parent-absent"
        summary["elapsed_ns"] = clock() - budget.started_ns
        return summary
    except OSError as error:
        summary["stop"] = "parent-io-" + errno_name(error)
        summary["elapsed_ns"] = clock() - budget.started_ns
        return summary
    except SweepStop as stop:
        summary["stop"] = stop.code
        summary["elapsed_ns"] = clock() - budget.started_ns
        return summary
    try:
        for index, name in enumerate(names):
            if not name.startswith(RUN_ROOT_LEAVES):
                summary["skipped_total"] += 1
                row(name, "not-a-run-root")
                continue
            summary["examined"] += 1
            try:
                result = sweep_one_root(os.path.join(parent, name), device, probe, budget, wall)
            except OSError as error:  # a peer's fault is a bounded skip, never this run's
                result = "io-" + errno_name(error)
            if result == "removed":
                summary["removed_total"] += 1
            else:
                summary["skipped_total"] += 1
            row(name, result)
    except SweepStop as stop:
        summary["stop"] = stop.code
        summary["omitted"] += len(names) - index
    if budget.exhausted and summary["stop"] is None:
        summary["stop"] = budget.exhausted  # a partial deletion used the last step
    summary["steps"] = budget.used
    summary["elapsed_ns"] = clock() - budget.started_ns
    return summary


def sweep_one_root(path, device, probe, budget, now):
    """One entry's verdict: "removed", or a bounded skip code. Reads only."""
    try:
        problem = entry_facts(path, device, probe, budget)
    except FileNotFoundError:
        return "vanished"
    except OSError:
        return "unreadable"
    if problem is not None:
        return problem
    try:
        marker = read_marker(path, probe)
        info = probe.lstat(os.path.join(path, MARKER_NAME))
        root_info = probe.lstat(path)
    except MarkerInvalid as invalid:
        return "marker-" + invalid.code
    except FileNotFoundError:
        return "vanished"
    except OSError:
        return "unreadable"
    marker_identity = (info.st_dev, info.st_ino)
    if root_info.st_dev != marker["device"] or root_info.st_ino != marker["inode"]:
        return "marker-identity"
    if marker["concluded_unix"] is None:
        return "unconcluded"
    if marker["outcome"] != "closed" or marker["closure"] != "positive":
        return "not-positive"
    if marker["kept_reason"] not in MARKER_EXPIRABLE_REASONS:
        return "kept-" + marker["kept_reason"]
    if marker["concluded_unix"] > now or marker["created_unix"] > now:
        return "future-clock"
    if now - marker["concluded_unix"] <= EXPIRY_S:
        return "young"
    try:  # the destructive step's own rechecks, immediately before it
        if entry_facts(path, device, probe, budget) is not None:
            return "changed-during-sweep"
        again = read_marker(path, probe)
        identity = probe.lstat(os.path.join(path, MARKER_NAME))
    except (MarkerInvalid, OSError):
        return "changed-during-sweep"
    if (again["run_id"] != marker["run_id"] or again["concluded_unix"] != marker["concluded_unix"]
            or (identity.st_dev, identity.st_ino) != marker_identity):
        return "changed-during-sweep"
    try:
        removed, reason = remove_marked_root(path, marker, marker_identity, probe, budget)
    except OSError as error:
        return "io-" + errno_name(error)
    return "removed" if removed else reason


class Monitor:
    def __init__(self, ctx, ctl_w, cmd_r, out_r, err_r):
        _ROLE[0] = "monitor"
        self.ctx, self.ctl_w, self.cmd_r = ctx, ctl_w, cmd_r
        self.lock = threading.Lock()
        self.seq = 0
        self.leader = None  # [pid, start] once bound
        self.stop = threading.Event()
        self.census_stop = threading.Event()
        self.inject = set(ctx["inject"])
        self.pipes = [(out_r, "stdout", 1), (err_r, "stderr", 2)]
        self.receipt = None
        # Observation-only settlement diagnostics; the receipt carries this
        # same object, so publishing includes it without extra work.
        self.settlement = settlement_record()

    def note(self, entry=_SETTLE_TOP, **fields):
        """One observation-only stamp, on the settlement object or on one
        round record (a record beyond the cap is None and takes none). Never
        raises, never blocks, takes no lock and is never consulted by a
        decision."""
        try:
            target = self.settlement if entry is _SETTLE_TOP else entry
            if target is None:
                return
            for name, value in fields.items():
                target[name] = value
        except BaseException:
            pass

    def note_stopped(self, command, received):
        """Stamp when the monitor processed `stopped`. `sent` is the
        custodian's own CLOCK_UPTIME_RAW value from the same system-wide
        clock; anything missing, ill-typed, negative or later than the receive
        is unknown (None), never guessed and never a fault."""
        try:
            sent = command.get("sent")
            if type(sent) is not int or sent < 0 or sent > received:
                sent = None
            self.note(stopped_received_ns=received, stopped_seq=command.get("seq"),
                      stopped_sent_ns=sent,
                      receive_latency_ns=None if sent is None else received - sent,
                      leader_bound=self.leader is not None, phase="received")
        except BaseException:
            pass

    def note_round(self, start_ns):
        """Count this round and keep one bounded record. Returns the record,
        or None past the cap where only the counters move. Never raises."""
        try:
            field = self.settlement
            field["rounds_started"] += 1
            if field["first_round_ns"] is None:
                field["first_round_ns"] = start_ns
                if field["stopped_received_ns"] is not None:
                    field["first_round_latency_ns"] = max(0, start_ns - field["stopped_received_ns"])
            if len(field["rounds"]) >= SETTLE_MAX_ROUNDS:
                field["omitted"] += 1
                return None
            entry = {"index": field["rounds_started"] - 1, "phase": "start", "start_ns": start_ns,
                     "scanned_ns": None, "snapshot_ns": None, "scan_ns": None, "rows": None,
                     "esrch_rows": None, "zombie_rows": None, "confirm_reads": None,
                     "pause_calls": None, "lock_wait_ns": None, "lock_acquired_ns": None,
                     "lock_released_ns": None, "sent_ns": None, "end_ns": None,
                     "certain": None, "members": None, "uncertain": None}
            field["rounds"].append(entry)
            return entry
        except BaseException:
            return None

    def owned_root(self):
        """This run's owned root and the selection facts behind it. A caller's
        `--root-parent` keeps its existing semantics; the default is the
        source-owned selected-volume choice, prepared inside `<mount>/scratch/oxide`
        with one bounded self-expiry sweep and an owner marker written before any
        run resource, with a warned fallback."""
        ctx = self.ctx
        if ctx["root_parent"] is not None:
            parent = ctx["root_parent"]
            root = os.path.realpath(tempfile.mkdtemp(prefix=SCRATCH_LEAF, dir=parent))
            return ({"source": "explicit-root-parent", "root_parent": parent, "reason": None,
                     "pool": None, "pool_source": None, "registration": None,
                     "mount": None, "volume_uuid": None, "device": None}, root,
                    {"path": None, "state": "none", "run_id": None, "sweep": None})
        probe = DefaultProbe()
        selection = select_default_parent(os.environ, probe)
        sweep, root, marker, refusal = None, None, None, None
        try:
            parent = prepare_scratch_parent(selection, probe)
            sweep = sweep_concluded_roots(parent, selection["device"], probe)
            root = create_owned_root(parent, probe)
            run_id = new_run_id()
            path = write_marker(root, marker_document(run_id, root, probe), probe)
            marker = {"path": path, "state": "written", "run_id": run_id, "sweep": sweep}
        except (SelectionRefused, MarkerInvalid) as error:
            refusal = str(error)
        except OSError as error:
            refusal = type_name(error)
        if marker is not None:
            selection["prepared_parent"] = parent
            return selection, root, marker
        if root is not None:  # remove exactly the empty root this call created
            try:
                shutil.rmtree(root, ignore_errors=True)
            except BaseException:
                pass
        fallback = {"source": "fallback", "root_parent": FALLBACK_PARENT,
                    "reason": f"{selection['root_parent']}: {refusal or 'the owned scratch parent is unusable'}",
                    "pool": selection.get("pool"), "pool_source": selection.get("pool_source"),
                    "registration": selection.get("registration"), "mount": None,
                    "volume_uuid": None, "device": None}
        return fallback, create_owned_root(FALLBACK_PARENT, probe), {
            "path": None, "state": "none", "run_id": None, "sweep": sweep}

    def setup(self):
        """All blocking setup, inside the custodian's setup deadline: the
        kinfo_proc layout check, path resolution and scratch/evidence
        creation. A stall here leaves `ready` unsent; the custodian KILLs
        this pinned monitor at the setup deadline and launches nothing."""
        ctx = self.ctx
        try:
            Kinfo.check()
        except Exception as error:
            raise SetupRefused(f"missing kinfo_proc ({type_name(error)})")
        if ctx["adopted"]:
            scratch = os.path.realpath(ctx["adopted"])
            if not os.path.isdir(scratch):
                raise SetupRefused(f"OXIDE_TASK_SCRATCH_ROOT {scratch} is not a directory")
            root, owned, reserve_path = scratch, False, scratch
            evidence = ctx["evidence_arg"] or tempfile.mkdtemp(prefix="supervisor-evidence.", dir=scratch)
            selection = {"source": "adopted-scratch-root", "root_parent": None, "reason": None,
                         "pool": None, "pool_source": None, "registration": None,
                         "mount": None, "volume_uuid": None, "device": None}
            marker = {"path": None, "state": "none", "run_id": None, "sweep": None}
        else:
            selection, root, marker = self.owned_root()
            scratch, owned = os.path.join(root, "scratch"), True
            reserve_path = root
            os.mkdir(scratch)
            evidence = ctx["evidence_arg"] or os.path.join(root, "evidence")
        ctx["marker"] = marker
        if selection["source"] == "fallback":
            report(fallback_warning(selection))
        try:
            reserve_device = os.stat(reserve_path).st_dev
        except OSError as error:
            raise SetupRefused(f"reserve path {reserve_path}: {type_name(error)}")
        selection["device"] = selection["device"] if selection["device"] is not None else reserve_device
        ctx.update(root=root, scratch=scratch, evidence=os.path.realpath(evidence), owned=owned,
                   reserve_path=reserve_path, reserve_device=reserve_device, selection=selection,
                   admission_path=os.path.join(os.path.realpath(evidence), ADMISSION_NAME),
                   waiting_path=os.path.join(os.path.realpath(evidence), WAITING_NAME))
        try:
            terminal_mode = _decide_terminal_mode()
        except BaseException:
            terminal_mode = {"mode": "strict", "reason": "identity_read_error",
                             "evidence": ALLOW_EVIDENCE_ID}
        if (not isinstance(terminal_mode, dict) or terminal_mode.get("mode") not in ("allowlisted", "strict")
                or terminal_mode.get("reason") not in TERMINAL_MODE_REASONS
                or terminal_mode.get("evidence") != ALLOW_EVIDENCE_ID):
            terminal_mode = {"mode": "strict", "reason": "identity_read_error",
                             "evidence": ALLOW_EVIDENCE_ID}
        ctx["terminal_mode"] = terminal_mode
        self.receipt = {
            "run_id": ctx["run_id"], "command": ctx["command"], "limit_s": ctx["limit_s"],
            "root": ctx["root"], "scratch": ctx["scratch"], "evidence": ctx["evidence"],
            "scratch_owned": ctx["owned"], "start_ns": ctx["start_ns"], "abort_threshold": THRESHOLD,
            "terminal_mode": dict(terminal_mode),
            "scratch_default": dict(ctx["selection"]),
            "scratch_sweep": marker.get("sweep"),
            "scratch_marker": {key: marker.get(key) for key in ("path", "state", "run_id", "reason")},
            "phase": None, "leader": None, "custodian": [], "outcome": None,
            "reserve": {"samples": 0, "minimum": None, "max_gap_ns": 0, "first_ns": None, "last_ns": None,
                        "path": ctx["reserve_path"], "device": ctx["reserve_device"]},
            "owned_bytes": {"samples": 0, "maximum": None, "budget": ctx["budget"], "last_ns": None},
            "output": {}, "census": None, "census_rounds": 0, "settlement": self.settlement,
            "admission": admission_record(), "admission_waiting": admission_waiting_record(),
        }

    def close_orphan_reads(self, ctx):
        """Attachment reads whose child was never reaped are closed here, bounded
        by what is left of the custodian's wait window. An orphan that cannot be
        closed is unaccounted custody: the monitor holds instead of completing."""
        if not INFO_ORPHANS:
            return True
        window_end = (ctx["start_ns"] + int(ctx["limit_s"] * NS) - EXIT_MARGIN_S * NS
                      - int(0.2 * NS))
        while INFO_ORPHANS:
            handle = INFO_ORPHANS.pop(0)
            reaped, _status = close_own_child(handle, window_end)
            if not reaped:
                INFO_ORPHANS.insert(0, handle)
                return False
        return True

    def finish_monitor(self, ctx, phase, drain=False):
        """Complete this monitor. Only an image-protocol run drains, and only
        inside what is left of the custodian's own wait window; the drain happens
        BEFORE the final publication so the persisted receipt carries the
        post-decision records. A run with neither a request nor a custody
        indication passes drain=False and reads nothing."""
        if not self.close_orphan_reads(ctx):
            # Unaccounted custody: keep everything and let the custodian end the run.
            self.publish("orphan_read_unaccounted: monitor_held")
            self.summary()
            end = ctx["start_ns"] + int(ctx["limit_s"] * NS) - int(0.1 * NS)
            while not self.stop.is_set() and now_ns() < end:
                time.sleep(0.02)
            return
        if drain:
            remaining = (ctx["start_ns"] + int(ctx["limit_s"] * NS) - EXIT_MARGIN_S * NS
                         - now_ns()) / NS
            # One short drain: the custodian's records are already in the pipe by
            # the time its last answer was read, and the wait never exceeds what is
            # left of its own window.
            self.drain_records(max(0.0, min(DRAIN_WAIT_S, remaining)))
        self.publish(phase)
        self.summary()
        self.send("monitor_complete")
        self.stop.set()

    def keep_record(self, record):
        """Append one custodian record to the receipt (anything else is ignored)."""
        if isinstance(record, dict):
            with self.lock:
                self.receipt["custodian"].append(record)

    def drain_records(self, wait_s):
        """Append `record` messages the custodian sent after the decision (detach
        pin/signal/observation, cleanup keeps) to the receipt. Bounded, never
        blocks past `wait_s`, and other command types are ignored."""
        deadline = time.monotonic() + max(0.0, wait_s)
        buffer = b""
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return
            try:
                ready, _, _ = select.select([self.cmd_r], [], [], remaining)
            except OSError:
                return
            if not ready:
                return
            try:
                chunk = os.read(self.cmd_r, 4096)
            except OSError:
                return
            if not chunk:
                return
            buffer += chunk
            while b"\n" in buffer:
                line, buffer = buffer.split(b"\n", 1)
                try:
                    command = json.loads(line)
                except ValueError:
                    return
                if isinstance(command, dict) and command.get("type") == "record":
                    self.keep_record(command.get("record"))

    def send(self, message_type, **fields):
        with self.lock:
            self.seq += 1
            message = {"run": self.ctx["run_id"], "leader": self.leader, "seq": self.seq,
                       "at": now_ns(), "type": message_type, **fields}
            os.write(self.ctl_w, (json.dumps(message) + "\n").encode())

    def publish(self, phase):
        with self.lock:
            self.receipt["phase"] = phase
            self.receipt["published_ns"] = now_ns()
            path = os.path.join(self.ctx["evidence"], "receipt.json")
            with open(path + ".tmp", "w") as handle:
                json.dump(self.receipt, handle, indent=1)
            os.replace(path + ".tmp", path)

    def sample_reserve(self):
        stats = self.receipt["reserve"]
        while not self.stop.is_set():
            try:
                reading = os.statvfs(self.ctx["reserve_path"])
                value = reading.f_bavail * reading.f_frsize
                if "reserve-low-after" in self.inject and stats["samples"] >= 8:
                    value = 1 * GIB  # injected reading; the disk is never filled
                at = now_ns()
                with self.lock:
                    if stats["last_ns"] is not None:
                        stats["max_gap_ns"] = max(stats["max_gap_ns"], at - stats["last_ns"])
                    stats["first_ns"] = stats["first_ns"] or at
                    stats["last_ns"] = at
                    stats["samples"] += 1
                    stats["minimum"] = value if stats["minimum"] is None else min(stats["minimum"], value)
                stall = "reserve-delay-stall" in self.inject and stats["samples"] >= 8
                if stall:
                    with self.lock:  # publication delayed by the shared I/O lock
                        time.sleep(RESERVE_DELAY_S)
                self.send("reading", kind="reserve", value=value, completed=at)
                if stall:
                    self.stop.wait()  # then the sampler stalls: no further reading
                    return
            except OSError as error:
                self.send("reading", kind="reserve", error=str(error))
            self.stop.wait(RESERVE_INTERVAL)

    def sample_owned(self):
        stats = self.receipt["owned_bytes"]
        while not self.stop.is_set():
            try:
                value = tree_bytes(self.ctx["root"])
                at = now_ns()
                with self.lock:
                    stats["samples"] += 1
                    stats["last_ns"] = at
                    stats["maximum"] = value if stats["maximum"] is None else max(stats["maximum"], value)
                stall = "scratch-delay-stall" in self.inject and stats["samples"] >= 2
                if stall:
                    time.sleep(SCRATCH_DELAY_S)  # publication delayed (not under the lock: reserve stays fresh)
                self.send("reading", kind="scratch", value=value, complete=True, completed=at)
                if stall:
                    self.stop.wait()
                    return
            except OSError as error:
                self.send("reading", kind="scratch", complete=False, error=str(error))
            self.stop.wait(SCRATCH_INTERVAL)

    def note_admission(self, error=False, key="admission"):
        """One bounded, never-latched admission-observation failure."""
        try:
            record = self.receipt[key]
            with self.lock:
                if error:
                    record["errors"] += 1
        except BaseException:
            pass

    def reject_admission(self, reason, key="admission", observed=True):
        """Record why one report was ignored, and tell the custodian.

        Cooperative evidence only: a rejected report never faults the run. A
        readable document that failed validation is still an observation of the
        fixed-name completion file, so the custodian must count it: it can have
        replaced a completion before the next poll. `observed=False` covers the
        paths that never read a document (a vanished path, a non-regular file, an
        open error), which are not observations of a write. At most one rejection
        is reported per poll and file, so the control-message rate stays bounded
        by the sampler."""
        try:
            record = self.receipt[key]
            with self.lock:
                record["ignored"] += 1
                if len(record["rejections"]) < ADMISSION_REJECTIONS:
                    record["rejections"].append(reason)
            if observed:
                self.send("admission_rejected", kind=key, completed=now_ns(), reason=reason[:64])
        except BaseException:
            pass

    def read_admission(self, path, seen=None, key="admission"):
        """One bounded read of an admission report (oxide#192).

        Only a regular file opened without following a link, at most
        ADMISSION_MAX_BYTES, validated against its own fixed schema: the
        completed-wait marker or the waiting report. Deduplication is bound to
        the identity of the file actually opened (`fstat` on the open handle),
        never to an identity sampled before the open: a writer publishes by
        atomic rename, so a replacement between the sampler's `lstat` and this
        open would otherwise report the new file once and then report it again on
        the next poll, and a duplicated completion would grant the same wait
        twice. A valid report is sent to the custodian with its own completed
        stamp; the extension decision is the custodian's. Every other outcome is
        recorded here and nothing is sent.

        Returns the identity this call consumed, or None when nothing was read."""
        waiting = key == "admission_waiting"
        try:
            info = os.lstat(path)
            if not stat.S_ISREG(info.st_mode):
                self.reject_admission("not a regular file", key, observed=False)
                return None
            handle = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
            try:
                opened = os.fstat(handle)
                identity = (opened.st_dev, opened.st_ino)
                if not stat.S_ISREG(opened.st_mode):
                    self.reject_admission("not a regular file", key, observed=False)
                    return None
                if seen is not None and identity == seen:
                    return identity  # already reported: one inode is one report
                raw = os.read(handle, ADMISSION_MAX_BYTES + 1)
            finally:
                os.close(handle)
        except OSError as error:
            self.reject_admission("read: " + errno_name(error), key, observed=False)
            return None
        completed = now_ns()
        if len(raw) > ADMISSION_MAX_BYTES:
            self.reject_admission(f"larger than {ADMISSION_MAX_BYTES} bytes", key)
            return None
        try:
            marker = json.loads(raw)
        except ValueError:
            self.reject_admission("not JSON", key)
            return None
        run_id = self.ctx.get("run_id")
        problem = (admission_waiting_malformed(marker, run_id) if waiting
                   else admission_malformed(marker, run_id))
        if problem:
            self.reject_admission(problem, key)
            return None
        try:
            record = self.receipt[key]
            with self.lock:
                record["observed"] += 1
                record["first_ns"] = completed if record["first_ns"] is None else record["first_ns"]
                record["last_ns"] = completed
                record["identity"] = None if identity is None else [int(identity[0]), int(identity[1])]
                if waiting:
                    record["reason"] = marker["reason"] if record["reason"] is None else record["reason"]
                else:
                    record["reported_wait_ns"] = min(U64MAX, record["reported_wait_ns"] + marker["waited_ns"])
                index = record["observed"]
            if waiting:
                self.send("admission_waiting", reason=marker["reason"], command=marker["command"],
                          completed=completed, marker=index)
            else:
                self.send("admission", waited_ns=marker["waited_ns"], command=marker["command"],
                          completed=completed, marker=index)
            return identity
        except BaseException:
            self.note_admission(error=True, key=key)
            return None

    def sample_admission(self):
        """Poll both admission reports for a new identity (oxide#192).

        A writer publishes a fresh file by rename, so one identity is one
        report. Absence is not an event, and no failure here is latched."""
        paths = (("admission", self.ctx.get("admission_path")),
                 ("admission_waiting", self.ctx.get("waiting_path")))
        seen = {"admission": None, "admission_waiting": None}
        while not self.stop.is_set():
            for key, path in paths:
                if not path:
                    continue
                try:
                    info = os.lstat(path)
                    candidate = (info.st_dev, info.st_ino)
                    if candidate != seen[key]:
                        # Mark what the read actually consumed, so a rename that
                        # lands between this stat and the open is reported once.
                        consumed = self.read_admission(path, seen[key], key)
                        seen[key] = candidate if consumed is None else consumed
                except FileNotFoundError:
                    seen[key] = None
                except OSError:
                    self.note_admission(error=True, key=key)
            self.stop.wait(ADMISSION_POLL_S)

    def drain(self, fd, name, sink):
        counts = self.receipt["output"].setdefault(
            name, {"bytes": 0, "persisted": 0, "forwarded": 0, "discarded": 0})
        with open(os.path.join(self.ctx["evidence"], f"{name}.log"), "wb") as log:
            while True:
                chunk = os.read(fd, 65536)
                if not chunk:
                    break
                counts["bytes"] += len(chunk)
                used = 0
                if counts["persisted"] < PERSIST:
                    part = chunk[: PERSIST - counts["persisted"]]
                    log.write(part)
                    counts["persisted"] += len(part)
                    used = len(part)
                if counts["forwarded"] < FORWARD and sink is not None:
                    part = chunk[: FORWARD - counts["forwarded"]]
                    try:
                        os.write(sink, part)
                        counts["forwarded"] += len(part)
                        used = max(used, len(part))
                    except OSError:
                        sink = None
                counts["discarded"] += len(chunk) - used
        os.close(fd)
        self.send("output_eof", stream=name)

    def census_loop(self, after):
        started = now_ns()
        self.note(thread_start_ns=started, phase="thread-start")
        try:  # observation only: how long after the stopped receive the thread ran
            received = self.settlement["stopped_received_ns"]
            if received is not None:
                self.note(thread_start_latency_ns=max(0, started - received))
        except BaseException:
            pass
        census = Census(self.leader[0], terminal_allowed=(
            self.ctx.get("terminal_mode", {}).get("mode") == "allowlisted"))
        self.note(census_built_ns=now_ns())
        while not self.census_stop.is_set():
            begin = now_ns()
            entry = self.note_round(begin)
            self.note(phase="round-start")
            result = census.round()
            scanned = now_ns()
            try:  # observation only: the round's measured phases and read counts
                stats = census.stats
                self.note(entry, phase="scanned", scanned_ns=scanned, snapshot_ns=stats["snapshot_ns"],
                          scan_ns=stats["scan_ns"], rows=stats["rows"],
                          esrch_rows=stats["esrch_rows"], zombie_rows=stats["zombie_rows"],
                          confirm_reads=stats["confirm_reads"], pause_calls=stats["pause_calls"])
                self.note(phase="scanned")
            except BaseException:
                pass
            if "census-uncertain" in self.inject or (
                    "census-uncertain-first" in self.inject and not self.receipt["census_rounds"]):
                result = dict(result, certain=False, uncertain=max(1, result["uncertain"]))
            waiting = now_ns()
            self.note(entry, phase="lock-wait", lock_wait_ns=waiting)
            self.note(phase="lock-wait")
            with self.lock:
                acquired = now_ns()
                self.receipt["census"] = result
                self.receipt["census_rounds"] += 1
            released = now_ns()
            self.note(entry, phase="locked", lock_acquired_ns=acquired, lock_released_ns=released,
                      certain=result["certain"], members=result["members"],
                      uncertain=result["uncertain"])
            self.note(phase="locked")
            self.send("census", after=after, members=result["members"], uncertain=result["uncertain"],
                      certain=result["certain"])
            # `sent_ns` also covers the send's own lock acquisition and pipe write.
            sent = now_ns()
            self.note(entry, sent_ns=sent)
            done = now_ns()
            self.note(entry, end_ns=done, phase="complete")
            self.note(rounds_completed=self.settlement["rounds_completed"] + 1, phase="round-complete")
            self.census_stop.wait(CENSUS_INTERVAL)
        try:
            self.settlement["loop_exit_ns"] = now_ns()
            self.settlement["phase"] = "loop-exit"
        except BaseException:
            pass

    def await_command(self, kind, wait_s):
        """One bounded read of a specific command from the custodian. A missing,
        late, malformed or different answer is None: no authority is inferred."""
        deadline = time.monotonic() + max(0.0, wait_s)
        buffer = b""
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return None
            try:
                ready, _, _ = select.select([self.cmd_r], [], [], remaining)
            except OSError:
                return None
            if not ready:
                continue
            try:
                chunk = os.read(self.cmd_r, 4096)
            except OSError:
                return None
            if not chunk:
                return None
            buffer += chunk
            while b"\n" in buffer:
                line, buffer = buffer.split(b"\n", 1)
                try:
                    command = json.loads(line)
                except ValueError:
                    return None
                if isinstance(command, dict) and command.get("type") == "record":
                    self.keep_record(command.get("record"))
                    continue
                if isinstance(command, dict) and command.get("type") == kind:
                    return command

    def detach_still_allowed(self):
        """Asked while the detach child runs: authority withdrawn mid-attempt
        (a fault latched, the deadline reached, the custodian gone) stops it."""
        allowed, reason, _granted = self.authorize_detach(0, DETACH_POLL_S)
        return allowed, reason

    def authorize_detach(self, budget_ns, wait_s=None):
        """Ask the custodian -- the only holder of the latched fault, the deadline
        and the closure verdict -- before the one detach attempt and again while it
        runs. Bounded by the budget itself: a missing, late, malformed or refused
        answer authorizes nothing, so the mount, backing and evidence are kept."""
        try:
            self.send("detach_ready", budget_ns=int(budget_ns))
        except OSError:
            return False, "the authorization request could not be sent", 0
        wait = DETACH_GATE_WAIT_S if wait_s is None else wait_s
        deadline = time.monotonic() + (min(wait, budget_ns / NS) if budget_ns else wait)
        buffer = b""
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return False, "the guard did not answer in time", 0
            try:
                ready, _, _ = select.select([self.cmd_r], [], [], remaining)
            except OSError:
                return False, "the authorization channel is unreadable", 0
            if not ready:
                continue
            try:
                chunk = os.read(self.cmd_r, 4096)
            except OSError:
                return False, "the authorization channel is unreadable", 0
            if not chunk:
                return False, "the guard closed the authorization channel", 0
            buffer += chunk
            while b"\n" in buffer:
                line, buffer = buffer.split(b"\n", 1)
                try:
                    command = json.loads(line)
                except ValueError:
                    return False, "the authorization answer is malformed", 0
                if not isinstance(command, dict) or command.get("type") != "detach_go":
                    continue
                granted = command.get("budget_ns")
                if not typed(granted, int) or granted < 0:
                    return False, "the authorization answer carries no usable budget", 0
                if command.get("allowed") is not True:
                    return False, str(command.get("reason") or "the guard refused"), 0
                return True, None, min(granted, budget_ns)

    def commands(self):
        buffer = b""
        while True:
            chunk = os.read(self.cmd_r, 4096)
            if not chunk:
                return
            buffer += chunk
            while b"\n" in buffer:
                line, buffer = buffer.split(b"\n", 1)
                yield json.loads(line)

    def run(self):
        ctx = self.ctx
        self.setup()
        os.makedirs(ctx["evidence"], exist_ok=True)
        self.publish("started: incomplete")
        self.send("setup", root=ctx["root"], scratch=ctx["scratch"], evidence=ctx["evidence"], owned=ctx["owned"])
        if "setup-stall" in self.inject:
            time.sleep(SETUP_S + 5)
        for target in (self.sample_reserve, self.sample_owned, self.sample_admission):
            threading.Thread(target=target, daemon=True).start()
        for fd, name, sink in self.pipes:
            threading.Thread(target=self.drain, args=(fd, name, sink), daemon=True).start()
        while self.receipt["reserve"]["samples"] == 0 or self.receipt["owned_bytes"]["samples"] == 0:
            time.sleep(0.02)
        self.send("ready")
        if "monitor-die" in self.inject:
            time.sleep(1.0)
            os._exit(3)
        decision = "keep"
        for command in self.commands():
            kind = command["type"]
            if kind == "leader":
                row, _problem = process_row(command["pid"])
                with self.lock:
                    self.leader = [command["pid"], None if row is None else row["start"]]
                    self.receipt["leader"] = self.leader
                if "malformed-message" in self.inject:  # a census with every field missing
                    threading.Timer(INJECT_AFTER_GATE_NS / NS, lambda: self.send("census")).start()
            elif kind == "record":
                with self.lock:
                    self.receipt["custodian"].append(command["record"])
            elif kind == "stopped":
                self.note_stopped(command, now_ns())
                if "monitor-stall" in self.inject:
                    with self.lock:  # every sender blocks: readings go stale
                        time.sleep(LIMIT_S)
                if self.leader is not None:
                    threading.Thread(target=self.census_loop, args=(command["seq"],), daemon=True).start()
            elif kind in ("cleanup", "keep"):
                decision = kind
                with self.lock:
                    self.receipt["outcome"] = command.get("outcome")
                break
        self.note(stop_requested_ns=now_ns())
        self.census_stop.set()
        # Only a run that took part in the image protocol drains at completion; a
        # keep/UNCLOSED decision never does, so the default is initialized here.
        opted_in = False
        if decision == "cleanup":
            self.publish("closure_established: cleanup_pending")
            # None unless this proof published a request; the custodian must also
            # authorize the one attempt from its own fault/deadline state.
            detach = post_census_detach(ctx, authorize=self.authorize_detach,
                                        still_allowed=self.detach_still_allowed)
            opted_in = detach is not None
            if detach is not None:
                handle = detach.pop("_handle", None)  # a live child never reaches the receipt
                with self.lock:
                    self.receipt["detach"] = detach
                self.send("detach", attempted=detach["attempted"], qualified=detach["qualified"],
                          post_state=detach["post_state"])
                if not detach["qualified"]:
                    # Never retried, never cleaned up and never a positive conclusion:
                    # the mount, its backing and the evidence are kept.
                    self.publish("detach_unqualified: scratch_kept")
                    self.receipt["scratch_marker"] = self.conclude_marker(
                        False, reason="detach", outcome="failed")
                    if handle is not None and not detach.get("reaped"):
                        # The direct child is live and unreaped. The monitor may not
                        # reap it after handing custody over (a reaped PID can be
                        # recycled, and a bare PID must never be signalled), so it
                        # names the child by PID AND start time, then waits for the
                        # custodian's acknowledgement before touching it again.
                        row, identity_problem = process_row(getattr(handle, "pid", 0) or 0)
                        window_end = (ctx["start_ns"] + int(ctx["limit_s"] * NS)
                                      - EXIT_MARGIN_S * NS - int(0.2 * NS))
                        ack = None
                        if row is not None:
                            self.send("detach_child", pid=int(row["pid"]), start=int(row["start"]))
                            ack = self.await_command("detach_child_ack",
                                                    max(0.0, (window_end - now_ns()) / NS))
                        with self.lock:
                            detach["custodian_closed"] = bool(ack and ack.get("closed") is True)
                            detach["custodian_owned"] = bool(ack and ack.get("custodian_closes") is True)
                            detach["identity_problem"] = identity_problem
                            self.receipt["detach"] = detach
                        if row is None and identity_problem is None:
                            # Positively absent: no row to name and no foreign PID to
                            # signal, so it is accounted here by reaping whatever the
                            # kernel still holds for this child. The receipt is only
                            # truthful when that reap actually succeeded; a failed
                            # reap is unaccounted custody and the monitor holds.
                            absent_reaped, _status = reap_own_child(handle, window_end)
                            with self.lock:
                                detach["reaped"] = bool(absent_reaped)
                                detach["reaped_by"] = "monitor" if absent_reaped else None
                                detach["child_absent"] = True
                                detach["reap_timed_out"] = not absent_reaped
                                self.receipt["detach"] = detach
                            if not absent_reaped:
                                self.publish("detach_child_unaccounted: monitor_held")
                                self.summary()
                                end = ctx["start_ns"] + int(ctx["limit_s"] * NS) - int(0.1 * NS)
                                while not self.stop.is_set() and now_ns() < end:
                                    time.sleep(0.02)
                                return
                            self.publish("detach_child_absent: scratch_kept")
                            self.finish_monitor(ctx, "monitor_complete: pending_custodian", drain=True)
                            return
                        if identity_problem is not None:
                            # Its identity could not be read, so no PID is handed
                            # over and nothing is signalled by identity. The child is
                            # still this monitor's own direct child, though: while it
                            # is held unreaped its PID cannot be recycled, so it is
                            # stopped and reaped here, positively, inside the
                            # custodian's remaining deadline.
                            self.publish("detach_child_identity_unreadable: closing_own_child")
                            reaped_here = False
                            status = None
                            if "detach-handoff-unreadable" not in injections():
                                # Spend at most three fifths of the remaining window
                                # on the monitor's own close, so a failed or slow
                                # reap still leaves budget for the handoff.
                                left_ns = max(0, window_end - now_ns())
                                own_deadline = now_ns() + (left_ns * 3) // 5
                                reaped_here, status = close_own_child(handle, own_deadline)
                            with self.lock:
                                detach["reaped"] = bool(reaped_here)
                                detach["reaped_by"] = "monitor" if reaped_here else None
                                self.receipt["detach"] = detach
                            if reaped_here:
                                # The direct child is positively closed by its own
                                # parent; the detach itself still stays unqualified.
                                self.publish("detach_child_closed_by_monitor: scratch_kept")
                                self.finish_monitor(ctx, "monitor_complete: pending_custodian", drain=True)
                                return
                            # Not closed here: hand it to the custodian by PARENTAGE
                            # (no start time to check) while still holding it
                            # unreaped, so it can signal and positively observe it.
                            self.send("detach_child", pid=int(getattr(handle, "pid", 0) or 0),
                                      start=None)
                            ack = self.await_command("detach_child_ack",
                                                    max(0.0, (window_end - now_ns()) / NS))
                            with self.lock:
                                detach["custodian_closed"] = bool(ack and ack.get("closed") is True)
                                detach["custodian_owned"] = bool(ack and ack.get("custodian_closes") is True)
                                self.receipt["detach"] = detach
                            if detach["custodian_closed"]:
                                self.publish("detach_child_closed_by_custodian: scratch_kept")
                                self.finish_monitor(ctx, "monitor_complete: pending_custodian", drain=True)
                                return
                            self.publish("detach_child_unaccounted: monitor_held")
                            self.summary()
                            end = ctx["start_ns"] + int(ctx["limit_s"] * NS) - int(0.1 * NS)
                            while not self.stop.is_set() and now_ns() < end:
                                time.sleep(0.02)
                            return
                        if detach["custodian_closed"]:
                            # The custodian positively closed the child it was handed.
                            self.publish("detach_child_closed_by_custodian: scratch_kept")
                            self.finish_monitor(ctx, "monitor_complete: pending_custodian", drain=True)
                            return
                        if not detach["custodian_owned"] and row is not None:
                            # Declined (already gone): the monitor may finish the
                            # reap itself, still only inside the window.
                            status = None
                            while now_ns() < window_end and not self.stop.is_set():
                                try:
                                    left_s = max(0.0, (window_end - now_ns()) / NS)
                                    status = handle.wait(timeout=min(0.05, left_s))
                                except BaseException:
                                    status = None
                                if status is not None:
                                    break
                            with self.lock:
                                detach["reaped"] = status is not None
                                detach["reap_timed_out"] = status is None
                                self.receipt["detach"] = detach
                            if status is not None:
                                self.publish("detach_child_accounted: scratch_kept")
                                self.finish_monitor(ctx, "monitor_complete: pending_custodian", drain=True)
                                return
                        # Still unaccounted: hold until the custodian ends the run,
                        # never claiming a closure nobody observed.
                        self.publish("detach_child_unaccounted: monitor_held")
                        self.summary()
                        end = ctx["start_ns"] + int(ctx["limit_s"] * NS) - int(0.1 * NS)
                        while not self.stop.is_set() and now_ns() < end:
                            time.sleep(0.02)
                        return
                    self.finish_monitor(ctx, "monitor_complete: pending_custodian", drain=True)
                    return
            # The custody indication is checked for EVERY cleanup run, whether or
            # not a detach request succeeded: a proof can declare more than one
            # backing, and a second live one must never be cleaned just because the
            # requested image detached. Without an indication this is one lstat and
            # no scan, so a run that published neither keeps the default path.
            custody = cleanup_safety(ctx)
            if custody is not None:
                opted_in = True
                if custody["state"] == "kept":
                    with self.lock:
                        self.receipt["cleanup"] = custody
                    self.send("cleanup_kept", reason=custody["reason"][:200])
                    self.publish("cleanup_kept: scratch_kept")
                    self.receipt["scratch_marker"] = self.conclude_marker(
                        False, reason="live-image", outcome="failed")
                    self.finish_monitor(ctx, "monitor_complete: pending_custodian", drain=True)
                    return
            # Final gate, for an OPTED-IN run only: cleanup of a root that took
            # part in the image protocol is destructive and needs one more
            # authorization from the custodian's CURRENT fault/deadline state and
            # what is left of its wait window. A run with neither indication nor
            # request is not gated, so its cleanup is unchanged.
            if opted_in:
                allowed, reason = self.detach_still_allowed()
                remaining_ms = detach_budget(ctx) / _MS
            else:
                allowed, reason, remaining_ms = True, None, 1.0
            if not allowed or remaining_ms <= 0:
                kept = {"state": "kept", "at_ns": now_ns(),
                        "reason": (f"the guard did not authorize cleanup: {reason}" if not allowed
                                   else "no custodian wait window is left for cleanup"),
                        "unrequested": []}
                with self.lock:
                    self.receipt["cleanup"] = kept
                self.send("cleanup_kept", reason=kept["reason"][:200])
                self.publish("cleanup_kept: scratch_kept")
                self.receipt["scratch_marker"] = self.conclude_marker(
                    False, reason="live-image", outcome="failed")
                self.finish_monitor(ctx, "monitor_complete: pending_custodian", drain=True)
                return
            self.publish("cleanup_partial_or_unknown")
            if ctx["owned"]:
                if "cleanup-interrupt" in self.inject:
                    os._exit(4)  # the published phase stays partial
                shutil.rmtree(ctx["scratch"], ignore_errors=True)
                if os.path.lexists(ctx["scratch"]):
                    self.send("cleanup_failed")
                    return
            self.publish("cleanup_complete")
        else:
            self.publish("unclosed: scratch_kept")
        self.receipt["scratch_marker"] = self.conclude_marker(decision == "cleanup")
        self.finish_monitor(ctx, "monitor_complete: pending_custodian", drain=opted_in)

    def conclude_marker(self, closed, reason=None, outcome=None):
        """Record this run's genuine conclusion on its owner marker, or leave it
        honestly unconcluded. Only the monitor's own closure decision (the
        custodian's, taken after the leader was reaped) is passed here, and a
        post-census detach that did not positively detach its own image -- or a
        cleanup that kept the root because it still held a mounted image -- is
        never a positive conclusion.

        A root the qualification apparatus protected as `hosting` before any
        nested case existed is never concluded from that outer closure: the
        marker stays unconcluded and the receipt says so. Any other failure
        leaves the marker untouched and is reported with its reason, never
        silently swallowed and never turned into a positive conclusion."""
        marker = self.ctx.get("marker") or {}
        record = {"path": marker.get("path"), "state": "none", "run_id": marker.get("run_id"),
                  "reason": None}
        if not marker.get("path") or not marker.get("run_id"):
            return record
        try:
            document = record_conclusion(self.ctx["root"], marker["run_id"], DefaultProbe(),
                                         outcome or ("closed" if closed else "unclosed"),
                                         "positive" if closed else "none",
                                         reason or ("evidence" if closed else "unclosed"))
            record.update(state="concluded" if closed else "kept",
                          kept_reason=document["kept_reason"],
                          concluded_unix=document["concluded_unix"])
        except MarkerInvalid as invalid:
            record["state"] = "nested-protected" if invalid.code == "nested-unconcluded" else "unknown"
            record["reason"] = invalid.code
        except BaseException as error:
            record["state"] = "unknown"
            record["reason"] = type_name(error)
        return record

    def summary(self):
        receipt, reserve = self.receipt, self.receipt["reserve"]
        outcome = receipt.get("outcome") or {}
        minimum = "none" if reserve["minimum"] is None else f"{reserve['minimum'] / GIB:.1f} GiB"
        peak = receipt["owned_bytes"]["maximum"] or 0
        lines = [
            f"supervisor: {outcome.get('closure', 'UNCLOSED')}; command status {outcome.get('status')};"
            f" fault {outcome.get('fault')}; deadline {outcome.get('deadline')}",
            f"supervisor: scratch default {receipt['scratch_default']['source']} at"
            f" {receipt['scratch_default']['root_parent']}; reserve sampled on {reserve['path']}"
            f" (device {reserve['device']})",
            f"supervisor: host free sampled {reserve['samples']} times every {RESERVE_INTERVAL} s (largest gap"
            f" {reserve['max_gap_ns'] / NS:.3f} s), minimum {minimum}, abort threshold 22 GiB; sampled, not an"
            " instantaneous guarantee",
            f"supervisor: owned bytes peak {peak / MIB:.1f} MiB of {receipt['owned_bytes']['budget'] // MIB} MiB"
            f" monitored budget; evidence {receipt['evidence']}",
        ]
        kept = receipt.get("cleanup")
        if kept is not None:
            lines.append(f"supervisor: cleanup kept the owned root: {kept['reason']}"
                         f" ({len(kept.get('unrequested') or [])} declared backing(s) checked)")
        detach = receipt.get("detach")
        if detach is not None:  # only a run whose proof asked for one carries this
            lines.append(
                f"supervisor: post-census detach {detach['state']} of {detach.get('mount')}"
                f" (device {detach.get('device')}, exit {detach['exit']}, timed out {detach['timed_out']},"
                f" post-state {detach['post_state']}, qualified {detach['qualified']})"
                + (f": {detach['reason']}" if detach.get("reason") else ""))
        for line in lines:  # one bounded write per line, so none truncates the rest
            report(line + "\n")
        admission = next((record for record in reversed(receipt["custodian"])
                          if isinstance(record, dict) and record.get("event") == "admission_decision"), None)
        if admission is not None:
            effective = (admission["absolute_ns"] - receipt["start_ns"]) / NS
            report(f"supervisor: admission wait reported {admission['reported_wait_ns'] / NS:.1f} s in"
                   f" {admission['counted']} marker(s), granted {admission['granted_ns'] / NS:.1f} s of the"
                   f" {ADMISSION_CAP_NS / NS:.0f} s cap (limit {admission['limit_s']:.0f} s, deadline"
                   f" {effective:.1f} s after start); {admission['ignored']} late;"
                   f" observed {receipt['admission']['observed']}, rejected {receipt['admission']['ignored']}\n")
            if admission["ignored"]:
                reason = admission.get("late_reason") or "unknown"
                report(f"supervisor: an admission marker reached the custodian after the stop decision"
                       f" ({reason}), so its wait is reported and not granted; the deadline classification"
                       " stands and no clock was moved\n")
            pairs = admission.get("admission_waiting") or {}
            rejected_waiting = receipt["admission_waiting"]["ignored"]
            rejected_grants = receipt["admission"]["ignored"]
            if pairs.get("commands") or rejected_waiting:
                report(f"supervisor: admission waits reported by {pairs.get('commands', 0)} command(s):"
                       f" {pairs.get('ended', 0)} ended, {pairs.get('unpaired_count', 0)} unpaired"
                       f" ({pairs.get('possibly_overwritten_count', 0)} with a later admission report);"
                       f" granted {admission['granted_ns'] / NS:.1f} s; {rejected_waiting} waiting and"
                       f" {rejected_grants} completed report(s) rejected"
                       + (f"; {admission['unclassified']} unpaired" if admission.get("unclassified") else "")
                       + (f"; {admission['duplicates']} duplicate completion(s) ignored"
                          if admission.get("duplicates") else "")
                       + "\n")
            for entry in (pairs.get("unpaired") or [])[:3]:
                if entry["later_observation"] or rejected_waiting or rejected_grants:
                    report(f"supervisor: command {entry['command']} reported waiting ({entry['reason']}) at"
                           f" {(entry['waiting_ns'] - receipt['start_ns']) / NS:.1f} s; a later admission"
                           " report or a rejected read may have overwritten its completion before the"
                           " supervisor polled it, so that wait is unclassified, not a proven unfinished"
                           " wait\n")
                else:
                    report(f"supervisor: command {entry['command']} reported waiting ({entry['reason']}) at"
                           f" {(entry['waiting_ns'] - receipt['start_ns']) / NS:.1f} s; no admission-file"
                           " observation followed it, but the fixed-name completion may have been missed"
                           " between polls, so its terminal evidence is missing and the wait is"
                           " unclassified, not a proven unfinished wait\n")
            if pairs.get("unpaired_count"):
                report("supervisor: the deadline classification stands for the unpaired command(s); no"
                       " clock was moved for them\n")
            if rejected_waiting:
                report(f"supervisor: {rejected_waiting} waiting report(s) were rejected"
                       f" ({', '.join(receipt['admission_waiting']['rejections'][:3])}); those waits are"
                       " unclassified\n")


class SetupRefused(Exception):
    pass


def monitor_main(ctx, ctl_w, cmd_r, out_r, err_r):
    code = 1
    try:
        Monitor(ctx, ctl_w, cmd_r, out_r, err_r).run()
        code = 0
    except SetupRefused as refusal:
        report(f"bounded-test.py: refused before any proof work: {refusal}\n")
        code = 2
    except BaseException as error:
        # No traceback and no source lookup: the class name only. A silent exit
        # here reached the custodian as an unexplained 125.
        report("bounded-test.py: monitor failed before completion: " + type_name(error) + "\n")
        code = 1
    finally:
        os._exit(code)


# Custodian -----------------------------------------------------------------------


class Custodian:
    def __init__(self, command, limit_s=LIMIT_S, budget=SCRATCH_BUDGET, evidence=None, root_parent=None):
        _ROLE[0] = "custodian"
        self.command, self.limit_s, self.budget, self.evidence_arg = command, limit_s, budget, evidence
        self.root_parent = root_parent
        self.inject = {item for item in os.environ.get("OXIDE_SUPERVISOR_INJECT", "").split(",") if item}
        if self.inject - INJECTIONS:
            raise SystemExit(f"bounded-test.py: unknown injection {sorted(self.inject - INJECTIONS)}")
        self.cancelled = None
        self.ctl_buffer = b""
        self.monitor_seq = self.seq = 0
        self.leader = self.monitor = None
        self.leader_start = None
        self.leader_bound = False
        self.leader_exited = self.monitor_exited = self.monitor_reaped = False
        self.leader_status = None
        self.ready = False
        self.eof = set()
        self.census_ok = False
        self.census_uncertain = False
        # Observation-only settlement counters (oxide#147): what the custodian
        # itself saw before its closure decision. They never change
        # eligibility, closure or any fault.
        self.stopped_at = None
        self.census_messages = 0
        self.census_eligible = 0
        self.census_first_at = None
        self.census_last_at = None
        self.reaped_seq = None
        self.monitor_complete = False
        self.detach_child = None
        self.deadline_hit = False
        self.gate_opened = False
        self.gate_at = None
        self.pre_gate_killed = self.leader_killed = False
        self.not_launched = None
        # Admission grants (oxide#192): counted from the monitor's marker
        # observations, bounded by the run's own elapsed time and the fixed cap.
        self.admission_counted = 0
        self.admission_ignored = 0
        self.admission_reported = 0
        self.admission_granted = 0
        self.admission_records = 0
        self.admission_clamped = self.admission_capped = False
        self.admission_late_ns = None
        self.admission_late_reason = None
        self.admission_waiting_reports = 0
        self.admission_waiting_ns = None
        self.admission_waiting_reason = None
        self.admission_pairs = {}
        self.admission_unclassified = 0
        self.admission_ordinal = 0
        self.admission_last_ordinal = 0
        self.admission_rejected_reports = 0
        self.admission_duplicates = 0
        self.sampling_ended = False
        self.exceptions = 0
        self.injected = set()
        self.paths = None  # from the monitor's setup message

    def record(self, kind, **fields):
        self.tell("record", record={"at": now_ns(), "event": kind, **fields})

    def tell(self, kind, **fields):
        self.seq += 1
        data = (json.dumps({"run": self.run_id, "seq": self.seq, "type": kind, **fields}) + "\n").encode()
        try:
            os.write(self.cmd_w, data)
        except OSError as error:
            self.guard.latch("protocol: command pipe write failed: " + errno_name(error))

    def signal_pinned(self, pid, number, group):
        """Signal a child this process holds unreaped, so neither its PID nor
        the group it leads can have been reused. Records custody at the
        moment of the signal."""
        held = (pid == self.leader and self.leader_status is None) or (
            pid == self.monitor and not self.monitor_reaped)
        entry = {"target": pid, "killpg": group, "signal": signal.Signals(number).name,
                 "pin": "leader" if pid == self.leader else "monitor" if pid == self.monitor else None,
                 "held_unreaped": held}
        if not held:
            entry["result"] = "refused: not a pinned child"
        else:
            try:
                (os.killpg if group else os.kill)(pid, number)
                entry["result"] = "sent"
            except OSError as error:
                entry["result"] = errno_name(error)
                if entry["result"] not in ("ESRCH", "EPERM"):  # EPERM: zombie-only group
                    self.guard.latch(f"signal {entry['signal']} to {pid}: {entry['result']}")
        self.record("signal", **entry)

    def reap(self, pid):
        try:
            done, status = os.waitpid(pid, os.WNOHANG)
        except ChildProcessError:
            self.guard.latch(f"reap: {pid} is not our child")
            return None
        return status if done == pid else None

    def exception(self, error):
        """Latch first, from the class name alone; then, best effort, record
        the innermost function and line (first few only). Never converts the
        exception to text, never looks up source, never raises."""
        self.guard.latch("custodian exception: " + type_name(error))
        try:
            self.exceptions += 1
            if self.exceptions <= EXCEPTION_RECORDS:
                self.record("exception", type=type_name(error), where=where_of(error))
        except BaseException:
            pass

    def inject_once(self, name):
        if (name in self.inject and name not in self.injected and self.gate_at is not None
                and now_ns() >= self.gate_at + INJECT_AFTER_GATE_NS):
            self.injected.add(name)
            return True
        return False

    def pump(self, timeout_ns):
        """One bounded wait for events and messages. A kqueue or message
        failure is latched and the wait degrades to select on the control
        pipe, so shutdown keeps reading the monitor and stays bounded."""
        timeout = max(0, min(timeout_ns, 50_000_000)) / NS
        if self.inject_once("pump-exception"):
            raise RuntimeError("injected event-loop exception")
        if self.inject_once("kqueue-exception"):
            self.kq = BrokenQueue()
        try:
            events = self.kq.control(None, 8, timeout)
        except Exception as error:
            self.exception(error)
            events = []
            try:
                select.select([self.ctl_r], [], [], timeout)
            except Exception:
                time.sleep(timeout)
        for event in events:
            if event.filter == select.KQ_FILTER_PROC:
                if event.ident == self.leader:
                    self.leader_exited = True
                elif event.ident == self.monitor:
                    self.monitor_exited = True
        try:
            self.read_messages()
        except Exception as error:
            self.exception(error)
        if self.cancelled:
            self.guard.latch(f"cancelled by signal {self.cancelled}")
        if self.monitor_exited and not self.monitor_complete:
            self.guard.latch("monitor died")
        self.check_readings()

    def check_readings(self):
        """Enforce reading freshness only while the run still has a decision to
        make (oxide#192).

        The monitor stops its samplers when it publishes `monitor_complete`, so
        from that point a missing reading is the designed shutdown, not a stalled
        monitor: latching staleness there would turn a completed, closed and
        cleaned run into exit 125 without changing any closure fact. Threshold
        faults from readings that do arrive and every closure rule are unchanged,
        and the whole settle phase still enforces freshness."""
        try:
            if not self.sampling_ended:
                self.guard.check(now_ns())
        except BaseException as error:
            self.exception(error)

    def read_messages(self):
        while True:
            try:
                chunk = os.read(self.ctl_r, 65536)
            except BlockingIOError:
                break
            if not chunk:
                break
            self.ctl_buffer += chunk
            if len(self.ctl_buffer) > 4 * MAX_LINE:
                break
        while b"\n" in self.ctl_buffer:
            line, self.ctl_buffer = self.ctl_buffer.split(b"\n", 1)
            try:
                message = json.loads(line)
            except ValueError:
                self.guard.latch("protocol: malformed control message")
                continue
            self.accept(message)
        if len(self.ctl_buffer) > MAX_LINE:
            self.guard.latch("protocol: control line overflow")
            self.ctl_buffer = b""

    def admit(self, message, at):
        """Grant bounded time for one reported admission wait (oxide#192).

        A child's marker is cooperative evidence: the custodian adds at most
        `waited_ns`, never more than this run's own elapsed wall clock and
        never beyond ADMISSION_CAP_NS in total, to `absolute`. Nothing is
        shortened, no exported deadline is rewritten and no classification
        changes: the grant is reported separately in the receipt. A marker that
        reaches the custodian after the stop decision, or after a deadline was
        declared, cannot move anything; it is recorded as late with its reason
        so the run is classified honestly rather than as an unexplained
        timeout."""
        try:
            self.admission_ordinal += 1
            self.admission_last_ordinal = self.admission_ordinal
            pair = self.admission_pair(message["command"])
            if pair is not None and pair["ended"] is not None:
                # One terminal per command grants time at most once, whatever the
                # monitor's deduplication does: a duplicate completion can never
                # grant the same wait twice.
                self.admission_duplicates += 1
                return
            if self.stopped_at is not None or self.deadline_hit:
                self.admission_ignored += 1
                self.admission_late_ns = at
                self.admission_late_reason = "stopped" if self.stopped_at is not None else "deadline"
                # The wait ended after the stop: still a completed wait for this
                # command, so it is never reported as unfinished.
                if pair is not None and pair["ended"] is None:
                    pair.update(ended=self.admission_late_reason, ended_ns=at,
                                waited_ns=message["waited_ns"])
                if self.admission_records < ADMISSION_RECORDS:
                    self.admission_records += 1
                    self.record("admission_late", command=message["command"],
                                waited_ns=message["waited_ns"], at=at,
                                reason=self.admission_late_reason, deadline_hit=self.deadline_hit,
                                stopped=self.stopped_at is not None)
                return
            # A completed wait ends this command's wait even when it grants no
            # time: a zero-duration grant or a cap-refused grant is an ending,
            # not an unfinished wait.
            if pair is not None and pair["ended"] is None:
                pair.update(ended="grant", ended_ns=at, waited_ns=message["waited_ns"])
            waited = message["waited_ns"]
            elapsed = max(0, at - self.start)
            clamped = waited > elapsed
            if clamped:
                waited = elapsed
            room = max(0, ADMISSION_CAP_NS - self.admission_granted)
            grant = min(waited, room)
            self.admission_counted += 1
            self.admission_reported = min(U64MAX, self.admission_reported + message["waited_ns"])
            self.admission_clamped = self.admission_clamped or clamped
            if grant <= 0:
                self.admission_capped = True
                return
            self.admission_granted += grant
            self.admission_capped = self.admission_granted >= ADMISSION_CAP_NS
            self.absolute = self.start + int(self.limit_s * NS) + self.admission_granted
            self.stop_at = self.absolute - STOP_RESERVE_S * NS
            self.exit_at = self.absolute - EXIT_MARGIN_S * NS
            if self.admission_records < ADMISSION_RECORDS:
                self.admission_records += 1
                self.record("admission", command=message["command"], waited_ns=message["waited_ns"],
                            grant_ns=grant, clamped=clamped, granted_ns=self.admission_granted,
                            absolute_ns=self.absolute)
        except BaseException as error:
            self.exception(error)

    def admission_pair(self, command):
        """One command token's paired record, or None when the table is full.

        A bounded table (ADMISSION_COMMANDS) pairs each supervised command's
        waiting report with its own completion, so a stale file, a zero-duration
        or late grant and a lost report are never reported as a proven
        unfinished wait. Overflow is counted and treated as unclassified, never
        as evidence."""
        entry = self.admission_pairs.get(command)
        if entry is not None:
            return entry
        if len(self.admission_pairs) >= ADMISSION_COMMANDS:
            self.admission_unclassified += 1
            return None
        entry = {"command": command, "waiting_ns": None, "reason": None,
                 "waiting_ordinal": None, "ended": None, "ended_ns": None, "waited_ns": None}
        self.admission_pairs[command] = entry
        return entry

    def note_waiting(self, message, at):
        """Record that a child reported a wait that has not ended (oxide#192).

        Report only: a waiting report never moves `absolute`, `stop_at` or
        `exit_at`, before or after a stop decision. It exists so a wait that
        never ends before the stop is classified separately from a plain
        timeout instead of leaving no trace."""
        try:
            self.admission_ordinal += 1
            self.admission_last_ordinal = self.admission_ordinal
            self.admission_waiting_reports += 1
            self.admission_waiting_ns = at
            if self.admission_waiting_reason is None:
                self.admission_waiting_reason = message["reason"]
            pair = self.admission_pair(message["command"])
            if pair is not None and pair["waiting_ns"] is None:
                pair["waiting_ns"], pair["reason"] = at, message["reason"]
                pair["waiting_ordinal"] = self.admission_ordinal
            if self.admission_records < ADMISSION_RECORDS:
                self.admission_records += 1
                self.record("admission_waiting", command=message["command"], reason=message["reason"],
                            at=at, after_stop=self.stopped_at is not None or self.deadline_hit,
                            granted_ns=self.admission_granted)
        except BaseException as error:
            self.exception(error)

    def accept(self, message):
        problem = malformed(message)
        if problem:
            self.guard.latch(f"protocol: malformed {problem} ({message.get('type') if isinstance(message, dict) else '?'})")
            if isinstance(message, dict) and typed(message.get("seq"), int):
                self.monitor_seq = max(self.monitor_seq, message["seq"])
            return
        if message.get("run") != self.run_id or message.get("seq") != self.monitor_seq + 1:
            self.guard.latch(f"protocol: unexpected run or sequence ({message.get('type')})")
            return
        self.monitor_seq = message["seq"]
        leader = message.get("leader")
        if leader is not None:
            if leader[0] != self.leader or self.leader is None or (
                    self.leader_start is not None and leader[1] != self.leader_start):
                self.guard.latch("protocol: message names a leader that is not the pinned leader")
                return
            self.leader_start, self.leader_bound = leader[1], True
        elif self.leader_bound:
            self.guard.latch("protocol: leader identity dropped after binding")
            return
        kind = message["type"]
        if kind == "reading":
            self.guard.reading(message, now_ns())
        elif kind == "setup":
            if self.paths is not None:
                self.guard.latch("protocol: second setup message")
                return
            self.paths = message
            if message["owned"]:
                for name in ("OXIDE_TASK_SCRATCH_ROOT", "TMPDIR", "TMP", "TEMP"):
                    self.env[name] = message["scratch"]
            self.env["OXIDE_PROOF_EVIDENCE"] = message["evidence"]  # kept; never removed by cleanup
        elif kind == "ready":
            # Ready counts only after setup and with completed readings received.
            if self.paths is None:
                self.guard.latch("protocol: ready before setup")
            elif self.guard.reserve_at is None or self.guard.scratch_at is None:
                self.guard.latch("protocol: ready before completed readings")
            self.ready = True
        elif kind == "output_eof":
            self.eof.add(message["stream"])
        elif kind == "admission":
            at = now_ns()
            if message["at"] > at:
                self.guard.latch("admission message stamped in the future")
            else:
                self.admit(message, at)
        elif kind == "admission_waiting":
            at = now_ns()
            if message["at"] > at:
                self.guard.latch("admission message stamped in the future")
            else:
                self.note_waiting(message, at)
        elif kind == "admission_rejected":
            at = now_ns()
            if message["at"] > at:
                self.guard.latch("admission message stamped in the future")
            else:
                # An observation of the fixed-name file, not evidence about any
                # command: it advances the ordinal so a wait before it can never
                # be called provably unfinished.
                self.admission_ordinal += 1
                self.admission_last_ordinal = self.admission_ordinal
                self.admission_rejected_reports += 1
        elif kind == "census":
            try:  # observation only, before the unchanged eligibility rule
                self.census_messages += 1
                if self.reaped_seq is not None and message["after"] >= self.reaped_seq:
                    at = now_ns()
                    self.census_eligible += 1
                    self.census_last_at = at
                    if self.census_first_at is None:
                        self.census_first_at = at
            except BaseException:
                pass
            if self.reaped_seq is not None and message["after"] >= self.reaped_seq:
                if not message["certain"] or message["uncertain"]:
                    # Latched: a later certain empty census never clears it.
                    self.census_uncertain = True
                    self.guard.latch(f"census uncertain: {message['uncertain']} unvalidated rows")
                self.census_ok = (not self.census_uncertain and message["certain"]
                                  and message["members"] == 0 and message["uncertain"] == 0)
        elif kind == "monitor_complete":
            self.monitor_complete = True
        elif kind == "detach_ready":
            # The monitor asks once, after its own validation, before any attempt.
            # The custodian answers from its own state only and never touches the
            # filesystem: a latched fault, a reached deadline or a spent wait
            # budget authorizes nothing.
            allowed, reason, budget = detach_authorization(self.guard.fault, self.deadline_hit,
                                                           now_ns(), self.exit_at)
            self.tell("detach_go", allowed=allowed, reason=reason, budget_ns=int(budget))
        elif kind == "detach":
            # A post-census detach that did not positively detach its own image
            # leaves the mount and backing in place and the run unqualified.
            if not message["qualified"]:
                self.guard.latch(f"post-census detach unqualified ({message['post_state']})")
        elif kind == "cleanup_kept":
            self.guard.latch(f"cleanup kept the owned root: {message['reason']}")
        elif kind == "detach_child":
            # `start` is optional: a parentage-only handoff carries none. Only a
            # present, malformed start is a protocol fault.
            pid, start = message["pid"], message.get("start")
            if not typed(pid, int) or pid <= 0 or (
                    start is not None and (not typed(start, int) or start <= 0)):
                self.guard.latch("protocol: detach_child without a valid pid and start")
                return
            if self.detach_child is not None:
                self.guard.latch("protocol: second detach_child")
                return
            row, problem = process_row(pid)
            if problem is not None:
                self.record("detach_child_refused", pid=pid, start=start,
                            observation=f"identity unreadable ({problem})")
                self.tell("detach_child_ack", custodian_closes=False, closed=False,
                          reason=f"the child's identity could not be read ({problem})")
                return
            if row is None:
                # Positively absent: there is nothing to signal, and that is a
                # positive answer, not an unreadable one.
                self.record("detach_child_refused", pid=pid, start=start,
                            observation="positively absent")
                self.tell("detach_child_ack", custodian_closes=False, closed=True,
                          reason="the child is positively gone")
                return
            # Positive identity: the same process (start time) and still this
            # monitor's child. A recycled bare PID can never match both, so it is
            # never signalled.
            if start is None:
                # Parentage-only handoff: the monitor could not read a start time,
                # so identity is this monitor's still-unreaped direct child.
                if not (row.get("pid") == pid and row.get("ppid") == self.monitor
                        and row.get("zombie") is False):
                    self.record("detach_child_refused", pid=pid, start=None,
                                observation="parentage no longer matches")
                    self.tell("detach_child_ack", custodian_closes=False, closed=True,
                              reason="the child is already gone or is not this monitor's")
                    return
                self.detach_child = [pid, row["start"]]
                self.record("detach_child_pinned", pid=pid, start=row["start"],
                            observation="live", identity="parentage")
                self.signal_detach_child()
                closed = self.observe_detach_child(NS)
                self.tell("detach_child_ack", custodian_closes=True, closed=closed,
                          reason=None if closed else "not observed closed yet")
                return
            if not detach_child_identity(row, pid, start, self.monitor):
                self.record("detach_child_refused", pid=pid, start=start,
                            observation="identity or parentage no longer matches")
                self.tell("detach_child_ack", custodian_closes=False, closed=True,
                          reason="the child is already gone or is not this monitor's")
                return
            self.detach_child = [pid, start]
            self.record("detach_child_pinned", pid=pid, start=start, observation="live")
            self.signal_detach_child()
            closed = self.observe_detach_child(NS)
            self.tell("detach_child_ack", custodian_closes=True, closed=closed,
                      reason=None if closed else "not observed closed yet")
        elif kind == "cleanup_failed":
            self.guard.latch("cleanup: owned scratch not fully removed")

    def setup(self):
        missing = capability()
        if missing:
            raise SystemExit(f"bounded-test.py: refused before any proof work; missing {', '.join(missing)}")
        signal.signal(signal.SIGCHLD, signal.SIG_DFL)  # never auto-reap a pinned child
        for number in (signal.SIGINT, signal.SIGTERM):
            signal.signal(number, lambda n, _f: setattr(self, "cancelled", self.cancelled or n))
        self.start = now_ns()
        self.absolute = self.start + int(self.limit_s * NS)
        self.stop_at = self.absolute - STOP_RESERVE_S * NS
        self.exit_at = self.absolute - EXIT_MARGIN_S * NS
        self.setup_at = min(self.start + SETUP_S * NS, self.stop_at)
        self.run_id = secrets.token_hex(16)
        self.guard = Guard(self.start, budget=self.budget)
        # No filesystem call here: path resolution and scratch/evidence
        # creation run in the monitor, inside the setup deadline, and arrive
        # in its `setup` message. The leader opens /dev/null itself.
        self.env = dict(os.environ)
        self.env["OXIDE_PROOF_DEADLINE_NS"] = str(self.absolute)
        self.env["OXIDE_PROOF_RUN_ID"] = self.run_id
        self.ctx = {
            "run_id": self.run_id, "command": self.command, "limit_s": self.limit_s,
            "adopted": os.environ.get("OXIDE_TASK_SCRATCH_ROOT") or None, "evidence_arg": self.evidence_arg,
            "root_parent": self.root_parent, "budget": self.budget, "start_ns": self.start,
            "inject": sorted(self.inject),
        }

    def launch_monitor(self):
        self.ctl_r, ctl_w = os.pipe()
        cmd_r, self.cmd_w = os.pipe()
        out_r, self.out_w = os.pipe()
        err_r, self.err_w = os.pipe()
        pid = os.fork()
        if pid == 0:
            for fd in (self.ctl_r, self.cmd_w, self.out_w, self.err_w):
                os.close(fd)
            monitor_main(self.ctx, ctl_w, cmd_r, out_r, err_r)
        self.monitor = pid
        for fd in (ctl_w, cmd_r, out_r, err_r):
            os.close(fd)
        os.set_blocking(self.ctl_r, False)
        os.set_blocking(self.cmd_w, False)
        self.kq = select.kqueue()
        self.kq.control([
            select.kevent(self.ctl_r, select.KQ_FILTER_READ, select.KQ_EV_ADD),
            select.kevent(pid, select.KQ_FILTER_PROC, select.KQ_EV_ADD | select.KQ_EV_ONESHOT, select.KQ_NOTE_EXIT),
        ], 0, 0)
        while not self.ready and self.guard.fault is None and not self.monitor_exited and not self.cancelled:
            if now_ns() >= self.setup_at:
                break
            self.pump(self.setup_at - now_ns())
        if self.ready and self.guard.fault is None and not self.cancelled:
            return True
        self.not_launched = self.guard.fault or ("cancelled" if self.cancelled else "monitor setup failed or slow")
        self.guard.latch(f"not launched: {self.not_launched}")
        return False

    def launch_leader(self):
        ack_r, ack_w = os.pipe()
        gate_r, gate_w = os.pipe()
        pid = os.fork()
        if pid == 0:
            try:
                os.setsid()
                os.dup2(os.open(os.devnull, os.O_RDONLY), 0)
                os.dup2(self.out_w, 1)
                os.dup2(self.err_w, 2)
                for fd in range(3, 256):
                    if fd not in (ack_w, gate_r):
                        try:
                            os.close(fd)
                        except OSError:
                            pass
                for number in (signal.SIGPIPE, signal.SIGINT, signal.SIGTERM, signal.SIGCHLD, signal.SIGALRM):
                    signal.signal(number, signal.SIG_DFL)
                signal.pthread_sigmask(signal.SIG_SETMASK, set())
                if "ack-eof" not in self.inject:  # session-ready: setsid and dup2 succeeded
                    os.write(ack_w, b"X" if "ack-wrong" in self.inject else b"R")
                os.close(ack_w)
                if os.read(gate_r, 2) != b"G":
                    os._exit(125)  # gate EOF, error or unexpected bytes: never exec
                os.close(gate_r)
                os.execvpe(self.command[0], self.command, self.env)
            except BaseException:
                pass
            os._exit(127)
        self.leader = pid
        for fd in (ack_w, gate_r, self.out_w, self.err_w):
            os.close(fd)
        if "cancel-before-ack" in self.inject:
            self.cancelled = signal.SIGTERM
        ack = b""
        while not self.cancelled and self.guard.fault is None and now_ns() < self.setup_at:
            ready, _, _ = select.select([ack_r], [], [], max(0, min(self.setup_at - now_ns(), 50_000_000)) / NS)
            if ready:
                try:
                    ack = os.read(ack_r, 2)
                except OSError:
                    ack = b"<error>"
                break
            self.pump(0)
        os.close(ack_r)
        detector = False
        if ack == b"R" and not self.cancelled and self.guard.fault is None and "no-detector" not in self.inject:
            try:
                detector = True
                for event in self.kq.control([select.kevent(
                        pid, select.KQ_FILTER_PROC, select.KQ_EV_ADD | select.KQ_EV_ONESHOT,
                        select.KQ_NOTE_EXIT)], 1, 0):
                    if event.flags & select.KQ_EV_ERROR:
                        if event.data == errno.ESRCH:
                            self.leader_exited = True  # already a zombie: still pinned
                        else:
                            detector = False
            except OSError:
                detector = False
        if detector and "cancel-after-ack" in self.inject:
            os.kill(os.getpid(), signal.SIGTERM)  # a real signal through the handler, after registration
        # The gate decision: INT and TERM are held blocked from the final check
        # through the gate write, so a cancellation recorded or pending before
        # the write always wins and none can land between check and write.
        held = {signal.SIGINT, signal.SIGTERM}
        signal.pthread_sigmask(signal.SIG_BLOCK, held)
        try:
            if detector and "cancel-at-gate" in self.inject:
                os.kill(os.getpid(), signal.SIGTERM)  # arrives while blocked: pending at the decision
            if detector:
                self.pump(0)
                pending = signal.sigpending() & held
                if pending and not self.cancelled:
                    self.cancelled = int(min(pending))
                if self.cancelled or self.guard.fault is not None:
                    detector = False
            if detector:
                os.write(gate_w, b"G")
                self.gate_opened, self.gate_at = True, now_ns()
        finally:
            signal.pthread_sigmask(signal.SIG_UNBLOCK, held)
        if not detector:
            reason = (f"signal {self.cancelled}" if self.cancelled else self.guard.fault) or (
                "no exit detector" if ack == b"R" else f"session-ready ack {ack!r}")
            self.record("cancel_before_gate", reason=reason, ack=ack.decode(errors="replace"))
            self.pre_gate_killed = True
            self.signal_pinned(pid, signal.SIGKILL, group=False)  # pinned PID; never killpg before the gate
            os.close(gate_w)
            self.guard.latch(f"launch cancelled: {reason}")
            return False
        os.close(gate_w)
        self.tell("leader", pid=pid)
        self.record("gate_open", leader=pid)
        return True

    def stop_leader(self, trigger):
        """TERM, bounded grace, then KILL of the pinned leader group. The KILL
        is sent even if anything before it raises."""
        try:
            self.record("stop", trigger=trigger, fault=self.guard.fault)
            end = min(now_ns() + TERM_GRACE_S * NS, self.exit_at)
            self.signal_pinned(self.leader, signal.SIGTERM, group=True)
            if "stop-exception" in self.inject:
                raise RuntimeError("injected exception during the stop")
            while now_ns() < end:
                try:
                    os.killpg(self.leader, 0)  # the pinned group: unreaped leader
                except OSError:
                    break  # no member a signal could reach
                self.pump(end - now_ns())
        finally:
            if not self.leader_killed:
                self.leader_killed = True
                self.signal_pinned(self.leader, signal.SIGKILL, group=True)

    def run(self):
        """Every phase after setup runs under an exception guard: an exception
        is latched as a fault and the bounded pinned stop, WNOHANG reap,
        census and finish (keep, never cleanup) still run."""
        self.setup()
        try:
            ready = self.launch_monitor()
        except BaseException as error:
            self.exception(error)
            ready = False
        if not ready:
            return self.abandon_launch()
        try:
            self.launch_leader()
            if self.gate_opened:
                self.watch()
        except BaseException as error:
            self.exception(error)
        launched = self.gate_opened
        for step in (self.stop, self.reap_leader, self.settle):
            try:
                step(launched)
            except BaseException as error:
                self.exception(error)
        try:
            return self.finish(launched)
        except BaseException as error:
            self.exception(error)
            return self.last_resort()

    def abandon_launch(self):
        """Setup slow or failed: launch nothing, stop the pinned monitor now.

        One bounded line names the reason before the pinned monitor is stopped, so
        a launch refusal is never silent."""
        report(f"bounded-test.py: not launched: {self.not_launched or 'setup did not finish'}\n")
        try:
            self.signal_pinned(self.monitor, signal.SIGKILL, group=False)
            status, end = None, min(now_ns() + NS, self.exit_at)
            while status is None and now_ns() < end:
                status = self.reap(self.monitor)
                if status is None:
                    self.pump(min(end - now_ns(), 20_000_000))
            self.monitor_reaped = status is not None
        except BaseException as error:
            self.exception(error)
        return 125

    def watch(self):
        while not (self.leader_exited or self.cancelled or self.guard.fault):
            if now_ns() >= self.stop_at:
                # One bounded drain before the deadline is declared, so a marker
                # that was already published and is waiting in the control pipe
                # can still grant and move the stop (oxide#192). Only a grant
                # that moves the stop past now continues the loop; a marker with
                # no room cannot spin it.
                self.pump(0)
                if now_ns() >= self.stop_at:
                    self.deadline_hit = True
                    break
                continue
            self.pump(self.stop_at - now_ns())

    def stop(self, launched):
        if self.leader is None or self.leader_status is not None:
            return
        if not launched:  # the gate never opened: the pinned PID only, never killpg
            if not self.pre_gate_killed:
                self.pre_gate_killed = True
                self.record("cancel_before_gate", reason=self.guard.fault, ack="")
                self.signal_pinned(self.leader, signal.SIGKILL, group=False)
            return
        if not self.leader_killed:
            self.stop_leader("leader exit" if self.leader_exited else "stop_at deadline" if self.deadline_hit
                             else f"signal {self.cancelled}" if self.cancelled else "fault")

    def reap_leader(self, launched):
        if self.leader is None:
            return
        while self.leader_status is None and now_ns() < self.exit_at:
            self.leader_status = self.reap(self.leader)
            if self.leader_status is None:
                self.pump(self.exit_at - now_ns())
        if self.leader_status is None:
            self.guard.latch("reap: leader not reaped by exit_at; pin kept, no further signal")
            self.deadline_hit = True

    def observe_settlement(self, launched, census_end):
        """Best-effort, observation-only decision-time snapshot of the
        settlement window. A failed write is dropped and never latched, so this
        can never turn a closed run into a fault, replace the first fault or
        depend on the monitor's later activity. Never raises."""
        try:
            at = now_ns()
            sent = self.stopped_at
            record = {
                "at": at, "event": "settlement", "launched": launched,
                "stopped_sent_ns": sent, "census_end_ns": census_end,
                "wait_ns": None if census_end is None or sent is None else max(0, min(at, census_end) - sent),
                "census_messages": self.census_messages, "census_eligible": self.census_eligible,
                "first_census_ns": self.census_first_at, "last_census_ns": self.census_last_at,                "census_ok": self.census_ok, "census_uncertain": self.census_uncertain,
                "eof": sorted(self.eof), "fault": self.guard.fault,
            }
            self.seq += 1
            data = (json.dumps({"run": self.run_id, "seq": self.seq, "type": "record",
                                "record": record}) + "\n").encode()
            os.write(self.cmd_w, data)
        except BaseException:
            pass

    def settle(self, launched):
        self.stopped_at = now_ns()
        self.tell("stopped", sent=self.stopped_at)
        self.reaped_seq = self.seq
        self.record("leader_reaped", status=self.leader_status)
        census_end = None
        if launched:
            census_end = min(now_ns() + CENSUS_GRACE_S * NS, self.exit_at)
            while not (self.census_uncertain or (self.census_ok and len(self.eof) == 2)) and now_ns() < census_end:
                self.pump(census_end - now_ns())
            if not self.census_ok:
                self.guard.latch("closure not established: no certain empty census after the leader was reaped")
            elif len(self.eof) != 2:
                self.guard.latch("closure not established: output EOF witness missing")
        self.observe_settlement(launched, census_end)
        self.observe_admission(launched)

    def admission_pairs_summary(self):
        """The bounded per-command pairing view for the decision record.

        Only commands whose waiting report was accepted are counted. A command
        is finished when its own completed report arrived at all (`grant`, even
        with a zero or clamped grant) or arrived after the stop (`late`). An
        unpaired wait is **never** called a proven unfinished wait: the completion
        file has one fixed name, so any later admission-file observation -
        another command's grant, another command's waiting report, or a rejected
        read - may have replaced a completion before the next 0.25 s poll, and a
        completion published between the last poll and the sampler's stop can be
        missed entirely. `later_observation` states whether such a later
        observation exists in this run (the summary also treats any rejected
        report as one). Both lists are bounded; the counts are the full ones."""
        waiting = [entry for entry in self.admission_pairs.values() if entry["waiting_ns"] is not None]
        ended = [entry for entry in waiting if entry["ended"] is not None]
        unpaired = [entry for entry in waiting if entry["ended"] is None]
        rows = [{"command": entry["command"], "reason": entry["reason"],
                 "waiting_ns": entry["waiting_ns"],
                 "later_observation": entry["waiting_ordinal"] != self.admission_last_ordinal}
                for entry in unpaired]
        overwritten = [row for row in rows if row["later_observation"]]
        return {"commands": len(waiting), "ended": len(ended),
                "unpaired_count": len(rows),
                "unpaired": rows[:ADMISSION_UNFINISHED],
                "possibly_overwritten_count": len(overwritten),
                "rejected_reports": self.admission_rejected_reports}

    def observe_admission(self, launched):
        """Best-effort decision-time record of the admission grants
        (oxide#192). A failed write is dropped and never latched, so this can
        never fail a run, replace the first fault or change the deadline."""
        try:
            self.seq += 1
            record = {"at": now_ns(), "event": "admission_decision", "launched": launched,
                      "counted": self.admission_counted, "ignored": self.admission_ignored,
                      "reported_wait_ns": self.admission_reported, "granted_ns": self.admission_granted,
                      "clamped": self.admission_clamped, "capped": self.admission_capped,
                      "late_ns": self.admission_late_ns, "late_reason": self.admission_late_reason,
                      "waiting_reports": self.admission_waiting_reports,
                      "waiting_ns": self.admission_waiting_ns,
                      "waiting_reason": self.admission_waiting_reason,
                      "admission_waiting": self.admission_pairs_summary(),
                      "duplicates": self.admission_duplicates,
                      "unclassified": self.admission_unclassified,
                      "cap_ns": ADMISSION_CAP_NS, "limit_s": self.limit_s,
                      "absolute_ns": self.absolute}
            os.write(self.cmd_w, (json.dumps({"run": self.run_id, "seq": self.seq, "type": "record",
                                              "record": record}) + "\n").encode())
        except BaseException:
            pass

    def last_resort(self):
        """finish raised: KILL the pinned monitor if still held, one WNOHANG
        reap, and exit non-zero. Never raises, and one bounded line names the
        reason."""
        try:
            report("bounded-test.py: finish failed:"
                   f" deadline {self.deadline_hit}; fault {self.guard.fault}\n")
            if self.monitor is not None and not self.monitor_reaped:
                self.signal_pinned(self.monitor, signal.SIGKILL, group=False)
                self.monitor_reaped = self.reap(self.monitor) is not None
        except BaseException:
            pass
        return 124 if self.deadline_hit else 125

    def signal_detach_child(self):
        """One identity-checked SIGKILL of the pinned detach child, recorded."""
        if self.detach_child is None:
            return
        pid, start = self.detach_child
        row, problem = process_row(pid)
        if problem is not None:
            self.guard.latch(f"detach child {pid} identity unreadable ({problem}); not signalled")
            return
        if not detach_child_identity(row, pid, start, self.monitor):
            return
        self.record("detach_child_signal", pid=pid, start=start)
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except OSError as error:
            self.guard.latch(f"detach child {pid} could not be signalled: {errno_name(error)}")

    def observe_detach_child(self, limit_ns):
        """Bounded observation that the pinned child is gone (or a zombie), by the
        same PID and start identity. Records the observation and never claims a
        closure it did not see."""
        if self.detach_child is None:
            return True
        pid, start = self.detach_child
        end = min(now_ns() + limit_ns, self.absolute)
        closed, uncertain = False, None
        while now_ns() < end:
            row, problem = process_row(pid)
            if problem is not None:
                # Unreadable facts are uncertainty, never closure.
                uncertain = problem
                break
            if row is None or row["start"] != start or row.get("zombie") is not False:
                closed = True
                break
            self.pump(min(end - now_ns(), 20_000_000))
        self.record("detach_child_observed", pid=pid, start=start, closed=closed,
                    uncertain=uncertain)
        return closed

    def close_detach_child(self):
        """Backstop for a monitor that ended before its child did: signal the
        pinned child again and observe it gone, bounded by the absolute deadline.
        An unobserved child is a latched fault, never a claim."""
        if self.detach_child is None:
            return
        pid, _start = self.detach_child
        self.signal_detach_child()
        if not self.observe_detach_child(NS):
            self.guard.latch(f"detach child {pid} was not observed closed")

    def closure(self, launched):
        return (launched and self.guard.fault is None and not self.census_uncertain and self.census_ok
                and len(self.eof) == 2)

    def finish(self, launched):
        self.closed = self.closure(launched)
        status = self.leader_status
        decoded = None if status is None else (
            os.WEXITSTATUS(status) if os.WIFEXITED(status) else -os.WTERMSIG(status) if os.WIFSIGNALED(status) else None)
        outcome = {"closure": "closed" if self.closed else "UNCLOSED", "status": decoded,
                   "fault": self.guard.fault, "deadline": self.deadline_hit, "launched": launched}
        self.tell("cleanup" if self.closed else "keep", outcome=outcome)
        # The monitor stops sampling once it publishes `monitor_complete`; its
        # shutdown is not a freshness window (see check_readings).
        self.sampling_ended = True
        if "custodian-timeout-after-monitor" in self.inject:
            self.exit_at = now_ns()  # injected: the custodian runs out right after deciding
        while not self.monitor_complete and not self.monitor_exited and now_ns() < self.exit_at:
            self.pump(self.exit_at - now_ns())
        if not self.monitor_complete:
            self.deadline_hit = self.deadline_hit or custodian_deadline_hit(
                now_ns(), self.absolute, self.inject)
            self.guard.latch("monitor did not complete")
        status = None
        end = min(now_ns() + NS // 2, self.absolute)
        while status is None and now_ns() < end:
            status = self.reap(self.monitor)
            if status is None:
                self.pump(min(end - now_ns(), 20_000_000))
        if status is None:
            self.signal_pinned(self.monitor, signal.SIGKILL, group=False)
            end = min(now_ns() + NS // 2, self.absolute)
            while status is None and now_ns() < end:
                status = self.reap(self.monitor)
                if status is None:
                    self.pump(min(end - now_ns(), 20_000_000))
        self.monitor_reaped = status is not None
        if status is None:
            self.guard.latch("monitor not reaped after KILL")
        self.close_detach_child()
        if not launched:
            return 124 if self.deadline_hit else 125
        return exit_code(self.deadline_hit, self.guard.fault, self.closed, self.leader_status)


class BrokenQueue:
    """Test-only stand-in for a kqueue whose every call fails."""

    def control(self, *_args):
        raise OSError(errno.EBADF, "injected kqueue failure")


def parse(argv):
    options = {"limit": LIMIT_S, "budget": SCRATCH_BUDGET, "evidence": None, "root_parent": None}
    while argv and argv[0].startswith("--"):
        flag = argv.pop(0)
        if flag == "--":
            break
        if flag not in ("--limit", "--scratch-budget-mib", "--evidence", "--root-parent") or not argv:
            raise SystemExit(f"bounded-test.py: unknown or incomplete option {flag}")
        value = argv.pop(0)
        if flag == "--limit":
            options["limit"] = float(value)
        elif flag == "--scratch-budget-mib":
            options["budget"] = int(value) * MIB
        elif flag == "--evidence":
            options["evidence"] = value
        else:
            options["root_parent"] = value
    if not argv:
        raise SystemExit("bounded-test.py: no command")
    if options["limit"] <= STOP_RESERVE_S + SETUP_S:
        raise SystemExit(f"bounded-test.py: --limit must exceed {STOP_RESERVE_S + SETUP_S} s")
    return argv, options


def main(argv):
    command, options = parse(list(argv))
    return Custodian(command, options["limit"], options["budget"], options["evidence"], options["root_parent"]).run()


if __name__ == "__main__":
    try:
        code = main(sys.argv[1:])
    except SystemExit:
        raise
    except BaseException as error:  # no traceback: bounded, non-blocking report only
        report("bounded-test.py: internal error, exit 125: " + type_name(error) + "\n")
        code = 125
    sys.exit(code)
