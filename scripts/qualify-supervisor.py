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
budget around `scripts/test_supervisor.py` and **no** `--root-parent` and no
`--evidence`, so the candidate's default scratch routing is what runs: one
owned root under `<mount>/scratch/oxide` on the volume the machine's
effect-free pool selection names, with its evidence directory inside that same
owned root. The bootstrap refuses before any effect when that selection does
not resolve to an eligible non-boot volume, because a qualification that only
exercised the `/private/tmp` fallback would not exercise the default. This
bootstrap prepares that owned parent through the candidate's own seam (missing
components created 0700; an unsafe existing component refuses), runs one
bounded startup self-expiry sweep there, and puts its own marked base in the
same parent. Because the suite's nested real-case directories live in that
evidence directory, every nested case resource, its retained receipts and any
kept UNCLOSED scratch stay inside the candidate's monitored root on the
selected volume. The suite records that layout in `topology.json` under this
bootstrap's base (also on the selected volume, under the bootstrap's own
never-delete retention), and the bootstrap verifies it against kernel facts
and the candidate's receipt rather than trusting it, including that the root is
inside the owned subtree and not at the drive root.

Owner markers and expiry: the base and the candidate root each carry one
private `.oxide-scratch.json` written before any run resource. A retained
nested UNCLOSED fixture keeps the candidate root unconcluded with the honest
`nested-unclosed` reason even when the outer session closed and the suite
passed; a failed qualification keeps the base unconcluded. Only a positively
concluded root strictly older than seven days is ever eligible for the bounded
startup sweep, which the candidate and this bootstrap each run once for their
own new resources. Nothing else is deleted and no marker is a liveness probe.
Pass requires: candidate exit 0, receipt phase `monitor_complete`, closure
established with command status 0 and no fault, the default scratch on the
selected volume with the selected volume's UUID, the receipt and the suite's
topology agreeing on one owned root whose scratch and evidence are its
members, every nested case directory inside that root with its receipt still
present after cleanup, a reserve sampled on the scratch's own device, reserve
sampling from before the gate with no gap over 1 s, the fixture writers
counted, custody at every signal, and every expected case passing. Reserve
readings identify the qualification's scratch volume; the boot volume is read
and labelled separately.
On overrun it TERMs, then KILLs, only its own unreaped candidate child.
Nothing is ever deleted: nested UNCLOSED cases leave live fixture processes
whose only bound is their fallback, which is not closure, so the whole
scratch base is kept and its path printed for the owner.
"""
from __future__ import annotations

import hashlib
import importlib.util
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
EXPECTED_CASES = 309  # 259 pure + 50 real (oxide#151 added the routing, marker,
                      # nested-window protection, bounded-sweep, qualify-entry,
                      # shared-ancestor/private-leaf and readiness cases; oxide#191
                      # adds fifty pure post-census detach cases and seven real
                      # ones: success, a refused identity, a failed attempt, an
                      # unrequested attachment whose opt-in custody keep is recorded
                      # separately, a stuck child the custodian closes itself, and the
                      # two unreadable-identity paths — closed by the monitor or
                      # handed off by parentage)
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
    "Host free space is a sampled abort threshold (0.25 s, 22 GiB = 20 GiB reserve + 2 GiB margin) read on the filesystem actually holding the scratch, so on the selected drive it is that volume's free space, not the internal disk's; no instantaneous guarantee. Owned bytes are walked every 2 s against a monitored budget, not a hard ceiling.",
    "The default scratch parent is the owned subtree <mount>/scratch/oxide, read passively from OXIDE_POOL, the user config file's [pool] path or the built-in pool, and the pool's removable registration, with one kernel volume-UUID read; that is not the product's verified build-time selection (no diskutil container/role check), the config reader understands only a simple [pool] path entry, and anything unreadable or unsupported falls back to /private/tmp with a warning. The bootstrap refuses to qualify on a fallback.",
    "Scratch on the selected drive is created as a new private oxide-proof.* directory under\n    <mount>/scratch/oxide and is kept on a failed or unclosed run exactly like the /private/tmp\n    default; the harness never removes any other directory on that volume. The bootstrap's own\n    base is one more marked directory in the same owned parent, kept by the bootstrap, which\n    deletes nothing.",
    "Owner markers and bounded self-expiry: each run root in the owned parent carries one private\n    .oxide-scratch.json written before any child or run resource. The fixed startup sweep\n    (1 s and 4096 steps, checked between filesystem steps) removes only real owner-marked roots\n    whose marker records a positive conclusion of all their owned resources strictly older than\n    seven days; unmarked, foreign, corrupt, future-dated, unconcluded, nested-UNCLOSED or\n    otherwise non-expirable roots are never touched, and an exhausted budget leaves the rest for\n    a later run. There is no background reaper, and a partial deletion keeps the marker and the\n    root. The marker is not a liveness probe.",
    "The candidate root that holds a retained nested UNCLOSED fixture stays unconcluded\n    (nested-unclosed) even when the outer session closed and the suite passed; a failed\n    qualification keeps the bootstrap base unconcluded. Neither can expire while that holds.",
    "Deadlines are enforced by a single-threaded custodian on CLOCK_UPTIME_RAW with bounded calls; no hard real-time scheduling claim.",
    "The candidate's settlement diagnostics (receipt `settlement` and the custodian's decision-time `settlement` record) are bounded observations, not acceptance gates: they never decide classification, closure, EOF, the deadline or cleanup, and a lost or truncated observation only means missing evidence. The receipt object keeps accumulating after the decision, so only the custodian's record is the decision-time view.",
    "macOS only (kqueue NOTE_EXIT, sysctl kinfo_proc at self-checked offsets); Linux is not qualified.",
    "Effigy admission waits happen before this script starts and are not inside its 300 s.",
    "Nested candidate sessions are outside the outer session; their UNCLOSED cases are settled by no guardian. Their case directories, receipts and kept UNCLOSED scratch live in the outer candidate's own evidence directory inside its selected-volume owned root, so the outer monitored walk and 512 MiB budget cover them and cleanup never removes them; their fixture processes end only at the finite fallback, which is not closure.",
    "The outer actor KILLs only its pinned worker at the deadline; a candidate still running then is bounded by its own 280 s limit, not by the actor.",
    "Synthetic fixtures model Cargo's setpgid groups; no real Cargo build or live application ran.",
    "Owned bytes are a non-atomic best-effort walk: an entry past the platform path limit is reached one descriptor-relative component at a time from its nearest nameable directory, and any error other than a vanished entry is a measurement fault, never a smaller total.",
]


def now_ns():
    return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)


START = now_ns()
DEADLINE = START + WALL_NS


def free_bytes(path):
    sample = os.statvfs(path)
    return sample.f_bavail * sample.f_frsize


MAX_DOCUMENT = 1024 * 1024  # topology record, cases and registration are far smaller


class KernelFacts:
    """The filesystem, device and document facts the pass checks read. Tests
    substitute their own, so the topology rules run without a real drive."""

    def device(self, path):
        return os.stat(path).st_dev

    def realpath(self, path):
        return os.path.realpath(path)

    def boot_device(self):
        return os.stat("/").st_dev

    def free(self, path):
        return free_bytes(path)

    def document(self, path):
        with open(path, "rb") as handle:
            return json.loads(handle.read(MAX_DOCUMENT).decode("utf-8"))


def reserve_readings(scratch_volume, boot_volume, facts=None):
    """Free space on the volume that actually holds the qualification's
    scratch, and separately on the boot volume. Both are labelled: the scratch
    volume is the one this qualification depends on, the boot volume keeps the
    checkout and the harness."""
    facts = facts if facts is not None else KernelFacts()
    return {"scratch_volume": {"path": scratch_volume, "free": facts.free(scratch_volume), "threshold": THRESHOLD},
            "boot_volume": {"path": boot_volume, "free": facts.free(boot_volume), "threshold": THRESHOLD}}


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


CANDIDATE = []


def candidate():
    """The candidate guard's own module: this bootstrap reuses its routing,
    preparation, marker and sweep seams instead of copying their rules."""
    if not CANDIDATE:
        spec = importlib.util.spec_from_file_location("bounded_test", SOURCES["supervisor"])
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        CANDIDATE.append(module)
    return CANDIDATE[0]


def selection():
    """The candidate's own default routing, resolved here before anything is
    created: this bootstrap refuses rather than spend a native run that could
    not exercise it."""
    return candidate().select_default_parent(os.environ)


def scratch_parent(mount):
    return candidate().scratch_parent(mount)


def registration(path):
    """The recorded registration this bootstrap reads for itself (bounded)."""
    with open(path, "rb") as handle:
        return json.loads(handle.read(65536).decode("utf-8"))


def canonical_uuid(value):
    """A volume UUID in either spelling the system uses, lowercased for
    comparison, or None when it is missing, not a string or not a UUID.

    The product writes the registered UUID uppercase and the kernel reports
    `ATTR_VOL_UUID` lowercase; the supervisor's own selector lowercases the
    recorded value before comparing. This independent check must accept the
    same identity in either spelling and still refuse a missing, malformed or
    different one."""
    if not isinstance(value, str) or len(value) != 36:
        return None
    pieces = value.split("-")
    if [len(piece) for piece in pieces] != [8, 4, 4, 4, 12]:
        return None
    if any(character not in "0123456789abcdefABCDEF" for piece in pieces for character in piece):
        return None
    return value.lower()


def topology(base, expected, facts=None):
    """The suite's own record of the layout it ran in: the candidate's owned
    root, its scratch and its evidence directory. Every nested real-case
    directory lives in that evidence directory, so dropping the external
    `--evidence` puts every nested case resource, its retained receipts and
    its kept UNCLOSED scratch inside the candidate's monitored root on the
    selected volume. Nothing here is taken on trust: the record must be
    canonical and directly on the selected volume, and the receipt must agree
    with it."""
    facts = facts if facts is not None else KernelFacts()
    path = os.path.join(base, "topology.json")
    try:
        record = facts.document(path)
    except (OSError, ValueError) as error:
        return None, [f"the suite topology record {path} is missing or malformed: {error!r}"]
    if not isinstance(record, dict):
        return None, [f"the suite topology record is not an object: {record!r}"]
    scratch, evidence = record.get("scratch"), record.get("evidence")
    if not isinstance(scratch, str) or not isinstance(evidence, str):
        return None, [f"the suite topology record has no scratch and evidence: {record!r}"]
    problems = []
    root = os.path.dirname(scratch)
    parent = expected.get("root_parent")
    if facts.realpath(scratch) != scratch or facts.realpath(evidence) != evidence:
        problems.append(f"the suite scratch or evidence is not canonical: {scratch} {evidence}")
    if os.path.dirname(root) != parent:
        problems.append(f"the suite's owned root {root} is not directly on the selected volume {parent}")
    if os.path.dirname(evidence) != root or os.path.dirname(scratch) != root:
        problems.append(f"the suite scratch and evidence are not members of one owned root: {root}")
    try:  # a root or parent that is absent is a problem, never an exception
        device, parent_device = facts.device(root), facts.device(parent)
    except OSError as error:
        return None, problems + [f"the suite's owned root or the selected volume is unreadable: {error!r}"]
    if device != parent_device:
        problems.append(f"the suite's owned root is not on the selected volume's device: {root}")
    return ({"root": root, "scratch": scratch, "evidence": evidence,
             "protection": record.get("protection")}, problems)


# The suite also records three controls that never run a nested supervisor, so
# they carry no case directory and no receipt. Each shape is fixed here: an
# unrecognised control, an extra field (such as a dir it does not have) or a
# different exit is a problem, never an exemption.
CONTROL_CASES = {
    "bootstrap_deadline_blocked_stdout": {"exit": 124, "keys": {"case", "exit", "wall_s"}},
    "direct_runner_setup_blocked": {"exit": 125,
                                    "keys": {"case", "exit", "wall_s", "kept"},
                                    "kept": True, "receipt": False},
    "fixture_deadline": {"exit": [-14, -14], "keys": {"case", "exit"}},
}
ORDINARY_KEYS = {"case", "exit", "phase", "dir", "retained_unclosed", "outcome", "census", "wall_s",
                 "signals", "stale_latency_ns", "held_streams", "nonce_ok",
                 "detach",  # oxide#191: the post-census detach outcome that case recorded
                 "cleanup"}  # oxide#191: the fail-closed pre-cleanup keep, when it fired


def control_case(case, name, root, facts):
    """One of the suite's own controls: exact shape, exact exit, and for the
    direct setup refusal one retained path with no receipt, since a setup
    blockage must never claim one."""
    problems, spec = [], CONTROL_CASES[name]
    extra = set(case) - spec["keys"]
    missing = spec["keys"] - set(case)
    if extra:
        problems.append(f"control {name} recorded an unmodelled field: {sorted(extra)}")
    if missing:
        problems.append(f"control {name} is missing a field: {sorted(missing)}")
    if case.get("exit") != spec["exit"]:
        problems.append(f"control {name} exit {case.get('exit')} is not the recorded {spec['exit']}")
    if not spec.get("kept"):
        return problems
    kept = case.get("kept")
    if not isinstance(kept, str) or root is None:
        return problems + [f"control {name} has no retained path inside the monitored root: {kept}"]
    if not kept.startswith(root + "/"):
        problems.append(f"control {name} retained path is outside the monitored root: {kept}")
    try:
        canonical = facts.realpath(kept) == kept
        device, root_device = facts.device(kept), facts.device(root)
    except OSError as error:
        return problems + [f"control {name} retained path is unreadable: {error!r}"]
    if not canonical:
        problems.append(f"control {name} retained path is not canonical: {kept}")
    if device != root_device:
        problems.append(f"control {name} retained path is not on the monitored root's device: {kept}")
    if spec.get("receipt") is False:
        receipt = os.path.join(kept, "evidence", "receipt.json")
        if os.path.lexists(receipt):
            problems.append(f"control {name} claims a receipt although setup was refused: {receipt}")
    return problems


def nested_cases(results, record, facts=None):
    """Every nested real case must run inside the monitored owned root and keep
    its receipt there after the candidate's cleanup; the evidence directory is
    never removed, so an UNCLOSED case's resources stay kept under the
    candidate's accounting rather than escaping to the internal disk. The
    suite's three controls are validated by exact recorded shape instead of
    being exempted: an ordinary case without a dir or receipt, an unknown
    control, an unmodelled field, or a retained path that escapes, is
    non-canonical or on another device is a problem."""
    facts = facts if facts is not None else KernelFacts()
    problems = []
    cases = results.get("cases") or []
    root = (record or {}).get("root")
    if not cases:
        problems.append("no nested case directory was recorded")
    for case in cases:
        name = case.get("case")
        if name in CONTROL_CASES:
            problems += control_case(case, name, root, facts)
            continue
        unexpected = set(case) - ORDINARY_KEYS
        if unexpected:
            problems.append(f"nested case {name} recorded an unmodelled field: {sorted(unexpected)}")
        directory = case.get("dir")
        if not isinstance(directory, str) or root is None or not directory.startswith(root + "/"):
            problems.append(f"nested case {name} ran outside the monitored root: {directory}")
            continue
        if not os.path.exists(os.path.join(directory, "evidence", "receipt.json")):
            problems.append(f"nested case evidence missing after cleanup: {name} {directory}")
    return problems


def default_scratch(receipt, expected, record=None, facts=None):
    """The changed default must have run on the selected volume: the
    candidate's claim is cross-checked against this bootstrap's own facts
    (the registration document, kernel devices and containment), not taken
    on trust, and a run that fell back to /private/tmp fails. The suite's
    topology record supplies the actual root, scratch and evidence, which
    must agree with the candidate's receipt and stay inside the selected
    volume."""
    facts = facts if facts is not None else KernelFacts()
    problems = []
    selection = receipt.get("scratch_default") or {}
    if selection.get("source") != "selected-volume":
        return [f"the candidate default did not use the selected volume: {selection}"]
    if expected.get("source") != "selected-volume":
        return [f"this bootstrap could not resolve the selected volume: {expected}"]
    if record is None:
        return ["the suite topology record is unavailable, so no nested resource is proven inside the root"]
    root = record["root"]
    mount = expected.get("mount")
    if isinstance(mount, str):
        # The routing change must be exercised: the parent is the owned subtree
        # of the selected volume, never the drive root itself.
        if expected.get("root_parent") != scratch_parent(mount):
            problems.append(f"the expected scratch parent {expected.get('root_parent')} is not"
                            f" {scratch_parent(mount)}")
        if root == mount or os.path.dirname(root) == mount:
            problems.append(f"the candidate root {root} is at the drive root, not inside"
                            f" {scratch_parent(mount)}")
        if not root.startswith(scratch_parent(mount) + "/"):
            problems.append(f"the candidate root {root} is not inside {scratch_parent(mount)}")
    if receipt.get("root") != root or receipt.get("scratch") != record["scratch"]:
        problems.append(f"the candidate receipt {receipt.get('root')} {receipt.get('scratch')}"
                        f" is not the suite's recorded layout {root} {record['scratch']}")
    if receipt.get("evidence") != record["evidence"]:
        problems.append(f"the candidate evidence {receipt.get('evidence')} is not the suite's"
                        f" recorded {record['evidence']}")
    if selection.get("root_parent") != expected.get("root_parent"):
        problems.append(f"candidate default parent {selection.get('root_parent')} is not the selected volume"
                        f" {expected.get('root_parent')}")
    if not root.startswith(str(selection.get("root_parent")) + "/"):
        problems.append(f"candidate root {root} is not directly under the selected volume")
    try:
        device = facts.device(root)
        mount_device = facts.device(selection.get("root_parent"))
        recorded = registration(selection.get("registration"))
        pool = facts.realpath(selection.get("pool"))
    except (OSError, ValueError, KeyError, TypeError) as error:
        return problems + [f"independent default-scratch facts unavailable: {error!r}"]
    if device == facts.boot_device():
        problems.append("the candidate scratch is on the boot volume")
    if device != mount_device:
        problems.append(f"candidate scratch device {device} is not the selected volume's {mount_device}")
    observed, registered = canonical_uuid(selection.get("volume_uuid")), canonical_uuid(
        recorded.get("volume_uuid"))
    if observed is None or registered is None:
        problems.append("the observed or registered volume UUID is missing or malformed:"
                        f" {selection.get('volume_uuid')!r} {recorded.get('volume_uuid')!r}")
    elif observed != registered:
        problems.append(f"candidate volume UUID {selection.get('volume_uuid')} is not the recorded"
                        f" {recorded.get('volume_uuid')}")
    if selection.get("mount") != recorded.get("mount_point"):
        problems.append(f"candidate mount {selection.get('mount')} is not the registered"
                        f" {recorded.get('mount_point')}")
    if root.startswith(pool + "/"):
        problems.append(f"candidate scratch {root} is inside the pool payload {pool}")
    reserve = receipt.get("reserve") or {}
    if reserve.get("path") is None or reserve.get("device") != device:
        problems.append(f"reserve is not sampled on the scratch's own device: {reserve.get('path')}"
                        f" {reserve.get('device')} != {device}")
    if not reserve.get("minimum"):
        problems.append("no reserve minimum was recorded on the selected volume")
    return problems


def scratch_sweep(receipt, expected, base=None, facts=None):
    """The candidate's own bounded startup sweep must have run on the owned
    parent, recorded its bounded summary, and left the bootstrap's unconcluded
    base in place. A closed candidate must also have recorded its owner-marker
    conclusion. Nothing here is taken on trust from prose: every field is the
    candidate's own receipt and the base is checked against kernel facts."""
    facts = facts if facts is not None else KernelFacts()
    problems = []
    sweep = receipt.get("scratch_sweep")
    if not isinstance(sweep, dict):
        return ["the candidate receipt records no bounded startup sweep"]
    if sweep.get("parent") != expected.get("root_parent"):
        problems.append(f"the sweep parent {sweep.get('parent')} is not the owned parent"
                        f" {expected.get('root_parent')}")
    for key in ("examined", "removed_total", "skipped_total", "steps"):
        value = sweep.get(key)
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            problems.append(f"the sweep summary has no valid {key}: {value!r}")
    if sweep.get("stop") not in (None, "entries", "time", "parent-absent", "parent-unreadable"):
        problems.append(f"the sweep stopped for an unrecognised reason: {sweep.get('stop')}")
    if isinstance(base, str):
        try:
            facts.device(base)
        except OSError as error:
            return problems + [f"the bootstrap base {base} is gone after the candidate sweep: {error!r}"]
    return problems


def protected_root(receipt, record, probe=None):
    """The candidate root must have been durably protected as `hosting` by the
    suite BEFORE any nested case could exist, and the candidate must therefore
    have deferred its own conclusion. Nothing is inferred from the outer
    closure: the marker and the suite's own protection record have to agree on
    one run identity."""
    guard = candidate()
    probe = probe if probe is not None else guard.DefaultProbe()
    problems = []
    marker = receipt.get("scratch_marker") or {}
    state = marker.get("state")
    if (receipt.get("outcome") or {}).get("closure") == "closed" and state != "nested-protected":
        problems.append(f"the candidate did not defer its marker to the nested window: {marker}")
    if state == "nested-protected" and marker.get("reason") != "nested-unconcluded":
        problems.append(f"the candidate marker state has an inconsistent reason: {marker}")
    protection = (record or {}).get("protection")
    if not isinstance(protection, dict):
        return problems + ["the suite recorded no nested-window protection"]
    if protection.get("state") != "protected":
        problems.append(f"the suite did not protect its outer root: {protection}")
        return problems
    if record is not None and protection.get("root") != record.get("root"):
        problems.append(f"the suite protected {protection.get('root')}, not its own root"
                        f" {record.get('root')}")
    try:
        document = guard.read_marker(record["root"], probe)
    except (KeyError, TypeError):
        return problems + [f"the suite record has no root to check: {record!r}"]
    except (guard.MarkerInvalid, OSError) as error:
        return problems + [f"the candidate root marker is unreadable: {error!r}"]
    if document["nested_scope"] != "hosting":
        problems.append(f"the candidate root is not protected as hosting: {document['nested_scope']}")
    if document["run_id"] != protection.get("run_id"):
        problems.append(f"the candidate marker run {document['run_id']} is not the suite's"
                        f" protected run {protection.get('run_id')}")
    return problems


def marker_state_of(guard, probe, root):
    """One marker's recorded state, read back after the transition: a bounded
    statement, never a claim of closure."""
    if not isinstance(root, str):
        return {"state": "none"}
    try:
        document = guard.read_marker(root, probe)
    except (guard.MarkerInvalid, OSError) as error:
        return {"state": "unreadable",
                "reason": getattr(error, "code", None) or type(error).__name__}
    return {"state": document["outcome"], "closure": document["closure"],
            "kept_reason": document["kept_reason"], "nested_scope": document["nested_scope"],
            "concluded": document["concluded_unix"] is not None,
            "run_id": document["run_id"]}


def finish_candidate(guard, probe, record, nested_unclosed, protection):
    """The candidate root's own marker transition, at one authority and now.

    The suite protected the root as `hosting` before any nested case existed, so
    the candidate's outer closure was never a whole-owned-resource conclusion.
    With retained nested UNCLOSED fixtures the root stays truthfully unconcluded
    (`retained`, kept-reason `nested-unclosed`); only when every nested case
    concluded does this bootstrap record the positive conclusion. Every refusal
    is a problem, never a silent pass, and an unmarked root (never expirable)
    is left alone."""
    guard_problems = []
    try:
        document = guard.read_marker(record["root"], probe)
    except guard.MarkerInvalid as invalid:
        if invalid.code == "absent":
            return guard_problems, False  # unmarked roots are never eligible
        return [f"the candidate root marker is unreadable: {invalid.code}"], False
    except OSError as error:
        return [f"the candidate root marker is unreadable: {error!r}"], False
    hosting = document["nested_scope"] == "hosting"
    if nested_unclosed:
        if not hosting:
            return ([f"the candidate root holds retained nested UNKNOWN resources but is not"
                     f" protected as hosting: {document['nested_scope']}"], False)
        try:
            guard.record_conclusion(record["root"], document["run_id"], probe, "retained",
                                    "none", "nested-unclosed")
        except (guard.MarkerInvalid, OSError) as error:
            return ([f"the candidate root could not record its nested-unknown retention: {error!r}"],
                    False)
        return guard_problems, False
    if document["concluded_unix"] is not None:
        if hosting:
            return ([f"the candidate root was concluded from its outer closure while protected as"
                     f" hosting: {document}"], False)
        return guard_problems, True
    if not hosting:
        return ([f"the candidate root is neither protected nor concluded: {document['nested_scope']}"],
                False)
    if not isinstance(protection, dict) or protection.get("state") != "protected" \
            or protection.get("run_id") != document["run_id"]:
        return ([f"the candidate root's protection record does not match its marker: {protection}"],
                False)
    try:
        guard.record_conclusion(record["root"], document["run_id"], probe, "closed", "positive",
                                "evidence", nested_concluded=True)
    except (guard.MarkerInvalid, OSError) as error:
        return ([f"the candidate root could not record its positive conclusion: {error!r}"], False)
    return guard_problems, True


def finish_base(guard, probe, base, run_id, qualified, retained, candidate_concluded, protection):
    """The bootstrap base's own marker transition. A passing qualification that
    intentionally retains nested UNKNOWN resources keeps the base unconcluded
    with the honest `nested-unclosed` reason; a failed qualification keeps it
    `qualify-failed`; only a qualification whose whole owned tree concluded
    records the positive `qualify-base` conclusion."""
    if not qualified:
        outcome, closure, reason = "failed", "none", "qualify-failed"
    elif retained or (isinstance(protection, dict) and protection.get("state") == "protected"
                      and not candidate_concluded):
        outcome, closure, reason = "retained", "none", "nested-unclosed"
    else:
        outcome, closure, reason = "closed", "positive", "qualify-base"
    try:
        guard.record_conclusion(base, run_id, probe, outcome, closure, reason,
                                nested_concluded=closure == "positive")
    except (guard.MarkerInvalid, OSError) as error:
        return [f"the bootstrap base could not record its conclusion: {error!r}"]
    return []


DETACH_CASES = (
    # (suite case, the detach state it must record, whether that detach qualified)
    ("test_success_detaches_exactly_the_requested_volume", "detached", True),
    ("test_an_unbound_identity_is_refused_before_any_detach", "refused", False),
    ("test_a_failed_attempt_keeps_the_mount_and_is_unqualified", "failed", False),
)

CLEANUP_CASE = "test_an_unrequested_attachment_keeps_the_owned_root"
STUCK_CASE = "test_a_stuck_detach_child_is_closed_by_the_custodian"


def check_plan(receipt, results, expected, record=None, facts=None, base=None):
    """The qualification's named oracles, in order. `check` flattens them; the
    uncounted diagnostic runs them one at a time so one failing oracle cannot
    hide the rest. Every oracle returns its own problem list."""
    facts = facts if facts is not None else KernelFacts()

    def receipt_phase():
        if receipt.get("phase") != "monitor_complete: pending_custodian":
            return [f"receipt phase {receipt.get('phase')}"]
        return []

    def candidate_outcome():
        outcome = receipt.get("outcome") or {}
        if (outcome.get("closure"), outcome.get("status"), outcome.get("fault"), outcome.get("deadline")) != (
                "closed", 0, None, False):
            return [f"candidate outcome {outcome}"]
        return []

    def reserve_sampling():
        reserve = receipt["reserve"]
        gate = [r["at"] for r in receipt["custodian"] if r["event"] == "gate_open"]
        problems = []
        if not reserve["samples"] or not gate or reserve["first_ns"] >= gate[0]:
            problems.append("reserve sampling did not start before the gate opened")
        if reserve["max_gap_ns"] > NS:
            problems.append(f"reserve sample gap {reserve['max_gap_ns']} ns over 1 s")
        return problems

    def owned_bytes():
        owned = receipt["owned_bytes"]
        if owned["maximum"] is None or not 5 * MIB <= owned["maximum"] <= owned["budget"]:
            return [f"owned-byte peak {owned['maximum']} does not show the fixture writers within budget"]
        return []

    def signal_custody():
        problems = []
        for entry in receipt["custodian"]:
            if entry["event"] == "signal" and not (entry["held_unreaped"] and entry["pin"] in ("leader", "monitor")):
                problems.append(f"signal without custody: {entry}")
        return problems

    def detach_paths():
        """Oxide#191: the qualification receipt must exercise the new post-census
        detach success, refusal and uncertainty paths, through the suite's own
        nested runs, and the outer run must itself publish no request."""
        problems = []
        if "detach" in receipt:
            problems.append("the qualification run itself published a detach request")
        recorded, kept = {}, {}
        for case in results.get("cases", []):
            if case.get("detach") is not None and case.get("case") not in recorded:
                recorded[case["case"]] = case["detach"]
            if case.get("cleanup") is not None and case.get("case") not in kept:
                kept[case["case"]] = case["cleanup"]
        for name, state, qualified in DETACH_CASES:
            detach = recorded.get(name)
            if detach is None:
                problems.append(f"detach case {name} recorded no detach outcome")
            elif (detach.get("state"), bool(detach.get("qualified"))) != (state, qualified):
                problems.append(f"detach case {name} recorded {detach.get('state')}/"
                                f"{detach.get('qualified')}, not {state}/{qualified}")
        stuck = recorded.get(STUCK_CASE)
        if stuck is None:
            problems.append(f"{STUCK_CASE} recorded no detach outcome")
        elif (stuck.get("state"), bool(stuck.get("qualified")), stuck.get("reaped"),
              stuck.get("custodian_closed")) != ("timed_out", False, False, True):
            problems.append(f"{STUCK_CASE} recorded {stuck.get('state')}/{stuck.get('qualified')}/"
                            f"reaped {stuck.get('reaped')}/custodian_closed "
                            f"{stuck.get('custodian_closed')}, not timed_out/False/False/True")
        if CLEANUP_CASE in recorded:
            problems.append(f"{CLEANUP_CASE} recorded a detach object for a request-free run")
        if CLEANUP_CASE not in kept:
            problems.append(f"{CLEANUP_CASE} recorded no pre-cleanup keep")
        elif kept[CLEANUP_CASE].get("state") != "kept":
            problems.append(f"{CLEANUP_CASE} recorded {kept[CLEANUP_CASE].get('state')}, not kept")
        return problems

    def cases():
        outcomes = results.get("outcomes", [])
        failed = [case for case, verdict in outcomes if verdict != "pass"]
        if failed or len(outcomes) != EXPECTED_CASES:
            return [f"cases: {len(outcomes)} of {EXPECTED_CASES} recorded, failed {failed}"]
        return []

    return [
        ("receipt-phase", receipt_phase),
        ("candidate-outcome", candidate_outcome),
        ("reserve-sampling", reserve_sampling),
        ("default-scratch", lambda: default_scratch(receipt, expected, record, facts)),
        ("scratch-sweep", lambda: scratch_sweep(receipt, expected, base, facts)),
        ("protected-root", lambda: protected_root(receipt, record)),
        ("nested-cases", lambda: nested_cases(results, record, facts)),
        ("owned-bytes", owned_bytes),
        ("signal-custody", signal_custody),
        ("detach-paths", detach_paths),
        ("cases", cases),
    ]


def check(receipt, results, expected, record=None, facts=None, base=None):
    problems = []
    for _name, oracle in check_plan(receipt, results, expected, record, facts, base):
        problems += oracle()
    return problems


PREFLIGHT_FLAG = "--preflight"


def preflight(argv):
    """Effect-free readiness for the selected host, for `proof:supervisor:preflight`.

    Metadata only: it selects and inspects paths, modes, owners and devices and
    prints a bounded JSON result. It creates nothing (an absent private leaf is
    reported as "absent; would create 0700"), never sweeps, marks, forks the
    deadline actor, launches the candidate or reads reserves. The invoked
    qualification re-runs the same checks in its own setup, so this is
    point-in-time readiness, never a reservation."""
    if argv != [PREFLIGHT_FLAG]:
        print(f"refused: preflight takes no arguments (got {argv})", file=sys.stderr)
        return 2
    guard = candidate()
    selection = guard.select_default_parent(os.environ)
    result = {"selector": "proof:supervisor:preflight", "head": git("rev-parse", "HEAD").strip(),
              "readiness": guard.readiness(selection)}
    print(json.dumps(result, indent=1), flush=True)
    return 0 if result["readiness"]["ready"] else 2


DIAGNOSTIC_FLAG = "--diagnostic"


def diagnostic_receipt(problems, rows, extra=None):
    """The diagnostic mode's receipt. Pure, so the suite can prove the one
    property that matters: an uncounted collection is NEVER `qualified`, not even
    when every named oracle passed. `result` is never `pass` either."""
    receipt = {"selector": "proof:supervisor:qualify:diagnostic", "result": "diagnostic",
               "qualified": False, "claims": ("diagnostic coverage and oracle outcomes only; "
                                              "never a counted native PASS, closure, census or custody claim"),
               "problems": list(problems),
               "checks": list(rows),
               "checks_passed": sum(1 for row in rows if row.get("result") == "pass"),
               "checks_failed": sum(1 for row in rows if row.get("result") != "pass"),
               "limitations": LIMITATIONS}
    receipt.update(extra or {})
    return receipt


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] == PREFLIGHT_FLAG:
        return preflight(list(argv))
    diagnostic = DIAGNOSTIC_FLAG in argv
    if diagnostic:
        argv.remove(DIAGNOSTIC_FLAG)
    if argv:
        raise SystemExit(f"qualify-supervisor.py: unknown argument {argv[0]}")
    return bounded(lambda: qualify(diagnostic=diagnostic), DEADLINE - ACTOR_MARGIN_NS, DEADLINE - NS // 2)


def refusal(message, diagnostic, extra=None):
    """A precondition refusal. Nothing was created. In diagnostic mode it also
    prints one bounded `qualified: false` receipt and returns 1, because an
    uncounted collection never wears the counted bootstrap's exit codes."""
    print(message, file=sys.stderr)
    if not diagnostic:
        return 2
    receipt = diagnostic_receipt([message], [], {"result": "refused"})
    try:
        receipt["head"] = git("rev-parse", "HEAD").strip()
    except Exception:
        receipt["head"] = None
    receipt.update(extra or {})
    print("qualification diagnostic receipt:", json.dumps(receipt, indent=1), flush=True)
    print("diagnostic (UNCOUNTED, never a PASS): refused before any effect", file=sys.stderr, flush=True)
    return 1


def qualify(diagnostic=False):
    forward = []  # INT and TERM go to the candidate, which cancels itself
    for number in (signal.SIGINT, signal.SIGTERM):
        signal.signal(number, lambda n, _f: forward.append(n))
    print("proof:supervisor:qualify: the supervisor is the candidate under test; this bootstrap (Effigy heavy"
          " admission, reserve readings, fixed 300 s deadline, finite fixture fallbacks) does not qualify it",
          flush=True)
    if sys.platform != "darwin":
        return refusal("refused: macOS only", diagnostic)
    head = git("rev-parse", "HEAD").strip()
    if git("status", "--porcelain").strip():
        return refusal("refused: the checkout is not clean, so HEAD does not name the tested source",
                       diagnostic)
    expected = selection()
    print(f"default scratch selection: {json.dumps(expected)}", flush=True)
    if expected.get("source") != "selected-volume":
        return refusal(f"refused: the candidate default would fall back to {expected.get('root_parent')}"
                       f" ({expected.get('reason')}), so this qualification could not exercise the"
                       " selected-volume default; run it where the machine's passive pool selection names"
                       " the registered drive (account config or OXIDE_POOL). Nothing was created.",
                       diagnostic, {"selection": expected})
    guard = candidate()
    probe = guard.DefaultProbe()
    try:
        parent = guard.prepare_scratch_parent(expected, probe)
    except (guard.SelectionRefused, OSError) as error:
        return refusal(f"refused: the owned scratch parent {expected.get('root_parent')} cannot be prepared:"
                       f" {error!r}. Nothing was created.", diagnostic)
    print(f"owned scratch parent {parent} (0700 inside the selected volume; a new subfolder is not a new"
          " volume)", flush=True)
    facts = KernelFacts()
    opening = reserve_readings(parent, "/private/tmp", facts)
    print(f"bootstrap scratch-volume free {opening['scratch_volume']['free'] / GIB:.1f} GiB on"
          f" {opening['scratch_volume']['path']} (abort threshold 22 GiB); boot-volume free"
          f" {opening['boot_volume']['free'] / GIB:.1f} GiB on {opening['boot_volume']['path']}", flush=True)
    if opening["scratch_volume"]["free"] < THRESHOLD:
        return refusal("refused: the selected qualification scratch volume has less than 22 GiB free",
                       diagnostic)
    if opening["boot_volume"]["free"] < THRESHOLD:
        return refusal("refused: the boot volume has less than 22 GiB free", diagnostic)
    sweep = guard.sweep_concluded_roots(parent, expected["device"], probe)
    print(f"bounded startup sweep: examined {sweep['examined']}, removed {sweep['removed_total']},"
          f" skipped {sweep['skipped_total']}, steps {sweep['steps']} of {sweep['entry_budget']},"
          f" stop {sweep['stop']}", flush=True)
    base = os.path.realpath(tempfile.mkdtemp(prefix=guard.QUALIFY_LEAF, dir=parent))
    run_id = guard.new_run_id()
    try:
        guard.write_marker(base, guard.marker_document(run_id, base, probe), probe)
    except (guard.MarkerInvalid, OSError) as error:
        return refusal(f"refused: the bootstrap base {base} could not be marked ({error!r}); it is kept"
                       " and never swept, so an owner must remove it", diagnostic)
    print(f"scratch base {base} on the selected volume (marked, kept: never deleted by this bootstrap)",
          flush=True)
    env = {k: v for k, v in os.environ.items() if k not in ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_SUPERVISOR_INJECT")}
    env["OXIDE_SUPERVISOR_CASES"] = os.path.join(base, "cases.json")
    env["OXIDE_SUPERVISOR_LAYOUT"] = os.path.join(base, "topology.json")
    # The Popen handle is a local: its name must not shadow the module-level
    # `candidate()` loader resolved above (diagnostic2 UnboundLocalError).
    candidate_process = subprocess.Popen(
        [sys.executable, SOURCES["supervisor"], "--limit", str(CANDIDATE_LIMIT), "--scratch-budget-mib", "512",
         "--", sys.executable, SOURCES["suite"]], env=env)
    problems = []
    # Reaped only by `hold`, never by Popen, so the candidate stays pinned.
    status = hold(candidate_process.pid, DEADLINE - 15 * NS, forward)
    if status is None:
        problems.append("candidate overran the bootstrap deadline")
        for number in (signal.SIGTERM, signal.SIGKILL):  # our own unreaped child only
            forward.append(number)
            status = hold(candidate_process.pid, now_ns() + 3 * NS, forward)
            if status is not None:
                break
    code = None if status is None else code_of(status)
    candidate_process.returncode = code
    if code != 0:
        problems.append(f"candidate exit {code}")
    closing = reserve_readings(expected["root_parent"], "/private/tmp", facts)
    if closing["scratch_volume"]["free"] < THRESHOLD:
        problems.append(f"selected qualification scratch volume free {closing['scratch_volume']['free']}"
                        " below 22 GiB")
    if closing["boot_volume"]["free"] < THRESHOLD:
        problems.append(f"boot volume free {closing['boot_volume']['free']} below 22 GiB")
    receipt, results, record, rows = {}, {}, None, []
    if diagnostic:
        # Uncounted keep-going: one unreadable phase or failing oracle must not
        # hide the rest, and a skipped check is never a pass.
        try:
            record, topology_problems = topology(base, expected, facts)
        except Exception as error:
            record, topology_problems = None, [f"candidate topology unreadable: {error!r}"]
        problems += topology_problems
        layout = record or {"evidence": os.path.join(base, "missing")}
        try:
            with open(os.path.join(layout["evidence"], "receipt.json")) as handle:
                receipt = json.load(handle)
            with open(env["OXIDE_SUPERVISOR_CASES"]) as handle:
                results = json.load(handle)
        except Exception as error:
            problems.append(f"missing or malformed candidate evidence: {error!r}")
        if receipt and results:
            for name, oracle in check_plan(receipt, results, expected, record, facts, base):
                try:
                    found = oracle()
                except Exception as error:
                    found = [f"unreadable: {error!r}"]
                rows.append({"check": name, "result": "fail" if found else "pass", "problems": found})
                problems += found
    else:
        try:
            record, topology_problems = topology(base, expected, facts)
            problems += topology_problems
            layout = record or {"evidence": os.path.join(base, "missing")}
            with open(os.path.join(layout["evidence"], "receipt.json")) as handle:
                receipt = json.load(handle)
            with open(env["OXIDE_SUPERVISOR_CASES"]) as handle:
                results = json.load(handle)
            problems += check(receipt, results, expected, record, facts, base)
        except (OSError, ValueError, KeyError, TypeError) as error:
            problems.append(f"missing or malformed candidate evidence: {error!r}")
    nested_unclosed = [case["dir"] for case in results.get("cases", []) if case.get("retained_unclosed")]
    # The suite result and the custody of the owned resources are separate
    # facts: intentional retained nested UNCLOSED fixtures are an expected PASS,
    # and they never flip the result. They do keep every marker above them
    # unconcluded, because a positive conclusion of ALL owned resources was not
    # established.
    qualified = not problems and code == 0 and (receipt.get("outcome") or {}).get("closure") == "closed"
    candidate_concluded = False
    if record is not None:
        try:
            candidate_problems, candidate_concluded = finish_candidate(
                guard, probe, record, nested_unclosed, (record or {}).get("protection"))
        except Exception as error:
            if not diagnostic:
                raise
            candidate_problems, candidate_concluded = [f"candidate conclusion unreadable: {error!r}"], False
        problems += candidate_problems
    try:
        base_problems = finish_base(guard, probe, base, run_id, qualified, bool(nested_unclosed),
                                    candidate_concluded, (record or {}).get("protection"))
    except Exception as error:
        if not diagnostic:
            raise
        base_problems = [f"bootstrap conclusion unreadable: {error!r}"]
    problems += base_problems
    reserve, owned = receipt.get("reserve", {}), receipt.get("owned_bytes", {})
    qualification = {
        "selector": ("proof:supervisor:qualify:diagnostic" if diagnostic else "proof:supervisor:qualify"),
        "result": "diagnostic" if diagnostic else ("fail" if problems else "pass"),
        "problems": problems,
        "head": head,
        "sha256": {name: sha256(path) for name, path in SOURCES.items()},
        "platform": {"system": platform.platform(), "machine": platform.machine(), "python": platform.python_version()},
        "candidate_exit": code,
        "candidate_phase": receipt.get("phase"),
        "candidate_outcome": receipt.get("outcome"),
        "default_scratch": {"expected": expected, "receipt": receipt.get("scratch_default"),
                            "root": receipt.get("root"), "topology": record,
                            "prepared_parent": parent},
        "scratch_sweep": sweep,
        "scratch_marker": {"candidate": marker_state_of(guard, probe, (record or {}).get("root")),
                           "base": marker_state_of(guard, probe, base)},
        "candidate_scratch_sweep": receipt.get("scratch_sweep"),
        "candidate_scratch_marker": receipt.get("scratch_marker"),
        "reserve": {"samples": reserve.get("samples"), "minimum_bytes": reserve.get("minimum"),
                    "max_gap_ns": reserve.get("max_gap_ns"), "path": reserve.get("path"),
                    "device": reserve.get("device")},
        "reserve_readings": {"scratch_volume": {"path": opening["scratch_volume"]["path"],
                                                "initial": opening["scratch_volume"]["free"],
                                                "final": closing["scratch_volume"]["free"],
                                                "threshold": THRESHOLD},
                             "boot_volume": {"path": opening["boot_volume"]["path"],
                                             "initial": opening["boot_volume"]["free"],
                                             "final": closing["boot_volume"]["free"],
                                             "threshold": THRESHOLD}},
        "owned_bytes_peak": owned.get("maximum"),
        "candidate_signals": [r for r in receipt.get("custodian", []) if r["event"] == "signal"],
        "cases": results.get("cases"),
        "kept": {"base": base, "nested_unclosed": nested_unclosed},
        "outcomes": results.get("outcomes"),
        "limitations": LIMITATIONS,
    }
    if diagnostic:  # an uncounted collection is never `qualified`, even when clean
        qualification.update(diagnostic_receipt(problems, rows))
        qualification["selector"] = "proof:supervisor:qualify:diagnostic"
        qualification["result"] = "diagnostic"
    qualification["worker_wall_ns_before_print"] = now_ns() - START
    print("qualification receipt:", json.dumps(qualification, indent=1), flush=True)
    if diagnostic:
        print(f"diagnostic (UNCOUNTED, never a PASS): {len(rows)} named checks, "
              f"{sum(1 for row in rows if row['result'] == 'pass')} passed, {len(problems)} problems;"
              f" evidence kept in {base}", file=sys.stderr, flush=True)
        return 1
    print(f"{'FAIL' if problems else 'pass'}; evidence kept in {base}", file=sys.stderr, flush=True)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
