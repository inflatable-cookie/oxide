#!/usr/bin/env python3
"""Observation-only diagnostic for the post-stop census stall (oxide#306).

One fixed diagnostic and THREE fixed Effigy selectors:

- `proof:census-stall-observation:test` runs `--self-test`: the pure apparatus
  check. It stages nothing, starts no supervisor, queries no kernel process or
  thread state and runs no engine, Cargo, image or fixture; it writes only one
  private temporary directory under `TMPDIR` and removes it before returning.
- `proof:census-stall-observation:plan` runs `--plan`: effect-free. It prints
  the fixed command, caps, custody rules, outcome classes, premise-checklist
  dispositions and the source digests it binds (source/tool metadata reads only).
- `rust:census-stall-observation` runs `--observe`: the ONE future occurrence.
  It is source-declared only and NOT activated by this task; the owner activates
  one fresh source-immutable occurrence separately. It always records
  `qualified: false` and awards no qualification, consumer or supervisor credit.

What `--observe` does, and does not do
--------------------------------------
It is an OUTSIDE-guard observer: it spawns the unchanged deployed default
supervisor (`scripts/bounded-test.py`, SHA-256
`fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b`) as its own
direct child with the normal default selected-volume scratch routing, and the
supervisor's leader is this same script in `--leader` mode. That synthetic
leader reads the guard's own exported layout, publishes one bounded identity
document (its own run id, PID/start, parent/custodian PID/start, session, group,
evidence/scratch/deadline and its DECLARED expected exit status 1) into a
directory the observer owns, then exits 1 by design. The declared exit is
labelled: it is never a relabelling of either historical producer failure.

The observer's whole authority is read-only observation of its OWN new run:

- the guard/custodian identity is its own direct child (PID plus the same
  `kinfo_proc` start token the guard's census uses), verified before and after
  every sample;
- the monitor is the guard's other direct child (the leader is forked after the
  monitor and calls `setsid`, so the remaining same-session child is the
  monitor). Exactly one remaining child with a matching identity is positive;
  zero, two or an unreadable row is recorded Unknown, never guessed;
- the census thread is the ONE monitor thread that appears after the guard's
  recorded `stopped` receive. Source establishes that the monitor creates no
  other thread after `ready`, so exactly one new thread id is positive; zero or
  more than one is Unknown;
- per sample it records CLOCK_UPTIME_RAW, the monitor thread ids/CPU/run state
  (bounded), the system process-table SIZE as a count only (libproc count, no
  foreign identities or rows persisted) and the load average. Sampling stops at
  the guard's exit, at 100 sample instants at ~4 Hz, or at one absolute
  deadline, whichever comes first;
- it reads the guard's OWN new `evidence/receipt.json` after exit and copies a
  bounded digest and the decision-relevant fields; the guard's own closure,
  signal, deadline and cleanup authority is untouched.

It never signals, reaps, suspends or traces the guard's monitor/leader, never
uses ptrace/DTrace/sample, never injects a signal, changes no host setting or
limit, touches no historical retained root/image/process, and never rebinds or
repins the supervisor or its candidate. Its only signal is a last-resort
SIGKILL of its own direct child (the guard) if the guard does not exit by the
observer's absolute deadline; the child is held unreaped, so its PID cannot
have been recycled, and it is positively reaped afterwards. An unreaped guard or
an unobserved monitor is recorded as unknown custody, keeps every resource and
fails the observation; the observer never claims the guard's closure.

Four outcome classes are recorded separately: `reproduced_discriminated`
(census-stall fingerprint plus attributed census-thread evidence that names the
observed shape), `reproduced_insufficient` (fingerprint but no honest thread
evidence), `not_reproduced` (the census completed and the guard closed with the
declared status in this one window) and `observation_incomplete` (apparatus,
identity, custody, receipt or guard-outcome facts are missing or contradictory).
A blocked kernel call cannot be named from thread state alone: the strongest
honest shape is "the attributed census thread made no measurable CPU progress
and was observed parked for N s", with the exact call recorded as Unknown. A
non-reproduction only bounds current behaviour; neither outcome is a historical
cause, a fix, a qualification or permission for any consumer native.

Usage (under the guard only, in `--leader` mode; the observer is the selector):
    python3 scripts/census-stall-observation.py --observe
    python3 scripts/census-stall-observation.py --self-test
    python3 scripts/census-stall-observation.py --plan
"""

from __future__ import annotations

import contextlib
import ctypes
import errno
import io
import hashlib
import importlib.util
import json
import os
import re
import secrets
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time

sys.dont_write_bytecode = True

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPT_PATH = os.path.abspath(__file__)
RUNNER_PATH = os.path.join(REPO, "scripts", "bounded-test.py")
CANDIDATE_PATH = os.path.join(REPO, "scripts", "bounded-test-candidate.py")
WORKFLOW_PATH = os.path.join(REPO, "docs", "knowledge", "contracts", "workflow.md")

SCHEMA = "oxide.census-stall-observation/1"
LEADER_SCHEMA = "oxide.census-stall-observation.leader/1"
CLAIM = ("observation only: one bounded outside-guard sample of this run's own "
         "supervisor monitor during the post-stop census window; it consumes the "
         "unchanged guard's own verdict, adds no signal, deadline, closure, cleanup "
         "or qualification authority, and is never a diagnosis, a fix or consumer "
         "approval")

# The deployed default supervisor bytes (oxide#195 custody hardening over
# oxide#197 ancestor/launcher-sibling exclusions, oxide#192 admission reports
# and oxide#191 opt-in post-census detach). Both bytes are refused on drift;
# the candidate is declared read-only and never executed, imported or repinned.
EXPECTED_RUNNER_SHA256 = "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b"
EXPECTED_CANDIDATE_SHA256 = "c578243126a2470cde0dfd19530254b3ec61b4ea5bb1fa43c6bbd2d41853c2b7"
SELECTED_VOLUME_UUID = "8a660440-c919-45c6-a3f1-3365b9bd2006"

# Fixed bounds. The guard runs its own unchanged 300 s / 512 MiB contract; these
# are the observer's own, so no fresh allowance exists per sample or retry.
LIMIT_S = 300
SCRATCH_BUDGET_MIB = 512
SAMPLE_INTERVAL_NS = 250_000_000
MAX_SAMPLES = 100
MAX_MONITOR_THREADS = 64
STORED_THREAD_ROWS = 16
REPORT_MAX_BYTES = 256 * 1024
LOG_MAX_BYTES = 64 * 1024
OBSERVER_TOTAL_MAX_BYTES = 1024 * 1024
LEADER_DOC_MAX_BYTES = 8 * 1024
RECEIPT_READ_MAX_BYTES = 2 * 1024 * 1024
LEADER_HOLD_S = 0.7
DOC_POLL_NS = 25_000_000
DOC_WAIT_NS = 15_000_000_000
EXIT_DRAIN_NS = 2_000_000_000
MONITOR_GONE_WAIT_NS = 5_000_000_000
LATE_WINDOW_START_NS = 1_000_000_000
MIN_LATE_WINDOW_NS = 1_000_000_000
SLOW_CPU_NS = 10_000_000
LEADER_EXPECTED_STATUS = 1

DOC_NAME = "leader-identity.json"
REPORT_NAME = "observation.json"
STDOUT_NAME = "guard.stdout"
STDERR_NAME = "guard.stderr"
PUBLISH_DIR = "publish"
PUBLISH_DIR_VAR = "OXIDE_CENSUS_OBSERVATION_PUBLISH_DIR"
TOKEN_VAR = "OXIDE_CENSUS_OBSERVATION_TOKEN"
SAMPLE_CLOCK = "CLOCK_UPTIME_RAW"

NS = 1_000_000_000
MIB = 1024 * 1024
_TH_STATES = {1: "running", 2: "stopped", 3: "waiting", 4: "uninterruptible",
              5: "halted"}
_PARKED_STATES = frozenset({2, 3, 4, 5})
_HEX40 = re.compile(r"\A[0-9a-f]{40}\Z")
_HEX32 = re.compile(r"\A[0-9a-f]{32}\Z")
_HEX16 = re.compile(r"\A[0-9a-f]{16}\Z")

OUTCOME_REPRODUCED_DISCRIMINATED = "reproduced_discriminated"
OUTCOME_REPRODUCED_INSUFFICIENT = "reproduced_insufficient"
OUTCOME_NOT_REPRODUCED = "not_reproduced"
OUTCOME_INCOMPLETE = "observation_incomplete"
OUTCOMES = (OUTCOME_REPRODUCED_DISCRIMINATED, OUTCOME_REPRODUCED_INSUFFICIENT,
            OUTCOME_NOT_REPRODUCED, OUTCOME_INCOMPLETE)

SHAPE_BLOCKED = "blocked_call_shape"
SHAPE_SLOW = "slow_round_shape"
SHAPE_INDETERMINATE = "indeterminate_shape"
SHAPES = (SHAPE_BLOCKED, SHAPE_SLOW, SHAPE_INDETERMINATE)

# The fixed nine-item proof-authoring premise checklist dispositions. This is
# Python-only, with no Cargo, toolchain, image, mount, pool, account-home or
# product child, so most premises do not apply; the three that do are bound to
# the source that implements them.
PREMISE_CHECKLIST = (
    ("private-home-rustup",
     "n/a: no Cargo, rustc, toolchain staging or `--locked` call exists in this "
     "diagnostic; the selector runs no product child at all"),
    ("account-home-control",
     "n/a: no Oxide product process, pool, account-home read or fixture runs"),
    ("environment-isolation",
     "applies: `guard_environment` removes `OXIDE_SUPERVISOR_INJECT` and "
     "`OXIDE_TASK_SCRATCH_ROOT` (recorded as `environment_removed`) so the guard's "
     "own default selected-volume routing and uninjected behaviour are used; the "
     "leader is handed only the observer's publish directory and one fresh token"),
    ("automatic-adoption",
     "n/a: no checkout, output link or permanent-drive mode is involved"),
    ("cargo-external-argv",
     "n/a: no Cargo external subcommand runs"),
    ("owned-images",
     "n/a: the diagnostic creates no image, mount or attachment; it publishes no "
     "detach request and no image-custody indication"),
    ("compiler-flags",
     "n/a: nothing is compiled and no Cargo environment is consumed"),
    ("temporary-storage",
     "applies: the observer root is one private 0700 directory under the run's "
     "`TMPDIR`, and the guard's own root is its unchanged default selected-volume "
     "`<mount>/scratch/oxide` leaf; `preflight` refuses a non-canonical, "
     "group/other-writable, boot-volume or non-Stash-UUID route, and the post-run "
     "gate re-derives the guard's recorded source, UUID and device"),
    ("mktemp",
     "applies: the observer creates exactly one root with `tempfile.mkdtemp` "
     "inside the explicit run `TMPDIR` (never a bare `mktemp` and never a fixed "
     "internal path), and the synthetic leader writes only inside the observer's "
     "own publish directory"),
)


# ---------------------------------------------------------------------------
# Bounded helpers
# ---------------------------------------------------------------------------


def now_ns():
    return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)


def type_name(error):
    return type(error).__name__


def errno_name(error):
    number = getattr(error, "errno", None)
    if isinstance(number, int) and number in errno.errorcode:
        return errno.errorcode[number]
    return type_name(error)


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def bounded_text(value, cap=200):
    text = str(value)
    return text[:cap]


def compact_json(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def strict_int(value):
    return type(value) is int


def hex40(value):
    return isinstance(value, str) and bool(_HEX40.match(value))


def hex32(value):
    return isinstance(value, str) and bool(_HEX32.match(value))


def abs_canonical(path):
    return (isinstance(path, str) and os.path.isabs(path)
            and os.path.normpath(path) == path)


def read_bounded(path, cap):
    """Read at most `cap` bytes and report the whole file's size."""
    handle = open(path, "rb")
    try:
        size = os.fstat(handle.fileno()).st_size
        data = handle.read(cap)
    finally:
        handle.close()
    return data, size


def write_atomic(path, data, mode=0o600):
    """One atomic publication in the target's own directory."""
    directory = os.path.dirname(path)
    temporary = os.path.join(directory, "." + os.path.basename(path) + "." + secrets.token_hex(4))
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    except BaseException:
        try:
            os.unlink(temporary)
        except OSError:
            pass
        raise


def read_json(path, cap):
    data, size = read_bounded(path, cap)
    if size > cap:
        raise ValueError("document is larger than its bound")
    return json.loads(data.decode("utf-8")), size, sha256_bytes(data)


def monotonic_list(values):
    return all(values[index] <= values[index + 1] for index in range(len(values) - 1))


# ---------------------------------------------------------------------------
# Real seams: one boundary for every kernel/process/filesystem observation, so
# the self-test can drive the real entry and orchestrator with fakes.
# ---------------------------------------------------------------------------


class RealSeams:
    """macOS/libproc and the qualified runner's own identity primitives.

    `libproc` reads are read-only, same-user observations of this run's own
    child processes. No ptrace, DTrace, sample, suspension or host setting.
    """

    PROC_PIDLISTTHREADS = 6
    PROC_PIDTHREADINFO = 5

    class _ThreadInfo(ctypes.Structure):
        _fields_ = [
            ("pth_user_time", ctypes.c_uint64), ("pth_system_time", ctypes.c_uint64),
            ("pth_cpu_usage", ctypes.c_int32), ("pth_policy", ctypes.c_int32),
            ("pth_run_state", ctypes.c_int32), ("pth_flags", ctypes.c_int32),
            ("pth_sleep_time", ctypes.c_int32), ("pth_curpri", ctypes.c_int32),
            ("pth_priority", ctypes.c_int32), ("pth_maxpriority", ctypes.c_int32),
            ("pth_name", ctypes.c_char * 64),
        ]

    def __init__(self):
        self.uid = os.getuid()
        self._runner = None
        self._libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        self._libproc.proc_pidinfo.restype = ctypes.c_int
        self._libproc.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int,
                                               ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
        self._libproc.proc_listchildpids.restype = ctypes.c_int
        self._libproc.proc_listchildpids.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_int]
        self._libproc.proc_listallpids.restype = ctypes.c_int
        self._libproc.proc_listallpids.argtypes = [ctypes.c_void_p, ctypes.c_int]

    # -- source and tool metadata ------------------------------------------
    def runner(self):
        if self._runner is None:
            spec = importlib.util.spec_from_file_location("bounded_test_runner", RUNNER_PATH)
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            self._runner = module
        return self._runner

    def now_ns(self):
        return now_ns()

    def sleep(self, seconds):
        time.sleep(max(0.0, seconds))

    def sha256_file(self, path):
        handle = open(path, "rb")
        try:
            digest = hashlib.sha256()
            while True:
                chunk = handle.read(1024 * 1024)
                if not chunk:
                    return digest.hexdigest()
                digest.update(chunk)
        finally:
            handle.close()

    def git(self, *arguments):
        try:
            result = subprocess.run(["git", *arguments], cwd=REPO, stdin=subprocess.DEVNULL,
                                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                    timeout=30, check=False)
        except (OSError, subprocess.SubprocessError) as error:
            return None, type_name(error)
        return result.returncode, result.stdout.decode("utf-8", "replace").strip()

    def stat(self, path):
        return os.stat(path)

    def lstat(self, path):
        return os.lstat(path)

    def realpath(self, path):
        return os.path.realpath(path)

    def volume_uuid(self, path):
        return self.runner().volume_uuid(path)

    def layout_check(self):
        self.runner().Kinfo.check()

    def process_row(self, pid):
        """(row or None, problem or None). Absent is a positive `None` row."""
        try:
            return self.runner().Kinfo.one(pid), None
        except OSError as error:
            return None, errno_name(error)

    def monitor_children(self, pid):
        count = self._libproc.proc_listchildpids(pid, None, 0)
        if count < 0:
            return None
        if count == 0:
            return []
        buffer = ctypes.create_string_buffer(count * 4 + 64)
        filled = self._libproc.proc_listchildpids(pid, buffer, len(buffer))
        if filled < 0:
            return None
        if filled == 0:
            return []
        return list(struct_unpack_int(filled, buffer.raw))

    def thread_ids(self, pid):
        buffer = ctypes.create_string_buffer(4096)
        filled = self._libproc.proc_pidinfo(pid, self.PROC_PIDLISTTHREADS, 0, buffer, len(buffer))
        if filled < 0:
            return None
        if filled == 0:
            return []
        count = filled // 8
        if count > MAX_MONITOR_THREADS * 8:
            return None
        return list(struct_unpack_u64(count, buffer.raw))

    def thread_info(self, pid, thread):
        info = self._ThreadInfo()
        filled = self._libproc.proc_pidinfo(pid, self.PROC_PIDTHREADINFO,
                                            ctypes.c_uint64(thread), ctypes.byref(info),
                                            ctypes.sizeof(info))
        if filled != ctypes.sizeof(info):
            return None
        name = info.pth_name.decode("utf-8", "replace").split("\x00", 1)[0]
        return {"cpu_ns": (int(info.pth_user_time) + int(info.pth_system_time)) * 1000,
                "state": int(info.pth_run_state), "name": name[:32]}

    def table_count(self):
        count = self._libproc.proc_listallpids(None, 0)
        return count if count >= 0 else None

    def load_average(self):
        try:
            return [float(value) for value in os.getloadavg()]
        except OSError:
            return None

    # -- one direct child ---------------------------------------------------
    def spawn(self, argv, env, cwd):
        process = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   close_fds=True)
        capture = {}
        threads = []
        for name, stream in (("stdout", process.stdout), ("stderr", process.stderr)):
            record = {"bytes": 0, "kept": bytearray(), "truncated": False}
            capture[name] = record

            def drain(stream=stream, record=record):
                try:
                    while True:
                        chunk = stream.read(65536)
                        if not chunk:
                            break
                        record["bytes"] += len(chunk)
                        record["kept"].extend(chunk)
                        if len(record["kept"]) > LOG_MAX_BYTES:
                            del record["kept"][: len(record["kept"]) - LOG_MAX_BYTES]
                            record["truncated"] = True
                except (OSError, ValueError):
                    pass
                finally:
                    try:
                        stream.close()
                    except OSError:
                        pass

            thread = threading.Thread(target=drain, daemon=True)
            thread.start()
            threads.append(thread)
        return {"process": process, "capture": capture, "drains": threads}

    def poll(self, handle):
        return handle["process"].poll()

    def wait(self, handle, timeout_s):
        try:
            return handle["process"].wait(timeout=max(0.0, timeout_s))
        except subprocess.TimeoutExpired:
            return None

    def kill(self, handle):
        """SIGKILL a still-unreaped direct child; its PID cannot be recycled."""
        process = handle["process"]
        if process.poll() is not None:
            return False
        try:
            os.kill(process.pid, signal.SIGKILL)
            return True
        except ProcessLookupError:
            return False

    def join_drains(self, handle, timeout_s=1.0):
        for thread in handle.get("drains") or []:
            thread.join(max(0.0, timeout_s))

    def guard_output(self, handle):
        return {name: {"bytes": record["bytes"],
                       "text": bytes(record["kept"]).decode("utf-8", "replace"),
                       "truncated": record["truncated"]}
                for name, record in (handle.get("capture") or {}).items()}

    # -- this process's own identity ---------------------------------------
    def self_pid(self):
        return os.getpid()

    def self_ppid(self):
        return os.getppid()

    def self_pgid(self):
        return os.getpgid(0)

    def self_sid(self):
        return os.getsid(0)

    # -- filesystem ---------------------------------------------------------
    def exists(self, path):
        return os.path.exists(path)

    def publish_new(self, path, data):
        """Publish one document exclusively, so a stale document is never reused."""
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        return len(data)

    def mkdtemp(self, parent, prefix):
        return tempfile.mkdtemp(prefix=prefix, dir=parent)

    def mkdir(self, path, mode=0o700):
        os.mkdir(path, mode)

    def write_document(self, path, document):
        data = (json.dumps(document, indent=1, sort_keys=True) + "\n").encode("utf-8")
        write_atomic(path, data)
        return len(data)

    def read_document(self, path, cap):
        return read_json(path, cap)

    def device(self, path):
        return os.stat(path).st_dev


def struct_unpack_int(count, raw):
    import struct
    return struct.unpack_from("<%di" % count, raw, 0)


def struct_unpack_u64(count, raw):
    import struct
    return struct.unpack_from("<%dQ" % count, raw, 0)


# ---------------------------------------------------------------------------
# Source and topology gates
# ---------------------------------------------------------------------------


def source_gate(seams):
    """Bind the exact reviewed source inputs. Refusal precedes every effect."""
    problems, facts = [], {}
    code, status = seams.git("status", "--porcelain", "--untracked-files=no")
    facts["worktree_status_exit"] = code
    if code != 0:
        problems.append(f"git status failed ({status})")
    elif status:
        problems.append("the tracked worktree is not clean, so this would not be HEAD")
    code, head = seams.git("rev-parse", "HEAD")
    facts["head"] = head if hex40(head) else None
    if code != 0 or not hex40(head):
        problems.append("HEAD is not an exact 40-hex commit identity")
    code, tree = seams.git("rev-parse", "HEAD^{tree}")
    facts["tree"] = tree if hex40(tree) else None
    if code != 0 or not hex40(tree):
        problems.append("the HEAD tree is not an exact 40-hex identity")
    code, branch = seams.git("branch", "--show-current")
    facts["branch"] = bounded_text(branch, 120) if code == 0 else None
    if code != 0:
        problems.append("the current branch could not be read")
    for key, path, expected, label in (
            ("runner_sha256", RUNNER_PATH, EXPECTED_RUNNER_SHA256, "supervisor"),
            ("candidate_sha256", CANDIDATE_PATH, EXPECTED_CANDIDATE_SHA256, "candidate")):
        try:
            digest = seams.sha256_file(path)
        except OSError as error:
            problems.append(f"{label} bytes are unreadable ({errno_name(error)})")
            facts[key] = None
            continue
        facts[key] = digest
        if digest != expected:
            problems.append(f"{label} bytes {digest} are not the bound {expected}")
    try:
        facts["diagnostic_sha256"] = seams.sha256_file(SCRIPT_PATH)
    except OSError as error:
        problems.append(f"the diagnostic's own bytes are unreadable ({errno_name(error)})")
        facts["diagnostic_sha256"] = None
    try:
        facts["workflow_sha256"] = seams.sha256_file(WORKFLOW_PATH)
    except OSError as error:
        problems.append(f"the owning workflow document is unreadable ({errno_name(error)})")
        facts["workflow_sha256"] = None
    facts["candidate_used"] = False
    return facts, problems


def mount_root(seams, path):
    """The highest ancestor of `path` on its own device (effect-free)."""
    current = seams.realpath(path)
    while True:
        parent = os.path.dirname(current)
        if parent == current or seams.device(parent) != seams.device(current):
            return current
        current = parent


def preflight(seams, env):
    """The observer's own route and custody preconditions, before any effect."""
    problems, facts = [], {}
    raw = env.get("TMPDIR")
    if not raw:
        return {"source": "refused", "problems": ["TMPDIR is not set"]}, ["TMPDIR is not set"]
    tmpdir = os.path.abspath(raw)
    facts["tmpdir"] = tmpdir
    if not abs_canonical(tmpdir):
        problems.append("TMPDIR is not an absolute canonical path")
    try:
        info = seams.lstat(tmpdir)
        if stat.S_ISLNK(info.st_mode):
            problems.append("TMPDIR is a symlink")
        if not stat.S_ISDIR(info.st_mode):
            problems.append("TMPDIR is not a directory")
        if info.st_uid != seams.uid:
            problems.append("TMPDIR is not owned by this account")
        if info.st_mode & 0o022:
            problems.append("TMPDIR is group- or other-writable")
        if seams.realpath(tmpdir) != tmpdir:
            problems.append("TMPDIR is not canonical")
        boot = seams.device("/")
        device = seams.device(tmpdir)
        facts["device"] = device
        facts["boot_device"] = boot
        if device == boot:
            problems.append("TMPDIR is on the boot volume")
        mount = mount_root(seams, tmpdir)
        facts["mount"] = mount
        if mount == "/" or mnt_boot(seams, mount):
            problems.append("the TMPDIR mount is the boot volume")
        try:
            uuid = seams.volume_uuid(mount)
        except Exception as error:
            uuid = None
            facts["volume_uuid_problem"] = errno_name(error)
        facts["volume_uuid"] = (uuid or "").lower() or None
        if facts["volume_uuid"] != SELECTED_VOLUME_UUID:
            problems.append(f"the TMPDIR volume UUID is {facts['volume_uuid']}, not the "
                            f"selected {SELECTED_VOLUME_UUID}")
        scratch = os.path.join(mount, "scratch")
        facts["scratch_parent"] = scratch
        try:
            parent_info = seams.stat(scratch)
            if not stat.S_ISDIR(parent_info.st_mode):
                problems.append("<mount>/scratch is not a directory")
            if parent_info.st_mode & 0o022:
                problems.append("<mount>/scratch is group- or other-writable")
        except OSError as error:
            problems.append(f"<mount>/scratch is unavailable ({errno_name(error)})")
    except OSError as error:
        problems.append(f"TMPDIR facts are unreadable ({errno_name(error)})")
    facts["source"] = "selected-volume" if not problems else "refused"
    return facts, problems


def mnt_boot(seams, mount):
    try:
        return (seams.realpath(mount) == "/"
                or seams.device(mount) == seams.device("/"))
    except OSError:
        return True


def guard_environment(env, publish_dir, token):
    """The guard's environment: its own route and no inherited test control."""
    child = dict(env)
    removed = []
    for name in ("OXIDE_SUPERVISOR_INJECT", "OXIDE_TASK_SCRATCH_ROOT"):
        if name in child:
            child.pop(name)
            removed.append(name)
    child[PUBLISH_DIR_VAR] = publish_dir
    child[TOKEN_VAR] = token
    return child, removed


def guard_argv():
    return [sys.executable, RUNNER_PATH, "--limit", str(LIMIT_S),
            "--scratch-budget-mib", str(SCRATCH_BUDGET_MIB), "--",
            sys.executable, SCRIPT_PATH, "--leader"]


# ---------------------------------------------------------------------------
# Identity binding
# ---------------------------------------------------------------------------


def row_identity(row):
    if not isinstance(row, dict):
        return None
    if not strict_int(row.get("pid")) or not strict_int(row.get("start")):
        return None
    return (row["pid"], row["start"])


def identity_facts(seams, pid, start):
    """(identity_ok, live, zombie, problem). A changed start token is never the
    same process; unreadable facts are never a positive identity."""
    row, problem = seams.process_row(pid)
    if problem is not None:
        return False, False, None, problem
    if row is None:
        return False, False, None, None
    identity = row_identity(row)
    if identity != (pid, start):
        return False, False, None, "changed-identity"
    return True, not row.get("zombie", False), bool(row.get("zombie")), None


def validate_leader_document(document, token, guard_pid, guard_start):
    """Every field this observer relies on, bounded and positively typed."""
    problems = []
    if not isinstance(document, dict):
        return ["the leader document is not an object"]
    if document.get("schema") != LEADER_SCHEMA:
        problems.append("the leader document schema is not the declared one")
    if not isinstance(token, str) or not _HEX16.match(token) or document.get("token") != token:
        problems.append("the leader document is not this observer's fresh run")
    leader = document.get("leader")
    if not isinstance(leader, dict):
        return problems + ["the leader identity is not an object"]
    if not strict_int(leader.get("pid")) or leader.get("pid", 0) <= 0:
        problems.append("the leader PID is ill-typed")
    if not strict_int(leader.get("start")) or leader.get("start", 0) < 0:
        problems.append("the leader start token is ill-typed")
    if leader.get("expected_status") != LEADER_EXPECTED_STATUS:
        problems.append("the leader declared status is not the fixed 1")
    if leader.get("ppid") != guard_pid:
        problems.append("the leader's parent is not this observer's guard child")
    if leader.get("ppid_start") != guard_start:
        problems.append("the leader's recorded parent start is not the guard's own")
    if leader.get("sid") != leader.get("pid"):
        problems.append("the leader is not its own session leader")
    guard = document.get("guard")
    if not isinstance(guard, dict):
        problems.append("the guard layout is not an object")
    else:
        for name in ("evidence", "scratch", "tmpdir"):
            value = guard.get(name)
            if not abs_canonical(value):
                problems.append(f"the guard {name} is not an absolute canonical path")
        deadline = guard.get("deadline_ns")
        if not strict_int(deadline) or deadline <= 0:
            problems.append("the guard deadline is ill-typed")
        if not isinstance(guard.get("run_id"), str) or not _HEX32.match(guard.get("run_id") or ""):
            problems.append("the guard run id is ill-typed")
    return problems


def resolve_monitor(seams, guard_pid, guard_start, leader_pid, leader_start):
    """The monitor is the guard's other direct child. One positive remaining
    child is an attribution; zero, several or unreadable is Unknown."""
    facts = {"attribution": "unknown", "pid": None, "start": None, "reason": None}
    children = seams.monitor_children(guard_pid)
    if children is None:
        facts["reason"] = "the guard's child list is unreadable"
        return facts
    rows = []
    for pid in children:
        row, problem = seams.process_row(pid)
        if problem is not None:
            facts["reason"] = f"child {pid} identity is unreadable ({problem})"
            return facts
        if row is None:
            continue
        rows.append(row)
    identities = [row_identity(row) for row in rows]
    remaining = [(row, identity) for row, identity in zip(rows, identities)
                 if identity != (leader_pid, leader_start)]
    if len(remaining) == 1:
        row, identity = remaining[0]
        if row.get("ppid") != guard_pid:
            facts["reason"] = "the remaining child's parent is not the guard"
            return facts
        if row.get("pgid") != _guard_pgid(seams, guard_pid):
            facts["reason"] = "the remaining child is not in the guard's process group"
            return facts
        facts.update(attribution="sole-remaining-child", pid=identity[0], start=identity[1])
        return facts
    if not remaining:
        facts["reason"] = "no child of the guard remains to attribute as the monitor"
        return facts
    facts["reason"] = f"{len(remaining)} remaining children; the monitor is ambiguous"
    return facts


def _guard_pgid(seams, guard_pid):
    row, problem = seams.process_row(guard_pid)
    if problem is not None or row is None:
        return None
    return row.get("pgid")


# ---------------------------------------------------------------------------
# Sampling
# ---------------------------------------------------------------------------


def sample_once(seams, guard, monitor, sample_index):
    """One bounded observation instant. Never raises and never persists a
    foreign identity: the table size is a count only."""
    at = seams.now_ns()
    sample = {"i": sample_index, "at_ns": at, "guard_ok": False, "monitor_ok": False,
              "table": None, "load": None, "threads": [], "threads_omitted": 0,
              "problem": None}
    try:
        sample["guard_ok"] = identity_facts(seams, guard["pid"], guard["start"])[0]
    except BaseException:
        sample["guard_ok"] = False
    if monitor.get("pid") is not None:
        try:
            ok, _live, _zombie, problem = identity_facts(seams, monitor["pid"], monitor["start"])
            sample["monitor_ok"] = bool(ok)
            if problem is not None:
                sample["problem"] = problem
        except BaseException:
            sample["monitor_ok"] = False
        if sample["monitor_ok"]:
            try:
                threads = seams.thread_ids(monitor["pid"])
            except BaseException:
                threads = None
            if threads is None:
                sample["problem"] = sample["problem"] or "thread-list-unreadable"
            else:
                if len(threads) > MAX_MONITOR_THREADS:
                    sample["problem"] = "thread-count-over-cap"
                    threads = []
                for thread in threads[:STORED_THREAD_ROWS]:
                    try:
                        info = seams.thread_info(monitor["pid"], thread)
                    except BaseException:
                        info = None
                    if info is None or not strict_int(info.get("cpu_ns")) \
                            or not strict_int(info.get("state")):
                        sample["problem"] = sample["problem"] or "thread-state-unreadable"
                        continue
                    sample["threads"].append([int(thread), int(info["cpu_ns"]),
                                              int(info["state"]),
                                              bounded_text(info.get("name") or "", 32)])
                sample["threads_omitted"] = max(0, len(threads) - STORED_THREAD_ROWS)
    try:
        sample["table"] = seams.table_count()
    except BaseException:
        sample["table"] = None
    try:
        sample["load"] = seams.load_average()
    except BaseException:
        sample["load"] = None
    return sample


def sample_window(seams, guard, monitor, deadline_ns, control, interval_ns=SAMPLE_INTERVAL_NS,
                  max_samples=MAX_SAMPLES):
    """The bounded sampling loop. It stops at the guard's own exit or the
    observer's absolute deadline, never extends either."""
    samples = []
    guard_exit = None
    cancelled = None
    while len(samples) < max_samples:
        if control.get("cancelled") is not None:
            cancelled = f"observer cancelled by signal {control['cancelled']}"
            break
        exit_code = seams.poll(guard["handle"])
        if exit_code is not None:
            guard_exit = exit_code
            break
        if seams.now_ns() >= deadline_ns:
            break
        samples.append(sample_once(seams, guard, monitor, len(samples)))
        seams.sleep(interval_ns / NS)
    return samples, guard_exit, cancelled


def attribute_census_thread(samples, settlement):
    """Exactly one monitor thread that appears after the guard's `stopped`
    receive is the census thread. Anything else is honestly Unknown.

    The baseline is the union of thread ids in the samples before the recorded
    stop, so a thread that existed before the census cannot be mistaken for it.
    Source establishes that the monitor creates no other thread after `ready`;
    that is the reason one new thread is positive."""
    result = {"state": "unknown", "tid": None, "reason": None, "first_ns": None,
              "baseline_threads": 0, "candidate_tids": []}
    if not isinstance(settlement, dict):
        result["reason"] = "no settlement object"
        return result
    stopped = settlement.get("stopped_received_ns")
    thread_start = settlement.get("thread_start_ns")
    if not strict_int(thread_start):
        result["reason"] = "the guard recorded no census thread start"
        return result
    if not strict_int(stopped):
        result["reason"] = "the guard recorded no processed stop stamp"
        return result
    threshold = stopped
    before = [sample for sample in samples
              if sample["at_ns"] < threshold and sample["monitor_ok"]]
    if not before:
        result["reason"] = "no pre-stop monitor thread baseline was observed"
        return result
    baseline_tids = {row[0] for sample in before for row in sample["threads"]}
    result["baseline_threads"] = len(baseline_tids)
    later = [sample for sample in samples
             if sample["at_ns"] >= threshold and sample["monitor_ok"]]
    if not later:
        result["reason"] = "no post-stop sample names the monitor"
        return result
    new_tids = sorted({row[0] for sample in later for row in sample["threads"]
                       if row[0] not in baseline_tids})
    result["candidate_tids"] = new_tids[:8]
    if not new_tids:
        result["reason"] = "no new monitor thread appeared after the stop"
        return result
    if len(new_tids) > 1:
        result["state"] = "ambiguous"
        result["reason"] = f"{len(new_tids)} new monitor threads appeared"
        return result
    tid = new_tids[0]
    first = next((sample["at_ns"] for sample in later
                  if any(row[0] == tid for row in sample["threads"])), None)
    if first is None:
        result["state"] = "ambiguous"
        result["reason"] = "the new thread appears in no sample"
        return result
    result.update(state="attributed", tid=tid, first_ns=first)
    return result


def census_thread_series(samples, attribution, settlement):
    """The attributed thread's bounded series, with the late measurement window
    starting after the documented in-round pause budget."""
    thread_start = settlement.get("thread_start_ns") if isinstance(settlement, dict) else None
    rows = []
    if attribution.get("state") == "attributed":
        for sample in samples:
            for tid, cpu, state, name in sample["threads"]:
                if tid == attribution["tid"]:
                    rows.append({"at_ns": sample["at_ns"], "cpu_ns": cpu, "state": state,
                                 "name": name})
    series = {"rows": len(rows), "window_ns": None}
    if len(rows) >= 2:
        series["window_ns"] = rows[-1]["at_ns"] - rows[0]["at_ns"]
        series["cpu_delta_ns"] = rows[-1]["cpu_ns"] - rows[0]["cpu_ns"]
        series["states"] = sorted({row["state"] for row in rows})
        series["monotonic_cpu"] = monotonic_list([row["cpu_ns"] for row in rows])
    if isinstance(thread_start, int):
        late = [row for row in rows
                if row["at_ns"] >= thread_start + LATE_WINDOW_START_NS]
        series["late_rows"] = len(late)
        if len(late) >= 2:
            series["late_window_ns"] = late[-1]["at_ns"] - late[0]["at_ns"]
            series["late_cpu_delta_ns"] = late[-1]["cpu_ns"] - late[0]["cpu_ns"]
            series["late_states"] = sorted({row["state"] for row in late})
            series["late_monotonic_cpu"] = monotonic_list([row["cpu_ns"] for row in late])
    return series


def observed_shape(series):
    """The strongest honest shape, or Unknown when the evidence cannot decide.

    A slow round keeps executing: CPU time grows. A round stuck inside one
    non-returning call makes no measurable CPU progress and the thread is
    parked. The exact call is NOT nameable from thread state alone."""
    late = series.get("late_rows") or 0
    window = series.get("late_window_ns")
    if late < 2 or not strict_int(window) or window < MIN_LATE_WINDOW_NS:
        return SHAPE_INDETERMINATE, "no late window of at least 1 s was sampled"
    if not series.get("late_monotonic_cpu", False):
        return SHAPE_INDETERMINATE, "the observed CPU time is not monotonic"
    delta = series.get("late_cpu_delta_ns")
    if not strict_int(delta):
        return SHAPE_INDETERMINATE, "the late CPU delta is missing"
    states = set(series.get("late_states") or [])
    if delta >= SLOW_CPU_NS:
        return SHAPE_SLOW, (f"the attributed census thread consumed {delta} ns of CPU "
                            f"across {window} ns; it was executing, not parked")
    if delta == 0 and states and states <= _PARKED_STATES:
        return SHAPE_BLOCKED, (f"the attributed census thread made no measurable CPU "
                               f"progress across {window} ns and was parked "
                               f"({', '.join(_TH_STATES.get(state, str(state)) for state in sorted(states))})")
    return SHAPE_INDETERMINATE, ("the attributed thread's CPU and run state do not "
                                 "separate a slow round from a parked call")


# ---------------------------------------------------------------------------
# Guard receipt validation and classification
# ---------------------------------------------------------------------------


def receipt_facts(receipt):
    """The decision-relevant fields, bounded. Never a rewrite of the receipt."""
    outcome = receipt.get("outcome") if isinstance(receipt.get("outcome"), dict) else {}
    settlement = receipt.get("settlement") if isinstance(receipt.get("settlement"), dict) else {}
    join = receipt.get("census_join") if isinstance(receipt.get("census_join"), dict) else {}
    deciding = receipt.get("deciding_census") if isinstance(receipt.get("deciding_census"), dict) else None
    marker = receipt.get("scratch_marker") if isinstance(receipt.get("scratch_marker"), dict) else {}
    selection = receipt.get("scratch_default") if isinstance(receipt.get("scratch_default"), dict) else {}
    reserve = receipt.get("reserve") if isinstance(receipt.get("reserve"), dict) else {}
    owned = receipt.get("owned_bytes") if isinstance(receipt.get("owned_bytes"), dict) else {}
    leaders = receipt.get("leader")
    return {
        "outcome": {"closure": outcome.get("closure"), "status": outcome.get("status"),
                    "fault": bounded_text(outcome.get("fault"), 200) if outcome.get("fault") else None,
                    "deadline": outcome.get("deadline"), "launched": outcome.get("launched")},
        "settlement": {
            "phase": settlement.get("phase"),
            "stopped_received_ns": settlement.get("stopped_received_ns"),
            "thread_start_ns": settlement.get("thread_start_ns"),
            "thread_start_latency_ns": settlement.get("thread_start_latency_ns"),
            "first_round_ns": settlement.get("first_round_ns"),
            "stop_requested_ns": settlement.get("stop_requested_ns"),
            "loop_exit_ns": settlement.get("loop_exit_ns"),
            "rounds_started": settlement.get("rounds_started"),
            "rounds_completed": settlement.get("rounds_completed"),
            "omitted": settlement.get("omitted"),
        },
        "join": {"joined": join.get("joined"), "started": join.get("started"),
                 "bound_s": join.get("bound_s"), "waited_ns": join.get("waited_ns"),
                 "frozen": join.get("frozen"), "refused": join.get("refused"),
                 "rows": join.get("rows")},
        "deciding": deciding,
        "census_rounds": receipt.get("census_rounds"),
        "leader": leaders if isinstance(leaders, list) and len(leaders) == 2 else None,
        "selection": {"source": selection.get("source"), "volume_uuid": selection.get("volume_uuid"),
                      "mount": selection.get("mount"), "registration": selection.get("registration"),
                      "root_parent": selection.get("root_parent")},
        "reserve_device": reserve.get("device"),
        "owned": {"budget": owned.get("budget"), "maximum": owned.get("maximum"),
                  "samples": owned.get("samples")},
        "marker": {"state": marker.get("state"), "run_id": marker.get("run_id"),
                   "reason": marker.get("reason")},
        "root": receipt.get("root"), "scratch": receipt.get("scratch"),
        "evidence": receipt.get("evidence"), "scratch_owned": receipt.get("scratch_owned"),
    }


CENSUS_FAULT_PREFIX = "closure not established: no certain empty census after the leader was reaped"


def classify(report, receipt, guard_exit, problems):
    """One of the four declared outcome classes, with its reasons. Every
    positive claim is bound to the guard's own receipt fields, and any
    apparatus problem makes the observation incomplete."""
    reasons = list(problems)
    apparatus = bool(problems)
    facts = receipt_facts(receipt) if isinstance(receipt, dict) else None
    stamp = {"stall_fingerprint": False, "normal_closure": False, "shape": None,
             "shape_reason": None}
    if facts is None:
        reasons.append("the guard receipt is missing or not an object")
        return OUTCOME_INCOMPLETE, stamp, reasons
    outcome, settlement, join = facts["outcome"], facts["settlement"], facts["join"]
    stamp["stall_fingerprint"] = bool(
        strict_int(settlement.get("rounds_started")) and settlement["rounds_started"] >= 1
        and settlement.get("rounds_completed") == 0
        and settlement.get("thread_start_ns") is not None
        and settlement.get("loop_exit_ns") is None
        and facts["census_rounds"] == 0
        and join.get("joined") is False
        and outcome.get("closure") == "UNCLOSED"
        and (outcome.get("fault") or "").startswith(CENSUS_FAULT_PREFIX))
    stamp["guard_exit"] = guard_exit
    if stamp["stall_fingerprint"]:
        if guard_exit != 125:
            reasons.append(f"a census-stall fingerprint with guard exit {guard_exit}, not 125")
            return OUTCOME_INCOMPLETE, stamp, reasons
        if outcome.get("deadline") is not False:
            reasons.append("the stalled run also reports a deadline, which is not the fingerprint")
            return OUTCOME_INCOMPLETE, stamp, reasons
        attribution = report["census_thread"]
        series = report["census_series"]
        if attribution.get("state") != "attributed":
            reasons.append(f"the census thread is {attribution.get('state')}: "
                           f"{attribution.get('reason')}")
            return (OUTCOME_INCOMPLETE if apparatus else OUTCOME_REPRODUCED_INSUFFICIENT), stamp, reasons
        shape, why = observed_shape(series)
        stamp["shape"], stamp["shape_reason"] = shape, why
        if shape in (SHAPE_BLOCKED, SHAPE_SLOW):
            reasons.append(why)
            return (OUTCOME_INCOMPLETE if apparatus else OUTCOME_REPRODUCED_DISCRIMINATED), stamp, reasons
        reasons.append(why)
        return (OUTCOME_INCOMPLETE if apparatus else OUTCOME_REPRODUCED_INSUFFICIENT), stamp, reasons
    deciding = facts.get("deciding") or {}
    stamp["normal_closure"] = bool(
        strict_int(settlement.get("rounds_completed")) and settlement["rounds_completed"] >= 1
        and settlement.get("loop_exit_ns") is not None
        and join.get("joined") is True
        and isinstance(deciding, dict)
        and deciding.get("certain") is True and deciding.get("members") == 0
        and deciding.get("uncertain") == 0
        and facts["census_rounds"] >= 1
        and outcome.get("closure") == "closed"
        and outcome.get("fault") is None
        and outcome.get("deadline") is False
        and outcome.get("launched") is True
        and outcome.get("status") == LEADER_EXPECTED_STATUS)
    if stamp["normal_closure"]:
        if guard_exit != LEADER_EXPECTED_STATUS:
            reasons.append(f"a normal closure with guard exit {guard_exit}, not the declared 1")
            return OUTCOME_INCOMPLETE, stamp, reasons
        return (OUTCOME_INCOMPLETE if apparatus else OUTCOME_NOT_REPRODUCED), stamp, reasons
    reasons.append("the guard receipt matches neither the census-stall fingerprint nor a "
                   "normal closure with the declared leader status")
    return OUTCOME_INCOMPLETE, stamp, reasons


def bounded_report(document):
    """Keep the publication inside its fixed byte ceiling. Dropping samples is
    recorded, never silent."""
    trims = []
    while True:
        encoded = (json.dumps(document, indent=1, sort_keys=True) + "\n").encode("utf-8")
        if len(encoded) <= REPORT_MAX_BYTES:
            break
        if document.get("samples"):
            document["samples_omitted"] = len(document["samples"])
            document["samples"] = []
            document["truncated"] = True
            trims.append("samples")
            continue
        settlement = (document.get("guard") or {}).get("settlement")
        if isinstance(settlement, dict) and settlement:
            (document.get("guard") or {})["settlement"] = {
                key: settlement.get(key) for key in
                ("phase", "rounds_started", "rounds_completed", "loop_exit_ns",
                 "thread_start_ns", "stopped_received_ns")}
            trims.append("settlement")
            continue
        for name in ("guard_stdout", "guard_stderr"):
            entry = document.get(name)
            if isinstance(entry, dict) and entry.get("text"):
                entry["text"] = entry["text"][:4096]
                entry["truncated"] = True
                trims.append(name)
                break
        else:
            reasons = document.get("classification", {}).get("reasons")
            if isinstance(reasons, list) and len(reasons) > 4:
                document["classification"]["reasons"] = reasons[:4]
                trims.append("reasons")
                continue
            if document.get("limitations"):
                document["limitations"] = []
                trims.append("limitations")
                continue
            raise ValueError("the report cannot be bounded to its declared ceiling")
    document["trims"] = trims
    return len(encoded)


# ---------------------------------------------------------------------------
# The observation run
# ---------------------------------------------------------------------------


def observe_core(seams, env, control=None):
    """The one bounded occurrence, seam-driven end to end. It never writes the
    report; `run_observe` publishes the bounded result."""
    control = control if control is not None else {}
    report = {
        "schema": SCHEMA, "qualified": False, "claim": CLAIM, "mode": "observe",
        "source": {}, "invocation": {"selector": "rust:census-stall-observation",
                                     "guard_argv": guard_argv(), "limit_s": LIMIT_S,
                                     "scratch_budget_mib": SCRATCH_BUDGET_MIB},
        "environment_removed": [], "topology": {}, "identity": {},
        "samples": [], "sample_cap": MAX_SAMPLES, "sample_interval_ns": SAMPLE_INTERVAL_NS,
        "census_thread": {"state": "unknown", "tid": None, "reason": None},
        "census_series": {}, "guard": {}, "guard_stdout": {}, "guard_stderr": {},
        "synthetic_leader": {"declared_status": LEADER_EXPECTED_STATUS,
                             "label": "synthetic declared exit; never a producer relabel"},
        "classification": {"outcome": OUTCOME_INCOMPLETE, "reasons": [],
                           "stall_fingerprint": False, "normal_closure": False},
        "custody": {"guard_spawned": False, "guard_reaped": False, "guard_exit": None,
                    "guard_killed_by_observer": False, "monitor_observed_gone": None},
        "limits": {"report_max_bytes": REPORT_MAX_BYTES, "log_max_bytes": LOG_MAX_BYTES,
                   "observer_total_max_bytes": OBSERVER_TOTAL_MAX_BYTES,
                   "leader_document_max_bytes": LEADER_DOC_MAX_BYTES,
                   "max_monitor_threads": MAX_MONITOR_THREADS,
                   "stored_thread_rows": STORED_THREAD_ROWS},
        "files": {}, "observer_bytes": None, "truncated": False,
        "limitations": [
            "thread state and CPU are sampled observations; they cannot name the "
            "kernel call a parked thread is inside, so blocked-versus-slow is only "
            "separated for the observed shape",
            "the sampler itself perturbs the monitor (libproc reads and its own "
            "scheduling); the observed counts are not a claim of zero observer effect",
            "process-table size is a count-only observation, never a foreign row and "
            "never a safety or acceptance gate",
            "one occurrence bounds this window only; a non-reproduction is not a cause "
            "and neither outcome qualifies a runner, a fix or any consumer",
            "the observer root, report, captured streams and leader document live under "
            "the run TMPDIR outside the guard's owned-byte accounting; their bounded "
            "allocation is counted in the task total and is not a guard measurement, and "
            "the observer supplies no closure, reserve or cleanup verdict",
        ],
        "finished_ns": None,
    }
    handle = None
    doc = None
    guard_pid = guard_start = None
    monitor = {"attribution": "unknown", "pid": None, "start": None, "reason": None}
    attempts = {"doc": 0, "receipt": 0}
    problems = []

    source, source_problems = source_gate(seams)
    report["source"] = source
    if source_problems:
        problems.extend(source_problems)
        report["classification"]["reasons"] = problems[:8]
        return report, 2

    topology, topology_problems = preflight(seams, env)
    report["topology"] = topology
    if topology_problems:
        problems.extend(topology_problems)
        report["classification"]["reasons"] = problems[:8]
        return report, 2

    start = seams.now_ns()
    deadline = start + LIMIT_S * NS
    token = secrets.token_hex(8)
    try:
        root = seams.mkdtemp(topology["tmpdir"], "census-stall-observation.")
        publish_dir = os.path.join(root, PUBLISH_DIR)
        seams.mkdir(publish_dir, 0o700)
        report["files"]["root"] = root
        report["files"]["publish_dir"] = publish_dir
        child_env, removed = guard_environment(env, publish_dir, token)
        report["environment_removed"] = removed
        handle = seams.spawn(guard_argv(), child_env, REPO)
        report["custody"]["guard_spawned"] = True
        guard_pid = handle["process"].pid
        row, problem = seams.process_row(guard_pid)
        if problem is not None or row is None or not strict_int(row.get("start")):
            raise RuntimeError(f"the guard's own identity is unreadable ({problem or 'absent'})")
        guard_start = row["start"]
        report["identity"]["guard"] = {"pid": guard_pid, "start": guard_start,
                                       "parent": seams.self_pid()}
        document_path = os.path.join(publish_dir, DOC_NAME)
        doc_wait_until = min(deadline, start + DOC_WAIT_NS)
        while seams.now_ns() < doc_wait_until:
            exit_code = seams.poll(handle)
            if exit_code is not None:
                report["custody"]["guard_exit"] = exit_code
                report["custody"]["guard_reaped"] = True
                break
            if seams.exists(document_path):
                try:
                    doc, doc_size, doc_sha = seams.read_document(document_path,
                                                                LEADER_DOC_MAX_BYTES)
                except (OSError, ValueError) as error:
                    attempts["doc"] += 1
                    problems.append(f"the leader document is unusable ({type_name(error)})")
                    break
                report["files"]["leader_identity"] = {"path": document_path,
                                                      "bytes": doc_size, "sha256": doc_sha}
                break
            attempts["doc"] += 1
            seams.sleep(DOC_POLL_NS / NS)
        if doc is not None:
            doc_problems = validate_leader_document(doc, token, guard_pid, guard_start)
            report["identity"]["leader"] = {
                key: (doc.get("leader") or {}).get(key) for key in
                ("pid", "start", "ppid", "ppid_start", "pgid", "sid", "expected_status")}
            if isinstance(doc.get("guard"), dict):
                report["identity"]["guard_layout"] = {
                    key: doc["guard"].get(key) for key in
                    ("run_id", "evidence", "scratch", "tmpdir", "deadline_ns")}
            problems.extend(doc_problems)
            leader_pid = (doc.get("leader") or {}).get("pid")
            leader_start = (doc.get("leader") or {}).get("start")
            if strict_int(leader_pid) and strict_int(leader_start):
                monitor = resolve_monitor(seams, guard_pid, guard_start, leader_pid,
                                          leader_start)
                report["identity"]["monitor"] = dict(monitor)
        samples, guard_exit, cancelled = sample_window(
            seams, {"pid": guard_pid, "start": guard_start, "handle": handle},
            monitor, deadline, control)
        report["samples"] = samples
        if guard_exit is None:
            guard_exit = seams.poll(handle)
        if guard_exit is None:
            drain_until = min(deadline, seams.now_ns() + EXIT_DRAIN_NS)
            while seams.now_ns() < drain_until:
                guard_exit = seams.poll(handle)
                if guard_exit is not None:
                    break
                seams.sleep(0.02)
        if guard_exit is None:
            report["custody"]["guard_killed_by_observer"] = seams.kill(handle)
            guard_exit = seams.wait(handle, EXIT_DRAIN_NS / NS)
            if guard_exit is None:
                problems.append("the guard child could not be positively reaped; custody is unknown")
            else:
                report["custody"]["guard_reaped"] = True
        else:
            report["custody"]["guard_reaped"] = True
        report["custody"]["guard_exit"] = guard_exit
        if cancelled:
            problems.append(cancelled)
        seams.join_drains(handle)
        output = {}
        try:
            output = seams.guard_output(handle) or {}
        except BaseException:
            output = {}
        for name, key in (("stdout", "guard_stdout"), ("stderr", "guard_stderr")):
            entry = output.get(name) or {}
            report[key] = {"text": entry.get("text", "") if isinstance(entry, dict) else "",
                           "bytes": (entry.get("bytes", 0) if isinstance(entry, dict) else 0),
                           "truncated": bool(entry.get("truncated")) if isinstance(entry, dict) else False}
        if monitor.get("pid") is not None:
            gone = False
            gone_until = min(deadline, seams.now_ns() + MONITOR_GONE_WAIT_NS)
            while seams.now_ns() < gone_until:
                ok, live, _zombie, problem = identity_facts(seams, monitor["pid"],
                                                           monitor["start"])
                if not ok and problem is None:
                    gone = True
                    break
                if not live:
                    gone = True
                    break
                seams.sleep(0.05)
            report["custody"]["monitor_observed_gone"] = gone
            if not gone:
                problems.append("the monitor was not observed gone after the guard exited")
        receipt_path = None
        if isinstance(doc, dict) and isinstance(doc.get("guard"), dict):
            evidence = doc["guard"].get("evidence")
            if abs_canonical(evidence):
                receipt_path = os.path.join(evidence, "receipt.json")
        receipt = None
        if receipt_path is not None:
            try:
                attempts["receipt"] += 1
                receipt, receipt_size, receipt_sha = seams.read_document(
                    receipt_path, RECEIPT_READ_MAX_BYTES)
                report["files"]["guard_receipt"] = {"path": receipt_path,
                                                    "bytes": receipt_size,
                                                    "sha256": receipt_sha}
            except (OSError, ValueError) as error:
                problems.append(f"the guard receipt is unreadable ({type_name(error)})")
        else:
            problems.append("the leader document names no usable evidence directory")
        if isinstance(receipt, dict):
            report["guard"] = receipt_facts(receipt)
            if isinstance(doc, dict):
                doc_guard = doc.get("guard") or {}
                if receipt.get("run_id") != doc_guard.get("run_id"):
                    problems.append("the guard receipt run id is not the leader's recorded run")
                if report["guard"].get("evidence") != doc_guard.get("evidence"):
                    problems.append("the guard receipt evidence is not the leader's layout")
                if report["guard"].get("leader") != [(doc.get("leader") or {}).get("pid"),
                                                     (doc.get("leader") or {}).get("start")]:
                    problems.append("the guard receipt leader identity is not the observed leader")
                command = receipt.get("command")
                if not (isinstance(command, list) and command == guard_argv()):
                    problems.append("the guard ran a different command than the fixed one")
                settlement = report["guard"].get("settlement") or {}
                report["census_thread"] = attribute_census_thread(samples, settlement)
                report["census_series"] = census_thread_series(
                    samples, report["census_thread"], settlement)
            selection = report["guard"].get("selection") or {}
            boot = topology.get("boot_device")
            if selection.get("source") != "selected-volume":
                problems.append(f"the guard's scratch source is {selection.get('source')}, "
                                "not the selected volume")
            if (selection.get("volume_uuid") or "").lower() != SELECTED_VOLUME_UUID:
                problems.append("the guard's scratch volume UUID is not the selected Stash UUID")
            if report["guard"].get("scratch_owned") is not True:
                problems.append("the guard did not own its scratch root")
            if report["guard"].get("reserve_device") != topology.get("device"):
                problems.append("the guard's reserve device is not the observer root's device")
            if boot is not None and report["guard"].get("reserve_device") == boot:
                problems.append("the guard's reserve device is the boot volume")
            root_path = report["guard"].get("root")
            prefix = os.path.join(topology.get("mount") or "", "scratch", "oxide") + os.sep
            if not abs_canonical(root_path):
                problems.append("the guard's root is not an absolute canonical path")
            elif not root_path.startswith(prefix):
                problems.append("the guard's root is outside the selected owned scratch subtree")
            marker = report["guard"].get("marker") or {}
            if marker.get("state") not in ("concluded", "kept"):
                problems.append(f"the guard's owner marker state is {marker.get('state')}")
        report["attempts"] = dict(attempts)
        report["sample_count"] = len(samples)
        report["table"] = bounded_table(samples)
        report["load"] = bounded_load(samples)
        outcome, stamp, reasons = classify(report, receipt, guard_exit, problems)
        report["classification"].update(outcome=outcome, reasons=reasons[:8], **stamp)
        return report, exit_for(outcome)
    except BaseException as error:
        problems.append(f"{type_name(error)}: {bounded_text(error)}")
        report["classification"]["outcome"] = OUTCOME_INCOMPLETE
        report["classification"]["reasons"] = problems[:8]
        return report, 1
    finally:
        report["finished_ns"] = seams.now_ns()
        report["custody"]["observer_children"] = 1 if report["custody"]["guard_spawned"] else 0
        if handle is not None:
            try:
                seams.join_drains(handle, 0.5)
            except BaseException:
                pass


def publish_report(seams, report):
    """Publish the bounded report and the captured streams. A failure is an
    incomplete observation, never a silent success."""
    root = (report.get("files") or {}).get("root")
    if not root:
        return 1
    try:
        files = report.setdefault("files", {})
        files["report"] = os.path.join(root, REPORT_NAME)
        extra = 0
        for key, name in (("guard_stdout", STDOUT_NAME), ("guard_stderr", STDERR_NAME)):
            entry = report.get(key) or {}
            if entry.get("text"):
                files[key] = os.path.join(root, name)
                data = (json.dumps({"text": entry["text"]}, sort_keys=True) + "\n").encode()
                extra += len(data)
        report["observer_bytes"] = (extra
                                    + ((files.get("leader_identity") or {}).get("bytes") or 0))
        total = bounded_report(report)
        if total + report["observer_bytes"] > OBSERVER_TOTAL_MAX_BYTES:
            report["limits"]["observer_bytes_over_cap"] = True
            report["classification"]["outcome"] = OUTCOME_INCOMPLETE
            report["classification"]["reasons"] = (
                list(report["classification"].get("reasons") or [])[:7]
                + ["the observer's own bytes exceed the declared allocation"])
            total = bounded_report(report)
        report["observer_bytes"] = total + report["observer_bytes"]
        seams.write_document(files["report"], report)
        for key, name in (("guard_stdout", STDOUT_NAME), ("guard_stderr", STDERR_NAME)):
            if files.get(key):
                entry = report.get(key) or {}
                data = (json.dumps({"text": entry["text"]}, sort_keys=True) + "\n").encode()
                seams.publish_new(files[key], data)
        return exit_for(report["classification"]["outcome"])
    except BaseException as error:
        report["classification"]["outcome"] = OUTCOME_INCOMPLETE
        report["classification"]["reasons"] = [
            f"the report could not be published ({type_name(error)})"]
        return 1


def run_observe(seams, env, control=None):
    report, code = observe_core(seams, env, control)
    if (report.get("files") or {}).get("root"):
        code = publish_report(seams, report)
    return report, code


def exit_for(outcome):
    """0 only for a completed observation with a declared non-incomplete class."""
    return 0 if outcome in (OUTCOME_NOT_REPRODUCED, OUTCOME_REPRODUCED_DISCRIMINATED,
                            OUTCOME_REPRODUCED_INSUFFICIENT) else 1


def bounded_table(samples):
    counts = [sample["table"] for sample in samples if strict_int(sample.get("table"))]
    return {"samples": len(counts),
            "minimum": min(counts) if counts else None,
            "maximum": max(counts) if counts else None,
            "last": counts[-1] if counts else None}


def bounded_load(samples):
    loads = [sample["load"] for sample in samples
             if isinstance(sample.get("load"), list) and len(sample["load"]) == 3]
    if not loads:
        return {"samples": 0, "last": None, "maximum": None}
    return {"samples": len(loads), "last": loads[-1],
            "maximum": [max(row[index] for row in loads) for index in range(3)]}


# ---------------------------------------------------------------------------
# The synthetic leader
# ---------------------------------------------------------------------------


def leader_main(seams, env):
    """Publish one bounded identity document, then exit the declared status 1.

    It refuses (exit 2) outside the unchanged guard's own exported layout, so a
    stray invocation can neither write anywhere nor pretend to be a leader."""
    required = ("OXIDE_PROOF_RUN_ID", "OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS",
                "OXIDE_TASK_SCRATCH_ROOT", PUBLISH_DIR_VAR, TOKEN_VAR)
    if not all(env.get(name) for name in required):
        print("census-stall-observation: leader refused: this mode runs only as the "
              "guard's own leader", file=sys.stderr)
        return 2
    run_id = env["OXIDE_PROOF_RUN_ID"]
    token = env[TOKEN_VAR]
    publish_dir = env[PUBLISH_DIR_VAR]
    evidence = env["OXIDE_PROOF_EVIDENCE"]
    scratch = env["OXIDE_TASK_SCRATCH_ROOT"]
    deadline = env["OXIDE_PROOF_DEADLINE_NS"]
    problems = []
    if not hex32(run_id):
        problems.append("the guard run id is not 32 hex characters")
    if not _HEX16.match(token):
        problems.append("the observer token is not 16 hex characters")
    for label, path in (("publish directory", publish_dir), ("evidence", evidence),
                        ("scratch", scratch)):
        if not abs_canonical(path):
            problems.append(f"the {label} is not an absolute canonical path")
    if problems:
        print("census-stall-observation: leader refused: " + "; ".join(problems),
              file=sys.stderr)
        return 2
    try:
        info = seams.lstat(publish_dir)
        if (not stat.S_ISDIR(info.st_mode) or stat.S_ISLNK(info.st_mode)
                or info.st_uid != seams.uid or info.st_mode & 0o077):
            problems.append("the publish directory is not this account's private directory")
        evidence_info = seams.lstat(evidence)
        scratch_info = seams.lstat(scratch)
        if (not stat.S_ISDIR(evidence_info.st_mode)
                or not stat.S_ISDIR(scratch_info.st_mode)):
            problems.append("the guard layout is not two real directories")
        if os.path.dirname(evidence) != os.path.dirname(scratch):
            problems.append("the guard evidence and scratch are not one root's siblings")
        if os.path.basename(evidence) != "evidence" or os.path.basename(scratch) != "scratch":
            problems.append("the guard layout is not the default root's evidence/scratch pair")
    except OSError as error:
        problems.append(f"the guard layout is unreadable ({errno_name(error)})")
    if problems:
        print("census-stall-observation: leader refused: " + "; ".join(problems),
              file=sys.stderr)
        return 2
    self_pid = seams.self_pid()
    self_ppid = seams.self_ppid()
    own, own_problem = seams.process_row(self_pid)
    parent, parent_problem = seams.process_row(self_ppid)
    if own_problem is not None or parent_problem is not None or own is None or parent is None:
        print("census-stall-observation: leader refused: its own or its parent's identity "
              "is unreadable", file=sys.stderr)
        return 2
    if own.get("pid") != self_pid or parent.get("pid") != self_ppid:
        print("census-stall-observation: leader refused: its own or its parent's identity "
              "does not match this process", file=sys.stderr)
        return 2
    document = {
        "schema": LEADER_SCHEMA, "token": token, "run_id": run_id,
        "published_ns": seams.now_ns(),
        "leader": {"pid": self_pid, "start": own["start"], "ppid": self_ppid,
                   "ppid_start": parent["start"], "pgid": seams.self_pgid(),
                   "sid": seams.self_sid(), "expected_status": LEADER_EXPECTED_STATUS},
        "guard": {"run_id": run_id, "evidence": evidence, "scratch": scratch,
                  "tmpdir": env.get("TMPDIR"), "deadline_ns": int(deadline)},
    }
    path = os.path.join(publish_dir, DOC_NAME)
    data = (json.dumps(document, indent=1, sort_keys=True) + "\n").encode("utf-8")
    if len(data) > LEADER_DOC_MAX_BYTES:
        print("census-stall-observation: leader refused: identity document over its bound",
              file=sys.stderr)
        return 2
    try:
        seams.publish_new(path, data)
    except (OSError, FileExistsError) as error:
        print(f"census-stall-observation: leader refused: the document could not be "
              f"published ({errno_name(error)})", file=sys.stderr)
        return 2
    print(f"leader: identity published {path} ({len(data)} bytes); synthetic declared "
          f"exit {LEADER_EXPECTED_STATUS}")
    remaining = (int(deadline) - seams.now_ns()) / NS
    seams.sleep(min(LEADER_HOLD_S, max(0.0, remaining)))
    return LEADER_EXPECTED_STATUS


# ---------------------------------------------------------------------------
# Plan and self-test
# ---------------------------------------------------------------------------


def plan_document(seams):
    source, problems = source_gate(seams) if seams is not None else ({}, [])
    return {
        "schema": SCHEMA, "qualified": False, "claim": CLAIM, "mode": "plan",
        "activated": False,
        "selectors": {
            "rust:census-stall-observation": "python3 scripts/census-stall-observation.py --observe",
            "proof:census-stall-observation:test":
                "python3 scripts/census-stall-observation.py --self-test",
            "proof:census-stall-observation:plan":
                "python3 scripts/census-stall-observation.py --plan",
        },
        "occurrence": {
            "count": 1, "owner_activated": True,
            "guard": guard_argv(),
            "guard_sha256": EXPECTED_RUNNER_SHA256,
            "candidate_sha256": EXPECTED_CANDIDATE_SHA256,
            "candidate_used": False,
            "selected_volume_uuid": SELECTED_VOLUME_UUID,
            "limits_s": LIMIT_S, "scratch_budget_mib": SCRATCH_BUDGET_MIB,
            "leader_declared_status": LEADER_EXPECTED_STATUS,
            "no_image": True, "no_build": True, "no_cargo": True, "no_staging": True,
        },
        "caps": {
            "sample_instants": MAX_SAMPLES, "sample_interval_ns": SAMPLE_INTERVAL_NS,
            "monitor_threads": MAX_MONITOR_THREADS, "stored_thread_rows": STORED_THREAD_ROWS,
            "report_bytes": REPORT_MAX_BYTES, "log_bytes": LOG_MAX_BYTES,
            "observer_total_bytes": OBSERVER_TOTAL_MAX_BYTES,
            "leader_document_bytes": LEADER_DOC_MAX_BYTES,
            "late_window_start_ns": LATE_WINDOW_START_NS,
            "min_late_window_ns": MIN_LATE_WINDOW_NS, "slow_cpu_ns": SLOW_CPU_NS,
        },
        "observer": {
            "outside_guard": True,
            "reads": ["own guard/custodian PID+start", "own monitor PID+start",
                      "monitor thread ids/CPU/run state", "process-table count only",
                      "load average", "guard receipt and output"],
            "never": ["ptrace", "DTrace", "sample", "signal injection", "suspension",
                      "host setting or limit change", "historical root/image/process read",
                      "foreign identity persistence"],
            "last_resort_signal": "SIGKILL of its own still-unreaped direct child guard",
        },
        "outcomes": list(OUTCOMES),
        "premises": [{"item": item, "disposition": text} for item, text in PREMISE_CHECKLIST],
        "source": source,
        "source_problems": problems,
        "note": ("source and ordinary effect-free readiness only; the heavy selector is "
                 "not activated and awards no qualification, credit or consumer permission"),
    }


def assert_that(condition, message):
    if not condition:
        raise AssertionError(message)


class FakeStat:
    def __init__(self, mode, uid, device, inode=1):
        self.st_mode = mode
        self.st_uid = uid
        self.st_dev = device
        self.st_ino = inode


class FakeClock:
    def __init__(self, start=1_000_000_000):
        self.value = start

    def now(self):
        return self.value

    def sleep(self, seconds):
        self.value += int(round(seconds * NS))


class FakeSeams:
    """Scripted external boundaries for the apparatus tests.

    The timeline is time-based, so the real sampling loop runs against it: the
    guard exits at `exit_at`, the leader document appears at `doc_at`, the
    monitor exists between `monitor_birth` and `monitor_death`, and the census
    thread appears at `census_at`."""

    def __init__(self, script):
        self.script = script
        self.clock = FakeClock(script.get("start_ns", 1_000_000_000))
        self.uid = 501
        self.spawned = []
        self.written = {}
        self.removed = []
        self.git_calls = []
        self.files = dict(script.get("files") or {})
        self.process = script.get("process") or {}
        self.children = script.get("children")
        self.threads = script.get("threads") or {}
        self.thread_info_rows = script.get("thread_info") or {}
        self.table = script.get("table")
        self.load = script.get("load")
        self.guard_exit = script.get("guard_exit")
        self.guard_output = script.get("guard_output") or {}
        self.killed = False
        self.kill_returns = script.get("kill_returns", True)
        self.wait_returns = script.get("wait_returns", 125)
        self.root = script.get("root", "/Volumes/Stash/scratch/oxide/3881b6d0/census-stall-observation.test")
        self.publish_dir = os.path.join(self.root, PUBLISH_DIR)

    # clock
    def now_ns(self):
        return self.clock.now()

    def sleep(self, seconds):
        self.clock.sleep(seconds)

    def sha256_file(self, path):
        if path in self.files:
            return self.files[path]
        if path == RUNNER_PATH:
            return EXPECTED_RUNNER_SHA256
        if path == CANDIDATE_PATH:
            return EXPECTED_CANDIDATE_SHA256
        return "0" * 64

    def git(self, *arguments):
        self.git_calls.append(arguments)
        head = self.script.get("head", "a" * 40)
        tree = self.script.get("tree", "b" * 40)
        status = self.script.get("status", "")
        if arguments == ("status", "--porcelain", "--untracked-files=no"):
            return self.script.get("status_exit", 0), status
        if arguments == ("rev-parse", "HEAD"):
            return self.script.get("head_exit", 0), head
        if arguments == ("rev-parse", "HEAD^{tree}"):
            return self.script.get("tree_exit", 0), tree
        if arguments == ("branch", "--show-current"):
            return self.script.get("branch_exit", 0), self.script.get("branch", "ns-test")
        return 0, ""

    def stat(self, path):
        stats = self.script.get("stats") or {}
        if path in stats:
            return stats[path]
        raise FileNotFoundError(path)

    def lstat(self, path):
        stats = self.script.get("lstats") or {}
        if path in stats:
            return stats[path]
        return self.stat(path)

    def realpath(self, path):
        return self.script.get("realpaths", {}).get(path, path)

    def volume_uuid(self, path):
        if self.script.get("uuid_error"):
            raise OSError(errno.EACCES, "uuid")
        return self.script.get("uuid", "8A660440-C919-45C6-A3F1-3365B9BD2006")

    def device(self, path):
        return self.script.get("devices", {}).get(path, 42)

    def layout_check(self):
        return None

    def self_pid(self):
        return self.script.get("self_pid", 5000)

    def self_ppid(self):
        return self.script.get("self_ppid", 5001)

    def self_pgid(self):
        return self.script.get("self_pgid", 5000)

    def self_sid(self):
        return self.script.get("self_sid", 5000)

    def exists(self, path):
        return path in self.files

    def publish_new(self, path, data):
        if path in self.written or path in self.files:
            raise FileExistsError(path)
        self.written[path] = bytes(data)
        return len(data)

    def process_row(self, pid):
        if pid in (4243, 4244) and self.clock.now() >= self.script.get("exit_at", 10 ** 12):
            return None, None
        rows = self.process.get(pid)
        if callable(rows):
            rows = rows(pid, self.clock.now())
        if isinstance(rows, list):
            if not rows:
                return None, None
            return rows.pop(0), None
        if rows is None and self.process.get("__problem__"):
            return None, "EPERM"
        return rows, None

    def monitor_children(self, pid):
        value = self.children
        if callable(value):
            value = value(pid, self.clock.now())
        if isinstance(value, list) and value and isinstance(value[0], list):
            return value.pop(0)
        if value == "unreadable":
            return None
        return value

    def thread_ids(self, pid):
        value = self.threads.get(pid)
        if callable(value):
            value = value(pid, self.clock.now())
        if value == "unreadable":
            return None
        return value

    def thread_info(self, pid, thread):
        rows = self.thread_info_rows.get(thread)
        if callable(rows):
            rows = rows(thread, self.clock.now())
        if isinstance(rows, list):
            return rows.pop(0) if rows else None
        if rows == "unreadable":
            return None
        return rows

    def table_count(self):
        return self.table

    def load_average(self):
        return self.load

    def spawn(self, argv, env, cwd):
        self.spawned.append({"argv": list(argv), "env": dict(env), "cwd": cwd})
        class Process:
            pid = 4242
        self.guard_pid = 4242
        self.guard_start = self.script.get("guard_start", 700)
        return {"process": Process(), "capture": {}, "drains": []}

    def poll(self, handle):
        if self.clock.now() >= self.script.get("exit_at", 10 ** 9):
            return self.guard_exit
        return None

    def wait(self, handle, timeout_s):
        self.clock.sleep(timeout_s)
        return self.wait_returns

    def kill(self, handle):
        self.killed = True
        return self.kill_returns

    def join_drains(self, handle, timeout_s=1.0):
        return None

    def guard_output(self, handle):
        return self.guard_output

    def mkdtemp(self, parent, prefix):
        return self.root

    def mkdir(self, path, mode=0o700):
        self.written[path] = None

    def write_document(self, path, document):
        self.written[path] = document
        return len(compact_json(document))

    def read_document(self, path, cap):
        if path in self.files:
            value = self.files[path]
            raw = value if isinstance(value, bytes) else compact_json(value).encode()
            return json.loads(raw.decode()), len(raw), sha256_bytes(raw)
        raise FileNotFoundError(path)


def base_script(**overrides):
    """A complete, honest normal-closure script for the fake seams."""
    tmpdir = "/Volumes/Stash/scratch/oxide/3881b6d0"
    root = os.path.join(tmpdir, "census-stall-observation.test")
    publish = os.path.join(root, PUBLISH_DIR)
    evidence = "/Volumes/Stash/scratch/oxide/oxide-proof.test/evidence"
    scratch = "/Volumes/Stash/scratch/oxide/oxide-proof.test/scratch"
    leader_pid, leader_start = 4243, 800
    monitor_pid, monitor_start = 4244, 801
    token = "0123456789abcdef"
    doc = {
        "schema": LEADER_SCHEMA, "token": token, "run_id": "c" * 32,
        "published_ns": 1_100_000_000,
        "leader": {"pid": leader_pid, "start": leader_start, "ppid": 4242,
                   "ppid_start": 700, "pgid": leader_pid, "sid": leader_pid,
                   "expected_status": 1},
        "guard": {"run_id": "c" * 32, "evidence": evidence, "scratch": scratch,
                  "tmpdir": scratch, "deadline_ns": 300 * NS},
    }
    receipt = {
        "run_id": "c" * 32, "command": guard_argv(), "root": scratch.rsplit("/", 1)[0],
        "scratch": scratch, "evidence": evidence, "scratch_owned": True,
        "leader": [leader_pid, leader_start],
        "outcome": {"closure": "closed", "status": 1, "fault": None,
                    "deadline": False, "launched": True},
        "settlement": {"phase": "loop-exit", "stopped_received_ns": 1_200_000_000,
                       "thread_start_ns": 1_200_100_000, "thread_start_latency_ns": 100_000,
                       "first_round_ns": 1_200_200_000, "stop_requested_ns": 1_250_000_000,
                       "loop_exit_ns": 1_250_500_000, "rounds_started": 3,
                       "rounds_completed": 3, "omitted": 0},
        "census_join": {"joined": True, "started": True, "bound_s": 3.0,
                        "waited_ns": 10_000, "rows": 0},
        "deciding_census": {"certain": True, "members": 0, "uncertain": 0},
        "census": {"certain": True, "members": 0, "uncertain": 0},
        "census_rounds": 3,
        "scratch_default": {"source": "selected-volume",
                            "volume_uuid": "8A660440-C919-45C6-A3F1-3365B9BD2006",
                            "mount": "/Volumes/Stash",
                            "registration": "/Users/tom/Dev/oxide/oxide-managed/removable.json",
                            "root_parent": "/Volumes/Stash/scratch/oxide"},
        "reserve": {"device": 42},
        "owned_bytes": {"budget": 512 * MIB, "maximum": 1024, "samples": 4},
        "scratch_marker": {"state": "concluded", "run_id": "d" * 64, "reason": "evidence"},
    }
    # A baseline of two monitor threads, plus the census thread after 1.2 s.
    def threads(_pid, at):
        ids = [9001, 9002]
        if at >= 1_200_100_000:
            ids.append(9003)
        return ids

    def info(thread, at):
        if thread == 9003:
            # A parked census thread: zero CPU progress over the late window.
            return {"cpu_ns": 3_000_000, "state": 3, "name": ""}
        return {"cpu_ns": 1_000_000, "state": 3, "name": ""}

    script = {
        "status": "", "start_ns": 1_000_000_000, "exit_at": 1_300_000_000,
        "guard_exit": 1,
        "lstats": {
            tmpdir: FakeStat(stat.S_IFDIR | 0o700, 501, 42),
            publish: FakeStat(stat.S_IFDIR | 0o700, 501, 42),
            evidence: FakeStat(stat.S_IFDIR | 0o700, 501, 42),
            scratch: FakeStat(stat.S_IFDIR | 0o700, 501, 42),
            "/Volumes/Stash": FakeStat(stat.S_IFDIR | 0o755, 0, 42),
            "/Volumes/Stash/scratch": FakeStat(stat.S_IFDIR | 0o755, 501, 42),
        },
        "stats": {"/Volumes/Stash/scratch": FakeStat(stat.S_IFDIR | 0o755, 501, 42)},
        "devices": {"/": 10, "/Volumes": 20, "/Volumes/Stash": 42, tmpdir: 42},
        "realpaths": {},
        "process": {4242: {"pid": 4242, "start": 700, "zombie": False, "ppid": 1,
                           "pgid": 4242, "uid": 501},
                    4243: {"pid": 4243, "start": 800, "zombie": False, "ppid": 4242,
                           "pgid": 4243, "uid": 501},
                    4244: {"pid": 4244, "start": 801, "zombie": False, "ppid": 4242,
                           "pgid": 4242, "uid": 501}},
        "children": [4243, 4244],
        "threads": {4244: threads},
        "thread_info": {9001: info, 9002: info, 9003: info},
        "table": 1900, "load": [1.0, 1.5, 2.0],
        "files": {os.path.join(publish, DOC_NAME): doc,
                  os.path.join(evidence, "receipt.json"): receipt},
        "guard_output": {"stdout": {"text": "supervisor: closed\n", "bytes": 20,
                                    "truncated": False},
                         "stderr": {"text": "", "bytes": 0, "truncated": False}},
    }
    script.update(overrides)
    return script


def run_script(overrides=None, token="0123456789abcdef"):
    """One scripted observation against the fake external boundaries."""
    script = base_script(**(overrides or {}))
    seams = FakeSeams(script)
    original = secrets.token_hex

    def token_hex(bits):
        if bits == 8:
            return token
        return original(bits)

    secrets.token_hex = token_hex
    try:
        report, code = run_observe(
            seams, {"TMPDIR": "/Volumes/Stash/scratch/oxide/3881b6d0"})
    finally:
        secrets.token_hex = original
    return report, code, seams


def receipt_path():
    return "/Volumes/Stash/scratch/oxide/oxide-proof.test/evidence/receipt.json"


def document_path():
    return os.path.join("/Volumes/Stash/scratch/oxide/3881b6d0/"
                        "census-stall-observation.test", PUBLISH_DIR, DOC_NAME)


def stall_receipt(receipt, shape="blocked"):
    receipt["outcome"].update(closure="UNCLOSED", status=1, deadline=False, launched=True,
                              fault=CENSUS_FAULT_PREFIX)
    receipt["settlement"].update(rounds_started=1, rounds_completed=0, loop_exit_ns=None,
                                 phase="round-start", first_round_ns=1_200_200_000)
    receipt["census_rounds"] = 0
    receipt["census"] = None
    receipt["deciding_census"] = None
    receipt["census_join"] = {"joined": False, "started": True, "bound_s": 3.0,
                              "waited_ns": 3_000_000_000, "frozen": True, "rows": 0}
    return receipt


def normal_receipt(receipt):
    return receipt


def stall_script(shape="blocked", extra=None):
    """A stall script whose census thread appears at 1.2 s and is parked."""
    base = base_script()
    publish = os.path.dirname(document_path())
    evidence = "/Volumes/Stash/scratch/oxide/oxide-proof.test/evidence"

    def threads(_pid, at):
        ids = [9001, 9002]
        if at >= 1_200_100_000:
            ids.append(9003)
        return ids

    def info(thread, at):
        if thread == 9003:
            if shape == "slow":
                return {"cpu_ns": 3_000_000 + max(0, (at - 1_200_000_000) // 20_000_000) * NS,
                        "state": 1, "name": ""}
            return {"cpu_ns": 3_000_000, "state": 3, "name": ""}
        return {"cpu_ns": 1_000_000, "state": 3, "name": ""}

    base["threads"] = {4244: threads}
    base["thread_info"] = {9001: info, 9002: info, 9003: info}
    base["exit_at"] = 13_000_000_000
    base["guard_exit"] = 125
    stall_receipt(base["files"][os.path.join(evidence, "receipt.json")], shape)
    base.update(extra or {})
    return base


def self_test():
    """The pure apparatus check: real entry/orchestrator/sampler/report seams
    driven with fake EXTERNAL process and kernel boundaries."""
    checks = []
    run_tmpdir = os.environ.get("TMPDIR")
    assert_that(run_tmpdir and abs_canonical(run_tmpdir),
                "the self-test requires the explicit run TMPDIR")
    temporary = tempfile.mkdtemp(prefix="census-stall-selftest.", dir=run_tmpdir)

    def check(name, function):
        function()
        checks.append(name)

    try:
        # -- real source metadata only (no kernel query) --------------------
        def source_files():
            assert_that(sha256_file_plain(RUNNER_PATH) == EXPECTED_RUNNER_SHA256,
                        "the deployed default supervisor bytes changed")
            assert_that(sha256_file_plain(CANDIDATE_PATH) == EXPECTED_CANDIDATE_SHA256,
                        "the read-only candidate bytes changed")
            assert_that(os.path.isfile(WORKFLOW_PATH), "the owning workflow document is missing")
            compile(open(SCRIPT_PATH, encoding="utf-8").read(), SCRIPT_PATH, "exec")
        check("source: bound runner, read-only candidate, owning document, own parse", source_files)

        def normal_run():
            report, code, seams = run_script()
            assert_that(len(seams.spawned) == 1, "the observer spawns exactly one guard child")
            spawned = seams.spawned[0]
            assert_that(spawned["argv"] == guard_argv(), "the fixed guard argv is used")
            assert_that(spawned["env"][TOKEN_VAR] == "0123456789abcdef", "the token is handed over")
            assert_that(spawned["env"][PUBLISH_DIR_VAR] == seams.publish_dir,
                        "the publish directory is handed over")
            assert_that(spawned["env"].get("OXIDE_SUPERVISOR_INJECT") is None
                        and spawned["env"].get("OXIDE_TASK_SCRATCH_ROOT") is None,
                        "the inherited test controls are removed")
            assert_that(code == 0, f"a normal closure exits 0, got {code}: "
                                   f"{report['classification']['reasons']}")
            assert_that(report["classification"]["outcome"] == OUTCOME_NOT_REPRODUCED,
                        "a normal closure is not_reproduced")
            assert_that(report["classification"]["normal_closure"] is True,
                        "the normal-closure fingerprint is recorded")
            assert_that(report["custody"]["guard_reaped"] is True, "the guard is positively reaped")
            assert_that(report["custody"]["guard_exit"] == 1, "the declared status is recorded")
            assert_that(report["custody"]["monitor_observed_gone"] is True,
                        "the monitor is observed gone")
            assert_that(report["samples"], "samples were taken")
            assert_that(report["table"]["minimum"] == 1900 and report["table"]["maximum"] == 1900,
                        "the process-table size is a count observation")
            assert_that(report["identity"]["monitor"]["attribution"] == "sole-remaining-child",
                        "the monitor is positively attributed")
            assert_that(report["census_thread"]["state"] == "attributed",
                        "the census thread is positively attributed")
            assert_that(spawned["env"].get("OXIDE_PROOF_EVIDENCE") is None,
                        "the observer does not fabricate guard variables")
            assert_that("observer.json" not in report["files"], "no invented file path")
            assert_that(report["observer_bytes"] and report["observer_bytes"] <= OBSERVER_TOTAL_MAX_BYTES,
                        "the observer's bytes stay inside its allocation")
        check("observe: normal closure, custody, identity, count-only table", normal_run)

        def blocked_run():
            report, code, _seams = run_script(stall_script("blocked"))
            assert_that(code == 0, f"a reproduced observation exits 0: {code}")
            classification = report["classification"]
            assert_that(classification["outcome"] == OUTCOME_REPRODUCED_DISCRIMINATED,
                        f"blocked shape discriminates: {classification['reasons']}")
            assert_that(classification["stall_fingerprint"] is True,
                        "the historical fingerprint is matched exactly")
            assert_that(classification["shape"] == SHAPE_BLOCKED, "the parked shape is named")
            assert_that(classification["shape_reason"] and "no measurable CPU progress" in
                        classification["shape_reason"], "the shape reason is bounded and honest")
            assert_that(report["census_series"]["late_rows"] >= 2,
                        "the late window has real samples")
            assert_that(report["guard"]["settlement"]["rounds_completed"] == 0,
                        "round completion is read from the guard receipt")
        check("classify: reproduced stall with a parked census thread", blocked_run)

        def slow_run():
            report, code, _seams = run_script(stall_script("slow"))
            assert_that(code == 0, "a reproduced slow-round observation exits 0")
            assert_that(report["classification"]["outcome"] == OUTCOME_REPRODUCED_DISCRIMINATED,
                        "a slow round also discriminates")
            assert_that(report["classification"]["shape"] == SHAPE_SLOW,
                        "CPU progress names the slow-round shape")
        check("classify: reproduced stall with CPU progress", slow_run)

        def insufficient_run():
            base = stall_script("blocked")

            def threads(_pid, at):
                return [9001, 9002]
            base["threads"] = {4244: threads}
            report, code, _seams = run_script(base)
            assert_that(code == 0, "an insufficient reproduction is still an observation")
            assert_that(report["classification"]["outcome"] == OUTCOME_REPRODUCED_INSUFFICIENT,
                        "no new thread cannot discriminate")
            assert_that(report["classification"]["stall_fingerprint"] is True,
                        "the fingerprint is still recorded")
        check("classify: reproduced but insufficient", insufficient_run)

        def ambiguous_run():
            base = stall_script("blocked")

            def threads(_pid, at):
                ids = [9001, 9002]
                if at >= 1_200_100_000:
                    ids.extend([9003, 9004])
                return ids
            base["threads"] = {4244: threads}
            base["thread_info"][9004] = lambda thread, at: {
                "cpu_ns": 4_000_000, "state": 3, "name": ""}
            report, _code, _seams = run_script(base)
            assert_that(report["classification"]["outcome"] == OUTCOME_REPRODUCED_INSUFFICIENT,
                        "two new threads are ambiguous, never guessed")
            assert_that(report["census_thread"]["state"] == "ambiguous",
                        "the ambiguity is recorded")
        check("classify: ambiguous thread attribution is not a diagnosis", ambiguous_run)

        def expected_status_versus_fault():
            base = stall_script("blocked")
            base["guard_exit"] = 1
            report, code, _seams = run_script(base)
            assert_that(code == 1 and report["classification"]["outcome"] == OUTCOME_INCOMPLETE,
                        "a stall fingerprint with the declared exit is contradictory")
            base = base_script()
            base["guard_exit"] = 125
            report, code, _seams = run_script(base)
            assert_that(code == 1 and report["classification"]["outcome"] == OUTCOME_INCOMPLETE,
                        "a normal receipt with a fault exit is contradictory")
            base = base_script()
            base["files"][receipt_path()]["outcome"]["deadline"] = True
            report, _code, _seams = run_script(base)
            assert_that(report["classification"]["outcome"] == OUTCOME_INCOMPLETE,
                        "a deadline is never a normal closure")
            base = base_script()
            base["files"][receipt_path()]["outcome"]["fault"] = "reserve readings stale"
            base["files"][receipt_path()]["outcome"]["closure"] = "UNCLOSED"
            report, _code, _seams = run_script(base)
            assert_that(report["classification"]["outcome"] == OUTCOME_INCOMPLETE,
                        "a different fault is not the census stall")
        check("classify: declared status, fault and deadline stay distinct", expected_status_versus_fault)

        def missing_packets():
            base = base_script()
            base["files"].pop(receipt_path())
            report, code, _seams = run_script(base)
            assert_that(code == 1 and report["classification"]["outcome"] == OUTCOME_INCOMPLETE,
                        "a missing guard receipt is incomplete")
            base = base_script()
            base["files"].pop(document_path())
            report, code, _seams = run_script(base)
            assert_that(code == 1 and report["classification"]["outcome"] == OUTCOME_INCOMPLETE,
                        "a missing leader document is incomplete")
            assert_that(report["identity"].get("monitor") is None,
                        "no monitor is invented without the leader identity")
        check("observe: missing leader/receipt packets are incomplete", missing_packets)

        def source_refusals():
            for name, mutation in (
                    ("dirty", {"status": " M scripts/bounded-test.py"}),
                    ("nonexact head", {"head": "abc"}),
                    ("status exit", {"status_exit": 1}),
                    ("head exit", {"head_exit": 2}),
                    ("tree exit", {"tree_exit": 2}),
                    ("branch exit", {"branch_exit": 2}),
                    ("runner drift", {"files": {RUNNER_PATH: "f" * 64}}),
                    ("candidate drift", {"files": {CANDIDATE_PATH: "f" * 64}})):
                report, code, seams = run_script(mutation)
                assert_that(code == 2, f"{name}: refused")
                assert_that(not seams.spawned, f"{name}: nothing is spawned")
                assert_that(not (report["files"] or {}).get("root"),
                            f"{name}: no observer root is created")
                assert_that(report["classification"]["reasons"], f"{name}: refusal is named")
        check("source gate: drift and dirty/nonexact identity refuse before effect", source_refusals)

        def topology_refusals():
            cases = [
                ("missing TMPDIR", {}, {"TMPDIR": None}),
                ("relative TMPDIR", {}, {"TMPDIR": "relative/path"}),
                ("group-writable TMPDIR", {
                    "lstats": {"/Volumes/Stash/scratch/oxide/3881b6d0":
                               FakeStat(stat.S_IFDIR | 0o770, 501, 42)}}, None),
                ("boot device", {"devices": {"/": 10, "/Volumes": 20,
                                             "/Volumes/Stash": 10,
                                             "/Volumes/Stash/scratch/oxide/3881b6d0": 10}}, None),
                ("wrong UUID", {"uuid": "11111111-2222-3333-4444-555555555555"}, None),
                ("uuid unreadable", {"uuid_error": True}, None),
            ]
            for name, overrides, env_override in cases:
                script = base_script(**overrides)
                seams = FakeSeams(script)
                env = {"TMPDIR": "/Volumes/Stash/scratch/oxide/3881b6d0"}
                if env_override is not None:
                    env = env_override
                report, code = run_observe(seams, env)
                assert_that(code == 2, f"{name}: refused")
                assert_that(not seams.spawned, f"{name}: nothing is spawned")
        check("preflight: route, ownership and selected UUID refuse before effect", topology_refusals)

        def post_topology():
            mutations = [
                ("fallback", lambda receipt: receipt["scratch_default"].update(source="fallback")),
                ("uuid", lambda receipt: receipt["scratch_default"].update(
                    volume_uuid="11111111-2222-3333-4444-555555555555")),
                ("not owned", lambda receipt: receipt.update(scratch_owned=False)),
                ("device", lambda receipt: receipt["reserve"].update(device=99)),
                ("outside root", lambda receipt: receipt.update(
                    root="/tmp/elsewhere")),
                ("marker", lambda receipt: receipt["scratch_marker"].update(state="running")),
            ]
            for name, mutate in mutations:
                base = base_script()
                mutate(base["files"][receipt_path()])
                report, code, _seams = run_script(base)
                assert_that(code == 1, f"{name}: incomplete")
                assert_that(report["classification"]["outcome"] == OUTCOME_INCOMPLETE,
                            f"{name}: a changed guard topology is not a pass")
        check("guard receipt: route, ownership, device and root are re-derived", post_topology)

        def identity_mismatches():
            mutations = [
                ("leader", lambda receipt: receipt.update(leader=[9999, 1])),
                ("run", lambda receipt: receipt.update(run_id="e" * 32)),
                ("command", lambda receipt: receipt.update(command=["python3", "other.py"])),
                ("evidence", lambda receipt: receipt.update(evidence="/tmp/other")),
            ]
            for name, mutate in mutations:
                base = base_script()
                mutate(base["files"][receipt_path()])
                report, _code, _seams = run_script(base)
                assert_that(report["classification"]["outcome"] == OUTCOME_INCOMPLETE,
                            f"{name}: an identity mismatch is never a closure")
            base = base_script()
            base["files"][document_path()]["leader"]["ppid"] = 4244
            report, _code, _seams = run_script(base)
            assert_that(report["classification"]["outcome"] == OUTCOME_INCOMPLETE,
                        "a leader document with the wrong parent is incomplete")
        check("identity: receipt and leader bindings are cross-checked", identity_mismatches)

        def monitor_resolution():
            base = base_script()
            seams = FakeSeams(base)
            assert_that(resolve_monitor(seams, 4242, 700, 4243, 800)["attribution"]
                        == "sole-remaining-child", "one remaining child is the monitor")
            seams = FakeSeams(base_script())
            seams.children = [4243]
            assert_that(resolve_monitor(seams, 4242, 700, 4243, 800)["attribution"]
                        == "unknown", "a leader-only child list is not a monitor")
            seams = FakeSeams(base_script())
            seams.children = [4243, 4244, 4245]
            seams.process[4245] = {"pid": 4245, "start": 802, "zombie": False,
                                   "ppid": 4242, "pgid": 4242, "uid": 501}
            assert_that(resolve_monitor(seams, 4242, 700, 4243, 800)["attribution"]
                        == "unknown", "two remaining children are ambiguous")
            seams = FakeSeams(base_script())
            seams.children = "unreadable"
            assert_that(resolve_monitor(seams, 4242, 700, 4243, 800)["attribution"]
                        == "unknown", "an unreadable child list is unknown")
            seams = FakeSeams(base_script())
            seams.children = [4244]
            assert_that(resolve_monitor(seams, 4242, 700, 4243, 800)["attribution"]
                        == "sole-remaining-child",
                        "a reaped leader leaves the monitor positively identifiable")
            seams = FakeSeams(base_script())
            seams.children = [4243, 4244]
            seams.process[4244] = {"pid": 4244, "start": 999, "zombie": False,
                                   "ppid": 4242, "pgid": 4242, "uid": 501}
            assert_that(resolve_monitor(seams, 4242, 700, 4243, 800)["attribution"]
                        == "sole-remaining-child",
                        "a changed monitor start is still a distinct row, then verified")
            ok, live, _zombie, problem = identity_facts(seams, 4244, 801)
            assert_that(not ok and problem == "changed-identity",
                        "a recycled PID/start is never the same process")
            base = base_script()
            base["files"][receipt_path()] = b"not json"
            report, _code, _seams = run_script(base)
            assert_that(report["classification"]["outcome"] == OUTCOME_INCOMPLETE,
                        "a malformed guard receipt is incomplete")
        check("identity: monitor attribution, recycled tokens, malformed receipt", monitor_resolution)

        def unreadable_data():
            base = stall_script("blocked")
            base["threads"] = {4244: "unreadable"}
            report, code, _seams = run_script(base)
            assert_that(code == 0, "unreadable thread data still completes the observation")
            assert_that(report["classification"]["outcome"] == OUTCOME_REPRODUCED_INSUFFICIENT,
                        "unreadable thread data cannot discriminate")
            assert_that(report["census_thread"]["state"] != "attributed",
                        "an unreadable thread list is never attributed")
            base = stall_script("blocked")
            base["threads"] = {4244: lambda _pid, _at: [9001, 9002, 9003]}

            def no_info(thread, at):
                return "unreadable" if thread == 9003 else {
                    "cpu_ns": 1_000_000, "state": 3, "name": ""}
            base["thread_info"] = {9001: no_info, 9002: no_info, 9003: no_info}
            report, _code, _seams = run_script(base)
            assert_that(report["classification"]["outcome"] == OUTCOME_REPRODUCED_INSUFFICIENT,
                        "unreadable census-thread state is never a shape")
            base = base_script()
            base["exit_at"] = 1_050_000_000
            base["files"].pop(document_path())
            report, code, _seams = run_script(base)
            assert_that(code == 1 and report["classification"]["outcome"]
                        == OUTCOME_INCOMPLETE,
                        "a leader document that never arrives is incomplete")
            assert_that(report["attempts"]["doc"] >= 1,
                        "the bounded document attempts are recorded, not hidden")
        check("data: unreadable thread state and a late document are not invented", unreadable_data)

        def window_bounds():
            script = base_script()
            script["exit_at"] = 10 ** 12
            seams = FakeSeams(script)

            def jump(seconds):
                seams.clock.value += 60 * NS
            seams.sleep = jump
            deadline = seams.now_ns() + 120 * NS
            samples, exit_code, cancelled = sample_window(
                seams, {"pid": 4242, "start": 700, "handle": object()},
                {"pid": None}, deadline, {"cancelled": None})
            assert_that(exit_code is None and cancelled is None,
                        "the deadline stop is not a guard exit or a cancellation")
            assert_that(len(samples) <= 3, "the sampling loop stops at the absolute deadline")
            assert_that(seams.now_ns() >= deadline, "no sample continues past the deadline")
            seams = FakeSeams(base_script())
            samples, _exit_code, cancelled = sample_window(
                seams, {"pid": 4242, "start": 700, "handle": object()},
                {"pid": None}, seams.now_ns() + 120 * NS, {"cancelled": 15})
            assert_that(samples == [] and cancelled and "15" in cancelled,
                        "a caller's signal ends sampling without inventing a class")
        check("window: absolute deadline and caller cancellation", window_bounds)

        def custody_paths():
            base = base_script()
            base["exit_at"] = 10 ** 12
            base["guard_exit"] = 125
            base["wait_returns"] = 125
            report, code, seams = run_script(base)
            assert_that(seams.killed is True, "the observer kills only its own unreaped child")
            assert_that(report["custody"]["guard_killed_by_observer"] is True,
                        "the last-resort kill is recorded")
            assert_that(report["custody"]["guard_reaped"] is True,
                        "the killed child is positively reaped")
            assert_that(report["classification"]["outcome"] == OUTCOME_INCOMPLETE,
                        "a killed guard never yields an observation class")
            base = base_script()
            base["exit_at"] = 10 ** 12
            base["wait_returns"] = None
            report, code, _seams = run_script(base)
            assert_that(report["custody"]["guard_reaped"] is False,
                        "an unreaped child is recorded, never claimed")
            assert_that(any("could not be positively reaped" in reason
                            for reason in report["classification"]["reasons"]),
                        "unresolved custody is an explicit observation failure")
            base = base_script()
            base["process"] = {4242: {"pid": 4242, "start": 700, "zombie": False,
                                      "ppid": 1, "pgid": 4242, "uid": 501},
                               4243: {"pid": 4243, "start": 800, "zombie": False,
                                      "ppid": 4242, "pgid": 4243, "uid": 501},
                               4244: {"pid": 4244, "start": 801, "zombie": False,
                                      "ppid": 4242, "pgid": 4242, "uid": 501}}
            report, _code, _seams = run_script(base)
            assert_that(report["custody"]["monitor_observed_gone"] is True,
                        "a normal run observes the monitor gone")
        check("custody: own-child kill, positive reap and unknown custody", custody_paths)

        def shapes():
            assert_that(observed_shape({"late_rows": 1})[0] == SHAPE_INDETERMINATE,
                        "one late row cannot decide")
            assert_that(observed_shape({"late_rows": 2, "late_window_ns": NS // 2})[0]
                        == SHAPE_INDETERMINATE, "a short late window cannot decide")
            parked = {"late_rows": 3, "late_window_ns": 2 * NS, "late_cpu_delta_ns": 0,
                      "late_states": [3], "late_monotonic_cpu": True}
            assert_that(observed_shape(parked)[0] == SHAPE_BLOCKED, "a parked thread is named")
            running = dict(parked, late_states=[1])
            assert_that(observed_shape(running)[0] == SHAPE_INDETERMINATE,
                        "a zero-CPU running thread is not a parked call")
            slow = dict(parked, late_cpu_delta_ns=SLOW_CPU_NS, late_states=[1])
            assert_that(observed_shape(slow)[0] == SHAPE_SLOW, "CPU growth names a slow round")
            mixed = dict(parked, late_cpu_delta_ns=0, late_states=[1, 3])
            assert_that(observed_shape(mixed)[0] == SHAPE_INDETERMINATE,
                        "mixed run states cannot decide")
            nonmonotonic = dict(parked, late_monotonic_cpu=False)
            assert_that(observed_shape(nonmonotonic)[0] == SHAPE_INDETERMINATE,
                        "non-monotonic CPU is unreadable, never progress")
        check("shape: the four honest shape outcomes", shapes)

        def attribution_units():
            settlement = {"stopped_received_ns": 1_200_000_000,
                          "thread_start_ns": 1_200_100_000}
            baseline = {"at_ns": 1_000_000_000, "monitor_ok": True,
                        "threads": [[1, 0, 3, ""], [2, 0, 3, ""]]}
            after = {"at_ns": 1_300_000_000, "monitor_ok": True,
                     "threads": [[1, 0, 3, ""], [2, 0, 3, ""], [3, 0, 3, ""]]}
            result = attribute_census_thread([baseline, after], settlement)
            assert_that(result["state"] == "attributed" and result["tid"] == 3,
                        "one new thread is attributed")
            result = attribute_census_thread([after], settlement)
            assert_that(result["state"] == "unknown", "no baseline is never an attribution")
            result = attribute_census_thread([baseline], settlement)
            assert_that(result["state"] == "unknown", "no post-stop sample is never an attribution")
            two = dict(after, threads=baseline["threads"] + [[3, 0, 3, ""], [4, 0, 3, ""]])
            assert_that(attribute_census_thread([baseline, two], settlement)["state"] == "ambiguous",
                        "two new threads are ambiguous")
            missing = attribute_census_thread([baseline, after], {})
            assert_that(missing["state"] == "unknown", "a missing settlement is unknown")
        check("attribution: one new thread only, otherwise Unknown", attribution_units)

        def bounds():
            samples = [{"i": index, "at_ns": index, "table": 100 + index,
                        "load": [1.0, 2.0, 3.0]} for index in range(10)]
            assert_that(bounded_table(samples)["minimum"] == 100
                        and bounded_table(samples)["maximum"] == 109, "table counts are bounded")
            assert_that(bounded_load(samples)["maximum"] == [1.0, 2.0, 3.0],
                        "load metadata is bounded")
            assert_that(bounded_table([])["minimum"] is None, "no table sample is honest null")
            document = {"samples": [{"i": index, "blob": "x" * 5000} for index in range(80)],
                        "classification": {"reasons": ["a"] * 8}, "limitations": ["b"] * 50}
            size = bounded_report(document)
            assert_that(size <= REPORT_MAX_BYTES, "the report is bounded")
            assert_that(document["truncated"] is True and document["samples_omitted"] == 80,
                        "dropped samples are recorded, never silent")
        check("bounds: report trimming and count-only aggregates", bounds)

        def environment():
            child, removed = guard_environment({"OXIDE_SUPERVISOR_INJECT": "x",
                                                "OXIDE_TASK_SCRATCH_ROOT": "/tmp/adopted",
                                                "TMPDIR": "/tmp"}, "/publish", "token")
            assert_that(removed == ["OXIDE_SUPERVISOR_INJECT", "OXIDE_TASK_SCRATCH_ROOT"],
                        "both inherited controls are removed")
            assert_that(child["TMPDIR"] == "/tmp", "the run TMPDIR is preserved for the guard")
            assert_that(child[PUBLISH_DIR_VAR] == "/publish" and child[TOKEN_VAR] == "token",
                        "the leader receives only its own publication seam")
            assert_that("OXIDE_PROOF_RUN_ID" not in child,
                        "the observer never fabricates guard variables")
            assert_that(exit_for(OUTCOME_NOT_REPRODUCED) == 0
                        and exit_for(OUTCOME_REPRODUCED_DISCRIMINATED) == 0
                        and exit_for(OUTCOME_REPRODUCED_INSUFFICIENT) == 0
                        and exit_for(OUTCOME_INCOMPLETE) == 1, "exit codes match the classes")
        check("environment and exit-code mapping", environment)

        def leader_document():
            script = base_script()
            script["self_pid"] = 5000
            script["self_ppid"] = 5001
            script["files"] = {}
            script["process"] = {
                5000: {"pid": 5000, "start": 11, "zombie": False, "ppid": 5001,
                       "pgid": 5000, "uid": 501},
                5001: {"pid": 5001, "start": 12, "zombie": False, "ppid": 1,
                       "pgid": 5001, "uid": 501},
            }
            seams = FakeSeams(script)
            evidence = "/Volumes/Stash/scratch/oxide/oxide-proof.test/evidence"
            scratch = "/Volumes/Stash/scratch/oxide/oxide-proof.test/scratch"
            publish = os.path.join("/Volumes/Stash/scratch/oxide/3881b6d0/"
                                   "census-stall-observation.test", PUBLISH_DIR)
            env = {"OXIDE_PROOF_RUN_ID": "c" * 32, "OXIDE_PROOF_EVIDENCE": evidence,
                   "OXIDE_PROOF_DEADLINE_NS": str(600 * NS),
                   "OXIDE_TASK_SCRATCH_ROOT": scratch, PUBLISH_DIR_VAR: publish,
                   TOKEN_VAR: "0123456789abcdef", "TMPDIR": scratch}
            with contextlib.redirect_stdout(io.StringIO()), \
                    contextlib.redirect_stderr(io.StringIO()):
                code = leader_main(seams, env)
            path = os.path.join(publish, DOC_NAME)
            assert_that(code == 1, "the synthetic leader exits its declared status")
            assert_that(path in seams.written, "the identity document is published once")
            document = json.loads(seams.written[path].decode())
            assert_that(document["leader"]["pid"] == 5000
                        and document["leader"]["ppid"] == 5001
                        and document["leader"]["ppid_start"] == 12,
                        "the leader binds its own parent identity")
            assert_that(document["guard"]["evidence"] == evidence
                        and document["guard"]["run_id"] == "c" * 32,
                        "the leader records the guard's own layout")
            assert_that(validate_leader_document(document, "0123456789abcdef", 5001, 12) == [],
                        "the leader document validates")
            assert_that(validate_leader_document(document, "0" * 16, 5001, 12),
                        "a foreign token is refused")
            for name, mutate in (
                    ("status", lambda doc: doc["leader"].update(expected_status=2)),
                    ("sid", lambda doc: doc["leader"].update(sid=1)),
                    ("ppid", lambda doc: doc["leader"].update(ppid=1)),
                    ("ppid_start", lambda doc: doc["leader"].update(ppid_start=99)),
                    ("run", lambda doc: doc["guard"].update(run_id="x")),
                    ("path", lambda doc: doc["guard"].update(evidence="relative")),
                    ("deadline", lambda doc: doc["guard"].update(deadline_ns="soon"))):
                changed = json.loads(json.dumps(document))
                mutate(changed)
                assert_that(validate_leader_document(changed, "0123456789abcdef", 5001, 12),
                            f"leader {name}: refused")
            empty = FakeSeams(script)
            with contextlib.redirect_stdout(io.StringIO()), \
                    contextlib.redirect_stderr(io.StringIO()):
                assert_that(leader_main(empty, {}) == 2, "a stray leader invocation refuses")
                assert_that(not empty.written, "a refusal publishes nothing")
                asymmetric = dict(env)
                asymmetric["OXIDE_TASK_SCRATCH_ROOT"] = "/Volumes/Stash/elsewhere"
                assert_that(leader_main(FakeSeams(script), asymmetric) == 2,
                            "an asymmetric guard layout refuses")
        check("leader: publication, declared exit and every identity gate", leader_document)

        def plan():
            document = plan_document(FakeSeams(base_script()))
            assert_that(document["qualified"] is False and document["activated"] is False,
                        "the plan activates nothing")
            assert_that(len(document["premises"]) == 9, "all nine premise dispositions are present")
            assert_that(document["caps"]["sample_instants"] == MAX_SAMPLES
                        and document["caps"]["monitor_threads"] == MAX_MONITOR_THREADS,
                        "the plan states the fixed caps")
            assert_that(document["selectors"]["rust:census-stall-observation"].endswith("--observe"),
                        "the future occurrence selector is fixed")
            assert_that(document["source_problems"] == [], "the fake source gate is clean")
        check("plan: effect-free, fixed caps and nine premises", plan)

        # -- one real filesystem fixture, inside this owned temp root ------
        def filesystem_fixture():
            path = os.path.join(temporary, "bounded.json")
            data = b'{"ok": true}\n'
            write_atomic(path, data)
            assert_that(open(path, "rb").read() == data, "an atomic publication round-trips")
            document, size, digest = read_json(path, 4096)
            assert_that(document == {"ok": True} and size == len(data)
                        and digest == sha256_bytes(data), "the bounded reader agrees")
            try:
                read_json(path, 4)
            except ValueError:
                pass
            else:
                raise AssertionError("a document over its cap must refuse")
            assert_that(abs_canonical(path) and not abs_canonical(path + "/../x"),
                        "canonical path checks refuse a non-canonical spelling")
        check("filesystem: atomic publication and bounded reads", filesystem_fixture)

        return checks
    finally:
        import shutil
        shutil.rmtree(temporary, ignore_errors=True)


def sha256_file_plain(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        while True:
            chunk = handle.read(1024 * 1024)
            if not chunk:
                return digest.hexdigest()
            digest.update(chunk)


def summary_line(report):
    classification = report["classification"]
    custody = report["custody"]
    table = report.get("table") or {}
    return (f"census-stall-observation: {classification['outcome']}"
            f" (shape {classification.get('shape') or 'n/a'}; guard exit"
            f" {custody.get('guard_exit')}; stall fingerprint"
            f" {classification.get('stall_fingerprint')}; samples"
            f" {report.get('sample_count', 0)}; table"
            f" {table.get('minimum')}..{table.get('maximum')};"
            f" qualified=false)")


USAGE = ("usage: census-stall-observation.py [--observe|--self-test|--plan|--leader]")


def main(argv):
    arguments = list(argv)
    if not arguments:
        arguments = ["--plan"]
    if len(arguments) != 1 or arguments[0] not in ("--observe", "--self-test", "--plan",
                                                  "--leader"):
        print(USAGE, file=sys.stderr)
        return 2
    mode = arguments[0]
    if mode == "--plan":
        document = plan_document(RealSeams())
        print(json.dumps(document, indent=1, sort_keys=True))
        return 0
    if mode == "--self-test":
        checks = self_test()
        print(f"census-stall-observation: self-test passed {len(checks)} checks; "
              "no supervisor, child, Cargo, image or kernel process/thread query ran")
        return 0
    if mode == "--leader":
        return leader_main(RealSeams(), dict(os.environ))
    seams = RealSeams()
    control = {"cancelled": None}

    def cancelled(number, _frame):
        control["cancelled"] = number

    for number in (signal.SIGINT, signal.SIGTERM):
        try:
            signal.signal(number, cancelled)
        except (OSError, ValueError):
            pass
    report, code = run_observe(seams, dict(os.environ), control)
    classification = report["classification"]
    if code == 2 and not (report.get("files") or {}).get("root"):
        for reason in classification.get("reasons") or ["refused"]:
            print(f"census-stall-observation: refused: {reason}", file=sys.stderr)
        return 2
    if report.get("files", {}).get("report"):
        print(f"census-stall-observation: report {report['files']['report']}")
    print(summary_line(report))
    for reason in classification.get("reasons") or []:
        print(f"census-stall-observation: {reason}", file=sys.stderr)
    return code


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except SystemExit:
        raise
    except BaseException as error:
        print(f"census-stall-observation: internal error ({type_name(error)}); refused", file=sys.stderr)
        sys.exit(2)
