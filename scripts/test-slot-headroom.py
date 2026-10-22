#!/usr/bin/env python3
"""Focused slot-headroom proof for oxide#118.

The qualified shared supervisor owns the 300 s deadline, 512 MiB monitored
scratch, process closure and cleanup. This fixed fixture script reuses the
reviewed oxide#105 staging/proof helper, keeps every write under its supplied
OXIDE_TASK_SCRATCH_ROOT, touches no live pool or drive, and stops work 30
seconds before the exported absolute deadline.

Covered here: the permanent ledger/control fences and the no-migration read;
the rank-exhaustion refusal before any transition; a 128-slot pool whose 129th
admission replaces the rank-LRU idle whole slot and keeps the count; repeated
replacement; the 8-slot project cap with a specific held-slot wait, the
honest all-held refusal and one LRU idle replacement after affinity is
dropped; ordinary compatible reuse and no eviction below both ceilings;
byte-pressure LRU with a held slot excluded; and a foreign payload entry
refusal. The worst-valid 128-slot ledger and 256-record control-state
serialization/round-trip and oversized-field refusals are unit tests in
`effigy test rust`, not native fixtures.
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
labelled_slot = fm.labelled_slot

REPO = os.path.dirname(HERE)
ROOT = os.path.realpath(os.environ.get("OXIDE_TASK_SCRATCH_ROOT", ""))
EVIDENCE = os.path.realpath(os.environ.get("OXIDE_PROOF_EVIDENCE", ""))
SHUTDOWN_MARGIN_S = 30
RUNNER_SHA256 = "d89115f1ca03587e1d9134efa153cc9c94860d2904f52e2a11bc8c8fbe21f25e"
CANDIDATE = "OxSh" + secrets.token_hex(4)
RESULTS = []
CHILDREN = []
COUNTER = 0

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
        fail("work deadline reached before the supervisor shutdown margin")
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
    for name in list(env):
        if name in ("OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS", "OXIDE_TASK_SCRATCH_ROOT"):
            continue
        if name.startswith(("OXIDE_", "XDG_", "SCCACHE_", "CARGO_BUILD_", "CARGO_TARGET_")):
            env.pop(name, None)
        elif name in ("CARGO", "CARGO_HOME", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
                      "RUSTUP_TOOLCHAIN", "CARGO_NET_OFFLINE", "CARGO_TERM_COLOR"):
            env.pop(name, None)
    env.update(
        {
            "OXIDE_POOL": pool,
            "OXIDE_POOL_CAP": "64MiB",
            "OXIDE_MAX_ACTIVE": "16",
            "HOME": os.path.join(ROOT, "home"),
            "CARGO_HOME": os.path.join(ROOT, "cargo-home"),
            "CARGO_NET_OFFLINE": "true",
            "CARGO_TERM_COLOR": "never",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "",
            "RUSTUP_TOOLCHAIN": fm.BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "OXIDE_TASK_SCRATCH_ROOT": ROOT,
            "TMPDIR": ROOT,
            "TMP": ROOT,
            "TEMP": ROOT,
        }
    )
    env.update(extra)
    return env


def run(proof: Proof, args: list, label: str, cwd: str, env: dict, code: int = 0) -> tuple:
    global COUNTER
    COUNTER += 1
    result = proof.run([proof.binary, *args], cwd=cwd, env=env, timeout=120)
    stdout = result.stdout.decode(errors="replace")
    stderr = result.stderr.decode(errors="replace")
    print(f"----- {label}: exit {result.returncode} -----\n{stdout}{stderr}", flush=True)
    expect(
        result.returncode == code,
        f"{label} exited {result.returncode}, expected {code}: {stdout[-2000:]}{stderr[-3000:]}",
    )
    RESULTS.append({"case": label, "exit": result.returncode})
    return stdout, stderr


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
    program = [proof.binary, *args]
    if hold:
        program += ["--", sys.executable, os.path.join(ROOT, "hold.py"), marker, release]
    with open(log, "wb") as output:
        process = subprocess.Popen(
            program, cwd=cwd, env=env, stdout=output, stderr=subprocess.STDOUT,
            start_new_session=True,
        )
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
    for child in list(CHILDREN):
        if child.process.poll() is None:
            try:
                os.killpg(child.process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        try:
            child.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            fail(f"{child.name} was not positively closed")
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


def prove_fences_and_exhaustion(proof: Proof, app: str, pool: str, env: dict) -> None:
    stdout, stderr = run(proof, ["exec", "--", sys.executable, "-c", "pass"], "first-grant", app, env)
    ledger_path = os.path.join(pool, "ledger.json")
    ledger = read_json(ledger_path)
    expect(ledger.get("capacity_policy") == 1, "the first grant did not install the capacity fence")
    expect(ledger.get("next_rank") == 3, f"unexpected rank sequence after grant+release: {ledger.get('next_rank')}")
    expect(len(ledger["slots"]) == 1, "the first grant did not record one slot")
    slot = ledger["slots"][0]
    expect(slot["rank"] == 2 and slot["state"] == "idle", f"release did not stamp the idle rank: {slot}")
    state_path = os.path.join(pool, "oxide-managed", "state.json")
    state = read_json(state_path)
    expect(state["version"] == 2, f"the first mutation did not install control version 2: {state}")
    RESULTS.append({"case": "fences-installed", "ledger_policy": 1, "state_version": 2})

    # A legacy representation (no fence, rank 0) still reads for status and
    # is not migrated by the observer.
    legacy = dict(ledger)
    legacy.pop("capacity_policy")
    legacy.pop("next_rank")
    legacy["slots"] = [dict(slot) for slot in ledger["slots"]]
    for record in legacy["slots"]:
        record["rank"] = 0
    write_json(ledger_path, legacy)
    before = open(ledger_path, "rb").read()
    run(proof, ["status"], "legacy-status", app, env)
    expect(open(ledger_path, "rb").read() == before, "a read-only status migrated the legacy ledger")
    legacy_state = dict(state)
    legacy_state["version"] = 1
    write_json(state_path, legacy_state)
    run(proof, ["status"], "legacy-state-status", app, env)
    expect(read_json(state_path)["version"] == 1, "a read-only status migrated the legacy control state")
    run(proof, ["exec", "--", sys.executable, "-c", "pass"], "legacy-first-write", app, env)
    fenced = read_json(ledger_path)
    expect(fenced.get("capacity_policy") == 1 and read_json(state_path)["version"] == 2,
           "the first revised mutation did not install both fences")
    RESULTS.append({"case": "legacy-read-no-migration", "fenced_on_mutation": True})

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


def prove_pool_ceiling(proof: Proof, app: str, pool: str, env: dict) -> None:
    ledger_path = os.path.join(pool, "ledger.json")
    ledger = read_json(ledger_path)
    real = ledger["slots"][0]
    folder = real["dir"].split("/", 1)[0]
    # A distinct project path keeps the synthetic idle slots incompatible
    # with this checkout's record; they exist only as pool members.
    module = real["project"] + "-seed"
    workspace = real["workspace"]
    store_id = ledger["store_id"]
    slots_root = os.path.join(pool, "payload", "slots", folder)
    os.makedirs(slots_root, mode=0o700, exist_ok=True)
    os.chmod(slots_root, 0o700)
    rank = max(record["rank"] for record in ledger["slots"]) + 1
    for slot_id in range(2, 129):
        record = {
            "id": slot_id,
            "incarnation": 1,
            "state": "idle",
            "project": module,
            "workspace": workspace,
            "profile": "dev",
            "holder": "seed",
            "lease_id": None,
            "last_lease_id": f"v1.{store_id}.{slot_id}.1.0",
            "dir": f"{folder}/quick-{slot_id}",
            "rank": rank,
        }
        ledger["slots"].append(record)
        rank += 1
        build = os.path.join(slots_root, f"quick-{slot_id}", "build")
        os.makedirs(build, mode=0o700, exist_ok=True)
        for path in (os.path.join(slots_root, f"quick-{slot_id}"), build):
            os.chmod(path, 0o700)
    ledger["next_slot"] = 129
    ledger["next_lease"] = 1000
    ledger["next_rank"] = rank
    write_json(ledger_path, ledger)
    run(proof, ["status"], "synthetic-128-status", app, env)
    expect(len(read_json(ledger_path)["slots"]) == 128, "the 128-slot fixture did not read back")
    RESULTS.append({"case": "synthetic-128-ledger", "slots": 128})

    # The 129th admission replaces the rank-LRU idle whole slot: slot 1 has
    # rank 2, the oldest. A debug-profile command has no compatible idle slot.
    victim = slot_dir(pool, "1")
    debug_marker = os.path.join(ROOT, "debug-executed")
    try:
        os.remove(debug_marker)
    except FileNotFoundError:
        pass
    _, stderr = run(
        proof,
        ["exec", "--profile", "debug", "--", sys.executable, "-c", "open(%r,'w').close()" % debug_marker],
        "pool-ceiling-129th",
        app,
        env,
    )
    expect(os.path.exists(debug_marker), "the 129th command did not run")
    expect(not os.path.lexists(victim), f"the rank-LRU idle slot {victim} was not evicted whole")
    after = read_json(ledger_path)
    expect(len(after["slots"]) == 128, f"pool count {len(after['slots'])} is not 128")
    expect(all(record["id"] != 1 for record in after["slots"]), "the evicted record remains")
    expect(any(record["id"] == 129 for record in after["slots"]), "the 129th slot was not allocated")
    RESULTS.append({"case": "pool-ceiling-129th-replaces-rank-lru", "slots": 128, "victim": 1})

    # A further quick admission replaces the next oldest idle slot; the count
    # stays at 128 and no old 32-slot churn occurred.
    victim = slot_dir(pool, "2")
    run(proof, ["exec", "--", sys.executable, "-c", "pass"], "pool-ceiling-130th", app, env)
    expect(not os.path.lexists(victim), f"the next rank-LRU idle slot {victim} was not evicted")
    after = read_json(ledger_path)
    expect(len(after["slots"]) == 128, f"pool count {len(after['slots'])} is not 128 after replacement")
    expect(any(record["id"] == 130 for record in after["slots"]), "the 130th slot was not allocated")
    RESULTS.append({"case": "pool-ceiling-repeats-without-churn", "slots": 128, "victim": 2})


def prove_project_cap(proof: Proof, pool: str, env: dict, workdir: str) -> None:
    app = write_tree(os.path.join(workdir, "project-app"), FIXTURE)
    holds = []
    for index in range(8):
        holds.append(start(proof, ["exec"], f"project-hold-{index}", app, env, hold=True))
    for child in holds:
        wait_started(child)
    ledger = read_json(os.path.join(pool, "ledger.json"))
    expect(len(ledger["slots"]) == 8, f"eight concurrent commands did not keep eight slots: {len(ledger['slots'])}")
    expect(all(record["state"] == "held" for record in ledger["slots"]), "a concurrent project slot is not held")
    RESULTS.append({"case": "project-cap-holds-eight", "slots": 8})

    waiter = start(proof, ["exec"], "project-waiter", app, env, hold=True)
    wait_log(waiter, "waiting for another oxide command in this checkout")
    expect(not os.path.exists(waiter.marker), "the ninth command started while all eight slots were held")
    RESULTS.append({"case": "project-cap-waits-for-specific-held-slot", "waiting": True})
    release(holds[0])
    wait_started(waiter)
    expect(os.path.lexists(slot_dir(pool, "1")), "the released slot is missing")
    release(waiter)
    for child in holds[1:]:
        release(child)
    ledger = read_json(os.path.join(pool, "ledger.json"))
    expect(len(ledger["slots"]) == 8, f"release did not leave eight idle slots: {len(ledger['slots'])}")
    expect(all(record["state"] == "idle" for record in ledger["slots"]), "a released project slot stayed held")

    # Drop this checkout's affinity, then a new command has no slot of its own
    # and the project cap replaces the rank-LRU idle whole slot.
    idle = [record for record in ledger["slots"] if record["state"] == "idle"]
    victim_record = min(idle, key=lambda record: (record["rank"], record["id"]))
    victim = slot_dir(pool, str(victim_record["id"]))
    run(proof, ["clean"], "project-cap-clean", app, env)
    _, stderr = run(proof, ["exec", "--", sys.executable, "-c", "pass"], "project-cap-9th", app, env)
    expect(f"evicted idle slot {victim_record['id']} (project slot cap 8)" in stderr,
           f"the project cap did not evict the rank-LRU idle slot: {stderr}")
    expect(not os.path.lexists(victim), f"the rank-LRU project slot {victim} was not evicted")
    after = read_json(os.path.join(pool, "ledger.json"))
    expect(len(after["slots"]) == 8, f"project cap count {len(after['slots'])} is not eight")
    RESULTS.append({"case": "project-cap-9th-replaces-rank-lru", "slots": 8, "victim": victim_record["id"]})


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
    run(proof, ["exec", "--", sys.executable, "-c", "pass"], "byte-first", first, limited)
    aged = slot_dir(pool, "1")
    with open(os.path.join(aged, "build", "blob"), "wb") as handle:
        handle.write(b"a" * (2 * MIB))
    run(proof, ["exec", "--profile", "debug", "--", sys.executable, "-c", "pass"], "byte-second", first, limited)
    newer = slot_dir(pool, "2")
    with open(os.path.join(newer, "build", "blob"), "wb") as handle:
        handle.write(b"b" * (1 * MIB))
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
    _, stderr = run(proof, ["status"], "foreign-entry-status", app, env, code=1)
    expect("unexpected entry" in stderr, f"the foreign entry was not refused by name: {stderr}")
    os.remove(foreign)
    RESULTS.append({"case": "foreign-payload-entry-refused", "exit": 1})


def main() -> None:
    expect(os.path.isdir(ROOT) and os.path.realpath(ROOT) == os.environ["OXIDE_TASK_SCRATCH_ROOT"],
           "scratch root is not canonical")
    expect(os.path.isdir(EVIDENCE), "qualified-runner evidence directory is missing")
    with open(os.path.join(REPO, "scripts", "bounded-test.py"), "rb") as handle:
        import hashlib
        runner_hash = hashlib.sha256(handle.read()).hexdigest()
    expect(runner_hash == RUNNER_SHA256, f"qualified runner hash mismatch: {runner_hash}")
    work_left()
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
    proof.binary = fm.stage(proof, REPO)
    import hashlib
    with open(proof.binary, "rb") as handle:
        binary_sha256 = hashlib.sha256(handle.read()).hexdigest()
    print(f"staged oxide sha256 {binary_sha256}", flush=True)
    proof.phase("after staging")

    fences_pool = os.path.join(ROOT, "pool-fences")
    fences_env = base_env(proof, fences_pool)
    fences_app = write_tree(os.path.join(ROOT, "checkouts", "fences"), FIXTURE)
    prove_fences_and_exhaustion(proof, fences_app, fences_pool, fences_env)
    proof.phase("after fences")
    prove_pool_ceiling(proof, fences_app, fences_pool, fences_env)
    proof.phase("after pool ceiling")

    project_pool = os.path.join(ROOT, "pool-project")
    project_env = base_env(proof, project_pool)
    prove_project_cap(proof, project_pool, project_env, os.path.join(ROOT, "checkouts", "project"))
    proof.phase("after project cap")

    reuse_pool = os.path.join(ROOT, "pool-reuse")
    prove_reuse_and_no_churn(proof, reuse_pool, base_env(proof, reuse_pool), os.path.join(ROOT, "checkouts", "reuse"))
    proof.phase("after reuse")

    byte_pool = os.path.join(ROOT, "pool-byte")
    prove_byte_pressure(proof, byte_pool, base_env(proof, byte_pool), os.path.join(ROOT, "checkouts", "byte"))
    proof.phase("after byte pressure")

    prove_foreign_refusal(proof, reuse_pool, base_env(proof, reuse_pool), os.path.join(ROOT, "checkouts", "reuse", "reuse-app"))
    proof.phase("after foreign refusal")

    receipt = {
        "task": "oxide#118",
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
            "worst-valid 128-slot ledger and 256-record control-state serialization/round-trip and oversized-field refusal are unit tests, not native fixtures",
            "the nine-grant freshness bound is a bounded loop inspection, not separately induced natively",
        ],
    }
    write_evidence("slot-headroom.json", json.dumps(receipt, indent=2) + "\n")
    print(f"slot-headroom proof PASS: {len(RESULTS)} cases; source {source}; tree {tree}; runner {runner_hash}", flush=True)


def terminated(signum, frame) -> None:
    close_all()
    fail(f"terminated by signal {signum}")


def overdue(signum, frame) -> None:
    close_all()
    fail("proof deadline reached")


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, terminated)
    signal.signal(signal.SIGALRM, overdue)
    try:
        main()
    finally:
        close_all()
