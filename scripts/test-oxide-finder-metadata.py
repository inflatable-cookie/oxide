#!/usr/bin/env python3
"""Focused proof for Finder metadata in pool accounting (oxide#105).

A short fixed script, not a supervision framework: Effigy heavy admission
gates the start; owned scratch bytes (`du`, the sparse image's allocation
included) are checked before every phase; every child, staging included,
starts only after its deadline (cut from one 300 s budget) and the 20 GiB
host reserve on /private/tmp are checked, runs in its own session, and has
host free space sampled every 250 ms while it runs; a breach, a failed
sample or the deadline kills that session whole. Cleanup is
ordinary `hdiutil detach` and removal of the scratch; anything uncertain
keeps its evidence.

Stages `oxide` offline with 1.97.1 into owned scratch with wrappers
disabled, then over a private HOME, CARGO_HOME and budget-only internal
pool: Finder `.DS_Store` and AppleDouble `._*` files at the pool root,
payload root, `slots`, slot and build tree leave status complete and the
next build warm; then one 128 MiB scratch APFS image, attached at a unique
`/Volumes/<name>`, registered as the removable pool, gets the same files
and stays selected and warm with the bytes counted; finally an unknown
entry in its payload root makes selection fall back, naming the entry, and
nothing is deleted. Not a benchmark and not full QA.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import json
import os
import plistlib
import secrets
import shutil
import signal
import subprocess
import time

BUILDER = "1.97.1"
PROOF_SECONDS = 300
SCRATCH_LIMIT_BYTES = 512 * 1024 * 1024
HOST_RESERVE_BYTES = 20 * 1024 * 1024 * 1024
RESERVE_POLL_SECONDS = 0.25
MIB = 1024 * 1024
STARTED = time.monotonic()
FIXTURE = {
    "Cargo.toml": '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n',
    "src/lib.rs": "pub fn value() -> u32 {\n    7\n}\n",
}
# Exact Finder names, sizes chosen so the MiB status figures move.
METADATA = ((".DS_Store", 2 * MIB), ("._lib.rs", 1 * MIB))
UNKNOWN = "stray\x07entry"


def fail(message: str) -> None:
    raise RuntimeError(message)


def remaining() -> float:
    left = PROOF_SECONDS - (time.monotonic() - STARTED)
    if left <= 5:
        fail(f"proof would exceed {PROOF_SECONDS} s")
    return left


def owned_bytes(root: str) -> int:
    result = subprocess.run(["/usr/bin/du", "-sk", root], capture_output=True, text=True, timeout=min(30, remaining()))
    if result.returncode:
        fail(f"du failed: {result.stderr}")
    return int(result.stdout.split()[0]) * 1024


class Proof:
    def __init__(self, root: str):
        self.root = root
        self.mount = None
        self.peak = 0
        self.low = None
        self.env = dict(os.environ)
        # Child sessions not yet positively closed; while any remains,
        # nothing is detached and the scratch is kept.
        self.unclosed = set()

    def phase(self, label: str) -> None:
        free = shutil.disk_usage("/private/tmp").free
        if free < HOST_RESERVE_BYTES:
            fail(f"host free {free // MIB} MiB is below the 20 GiB reserve at {label}")
        used = owned_bytes(self.root)
        self.peak = max(self.peak, used)
        self.low = free if self.low is None else min(self.low, free)
        if used > SCRATCH_LIMIT_BYTES:
            fail(f"owned scratch {used // MIB} MiB exceeds 512 MiB at {label}")
        print(f"phase {label}: {time.monotonic() - STARTED:.1f} s, owned {used // 1024} KiB, host free {free // MIB} MiB", flush=True)

    def run(self, command: list, *, cwd: str | None = None, env: dict | None = None, timeout: float = 120) -> subprocess.CompletedProcess:
        # The deadline is fixed before the child exists, so a refused
        # deadline never leaves an untracked writer behind.
        deadline = time.monotonic() + min(timeout, remaining())
        if shutil.disk_usage("/private/tmp").free < HOST_RESERVE_BYTES:
            fail(f"host free space is below the 20 GiB reserve before {command[0]}")
        child = subprocess.Popen(
            command,
            cwd=cwd,
            env=env or self.env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        self.unclosed.add(child.pid)
        try:
            out, err = self.watch(child, command[0], deadline)
        except BaseException:
            # Any exception after spawn (KeyboardInterrupt, SIGTERM, a
            # failed sample) closes the child's session before it unwinds.
            self.close(child)
            raise
        self.unclosed.discard(child.pid)
        return subprocess.CompletedProcess(command, child.returncode, out, err)

    def close(self, child: subprocess.Popen) -> None:
        """KILL the child's whole session, then reap its leader within
        10 s. Only a reaped leader counts as closed; otherwise the pid stays
        in `unclosed`. Members that left the session are not covered."""
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            child.communicate(timeout=10)
        except BaseException as error:
            print(f"session {child.pid} not positively closed: {error!r}", file=sys.stderr)
            return
        self.unclosed.discard(child.pid)

    def watch(self, child: subprocess.Popen, name: str, deadline: float) -> tuple:
        # While the child runs, host free space is sampled every 250 ms; a
        # breach, a failed sample or the deadline kills its whole session.
        # communicate() keeps partial output across timeouts.
        while True:
            try:
                out, err = child.communicate(timeout=RESERVE_POLL_SECONDS)
                break
            except subprocess.TimeoutExpired:
                pass
            try:
                free = shutil.disk_usage("/private/tmp").free
                problem = None if free >= HOST_RESERVE_BYTES else f"host free {free // MIB} MiB is below the 20 GiB reserve"
            except OSError as error:
                problem = f"host free space sample failed ({error})"
            if problem is None and time.monotonic() >= deadline:
                problem = "deadline reached"
            if problem is not None:
                fail(f"{name}: {problem}; its session is killed")
            self.low = free if self.low is None else min(self.low, free)
        return out, err

    def oxide(self, args: list, label: str, cwd: str) -> tuple:
        result = self.run([self.binary, *args], cwd=cwd)
        stdout, stderr = result.stdout.decode(errors="replace"), result.stderr.decode(errors="replace")
        print(f"----- {label}: exit {result.returncode} -----\n{stdout}{stderr}", flush=True)
        return result.returncode, stdout, stderr

    def detach(self) -> None:
        if self.unclosed:
            fail(f"sessions {sorted(self.unclosed)} are not positively closed; no detach, evidence kept")
        if self.mount and os.path.ismount(self.mount):
            result = self.run(["/usr/bin/hdiutil", "detach", self.mount], timeout=60)
            if result.returncode:
                fail(f"ordinary detach of {self.mount} failed; evidence kept: {result.stderr!r}")
            print(f"detached {self.mount}")


def expect(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def summary(stderr: str) -> str:
    lines = [line for line in stderr.splitlines() if " profile, " in line and " strategy, slot " in line]
    expect(len(lines) == 1, f"expected one summary line: {stderr}")
    return lines[0]


def payload_mib(stdout: str, marker: str) -> int:
    """The MiB allocated figure on the status line beginning with `marker`."""
    lines = [line for line in stdout.splitlines() if line.strip().startswith(marker) and "MiB allocated by slots" in line]
    expect(len(lines) == 1, f"no {marker!r} accounting line")
    expect(lines[0].endswith("slot accounting Complete"), f"accounting not complete: {lines[0]}")
    return int(lines[0].split(" MiB allocated by slots")[0].split()[-1])


def place_metadata(dirs: list) -> list:
    placed = []
    for directory in dirs:
        for name, size in METADATA:
            path = os.path.join(directory, name)
            with open(path, "wb") as handle:
                handle.write(b"\0" * size)
            placed.append(path)
    return placed


def stage(proof: Proof, repo: str) -> str:
    env = dict(os.environ)
    for name in list(env):
        if name.startswith("SCCACHE_") or name in ("CARGO", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "RUSTC"):
            env.pop(name)
    env.update(
        {
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(proof.root, "stage-target"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_INCREMENTAL": "0",
        }
    )
    result = proof.run(
        ["cargo", "build", "--locked", "--offline", "--bin", "oxide", "--message-format=json-render-diagnostics"],
        cwd=repo,
        env=env,
        timeout=200,
    )
    print(result.stderr.decode(errors="replace")[-4000:])
    expect(result.returncode == 0, f"staging exit {result.returncode}")
    for line in result.stdout.decode(errors="replace").splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") == "compiler-artifact" and message.get("target", {}).get("name") == "oxide" and message.get("executable"):
            return message["executable"]
    fail("staging reported no oxide executable")
    return ""


def run(proof: Proof, repo: str) -> None:
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    name = "OxFm" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    print(f"finder-metadata proof {os.getpid()} source {source} volume {name}")
    expect(not os.path.lexists(mount), f"{mount} already exists; never touched")
    proof.phase("start")
    proof.binary = stage(proof, repo)
    proof.phase("after staging")

    root = proof.root
    pool = os.path.join(root, "pool")
    home = os.path.join(root, "home")
    cargo_home = os.path.join(root, "cargo-home")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    env = dict(os.environ)
    real_rustup = env.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")
    for key in list(env):
        if key.startswith(("SCCACHE_", "OXIDE_", "XDG_")) or key in ("CARGO", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "RUSTC", "RUSTUP_TOOLCHAIN"):
            env.pop(key)
    tmp = os.environ["TMPDIR"]
    env.update(
        {
            "OXIDE_POOL": pool,
            "OXIDE_POOL_BACKEND": "directory-budget",
            "OXIDE_POOL_CAP": "64MiB",
            "HOME": home,
            "CARGO_HOME": cargo_home,
            "RUSTUP_HOME": real_rustup,
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
    app = os.path.join(root, "app")
    for relative, text in FIXTURE.items():
        path = os.path.join(app, relative)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text)
    app = os.path.realpath(app)

    # Internal pool regression: metadata at every level, still complete and warm.
    code, _, stderr = proof.oxide(["check"], "internal cold", app)
    expect(code == 0 and "directory-budget-v1" in summary(stderr) and "slot 1 cold" in summary(stderr), "internal cold check failed")
    code, stdout, _ = proof.oxide(["status"], "internal status before metadata", app)
    before = payload_mib(stdout, "soft budget")
    payload = os.path.join(pool, "payload")
    slot = os.path.join(payload, "slots", "1")
    internal_placed = place_metadata([pool, payload, os.path.join(payload, "slots"), slot, os.path.join(slot, "build")])
    code, stdout, _ = proof.oxide(["status"], "internal status with metadata", app)
    after = payload_mib(stdout, "soft budget")
    expect(code == 0 and "unknown_accounting" not in stdout, "internal status refused metadata")
    # Payload root, slots, slot and build: 4 x 3 MiB; pool-root control bytes are not payload.
    expect(after >= before + 12, f"internal metadata not counted: {before} -> {after} MiB")
    code, _, stderr = proof.oxide(["check"], "internal warm with metadata", app)
    line = summary(stderr)
    expect(code == 0 and "slot 1 warm" in line and "directory-budget-v1" in line, f"internal pool not reused: {line}")
    proof.phase("after internal pool")

    # Removable: one owned scratch APFS image at a unique /Volumes name.
    image = os.path.join(root, "volume.sparseimage")
    created = proof.run(["/usr/bin/hdiutil", "create", "-size", "128m", "-fs", "APFS", "-volname", name, "-type", "SPARSE", image], timeout=60)
    expect(created.returncode == 0, f"image creation failed: {created.stderr!r}")
    attached = proof.run(["/usr/bin/hdiutil", "attach", "-nobrowse", "-plist", image], timeout=60)
    expect(attached.returncode == 0, f"attach failed; attachment unresolved, evidence kept: {attached.stderr!r}")
    mounts = [entity["mount-point"] for entity in plistlib.loads(attached.stdout)["system-entities"] if entity.get("mount-point")]
    expect(len(mounts) == 1, f"expected one mount, got {mounts}")
    proof.mount = mounts[0]
    expect(proof.mount == mount, f"scratch volume mounted at {proof.mount}, not {mount}")
    print(f"attached {image} at {mount}")
    code, stdout, stderr = proof.oxide(["pool", "register-removable", mount, "--reserve", "16MiB", "--cap", "64MiB"], "register", app)
    expect(code == 0 and "registered removable pool" in stdout, "registration failed")
    code, _, stderr = proof.oxide(["check"], "removable cold", app)
    line = summary(stderr)
    expect(code == 0 and "apfs-volume-v1" in line and "slot 1 cold" in line, f"removable not selected: {line}")
    code, stdout, _ = proof.oxide(["status"], "removable status before metadata", app)
    before = payload_mib(stdout, "payload ")
    volume_payload = os.path.join(mount, "oxide-pool")
    volume_slot = os.path.join(volume_payload, "slots", "1")
    removable_control = os.path.join(pool, "oxide-managed", "removable")
    removable_placed = place_metadata(
        [removable_control, volume_payload, os.path.join(volume_payload, "slots"), volume_slot, os.path.join(volume_slot, "build")]
    )
    code, stdout, _ = proof.oxide(["status"], "removable status with metadata", app)
    expect(code == 0 and "selected for the next build: removable pool" in stdout, "metadata disabled the removable pool")
    after = payload_mib(stdout, "payload ")
    expect(after >= before + 12, f"removable metadata not counted: {before} -> {after} MiB")
    code, _, stderr = proof.oxide(["check"], "removable warm with metadata", app)
    line = summary(stderr)
    expect(code == 0 and "slot 1 warm" in line and "apfs-volume-v1" in line and "removable volume not used" not in stderr, f"removable not reused: {line}")
    proof.phase("after removable metadata")

    # An unknown entry still refuses, by name, and nothing is deleted.
    stray = os.path.join(volume_payload, UNKNOWN)
    with open(stray, "wb") as handle:
        handle.write(b"x")
    code, stdout, _ = proof.oxide(["status"], "status with unknown entry", app)
    expect(code == 0 and "selected for the next build: internal pool (removable volume not used:" in stdout, "unknown entry did not refuse selection")
    expect("unknown_accounting" in stdout and 'unexpected entry "stray\\x07entry"' in stdout, "the refusal does not name the entry")
    code, _, stderr = proof.oxide(["check"], "check with unknown entry", app)
    line = summary(stderr)
    expect(code == 0 and "directory-budget-v1" in line and 'unexpected entry "stray\\x07entry"' in stderr, f"fallback did not name the entry: {line}")
    for path in [*internal_placed, *removable_placed, stray]:
        expect(os.path.isfile(path), f"{path} was removed")
    print(f"kept: {len(internal_placed) + len(removable_placed)} metadata files and the unknown entry")
    proof.phase("after unknown entry")
    proof.detach()


def terminated(signum, frame) -> None:
    # SIGTERM unwinds like Ctrl-C, so Proof.run closes its child first.
    raise KeyboardInterrupt(f"signal {signum}")


def main() -> int:
    if sys.platform != "darwin":
        fail("the Finder metadata proof requires macOS")
    made = subprocess.run(["mktemp", "-d", "/private/tmp/oxide-finder.XXXXXX"], capture_output=True, text=True, timeout=30)
    expect(made.returncode == 0, "mktemp failed")
    root = os.path.realpath(made.stdout.strip())
    expect(root.startswith("/private/tmp/oxide-finder."), f"scratch {root} is not under /private/tmp")
    tmp = os.path.join(root, "tmp")
    os.makedirs(tmp, mode=0o700)
    for name in ("OXIDE_TASK_SCRATCH_ROOT", "TMPDIR", "TMP", "TEMP"):
        os.environ[name] = tmp
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    signal.signal(signal.SIGTERM, terminated)
    proof = Proof(root)
    try:
        run(proof, repo)
    except BaseException:
        try:
            proof.detach()
        except BaseException as error:
            print(f"cleanup after failure: {error!r}", file=sys.stderr)
        print(f"finder-metadata proof failed after {time.monotonic() - STARTED:.0f} s; evidence kept at {root}", file=sys.stderr)
        raise
    proof.phase("before removal")
    shutil.rmtree(root)
    elapsed = time.monotonic() - STARTED
    print(
        f"finder-metadata proof held: {elapsed:.0f} s, peak owned {proof.peak // MIB} MiB at phase checks "
        f"(512 MiB budget, checked between phases, not a ceiling), host free minimum {proof.low // MIB} MiB, scratch removed"
    )
    expect(elapsed <= PROOF_SECONDS, f"proof exceeded {PROOF_SECONDS} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
