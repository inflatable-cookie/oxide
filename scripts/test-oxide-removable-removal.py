#!/usr/bin/env python3
"""Focused proof for surprise removal of the removable build volume (oxide#098).

A short fixed script run as the whole command of the qualified shared proof
supervisor (`scripts/bounded-test.py`, oxide#109): 300 s, 512 MiB monitored
scratch, children in its cooperative session, evidence under
OXIDE_PROOF_EVIDENCE. It reuses the oxide#104 script's child runner and
staging; it adds no supervision of its own.

Stages this HEAD's `oxide` offline with 1.97.1. Over a private HOME,
CARGO_HOME and budget-only internal pool, one owned 128 MiB scratch APFS
image at a unique `/Volumes/<name>` is registered as the removable pool.
Surprise removal is `hdiutil detach -force` of that image's own device, bound
by image path, device, mount and volume UUID immediately before, in three
predeclared cases:

- idle: a released, flushed slot survives removal and re-attach warm; the
  build in between falls back to the internal pool;
- held: a build whose build script is blocked in its slot fails cleanly when
  the volume vanishes; its later writes fail rather than land on the internal
  disk; the slot is recorded lost; the next build falls back; `run` and
  `test` use checkout-local outputs; after re-attach the next build deletes
  the lost slot whole and keeps the idle one;
- quarantine: the same with `OXIDE_ALLOW_RUSTC_WRAPPER=1` (a wrapper server
  could outlive the build): after re-attach the slot is kept held with its
  bytes, never reused or deleted.

Final outputs stay in `<checkout>/target` throughout. Cleanup is an ordinary
detach. Not a benchmark and not real-drive qualification.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import importlib.util
import os
import plistlib
import re
import secrets
import signal
import stat
import subprocess
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("pool_layout", os.path.join(HERE, "test-oxide-pool-layout.py"))
layout = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(layout)
runner = layout.runner
expect, fail, summary, MIB = runner.expect, runner.fail, runner.summary, runner.MIB

CANDIDATE_MARKER = b"lost with the removable volume; deleted whole when the volume returns"
WAIT_SECONDS = 60

APP = {
    "Cargo.toml": '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n',
    "src/main.rs": 'fn main() {\n    println!("fixture app ok");\n}\n\n#[test]\nfn works() {\n    assert_eq!(1 + 1, 2);\n}\n',
}

# The build script blocks in its slot until the proof releases it, then
# tries to write again where it wrote before and reports what happened.
BUILD_RS = r'''
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let gate = PathBuf::from(std::env::var("FIXTURE_GATE").unwrap()).join(std::env::var("CARGO_PKG_NAME").unwrap());
    fs::create_dir_all(&gate).unwrap();
    fs::write(out.join("before"), b"written in the slot").unwrap();
    if gate.join("go").exists() {
        return;
    }
    fs::write(gate.join("started"), out.to_string_lossy().as_bytes()).unwrap();
    let start = Instant::now();
    while !gate.join("go").exists() {
        assert!(start.elapsed() < Duration::from_secs(90), "never released");
        std::thread::sleep(Duration::from_millis(50));
    }
    let write = fs::write(out.join("after"), b"late write");
    let create = fs::create_dir_all(out.join("late-dir"));
    let report = format!("write {:?}\ncreate {:?}\n", write.as_ref().err(), create.as_ref().err());
    fs::write(gate.join("report"), report.as_bytes()).unwrap();
    if write.is_err() || create.is_err() {
        panic!("the slot vanished: {report}");
    }
}
'''


def held_fixture(name: str) -> dict:
    return {
        "Cargo.toml": f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\nbuild = "build.rs"\n',
        "build.rs": BUILD_RS,
        "src/main.rs": f'fn main() {{\n    println!("fixture {name} ok");\n}}\n\n#[test]\nfn works() {{\n    assert_eq!(2 + 2, 4);\n}}\n',
    }


class Proof(layout.Proof):
    """The #104 runner plus one background child for the held build. The
    child stays in the supervisor's session and in `unclosed` until reaped."""

    def start(self, args: list, cwd: str, env: dict) -> subprocess.Popen:
        child = subprocess.Popen([self.binary, *args], cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.unclosed.add(child.pid)
        return child

    def finish(self, child: subprocess.Popen, label: str, timeout: float) -> tuple:
        try:
            out, err = child.communicate(timeout=min(timeout, layout.work_left()))
        except subprocess.TimeoutExpired:
            child.kill()
            try:
                child.communicate(timeout=5)
                self.unclosed.discard(child.pid)
            except BaseException as error:
                print(f"child {child.pid} not reaped: {error!r}", file=sys.stderr)
            fail(f"{label}: oxide did not exit within {timeout} s after the removal (hang)")
        self.unclosed.discard(child.pid)
        stdout, stderr = out.decode(errors="replace"), err.decode(errors="replace")
        print(f"----- {label}: exit {child.returncode} -----\n{stdout}{stderr}", flush=True)
        return child.returncode, stdout, stderr


def write_tree(root: str, files: dict) -> str:
    for relative, text in files.items():
        path = os.path.join(root, relative)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text)
    return os.path.realpath(root)


def wait_for(path: str, label: str) -> None:
    deadline = time.monotonic() + min(WAIT_SECONDS, layout.work_left())
    while not os.path.exists(path):
        if time.monotonic() > deadline:
            fail(f"{label}: {path} did not appear")
        time.sleep(0.05)


class Volume:
    """The one owned scratch image: path, root device, mount and UUID."""

    def __init__(self, proof: Proof, image: str, mount: str):
        self.proof, self.image, self.mount = proof, image, mount
        self.device = None
        self.uuid = None

    def attach(self) -> None:
        expect(not os.path.lexists(self.mount), f"{self.mount} exists before attach")
        attached = self.proof.run(["/usr/bin/hdiutil", "attach", "-nobrowse", "-noverify", "-plist", self.image], timeout=60)
        expect(attached.returncode == 0, f"attach failed; attachment unresolved, evidence kept: {attached.stderr!r}")
        entities = plistlib.loads(attached.stdout)["system-entities"]
        mounts = [entity["mount-point"] for entity in entities if entity.get("mount-point")]
        expect(mounts == [self.mount], f"scratch volume mounted at {mounts}, not {self.mount}")
        self.proof.mount = self.mount
        wholes = sorted(
            (entity["dev-entry"] for entity in entities if re.fullmatch(r"/dev/disk\d+", entity.get("dev-entry", ""))),
            key=lambda dev: int(dev[len("/dev/disk"):]),
        )
        expect(bool(wholes), f"no whole-disk device in {entities}")
        self.device = wholes[0]
        info = plistlib.loads(self.proof.run(["/usr/sbin/diskutil", "info", "-plist", self.mount], timeout=30).stdout)
        expect(self.uuid is None or info["VolumeUUID"] == self.uuid, "re-attached volume UUID changed")
        self.uuid = info["VolumeUUID"]
        print(f"attached {self.image} as {self.device} at {self.mount}, volume {self.uuid}")

    def bound(self) -> None:
        """Positive identity immediately before a forced detach: this image
        path owns this device and this mount, and the mount is this UUID."""
        info = plistlib.loads(self.proof.run(["/usr/bin/hdiutil", "info", "-plist"], timeout=30).stdout)
        own = [image for image in info.get("images", []) if os.path.realpath(image.get("image-path", "")) == os.path.realpath(self.image)]
        expect(len(own) == 1, f"image {self.image} attached {len(own)} times")
        devices = {entity.get("dev-entry") for entity in own[0]["system-entities"]}
        mounts = {entity.get("mount-point") for entity in own[0]["system-entities"]} - {None}
        expect(self.device in devices and mounts == {self.mount}, f"device {self.device} / mount {self.mount} not this image's: {devices} {mounts}")
        volume = plistlib.loads(self.proof.run(["/usr/sbin/diskutil", "info", "-plist", self.mount], timeout=30).stdout)
        expect(volume.get("VolumeUUID") == self.uuid, f"{self.mount} is not volume {self.uuid}")

    def force_detach(self, label: str) -> None:
        self.bound()
        result = self.proof.run(["/usr/bin/hdiutil", "detach", "-force", self.device], timeout=60)
        expect(result.returncode == 0, f"{label}: forced detach of {self.device} failed; evidence kept: {result.stderr!r}")
        self.proof.mount = None
        print(f"{label}: forced detach of {self.device} ({self.mount})")
        self.no_spill(label)

    def no_spill(self, label: str) -> None:
        """The mount point is gone or not writable, `/Volumes` stays root
        owned with no group/other write, and a write by this unprivileged
        process fails without creating anything."""
        parent = os.stat("/Volumes")
        expect(parent.st_uid == 0 and not parent.st_mode & (stat.S_IWGRP | stat.S_IWOTH), f"/Volumes is {oct(parent.st_mode)} uid {parent.st_uid}")
        present = os.path.lexists(self.mount)
        if present:
            left = os.lstat(self.mount)
            print(f"{label}: {self.mount} remains: uid {left.st_uid} mode {oct(left.st_mode)}")
            expect(left.st_uid == 0 and not os.access(self.mount, os.W_OK), f"{label}: stale {self.mount} is writable")
        else:
            print(f"{label}: {self.mount} is gone after removal")
        try:
            os.makedirs(os.path.join(self.mount, "oxide-pool", "spill"))
            fail(f"{label}: an unprivileged write recreated {self.mount} on the internal disk")
        except PermissionError as error:
            print(f"{label}: unprivileged write refused: {error}")
        expect(os.path.lexists(self.mount) == present, f"{label}: the mount point changed after a refused write")


def run(proof: Proof, repo: str) -> None:
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    name = "OxRm" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    print(f"removal proof {os.getpid()} source {source} ({layout.source_facts(repo)}) volume {name}")
    expect(not os.path.lexists(mount), f"{mount} already exists; never touched")
    proof.phase("start")
    root = proof.root
    candidate = layout.stage_at(proof, repo, "oxide-candidate")
    print(f"staged candidate sha256 {layout.sha256(candidate)}")
    with open(candidate, "rb") as handle:
        expect(CANDIDATE_MARKER in handle.read(), "candidate binary lacks the #098 lost-volume text")
    proof.binary = candidate
    proof.phase("after staging")

    pool = os.path.join(root, "pool")
    home = os.path.join(root, "home")
    cargo_home = os.path.join(root, "cargo-home")
    gate = os.path.join(root, "gate")
    for path in (home, cargo_home, gate):
        os.makedirs(path, mode=0o700)
    env = dict(os.environ)
    real_rustup = env.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")
    for key in list(env):
        if key in ("OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS"):
            continue
        if key.startswith(("SCCACHE_", "OXIDE_", "XDG_")) or key in ("CARGO", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "RUSTC", "RUSTUP_TOOLCHAIN", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER"):
            env.pop(key)
    tmp = os.environ["TMPDIR"]
    env.update(
        {
            "OXIDE_POOL": pool,
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
            "FIXTURE_GATE": gate,
            "OXIDE_TASK_SCRATCH_ROOT": tmp,
            "TMPDIR": tmp,
            "TMP": tmp,
            "TEMP": tmp,
        }
    )
    proof.env = env
    app = write_tree(os.path.join(root, "app"), APP)
    held = write_tree(os.path.join(root, "held"), held_fixture("held"))
    kept = write_tree(os.path.join(root, "kept"), held_fixture("kept"))

    image = os.path.join(root, "volume.sparseimage")
    created = proof.run(["/usr/bin/hdiutil", "create", "-size", "128m", "-fs", "APFS", "-volname", name, "-type", "SPARSE", image], timeout=60)
    expect(created.returncode == 0, f"image creation failed: {created.stderr!r}")
    volume = Volume(proof, image, mount)
    volume.attach()
    code, stdout, _ = proof.oxide(["pool", "register-removable", mount, "--reserve", "16MiB", "--cap", "64MiB"], "register", app)
    expect(code == 0 and "registered removable pool" in stdout, "registration failed")
    slots = os.path.join(mount, "oxide-pool", "slots")

    # Idle removal: a flushed idle slot is kept warm across forced removal.
    code, _, stderr = proof.oxide(["build"], "removable cold app", app)
    line = summary(stderr)
    expect(code == 0 and "apfs-volume-v1" in line and "slot 1 cold" in line, f"removable not selected: {line}")
    expect("flushed the removable volume before releasing slot 1" in stderr, "no flush before release")
    on_volume = [entry for entry in os.listdir(mount) if not entry.startswith(".")]
    expect(on_volume == ["oxide-pool"], f"volume holds more than the pool: {os.listdir(mount)}")
    app_binary = os.path.join(app, "target", "debug", "app")
    expect(os.path.isfile(app_binary), "default output is not checkout-local while docked")
    app_slot = runner.labelled_slot(slots, 1)
    proof.phase("after idle build")
    volume.force_detach("idle removal")
    ran = proof.run([app_binary], cwd=app, timeout=30)
    expect(ran.returncode == 0 and b"fixture app ok" in ran.stdout, "checkout-local binary does not run after removal")
    code, _, stderr = proof.oxide(["build"], "fallback app (idle removal)", app)
    line = summary(stderr)
    expect(code == 0 and "directory-budget-v1" in line and "removable volume not used" in stderr, f"no internal fallback: {line}")
    volume.no_spill("after fallback build")
    volume.attach()
    code, _, stderr = proof.oxide(["build"], "removable app after re-attach", app)
    line = summary(stderr)
    expect(code == 0 and "apfs-volume-v1" in line and "slot 1 warm" in line, f"idle slot not kept warm: {line}")
    expect(os.path.isdir(app_slot), "idle slot directory gone")
    proof.phase("after idle removal")

    # Held removal: the running build fails cleanly and its slot is lost.
    child = proof.start(["build"], held, env)
    wait_for(os.path.join(gate, "held", "started"), "held build")
    with open(os.path.join(gate, "held", "started"), encoding="utf-8") as handle:
        out_dir = handle.read()
    expect(out_dir.startswith(slots + "/"), f"build script OUT_DIR {out_dir} is not on the volume")
    code, stdout, _ = proof.oxide(["status"], "status, build held", app)
    expect(code == 0 and re.search(r"  slot 2 .*held", stdout) is not None, "held slot not shown held")
    volume.force_detach("held removal")
    with open(os.path.join(gate, "held", "go"), "w", encoding="utf-8") as handle:
        handle.write("go")
    started = time.monotonic()
    code, _, stderr = proof.finish(child, "held build after removal", WAIT_SECONDS)
    print(f"held build exited {time.monotonic() - started:.1f} s after release")
    expect(code != 0, "the build survived the removal")
    expect("slot 2 is lost with the removable volume; deleted whole when the volume returns" in stderr, "lost-volume not recorded")
    expect("slot 2 was lost with the removable volume" in stderr, "no lost-volume summary line")
    with open(os.path.join(gate, "held", "report"), encoding="utf-8") as handle:
        report = handle.read()
    print(f"build script after removal:\n{report}")
    expect("write None" not in report and "create None" not in report, f"a late write succeeded: {report}")
    volume.no_spill("after held build")
    expect(not os.path.lexists(out_dir), "the slot's OUT_DIR exists after removal")
    code, stdout, _ = proof.oxide(["status"], "status, volume gone", app)
    expect(code == 0 and "slot 2 (" + held + "): lost with the removable volume; deleted whole when the volume returns" in stdout, "status does not show the lost slot")
    proof.phase("after held removal")

    # The next commands fall back; run and test use checkout-local outputs.
    code, _, stderr = proof.oxide(["build"], "fallback held", held)
    expect(code == 0 and "directory-budget-v1" in summary(stderr), "held fallback build failed")
    code, stdout, stderr = proof.oxide(["run"], "fallback run held", held)
    expect(code == 0 and "fixture held ok" in stdout and "directory-budget-v1" in summary(stderr), "run after fallback failed")
    expect(os.path.isfile(os.path.join(held, "target", "debug", "held")), "fallback output is not checkout-local")
    code, stdout, stderr = proof.oxide(["test"], "fallback test held", held)
    expect(code == 0 and "test works ... ok" in stdout and "directory-budget-v1" in summary(stderr), "test after fallback failed")
    volume.no_spill("after fallback run/test")

    # Re-attach: the lost slot is deleted whole, the idle one kept.
    volume.attach()
    parts = os.path.relpath(out_dir, slots).split(os.sep)
    lost_slot = os.path.join(slots, parts[0], parts[1])
    print(f"lost slot directory {lost_slot} present after re-attach: {os.path.isdir(lost_slot)}")
    code, _, stderr = proof.oxide(["build"], "removable held after re-attach", held)
    line = summary(stderr)
    expect(code == 0 and "apfs-volume-v1" in line and "slot 2" not in line, f"rebuild after re-attach: {line}")
    expect(re.search(r"deleted slot 2 held when the removable volume was lost \(Cargo group \d+ empty\)", stderr) is not None, "lost slot not deleted")
    expect(not os.path.exists(lost_slot), f"lost slot {lost_slot} still on the volume")
    expect(os.path.isdir(app_slot), "idle slot deleted with the lost one")
    code, stdout, _ = proof.oxide(["status"], "status after re-attach", app)
    expect(code == 0 and "lost with the removable volume" not in stdout, "status still shows a lost slot")
    proof.phase("after held re-attach")

    # Quarantine: a wrapper allowed to run means closure is not positive.
    wrapped = dict(env, OXIDE_ALLOW_RUSTC_WRAPPER="1")
    child = proof.start(["build"], kept, wrapped)
    wait_for(os.path.join(gate, "kept", "started"), "kept build")
    with open(os.path.join(gate, "kept", "started"), encoding="utf-8") as handle:
        kept_out = handle.read()
    volume.force_detach("quarantine removal")
    with open(os.path.join(gate, "kept", "go"), "w", encoding="utf-8") as handle:
        handle.write("go")
    code, _, stderr = proof.finish(child, "kept build after removal", WAIT_SECONDS)
    expect(code != 0, "the kept build survived the removal")
    found = re.search(r"slot (\d+) is quarantined: lost with the removable volume and an inherited rustc wrapper was allowed", stderr)
    expect(found is not None, "quarantine not recorded")
    kept_id = found.group(1)
    volume.attach()
    expect(os.path.isdir(kept_out), "quarantined slot bytes missing after re-attach")
    code, _, stderr = proof.oxide(["build"], "removable kept after re-attach", kept)
    line = summary(stderr)
    expect(code == 0 and "apfs-volume-v1" in line and f"slot {kept_id} " not in line, f"kept rebuild: {line}")
    expect("deleted slot" not in stderr, "a quarantined slot was deleted")
    expect(os.path.isdir(kept_out), "quarantined slot deleted")
    code, stdout, _ = proof.oxide(["status"], "status with quarantine", app)
    expect(code == 0 and re.search(rf"  slot {kept_id} .*quarantined: lost with the removable volume", stdout) is not None, "status does not show quarantine")
    proof.phase("after quarantine")


def main() -> int:
    if sys.platform != "darwin":
        fail("the removal proof requires macOS")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    expect(bool(scratch) and os.path.realpath(scratch) == scratch, f"OXIDE_TASK_SCRATCH_ROOT {scratch!r} is not canonical")
    for name in ("TMPDIR", "TMP", "TEMP"):
        expect(os.environ.get(name) == scratch, f"{name} is not the runner's scratch")
    expect(os.path.isdir(os.environ.get("OXIDE_PROOF_EVIDENCE", "")) and "OXIDE_PROOF_DEADLINE_NS" in os.environ, "not under the qualified runner")
    root = tempfile.mkdtemp(prefix="removal.", dir=scratch)
    repo = os.path.dirname(HERE)
    signal.signal(signal.SIGTERM, layout.terminated)
    signal.signal(signal.SIGALRM, layout.overdue)
    signal.setitimer(signal.ITIMER_REAL, layout.work_left())
    proof = Proof(root)
    try:
        run(proof, repo)
        proof.detach()
    except BaseException as failure:
        try:
            proof.detach()
        except BaseException as error:
            print(f"cleanup after failure: {error!r}", file=sys.stderr)
        layout.record("result.txt", f"failed: {failure!r}\nunclosed children: {sorted(proof.unclosed)}\nmount: {proof.mount}\nroot: {root}\n")
        print(f"removal proof failed after {time.monotonic() - runner.STARTED:.0f} s; scratch {root} left to the runner", file=sys.stderr)
        raise
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
    proof.phase("end")
    elapsed = time.monotonic() - runner.STARTED
    line = (
        f"removal proof held: {elapsed:.0f} s script time, peak owned {proof.peak // MIB} MiB at phase checks, "
        f"host free minimum {proof.low // MIB} MiB; scratch closure and cleanup are the shared runner's"
    )
    layout.record("result.txt", f"passed\n{line}\n")
    print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
