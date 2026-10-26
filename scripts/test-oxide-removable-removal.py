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

`--boot-closure` is the focused mode for oxide#161: a restart ends every
process, so a quarantined lost slot whose holder started before the current
boot session is positively closed. It stages the same candidate and uses the
same owned image, and runs three independent cases over private checkouts:
a quarantined `exec` slot whose override boot time follows its holder start
is reported closed by `status`, an explicit `outputs adopt` of a whole-target
fixture then migrates, and the next admission deletes the slot whole with
`oxide: deleted slot <n> lost before this boot (...)`; a quarantined slot
whose override boot time precedes its holder start stays kept with its
bytes; and an unreadable override stays kept. The one seam is the debug-only
`OXIDE_TEST_BOOT_MS` override, which the product honors only under this
supervisor's own scratch/evidence/deadline layout. The quarantines are made by
the product's own removal path: a blocked `oxide exec` tool loses the volume
under the removal stop, and its release flush fails with the mount gone, so
the lost record names the exec holder (an exec may start processes outside
its group) and is quarantined. No state file is edited.
`--diagnostic` (with `--boot-closure`) is the uncounted collection mode: it
records a bounded `qualified: false` receipt of reached, completed and failed
cases and stops on any outcome it cannot classify. `--self-check` is the pure
apparatus check run before any activation: no child, pool, image or device.
It pins the full fixture environment every boot-closure child (`status`
included) receives, the diagnostic receipt's bounded unqualified `json`
document, the argument dispatch and the product deletion line the candidate
marker quotes.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import hashlib
import importlib.util
import json
import os
import plistlib
import re
import secrets
import shutil
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

# The boot-session closure mode (oxide#161): a marker only a candidate with
# the boot fact carries, the ONE debug-only override that names a boot time,
# and the fixed safety margin the product uses between a holder start and the
# boot session it must precede.
BOOT_MARKER = b"lost before this boot (holder started"
BOOT_HOOK = "OXIDE_TEST_BOOT_MS"
BOOT_MARGIN_MS = 60_000
REMOVABLE_STATE = os.path.join("oxide-managed", "removable", "oxide-managed", "state.json")
DIAGNOSTIC_RECEIPT = "removal-boot-closure.diagnostic"
DIAGNOSTIC_RECEIPT_BYTES = 16 * 1024
BOOT_CASES = (
    ("before-boot", "a quarantined holder from an earlier boot session is deleted whole and its checkout adopts"),
    ("after-boot", "a holder from this boot session stays quarantined"),
    ("unreadable-boot", "an unreadable boot time stays quarantined"),
)
BOOT_APP = {
    "Cargo.toml": '[package]\nname = "bootapp"\nversion = "0.1.0"\nedition = "2021"\n',
    "src/main.rs": 'fn main() {\n    println!("bootapp ok");\n}\n',
}
# A tool that holds its slot until the volume goes away. It ends by itself
# after a bounded wait, so a removal stop that failed to end its group cannot
# leave a writer running into the next case or the supervisor's census.
HOLD_TOOL = """#!/bin/sh
set -eu
echo started > "$FIXTURE_GATE/started"
n=0
while [ "$n" -lt 1200 ]; do
    if [ -e "$FIXTURE_GATE/go" ]; then exit 0; fi
    n=$((n + 1))
    sleep 0.05
done
exit 0
"""

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

    def oxide(self, args: list, label: str, cwd: str, env: dict | None = None) -> tuple:
        """One `oxide` child; `env` names the child's own environment (the
        boot-session mode runs some children with a debug-only override)."""
        result = self.run([self.binary, *args], cwd=cwd, env=env)
        stdout, stderr = result.stdout.decode(errors="replace"), result.stderr.decode(errors="replace")
        print(f"----- {label}: exit {result.returncode} -----\n{stdout}{stderr}", flush=True)
        return result.returncode, stdout, stderr


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

    def __init__(self, proof: Proof, image: str, mount: str, owners: bool = False):
        self.proof, self.image, self.mount = proof, image, mount
        self.owners = owners
        self.device = None
        self.uuid = None

    def attach(self) -> None:
        expect(not os.path.lexists(self.mount), f"{self.mount} exists before attach")
        options = ["-owners", "on"] if self.owners else []
        attached = self.proof.run(["/usr/bin/hdiutil", "attach", *options, "-nobrowse", "-noverify", "-plist", self.image], timeout=60)
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


def late_writes(out_dir: str, label: str) -> None:
    """An unprivileged writer that kept the slot path: a file write and a
    directory creation both fail and create nothing."""
    try:
        with open(os.path.join(out_dir, "after"), "w", encoding="utf-8") as handle:
            handle.write("late")
        fail(f"{label}: a late file write into {out_dir} succeeded")
    except OSError as error:
        print(f"{label}: late file write refused: {error}")
    try:
        os.makedirs(os.path.join(out_dir, "late-dir"))
        fail(f"{label}: a late directory creation under {out_dir} succeeded")
    except OSError as error:
        print(f"{label}: late directory creation refused: {error}")


def private_environment(root: str) -> dict:
    """The private HOME, CARGO_HOME, gate and sanitized command environment
    every fixture child gets; nothing here may reach the operator's account,
    Cargo home, wrapper or target settings."""
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
            "OXIDE_POOL": os.path.join(root, "pool"),
            "OXIDE_POOL_CAP": "64MiB",
            "HOME": home,
            "OXIDE_TEST_HOME_FROM_ENV": "1",
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
    return env


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

    env = private_environment(root)
    gate = env["FIXTURE_GATE"]
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
    expect(re.search(rf"removable volume {re.escape(mount)} is gone; stopped Cargo group \d+: TERM", stderr) is not None, "no removal stop")
    report_path = os.path.join(gate, "held", "report")
    if os.path.exists(report_path):
        with open(report_path, encoding="utf-8") as handle:
            report = handle.read()
        print(f"build script after removal:\n{report}")
        expect("write None" not in report and "create None" not in report, f"a late write succeeded: {report}")
    else:
        print("build script was stopped before its late writes")
    late_writes(out_dir, "held removal")
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
    late_writes(kept_out, "quarantine removal")
    found = re.search(r"slot (\d+) is quarantined: lost with the removable volume and an inherited rustc wrapper was allowed", stderr)
    expect(found is not None, "quarantine not recorded")
    kept_id = found.group(1)
    volume.attach()
    parts = os.path.relpath(kept_out, slots).split(os.sep)
    kept_slot = os.path.join(slots, parts[0], parts[1])
    # A forced detach may not have persisted the slot's last writes; whatever
    # is there must stay.
    kept_present = os.path.isdir(kept_slot)
    print(f"quarantined slot directory {kept_slot} present after re-attach: {kept_present}")
    code, _, stderr = proof.oxide(["build"], "removable kept after re-attach", kept)
    line = summary(stderr)
    expect(code == 0 and "apfs-volume-v1" in line and f"slot {kept_id} " not in line, f"kept rebuild: {line}")
    expect("deleted slot" not in stderr, "a quarantined slot was deleted")
    expect(os.path.isdir(kept_slot) == kept_present, "quarantined slot directory changed")
    code, stdout, _ = proof.oxide(["status"], "status with quarantine", app)
    expect(code == 0 and re.search(rf"  slot {kept_id} .*quarantined: lost with the removable volume", stdout) is not None, "status does not show quarantine")
    proof.phase("after quarantine")


# --- Boot-session closure mode (oxide#161) --------------------------------


class OracleFailure(RuntimeError):
    """A product expectation that did not hold (recoverable in a diagnostic)."""


class UnknownOutcome(RuntimeError):
    """An outcome this proof cannot classify: writers, volume, device or custody."""


def oracle(condition: bool, message: str) -> None:
    if not condition:
        raise OracleFailure(message)


def need(condition: bool, message: str) -> None:
    if not condition:
        raise UnknownOutcome(message)


def read_json(path: str) -> dict:
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def write_tool(path: str) -> str:
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(HOLD_TOOL)
    os.chmod(path, 0o755)
    return os.path.realpath(path)


def git_fixture(proof: Proof, path: str, files: dict) -> str:
    """A tiny crate that is a Git repository already ignoring `/target`, the
    prerequisite a first conversion checks."""
    root = write_tree(path, files)
    proof.run(["git", "init", "-q"], cwd=root, timeout=30)
    with open(os.path.join(root, ".gitignore"), "w", encoding="utf-8") as handle:
        handle.write("/target\n")
    return root


def diagnostic_receipt(outcome: dict) -> str:
    """Write ONE bounded, uniquely named, explicitly unqualified receipt."""
    recorded = {
        **outcome,
        "failed": [[case, message[:400]] for case, message in outcome["failed"][:8]],
    }
    body = json.dumps(recorded, indent=1) + "\n"
    if len(body.encode()) > DIAGNOSTIC_RECEIPT_BYTES:
        recorded = {
            "version": recorded["version"],
            "mode": "diagnostic",
            "qualified": False,
            "incomplete": True,
            "reason": f"the bounded receipt exceeded {DIAGNOSTIC_RECEIPT_BYTES} bytes",
            "completed": recorded["completed"],
            "failed_cases": [case for case, _ in recorded["failed"]],
        }
        body = json.dumps(recorded, indent=1) + "\n"
    name = f"{DIAGNOSTIC_RECEIPT}-{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}-{os.getpid()}.json"
    path = os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], name)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(body)
    return path


class BootClosure:
    """The focused session: one candidate, one owned image, three independent
    cases. Every quarantine is made by the product's own removal stop; no
    state file is edited."""

    def __init__(self, proof: Proof, repo: str):
        self.proof = proof
        self.repo = repo
        self.source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
        dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
        expect(not dirty, f"candidate checkout is not clean: {dirty}")
        self.name = "OxBc" + secrets.token_hex(4)
        self.mount = f"/Volumes/{self.name}"
        expect(not os.path.lexists(self.mount), f"{self.mount} already exists; never touched")
        print(f"boot-closure proof {os.getpid()} source {self.source} ({layout.source_facts(repo)}) volume {self.name}")
        proof.phase("start")
        # Premise 7: the staged candidate's debug capability is what the boot
        # hook needs, and staging inherits these flags. Refuse an inherited
        # value rather than let it change what the candidate is.
        for name in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
            need(
                not os.environ.get(name),
                f"{name} is inherited; the candidate's debug capability is not frozen",
            )
        candidate = layout.stage_at(proof, repo, "oxide-boot-closure")
        print(f"staged candidate sha256 {layout.sha256(candidate)}")
        with open(candidate, "rb") as handle:
            need(BOOT_MARKER in handle.read(), "the candidate binary lacks the boot-session closure text")
        proof.binary = candidate
        self.env = private_environment(proof.root)
        proof.env = self.env
        self.pool = self.env["OXIDE_POOL"]
        self.tool = write_tool(os.path.join(proof.root, "hold-tool"))
        self.victims = {}
        for case, _ in BOOT_CASES:
            victim = git_fixture(proof, os.path.join(proof.root, f"victim-{case}"), BOOT_APP)
            # A nonempty real target keeps this checkout unconverted, so only
            # the normal pool path runs there (automatic first conversion is
            # the separate oxide#121 feature, not this proof's subject).
            os.makedirs(os.path.join(victim, "target"), exist_ok=True)
            with open(os.path.join(victim, "target", "keep.txt"), "wb") as handle:
                handle.write(b"checkout-local output")
            self.victims[case] = victim
        # The whole-target fixture: a Git checkout ignoring `/target` with an
        # empty real target, which only an explicit `outputs adopt` converts.
        self.whole = git_fixture(proof, os.path.join(proof.root, "whole"), BOOT_APP)
        os.makedirs(os.path.join(self.whole, "target"), exist_ok=True)
        proof.phase("after staging")

        image = os.path.join(proof.root, "volume.sparseimage")
        created = proof.run(
            ["/usr/bin/hdiutil", "create", "-size", "128m", "-fs", "APFS", "-volname", self.name, "-type", "SPARSE", image],
            timeout=60,
        )
        need(created.returncode == 0, f"image creation failed: {created.stderr!r}")
        # `-owners on` on every attach: the product judges the mounted root's
        # own ownership and mode, and a mount that ignores ownership would
        # make that judgement meaningless.
        self.volume = Volume(proof, image, self.mount, owners=True)
        self.volume.attach()
        code, stdout, stderr = proof.oxide(
            ["pool", "register-removable", self.mount, "--reserve", "16MiB", "--cap", "64MiB", "--mode", "permanent"],
            "register permanent",
            self.whole,
        )
        need(code == 0 and "registered removable pool" in stdout, f"registration failed: {stderr[-800:]}")
        self.slots = os.path.join(self.mount, "oxide-pool", "slots")
        proof.phase("after registration")

    def child_env(self, extra: dict) -> dict:
        """The full fixture environment plus one child's own controls. Every
        boot-closure child — `status` included — starts here, so no call can
        hand a child only the boot override."""
        return dict(self.env, **extra)

    def state(self) -> dict:
        path = os.path.join(self.pool, REMOVABLE_STATE)
        need(os.path.isfile(path), f"no removable pool state at {path}; custody unknown")
        return read_json(path)

    def lost_record(self, slot: str) -> dict:
        for record in self.state().get("records", []):
            if str((record.get("lost") or {}).get("slot_id")) == slot:
                return record
        raise UnknownOutcome(f"no state record for lost slot {slot}; custody unknown")

    def status(self, label: str, cwd: str, extra: dict) -> str:
        code, stdout, stderr = self.proof.oxide(["status"], label, cwd, self.child_env(extra))
        need(code == 0, f"{label}: status exit {code}: {stderr[-800:]}")
        return stdout

    def quarantine(self, case: str) -> dict:
        """Hold a removable slot with `oxide exec`, force the volume away, and
        return the product's own quarantined record facts."""
        env = self.child_env({"FIXTURE_GATE": os.path.join(self.proof.root, "gate", case)})
        os.makedirs(env["FIXTURE_GATE"], mode=0o700)
        victim = self.victims[case]
        before = set(os.listdir(self.slots))
        child = self.proof.start(["exec", "--", self.tool], victim, env)
        wait_for(os.path.join(env["FIXTURE_GATE"], "started"), f"{case} exec")
        self.volume.force_detach(f"{case} removal")
        code, _, stderr = self.proof.finish(child, f"{case} exec after removal", WAIT_SECONDS)
        # A surviving exec, or a removal without the product's own quarantine
        # record, leaves the slot's custody unknown: no later case may assume
        # this pool is in a state the proof can classify.
        need(code != 0, f"{case}: the exec survived the removal; custody unknown")
        found = re.search(
            r"slot (\d+) is quarantined: lost with the removable volume and .+; kept with its bytes, never reused or deleted",
            stderr,
        )
        need(found is not None, f"{case}: the quarantine was not recorded: {stderr[-1200:]}")
        slot = found.group(1)
        record = self.lost_record(slot)
        command = str((record.get("holder") or {}).get("command") or "")
        need(command.startswith("exec "), f"{case}: the quarantined record is not an exec holder: {command!r}")
        need(
            bool((record.get("lost") or {}).get("unclosed")),
            f"{case}: the lost record carries no unclosed reason",
        )
        self.volume.attach()
        after = set(os.listdir(self.slots))
        new = sorted(after - before)
        need(len(new) == 1, f"{case}: {len(new)} new slot directories appeared: {new}")
        directory = os.path.join(self.slots, new[0])
        need(os.path.isdir(directory), f"{case}: {directory} is not a new slot directory")
        started = int((record.get("holder") or {}).get("started_ms") or 0)
        need(started > 0, f"{case}: the recorded holder start is {started}")
        print(
            f"{case}: quarantined slot {slot} in {directory}, holder started {started} ms, "
            f"state record {record.get('id')}"
        )
        return {"case": case, "slot": slot, "directory": directory, "started": started, "victim": victim}

    def kept(self, found: dict, extra: dict) -> None:
        """The slot stays held with its bytes: no deletion, the directory and
        the record unchanged, and status still reports the quarantine."""
        case, slot, directory, victim = found["case"], found["slot"], found["directory"], found["victim"]
        stdout = self.status(f"{case} status", victim, extra)
        oracle(
            re.search(
                rf"slot {slot} retired and held, never reused \(quarantined: lost with the removable volume and .+; kept with its bytes, never reused or deleted",
                stdout,
            )
            is not None,
            f"{case}: status does not report the quarantine: {stdout[-1200:]}",
        )
        code, _, stderr = self.proof.oxide(["build"], f"{case} admission", victim, self.child_env(extra))
        oracle(code == 0, f"{case}: the admission failed: {stderr[-1200:]}")
        oracle("deleted slot" not in stderr, f"{case}: a quarantined slot was deleted: {stderr[-1200:]}")
        oracle(os.path.isdir(directory), f"{case}: {directory} was removed")
        self.lost_record(slot)
        print(f"{case}: slot {slot} kept at {directory} with its record")

    def before_boot(self) -> None:
        found = self.quarantine("before-boot")
        slot, directory, victim = found["slot"], found["directory"], found["victim"]
        # A later boot session: the holder started more than the safety
        # margin before this one. The proof cannot reboot the host, so it
        # names the session through the debug-only override.
        override = {BOOT_HOOK: str(found["started"] + 2 * BOOT_MARGIN_MS)}
        stdout = self.status("before-boot status", victim, override)
        oracle(
            re.search(rf"slot {slot} retired and held, never reused \(.*predates this boot session", stdout) is not None,
            f"before-boot: status does not report the restart closure: {stdout[-1200:]}",
        )
        oracle(os.path.isdir(directory), "before-boot: the quarantined slot is already gone")
        # The observed defect: any whole-target checkout's explicit conversion
        # refused on the quarantined record in the paired pool.
        code, stdout, stderr = self.proof.oxide(
            ["outputs", "adopt"], "before-boot adopt", self.whole, self.child_env(override)
        )
        oracle(
            code == 0 and "now selects" in stdout,
            f"before-boot: adopt refused or did not migrate: {code} {stdout[-800:]} {stderr[-800:]}",
        )
        # The conversion this head publishes for an empty whole-target
        # checkout is the profile layout: `target` stays a real directory and
        # the host members become owned links, with one committed receipt.
        target = os.path.join(self.whole, "target")
        oracle(
            os.path.isdir(target) and not os.path.islink(target),
            "before-boot: the whole target directory did not stay real",
        )
        for member in ("debug", "release"):
            oracle(
                os.path.islink(os.path.join(target, member)),
                f"before-boot: the {member} member was not converted to an owned link",
            )
        key = hashlib.sha256(self.whole.encode()).hexdigest()
        receipt = os.path.join(self.pool, "oxide-managed", "outputs", key + ".json")
        oracle(os.path.isfile(receipt), f"before-boot: no committed output receipt at {receipt}")
        # The next admission releases and deletes the slot whole.
        code, _, stderr = self.proof.oxide(
            ["build"], "before-boot deleting admission", victim, self.child_env(override)
        )
        oracle(code == 0, f"before-boot: the deleting admission failed: {stderr[-1200:]}")
        oracle(
            re.search(
                rf"deleted slot {slot} lost before this boot \(holder started \d+ s before boot; a restart ends every process\)",
                stderr,
            )
            is not None,
            f"before-boot: the boot-closed slot was not deleted: {stderr[-1200:]}",
        )
        oracle(not os.path.exists(directory), f"before-boot: {directory} still exists after deletion")
        oracle(
            not any(
                str((record.get("lost") or {}).get("slot_id")) == slot
                for record in self.state().get("records", [])
            ),
            "before-boot: the deleted slot's record is still in state",
        )
        stdout = self.status("before-boot status after", victim, override)
        oracle(
            re.search(rf"slot {slot} retired and held", stdout) is None,
            "before-boot: status still lists the deleted slot",
        )

    def after_boot(self) -> None:
        found = self.quarantine("after-boot")
        # A boot session that began before the holder: this session's writers
        # may still run, so the record keeps today's behaviour exactly.
        self.kept(found, {BOOT_HOOK: str(found["started"] - BOOT_MARGIN_MS)})

    def unreadable_boot(self) -> None:
        found = self.quarantine("unreadable-boot")
        # A present but unusable override is an unknown boot time, never the
        # real one and never closure.
        self.kept(found, {BOOT_HOOK: "unreadable"})


def boot_closure(proof: Proof, repo: str, diagnostic: bool) -> str:
    """Run the three cases; return the one result line. The diagnostic mode
    records recoverable failures and stops on an outcome it cannot classify."""
    outcome = {
        "version": "oxide.removal.boot-closure-diagnostic/1",
        "mode": "diagnostic",
        "qualified": False,
        "claims": "diagnostic case coverage only; never a counted native PASS, closure, census or resource claim",
        "cases": [case for case, _ in BOOT_CASES],
        "purposes": dict(BOOT_CASES),
        "source": None,
        "limits": {"seconds": 300, "scratch_mib": 512, "image_mib": 128, "boot_margin_ms": BOOT_MARGIN_MS},
        "reached": [],
        "completed": [],
        "failed": [],
        "first_fault": None,
        "aborted": None,
        "unexecuted": [],
        "root": proof.root,
    }
    session = None
    try:
        session = BootClosure(proof, repo)
        outcome["source"] = session.source
        outcome["manifest_lock"] = layout.source_facts(repo)
        outcome["mount"] = session.mount
        outcome["device"] = session.volume.device
        outcome["volume_uuid"] = session.volume.uuid
        for case, _ in BOOT_CASES:
            outcome["reached"].append(case)
            try:
                getattr(session, case.replace("-", "_"))()
            except OracleFailure as failure:
                outcome["failed"].append([case, str(failure)])
                outcome["first_fault"] = outcome["first_fault"] or f"{case}: {failure}"
                print(f"{case}: FAILED {failure}", file=sys.stderr)
                if not diagnostic:
                    raise
                continue
            except UnknownOutcome as failure:
                outcome["aborted"] = f"{case}: {failure}"
                raise
            outcome["completed"].append(case)
            proof.phase(f"after {case}")
    finally:
        if diagnostic:
            outcome["unexecuted"] = [case for case, _ in BOOT_CASES if case not in outcome["reached"]]
            outcome["unclosed"] = sorted(proof.unclosed)
            outcome["mount"] = proof.mount
            path = diagnostic_receipt(outcome)
            print(f"boot-closure diagnostic receipt {path}")
    elapsed = time.monotonic() - runner.STARTED
    if diagnostic or outcome["failed"]:
        line = (
            f"boot-closure diagnostic collected (UNCOUNTED, never a PASS): "
            f"{len(outcome['completed'])}/{len(BOOT_CASES)} cases completed, failed "
            f"{[case for case, _ in outcome['failed']]}, unexecuted {outcome['unexecuted']}; "
            f"{elapsed:.0f} s script time, peak owned {proof.peak // MIB} MiB at phase checks, "
            f"host free minimum {proof.low // MIB} MiB; one owned 128 MiB scratch image, "
            f"ordinary detach; receipt {path if diagnostic else 'none'}"
        )
    else:
        line = (
            f"boot-closure proof held: {elapsed:.0f} s script time, peak owned "
            f"{proof.peak // MIB} MiB at phase checks, host free minimum {proof.low // MIB} MiB; "
            f"one owned 128 MiB scratch image, ordinary detach"
        )
    layout.record("result.txt", f"{'diagnostic (not a PASS)' if diagnostic else 'passed'}\n{line}\n")
    print(line)
    return line


USAGE = "usage: test-oxide-removable-removal.py [--boot-closure [--diagnostic]] | --self-check"


def mode_from_args(arguments: list) -> tuple | None:
    """`(focused, diagnostic)` for a fixed argument list, or `None` when the
    request is not one this script defines."""
    focused = "--boot-closure" in arguments
    diagnostic = "--diagnostic" in arguments
    if (focused, diagnostic) not in ((False, False), (True, False), (True, True)):
        return None
    if not focused and arguments:
        return None
    if focused and sorted(arguments) != sorted(["--boot-closure"] + (["--diagnostic"] if diagnostic else [])):
        return None
    return focused, diagnostic


def self_check() -> int:
    """Pure apparatus check for the boot-closure mode: no child, pool, image,
    device or supervisor. It pins the two defects an exact-head review found
    before any native activation — the diagnostic receipt's `json` dependency
    and the full fixture environment every child (`status` included) receives
    — plus the dispatch, the product-message marker and the receipt bound."""
    fixture = {
        "OXIDE_POOL": "/fixture/pool",
        "HOME": "/fixture/home",
        "CARGO_HOME": "/fixture/cargo-home",
        "OXIDE_TEST_HOME_FROM_ENV": "1",
        "OXIDE_TASK_SCRATCH_ROOT": "/fixture/scratch",
        "TMPDIR": "/fixture/scratch",
        "FIXTURE_GATE": "/fixture/gate",
    }
    override = {BOOT_HOOK: "1900000000000"}
    quarantine = (
        "  slot 7 retired and held, never reused (quarantined: lost with the removable volume and "
        "exec /fixture/tool may have started processes outside its group; kept with its bytes, "
        "never reused or deleted) project victim /fixture/victim 12 MiB allocated by slots, "
        "slots/victim-7/quick-7"
    )
    calls = []

    class Recorder:
        root = "/fixture/root"

        def oxide(self, args, label, cwd, env=None):
            calls.append((list(args), dict(env or {})))
            if args == ["status"]:
                return 0, quarantine, ""
            return 0, "", "oxide: fixture build summary"

    root = tempfile.mkdtemp(prefix="boot-closure-self-check.")
    try:
        case = BootClosure.__new__(BootClosure)
        case.env = dict(fixture)
        case.proof = Recorder()
        case.lost_record = lambda slot: {"holder": {"command": "exec /fixture/tool"}}
        slot_dir = os.path.join(root, "slots", "victim-7")
        os.makedirs(slot_dir)
        # status(): one child, the whole fixture environment plus the override.
        case.status("self-check status", "/fixture/victim", override)
        expect(len(calls) == 1, f"status launched {len(calls)} children")
        expect(calls[0][0] == ["status"], calls[0][0])
        expect(
            calls[0][1] == case.child_env(override),
            f"status child env {sorted(calls[0][1])}",
        )
        for name in fixture:
            expect(name in calls[0][1], f"status child lost {name}")
        expect(
            calls[0][1][BOOT_HOOK] == override[BOOT_HOOK],
            "status child lost the override",
        )
        # kept(): its status and build children carry the same full
        # environment, and its oracles pass on the canned product output.
        case.kept(
            {
                "case": "self-check",
                "slot": "7",
                "directory": slot_dir,
                "started": 1,
                "victim": "/fixture/victim",
            },
            override,
        )
        expect(
            [args for args, _ in calls] == [["status"], ["status"], ["build"]],
            calls,
        )
        for args, env in calls:
            expect(
                env == case.child_env(override),
                f"{args} child env {sorted(env)}",
            )
        # The diagnostic receipt uses json, stays bounded and is never
        # qualified; an oversized body is honestly marked incomplete.
        holder = dict(os.environ)
        os.environ["OXIDE_PROOF_EVIDENCE"] = root
        try:
            path = diagnostic_receipt(
                {
                    "version": "oxide.removal.boot-closure-diagnostic/1",
                    "mode": "diagnostic",
                    "qualified": False,
                    "completed": ["before-boot"],
                    "failed": [["after-boot", "x" * 4000]],
                    "reached": ["before-boot", "after-boot"],
                    "unexecuted": [],
                }
            )
            with open(path, "rb") as handle:
                body = handle.read()
            receipt = json.loads(body)
            expect(
                receipt["qualified"] is False and receipt["mode"] == "diagnostic",
                f"diagnostic receipt {receipt.get('mode')}/{receipt.get('qualified')}",
            )
            expect(len(body) <= DIAGNOSTIC_RECEIPT_BYTES, f"receipt {len(body)} bytes")
            path = diagnostic_receipt(
                {
                    "version": "v",
                    "mode": "diagnostic",
                    "qualified": False,
                    "completed": [],
                    "failed": [],
                    "unexecuted": [],
                    "huge": "z" * DIAGNOSTIC_RECEIPT_BYTES,
                }
            )
            with open(path, "rb") as handle:
                body = handle.read()
            receipt = json.loads(body)
            expect(
                receipt.get("incomplete") is True
                and len(body) <= DIAGNOSTIC_RECEIPT_BYTES,
                f"oversized receipt {len(body)} bytes",
            )
        finally:
            os.environ.clear()
            os.environ.update(holder)
    finally:
        shutil.rmtree(root, ignore_errors=True)
    # Dispatch: only the documented argument lists select a mode.
    expect(mode_from_args([]) == (False, False), "no-argument mode")
    expect(mode_from_args(["--boot-closure"]) == (True, False), "focused mode")
    expect(
        mode_from_args(["--boot-closure", "--diagnostic"]) == (True, True),
        "diagnostic mode",
    )
    expect(
        mode_from_args(["--diagnostic", "--boot-closure"]) == (True, True),
        "diagnostic mode, reversed",
    )
    for refused in (
        ["--diagnostic"],
        ["--bogus"],
        ["--self-check", "--boot-closure"],
        ["--boot-closure", "--diagnostic", "--diagnostic"],
    ):
        expect(mode_from_args(refused) is None, f"{refused} selected a mode")
    # The candidate marker is the product's own deletion line, as documented.
    expect(
        BOOT_MARKER
        in b"oxide: deleted slot 2 lost before this boot (holder started 3 s before boot; a restart ends every process)",
        "BOOT_MARKER is not the product's boot-session line",
    )
    print(
        "boot-closure self-check ok: status/build child environments, diagnostic "
        "receipt (json, bounded, unqualified), dispatch, marker"
    )
    return 0


def main() -> int:
    arguments = list(sys.argv[1:])
    if arguments == ["--self-check"]:
        return self_check()
    mode = mode_from_args(arguments)
    if mode is None:
        print(USAGE, file=sys.stderr)
        return 2
    focused, diagnostic = mode
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
        if focused:
            boot_closure(proof, repo, diagnostic)
        else:
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
    if focused:
        # The mode already recorded its own result line and, for a
        # diagnostic, its unqualified receipt.
        return 0
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
