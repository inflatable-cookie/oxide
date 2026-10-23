#!/usr/bin/env python3
"""Focused host-wide active-build limit proof (oxide#115).

The qualified shared supervisor owns the deadline, host reserve, scratch
budget, process closure and cleanup. This fixed fixture script keeps every
write under its supplied OXIDE_TASK_SCRATCH_ROOT, uses no real removable
volume, and stops work 30 seconds before the exported absolute deadline.
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
MAX_ACTIVE_RUNNER_SHA256 = "8e379c93416c6e06b7f06bec7f86dba4fca4c8900b1cc87f1eb6303d39897a9d"
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ROOT = os.path.realpath(os.environ["OXIDE_TASK_SCRATCH_ROOT"])
EVIDENCE = os.path.realpath(os.environ["OXIDE_PROOF_EVIDENCE"])
DEADLINE_NS = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
LOGS = os.path.join(ROOT, "logs")
WORKSPACES = os.path.join(ROOT, "workspaces")
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

    `scripts/offline_seed.py` (oxide#131) copies every candidate first through
    anchored descriptors, generates its own private Git config, refuses host
    alternates, symlinks and changed sources, charges rejected candidates and
    verifies the locked commit only inside the private copy. Its hash is part
    of this script's source set: importing it transfers no qualification.
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
    build = os.path.join(stage_root, "build")
    make_dir(stage_root)
    stage_env = dict(env)
    stage_env["CARGO_TARGET_DIR"] = target
    stage_env["CARGO_BUILD_BUILD_DIR"] = build
    stage_env["CARGO_PROFILE_DEV_DEBUG"] = "0"
    stage_env["CARGO_INCREMENTAL"] = "0"
    stage_env["RUSTC_WRAPPER"] = ""
    stage_env["RUSTC_WORKSPACE_WRAPPER"] = ""
    stage_env["CARGO_BUILD_RUSTC_WRAPPER"] = ""
    output = run(
        ["cargo", "build", "--locked", "--offline", "--bin", "oxide"],
        cwd=REPO,
        env=stage_env,
        label="stage-oxide",
    )
    binary = os.path.join(target, "debug", "oxide")
    expect(os.path.isfile(binary), f"staging did not produce {binary}: {output[-3000:]}")
    with open(binary, "rb") as handle:
        digest = hashlib.sha256(handle.read()).hexdigest()
    print(f"staged oxide sha256 {digest}", flush=True)
    RESULTS[-1]["binary_sha256"] = digest
    return binary


def create_workspace(label: str) -> str:
    path = os.path.join(WORKSPACES, label)
    make_dir(os.path.join(path, "src"))
    write_file(
        os.path.join(path, "Cargo.toml"),
        f'[package]\nname = "oxide-max-active-{label}"\nversion = "0.1.0"\nedition = "2021"\n',
    )
    write_file(os.path.join(path, "src", "lib.rs"), "pub fn fixture() {}\n")
    return path


def status(binary: str, env: dict[str, str], cwd: str, label: str, code: int = 0) -> str:
    return run([binary, "status"], cwd=cwd, env=env, label=label, expect_code=code)


def prove_status(binary: str, env: dict[str, str], workspace: str) -> None:
    config = os.path.join(ROOT, "config", "config.toml")
    # macOS must ignore this legacy location when it selects the platform
    # config path. A deliberately out-of-range value must have no effect.
    legacy = os.path.join(ROOT, "home", ".config", "oxide", "config.toml")
    write_file(legacy, "[pool]\nmax_active = 33\n")
    default_env = dict(env)
    default_env.pop("OXIDE_CONFIG", None)
    default_env.pop("OXIDE_MAX_ACTIVE", None)
    default_env["OXIDE_POOL"] = os.path.join(ROOT, "pool")
    default = status(binary, default_env, workspace, "status-derived-default")
    expect("ignored:" in default and ".config/oxide/config.toml" in default, "macOS legacy config was not reported ignored")
    line = next((line for line in default.splitlines() if line.startswith("host-wide active-build limit: ")), "")
    expect(line, f"status omitted the effective active-build limit:\n{default}")
    if "physical cores from hw.physicalcpu=" in line:
        match = re.search(r"limit: (\d+) .*hw\.physicalcpu=(\d+)", line)
        expect(match is not None, f"derived source is incomplete: {line}")
        cores = int(match.group(2))
        expected = min(4, max(2, cores // 4))
        expect(int(match.group(1)) == expected, f"status value does not match formula: {line}")
    else:
        expect("physical-core fallback:" in line and "limit: 2 " in line, f"fallback is not reported truthfully: {line}")

    write_file(config, "[pool]\nmax_active = 0\n")
    configured_env = dict(env)
    configured_env["OXIDE_CONFIG"] = config
    configured_env["OXIDE_POOL"] = os.path.join(ROOT, "pool")
    configured_env.pop("OXIDE_MAX_ACTIVE", None)
    invalid_config = status(binary, configured_env, workspace, "status-invalid-config", code=1)
    expect(config in invalid_config and "[pool] max_active" in invalid_config, f"config error did not name its source: {invalid_config}")

    overridden = dict(configured_env)
    overridden["OXIDE_MAX_ACTIVE"] = "4"
    override = status(binary, overridden, workspace, "status-env-over-invalid-config")
    expect("host-wide active-build limit: 4 (from OXIDE_MAX_ACTIVE)" in override, f"environment precedence was not reported: {override}")

    write_file(config, "[pool]\nmax_active = 3\n")
    configured = status(binary, configured_env, workspace, "status-config-source")
    expect(f"host-wide active-build limit: 3 (from config file {config} [pool].max_active)" in configured, f"config source was not reported: {configured}")
    override = status(binary, overridden, workspace, "status-env-over-config")
    expect("host-wide active-build limit: 4 (from OXIDE_MAX_ACTIVE)" in override, f"environment did not override config: {override}")

    for value in ("1", "32"):
        bounded = dict(configured_env)
        bounded["OXIDE_MAX_ACTIVE"] = value
        output = status(binary, bounded, workspace, f"status-env-bound-{value}")
        expect(f"host-wide active-build limit: {value} (from OXIDE_MAX_ACTIVE)" in output, f"environment bound {value} was not accepted")


def write_fake_removable_state(pool: str, root: str) -> None:
    control = os.path.join(pool, "oxide-managed")
    fake_pool = os.path.join(control, "removable")
    fake_control = os.path.join(fake_pool, "oxide-managed")
    make_dir(fake_control)
    registration = {
        "version": 1,
        "mount_point": os.path.join(root, "not-a-volume"),
        "volume_uuid": "fixture-volume",
        "container_uuid": "fixture-container",
        "reserve_bytes": 1,
        "cap_bytes": 67108864,
        "quota_bytes": None,
        "capacity_bytes": None,
        "registered_ms": 0,
    }
    write_file(os.path.join(control, "removable.json"), json.dumps(registration) + "\n")
    record = {
        "id": 1,
        "identity": "synthetic-removable",
        "compatibility": "synthetic-key",
        "profile": "quick",
        "checkout": None,
        "phase": "running",
        "process_group": None,
        "last_used": 1,
    }
    state = {
        "version": 1,
        "next_id": 2,
        "next_use": 2,
        "tickets": [],
        "cleans": [],
        "records": [record],
        "recoveries": [],
        "flushes": [],
    }
    write_file(os.path.join(fake_control, "state.json"), json.dumps(state) + "\n")


class Holder:
    def __init__(self, process: subprocess.Popen, log: str, marker: str, release: str, label: str):
        self.process = process
        self.log = log
        self.marker = marker
        self.release = release
        self.label = label


def start_command(binary: str, env: dict[str, str], workspace: str, hold_script: str, label: str) -> Holder:
    marker = os.path.join(ROOT, "markers", f"{label}.started")
    release = os.path.join(ROOT, "markers", f"{label}.release")
    try:
        os.remove(marker)
    except FileNotFoundError:
        pass
    try:
        os.remove(release)
    except FileNotFoundError:
        pass
    process, log = launch(
        [binary, "exec", "--", sys.executable, hold_script, marker, release],
        cwd=workspace,
        env=env,
        label=label,
    )
    return Holder(process, log, marker, release, label)


def wait_started(holder: Holder) -> None:
    while not os.path.exists(holder.marker):
        work_left()
        if holder.process.poll() is not None:
            fail(f"{holder.label} exited before its tool started: {read_text(holder.log)[-2000:]}")
        time.sleep(0.05)
    RESULTS.append({"case": f"holder-started-{holder.label}", "pid": holder.process.pid})


def start_hold(binary: str, env: dict[str, str], workspace: str, hold_script: str, label: str) -> Holder:
    holder = start_command(binary, env, workspace, hold_script, label)
    wait_started(holder)
    return holder


def wait_for_log(holder: Holder, phrase: str) -> None:
    while phrase not in read_text(holder.log):
        work_left()
        if holder.process.poll() is not None:
            fail(f"{holder.label} exited before '{phrase}': {read_text(holder.log)[-2000:]}")
        time.sleep(0.05)


def release_hold(holder: Holder) -> None:
    write_file(holder.release, "release\n")
    code = wait_process(holder.process, f"release-{holder.label}")
    output = read_text(holder.log)
    expect(code == 0, f"{holder.label} exited {code}: {output[-2000:]}")
    RESULTS.append({"case": f"released-{holder.label}", "exit": code})


def prove_cap_two(binary: str, env: dict[str, str], workspaces: dict[str, str], hold_script: str) -> None:
    limited = dict(env)
    limited["OXIDE_MAX_ACTIVE"] = "2"
    first = start_hold(binary, limited, workspaces["a"], hold_script, "cap2-a")
    second = start_hold(binary, limited, workspaces["b"], hold_script, "cap2-b")
    waiter = start_command(binary, limited, workspaces["c"], hold_script, "cap2-waiter")
    wait_for_log(waiter, "waiting for a build slot (2 of 2 host-wide in use)")
    expect(not os.path.exists(waiter.marker), "third command started while the limit was 2")
    status_output = status(binary, limited, workspaces["c"], "status-cap2-waiter")
    ticket_line = next(
        (line for line in status_output.splitlines() if f"recorded lease-ticket waiter PID {waiter.process.pid}," in line),
        "",
    )
    expect(
        re.search(r"age approximately \d+s \(file (creation time|modification time fallback)\)", ticket_line) is not None,
        f"status did not show the waiter's approximate ticket age and source: {status_output}",
    )
    RESULTS.append({"case": "cap-2-waits-third", "waiting": True})
    release_hold(first)
    while not os.path.exists(waiter.marker):
        work_left()
        if waiter.process.poll() is not None:
            fail(f"released cap-2 waiter exited: {read_text(waiter.log)[-2000:]}")
        time.sleep(0.05)
    RESULTS.append({"case": "cap-2-release-admits-waiter", "admitted": True})
    waiter_output = read_text(waiter.log)
    ready_at = waiter_output.find("oxide: waited ")
    marker_at = waiter_output.find("tool marker")
    expect(ready_at >= 0 and marker_at > ready_at, f"wait completion was not printed before the tool marker: {waiter_output[-3000:]}")
    expect(
        re.search(
            r"oxide: waited \d+\.\d+s \([^)]*host limit \d+\.\d+s[^)]*\); slot ready",
            waiter_output[ready_at:marker_at],
        )
        is not None,
        f"wait completion lacked a measured total and host-limit component: {waiter_output[-3000:]}",
    )
    release_hold(second)
    release_hold(waiter)
    waiter_output = read_text(waiter.log)
    summary = next((line for line in waiter_output.splitlines() if line.startswith("oxide: quick profile,")), "")
    expect(re.search(r", waited \d+\.\d+s(?:,|$)", summary) is not None, f"waited summary suffix is missing: {waiter_output[-3000:]}")
    for holder in (first, second):
        output = read_text(holder.log)
        expect("oxide: waited " not in output and ", waited " not in output, f"a command with no wait printed a wait diagnostic: {output[-2000:]}")


def prove_clean_wait(binary: str, env: dict[str, str], workspace: str, hold_script: str) -> None:
    holder = start_hold(binary, env, workspace, hold_script, "clean-wait-holder")
    process, log = launch([binary, "clean"], cwd=workspace, env=env, label="clean-waiter")
    clean = Holder(process, log, "", "", "clean-waiter")
    wait_for_log(clean, "waiting for active oxide commands in this checkout")
    release_hold(holder)
    code = wait_process(clean.process, "clean-waiter")
    output = read_text(clean.log)
    expect(code == 0, f"clean waiter exited {code}: {output[-3000:]}")
    ready_at = output.find("oxide: waited ")
    summary_at = output.find(" does not exist; nothing removed, waited ")
    expect(ready_at >= 0 and summary_at > ready_at, f"clean wait completion or summary suffix is missing or out of order: {output[-3000:]}")
    expect("; clean barrier acquired" in output[ready_at:summary_at], f"clean completion did not name the barrier: {output[-3000:]}")
    RESULTS.append({"case": "clean-barrier-wait-and-summary", "exit": code})


def prove_configured_three(binary: str, env: dict[str, str], workspaces: dict[str, str], hold_script: str) -> None:
    config = os.path.join(ROOT, "config", "config.toml")
    write_file(config, "[pool]\nmax_active = 3\n")
    configured = dict(env)
    configured["OXIDE_CONFIG"] = config
    configured.pop("OXIDE_MAX_ACTIVE", None)
    first = start_hold(binary, configured, workspaces["a"], hold_script, "cap3-a")
    second = start_hold(binary, configured, workspaces["b"], hold_script, "cap3-b")
    third = start_hold(binary, configured, workspaces["c"], hold_script, "cap3-c")
    fourth = start_command(binary, configured, workspaces["d"], hold_script, "cap3-waiter")
    wait_for_log(fourth, "waiting for a build slot (3 of 3 host-wide in use)")
    expect(not os.path.exists(fourth.marker), "fourth command started while the limit was 3")
    RESULTS.append({"case": "config-3-permits-third-waits-fourth", "waiting": True})
    release_hold(first)
    while not os.path.exists(fourth.marker):
        work_left()
        if fourth.process.poll() is not None:
            fail(f"released config-3 waiter exited: {read_text(fourth.log)[-2000:]}")
        time.sleep(0.05)
    RESULTS.append({"case": "config-3-release-admits-waiter", "admitted": True})
    release_hold(second)
    release_hold(third)
    release_hold(fourth)


def prove_cross_pool(binary: str, env: dict[str, str], workspaces: dict[str, str], hold_script: str, pool: str) -> None:
    config = os.path.join(ROOT, "config", "config.toml")
    write_file(config, "[pool]\nmax_active = 2\n")
    configured = dict(env)
    configured["OXIDE_CONFIG"] = config
    configured["OXIDE_POOL"] = pool
    configured.pop("OXIDE_MAX_ACTIVE", None)
    write_fake_removable_state(pool, ROOT)
    local = start_hold(binary, configured, workspaces["a"], hold_script, "cross-pool-internal")
    waiter = start_command(binary, configured, workspaces["b"], hold_script, "cross-pool-waiter")
    wait_for_log(waiter, "waiting for a build slot (2 of 2 host-wide in use)")
    expect("removable volume not used" in read_text(waiter.log), "synthetic route did not select the internal fallback")
    expect(not os.path.exists(waiter.marker), "internal admission ignored active removable state")
    RESULTS.append({"case": "synthetic-removable-counted-with-internal", "waiting": True})
    release_hold(local)
    while not os.path.exists(waiter.marker):
        work_left()
        if waiter.process.poll() is not None:
            fail(f"cross-pool waiter exited after release: {read_text(waiter.log)[-2000:]}")
        time.sleep(0.05)
    RESULTS.append({"case": "synthetic-cross-pool-release-admits-waiter", "admitted": True})
    release_hold(waiter)


def main() -> None:
    expect(os.path.isdir(ROOT) and os.path.realpath(ROOT) == os.environ["OXIDE_TASK_SCRATCH_ROOT"], "scratch root is not canonical")
    expect(os.path.isdir(EVIDENCE), "qualified-runner evidence directory is missing")
    with open(os.path.join(REPO, "scripts", "bounded-test.py"), "rb") as runner:
        runner_hash = hashlib.sha256(runner.read()).hexdigest()
    expect(runner_hash == MAX_ACTIVE_RUNNER_SHA256, f"qualified runner hash mismatch: {runner_hash}")
    work_left()
    source = subprocess.check_output(["git", "rev-parse", "HEAD^{commit}"], cwd=REPO, text=True).strip()
    work_left()
    tree = subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], cwd=REPO, text=True).strip()
    work_left()
    dirty = subprocess.check_output(["git", "status", "--porcelain"], cwd=REPO, text=True).strip()
    expect(not dirty, f"source checkout is dirty: {dirty}")
    make_dir(LOGS)
    make_dir(WORKSPACES)
    make_dir(os.path.join(ROOT, "markers"))

    env = clear_owned_environment()
    config = os.path.join(ROOT, "config", "config.toml")
    env["OXIDE_CONFIG"] = config
    env["OXIDE_POOL"] = os.path.join(ROOT, "pool")
    env["OXIDE_POOL_CAP"] = "64MiB"
    binary = stage(env)
    workspaces = {name: create_workspace(name) for name in ("a", "b", "c", "d")}
    prove_status(binary, env, workspaces["a"])

    hold_script = os.path.join(ROOT, "hold.py")
    write_file(
        hold_script,
        "import pathlib, sys, time\n"
        "pathlib.Path(sys.argv[1]).write_text(str(__import__('os').getpid()))\n"
        "print('tool marker', flush=True)\n"
        "release = pathlib.Path(sys.argv[2])\n"
        "while not release.exists(): time.sleep(0.02)\n",
    )
    prove_cap_two(binary, env, workspaces, hold_script)
    prove_clean_wait(binary, env, workspaces["d"], hold_script)
    prove_configured_three(binary, env, workspaces, hold_script)
    prove_cross_pool(binary, env, workspaces, hold_script, env["OXIDE_POOL"])
    work_left()
    receipt = {
        "task": "oxide#115",
        "source_commit": source,
        "source_tree": tree,
        "binary_sha256": RESULTS[0].get("binary_sha256"),
        "helper_sha256": {
            "scripts/offline_seed.py": hashlib.sha256(
                open(os.path.join(REPO, "scripts", "offline_seed.py"), "rb").read()
            ).hexdigest()
        },
        "runner_sha256": runner_hash,
        "result": "pass",
        "cases": RESULTS,
        "limits": {"wall_clock_seconds": 300, "shutdown_margin_seconds": SHUTDOWN_MARGIN_S, "scratch_budget_mib": 512},
    }
    write_evidence("max-active.json", json.dumps(receipt, indent=2) + "\n")
    print(f"max-active proof PASS: {len(RESULTS)} recorded cases; source {source}; tree {tree}; runner {runner_hash}", flush=True)


if __name__ == "__main__":
    main()
