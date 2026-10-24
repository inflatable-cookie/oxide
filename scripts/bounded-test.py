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

Scope: the cooperative session. A process that calls setsid leaves it and is
seen only while tracked or holding an output pipe; arbitrary same-user
writers are outside the promise. Host free space is a sampled abort
threshold; the owned-byte budget is monitored, not a hard ceiling. macOS
only.

OXIDE_SUPERVISOR_INJECT (comma list from INJECTIONS) is a test-only
fault-injection hook used by scripts/test_supervisor.py.
"""
from __future__ import annotations

import ctypes
import errno
import json
import os
import secrets
import select
import shutil
import signal
import stat
import struct
import sys
import tempfile
import threading
import time

NS = 1_000_000_000
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
INJECTIONS = {
    "setup-stall", "no-detector", "ack-eof", "ack-wrong", "cancel-before-ack", "monitor-die",
    "monitor-stall", "cleanup-interrupt", "custodian-timeout-after-monitor", "census-uncertain",
    "census-uncertain-first", "reserve-low-after", "cancel-after-ack", "cancel-at-gate",
    "pump-exception", "kqueue-exception", "malformed-message", "stop-exception",
    "reserve-delay-stall", "scratch-delay-stall",
}
RESERVE_DELAY_S, SCRATCH_DELAY_S = 0.5, 4.0  # injected publication delays before a sampler stall
INJECT_AFTER_GATE_NS = 1_500_000_000  # post-gate injections wait for the fixture to start
EXCEPTION_RECORDS = 8
FIELDS = {  # required fields per monitor message type; bool is never accepted as int
    "reading": {"kind": str},
    "ready": {}, "monitor_complete": {}, "cleanup_failed": {},
    "output_eof": {"stream": str},
    "census": {"after": int, "members": int, "uncertain": int, "certain": bool},
    "setup": {"root": str, "scratch": str, "evidence": str, "owned": bool},
}


def now_ns():
    return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)


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
            tries += 1
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

    def confirm_gone(self, row):
        """True only on a successful read that lacks the identity."""
        gone, _t, _o, _e, _fr, _fe, _ac = self.confirm_gone_info(row)
        return gone

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
        try:
            rows = self.snapshot()
        except Exception as error:
            return {"certain": False, "members": len(self.tracked), "uncertain": 1,
                    "error": f"snapshot: {error}", "member_rows": [], "uncertain_rows": [],
                    "terminal_count_status": "invalid", "terminal_nonwriters": None}
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

    An entry that is gone is skipped exactly as before. Every other error
    raises, so the sampler reports a measurement fault and never a smaller,
    unbounded or partly counted total.
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
                inner = os.open(names[-1], os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
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


class Monitor:
    def __init__(self, ctx, ctl_w, cmd_r, out_r, err_r):
        self.ctx, self.ctl_w, self.cmd_r = ctx, ctl_w, cmd_r
        self.lock = threading.Lock()
        self.seq = 0
        self.leader = None  # [pid, start] once bound
        self.stop = threading.Event()
        self.census_stop = threading.Event()
        self.inject = set(ctx["inject"])
        self.pipes = [(out_r, "stdout", 1), (err_r, "stderr", 2)]
        self.receipt = None

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
        else:
            root = os.path.realpath(tempfile.mkdtemp(prefix="oxide-proof.", dir=ctx["root_parent"]))
            scratch, owned, reserve_path = os.path.join(root, "scratch"), True, os.path.realpath(ctx["root_parent"])
            os.mkdir(scratch)
            evidence = ctx["evidence_arg"] or os.path.join(root, "evidence")
        ctx.update(root=root, scratch=scratch, evidence=os.path.realpath(evidence), owned=owned,
                   reserve_path=reserve_path)
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
            "phase": None, "leader": None, "custodian": [], "outcome": None,
            "reserve": {"samples": 0, "minimum": None, "max_gap_ns": 0, "first_ns": None, "last_ns": None},
            "owned_bytes": {"samples": 0, "maximum": None, "budget": ctx["budget"], "last_ns": None},
            "output": {}, "census": None, "census_rounds": 0,
        }

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
        census = Census(self.leader[0], terminal_allowed=(
            self.ctx.get("terminal_mode", {}).get("mode") == "allowlisted"))
        while not self.census_stop.is_set():
            result = census.round()
            if "census-uncertain" in self.inject or (
                    "census-uncertain-first" in self.inject and not self.receipt["census_rounds"]):
                result = dict(result, certain=False, uncertain=max(1, result["uncertain"]))
            with self.lock:
                self.receipt["census"] = result
                self.receipt["census_rounds"] += 1
            self.send("census", after=after, members=result["members"], uncertain=result["uncertain"],
                      certain=result["certain"])
            self.census_stop.wait(CENSUS_INTERVAL)

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
        for target in (self.sample_reserve, self.sample_owned):
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
                row = Kinfo.one(command["pid"])
                with self.lock:
                    self.leader = [command["pid"], None if row is None else row["start"]]
                    self.receipt["leader"] = self.leader
                if "malformed-message" in self.inject:  # a census with every field missing
                    threading.Timer(INJECT_AFTER_GATE_NS / NS, lambda: self.send("census")).start()
            elif kind == "record":
                with self.lock:
                    self.receipt["custodian"].append(command["record"])
            elif kind == "stopped":
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
        self.census_stop.set()
        if decision == "cleanup":
            self.publish("closure_established: cleanup_pending")
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
        self.publish("monitor_complete: pending_custodian")
        self.summary()
        self.send("monitor_complete")
        self.stop.set()

    def summary(self):
        receipt, reserve = self.receipt, self.receipt["reserve"]
        outcome = receipt.get("outcome") or {}
        minimum = "none" if reserve["minimum"] is None else f"{reserve['minimum'] / GIB:.1f} GiB"
        peak = receipt["owned_bytes"]["maximum"] or 0
        lines = [
            f"supervisor: {outcome.get('closure', 'UNCLOSED')}; command status {outcome.get('status')};"
            f" fault {outcome.get('fault')}; deadline {outcome.get('deadline')}",
            f"supervisor: host free sampled {reserve['samples']} times every {RESERVE_INTERVAL} s (largest gap"
            f" {reserve['max_gap_ns'] / NS:.3f} s), minimum {minimum}, abort threshold 22 GiB; sampled, not an"
            " instantaneous guarantee",
            f"supervisor: owned bytes peak {peak / MIB:.1f} MiB of {receipt['owned_bytes']['budget'] // MIB} MiB"
            f" monitored budget; evidence {receipt['evidence']}",
        ]
        try:
            os.write(2, ("\n".join(lines) + "\n").encode())
        except OSError:
            pass


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
    except BaseException:
        pass
    finally:
        os._exit(code)


# Custodian -----------------------------------------------------------------------


class Custodian:
    def __init__(self, command, limit_s=LIMIT_S, budget=SCRATCH_BUDGET, evidence=None, root_parent="/private/tmp"):
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
        self.reaped_seq = None
        self.monitor_complete = False
        self.deadline_hit = False
        self.gate_opened = False
        self.gate_at = None
        self.pre_gate_killed = self.leader_killed = False
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
        self.guard.check(now_ns())

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
        elif kind == "census":
            if self.reaped_seq is not None and message["after"] >= self.reaped_seq:
                if not message["certain"] or message["uncertain"]:
                    # Latched: a later certain empty census never clears it.
                    self.census_uncertain = True
                    self.guard.latch(f"census uncertain: {message['uncertain']} unvalidated rows")
                self.census_ok = (not self.census_uncertain and message["certain"]
                                  and message["members"] == 0 and message["uncertain"] == 0)
        elif kind == "monitor_complete":
            self.monitor_complete = True
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
        """Setup slow or failed: launch nothing, stop the pinned monitor now."""
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
                self.deadline_hit = True
                break
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

    def settle(self, launched):
        self.tell("stopped")
        self.reaped_seq = self.seq
        self.record("leader_reaped", status=self.leader_status)
        if launched:
            census_end = min(now_ns() + CENSUS_GRACE_S * NS, self.exit_at)
            while not (self.census_uncertain or (self.census_ok and len(self.eof) == 2)) and now_ns() < census_end:
                self.pump(census_end - now_ns())
            if not self.census_ok:
                self.guard.latch("closure not established: no certain empty census after the leader was reaped")
            elif len(self.eof) != 2:
                self.guard.latch("closure not established: output EOF witness missing")

    def last_resort(self):
        """finish raised: KILL the pinned monitor if still held, one WNOHANG
        reap, and exit non-zero. Never raises."""
        try:
            if self.monitor is not None and not self.monitor_reaped:
                self.signal_pinned(self.monitor, signal.SIGKILL, group=False)
                self.monitor_reaped = self.reap(self.monitor) is not None
        except BaseException:
            pass
        return 124 if self.deadline_hit else 125

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
        if "custodian-timeout-after-monitor" in self.inject:
            self.exit_at = now_ns()  # injected: the custodian runs out right after deciding
        while not self.monitor_complete and not self.monitor_exited and now_ns() < self.exit_at:
            self.pump(self.exit_at - now_ns())
        if not self.monitor_complete:
            self.deadline_hit = self.deadline_hit or now_ns() >= self.exit_at
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
        if not launched:
            return 124 if self.deadline_hit else 125
        return exit_code(self.deadline_hit, self.guard.fault, self.closed, self.leader_status)


class BrokenQueue:
    """Test-only stand-in for a kqueue whose every call fails."""

    def control(self, *_args):
        raise OSError(errno.EBADF, "injected kqueue failure")


def parse(argv):
    options = {"limit": LIMIT_S, "budget": SCRATCH_BUDGET, "evidence": None, "root_parent": "/private/tmp"}
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
