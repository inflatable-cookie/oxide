#!/usr/bin/env python3
"""Focused proof for surprise removal of the removable build volume (oxide#098).

A short fixed script run as the whole command of the qualified shared proof
supervisor (`scripts/bounded-test.py`, oxide#109): 300 s, 512 MiB monitored
scratch, children in its cooperative session, evidence under
OXIDE_PROOF_EVIDENCE. It reuses the oxide#104 script's child runner and
staging; it adds no supervision of its own.

Stages this HEAD's `oxide` offline with 1.97.1. Over a private HOME,
CARGO_HOME and one budget-only `directory-budget-v1` pool, one owned 128 MiB
scratch APFS image at a unique `/Volumes/<name>` is attached with
`hdiutil -owners on` (every attach and re-attach, observed read-only from
`/sbin/mount` as carrying no `noowners` flag) and registered as the removable
pool; the product's own registration would refuse an ignore-ownership mount.
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

The fixture's own directory-budget pool is a DECLARED SCRATCH POOL
([scratch pools](../../docs/knowledge/contracts/scratch-pools.md) S4,
oxide#212): `private_environment` canonicalizes the run root once, creates one
private `0700` `OXIDE_SCRATCH_ROOT` inside it, keeps `OXIDE_POOL` strictly
inside that root and sets an explicit `OXIDE_POOL_CAP`, after dropping every
inherited `OXIDE_*` value. The root lives on the supervisor-selected validation
volume, so the S3 scratch route admits the pool where the ordinary admission
would refuse an external path; the selectors pass no `--root-parent
/private/tmp`. On a `/private/tmp` guard fallback the declaration is inert —
ordinary admission already succeeds on the internal volume — so such a run is
not scratch-pool coverage and says so.

Final outputs stay in `<checkout>/target` throughout. Custody of this proof's
own image is the qualified guard's, not this script's: after each attach the
script publishes one bounded request under `OXIDE_PROOF_EVIDENCE` naming the
backing, mount, device and volume UUID, and the guard performs at most ONE
ordinary detach of exactly that image after its own post-leader census
(oxide#191, Tom decision ee263cea). The script withdraws the request before
any detach it performs itself (only when no child was ever launched) and the
guard keeps the mount, backing and evidence whenever it cannot establish
closure. Not a benchmark and not real-drive qualification.

`--boot-closure` is the focused mode for oxide#161: a restart ends every
process, so a quarantined lost slot whose holder started before the current
boot session is positively closed. It stages the same candidate, attaches
the same owned image through the same owners-on attach and mount
observation, and runs three independent cases over private checkouts:
a quarantined `exec` slot whose override boot time follows its holder start
is reported closed by `status`, an explicit `outputs adopt` of a whole-target
fixture then migrates, and the next admission deletes the slot whole with
`oxide: deleted slot <n> lost before this boot (...)`; a quarantined slot
whose override boot time precedes its holder start stays kept with its
bytes; an unreadable override stays kept. The former descriptor-census release
case and its escaped-writer fixture are retired with that storage authority.
The one product seam is the debug-only
`OXIDE_TEST_BOOT_MS` override, which the product honors only under this
supervisor's own scratch/evidence/deadline layout. The quarantines are made by
the product's own removal path: a blocked `oxide exec` tool loses the volume
under the removal stop, and its release flush fails with the mount gone, so
the lost record names the exec holder (an exec may start processes outside
its group) and is quarantined. No state file is edited.
`--diagnostic` is the uncounted keep-going collection for either mode. The
main mode's fixed case map is `idle` -> `held` -> `held-delete` ->
`quarantine`, each case one named premise; the receipt records reached,
completed, failed, dependent-skipped and unexecuted cases and the latched
first fault as one bounded `qualified: false` document beside the guard's.
Only a recoverable `oracle(...)` product expectation continues into
dependency-independent cases. A timed-out or otherwise unclassifiable child,
writer, mount, device or custody outcome aborts the collection and is
recorded as `aborted` with the remaining cases `unexecuted`: this script can
never conclude a descendant group's closure, only the guard's census can.
`--plan` prints both modes' case tables, allocation and the nine premise
dispositions with no effect. `--self-check` is the pure apparatus check run
before any activation: no child, pool, image or device.
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

import contextlib
import glob
import hashlib
import importlib.util
import inspect
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
# One fixed compiler-flag value for every fixture child (premise 7). Staging
# refuses an inherited RUSTFLAGS/CARGO_ENCODED_RUSTFLAGS in the process
# environment, so it builds with the same empty flag set.
FROZEN_RUSTFLAGS = ""
# One excerpt bound for a recorded case failure in the diagnostic receipt.
RECEIPT_EXCERPT = 400
# The fixture's directory-budget pool is a declared scratch pool: one private
# root inside this run's own guard root, an explicit cap, and the pool strictly
# inside that root.
SCRATCH_POOL_ROOT = "scratch-pools"
SCRATCH_POOL_CAP = "64MiB"

# Every product line this proof's oracles match, pinned to the source text that
# carries it. The S4 pre-review sweep against main `d0c383b` found each one
# current, so there is nothing stale to carry; keeping the table makes a later
# wording change fail the effect-free self-check instead of a native round.
# Historical product identities are not recorded here: this proof always stages
# this HEAD, and its only historical evidence is its own earlier receipts.
PRODUCT_ORACLES = (
    (
        "lost-slot reason",
        "src/lease/control.rs",
        "lost with the removable volume; deleted whole when the volume returns",
    ),
    (
        "quarantine reason",
        "src/lease/control.rs",
        "quarantined: lost with the removable volume and {why}; kept with its bytes, never reused or deleted",
    ),
    (
        "boot-session closure reason",
        "src/lease/control.rs",
        "its holder predates this boot session, so it is deleted whole when the volume returns",
    ),
    (
        "lost-slot deletion line",
        "src/bin/oxide/store.rs",
        "held when the removable volume was lost (Cargo group {group} empty)",
    ),
    (
        "boot-session deletion line",
        "src/bin/oxide/store.rs",
        "lost before this boot (holder started {elapsed} s before boot; a restart ends every process)",
    ),
    (
        "wrapper quarantine reason",
        "src/bin/oxide/store.rs",
        "an inherited rustc wrapper was allowed and may outlive the build",
    ),
    (
        "removal stop line",
        "src/bin/oxide/watchdog.rs",
        "oxide: removable volume {} is gone; stopped Cargo group {group}: {signals}, stop latency {latency}",
    ),
    (
        "internal fallback selection",
        "src/bin/oxide/main.rs",
        "selected for the next build: internal pool (removable volume not used: {reason})",
    ),
    (
        "held-slot status line",
        "src/bin/oxide/main.rs",
        "retired and held, never reused ({})",
    ),
    (
        "removable flush line",
        "src/bin/oxide/main.rs",
        "oxide: flushed the removable volume before releasing slot {slot} in {:.1} ms",
    ),
    (
        "lost-slot summary line",
        "src/bin/oxide/main.rs",
        "oxide: slot {slot} was lost with the removable volume",
    ),
    (
        "registration line",
        "src/bin/oxide/removable.rs",
        "registered removable pool on {} (volume {}, container {})",
    ),
    (
        "outputs adoption line",
        "src/bin/oxide/outputs.rs",
        "now selects the owned {} root {} (generation {})",
    ),
)


def check_product_oracles() -> int:
    """Assert every pinned product line is still in the source file that
    carries it, so a stale oracle fails here rather than in a native round."""
    repo = os.path.dirname(HERE)
    for label, relative, needle in PRODUCT_ORACLES:
        path = os.path.join(repo, relative)
        expect(os.path.isfile(path), f"product oracle {label!r} names a missing file {relative}")
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
        expect(
            needle in text,
            f"product oracle {label!r} is gone from {relative}; re-sweep the proof's oracles",
        )
    return len(PRODUCT_ORACLES)


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


# Every child this proof launches may be writing to the removable image, and
# `Proof.reap`/the shared runner's `close` reap direct leaders only: neither
# proves the Cargo group has no descendant left. So this script never detaches
# after a launch. Its image is detached at most once, by the qualified guard,
# after the guard's own post-leader census (the request published by
# `publish_detach_request`), and any collection that left a child unreaped is
# kept whole by that guard.
WRITERS = {"launched": False}

# The one post-census detach request (oxide#191). The guard re-derives every
# binding itself, so this document is a request for an identity it must confirm,
# never an instruction to detach a path.
DETACH_REQUEST = "detach-request.json"

# The opt-in image-custody indication (oxide#191). Published BEFORE an attach, so
# a request-free failure (for example a refused attach) still leaves the guard
# able to keep a mounted or attached-but-unmounted backing instead of cleaning its
# root. A proof that publishes none gets no scan at all.
CUSTODY = "image-custody.json"


def publish_image_custody(image: str) -> str:
    """Declare the one backing this proof may attach, before any attach."""
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
    need(os.path.isdir(evidence), f"no OXIDE_PROOF_EVIDENCE directory for the custody indication: {evidence!r}")
    path = os.path.join(evidence, CUSTODY)
    body = {"version": 1, "backings": [os.path.realpath(image)]}
    temporary = path + ".tmp"
    try:
        with open(temporary, "w", encoding="utf-8") as handle:
            json.dump(body, handle, sort_keys=True)
        os.replace(temporary, path)
    finally:
        if os.path.lexists(temporary):
            os.unlink(temporary)
    print(f"image custody {path} names {body['backings'][0]}", flush=True)
    return path


def note_writer_launch() -> None:
    WRITERS["launched"] = True


def detach_request_path() -> str:
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
    need(os.path.isdir(evidence), f"no OXIDE_PROOF_EVIDENCE directory for the detach request: {evidence!r}")
    return os.path.join(evidence, DETACH_REQUEST)


def publish_detach_request(proof, image: str, mount: str, device: str, uuid: str) -> str:
    """Ask the qualified guard for one ordinary detach of exactly this image.

    Written atomically after the image exists and every identity is known. The
    guard validates containment, the attachment and the volume UUID before it
    detaches anything, and only after its own post-leader census; no request
    means it does nothing.
    """
    path = detach_request_path()
    body = {
        "version": 1,
        "backing": os.path.realpath(image),
        "mount": mount,
        "device": device,
        "volume_uuid": uuid.lower(),
        "note": f"test-oxide-removable-removal.py pid {os.getpid()}",
    }
    temporary = path + ".tmp"
    try:
        with open(temporary, "w", encoding="utf-8") as handle:
            json.dump(body, handle, sort_keys=True)
        os.replace(temporary, path)
    finally:
        if os.path.lexists(temporary):
            os.unlink(temporary)
    print(f"detach request {path} names {mount} ({device}, {uuid}) for the guard's post-census step", flush=True)
    return path


def withdraw_detach_request(reason: str) -> None:
    """Remove this run's request before a detach this script performs itself
    (only possible before any child launch), so the guard never reads a stale
    request for an image that is already gone."""
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
    if not os.path.isdir(evidence):
        return
    try:
        os.unlink(os.path.join(evidence, DETACH_REQUEST))
    except FileNotFoundError:
        return
    print(f"detach request withdrawn ({reason}); this script detaches before any child launch", flush=True)


def cleanup_after_failure(proof) -> bool:
    """Clean up a failed collection without presuming writer closure.

    Returns True when the mount was retained for the guard. Nothing is
    detached once a child was launched or one is unreaped; with no launch there
    is no possible descendant writer, so the request is withdrawn and the
    ordinary detach runs here.
    """
    if WRITERS["launched"] or proof.unclosed:
        print(
            "failure after a child launch: keeping the mount and backing for the "
            "guard's post-leader census",
            file=sys.stderr,
        )
        return True
    withdraw_detach_request("failure before any child launch")
    try:
        proof.detach()
    except BaseException as error:
        print(f"cleanup after failure: {error!r}", file=sys.stderr)
    return False


def retain_for_census(proof, outcome) -> bool:
    """Whether this collection reports its image as retained for the guard
    rather than answered by one request the guard made after its census.

    A collection that recorded ANY failure, abort, skip or unexecuted case —
    or that left a child unreaped — is not a state this script vouches for, so
    its result line claims no detach: the guard's own census decides whether
    the mount is detached or the whole root kept. This is reporting only; the
    detach request is never withdrawn on this path.
    """
    if proof.unclosed:
        return True
    if not outcome:
        return False
    return any(outcome.get(key) for key in ("failed", "aborted", "skipped", "unexecuted"))


class Proof(layout.Proof):
    """The #104 runner plus one background child for the held build. The
    child stays in the supervisor's session and in `unclosed` until reaped."""

    def run(self, command, *, cwd=None, env=None, timeout=120):
        note_writer_launch()
        return super().run(command, cwd=cwd, env=env, timeout=timeout)

    def start(self, args: list, cwd: str, env: dict) -> subprocess.Popen:
        note_writer_launch()
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

    def reap(self, child: subprocess.Popen) -> None:
        """Kill (if needed) and positively reap this proof's own child, so a
        failure path never leaves the guard's census unclosed."""
        if child.poll() is None:
            child.kill()
        try:
            child.wait(timeout=10)
        except BaseException:
            return
        self.unclosed.discard(child.pid)

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


def attach_options() -> list:
    """Every attach and re-attach of this proof's own image requests
    filesystem owners. The product judges the mounted root's own ownership and
    mode and refuses an ignore-ownership mount (MNT_IGNORE_OWNERSHIP), so a
    proof that attached without this could only ever test the refusal."""
    return ["-owners", "on"]


def owners_honored(proof, mount: str, label: str) -> None:
    """Bounded read-only observation that the private image's mounted root
    does not carry the ignore-ownership flag."""
    result = proof.run(["/sbin/mount"], timeout=30)
    expect(result.returncode == 0, f"{label}: mount listing failed: {result.stderr!r}")
    line = next(
        (
            entry
            for entry in result.stdout.decode(errors="replace").splitlines()
            if f" on {mount} " in entry
        ),
        None,
    )
    expect(line is not None, f"{label}: {mount} is not in the mount listing")
    expect(
        "noowners" not in line,
        f"{label}: {mount} was mounted with owners ignored (MNT_IGNORE_OWNERSHIP): {line}",
    )
    print(f"{label}: {mount} mounted with owners honored: {line.strip()}")


class Volume:
    """The one owned scratch image: path, root device, mount and UUID."""

    def __init__(self, proof: Proof, image: str, mount: str):
        self.proof, self.image, self.mount = proof, image, mount
        self.device = None
        self.mount_device = None
        self.uuid = None

    def attachment_entries(self) -> list:
        """Every `hdiutil info` entry whose image path is this proof's own
        image. A forced detach can leave a stale *detached* entry behind, so
        only entries that still carry a mounted entity are custody."""
        info = plistlib.loads(self.proof.run(["/usr/bin/hdiutil", "info", "-plist"], timeout=30).stdout)
        return [
            image
            for image in info.get("images", [])
            if os.path.realpath(image.get("image-path", "")) == os.path.realpath(self.image)
        ]

    @staticmethod
    def mounted_entities(entry: dict) -> list:
        return [entity for entity in entry.get("system-entities", []) if entity.get("mount-point")]

    def live_entries(self) -> list:
        return [entry for entry in self.attachment_entries() if self.mounted_entities(entry)]

    def attach(self) -> None:
        expect(not os.path.lexists(self.mount), f"{self.mount} exists before attach")
        expect(not self.live_entries(), f"{self.image} is still mounted before attach")
        attached = self.proof.run(["/usr/bin/hdiutil", "attach", *attach_options(), "-nobrowse", "-noverify", "-plist", self.image], timeout=60)
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
        # An APFS image attaches as a container whole disk PLUS a synthesized whole
        # disk; the mounted volume is one entity's own device. The detach request
        # names THAT entity, which is what the guard binds to the mount (a
        # whole-disk name can be the container, which the mount is not on).
        mounted = [entity for entity in entities if entity.get("mount-point") == self.mount]
        expect(len(mounted) == 1 and mounted[0].get("dev-entry"),
               f"the volume is not exactly one entity of {self.image}: {mounted}")
        self.mount_device = mounted[0]["dev-entry"]
        info = plistlib.loads(self.proof.run(["/usr/sbin/diskutil", "info", "-plist", self.mount], timeout=30).stdout)
        expect(self.uuid is None or info["VolumeUUID"] == self.uuid, "re-attached volume UUID changed")
        self.uuid = info["VolumeUUID"]
        owners_honored(self.proof, self.mount, "attach")
        # The guard detaches this image at most once, after its own census: every
        # attach (and re-attach) leaves exactly one request naming the identities
        # the guard must re-derive for itself.
        publish_detach_request(self.proof, self.image, self.mount, self.mount_device, self.uuid)
        print(f"attached {self.image} as {self.device} at {self.mount}, volume {self.uuid}")

    def bound(self) -> None:
        """Positive identity immediately before a forced detach: this image is
        mounted exactly once, that mount is this device and this mount point,
        and it is this UUID. A stale detached entry is not custody and cannot
        satisfy or fail the claim; a second *mount* still refuses."""
        own = self.live_entries()
        expect(len(own) == 1, f"image {self.image} is mounted {len(own)} times")
        devices = {entity.get("dev-entry") for entity in own[0]["system-entities"]}
        mounts = {entity.get("mount-point") for entity in own[0]["system-entities"]} - {None}
        expect(self.device in devices and mounts == {self.mount}, f"device {self.device} / mount {self.mount} not this image's: {devices} {mounts}")
        volume = plistlib.loads(self.proof.run(["/usr/sbin/diskutil", "info", "-plist", self.mount], timeout=30).stdout)
        expect(volume.get("VolumeUUID") == self.uuid, f"{self.mount} is not volume {self.uuid}")

    def force_detach(self, label: str) -> None:
        self.bound()
        # The surprise removal this case proves is this script's own forced
        # detach, so its request must not survive it: a stale request would name
        # an image that is already gone.
        withdraw_detach_request(f"{label}: forced removal of this run's own image")
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


# The guard's own control variables. They are not fixture configuration: the
# shared `layout.stage_at` -> `Proof.run` -> `work_left()` path reads
# OXIDE_PROOF_DEADLINE_NS for every bounded child, and OXIDE_PROOF_EVIDENCE is
# the same guard layout's evidence directory. The staging scrub preserves them
# verbatim (re-set explicitly after the scrub, so the prefix list cannot ever
# take them away) while removing every fixture-facing inherited value.
GUARD_CONTROLS = ("OXIDE_PROOF_DEADLINE_NS", "OXIDE_PROOF_EVIDENCE")


@contextlib.contextmanager
def staging_environment_scrubbed():
    """The shared `layout.stage_at` helper builds from this process's
    environment. Staging runs with every fixture-facing inherited OXIDE_/XDG_/
    CARGO_/SCCACHE_/RUSTC* key removed and only the invoking Cargo home
    restored, set explicitly, so this repository's committed lock resolves its
    offline registry cache there and no unknown CARGO_* value reaches the
    candidate. The guard's own OXIDE_PROOF_DEADLINE_NS and OXIDE_PROOF_EVIDENCE
    stay visible for the helper, which needs them to bound every staged child.
    Everything is restored on exit, including on failure."""
    saved = dict(os.environ)
    guard = {name: saved[name] for name in GUARD_CONTROLS if name in saved}
    invoking_cargo_home = saved.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo")
    try:
        for key in list(os.environ):
            if key.startswith(("OXIDE_", "XDG_", "CARGO_", "SCCACHE_", "RUSTC")):
                os.environ.pop(key, None)
        os.environ["CARGO_HOME"] = invoking_cargo_home
        os.environ.update(guard)
        yield
    finally:
        os.environ.clear()
        os.environ.update(saved)


def private_environment(root: str) -> dict:
    """The private HOME, CARGO_HOME, gate and sanitized command environment
    every fixture child gets; nothing here may reach the operator's account,
    Cargo home, wrapper or target settings.

    The fixture's directory-budget pool is a declared scratch pool
    (scratch-pools S4, oxide#212): the run root is canonicalized once, one
    private `0700` `scratch-pools` root is created inside it, and the pool is
    kept strictly inside that root with an explicit cap. The root is on the
    supervisor-selected volume, so the S3 scratch route admits the pool there.
    An inherited declaration never reaches a child: every OXIDE_* value is
    dropped below before this fixture's own are set."""
    root = os.path.realpath(root)
    home = os.path.join(root, "home")
    cargo_home = os.path.join(root, "cargo-home")
    gate = os.path.join(root, "gate")
    pool_root = os.path.join(root, SCRATCH_POOL_ROOT)
    for path in (home, cargo_home, gate, pool_root):
        os.makedirs(path, mode=0o700, exist_ok=True)
    env = dict(os.environ)
    real_rustup = env.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")
    # Premise 3 and 7: every inherited OXIDE_/XDG_/CARGO_/SCCACHE_/RUSTC* key,
    # every Cargo target/build override and every encoded compiler flag is
    # dropped before this fixture's own controls and the one frozen flag value
    # are set. Nothing of the host's pool, account or flag provenance leaks in.
    for key in list(env):
        if key in ("OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS"):
            continue
        if key.startswith(("SCCACHE_", "OXIDE_", "XDG_", "CARGO_", "RUSTC")) or key in (
            "CARGO",
            "CARGO_INCREMENTAL",
            "CARGO_ENCODED_RUSTFLAGS",
            "RUSTFLAGS",
            "RUSTUP_TOOLCHAIN",
        ):
            env.pop(key)
    tmp = os.path.realpath(os.environ["TMPDIR"])
    env.update(
        {
            "OXIDE_SCRATCH_ROOT": pool_root,
            "OXIDE_POOL": os.path.join(pool_root, "pool"),
            "OXIDE_POOL_CAP": SCRATCH_POOL_CAP,
            "HOME": home,
            "OXIDE_TEST_HOME_FROM_ENV": "1",
            "CARGO_HOME": cargo_home,
            "RUSTUP_HOME": real_rustup,
            "RUSTUP_TOOLCHAIN": runner.BUILDER,
            "CARGO_NET_OFFLINE": "true",
            "CARGO_TERM_COLOR": "never",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "",
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTFLAGS": FROZEN_RUSTFLAGS,
            "FIXTURE_GATE": gate,
            "OXIDE_TASK_SCRATCH_ROOT": tmp,
            "TMPDIR": tmp,
            "TMP": tmp,
            "TEMP": tmp,
        }
    )
    return env


def product_summary(stderr: str) -> str:
    """The one product summary line. Its absence is a recoverable oracle
    failure, not an unclassifiable writer, mount or custody outcome."""
    lines = [line for line in stderr.splitlines() if " profile, " in line and " strategy, slot " in line]
    oracle(len(lines) == 1, f"expected one summary line: {stderr[-600:]}")
    return lines[0]


def labelled_slot(slots: str, slot: int) -> str:
    """oxide#104: a new slot is `slots/<project folder>/<profile>-<id>`. A
    missing or duplicated label is a recoverable oracle failure."""
    found = glob.glob(os.path.join(slots, "*", f"*-{slot}"))
    oracle(len(found) == 1, f"slot {slot} not found once under {slots}: {found}")
    return found[0]


def diagnostic_outcome() -> dict:
    """The main-mode diagnostic's one bounded document."""
    return {
        "version": "oxide.removal.diagnostic/1",
        "mode": "diagnostic",
        "qualified": False,
        "claims": "diagnostic case coverage only; never a counted native PASS, "
                  "closure, census or resource claim",
        "cases": [case for case, _, _ in Removal.CASES],
        "purposes": {case: purpose for case, _, purpose in Removal.CASES},
        "dependencies": {case: list(deps) for case, deps, _ in Removal.CASES},
        "source": None,
        "root": None,
        "mount": None,
        "device": None,
        "volume_uuid": None,
        "limits": {"seconds": 300, "scratch_mib": 512, "image_mib": 128},
        "reached": [],
        "completed": [],
        "failed": [],
        "skipped": [],
        "unexecuted": [],
        "first_fault": None,
        "aborted": None,
        "unclosed": [],
    }


class Removal:
    """The three surprise-removal cases over one owned image, pool and
    candidate. Each case is one named premise; `state` carries only what an
    earlier case established. A case that cannot classify an outcome raises
    [`UnknownOutcome`] and the collection stops; a product expectation that
    did not hold raises [`OracleFailure`] and a diagnostic continues into the
    cases whose declared dependencies completed."""

    CASES = (
        (
            "idle",
            (),
            "an idle flushed slot survives forced removal and re-attach warm, with an internal "
            "fallback build and a running checkout-local binary between",
        ),
        (
            "held",
            ("idle",),
            "a blocked build fails cleanly on removal, keeps its lost slot recorded, refuses late "
            "writes and lets build/run/test fall back",
        ),
        (
            "held-delete",
            ("held",),
            "after re-attach the next admission deletes the lost slot whole and keeps the idle one",
        ),
        (
            "quarantine",
            ("held-delete",),
            "a wrapper-allowed build is quarantined on removal and kept with its bytes after re-attach",
        ),
    )

    def __init__(self, proof: Proof, repo: str, source: str):
        self.proof = proof
        self.state = {}
        name = "OxRm" + secrets.token_hex(4)
        self.mount = f"/Volumes/{name}"
        # The caller resolves and validates the source (`resolve_source`)
        # *before* this constructor stages or creates anything, so no path can
        # attach an image to an unpinned source.
        need(bool(re.fullmatch(r"[0-9a-f]{40}", source)), f"no exact source identity: {source!r}")
        self.source = source
        print(f"removal proof {os.getpid()} source {self.source} ({layout.source_facts(repo)}) volume {name}")
        need(not os.path.lexists(self.mount), f"{self.mount} already exists; never touched")
        proof.phase("start")
        with staging_environment_scrubbed():
            candidate = layout.stage_at(proof, repo, "oxide-candidate", retain_stage_root=True)
        print(f"staged candidate sha256 {layout.sha256(candidate)}")
        with open(candidate, "rb") as handle:
            oracle(CANDIDATE_MARKER in handle.read(), "candidate binary lacks the #098 lost-volume text")
        proof.binary = candidate
        proof.phase("after staging")
        self.env = private_environment(proof.root)
        proof.env = self.env
        self.gate = self.env["FIXTURE_GATE"]
        self.app = write_tree(os.path.join(proof.root, "app"), APP)
        self.held = write_tree(os.path.join(proof.root, "held"), held_fixture("held"))
        self.kept = write_tree(os.path.join(proof.root, "kept"), held_fixture("kept"))
        self.image = os.path.join(proof.root, "volume.sparseimage")
        created = proof.run(
            ["/usr/bin/hdiutil", "create", "-size", "128m", "-fs", "APFS", "-volname", name,
             "-type", "SPARSE", self.image],
            timeout=60,
        )
        need(created.returncode == 0, f"image creation failed: {created.stderr!r}")
        publish_image_custody(self.image)
        self.volume = Volume(proof, self.image, self.mount)
        self.volume.attach()
        code, stdout, _ = proof.oxide(
            ["pool", "register-removable", self.mount, "--reserve", "16MiB", "--cap", "64MiB"],
            "register",
            self.app,
        )
        oracle(code == 0 and "registered removable pool" in stdout, "registration failed")
        self.slots = os.path.join(self.mount, "oxide-pool", "slots")
        proof.phase("after registration")

    def case_idle(self) -> None:
        proof, state = self.proof, self.state
        code, _, stderr = proof.oxide(["build"], "removable cold app", self.app)
        line = product_summary(stderr)
        oracle(code == 0 and "apfs-volume-v1" in line and "slot 1 cold" in line,
               f"removable not selected: {line}")
        oracle("flushed the removable volume before releasing slot 1" in stderr, "no flush before release")
        on_volume = [entry for entry in os.listdir(self.mount) if not entry.startswith(".")]
        oracle(on_volume == ["oxide-pool"], f"volume holds more than the pool: {on_volume}")
        app_binary = os.path.join(self.app, "target", "debug", "app")
        oracle(os.path.isfile(app_binary), "default output is not checkout-local while docked")
        app_slot = labelled_slot(self.slots, 1)
        self.volume.force_detach("idle removal")
        ran = proof.run([app_binary], cwd=self.app, timeout=30)
        oracle(ran.returncode == 0 and b"fixture app ok" in ran.stdout,
               "checkout-local binary does not run after removal")
        code, _, stderr = proof.oxide(["build"], "fallback app (idle removal)", self.app)
        line = product_summary(stderr)
        oracle(code == 0 and "directory-budget-v1" in line and "removable volume not used" in stderr,
               f"no internal fallback: {line}")
        self.volume.no_spill("after fallback build")
        self.volume.attach()
        code, _, stderr = proof.oxide(["build"], "removable app after re-attach", self.app)
        line = product_summary(stderr)
        oracle(code == 0 and "apfs-volume-v1" in line and "slot 1 warm" in line,
               f"idle slot not kept warm: {line}")
        oracle(os.path.isdir(app_slot), "idle slot directory gone")
        state["app_slot"] = app_slot

    def case_held(self) -> None:
        proof, state = self.proof, self.state
        child = proof.start(["build"], self.held, self.env)
        try:
            wait_for(os.path.join(self.gate, "held", "started"), "held build")
            with open(os.path.join(self.gate, "held", "started"), encoding="utf-8") as handle:
                out_dir = handle.read()
            oracle(out_dir.startswith(self.slots + "/"),
                   f"build script OUT_DIR {out_dir} is not on the volume")
            code, stdout, _ = proof.oxide(["status"], "status, build held", self.app)
            oracle(code == 0 and re.search(r"  slot 2 .*held", stdout) is not None,
                   "held slot not shown held")
            self.volume.force_detach("held removal")
            with open(os.path.join(self.gate, "held", "go"), "w", encoding="utf-8") as handle:
                handle.write("go")
            started = time.monotonic()
            code, _, stderr = proof.finish(child, "held build after removal", WAIT_SECONDS)
            print(f"held build exited {time.monotonic() - started:.1f} s after release")
            oracle(code != 0, "the build survived the removal")
            oracle(
                "slot 2 is lost with the removable volume; deleted whole when the volume returns" in stderr,
                "lost-volume not recorded",
            )
            oracle("slot 2 was lost with the removable volume" in stderr, "no lost-volume summary line")
            oracle(
                re.search(rf"removable volume {re.escape(self.mount)} is gone; stopped Cargo group \d+: TERM",
                          stderr) is not None,
                "no removal stop",
            )
            report_path = os.path.join(self.gate, "held", "report")
            if os.path.exists(report_path):
                with open(report_path, encoding="utf-8") as handle:
                    report = handle.read()
                print(f"build script after removal:\n{report}")
                oracle("write None" not in report and "create None" not in report,
                       f"a late write succeeded: {report}")
            else:
                print("build script was stopped before its late writes")
            late_writes(out_dir, "held removal")
            self.volume.no_spill("after held build")
            oracle(not os.path.lexists(out_dir), "the slot's OUT_DIR exists after removal")
            code, stdout, _ = proof.oxide(["status"], "status, volume gone", self.app)
            oracle(
                code == 0
                and re.search(
                    r"slot 2 retired and held, never reused \(lost with the removable volume; "
                    r"deleted whole when the volume returns\)",
                    stdout,
                )
                is not None,
                "status does not show the lost slot",
            )
            code, _, stderr = proof.oxide(["build"], "fallback held", self.held)
            oracle(code == 0 and "directory-budget-v1" in product_summary(stderr), "held fallback build failed")
            code, stdout, stderr = proof.oxide(["run"], "fallback run held", self.held)
            oracle(code == 0 and "fixture held ok" in stdout
                   and "directory-budget-v1" in product_summary(stderr),
                   "run after fallback failed")
            oracle(os.path.isfile(os.path.join(self.held, "target", "debug", "held")),
                   "fallback output is not checkout-local")
            code, stdout, stderr = proof.oxide(["test"], "fallback test held", self.held)
            oracle(code == 0 and "test works ... ok" in stdout
                   and "directory-budget-v1" in product_summary(stderr), "test after fallback failed")
            self.volume.no_spill("after fallback run/test")
            state["out_dir"] = out_dir
        finally:
            proof.reap(child)

    def case_held_delete(self) -> None:
        proof, state = self.proof, self.state
        self.volume.attach()
        need(bool(state.get("out_dir")), "the held case did not record its slot's OUT_DIR")
        parts = os.path.relpath(state["out_dir"], self.slots).split(os.sep)
        lost_slot = os.path.join(self.slots, parts[0], parts[1])
        print(f"lost slot directory {lost_slot} present after re-attach: {os.path.isdir(lost_slot)}")
        code, _, stderr = proof.oxide(["build"], "removable held after re-attach", self.held)
        line = product_summary(stderr)
        oracle(code == 0 and "apfs-volume-v1" in line and "slot 2" not in line,
               f"rebuild after re-attach: {line}")
        oracle(
            re.search(r"deleted slot 2 held when the removable volume was lost \(Cargo group \d+ empty\)",
                      stderr) is not None,
            "lost slot not deleted",
        )
        oracle(not os.path.exists(lost_slot), f"lost slot {lost_slot} still on the volume")
        oracle(os.path.isdir(state["app_slot"]), "idle slot deleted with the lost one")
        code, stdout, _ = proof.oxide(["status"], "status after re-attach", self.app)
        oracle(code == 0 and "lost with the removable volume" not in stdout,
               "status still shows a lost slot")

    def case_quarantine(self) -> None:
        proof, state = self.proof, self.state
        wrapped = dict(self.env, OXIDE_ALLOW_RUSTC_WRAPPER="1")
        child = proof.start(["build"], self.kept, wrapped)
        try:
            wait_for(os.path.join(self.gate, "kept", "started"), "kept build")
            with open(os.path.join(self.gate, "kept", "started"), encoding="utf-8") as handle:
                kept_out = handle.read()
            self.volume.force_detach("quarantine removal")
            with open(os.path.join(self.gate, "kept", "go"), "w", encoding="utf-8") as handle:
                handle.write("go")
            code, _, stderr = proof.finish(child, "kept build after removal", WAIT_SECONDS)
            oracle(code != 0, "the kept build survived the removal")
            late_writes(kept_out, "quarantine removal")
            found = re.search(
                r"slot (\d+) is quarantined: lost with the removable volume and an inherited rustc wrapper was allowed",
                stderr,
            )
            oracle(found is not None, "quarantine not recorded")
            kept_id = found.group(1)
            self.volume.attach()
            parts = os.path.relpath(kept_out, self.slots).split(os.sep)
            kept_slot = os.path.join(self.slots, parts[0], parts[1])
            # A forced detach may not have persisted the slot's last writes;
            # whatever is there must stay.
            kept_present = os.path.isdir(kept_slot)
            print(f"quarantined slot directory {kept_slot} present after re-attach: {kept_present}")
            code, _, stderr = proof.oxide(["build"], "removable kept after re-attach", self.kept)
            line = product_summary(stderr)
            oracle(code == 0 and "apfs-volume-v1" in line and f"slot {kept_id} " not in line,
                   f"kept rebuild: {line}")
            oracle("deleted slot" not in stderr, "a quarantined slot was deleted")
            oracle(os.path.isdir(kept_slot) == kept_present, "quarantined slot directory changed")
            code, stdout, _ = proof.oxide(["status"], "status with quarantine", self.app)
            oracle(
                code == 0
                and re.search(rf"  slot {kept_id} .*quarantined: lost with the removable volume", stdout)
                is not None,
                "status does not show quarantine",
            )
            state.update(kept_id=kept_id, kept_slot=kept_slot, kept_present=kept_present)
        finally:
            proof.reap(child)


def record_abort(outcome: dict, case: str, failure) -> None:
    """Record one abort truthfully: the fault, the aborted case and everything
    that never ran. Undecided cases stay unexecuted, never skipped."""
    detail = failure if isinstance(failure, str) else (str(failure) or repr(failure))
    outcome["aborted"] = outcome["aborted"] or f"{case}: {detail}"
    outcome["first_fault"] = outcome["first_fault"] or outcome["aborted"]
    outcome["unexecuted"] = [name for name, _, _ in Removal.CASES if name not in outcome["reached"]]


def collect(session: Removal, diagnostic: bool, outcome: dict, bodies: dict | None = None) -> dict:
    """Run the fixed case map; dependents of a case that did not complete are
    skipped in a diagnostic and never run against a missing premise."""
    bodies = (
        {
            "idle": session.case_idle,
            "held": session.case_held,
            "held-delete": session.case_held_delete,
            "quarantine": session.case_quarantine,
        }
        if bodies is None
        else bodies
    )
    for case, deps, _ in Removal.CASES:
        missing = [dep for dep in deps if dep not in outcome["completed"]]
        if diagnostic and missing:
            outcome["skipped"].append([case, missing])
            print(f"{case}: skipped (dependency {', '.join(missing)} did not complete)")
            continue
        outcome["reached"].append(case)
        try:
            bodies[case]()
        except OracleFailure as failure:
            outcome["failed"].append([case, str(failure)[:RECEIPT_EXCERPT]])
            outcome["first_fault"] = outcome["first_fault"] or f"{case}: {failure}"
            print(f"{case}: FAILED {failure}", file=sys.stderr)
            if not diagnostic:
                raise
            continue
        except UnknownOutcome as failure:
            record_abort(outcome, case, failure)
            raise
        except (KeyboardInterrupt, SystemExit):
            record_abort(outcome, case, "interrupted")
            raise
        except BaseException as failure:
            # A timed-out or otherwise unclassifiable child, writer, mount or
            # device outcome aborts the collection; it is never a recoverable
            # oracle failure and never a pass.
            record_abort(outcome, case, failure)
            print(f"{case}: ABORTED unexpected {failure!r}", file=sys.stderr)
            raise
        outcome["completed"].append(case)
        session.proof.phase(f"after {case}")
    outcome["unexecuted"] = [name for name, _, _ in Removal.CASES if name not in outcome["reached"]]
    return outcome


def resolve_source(proof: Proof, repo: str) -> str:
    """The exact source identity this run binds to, or an abort.

    This runs before staging and before any image is created or attached, so a
    run whose source cannot be pinned never touches the volume. A lookup that
    times out, dies or is interrupted, an empty or non-exact identity and a
    dirty tracked worktree are all outcomes this proof cannot classify: they
    raise [`UnknownOutcome`] and the caller records the fault with every planned
    case unexecuted.
    """
    try:
        head = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30)
        status = proof.run(
            ["git", "status", "--porcelain", "--untracked-files=no"], cwd=repo, timeout=30
        )
    except (KeyboardInterrupt, SystemExit):
        raise
    except BaseException as failure:
        raise UnknownOutcome(f"source lookup failed: {failure!r}") from failure
    # A child that exited nonzero produced no trustworthy answer: an empty
    # stdout from a failed `git status` is not a clean worktree, and a failed
    # `git rev-parse` is not an identity. Both are checked before any output is
    # interpreted, and both abort before staging or any image effect.
    if head.returncode != 0:
        raise UnknownOutcome(
            f"the source lookup exited {head.returncode}: "
            f"{head.stderr.decode(errors='replace').strip()[-400:]!r}"
        )
    if status.returncode != 0:
        raise UnknownOutcome(
            f"the worktree status exited {status.returncode} (its output is not a clean "
            f"worktree): {status.stderr.decode(errors='replace').strip()[-400:]!r}"
        )
    source = head.stdout.decode(errors="replace").strip()
    if not re.fullmatch(r"[0-9a-f]{40}", source):
        raise UnknownOutcome(f"no exact source identity: {source!r}")
    dirty = status.stdout.decode(errors="replace").strip()
    if dirty:
        raise UnknownOutcome(
            f"the tracked worktree is not clean: {dirty.splitlines()[:4]}"
        )
    return source


def run(proof: Proof, repo: str, diagnostic: bool) -> dict:
    """The main (ephemeral, surprise-removal) mode. Returns its outcome
    document; the diagnostic receipt is written here, in every outcome.

    The source gate and the setup both sit inside the receipt-writing `try`, so
    a run that cannot pin its source, that is interrupted, or whose setup dies
    still leaves one truthful receipt with the real fault and every planned
    case unexecuted — and never stages or attaches anything first.
    """
    outcome = diagnostic_outcome()
    outcome["root"] = proof.root
    session = None
    try:
        try:
            source = resolve_source(proof, repo)
        except (KeyboardInterrupt, SystemExit):
            record_abort(outcome, "source", "interrupted")
            raise
        except BaseException as failure:
            # No print-and-continue: a run that cannot pin its source aborts
            # here, before staging or any image effect.
            record_abort(outcome, "source", failure)
            print(f"source: ABORTED {failure}", file=sys.stderr)
            raise
        outcome["source"] = source
        try:
            session = Removal(proof, repo, source)
        except OracleFailure as failure:
            outcome["failed"].append(["setup", str(failure)[:RECEIPT_EXCERPT]])
            outcome["first_fault"] = outcome["first_fault"] or f"setup: {failure}"
            outcome["unexecuted"] = [case for case, _, _ in Removal.CASES]
            if diagnostic:
                print(f"setup: FAILED {failure}", file=sys.stderr)
                return outcome
            raise
        except (KeyboardInterrupt, SystemExit):
            record_abort(outcome, "setup", "interrupted")
            raise
        except BaseException as failure:
            # An unexpected setup exception is an outcome this collection
            # cannot classify: record the fault, abort and leave every case
            # unexecuted, then fail the run. An empty, fault-less receipt is
            # never a truthful outcome.
            record_abort(outcome, "setup", failure)
            print(f"setup: ABORTED unexpected {failure!r}", file=sys.stderr)
            raise
        outcome.update(
            mount=session.mount,
            device=session.volume.device,
            volume_uuid=session.volume.uuid,
        )
        collect(session, diagnostic, outcome)
    finally:
        if diagnostic:
            outcome["unclosed"] = sorted(proof.unclosed)
            outcome["mount"] = proof.mount
            path = diagnostic_receipt(outcome, "removal.diagnostic")
            print(f"removal diagnostic receipt {path}")
    return outcome


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






def git_fixture(proof: Proof, path: str, files: dict) -> str:
    """A tiny crate that is a Git repository already ignoring `/target`, the
    prerequisite a first conversion checks."""
    root = write_tree(path, files)
    proof.run(["git", "init", "-q"], cwd=root, timeout=30)
    with open(os.path.join(root, ".gitignore"), "w", encoding="utf-8") as handle:
        handle.write("/target\n")
    return root


def diagnostic_receipt(outcome: dict, prefix: str = DIAGNOSTIC_RECEIPT) -> str:
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
    name = f"{prefix}-{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}-{os.getpid()}.json"
    path = os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], name)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(body)
    return path


class BootClosure:
    """The focused session: one candidate, one owned image, three independent
    boot-session cases. Every
    quarantine is made by the product's own removal stop; no state file is
    edited."""

    def __init__(self, proof: Proof, repo: str, source: str):
        self.proof = proof
        self.repo = repo
        need(bool(re.fullmatch(r"[0-9a-f]{40}", source)), f"no exact source identity: {source!r}")
        self.source = source
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
        with staging_environment_scrubbed():
            candidate = layout.stage_at(proof, repo, "oxide-boot-closure", retain_stage_root=True)
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
        publish_image_custody(image)
        # `-owners on` on every attach: the product judges the mounted root's
        # own ownership and mode, and a mount that ignores ownership would
        # make that judgement meaningless.
        self.volume = Volume(proof, image, self.mount)
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


    def quarantine(self, case: str) -> dict:
        """Hold an exec slot, remove its volume and return its recorded facts."""
        env = self.child_env({"FIXTURE_GATE": os.path.join(self.proof.root, "gate", case)})
        os.makedirs(env["FIXTURE_GATE"], mode=0o700)
        victim = self.victims[case]
        before = set(os.listdir(self.slots))
        child = self.proof.start(["exec", "--", self.tool], victim, env)
        wait_for(os.path.join(env["FIXTURE_GATE"], "started"), f"{case} exec")
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



def boot_closure(proof: Proof, repo: str, diagnostic: bool) -> tuple:
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
        try:
            source = resolve_source(proof, repo)
        except (KeyboardInterrupt, SystemExit):
            outcome["aborted"] = "source: interrupted"
            raise
        except BaseException as failure:
            outcome["aborted"] = f"source: {failure}"
            outcome["first_fault"] = outcome["first_fault"] or outcome["aborted"]
            print(f"source: ABORTED {failure}", file=sys.stderr)
            raise
        outcome["source"] = source
        try:
            session = BootClosure(proof, repo, source)
        except (KeyboardInterrupt, SystemExit):
            outcome["aborted"] = outcome["aborted"] or "setup: interrupted"
            raise
        except BaseException as failure:
            outcome["aborted"] = outcome["aborted"] or f"setup: {failure}"
            outcome["first_fault"] = outcome["first_fault"] or outcome["aborted"]
            print(f"setup: ABORTED {failure}", file=sys.stderr)
            raise
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
                outcome["first_fault"] = outcome["first_fault"] or outcome["aborted"]
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
    # The custody outcome is NOT claimed here: `main` evaluates
    # `retain_for_census` after this returns, so the line carries a
    # `{custody}` placeholder that the top-level decision fills in. Claiming
    # one here would let a failed diagnostic that retains the mount report a
    # detach that never happened.
    if diagnostic or outcome["failed"]:
        line = (
            f"boot-closure diagnostic collected (UNCOUNTED, never a PASS): "
            f"{len(outcome['completed'])}/{len(BOOT_CASES)} cases completed, failed "
            f"{[case for case, _ in outcome['failed']]}, unexecuted {outcome['unexecuted']}; "
            f"{elapsed:.0f} s script time, peak owned {proof.peak // MIB} MiB at phase checks, "
            f"host free minimum {proof.low // MIB} MiB; one owned 128 MiB scratch image, "
            f"{{custody}}; receipt {path if diagnostic else 'none'}"
        )
    else:
        line = (
            f"boot-closure proof held: {elapsed:.0f} s script time, peak owned "
            f"{proof.peak // MIB} MiB at phase checks, host free minimum {proof.low // MIB} MiB; "
            f"one owned 128 MiB scratch image, {{custody}}"
        )
    return line, outcome


# The nine-item proof-authoring premise checklist, traced through the actual
# child environments and helpers and bound by `--self-check`. `--plan` prints
# it. It covers both modes: the boot closure adds the debug boot-time override
# and registers the same image in permanent mode.
PREMISE_CHECKLIST = (
    (
        "private HOME and rustup",
        "applicable: `private_environment` keeps the invoking RUSTUP_HOME, gives every child a "
        "private HOME and CARGO_HOME, pins RUSTUP_TOOLCHAIN=1.97.1 and RUSTUP_AUTO_INSTALL=0; staging "
        "runs inside `staging_environment_scrubbed`, which removes every fixture-facing inherited "
        "OXIDE_/XDG_/CARGO_/SCCACHE_/RUSTC* value, restores only the invoking CARGO_HOME (set "
        "explicitly, because this repository's committed lock resolves its offline registry cache "
        "there), and keeps the guard's own OXIDE_PROOF_DEADLINE_NS and OXIDE_PROOF_EVIDENCE visible "
        "because the shared `layout.stage_at` -> `work_left()` path bounds every staged child with "
        "them; `layout.stage_at` is the only --locked call, and the dependency-free fixtures make "
        "their own lock offline",
    ),
    (
        "account-home control",
        "applicable: `private_environment` sets OXIDE_TEST_HOME_FROM_ENV=1 beside the private HOME, "
        "and every boot-closure child starts from that same environment plus its own override",
    ),
    (
        "environment isolation",
        "applicable: `private_environment` drops every OXIDE_/XDG_/CARGO_/SCCACHE_/RUSTC* key "
        "(including all CARGO_* configuration and SCCACHE_* values) and re-adds only this fixture's "
        "controls, among them its own OXIDE_SCRATCH_ROOT, OXIDE_POOL strictly inside it and an "
        "explicit OXIDE_POOL_CAP; `staging_environment_scrubbed` does the same for staging, restores "
        "only the invoking CARGO_HOME and preserves only the guard's two OXIDE_PROOF_* control "
        "variables (`GUARD_CONTROLS`), which are not fixture configuration",
    ),
    (
        "automatic adoption",
        "not applicable: the main mode is ephemeral and no fixture is a Git repository, so no "
        "outputs conversion can happen; the boot-closure whole-target fixture is converted only by "
        "its explicit `outputs adopt` case",
    ),
    (
        "Cargo external argv",
        "not applicable: no external Cargo subcommand runs; the boot closure's `oxide exec` tool is "
        "a shell script, not a Cargo subcommand",
    ),
    (
        "owned images",
        "applicable: every attach and re-attach uses `hdiutil -owners on` and is observed read-only "
        "in /sbin/mount as carrying no `noowners` flag; identity is positively bound before each "
        "forced detach, each attach publishes one post-census detach request the qualified guard "
        "re-derives and performs at most once after its own census, and an in-script detach (only "
        "before any child launch) withdraws that request first",
    ),
    (
        "compiler flags",
        "applicable: main() refuses an inherited RUSTFLAGS or CARGO_ENCODED_RUSTFLAGS and staging "
        "runs in the scrubbed environment (which keeps only the guard's OXIDE_PROOF_* controls), so "
        "neither reaches it; every fixture child gets RUSTFLAGS=FROZEN_RUSTFLAGS with no encoded "
        "value",
    ),
    (
        "temporary storage",
        "applicable: the scratch is one fresh directory inside the qualified supervisor's "
        "OXIDE_TASK_SCRATCH_ROOT on the supervisor-selected volume (the selectors pass no "
        "--root-parent /private/tmp), TMPDIR/TMP/TEMP name that same directory, and the fixture's "
        "directory-budget pool is a declared scratch pool: one private 0700 OXIDE_SCRATCH_ROOT "
        "inside the run root with OXIDE_POOL strictly inside it and an explicit OXIDE_POOL_CAP, so "
        "the S3 scratch route admits it. On a /private/tmp guard fallback the declaration is inert, "
        "which is not scratch-pool coverage",
    ),
    (
        "macOS mktemp",
        "applicable: tempfile.mkdtemp with a template inside OXIDE_TASK_SCRATCH_ROOT; every fixture "
        "child inherits that TMPDIR",
    ),
)

USAGE = ("usage: test-oxide-removable-removal.py "
         "[--diagnostic | --plan | --boot-closure [--diagnostic]] | --self-check")


def plan() -> int:
    """Print both modes' fixed case tables, allocation and premise
    dispositions. No child, pool, image or supervisor."""
    print("removable-removal proof plan (effect-free)")
    print(f"candidate: this HEAD's debug oxide, staged offline with {runner.BUILDER} from {HERE}")
    print("selector (main): python3 scripts/bounded-test.py --limit 300 "
          "--scratch-budget-mib 512 -- python3 scripts/test-oxide-removable-removal.py")
    print("selector (boot closure): python3 scripts/bounded-test.py --limit 300 "
          "--scratch-budget-mib 512 -- python3 scripts/test-oxide-removable-removal.py --boot-closure")
    print("allocation: one owned 128 MiB scratch APFS image at a unique /Volumes/<name> and one "
          "private budget-only directory-budget scratch pool under this run's OXIDE_SCRATCH_ROOT "
          "inside OXIDE_TASK_SCRATCH_ROOT on the supervisor-selected volume, four fixture "
          "checkouts, inside that one owned scratch (monitored peak 99.7 MiB of the 512 MiB budget)")
    print("scratch pool: the fixture declares OXIDE_SCRATCH_ROOT (one private 0700 root inside the "
          f"run root), keeps OXIDE_POOL strictly inside it and sets OXIDE_POOL_CAP={SCRATCH_POOL_CAP}; "
          "no --root-parent is passed, so the guard's selected-volume default is used. A "
          "/private/tmp fallback is internal, where the declaration is inert and the run is not "
          "scratch-pool coverage")
    print("product oracle sweep (main d0c383b, effect-free): " + "; ".join(
        f"{label} in {relative}" for label, relative, _ in PRODUCT_ORACLES))
    print("limits: 300 s wall including cleanup, 512 MiB monitored scratch, at most one ordinary "
          "detach of this run's own image, performed by the qualified guard after its post-leader "
          "census (never by this script once a child has launched)")
    print("cases (main mode):")
    for case, deps, purpose in Removal.CASES:
        print(f"  {case}: {purpose} (after {', '.join(deps) or 'nothing'})")
    print("cases (boot-closure mode):")
    for case, purpose in BOOT_CASES:
        print(f"  {case}: {purpose} (independent)")
    print("premise checklist:")
    for item, disposition in PREMISE_CHECKLIST:
        print(f"  {item}: {disposition}")
    print("modes: --diagnostic (uncounted keep-going collection for the main mode), "
          "--boot-closure [--diagnostic], --plan (this), --self-check (pure apparatus); "
          "the counted main-mode run takes no argument")
    return 0


def mode_from_args(arguments: list) -> tuple | None:
    """`(focused, diagnostic, plan)` for a fixed argument list, or `None` when
    the request is not one this script defines."""
    if arguments == ["--plan"]:
        return (False, False, True)
    focused = "--boot-closure" in arguments
    diagnostic = "--diagnostic" in arguments
    if (focused, diagnostic) not in ((False, False), (False, True), (True, False), (True, True)):
        return None
    if not focused and arguments not in ([], ["--diagnostic"]):
        return None
    if focused and sorted(arguments) != sorted(["--boot-closure"] + (["--diagnostic"] if diagnostic else [])):
        return None
    return (focused, diagnostic, False)


def self_check() -> int:
    """Pure apparatus check for the boot-closure mode: no child, pool, image,
    device or supervisor. It pins the two defects an exact-head review found
    before any native activation — the diagnostic receipt's `json` dependency
    and the full fixture environment every child (`status` included) receives
    — plus the deletion oracle's slot-leaf resolution (the slot's own leaf,
    never the project-label folder a build or an adopt also creates or
    reuses), the owners-on attach and mount observation every mode shares,
    the product-message marker and the receipt bound.

    It also pins the scratch-pool environment contract (scratch-pools S4,
    oxide#212), so a script left half-migrated off the internal root fails
    here instead of running: the canonical private `OXIDE_SCRATCH_ROOT` inside
    the monitored guard root, `OXIDE_POOL` strictly inside it with an explicit
    cap, both modes taking it from the same `private_environment`, the four
    selectors passing no `--root-parent`, and every product line the oracles
    match still present in the source file that carries it."""
    fixture = {
        "OXIDE_SCRATCH_ROOT": "/fixture/scratch-pools",
        "OXIDE_POOL": "/fixture/scratch-pools/pool",
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
    # Every attach requests filesystem owners, and the mount observation
    # refuses an ignore-ownership mount. The real `/sbin/mount` listing is
    # reproduced here so the observation cannot silently pass on any mount,
    # and `Volume.attach` is pinned to that one option list, so the main mode
    # and the focused boot-closure mode cannot diverge again.
    expect(attach_options() == ["-owners", "on"], f"attach options {attach_options()}")
    expect(
        "*attach_options()" in inspect.getsource(Volume.attach),
        "Volume.attach does not use the shared owners option list",
    )
    # Custody of this proof's own image is the qualified guard's (oxide#191):
    # every attach publishes one bounded request naming the identities the guard
    # re-derives itself, the success path never detaches in-script, and any
    # detach this script does perform withdraws that request first.
    expect(
        "publish_detach_request" in inspect.getsource(Volume.attach),
        "Volume.attach does not publish the post-census detach request",
    )
    expect(
        "self.mount_device" in inspect.getsource(Volume.attach)
        and "publish_detach_request(self.proof, self.image, self.mount, self.mount_device, self.uuid)"
        in inspect.getsource(Volume.attach),
        "Volume.attach does not name the mounted entity's own device in the request",
    )
    expect(
        "publish_image_custody" in inspect.getsource(Removal.__init__)
        and "publish_image_custody" in inspect.getsource(BootClosure.__init__),
        "a mode does not declare its image custody before attaching",
    )
    expect(
        "withdraw_detach_request" in inspect.getsource(Volume.force_detach),
        "Volume.force_detach does not withdraw the request it invalidates",
    )
    expect(
        "withdraw_detach_request" in inspect.getsource(cleanup_after_failure),
        "cleanup_after_failure detaches without withdrawing the request",
    )
    expect(
        "proof.detach()" not in inspect.getsource(main),
        "the success path detaches in-script instead of requesting the guard's post-census step",
    )
    holder = dict(os.environ)
    evidence = tempfile.mkdtemp(prefix="removal-detach-request.")
    try:
        os.environ["OXIDE_PROOF_EVIDENCE"] = evidence
        published = publish_detach_request(
            object(),
            "/fixture/volume.sparseimage",
            "/Volumes/OxSelf",
            "/dev/disk4s1",
            "0F1E2D3C-4B5A-6978-8796-A5B4C3D2E1F0",
        )
        with open(published, encoding="utf-8") as handle:
            body = json.load(handle)
        expect(
            sorted(body) == ["backing", "device", "mount", "note", "version", "volume_uuid"],
            f"request fields {sorted(body)}",
        )
        expect(body["version"] == 1, f"request version {body['version']!r}")
        expect(body["mount"] == "/Volumes/OxSelf" and body["device"] == "/dev/disk4s1",
               f"request identity {body}")
        expect(body["volume_uuid"] == "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0",
               f"request UUID is not the lowercased volume identity: {body['volume_uuid']}")
        expect(body["backing"].startswith("/") and os.path.normpath(body["backing"]) == body["backing"],
               f"request backing is not canonical: {body['backing']}")
        expect(os.listdir(evidence) == [DETACH_REQUEST],
               f"the request was not written atomically: {os.listdir(evidence)}")
        withdraw_detach_request("self-check")
        expect(os.listdir(evidence) == [], f"withdrawal left {os.listdir(evidence)}")
        # The opt-in custody indication is separate from the request: it is
        # published before an attach, it is atomic, and withdrawing the request
        # never removes it.
        custody = publish_image_custody("/fixture/volume.sparseimage")
        with open(custody, encoding="utf-8") as handle:
            declared = json.load(handle)
        expect(sorted(declared) == ["backings", "version"] and declared["version"] == 1,
               f"custody indication body {declared}")
        expect(declared["backings"] == [os.path.realpath("/fixture/volume.sparseimage")],
               f"custody indication backings {declared['backings']}")
        expect(os.listdir(evidence) == [CUSTODY],
               f"the custody indication was not written atomically: {os.listdir(evidence)}")
        os.unlink(custody)
        withdraw_detach_request("self-check, already absent")  # idempotent, never an error
        os.environ.pop("OXIDE_PROOF_EVIDENCE", None)
        try:
            publish_detach_request(object(), "/fixture/volume.sparseimage", "/Volumes/OxSelf",
                                   "/dev/disk4s1", "0F1E2D3C-4B5A-6978-8796-A5B4C3D2E1F0")
        except UnknownOutcome as refused:
            expect("OXIDE_PROOF_EVIDENCE" in str(refused), f"refusal {refused}")
        else:
            expect(False, "a detach request was published without an evidence directory")
        expect(withdraw_detach_request("self-check, no evidence") is None,
               "withdrawal without an evidence directory is not silent")
    finally:
        os.environ.clear()
        os.environ.update(holder)
        shutil.rmtree(evidence, ignore_errors=True)

    class MountListing:
        def __init__(self, line: str):
            self.line = line

        def run(self, argv, cwd=None, env=None, timeout=None):
            return subprocess.CompletedProcess(argv, 0, stdout=(self.line + "\n").encode(), stderr=b"")

    honored = MountListing(
        "/dev/disk4s1 on /Volumes/OxSelf (apfs, local, nodev, nosuid, journaled, noatime)"
    )
    owners_honored(honored, "/Volumes/OxSelf", "self-check")
    for refused, why in (
        (
            "/dev/disk4s1 on /Volumes/OxSelf (apfs, local, nodev, nosuid, journaled, noatime, noowners)",
            "an ignore-ownership mount",
        ),
        ("/dev/disk4s1 on /Volumes/Other (apfs, local, journaled)", "a mount listing without this image"),
    ):
        try:
            owners_honored(MountListing(refused), "/Volumes/OxSelf", "self-check")
        except RuntimeError as error:
            print(f"owners observation refused {why}: {error}")
        else:
            expect(False, f"the owners observation accepted {why}")
    # A forced detach can leave a stale *detached* hdiutil entry behind. Only
    # entries that still carry a mounted entity are custody: the stale entry
    # can neither satisfy nor fail the identity claim, while a real second
    # mount and a mismatched device still refuse.
    class Attachments:
        def __init__(self, images: list, uuid: str = "UUID"):
            self.images = images
            self.uuid = uuid

        def run(self, argv, cwd=None, env=None, timeout=None):
            if os.path.basename(argv[0]) == "hdiutil":
                body = plistlib.dumps(
                    {
                        "images": [
                            {"image-path": "/fixture/volume.sparseimage", "system-entities": entities}
                            for entities in self.images
                        ]
                    }
                )
            else:
                body = plistlib.dumps({"VolumeUUID": self.uuid})
            return subprocess.CompletedProcess(argv, 0, stdout=body, stderr=b"")

    whole = {"dev-entry": "/dev/disk4"}
    mounted = {"dev-entry": "/dev/disk4s1", "mount-point": "/Volumes/OxSelf"}
    detached = Volume(
        Attachments([[whole]]), "/fixture/volume.sparseimage", "/Volumes/OxSelf"
    )
    expect(detached.live_entries() == [], "a stale detached entry counted as custody")
    one = Volume(Attachments([[whole, mounted]]), "/fixture/volume.sparseimage", "/Volumes/OxSelf")
    expect(len(one.live_entries()) == 1, "a mounted entry was not custody")
    one.device, one.uuid = "/dev/disk4", "UUID"
    one.bound()
    twice = Volume(
        Attachments([[whole, mounted], [{"dev-entry": "/dev/disk6"}, {"dev-entry": "/dev/disk6s1", "mount-point": "/Volumes/OxOther"}]]),
        "/fixture/volume.sparseimage",
        "/Volumes/OxSelf",
    )
    twice.device, twice.uuid = "/dev/disk4", "UUID"
    try:
        twice.bound()
    except RuntimeError as error:
        expect("mounted 2 times" in str(error), f"second-mount refusal: {error}")
    else:
        expect(False, "a second mount was accepted as one attachment")
    wrong = Volume(Attachments([[whole, mounted]]), "/fixture/volume.sparseimage", "/Volumes/OxSelf")
    wrong.device, wrong.uuid = "/dev/disk9", "UUID"
    try:
        wrong.bound()
    except RuntimeError as error:
        expect("not this image's" in str(error), f"device refusal: {error}")
    else:
        expect(False, "a mismatched device was accepted")

    # A failure path reaps its own child positively, so the guard's census
    # never stays unclosed for a child this script already waited for.
    class FakeChild:
        def __init__(self, pid: int):
            self.pid = pid
            self.returncode = None

        def poll(self):
            return self.returncode

        def kill(self):
            self.returncode = -9

        def wait(self, timeout=None):
            if self.returncode is None:
                self.returncode = 0
            return self.returncode

    census = Proof(root)
    killer = FakeChild(4242)
    census.unclosed.add(4242)
    census.reap(killer)
    expect(census.unclosed == set(), f"a killed and reaped child stayed unclosed: {census.unclosed}")
    exited = FakeChild(4243)
    census.unclosed.add(4243)
    exited.returncode = 0
    census.reap(exited)
    expect(census.unclosed == set(), "a child that exited on its own stayed unclosed")

    # The main mode's case map: every case has a body and a purpose, and every
    # dependency names an earlier case, so a diagnostic can skip only
    # dependents of what did not complete.
    expect(len(Removal.CASES) == 4, f"main case table {Removal.CASES}")
    seen = []
    for case, deps, purpose in Removal.CASES:
        expect(purpose.strip() != "", f"case {case} has no purpose")
        expect(case not in seen, f"duplicate case {case}")
        for dep in deps:
            expect(dep in seen, f"case {case} depends on {dep}, which is not earlier")
        seen.append(case)

    class PhaseRecorder:
        def __init__(self):
            self.phases = []

        def phase(self, label):
            self.phases.append(label)

    class FakeSession:
        def __init__(self):
            self.proof = PhaseRecorder()

        @staticmethod
        def body(label, outcome):
            def run_body():
                if outcome == "fail":
                    raise OracleFailure(f"{label} failed")
                if outcome == "abort":
                    raise UnknownOutcome(f"{label} unknown")
                return []
            return run_body

    def stub_bodies(**outcomes):
        calls = []

        def make(case):
            def run_body():
                calls.append(case)
                outcome = outcomes.get(case)
                if outcome == "fail":
                    raise OracleFailure(f"{case} failure")
                if outcome == "abort":
                    raise UnknownOutcome(f"{case} unknown")
                return []
            return run_body

        return {case: make(case) for case, _, _ in Removal.CASES}, calls

    # A recoverable failure in the first case skips every dependent and runs
    # nothing a missing premise could satisfy.
    session = FakeSession()
    outcome = diagnostic_outcome()
    bodies, calls = stub_bodies(idle="fail")
    collect(session, True, outcome, bodies)
    expect(outcome["completed"] == [], f"a failed case was completed: {outcome['completed']}")
    expect([case for case, _ in outcome["failed"]] == ["idle"], f"failed {outcome['failed']}")
    expect(
        outcome["skipped"] == [["held", ["idle"]], ["held-delete", ["held"]], ["quarantine", ["held-delete"]]],
        f"skipped {outcome['skipped']}",
    )
    expect(calls == ["idle"], f"calls {calls}")
    # An independent case is still recorded as skipped by dependency only; a
    # later independent case after a completed chain still runs.
    session = FakeSession()
    outcome = diagnostic_outcome()
    bodies, calls = stub_bodies(held="fail")
    collect(session, True, outcome, bodies)
    expect(outcome["completed"] == ["idle"], f"completed {outcome['completed']}")
    expect([case for case, _ in outcome["failed"]] == ["held"], f"failed {outcome['failed']}")
    expect(outcome["skipped"] == [["held-delete", ["held"]], ["quarantine", ["held-delete"]]],
           f"skipped {outcome['skipped']}")
    expect(outcome["first_fault"] == "held: held failure", f"first fault {outcome['first_fault']}")
    # An unknown outcome aborts and leaves the rest unexecuted.
    session = FakeSession()
    outcome = diagnostic_outcome()
    bodies, _ = stub_bodies(held="abort")
    try:
        collect(session, True, outcome, bodies)
    except UnknownOutcome:
        pass
    else:
        expect(False, "an unknown outcome did not abort the collection")
    expect(outcome["aborted"] == "held: held unknown", f"aborted {outcome['aborted']}")
    expect(outcome["unexecuted"] == ["held-delete", "quarantine"], f"unexecuted {outcome['unexecuted']}")
    # An unexpected exception aborts as unknown, never as a case pass.
    session = FakeSession()
    outcome = diagnostic_outcome()
    bodies, _ = stub_bodies()

    def explode():
        raise KeyError("unexpected")

    bodies["idle"] = explode
    try:
        collect(session, True, outcome, bodies)
    except KeyError:
        pass
    else:
        expect(False, "an unexpected exception did not abort the collection")
    expect(outcome["aborted"] is not None and "unexpected" in outcome["aborted"],
           f"aborted {outcome['aborted']}")
    expect(outcome["unexecuted"] == ["held", "held-delete", "quarantine"],
           f"unexecuted {outcome['unexecuted']}")
    # A timed-out child is an unknown writer outcome: it aborts the collection,
    # is never recorded as a recoverable failure, and the receipt keeps the
    # abort and the unexecuted set.
    session = FakeSession()
    outcome = diagnostic_outcome()

    def timed_out():
        raise subprocess.TimeoutExpired("oxide build", 5)

    bodies, _ = stub_bodies()
    bodies["idle"] = timed_out
    try:
        collect(session, True, outcome, bodies)
    except subprocess.TimeoutExpired:
        pass
    else:
        expect(False, "a timed-out child did not abort the collection")
    expect(outcome["aborted"] is not None and "timed out" in outcome["aborted"],
           f"aborted {outcome['aborted']}")
    expect(outcome["failed"] == [], f"a timeout was recorded as a recoverable failure: {outcome['failed']}")
    expect(outcome["completed"] == [], f"a timeout completed a case: {outcome['completed']}")
    expect(outcome["unexecuted"] == ["held", "held-delete", "quarantine"],
           f"unexecuted {outcome['unexecuted']}")
    holder = dict(os.environ)
    evidence = tempfile.mkdtemp(prefix="removal-diagnostic-check.")
    os.environ["OXIDE_PROOF_EVIDENCE"] = evidence
    try:
        with open(diagnostic_receipt(outcome, "removal.diagnostic"), encoding="utf-8") as handle:
            receipt = json.load(handle)
    finally:
        os.environ.clear()
        os.environ.update(holder)
        shutil.rmtree(evidence, ignore_errors=True)
    expect(receipt["qualified"] is False and receipt["aborted"] == outcome["aborted"],
           f"receipt abort {receipt.get('aborted')!r}")
    expect(receipt["unexecuted"] == outcome["unexecuted"], f"receipt unexecuted {receipt.get('unexecuted')}")
    expect(receipt["failed"] == [], f"receipt failures {receipt.get('failed')}")

    # A setup exception must record the real fault, the abort and all four
    # cases unexecuted, never an empty fault-less receipt. `run` is driven with
    # a stubbed `Removal` (an unexpected exception and a marker-style oracle
    # failure) and a stubbed proof, each with its own real evidence directory.
    class FakeProof:
        root = "/fixture/guard-root"

        def __init__(self):
            self.unclosed = set()
            self.mount = None

        source = "deadbeef" * 5  # 40 hex characters: a valid exact identity

        def run(self, command, *, cwd=None, env=None, timeout=None):
            if command[:2] == ["git", "rev-parse"]:
                return subprocess.CompletedProcess(
                    command, 0, stdout=(self.source + "\n").encode(), stderr=b""
                )
            if command[:2] == ["git", "status"]:
                return subprocess.CompletedProcess(command, 0, stdout=b"", stderr=b"")
            raise AssertionError(f"unexpected command {command}")

    for failure, expect_aborted in (
        (KeyError("OXIDE_PROOF_DEADLINE_NS"), True),
        (OracleFailure("candidate binary lacks the marker"), False),
    ):
        evidence = tempfile.mkdtemp(prefix="removal-setup-check.")
        os.environ["OXIDE_PROOF_EVIDENCE"] = evidence

        def explode(proof, repo, source=None, failure=failure):
            raise failure

        original = globals()["Removal"]
        # The stub keeps the real case table so `run`/`record_abort`/
        # `diagnostic_outcome` name the same four cases.
        explode.CASES = original.CASES
        globals()["Removal"] = explode
        try:
            try:
                run(FakeProof(), "/fixture/repo", True)
            except BaseException as raised:
                expect(raised is failure, f"setup re-raised {raised!r}")
            receipts = [name for name in os.listdir(evidence) if name.startswith("removal.diagnostic")]
            expect(len(receipts) == 1, f"setup wrote {len(receipts)} receipts")
            with open(os.path.join(evidence, receipts[0]), encoding="utf-8") as handle:
                receipt = json.load(handle)
        finally:
            globals()["Removal"] = original
            os.environ.pop("OXIDE_PROOF_EVIDENCE", None)
            shutil.rmtree(evidence, ignore_errors=True)
        expect(receipt["qualified"] is False, "a setup receipt claimed qualification")
        expect(receipt["source"] == "deadbeef" * 5, f"receipt source {receipt.get('source')!r}")
        expect(receipt["root"] == "/fixture/guard-root", f"receipt root {receipt.get('root')!r}")
        expect(
            receipt["unexecuted"] == ["idle", "held", "held-delete", "quarantine"],
            f"setup receipt unexecuted {receipt.get('unexecuted')}",
        )
        expect(receipt["completed"] == [], f"setup receipt completed {receipt.get('completed')}")
        if expect_aborted:
            expect(receipt["aborted"] is not None, f"a setup exception left no aborted fault: {receipt}")
            expect(receipt["failed"] == [], f"a setup abort was recorded as a failure: {receipt['failed']}")
        else:
            expect(
                [case for case, _ in receipt["failed"]] == ["setup"],
                f"a setup oracle failure was not recorded: {receipt['failed']}",
            )
            expect(receipt["aborted"] is None, f"a setup oracle failure recorded an abort: {receipt.get('aborted')}")

    # The source gate runs before staging and before any image effect. A
    # lookup that dies or is interrupted, a non-exact identity and a dirty
    # tracked worktree each abort with the real fault recorded and every planned
    # case unexecuted, and `Removal` (which stages and attaches) is never
    # constructed. A clean identity lets setup begin with exactly that source.
    class SourceProof:
        root = "/fixture/guard-root"

        def __init__(self, source: str = "a" * 40, status: str = "", failure=None, interrupt: bool = False,
                     head_returncode: int = 0, status_returncode: int = 0, status_stderr: str = ""):
            self.source, self.status, self.failure, self.interrupt = source, status, failure, interrupt
            self.head_returncode, self.status_returncode = head_returncode, status_returncode
            self.status_stderr = status_stderr
            self.unclosed = set()
            self.mount = None
            self.commands = []

        def run(self, command, *, cwd=None, env=None, timeout=None):
            self.commands.append(list(command))
            if command[:2] == ["git", "rev-parse"]:
                if self.interrupt:
                    raise KeyboardInterrupt()
                if self.failure is not None:
                    raise self.failure
                return subprocess.CompletedProcess(
                    command, self.head_returncode, stdout=(self.source + "\n").encode(), stderr=b""
                )
            if command[:2] == ["git", "status"]:
                return subprocess.CompletedProcess(
                    command,
                    self.status_returncode,
                    stdout=self.status.encode(),
                    stderr=self.status_stderr.encode(),
                )
            raise AssertionError(f"no command may run before the source gate: {command}")

    original_removal = globals()["Removal"]
    for label, case, needle in (
        ("lookup timeout", SourceProof(failure=subprocess.TimeoutExpired("git rev-parse", 30)), "source lookup failed"),
        ("interrupt", SourceProof(interrupt=True), "interrupted"),
        ("empty identity", SourceProof(source=""), "no exact source identity"),
        ("non-exact identity", SourceProof(source="deadbeef"), "no exact source identity"),
        (
            "dirty tracked worktree",
            SourceProof(status=" M scripts/test-oxide-removable-removal.py\n"),
            "tracked worktree is not clean",
        ),
        (
            # The reviewer's case: `layout.Proof.run` can return a nonzero
            # CompletedProcess with EMPTY stdout, which must never read as a
            # clean worktree.
            "nonzero status with empty stdout",
            SourceProof(status="", status_returncode=1, status_stderr="fatal: not a git repository"),
            "worktree status exited 1",
        ),
        (
            "nonzero source lookup",
            SourceProof(head_returncode=128),
            "source lookup exited 128",
        ),
    ):
        built = []

        def gate_stub(proof, repo, source, built=built):
            built.append(source)
            raise OracleFailure("stub removal reached")

        gate_stub.CASES = original_removal.CASES
        evidence = tempfile.mkdtemp(prefix="removal-source-check.")
        os.environ["OXIDE_PROOF_EVIDENCE"] = evidence
        globals()["Removal"] = gate_stub
        try:
            try:
                run(case, "/fixture/repo", True)
            except (UnknownOutcome, KeyboardInterrupt):
                pass
            else:
                fail(f"{label}: the source gate did not abort")
        finally:
            globals()["Removal"] = original_removal
            os.environ.pop("OXIDE_PROOF_EVIDENCE", None)
        expect(built == [], f"{label}: setup was constructed before the gate: {built}")
        expect(
            case.commands and all(command[:2] in (["git", "rev-parse"], ["git", "status"]) for command in case.commands),
            f"{label}: commands {case.commands}",
        )
        receipts = [name for name in os.listdir(evidence) if name.startswith("removal.diagnostic")]
        expect(len(receipts) == 1, f"{label}: {len(receipts)} receipts")
        with open(os.path.join(evidence, receipts[0]), encoding="utf-8") as handle:
            receipt = json.load(handle)
        shutil.rmtree(evidence, ignore_errors=True)
        expect(receipt["qualified"] is False, f"{label}: receipt claimed qualification")
        expect(
            receipt["aborted"] is not None and needle in receipt["aborted"],
            f"{label}: receipt abort {receipt.get('aborted')!r}",
        )
        expect(receipt["first_fault"] == receipt["aborted"], f"{label}: receipt fault {receipt.get('first_fault')!r}")
        expect(receipt["completed"] == [] and receipt["failed"] == [], f"{label}: receipt cases {receipt}")
        expect(
            receipt["unexecuted"] == [name for name, _, _ in original_removal.CASES],
            f"{label}: receipt unexecuted {receipt.get('unexecuted')}",
        )
        expect(receipt["root"] == "/fixture/guard-root", f"{label}: receipt root {receipt.get('root')!r}")

    built = []

    def gate_stub_ok(proof, repo, source, built=built):
        built.append(source)
        raise OracleFailure("stub removal reached")

    gate_stub_ok.CASES = original_removal.CASES
    clean_case = SourceProof()
    evidence = tempfile.mkdtemp(prefix="removal-source-ok.")
    os.environ["OXIDE_PROOF_EVIDENCE"] = evidence
    globals()["Removal"] = gate_stub_ok
    try:
        clean = run(clean_case, "/fixture/repo", True)
    finally:
        globals()["Removal"] = original_removal
        os.environ.pop("OXIDE_PROOF_EVIDENCE", None)
    shutil.rmtree(evidence, ignore_errors=True)
    expect(built == ["a" * 40], f"a clean source did not reach setup with its exact identity: {built}")
    expect(
        clean_case.commands
        == [["git", "rev-parse", "HEAD"], ["git", "status", "--porcelain", "--untracked-files=no"]],
        f"the source gate ran {clean_case.commands}",
    )
    expect(clean_case.head_returncode == 0 and clean_case.status_returncode == 0,
           "the clean fixture did not exit 0 on both git calls")
    expect(clean["source"] == "a" * 40, f"the outcome source is {clean.get('source')!r}")
    expect([case for case, _ in clean["failed"]] == ["setup"], f"clean case failures {clean['failed']}")
    expect(
        clean["unexecuted"] == [name for name, _, _ in original_removal.CASES],
        f"clean case unexecuted {clean['unexecuted']}",
    )

    # `Removal` itself refuses an unvalidated source before any child runs, so
    # no direct construction can stage or attach an image to an unpinned source.
    class BareProof:
        def run(self, command, **kwargs):
            raise AssertionError(f"no command may run before the source gate: {command}")

    for bad in ("", "not-a-sha", "A" * 40, "a" * 39):
        try:
            original_removal(BareProof(), "/fixture/repo", bad)
        except UnknownOutcome:
            pass
        else:
            fail(f"Removal accepted the unvalidated source {bad!r}")

    # The premise checklist is complete, and its environment claims are
    # checked against `private_environment` with hostile inherited values.
    expect(len(PREMISE_CHECKLIST) == 9, f"premise checklist has {len(PREMISE_CHECKLIST)} items")
    names = [item for item, _ in PREMISE_CHECKLIST]
    expect(len(set(names)) == 9, f"duplicate premise names {names}")
    for item, disposition in PREMISE_CHECKLIST:
        expect(disposition.strip() != "", f"premise {item} has no disposition")
    holder = dict(os.environ)
    # The fixture root sits inside the guard root it hands to
    # `private_environment`, exactly as `proof.root` does in a real run, so the
    # contained-declaration assertion below is about the real relationship.
    fixture_root = os.path.join(root, "scratch", "fixture-env")
    os.makedirs(os.path.join(root, "scratch"), mode=0o700, exist_ok=True)
    os.environ.update(
        {
            "TMPDIR": os.path.join(root, "scratch"),
            "OXIDE_POOL": "/fixture/host-pool",
            "OXIDE_POOL_CAP": "9MiB",
            "OXIDE_SCRATCH_ROOT": "/fixture/host-scratch-pools",
            "OXIDE_CONFIG": "/fixture/host-config",
            "OXIDE_TEST_HOME_FROM_ENV": "0",
            "XDG_CONFIG_HOME": "/fixture/xdg",
            "RUSTFLAGS": "-Ctarget-cpu=native",
            "CARGO_ENCODED_RUSTFLAGS": "-Ctarget-cpu=native",
            "RUSTUP_TOOLCHAIN": "nightly",
            "SCCACHE_DIR": "/fixture/sccache",
            "CARGO_BUILD_RUSTC_WRAPPER": "/fixture/wrapper",
            "CARGO_TARGET_DIR": "/fixture/target",
            "CARGO_UNSTABLE_ALLOW_FEATURES": "fixture",
            "CARGO_REGISTRIES_FIXTURE_INDEX": "/fixture/index",
            "CARGO_BUILD_JOBS": "9",
            "SCCACHE_BUCKET": "/fixture/bucket",
        }
    )
    try:
        env = private_environment(fixture_root)
        # The run root is canonicalized once, so every fixture path and the
        # scratch pool are canonical even when the guard root is reached
        # through a symlinked component (the self-check's own /var fixture
        # exercises exactly that).
        canonical_root = os.path.realpath(fixture_root)
        guard_root = os.path.realpath(os.environ["TMPDIR"])
        expect(env["OXIDE_TEST_HOME_FROM_ENV"] == "1", "account-home control was not set")
        expect(env["HOME"] == os.path.join(canonical_root, "home"), "HOME is not private")
        expect(env["CARGO_HOME"] == os.path.join(canonical_root, "cargo-home"), "CARGO_HOME is not private")
        expect(env["RUSTUP_TOOLCHAIN"] == runner.BUILDER, "the toolchain is not pinned")
        expect(env["RUSTUP_AUTO_INSTALL"] == "0", "auto-install is not disabled")
        expect(env["RUSTFLAGS"] == FROZEN_RUSTFLAGS, f"flags not frozen: {env['RUSTFLAGS']!r}")
        expect("CARGO_ENCODED_RUSTFLAGS" not in env, "encoded flags were inherited")
        for name in ("TMPDIR", "TMP", "TEMP"):
            expect(env[name] == guard_root, f"{name} is not the guard's scratch")
        # The scratch-pool contract: one private canonical root inside the
        # monitored guard root, the pool strictly inside that root with an
        # existing parent, and an explicit cap. No inherited declaration or
        # budget may survive.
        scratch_pool = os.path.join(canonical_root, SCRATCH_POOL_ROOT)
        expect(
            env["OXIDE_SCRATCH_ROOT"] == scratch_pool,
            f"the fixture scratch root is {env['OXIDE_SCRATCH_ROOT']!r}, not its own {scratch_pool!r}",
        )
        expect(
            env["OXIDE_SCRATCH_ROOT"].startswith(guard_root + os.sep),
            f"the scratch root {env['OXIDE_SCRATCH_ROOT']!r} escapes the monitored guard root {guard_root!r}",
        )
        expect(
            os.path.realpath(env["OXIDE_SCRATCH_ROOT"]) == env["OXIDE_SCRATCH_ROOT"],
            "the fixture scratch root is not canonical",
        )
        expect(
            os.path.isdir(env["OXIDE_SCRATCH_ROOT"])
            and stat.S_IMODE(os.stat(env["OXIDE_SCRATCH_ROOT"]).st_mode) == 0o700,
            f"the fixture scratch root is not one private 0700 directory: "
            f"{os.stat(env['OXIDE_SCRATCH_ROOT']).st_mode:o}",
        )
        expect(
            env["OXIDE_POOL"].startswith(env["OXIDE_SCRATCH_ROOT"] + os.sep)
            and env["OXIDE_POOL"] != env["OXIDE_SCRATCH_ROOT"],
            f"the fixture pool {env['OXIDE_POOL']!r} is not strictly inside the scratch root",
        )
        expect(
            os.path.isdir(os.path.dirname(env["OXIDE_POOL"])),
            "the fixture pool's parent does not exist under the scratch root",
        )
        expect(env["OXIDE_POOL_CAP"] == SCRATCH_POOL_CAP, "the fixture cap is not its own")
        expect(env["CARGO_BUILD_RUSTC_WRAPPER"] == "", "an inherited wrapper reached the fixture")
        for name in (
            "OXIDE_CONFIG",
            "XDG_CONFIG_HOME",
            "SCCACHE_DIR",
            "SCCACHE_BUCKET",
            "CARGO_TARGET_DIR",
            "CARGO_UNSTABLE_ALLOW_FEATURES",
            "CARGO_REGISTRIES_FIXTURE_INDEX",
            "CARGO_BUILD_JOBS",
        ):
            expect(name not in env, f"inherited {name} was copied into the fixture")

        # Staging runs in the scrubbed process environment: every inherited
        # OXIDE_/XDG_/CARGO_/SCCACHE_/RUSTC* key is gone, only the invoking
        # Cargo home is restored (explicitly), and the process environment is
        # exactly restored afterwards, including when the body raises.
        staging_root = os.path.join(root, "fixture-staging")
        os.makedirs(os.path.join(staging_root, "home"), mode=0o700, exist_ok=True)
        hostile = {
            "OXIDE_POOL": "/fixture/host-pool",
            "OXIDE_CONFIG": "/fixture/host-config",
            "OXIDE_TEST_HOME_FROM_ENV": "0",
            "XDG_CONFIG_HOME": "/fixture/xdg",
            "CARGO_UNSTABLE_ALLOW_FEATURES": "fixture",
            "CARGO_REGISTRIES_FIXTURE_INDEX": "/fixture/index",
            "CARGO_TARGET_DIR": "/fixture/target",
            "SCCACHE_DIR": "/fixture/sccache",
            "RUSTC_WRAPPER": "/fixture/wrapper",
        }
        os.environ.update(hostile)
        os.environ["CARGO_HOME"] = "/fixture/invoking-cargo-home"
        # The guard's own control variables are not fixture configuration: the
        # shared staging helper bounds every child with the deadline and writes
        # under the evidence directory, so the scrub must keep them visible.
        os.environ["OXIDE_PROOF_DEADLINE_NS"] = str(
            time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) + 300 * 10**9
        )
        os.environ["OXIDE_PROOF_EVIDENCE"] = os.path.join(root, "fixture-evidence")
        os.makedirs(os.environ["OXIDE_PROOF_EVIDENCE"], mode=0o700, exist_ok=True)
        before = dict(os.environ)
        seen = {}
        left = 0.0
        with staging_environment_scrubbed():
            seen = dict(os.environ)
            # A real exercise of the path that broke: the shared helper's own
            # bounded-deadline call must not raise inside the scrub.
            left = layout.work_left()
        expect(left > 0, f"layout.work_left() inside the scrub returned {left}")
        expect(
            seen.get("OXIDE_PROOF_DEADLINE_NS") == before["OXIDE_PROOF_DEADLINE_NS"]
            and seen.get("OXIDE_PROOF_EVIDENCE") == before["OXIDE_PROOF_EVIDENCE"],
            f"the staging scrub hid the guard controls: {sorted(set(GUARD_CONTROLS) - set(seen))}",
        )
        expect(os.environ == before, "the staging scrub did not restore the process environment")
        expect(seen.get("CARGO_HOME") == "/fixture/invoking-cargo-home",
               "staging did not restore the invoking Cargo home explicitly")
        for name in (
            "OXIDE_POOL",
            "OXIDE_CONFIG",
            "OXIDE_TEST_HOME_FROM_ENV",
            "XDG_CONFIG_HOME",
            "CARGO_UNSTABLE_ALLOW_FEATURES",
            "CARGO_REGISTRIES_FIXTURE_INDEX",
            "CARGO_TARGET_DIR",
            "SCCACHE_DIR",
            "RUSTC_WRAPPER",
        ):
            expect(name not in seen, f"staging inherited {name}")
        try:
            with staging_environment_scrubbed():
                raise RuntimeError("fixture body failure")
        except RuntimeError:
            pass
        expect(os.environ == before, "the staging scrub did not restore after an exception")
        os.environ.clear()
        os.environ.update(holder)
        expect(env["RUSTUP_HOME"] == (holder.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")),
               "the invoking rustup home was not kept")
    finally:
        os.environ.clear()
        os.environ.update(holder)

    # Both modes take their fixture environment from the same helper, so no
    # mode can keep an undeclared pool, and the four selectors pass no explicit
    # internal root: the guard's selected-volume default supplies the monitored
    # root that the declaration must live inside, and the bounds are unchanged.
    for label, constructor in (("Removal", Removal.__init__), ("BootClosure", BootClosure.__init__)):
        expect(
            "private_environment(proof.root)" in inspect.getsource(constructor),
            f"{label} does not take its fixture environment from private_environment",
        )
    with open(os.path.join(os.path.dirname(HERE), "effigy.toml"), encoding="utf-8") as handle:
        manifest = handle.read()

    def selector_run(name: str) -> str:
        marker = f'[tasks."{name}"]\n'
        expect(manifest.count(marker) == 1, f"selector {name} is not declared exactly once")
        for row in manifest.split(marker, 1)[1].splitlines()[1:]:
            if row.startswith("run = "):
                return row
            expect(not row.startswith("["), f"selector {name} has no run line")
        fail(f"selector {name} has no run line")

    for task in (
        "rust:oxide-removable-removal",
        "rust:oxide-removable-removal:diagnostic",
        "rust:oxide-removable-boot-closure",
        "rust:oxide-removable-boot-closure:diagnostic",
    ):
        task_run = selector_run(task)
        expect("scripts/test-oxide-removable-removal.py" in task_run, f"{task} does not run this proof")
        expect("bounded-test.py" in task_run, f"{task} is not the whole command of the qualified guard")
        expect("--root-parent" not in task_run, f"{task} still passes an explicit internal root")
        expect(
            "--limit 300" in task_run and "--scratch-budget-mib 512" in task_run,
            f"{task} changed its wall clock or scratch budget: {task_run}",
        )

    # The S4 pre-review oracle sweep is a real effect-free gate: every product
    # line this proof matches is asserted against the source file that carries
    # it, so a reworded product message fails here instead of in a native round.
    needles = [(relative, needle) for _, relative, needle in PRODUCT_ORACLES]
    expect(len(set(needles)) == len(needles), "the product oracle table repeats an entry")
    expect(
        check_product_oracles() == len(PRODUCT_ORACLES),
        "the product oracle sweep did not check every pinned line",
    )

    # The shared staging helper's narrowly scoped retention opt-in, exercised
    # through the REAL `layout.stage_at` route with a fake staged build (no
    # Cargo): the default removes the staged tree exactly as before, and the
    # opt-in this proof passes keeps it for the guard's post-leader census.
    class StageProof:
        def __init__(self, root: str):
            self.root = root

        def run(self, command, *, cwd=None, env=None, timeout=None):
            built = os.path.join(env["CARGO_TARGET_DIR"], "debug", "oxide")
            os.makedirs(os.path.dirname(built), exist_ok=True)
            with open(built, "wb") as handle:
                handle.write(b"oxide")
            line = json.dumps(
                {"reason": "compiler-artifact", "target": {"name": "oxide"}, "executable": built}
            )
            return subprocess.CompletedProcess(command, 0, stdout=(line + "\n").encode(), stderr=b"")

    for retain, expect_tree in ((False, False), (True, True)):
        helper_root = os.path.realpath(tempfile.mkdtemp(prefix="removal-stage-check."))
        try:
            copy = layout.stage_at(
                StageProof(helper_root), "/fixture/source", "check", jobs=1,
                retain_stage_root=retain,
            )
            expect(os.path.isfile(copy), f"stage_at did not copy {copy}")
            tree = os.path.join(helper_root, "stage-check", "target", "debug", "oxide")
            expect(
                os.path.isfile(tree) is expect_tree,
                f"stage_at retain_stage_root={retain} left the staged tree={os.path.isfile(tree)}",
            )
        finally:
            shutil.rmtree(helper_root, ignore_errors=True)
    # Every removal-mode staging call opts in, so both modes keep the tree.
    source_text = open(os.path.abspath(__file__), encoding="utf-8").read()
    main_source = source_text
    staging_calls = [
        line.strip()
        for line in source_text.splitlines()
        if line.strip().startswith("candidate = layout.stage_at(")
    ]
    expect(len(staging_calls) == 2, f"expected two removal-mode staging calls, saw {staging_calls}")
    expect(
        all("retain_stage_root=True" in line for line in staging_calls),
        f"not every removal-mode staging call retains its tree: {staging_calls}",
    )

    # A failure after any writer launch keeps the mount and backing for the
    # guard's post-leader census; only a child-free failure detaches, and a
    # successful collection still detaches ordinarily (pinned below).
    class CleanupProof:
        def __init__(self, unclosed: tuple = ()):
            self.unclosed = set(unclosed)
            self.detached = False

        def detach(self):
            self.detached = True

    WRITERS["launched"] = False
    clean = CleanupProof()
    expect(cleanup_after_failure(clean) is False and clean.detached,
           "a child-free failure did not run the ordinary detach")
    WRITERS["launched"] = True
    dirty = CleanupProof()
    expect(cleanup_after_failure(dirty) is True and not dirty.detached,
           "a failure after a launch detached the mount")
    WRITERS["launched"] = False
    unreaped = CleanupProof(unclosed=(4242,))
    expect(cleanup_after_failure(unreaped) is True and not unreaped.detached,
           "a failure with an unreaped child detached the mount")
    WRITERS["launched"] = False
    boot_source = inspect.getsource(boot_closure)
    expect("ordinary detach" not in boot_source,
           "boot_closure claims a detach before the top-level custody decision")
    expect("{custody}" in boot_source,
           "boot_closure does not carry the custody placeholder for the top-level decision")
    expect("layout.record(" not in boot_source,
           "boot_closure records its own result line before custody is decided")
    source_text = open(os.path.abspath(__file__), encoding="utf-8").read()
    handler = main_source[main_source.index("    except BaseException as failure:\n        retained = "):]
    expect("cleanup_after_failure(proof)" in handler,
           "main's failure path does not use the custody decision")
    expect(
        handler.index("cleanup_after_failure(proof)") < handler.index("layout.record("),
        "main records the failure before deciding custody",
    )

    # The REAL `main` path, for both applicable modes: a diagnostic collection
    # that RETURNS with a recorded failure (no exception propagates) must claim
    # no detach and leave the image to the guard's post-leader census, while a
    # fully clean counted collection reports the guard's post-census request —
    # and neither path detaches in-script (oxide#191), because a direct-child
    # reap is not descendant-writer closure.
    class MainProof:
        instances = []
        extra_unclosed = ()

        def __init__(self, root):
            self.root = root
            self.unclosed = set(MainProof.extra_unclosed)
            self.mount = None
            self.peak = 0
            self.low = 1 << 40
            self.detached = False
            MainProof.instances.append(self)

        def detach(self):
            self.detached = True

        def phase(self, label):
            return None

    cases = [case for case, _, _ in Removal.CASES]
    failing = {
        "version": "x", "mode": "diagnostic", "qualified": False, "cases": cases,
        "reached": ["idle"], "completed": [], "failed": [["idle", "boom"]],
        "skipped": [], "unexecuted": ["held", "held-delete", "quarantine"],
        "first_fault": "idle: boom", "aborted": None, "unclosed": [],
    }
    clean = {
        "version": "x", "mode": "diagnostic", "qualified": False, "cases": cases,
        "reached": cases, "completed": cases, "failed": [], "skipped": [],
        "unexecuted": [], "first_fault": None, "aborted": None, "unclosed": [],
    }
    recorded = {}
    saved_main = {
        "Proof": globals()["Proof"],
        "run": globals()["run"],
        "boot_closure": globals()["boot_closure"],
        "argv": sys.argv,
        "record": layout.record,
        "work_left": layout.work_left,
    }
    holder = dict(os.environ)
    main_scratch = os.path.realpath(tempfile.mkdtemp(prefix="removal-main-check."))
    os.makedirs(os.path.join(main_scratch, "evidence"), mode=0o700)
    try:
        os.environ.update(
            {
                "OXIDE_TASK_SCRATCH_ROOT": main_scratch,
                "OXIDE_PROOF_EVIDENCE": os.path.join(main_scratch, "evidence"),
                "TMPDIR": main_scratch,
                "TMP": main_scratch,
                "TEMP": main_scratch,
                "OXIDE_PROOF_DEADLINE_NS": str(
                    time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) + 300 * 10**9
                ),
            }
        )
        os.environ.pop("RUSTFLAGS", None)
        os.environ.pop("CARGO_ENCODED_RUSTFLAGS", None)
        layout.record = lambda name, body: recorded.update({name: body})
        layout.work_left = lambda: 300.0
        globals()["Proof"] = MainProof
        for argv, body, unclosed, expect_request, label, main_mode in (
            (["x", "--diagnostic"], failing, (), False, "main-mode diagnostic failure", True),
            (["x", "--boot-closure", "--diagnostic"], failing, (), False,
             "boot-closure diagnostic failure", False),
            (["x"], clean, (), True, "counted clean collection", True),
            (["x", "--diagnostic"], clean, (9999,), False, "unreaped child with an otherwise clean outcome", True),
        ):
            MainProof.instances.clear()
            MainProof.extra_unclosed = unclosed
            WRITERS["launched"] = False
            note_writer_launch()

            def fake_run(proof, repo, diagnostic, body=body):
                return dict(body)

            def fake_boot(proof, repo, diagnostic, body=body):
                # The real `boot_closure` returns a `{custody}` template and
                # never claims the detach itself; the top-level decision fills
                # it in.
                return ("boot-closure stub; one owned 128 MiB scratch image, {custody}", dict(body))

            globals()["run"] = fake_run
            globals()["boot_closure"] = fake_boot
            sys.argv = argv
            recorded.clear()
            code = main()
            expect(code == 0, f"{label}: main returned {code}")
            instance = MainProof.instances[-1]
            expect(not instance.detached,
                   f"{label}: main detached in-script; custody belongs to the guard's post-census step")
            line = recorded.get("result.txt", "")
            if expect_request:
                expect("ordinary detach requested for the qualified guard's post-census step" in line,
                       f"{label}: the recorded line does not report the guard's post-census request: {line!r}")
                expect("left to the qualified guard" not in line,
                       f"{label}: a clean collection claims retention: {line!r}")
            else:
                expect("detach request left to the qualified guard" in line,
                       f"{label}: the recorded line does not report the retained custody: {line!r}")
                expect("ordinary detach requested" not in line,
                       f"{label}: the recorded line claims a detach this script did not request: {line!r}")
            expect("{custody}" not in line, f"{label}: the custody placeholder was recorded: {line!r}")

    finally:
        MainProof.extra_unclosed = ()
        globals()["Proof"] = saved_main["Proof"]
        globals()["run"] = saved_main["run"]
        globals()["boot_closure"] = saved_main["boot_closure"]
        sys.argv = saved_main["argv"]
        layout.record = saved_main["record"]
        layout.work_left = saved_main["work_left"]
        WRITERS["launched"] = False
        os.environ.clear()
        os.environ.update(holder)
        shutil.rmtree(main_scratch, ignore_errors=True)

    # Dispatch: only the documented argument lists select a mode.
    expect(mode_from_args([]) == (False, False, False), "no-argument mode")
    expect(mode_from_args(["--diagnostic"]) == (False, True, False), "main diagnostic mode")
    expect(mode_from_args(["--plan"]) == (False, False, True), "plan mode")
    expect(mode_from_args(["--boot-closure"]) == (True, False, False), "focused mode")
    expect(mode_from_args(["--boot-closure", "--diagnostic"]) == (True, True, False), "boot diagnostic mode")
    expect(mode_from_args(["--diagnostic", "--boot-closure"]) == (True, True, False), "boot diagnostic, reversed")
    for refused in (
        ["--bogus"],
        ["--self-check", "--boot-closure"],
        ["--boot-closure", "--diagnostic", "--diagnostic"],
        ["--diagnostic", "--plan"],
        ["--plan", "--boot-closure"],
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
        "owners-on attach and mount observation, staging and fixture environment "
        "scrub, scratch-pool declaration and selector environment contract, "
        f"product oracle sweep ({len(PRODUCT_ORACLES)} lines), "
        "timeout/abort driver semantics, dispatch, marker"
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
    focused, diagnostic, plan_mode = mode
    if plan_mode:
        return plan()
    if sys.platform != "darwin":
        fail("the removal proof requires macOS")
    # Premise 7: an inherited compiler-flag value must not reach staging or a
    # fixture. Staging inherits this process environment, so refuse it here.
    for name in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
        expect(not os.environ.get(name), f"{name} is inherited; the candidate and fixtures would not share one frozen flag set")
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
    outcome = None
    retained = False
    focused_line = ""
    try:
        if focused:
            focused_line, outcome = boot_closure(proof, repo, diagnostic)
        else:
            outcome = run(proof, repo, diagnostic)
        retained = retain_for_census(proof, outcome)
        if retained:
            custody = ("detach request left to the qualified guard, which detaches only after its "
                       "post-leader census and otherwise keeps the mount, backing and evidence")
            print("collection recorded a failure or an unreaped child: the guard's post-census decision, "
                  "not this script, decides whether the mount is detached", file=sys.stderr)
        else:
            custody = "ordinary detach requested for the qualified guard's post-census step"
    except BaseException as failure:
        retained = cleanup_after_failure(proof)
        layout.record("result.txt", f"failed: {failure!r}\nunclosed children: {sorted(proof.unclosed)}\nmount: {proof.mount}\nbacking retained for the guard: {retained}\nroot: {root}\n")
        print(f"removal proof failed after {time.monotonic() - runner.STARTED:.0f} s; scratch {root} left to the runner", file=sys.stderr)
        raise
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
    proof.phase("end")
    if focused:
        # The line is recorded here, after the top-level custody decision, so
        # it can never claim a detach that was replaced by retention.
        line = focused_line.format(custody=custody)
        layout.record("result.txt", f"{'diagnostic (not a PASS)' if diagnostic else 'passed'}\n{line}\n")
        print(line)
        return 0
    elapsed = time.monotonic() - runner.STARTED
    if diagnostic:
        line = (
            f"removal diagnostic collected (UNCOUNTED, never a PASS): "
            f"{len(outcome['completed'])}/{len(Removal.CASES)} cases completed, failed "
            f"{[case for case, _ in outcome['failed']]}, skipped "
            f"{[case for case, _ in outcome['skipped']]}, unexecuted {outcome['unexecuted']}; "
            f"{elapsed:.0f} s script time, peak owned {proof.peak // MIB} MiB at phase checks, "
            f"host free minimum {proof.low // MIB} MiB; one owned 128 MiB scratch image, "
            f"{custody}"
        )
        layout.record("result.txt", f"diagnostic (not a PASS)\n{line}\n")
        print(line)
        return 0
    line = (
        f"removal proof held: {elapsed:.0f} s script time, peak owned {proof.peak // MIB} MiB at phase checks, "
        f"host free minimum {proof.low // MIB} MiB; scratch closure and cleanup are the shared runner's; "
        f"{custody}"
    )
    layout.record("result.txt", f"passed\n{line}\n")
    print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
