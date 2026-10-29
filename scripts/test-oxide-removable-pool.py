#!/usr/bin/env python3
"""Focused proof for the removable build volume, part 1 (oxide#097).

Stages `oxide` offline with 1.97.1 (debug, so the flush-failure injection
exists), then models the removable drive as ONE separately attached 128 MiB
APFS sparse image in owned scratch, mounted normally by hdiutil at a unique
/Volumes/<name>. The internal pool is a budget-only directory pool in the same
scratch, so no second image exists. Covers registration and its refusals,
selection when the volume is present, below its reserve and absent (with no
mount point recreated), flush-on-release with its cost, retirement on an
injected flush failure, warm reuse after re-attaching, and status.

Bounds: 300 s from admission through teardown, 512 MiB owned bytes
(mounted payload included), 8 MiB fixture source, 32 MiB output, 96 Cargo
processes, 20 GiB host reserve sampled every 250 ms. No forced detach: an
ordinary detach that fails stops the proof and keeps the evidence. Not a
benchmark and not full QA.

`--budget-default` (oxide#102, selector `rust:oxide-budget-default`) first
runs the budget-only default-pool cases in the same guarded session, over a
scratch HOME (Rust toolchains still from the real RUSTUP_HOME): legacy image
pool detection without touching it, default creation at the new path as a
directory pool with a verified Time Machine exclusion, honest Spotlight
status, the ignored macOS `~/.config` file, config-file and environment
precedence, refusals (Git work tree, symlink alias, `..` traversal in
OXIDE_POOL, /Volumes, unknown key, malformed file) creating nothing, a
symlink alias to the legacy pool refused before any lock, the orphan report after a path change, a
build growing past a tiny soft budget completing, whole-idle-slot eviction
at the next admission and an injected low-reserve watchdog stop. The
removable phase then also proves the previous-registration refusal (idle and
held legacy fixtures) before its ordinary registration.
"""

from __future__ import annotations

import sys

# No bytecode files in the counted tree or the repository.
sys.dont_write_bytecode = True

import glob
import json
import os
import plistlib
import secrets
import shutil
import signal
import subprocess
import time

import bounded_runner as br
import bounded_runner_cases

BUILDER = "1.97.1"
IMAGE_MIB = 128
PROOF_SECONDS = 300
SCRATCH_LIMIT_BYTES = 512 * 1024 * 1024
SOURCE_LIMIT_BYTES = 8 * 1024 * 1024
LOG_LIMIT_BYTES = 32 * 1024 * 1024
HOST_RESERVE_BYTES = 20 * 1024 * 1024 * 1024
CARGO_LIMIT = 96
MIB = 1024 * 1024

BUDGET_DEFAULT = "--budget-default" in sys.argv[1:]
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
        self.mount = None
        self.image = os.path.join(parent, "volume.sparseimage")
        self.env = dict(os.environ)
        self.name = "OxRm" + secrets.token_hex(4)

    def account(self, phase: str) -> None:
        GUARDIAN.check()
        print(
            f"sample {phase}: {time.monotonic() - STARTED:.1f} s, max sampled owned {GUARDIAN.walk_max // 1024} KiB "
            f"over {GUARDIAN.walk_count} complete walks, host free minimum {GUARDIAN.reserve_min // MIB} MiB"
        )

    def hdiutil(self, *args: str, cleanup: bool = False) -> subprocess.CompletedProcess:
        result = run_owned(["/usr/bin/hdiutil", *args], env=self.env, text=False, timeout=60, cleanup=cleanup)
        emit(f"hdiutil {args[0]} exit {result.returncode}", result.stderr.decode(errors="replace"))
        return result

    def attach(self, read_only: bool = False) -> str:
        args = ["attach", "-nobrowse", "-plist", *(["-readonly"] if read_only else []), self.image]
        # Attach intent before mutation: until resolved, owned bytes are
        # unknown and the walk faults rather than counting zero.
        GUARDIAN.walker.unresolved_attach = (self.image, IMAGE_MIB * MIB)
        result = self.hdiutil(*args)
        if result.returncode:
            fail("attach failed; attachment unresolved, image preserved")
        entities = plistlib.loads(result.stdout)["system-entities"]
        mounts = [entity for entity in entities if entity.get("mount-point")]
        if len(mounts) != 1:
            fail(f"expected one mounted volume, got {mounts}")
        mount = mounts[0]["mount-point"]
        if mount != f"/Volumes/{self.name}":
            # Another volume holds the name: never touch it.
            self.mount = mount
            fail(f"the scratch volume mounted at {mount}, not /Volumes/{self.name}")
        self.mount = mount
        GUARDIAN.walker.mounts[mount] = os.stat(mount).st_dev
        GUARDIAN.walker.unresolved_attach = None
        print(f"attached {self.image} at {mount} ({mounts[0].get('dev-entry')}, read-only {read_only})")
        return mount

    def detach(self, cleanup: bool = False) -> None:
        if GUARDIAN.walker.unresolved_attach is not None:
            fail(f"attachment of {self.image} is unresolved; image and scratch preserved")
        if not self.mount or not os.path.ismount(self.mount):
            return
        if GUARDIAN.unclosed:
            fail(f"owned sessions {sorted(GUARDIAN.unclosed)} are not positively closed; no detach, evidence kept")
        # Ordinary detach only; a busy volume stops the proof. The payload
        # stays counted until the mount is confirmed gone.
        GUARDIAN.walker.hold(self.mount)
        result = self.hdiutil("detach", self.mount, cleanup=cleanup)
        if result.returncode:
            fail(f"ordinary detach of {self.mount} failed; evidence kept")
        GUARDIAN.walker.release(self.mount)


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
    if sys.argv[1:] not in ([], ["--budget-default"]):
        fail("usage: test-oxide-removable-pool.py [--budget-default]")
    if sys.platform != "darwin":
        fail("the removable-pool proof requires macOS")
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
    # process and every child (staging, oxide, Cargo, hdiutil) before any of
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
            proof.detach(cleanup=True)
        except BaseException as error:  # keep the first failure
            print(f"cleanup after failure: {error!r}", file=sys.stderr)
        for failure in GUARDIAN.cleanup_failures:
            print(f"cleanup failure: {failure}", file=sys.stderr)
        print(
            f"removable-pool proof failed after {time.monotonic() - STARTED:.0f} s (guardian first fault: {GUARDIAN.first}); "
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
        f"removable-pool proof held: {elapsed:.0f} s, max sampled owned {peak // MIB} MiB over {walks} complete walks "
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


def previous_registration_refused(home: str, mount: str, control: str, oxide_run, expect) -> None:
    """A registration under the previous (legacy) pool refuses a new one and
    is never moved, forked or removed: idle and held fixtures."""
    legacy = os.path.join(home, "Library/Caches/oxide/pool")
    managed = os.path.join(legacy, "oxide-managed")
    os.makedirs(managed, mode=0o700)
    with open(os.path.join(managed, "removable.json"), "w", encoding="utf-8") as handle:
        handle.write('{"version":1}')
    for case in ("idle", "held"):
        if case == "held":
            held = os.path.join(managed, "removable")
            os.makedirs(held, mode=0o700)
            with open(os.path.join(held, "ledger.json"), "w", encoding="utf-8") as handle:
                handle.write('{"slots":[{"id":1,"state":"held"}]}')
        before = snapshot(legacy)
        code, _, stderr = oxide_run(["pool", "register-removable", mount, "--reserve", "16MiB", "--cap", "64MiB"], f"refusal: previous registration ({case})")
        expect(code != 0 and "registered under the previous pool" in stderr and "no held slots or waiting commands" in stderr, f"{case} previous registration was not refused")
        expect(snapshot(legacy) == before, f"the {case} previous registration changed")
        expect(not os.path.exists(os.path.join(control, "removable.json")) and not os.path.exists(os.path.join(control, "removable")), "a refusal forked a removable ledger")
        expect(not os.path.exists(os.path.join(mount, "oxide-pool")), "a refusal wrote to the volume")
    code, _, stderr = oxide_run(["check"], "build note: previous registration")
    expect(code == 0 and "the removable registration under the previous pool" in stderr, "the previous registration was silently dropped")
    # The fixture is this proof's own scratch; remove it for the no-registration path.
    shutil.rmtree(legacy)


def run(proof: Proof, parent: str) -> None:
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    real_cargo = shutil.which("cargo")
    if not real_cargo:
        fail("no cargo on PATH")
    source = run_owned(["git", "rev-parse", "HEAD"], cwd=repo, env=dict(os.environ), timeout=30)
    print(f"removable-pool proof {os.getpid()} source {source.stdout.strip()} volume name {proof.name}")
    if os.path.lexists(f"/Volumes/{proof.name}"):
        fail(f"/Volumes/{proof.name} already exists; never touched")
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

    if BUDGET_DEFAULT:
        budget_default(proof, parent, home, env, oxide_run, expect, summary)

    control = os.path.join(pool, "oxide-managed")
    removable_root = os.path.join(control, "removable")

    def removable_state() -> dict:
        with open(os.path.join(removable_root, "oxide-managed", "state.json"), encoding="utf-8") as handle:
            return json.load(handle)

    def ledger() -> dict:
        with open(os.path.join(removable_root, "ledger.json"), encoding="utf-8") as handle:
            return json.load(handle)

    # Internal pool first; nothing registered: unchanged behavior.
    code, _, stderr = oxide_run(["check"], "internal, nothing registered")
    expect(code == 0, "internal check failed")
    line = summary(stderr)
    # The scratch path itself contains "removable"; look for Oxide's words.
    expect(
        "directory-budget-v1" in line and "removable volume" not in stderr and "apfs-volume-v1" not in stderr,
        f"unregistered build changed: {line}",
    )
    code, stdout, _ = oxide_run(["status"], "status, nothing registered")
    expect(code == 0 and "removable pool: none registered" in stdout, "status did not report no registration")

    # Mount-parent rule: the real standard /Volumes (read only), then an
    # owned scratch directory before and after an account-name ACL. The
    # ACL refusal must be reported by name, not hidden behind the owner.
    import pwd

    shape = run_owned(["/usr/bin/stat", "-f", "%Su %Sp %Sf flags=%f", "/Volumes"], env=env, timeout=30)
    emit(f"/Volumes shape exit {shape.returncode}", shape.stdout + shape.stderr)
    code, stdout, _ = oxide_run(["pool", "debug-check-parent", "/Volumes"], "parent rule: /Volumes")
    expect(code == 0 and "parent accepted: /Volumes" in stdout, "the standard /Volumes parent was refused")
    acl_dir = os.path.join(parent, "acl-parent")
    os.makedirs(acl_dir, mode=0o755)
    os.chmod(acl_dir, 0o755)
    code, stdout, _ = oxide_run(["pool", "debug-check-parent", acl_dir], "parent rule: owned, no ACL")
    expect(code == 1 and "not root" in stdout and "ACL" not in stdout, "owned directory control did not refuse on owner alone")
    account = pwd.getpwuid(os.geteuid()).pw_name
    expect(account and not account.isdigit() and pwd.getpwnam(account).pw_uid == os.geteuid(), f"account name {account!r} does not round-trip")
    grant = run_owned(["/bin/chmod", "+a", f"{account} allow add_subdirectory", acl_dir], env=env, timeout=30)
    emit(f"chmod +a '{account} allow add_subdirectory' exit {grant.returncode}", grant.stdout + grant.stderr)
    expect(grant.returncode == 0, "the owned ACL could not be set")
    listing = run_owned(["/bin/ls", "-led", acl_dir], env=env, timeout=30)
    emit(f"ls -led exit {listing.returncode}", listing.stdout + listing.stderr)
    expect(f"user:{account} allow add_subdirectory" in listing.stdout, "the owned ACL is not visible")
    code, stdout, _ = oxide_run(["pool", "debug-check-parent", acl_dir], "parent rule: owned, with ACL")
    expect(code == 1 and "1 extended ACL entries" in stdout and "not root" in stdout, "the ACL was not refused by name")

    # Refusals before any image exists.
    image_size = f"{IMAGE_MIB}m"
    for args, needle, label in (
        (["pool", "register-removable", "/Volumes/" + proof.name, "--cap", "64MiB"], "usage", "missing reserve"),
        (["pool", "register-removable", "/Volumes/" + proof.name, "--reserve", "0", "--cap", "64MiB"], "--reserve", "zero reserve"),
        (["pool", "register-removable", "/Volumes/" + proof.name, "--reserve", "99999999999TiB", "--cap", "64MiB"], "--reserve", "overflow reserve"),
        (["pool", "register-removable", parent, "--reserve", "16MiB", "--cap", "64MiB"], "directly under /Volumes", "custom mount parent"),
        (["pool", "register-removable", "/Volumes/" + proof.name + "/nested", "--reserve", "16MiB", "--cap", "64MiB"], "directly under /Volumes", "nested path"),
        (["pool", "register-removable", "/Volumes/" + proof.name, "--reserve", "16MiB", "--cap", "64MiB"], "not available", "absent volume"),
        (["pool", "register-removable", "/Volumes/Macintosh HD", "--reserve", "16MiB", "--cap", "64MiB"], "not available", "symlink alias"),
    ):
        code, stdout, stderr = oxide_run(args, f"refusal: {label}")
        expect(code != 0 and needle in (stdout + stderr), f"{label} was not refused with {needle!r}")
    expect(not os.path.exists(os.path.join(control, "removable.json")), "a refusal wrote a registration")
    expect(not os.path.lexists(f"/Volumes/{proof.name}"), "a refusal created a mount point")

    create = run_owned(
        ["/usr/bin/hdiutil", "create", "-size", image_size, "-fs", "APFS", "-volname", proof.name, "-type", "SPARSE", proof.image],
        env=env,
        timeout=60,
    )
    emit(f"hdiutil create exit {create.returncode}", create.stdout + create.stderr)
    expect(create.returncode == 0, "image creation failed")
    proof.account("after image creation")

    # Read-only attach refuses.
    mount = proof.attach(read_only=True)
    code, stdout, stderr = oxide_run(["pool", "register-removable", mount, "--reserve", "16MiB", "--cap", "64MiB"], "refusal: read-only")
    expect(code != 0 and "read-only" in stderr, "read-only volume was not refused")
    proof.detach()

    mount = proof.attach()
    proof.account("after attach")
    code, stdout, stderr = oxide_run(["pool", "register-removable", mount + "x", "--reserve", "16MiB", "--cap", "64MiB"], "refusal: prefix look-alike")
    expect(code != 0, "prefix look-alike registered")
    if BUDGET_DEFAULT:
        previous_registration_refused(home, mount, control, oxide_run, expect)
    code, stdout, stderr = oxide_run(["pool", "register-removable", mount, "--reserve", "16MiB", "--cap", "64MiB"], "register")
    expect(code == 0 and "registered removable pool" in stdout, "registration failed")
    with open(os.path.join(control, "removable.json"), encoding="utf-8") as handle:
        registration = json.load(handle)
    info = plistlib.loads(run_owned(["/usr/sbin/diskutil", "info", "-plist", mount], env=env, text=False, timeout=30).stdout)
    expect(registration["volume_uuid"] == info["VolumeUUID"], "registered UUID is not diskutil's")
    expect(registration["mount_point"] == mount and registration["reserve_bytes"] == 16 * MIB and registration["cap_bytes"] == 64 * MIB, f"registration {registration}")
    # Control plane internal, data plane external.
    expect(sorted(os.listdir(os.path.join(mount, "oxide-pool"))) == ["slots"], "the volume holds more than slots")
    for name in ("config.json", "ledger.json"):
        expect(os.path.isfile(os.path.join(removable_root, name)), f"internal {name} missing")
    code, stdout, stderr = oxide_run(["pool", "register-removable", mount, "--reserve", "16MiB", "--cap", "64MiB"], "refusal: already registered")
    expect(code != 0 and "already registered" in stderr, "second registration was not refused")

    # Present: selected, flushed before release, warm on repeat.
    code, _, stderr = oxide_run(["check"], "removable present")
    expect(code == 0, "removable check failed")
    line = summary(stderr)
    expect("apfs-volume-v1" in line and f"removable {mount}/oxide-pool" in line and "cold" in line, f"removable not selected: {line}")
    expect("flushed the removable volume before releasing slot 1 in " in stderr, "no flush cost line")
    flushes = removable_state()["flushes"]
    expect(len(flushes) == 1 and flushes[0]["micros"] is not None and flushes[0]["slot_id"] == "1", f"flush not recorded: {flushes}")
    print(f"flush cost: {flushes[0]['micros']} us")
    # oxide#104: slot 1 is `slots/<project folder>/<profile>-1`.
    expect(len(glob.glob(os.path.join(mount, "oxide-pool", "slots", "*", "*-1", "build"))) == 1, "slot 1 is not on the volume")
    code, _, stderr = oxide_run(["check"], "removable repeat")
    line = summary(stderr)
    expect(code == 0 and "slot 1 warm" in line and "apfs-volume-v1" in line, f"repeat not warm on the volume: {line}")
    code, stdout, _ = oxide_run(["status"], "status, removable present")
    expect(code == 0, "status failed")
    for needle in (
        f"removable pool: {mount} volume {registration['volume_uuid']} container {registration['container_uuid']}",
        "reserve 16 MiB, Oxide admission cap 64 MiB",
        "selected for the next build: removable pool",
        "backend apfs-volume-v1",
        "flush before releasing slot 1: ",
        "backend directory-budget-v1",
    ):
        expect(needle in stdout, f"status lacks {needle!r}")
    proof.account("after removable builds")

    # Below the reserve: the registration's reserve is raised past the
    # volume's free space (an owned scratch control file).
    registration_path = os.path.join(control, "removable.json")
    raised = dict(registration, reserve_bytes=IMAGE_MIB * MIB)
    with open(registration_path, "w", encoding="utf-8") as handle:
        json.dump(raised, handle)
    code, _, stderr = oxide_run(["check"], "removable below reserve")
    line = summary(stderr)
    expect(code == 0 and "directory-budget-v1" in line and "not above the 128 MiB reserve" in stderr, f"below-reserve volume was used: {line}")
    with open(registration_path, "w", encoding="utf-8") as handle:
        json.dump(registration, handle)

    # Injected flush failure: the slot is retired and stays held.
    with open(os.path.join(app, "src/lib.rs"), "a", encoding="utf-8") as handle:
        handle.write("pub fn more() -> u32 {\n    8\n}\n")
    code, _, stderr = oxide_run(["check"], "flush failure injected", {"OXIDE_TEST_FAIL_FLUSH": "1"})
    expect(code == 0 and "flush failed for slot 1" in stderr and "slot 1 is retired" in stderr, "injected flush failure did not retire")
    held = [slot for slot in ledger()["slots"] if slot["id"] == 1]
    expect(held and held[0]["state"] == "held", f"retired slot is not held: {held}")
    records = removable_state()["records"]
    expect(any(record["phase"] == "retired" for record in records), f"no retired record: {records}")
    code, _, stderr = oxide_run(["check"], "after retirement")
    line = summary(stderr)
    expect(code == 0 and "slot 2 cold" in line and "apfs-volume-v1" in line, f"retired slot reused or volume lost: {line}")
    code, stdout, _ = oxide_run(["status"], "status, retired slot")
    expect("slot 1 retired and held, never reused" in stdout and "failed (injected flush failure" in stdout, "status lacks the retired slot")
    proof.account("after retirement")

    # Absent: ordinary detach of the idle volume, then fallback; the mount
    # point is never recreated.
    proof.detach()
    code, _, stderr = oxide_run(["check"], "removable absent")
    line = summary(stderr)
    expect(code == 0 and "directory-budget-v1" in line and "removable volume not used" in stderr, f"absent volume selected: {line}")
    expect(not os.path.lexists(mount), f"{mount} was recreated while the volume was absent")
    code, stdout, _ = oxide_run(["status"], "status, removable absent")
    expect(code == 0 and "selected for the next build: internal pool (removable volume not used:" in stdout, "status did not report the absent volume")
    expect(not os.path.lexists(mount), f"status recreated {mount}")

    # Re-attached: selected again, its own warmth kept.
    proof.attach()
    code, _, stderr = oxide_run(["check"], "removable re-attached")
    line = summary(stderr)
    expect(code == 0 and "slot 2 warm" in line and "apfs-volume-v1" in line, f"re-attached volume lost its warmth: {line}")
    proof.account("after re-attach")

    with open(cargo_log, encoding="utf-8") as handle:
        calls = handle.read().splitlines()
    emit("fixture Cargo", "\n".join(calls) + "\n")
    total = len(calls) + 1
    print(f"Cargo processes: {total} (staging 1, fixture {len(calls)})")
    expect(total <= CARGO_LIMIT, f"{total} Cargo processes exceed {CARGO_LIMIT}")
    proof.detach()
    proof.account("after final detach")


if __name__ == "__main__":
    sys.exit(main())
