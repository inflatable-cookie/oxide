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
"""

from __future__ import annotations

import sys

# No bytecode files in the counted tree or the repository.
sys.dont_write_bytecode = True

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
    GUARDIAN = br.Guardian(STARTED, parent, br.Walker([parent]), work_seconds=240, total_seconds=PROOF_SECONDS)
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
    walks, peak, low = GUARDIAN.walk_count, GUARDIAN.walk_max, GUARDIAN.reserve_min
    GUARDIAN.shutdown()
    shutil.rmtree(parent)
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
    for name in list(env):
        if name.startswith("SCCACHE_") or name.startswith("OXIDE_") or name in (
            "CARGO", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "RUSTC", "RUSTUP_TOOLCHAIN"
        ):
            env.pop(name)
    env.update(
        {
            "OXIDE_POOL": pool,
            "OXIDE_POOL_BACKEND": "directory-budget",
            "OXIDE_POOL_CAP": "64MiB",
            "CARGO_HOME": cargo_home,
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
    expect(os.path.isdir(os.path.join(mount, "oxide-pool", "slots", "1", "build")), "slot 1 is not on the volume")
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
