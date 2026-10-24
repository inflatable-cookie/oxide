#!/usr/bin/env python3
"""Focused native proof for a fast, read-only default `oxide status` (oxide#128).

One fixed script under the qualified shared supervisor
(`bounded-test.py`): the brief's 300 s wall limit and 512 MiB monitored scratch
budget including staging, fixture creation and cleanup. It stages this HEAD's
debug `oxide` with plain Cargo and the pinned 1.97.1, offline, into its own
scratch target, copies the binary out and deletes the target before the
fixtures. Debug builds carry the walk observer (`OXIDE_TEST_WALK_LOG`), which
records every accounting/checkout/Cargo-home walk root.

The fixtures are small private ledgers and pools. Default `oxide status`
walks nothing (structural: the debug walk log records no root) and reconciles
nothing (structural: tree and control bytes unchanged, a recorded `Evicting`
slot intact). `--measure` logs exactly the payload walk and one walk per slot
(structural, no repeated display walk) and reports its bounded scan; the
shared cumulative deadline is proved deterministically by the library test
`measure_walks_share_one_cumulative_deadline` added at 9269. Wall-clock
timings are sanity observations only, not a large-pool or universal latency
claim. The first fixture holds one ledger-recorded slot, a
tiny valid directory-pool config/ledger/lock, and
a control state with a held command, a waiting ticket, a clean mark and a
flush. Cases:

- default status end-to-end timing on that fixture (one run, no retry), with
  observed wall-clock time, load average and output size published;
- no payload traversal, independent of timing: the debug walk log is never
  created, so no accounting, checkout or Cargo-home walk ran;
- zero mutation: a bounded metadata digest and the exact control-file bytes
  are identical before and after every run, no sidecar is created and the
  lock's inode/mtime/bytes do not change;
- a recorded `Evicting` slot stays intact and is labelled as a pending
  reconciliation, never idle or held;
- held/waits/clean/flush rendering, and the recorded directory of a nested
  slot shown instead of a decoy `slots/<id>` sibling;
- valid, stale, future, incarnation-mismatched, directory-mismatched, missing,
  corrupt and unknown-version observation facts, each with an honest label,
  and the sidecar is never refreshed by a read;
- a pool without a lock file gains none;
- a registered removable pool with an absent volume: mode and recorded
  selection are shown without probing the volume or calling diskutil;
- `--measure` on two tiny valid directory pools: actual walked bytes and
  complete coverage for a settled pool, exactly one payload walk plus one
  slot walk (no repeated display walks), and a recorded deletion left intact
  with honest unknown coverage.

No sparse image, mounted volume, real pool, host install, config, mode or
cleanup is touched. This is not a benchmark, a universal filesystem-latency
guarantee, real-drive qualification or an alias for full QA.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import time
from typing import Optional

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILDER = "1.97.1"
EXPECTED_BRANCH = "ns-51242973-65d6-495f-b8f8-c893afbb7d72"
EXPECTED_RUNNER_SHA256 = "d89115f1ca03587e1d9134efa153cc9c94860d2904f52e2a11bc8c8fbe21f25e"
MIB = 1024 * 1024
# Small real private fixtures only (brief v3): the walk and reconciliation
# claims are structural (debug walk log plus debug-only scoped injection), and
# the wall clock is a sanity observation, not a large-pool latency claim.
PAYLOAD_FILES = 5
SHUTDOWN_MARGIN_S = 30
DEFAULT_STATUS_LIMIT_S = 1.0
MISSING_FACT = "not measured (no recorded observation)"
GENERATION_FACT = "not measured (no recorded observation for this slot generation)"
UNREADABLE_FACT = "not measured (recorded observations unreadable"
CASES: list[str] = []


def fail(message: str) -> None:
    print(f"status-fast proof failed: {message}", file=sys.stderr)
    sys.exit(1)


def expect(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


PHASE_STARTED = time.perf_counter()
LAST_STDERR = ""


def say(message: str) -> None:
    print(f"[{time.perf_counter() - PHASE_STARTED:7.1f}s] {message}", flush=True)


def case(name: str) -> None:
    CASES.append(name)
    say(f"case {name}")


def work_left() -> float:
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    left = (
        deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
    ) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 5:
        fail("work deadline reached before the supervisor's shutdown margin")
    return left


def sha256_file(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def git(*args: str) -> str:
    result = subprocess.run(
        ["git", *args], cwd=REPO, check=True, capture_output=True, text=True
    )
    return result.stdout.strip()


def private_dir(path: str) -> str:
    os.makedirs(path, exist_ok=True)
    os.chmod(path, 0o700)
    return path


def write_private(path: str, data, mode: int = 0o600) -> None:
    private_dir(os.path.dirname(path))
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(data)
    os.chmod(path, mode)


def now_ms() -> int:
    return int(time.time() * 1000)


def stage_subject(root: str) -> str:
    """Stage this clean HEAD's debug `oxide` with plain Cargo, offline.

    The staged target is deleted once the binary is copied out, keeping the
    monitored scratch far below the 512 MiB gate.
    """
    stage = os.path.join(root, "stage")
    log = os.path.join(root, "stage.log")
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("CARGO_TARGET_", "CARGO_BUILD_", "SCCACHE_"))
        and key not in ("CARGO", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER")
    }
    env.update(
        {
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(stage, "target"),
            "CARGO_BUILD_BUILD_DIR": os.path.join(stage, "build"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_PROFILE_DEV_DEBUG_ASSERTIONS": "true",
            "CARGO_PROFILE_DEV_OPT_LEVEL": "s",
            "CARGO_PROFILE_DEV_CODEGEN_UNITS": "1",
            "CARGO_PROFILE_DEV_PANIC": "abort",
            "CARGO_INCREMENTAL": "0",
            "RUSTFLAGS": "-C strip=symbols",
        }
    )
    with open(log, "w", encoding="utf-8") as handle:
        build = subprocess.run(
            ["cargo", "build", "-j", "1", "--locked", "--offline", "--bin", "oxide"],
            cwd=REPO,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            timeout=work_left(),
        )
    if build.returncode:
        with open(log, encoding="utf-8") as handle:
            fail(f"staging exit {build.returncode}: {handle.read()[-2000:]}")
    built = os.path.join(stage, "target", "debug", "oxide")
    expect(os.path.isfile(built), f"staging produced no binary at {built}")
    subject = os.path.join(root, "subject-oxide")
    shutil.copy2(built, subject)
    os.chmod(subject, 0o755)
    shutil.rmtree(stage)
    os.unlink(log)
    return subject


# --------------------------------------------------------------------------
# Fixtures. Every one is private and hand-built from the documented files.


def slot_record(
    slot_id: int,
    incarnation: int,
    state: str,
    project: str,
    workspace: str,
    *,
    holder: str = "holder",
    lease: Optional[str] = None,
    directory: Optional[str] = None,
) -> dict:
    record = {
        "id": slot_id,
        "incarnation": incarnation,
        "state": state,
        "project": project,
        "workspace": workspace,
        "profile": "dev",
        "holder": holder,
        "lease_id": lease,
        "last_lease_id": lease or "",
    }
    if directory is not None:
        record["dir"] = directory
    return record


def control_record(
    record_id: int,
    checkout: str,
    *,
    phase: str = "idle",
    command: Optional[str] = None,
    lease: Optional[str] = None,
    last_used: int = 1,
) -> dict:
    return {
        "id": record_id,
        "identity": "a" * 16,
        "compatibility": "b" * 16,
        "profile": "quick",
        "checkout": checkout,
        "phase": phase,
        "process_group": None,
        "last_used": last_used,
        "last_used_ms": 1_700_000_000_000 + last_used,
        "holder": (
            {
                "pid": 424242,
                "started_ms": 1_700_000_000_000,
                "lease_holder": f"oxide-424242-{record_id}",
                "command": command,
            }
            if phase != "idle"
            else None
        ),
        "lease_id": lease,
    }


def build_pool(
    root: str,
    slots: list[dict],
    *,
    with_lock: bool = True,
    cap_bytes: int = 100_000_000,
) -> dict:
    """Write one tiny valid `directory-budget-v1` pool plus its slot dirs."""
    store_id = hashlib.sha256(root.encode()).hexdigest()[:32]
    pool = private_dir(os.path.join(root, "pool"))
    payload = private_dir(os.path.join(pool, "payload"))
    slots_root = private_dir(os.path.join(payload, "slots"))
    control = private_dir(os.path.join(pool, "oxide-managed"))
    for slot in slots:
        leaf = (
            os.path.join(slots_root, slot["dir"])
            if "dir" in slot
            else os.path.join(slots_root, str(slot["id"]))
        )
        private_dir(leaf)
        private_dir(os.path.join(leaf, "build"))
    config = {
        "version": 1,
        "backend": "directory-budget-v1",
        "store_id": store_id,
        "cap_bytes": cap_bytes,
        "image_bytes": 0,
    }
    ledger = {
        "version": 1,
        "store_id": store_id,
        "cap_bytes": cap_bytes,
        "next_slot": max((slot["id"] for slot in slots), default=0) + 1,
        "next_lease": 3,
        "slots": slots,
    }
    write_private(os.path.join(pool, "config.json"), json.dumps(config))
    write_private(os.path.join(pool, "ledger.json"), json.dumps(ledger))
    if with_lock:
        write_private(os.path.join(pool, "lock"), "")
    return {
        "root": root,
        "pool": pool,
        "payload": payload,
        "slots_root": slots_root,
        "control": control,
        "store_id": store_id,
        "config": config,
        "ledger": ledger,
    }


def write_state(pool: dict, records: list[dict], **extra) -> None:
    document = {
        "version": 2,
        "next_id": max((record["id"] for record in records), default=0) + 1,
        "next_use": 3,
        "tickets": extra.get("tickets", []),
        "cleans": extra.get("cleans", []),
        "records": records,
        "recoveries": extra.get("recoveries", []),
        "flushes": extra.get("flushes", []),
    }
    if extra.get("owned_outputs"):
        document["owned_outputs"] = extra["owned_outputs"]
    write_private(os.path.join(pool["control"], "state.json"), json.dumps(document))


def observation(slot: dict, store_id: str, *, at_ms: int, directory=...) -> dict:
    fact = {
        "store_id": store_id,
        "slot_id": str(slot["id"]),
        "incarnation": str(slot["incarnation"]),
        "measured_at_ms": at_ms,
        "allocated_bytes": 12 * MIB,
        "source": "lease-reuse",
        "lease_id": f"v1.{'c' * 32}.{slot['id']}.{slot['incarnation']}.2",
    }
    if directory is ...:
        fact["dir"] = slot.get("dir")
    elif directory is not None:
        fact["dir"] = directory
    return fact


def write_observations(pool: dict, facts: list[dict], *, schema=None) -> None:
    write_private(
        os.path.join(pool["control"], "status-observations.json"),
        json.dumps(
            {
                "schema": schema or "oxide.status-observations/1",
                "observations": facts,
            }
        ),
    )


def fixture_env(pool: dict, walk_log: str) -> dict:
    """A minimal allowlist: no host XDG, Oxide or Cargo setting leaks in."""
    home = private_dir(os.path.join(pool["root"], "home"))
    env = {
        "HOME": home,
        "OXIDE_TEST_HOME_FROM_ENV": "1",
        "TMPDIR": pool["root"],
        "TMP": pool["root"],
        "TEMP": pool["root"],
        "OXIDE_POOL": pool["pool"],
        "CARGO_HOME": os.path.join(pool["root"], "cargo-home"),
        "OXIDE_TEST_WALK_LOG": walk_log,
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "NO_COLOR": "1",
        "RUSTUP_AUTO_INSTALL": "0",
    }
    for name in ("LANG", "LC_ALL", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "TZ"):
        if name in os.environ:
            env[name] = os.environ[name]
    return env


def run_status(
    subject: str,
    pool: dict,
    walk_log: str,
    *args: str,
    extra: Optional[dict] = None,
    timeout: Optional[float] = None,
):
    env = fixture_env(pool, walk_log)
    # The debug walk hook is enabled only by the guard's own variables; pass
    # them through unchanged so an enabled injection is never vacuous.
    for name in ("OXIDE_PROOF_DEADLINE_NS", "OXIDE_TASK_SCRATCH_ROOT", "OXIDE_PROOF_EVIDENCE"):
        if name in os.environ:
            env[name] = os.environ[name]
    if extra:
        env.update(extra)
    started = time.perf_counter_ns()
    result = subprocess.run(
        [subject, "status", *args],
        env=env,
        cwd=pool["root"],
        capture_output=True,
        text=True,
        timeout=timeout if timeout is not None else work_left(),
    )
    elapsed_ns = time.perf_counter_ns() - started
    global LAST_STDERR
    LAST_STDERR = result.stderr
    if result.returncode != 0:
        fail(
            f"status {args} exit {result.returncode}: {result.stderr[-2000:]}"
        )
    return result.stdout, elapsed_ns


def walk_roots(walk_log: str) -> list[str]:
    if not os.path.exists(walk_log):
        return []
    return [line for line in open(walk_log, encoding="utf-8").read().splitlines() if line]


def clear_walk_log(walk_log: str) -> None:
    if os.path.exists(walk_log):
        os.unlink(walk_log)


def mutation(actions: list[dict]) -> dict:
    return {"OXIDE_TEST_WALK_MUTATIONS": json.dumps(actions)}


def reject_walk_warnings(stderr: str, label: str) -> None:
    if "walk log" in stderr and "not written" in stderr:
        fail(f"{label}: the observer could not write its walk log: {stderr[-800:]}")


def tree_digest(root: str) -> str:
    """Bounded no-follow metadata digest of every entry under `root`."""
    digest = hashlib.sha256()
    stack = [root]
    count = 0
    while stack:
        current = stack.pop()
        with os.scandir(current) as entries:
            listed = sorted(entries, key=lambda entry: entry.name)
        for entry in listed:
            item = entry.stat(follow_symlinks=False)
            count += 1
            digest.update(
                f"{os.path.relpath(entry.path, root)}\0{stat.S_IFMT(item.st_mode)}\0"
                f"{item.st_mode:o}\0{item.st_size}\0{item.st_mtime_ns}\0"
                f"{item.st_ino}\0{item.st_nlink}\n".encode()
            )
            if stat.S_ISDIR(item.st_mode):
                stack.append(entry.path)
            elif stat.S_ISREG(item.st_mode) and item.st_size <= 64 * 1024:
                with open(entry.path, "rb") as handle:
                    digest.update(handle.read())
    digest.update(f"entries {count}\n".encode())
    return digest.hexdigest()


def control_hashes(pool: dict) -> dict:
    """Exact bytes of every control file that must never change on a read.

    The payload is covered separately by its entry count and directory
    metadata, so a 200,000-entry slot is not reopened once per case.
    """
    hashes = {}
    pool_root = pool["pool"]
    for base, dirs, names in os.walk(pool_root):
        dirs[:] = [
            name
            for name in dirs
            if os.path.relpath(os.path.join(base, name), pool_root) != "payload"
        ]
        for name in names:
            if name == "status-observations.json":
                # The fixture writes this diagnostic itself; each case checks
                # its bytes separately.
                continue
            path = os.path.join(base, name)
            relative = os.path.relpath(path, pool_root)
            if os.path.getsize(path) > 4 * MIB:
                continue
            hashes[relative] = sha256_file(path)
    return hashes


def entry_count(slot_path: str) -> int:
    return len(os.listdir(slot_path))


def cheap_state(pool: dict) -> tuple:
    """Control bytes plus the payload markers a read must never touch.

    `payload/slots/project-aaaa/quick-1/build` holds the 200,000 fixture
    entries; its own mtime and inode change if anything is written there.
    """
    payload = pool["payload"]
    build = os.path.join(payload, "slots/project-aaaa/quick-1/build")
    item = os.stat(build)
    return (
        tuple(sorted(control_hashes(pool).items())),
        os.stat(payload).st_mtime_ns,
        (item.st_ino, item.st_mtime_ns, entry_count(build)),
    )


def expect_line(output: str, needle: str, case_name: str) -> None:
    expect(
        needle in output,
        f"{case_name}: missing {needle!r} in output:\n{output}",
    )


def reject_line(output: str, needle: str, case_name: str) -> None:
    expect(
        needle not in output,
        f"{case_name}: unexpected {needle!r} in output:\n{output}",
    )


def loadavg() -> tuple[float, float, float]:
    try:
        return os.getloadavg()
    except OSError:
        return (0.0, 0.0, 0.0)


def main() -> None:
    branch = git("branch", "--show-current")
    expect(branch == EXPECTED_BRANCH, f"branch is {branch!r}, not {EXPECTED_BRANCH!r}")
    runner = os.path.join(REPO, "scripts", "bounded-test.py")
    runner_sha = sha256_file(runner)
    expect(
        runner_sha == EXPECTED_RUNNER_SHA256,
        f"supervisor {runner_sha} is not the qualified {EXPECTED_RUNNER_SHA256}",
    )
    dirty = git("status", "--porcelain")
    expect(
        dirty == "",
        f"the worktree is not clean, so the staged source would not be HEAD: {dirty[:400]}",
    )
    commit = git("rev-parse", "HEAD")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT") or fail(
        "OXIDE_TASK_SCRATCH_ROOT is not set"
    )
    scratch = os.path.abspath(scratch)
    expect(
        scratch.startswith("/private/tmp/") or scratch.startswith("/tmp/"),
        f"scratch root {scratch} is not a private temporary directory",
    )
    walk_log = os.path.join(scratch, "walks.log")
    load_before = loadavg()
    print(
        f"host: project={git('remote', 'get-url', 'origin')} commit={commit} "
        f"branch={branch} cpu={os.cpu_count()} load={load_before}"
    )

    run_cases(scratch, walk_log, branch, commit, runner_sha, load_before)


def run_cases(
    scratch: str,
    walk_log: str,
    branch: str,
    commit: str,
    runner_sha: str,
    load_before: tuple,
) -> None:
    case("stage")
    subject = stage_subject(scratch)
    print(f"subject sha256 {sha256_file(subject)}")

    # ---------------------------------------------------------------- big
    case("big-fixture")
    big_root = private_dir(os.path.join(scratch, "big"))
    checkout = private_dir(os.path.join(big_root, "checkout"))
    # Records live under the pool's own control directory, never the checkout.
    big_control = os.path.join(big_root, "pool", "oxide-managed")
    project = os.path.join(big_control, "affinity", "1")
    slots = [
        slot_record(
            1,
            3,
            "held",
            project,
            checkout,
            holder="oxide-424242-1",
            lease="v1.%s.1.3.2" % ("a" * 32),
            directory="project-aaaa/quick-1",
        ),
        slot_record(2, 1, "evicting", os.path.join(big_control, "affinity", "2"), checkout),
    ]
    big = build_pool(big_root, slots)
    for slot in slots:
        private_dir(os.path.join(slot["project"]))
    served = os.path.join(big["slots_root"], "project-aaaa/quick-1/build")
    for index in range(PAYLOAD_FILES):
        with open(os.path.join(served, f"entry-{index:03d}"), "wb") as handle:
            handle.write(b"payload")
    with open(os.path.join(served, "blob"), "wb") as handle:
        handle.write(os.urandom(64 * 1024))
    # A decoy sibling for the legacy `slots/<id>` layout. A status that
    # reconstructed the path would show or measure it.
    decoy = private_dir(os.path.join(big["slots_root"], "1"))
    with open(os.path.join(decoy, "decoy"), "wb") as handle:
        handle.write(b"x" * 4096)
    evicting_dir = os.path.join(big["slots_root"], "2")
    with open(os.path.join(evicting_dir, "pending"), "wb") as handle:
        handle.write(b"y" * 4096)
    write_state(
        big,
        [
            control_record(1, checkout, phase="running", command=None, lease="v1.%s.1.3.2" % ("a" * 32)),
            control_record(2, checkout + "/other"),
        ],
        tickets=[{"id": 1, "pid": 424243}],
        cleans=[{"id": 1, "checkout": checkout}],
    )
    write_private(os.path.join(big["control"], "waiters", "1.lock"), "")
    expect(
        entry_count(served) == PAYLOAD_FILES + 1,
        "the fixture has its recorded payload entries",
    )
    walk_log_before = os.path.exists(walk_log)

    # A live record lock makes the held slot render as a plain held command
    # rather than an interrupted one. Only this proof's child is signalled.
    lock_path = os.path.join(big["control"], "locks", "1.lock")
    write_private(lock_path, "")
    holder = subprocess.Popen(
        [
            sys.executable,
            "-c",
            "import fcntl,sys,time\n"
            "handle=open(sys.argv[1],'r+')\n"
            "fcntl.flock(handle,fcntl.LOCK_EX)\n"
            "print('held',flush=True)\n"
            "time.sleep(float(sys.argv[2]))\n",
            lock_path,
            str(min(240, work_left())),
        ],
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        expect(holder.stdout is not None, "holder helper has no pipe")
        expect(holder.stdout.readline().strip() == "held", "holder could not lock")

        # The observer is established by a positive control before any
        # zero-root claim, and the log is cleared between checks.
        case("observer-control")
        control_root = private_dir(os.path.join(scratch, "observer"))
        control_checkout = private_dir(os.path.join(control_root, "checkout"))
        control = build_pool(
            control_root,
            [
                slot_record(
                    1,
                    1,
                    "idle",
                    os.path.join(control_root, "pool", "oxide-managed", "affinity", "1"),
                    control_checkout,
                    directory="observer-aaaa/quick-1",
                )
            ],
        )
        private_dir(control["ledger"]["slots"][0]["project"])
        write_state(control, [control_record(1, control_checkout)])
        with open(os.path.join(control["slots_root"], "observer-aaaa/quick-1/build/blob"), "wb") as handle:
            handle.write(os.urandom(4096))
        clear_walk_log(walk_log)
        run_status(subject, control, walk_log, "--measure")
        control_roots = walk_roots(walk_log)
        expect(
            control_roots
            == [
                os.path.realpath(control["payload"]),
                os.path.realpath(os.path.join(control["slots_root"], "observer-aaaa/quick-1")),
            ],
            f"the positive observer control logged {control_roots}",
        )
        reject_walk_warnings(LAST_STDERR, "observer control")
        say(f"observer control: {len(control_roots)} walk roots")
        clear_walk_log(walk_log)

        # EACCES tripwire and positive control. Default never reaches the
        # walker or the hook; --measure does and reports partial coverage.
        case("eacces-tripwire")
        denied_root = private_dir(os.path.join(scratch, "denied"))
        denied_checkout = private_dir(os.path.join(denied_root, "checkout"))
        denied = build_pool(
            denied_root,
            [
                slot_record(
                    1,
                    2,
                    "idle",
                    os.path.join(denied_root, "pool", "oxide-managed", "affinity", "1"),
                    denied_checkout,
                    directory="denied-aaaa/quick-1",
                )
            ],
        )
        private_dir(denied["ledger"]["slots"][0]["project"])
        write_state(denied, [control_record(1, denied_checkout)])
        child = private_dir(os.path.join(denied["slots_root"], "denied-aaaa/quick-1/child"))
        with open(os.path.join(child, "entry"), "wb") as handle:
            handle.write(b"child")
        denied_spec = mutation(
            [{"path": os.path.realpath(child), "action": "permission_denied_before_read_dir"}]
        )
        clear_walk_log(walk_log)
        denied_before = tree_digest(denied["pool"])
        output, _ = run_status(subject, denied, walk_log, extra=denied_spec)
        expect(walk_roots(walk_log) == [], "default status walked under an enabled denial hook")
        expect(tree_digest(denied["pool"]) == denied_before, "the denial tripwire changed the pool")
        reject_line(output, "unreadable", "the denial tripwire reached default output")
        reject_walk_warnings(LAST_STDERR, "denial tripwire")
        output, _ = run_status(subject, denied, walk_log, "--measure", extra=denied_spec)
        expect(
            "unreadable_entry" in output or "partial" in output,
            f"the enabled denial produced no partial coverage:\n{output}",
        )
        expect(walk_roots(walk_log), "the enabled denial produced no walk")
        say("eacces: default zero walks; --measure partial coverage")

        # ONE 61 s delay at the payload root drives the real 60 s measure
        # deadline. Its phase allocation is checked here.
        case("measure-deadline")
        expect(
            work_left() >= 76.0,
            f"only {work_left():.1f} s left before the work cutoff; need >=76 s for the 61 s case",
        )
        deadline_root = private_dir(os.path.join(scratch, "deadline"))
        deadline_checkout = private_dir(os.path.join(deadline_root, "checkout"))
        deadline = build_pool(
            deadline_root,
            [
                slot_record(
                    1,
                    3,
                    "idle",
                    os.path.join(deadline_root, "pool", "oxide-managed", "affinity", "1"),
                    deadline_checkout,
                    directory="deadline-aaaa/quick-1",
                ),
                slot_record(
                    2,
                    1,
                    "evicting",
                    os.path.join(deadline_root, "pool", "oxide-managed", "affinity", "2"),
                    deadline_checkout,
                    directory="deadline-aaaa/quick-2",
                ),
            ],
        )
        for slot in deadline["ledger"]["slots"]:
            private_dir(slot["project"])
        write_state(
            deadline,
            [control_record(1, deadline_checkout), control_record(2, deadline_checkout)],
        )
        with open(os.path.join(deadline["slots_root"], "deadline-aaaa/quick-1/build/entry"), "wb") as handle:
            handle.write(b"entry")
        pending = os.path.join(deadline["slots_root"], "deadline-aaaa/quick-2/pending")
        with open(pending, "wb") as handle:
            handle.write(b"pending")
        deadline_spec = mutation(
            [
                {
                    "path": os.path.realpath(deadline["payload"]),
                    "action": "delay_before_read_dir",
                    "delay_ms": 61_000,
                }
            ]
        )
        clear_walk_log(walk_log)
        ledger_hash = sha256_file(os.path.join(deadline["pool"], "ledger.json"))
        pending_hash = sha256_file(pending)
        output, elapsed_ns = run_status(
            subject, deadline, walk_log, "--measure", extra=deadline_spec, timeout=75.0
        )
        markers = LAST_STDERR.count("oxide: debug walk delay 61000 ms")
        expect(markers == 1, f"the delay marker fired {markers} times, not once")
        expect(
            os.path.realpath(deadline["payload"]) in walk_roots(walk_log),
            f"the payload walk is missing from {walk_roots(walk_log)}",
        )
        expect("elapsed_deadline" in output, f"no elapsed-deadline gap:\n{output}")
        expect(
            "partial" in output or "at least" in output,
            f"no partial coverage after the deadline:\n{output}",
        )
        expect(
            sha256_file(os.path.join(deadline["pool"], "ledger.json")) == ledger_hash,
            "the deadline measurement wrote the ledger",
        )
        expect(sha256_file(pending) == pending_hash, "the deadline measurement touched the pending slot")
        say(f"measure deadline: {elapsed_ns / 1e9:.1f} s, one delay marker, partial coverage")

        case("default-status-fast")
        before = tree_digest(big["pool"])
        control_before = control_hashes(big)
        cheap_before = cheap_state(big)
        clear_walk_log(walk_log)
        output, elapsed_ns = run_status(subject, big, walk_log)
        expect(walk_roots(walk_log) == [], "default status walked something")
        reject_walk_warnings(LAST_STDERR, "default status")
        after = tree_digest(big["pool"])
        elapsed = elapsed_ns / 1e9
        print(
            f"default status: {elapsed_ns / 1e6:.1f} ms wall, "
            f"{len(output)} output bytes, {entry_count(served)} slot entries, "
            f"load {loadavg()} (sanity only)"
        )
        # A sanity bound only: the structural no-walk evidence below is the
        # acceptance claim, not this machine's latency.
        expect(
            elapsed < DEFAULT_STATUS_LIMIT_S,
            f"default status took {elapsed:.3f}s, not even the 1 s sanity bound",
        )
        expect(before == after, "default status changed pool metadata")
        expect(control_hashes(big) == control_before, "default status changed control bytes")
        expect(
            not os.path.exists(os.path.join(big["control"], "status-observations.json")),
            "default status created the observations sidecar",
        )
        expect(
            not walk_log_before and not os.path.exists(walk_log),
            "default status walked something (walk log exists)",
        )
        expect_line(output, "slot 1 held quick", "held rendering")
        expect_line(output, "recorded lease-ticket waiter PID 424243", "waits rendering")
        expect_line(output, f"cleaning {checkout}", "clean rendering")
        expect_line(
            output,
            "recorded deletions pending reconciliation: slot 2",
            "pending reconciliation",
        )
        expect_line(
            output,
            "slot 2 recorded deletion pending reconciliation (not idle or held)",
            "evicting slot label",
        )
        expect_line(output, "slots/project-aaaa/quick-1", "recorded nested directory")
        reject_line(output, "slots/1 ", "decoy |slots/<id>| path")
        expect_line(output, "slot sizes: recorded observations only", "recorded mode")
        expect_line(output, MISSING_FACT, "no recorded fact")
        reject_line(output, "slot accounting walk", "no walk in default status")
        reject_line(output, "unreadable_entry", "no walk in default status")
        expect(
            os.path.exists(evicting_dir) and os.path.exists(os.path.join(evicting_dir, "pending")),
            "default status touched the recorded Evicting slot",
        )
        expect(
            os.path.exists(os.path.join(served, "entry-000"))
            and entry_count(served) == PAYLOAD_FILES + 1,
            "the payload fixture was changed",
        )

        # ------------------------------------------------------- fact matrix
        slot = {"id": 1, "incarnation": 3, "dir": "project-aaaa/quick-1"}
        now = now_ms()
        matrix = [
            ("valid", [observation(slot, big["store_id"], at_ms=now - 42_000)], "recorded 42s ago"),
            ("stale", [observation(slot, big["store_id"], at_ms=now - 3 * 3600_000)], "recorded 3h ago"),
            (
                "incarnation-mismatch",
                [
                    observation(
                        {"id": 1, "incarnation": 999, "dir": slot["dir"]},
                        big["store_id"],
                        at_ms=now,
                    )
                ],
                GENERATION_FACT,
            ),
            (
                "directory-mismatch",
                [observation(slot, big["store_id"], at_ms=now, directory=None)],
                GENERATION_FACT,
            ),
            (
                "future",
                [observation(slot, big["store_id"], at_ms=now + 3_600_000)],
                "age is unknown",
            ),
        ]
        for name, facts, needle in matrix:
            case(f"fact-{name}")
            write_observations(big, facts)
            sidecar = os.path.join(big["control"], "status-observations.json")
            before_bytes = open(sidecar, "rb").read()
            clear_walk_log(walk_log)
            output, _ = run_status(subject, big, walk_log)
            expect(walk_roots(walk_log) == [], f"fact {name}: default status walked")
            expect_line(output, needle, f"fact {name}")
            if name == "valid":
                expect_line(output, "measured by the reuse walk when leased", "fact valid source")
                expect_line(output, "may have changed since measurement", "fact valid caveat")
                expect_line(output, "12 MiB", "fact valid size")
            if name == "stale":
                expect_line(output, "3h", "fact stale age")
            expect(
                open(sidecar, "rb").read() == before_bytes,
                f"fact {name}: status refreshed the sidecar",
            )
            expect(cheap_state(big) == cheap_before, f"fact {name}: pool changed")
        os.unlink(os.path.join(big["control"], "status-observations.json"))
        case("fact-missing")
        output, _ = run_status(subject, big, walk_log)
        expect_line(output, MISSING_FACT, "fact missing")
        case("fact-corrupt")
        write_private(os.path.join(big["control"], "status-observations.json"), "not json")
        output, _ = run_status(subject, big, walk_log)
        expect_line(output, UNREADABLE_FACT, "fact corrupt")
        case("fact-unknown-version")
        write_private(
            os.path.join(big["control"], "status-observations.json"),
            json.dumps({"schema": "oxide.status-observations/2", "observations": []}),
        )
        output, _ = run_status(subject, big, walk_log)
        expect_line(output, UNREADABLE_FACT, "fact unknown version")
        os.unlink(os.path.join(big["control"], "status-observations.json"))
        expect(
            cheap_state(big) == cheap_before,
            "the fact matrix changed the pool beyond the sidecar it wrote",
        )
        case("default-status-no-lock")
        nolock_root = private_dir(os.path.join(scratch, "nolock"))
        nolock_control = os.path.join(nolock_root, "pool", "oxide-managed")
        nolock = build_pool(
            nolock_root,
            [
                slot_record(
                    1,
                    1,
                    "idle",
                    os.path.join(nolock_control, "affinity", "1"),
                    nolock_root + "/checkout",
                    directory="nolock-aaaa/quick-1",
                )
            ],
            with_lock=False,
        )
        private_dir(nolock["ledger"]["slots"][0]["project"])
        write_state(nolock, [control_record(1, nolock_root + "/checkout")])
        clear_walk_log(walk_log)
        output, _ = run_status(subject, nolock, walk_log)
        expect(walk_roots(walk_log) == [], "the no-lock default status walked")
        expect(
            not os.path.exists(os.path.join(nolock["pool"], "lock")),
            "default status created a pool lock",
        )
        expect_line(output, "slots 1", "no-lock pool")
        expect(not os.path.exists(walk_log), "a recorded status walked something")

        # -------------------------------------------------------- removable
        case("removable-recorded")
        rem_root = private_dir(os.path.join(scratch, "removable"))
        rem_checkout = private_dir(os.path.join(rem_root, "checkout"))
        rem = build_pool(
            rem_root,
            [
                slot_record(
                    1,
                    1,
                    "idle",
                    os.path.join(rem_root, "pool", "oxide-managed", "affinity", "1"),
                    rem_checkout,
                    directory="rem-aaaa/quick-1",
                )
            ],
        )
        private_dir(rem["ledger"]["slots"][0]["project"])
        write_state(rem, [control_record(1, rem_checkout)])
        mount = "/Volumes/OxideStatusProofAbsent"
        registration = {
            "version": 2,
            "mount_point": mount,
            "volume_uuid": "A6AC3ED7-33EA-46B4-8082-7254CF291ED2",
            "container_uuid": "A6AC3ED7-33EA-46B4-8082-7254CF291ED2",
            "reserve_bytes": 1 * MIB,
            "cap_bytes": 2 * MIB,
            "quota_bytes": None,
            "capacity_bytes": None,
            "registered_ms": 17,
            "mode": "permanent",
        }
        write_private(
            os.path.join(rem["control"], "removable.json"), json.dumps(registration)
        )
        # The removable pool root is `<internal control>/removable`; its own
        # control directory is `<root>/oxide-managed`.
        rem_pool = private_dir(os.path.join(rem["control"], "removable"))
        rem_control = private_dir(os.path.join(rem_pool, "oxide-managed"))
        rem_store = hashlib.sha256(rem_pool.encode()).hexdigest()[:32]
        rem_workspace = private_dir(os.path.join(rem_root, "rem-checkout"))
        rem_project = private_dir(os.path.join(rem_control, "affinity", "1"))
        rem_dir = os.path.join(mount, "oxide-payload")
        write_private(
            os.path.join(rem_pool, "config.json"),
            json.dumps(
                {
                    "version": 1,
                    "backend": "apfs-volume-v1",
                    "store_id": rem_store,
                    "cap_bytes": 2 * MIB,
                    "image_bytes": 0,
                    "volume": {
                        "mount_point": mount,
                        "volume_uuid": registration["volume_uuid"],
                        "payload_dir": "oxide-payload",
                    },
                }
            ),
        )
        write_private(
            os.path.join(rem_pool, "ledger.json"),
            json.dumps(
                {
                    "version": 1,
                    "store_id": rem_store,
                    "cap_bytes": 2 * MIB,
                    "next_slot": 3,
                    "next_lease": 3,
                    "slots": [
                        slot_record(
                            2,
                            5,
                            "held",
                            rem_project,
                            rem_workspace,
                            holder="oxide-777-9",
                            lease=f"v1.{rem_store}.2.5.2",
                            directory="foo-1234/quick-2",
                        ),
                        slot_record(
                            3,
                            1,
                            "evicting",
                            rem_project + "-other",
                            rem_checkout,
                        ),
                    ],
                }
            ),
        )
        write_private(os.path.join(rem_pool, "lock"), "")
        write_private(
            os.path.join(rem_control, "state.json"),
            json.dumps(
                {
                    "version": 2,
                    "next_id": 2,
                    "next_use": 2,
                    "tickets": [],
                    "cleans": [],
                    "records": [
                        control_record(
                            1,
                            rem_workspace,
                            phase="running",
                            lease=f"v1.{rem_store}.2.5.2",
                        )
                    ],
                    "recoveries": [],
                    "flushes": [
                        {"slot_id": "2", "at_ms": now - 60_000, "micros": 3200}
                    ],
                }
            ),
        )
        before = tree_digest(rem["pool"])
        clear_walk_log(walk_log)
        output, elapsed_ns = run_status(subject, rem, walk_log)
        expect(walk_roots(walk_log) == [], "the removable recorded status walked")
        say(f"removable recorded status: {elapsed_ns / 1e6:.1f} ms wall")
        expect(tree_digest(rem["pool"]) == before, "removable recorded status changed the pool")
        expect_line(output, "mode: permanent", "drive mode")
        expect_line(output, "no volume is present at /Volumes/OxideStatusProofAbsent", "offline selection")
        expect_line(output, "flush before releasing slot 2: 3.2 ms", "flush rendering")
        expect_line(output, "project foo-1234", "recorded removable slot label")
        # With no volume present the internal pool is the recorded selection.
        expect_line(
            output,
            f"internal pool: {os.path.realpath(rem['pool'])} (selected for the next build)",
            "internal selection note",
        )
        expect(
            output.index("internal pool: ") < output.index("removable pool: "),
            "the recorded selection is not listed first",
        )
        expect_line(output, "volume now: not checked by status", "no volume probe")
        expect(
            re.search(r"volume now: \d", output) is None,
            f"a recorded status read the volume:\n{output}",
        )

        # --------------------------------------------------------- --measure
        case("measure-settled")
        small_root = private_dir(os.path.join(scratch, "small"))
        small_checkout = private_dir(os.path.join(small_root, "checkout"))
        small = build_pool(
            small_root,
            [
                slot_record(
                    1,
                    1,
                    "idle",
                    os.path.join(small_root, "pool", "oxide-managed", "affinity", "1"),
                    small_checkout,
                    directory="small-aaaa/quick-1",
                )
            ],
        )
        private_dir(small["ledger"]["slots"][0]["project"])
        write_state(small, [control_record(1, small_checkout)])
        blob = os.path.join(small["slots_root"], "small-aaaa/quick-1/build/blob")
        with open(blob, "wb") as handle:
            handle.write(os.urandom(128 * 1024))
        if os.path.exists(walk_log):
            os.unlink(walk_log)
        ledger_before = sha256_file(os.path.join(small["pool"], "ledger.json"))
        output, elapsed_ns = run_status(subject, small, walk_log, "--measure")
        print(f"measure settled: {elapsed_ns / 1e6:.1f} ms wall")
        expect_line(output, "128 KiB allocated", "measure actual bytes")
        expect_line(output, "measured now", "measure is live")
        expect_line(output, "slot accounting walk: complete", "measure coverage")
        expect_line(output, "allocated by slots", "measure pool bytes")
        reject_line(output, "at least", "complete coverage is exact")
        walks = (
            open(walk_log, encoding="utf-8").read().splitlines()
            if os.path.exists(walk_log)
            else []
        )
        print(f"measure walks: {walks}")
        expect(
            len(walks) == 2,
            f"measure walked {walks}, not exactly the payload and its slot",
        )
        expect(
            walks[0] == os.path.realpath(small["payload"]),
            f"measure's first walk is not the payload: {walks}",
        )
        expect(
            walks[1].endswith("slots/small-aaaa/quick-1"),
            f"measure's second walk is not the slot: {walks}",
        )
        expect(
            sha256_file(os.path.join(small["pool"], "ledger.json")) == ledger_before,
            "measure wrote the ledger",
        )
        expect(
            not os.path.exists(os.path.join(small["control"], "status-observations.json")),
            "--measure persisted an observation",
        )
        expect(
            re.search(r"depth 66, \d+ ms", output) is not None,
            f"measure did not print the shared walk bounds:\n{output}",
        )

        case("measure-evicting")
        ev_root = private_dir(os.path.join(scratch, "evicting"))
        ev_checkout = private_dir(os.path.join(ev_root, "checkout"))
        evicting = build_pool(
            ev_root,
            [
                slot_record(
                    1,
                    4,
                    "evicting",
                    os.path.join(ev_root, "pool", "oxide-managed", "affinity", "1"),
                    ev_checkout,
                    directory="evict-aaaa/quick-1",
                ),
                slot_record(
                    2,
                    1,
                    "idle",
                    os.path.join(ev_root, "pool", "oxide-managed", "affinity", "2"),
                    ev_checkout,
                    directory="evict-aaaa/quick-2",
                ),
            ],
        )
        for slot in evicting["ledger"]["slots"]:
            private_dir(slot["project"])
        write_state(
            evicting,
            [control_record(1, ev_checkout), control_record(2, ev_checkout)],
        )
        ev_slot = os.path.join(evicting["slots_root"], "evict-aaaa/quick-1")
        with open(os.path.join(ev_slot, "pending"), "wb") as handle:
            handle.write(b"z" * 8192)
        ev_ledger = sha256_file(os.path.join(evicting["pool"], "ledger.json"))
        output, _ = run_status(subject, evicting, walk_log, "--measure")
        expect_line(
            output, "recorded deletions pending reconciliation: slot 1", "measure evicting"
        )
        expect_line(
            output,
            "slot 1 recorded deletion pending reconciliation (not idle or held)",
            "measure evicting slot label",
        )
        expect_line(output, "at least", "unknown coverage is a lower bound")
        expect_line(output, "pool coverage: Unknown", "measure reports unknown coverage")
        expect_line(output, "slots 2", "measure keeps every recorded slot")
        expect(
            os.path.exists(os.path.join(ev_slot, "pending")),
            "--measure reconciled and deleted a recorded slot",
        )
        expect(
            sha256_file(os.path.join(evicting["pool"], "ledger.json")) == ev_ledger,
            "--measure wrote the ledger",
        )
        expect(
            not os.path.exists(os.path.join(evicting["control"], "status-observations.json")),
            "--measure persisted an observation",
        )
    finally:
        holder.terminate()
        try:
            holder.wait(timeout=10)
        except subprocess.TimeoutExpired:
            holder.kill()
            holder.wait(timeout=10)

    receipt = {
        "task": "oxide#128",
        "commit": commit,
        "branch": branch,
        "supervisor_sha256": runner_sha,
        "subject_sha256": sha256_file(subject),
        "platform": sys.platform,
        "fixture": "small private structural",
        "default_status_limit_s": DEFAULT_STATUS_LIMIT_S,
        "cases": CASES,
        "load_before": load_before,
        "load_after": loadavg(),
        "limitations": [
            "one author run; not a universal filesystem-latency guarantee",
            "fixture pools are hand-built and private; no real or live pool was read",
            "no installed binary, real drive, consumer or host state was touched",
        ],
    }
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE")
    if evidence:
        os.makedirs(evidence, exist_ok=True)
        with open(os.path.join(evidence, "status-fast.json"), "w", encoding="utf-8") as handle:
            json.dump(receipt, handle, indent=2)
    print("status-fast receipt: " + json.dumps(receipt))


if __name__ == "__main__":
    main()
