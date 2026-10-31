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
same owned image, and runs four independent cases over private checkouts:
a quarantined `exec` slot whose override boot time follows its holder start
is reported closed by `status`, an explicit `outputs adopt` of a whole-target
fixture then migrates, and the next admission deletes the slot whole with
`oxide: deleted slot <n> lost before this boot (...)`; a quarantined slot
whose override boot time precedes its holder start stays kept with its
bytes; an unreadable override stays kept; and an operator-confirmed
`oxide pool release-quarantined <slot> --confirm-writers-stopped`
(oxide#196) refuses while an escaped process keeps a file open inside the
slot, then -- once that process is stopped -- a dry run is clean and the
release deletes the slot whole through the holding area and forgets its
record. The escaped writer is the `oxide exec` tool's own forked child: it
leaves the product's exec group by joining the supervisor's leader process
group (never a setsid escape), holds a file in its slot scratch, and is
confirmed in that group before the removal; the product's stop cannot end it,
the supervisor can census and signal it, and the proof stops it in a
`finally`. The one product seam is the debug-only
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
included) receives, the deletion oracle's slot-leaf resolution (the slot's
own `<profile>-<id>` leaf, never the project-label folder a build or an
`outputs adopt` also creates or reuses), the diagnostic receipt's bounded
unqualified `json` document, the argument dispatch and the product deletion
line the candidate marker quotes.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import glob
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
    ("release-quarantined", "an operator-confirmed release refuses while an escaped writer holds the slot, then deletes it whole once that writer is stopped"),
)
# The escaped exec writer (oxide#196): the `oxide exec` tool forks a child
# that leaves the product's exec group — the removal stop signals that group —
# by joining the qualified supervisor's own leader process group
# (`FIXTURE_LEADER_PGID`, this proof's group). It therefore stays inside the
# supervisor's custody for its census and closure while the product's stop
# cannot end it. It holds a file in its slot scratch until the proof stops it.
ESCAPE_TOOL = r"""#!/usr/bin/env python3
import os, sys, time
gate = os.environ["FIXTURE_GATE"]
scratch = os.environ["TMPDIR"]
leader = int(os.environ["FIXTURE_LEADER_PGID"])
with open(os.path.join(gate, "started"), "w", encoding="utf-8") as handle:
    handle.write(scratch + "\n")
if os.fork() == 0:
    try:
        os.setpgid(0, leader)
    except OSError as error:
        with open(os.path.join(gate, "escaped-error"), "w", encoding="utf-8") as handle:
            handle.write(str(error))
        os._exit(9)
    with open(os.path.join(gate, "escaped"), "w", encoding="utf-8") as handle:
        handle.write(str(os.getpid()) + "\n")
    held = None
    fresh = False
    stop = os.path.join(gate, "stop")
    end = time.monotonic() + 240.0
    while time.monotonic() < end and not os.path.exists(stop):
        if not fresh:
            if held is None:
                try:
                    held = open(os.path.join(scratch, "escaped-writer"), "a")
                except OSError:
                    pass
            try:
                os.stat(scratch)
            except OSError:
                pass
            else:
                # The volume is back: replace the pre-detach handle with one
                # on the re-attached tree, then mark it ready.
                if held is not None:
                    held.close()
                held = open(os.path.join(scratch, "escaped-writer"), "a")
                fresh = True
                with open(os.path.join(gate, "escaped-ready"), "w", encoding="utf-8") as marker:
                    marker.write("ready\n")
        time.sleep(0.05)
    if held is not None:
        held.close()
    os._exit(0)
while True:
    time.sleep(0.5)
"""
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


def slot_leaf_directory(slots: str, slot: str) -> str:
    """The slot's own directory, `slots/<project folder>/<profile>-<id>`
    (`split_slot_dir` in `src/lease/pool.rs`), resolved by the product's
    recorded slot id across the whole slots tree. `slots/<folder>` is only
    the project label a build, an adopt or a relink also creates or reuses,
    so it is never what a lost-slot oracle names."""
    leaves = sorted(glob.glob(os.path.join(glob.escape(slots), "*", f"*-{slot}")))
    need(len(leaves) == 1, f"slot {slot} has {len(leaves)} slot leaves under {slots}: {leaves}")
    leaf = leaves[0]
    need(
        os.path.dirname(os.path.dirname(leaf)) == slots,
        f"slot {slot} leaf {leaf} is not one level below a project folder in {slots}",
    )
    return leaf


def deletion_oracle_leaf(slots: str, slot: str, folder: str) -> str:
    """The exact path a lost-slot deletion oracle names: the slot's own leaf,
    required to sit in the project folder just discovered and to be distinct
    from it. Every fact is re-derived from the recorded id alone, so no
    build output or project directory can satisfy or fail the oracle."""
    leaf = slot_leaf_directory(slots, slot)
    need(
        os.path.dirname(leaf) == folder,
        f"slot {slot} leaf {leaf} is not in the project folder {folder}",
    )
    need(
        os.path.realpath(leaf) != os.path.realpath(folder),
        f"slot {slot} resolved to the project-label directory {folder}",
    )
    return leaf


def write_script(path: str, text: str) -> str:
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)
    os.chmod(path, 0o755)
    return os.path.realpath(path)


def process_refusal(pid: int) -> str:
    """The census message names a writer as `process <pid> ...`; the old
    `pid <pid>` form never matches it."""
    return f"process {pid}"


def process_alive(pid: int) -> bool:
    """Whether a pid this proof started is still running; a permission error
    is not proof it is gone."""
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


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
        self.tool = write_script(os.path.join(proof.root, "hold-tool"), HOLD_TOOL)
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

    def ledger(self) -> dict:
        path = os.path.join(self.pool, "oxide-managed", "removable", "ledger.json")
        need(os.path.isfile(path), f"no removable pool ledger at {path}; custody unknown")
        return read_json(path)

    def custody_digest(self) -> str:
        """A bounded digest of the removable pool's state and ledger, so a
        refusal can be shown to change nothing."""
        digest = hashlib.sha256()
        for path in (
            os.path.join(self.pool, REMOVABLE_STATE),
            os.path.join(self.pool, "oxide-managed", "removable", "ledger.json"),
        ):
            need(os.path.isfile(path), f"no custody document at {path}")
            with open(path, "rb") as handle:
                digest.update(handle.read())
        return digest.hexdigest()

    def stop_escaped(self, gate: str, pid: int) -> None:
        """Stop the escaped exec writer and wait for it to be gone; a writer
        this proof cannot positively end leaves custody unknown."""
        try:
            with open(os.path.join(gate, "stop"), "w", encoding="utf-8") as handle:
                handle.write("stop\n")
        except OSError as error:
            raise UnknownOutcome(f"cannot write the escaped writer's stop file: {error}")
        end = time.monotonic() + 10.0
        while time.monotonic() < end:
            if not process_alive(pid):
                return
            time.sleep(0.05)
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            return
        end = time.monotonic() + 5.0
        while time.monotonic() < end:
            if not process_alive(pid):
                return
            time.sleep(0.05)
        raise UnknownOutcome(f"the escaped writer pid {pid} did not stop; custody unknown")

    def quarantine(self, case: str, escaped: bool = False) -> dict:
        """Hold a removable slot with `oxide exec`, force the volume away, and
        return the product's own quarantined record facts. With `escaped`, the
        exec tool forks a child that leaves the product's exec group by joining
        this proof's (supervisor leader) process group, holding a file in its
        slot scratch until the case stops it."""
        env = self.child_env({"FIXTURE_GATE": os.path.join(self.proof.root, "gate", case)})
        if escaped:
            env["FIXTURE_LEADER_PGID"] = str(os.getpgrp())
        os.makedirs(env["FIXTURE_GATE"], mode=0o700)
        victim = self.victims[case]
        before = set(os.listdir(self.slots))
        tool = self.escape_tool if escaped else self.tool
        child = self.proof.start(["exec", "--", tool], victim, env)
        wait_for(os.path.join(env["FIXTURE_GATE"], "started"), f"{case} exec")
        escaped_pid = None
        if escaped:
            with open(os.path.join(env["FIXTURE_GATE"], "started"), encoding="utf-8") as handle:
                scratch = handle.read().strip()
            need(
                scratch.startswith(self.slots + os.sep),
                f"{case}: the exec tool's TMPDIR {scratch!r} is not inside the slot tree",
            )
            deadline = time.monotonic() + WAIT_SECONDS
            marker = os.path.join(env["FIXTURE_GATE"], "escaped")
            while not os.path.isfile(marker):
                error_path = os.path.join(env["FIXTURE_GATE"], "escaped-error")
                if os.path.isfile(error_path):
                    with open(error_path, encoding="utf-8") as handle:
                        raise UnknownOutcome(
                            f"{case}: the escaped child could not join the supervisor's leader group: {handle.read()[:300]}"
                        )
                need(
                    time.monotonic() < deadline,
                    f"{case}: the escaped exec child did not publish its pid",
                )
                time.sleep(0.01)
            with open(marker, encoding="utf-8") as handle:
                escaped_pid = int(handle.read().strip())
            need(
                os.getpgid(escaped_pid) == os.getpgrp(),
                f"{case}: escaped writer pid {escaped_pid} is not in the supervisor's leader group",
            )
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
        folder = os.path.join(self.slots, new[0])
        need(os.path.isdir(folder), f"{case}: {folder} is not a new project folder")
        # `slots/<folder>` is the project-label directory, not the slot: the
        # slot is the single `<profile>-<id>` leaf inside it. Resolve that
        # leaf from the product's recorded id here, before any admission of
        # this case runs, so the deletion oracle names the slot itself.
        directory = deletion_oracle_leaf(self.slots, slot, folder)
        need(os.path.isdir(directory), f"{case}: slot leaf {directory} is not a directory")
        started = int((record.get("holder") or {}).get("started_ms") or 0)
        need(started > 0, f"{case}: the recorded holder start is {started}")
        print(
            f"{case}: quarantined slot {slot} leaf {directory} in project folder {folder}, "
            f"holder started {started} ms, state record {record.get('id')}"
        )
        found = {
            "case": case,
            "slot": slot,
            "folder": folder,
            "directory": directory,
            "started": started,
            "victim": victim,
            "gate": env["FIXTURE_GATE"],
        }
        if escaped:
            wait_for(
                os.path.join(env["FIXTURE_GATE"], "escaped-ready"),
                f"{case} escaped writer after re-attach",
            )
            need(
                escaped_pid is not None and process_alive(escaped_pid),
                f"{case}: the escaped exec writer did not survive the removal",
            )
            found["escaped"] = escaped_pid
        return found

    def kept(self, found: dict, extra: dict) -> None:
        """The slot stays held with its bytes: no deletion, its project folder
        and record unchanged, and status still reports the quarantine. A
        forced detach may not persist the leaf's last writes (the main mode's
        `kept_present` precedent), so this oracle keeps the folder and the
        record rather than the leaf."""
        case, slot, folder, victim = found["case"], found["slot"], found["folder"], found["victim"]
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
        oracle(os.path.isdir(folder), f"{case}: the project folder {folder} was removed")
        self.lost_record(slot)
        print(f"{case}: slot {slot} kept in {folder} with its record")

    def before_boot(self) -> None:
        found = self.quarantine("before-boot")
        slot, folder, directory, victim = found["slot"], found["folder"], found["directory"], found["victim"]
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
        # Pre-build and project-directory-independent: re-resolve the slot
        # leaf from the recorded id alone — no build output, no reliance on
        # the project folder `slots/<folder>` — and require it to be the path
        # `quarantine` named, inside its folder and not that folder. The
        # deleting admission below creates a fresh slot of this same project
        # in the same folder, so a folder oracle could not tell deletion from
        # replacement.
        oracle(
            deletion_oracle_leaf(self.slots, slot, folder) == directory,
            f"before-boot: slot {slot} leaf resolved away from {directory} before the build",
        )
        oracle(
            os.path.dirname(directory) == folder and os.path.realpath(directory) != os.path.realpath(folder),
            f"before-boot: the oracle names {directory}, not the slot leaf of {slot} in {folder}",
        )
        oracle(
            os.path.isdir(directory),
            f"before-boot: slot leaf {directory} is not on the volume before the build",
        )
        self.lost_record(slot)
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
        oracle(
            not os.path.lexists(directory),
            f"before-boot: slot leaf {directory} still exists after deletion",
        )
        # The project-label folder is a different path and legitimately outlives
        # the slot: the deleting admission creates its own slot there. The
        # deletion is proven by the slot leaf above, the product's own line and
        # the record, not by the folder's absence.
        oracle(
            os.path.isdir(folder),
            f"before-boot: the project folder {folder} is gone; the leaf and folder are not distinguishable",
        )
        records = [
            record
            for record in self.state().get("records", [])
            if str((record.get("lost") or {}).get("slot_id")) == slot
        ]
        oracle(
            not records,
            f"before-boot: the deleted slot's record is still in state: {records[:1]}",
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

    def release_quarantined(self) -> None:
        """The operator-confirmed release (oxide#196): the exec tool's escaped
        child, outside the product's exec group but inside the supervisor's
        leader group, keeps a file open inside the quarantined slot, so every
        release refuses and nothing changes; once the writer is stopped, a dry
        run is clean and the release deletes the slot whole through the holding
        area."""
        found = self.quarantine("release-quarantined", escaped=True)
        case = found["case"]
        slot, folder, directory, victim = found["slot"], found["folder"], found["directory"], found["victim"]
        gate, escaped = found["gate"], found["escaped"]
        try:
            before = self.custody_digest()
            # The escaped child is the exec tool's own fork, outside the
            # product's exec group but inside the supervisor's leader group,
            # and holds a file in the slot scratch.
            need(os.path.isdir(directory), f"{case}: the quarantined slot leaf is already gone")
            need(process_alive(escaped), f"{case}: the escaped writer is not running")
            # Without the operator's flag: refused before any pool read.
            code, _, stderr = self.proof.oxide(
                ["pool", "release-quarantined", slot],
                "release without the flag",
                victim,
                self.child_env({}),
            )
            oracle(
                code != 0 and "--confirm-writers-stopped" in stderr and "would check" in stderr,
                f"{case}: the unflagged release was not refused with its checks: {code} {stderr[-1200:]}",
            )
            # With the flag the census refuses, naming the escaped process.
            code, _, stderr = self.proof.oxide(
                ["pool", "release-quarantined", slot, "--confirm-writers-stopped"],
                "release with an escaped writer",
                victim,
                self.child_env({}),
            )
            oracle(
                code != 0 and process_refusal(escaped) in stderr and "inside" in stderr,
                f"{case}: the census did not refuse the escaped writer {escaped}: {code} {stderr[-1600:]}",
            )
            oracle("nothing was released" in stderr, f"{case}: the refusal did not say nothing changed")
            # A dry run makes the same check and changes nothing.
            code, _, stderr = self.proof.oxide(
                ["pool", "release-quarantined", slot, "--dry-run", "--confirm-writers-stopped"],
                "dry run with an escaped writer",
                victim,
                self.child_env({}),
            )
            oracle(
                code != 0 and process_refusal(escaped) in stderr,
                f"{case}: the dry run did not refuse the escaped writer: {code} {stderr[-1200:]}",
            )
            oracle(
                self.custody_digest() == before and os.path.isdir(directory),
                f"{case}: a refusal changed the pool or the slot tree",
            )
            # Stop the escaped writer: no process holds a file in the slot.
            self.stop_escaped(gate, escaped)
            code, stdout, stderr = self.proof.oxide(
                ["pool", "release-quarantined", slot, "--dry-run", "--confirm-writers-stopped"],
                "clean dry run",
                victim,
                self.child_env({}),
            )
            oracle(code == 0, f"{case}: the clean dry run failed: {code} {stderr[-1200:]}")
            oracle(
                f"would release quarantined slot {slot}" in stdout
                and "census:" in stdout
                and "operator confirmed writers stopped" in stdout,
                f"{case}: the dry-run line is incomplete: {stdout[-1200:]}",
            )
            oracle(
                self.custody_digest() == before and os.path.isdir(directory),
                f"{case}: the dry run changed the pool or the slot tree",
            )
            # The release deletes the slot whole through the holding area.
            code, stdout, stderr = self.proof.oxide(
                ["pool", "release-quarantined", slot, "--confirm-writers-stopped"],
                "operator-confirmed release",
                victim,
                self.child_env({}),
            )
            oracle(code == 0, f"{case}: the release failed: {code} {stderr[-1600:]}")
            oracle(
                f"released quarantined slot {slot}" in stdout
                and "census:" in stdout
                and "operator confirmed writers stopped" in stdout,
                f"{case}: the release line is incomplete: {stdout[-1200:]}",
            )
            oracle(
                f"deleted slot {slot} quarantined:" in stderr,
                f"{case}: no deletion line names the quarantined slot: {stderr[-1600:]}",
            )
            oracle(
                "deleted the held slot tree" in stderr,
                f"{case}: the holding-area pass did not delete the tree: {stderr[-1600:]}",
            )
            oracle(
                not os.path.lexists(directory),
                f"{case}: slot leaf {directory} still exists after the release",
            )
            oracle(os.path.isdir(folder), f"{case}: the project folder {folder} was removed")
            records = [
                record
                for record in self.state().get("records", [])
                if str((record.get("lost") or {}).get("slot_id")) == slot
            ]
            oracle(not records, f"{case}: the released slot's record is still in state: {records[:1]}")
            ledger = self.ledger()
            oracle(
                all(str(row.get("slot_id")) != slot for row in ledger.get("slots", [])),
                f"{case}: the released slot is still in the ledger",
            )
            oracle(
                ledger.get("holding", []) == [],
                f"{case}: the released tree is still charged in the holding area: {ledger.get('holding', [])[:2]}",
            )
            stdout = self.status("release status", victim, {})
            oracle(f"slot {slot} " not in stdout, f"{case}: status still lists the released slot")
        finally:
            self.stop_escaped(gate, escaped)


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
    — plus the deletion oracle's slot-leaf resolution (the slot's own leaf,
    never the project-label folder a build or an adopt also creates or
    reuses), the escaped exec writer's syntax, exec-group escape and
    supervisor-leader-group membership, the release oracle's `process <pid>`
    form, the dispatch, the product-message marker and the receipt bound."""
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

    # The escaped exec writer (oxide#196) must be valid Python, leave the
    # product's exec group by joining the supervisor's own leader group, and
    # never leave the supervisor's session: the qualified supervisor can only
    # census and signal its leader group. The release oracle must match the
    # product's `process <pid>` census message, never a `pid <pid>` form.
    compile(ESCAPE_TOOL, "escape-tool", "exec")
    expect("os.setpgid(0, leader)" in ESCAPE_TOOL, "the escaped writer does not leave the exec group")
    expect("FIXTURE_LEADER_PGID" in ESCAPE_TOOL, "the escaped writer does not join the supervisor's leader group")
    expect("setsid" not in ESCAPE_TOOL, "the escaped writer must not leave the supervisor's session")
    expect('"escaped-writer"' in ESCAPE_TOOL, "the escaped writer does not hold a file in the slot")
    census_message = "process 4242 (python3) holds /slot/scratch/escaped-writer open inside /slot"
    expect(process_refusal(4242) in census_message, "the release oracle does not match the census message")
    expect(f"pid {4242}" not in census_message, "the old pid-only oracle would not match the census message")
    root = tempfile.mkdtemp(prefix="boot-closure-self-check.")
    try:
        case = BootClosure.__new__(BootClosure)
        case.env = dict(fixture)
        case.proof = Recorder()
        case.lost_record = lambda slot: {"holder": {"command": "exec /fixture/tool"}}
        slots = os.path.join(root, "slots")
        folder = os.path.join(slots, "victim-deadbeef")
        slot_dir = os.path.join(folder, "quick-7")
        os.makedirs(slot_dir)
        os.makedirs(os.path.join(slots, "other-cafe", "debug-8"))
        # The deletion oracle names the slot's own `<profile>-<id>` leaf, not
        # the project-label folder a build, an adopt or a relink also creates
        # or reuses: resolve it by the recorded id across the tree, require it
        # inside the discovered folder, and refuse anything else.
        expect(slot_leaf_directory(slots, "7") == slot_dir, "slot leaf resolution")
        expect(os.path.dirname(slot_leaf_directory(slots, "7")) != slots, "the slot leaf is the slots root")
        expect(deletion_oracle_leaf(slots, "7", folder) == slot_dir, "deletion oracle leaf")
        for bad, why in (
            ((slots, "7", os.path.join(slots, "other-cafe")), "a leaf in another project folder"),
            ((slots, "9", folder), "a missing slot leaf"),
        ):
            try:
                deletion_oracle_leaf(*bad)
            except UnknownOutcome:
                print(f"deletion oracle refused {why}")
            else:
                expect(False, f"the deletion oracle accepted {why}")
        # Counterfactual: the pre-fix oracle named `slots/<folder>`, the
        # project-label directory. A resolver that returned it is refused,
        # never silently accepted as the deleted slot.
        original = globals()["slot_leaf_directory"]
        globals()["slot_leaf_directory"] = lambda tree, slot_id: folder
        try:
            try:
                deletion_oracle_leaf(slots, "7", folder)
            except UnknownOutcome as error:
                print(f"deletion oracle refused the project-label folder: {error}")
            else:
                expect(False, "the project-label folder was accepted as the slot leaf")
        finally:
            globals()["slot_leaf_directory"] = original
        # A second leaf ending in the recorded id is ambiguous, never a pick.
        os.makedirs(os.path.join(slots, "other-cafe", "debug-7"))
        try:
            slot_leaf_directory(slots, "7")
        except UnknownOutcome as error:
            expect("2 slot leaves" in str(error), f"ambiguous leaf message {error}")
        else:
            expect(False, "two slot leaves with the recorded id resolved to one")
        os.rmdir(os.path.join(slots, "other-cafe", "debug-7"))
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
                "folder": folder,
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
        "boot-closure self-check ok: status/build child environments, deletion "
        "oracle slot leaf, diagnostic receipt (json, bounded, unqualified), "
        "dispatch, marker"
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
