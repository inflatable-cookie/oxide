#!/usr/bin/env python3
"""Focused slot-headroom proof for oxide#118, extended for oxide#177.

The qualified shared supervisor owns the 300 s deadline, 512 MiB monitored
scratch, process closure and cleanup. This fixed fixture script reuses the
reviewed oxide#105 staging/proof helper, keeps every write under its supplied
OXIDE_TASK_SCRATCH_ROOT, touches no live pool or drive, and stops work 30
seconds before the exported absolute deadline.

Covered here: the permanent ledger/control fences and the no-migration read;
the rank-exhaustion refusal before any transition; a 256-slot pool whose 257th
and 258th admissions replace the rank-LRU idle whole slot and keep the count,
with no inventory walk at admission and no whole-payload walk on release or
default status (oxide#177's accounting-cost gate); the 12-slot project cap with
a specific held-slot wait, the honest all-held refusal and one LRU idle
replacement after affinity is dropped; ordinary compatible reuse and no
eviction below both ceilings; byte-pressure LRU with a held slot excluded; and
a foreign payload entry refusal, and a native crossing of the legacy 128-slot
ceiling that must write `capacity_policy: 4`. The worst-valid 256-slot ledger
and 512-record control-state serialization/round-trip and oversized-field
refusals are unit tests in `effigy test rust`, not native fixtures.

Modes. The default run is the counted proof and fails fast. `--diagnostic` is
the uncounted keep-going collection: a recoverable phase failure is recorded
and only its declared dependents are skipped, while an unknown outcome, an
unclosed proof child or an exhausted work window aborts the rest. It writes a
bounded `qualified: false` receipt beside the guard's evidence and never a PASS
receipt. `--self-check` is the pure apparatus check of that collection (no
child, scratch, pool or evidence write). An unknown argument refuses before any
effect.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import importlib.util
import json
import os
import secrets
import shutil
import signal
import subprocess
import time

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location(
    "finder_metadata", os.path.join(HERE, "test-oxide-finder-metadata.py")
)
fm = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(fm)
Proof = fm.Proof
expect, fail, summary, MIB = fm.expect, fm.fail, fm.summary, fm.MIB
BINARY = ""
labelled_slot = fm.labelled_slot

REPO = os.path.dirname(HERE)
ROOT = os.path.realpath(os.environ.get("OXIDE_TASK_SCRATCH_ROOT", ""))
EVIDENCE = os.path.realpath(os.environ.get("OXIDE_PROOF_EVIDENCE", ""))
SHUTDOWN_MARGIN_S = 30
RUNNER_SHA256 = "9af104440f4b30d4c5df3828861cb10889cdd5afd315d15a940d5dfb66784e14"
CANDIDATE = "OxSh" + secrets.token_hex(4)
RESULTS = []
CHILDREN = []
COUNTER = 0

# The uncounted diagnostic collection mode (oxide#177). It records a
# recoverable phase failure and continues into the phases that do not depend on
# it, and aborts on any unknown outcome or unclosed proof child. It never
# writes a PASS receipt and never claims qualification.
DIAGNOSTIC = "--diagnostic" in sys.argv[1:]
SCHEMA = "oxide.slot-headroom.receipt/1"
MAX_RECEIPT_BYTES = 128 * 1024
MAX_RECEIPT_ENTRIES = 32
MAX_RECEIPT_EXCERPT = 500
PHASE_NAMES: list = []
PHASE_STATES: dict = {}
PHASE_FAILURES: list = []
PHASE_SKIPS: list = []
TAINTED: set = set()
ABORTED = False
PRIMARY: list = []
SECONDARY: list = []

FIXTURE = {
    "Cargo.toml": '[package]\nname = "slot-headroom-app"\nversion = "0.1.0"\nedition = "2021"\n',
    "src/lib.rs": "pub fn value() -> u32 {\n    3\n}\n",
}

HOLD_SCRIPT = (
    "import pathlib, sys, time\n"
    "pathlib.Path(sys.argv[1]).write_text(str(__import__('os').getpid()))\n"
    "print('tool marker', flush=True)\n"
    "release = pathlib.Path(sys.argv[2])\n"
    "while not release.exists(): time.sleep(0.02)\n"
)


def work_left() -> float:
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    left = (deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 5:
        deadline_fail("work deadline reached before the supervisor shutdown margin")
    return left


def inside_root(path: str) -> bool:
    return os.path.realpath(path).startswith(ROOT + os.sep)


def write_file(path: str, contents: str, mode: int = 0o600) -> None:
    expect(inside_root(path), f"write path is outside supervisor scratch: {path}")
    os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(contents)
    os.chmod(path, mode)


def write_tree(root: str, files: dict[str, str]) -> str:
    for relative, text in files.items():
        write_file(os.path.join(root, relative), text)
    return os.path.realpath(root)


def read_json(path: str):
    with open(path, "r", encoding="utf-8") as handle:
        return json.load(handle)


def walk_log_path() -> str:
    return os.path.join(ROOT, "walk.log")


def walk_snapshot() -> list:
    """The inventory-walk roots recorded so far, one per line."""
    try:
        with open(walk_log_path(), "r", encoding="utf-8", errors="replace") as handle:
            return [line.strip() for line in handle if line.strip()]
    except FileNotFoundError:
        return []


def write_json(path: str, value) -> None:
    write_file(path, json.dumps(value, separators=(",", ":")))


def write_evidence(name: str, contents: str) -> None:
    path = os.path.realpath(os.path.join(EVIDENCE, name))
    expect(path.startswith(EVIDENCE + os.sep), f"evidence path escaped: {path}")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(contents)
    os.chmod(path, 0o600)


def base_env(proof: Proof, pool: str, **extra: str) -> dict:
    env = dict(os.environ)
    # A private HOME must not hide the installed toolchain: keep the real
    # RUSTUP_HOME and the project's exact channel.
    real_rustup = env.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")
    for name in list(env):
        if name in ("OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS", "OXIDE_TASK_SCRATCH_ROOT"):
            continue
        if name.startswith(("OXIDE_", "XDG_", "SCCACHE_", "CARGO_BUILD_", "CARGO_TARGET_")):
            env.pop(name, None)
        elif name in ("CARGO", "CARGO_HOME", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
                      "RUSTUP_TOOLCHAIN", "CARGO_NET_OFFLINE", "CARGO_TERM_COLOR",
                      "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
            env.pop(name, None)
    env.update(
        {
            "OXIDE_POOL": pool,
            "OXIDE_POOL_CAP": "64MiB",
            "OXIDE_MAX_ACTIVE": "16",
            "HOME": os.path.join(ROOT, "home"),
            "OXIDE_TEST_HOME_FROM_ENV": "1",
            "CARGO_HOME": os.path.join(ROOT, "cargo-home"),
            "CARGO_NET_OFFLINE": "true",
            "CARGO_TERM_COLOR": "never",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "",
            # One frozen compiler-flag environment for every fixture child: an
            # inherited RUSTFLAGS or encoded `--config` value would change the
            # fixtures behind the proof's back.
            "RUSTFLAGS": "",
            "RUSTUP_HOME": real_rustup,
            "RUSTUP_TOOLCHAIN": fm.BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "OXIDE_TASK_SCRATCH_ROOT": ROOT,
            "OXIDE_TEST_WALK_LOG": walk_log_path(),
            "OXIDE_TEST_SCOPE_LOG": os.path.join(ROOT, "scope.log"),
            "TMPDIR": ROOT,
            "TMP": ROOT,
            "TEMP": ROOT,
        }
    )
    env.update(extra)
    return env


def spawn(command: list, *, cwd: str, env: dict, log: str) -> subprocess.Popen:
    """Start one child in the supervisor's own session. Nothing here calls
    setsid or start_new_session, so the qualified runner can close every
    descendant on a fault; on a script-side failure only this direct child
    PID is killed and the runner closes the rest of its session."""
    work_left()
    os.makedirs(os.path.dirname(log), mode=0o700, exist_ok=True)
    with open(log, "wb") as output:
        return subprocess.Popen(
            command, cwd=cwd, env=env, stdout=output, stderr=subprocess.STDOUT
        )


def settle(child: subprocess.Popen, log: str) -> tuple:
    try:
        while child.poll() is None:
            work_left()
            time.sleep(0.02)
    except BaseException:
        if child.poll() is None:
            child.kill()
        child.wait()
        raise
    code = child.wait()
    with open(log, "r", encoding="utf-8", errors="replace") as handle:
        return code, handle.read()


def stage(proof: Proof, repo: str) -> str:
    """The reviewed staging recipe: one offline, locked debug build of
    `oxide` into owned target/build directories, on the host Cargo cache."""
    env = dict(os.environ)
    for name in list(env):
        if name.startswith(("SCCACHE_", "OXIDE_", "XDG_")) or name in (
            "CARGO", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR",
            "CARGO_BUILD_BUILD_DIR", "RUSTC", "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_RUSTC_WRAPPER",
            "RUSTUP_TOOLCHAIN", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS",
        ):
            env.pop(name, None)
    target = os.path.join(ROOT, "stage-target")
    env.update(
        {
            "CARGO_TARGET_DIR": target,
            "CARGO_BUILD_BUILD_DIR": os.path.join(ROOT, "stage-build"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_INCREMENTAL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "",
            # The premise checklist's frozen compiler flags: staging never
            # inherits the caller's RUSTFLAGS or an encoded RUSTFLAGS value.
            "RUSTFLAGS": "",
            "RUSTUP_TOOLCHAIN": fm.BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "CARGO_NET_OFFLINE": "true",
        }
    )
    log = os.path.join(ROOT, "logs", "stage-oxide.log")
    child = spawn(
        ["cargo", "build", "--locked", "--offline", "--bin", "oxide"],
        cwd=repo,
        env=env,
        log=log,
    )
    code, text = settle(child, log)
    print(text[-4000:], flush=True)
    expect(code == 0, f"staging exit {code}: {text[-4000:]}")
    binary = os.path.join(target, "debug", "oxide")
    expect(os.path.isfile(binary), f"staging produced no {binary}")
    return binary


def run(proof: Proof, args: list, label: str, cwd: str, env: dict, code: int = 0) -> tuple:
    global COUNTER
    COUNTER += 1
    log = os.path.join(ROOT, "logs", f"{COUNTER:03d}-{label}.log")
    child = spawn([BINARY, *args], cwd=cwd, env=env, log=log)
    actual, text = settle(child, log)
    print(f"----- {label}: exit {actual} -----\n{text}", flush=True)
    expect(actual == code, f"{label} exited {actual}, expected {code}: {text[-4000:]}")
    RESULTS.append({"case": label, "exit": actual})
    return text, text


class Child:
    def __init__(self, name: str, process: subprocess.Popen, log: str, marker: str, release: str):
        self.name = name
        self.process = process
        self.log = log
        self.marker = marker
        self.release = release


def start(proof: Proof, args: list, name: str, cwd: str, env: dict, hold: bool = False) -> Child:
    global COUNTER
    COUNTER += 1
    work_left()
    log = os.path.join(ROOT, "logs", f"{COUNTER:03d}-{name}.log")
    os.makedirs(os.path.dirname(log), mode=0o700, exist_ok=True)
    marker = os.path.join(ROOT, "markers", f"{name}.started")
    release = os.path.join(ROOT, "markers", f"{name}.release")
    for path in (marker, release):
        try:
            os.remove(path)
        except FileNotFoundError:
            pass
    program = [BINARY, *args]
    if hold:
        program += ["--", sys.executable, os.path.join(ROOT, "hold.py"), marker, release]
    process = spawn(program, cwd=cwd, env=env, log=log)
    child = Child(name, process, log, marker, release)
    CHILDREN.append(child)
    return child


def log_text(child: Child) -> str:
    with open(child.log, "r", encoding="utf-8", errors="replace") as handle:
        return handle.read()


def wait_log(child: Child, phrase: str, timeout: float = 60) -> None:
    deadline = time.monotonic() + timeout
    while phrase not in log_text(child):
        work_left()
        if time.monotonic() > deadline:
            fail(f"{child.name} never printed {phrase!r}: {log_text(child)[-2000:]}")
        if child.process.poll() is not None:
            fail(f"{child.name} exited before {phrase!r}: {log_text(child)[-2000:]}")
        time.sleep(0.05)


def wait_child(child: Child, code: int = 0) -> str:
    while child.process.poll() is None:
        work_left()
        time.sleep(0.05)
    actual = child.process.wait()
    text = log_text(child)
    print(f"----- {child.name}: exit {actual} -----\n{text}", flush=True)
    expect(actual == code, f"{child.name} exited {actual}, expected {code}: {text[-3000:]}")
    if child in CHILDREN:
        CHILDREN.remove(child)
    RESULTS.append({"case": child.name, "exit": actual})
    return text


def wait_started(child: Child) -> None:
    wait_log(child, "tool marker", timeout=90)


def release(child: Child) -> None:
    write_file(child.release, "release\n")
    wait_child(child)


def close_all() -> None:
    # Only direct child PIDs we started. Every child stays in the qualified
    # supervisor's session, so its descendants are the runner's to close; no
    # session or process-group signal is sent from here.
    for child in list(CHILDREN):
        if child.process.poll() is None:
            child.process.kill()
        child.process.wait()
        CHILDREN.remove(child)


def slot_dir(pool: str, slot_id: str) -> str:
    return labelled_slot(os.path.join(pool, "payload", "slots"), int(slot_id))


def slot_dirs(pool: str) -> list:
    slots_root = os.path.join(pool, "payload", "slots")
    found = []
    for folder in os.listdir(slots_root):
        base = os.path.join(slots_root, folder)
        if not os.path.isdir(base):
            continue
        for leaf in os.listdir(base):
            found.append(os.path.join(base, leaf))
    return sorted(found)


def own_slot_walk_root(payload_root: str, slot_id: str, record: dict) -> str:
    """The exact own-slot walk root for a slot's release, from its ledger row.

    oxide#104's fallback is the legacy flat `slots/<id>` when no project folder
    is available (`dir` absent or null); a recorded `folder/leaf` is that exact
    path under `slots/`.
    """
    slots_root = os.path.join(payload_root, "slots")
    directory = record.get("dir")
    if directory is None:
        return os.path.join(slots_root, slot_id)
    return os.path.join(slots_root, *directory.split("/"))


def prove_fences_and_exhaustion(proof: Proof, app: str, pool: str, env: dict) -> None:
    stdout, stderr = run(proof, ["exec", "--", sys.executable, "-c", "pass"], "first-grant", app, env)
    ledger_path = os.path.join(pool, "ledger.json")
    ledger = read_json(ledger_path)
    expect(ledger.get("capacity_policy") == 3, "the first grant did not install the merged capacity fence")
    expect(ledger.get("next_rank") == 3, f"unexpected rank sequence after grant+release: {ledger.get('next_rank')}")
    expect(len(ledger["slots"]) == 1, "the first grant did not record one slot")
    slot = ledger["slots"][0]
    expect(slot["rank"] == 2 and slot["state"] == "idle", f"release did not stamp the idle rank: {slot}")
    state_path = os.path.join(pool, "oxide-managed", "state.json")
    state = read_json(state_path)
    expect(state["version"] == 8, f"the first mutation did not install the version-8 control fences: {state}")
    expect(state.get("slot_ceiling") == "oxide.slots/256", f"the raised-ceiling fence is missing: {state}")
    expect(state.get("eviction_accounting") == "oxide.eviction-accounting/1", f"the eviction-accounting fence is missing: {state}")
    RESULTS.append({"case": "fences-installed", "ledger_policy": 3, "state_version": 8})

    # A legacy representation (no fence, rank 0) still reads for status and
    # is not migrated by the observer.
    legacy = dict(ledger)
    legacy.pop("capacity_policy")
    legacy.pop("next_rank")
    legacy["slots"] = [dict(slot) for slot in ledger["slots"]]
    for record in legacy["slots"]:
        record["rank"] = 0
        # A legacy representation has no capacity fence, so it cannot carry a
        # closed observation either: the product refuses that combination.
        record.pop("recorded", None)
    write_json(ledger_path, legacy)
    before = open(ledger_path, "rb").read()
    run(proof, ["status"], "legacy-status", app, env)
    expect(open(ledger_path, "rb").read() == before, "a read-only status migrated the legacy ledger")
    legacy_state = dict(state)
    # A genuinely legacy document carries none of the forward fences; keeping
    # the recorded-accounting or raised-ceiling fence would test a shape no
    # legacy binary wrote.
    for fence in ("recorded_accounting", "one_use_workspaces", "eviction_accounting", "slot_ceiling"):
        legacy_state.pop(fence, None)
    legacy_state["version"] = 1
    write_json(state_path, legacy_state)
    before_state = open(state_path, "rb").read()
    run(proof, ["status"], "legacy-state-status", app, env)
    expect(open(state_path, "rb").read() == before_state,
           "a read-only status migrated the legacy control state")
    expect(read_json(state_path)["version"] == 1, "a read-only status migrated the legacy control state")
    run(proof, ["exec", "--", sys.executable, "-c", "pass"], "legacy-first-write", app, env)
    fenced = read_json(ledger_path)
    fenced_state = read_json(state_path)
    expect(fenced.get("capacity_policy") == 3 and fenced_state["version"] == 8,
           "the first revised mutation did not install the version-8 control fences")
    # Fence first: even this one-slot pool gains the raised-ceiling and
    # eviction-accounting fences before its first ledger write.
    expect(fenced_state.get("slot_ceiling") == "oxide.slots/256",
           f"the first revised mutation did not fence the raised ceiling: {fenced_state}")
    expect(fenced_state.get("eviction_accounting") == "oxide.eviction-accounting/1",
           f"the first revised mutation did not fence eviction accounting: {fenced_state}")
    RESULTS.append({"case": "legacy-read-no-migration", "fenced_on_mutation": True, "raised_fence": True})

    # Counter exhaustion refuses before any transition.
    exhausted = read_json(ledger_path)
    exhausted["next_rank"] = 18446744073709551615
    write_json(ledger_path, exhausted)
    before = open(ledger_path, "rb").read()
    _, stderr = run(proof, ["exec", "--", sys.executable, "-c", "pass"], "rank-exhaustion", app, env, code=1)
    expect("finite ledger or slot count is exhausted" in stderr, f"exhaustion refusal was not honest: {stderr}")
    expect(open(ledger_path, "rb").read() == before, "an exhausted rank sequence changed the pool")
    run(proof, ["status"], "exhaustion-status", app, env)
    restored = read_json(ledger_path)
    restored["next_rank"] = fenced["next_rank"]
    write_json(ledger_path, restored)
    RESULTS.append({"case": "rank-exhaustion-refuses-before-transition", "unchanged": True})


def prove_raised_fence(proof: Proof, pool: str, env: dict, workdir: str) -> None:
    """A slot table crossing the legacy 128 ceiling must carry the raised
    fence, written by the product in the same ledger replacement (oxide#177)."""
    ledger_path = os.path.join(pool, "ledger.json")
    app = write_tree(os.path.join(workdir, "fence-app"), FIXTURE)
    run(proof, ["exec", "--", sys.executable, "-c", "pass"], "raised-fence-prime", app, env)
    ledger = read_json(ledger_path)
    expect(ledger.get("capacity_policy") == 3, "the priming grant did not install the merged fence")
    real = ledger["slots"][0]
    folder = real["dir"].split("/", 1)[0]
    store_id = ledger["store_id"]
    slots_root = os.path.join(pool, "payload", "slots", folder)
    os.makedirs(slots_root, mode=0o700, exist_ok=True)
    os.chmod(slots_root, 0o700)
    rank = max(record["rank"] for record in ledger["slots"]) + 1
    for slot_id in range(2, 129):
        last_lease_id = f"v1.{store_id}.{slot_id}.1.0"
        ledger["slots"].append({
            "id": slot_id,
            "incarnation": 1,
            "state": "idle",
            "project": real["project"] + "-seed",
            "workspace": real["workspace"],
            "profile": "dev",
            "holder": "seed",
            "lease_id": None,
            "last_lease_id": last_lease_id,
            "dir": f"{folder}/quick-{slot_id}",
            "rank": rank,
            "recorded": {
                "for_lease": last_lease_id,
                "unique_bytes": 0,
                "logical_bytes": 0,
                "entries": 0,
                "at_ms": 1,
            },
        })
        rank += 1
        build = os.path.join(slots_root, f"quick-{slot_id}", "build")
        os.makedirs(build, mode=0o700, exist_ok=True)
        for path in (os.path.join(slots_root, f"quick-{slot_id}"), build):
            os.chmod(path, 0o700)
    ledger["next_slot"] = 129
    ledger["next_rank"] = rank
    write_json(ledger_path, ledger)

    # The version-2 representation at exactly the legacy ceiling still reads.
    run(proof, ["status"], "raised-fence-128-status", app, env)
    expect(len(read_json(ledger_path)["slots"]) == 128, "the 128-slot fixture did not read back")
    # A debug-profile command has no compatible idle slot, so it allocates the
    # 129th slot. That single ledger replacement must raise the fence.
    run(
        proof,
        ["exec", "--profile", "debug", "--", sys.executable, "-c", "pass"],
        "raised-fence-129th",
        app,
        env,
    )
    after = read_json(ledger_path)
    expect(len(after["slots"]) == 129, f"the 129th slot was not recorded: {len(after['slots'])}")
    expect(any(record["id"] == 129 for record in after["slots"]), "the 129th slot was not allocated")
    expect(
        after.get("capacity_policy") == 4,
        f"crossing the legacy 128-slot ceiling did not raise the ledger fence: {after.get('capacity_policy')}",
    )
    state = read_json(os.path.join(pool, "oxide-managed", "state.json"))
    expect(
        state["version"] == 8 and state.get("slot_ceiling") == "oxide.slots/256",
        f"the 129th slot did not make the raised-ceiling control fence durable: {state}",
    )
    RESULTS.append({
        "case": "raised-fence-at-129-slots",
        "policy_before": 3,
        "policy_after": 4,
        "state_version": state["version"],
        "slots": 129,
    })


def prove_pool_ceiling(proof: Proof, app: str, pool: str, env: dict) -> None:
    ledger_path = os.path.join(pool, "ledger.json")
    ledger = read_json(ledger_path)
    real = ledger["slots"][0]
    folder = real["dir"].split("/", 1)[0]
    # A distinct project path keeps the synthetic idle slots incompatible
    # with this checkout's record; they exist only as pool members. Every one
    # carries a current closed observation, so a count replacement walks no
    # inventory at all (oxide#146).
    module = real["project"] + "-seed"
    workspace = real["workspace"]
    store_id = ledger["store_id"]
    slots_root = os.path.join(pool, "payload", "slots", folder)
    os.makedirs(slots_root, mode=0o700, exist_ok=True)
    os.chmod(slots_root, 0o700)
    rank = max(record["rank"] for record in ledger["slots"]) + 1
    for slot_id in range(2, 257):
        last_lease_id = f"v1.{store_id}.{slot_id}.1.0"
        record = {
            "id": slot_id,
            "incarnation": 1,
            "state": "idle",
            "project": module,
            "workspace": workspace,
            "profile": "dev",
            "holder": "seed",
            "lease_id": None,
            "last_lease_id": last_lease_id,
            "dir": f"{folder}/quick-{slot_id}",
            "rank": rank,
            "recorded": {
                "for_lease": last_lease_id,
                "unique_bytes": 0,
                "logical_bytes": 0,
                "entries": 0,
                "at_ms": 1,
            },
        }
        ledger["slots"].append(record)
        rank += 1
        build = os.path.join(slots_root, f"quick-{slot_id}", "build")
        os.makedirs(build, mode=0o700, exist_ok=True)
        for path in (os.path.join(slots_root, f"quick-{slot_id}"), build):
            os.chmod(path, 0o700)
    ledger["next_slot"] = 257
    ledger["next_lease"] = 1000
    ledger["next_rank"] = rank
    # A slot table above the legacy 128 must carry the raised-ceiling fence,
    # which is what an older binary refuses.
    ledger["capacity_policy"] = 4
    write_json(ledger_path, ledger)
    # The product makes the matching control fence durable before it first
    # records the 129th slot (proved by the raised-fence case), so the fixture
    # models the same consistent pair instead of a state no writer produces.
    state_path = os.path.join(pool, "oxide-managed", "state.json")
    state = read_json(state_path)
    state["version"] = 8
    state["slot_ceiling"] = "oxide.slots/256"
    state["eviction_accounting"] = "oxide.eviction-accounting/1"
    write_json(state_path, state)

    # Default status at the 256-slot ceiling is recorded-only: it walks
    # nothing.
    before = walk_snapshot()
    stdout, stderr = run(proof, ["status"], "synthetic-256-status", app, env)
    added = walk_snapshot()[len(before):]
    expect(not added, f"default status at 256 slots walked inventory: {added}")
    expect(
        "slots 256" in stdout + stderr,
        f"status did not report the 256 recorded slots: {stdout}{stderr}",
    )
    expect(len(read_json(ledger_path)["slots"]) == 256, "the 256-slot fixture did not read back")
    RESULTS.append({"case": "synthetic-256-ledger", "slots": 256, "status_walks": 0})

    payload_root = os.path.realpath(os.path.join(pool, "payload"))

    # The 257th admission replaces the rank-LRU idle whole slot: slot 1 has
    # rank 2, the oldest. A debug-profile command has no compatible idle slot.
    victim = slot_dir(pool, "1")
    debug_marker = os.path.join(ROOT, "debug-executed")
    try:
        os.remove(debug_marker)
    except FileNotFoundError:
        pass
    before = walk_snapshot()
    _, stderr = run(
        proof,
        ["exec", "--profile", "debug", "--", sys.executable, "-c", "open(%r,'w').close()" % debug_marker],
        "pool-ceiling-257th",
        app,
        env,
    )
    added = walk_snapshot()[len(before):]
    expect(os.path.exists(debug_marker), "the 257th command did not run")
    expect(not os.path.lexists(victim), f"the rank-LRU idle slot {victim} was not evicted whole")
    # No whole-payload walk and no walk of the evicted slot: the only walk is
    # the release of the command's own new slot.
    expect(
        all(root != payload_root for root in added),
        f"the count replacement walked the whole payload: {added}",
    )
    expect(
        all(not root.startswith(os.path.realpath(victim)) for root in added),
        f"the count replacement walked the evicted slot: {added}",
    )
    # The sole added walk is the release of slot 257's own root. Derive that
    # exact root from the slot's own ledger row: the legacy flat `slots/257`
    # when `dir` is absent (oxide#104's fallback), otherwise the recorded
    # folder/leaf path.
    after = read_json(ledger_path)
    own = next((record for record in after["slots"] if record["id"] == 257), None)
    expect(own is not None, "the 257th slot has no ledger record")
    expected_walk = own_slot_walk_root(payload_root, "257", own)
    expect(
        added == [expected_walk],
        f"the 257th admission/release walked {added}, expected only {expected_walk}",
    )
    expect(len(after["slots"]) == 256, f"pool count {len(after['slots'])} is not 256")
    expect(all(record["id"] != 1 for record in after["slots"]), "the evicted record remains")
    expect(any(record["id"] == 257 for record in after["slots"]), "the 257th slot was not allocated")
    expect(after.get("capacity_policy") == 4, "the 257th slot did not raise the ledger fence")
    state = read_json(state_path)
    expect(
        state["version"] == 8 and state.get("slot_ceiling") == "oxide.slots/256",
        f"an ordinary release dropped the raised-ceiling control fence: {state}",
    )
    RESULTS.append({
        "case": "pool-ceiling-257th-replaces-rank-lru",
        "slots": 256,
        "victim": 1,
        "walks": len(added),
    })

    # A further quick admission replaces the next oldest idle slot; the count
    # stays at 256 and no old 32-slot churn occurred.
    victim = slot_dir(pool, "2")
    before = walk_snapshot()
    run(proof, ["exec", "--", sys.executable, "-c", "pass"], "pool-ceiling-258th", app, env)
    added = walk_snapshot()[len(before):]
    expect(not os.path.lexists(victim), f"the next rank-LRU idle slot {victim} was not evicted")
    after = read_json(ledger_path)
    own = next((record for record in after["slots"] if record["id"] == 258), None)
    expect(own is not None, "the 258th slot has no ledger record")
    expected_walk = own_slot_walk_root(payload_root, "258", own)
    expect(
        added == [expected_walk],
        f"the 258th admission/release walked {added}, expected only {expected_walk}",
    )
    expect(len(after["slots"]) == 256, f"pool count {len(after['slots'])} is not 256 after replacement")
    expect(any(record["id"] == 258 for record in after["slots"]), "the 258th slot was not allocated")
    RESULTS.append({"case": "pool-ceiling-repeats-without-churn", "slots": 256, "victim": 2, "walks": len(added)})


PROJECT_SLOT_CAP = 12


def prove_project_cap(proof: Proof, pool: str, env: dict, workdir: str) -> None:
    app = write_tree(os.path.join(workdir, "project-app"), FIXTURE)
    # Create the pool with one sequential command first: simultaneous first
    # commands race the pool's own one-time creation, which is a separate
    # pre-existing issue and not what this case measures.
    run(proof, ["exec", "--", sys.executable, "-c", "pass"], "project-priming", app, env)
    holds = []
    for index in range(PROJECT_SLOT_CAP):
        holds.append(start(proof, ["exec"], f"project-hold-{index}", app, env, hold=True))
    for child in holds:
        wait_started(child)
    ledger = read_json(os.path.join(pool, "ledger.json"))
    expect(len(ledger["slots"]) == PROJECT_SLOT_CAP,
           f"{PROJECT_SLOT_CAP} concurrent commands did not keep {PROJECT_SLOT_CAP} slots: {len(ledger['slots'])}")
    expect(all(record["state"] == "held" for record in ledger["slots"]), "a concurrent project slot is not held")
    RESULTS.append({"case": "project-cap-holds-twelve", "slots": PROJECT_SLOT_CAP})

    waiter = start(proof, ["exec"], "project-waiter", app, env, hold=True)
    wait_log(waiter, "waiting for another oxide command in this checkout")
    expect(not os.path.exists(waiter.marker),
           f"the {PROJECT_SLOT_CAP + 1}th command started while all {PROJECT_SLOT_CAP} slots were held")
    RESULTS.append({"case": "project-cap-waits-for-specific-held-slot", "waiting": True})
    release(holds[0])
    wait_started(waiter)
    expect(os.path.lexists(slot_dir(pool, "1")), "the released slot is missing")
    release(waiter)
    for child in holds[1:]:
        release(child)
    ledger = read_json(os.path.join(pool, "ledger.json"))
    expect(len(ledger["slots"]) == PROJECT_SLOT_CAP,
           f"release did not leave {PROJECT_SLOT_CAP} idle slots: {len(ledger['slots'])}")
    expect(all(record["state"] == "idle" for record in ledger["slots"]), "a released project slot stayed held")

    # Drop this checkout's affinity, then a new command has no slot of its own
    # and the project cap replaces the rank-LRU idle whole slot.
    idle = [record for record in ledger["slots"] if record["state"] == "idle"]
    victim_record = min(idle, key=lambda record: (record["rank"], record["id"]))
    victim = slot_dir(pool, str(victim_record["id"]))
    run(proof, ["clean"], "project-cap-clean", app, env)

    # Recorded accounting trusts a current idle measurement (oxide#146), so a
    # current record would mean no walk at all and the depth-gap oracle below
    # would never run. The victim is made unrecorded exactly as a legacy pool or
    # a release whose walk did not complete leaves it, and the replacement then
    # has to walk it before deciding.
    ledger_path = os.path.join(pool, "ledger.json")
    unrecorded = read_json(ledger_path)
    for record in unrecorded["slots"]:
        if record["id"] == victim_record["id"]:
            record.pop("recorded", None)
    write_json(ledger_path, unrecorded)
    victim_record = next(
        record for record in unrecorded["slots"] if record["id"] == victim_record["id"]
    )
    expect("recorded" not in victim_record, "the depth-gap victim still carries a current record")

    # A victim whose tree exceeds the accounting depth bound must refuse the
    # replacement before deleting anything: the payload walk over the
    # unrecorded victim is not complete, so no idle slot is removed.
    deep = os.path.join(victim, "build", "deep")
    current = deep
    for _ in range(70):
        current = os.path.join(current, "d")
        os.makedirs(current, mode=0o700, exist_ok=True)
    _, refused = run(
        proof, ["exec", "--", sys.executable, "-c", "pass"], "project-cap-unknown", app, env, code=1
    )
    expect(
        "unknown_accounting" in refused and "depth_limit" in refused,
        f"the project replacement refused without naming its accounting gap: {refused}",
    )
    expect(os.path.isdir(victim), "the victim was deleted before complete accounting")
    expect(
        len(read_json(os.path.join(pool, "ledger.json"))["slots"]) == PROJECT_SLOT_CAP,
        "an incomplete-accounting refusal changed the project membership",
    )
    RESULTS.append({"case": "project-cap-unknown-refuses-before-deletion", "victim": victim_record["id"]})
    shutil.rmtree(deep)

    _, stderr = run(proof, ["exec", "--", sys.executable, "-c", "pass"], "project-cap-overflow", app, env)
    expect(
        f"evicted idle slot {victim_record['id']} (project slot cap {PROJECT_SLOT_CAP})" in stderr,
        f"the project cap did not evict the rank-LRU idle slot: {stderr}",
    )
    expect(not os.path.lexists(victim), f"the rank-LRU project slot {victim} was not evicted")
    after = read_json(os.path.join(pool, "ledger.json"))
    expect(len(after["slots"]) == PROJECT_SLOT_CAP,
           f"project cap count {len(after['slots'])} is not {PROJECT_SLOT_CAP}")
    RESULTS.append({"case": "project-cap-overflow-replaces-rank-lru", "slots": PROJECT_SLOT_CAP, "victim": victim_record["id"]})


def prove_reuse_and_no_churn(proof: Proof, pool: str, env: dict, workdir: str) -> None:
    app = write_tree(os.path.join(workdir, "reuse-app"), FIXTURE)
    _, stderr = run(proof, ["exec", "--", sys.executable, "-c", "pass"], "reuse-cold", app, env)
    expect("cold" in summary(stderr), f"the first command was not cold: {summary(stderr)}")
    _, stderr = run(proof, ["exec", "--", sys.executable, "-c", "pass"], "reuse-warm", app, env)
    expect("warm" in summary(stderr), f"the compatible idle slot was not reused: {summary(stderr)}")
    ledger = read_json(os.path.join(pool, "ledger.json"))
    expect(len(ledger["slots"]) == 1, f"compatible reuse allocated another slot: {len(ledger['slots'])}")
    RESULTS.append({"case": "compatible-reuse-first", "slots": 1})


def prove_byte_pressure(proof: Proof, pool: str, env: dict, workdir: str) -> None:
    first = write_tree(os.path.join(workdir, "byte-app-a"), FIXTURE)
    second = write_tree(os.path.join(workdir, "byte-app-b"), FIXTURE)
    third = write_tree(os.path.join(workdir, "byte-app-c"), FIXTURE)
    limited = dict(env)
    limited["OXIDE_TEST_BUDGET_BYTES"] = str(3 * MIB)
    # Each leased command writes its own slot (through the build directory the
    # lease hands it), so the release's complete walk records those bytes. An
    # idle slot is never edited after release: that would leave the recorded
    # figure this case relies on stale.
    def writer(letter: str, count: int) -> str:
        return (
            "import os, pathlib\n"
            f"pathlib.Path(os.environ['CARGO_BUILD_BUILD_DIR'], 'blob').write_bytes(b'{letter}' * ({count}))\n"
        )

    run(proof, ["exec", "--", sys.executable, "-c", writer("a", 2 * MIB)], "byte-first", first, limited)
    aged = slot_dir(pool, "1")
    run(
        proof,
        ["exec", "--profile", "debug", "--", sys.executable, "-c", writer("b", 1 * MIB)],
        "byte-second",
        first,
        limited,
    )
    newer = slot_dir(pool, "2")
    held = start(proof, ["exec"], "byte-held", third, limited, hold=True)
    wait_started(held)
    held_marker = os.path.join(ROOT, "markers", "byte-held.started")
    run(proof, ["exec", "--", sys.executable, "-c", "pass"], "byte-third", second, limited)
    expect(not os.path.lexists(aged), "byte pressure did not evict the rank-LRU idle whole slot")
    expect(os.path.lexists(newer), "byte pressure evicted a newer idle slot first")
    expect(os.path.exists(held_marker), "byte pressure removed the held slot's command marker")
    ledger = read_json(os.path.join(pool, "ledger.json"))
    held_record = next(record for record in ledger["slots"] if record["state"] == "held")
    expect(held_record is not None, "the held slot is not recorded as held")
    RESULTS.append({"case": "byte-pressure-rank-lru-held-excluded", "victim": 1, "held": held_record["id"]})
    release(held)


def prove_foreign_refusal(proof: Proof, pool: str, env: dict, app: str) -> None:
    foreign = os.path.join(pool, "payload", "foreign-entry")
    write_file(foreign, "foreign\n")
    stdout, stderr = run(proof, ["status"], "foreign-entry-status", app, env)
    expect(
        'unexpected entry "foreign-entry"' in stdout + stderr,
        f"the foreign entry was not refused by name: {stdout}{stderr}",
    )
    os.remove(foreign)
    RESULTS.append({"case": "foreign-payload-entry-refused", "reported": True})


class PhaseFailure(Exception):
    """A recorded phase failure; `abort` marks an outcome no oracle may continue."""

    def __init__(self, message: str, *, abort: bool = False) -> None:
        super().__init__(message)
        self.abort = abort


def note_fault(message: str) -> None:
    """Preserve the first fault; later ones stay visible as secondary."""
    if not PRIMARY:
        PRIMARY.append(message)
    else:
        SECONDARY.append(message)


def abort_now(message: str) -> None:
    """The one abort transition: module state and the first fault."""
    global ABORTED
    ABORTED = True
    note_fault(message)


def phase_fail(message: str) -> None:
    """Diagnostic-mode `fail`: record it instead of exiting the process."""
    raise PhaseFailure(message)


def phase_expect(condition: bool, message: str) -> None:
    if not condition:
        raise PhaseFailure(message)


def deadline_fail(message: str) -> None:
    """An exhausted work window ends the collection; it is never an oracle."""
    if DIAGNOSTIC:
        raise PhaseFailure(message, abort=True)
    fail(message)


def dispatch(phases: list, diagnostic: bool) -> None:
    """Run the fixed phases in dependency order.

    Counted mode propagates the first failure. Diagnostic mode records a
    recoverable failure, skips only phases whose declared prerequisites are
    tainted, and aborts the collection on an unknown outcome or unclosed proof
    child (a killed leader can leave its tool group in the supervisor session).
    """
    global PHASE_NAMES
    PHASE_NAMES = [name for name, _, _ in phases]
    if diagnostic:
        # Every planned phase starts unrun, so an abort leaves a complete,
        # truthful map instead of a missing entry.
        PHASE_STATES.update({name: "not_run" for name in PHASE_NAMES})
    for name, depends, body in phases:
        if not diagnostic:
            body()
            continue
        blocked = [dependency for dependency in depends if dependency in TAINTED]
        if blocked:
            PHASE_STATES[name] = "skip"
            PHASE_SKIPS.append((name, f"dependency failed: {', '.join(blocked)}"))
            TAINTED.add(name)
            print(f"skip {name} (dependency failed: {', '.join(blocked)})", flush=True)
            continue
        PHASE_STATES[name] = "reached"
        print(f"phase {name}", flush=True)
        try:
            body()
        except PhaseFailure as failure:
            PHASE_STATES[name] = "aborted" if failure.abort else "fail"
            PHASE_FAILURES.append((name, str(failure)))
            TAINTED.add(name)
            note_fault(f"{name}: {failure}")
            if failure.abort:
                abort_now(f"{name}: {failure}")
                print(f"abort {name}: {failure}", flush=True)
                return
            print(f"failed {name}: {failure}", flush=True)
            if CHILDREN:
                abort_now(f"{name}: unclosed proof children after a failure")
                print(f"abort {name}: unclosed proof children", flush=True)
                return
        except OSError as error:
            PHASE_STATES[name] = "aborted"
            PHASE_FAILURES.append((name, f"infrastructure error: {error}"))
            abort_now(f"{name}: infrastructure error: {error}")
            print(f"abort {name}: {error}", flush=True)
            return
        except Exception as error:  # noqa: BLE001 - never an oracle continuation
            PHASE_STATES[name] = "aborted"
            PHASE_FAILURES.append((name, f"unexpected {type(error).__name__}: {error}"))
            abort_now(f"{name}: unexpected {type(error).__name__}: {error}")
            print(f"abort {name}: unexpected {type(error).__name__}: {error}", flush=True)
            return
        else:
            if CHILDREN:
                PHASE_STATES[name] = "aborted"
                abort_now(f"{name}: proof children were not closed")
                print(f"abort {name}: proof children were not closed", flush=True)
                return
            PHASE_STATES[name] = "pass"


def bounded_excerpt(text) -> str:
    text = str(text)
    if len(text) > MAX_RECEIPT_EXCERPT:
        return text[:MAX_RECEIPT_EXCERPT] + "..."
    return text


def receipt_document(source: str, tree: str, runner_hash: str, binary_sha256: str) -> dict:
    states = {name: PHASE_STATES.get(name, "not_run") for name in PHASE_NAMES}
    counts = {
        "pass": sum(1 for state in states.values() if state == "pass"),
        "fail": sum(1 for state in states.values() if state == "fail"),
        "skip": sum(1 for state in states.values() if state == "skip"),
        "aborted": sum(1 for state in states.values() if state == "aborted"),
        "not_run": sum(1 for state in states.values() if state == "not_run"),
        "planned": len(states),
    }
    with open(os.path.abspath(__file__), "rb") as handle:
        import hashlib
        script_sha256 = hashlib.sha256(handle.read()).hexdigest()
    document = {
        "schema": SCHEMA,
        "task": "oxide#177",
        "mode": "diagnostic",
        "qualified": False,
        "source_commit": source,
        "source_tree": tree,
        "binary_sha256": binary_sha256,
        "runner_sha256": runner_hash,
        "script_sha256": script_sha256,
        "phase_states": states,
        "counts": counts,
        "failures": [(name, bounded_excerpt(reason)) for name, reason in PHASE_FAILURES][:MAX_RECEIPT_ENTRIES],
        "skips": [(name, bounded_excerpt(reason)) for name, reason in PHASE_SKIPS][:MAX_RECEIPT_ENTRIES],
        "tainted": sorted(TAINTED),
        "aborted": ABORTED,
        "primary_error": bounded_excerpt(PRIMARY[0]) if PRIMARY else None,
        "secondary_errors": [bounded_excerpt(entry) for entry in SECONDARY][:MAX_RECEIPT_ENTRIES // 4],
        "collection_complete": (not ABORTED) and counts["not_run"] == 0 and counts["skip"] == 0,
        "limits": {
            "wall_clock_seconds": 300,
            "shutdown_margin_seconds": SHUTDOWN_MARGIN_S,
            "scratch_budget_mib": 512,
            "receipt_bytes": MAX_RECEIPT_BYTES,
            "receipt_entries": MAX_RECEIPT_ENTRIES,
        },
        "claim": "uncounted diagnostic collection; never a PASS, closure or native credit",
    }
    encoded = json.dumps(document, indent=2, sort_keys=True)
    if len(encoded.encode()) > MAX_RECEIPT_BYTES:
        document["failures"] = document["failures"][: MAX_RECEIPT_ENTRIES // 4]
        document["skips"] = document["skips"][: MAX_RECEIPT_ENTRIES // 4]
        document["secondary_errors"] = document["secondary_errors"][: MAX_RECEIPT_ENTRIES // 8]
        document["omitted"] = "entries truncated to the receipt byte bound"
        encoded = json.dumps(document, indent=2, sort_keys=True)
    if len(encoded.encode()) > MAX_RECEIPT_BYTES:
        raise PhaseFailure("the diagnostic receipt cannot fit its byte bound", abort=True)
    return document


def write_diagnostic_receipt(document: dict) -> str:
    path = os.path.realpath(
        os.path.join(EVIDENCE, f"slot-headroom-diagnostic-{os.getpid()}-{time.time_ns()}.json")
    )
    expect(path.startswith(EVIDENCE + os.sep), f"diagnostic receipt escaped: {path}")
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(document, handle, indent=2, sort_keys=True)
    os.chmod(path, 0o600)
    return path


def reset_collection() -> None:
    global PHASE_NAMES, PHASE_STATES, PHASE_FAILURES, PHASE_SKIPS, TAINTED
    global ABORTED, PRIMARY, SECONDARY
    PHASE_NAMES = []
    PHASE_STATES = {}
    PHASE_FAILURES = []
    PHASE_SKIPS = []
    TAINTED = set()
    ABORTED = False
    PRIMARY = []
    SECONDARY = []


def self_check() -> None:
    """Pure apparatus check of the diagnostic collection: no child, scratch,
    pool or evidence write."""
    global PHASE_NAMES, PHASE_FAILURES, PHASE_SKIPS

    def ok() -> None:
        pass

    def recoverable() -> None:
        raise PhaseFailure("recoverable oracle failure")

    def unknown() -> None:
        raise PhaseFailure("custody unknown", abort=True)

    reset_collection()
    dispatch(
        [
            ("a", [], ok),
            ("b", [], recoverable),
            ("c", ["b"], ok),
            ("d", ["a"], ok),
        ],
        True,
    )
    expect(
        PHASE_STATES == {"a": "pass", "b": "fail", "c": "skip", "d": "pass"},
        f"recoverable collection states are wrong: {PHASE_STATES}",
    )
    expect(PHASE_FAILURES == [("b", "recoverable oracle failure")], f"failures: {PHASE_FAILURES}")
    expect(PHASE_SKIPS == [("c", "dependency failed: b")], f"skips: {PHASE_SKIPS}")
    expect(not ABORTED, "a recoverable failure aborted the collection")
    expect(PRIMARY == ["b: recoverable oracle failure"], f"primary fault: {PRIMARY}")

    reset_collection()
    dispatch([("a", [], unknown), ("b", [], ok)], True)
    expect(PHASE_STATES["a"] == "aborted" and PHASE_STATES["b"] == "not_run",
           f"an abort did not leave later phases unrun: {PHASE_STATES}")
    expect(ABORTED, "an unknown outcome did not abort the collection")

    reset_collection()
    try:
        dispatch([("a", [], recoverable)], False)
    except PhaseFailure:
        pass
    else:
        fail("counted dispatch swallowed a failure")

    reset_collection()
    PHASE_NAMES = ["phase-%02d" % index for index in range(MAX_RECEIPT_ENTRIES)]
    PHASE_FAILURES = [(name, "x" * 4000) for name in PHASE_NAMES]
    PHASE_SKIPS = [(name, "y" * 4000) for name in PHASE_NAMES]
    SECONDARY[:] = ["z" * 4000 for _ in range(MAX_RECEIPT_ENTRIES)]
    document = receipt_document("a" * 40, "b" * 40, "c" * 64, "d" * 64)
    encoded = json.dumps(document, indent=2, sort_keys=True)
    expect(len(encoded.encode()) <= MAX_RECEIPT_BYTES, f"worst receipt {len(encoded)} bytes exceeds the bound")
    expect(document["qualified"] is False, "a diagnostic receipt claimed qualification")
    expect(document["mode"] == "diagnostic", "a diagnostic receipt has the wrong mode")
    expect(document["counts"]["planned"] == MAX_RECEIPT_ENTRIES, "the receipt lost a planned phase")
    reset_collection()
    # The own-slot walk oracle derives the exact release root from the ledger
    # row: oxide#104's legacy flat fallback and the recorded labelled path.
    self_slots = "/tmp/payload/slots"
    expect(
        own_slot_walk_root("/tmp/payload", "257", {"id": 257})
        == os.path.join(self_slots, "257"),
        "the flat own-slot walk root is wrong",
    )
    expect(
        own_slot_walk_root("/tmp/payload", "257", {"id": 257, "dir": None})
        == os.path.join(self_slots, "257"),
        "an explicit null dir is not the flat own-slot walk root",
    )
    expect(
        own_slot_walk_root(
            "/tmp/payload", "257", {"id": 257, "dir": "fences-abc/quick-257"}
        )
        == os.path.join(self_slots, "fences-abc", "quick-257"),
        "the labelled own-slot walk root is wrong",
    )
    print("slot-headroom self-check PASS: diagnostic collection, taint, abort, receipt bounds and own-slot walk paths", flush=True)


def main() -> None:
    arguments = [argument for argument in sys.argv[1:] if argument != "--diagnostic"]
    if arguments == ["--self-check"]:
        self_check()
        return
    if arguments:
        print(f"unknown argument: {arguments[0]}", file=sys.stderr)
        raise SystemExit(2)
    expect(os.path.isdir(ROOT) and os.path.realpath(ROOT) == os.environ["OXIDE_TASK_SCRATCH_ROOT"],
           "scratch root is not canonical")
    expect(os.path.isdir(EVIDENCE), "qualified-runner evidence directory is missing")

    source = ""
    tree = ""
    binary_sha256 = ""
    runner_hash = ""
    exit_code = 0
    try:
        work_left()
        with open(os.path.join(REPO, "scripts", "bounded-test.py"), "rb") as handle:
            import hashlib
            runner_hash = hashlib.sha256(handle.read()).hexdigest()
        expect(runner_hash == RUNNER_SHA256, f"qualified runner hash mismatch: {runner_hash}")
        source = subprocess.check_output(["git", "rev-parse", "HEAD^{commit}"], cwd=REPO, text=True).strip()
        tree = subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], cwd=REPO, text=True).strip()
        dirty = subprocess.check_output(["git", "status", "--porcelain"], cwd=REPO, text=True).strip()
        expect(not dirty, f"source checkout is dirty: {dirty}")

        os.makedirs(os.path.join(ROOT, "home"), mode=0o700, exist_ok=True)
        os.makedirs(os.path.join(ROOT, "cargo-home"), mode=0o700, exist_ok=True)
        os.makedirs(os.path.join(ROOT, "markers"), mode=0o700, exist_ok=True)
        write_file(os.path.join(ROOT, "hold.py"), HOLD_SCRIPT)
        proof = Proof(ROOT)
        proof.phase("start")
        global BINARY
        BINARY = stage(proof, REPO)
        with open(BINARY, "rb") as handle:
            binary_sha256 = hashlib.sha256(handle.read()).hexdigest()
        print(f"staged oxide sha256 {binary_sha256}", flush=True)
        proof.phase("after staging")

        fences_pool = os.path.join(ROOT, "pool-fences")
        fences_env = base_env(proof, fences_pool)
        fences_app = write_tree(os.path.join(ROOT, "checkouts", "fences"), FIXTURE)
        raised_pool = os.path.join(ROOT, "pool-raised")
        project_pool = os.path.join(ROOT, "pool-project")
        project_env = base_env(proof, project_pool)
        reuse_pool = os.path.join(ROOT, "pool-reuse")
        reuse_env = base_env(proof, reuse_pool)
        byte_pool = os.path.join(ROOT, "pool-byte")
        phases = [
            ("fences", [], lambda: prove_fences_and_exhaustion(proof, fences_app, fences_pool, fences_env)),
            (
                "raised_fence",
                [],
                lambda: prove_raised_fence(
                    proof,
                    raised_pool,
                    base_env(proof, raised_pool),
                    os.path.join(ROOT, "checkouts", "raised"),
                ),
            ),
            ("pool_ceiling", ["fences"], lambda: prove_pool_ceiling(proof, fences_app, fences_pool, fences_env)),
            (
                "project_cap",
                [],
                lambda: prove_project_cap(proof, project_pool, project_env, os.path.join(ROOT, "checkouts", "project")),
            ),
            (
                "reuse",
                [],
                lambda: prove_reuse_and_no_churn(
                    proof, reuse_pool, reuse_env, os.path.join(ROOT, "checkouts", "reuse")
                ),
            ),
            (
                "byte_pressure",
                [],
                lambda: prove_byte_pressure(
                    proof, byte_pool, base_env(proof, byte_pool), os.path.join(ROOT, "checkouts", "byte")
                ),
            ),
            (
                "foreign_refusal",
                ["reuse"],
                lambda: prove_foreign_refusal(
                    proof, reuse_pool, base_env(proof, reuse_pool), os.path.join(ROOT, "checkouts", "reuse", "reuse-app")
                ),
            ),
        ]
        dispatch(phases, DIAGNOSTIC)
        if not DIAGNOSTIC:
            receipt = {
                "task": "oxide#177",
                "source_commit": source,
                "source_tree": tree,
                "binary_sha256": binary_sha256,
                "runner_sha256": runner_hash,
                "result": "pass",
                "cases": RESULTS,
                "limits": {
                    "wall_clock_seconds": 300,
                    "shutdown_margin_seconds": SHUTDOWN_MARGIN_S,
                    "scratch_budget_mib": 512,
                },
                "coverage_gaps": [
                    "the 8 MiB MAX_LEDGER_BYTES is the worst-valid byte bound (a larger ledger is corrupt and refused); the measured writer-reachable 256-slot maxima are 4,213,726 bytes all-idle and 5,625,456 bytes with one maximal journal on an Evicting record that retains its closed observation (2,763,152 bytes headroom), and the maximal serialization, 512-record control-state round-trip and oversized-field refusals are unit tests, not native fixtures",
                    "the 128-slot vs 256-slot ledger read/rank/write cost is a unit measurement with a fixed 1000 ms ceiling, not a native fixture",
                    "the thirteen-grant freshness bound is a bounded loop inspection, not separately induced natively",
                ],
            }
            write_evidence("slot-headroom.json", json.dumps(receipt, indent=2) + "\n")
            print(
                f"slot-headroom proof PASS: {len(RESULTS)} cases; source {source}; tree {tree}; runner {runner_hash}",
                flush=True,
            )
        elif any(state != "pass" for state in PHASE_STATES.values()) or ABORTED:
            exit_code = 1
    except PhaseFailure as failure:
        abort_now(str(failure))
        if not DIAGNOSTIC:
            print(f"FAILED: {failure}", flush=True)
            raise SystemExit(1)
        exit_code = 1
    except BaseException as error:  # noqa: BLE001 - re-raised outside diagnostic mode
        # An unrecordable or unexpected outcome is never an oracle: record it,
        # then let counted mode re-raise so the failure is not swallowed.
        abort_now(f"unknown outcome: {type(error).__name__}: {error}")
        if not DIAGNOSTIC:
            raise
        exit_code = 1
    finally:
        if DIAGNOSTIC and os.path.isdir(EVIDENCE):
            try:
                document = receipt_document(source, tree, runner_hash, binary_sha256)
                print(f"diagnostic receipt: {write_diagnostic_receipt(document)}", flush=True)
            except Exception as error:  # noqa: BLE001 - an unwritable receipt is never success
                print(f"diagnostic receipt failed: {error}", flush=True)
                exit_code = 1
    if DIAGNOSTIC:
        print(
            json.dumps(
                {
                    "phase_states": PHASE_STATES,
                    "aborted": ABORTED,
                    "qualified": False,
                },
                sort_keys=True,
            ),
            flush=True,
        )
        raise SystemExit(exit_code)


def terminated(signum, frame) -> None:
    close_all()
    fail(f"terminated by signal {signum}")


def overdue(signum, frame) -> None:
    close_all()
    fail("proof deadline reached")


if __name__ == "__main__":
    if DIAGNOSTIC:
        # The uncounted collection records a phase failure instead of exiting.
        fail = phase_fail
        expect = phase_expect
    signal.signal(signal.SIGTERM, terminated)
    signal.signal(signal.SIGALRM, overdue)
    try:
        main()
    finally:
        close_all()
