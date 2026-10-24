#!/usr/bin/env python3
"""Focused native proof for enumerated paths that vanish during accounting.

One fixed debug build and tiny Cargo fixtures run inside the qualified
supervisor's cooperative session and scratch root. No child gets its own
session or deadline; the supervisor owns closure, reserve sampling and scratch
cleanup.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import time
from typing import Optional

sys.dont_write_bytecode = True

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)
import offline_seed

BUILDER = "1.97.1"
EXPECTED_BRANCH = "ns-ff568299-4433-4c44-bbcb-d046ef2d9d0c"
EXPECTED_RUNNER_SHA256 = "8e379c93416c6e06b7f06bec7f86dba4fca4c8900b1cc87f1eb6303d39897a9d"
NS = 1_000_000_000
MIB = 1024 * 1024
POOL_CAP = 64 * MIB
HELD_TARGET = 40 * MIB
IDLE_TARGET = 28 * MIB
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RAW_CLOCK = getattr(time, "CLOCK_UPTIME_RAW")


def fail(message: str) -> None:
    raise RuntimeError(message)


def now_ns() -> int:
    return time.clock_gettime_ns(RAW_CLOCK)


def remaining() -> float:
    # Leave the qualified runner its 20 s closure reserve and 2 s exit margin.
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"]) - 22 * NS
    left = (deadline - now_ns()) / NS
    if left <= 2:
        fail("the proof has reached its absolute work cutoff")
    return left


def sha256(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def git(*args: str) -> str:
    remaining()
    result = subprocess.run(
        ["git", *args], cwd=REPO, check=True, capture_output=True, text=True
    )
    return result.stdout.strip()


def unique_file_bytes(root: str, excluded: Optional[set[str]] = None) -> int:
    excluded = excluded or set()
    seen: set[tuple[int, int]] = set()
    total = 0
    for current, dirs, files in os.walk(root, followlinks=False):
        dirs[:] = [name for name in dirs if not os.path.islink(os.path.join(current, name))]
        for name in files:
            path = os.path.join(current, name)
            if path in excluded:
                continue
            item = os.lstat(path)
            if not stat.S_ISREG(item.st_mode):
                continue
            key = (item.st_dev, item.st_ino)
            if key not in seen:
                seen.add(key)
                total += item.st_blocks * 512
    return total


def fill_slot_to(slot: str, path: str, target_bytes: int) -> int:
    current = unique_file_bytes(slot)
    if current >= target_bytes:
        fail(f"fixture slot already meets target {target_bytes}: {current}")
    remaining_bytes = target_bytes - current
    with open(path, "wb") as output:
        while remaining_bytes:
            block = os.urandom(min(1 * MIB, remaining_bytes))
            output.write(block)
            remaining_bytes -= len(block)
    observed = unique_file_bytes(slot)
    if observed < target_bytes:
        fail(f"fixture allocation stayed below target: {observed} < {target_bytes}")
    return observed


def fixture(root: str, name: str, hold: bool = False) -> str:
    path = os.path.join(root, name)
    os.makedirs(os.path.join(path, "src"), exist_ok=True)
    with open(os.path.join(path, "Cargo.toml"), "w") as output:
        output.write(
            f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n'
        )
    with open(os.path.join(path, "rust-toolchain.toml"), "w") as output:
        output.write(f'[toolchain]\nchannel = "{BUILDER}"\n')
    with open(os.path.join(path, "src", "main.rs"), "w") as output:
        output.write(f'fn main() {{ println!("{name}"); }}\n')
    if hold:
        tests = os.path.join(path, "tests")
        os.makedirs(tests, exist_ok=True)
        with open(os.path.join(tests, "hold.rs"), "w") as output:
            output.write(
                "use std::{path::PathBuf, thread, time::Duration};\n"
                "#[test]\nfn held_writer() {\n"
                " let ready = PathBuf::from(std::env::var_os(\"OXIDE_FIXTURE_READY\").unwrap());\n"
                " let release = PathBuf::from(std::env::var_os(\"OXIDE_FIXTURE_RELEASE\").unwrap());\n"
                " std::fs::write(ready, b\"ready\").unwrap();\n"
                " while !release.exists() { thread::sleep(Duration::from_millis(25)); }\n"
                "}\n"
            )
    return path


def base_env(scratch: str, pool: str, cap: str = "8MiB") -> dict[str, str]:
    env = os.environ.copy()
    for name in (
        "OXIDE_CONFIG",
        "OXIDE_ALLOW_RUSTC_WRAPPER",
        "OXIDE_TEST_WALK_LOG",
        "OXIDE_TEST_WALK_MUTATIONS",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_BUILD_DIR",
        "RUSTC",
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
    ):
        env.pop(name, None)
    env.update(
        {
            "HOME": os.path.join(scratch, "home"),
            "OXIDE_TEST_HOME_FROM_ENV": "1",
            "XDG_CONFIG_HOME": os.path.join(scratch, "xdg-config"),
            "CARGO_HOME": os.path.join(scratch, "cargo-home"),
            "RUSTUP_HOME": os.environ.get(
                "RUSTUP_HOME", os.path.expanduser("~/.rustup")
            ),
            "OXIDE_POOL": pool,
            "OXIDE_POOL_CAP": cap,
            "CARGO_NET_OFFLINE": "true",
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTC_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "",
            "OXIDE_TASK_SCRATCH_ROOT": scratch,
        }
    )
    return env


def run_oxide(
    binary: str,
    env: dict[str, str],
    cwd: str,
    args: list[str],
    mutations: Optional[list[dict[str, str]]] = None,
) -> tuple[int, str, str]:
    remaining()
    child_env = env.copy()
    if mutations:
        child_env["OXIDE_TEST_WALK_MUTATIONS"] = json.dumps(mutations)
    else:
        child_env.pop("OXIDE_TEST_WALK_MUTATIONS", None)
    result = subprocess.run(
        [binary, *args], cwd=cwd, env=child_env, capture_output=True, text=True
    )
    return result.returncode, result.stdout, result.stderr


def slot_roots(pool: str) -> dict[str, str]:
    payload = os.path.join(pool, "payload")
    slots = os.path.join(payload, "slots")
    if os.path.islink(payload) or os.path.islink(slots):
        fail("fixture payload or slots root is a symlink")
    payload_root = os.path.realpath(payload)
    slots_root = os.path.realpath(slots)
    if os.path.commonpath((payload_root, slots_root)) != payload_root or slots_root == payload_root:
        fail("fixture slots root escapes the payload")

    with open(os.path.join(pool, "ledger.json"), encoding="utf-8") as ledger_file:
        ledger = json.load(ledger_file)
    records = ledger.get("slots")
    if not isinstance(records, list):
        fail("fixture ledger has no slot records")

    result: dict[str, str] = {}
    for record in records:
        slot_id = record.get("id") if isinstance(record, dict) else None
        if isinstance(slot_id, bool) or not isinstance(slot_id, int) or slot_id <= 0:
            fail(f"fixture ledger has an invalid slot id: {slot_id!r}")
        key = str(slot_id)
        if key in result:
            fail(f"fixture ledger repeats slot id {key}")

        recorded_dir = record.get("dir")
        if recorded_dir is None:
            parts = [key]
        elif isinstance(recorded_dir, str):
            parts = recorded_dir.split("/")
            suffix = f"-{key}"
            valid_name = lambda part: (
                0 < len(part) <= 80
                and not part.startswith(".")
                and all(char in "abcdefghijklmnopqrstuvwxyz0123456789-_." for char in part)
            )
            if (
                len(parts) != 2
                or not all(valid_name(part) for part in parts)
                or not parts[1].endswith(suffix)
                or not parts[1][: -len(suffix)]
            ):
                fail(f"fixture ledger has an invalid directory for slot {key}: {recorded_dir!r}")
        else:
            fail(f"fixture ledger has a non-string directory for slot {key}")

        path = slots_root
        for part in parts:
            path = os.path.join(path, part)
            if os.path.islink(path) or not os.path.isdir(path):
                fail(f"recorded fixture slot path is missing or unsafe for slot {key}")
        canonical = os.path.realpath(path)
        if os.path.commonpath((slots_root, canonical)) != slots_root or canonical == slots_root:
            fail(f"recorded fixture slot path escapes slots for slot {key}")
        result[key] = canonical
    return result


def wait_for_ready(process: subprocess.Popen, marker: str) -> None:
    while not os.path.exists(marker):
        remaining()
        if process.poll() is not None:
            fail(f"held fixture exited before its ready marker (exit {process.returncode})")
        time.sleep(0.05)


def start_held(binary: str, env: dict[str, str], project: str, evidence: str, name: str):
    remaining()
    ready = os.path.join(project, "hold.ready")
    release = os.path.join(project, "hold.release")
    if os.path.lexists(ready) or os.path.lexists(release):
        fail(f"held fixture markers already exist for {name}")
    child_env = env.copy()
    child_env["OXIDE_FIXTURE_READY"] = ready
    child_env["OXIDE_FIXTURE_RELEASE"] = release
    log = open(os.path.join(evidence, f"{name}.log"), "wb")
    process = subprocess.Popen(
        [binary, "test"], cwd=project, env=child_env, stdout=log, stderr=subprocess.STDOUT
    )
    wait_for_ready(process, ready)
    return process, release, log


def release_held(process, release: str, log) -> None:
    # This observes the fixture command's exit only; the supervisor still
    # proves session closure after this script exits.
    with open(release, "wb") as output:
        output.write(b"release")
    while process.poll() is None:
        remaining()
        time.sleep(0.05)
    code = process.returncode
    log.close()
    if code != 0:
        fail(f"held fixture exited {code} after release")


def stage_oxide(scratch: str) -> str:
    remaining()
    target = os.path.join(scratch, "stage-target")
    cargo_home = os.path.join(scratch, "cargo-home")
    seed_private_cargo_home(cargo_home)
    env = os.environ.copy()
    for name in (
        "OXIDE_CONFIG",
        "OXIDE_ALLOW_RUSTC_WRAPPER",
        "OXIDE_TEST_WALK_LOG",
        "OXIDE_TEST_WALK_MUTATIONS",
        "RUSTC",
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
    ):
        env.pop(name, None)
    env["CARGO_TARGET_DIR"] = target
    env["CARGO_BUILD_BUILD_DIR"] = target
    env["CARGO_HOME"] = cargo_home
    env["HOME"] = os.path.join(scratch, "home")
    env["OXIDE_TEST_HOME_FROM_ENV"] = "1"
    env["XDG_CONFIG_HOME"] = os.path.join(scratch, "xdg-config")
    env["RUSTUP_HOME"] = os.environ.get(
        "RUSTUP_HOME", os.path.expanduser("~/.rustup")
    )
    env["CARGO_NET_OFFLINE"] = "true"
    env["RUSTUP_TOOLCHAIN"] = BUILDER
    env["RUSTC_WRAPPER"] = ""
    env["CARGO_BUILD_RUSTC_WRAPPER"] = ""
    build = subprocess.run(
        ["cargo", "build", "--locked", "--offline", "--bin", "oxide"],
        cwd=REPO,
        env=env,
        capture_output=True,
        text=True,
    )
    if build.returncode:
        fail(f"offline staging failed: {build.stderr[-3000:]}")
    source = os.path.join(target, "debug", "oxide")
    binary = os.path.join(scratch, "oxide")
    shutil.copy2(source, binary)
    with open(binary, "rb") as staged:
        contents = staged.read()
    if b"OXIDE_TEST_WALK_MUTATIONS" not in contents:
        fail("staged debug oxide does not contain the scratch-only walk fixture hook")
    return binary


def seed_private_cargo_home(destination: str) -> None:
    """Seed the private offline CARGO_HOME from this checkout's Cargo.lock.

    The shared bounded seeder (`scripts/offline_seed.py`, oxide#131) copies
    only the lock's registry and pinned Git inputs, checks every archive
    against its lock checksum and copies no host `registry/src` tree. Its
    source hash is in this proof's receipt: importing it transfers no
    qualification.
    """
    lock_bytes = open(os.path.join(REPO, "Cargo.lock"), "rb").read()
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"]) - 30 * NS
    try:
        seed = offline_seed.seed_cargo_home(
            os.environ.get("CARGO_HOME") or os.path.expanduser("~/.cargo"),
            destination,
            lock_bytes,
            deadline_ns=deadline,
        )
    except offline_seed.SeedRefusal as error:
        fail(f"bounded offline seed refused: {error}")
    print(
        f"private offline Cargo seed: {seed['packages']} registry packages, "
        f"{len(seed['git']['candidates'])} Git candidates, "
        f"{seed['aggregate_bytes']} bytes, {seed['entries']} entries"
    )


def prepare_pool(
    binary: str,
    root: str,
    pool: str,
    held_name: str,
    idle_name: str,
    evidence: str,
    cap: str = "64MiB",
):
    if os.path.lexists(pool):
        fail(f"private fixture pool unexpectedly exists: {pool}")
    held_project = fixture(root, held_name, hold=True)
    idle_project = fixture(root, idle_name)
    env = base_env(root, pool, cap)
    held_process, release, log = start_held(binary, env, held_project, evidence, held_name)
    held_slots = slot_roots(pool)
    if len(held_slots) != 1:
        fail(f"expected one held slot after first build, found {list(held_slots)}")
    held_id = next(iter(held_slots))
    code, out, err = run_oxide(binary, env, idle_project, ["check"])
    if code != 0:
        fail(f"idle fixture build failed ({code}): {err[-1500:]}")
    slots = slot_roots(pool)
    idle_ids = [slot_id for slot_id in slots if slot_id != held_id]
    if len(idle_ids) != 1:
        fail(f"expected one separate idle slot, found {list(slots)}")
    return {
        "env": env,
        "held_process": held_process,
        "held_release": release,
        "held_log": log,
        "held_project": held_project,
        "held_id": held_id,
        "held_slot": slots[held_id],
        "idle_project": idle_project,
        "idle_id": idle_ids[0],
        "idle_slot": slots[idle_ids[0]],
    }


def prove_complete_vanish_and_eviction(binary: str, root: str, evidence: str) -> str:
    pool = os.path.join(root, "pool-eviction")
    state = prepare_pool(binary, root, pool, "held-vanish", "idle-vanish", evidence)
    env = state["env"]
    build = os.path.join(state["held_slot"], "build")

    diag_dir = os.path.join(build, "diagnostic-dir")
    diag_file = os.path.join(build, "diagnostic-file")
    stable_path = os.path.join(build, "diagnostic-stable")
    os.makedirs(diag_dir)
    with open(stable_path, "wb") as output:
        output.write(os.urandom(128 * 1024))
    stable_allocation = os.lstat(stable_path).st_blocks * 512
    os.link(stable_path, os.path.join(diag_dir, "hardlink-alias"))
    with open(os.path.join(diag_dir, "payload"), "wb") as output:
        output.write(b"temporary")
    with open(diag_file, "wb") as output:
        output.write(b"temporary")
    code, out, err = run_oxide(
        binary,
        env,
        state["held_project"],
        ["status"],
        [
            {"path": diag_dir, "action": "remove_before_read_dir"},
            {"path": diag_file, "action": "remove_before_symlink_metadata"},
        ],
    )
    if (
        code != 0
        or "slot accounting walk: complete, vanished x2" not in out
        or "allocation reclamation not inferred" not in out
    ):
        fail(f"status missed complete vanished diagnostics ({code}): {out[-2500:]} {err[-1000:]}")
    if os.path.exists(diag_dir) or os.path.exists(diag_file):
        fail("the status fixture did not remove both enumerated paths")
    if not os.path.exists(stable_path) or os.lstat(stable_path).st_blocks * 512 != stable_allocation:
        fail("status changed the allocation of the surviving known inode")
    print("vanished status: complete, vanished x2; no coverage gap")
    print(f"surviving known inode allocation unchanged: {stable_allocation} bytes")

    stable = os.path.join(build, "stable-occupancy")
    idle_file = os.path.join(state["idle_slot"], "build", "idle-occupancy")
    admit_file = os.path.join(build, "admit-file")
    admit_dir = os.path.join(build, "admit-dir")
    os.makedirs(admit_dir)
    held_base = fill_slot_to(state["held_slot"], stable, HELD_TARGET)
    idle_bytes = fill_slot_to(state["idle_slot"], idle_file, IDLE_TARGET)
    for path, size in ((admit_file, 256 * 1024), (os.path.join(admit_dir, "payload"), 128 * 1024)):
        with open(path, "wb") as output:
            output.write(os.urandom(size))
    held_base = unique_file_bytes(
        state["held_slot"], {admit_file, os.path.join(admit_dir, "payload")}
    )
    if held_base >= POOL_CAP or held_base + idle_bytes <= POOL_CAP:
        fail(
            f"fixture is not non-vacuous: held={held_base}, idle={idle_bytes}, cap={POOL_CAP}"
        )
    before_ids = set(slot_roots(pool))
    next_project = fixture(root, "new-after-vanish")
    code, out, err = run_oxide(
        binary,
        env,
        next_project,
        ["check"],
        [
            {"path": admit_file, "action": "remove_before_symlink_metadata"},
            {"path": admit_dir, "action": "remove_before_read_dir"},
        ],
    )
    if code != 0:
        fail(f"complete vanished observation did not admit: {err[-2000:]}")
    after = slot_roots(pool)
    after_ids = set(after)
    removed_ids = before_ids - after_ids
    new_ids = after_ids - before_ids
    if removed_ids != {state["idle_id"]} or len(new_ids) != 1:
        fail(f"admission changed unexpected slot membership: before={before_ids}, after={after_ids}")
    if state["idle_id"] in after_ids or state["held_id"] not in after_ids:
        fail(f"expected whole idle slot eviction only: {after_ids}")
    if not os.path.exists(stable) or os.path.exists(admit_file) or os.path.exists(admit_dir):
        fail("the held slot's stable contents changed or vanished paths remain")
    held_after = unique_file_bytes(state["held_slot"])
    if held_after != held_base:
        fail(f"held-slot observed allocation changed after vanished paths: {held_base} -> {held_after}")
    new_id = next(iter(new_ids))
    print(
        f"eviction: held {state['held_id']} retained ({held_base} bytes), "
        f"idle {state['idle_id']} evicted whole ({idle_bytes} bytes), "
        f"new slot {new_id}, cap {POOL_CAP} bytes; held allocation unchanged at {held_after} bytes"
    )
    release_held(state["held_process"], state["held_release"], state["held_log"])
    return new_id


def prove_gap_refuses_before_deletion(binary: str, root: str, evidence: str) -> None:
    pool = os.path.join(root, "pool-gap")
    state = prepare_pool(binary, root, pool, "held-gap", "idle-gap", evidence)
    env = state["env"]
    build = os.path.join(state["held_slot"], "build")
    stable = os.path.join(build, "stable-occupancy")
    idle_file = os.path.join(state["idle_slot"], "build", "idle-occupancy")
    blocked_dir = os.path.join(build, "blocked-dir")
    vanished_file = os.path.join(build, "vanished-file")
    os.makedirs(blocked_dir)
    held_base = fill_slot_to(state["held_slot"], stable, HELD_TARGET)
    idle_bytes = fill_slot_to(state["idle_slot"], idle_file, IDLE_TARGET)
    with open(os.path.join(blocked_dir, "payload"), "wb") as output:
        output.write(b"unreadable")
    with open(vanished_file, "wb") as output:
        output.write(b"vanishes")
    held_base = unique_file_bytes(
        state["held_slot"], {os.path.join(blocked_dir, "payload"), vanished_file}
    )
    if held_base >= POOL_CAP or held_base + idle_bytes <= POOL_CAP:
        fail(f"gap fixture is not non-vacuous: held={held_base}, idle={idle_bytes}, cap={POOL_CAP}")
    refused_project = fixture(root, "new-after-gap")
    code, out, err = run_oxide(
        binary,
        env,
        refused_project,
        ["check"],
        [
            {"path": blocked_dir, "action": "permission_denied_before_read_dir"},
            {"path": vanished_file, "action": "remove_before_symlink_metadata"},
        ],
    )
    detail = err + out
    if (
        code == 0
        or "owned accounting is incomplete" not in detail
        or "unreadable_entry x1" not in detail
        or "vanished x1" not in detail
        or "allocation reclamation not inferred" not in detail
    ):
        fail(f"true gap did not produce the expected accounting refusal: {code} {detail[-2000:]}")
    after = slot_roots(pool)
    expected_slots = {state["idle_id"], state["held_id"]}
    if set(after) != expected_slots:
        fail(f"true gap changed slots before refusal: expected={expected_slots}, actual={set(after)}")
    if not os.path.exists(stable) or not os.path.exists(blocked_dir) or os.path.exists(vanished_file):
        fail("true-gap refusal changed held contents")
    print(
        f"true gap: unreadable_entry x1 + vanished x1 refused before deletion; "
        f"idle {state['idle_id']} and held {state['held_id']} retained"
    )
    release_held(state["held_process"], state["held_release"], state["held_log"])


def prove_root_missing_after_open(binary: str, root: str, pool: str, slot_id: str) -> None:
    payload_root = os.path.join(pool, "payload")
    if not os.path.isdir(payload_root):
        fail(f"payload accounting root is missing before its walk: {payload_root}")
    env = base_env(root, pool, "64MiB")
    project = fixture(root, "new-after-root-missing")
    code, out, err = run_oxide(
        binary,
        env,
        project,
        ["status"],
        [{"path": payload_root, "action": "remove_after_root_open"}],
    )
    if code != 0 or "root_missing x1" not in out:
        fail(f"opened-then-removed root was not reported root_missing: {code} {out[-2500:]} {err[-1000:]}")
    if os.path.exists(payload_root):
        fail("root-open hook did not remove the scratch payload root")
    print(f"root disappearance: payload for slot {slot_id} reported root_missing after open")


def main() -> None:
    started = now_ns()
    if sys.platform != "darwin":
        fail("this proof requires the qualified macOS session supervisor")
    scratch = os.path.realpath(os.environ["OXIDE_TASK_SCRATCH_ROOT"])
    if not os.path.isdir(scratch):
        fail("OXIDE_TASK_SCRATCH_ROOT is not a directory")
    if not scratch.startswith("/private/tmp/"):
        fail("qualified proof scratch must be a child of /private/tmp")
    evidence = os.path.realpath(os.environ.get("OXIDE_PROOF_EVIDENCE", ""))
    evidence_is_nested = evidence.startswith(scratch + os.sep)
    evidence_is_supervisor_sibling = (
        os.path.basename(scratch) == "scratch"
        and os.path.basename(evidence) == "evidence"
        and os.path.dirname(scratch) == os.path.dirname(evidence)
    )
    if (
        not evidence
        or not os.path.isdir(evidence)
        or not (evidence_is_nested or evidence_is_supervisor_sibling)
    ):
        fail("the qualified supervisor did not provide OXIDE_PROOF_EVIDENCE")
    if not os.environ.get("OXIDE_PROOF_DEADLINE_NS"):
        fail("the qualified supervisor did not provide its absolute deadline")
    if any(os.path.realpath(os.environ[name]) != scratch for name in ("TMPDIR", "TMP", "TEMP")):
        fail("the four temporary-directory variables do not share supervisor scratch")
    if git("branch", "--show-current") != EXPECTED_BRANCH:
        fail("proof did not run on the immutable queue branch")
    if git("status", "--porcelain"):
        fail("proof requires the exact published source and a clean worktree")
    runner = os.path.join(REPO, "scripts", "bounded-test.py")
    runner_hash = sha256(runner)
    if runner_hash != EXPECTED_RUNNER_SHA256:
        fail(f"qualified runner hash mismatch: {runner_hash}")
    free = os.statvfs("/private/tmp").f_bavail * os.statvfs("/private/tmp").f_frsize
    print(f"accounting-vanished reserve start: {free // MIB} MiB available")

    os.makedirs(os.path.join(scratch, "home"), exist_ok=True)
    os.makedirs(os.path.join(scratch, "cargo-home"), exist_ok=True)
    print(f"accounting-vanished evidence: {evidence}")
    source_head = git("rev-parse", "HEAD")
    remote_head = git("rev-parse", f"refs/remotes/origin/{EXPECTED_BRANCH}")
    if remote_head != source_head:
        fail(f"published queue branch differs from the proof source: {remote_head} != {source_head}")
    source_tree = git("rev-parse", "HEAD^{tree}")
    source_paths = (
        "src/inventory.rs",
        "src/lease/mod.rs",
        "src/lease/pool.rs",
        "src/bin/oxide/main.rs",
        "scripts/test-accounting-vanished.py",
        "scripts/offline_seed.py",
    )
    print(
        "accounting-vanished source: "
        + json.dumps(
            {
                "head": source_head,
                "published_head": remote_head,
                "tree": source_tree,
                "files": {path: sha256(os.path.join(REPO, path)) for path in source_paths},
                "runner_sha256": runner_hash,
            },
            sort_keys=True,
        )
    )
    binary = stage_oxide(scratch)
    print(f"accounting-vanished artifact: {binary}, sha256 {sha256(binary)}")

    pool = os.path.join(scratch, "pool-eviction")
    new_id = prove_complete_vanish_and_eviction(binary, scratch, evidence)
    prove_gap_refuses_before_deletion(binary, scratch, evidence)
    prove_root_missing_after_open(binary, scratch, pool, new_id)
    free_end = os.statvfs("/private/tmp").f_bavail * os.statvfs("/private/tmp").f_frsize
    print(f"accounting-vanished reserve end: {free_end // MIB} MiB available")
    print(f"accounting-vanished work elapsed: {(now_ns() - started) / NS:.1f} s")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"accounting-vanished proof failed: {error}", file=sys.stderr)
        raise
