#!/usr/bin/env python3
"""Budget-only default-pool regression (oxide#102).

The retained --budget-default mode exercises default creation, legacy-image
refusal, configuration precedence, soft-budget growth and idle eviction, and
the ordinary host-reserve watchdog. All fixture state is private scratch.
The former mounted-volume qualification is retired: Barge location, ownership,
pruning and trash behavior is covered by ordinary temporary-fixture tests.
"""

from __future__ import annotations

import sys

# No bytecode files in the counted tree or the repository.
sys.dont_write_bytecode = True

import json
import os
import shutil
import signal
import subprocess
import time

import bounded_runner as br
import bounded_runner_cases

BUILDER = "1.97.1"
PROOF_SECONDS = 300
SCRATCH_LIMIT_BYTES = 512 * 1024 * 1024
SOURCE_LIMIT_BYTES = 8 * 1024 * 1024
LOG_LIMIT_BYTES = 32 * 1024 * 1024
HOST_RESERVE_BYTES = 20 * 1024 * 1024 * 1024
CARGO_LIMIT = 96
MIB = 1024 * 1024

# A build script that writes 4 MiB into the slot, past a 2 MB soft budget.
GROWER = {
    "Cargo.toml": '[package]\nname = "grower"\nversion = "0.1.0"\nedition = "2021"\nbuild = "build.rs"\n',
    "build.rs": 'fn main() {\n    let out = std::env::var("OUT_DIR").unwrap();\n    std::fs::write(format!("{out}/grown.bin"), vec![7_u8; 4 * 1024 * 1024]).unwrap();\n}\n',
    "src/lib.rs": "pub fn value() -> u32 {\n    3\n}\n",
}
# A build script that runs long enough for the watchdog to stop it.
SLEEPER = {
    "Cargo.toml": '[package]\nname = "sleeper"\nversion = "0.1.0"\nedition = "2021"\nbuild = "build.rs"\n',
    "build.rs": "fn main() {\n    std::thread::sleep(std::time::Duration::from_secs(30));\n}\n",
    "src/lib.rs": "pub fn value() -> u32 {\n    5\n}\n",
}
LEGACY_CONFIG = '{"version":1,"backend":"apfs-sparse-image-v1","store_id":"00","cap_bytes":134217728,"image_bytes":134217728}'

FIXTURE = {
    "Cargo.toml": '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n',
    "src/lib.rs": "pub fn value() -> u32 {\n    7\n}\n",
}
# Logs every fixture Cargo process, then runs the real one.
CARGO_WRAPPER = """#!/bin/sh
printf '%s\\n' "$*" >> "LOG"
exec "REAL_CARGO" "$@"
"""

STARTED = time.monotonic()
OUTPUT = 0
SCRATCH_VARIABLES = ("OXIDE_TASK_SCRATCH_ROOT", "TMPDIR", "TMP", "TEMP")
# The session guardian (scripts/bounded_runner.py), created at admission.
GUARDIAN = None


_print = print


def print(*args, **kwargs):  # runner diagnostics count toward the output allowance
    if GUARDIAN is not None:
        GUARDIAN.count_output(sum(len(str(arg)) + 1 for arg in args))
    _print(*args, **kwargs)


def fail(message: str) -> None:
    raise RuntimeError(message)


def remaining() -> float:
    left = PROOF_SECONDS - (time.monotonic() - STARTED)
    if left <= 0:
        fail(f"proof exceeded {PROOF_SECONDS} s")
    return left


def emit(label: str, text: str) -> None:
    global OUTPUT
    OUTPUT += len(text.encode())
    if GUARDIAN is not None:
        GUARDIAN.count_output(len(text.encode()) + len(label) + 40)
        GUARDIAN.check(experimental=False)
    if OUTPUT > LOG_LIMIT_BYTES:
        fail("proof output exceeds 32 MiB")
    print(f"----- {label} ({len(text.encode())} bytes) -----")
    print(text, end="" if text.endswith("\n") or not text else "\n")


def run_owned(command: list, *, env: dict, cwd: str | None = None, text: bool = True, timeout: float = 120, cleanup: bool = False):
    """Every child goes through the guardian's single supervised spawn path."""
    result = GUARDIAN.run(command, env=env, cwd=cwd, timeout=timeout, cleanup=cleanup)
    if text:
        result.stdout = result.stdout.decode(errors="replace")
        result.stderr = result.stderr.decode(errors="replace")
    return result


class Proof:
    def __init__(self, parent: str):
        self.parent = parent
        self.env = dict(os.environ)

    def account(self, phase: str) -> None:
        GUARDIAN.check()
        print(
            f"sample {phase}: {time.monotonic() - STARTED:.1f} s, max sampled owned {GUARDIAN.walk_max // 1024} KiB "
            f"over {GUARDIAN.walk_count} complete walks, host free minimum {GUARDIAN.reserve_min // MIB} MiB"
        )


def build(parent: str, repo: str) -> str:
    # os.environ already carries the four scratch variables.
    stage = dict(os.environ)
    for name in list(stage):
        if name.startswith("SCCACHE_") or name in ("CARGO", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "RUSTC"):
            stage.pop(name)
    stage.update(
        {
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(parent, "stage-target"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
            # Fewer transient files under the walked staging target.
            "CARGO_INCREMENTAL": "0",
            "PYTHONDONTWRITEBYTECODE": "1",
        }
    )
    result = run_owned(
        ["cargo", "build", "--locked", "--offline", "--bin", "oxide", "--message-format=json-render-diagnostics"],
        cwd=repo,
        env=stage,
        timeout=240,
    )
    emit(f"staging exit {result.returncode} stderr", result.stderr)
    if result.returncode:
        fail(f"staging exit {result.returncode}")
    for line in result.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") == "compiler-artifact" and message.get("target", {}).get("name") == "oxide" and message.get("executable"):
            return message["executable"]
    fail("staging reported no oxide executable")
    return ""


def main() -> int:
    if sys.argv[1:] != ["--budget-default"]:
        fail("usage: test-oxide-removable-pool.py --budget-default")
    if sys.platform != "darwin":
        fail("the budget-default regression requires macOS")
    raw = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
    if raw:
        root = os.path.realpath(raw)
    else:
        made = subprocess.run(["mktemp", "-d", "/private/tmp/oxide-rmpool.XXXXXX"], capture_output=True, text=True, timeout=30)
        if made.returncode:
            fail("mktemp failed")
        root = os.path.realpath(made.stdout.strip())
    if not root.startswith("/private/tmp/"):
        fail(f"scratch {root} is not under /private/tmp")
    parent = os.path.join(root, "removable-pool")
    os.makedirs(parent, mode=0o700, exist_ok=False)
    # One fresh canonical temp directory, inside the counted tree, for this
    # process and every child (staging, oxide and Cargo) before any of
    # them writes.
    scratch = os.path.join(parent, "tmp")
    os.makedirs(scratch, mode=0o700)
    scratch = os.path.realpath(scratch)
    if not scratch.startswith("/private/tmp/"):
        fail(f"scratch {scratch} is not canonical under /private/tmp")
    for name in SCRATCH_VARIABLES:
        os.environ[name] = scratch
    print(f"scratch variables {', '.join(SCRATCH_VARIABLES)} = {scratch}")
    global GUARDIAN
    os.environ["PYTHONDONTWRITEBYTECODE"] = "1"
    br.install_cancellation()
    if br.host_free(parent) < HOST_RESERVE_BYTES:
        fail("host free space is below the 20 GiB reserve before any effect")
    # The reserve is sampled on /private/tmp, which outlives the guarded
    # removal of the owned scratch.
    GUARDIAN = br.Guardian(STARTED, "/private/tmp", br.Walker([parent]), work_seconds=240, total_seconds=PROOF_SECONDS)
    proof = Proof(parent)
    first = None
    try:
        GUARDIAN.wait_healthy()
        # Synthetic fault cases first, under this real guardian.
        cases_root = os.path.join(parent, "synthetic")
        os.makedirs(cases_root, mode=0o700)
        count = bounded_runner_cases.run_cases(GUARDIAN, cases_root, dict(os.environ), print)
        shutil.rmtree(cases_root)
        print(f"synthetic preflight held: {count} cases, {time.monotonic() - STARTED:.1f} s")
        GUARDIAN.check()
        run(proof, parent)
    except BaseException as error:
        first = error
    if first is not None:
        # Cleanup-only: ordinary detach after positive closure, never
        # clearing the fault or starting experimental work.
        try:
            GUARDIAN.settle()
        except BaseException as error:  # keep the first failure
            print(f"cleanup after failure: {error!r}", file=sys.stderr)
        for failure in GUARDIAN.cleanup_failures:
            print(f"cleanup failure: {failure}", file=sys.stderr)
        print(
            f"budget-default regression failed after {time.monotonic() - STARTED:.0f} s (guardian first fault: {GUARDIAN.first}); "
            f"evidence kept at {parent}",
            file=sys.stderr,
        )
        GUARDIAN.shutdown()
        raise first
    GUARDIAN.check(experimental=False)
    # Guarded removal: the owned-byte record switches explicitly to the last
    # complete total (removal only shrinks it), the reserve and deadline
    # monitors stay live, and removal is a bounded cleanup step.
    GUARDIAN.sample_storage()
    GUARDIAN.check(experimental=False)
    GUARDIAN.walker.frozen = GUARDIAN.walk_max
    removal = run_owned(["/bin/rm", "-rf", parent], env=dict(os.environ), cwd="/", timeout=60, cleanup=True)
    if removal.returncode != 0 or os.path.lexists(parent):
        fail(f"guarded removal of {parent} failed (exit {removal.returncode})")
    GUARDIAN.check(experimental=False)
    walks, peak, low = GUARDIAN.walk_count, GUARDIAN.walk_max, GUARDIAN.reserve_min
    GUARDIAN.shutdown()
    if raw is None:
        os.rmdir(root)
    elapsed = time.monotonic() - STARTED
    print(
        f"budget-default regression held: {elapsed:.0f} s, max sampled owned {peak // MIB} MiB over {walks} complete walks "
        f"(at most every 2 s, each within 1 s; 512 MiB monitored cap, not an instantaneous ceiling), "
        f"host free minimum {low // MIB} MiB, output {OUTPUT} bytes, scratch removed"
    )
    if elapsed > PROOF_SECONDS:
        fail(f"proof exceeded {PROOF_SECONDS} s including cleanup")
    return 0


def write_tree(root: str, files: dict) -> str:
    for relative, text in files.items():
        path = os.path.join(root, relative)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text)
    return os.path.realpath(root)


def snapshot(root: str) -> dict:
    found = {}
    for directory, _, files in os.walk(root):
        for name in files:
            path = os.path.join(directory, name)
            with open(path, "rb") as handle:
                found[os.path.relpath(path, root)] = handle.read()
    return found


def legacy_image_pool(home: str) -> str:
    """The retired image pool's shape at its default path, in scratch."""
    legacy = os.path.join(home, "Library/Caches/oxide/pool")
    os.makedirs(os.path.join(legacy, "payload"), mode=0o700)
    for name, text in (("config.json", LEGACY_CONFIG), ("receipt.json", "{}"), ("image.sparseimage", "")):
        with open(os.path.join(legacy, name), "w", encoding="utf-8") as handle:
            handle.write(text)
    return legacy


def budget_default(proof, parent: str, home: str, base: dict, oxide_run, expect, summary) -> None:
    """oxide#102: the budget-only default pool, config, refusals, soft
    budget growth and eviction, watchdog, legacy detection."""
    env = {name: value for name, value in base.items() if not name.startswith("OXIDE_POOL")}
    env["OXIDE_TASK_SCRATCH_ROOT"] = base["OXIDE_TASK_SCRATCH_ROOT"]

    def call(args: list, label: str, extra: dict | None = None, cwd: str | None = None, drop: tuple = ()) -> tuple:
        call_env = {name: value for name, value in env.items() if name not in drop}
        call_env.update(extra or {})
        result = run_owned([proof.oxide, *args], cwd=cwd or app, env=call_env, timeout=120)
        emit(f"budget: {label}: exit {result.returncode} stdout", result.stdout)
        emit(f"budget: {label}: stderr", result.stderr)
        return result.returncode, result.stdout, result.stderr

    caches = os.path.join(home, "Library/Caches/oxide")
    default = os.path.join(caches, "build-pool")
    support = os.path.join(home, "Library/Application Support/oxide")
    config_file = os.path.join(support, "config/config.toml")
    legacy = legacy_image_pool(home)
    before = snapshot(legacy)
    app = write_tree(os.path.join(parent, "budget-app"), FIXTURE)

    # Nothing created: status names the platform config, default path and
    # budget, and reports the legacy image pool without touching it.
    code, stdout, _ = call(["status"], "status before any pool")
    for needle in (
        f"config: {config_file} (platform location, absent)",
        f"pool: {default} (from built-in default)",
        "soft budget 100000 MB (from built-in default)",
        "not created; the first building oxide command creates it (directory-budget-v1",
        f"legacy pool: {legacy} (retired apfs-sparse-image-v1; not used, attached or written",
    ):
        expect(code == 0 and needle in stdout, f"status lacks {needle!r}")
    expect(not os.path.exists(default), "status created the default pool")

    # A pool path at the legacy image pool refuses without touching it.
    code, _, stderr = call(["check"], "legacy image pool selected", {"OXIDE_POOL": legacy})
    expect(code != 0 and "retired apfs-sparse-image-v1" in stderr, "the legacy image pool was not refused")
    # A symlink alias to it refuses before any lock is created or taken.
    alias = os.path.join(parent, "legacy-alias")
    os.symlink(legacy, alias)
    code, _, stderr = call(["check"], "legacy image pool alias", {"OXIDE_POOL": alias})
    expect(code != 0 and "retired apfs-sparse-image-v1" in stderr, "the legacy alias was not refused")
    expect(snapshot(legacy) == before and not os.path.exists(os.path.join(legacy, "lock")), "the legacy alias touched the pool")
    code, _, stderr = call(["check"], "retired backend request", {"OXIDE_POOL_BACKEND": "apfs-sparse-image"})
    expect(code != 0 and "OXIDE_POOL_BACKEND is retired" in stderr, "the retired backend request was not refused")

    # First build: the default pool is a budget-only directory at the new path.
    code, _, stderr = call(["check"], "first build creates the default pool")
    line = summary(stderr)
    expect(
        code == 0 and "directory-budget-v1" in line and f"creating the build pool at {os.path.realpath(caches)}/build-pool (100000 MB soft budget, directory-budget-v1" in stderr,
        f"default creation: {stderr}",
    )
    expect(os.path.isfile(os.path.join(default, "config.json")) and not os.path.exists(os.path.join(default, "image.sparseimage")), "default pool is not a directory pool")
    with open(os.path.join(default, "config.json"), encoding="utf-8") as handle:
        recorded = json.load(handle)
    expect(recorded["backend"] == "directory-budget-v1" and recorded["cap_bytes"] == 100_000_000_000, f"recorded {recorded}")
    excluded = run_owned(["/usr/bin/tmutil", "isexcluded", default], env=env, timeout=30)
    emit(f"tmutil isexcluded exit {excluded.returncode}", excluded.stdout + excluded.stderr)
    expect(excluded.stdout.startswith("[Excluded]"), "the new pool is not excluded from Time Machine")
    expect(snapshot(legacy) == before, "the legacy image pool changed")

    # Ignored ~/.config, Time Machine and Spotlight lines, no file contents.
    dot_config = os.path.join(home, ".config/oxide")
    os.makedirs(dot_config, mode=0o700)
    with open(os.path.join(dot_config, "config.toml"), "w", encoding="utf-8") as handle:
        handle.write('[pool]\npath = "/never-read-secret"\n')
    code, stdout, _ = call(["status"], "status with the default pool")
    for needle in (
        f"ignored: {dot_config}/config.toml exists but is never read on macOS",
        "backend directory-budget-v1: budget-only",
        "soft budget 100000 MB (checked at admission",
        "(shared; no Oxide quota or hard ceiling)",
        "Time Machine: excluded (tmutil isexcluded)",
        f"Spotlight: not excluded by Oxide; to stop indexing, add {os.path.realpath(default)} in System Settings",
        f"legacy pool: {legacy}",
    ):
        expect(code == 0 and needle in stdout, f"status lacks {needle!r}")
    expect("never-read-secret" not in stdout, "status printed ignored file contents")

    # Config file: path and budget, then environment precedence.
    dev = os.path.join(parent, "dev")
    os.makedirs(dev, mode=0o700)
    configured = os.path.join(dev, "oxide")
    os.makedirs(os.path.dirname(config_file), mode=0o700)

    def write_config(text: str) -> None:
        with open(config_file, "w", encoding="utf-8") as handle:
            handle.write(text)

    write_config(f'[pool]\npath = "{configured}"\nbudget = "2MB"\n')
    code, stdout, _ = call(["status"], "status from the config file")
    for needle in (
        f"config: {config_file} (platform location)",
        f"pool: {configured} (from config file)",
        "soft budget 2 MB (from config file)",
        f"orphaned pool: {default} (not the current pool; Oxide never moves or deletes it",
    ):
        expect(code == 0 and needle in stdout, f"status lacks {needle!r}")
    code, stdout, _ = call(["status"], "environment over the file", {"OXIDE_POOL_CAP": "3MB"})
    expect(code == 0 and "soft budget 3 MB (from OXIDE_POOL_CAP)" in stdout, "OXIDE_POOL_CAP did not win")
    explicit = os.path.join(parent, "explicit.toml")
    with open(explicit, "w", encoding="utf-8") as handle:
        handle.write('[pool]\nbudget = "5MB"\n')
    code, stdout, _ = call(["status"], "OXIDE_CONFIG", {"OXIDE_CONFIG": explicit})
    expect(code == 0 and f"config: {explicit} (selected by OXIDE_CONFIG)" in stdout and "soft budget 5 MB (from config file)" in stdout, "OXIDE_CONFIG was not used")

    # Refusals create nothing.
    repo = os.path.join(parent, "repo")
    os.makedirs(os.path.join(repo, ".git"), mode=0o700)
    alias = os.path.join(parent, "alias")
    os.symlink(repo, alias)
    for text, needle, label in (
        (f'[pool]\npath = "{repo}/pool"\n', "inside the Git work tree", "git work tree"),
        (f'[pool]\npath = "{alias}/pool"\n', "inside the Git work tree", "symlink alias into a work tree"),
        ('[pool]\npath = "/Volumes/OxNone/pool"\n', "external volume", "/Volumes"),
        ('[pool]\nbudgte = "2MB"\n', config_file, "unknown key"),
        ("[pool\n", config_file, "malformed file"),
        ('[pool]\npath = "relative/pool"\n', "must be absolute", "relative path"),
    ):
        write_config(text)
        code, _, stderr = call(["check"], f"refusal: {label}")
        expect(code != 0 and needle in stderr, f"{label} was not refused with {needle!r}")
    traversal = os.path.join(parent, "missing", "..", "repo", "pool")
    code, _, stderr = call(["check"], "refusal: OXIDE_POOL traversal into a work tree", {"OXIDE_POOL": traversal})
    expect(code != 0 and ".. components" in stderr, "OXIDE_POOL traversal was not refused")
    expect(not os.path.exists(os.path.join(parent, "missing")), "the traversal refusal created parents")
    expect(not os.path.exists(os.path.join(repo, "pool")), "a refusal created a pool")

    # Soft budget: a build grows past 2 MB and completes; the next
    # admission evicts the whole idle slot.
    write_config(f'[pool]\npath = "{configured}"\nbudget = "2MB"\n')
    grower = write_tree(os.path.join(parent, "grower"), GROWER)
    code, _, stderr = call(["check"], "build grows past the soft budget", cwd=grower)
    line = summary(stderr)
    expect(code == 0 and ", 1 MiB budget, pool total: see oxide status" in line, f"growth build: {line}")
    with open(os.path.join(configured, "ledger.json"), encoding="utf-8") as handle:
        slots = json.load(handle)["slots"]
    expect(len(slots) == 1 and slots[0]["state"] == "idle", f"ledger after growth {slots}")
    grown = slots[0]["id"]
    # oxide#104: a new slot sits at its recorded `dir`.
    grown_dir = slots[0].get("dir") or str(grown)
    excluded = run_owned(["/usr/bin/tmutil", "isexcluded", configured], env=env, timeout=30)
    expect(excluded.stdout.startswith("[Excluded]"), "the configured pool is not excluded from Time Machine")
    code, stdout, _ = call(["status"], "status after growth")
    allocated = [l for l in stdout.splitlines() if "MiB allocated by slots" in l]
    expect(code == 0 and allocated and int(allocated[0].split("MiB allocated by slots")[0].split()[-1]) >= 4, f"growth not reported: {allocated}")
    code, _, stderr = call(["check"], "next admission evicts the idle slot")
    expect(code == 0, "admission after growth failed")
    with open(os.path.join(configured, "ledger.json"), encoding="utf-8") as handle:
        slots = json.load(handle)["slots"]
    expect(all(slot["id"] != grown for slot in slots), f"the grown idle slot was not evicted: {slots}")
    expect(not os.path.exists(os.path.join(configured, "payload/slots", grown_dir)), "evicted slot bytes remain")

    # Watchdog: an injected low reading stops a running build.
    sleeper = write_tree(os.path.join(parent, "sleeper"), SLEEPER)
    readings = os.path.join(parent, "watchdog.readings")
    with open(readings, "w", encoding="utf-8") as handle:
        handle.write(str(1024 * MIB))
    started = time.monotonic()
    code, _, stderr = call(["check"], "injected low reserve", {"OXIDE_TEST_WATCHDOG_READINGS": readings}, cwd=sleeper)
    expect(code != 0 and "oxide: reserve stop (injected test readings)" in stderr, "the watchdog did not stop the build")
    expect(time.monotonic() - started < 25, "the watchdog stop took the whole sleep")
    expect(snapshot(legacy) == before, "the legacy image pool changed")
    proof.account("after budget-default cases")


def run(proof: Proof, parent: str) -> None:
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    real_cargo = shutil.which("cargo")
    if not real_cargo:
        fail("no cargo on PATH")
    source = run_owned(["git", "rev-parse", "HEAD"], cwd=repo, env=dict(os.environ), timeout=30)
    print(f"budget-default regression {os.getpid()} source {source.stdout.strip()}")
    oxide = build(parent, repo)
    proof.oxide = oxide
    print(f"staging: {time.monotonic() - STARTED:.0f} s (1 Cargo process)")
    proof.account("after staging")

    bin_dir = os.path.join(parent, "bin")
    cargo_home = os.path.join(parent, "cargo-home")
    tmp = os.environ["TMPDIR"]
    for path in (bin_dir, cargo_home):
        os.makedirs(path, mode=0o700)
    cargo_log = os.path.join(parent, "cargo.log")
    wrapper = os.path.join(bin_dir, "cargo")
    with open(wrapper, "w", encoding="utf-8") as handle:
        handle.write(CARGO_WRAPPER.replace("LOG", cargo_log).replace("REAL_CARGO", real_cargo))
    os.chmod(wrapper, 0o755)
    pool = os.path.join(parent, "pool")
    env = dict(os.environ)
    real_rustup = env.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")
    home = os.path.join(parent, "home")
    os.makedirs(home, mode=0o700)
    for name in list(env):
        if name.startswith("SCCACHE_") or name.startswith("OXIDE_") or name.startswith("XDG_") or name in (
            "CARGO", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "RUSTC", "RUSTUP_TOOLCHAIN"
        ):
            env.pop(name)
    env.update(
        {
            "OXIDE_POOL": pool,
            "OXIDE_POOL_CREATE": "1",
            "OXIDE_POOL_BACKEND": "directory-budget",
            "OXIDE_POOL_CAP": "64MiB",
            "CARGO_HOME": cargo_home,
            # Oxide's config, state and legacy lookups stay in scratch.
            "HOME": home,
            "OXIDE_TEST_HOME_FROM_ENV": "1",
            "RUSTUP_HOME": real_rustup,
            "PATH": bin_dir + os.pathsep + env.get("PATH", ""),
            "CARGO_NET_OFFLINE": "true",
            "CARGO_TERM_COLOR": "never",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "",
            "RUSTUP_AUTO_INSTALL": "0",
            "OXIDE_TASK_SCRATCH_ROOT": tmp,
            "TMPDIR": tmp,
            "TMP": tmp,
            "TEMP": tmp,
        }
    )
    proof.env = env
    app = os.path.join(parent, "app")
    for relative, text in FIXTURE.items():
        path = os.path.join(app, relative)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text)
    app = os.path.realpath(app)
    inputs = sum(len(text.encode()) for text in FIXTURE.values())
    if inputs > SOURCE_LIMIT_BYTES:
        fail("fixture source exceeds 8 MiB")

    def oxide_run(args: list, label: str, extra: dict | None = None, cwd: str = app) -> tuple:
        call_env = dict(env)
        call_env.update(extra or {})
        result = run_owned([oxide, *args], cwd=cwd, env=call_env, timeout=120)
        emit(f"{label}: exit {result.returncode} stdout", result.stdout)
        emit(f"{label}: stderr", result.stderr)
        return result.returncode, result.stdout, result.stderr

    def expect(condition: bool, message: str) -> None:
        if not condition:
            fail(message)

    def summary(stderr: str) -> str:
        lines = [line for line in stderr.splitlines() if " profile, " in line and " strategy, slot " in line]
        expect(len(lines) == 1, f"expected one summary line: {stderr}")
        return lines[0]

    budget_default(proof, parent, home, env, oxide_run, expect, summary)
    with open(cargo_log, encoding="utf-8") as handle:
        calls = handle.read().splitlines()
    total = len(calls) + 1
    print(f"Cargo processes: {total} (staging 1, fixture {len(calls)})")
    expect(total <= CARGO_LIMIT, f"{total} Cargo processes exceed {CARGO_LIMIT}")


if __name__ == "__main__":
    sys.exit(main())
