#!/usr/bin/env python3
"""Retained-evidence fixtures for held-child custody (oxide#237, slice B).

One fixed script and the whole command of the qualified shared supervisor
(`bounded-test.py --limit 300 --scratch-budget-mib 512 -- python3
scripts/test-cargo-group-custody.py`): 300 s wall clock, 512 MiB monitored
scratch, no Cargo, no toolchain, no image, no mount, no live pool and no
account-home read. It implements slice B of the frozen retention design in
[proof-supervisor-correction](docs/knowledge/contracts/proof-supervisor-correction.md)
(Retention redesign 2026-11-02): the fixture runs as the guard's proof leader,
validates the guard's already-published default selected-volume owned-root
receipt and topology, loads and hashes the exact qualified runner bytes, uses
the runner's own `protect_nested_scope` to make the run root `hosting` BEFORE
any fixture resource exists, and then creates one private
`evidence/cargo-group-custody.<random>/` directory under the held evidence
descriptor. Every fixture writer target (HOME, cwd, temp, inputs, outputs,
control files, receipts) lives inside that directory — never in the guard's
auto-cleaned scratch.

What it proves, natively, on its OWN controlled direct children only:

- a fixture child creates its own session and process group (`setsid` before
  exec), and the parent binds that identity (PID, PGID == PID, SID == PID,
  `kinfo_proc` start token, PPID == this leader) while the child is live;
- a session leader cannot leave its group (`setpgid` escape attempts fail
  `EPERM`; a second `setsid` fails `EPERM`), verified from the child's report
  and the parent's own observations;
- exclusive unreaped custody: the parent reaps nothing implicitly —
  `SIGCHLD` stays `SIG_DFL` (a `SIG_IGN` counterfactual demo child shows the
  kernel auto-reaping a grandchild, `ECHILD`, so no custody is possible);
  `waitpid(WNOHANG)` while the child is live collects nothing; a second
  `waitpid` after the single positive reap fails `ECHILD` (a wait cannot be
  undone and a concurrent wait is refused);
- zombie lifetime: an exited child stays a live group member while held
  unreaped — `kill(pid, 0)` and `killpg(pgid, 0)` persist over a bounded
  observation window and the `kinfo_proc` row keeps the same
  (PID, start) identity with its zombie flag set; the group ends only at the
  positive reap;
- the prospective cooperative stop mechanics on a fixture-owned child: a
  bounded `killpg(pgid, SIGTERM)` (escalating to `SIGKILL`) is issued ONLY
  while the holder positively holds that unreaped direct child, and the child
  is positively reaped afterwards;
- refusal after lost custody: once the custody state is settled the authority
  model refuses any further signal or reap without a syscall, and zero-target
  probes (`kill(0, 0)`, `killpg(0, 0)`) are demonstrated vacuous — they answer
  against the caller's own scope and carry no information about the held
  identity, so observations are not signal handles.

The native receipt binds the supervisor run token and the marker run token
separately (they are never equated), the exact root/evidence/fixture device
and inode identities, the protection readback, parent/child/group held
intervals, every attempted or refused signal, the actual direct-child reap and
the retained root. It distinguishes `outer` (the guard's own closure, which
this receipt never claims) from `nested` (this fixture's custody facts). A
missing, partial or unwritable nested receipt aborts later cases. The JSON is
bounded to 64 KiB with at most 32 case rows, 32 signal/refusal rows and 64
identity/transition rows; actual totals and omitted counts are reported and a
truncated row never turns custody uncertainty into success.

Held, source/pure-only (never executed natively here): actual parent death,
an owner-unresponsive holder, and a descendant that escapes the held group via
`setpgid(0, 0)` — a non-leader descendant CAN leave, so holding the leader
does not contain descendants, and no bounded no-new-authority
containment/lifetime argument has been accepted for those cases yet. No
historical or live survivor is looked up or signalled; no observed non-child
PID is ever a signal authority; `killpg(..., 0)` probes, kqueue events and
PID/start/session checks are observations, never handles. This script never
concludes the hosting root, never sweeps, never cleans and never restores:
even on success the root stays `hosting` and unconcluded, and its retained
evidence directory keeps every receipt.

Modes:
- bare invocation (counted, fail-fast) under the qualified supervisor;
- `--diagnostic`: the same guard, root and limits, uncounted keep-going — it
  continues only into dependency-independent cases after a recoverable case
  failure, marks dependent cases skipped, aborts on infrastructure, device,
  writer, custody or receipt uncertainty and always writes a bounded
  `qualified: false` receipt. It is never a counted PASS.
- `--plan`: effect-free case table, bounds, premise-checklist dispositions
  and refusal premises; `--self-check`: the pure apparatus check, which
  stages nothing, starts no child, and writes only inside one private
  temporary directory under `TMPDIR` that it removes before returning.

Native execution is gated: the direct-child/group lifetime premises and this
exact source need the retained Queue reviewer's exact-head source/premise
approval first, and the one counted diagnostic runs through the task's proof
route, never by the author or reviewer. Fit is unmeasured until that run.
"""

from __future__ import annotations

import ast
import ctypes
import errno
import hashlib
import json
import os
import secrets
import signal
import stat
import subprocess
import sys
import tempfile
import time
import types

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPT_PATH = os.path.abspath(__file__)
RUNNER_PATH = os.path.join(REPO, "scripts", "bounded-test.py")
# The qualified shared supervisor bytes (the integrated oxide#195 custody
# hardening over oxide#197's ancestor rules, oxide#192's admission reports and
# oxide#191's opt-in post-census detach; workflow.md records the qualification
# chain). Missing or changed bytes refuse before any effect; a mutable file is
# never loaded after hashing.
EXPECTED_RUNNER_SHA256 = "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b"
GUARD_LIMIT_S = 300
GUARD_BUDGET_MIB = 512
COUNTED_COMMAND = (
    "python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 -- "
    "python3 scripts/test-cargo-group-custody.py"
)
DIAGNOSTIC_COMMAND = COUNTED_COMMAND + " --diagnostic"

FIXTURE_PREFIX = "cargo-group-custody."
RECEIPT_NAME = "receipt.json"
CHILDREN_DIR = "children"
CHILD_LIFETIME_S = 12          # every fixture child's own alarm bound
CHILD_STOP_POLL_S = 0.02
READINESS_WAIT_S = 3.0
SESSION_WAIT_S = 2.0
ZOMBIE_WINDOW_S = 1.5
ZOMBIE_SAMPLES = 5
LIVE_HOLD_S = 1.2
TERM_WINDOW_S = 3.0
CASE_CAP_S = 30.0
DEADLINE_MARGIN_S = 30.0       # stop launching work this long before the guard deadline
SETTLE_BUDGET_S = 2.0

RECEIPT_MAX_BYTES = 65536
CASE_ROWS_MAX = 32
SIGNAL_ROWS_MAX = 32
IDENTITY_ROWS_MAX = 64
DETAIL_CAP = 200
REPORT_MAX_BYTES = 8192
LOG_MAX_BYTES = 16384

GUARD_PHASE_STARTED = "started: incomplete"
RECEIPT_REQUIRED_KEYS = frozenset({
    "run_id", "root", "scratch", "evidence", "scratch_owned",
    "scratch_default", "scratch_marker", "phase", "command", "limit_s",
})

CASE_STATUS_COMPLETED = "completed"
CASE_STATUS_FAILED = "failed"
CASE_STATUS_ABORTED = "aborted"
CASE_STATUS_UNEXECUTED = "unexecuted"
CASE_STATUS_SKIPPED = "skipped-dependent"
CASE_STATUS_SOURCE_ONLY = "source-only"

HEX = "0123456789abcdef"


# ---------------------------------------------------------------------------
# small helpers


def say(message):
    """One bounded progress line on the leader's stdout (the guard drains it)."""
    print(message[:400], flush=True)


def now_ns():
    return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)  # the guard's one clock


def type_name(error):
    return type(error).__name__


def is_hex_token(value):
    return isinstance(value, str) and len(value) == 32 and all(c in HEX for c in value)


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def sha256_file(path):
    digest = hashlib.sha256()
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        while True:
            chunk = os.read(fd, 262144)
            if not chunk:
                break
            digest.update(chunk)
    finally:
        os.close(fd)
    return digest.hexdigest()


class Refused(Exception):
    """A validation refusal before any fixture resource exists."""


class CaseFailure(Exception):
    """A recoverable, case-local oracle failure."""


class Uncertain(Exception):
    """Infrastructure, device, writer, custody or receipt uncertainty."""


class CustodyLost(Exception):
    """The authority model's refusal after custody ended (no syscall made)."""


# ---------------------------------------------------------------------------
# qualified runner bytes


def read_exact_verified(fd, expected_size):
    """Read exactly `expected_size` bytes from an already-fstat'ed descriptor.

    A short read (the file shrank between the size probe and the data read) or
    an over-long file is a change mid-read and refuses — the bytes hashed are
    always exactly the bytes stat'ed."""
    chunks = []
    remaining = expected_size
    while remaining > 0:
        chunk = os.read(fd, min(262144, remaining))
        if not chunk:
            raise Refused("runner bytes changed mid-read (short read)")
        chunks.append(chunk)
        remaining -= len(chunk)
    if os.read(fd, 1):
        raise Refused("runner bytes changed mid-read (grew after the stat)")
    return b"".join(chunks)


def load_runner_from(path, read_all=read_exact_verified):
    """Hash and load the exact qualified runner bytes.

    Read no-follow with a stable-identity double check, verify the pinned
    SHA-256, then compile and exec THOSE verified bytes with bytecode writing
    disabled. There is no path switch and no import of a mutable name: the
    module namespace is built from the exact bytes that were hashed, so a
    changed file can never be loaded after hashing."""
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except OSError as error:
        raise Refused(f"qualified runner {path} cannot be opened no-follow ({type_name(error)})")
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode):
            raise Refused(f"qualified runner {path} is not a regular file")
        data = read_all(fd, before.st_size)
        after = os.fstat(fd)
    finally:
        os.close(fd)
    for field in ("st_dev", "st_ino", "st_size", "st_mtime_ns"):
        if getattr(before, field) != getattr(after, field):
            raise Refused(f"qualified runner {path} changed while reading ({field})")
    digest = sha256_bytes(data)
    if digest != EXPECTED_RUNNER_SHA256:
        raise Refused(f"qualified runner {path} is {digest}, not the pinned {EXPECTED_RUNNER_SHA256}")
    bytecode_was_disabled = sys.dont_write_bytecode
    sys.dont_write_bytecode = True
    try:
        module = types.ModuleType("oxide_qualified_supervisor")
        module.__dict__["__file__"] = path
        code = compile(data, path, "exec")
        exec(code, module.__dict__)  # the exact verified bytes only
    finally:
        sys.dont_write_bytecode = bytecode_was_disabled
    for attribute in ("protect_nested_scope", "read_marker_with_identity",
                      "MarkerInvalid", "DefaultProbe", "Kinfo", "MARKER_NAME",
                      "SweepBudget", "sweep_one_root", "record_conclusion"):
        if not hasattr(module, attribute):
            raise Refused(f"the qualified runner is missing {attribute}")
    return module, {"path": path, "sha256": digest, "bytes": len(data),
                    "bytecode_disabled_during_load": True,
                    "loaded_from_verified_bytes": True}


# ---------------------------------------------------------------------------
# guard receipt / topology validation (all before any fixture resource)


def open_dir_no_follow(path):
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)
    except OSError as error:
        raise Refused(f"{path} is not an openable real directory ({type_name(error)})")
    return fd


def read_exact_file(path, limit):
    """One bounded no-follow regular-file read with a stable-identity check."""
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except OSError as error:
        raise Refused(f"{path} cannot be opened no-follow ({type_name(error)})")
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode):
            raise Refused(f"{path} is not a regular file")
        if before.st_size > limit:
            raise Refused(f"{path} is larger than {limit} bytes")
        data = read_exact_verified(fd, before.st_size)
        after = os.fstat(fd)
    finally:
        os.close(fd)
    if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != \
       (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
        raise Refused(f"{path} changed while reading")
    return data


def identity_of(path, fd=None):
    info = os.fstat(fd) if fd is not None else os.lstat(path)
    return {"path": path, "dev": info.st_dev, "ino": info.st_ino,
            "mode": stat.S_IMODE(info.st_mode), "uid": info.st_uid}


def check_dir(directory, fd, *, private_exact, device, label):
    """One held directory descriptor: real directory, owned, mode, device."""
    info = os.fstat(fd)
    if not stat.S_ISDIR(info.st_mode):
        raise Refused(f"{label} {directory} is not a directory")
    if info.st_uid != os.geteuid():
        raise Refused(f"{label} {directory} is not owned by this user")
    if private_exact:
        if stat.S_IMODE(info.st_mode) & 0o077:
            raise Refused(f"{label} {directory} is not private (mode {oct(info.st_mode)})")
    else:
        if stat.S_IMODE(info.st_mode) & 0o022:
            raise Refused(f"{label} {directory} is group/other writable (mode {oct(info.st_mode)})")
    if device is not None and info.st_dev != device:
        raise Refused(f"{label} {directory} is not on the selected device")
    if os.path.realpath(directory) != directory:
        raise Refused(f"{label} {directory} is not canonical")


def read_guard_receipt(evidence_path):
    """The guard's already-published receipt, bounded and no-follow."""
    path = os.path.join(evidence_path, "receipt.json")
    data = read_exact_file(path, 2 * 1024 * 1024)
    try:
        document = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, ValueError):
        raise Refused("the guard receipt is not JSON")
    if not isinstance(document, dict) or not RECEIPT_REQUIRED_KEYS.issubset(document):
        raise Refused("the guard receipt schema is missing required fields")
    info = os.lstat(path)
    if info.st_mode & 0o022:
        raise Refused("the guard receipt is group/other writable")
    return document


def validate_context(env):
    """Validate the guard's published facts and the owned-root topology.

    This refuses adopted scratch, an explicit `--root-parent`, explicit
    `--evidence`, the fallback parent, unmarked or stale roots, separate-token
    confusion, aliases, mount crossings, and any ownership/mode/device fault —
    all before any fixture resource or child exists. Same-UID hostile
    replacement is outside the cooperative boundary."""
    for name in ("OXIDE_PROOF_RUN_ID", "OXIDE_PROOF_EVIDENCE",
                 "OXIDE_PROOF_DEADLINE_NS", "OXIDE_TASK_SCRATCH_ROOT"):
        value = env.get(name)
        if not value:
            raise Refused(f"{name} is missing: this script runs only as the guard's proof leader")
    supervisor_token = env["OXIDE_PROOF_RUN_ID"]
    if not is_hex_token(supervisor_token):
        raise Refused("OXIDE_PROOF_RUN_ID is not a 32-hex token")
    try:
        deadline_ns = int(env["OXIDE_PROOF_DEADLINE_NS"])
    except ValueError:
        raise Refused("OXIDE_PROOF_DEADLINE_NS is not an integer")
    if deadline_ns <= now_ns():
        raise Refused("the exported proof deadline is already past")

    scratch = env["OXIDE_TASK_SCRATCH_ROOT"]
    evidence = env["OXIDE_PROOF_EVIDENCE"]
    root = os.path.dirname(scratch)
    if os.path.basename(scratch) != "scratch":
        raise Refused(f"scratch {scratch} is not a `<root>/scratch` child of an owned root")
    receipt = read_guard_receipt(evidence)
    if receipt["phase"] != GUARD_PHASE_STARTED:
        raise Refused(f"the guard receipt phase is {receipt['phase']!r}, not {GUARD_PHASE_STARTED!r}")
    if receipt["run_id"] != supervisor_token:
        raise Refused("the guard receipt run token is not this run's token")
    if receipt["scratch_owned"] is not True:
        raise Refused("the guard scratch is not an owned root (adopted scratch refuses)")
    selection = receipt["scratch_default"]
    if not isinstance(selection, dict) or selection.get("source") != "selected-volume":
        raise Refused(f"scratch selection source is "
                      f"{selection.get('source') if isinstance(selection, dict) else '?'}: "
                      "only the default selected-volume owned root qualifies")
    for field in ("mount", "root_parent", "prepared_parent"):
        if not isinstance(selection.get(field), str) or not selection[field]:
            raise Refused(f"scratch selection {field} is missing")
    if not isinstance(selection.get("device"), int) or isinstance(selection.get("device"), bool):
        raise Refused("scratch selection device is missing")
    if not isinstance(selection.get("volume_uuid"), str) or not selection["volume_uuid"]:
        raise Refused("scratch selection volume_uuid is missing")
    mount, root_parent, prepared_parent = (selection["mount"], selection["root_parent"],
                                           selection["prepared_parent"])
    if root_parent != os.path.join(mount, "scratch", "oxide"):
        raise Refused(f"root parent {root_parent} is not the private <mount>/scratch/oxide subtree")
    if prepared_parent != root_parent:
        raise Refused("the prepared parent is not the selection's root parent")
    if os.path.dirname(root) != root_parent:
        raise Refused(f"root {root} is not a direct child of the prepared parent {root_parent}")
    if receipt["root"] != root or receipt["scratch"] != scratch or receipt["evidence"] != evidence:
        raise Refused("the guard receipt's root/scratch/evidence paths do not match this run's environment")
    if evidence != os.path.join(root, "evidence"):
        raise Refused(f"evidence {evidence} is not the owned root's own evidence child "
                      "(an explicit --evidence refuses)")
    marker_receipt = receipt["scratch_marker"]
    if not isinstance(marker_receipt, dict) or marker_receipt.get("state") != "written" \
            or not is_hex_token(marker_receipt.get("run_id")):
        raise Refused("the guard receipt does not record a written scratch marker")
    marker_token = marker_receipt["run_id"]
    if marker_receipt.get("path") != os.path.join(root, ".oxide-scratch.json"):
        raise Refused("the receipt's marker path is not the root's own marker")
    if marker_token == supervisor_token:
        raise Refused("the marker run token equals the supervisor run token; they are separate tokens")

    device = selection["device"]
    root_fd = open_dir_no_follow(root)
    try:
        check_dir(root, root_fd, private_exact=True, device=device, label="root")
        parent_fd = open_dir_no_follow(root_parent)
        try:
            check_dir(root_parent, parent_fd, private_exact=True, device=device, label="parent")
            scratch_fd = open_dir_no_follow(scratch)
            try:
                check_dir(scratch, scratch_fd, private_exact=False, device=device, label="scratch")
                evidence_fd = open_dir_no_follow(evidence)
                try:
                    check_dir(evidence, evidence_fd, private_exact=False, device=device, label="evidence")
                    for name, fd in (("scratch", scratch_fd), ("evidence", evidence_fd)):
                        direct = os.stat(name, dir_fd=root_fd, follow_symlinks=False)
                        held = os.fstat(fd)
                        if (direct.st_dev, direct.st_ino) != (held.st_dev, held.st_ino):
                            raise Refused(f"{name} is not the root's own direct child by descriptor")
                    if os.fstat(scratch_fd).st_ino == os.fstat(evidence_fd).st_ino:
                        raise Refused("scratch and evidence are the same directory")
                    if os.path.ismount(scratch) or os.path.ismount(evidence) or os.path.ismount(root):
                        raise Refused("a mount crossing inside the owned root")
                finally:
                    os.close(evidence_fd)
            finally:
                os.close(scratch_fd)
        finally:
            os.close(parent_fd)
    except BaseException:
        os.close(root_fd)
        raise
    # root_fd stays open: it is the held root descriptor for the protection step.
    return {
        "supervisor_token": supervisor_token,
        "marker_token": marker_token,
        "deadline_ns": deadline_ns,
        "root": root, "root_fd": root_fd,
        "scratch": scratch, "evidence": evidence,
        "mount": mount, "device": device, "volume_uuid": selection["volume_uuid"],
        "selection": selection,
        "guard_phase_observed": receipt["phase"],
    }


def strict_marker_facts(module, ctx):
    """The strict marker reread, bound to the held root descriptor identity."""
    root_info = os.fstat(ctx["root_fd"])
    try:
        document, _marker_identity = module.read_marker_with_identity(ctx["root"], module.DefaultProbe())
    except module.MarkerInvalid as invalid:
        raise Refused(f"the scratch marker is not strict-valid: {invalid.code}")
    if document["run_id"] != ctx["marker_token"]:
        raise Refused("the marker run token is not the receipt's marker token")
    if document["root"] != ctx["root"] or document["device"] != root_info.st_dev \
            or document["inode"] != root_info.st_ino or document["owner_uid"] != os.geteuid():
        raise Refused("the marker identity does not bind the held root")
    if document["outcome"] != "running" or document["closure"] != "none" \
            or document["kept_reason"] != "none" or document["concluded_unix"] is not None:
        raise Refused("the marker is not an unconcluded running root")
    return document


def protect_and_verify(module, ctx, trace):
    """Protect the root `hosting` through the runner's own function, then
    independently reread and rebind — before any fixture resource exists."""
    strict_marker_facts(module, ctx)  # the initial, unhosted, unconcluded marker
    trace.append("initial-marker-verified")
    try:
        module.protect_nested_scope(ctx["root"], module.DefaultProbe())
    except module.MarkerInvalid as invalid:
        raise Refused(f"protect_nested_scope refused: {invalid.code}")
    trace.append("protected")
    document = strict_marker_facts(module, ctx)
    if document["nested_scope"] != "hosting":
        raise Refused(f"the marker reread is {document['nested_scope']!r}, not hosting")
    trace.append("hosting-readback-verified")
    held = os.fstat(ctx["root_fd"])
    current = os.stat(ctx["root"], follow_symlinks=False)
    if (held.st_dev, held.st_ino, held.st_mode) != (current.st_dev, current.st_ino, current.st_mode):
        raise Refused("the held root descriptor no longer matches the root path")
    evidence_fd = open_dir_no_follow(ctx["evidence"])
    try:
        check_dir(ctx["evidence"], evidence_fd, private_exact=False,
                  device=ctx["device"], label="evidence")
    finally:
        os.close(evidence_fd)
    trace.append("descriptors-revalidated")
    return document


def create_fixture_dir(ctx, trace):
    """Exclusively create the private fixture directory under the held
    evidence descriptor and hold its own descriptor."""
    name = FIXTURE_PREFIX + secrets.token_hex(8)
    evidence_fd = open_dir_no_follow(ctx["evidence"])
    fd = None
    try:
        try:
            os.mkdir(name, 0o700, dir_fd=evidence_fd)
        except OSError as error:
            raise Refused(f"the fixture directory cannot be created exclusively ({type_name(error)})")
        trace.append("fixture-dir-created")
        try:
            fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY, dir_fd=evidence_fd)
        except OSError as error:
            raise Refused(f"the fixture directory cannot be opened no-follow ({type_name(error)})")
        try:
            info = os.fstat(fd)
            if not stat.S_ISDIR(info.st_mode) or stat.S_IMODE(info.st_mode) & 0o077 \
                    or info.st_uid != os.geteuid() or info.st_dev != ctx["device"]:
                raise Refused("the fixture directory is not a private directory on the selected device")
            direct = os.stat(name, dir_fd=evidence_fd, follow_symlinks=False)
            if (direct.st_dev, direct.st_ino) != (info.st_dev, info.st_ino):
                raise Refused("the fixture directory was replaced before it could be held")
        except BaseException:
            os.close(fd)
            fd = None
            raise
    finally:
        # A partially created or unheld fixture directory is KEPT as retained
        # evidence (the interruption table: keep the protected root and any
        # partial evidence); only this helper's descriptors close here.
        os.close(evidence_fd)
    fixture_path = os.path.join(ctx["evidence"], name)
    try:
        for leaf in (CHILDREN_DIR, "home", "tmp"):
            os.mkdir(leaf, 0o700, dir_fd=fd)
    except BaseException:
        os.close(fd)  # the directory and its partial leaves stay as evidence
        raise
    return {"name": name, "path": fixture_path, "fd": fd,
            "children_path": os.path.join(fixture_path, CHILDREN_DIR),
            "identity": identity_of(fixture_path, fd)}


def establish_hosting(env, trace):
    """The whole pre-child sequence: validate, load, protect, verify, create.

    The trace records the exact order; the self-check asserts that protection
    and readback strictly precede the fixture directory, which precedes any
    child. A refusal after validation closes the held root descriptor and
    leaves no fixture resource."""
    ctx = validate_context(env)
    trace.append("context-validated")
    module, facts = load_runner_from(RUNNER_PATH)
    ctx["runner"] = module
    ctx["runner_facts"] = facts
    trace.append("runner-loaded")
    try:
        strict_marker_facts(module, ctx)
        protect_and_verify(module, ctx, trace)
        fixture = create_fixture_dir(ctx, trace)
    except BaseException:
        try:
            os.close(ctx["root_fd"])
        except OSError:
            pass
        raise
    ctx["fixture"] = fixture
    return ctx


# Darwin sigaction facts: SA_NOCLDWAIT and the SIG_DFL/SIG_IGN handler values.
SA_NOCLDWAIT = 0x0002
SIG_DFL_VALUE = 0
SIG_IGN_VALUE = 1


class _SigAction(ctypes.Structure):
    """struct sigaction on darwin/arm64+armv7: the handler union at 0, the
    4-byte signal set, then flags."""

    _fields_ = [("sa_handler", ctypes.c_void_p), ("sa_mask", ctypes.c_uint32),
                ("sa_flags", ctypes.c_int)]


def sigchld_disposition():
    """The real kernel SIGCHLD disposition of this process, via sigaction(2).

    Python's `signal.getsignal` reads its own table and cannot be trusted for
    a disposition this process never touched, so the exclusive-unreaped
    premise reads the actual kernel fact: (handler, flags) or None when the
    call is unsupported on this platform."""
    if sys.platform != "darwin":
        return None
    try:
        libc = ctypes.CDLL(None, use_errno=True)
        old = _SigAction()
        if libc.sigaction(signal.SIGCHLD, None, ctypes.byref(old)) != 0:
            return None
        return {"handler": old.sa_handler, "flags": old.sa_flags}
    except (OSError, AttributeError, ValueError):
        return None


# ---------------------------------------------------------------------------
# child environment


def child_environment(inherited, fixture_dir):
    """The only environment a fixture child ever sees.

    Every inherited account, pool, test, XDG, Cargo, sccache and rust* control
    is dropped, and temp and HOME are redirected into the fixture directory —
    never the guard's auto-cleaned scratch, never the host."""
    keep = {}
    path_value = inherited.get("PATH")
    if path_value:
        keep["PATH"] = path_value
    keep["HOME"] = os.path.join(fixture_dir, "home")
    tmp = os.path.join(fixture_dir, "tmp")
    keep["TMPDIR"] = keep["TMP"] = keep["TEMP"] = tmp
    keep["PYTHONDONTWRITEBYTECODE"] = "1"
    return keep


def is_scrubbed(env, fixture_dir):
    for name in env:
        if name.startswith(("OXIDE_", "XDG_", "CARGO_", "SCCACHE_", "RUST")):
            return False
    return (env.get("HOME") == os.path.join(fixture_dir, "home")
            and env.get("TMPDIR") == os.path.join(fixture_dir, "tmp"))


# ---------------------------------------------------------------------------
# fixture child program (exec'd from a string; writes only inside the fixture)


def child_program(role, report_fd, stop_path, parent_pgid, absent_pgid):
    """The bounded program of one fixture child. It verifies its own session
    and group, reports through the pre-created descriptor, watches the stop
    file, and is killed by its own alarm if its owner never returns."""
    return (
        "import json, os, signal, time\n"
        f"REPORT_FD = {report_fd}\n"
        f"STOP_PATH = {stop_path!r}\n"
        f"signal.alarm({CHILD_LIFETIME_S})\n"
        "def write_report(document):\n"
        "    document['pid'] = os.getpid()\n"
        "    document['pgid_self'] = os.getpgid(0)\n"
        "    document['sid_self'] = os.getsid(0)\n"
        "    payload = json.dumps(document).encode()\n"
        "    if len(payload) > 4096:\n"
        "        os._exit(97)\n"
        "    written = 0\n"
        "    while written < len(payload):\n"
        "        written += os.write(REPORT_FD, payload[written:])\n"
        f"ROLE = {role!r}\n"
        "identity = {'pgid_is_pid': os.getpgid(0) == os.getpid(),\n"
        "            'sid_is_pid': os.getsid(0) == os.getpid()}\n"
        "if ROLE == 'hold':\n"
        "    attempts = []\n"
        "    escaped = False\n"
        "    for label, target in (('setpgid-parent-group', %d), ('setpgid-absent-group', %d)):\n"
        "        try:\n"
        "            os.setpgid(0, target)\n"
        "            attempts.append([label, 'ok'])\n"
        "            escaped = True\n"
        "            try:\n"
        "                os.setsid()\n"  # defensive re-isolation if an escape ever succeeded
        "            except OSError:\n"
        "                pass\n"
        "        except OSError as error:\n"
        "            attempts.append([label, error.errno])\n"
        "    try:\n"
        "        os.setsid()\n"
        "        attempts.append(['setsid-again', 'ok'])\n"
        "    except OSError as error:\n"
        "        attempts.append(['setsid-again', error.errno])\n"
        "    write_report({'role': ROLE, 'identity': identity,\n"
        "                  'attempts': attempts, 'escaped': escaped})\n"
        "    while not os.path.exists(STOP_PATH):\n"
        "        time.sleep(%s)\n"
        "    os._exit(0)\n"
        "if ROLE == 'self-exit':\n"
        "    write_report({'role': ROLE, 'identity': identity})\n"
        "    time.sleep(0.3)\n"
        "    os._exit(0)\n"
        "if ROLE == 'term-target':\n"
        "    write_report({'role': ROLE, 'identity': identity})\n"
        "    while not os.path.exists(STOP_PATH):\n"
        "        time.sleep(%s)\n"
        "    os._exit(0)\n"
        "if ROLE == 'demo-ignore':\n"
        "    signal.signal(signal.SIGCHLD, signal.SIG_IGN)\n"
        "    grandchild = os.fork()\n"
        "    if grandchild == 0:\n"
        "        time.sleep(0.3)\n"
        "        os._exit(0)\n"
        "    try:\n"
        "        pid, status = os.waitpid(grandchild, 0)\n"
        "        waited = 'reaped:%%d' %% pid\n"
        "    except OSError as error:\n"
        "        waited = 'errno:%%d' %% error.errno\n"
        "    try:\n"
        "        os.kill(grandchild, 0)\n"
        "        probe = 'alive'\n"
        "    except OSError as error:\n"
        "        probe = 'errno:%%d' %% error.errno\n"
        "    write_report({'role': ROLE, 'identity': identity, 'grandchild': grandchild,\n"
        "                  'wait': waited, 'probe': probe})\n"
        "    os._exit(0)\n"
        "os._exit(96)\n" % (parent_pgid, absent_pgid, CHILD_STOP_POLL_S, CHILD_STOP_POLL_S)
    )


# ---------------------------------------------------------------------------
# custody model


def _bounded_row(row, cap):
    bounded = {}
    for key, value in row.items():
        if isinstance(value, str) and len(value) > cap:
            bounded[key] = value[:cap]
        else:
            bounded[key] = value
    return bounded


class NestedReceipt:
    """The bounded nested receipt, republished atomically after every
    transition. Rows are capped; totals and omitted counts stay truthful, so a
    capped row never turns custody uncertainty into success. Atomic
    replacement is not power-loss durability: that cooperative-host limitation
    is preserved, not repaired."""

    def __init__(self, fixture_fd, base):
        self.fd = fixture_fd
        self.document = dict(base)
        self.write_error = None

    def add_case(self, row):
        rows = self.document.setdefault("cases", [])
        self.document["totals"]["cases_total"] += 1
        if len(rows) < CASE_ROWS_MAX:
            rows.append(_bounded_row(row, DETAIL_CAP))
        else:
            self.document["totals"]["cases_omitted"] += 1
        self._publish_transition()

    def add_signal(self, row):
        rows = self.document.setdefault("signals", [])
        self.document["totals"]["signals_total"] += 1
        if len(rows) < SIGNAL_ROWS_MAX:
            rows.append(_bounded_row(row, DETAIL_CAP))
        else:
            self.document["totals"]["signals_omitted"] += 1
        self._publish_transition()

    def add_identity(self, row):
        row = dict(row)
        row.setdefault("at_ns", now_ns())
        rows = self.document.setdefault("identities", [])
        self.document["totals"]["identities_total"] += 1
        if len(rows) < IDENTITY_ROWS_MAX:
            rows.append(_bounded_row(row, DETAIL_CAP))
        else:
            self.document["totals"]["identities_omitted"] += 1
        self._publish_transition()

    def _publish_transition(self):
        if self.write_error is not None or not self.publish():
            raise Uncertain(f"nested receipt publication failed: {self.write_error}")

    def publish(self):
        try:
            payload = json.dumps(self.document, sort_keys=True).encode("utf-8")
            if len(payload) > RECEIPT_MAX_BYTES:
                # Even the capped shape must stay bounded: drop optional prose
                # before ever touching custody obligations.
                self.document.pop("plan_notes", None)
                self.document["notes"] = ["receipt exceeded the byte bound; prose dropped"]
                payload = json.dumps(self.document, sort_keys=True).encode("utf-8")
            if len(payload) > RECEIPT_MAX_BYTES:
                self.write_error = "receipt over the 64 KiB bound even after prose drop"
                return False
            tmp = RECEIPT_NAME + ".tmp"
            fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW,
                         0o600, dir_fd=self.fd)
            try:
                written = 0
                while written < len(payload):
                    written += os.write(fd, payload[written:])
            finally:
                os.close(fd)
            os.replace(tmp, RECEIPT_NAME, src_dir_fd=self.fd, dst_dir_fd=self.fd)
            return True
        except OSError as error:
            self.write_error = f"{type_name(error)}: {error}"
            return False


def derive_nested_outcome(children, cases, receipt_ok):
    """The pure nested-outcome mapping.

    EOF, a nonzero command exit, an absent observed PID or the outer guard's
    own closure never turn nested unknown into closed. Nested closure means:
    every owned child positively reaped, every declared native case completed,
    and the receipt published complete — and even then the hosting root stays
    unconcluded and retained."""
    native_cases = [case for case in cases if case.get("kind") == "native"]
    all_completed = bool(native_cases) and all(
        case["status"] == CASE_STATUS_COMPLETED for case in native_cases)
    all_reaped = bool(children) and all(child.get("reaped") is True for child in children)
    if receipt_ok and all_completed and all_reaped:
        return {"outcome": "nested-closed",
                "writer_closure": "positively-reaped-all-owned-children"}
    if receipt_ok and all_reaped and all_completed:
        return {"outcome": "unknown", "writer_closure": "positively-reaped-all-owned-children"}
    return {"outcome": "unknown", "writer_closure": "unknown"}


class HeldChild:
    """One owned, unreaped, direct fixture child under the authority model.

    `probes` receives every real syscall this model makes, so the self-check
    and the reviewer can see that a refusal makes none. Zero-target and sig-0
    probes are recorded as observations; PID/start/session checks and
    `killpg(..., 0)` are never signal handles."""

    def __init__(self, module, receipt, case_name, role, pid, report_path, log_path, stop_path):
        self.module = module
        self.receipt = receipt
        self.case = case_name
        self.role = role
        self.pid = pid
        self.report_path = report_path
        self.log_path = log_path
        self.stop_path = stop_path
        self.state = "held"
        self.pgid = None
        self.sid = None
        self.start = None
        self.exit_status = None
        self.reaped = False
        self.probes = []

    def _identity_row(self, transition, extra=None):
        row = {"case": self.case, "role": self.role, "pid": self.pid,
               "pgid": self.pgid, "sid": self.sid, "start": self.start,
               "state": self.state, "transition": transition}
        if extra:
            row.update(extra)
        self.receipt.add_identity(row)

    def await_session(self, cap_s=SESSION_WAIT_S):
        """Bounded wait for the child's own setsid: its PGID becomes its PID."""
        end = time.monotonic() + cap_s
        while time.monotonic() < end:
            try:
                if os.getpgid(self.pid) == self.pid:
                    return
            except OSError:
                pass
            time.sleep(CHILD_STOP_POLL_S)
        raise CaseFailure("the child never created its own session and group")

    def bind(self):
        """Bind the live child's identity: PGID == PID == SID and this leader
        as the real parent, via getpgid/getsid and one kinfo_proc row."""
        self.await_session()
        try:
            pgid = os.getpgid(self.pid)
            sid = os.getsid(self.pid)
        except OSError as error:
            raise CaseFailure(f"the child's identity is unreadable ({type_name(error)})")
        try:
            row = self.module.Kinfo.one(self.pid)
        except OSError as error:
            raise Uncertain(f"the child's kinfo_proc row is unreadable ({type_name(error)})")
        if pgid != self.pid or sid != self.pid:
            raise CaseFailure(f"the child is not its own group/session leader (pgid={pgid}, sid={sid})")
        if row is None:
            raise CaseFailure("the child has no kinfo_proc row")
        if row["ppid"] != os.getpid() or row["pgid"] != self.pid:
            raise CaseFailure(f"the child is not this leader's direct group child (row={row})")
        self.pgid, self.sid, self.start = pgid, sid, row["start"]
        self._identity_row("bound", {"zombie": row["zombie"], "ppid": row["ppid"]})

    def _probe(self, label, call):
        self.probes.append({"label": label, "at_ns": now_ns()})
        try:
            call()
            result = "0"
        except OSError as error:
            result = errno.errorcode.get(error.errno, str(error.errno))
        return result

    def _kinfo_row(self):
        try:
            return self.module.Kinfo.one(self.pid)
        except OSError:
            return None

    def observe_zombie_window(self, duration_s, samples):
        """Verdict-grade while custody is held: the PID cannot be recycled
        while this holder keeps the child unreaped, so a `kill(pid, 0)`
        failure means the premise broke, not that identity was reused."""
        if self.state not in ("held", "held-exited") or self.reaped:
            raise CustodyLost("zombie observation requires unreaped custody")
        interval = duration_s / max(1, samples)
        seen = []
        for index in range(samples):
            kill_result = self._probe("kill-pid-0", lambda: os.kill(self.pid, 0))
            killpg_result = self._probe("killpg-pgid-0", lambda: os.killpg(self.pgid, 0))
            row = self._kinfo_row()
            seen.append((kill_result, killpg_result, row))
            self.receipt.add_identity({"case": self.case, "role": self.role, "pid": self.pid,
                                       "pgid": self.pgid, "sid": self.sid, "start": self.start,
                                       "state": self.state, "transition": "zombie-observation",
                                       "kill_pid_0": kill_result, "killpg_pgid_0": killpg_result,
                                       "row_zombie": None if row is None else row["zombie"],
                                       "row_start": None if row is None else row["start"]})
            if kill_result != "0" or killpg_result != "0":
                raise CaseFailure(f"the held exited child did not persist as a group member "
                                  f"(kill={kill_result}, killpg={killpg_result})")
            if row is not None and row["start"] != self.start:
                raise Uncertain("the held child's identity changed while unreaped")
            if index < samples - 1:
                time.sleep(interval)
        return seen

    def signal_group(self, number):
        """The group signal authority: only while the child is positively
        held, unreaped, and this leader's direct group child. After custody
        ends this refuses without any syscall."""
        if self.state == "settled" or self.reaped:
            self.receipt.add_signal({"case": self.case, "at_ns": now_ns(), "call": "killpg",
                                     "target": self.pgid, "sig": number,
                                     "result": "refused-custody-settled", "custody": self.state,
                                     "syscall_made": False})
            raise CustodyLost("custody is settled; the model refuses without a syscall")
        if self.state not in ("held", "held-exited") or self.pgid is None:
            raise Uncertain("signal attempted in an unknown custody state")
        result = self._probe("killpg-signal", lambda: os.killpg(self.pgid, number))
        self.receipt.add_signal({"case": self.case, "at_ns": now_ns(), "call": "killpg",
                                 "target": self.pgid, "sig": number,
                                 "result": "delivered" if result == "0" else result,
                                 "custody": self.state, "syscall_made": True})
        if result != "0":
            raise CaseFailure(f"killpg(pgid, {number}) failed with {result}")

    def zero_probe(self, label, call):
        """A sig-0 probe: recorded as an observation, never authority."""
        result = self._probe(label, call)
        self.receipt.add_signal({"case": self.case, "at_ns": now_ns(), "call": label,
                                 "sig": 0, "result": result, "custody": self.state,
                                 "probe_only": True, "syscall_made": True})
        return result

    def wait_exit(self, cap_s):
        """Bounded WNOHANG poll for the exit; never a blocking unbounded wait."""
        if self.state == "settled" or self.reaped:
            raise CustodyLost("the child is already reaped")
        end = time.monotonic() + max(0.05, cap_s)
        while time.monotonic() < end:
            try:
                waited, status = os.waitpid(self.pid, os.WNOHANG)
            except OSError as error:
                raise Uncertain(f"waitpid failed on a held child "
                                f"({errno.errorcode.get(error.errno, error.errno)})")
            if waited == self.pid:
                self.state = "held-exited"
                self.exit_status = status
                self._identity_row("exited", {"status": status})
                return status
            time.sleep(CHILD_STOP_POLL_S)
        raise Uncertain("a held child did not exit inside its window: custody unknown")

    def reap(self, cap_s):
        """The single positive reap. A second reap refuses without a syscall."""
        if self.state == "settled" or self.reaped:
            self.receipt.add_signal({"case": self.case, "at_ns": now_ns(), "call": "waitpid",
                                     "result": "refused-already-settled", "custody": self.state,
                                     "syscall_made": False})
            raise CustodyLost("the child is already positively reaped")
        if self.state == "held":
            self.wait_exit(cap_s)
        try:
            waited, status = os.waitpid(self.pid, 0)
        except OSError as error:
            raise Uncertain(f"the positive reap failed "
                            f"({errno.errorcode.get(error.errno, error.errno)})")
        if waited != self.pid:
            raise Uncertain("the positive reap returned another pid")
        self.state = "settled"
        self.exit_status = status
        self.reaped = True
        self._identity_row("reaped", {"status": status})
        return status

    def request_stop(self):
        """Create this child's stop file exclusively; best-effort."""
        try:
            fd = os.open(self.stop_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            os.close(fd)
            return True
        except OSError:
            return False

    def read_report(self):
        try:
            info = os.lstat(self.report_path)
            if not stat.S_ISREG(info.st_mode) or info.st_size > REPORT_MAX_BYTES:
                return None
            with open(self.report_path, "rb") as handle:
                data = handle.read(REPORT_MAX_BYTES + 1)
            if len(data) > REPORT_MAX_BYTES:
                return None
            return json.loads(data.decode("utf-8"))
        except (OSError, ValueError, UnicodeDecodeError):
            return None


# ---------------------------------------------------------------------------
# case spawning


class FixtureChildren:
    """Spawns raw fork/exec children (never subprocess), so no library reaper
    can collect a held child behind this proof's back. Every child is tracked
    until it is positively reaped; a child that cannot be reaped is recorded
    as unknown custody."""

    def __init__(self, module, ctx, receipt, env):
        self.module = module
        self.ctx = ctx
        self.receipt = receipt
        self.env = env
        self.tracked = []
        self.sequence = 0

    def spawn(self, case_name, role, parent_pgid=None):
        fixture_fd = self.ctx["fixture"]["fd"]
        self.sequence += 1
        name = f"{case_name}.{role}.{self.sequence}"
        children_fd = self._children_fd()
        try:
            report_fd = os.open(f"{name}.report.json",
                                os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                                0o600, dir_fd=children_fd)
            log_fd = os.open(f"{name}.log",
                             os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                             0o600, dir_fd=children_fd)
        finally:
            os.close(children_fd)
        os.set_inheritable(report_fd, True)
        os.set_inheritable(log_fd, True)
        stop_path = os.path.join(self.ctx["fixture"]["children_path"],
                                 f"{case_name}.{self.sequence}.stop")
        if parent_pgid is None:
            try:
                parent_pgid = os.getpgid(0)
            except OSError:
                parent_pgid = 0
        code = child_program(role, report_fd, stop_path, parent_pgid, parent_pgid + 1_000_003)
        fixture_path = self.ctx["fixture"]["path"]
        held_dir_fds = [self.ctx["root_fd"], fixture_fd]
        pid = os.fork()
        if pid == 0:
            try:
                os.setsid()  # the child creates its own session and process group
                os.chdir(fixture_path)
                devnull = os.open("/dev/null", os.O_RDONLY)
                os.dup2(devnull, 0)
                os.dup2(log_fd, 1)
                os.dup2(log_fd, 2)
                for fd in held_dir_fds:
                    if fd > 2:
                        try:
                            os.close(fd)
                        except OSError:
                            pass
                os.close(devnull)
                os.close(log_fd)
                os.execve(sys.executable, [sys.executable, "-c", code], self.env)
            except BaseException:
                os._exit(127)
        os.close(report_fd)
        os.close(log_fd)
        child = HeldChild(self.module, self.receipt, case_name, role, pid,
                          os.path.join(self.ctx["fixture"]["children_path"], f"{name}.report.json"),
                          os.path.join(self.ctx["fixture"]["children_path"], f"{name}.log"),
                          stop_path)
        self.tracked.append(child)
        child._identity_row("spawned")
        return child

    def _children_fd(self):
        return os.open(self.ctx["fixture"]["children_path"],
                       os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)

    def stop_all(self):
        for child in self.tracked:
            if not child.reaped:
                child.request_stop()

    def settle(self, budget_s=SETTLE_BUDGET_S):
        """Best-effort bounded settle: stop files, then one bounded reap each.
        A child that cannot be positively reaped stays tracked and recorded."""
        self.stop_all()
        each = max(0.1, budget_s / max(1, len(self.tracked)))
        for child in self.tracked:
            if child.reaped:
                continue
            try:
                child.reap(each)
            except (CustodyLost, Uncertain, CaseFailure):
                pass

    def unreaped(self):
        return [child for child in self.tracked if not child.reaped]

    def log_size(self, child):
        try:
            info = os.lstat(child.log_path)
            return info.st_size if stat.S_ISREG(info.st_mode) else -1
        except OSError:
            return -1


def wait_for_report(child, cap_s=READINESS_WAIT_S):
    """Bounded readiness: the child's report file, non-empty and parseable."""
    end = time.monotonic() + cap_s
    while time.monotonic() < end:
        report = child.read_report()
        if report is not None:
            return report
        time.sleep(CHILD_STOP_POLL_S)
    return None


def check_log_size(children, child):
    size = children.log_size(child)
    if size < 0 or size > LOG_MAX_BYTES:
        raise Uncertain(f"child {child.pid} output is missing or over the bound ({size} bytes)")


# ---------------------------------------------------------------------------
# case table


CASES = [
    {"name": "c01-bind-live-hold", "kind": "native", "depends": [],
     "children": 1, "summary": "child-created session/group bound live; WNOHANG collects "
     "nothing; stop-file exit; zombie witness; positive reap"},
    {"name": "c02-zombie-group-lifetime", "kind": "native", "depends": [],
     "children": 1, "summary": "an exited child stays a group member while held unreaped "
     "(bounded kill/killpg-0 window, same start identity); the group ends at the reap"},
    {"name": "c03-stop-under-custody", "kind": "native", "depends": ["c01-bind-live-hold"],
     "children": 1, "summary": "the prospective cooperative stop: bounded killpg TERM, "
     "escalating KILL, only while the holder positively holds the unreaped direct child; "
     "positive reap afterwards"},
    {"name": "c04-session-leader-cannot-leave", "kind": "native", "depends": [],
     "children": 1, "summary": "the child's own setpgid escape attempts fail EPERM and a "
     "second setsid fails EPERM; the parent verifies the group never changed"},
    {"name": "c05-auto-reap-refused", "kind": "native", "depends": [],
     "children": 2, "summary": "SIGCHLD is SIG_DFL at hold time; a SIG_IGN demo child "
     "shows the kernel auto-reaping (ECHILD, no zombie), so the disposition premise is "
     "required; the leader's disposition is restored and re-asserted"},
    {"name": "c06-concurrent-wait-refused", "kind": "native", "depends": [],
     "children": 1, "summary": "one positive reap; a second waitpid fails ECHILD, so a "
     "wait cannot be undone and a concurrent wait is refused"},
    {"name": "c07-lost-custody-refusal", "kind": "native", "depends": ["c01-bind-live-hold"],
     "children": 1, "summary": "after the positive reap the model refuses signal and reap "
     "without a syscall; kernel witnesses recorded as observations"},
    {"name": "c08-observation-not-authority", "kind": "native", "depends": [],
     "children": 1, "summary": "probe-only sig-0 rows on the held identity, ESRCH for "
     "absent identities, and vacuous kill(0,0)/killpg(0,0): observations are not handles"},
    {"name": "s01-owner-death-custody", "kind": "source-only", "depends": [],
     "children": 0, "summary": "HELD: actual parent death with a held child — no bounded "
     "no-new-authority containment/lifetime argument accepted; source/pure analysis only"},
    {"name": "s02-owner-unresponsive", "kind": "source-only", "depends": [],
     "children": 0, "summary": "HELD: an owner that cannot answer cannot delegate custody "
     "by publishing its PID; source/pure analysis only"},
    {"name": "s03-descendant-group-escape", "kind": "source-only", "depends": [],
     "children": 0, "summary": "HELD: a non-leader descendant can setpgid(0, 0) out of the "
     "held group, so holding the leader does not contain descendants; source/pure only"},
]


def case_names(kind=None):
    return [case["name"] for case in CASES if kind is None or case["kind"] == kind]


# ---------------------------------------------------------------------------
# case bodies


def case_c01(runner, children, receipt, cap_s):
    child = children.spawn("c01-bind-live-hold", "hold")
    child.bind()
    end = time.monotonic() + LIVE_HOLD_S
    while time.monotonic() < end:
        waited, _status = os.waitpid(child.pid, os.WNOHANG)
        if waited != 0:
            raise CaseFailure("the held live child was collected by an implicit wait")
        if os.getpgid(child.pid) != child.pid or os.getsid(child.pid) != child.pid:
            raise CaseFailure("the held child's group/session changed while live")
        time.sleep(0.1)
    report = wait_for_report(child)
    if report is None:
        raise CaseFailure("the child's report never arrived")
    if not report.get("identity", {}).get("pgid_is_pid") \
            or not report.get("identity", {}).get("sid_is_pid"):
        raise CaseFailure("the child does not verify its own session/group leadership")
    child.request_stop()
    child.wait_exit(cap_s)
    child.observe_zombie_window(0.3, 2)
    child.reap(cap_s)
    check_log_size(children, child)
    if child.exit_status != 0:
        raise CaseFailure(f"the child exited {child.exit_status}")
    # post-reap closure witnesses: corroboration rows, race-aware
    kill_result = child.zero_probe("kill-pid-0-after-reap", lambda: os.kill(child.pid, 0))
    killpg_result = child.zero_probe("killpg-pgid-0-after-reap", lambda: os.killpg(child.pgid, 0))
    if kill_result != "ESRCH" or killpg_result != "ESRCH":
        receipt.add_identity({"case": "c01-bind-live-hold", "role": child.role, "pid": child.pid,
                              "pgid": child.pgid, "sid": child.sid, "start": child.start,
                              "state": "settled", "transition": "post-reap-probe-inconclusive",
                              "kill_pid_0": kill_result, "killpg_pgid_0": killpg_result})
    return {"detail": "bound live, held, stopped, zombie witnessed, reaped; identity held across the arc"}


def case_c02(runner, children, receipt, cap_s):
    child = children.spawn("c02-zombie-group-lifetime", "self-exit")
    child.bind()
    child.wait_exit(cap_s)
    child.observe_zombie_window(ZOMBIE_WINDOW_S, ZOMBIE_SAMPLES)
    child.reap(cap_s)
    check_log_size(children, child)
    killpg_result = child.zero_probe("killpg-pgid-0-after-reap", lambda: os.killpg(child.pgid, 0))
    if killpg_result != "ESRCH":
        receipt.add_identity({"case": "c02-zombie-group-lifetime", "role": child.role,
                              "pid": child.pid, "pgid": child.pgid, "sid": child.sid,
                              "start": child.start, "state": "settled",
                              "transition": "post-reap-probe-inconclusive",
                              "killpg_pgid_0": killpg_result})
    return {"detail": "the zombie persisted as a group member across the bounded window; "
                      "the group ended at the positive reap"}


def case_c03(runner, children, receipt, cap_s):
    child = children.spawn("c03-stop-under-custody", "term-target")
    child.bind()
    report = wait_for_report(child)
    if report is None:
        raise CaseFailure("the stop target never reported ready")
    if os.getpgid(child.pid) != child.pid or os.getsid(child.pid) != child.pid:
        raise CaseFailure("the stop target left its own group before the stop")
    child.signal_group(signal.SIGTERM)
    try:
        child.wait_exit(TERM_WINDOW_S)
        by = signal.SIGTERM
    except Uncertain:
        child.signal_group(signal.SIGKILL)
        child.wait_exit(TERM_WINDOW_S)
        by = signal.SIGKILL
    child.reap(cap_s)
    check_log_size(children, child)
    status = child.exit_status
    signalled = os.WIFSIGNALED(status)
    term_number = os.WTERMSIG(status) if signalled else None
    if not signalled or term_number != by:
        raise CaseFailure(f"the stop did not end the child by signal {by} (status={status})")
    return {"detail": f"group stop by signal {by} under held custody, then positive reap"}


def case_c04(runner, children, receipt, cap_s):
    child = children.spawn("c04-session-leader-cannot-leave", "hold")
    child.bind()
    report = wait_for_report(child)
    if report is None:
        raise CaseFailure("the child's escape-attempt report never arrived")
    attempts = {(row[0], row[1]) for row in report.get("attempts", [])}
    expected = {("setpgid-parent-group", errno.EPERM), ("setpgid-absent-group", errno.EPERM),
                ("setsid-again", errno.EPERM)}
    if report.get("escaped") is True or not expected.issubset(attempts):
        raise CaseFailure(f"the escape matrix is not EPERM-refused: "
                          f"attempts={sorted([str(a) for a in attempts])}")
    # the holder cannot move the child either: after the child's exec, the
    # parent's setpgid(child, ...) refuses EPERM
    try:
        os.setpgid(child.pid, child.pid + 1_000_003)
        moved = True
    except OSError as error:
        moved = False
        refused = errno.errorcode.get(error.errno, str(error.errno))
    if moved:
        raise CaseFailure("the parent moved a post-exec child out of its group")
    receipt.add_signal({"case": "c04-session-leader-cannot-leave", "at_ns": now_ns(),
                        "call": "setpgid-parent-of-child", "target": child.pid,
                        "result": f"refused-{refused}", "custody": "held",
                        "syscall_made": True})
    if os.getpgid(child.pid) != child.pid or os.getsid(child.pid) != child.pid:
        raise CaseFailure("the child's group changed despite the EPERM attempts")
    child.request_stop()
    child.wait_exit(cap_s)
    child.reap(cap_s)
    check_log_size(children, child)
    return {"detail": "the child's setpgid/setsid escapes and the parent's post-exec "
                      "setpgid all refuse EPERM; group unchanged"}


def case_c05(runner, children, receipt, cap_s):
    disposition = sigchld_disposition()
    if disposition is None:
        python_level = signal.getsignal(signal.SIGCHLD)
        if python_level != signal.SIG_DFL:
            raise Uncertain(f"the SIGCHLD disposition is unreadable and the Python level is "
                            f"{python_level!r}: the exclusive-unreaped premise cannot be "
                            "verified on this platform")
    else:
        if disposition["handler"] not in (None, SIG_DFL_VALUE):
            raise Uncertain(f"the kernel SIGCHLD handler is {disposition['handler']}, not "
                            "SIG_DFL: the exclusive-unreaped premise cannot be exercised")
        if disposition["flags"] & SA_NOCLDWAIT:
            raise Uncertain("SIGCHLD carries SA_NOCLDWAIT: the kernel would never leave "
                            "zombies, so exclusive unreaped custody is impossible")
    receipt.add_identity({"case": "c05-auto-reap-refused", "role": "leader-disposition",
                          "pid": os.getpid(), "pgid": None, "sid": None, "start": None,
                          "state": "held", "transition": "sigchld-disposition-verified",
                          "handler": None if disposition is None else disposition["handler"],
                          "sa_flags": None if disposition is None else disposition["flags"],
                          "source": "sigaction(2)" if disposition is not None else
                          "signal.getsignal (sigaction unavailable)"})
    demo = children.spawn("c05-auto-reap-refused", "demo-ignore")
    demo.bind()
    report = wait_for_report(demo)
    if report is None:
        raise CaseFailure("the SIG_IGN demo never reported")
    if report.get("wait") != "errno:%d" % errno.ECHILD:
        raise CaseFailure(f"the SIG_IGN demo did not show kernel auto-reap: wait={report.get('wait')}")
    if report.get("probe") != "errno:%d" % errno.ESRCH:
        raise CaseFailure(f"the auto-reaped grandchild still probes alive: {report.get('probe')}")
    demo.reap(cap_s)
    check_log_size(children, demo)
    after = sigchld_disposition()
    if after is None:
        if signal.getsignal(signal.SIGCHLD) != signal.SIG_DFL:
            raise Uncertain("the leader's SIGCHLD disposition changed")
    elif after["handler"] not in (None, SIG_DFL_VALUE) or after["flags"] & SA_NOCLDWAIT:
        raise Uncertain("the leader's kernel SIGCHLD disposition changed during the case")
    receipt.add_identity({"case": "c05-auto-reap-refused", "role": "grandchild",
                          "pid": report.get("grandchild"), "pgid": None, "sid": None,
                          "start": None, "state": "auto-reaped", "transition": "no-zombie",
                          "note": "SIG_IGN counterfactual: no custody is possible"})
    return {"detail": "SIG_DFL asserted; the SIG_IGN counterfactual shows auto-reap "
                      "(ECHILD, no zombie)"}


def case_c06(runner, children, receipt, cap_s):
    child = children.spawn("c06-concurrent-wait-refused", "self-exit")
    child.bind()
    waited, _status = os.waitpid(child.pid, os.WNOHANG)
    if waited != 0:
        raise CaseFailure("WNOHANG collected the child while it was still live")
    child.wait_exit(cap_s)
    child.reap(cap_s)
    check_log_size(children, child)
    try:
        os.waitpid(child.pid, 0)
    except ChildProcessError:
        receipt.add_signal({"case": "c06-concurrent-wait-refused", "at_ns": now_ns(),
                            "call": "waitpid-second", "result": "refused-ECHILD",
                            "custody": "settled", "syscall_made": True})
        return {"detail": "one positive reap; the second waitpid is refused ECHILD"}
    except OSError as error:
        raise Uncertain(f"the second waitpid failed with "
                        f"{errno.errorcode.get(error.errno, error.errno)}, not ECHILD")
    raise CaseFailure("a second waitpid after the reap succeeded: a wait was undone")


def case_c07(runner, children, receipt, cap_s):
    child = children.spawn("c07-lost-custody-refusal", "self-exit")
    child.bind()
    child.wait_exit(cap_s)
    child.reap(cap_s)
    check_log_size(children, child)
    probes_before = len(child.probes)
    try:
        child.signal_group(signal.SIGTERM)
        raise CaseFailure("the model signalled a settled child")
    except CustodyLost:
        if len(child.probes) != probes_before:
            raise Uncertain("the refusal made a syscall")
        receipt.add_signal({"case": "c07-lost-custody-refusal", "at_ns": now_ns(),
                            "call": "killpg-model-refusal", "sig": int(signal.SIGTERM),
                            "result": "refused-custody-settled", "custody": "settled",
                            "syscall_made": False})
    try:
        child.reap(cap_s)
        raise CaseFailure("the model re-reaped a settled child")
    except CustodyLost:
        if len(child.probes) != probes_before:
            raise Uncertain("the refusal made a syscall")
    kill_result = child.zero_probe("kill-pid-0-after-reap", lambda: os.kill(child.pid, 0))
    killpg_result = child.zero_probe("killpg-pgid-0-after-reap", lambda: os.killpg(child.pgid, 0))
    detail = "settled custody refuses signal and reap without a syscall"
    if kill_result != "ESRCH" or killpg_result != "ESRCH":
        detail += f"; post-reap probes inconclusive (pid={kill_result}, pgid={killpg_result})"
    return {"detail": detail}


def case_c08(runner, children, receipt, cap_s):
    child = children.spawn("c08-observation-not-authority", "hold")
    child.bind()
    held_pid = child.zero_probe("kill-pid-0-held", lambda: os.kill(child.pid, 0))
    held_pgid = child.zero_probe("killpg-pgid-0-held", lambda: os.killpg(child.pgid, 0))
    if held_pid != "0" or held_pgid != "0":
        raise CaseFailure(f"the held child's own sig-0 probes failed "
                          f"(pid={held_pid}, pgid={held_pgid})")
    absent = child.pid + 1_000_007
    absent_result = child.zero_probe("kill-absent-pid-0", lambda: os.kill(absent, 0))
    if absent_result != "ESRCH":
        raise Uncertain(f"an absent pid probed {absent_result}, not ESRCH: identity reuse?")
    kill_zero = child.zero_probe("kill-zero-target-0", lambda: os.kill(0, 0))
    killpg_zero = child.zero_probe("killpg-zero-target-0", lambda: os.killpg(0, 0))
    if kill_zero != "0" or killpg_zero != "0":
        raise CaseFailure("zero-target probes were refused, so their vacuity is not demonstrated")
    child.request_stop()
    child.wait_exit(cap_s)
    child.reap(cap_s)
    check_log_size(children, child)
    return {"detail": "held-identity sig-0 probes are observations; kill(0,0)/killpg(0,0) "
                      "are vacuous; absent identities answer ESRCH"}


CASE_BODIES = {
    "c01-bind-live-hold": case_c01,
    "c02-zombie-group-lifetime": case_c02,
    "c03-stop-under-custody": case_c03,
    "c04-session-leader-cannot-leave": case_c04,
    "c05-auto-reap-refused": case_c05,
    "c06-concurrent-wait-refused": case_c06,
    "c07-lost-custody-refusal": case_c07,
    "c08-observation-not-authority": case_c08,
}


# ---------------------------------------------------------------------------
# native run


def git(*arguments):
    result = subprocess.run(["git", *arguments], cwd=REPO, capture_output=True, text=True)
    if result.returncode != 0:
        raise Refused(f"git {' '.join(arguments)} failed: {result.stderr.strip()[:200]}")
    return result.stdout.strip()


def install_signal_settle(children, receipt):
    """On the guard's TERM/INT of the leader's group, settle every owned child
    inside a short bounded window (the children are in their OWN sessions and
    groups, so the guard's pinned-group stop cannot reach them), republish the
    truthful receipt, and exit. A child that cannot be positively reaped stays
    recorded as unknown custody and dies by its own alarm bound."""

    def handler(number, _frame):
        try:
            children.settle()
        except BaseException:  # noqa - the handler never raises past the exit
            pass
        receipt.document["first_fault"] = receipt.document.get("first_fault") \
            or f"signalled: {number}"
        receipt.document["nested"]["outcome"] = "unknown"
        receipt.document["nested"]["writer_closure"] = \
            "unknown" if children.unreaped() else "positively-reaped-all-owned-children"
        receipt.document["result"] = "unqualified"
        receipt.publish()
        os._exit(128 + int(number))

    for number in (signal.SIGTERM, signal.SIGINT):
        signal.signal(number, handler)


def run_native(diagnostic):
    mode_name = "diagnostic" if diagnostic else "counted"
    try:
        dirty = git("status", "--porcelain")
        if dirty:
            raise Refused(f"the worktree is not clean: {dirty[:300]}")
        commit = git("rev-parse", "HEAD")
        tree = git("rev-parse", "HEAD^{tree}")
    except Refused as error:
        say(f"cargo-group-custody: source binding refused: {error}")
        return 2

    trace = []
    try:
        ctx = establish_hosting(os.environ, trace)
    except Refused as error:
        say(f"cargo-group-custody: refused before any fixture resource: {error}")
        return 2
    except Uncertain as error:
        say(f"cargo-group-custody: uncertain before any fixture resource: {error}")
        return 125
    runner = ctx["runner"]

    fixture = ctx["fixture"]
    receipt_base = {
        "schema": 1,
        "task": "oxide#237",
        "mode": mode_name,
        "uncounted": bool(diagnostic),
        "script": os.path.relpath(SCRIPT_PATH, REPO),
        "script_sha256": sha256_file(SCRIPT_PATH),
        "source_commit": commit,
        "source_tree": tree,
        "command": DIAGNOSTIC_COMMAND if diagnostic else COUNTED_COMMAND,
        "guard_limit_s": GUARD_LIMIT_S,
        "guard_scratch_budget_mib": GUARD_BUDGET_MIB,
        "tokens": {
            "supervisor_run_id": ctx["supervisor_token"],
            "marker_run_id": ctx["marker_token"],
            "separate_tokens_verified": ctx["supervisor_token"] != ctx["marker_token"],
        },
        "root": identity_of(ctx["root"], ctx["root_fd"]),
        "parent_path": ctx["selection"]["root_parent"],
        "scratch_path": ctx["scratch"],
        "evidence": identity_of(ctx["evidence"]),
        "mount": ctx["mount"],
        "volume_uuid": ctx["volume_uuid"],
        "fixture_dir": fixture["identity"],
        "runner": ctx["runner_facts"],
        "guard_receipt": {
            "phase_at_read": ctx["guard_phase_observed"],
            "scratch_default_source": ctx["selection"]["source"],
            "run_id_matches_environment": True,
        },
        "protection": {"sequence": trace, "nested_scope": "hosting",
                       "readback": "independent-strict-reread",
                       "self_conclusion": False, "sweep": False, "cleanup": "none",
                       "restoration": "none"},
        "outer": {
            "claimed": False,
            "note": "outer closure, cleanup and watchdog belong to the guard's own receipt; "
                    "this nested receipt never claims them",
            "monitoring_ends_with_outer_run": True,
            "deadline_ns": ctx["deadline_ns"],
        },
        "nested": {
            "outcome": "unknown",
            "writer_closure": "unknown",
            "census_scope": "this fixture's own direct children only; no claim about "
                            "arbitrary descendants",
            "retained": True,
            "root_concluded": False,
        },
        "cases": [],
        "signals": [],
        "identities": [],
        "totals": {"cases_total": 0, "cases_omitted": 0, "signals_total": 0,
                   "signals_omitted": 0, "identities_total": 0, "identities_omitted": 0},
        "limits": {"receipt_max_bytes": RECEIPT_MAX_BYTES, "case_rows": CASE_ROWS_MAX,
                   "signal_rows": SIGNAL_ROWS_MAX, "identity_rows": IDENTITY_ROWS_MAX},
        "atomic_publication_is_not_durable": True,
        "held_cases": case_names("source-only"),
        "plan_notes": "atomic publication is not power-loss durability; cooperative-host "
                      "limitations preserved",
    }
    if diagnostic:
        receipt_base["qualified"] = False
    receipt = NestedReceipt(fixture["fd"], receipt_base)
    if not receipt.publish():
        raise Uncertain(f"initial nested receipt publication failed: {receipt.write_error}")
    child_env = child_environment(os.environ, fixture["path"])
    children = FixtureChildren(runner, ctx, receipt, child_env)
    install_signal_settle(children, receipt)

    fault = None
    first_fault = None
    results = {}

    def record_case(name, status, detail):
        results[name] = status
        try:
            receipt.add_case({"case": name, "status": status, "detail": detail, "at_ns": now_ns()})
        except Uncertain:
            pass  # sticky write_error is handled before another case launches

    def deadline_exhausted():
        return now_ns() >= ctx["deadline_ns"] - int(DEADLINE_MARGIN_S * 1e9)

    # Case execution. Counted mode fails fast; the diagnostic keeps going only
    # over dependency-independent cases with known fixture state, and aborts
    # on infrastructure, device, writer, custody or receipt uncertainty.
    for case in CASES:
        name = case["name"]
        if receipt.write_error is not None:
            fault = fault or f"the nested receipt cannot be published ({receipt.write_error})"
            break
        if deadline_exhausted():
            record_case(name, CASE_STATUS_UNEXECUTED,
                        "the work window before the guard deadline is exhausted; "
                        "no new case launches")
            continue
        if case["kind"] == "source-only":
            record_case(name, CASE_STATUS_SOURCE_ONLY,
                        "HELD: not executed; no bounded no-new-authority containment or "
                        "lifetime argument is accepted for this case")
            continue
        unmet = [dependency for dependency in case["depends"]
                 if results.get(dependency) != CASE_STATUS_COMPLETED]
        if unmet:
            if diagnostic:
                record_case(name, CASE_STATUS_SKIPPED,
                            f"dependency not completed: {', '.join(unmet)}")
                continue
            record_case(name, CASE_STATUS_UNEXECUTED,
                        f"fail-fast: dependency not completed: {', '.join(unmet)}")
            break
        cap_s = min(CASE_CAP_S,
                    max(1.0, (ctx["deadline_ns"] - now_ns()) / 1e9 - DEADLINE_MARGIN_S))
        try:
            detail = CASE_BODIES[name](runner, children, receipt, cap_s)
            record_case(name, CASE_STATUS_COMPLETED, detail)
        except CaseFailure as error:
            record_case(name, CASE_STATUS_FAILED, str(error))
            if first_fault is None:
                first_fault = f"{name}: {error}"
            if not diagnostic:
                fault = fault or f"{name}: {error}"
                break
        except (Uncertain, CustodyLost) as error:
            record_case(name, CASE_STATUS_ABORTED, f"uncertainty: {error}")
            fault = fault or f"{name}: uncertain: {error}"
            if first_fault is None:
                first_fault = fault
            break
        except BaseException as error:  # unclassified outcomes are not recoverable
            record_case(name, CASE_STATUS_ABORTED, f"unclassified: {type_name(error)}: {error}")
            fault = fault or f"{name}: unclassified {type_name(error)}"
            if first_fault is None:
                first_fault = fault
            break
        if receipt.publish() is False:
            fault = fault or f"the nested receipt cannot be published ({receipt.write_error})"
            if first_fault is None:
                first_fault = fault
            record_case("receipt", CASE_STATUS_ABORTED, "later cases refuse: the receipt is lost")
            break

    # Final settle: every owned child must end positively reaped, or nested
    # custody stays unknown. This runs even after a fault.
    settle_fault = None
    try:
        children.settle()
    except BaseException as error:  # noqa
        settle_fault = f"settle failed: {type_name(error)}: {error}"
    unreaped = children.unreaped()
    if unreaped:
        settle_fault = settle_fault or f"{len(unreaped)} owned child(ren) not positively reaped"
        for child in unreaped:
            receipt.add_identity({"case": child.case, "role": child.role, "pid": child.pid,
                                  "pgid": child.pgid, "sid": child.sid, "start": child.start,
                                  "state": child.state, "transition": "unreaped-kept"})

    for case in CASES:
        if case["name"] not in results:
            if case["kind"] == "source-only":
                record_case(case["name"], CASE_STATUS_SOURCE_ONLY,
                            "HELD: not executed; source/pure analysis only")
            else:
                record_case(case["name"], CASE_STATUS_UNEXECUTED,
                            "not reached" + (f": {first_fault}" if first_fault else ""))

    nested = derive_nested_outcome(
        [{"reaped": child.reaped} for child in children.tracked],
        [{"name": case["name"], "kind": case["kind"],
          "status": results.get(case["name"], CASE_STATUS_UNEXECUTED)} for case in CASES],
        receipt.write_error is None and fault is None,
    )
    nested["census_limitation"] = ("positive direct-child reaps and the bounded probes above; "
                                   "the guard's own census remains the outer authority")
    receipt.document["nested"].update(nested)
    receipt.document["first_fault"] = first_fault or fault or settle_fault
    receipt.document["native_children_spawned"] = len(children.tracked)
    receipt.document["native_children_reaped"] = len(children.tracked) - len(unreaped)
    all_native_completed = all(results.get(case["name"]) == CASE_STATUS_COMPLETED
                               for case in CASES if case["kind"] == "native")
    receipt.document["result"] = (
        "pass" if (not diagnostic and fault is None and settle_fault is None
                   and all_native_completed and nested["outcome"] == "nested-closed")
        else "unqualified")
    if not receipt.publish():
        say(f"cargo-group-custody: the nested receipt could not be published: {receipt.write_error}")
        return 125

    completed = sum(1 for case in CASES
                    if results.get(case["name"]) == CASE_STATUS_COMPLETED)
    say(f"cargo-group-custody: receipt {os.path.join(fixture['path'], RECEIPT_NAME)}; "
        f"outcome={receipt.document['nested']['outcome']}; "
        f"completed={completed}/{len(case_names('native'))} native cases; "
        f"children={receipt.document['native_children_reaped']}/"
        f"{receipt.document['native_children_spawned']} reaped")

    if fault is None and settle_fault is None and all_native_completed \
            and nested["outcome"] == "nested-closed":
        return 0
    if unreaped or settle_fault or "uncertain" in str(fault or ""):
        return 125
    return 1


# ---------------------------------------------------------------------------
# plan (effect-free)


def premise_dispositions():
    """The fixed nine-item proof-authoring premise checklist, applied to the
    actual executable helpers."""
    return [
        {"item": 1, "name": "Private HOME and rustup",
         "disposition": "applies-as-hygiene: no toolchain, Cargo or rustup call exists in "
         "this script, so RUSTUP_HOME capture is not applicable; fixture children still get "
         "a private HOME inside the fixture directory (child_environment). No --locked "
         "Cargo call exists, so no lock premise applies."},
        {"item": 2, "name": "Account-home control",
         "disposition": "not-applicable: no product or oxide binary is executed; no "
         "OXIDE_TEST_HOME_FROM_ENV premise exists. The account home is never read."},
        {"item": 3, "name": "Environment isolation",
         "disposition": "applies: child_environment drops every inherited OXIDE_/XDG_/CARGO_/"
         "SCCACHE_/RUST* key and builds only PATH plus fixture-local HOME, TMPDIR/TMP/TEMP "
         "and PYTHONDONTWRITEBYTECODE; the self-check drives it with a hostile env."},
        {"item": 4, "name": "Automatic adoption",
         "disposition": "not-applicable: no product build or checkout is involved."},
        {"item": 5, "name": "Cargo external argv",
         "disposition": "not-applicable: no Cargo or external-subcommand argv exists."},
        {"item": 6, "name": "Owned images",
         "disposition": "not-applicable: no image, hdiutil call, mount or detach exists."},
        {"item": 7, "name": "Compiler flags",
         "disposition": "not-applicable: nothing is compiled; no RUSTFLAGS is set or kept."},
        {"item": 8, "name": "Temporary storage",
         "disposition": "applies: the fixture runs as the guard's leader under the default "
         "selected-volume owned root and refuses adopted, explicit-parent, explicit-evidence "
         "and fallback placements; every fixture writer target lives in "
         "evidence/cargo-group-custody.<random>/ under the held evidence descriptor, never "
         "in the guard's auto-cleaned scratch and never on /private/tmp."},
        {"item": 9, "name": "macOS mktemp",
         "disposition": "applies: the native path never names a temp file without an "
         "explicitly created owned parent (os.mkdir with dir_fd and explicit names); the "
         "self-check's one temp directory uses mkdtemp with an explicit dir under TMPDIR. "
         "A bare-templateless mkdtemp call is AST-refused by the self-check."},
    ]


def plan_document():
    return {
        "task": "oxide#237",
        "script": os.path.relpath(SCRIPT_PATH, REPO),
        "command_counted": COUNTED_COMMAND,
        "command_diagnostic": DIAGNOSTIC_COMMAND,
        "bounds": {"guard_limit_s": GUARD_LIMIT_S, "guard_scratch_budget_mib": GUARD_BUDGET_MIB,
                   "case_cap_s": CASE_CAP_S, "deadline_margin_s": DEADLINE_MARGIN_S,
                   "child_alarm_s": CHILD_LIFETIME_S},
        "qualified_runner": {"path": os.path.relpath(RUNNER_PATH, REPO),
                             "sha256_pinned": EXPECTED_RUNNER_SHA256},
        "cases": CASES,
        "case_count": {"native": len(case_names("native")),
                       "source-only-held": len(case_names("source-only"))},
        "resource_fit": {
            "children_total": sum(case["children"] for case in CASES),
            "estimated_wall_s": "each native case holds a child for about 1-4 s plus the "
                                "bounded zombie window; the eight native cases fit well "
                                "inside a minute of work; unmeasured natively until the "
                                "one diagnostic runs through the task's proof route",
            "scratch_bytes": "the fixture writes only bounded reports, logs, stop files "
                             "and one <=64 KiB receipt inside the retained evidence "
                             "directory; the guard's whole-root accounting includes it",
            "cargo_or_images": "none",
        },
        "receipt": {"max_bytes": RECEIPT_MAX_BYTES, "case_rows": CASE_ROWS_MAX,
                    "signal_rows": SIGNAL_ROWS_MAX, "identity_rows": IDENTITY_ROWS_MAX,
                    "tokens": "supervisor run token and marker run token bound separately"},
        "refusal_premises": [
            "no OXIDE_PROOF_* environment (not the guard's leader)",
            "guard receipt missing, partial, changed phase, stale run token or foreign paths",
            "adopted scratch, explicit --root-parent, explicit --evidence, fallback selection",
            "unmarked, stale, replaced, foreign-owned, non-private or oversized marker",
            "marker token equated to the supervisor token (separate tokens)",
            "alias/symlink/mount-crossing, mode, ownership or device faults",
            "protection publication or independent hosting readback failing",
            "qualified runner bytes missing or changed from the pinned SHA-256",
            "dirty worktree at the native start",
        ],
        "premise_checklist": premise_dispositions(),
        "held": "s01-owner-death-custody, s02-owner-unresponsive and "
                "s03-descendant-group-escape are source/pure-only; native execution of "
                "any of them waits for its own bounded no-new-authority "
                "containment/lifetime argument, which is separately reviewed",
        "activation": "native execution is gated on the retained Queue reviewer's exact-head "
                      "source/premise approval; the one counted-path diagnostic then runs "
                      "through the task's proof route (never by the author or reviewer); "
                      "receipts, not exit codes alone, are the findings",
    }


# ---------------------------------------------------------------------------
# self-check (pure apparatus)


def _remove_selftemp(path):
    """The only deletion site in this script: the self-check's own private
    temporary directory, removed before it returns."""
    import shutil
    shutil.rmtree(path, ignore_errors=True)


class SelfTemp:
    """One private directory under TMPDIR, removed before returning."""

    def __init__(self):
        parent = os.environ.get("TMPDIR") or os.environ.get("TMP") or os.environ.get("TEMP")
        if not parent or not os.path.isdir(parent):
            raise SystemExit("self-check: set TMPDIR to a private directory first")
        self.path = os.path.realpath(
            tempfile.mkdtemp(prefix="cargo-group-custody-selfcheck.", dir=parent))

    def clean(self):
        _remove_selftemp(self.path)


def expect(condition, message):
    if not condition:
        raise AssertionError("self-check: " + message)


class FakeGuard:
    """Builds a faithful fake of the guard's default selected-volume owned
    root inside the self-check temp directory, using the loaded runner's own
    marker and publication machinery."""

    def __init__(self, runner, mount):
        self.runner = runner
        self.mount = mount
        self.root_parent = os.path.join(mount, "scratch", "oxide")
        os.makedirs(self.root_parent, mode=0o700, exist_ok=True)
        os.chmod(self.root_parent, 0o700)
        self.root = tempfile.mkdtemp(prefix="run.", dir=self.root_parent)
        os.chmod(self.root, 0o700)
        self.scratch = os.path.join(self.root, "scratch")
        self.evidence = os.path.join(self.root, "evidence")
        os.mkdir(self.scratch, 0o755)
        os.mkdir(self.evidence, 0o755)
        self.device = os.stat(self.mount).st_dev
        self.run_id = secrets.token_hex(16)
        self.marker_token = secrets.token_hex(16)
        marker_info = os.lstat(self.root)
        self.marker_path = self.runner.write_marker(
            self.root,
            {"version": runner.MARKER_VERSION, "run_id": self.marker_token,
             "root": self.root, "owner_uid": os.geteuid(), "device": self.device,
             "inode": marker_info.st_ino, "created_unix": time.time(),
             "concluded_unix": None, "outcome": "running", "closure": "none",
             "kept_reason": "none", "nested_scope": "exclusive"},
            runner.DefaultProbe())
        self.publish_receipt()

    def publish_receipt(self, phase=GUARD_PHASE_STARTED, **overrides):
        document = {
            "run_id": self.run_id, "command": ["fixture"], "limit_s": GUARD_LIMIT_S,
            "root": self.root, "scratch": self.scratch, "evidence": self.evidence,
            "scratch_owned": True, "start_ns": 0, "abort_threshold": 1, "phase": phase,
            "scratch_default": {"source": "selected-volume", "root_parent": self.root_parent,
                                "prepared_parent": self.root_parent, "mount": self.mount,
                                "volume_uuid": "0123abcd-0000-0000-0000-000000000001",
                                "device": self.device, "pool": None, "pool_source": None,
                                "registration": None},
            "scratch_marker": {"path": self.marker_path, "state": "written",
                               "run_id": self.marker_token, "reason": None},
        }
        document.update(overrides)
        path = os.path.join(self.evidence, "receipt.json")
        with open(path + ".tmp", "w") as handle:
            json.dump(document, handle)
        os.replace(path + ".tmp", path)
        return document

    def environment(self):
        return {
            "OXIDE_PROOF_RUN_ID": self.run_id,
            "OXIDE_PROOF_EVIDENCE": self.evidence,
            "OXIDE_PROOF_DEADLINE_NS": str(now_ns() + int(240 * 1e9)),
            "OXIDE_TASK_SCRATCH_ROOT": self.scratch,
        }

    def host_context(self, trace):
        """Run the real pre-child sequence against this fake guard."""
        return establish_hosting(self.environment(), trace)


def self_check():
    temp = SelfTemp()
    failures = []
    try:
        for name, body in _self_checks(temp):
            try:
                body()
                say(f"self-check: ok {name}")
            except AssertionError as error:
                failures.append(str(error))
                say(f"self-check: FAIL {name}: {error}")
            except (Refused, Uncertain, CaseFailure) as error:
                failures.append(f"{name}: unexpected refusal {error}")
                say(f"self-check: FAIL {name}: unexpected refusal {error}")
    finally:
        temp.clean()
    if failures:
        say(f"self-check: {len(failures)} failing checks")
        return 1
    say("self-check: all checks passed")
    return 0


def _self_checks(temp):
    """Each check drives an actual seam of this script or the loaded qualified
    runner. Nothing here starts a child or leaves any file behind."""
    checks = []

    def check(name):
        def register(body):
            checks.append((name, body))
            return body
        return register

    runner, runner_facts = load_runner_from(RUNNER_PATH)

    # --- qualified runner bytes -------------------------------------------------
    @check("runner-load-happy-path")
    def _():
        module, facts = load_runner_from(RUNNER_PATH)
        expect(facts["sha256"] == EXPECTED_RUNNER_SHA256, "the pinned hash did not verify")
        expect(facts["loaded_from_verified_bytes"], "the loader did not record verified-bytes load")
        expect(module.protect_nested_scope.__code__.co_code
               == runner.protect_nested_scope.__code__.co_code,
               "two loads of the same bytes diverged")

    @check("runner-load-hash-mismatch")
    def _():
        copy = os.path.join(temp.path, "changed-bounded-test.py")
        with open(RUNNER_PATH, "rb") as handle:
            data = handle.read()
        with open(copy, "wb") as handle:
            handle.write(data.replace(b"MARKER_NAME", b"MARKER_NAME_", 1))
        try:
            load_runner_from(copy)
            raise AssertionError("a changed runner loaded")
        except Refused as error:
            expect(EXPECTED_RUNNER_SHA256 in str(error), "the refusal did not name the pin")

    @check("runner-load-symlink-refused")
    def _():
        link = os.path.join(temp.path, "linked-bounded-test.py")
        os.symlink(RUNNER_PATH, link)
        try:
            load_runner_from(link)
            raise AssertionError("a symlinked runner loaded")
        except Refused as error:
            expect("no-follow" in str(error), "the refusal did not name no-follow")

    @check("runner-reader-exact-size-seam")
    def _():
        # Drive the real reader seam: the bytes hashed are exactly the bytes
        # stat'ed, so a short or grown read refuses instead of hashing wrong
        # bytes. A seam returning fewer bytes than stat'ed is refused too.
        fd = os.open(RUNNER_PATH, os.O_RDONLY | os.O_NOFOLLOW)
        try:
            real_size = os.fstat(fd).st_size
            data = read_exact_verified(fd, real_size)
            expect(len(data) == real_size, "the exact-size reader returned short bytes")
        finally:
            os.close(fd)
        fd = os.open(RUNNER_PATH, os.O_RDONLY | os.O_NOFOLLOW)
        try:
            try:
                read_exact_verified(fd, os.fstat(fd).st_size - 1)
                raise AssertionError("a short read was accepted")
            except Refused as error:
                expect("changed mid-read" in str(error), "the refusal did not name the race")
        finally:
            os.close(fd)
        fd = os.open(RUNNER_PATH, os.O_RDONLY | os.O_NOFOLLOW)
        try:
            try:
                read_exact_verified(fd, os.fstat(fd).st_size + 1)
                raise AssertionError("an over-long read was accepted")
            except Refused as error:
                expect("changed mid-read" in str(error), "the refusal did not name the race")
        finally:
            os.close(fd)
        def short_seam(fd, expected_size):
            return os.read(fd, max(1, expected_size - 1))
        try:
            load_runner_from(RUNNER_PATH, read_all=short_seam)
            raise AssertionError("a seam with changed bytes loaded")
        except Refused as error:
            expect(EXPECTED_RUNNER_SHA256 in str(error) or "changed" in str(error),
                   "the changed-bytes refusal did not name the pin or the race")

    @check("runner-no-bytecode-no-path-switch")
    def _():
        before_path = list(sys.path)
        bytecode_before = sys.dont_write_bytecode
        cache = os.path.join(REPO, "scripts", "__pycache__")
        cache_before = os.path.exists(cache)
        _module, facts = load_runner_from(RUNNER_PATH)
        expect(list(sys.path) == before_path, "the loader mutated sys.path")
        expect(sys.dont_write_bytecode == bytecode_before, "the loader left the bytecode flag changed")
        expect(facts["bytecode_disabled_during_load"], "bytecode writing was not disabled during load")
        if not cache_before:
            expect(not os.path.exists(cache), "the load wrote a bytecode cache")

    # --- source discipline of this script (AST) ---------------------------------
    @check("script-never-concludes-sweeps-cleans")
    def _():
        with open(SCRIPT_PATH, "rb") as handle:
            source = handle.read().decode("utf-8")
        tree = ast.parse(source)
        # `_remove_selftemp` and the whole self-check apparatus region below
        # legitimately mutate and remove their OWN fake fixtures and drive the
        # runner's conclusion/sweep seams as NEGATIVE controls. Every other
        # line — the whole native path — is covered by the forbidden names.
        region_start = None
        region_end = None
        for node in tree.body:
            if isinstance(node, ast.ClassDef) and node.name == "SelfTemp":
                region_start = node.lineno if region_start is None else min(region_start, node.lineno)
            if isinstance(node, ast.FunctionDef) and node.name == "_remove_selftemp":
                region_start = node.lineno if region_start is None else min(region_start, node.lineno)
            if isinstance(node, ast.Assign) and any(
                    isinstance(target, ast.Name) and target.id == "USAGE"
                    for target in node.targets):
                region_end = node.lineno
        excluded_lines = set()
        if region_start is not None and region_end is not None:
            excluded_lines.update(range(region_start, region_end))
        forbidden_names = {"record_conclusion", "remove_marked_root", "sweep_concluded_roots",
                           "sweep_one_root", "conclude_marker", "nested_concluded"}
        forbidden_attributes = {"rmtree", "unlink", "rmdir", "remove"}
        for node in ast.walk(tree):
            lineno = getattr(node, "lineno", None)
            if lineno is None or lineno in excluded_lines:
                continue
            if isinstance(node, ast.Name) and node.id in forbidden_names:
                raise AssertionError(f"forbidden name {node.id} at line {node.lineno}")
            if isinstance(node, ast.Attribute) and node.attr in forbidden_attributes:
                raise AssertionError(f"forbidden attribute {node.attr} at line {node.lineno}")
            if isinstance(node, ast.Attribute) and node.attr in forbidden_names:
                raise AssertionError(f"forbidden attribute {node.attr} at line {node.lineno}")
            if isinstance(node, ast.keyword) and node.arg == "nested_concluded":
                raise AssertionError("nested_concluded appears in this script")

    @check("script-mktemp-discipline")
    def _():
        with open(SCRIPT_PATH, "rb") as handle:
            tree = ast.parse(handle.read().decode("utf-8"))
        for node in ast.walk(tree):
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) \
                    and node.func.attr == "mkdtemp":
                keywords = {keyword.arg for keyword in node.keywords}
                expect("dir" in keywords, f"mkdtemp without dir= at line {node.lineno}")
                expect("prefix" in keywords, f"mkdtemp without an explicit template at line {node.lineno}")

    @check("script-one-deletion-site")
    def _():
        with open(SCRIPT_PATH, "rb") as handle:
            tree = ast.parse(handle.read().decode("utf-8"))
        found = [node.lineno for node in ast.walk(tree)
                 if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                 and node.func.attr == "rmtree"]
        expect(len(found) == 1, f"expected exactly one rmtree site, found {found}")

    # --- topology and separate-token refusals -----------------------------------
    def make_guard(mount_name="mount", **receipt_overrides):
        guard = FakeGuard(runner, os.path.join(temp.path, mount_name))
        if receipt_overrides:
            guard.publish_receipt(**receipt_overrides)
        return guard

    @check("topology-happy-path")
    def _():
        guard = make_guard("happy")
        ctx = validate_context(guard.environment())
        expect(ctx["root"] == guard.root, "the validated root differs")
        expect(ctx["supervisor_token"] != ctx["marker_token"], "tokens were equated")
        os.close(ctx["root_fd"])

    @check("topology-refuses-not-guard-leader")
    def _():
        for index, missing in enumerate(("OXIDE_PROOF_RUN_ID", "OXIDE_PROOF_EVIDENCE",
                                         "OXIDE_PROOF_DEADLINE_NS", "OXIDE_TASK_SCRATCH_ROOT")):
            guard = make_guard(f"missing-{index}")
            env = guard.environment()
            env.pop(missing)
            try:
                validate_context(env)
                raise AssertionError(f"missing {missing} was accepted")
            except Refused as error:
                expect(missing in str(error), f"the refusal did not name {missing}")

    @check("topology-refuses-adopted-fallback-explicit")
    def _():
        guard = make_guard("adopted-flag", scratch_owned=False)
        try:
            validate_context(guard.environment())
            raise AssertionError("adopted scratch accepted")
        except Refused as error:
            expect("owned root" in str(error), "the refusal did not name adoption")
        guard = make_guard("adopted-path")
        env = guard.environment()
        env["OXIDE_TASK_SCRATCH_ROOT"] = os.path.join(temp.path, "adopted-scratch-root")
        os.makedirs(env["OXIDE_TASK_SCRATCH_ROOT"], exist_ok=True)
        try:
            validate_context(env)
            raise AssertionError("adopted scratch path accepted")
        except Refused:
            pass
        guard = make_guard("fallback", **{"scratch_default": {"source": "fallback"}})
        try:
            validate_context(guard.environment())
            raise AssertionError("fallback selection accepted")
        except Refused as error:
            expect("selected-volume" in str(error), "the refusal did not name the selection")
        guard = make_guard("explicit-parent", **{"scratch_default": {"source": "explicit-root-parent"}})
        try:
            validate_context(guard.environment())
            raise AssertionError("explicit root parent accepted")
        except Refused as error:
            expect("selected-volume" in str(error), "the refusal did not name the selection")

    @check("topology-refuses-explicit-evidence")
    def _():
        guard = make_guard("explicit-evidence")
        env = guard.environment()
        explicit = tempfile.mkdtemp(prefix="elsewhere.", dir=temp.path)
        env["OXIDE_PROOF_EVIDENCE"] = explicit
        # A real explicit --evidence run publishes its receipt in that exact
        # directory, so the fake must too; otherwise this only proves a missing
        # receipt refuses.
        guard.publish_receipt(evidence=explicit)
        with open(os.path.join(guard.evidence, "receipt.json"), "rb") as handle:
            data = handle.read()
        fd = os.open(os.path.join(explicit, "receipt.json"),
                     os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o644)
        os.write(fd, data)
        os.close(fd)
        try:
            validate_context(env)
            raise AssertionError("explicit evidence accepted")
        except Refused as error:
            expect("--evidence" in str(error), "the refusal did not name explicit evidence: " + str(error))

    @check("topology-refuses-stale-and-foreign-tokens")
    def _():
        guard = make_guard("stale-phase", phase="unclosed: scratch_kept")
        try:
            validate_context(guard.environment())
            raise AssertionError("a stale phase accepted")
        except Refused as error:
            expect("phase" in str(error), "the refusal did not name the phase")
        guard = make_guard("foreign-token", run_id="f" * 32)
        try:
            validate_context(guard.environment())
            raise AssertionError("a foreign run token accepted")
        except Refused as error:
            expect("run token" in str(error), "the refusal did not name the run token")
        guard = make_guard("equated-tokens")
        env = guard.environment()
        guard.publish_receipt(**{"scratch_marker": {
            "path": os.path.join(guard.root, ".oxide-scratch.json"),
            "state": "written", "run_id": guard.run_id, "reason": None}})
        try:
            validate_context(env)
            raise AssertionError("an equated marker token accepted")
        except Refused as error:
            expect("separate tokens" in str(error), "the refusal did not name separate tokens")

    @check("topology-refuses-alias-modes-device")
    def _():
        guard = make_guard("alias")
        os.rmdir(guard.scratch)
        os.symlink(guard.evidence, guard.scratch)
        try:
            validate_context(guard.environment())
            raise AssertionError("a scratch alias accepted")
        except Refused:
            pass
        guard = make_guard("mode-scratch")
        os.chmod(guard.scratch, 0o775)
        try:
            validate_context(guard.environment())
            raise AssertionError("a group-writable scratch accepted")
        except Refused as error:
            expect("group/other writable" in str(error), "the refusal did not name the mode")
        guard = make_guard("mode-root")
        os.chmod(guard.root, 0o755)
        try:
            validate_context(guard.environment())
            raise AssertionError("a non-private root accepted")
        except Refused as error:
            expect("private" in str(error), "the refusal did not name the root mode")
        guard = make_guard("device")
        env = guard.environment()
        guard.publish_receipt(**{"scratch_default": dict(
            FakeGuard.publish_receipt(guard)["scratch_default"], device=guard.device + 1)})
        try:
            validate_context(env)
            raise AssertionError("a wrong device accepted")
        except Refused as error:
            expect("device" in str(error), "the refusal did not name the device")

    @check("topology-refuses-unmarked-root")
    def _():
        guard = make_guard("unmarked")
        os.unlink(guard.marker_path)
        trace = []
        try:
            establish_hosting(guard.environment(), trace)
            raise AssertionError("an unmarked root was hosted")
        except Refused as error:
            expect("strict-valid" in str(error), "the refusal did not name strict validity")
        expect("fixture-dir-created" not in trace, "a fixture directory exists after refusal")

    @check("topology-refuses-changed-marker")
    def _():
        guard = make_guard("changed-marker")
        env = guard.environment()
        ctx = validate_context(env)
        foreign = os.path.join(temp.path, "foreign-marker")
        with open(foreign, "w") as handle:
            handle.write("{}\n")
        os.replace(foreign, guard.marker_path)
        try:
            strict_marker_facts(runner, ctx)
            raise AssertionError("a corrupt marker was accepted")
        except Refused as error:
            expect("strict-valid" in str(error), "the refusal did not name strict validity")
        finally:
            os.close(ctx["root_fd"])

    # --- protection ordering, readback, fixture creation ------------------------
    @check("protect-before-create-ordering")
    def _():
        guard = make_guard("ordering")
        trace = []
        ctx = guard.host_context(trace)
        expect(trace == ["context-validated", "runner-loaded", "initial-marker-verified",
                         "protected", "hosting-readback-verified", "descriptors-revalidated",
                         "fixture-dir-created"],
               f"the sequence diverged: {trace}")
        marker = runner.read_marker(guard.root, runner.DefaultProbe())
        expect(marker["nested_scope"] == "hosting", "the marker is not hosting")
        expect(marker["outcome"] == "running" and marker["concluded_unix"] is None,
               "the marker was concluded by the fixture")
        expect(ctx["fixture"]["name"].startswith(FIXTURE_PREFIX), "the fixture name diverged")
        info = os.lstat(ctx["fixture"]["path"])
        expect(stat.S_IMODE(info.st_mode) == 0o700, "the fixture directory is not 0700")
        for leaf in ("children", "home", "tmp"):
            leaf_info = os.lstat(os.path.join(ctx["fixture"]["path"], leaf))
            expect(stat.S_IMODE(leaf_info.st_mode) == 0o700, f"{leaf} is not 0700")
        expect(not os.path.exists(os.path.join(guard.scratch, FIXTURE_PREFIX)),
               "a fixture resource exists in the auto-cleaned scratch")
        os.close(ctx["root_fd"])
        os.close(ctx["fixture"]["fd"])

    @check("protect-refusal-leaves-no-fixture-dir")
    def _():
        guard = make_guard("concluded")
        document, identity = runner.read_marker_with_identity(guard.root, runner.DefaultProbe())
        document = dict(document, concluded_unix=time.time(), outcome="closed",
                        closure="positive", kept_reason="evidence")
        runner.write_marker(guard.root, document, runner.DefaultProbe(), expected=identity)
        trace = []
        try:
            guard.host_context(trace)
            raise AssertionError("a concluded root was hosted")
        except Refused as error:
            expect("unconcluded running" in str(error), "the refusal did not name the conclusion")
        expect("fixture-dir-created" not in trace, "a fixture directory exists after refusal")
        expect(trace == ["context-validated", "runner-loaded"],
               f"the trace diverged after the refusal: {trace}")

    @check("protect-readback-detects-replacement")
    def _():
        guard = make_guard("replaced")
        document, identity = runner.read_marker_with_identity(guard.root, runner.DefaultProbe())
        runner.write_marker(guard.root, dict(document, nested_scope="hosting"),
                            runner.DefaultProbe(), expected=identity)
        marker_info = os.lstat(guard.marker_path)
        runner.write_marker(guard.root, dict(document, nested_scope="hosting",
                                             run_id=secrets.token_hex(16)),
                            runner.DefaultProbe(), expected=(marker_info.st_dev, marker_info.st_ino))
        ctx = validate_context(guard.environment())
        trace = []
        try:
            protect_and_verify(runner, ctx, trace)
            raise AssertionError("a replaced marker passed the readback")
        except Refused as error:
            expect("marker token" in str(error), "the refusal did not name the token")
        finally:
            os.close(ctx["root_fd"])

    @check("fixture-dir-exclusive-and-held")
    def _():
        guard = make_guard("exclusive")
        trace = []
        ctx = guard.host_context(trace)
        fd = ctx["fixture"]["fd"]
        expect(os.lstat(ctx["fixture"]["path"]).st_ino == os.fstat(fd).st_ino,
               "the held descriptor is not the fixture directory")
        evidence_fd = os.open(guard.evidence, os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)
        try:
            os.mkdir(ctx["fixture"]["name"], 0o700, dir_fd=evidence_fd)
            raise AssertionError("a second exclusive creation succeeded")
        except OSError:
            pass
        finally:
            os.close(evidence_fd)
        os.close(fd)

    # --- nested outcome model ---------------------------------------------------
    @check("nested-unknown-never-becomes-closed")
    def _():
        completed = [{"name": "c", "kind": "native", "status": CASE_STATUS_COMPLETED}]
        all_reaped = [{"reaped": True}]
        good = derive_nested_outcome(all_reaped, completed, receipt_ok=True)
        expect(good["outcome"] == "nested-closed", "a fully settled run is not nested-closed")
        expect(good["writer_closure"] == "positively-reaped-all-owned-children",
               "the writer closure drifted")
        for children_rows, cases, receipt_ok, label in (
            (all_reaped, [{"name": "c", "kind": "native", "status": CASE_STATUS_FAILED}],
             True, "a failed case"),
            ([{"reaped": True}, {"reaped": False}], completed, True, "an unreaped child"),
            (all_reaped, completed, False, "a lost receipt"),
            (all_reaped, [{"name": "s", "kind": "source-only", "status": CASE_STATUS_SOURCE_ONLY}],
             True, "source-only rows only"),
            ([], completed, True, "no children at all"),
        ):
            outcome = derive_nested_outcome(children_rows, cases, receipt_ok=receipt_ok)
            expect(outcome["outcome"] == "unknown", f"{label} turned into {outcome['outcome']}")

    # --- evidence survives scratch cleanup --------------------------------------
    @check("evidence-survives-scratch-cleanup")
    def _():
        with open(RUNNER_PATH, "rb") as handle:
            tree = ast.parse(handle.read().decode("utf-8"))
        monitor_run = None
        for node in ast.walk(tree):
            if isinstance(node, ast.ClassDef) and node.name == "Monitor":
                for item in node.body:
                    if isinstance(item, ast.FunctionDef) and item.name == "run":
                        monitor_run = item
        expect(monitor_run is not None, "the runner's Monitor.run moved")
        sites = []
        for node in ast.walk(monitor_run):
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) \
                    and node.func.attr == "rmtree" and node.args:
                target = node.args[0]
                if isinstance(target, ast.Subscript) and isinstance(target.value, ast.Name) \
                        and isinstance(target.slice, ast.Constant):
                    sites.append((target.value.id, target.slice.value))
                else:
                    sites.append((type(target).__name__, "?"))
        expect(sites == [("ctx", "scratch")],
               f"the runner's cleanup decision does not remove only ctx['scratch']: {sites}")
        guard = make_guard("evidence-placement")
        trace = []
        ctx = guard.host_context(trace)
        expect(ctx["fixture"]["path"].startswith(guard.evidence + os.sep),
               "the fixture directory is not under evidence")
        expect(not ctx["fixture"]["path"].startswith(guard.scratch),
               "the fixture directory is under the auto-cleaned scratch")
        os.close(ctx["root_fd"])
        os.close(ctx["fixture"]["fd"])

    @check("protected-root-ineligible-for-expiry")
    def _():
        guard = make_guard("expiry")
        runner.protect_nested_scope(guard.root, runner.DefaultProbe())
        budget = runner.SweepBudget()
        verdict = runner.sweep_one_root(guard.root, guard.device, runner.DefaultProbe(),
                                        budget, time.time())
        expect(verdict == "unconcluded", f"a hosting unconcluded root swept as {verdict}")
        try:
            runner.record_conclusion(guard.root, guard.marker_token, runner.DefaultProbe(),
                                     "closed", "positive", "evidence")
            raise AssertionError("a hosting root concluded from outer facts")
        except runner.MarkerInvalid as invalid:
            expect(invalid.code == "nested-unconcluded", f"the refusal was {invalid.code}")
        document = runner.read_marker(guard.root, runner.DefaultProbe())
        expect(document["outcome"] == "running" and document["nested_scope"] == "hosting",
               "the hosting marker changed")

    # --- bounded receipts and caps ----------------------------------------------
    def receipt_totals():
        return {"cases_total": 0, "cases_omitted": 0, "signals_total": 0,
                "signals_omitted": 0, "identities_total": 0, "identities_omitted": 0}

    @check("receipt-caps-and-omissions")
    def _():
        fixture_dir = tempfile.mkdtemp(prefix="receipt-caps.", dir=temp.path)
        fd = os.open(fixture_dir, os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)
        try:
            receipt = NestedReceipt(fd, {"schema": 1, "totals": receipt_totals()})
            for index in range(CASE_ROWS_MAX + 8):
                receipt.add_case({"case": f"case-{index}", "status": "completed",
                                  "detail": "d" * (DETAIL_CAP + 50), "at_ns": index})
            for index in range(SIGNAL_ROWS_MAX + 5):
                receipt.add_signal({"sig": 0, "result": "0", "call": f"probe-{index}"})
            for index in range(IDENTITY_ROWS_MAX + 11):
                receipt.add_identity({"transition": "spawned", "pid": index})
            with open(os.path.join(fixture_dir, RECEIPT_NAME), encoding="utf-8") as handle:
                persisted = json.load(handle)
            expect(len(persisted["identities"]) == IDENTITY_ROWS_MAX
                   and all(isinstance(row.get("at_ns"), int) for row in persisted["identities"]),
                   "identity transitions were not timestamped and published")
            expect(len(receipt.document["cases"]) == CASE_ROWS_MAX, "case rows over cap")
            expect(len(receipt.document["signals"]) == SIGNAL_ROWS_MAX, "signal rows over cap")
            expect(len(receipt.document["identities"]) == IDENTITY_ROWS_MAX, "identity rows over cap")
            totals = receipt.document["totals"]
            expect(totals["cases_total"] == CASE_ROWS_MAX + 8 and totals["cases_omitted"] == 8,
                   "case omissions drifted")
            expect(totals["identities_total"] == IDENTITY_ROWS_MAX + 11
                   and totals["identities_omitted"] == 11, "identity omissions drifted")
            expect(all(len(row["detail"]) <= DETAIL_CAP for row in receipt.document["cases"]),
                   "details were not bounded")
            expect(receipt.publish(), "the capped receipt did not publish")
            size = os.stat(RECEIPT_NAME, dir_fd=fd, follow_symlinks=False).st_size
            expect(size <= RECEIPT_MAX_BYTES, f"the published receipt is {size} bytes")
        finally:
            os.close(fd)

    @check("receipt-unwritable-aborts")
    def _():
        fixture_dir = tempfile.mkdtemp(prefix="receipt-readonly.", dir=temp.path)
        os.chmod(fixture_dir, 0o500)
        fd = os.open(fixture_dir, os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)
        try:
            receipt = NestedReceipt(fd, {"schema": 1, "totals": receipt_totals()})
            expect(receipt.publish() is False, "an unwritable receipt published")
            expect(receipt.write_error, "the write error was not recorded")
        finally:
            os.close(fd)
            os.chmod(fixture_dir, 0o700)

    # --- environment scrub and model refusals -----------------------------------
    @check("child-env-scrubbed")
    def _():
        fixture_dir = os.path.join(temp.path, "fixture-env")
        os.makedirs(fixture_dir, exist_ok=True)
        hostile = {
            "PATH": "/usr/bin:/bin", "HOME": "/Users/tom", "TMPDIR": "/var/folders/xx",
            "OXIDE_POOL": "/somewhere/pool", "OXIDE_CONFIG": "/somewhere/config",
            "OXIDE_TEST_HOME_FROM_ENV": "1", "OXIDE_TASK_SCRATCH_ROOT": "/auto/cleaned/scratch",
            "XDG_CONFIG_HOME": "/Users/tom/.config", "CARGO_HOME": "/Users/tom/.cargo",
            "SCCACHE_DIR": "/Users/tom/.cache/sccache", "RUSTUP_HOME": "/Users/tom/.rustup",
            "RUSTC_WRAPPER": "sccache", "SHELL": "/bin/zsh",
        }
        env = child_environment(hostile, fixture_dir)
        expect(is_scrubbed(env, fixture_dir), "the scrubbed env leaks a dropped key")
        expect(env.get("HOME") == os.path.join(fixture_dir, "home"), "HOME is not fixture-local")
        expect(env.get("TMPDIR") == os.path.join(fixture_dir, "tmp"), "TMPDIR is not fixture-local")
        expect(env.get("PYTHONDONTWRITEBYTECODE") == "1", "bytecode writing not disabled for children")
        expect(env.get("PATH") == "/usr/bin:/bin", "PATH was dropped")
        for dropped in ("OXIDE_POOL", "OXIDE_CONFIG", "OXIDE_TEST_HOME_FROM_ENV",
                        "OXIDE_TASK_SCRATCH_ROOT", "XDG_CONFIG_HOME", "CARGO_HOME",
                        "SCCACHE_DIR", "RUSTUP_HOME", "RUSTC_WRAPPER", "SHELL"):
            expect(dropped not in env, f"{dropped} survived the scrub")

    @check("custody-model-refusals-make-no-syscall")
    def _():
        fixture_dir = tempfile.mkdtemp(prefix="model.", dir=temp.path)
        fd = os.open(fixture_dir, os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)
        try:
            receipt = NestedReceipt(fd, {"schema": 1, "totals": receipt_totals()})
            child = HeldChild(runner, receipt, "model-case", "hold", os.getpid() + 999_999,
                              os.path.join(fixture_dir, "r.json"), "log", "stop")
            child.state = "settled"
            child.reaped = True
            before = len(child.probes)
            try:
                child.signal_group(signal.SIGTERM)
                raise AssertionError("a settled child was signalled")
            except CustodyLost:
                expect(len(child.probes) == before, "the signal refusal made a syscall")
            try:
                child.reap(0.1)
                raise AssertionError("a settled child was re-reaped")
            except CustodyLost:
                expect(len(child.probes) == before, "the reap refusal made a syscall")
            refusals = [row for row in receipt.document["signals"]
                        if str(row.get("result")).startswith("refused")]
            expect(len(refusals) == 2, f"the refusal rows are missing: {refusals}")
            expect(all(row.get("syscall_made") is False for row in refusals),
                   "a refusal row claims a syscall")
        finally:
            os.close(fd)

    @check("vacuous-zero-probes-documented-in-model")
    def _():
        # kill(0, 0) / killpg(0, 0) answer against the caller's own scope, so
        # they carry no information about a held identity. This must hold on
        # this host for the c08 oracle to be meaningful.
        expect(os.kill(0, 0) is None, "kill(0, 0) did not succeed")
        expect(os.killpg(0, 0) is None, "killpg(0, 0) did not succeed")

    return checks


# ---------------------------------------------------------------------------
# argument dispatch


USAGE = """usage:
  test-cargo-group-custody.py                 counted native proof under the qualified supervisor
  test-cargo-group-custody.py --diagnostic    uncounted keep-going collection (never a PASS)
  test-cargo-group-custody.py --plan          effect-free case table, bounds and dispositions
  test-cargo-group-custody.py --self-check    pure apparatus check (one removed temp dir)
"""


def main(argv):
    if argv == []:
        return run_native(diagnostic=False)
    if argv == ["--diagnostic"]:
        return run_native(diagnostic=True)
    if argv == ["--plan"]:
        print(json.dumps(plan_document(), indent=2, sort_keys=True))
        return 0
    if argv == ["--self-check"]:
        return self_check()
    if argv in (["--help"], ["-h"]):
        print(USAGE, end="")
        return 0
    print(f"cargo-group-custody: unknown arguments {argv}", file=sys.stderr)
    print(USAGE, end="", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
