#!/usr/bin/env python3
"""Focused storage-layout proof (oxide#103).

Stages this clean HEAD's `oxide`, whose config, cache and state roots now
come from longhorn-storage-layout v0.4.1, and shows `oxide status` resolves
the same paths as before on a private macOS HOME with spaces and Unicode,
ignores XDG variables on macOS, needs no HOME when OXIDE_CONFIG and
OXIDE_POOL are explicit, refuses without HOME otherwise, routes the legacy
pool, its removable registration and the known-pools record through the
cache and state roots, and creates nothing. It also records the resolved
normal dependency tree and requires exactly one sha2.

The qualified shared supervisor owns the deadline, host reserve, scratch
budget, process closure and cleanup. Every write stays under
OXIDE_TASK_SCRATCH_ROOT; work stops 30 seconds before the exported deadline.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time

sys.dont_write_bytecode = True

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)
import offline_seed  # scripts/offline_seed.py, oxide#131; its hash is in the receipt

BUILDER = "1.97.1"
SHUTDOWN_MARGIN_S = 30
RUNNER_SHA256 = "8e379c93416c6e06b7f06bec7f86dba4fca4c8900b1cc87f1eb6303d39897a9d"
LONGHORN_COMMIT = "5caf1e2c763588d6b7b73bbf9c24cc2d83cffe41"
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ROOT = os.path.realpath(os.environ["OXIDE_TASK_SCRATCH_ROOT"])
EVIDENCE = os.path.realpath(os.environ["OXIDE_PROOF_EVIDENCE"])
DEADLINE_NS = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
LOGS = os.path.join(ROOT, "logs")
COUNTER = 0
RESULTS = []


def fail(message: str) -> None:
    raise RuntimeError(message)


def expect(value: bool, message: str) -> None:
    if not value:
        fail(message)


def work_left() -> float:
    """Remaining work before the supervisor's stop reserve."""
    if not hasattr(time, "CLOCK_UPTIME_RAW"):
        fail("CLOCK_UPTIME_RAW is unavailable")
    left = (DEADLINE_NS - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9
    left -= SHUTDOWN_MARGIN_S
    if left <= 0:
        fail("work deadline reached before the supervisor shutdown margin")
    return left


def inside_root(path: str) -> bool:
    return os.path.realpath(path).startswith(ROOT + os.sep)


def inside_root_directory(path: str) -> bool:
    return path == ROOT or inside_root(path)


def make_dir(path: str) -> None:
    expect(inside_root_directory(path), f"write path is outside supervisor scratch: {path}")
    os.makedirs(path, mode=0o700, exist_ok=True)


def write_file(path: str, contents: str, mode: int = 0o600) -> None:
    expect(inside_root(path), f"write path is outside supervisor scratch: {path}")
    make_dir(os.path.dirname(path))
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(contents)
    os.chmod(path, mode)


def write_evidence(name: str, contents: str) -> None:
    path = os.path.realpath(os.path.join(EVIDENCE, name))
    expect(path.startswith(EVIDENCE + os.sep), f"evidence path escaped runner directory: {path}")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(contents)
    os.chmod(path, 0o600)


def seed_private_cargo_home(host_home: str, private_home: str) -> None:
    """Seed the lock's registry and pinned Git inputs with the shared helper.

    `scripts/offline_seed.py` (oxide#131) copies Git candidates first through
    anchored descriptors, generates its own private Git config, refuses host
    alternates, symlinks and changed sources, charges rejected candidates and
    verifies the locked commit only inside the private copy. Cargo creates the
    checkout during staging. The helper's hash is in this proof's receipt:
    importing it transfers no qualification.
    """
    lock_bytes = open(os.path.join(REPO, "Cargo.lock"), "rb").read()
    try:
        seed = offline_seed.seed_cargo_home(
            host_home,
            private_home,
            lock_bytes,
            deadline_ns=DEADLINE_NS - int(SHUTDOWN_MARGIN_S * 1e9),
        )
    except offline_seed.SeedRefusal as error:
        fail(f"bounded offline seed refused: {error}")
    print(
        f"private offline Cargo seed: {seed['packages']} registry packages, "
        f"{len(seed['git']['candidates'])} Git candidates, "
        f"{seed['aggregate_bytes']} bytes, {seed['entries']} entries"
    )


def read_text(path: str) -> str:
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as handle:
            return handle.read()
    except FileNotFoundError:
        return ""


def wait_process(process: subprocess.Popen, label: str) -> int:
    while process.poll() is None:
        work_left()
        time.sleep(0.05)
    return process.wait()


def launch(command: list[str], *, cwd: str, env: dict[str, str], label: str):
    global COUNTER
    work_left()
    COUNTER += 1
    log = os.path.join(LOGS, f"{COUNTER:03d}-{label}.log")
    make_dir(LOGS)
    with open(log, "wb") as output:
        process = subprocess.Popen(command, cwd=cwd, env=env, stdout=output, stderr=subprocess.STDOUT)
    return process, log


def run(command: list[str], *, cwd: str, env: dict[str, str], label: str, expect_code: int = 0):
    process, log = launch(command, cwd=cwd, env=env, label=label)
    code = wait_process(process, label)
    output = read_text(log)
    expect(code == expect_code, f"{label} exited {code}, expected {expect_code}: {output[-3000:]}")
    RESULTS.append({"case": label, "exit": code})
    return output


def clear_owned_environment() -> dict[str, str]:
    env = dict(os.environ)
    for name in list(env):
        if name in ("OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS", "OXIDE_TASK_SCRATCH_ROOT"):
            continue
        if name.startswith(("OXIDE_", "XDG_", "SCCACHE_", "CARGO_BUILD_", "CARGO_TARGET_")) or name in (
            "CARGO", "CARGO_HOME", "CARGO_NET_OFFLINE", "RUSTC", "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER", "RUSTUP_TOOLCHAIN",
        ):
            env.pop(name, None)
    real_rustup = env.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")
    host_cargo_home = os.environ.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo")
    home = os.path.join(ROOT, "home")
    cargo_home = os.path.join(ROOT, "cargo-home")
    make_dir(home)
    make_dir(cargo_home)
    seed_private_cargo_home(host_cargo_home, cargo_home)
    env.update(
        {
            "HOME": home,
            "OXIDE_TEST_HOME_FROM_ENV": "1",
            "CARGO_HOME": cargo_home,
            "RUSTUP_HOME": real_rustup,
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "CARGO_NET_OFFLINE": "true",
            "CARGO_TERM_COLOR": "never",
            "OXIDE_TASK_SCRATCH_ROOT": ROOT,
            "TMPDIR": ROOT,
            "TMP": ROOT,
            "TEMP": ROOT,
        }
    )
    return env


def stage(env: dict[str, str]) -> str:
    stage_root = os.path.join(ROOT, "stage")
    target = os.path.join(stage_root, "target")
    make_dir(stage_root)
    stage_env = dict(env)
    stage_env["CARGO_TARGET_DIR"] = target
    stage_env["CARGO_BUILD_BUILD_DIR"] = os.path.join(stage_root, "build")
    stage_env["CARGO_PROFILE_DEV_DEBUG"] = "0"
    stage_env["CARGO_INCREMENTAL"] = "0"
    stage_env["RUSTC_WRAPPER"] = ""
    stage_env["RUSTC_WORKSPACE_WRAPPER"] = ""
    stage_env["CARGO_BUILD_RUSTC_WRAPPER"] = ""
    tree = run(
        ["cargo", "tree", "--locked", "--offline", "-e", "normal", "--prefix", "none"],
        cwd=REPO, env=stage_env, label="tree-normal",
    )
    write_evidence("tree-normal.txt", tree)
    sha2 = sorted({line.split()[1] for line in tree.splitlines() if line.startswith("sha2 ")})
    expect(sha2 == ["v0.11.0"], f"expected exactly one sha2 v0.11.0, found {sha2}")
    expect(f"#{LONGHORN_COMMIT[:8]}" in tree and "tag=v0.4.1" in tree, "Longhorn is not the v0.4.1 tag commit")
    duplicates = run(
        ["cargo", "tree", "--locked", "--offline", "-e", "normal", "-d", "--target", "all"],
        cwd=REPO, env=stage_env, label="tree-duplicates",
    )
    write_evidence("tree-duplicates.txt", duplicates)
    expect("sha2" not in duplicates, f"sha2 is duplicated: {duplicates}")
    RESULTS.append({"case": "single-sha2", "sha2": sha2})
    output = run(
        ["cargo", "build", "--locked", "--offline", "--bin", "oxide"],
        cwd=REPO, env=stage_env, label="stage-oxide",
    )
    binary = os.path.join(target, "debug", "oxide")
    expect(os.path.isfile(binary), f"staging did not produce {binary}: {output[-3000:]}")
    with open(binary, "rb") as handle:
        digest = hashlib.sha256(handle.read()).hexdigest()
    print(f"staged oxide sha256 {digest}", flush=True)
    RESULTS[-1]["binary_sha256"] = digest
    return binary


def snapshot(path: str) -> list[str]:
    entries = []
    for parent, dirs, files in os.walk(path):
        dirs.sort()
        for name in sorted(dirs + files):
            entries.append(os.path.relpath(os.path.join(parent, name), path))
    return entries


def lines(output: str, prefix: str) -> list[str]:
    return [line for line in output.splitlines() if line.startswith(prefix)]


def status(binary: str, env: dict[str, str], label: str, code: int = 0) -> str:
    return run([binary, "status"], cwd=os.path.join(ROOT, "cwd"), env=env, label=label, expect_code=code)


def prove(binary: str, env: dict[str, str]) -> None:
    home = os.path.join(ROOT, "home dir", "Zoë Ünïcode")
    make_dir(home)
    make_dir(os.path.join(ROOT, "cwd"))
    native = dict(env)
    native["HOME"] = home
    native["OXIDE_TEST_HOME_FROM_ENV"] = "1"
    for name in ("OXIDE_CONFIG", "OXIDE_POOL", "OXIDE_POOL_CAP", "OXIDE_MAX_ACTIVE"):
        native.pop(name, None)
    config = f"{home}/Library/Application Support/oxide/config/config.toml"
    pool = f"{home}/Library/Caches/oxide/build-pool"

    before = snapshot(home)
    output = status(binary, native, "status-default")
    expect(lines(output, "config: ") == [f"config: {config} (platform location, absent)"], f"config path changed:\n{output}")
    expect(lines(output, "pool: ") == [f"pool: {pool} (from built-in default)"], f"pool path changed:\n{output}")
    expect(snapshot(home) == before, f"status created files: {snapshot(home)}")
    RESULTS.append({"case": "macos-default-paths", "config": config, "pool": pool, "created": []})

    xdg = dict(native)
    xdg.update({
        "XDG_CONFIG_HOME": os.path.join(ROOT, "xdg", "config"),
        "XDG_CACHE_HOME": "relative-cache",
        "XDG_STATE_HOME": os.path.join(ROOT, "xdg", "state"),
    })
    xdg.pop("XDG_RUNTIME_DIR", None)
    output_xdg = status(binary, xdg, "status-xdg-ignored")
    expect(lines(output_xdg, "config: ") + lines(output_xdg, "pool: ") == lines(output, "config: ") + lines(output, "pool: "), "XDG changed macOS roots")
    expect(not os.path.exists(os.path.join(ROOT, "xdg")), "status created an XDG root")
    RESULTS.append({"case": "macos-ignores-xdg-and-missing-runtime"})

    # Routing through the cache and state roots: the legacy pool's removable
    # registration and the known-pools record, against an explicit pool.
    legacy = f"{home}/Library/Caches/oxide/pool"
    write_file(os.path.join(legacy, "config.json"), json.dumps({"backend": "directory-budget-v1"}))
    write_file(os.path.join(legacy, "oxide-managed", "removable.json"), "{}\n")
    remembered = os.path.join(ROOT, "remembered pool")
    write_file(os.path.join(remembered, "config.json"), json.dumps({"backend": "directory-budget-v1"}))
    state = f"{home}/Library/Application Support/oxide/state/pools.json"
    write_file(state, json.dumps([remembered]))
    routed = dict(native)
    routed["OXIDE_POOL"] = os.path.join(ROOT, "explicit pool")
    before = snapshot(ROOT + "/home dir")
    output = status(binary, routed, "status-routing")
    expect(
        f"orphaned pool: {legacy} (not the current pool; Oxide never moves or deletes it; holds a removable registration Oxide does not use from here)" in output,
        f"legacy pool registration was not routed through the cache root:\n{output}",
    )
    expect(f"orphaned pool: {remembered} (not the current pool" in output, f"known pools were not read from the state root:\n{output}")
    expect(snapshot(ROOT + "/home dir") == before, "status changed the routed fixture")
    expect(not os.path.exists(routed["OXIDE_POOL"]), "status created the explicit pool")
    RESULTS.append({"case": "cache-and-state-routing", "legacy": legacy, "state": state})

    homeless = dict(env)
    homeless.pop("HOME", None)
    homeless.update({
        "OXIDE_CONFIG": os.path.join(ROOT, "explicit", "oxide.toml"),
        "OXIDE_POOL": os.path.join(ROOT, "explicit", "pool"),
        "XDG_CONFIG_HOME": "relative",
        "XDG_CACHE_HOME": "relative",
        "XDG_STATE_HOME": "relative",
    })
    output = status(binary, homeless, "status-overrides-without-home")
    expect(lines(output, "config: ") == [f"config: {homeless['OXIDE_CONFIG']} (selected by OXIDE_CONFIG, absent)"], output)
    expect(lines(output, "pool: ") == [f"pool: {homeless['OXIDE_POOL']} (from OXIDE_POOL)"], output)
    expect(not os.path.exists(os.path.join(ROOT, "explicit")), "status created an override path")
    RESULTS.append({"case": "overrides-without-home"})

    refused = dict(homeless)
    refused.pop("OXIDE_CONFIG")
    refused.pop("OXIDE_POOL")
    process, log = launch([binary, "status"], cwd=os.path.join(ROOT, "cwd"), env=refused, label="status-missing-home")
    code = wait_process(process, "status-missing-home")
    output = read_text(log)
    expect(code != 0 and "HOME is unset or not an absolute path" in output, f"missing HOME did not refuse ({code}):\n{output}")
    RESULTS.append({"case": "missing-home-refuses", "exit": code})


def main() -> None:
    expect(os.path.isdir(ROOT) and os.path.realpath(ROOT) == os.environ["OXIDE_TASK_SCRATCH_ROOT"], "scratch root is not canonical")
    expect(os.path.isdir(EVIDENCE), "qualified-runner evidence directory is missing")
    with open(os.path.join(REPO, "scripts", "bounded-test.py"), "rb") as runner:
        runner_hash = hashlib.sha256(runner.read()).hexdigest()
    expect(runner_hash == RUNNER_SHA256, f"qualified runner hash mismatch: {runner_hash}")
    work_left()
    source = subprocess.check_output(["git", "rev-parse", "HEAD^{commit}"], cwd=REPO, text=True).strip()
    tree = subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], cwd=REPO, text=True).strip()
    dirty = subprocess.check_output(["git", "status", "--porcelain"], cwd=REPO, text=True).strip()
    expect(not dirty, f"source checkout is dirty: {dirty}")
    make_dir(LOGS)
    env = clear_owned_environment()
    binary = stage(env)
    prove(binary, env)
    work_left()
    receipt = {
        "task": "oxide#103",
        "source_commit": source,
        "source_tree": tree,
        "helper_sha256": {
            "scripts/offline_seed.py": hashlib.sha256(
                open(os.path.join(REPO, "scripts", "offline_seed.py"), "rb").read()
            ).hexdigest()
        },
        "longhorn_commit": LONGHORN_COMMIT,
        "runner_sha256": runner_hash,
        "result": "pass",
        "cases": RESULTS,
        "limits": {"wall_clock_seconds": 300, "shutdown_margin_seconds": SHUTDOWN_MARGIN_S, "scratch_budget_mib": 512},
        "gaps": ["macOS host only: Linux XDG behaviour is proven by injected unit tests, not natively"],
    }
    write_evidence("storage-layout.json", json.dumps(receipt, indent=2) + "\n")
    print(f"storage-layout proof PASS: {len(RESULTS)} recorded cases; source {source}; tree {tree}; runner {runner_hash}", flush=True)


if __name__ == "__main__":
    main()
