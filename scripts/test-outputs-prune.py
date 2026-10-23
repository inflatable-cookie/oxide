#!/usr/bin/env python3
"""Owned-output prune proof (oxide#129): the fixed 21-case native script.

Run only as the Effigy heavy task `rust:outputs-prune`, whose whole command is
`python3 scripts/bounded-test.py --limit 360 --scratch-budget-mib 512 --
python3 scripts/test-outputs-prune.py` under the qualified shared supervisor
(`scripts/bounded-test.py`, SHA-256
d89115f1ca03587e1d9134efa153cc9c94860d2904f52e2a11bc8c8fbe21f25e). The
supervisor owns the 360 s deadline (root-approved one larger bound before
any activation; the historical 300 s estimate is retained), the 512 MiB
monitored scratch, process closure and cleanup; work stops 30 s before its
exported deadline, and every case start and the ordinary detach check the
remaining work window first. Activation
is a separate counted owner gate for the author and for the Elevated
reviewer; `--plan` prints the allocation and case list and touches nothing.

Static allocation (one 128 MiB owned APFS sparse image, private HOME,
CARGO_HOME and directory pool; no real Builds drive, consumer, Windows,
provider or install):

    staging (constrained debug oxide recipe)      288 MiB
    APFS sparse image (payload + quotas)          128 MiB
    fixtures (incl. ~2.7 MiB old-binary copy)      32 MiB
    evidence                                       32 MiB
    margin                                         32 MiB
    ---------------------------------------------  ------
    total                                         512 MiB

Staging is the rust:installed-handoff recipe (this HEAD's debug `oxide`,
plain Cargo offline with 1.97.1, -j1, one codegen unit, no debug info,
symbols stripped, opt-level s, debug assertions kept so the debug-only
OXIDE_TEST_PRUNE_CRASH and OXIDE_TEST_PAUSE hooks exist; the staged target is
deleted after the copy).

Static time allocation, UNMEASURED (a successful run must finish everything,
including the ordinary detach, by the exported deadline minus 30 s, with the
detach helper's 5 s refusal margin: about 325 s of the 360 s limit):

    supervisor start offset                         <= 5 s
    staging (constrained debug oxide recipe)       37-60 s
    image attach, registration, first build        10-20 s
    old-binary verify/copy, probe, two refusals     2-5 s
    cases 01-21 (base 60-120 s + source-fit
      additions: gated writer build, kappa and
      recreated builds, image checkout, 600-entry
      root, extra prune runs, status reads)       95-165 s
    ordinary detach                                 2-10 s
    ---------------------------------------------  --------
    total                                        151-265 s   (~60 s margin)

No timing fit is claimed. An overrun fails at the deadline (never a timeout
success); no split, extra session or automatic retry.

Prune-worker monitoring call chain exercised by the deletion cases (product
source, not a bespoke harness): prune::prune_one -> Store::begin_clean ->
lock_binding -> recheck_candidate/recheck_receiptless -> RootSession::open
(removable::Binding::open + UUID, device, mount; registration) ->
RootSession::check(control, deleted) [internal control volume host reserve,
Binding::verify, registered reserve; unknown readings stop] before the
rename -> journal_intent -> rename -> finish_quarantine: Deleter::check
before the first and after the last delete and before the quarantine leaf
removal, Deleter::check after each directory's names are sorted,
check_due (>= 1 s, watchdog INTERVAL) before each enumeration, every 512
names while listing and after each unlink, and a forced check every 512
deleted entries. Any stop leaves the intent pending (exit 3). The debug-only
injections OXIDE_TEST_PRUNE_LOW_RESERVE_AT / _AFTER_ENTRIES drive the
pre-rename refusal and the mid-batch stop.

Cases (21 = the original 19-case matrix plus two resolve-intent cases), each
on tiny owned fixtures with exact exit codes and state assertions:

  fence / init (2)
    01 init: adopt publishes state version 3 with oxide.outputs/2 and a ready
       transaction in both controls, and oxide.outputs.registry/1 exists.
       Older-binary control (root-authorized read-only artifact): the exact
       installed d246d400 binary and its install receipt are verified (pin,
       SHA-256 bd52cb42..., size, no-follow regular owned files, metadata
       stable across the read), copied into owned scratch, and only the copy
       runs: its capability probe reports control state 2, and its `check`
       and `outputs adopt` in the fixture both exit nonzero with no handoff,
       no host path and complete pool, volume-tree and checkout bytes and
       metadata unchanged.
    02 missing ready registry: dry run exits 2, adopt of another checkout
       refuses, nothing is rebuilt; restoring the exact bytes recovers.
  nonce / reservation / cancel / recreation key (4)
    03 root nonce: the external root's v2 marker nonce (32 hex) equals the
       receipt's selected nonce and the registry row's nonce.
    04 shared reservation: two starters paused after slot keying (their
       captured destination keys compared before release) share
       one reserved nonce; both bind it and the created root carries it.
    05 cancel + stale starter: a starter paused after keying has its reservation cancelled by
       `outputs cancel-reservation`; the starter refuses before Cargo with
       "cancelled or replaced"; the root is never created; the row is gone.
    06 recreation: a pruned checkout re-created at the same path adopts a
       fresh root nonce different from the deleted root's recorded nonce, and
       its first build keys a different, cold slot.
  registry / snapshot / overflow (3)
    07 dry run: canonical preview bytes on stdout match the SHA-256 on
       stderr, schema prune-preview/1, control bytes unchanged.
    08 malformed receipt: one corrupt receipt keeps the whole batch.
    09 overflow: a sticky-incomplete registry keeps every root and a new
       reservation carries an overflow tag naming the registry; the starter
       binds it with no invented row. Run last: the flag is never cleared.
  eligibility / operator list / absence (3)
    10 live checkout kept; a live (present) detached checkout's root kept;
       the same released root is a candidate once its checkout is gone; a
       registered checkout on the attached image is a candidate when gone
       (absence through the verified mount and recorded UUID).
    11 unreadable ancestor kept: a debug-only EACCES (OXIDE_TEST_PRUNE_EACCES,
       bound to the supervisor's scratch/evidence layout) keeps the root, its
       firing is recorded in evidence, and the same fixture without injection
       is a genuine-absence candidate; no real unreadable directory exists.
       A listed relative path and a duplicate refuse with exit 2.
    12 receiptless legacy roots: the one whose checkout is on the boot Data
       volume (device + APFS UUID proof) gets its exclusive pruning receipt
       and is deleted (receipt `pruned`/`deleted`); the one whose checkout is
       on the attached scratch image, a foreign directory and a marker whose
       checkout identity is not its path digest are kept, bytes unchanged.
  custody / containment / removal (2)
    13 active writer: a build script holds Cargo Running in a vanished
       checkout; its root is kept with a writer reason.
    14 containment and reserve: a root replaced by a symlink is kept; a
       symlink descendant stops deletion (exit 3, target untouched) and
       replay completes; an injected low reserve at the pre-rename check
       refuses (exit 2, root bytes unchanged, nothing journaled); one between
       delete batches of a 600-entry root stops (exit 3) with some but not
       all entries deleted, and replay completes.
  crash boundaries (4)
    15 after intent: exit 3, root in place; replay rolls back and deletes.
    16 after rename: exit 3, quarantine present; a recreated checkout's
       build refuses while the intent is unsettled; replay completes.
    17 during delete: exit 3; replay completes.
    18 after registry terminal: exit 3, receipt not yet pruned; replay
       settles the receipt.
  completion / diagnostics / logical-allocated (1)
    19 completion: deleted root, registry row and intent gone, tombstone,
       prune-observations recorded; logical bytes equal an independent
       by-name oracle and allocated bytes equal an independent unique-inode
       oracle (a hard-linked 64 KiB inode counted once; complete coverage);
       `oxide status` shows the observation.
  resolve-intent (2)
    20 both leaves absent: resolution receipt, `resolved-by-operator`.
    21 quarantine present: exit 2, intent unchanged; replay then completes.

Not run here: an older binary refusing state version 3 (rust:installed-handoff
covers the numeric peek), real-drive qualification, live inventory or cleanup.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import glob
import hashlib
import importlib.util
import json
import os
import secrets
import shutil
import signal
import stat
import subprocess
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))


def _load(name: str, file: str):
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, file))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SUPERVISOR = "d89115f1ca03587e1d9134efa153cc9c94860d2904f52e2a11bc8c8fbe21f25e"
LIMIT_SECONDS = 360
SCRATCH_MIB = 512
CASES = 21

ALLOCATION = [
    ("staging (constrained debug oxide)", 288),
    ("APFS sparse image", 128),
    ("fixtures", 32),
    ("evidence", 32),
    ("margin", 32),
]

CASE_ALLOCATION = [
    ("forward fence + initialization", 2),
    ("nonce / reservation / cancel / recreation key", 4),
    ("registry / snapshot / overflow", 3),
    ("eligibility / operator list / absence", 3),
    ("custody / containment / removal", 2),
    ("crash boundaries", 4),
    ("completion / diagnostic / logical-allocated", 1),
    ("resolve-intent", 2),
]

PLAIN = '[package]\nname = "plain"\nversion = "0.1.0"\nedition = "2021"\n'

# The older-binary fence control (Opus 9a6af8bb, root-authorized READ-ONLY):
# the exact installed d246d400 binary and its install receipt. They are read
# with no-follow bounded reads, verified, copied into owned scratch and only
# the COPY is executed, never against the host pool.
OLD_BINARY = "/Users/tom/.local/bin/oxide"
OLD_RECEIPT = "/Users/tom/.local/state/oxide/install/20261023T095314Z-d246d400f5cc.json"
OLD_PIN = "d246d400f5ccb8e0a601ea6415b93b8346bc2698"
OLD_SHA = "bd52cb42842c06451b0f8a9bc364ba7de74cfe6f014d33804890c4adbc51fc2b"
OLD_BINARY_CAP = 8 * 1024 * 1024
OLD_RECEIPT_CAP = 64 * 1024
CAPABILITIES_PROBE = "__oxide-build-capabilities"
HANDOFF_NOTICE = "handing off to the installed binary"
# A build script that holds Cargo running until its gate releases it
# (bounded at 60 s), so a writer is positively Running, not pre-launch.
GATED_BUILD = r"""
use std::{env, fs, path::Path, thread, time::{Duration, Instant}};
fn main() {
    println!("cargo:rerun-if-env-changed=PRUNE_GATE");
    let Ok(gate) = env::var("PRUNE_GATE") else { return };
    let _ = fs::write(Path::new(&gate).join("started"), b"");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !Path::new(&gate).join("release").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
}
"""
DONE: list[str] = []


def plan() -> int:
    print("oxide#129 owned-output prune proof plan")
    print(f"supervisor bounded-test.py sha256 {SUPERVISOR}")
    print(f"wall limit {LIMIT_SECONDS} s; monitored owned scratch {SCRATCH_MIB} MiB")
    print("static allocation:")
    total = 0
    for label, mib in ALLOCATION:
        total += mib
        print(f"  {label:<42} {mib:>4} MiB")
    print(f"  {'total':<42} {total:>4} MiB")
    print(f"cases ({CASES}):")
    for label, count in CASE_ALLOCATION:
        print(f"  {count:>2}  {label}")
    assert sum(count for _, count in CASE_ALLOCATION) == CASES
    assert total == SCRATCH_MIB
    print("activation: separate counted owner gate per side; --plan touches nothing")
    return 0


# Loaded only for a real run, so `--plan` imports nothing that executes.
permanent = None
handoff = None
expect = fail = None


def case(name: str) -> None:
    # Every case starts inside the exported work window (fails otherwise).
    permanent.work_left()
    DONE.append(name)
    print(f"===== case {len(DONE):02d}: {name} =====", flush=True)


class World:
    """The fixture: one staged binary, private HOME/CARGO_HOME/pool and one
    owned scratch volume registered in permanent mode."""

    def __init__(self, proof, root: str):
        self.proof = proof
        self.root = root
        self.pool = os.path.join(root, "pool")
        self.control = os.path.join(self.pool, "oxide-managed")
        self.outputs = os.path.join(self.control, "outputs")
        self.registry = os.path.join(self.outputs, "registry.json")
        self.gone = os.path.join(root, "gone")
        self.gate = os.path.join(root, "gate")
        self.mount = ""
        os.makedirs(self.gone)

    def oxide(self, args: list, label: str, cwd: str, extra: dict | None = None) -> tuple:
        env = dict(self.proof.env)
        env.update(extra or {})
        return permanent.oxide_env(self.proof, args, label, cwd, env)

    def checkout(self, name: str, base: str | None = None) -> str:
        path = os.path.join(base or self.root, "checkouts", name)
        os.makedirs(os.path.join(path, "src"))
        with open(os.path.join(path, "Cargo.toml"), "w", encoding="utf-8") as handle:
            handle.write(PLAIN)
        with open(os.path.join(path, "src", "lib.rs"), "w", encoding="utf-8") as handle:
            handle.write("pub fn v() -> u32 { 1 }\n")
        self.proof.run(["git", "init", "-q"], cwd=path, timeout=30)
        with open(os.path.join(path, ".gitignore"), "w", encoding="utf-8") as handle:
            handle.write("/target\n/.oxide-outputs/\n")
        return os.path.realpath(path)

    def adopt(self, checkout: str) -> dict:
        code, _, stderr = self.oxide(["outputs", "adopt"], f"adopt {os.path.basename(checkout)}", checkout)
        expect(code == 0, f"adopt failed: {stderr}")
        return self.receipt(checkout)

    def receipt(self, checkout: str) -> dict:
        return permanent.read_json(permanent.receipt_path(self.pool, checkout))

    def selected(self, checkout: str) -> dict:
        receipt = self.receipt(checkout)
        expect(receipt["state"] == "committed", f"{checkout} receipt is {receipt['state']}")
        return receipt["links"][0]["selected"]

    def vanish(self, checkout: str) -> str:
        # A rename never crosses devices: an image checkout moves within it.
        base = self.gone
        if self.mount and checkout.startswith(self.mount + "/"):
            base = os.path.join(self.mount, "gone")
            os.makedirs(base, exist_ok=True)
        away = os.path.join(base, os.path.basename(checkout) + "-" + secrets.token_hex(3))
        os.rename(checkout, away)
        expect(not os.path.lexists(checkout), f"{checkout} still exists")
        return away

    def restore(self, checkout: str, away: str) -> None:
        os.rename(away, checkout)

    def registry_doc(self) -> dict:
        return permanent.read_json(self.registry)

    def dry_run(self, label: str, cwd: str, listed: str | None = None, extra: dict | None = None) -> tuple:
        args = ["outputs", "prune", "--dry-run"] + (["--operator-list", listed] if listed else [])
        env = dict(self.proof.env)
        env.update(extra or {})
        result = self.proof.run([self.proof.binary, *args], cwd=cwd, env=env)
        stdout, stderr = result.stdout, result.stderr.decode(errors="replace")
        print(f"----- {label}: exit {result.returncode} -----\n{stderr}", flush=True)
        return result.returncode, stdout, stderr

    def plan_file(self, label: str, cwd: str, listed: str | None = None, extra: dict | None = None) -> tuple:
        code, stdout, stderr = self.dry_run(label, cwd, listed, extra)
        expect(code == 0, f"{label}: dry run exited {code}: {stderr}")
        sha = hashlib.sha256(stdout).hexdigest()
        expect(sha in stderr, f"{label}: stderr does not carry the preview digest")
        path = os.path.join(self.root, f"plan-{secrets.token_hex(4)}.json")
        with open(path, "wb") as handle:
            handle.write(stdout)
        return path, sha, json.loads(stdout)

    def real_run(self, label: str, cwd: str, plan: str, sha: str, listed: str | None = None, extra: dict | None = None) -> tuple:
        args = ["outputs", "prune", "--plan", plan, "--plan-sha256", sha]
        if listed:
            args += ["--operator-list", listed]
        return self.oxide(args, label, cwd, extra)

    def candidates(self, preview: dict) -> set:
        return {candidate["root"] for candidate in preview["candidates"]}

    def kept(self, preview: dict, root: str) -> str:
        for kept in preview["kept"]:
            if kept.get("root") == root:
                return kept["reason"]
        return ""

    def intents(self) -> list:
        return self.registry_doc().get("intents", [])

    def quarantines(self) -> list:
        return sorted(glob.glob(os.path.join(self.mount, "oxide-outputs", ".prune-*")))

    def paused(self, checkout: str, point: str, label: str):
        """Start `oxide check` paused at a debug-only point; returns the child
        once it has reached the point."""
        gate = os.path.join(self.gate, secrets.token_hex(4))
        os.makedirs(self.gate, exist_ok=True)
        env = dict(self.proof.env)
        env["OXIDE_TEST_PAUSE"] = f"{point}:{gate}"
        child = permanent.start_child(self.proof, [self.proof.binary, "check"], checkout, env)
        deadline = time.monotonic() + min(60, permanent.work_left())
        while not glob.glob(f"{gate}.{point}-*"):
            if child.poll() is not None or time.monotonic() >= deadline:
                code, out, err = permanent.finish_child(self.proof, child, label)
                fail(f"{label}: never reached {point} (exit {code}): {err}")
            time.sleep(0.05)
        return child, gate


def keyed_destination(gate: str) -> str:
    """The destination facts a starter's slot key captured (keyed pause)."""
    [marker_file] = glob.glob(f"{gate}.keyed-*")
    return permanent.read_bytes(marker_file).decode()


def accounting_oracle(root: str) -> tuple[int, int]:
    """Independent expectation: regular files' sizes by name, and allocated
    blocks of each regular-file inode counted once (hard links once)."""
    logical, allocated, seen = 0, 0, set()
    for directory, _, files in os.walk(root):
        for name in files:
            facts = os.lstat(os.path.join(directory, name))
            if not stat.S_ISREG(facts.st_mode):
                continue
            logical += facts.st_size
            if (facts.st_dev, facts.st_ino) not in seen:
                seen.add((facts.st_dev, facts.st_ino))
                allocated += facts.st_blocks * 512
    return logical, allocated


def release(gate: str) -> None:
    with open(gate, "wb") as handle:
        handle.write(b"go")


def tree_bytes(root: str) -> dict:
    digest = {}
    for directory, _, files in os.walk(root):
        for name in files:
            path = os.path.join(directory, name)
            if os.path.islink(path):
                continue
            with open(path, "rb") as handle:
                digest[os.path.relpath(path, root)] = hashlib.sha256(handle.read()).hexdigest()
    return digest


def read_nofollow(path: str, cap: int) -> tuple[bytes, tuple]:
    """A bounded no-follow read of one regular file owned by this user;
    returns the bytes and the (dev, ino, size, mtime_ns) identity."""
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        facts = os.fstat(fd)
        expect(stat.S_ISREG(facts.st_mode), f"{path} is not a regular file")
        expect(facts.st_uid == os.getuid(), f"{path} is not owned by this user")
        expect(facts.st_size <= cap, f"{path} exceeds its {cap}-byte cap")
        chunks, left = [], cap + 1
        while left > 0:
            chunk = os.read(fd, min(left, 1 << 20))
            if not chunk:
                break
            chunks.append(chunk)
            left -= len(chunk)
        data = b"".join(chunks)
        expect(len(data) == facts.st_size, f"{path} changed size while read")
        return data, (facts.st_dev, facts.st_ino, facts.st_size, facts.st_mtime_ns)
    finally:
        os.close(fd)


def copy_old_artifact(root: str) -> str:
    """Verify the exact receipt and installed binary, copy the binary into
    owned scratch and return the copy. Any mismatch fails; nothing is
    reinstalled, skipped or substituted."""
    receipt_bytes, receipt_id = read_nofollow(OLD_RECEIPT, OLD_RECEIPT_CAP)
    receipt = json.loads(receipt_bytes)
    installed = receipt.get("installed", {})
    expect(receipt.get("pin") == OLD_PIN, f"receipt pin {receipt.get('pin')} is not {OLD_PIN}")
    expect(installed.get("path") == OLD_BINARY, f"receipt path {installed.get('path')}")
    expect(installed.get("sha256") == OLD_SHA and receipt.get("built_sha256") == OLD_SHA, "receipt hash mismatch")
    expect(receipt.get("control_state_version") == 2, f"receipt control state {receipt.get('control_state_version')}")
    binary, binary_id = read_nofollow(OLD_BINARY, OLD_BINARY_CAP)
    expect(len(binary) == installed.get("size"), "installed size differs from the receipt")
    expect(hashlib.sha256(binary).hexdigest() == OLD_SHA, "installed binary hash differs from the pin")
    for path, before in ((OLD_RECEIPT, receipt_id), (OLD_BINARY, binary_id)):
        after = os.lstat(path)
        expect(
            (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns) == before,
            f"{path} changed while it was copied",
        )
    copy = os.path.join(root, "old-oxide-d246d400")
    fd = os.open(copy, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o700)
    try:
        os.write(fd, binary)
    finally:
        os.close(fd)
    expect(hashlib.sha256(permanent.read_bytes(copy)).hexdigest() == OLD_SHA, "the copy's hash differs")
    permanent.record(
        "old-artifact.json",
        json.dumps(
            {
                "source_pin": OLD_PIN,
                "binary_sha256": OLD_SHA,
                "binary_size": len(binary),
                "receipt_path": OLD_RECEIPT,
                "receipt_sha256": hashlib.sha256(receipt_bytes).hexdigest(),
                "copy": copy,
            },
            indent=2,
        ),
    )
    return copy


def tree_snapshot(root: str) -> dict:
    """Complete bounded bytes + metadata of a fixture tree (no atime)."""
    snapshot = {}
    for directory, directories, files in os.walk(root):
        for name in directories + files:
            path = os.path.join(directory, name)
            facts = os.lstat(path)
            entry = (facts.st_mode, facts.st_size, facts.st_mtime_ns, facts.st_ino)
            if stat.S_ISREG(facts.st_mode):
                entry += (hashlib.sha256(permanent.read_bytes(path)).hexdigest(),)
            elif stat.S_ISLNK(facts.st_mode):
                entry += (os.readlink(path),)
            snapshot[os.path.relpath(path, root)] = entry
    return snapshot


def marker(root: str) -> dict:
    return permanent.read_json(os.path.join(root, ".oxide-output.json"))


def run(proof, repo: str) -> None:
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    name = "OxPr" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    expect(not os.path.lexists(mount), f"{mount} exists before the proof; never touched")
    print(f"outputs-prune proof {os.getpid()} source {source} mount {mount}", flush=True)
    proof.phase("start")

    proof.binary = handoff.stage_subject(proof.root)
    proof.phase("after staging")

    world = World(proof, proof.root)
    home = os.path.join(proof.root, "home")
    cargo_home = os.path.join(proof.root, "cargo-home")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    proof.env = permanent.proof_env(world.pool, home, cargo_home)

    alpha = world.checkout("alpha")
    code, _, stderr = world.oxide(["check"], "first build creates the internal pool", alpha)
    expect(code == 0, stderr)
    world.mount = permanent.attach_image(proof, proof.root, name)
    expect(world.mount == mount, f"mounted at {world.mount}, not {mount}")
    code, stdout, stderr = world.oxide(
        ["pool", "register-removable", mount, "--reserve", "16MiB", "--cap", "64MiB", "--mode", "permanent"],
        "register permanent drive",
        alpha,
    )
    expect(code == 0 and "registered removable pool" in stdout, stderr)
    good = permanent.real_uuid(world.pool)
    proof.phase("after registration")

    # ---- fence / init ------------------------------------------------------
    case("init: forward fence and ready registry")
    world.adopt(alpha)
    for control in (world.control, os.path.join(world.control, "removable", "oxide-managed")):
        state_file = os.path.join(control, "state.json")
        expect(os.path.isfile(state_file), f"paired control state {state_file} is missing")
        state = permanent.read_json(state_file)
        expect(state["version"] == 3, f"{control}: state version {state['version']}")
        expect(state.get("owned_outputs") == "oxide.outputs/2", f"{control}: {state.get('owned_outputs')}")
        expect("ready" in json.dumps(state.get("outputs_init")), f"{control}: {state.get('outputs_init')}")
    registry = world.registry_doc()
    expect(registry["version"] == "oxide.outputs.registry/1" and registry.get("schema") == 1 and registry["complete"], registry)
    # Older-binary fence control: the exact d246d400 artifact's copy reports
    # control state 2, then its `check` and `outputs adopt` in this fixture
    # (private HOME without any install receipt, private CARGO_HOME, pool and
    # registration) both refuse before any effect.
    old = copy_old_artifact(proof.root)
    probe = proof.run([old, CAPABILITIES_PROBE], cwd=alpha, env=proof.env, timeout=30)
    expect(probe.returncode == 0, f"old capability probe exit {probe.returncode}")
    expect(json.loads(probe.stdout).get("control_state_version") == 2, f"old probe {probe.stdout!r}")
    watched = [world.pool, os.path.join(mount, "oxide-outputs"), alpha]
    before = [tree_snapshot(path) for path in watched]
    for args in (["check"], ["outputs", "adopt"]):
        result = proof.run([old, *args], cwd=alpha, env=proof.env, timeout=30)
        stderr = result.stderr.decode(errors="replace")
        print(f"----- old binary {args}: exit {result.returncode} -----\n{stderr}", flush=True)
        expect(result.returncode != 0, f"the older binary's {args} did not refuse")
        expect(HANDOFF_NOTICE not in stderr, f"the older binary's {args} selected a handoff; STOP")
        expect(OLD_BINARY not in stderr and "/.local/state/oxide" not in stderr, f"a host path appeared: {stderr}")
        expect([tree_snapshot(path) for path in watched] == before, f"the older binary's {args} changed fixture state")

    case("missing ready registry refuses and is never rebuilt")
    saved = permanent.read_bytes(world.registry)
    os.rename(world.registry, world.registry + ".aside")
    code, _, stderr = world.dry_run("dry run without the registry", alpha)
    expect(code == 2, f"dry run without the registry exited {code}")
    beta = world.checkout("beta")
    code, _, stderr = world.oxide(["outputs", "adopt"], "adopt without the registry", beta)
    expect(code != 0, "adopt succeeded without the registry")
    expect(not os.path.exists(world.registry), "the registry was rebuilt")
    os.rename(world.registry + ".aside", world.registry)
    expect(permanent.read_bytes(world.registry) == saved, "registry bytes changed")
    proof.phase("after fence cases")

    # ---- nonce / reservation ----------------------------------------------
    case("root nonce in marker, receipt and registry")
    selected = world.selected(alpha)
    root_marker = marker(selected["root"])
    expect(root_marker["version"] == "oxide.outputs.root/2", root_marker)
    nonce = root_marker["nonce"]
    expect(len(nonce) == 32 and all(c in "0123456789abcdef" for c in nonce), nonce)
    expect(selected.get("nonce") == nonce, f"receipt nonce {selected.get('nonce')} != {nonce}")
    rows = [row for row in world.registry_doc()["rows"] if row["root"] == selected["root"]]
    expect(len(rows) == 1 and rows[0]["nonce"] == nonce and rows[0]["selected"], rows)

    case("identical starters share one reserved future-root nonce")
    world.adopt(beta)
    permanent.set_recorded_uuid(world.pool, "00000000-0000-0000-0000-000000000000")
    first, first_gate = world.paused(beta, "keyed", "starter one")
    second, second_gate = world.paused(beta, "keyed", "starter two")
    reserved = world.receipt(beta)["links"][0].get("reserved")
    expect(reserved is not None and len(reserved["nonce"]) == 32, f"no reservation: {reserved}")
    expect(reserved["kind"] == "local", reserved)
    # Both starters keyed the same reserved nonce, before either proceeds.
    first_key, second_key = keyed_destination(first_gate), keyed_destination(second_gate)
    expect(first_key == second_key and reserved["nonce"] in first_key, f"keys {first_key!r} / {second_key!r}")
    release(first_gate)
    release(second_gate)
    for child, label in ((first, "starter one"), (second, "starter two")):
        code, _, stderr = permanent.finish_child(proof, child, label)
        expect(code == 0, f"{label} failed: {stderr}")
    local_root = reserved["root"]
    expect(marker(local_root)["nonce"] == reserved["nonce"], "the created root does not carry the reserved nonce")
    expect(world.receipt(beta)["links"][0].get("reserved") is None, "the reservation was not consumed")
    permanent.set_recorded_uuid(world.pool, good)

    case("cancelled reservation: the stale starter refuses before Cargo")
    gamma = world.checkout("gamma")
    world.adopt(gamma)
    permanent.set_recorded_uuid(world.pool, "00000000-0000-0000-0000-000000000000")
    stale, stale_gate = world.paused(gamma, "keyed", "stale starter")
    reserved = world.receipt(gamma)["links"][0]["reserved"]
    expect(reserved["nonce"] in keyed_destination(stale_gate), "the stale starter did not key the reserved nonce")
    code, stdout, stderr = world.oxide(["outputs", "cancel-reservation"], "cancel the reservation", gamma)
    expect(code == 0 and "cancelled" in stdout, stdout + stderr)
    expect(world.receipt(gamma)["links"][0].get("reserved") is None, "the receipt kept the reservation")
    expect(
        not any(row.get("reservation") == reserved["id"] or row.get("cancelling") for row in world.registry_doc()["rows"]),
        "the cancelled reservation's row remains",
    )
    release(stale_gate)
    code, _, stderr = permanent.finish_child(proof, stale, "stale starter")
    expect(code != 0 and "cancelled or replaced" in stderr, f"stale starter exit {code}: {stderr}")
    expect(not os.path.lexists(reserved["root"]), "the cancelled root was created")
    permanent.set_recorded_uuid(world.pool, good)
    proof.phase("after reservation cases")

    # ---- registry / snapshot ----------------------------------------------
    case("dry run: canonical preview, digest, no mutation")
    before = tree_bytes(world.control)
    volume_before = tree_bytes(os.path.join(mount, "oxide-outputs"))
    expect("outputs/registry.json" in before and len(volume_before) > 0, "the byte oracle saw nothing")
    path, sha, preview = world.plan_file("dry run", alpha)
    expect(preview["version"] == "oxide.outputs.prune-preview/1", preview["version"])
    expect(tree_bytes(world.control) == before, "a dry run changed control bytes")
    expect(tree_bytes(os.path.join(mount, "oxide-outputs")) == volume_before, "a dry run changed volume bytes")

    case("malformed receipt keeps the whole batch")
    delta = world.checkout("delta")
    world.adopt(delta)
    delta_root = world.selected(delta)["root"]
    delta_away = world.vanish(delta)
    gamma_receipt = permanent.receipt_path(world.pool, gamma)
    saved_receipt = permanent.read_bytes(gamma_receipt)
    with open(gamma_receipt, "wb") as handle:
        handle.write(b"{corrupt")
    code, stdout, _ = world.dry_run("dry run with a corrupt receipt", alpha)
    preview = json.loads(stdout)
    expect(code == 0 and not preview["candidates"], f"candidates despite a corrupt receipt: {preview['candidates']}")
    expect("whole batch" in world.kept(preview, delta_root), world.kept(preview, delta_root))
    with open(gamma_receipt, "wb") as handle:
        handle.write(saved_receipt)

    # ---- eligibility --------------------------------------------------------
    case("live checkout and live detached root kept")
    _, _, preview = world.plan_file("eligibility dry run", alpha)
    expect(world.candidates(preview) == {delta_root}, f"candidates {world.candidates(preview)}")
    expect("present" in world.kept(preview, world.selected(alpha)["root"]), "alpha not kept as present")
    epsilon = world.checkout("epsilon")
    epsilon_root = world.adopt(epsilon)["links"][0]["selected"]["root"]
    code, _, stderr = world.oxide(["outputs", "detach"], "detach epsilon", epsilon)
    expect(code == 0, stderr)
    _, _, preview = world.plan_file("live detached dry run", alpha)
    expect(epsilon_root not in world.candidates(preview), "a live detached root became a candidate")
    expect("present" in world.kept(preview, epsilon_root), world.kept(preview, epsilon_root))
    # The same released root becomes eligible once its checkout is gone.
    epsilon_away = world.vanish(epsilon)
    _, _, preview = world.plan_file("released vanished dry run", alpha)
    expect(epsilon_root in world.candidates(preview), f"a released vanished root was kept: {world.kept(preview, epsilon_root)}")
    world.restore(epsilon, epsilon_away)

    # A registered checkout on the attached image: absence is proved through
    # the verified mount and its recorded volume UUID (the /Volumes branch).
    omega = world.checkout("omega", base=mount)
    omega_root = world.adopt(omega)["links"][0]["selected"]["root"]
    omega_away = world.vanish(omega)
    _, _, preview = world.plan_file("registered image checkout dry run", alpha)
    expect(omega_root in world.candidates(preview), f"a registered /Volumes checkout was kept: {world.kept(preview, omega_root)}")
    world.restore(omega, omega_away)

    case("injected EACCES ancestor kept; genuine absence is a candidate; bad lists refuse")
    # No real unreadable directory: a debug-only EACCES is injected inside the
    # supervisor-owned scratch, its firing is recorded in the evidence
    # directory, and the same fixture without injection is genuine absence.
    locked = os.path.realpath(os.path.join(proof.root, "locked"))
    os.makedirs(locked)
    eta = os.path.join(locked, "eta")
    shutil.move(world.checkout("eta-src"), eta)
    eta_root = world.adopt(eta)["links"][0]["selected"]["root"]
    eta_away = world.vanish(eta)
    fired = os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], "prune-eacces-fired")
    expect(not os.path.exists(fired), "the EACCES marker exists before injection")
    _, _, preview = world.plan_file("injected EACCES dry run", alpha, extra={"OXIDE_TEST_PRUNE_EACCES": locked})
    expect(eta_root not in world.candidates(preview), "an injected unreadable ancestor became absence")
    reason = world.kept(preview, eta_root)
    expect("unknown" in reason and "Permission denied" in reason, reason)
    expect(os.path.isfile(fired) and locked in permanent.read_bytes(fired).decode(), "the EACCES injection did not fire")
    _, _, preview = world.plan_file("genuine absence control dry run", alpha)
    expect(eta_root in world.candidates(preview), f"genuine absence not a candidate: {world.kept(preview, eta_root)}")
    world.restore(eta, eta_away)
    for label, body in (("relative", '["relative/root"]'), ("duplicate", json.dumps([delta_root, delta_root]))):
        listed = os.path.join(proof.root, f"list-{label}.json")
        with open(listed, "w", encoding="utf-8") as handle:
            handle.write(body)
        code, _, _ = world.dry_run(f"{label} operator list", alpha, listed)
        expect(code == 2, f"{label} operator list exited {code}")

    case("receiptless legacy root: exclusive pruning receipt, deleted")
    legacy_checkout = os.path.join(proof.root, "never", "legacy-app")
    os.makedirs(os.path.dirname(legacy_checkout))
    legacy_id = hashlib.sha256(legacy_checkout.encode()).hexdigest()
    legacy_root = os.path.join(mount, "oxide-outputs", "legacy-" + legacy_id[:8])
    os.makedirs(legacy_root, mode=0o700)
    os.chmod(legacy_root, 0o700)
    legacy_marker = dict(marker(world.selected(alpha)["root"]))
    legacy_marker.pop("nonce", None)
    legacy_marker.pop("checkout_volume_uuid", None)
    legacy_marker.update({"version": "oxide.outputs.root/1", "checkout": legacy_checkout, "checkout_id": legacy_id, "generation": 1})
    with open(os.path.join(legacy_root, ".oxide-output.json"), "w", encoding="utf-8") as handle:
        json.dump(legacy_marker, handle)
    with open(os.path.join(legacy_root, "artifact"), "wb") as handle:
        handle.write(b"x" * 4096)
    # A second legacy root whose recorded checkout is on the separately
    # attached scratch image (not the boot Data volume): kept.
    image_checkout = os.path.join(mount, "never", "image-app")
    image_id = hashlib.sha256(image_checkout.encode()).hexdigest()
    image_root = os.path.join(mount, "oxide-outputs", "legacy-" + image_id[:8])
    os.makedirs(image_root, mode=0o700)
    os.chmod(image_root, 0o700)
    image_marker = dict(legacy_marker)
    image_marker.update({"checkout": image_checkout, "checkout_id": image_id})
    with open(os.path.join(image_root, ".oxide-output.json"), "w", encoding="utf-8") as handle:
        json.dump(image_marker, handle)
    # A foreign directory (no marker) and a marker whose checkout identity
    # is not the digest of its checkout path: both kept, bytes unchanged.
    foreign_root = os.path.join(mount, "oxide-outputs", "foreign-dir")
    os.makedirs(foreign_root, mode=0o700)
    with open(os.path.join(foreign_root, "data"), "wb") as handle:
        handle.write(b"foreign")
    forged_root = os.path.join(mount, "oxide-outputs", "legacy-forged")
    os.makedirs(forged_root, mode=0o700)
    os.chmod(forged_root, 0o700)
    forged_marker = dict(legacy_marker)
    forged_marker.update({"checkout": os.path.join(proof.root, "never", "forged"), "checkout_id": "f" * 64})
    with open(os.path.join(forged_root, ".oxide-output.json"), "w", encoding="utf-8") as handle:
        json.dump(forged_marker, handle)
    kept_before = {root: tree_bytes(root) for root in (foreign_root, forged_root, image_root)}
    listed = os.path.join(proof.root, "list-legacy.json")
    with open(listed, "w", encoding="utf-8") as handle:
        json.dump([legacy_root, image_root, foreign_root, forged_root], handle)
    # Delta (an ordinary eligible root) is present for this run so only the
    # listed legacy roots are candidates; it vanishes again afterwards.
    world.restore(delta, delta_away)
    plan, sha, preview = world.plan_file("legacy dry run", alpha, listed)
    expect(legacy_root in world.candidates(preview), f"legacy root not a candidate: {world.kept(preview, legacy_root)}")
    expect(image_root not in world.candidates(preview), "a legacy root of an image checkout became a candidate")
    expect("volume" in world.kept(preview, image_root), world.kept(preview, image_root))
    expect(foreign_root not in world.candidates(preview), "a foreign directory became a candidate")
    expect("digest" in world.kept(preview, forged_root), world.kept(preview, forged_root))
    code, stdout, stderr = world.real_run("legacy real run", alpha, plan, sha, listed)
    expect(code == 0, f"legacy real run exited {code}: {stderr}")
    expect(not os.path.lexists(legacy_root), "the legacy root remains")
    expect(os.path.isdir(image_root), "the kept image-checkout legacy root was deleted")
    delta_away = world.vanish(delta)
    for root, digest in kept_before.items():
        expect(tree_bytes(root) == digest, f"kept listed root {root} changed")
    legacy_receipt = permanent.read_json(permanent.receipt_file(world.pool, legacy_checkout))
    expect(legacy_receipt["state"] == "pruned" and legacy_receipt["pruned"]["outcome"] == "deleted", legacy_receipt)
    proof.phase("after eligibility cases")

    # ---- custody / containment -------------------------------------------
    case("active writer keeps a vanished checkout's root")
    theta = world.checkout("theta")
    with open(os.path.join(theta, "build.rs"), "w", encoding="utf-8") as handle:
        handle.write(GATED_BUILD)
    theta_root = world.adopt(theta)["links"][0]["selected"]["root"]
    writer_gate = os.path.join(proof.root, "writer-gate")
    os.makedirs(writer_gate)
    writer_env = dict(proof.env)
    writer_env["PRUNE_GATE"] = writer_gate
    writer = permanent.start_child(proof, [proof.binary, "check"], theta, writer_env)
    permanent.wait_for(os.path.join(writer_gate, "started"), "running writer")
    theta_away = world.vanish(theta)
    _, _, preview = world.plan_file("running writer dry run", alpha)
    expect(theta_root not in world.candidates(preview), "a running writer's root became a candidate")
    expect("writer" in world.kept(preview, theta_root), world.kept(preview, theta_root))
    world.restore(theta, theta_away)
    with open(os.path.join(writer_gate, "release"), "wb") as handle:
        handle.write(b"go")
    code, _, stderr = permanent.finish_child(proof, writer, "running writer")
    expect(code == 0, f"the gated writer failed: {stderr}")

    case("symlink root kept; symlink descendant stops deletion")
    iota = world.checkout("iota")
    iota_root = world.adopt(iota)["links"][0]["selected"]["root"]
    outside = os.path.join(proof.root, "outside")
    os.makedirs(outside)
    with open(os.path.join(outside, "keep"), "wb") as handle:
        handle.write(b"keep")
    os.symlink(outside, os.path.join(iota_root, "escape"))
    world.vanish(iota)
    plan, sha, preview = world.plan_file("symlink descendant dry run", alpha)
    expect(iota_root in world.candidates(preview), world.kept(preview, iota_root))
    expect(delta_root in world.candidates(preview), "delta dropped out")
    code, _, stderr = world.real_run("symlink descendant real run", alpha, plan, sha)
    expect(code == 3, f"a symlink descendant did not stop deletion (exit {code})")
    expect(os.path.exists(os.path.join(outside, "keep")), "the symlink target was touched")
    quarantined = [path for path in world.quarantines() if os.path.lexists(os.path.join(path, "escape"))]
    expect(len(quarantined) == 1, f"quarantines {world.quarantines()}")
    os.unlink(os.path.join(quarantined[0], "escape"))
    code, _, stderr = world.real_run("replay after the link is removed", alpha, plan, sha)
    expect(code == 0, f"replay exited {code}: {stderr}")
    expect(not world.quarantines(), f"quarantine left: {world.quarantines()}")
    expect(not os.path.lexists(delta_root), "delta remains")
    kappa = world.checkout("kappa")
    kappa_selected = world.adopt(kappa)["links"][0]["selected"]
    kappa_root = kappa_selected["root"]
    kappa_nonce = kappa_selected.get("nonce")
    expect(kappa_nonce and len(kappa_nonce) == 32, f"kappa has no root nonce: {kappa_selected}")
    code, _, stderr = world.oxide(["check"], "kappa build before prune", kappa)
    expect(code == 0, stderr)
    kappa_slot, _ = permanent.slot_state(stderr)
    kappa_pool = "removable pool:" if "removable" in permanent.summary(stderr) else "internal pool:"
    kappa_away = world.vanish(kappa)
    os.rename(kappa_root, kappa_root + ".real")
    os.symlink(kappa_root + ".real", kappa_root)
    _, _, preview = world.plan_file("symlink root dry run", alpha)
    expect(kappa_root not in world.candidates(preview), "a symlink root became a candidate")
    os.unlink(kappa_root)
    os.rename(kappa_root + ".real", kappa_root)
    world.restore(kappa, kappa_away)
    # Reserve: a low reserve at the check before the rename refuses with the
    # root unchanged (exit 2); a low reserve between delete batches of a
    # root with more than one batch of entries stops deletion (exit 3) and
    # exact replay completes. Each injection's firing is recorded.
    lam = world.checkout("lambda")
    lam_root = world.adopt(lam)["links"][0]["selected"]["root"]
    bulk = os.path.join(lam_root, "bulk")
    os.makedirs(bulk)
    for index in range(600):
        with open(os.path.join(bulk, f"f{index:04d}"), "wb") as handle:
            handle.write(b"x")
    world.vanish(lam)
    fired = os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], "prune-low-reserve-fired")
    plan, sha, preview = world.plan_file("reserve dry run", alpha)
    expect(world.candidates(preview) == {lam_root}, f"candidates {world.candidates(preview)}")
    before = tree_bytes(lam_root)
    code, _, stderr = world.real_run("low reserve before rename", alpha, plan, sha, extra={"OXIDE_TEST_PRUNE_LOW_RESERVE_AT": "1"})
    expect(code == 2 and "reserve" in stderr, f"low reserve before rename exited {code}")
    expect(tree_bytes(lam_root) == before and not world.intents(), "a refused reserve changed the root or journaled")
    expect(os.path.isfile(fired), "the low-reserve injection did not fire")
    os.unlink(fired)
    code, _, stderr = world.real_run("low reserve mid-batch", alpha, plan, sha, extra={"OXIDE_TEST_PRUNE_LOW_RESERVE_AFTER_ENTRIES": "1"})
    expect(code == 3 and "reserve" in stderr, f"low reserve mid-batch exited {code}")
    expect(os.path.isfile(fired) and world.intents() and world.quarantines(), "no pending mid-batch stop")
    expect("entries:" in permanent.read_bytes(fired).decode(), "the entry-count injection did not fire")
    # The entry-count trigger fires at the first check after one entry was
    # actually deleted: some entries are gone, none after the stop.
    remaining = sum(len(files) for _, _, files in os.walk(world.quarantines()[0]))
    expect(0 < remaining < 601, f"the reserve stop was not mid-batch ({remaining} entries left)")
    code, _, stderr = world.real_run("replay after the reserve stop", alpha, plan, sha)
    expect(code == 0 and not world.quarantines() and not world.intents(), f"reserve replay exited {code}: {stderr}")
    proof.phase("after custody cases")

    # ---- crash boundaries -------------------------------------------------
    for point, label in (
        ("intent", "after intent"),
        ("renamed", "after rename"),
        ("deleting", "during delete"),
        ("terminal", "after registry terminal"),
    ):
        case(f"crash {label}: pending, then exact replay")
        checkout = world.checkout("crash-" + point)
        root = world.adopt(checkout)["links"][0]["selected"]["root"]
        world.vanish(checkout)
        plan, sha, preview = world.plan_file(f"crash {point} dry run", alpha)
        expect(root in world.candidates(preview), world.kept(preview, root))
        code, _, stderr = world.real_run(f"crash {point}", alpha, plan, sha, extra={"OXIDE_TEST_PRUNE_CRASH": point})
        expect(code == 3, f"crash at {point} exited {code}")
        pending = [intent for intent in world.intents() if intent["identity"]["root"] == root]
        expect(len(pending) == 1, f"no pending intent after {point}: {world.intents()}")
        if point == "intent":
            expect(os.path.isdir(root), "the root moved before its rename")
        if point == "renamed":
            expect(not os.path.lexists(root) and world.quarantines(), "no quarantine after the rename")
            # A recreated checkout cannot bind while the intent is pending.
            os.makedirs(os.path.join(checkout, "src"))
            with open(os.path.join(checkout, "Cargo.toml"), "w", encoding="utf-8") as handle:
                handle.write(PLAIN)
            with open(os.path.join(checkout, "src", "lib.rs"), "w", encoding="utf-8") as handle:
                handle.write("pub fn v() -> u32 { 1 }\n")
            code, _, stderr = world.oxide(["check"], "recreated checkout during a pending intent", checkout)
            expect(code != 0 and "unsettled" in stderr, f"recreated checkout exit {code}: {stderr}")
            expect(not os.path.lexists(root), "the pending root was recreated")
            shutil.rmtree(checkout)
        if point == "terminal":
            expect(not os.path.lexists(root), "the root remains after its registry terminal state")
            expect(world.receipt(checkout)["state"] == "committed", "the receipt settled before its replay")
        code, _, stderr = world.real_run(f"replay {point}", alpha, plan, sha)
        expect(code == 0, f"replay after {point} exited {code}: {stderr}")
        expect(not os.path.lexists(root) and not world.quarantines(), f"{point}: root or quarantine left")
        expect(not world.intents(), f"{point}: intents left {world.intents()}")
    proof.phase("after crash cases")

    # ---- completion / diagnostics ----------------------------------------
    case("completion: terminal state, tombstone, observations, status")
    # Hard-link control: one 64 KiB inode under two names counts twice by
    # name and once allocated.
    with open(os.path.join(kappa_root, "payload"), "wb") as handle:
        handle.write(os.urandom(64 * 1024))
    os.link(os.path.join(kappa_root, "payload"), os.path.join(kappa_root, "payload-link"))
    expected_logical, expected_allocated = accounting_oracle(kappa_root)
    world.vanish(kappa)
    plan, sha, preview = world.plan_file("completion dry run", alpha)
    expect(kappa_root in world.candidates(preview), world.kept(preview, kappa_root))
    [observed] = [candidate for candidate in preview["candidates"] if candidate["root"] == kappa_root]
    expect(observed["accounting_complete"], observed)
    expect(observed["logical_bytes"] == expected_logical, f"logical {observed['logical_bytes']} != oracle {expected_logical}")
    # Inventory counts regular files only, allocated once per inode: the
    # oracle is exact, and the hard-linked inode is not allocated twice.
    expect(observed["allocated_bytes"] == expected_allocated, f"allocated {observed['allocated_bytes']} != oracle {expected_allocated}")
    expect(preview["coverage"]["source"] and preview["coverage"]["observed_ms"] > 0, preview["coverage"])
    code, stdout, stderr = world.real_run("completion real run", alpha, plan, sha)
    expect(code == 0 and "deleted" in stdout, f"completion exited {code}: {stderr}")
    expect(not os.path.lexists(kappa_root), "kappa remains")
    expect(all(row["root"] != kappa_root for row in world.registry_doc()["rows"]), "kappa's row remains")
    tombstones = permanent.read_json(os.path.join(world.outputs, "tombstones.json"))
    expect(any(entry["root"] == kappa_root for entry in tombstones["entries"]), "no tombstone for kappa")
    observations = permanent.read_json(os.path.join(world.outputs, "prune-observations.json"))
    expect(observations["version"] == "oxide.outputs.prune-observations/1", observations["version"])
    code, stdout, _ = world.oxide(["status"], "status shows the recorded observation", alpha)
    expect(code == 0 and "prune" in stdout, stdout)

    case("recreated checkout adopts a fresh root nonce")
    old_nonce = kappa_nonce
    os.makedirs(kappa)
    shutil.copytree(world.checkout("kappa-src"), kappa, dirs_exist_ok=True)
    code, _, stderr = world.oxide(["outputs", "adopt"], "re-adopt the recreated checkout", kappa)
    expect(code == 0, stderr)
    # The old root's slot is still present and idle (read-only status), so a
    # cold, different slot below proves key separation, not eviction.
    code, stdout, _ = world.oxide(["status"], "old kappa slot still idle", alpha)
    expect(code == 0, "status failed")
    section, idle = None, False
    for line in stdout.splitlines():
        if line.startswith(("internal pool:", "removable pool:")):
            section = line.split(":")[0] + ":"
        elif section == kappa_pool and line.split()[:3] == ["slot", kappa_slot, "idle"] and kappa in line:
            idle = True
    expect(idle, f"the old kappa slot {kappa_slot} ({kappa_pool} {kappa}) is not present and idle before the recreated build")
    new_nonce = world.selected(kappa)["nonce"]
    expect(new_nonce and new_nonce != old_nonce, f"recreated root nonce {new_nonce} reused {old_nonce}")
    code, _, stderr = world.oxide(["check"], "recreated root build", kappa)
    expect(code == 0, stderr)
    slot, state = permanent.slot_state(stderr)
    expect(state == "cold" and slot != kappa_slot, f"the recreated root reused slot {slot} ({state}); key not separated")
    proof.phase("after completion")

    # ---- resolve-intent ---------------------------------------------------
    case("resolve-intent: both leaves absent")
    rho = world.checkout("rho")
    rho_root = world.adopt(rho)["links"][0]["selected"]["root"]
    world.vanish(rho)
    plan, sha, _ = world.plan_file("rho dry run", alpha)
    code, _, _ = world.real_run("rho crash after rename", alpha, plan, sha, extra={"OXIDE_TEST_PRUNE_CRASH": "renamed"})
    expect(code == 3, f"rho crash exited {code}")
    [intent] = [intent for intent in world.intents() if intent["identity"]["root"] == rho_root]
    for quarantine in world.quarantines():
        shutil.rmtree(quarantine)
    code, stdout, stderr = world.oxide(["outputs", "resolve-intent", intent["id"]], "resolve rho", alpha)
    expect(code == 0 and "resolved by operator" in stdout, stdout + stderr)
    rho_receipt = permanent.read_json(permanent.receipt_file(world.pool, rho))
    expect(rho_receipt["pruned"]["outcome"] == "resolved-by-operator", rho_receipt)
    expect(os.path.isfile(os.path.join(world.outputs, "prune-resolutions", intent["id"] + ".json")), "no resolution receipt")

    case("resolve-intent refuses while the quarantine is present")
    sigma = world.checkout("sigma")
    sigma_root = world.adopt(sigma)["links"][0]["selected"]["root"]
    world.vanish(sigma)
    plan, sha, _ = world.plan_file("sigma dry run", alpha)
    code, _, _ = world.real_run("sigma crash after rename", alpha, plan, sha, extra={"OXIDE_TEST_PRUNE_CRASH": "renamed"})
    expect(code == 3, f"sigma crash exited {code}")
    [intent] = [intent for intent in world.intents() if intent["identity"]["root"] == sigma_root]
    before = json.dumps(intent, sort_keys=True)
    code, _, stderr = world.oxide(["outputs", "resolve-intent", intent["id"]], "resolve sigma", alpha)
    expect(code == 2, f"resolve with a present quarantine exited {code}")
    [after] = [record for record in world.intents() if record["id"] == intent["id"]]
    expect(json.dumps(after, sort_keys=True) == before, "a refused resolution changed the intent")
    code, _, stderr = world.real_run("sigma replay", alpha, plan, sha)
    expect(code == 0, f"sigma replay exited {code}: {stderr}")
    proof.phase("after resolve cases")

    # ---- overflow (last: the sticky flag is never cleared) -----------------
    case("overflow: keep every root; tagged reservation binds with no row")
    doc = world.registry_doc()
    doc["complete"] = False
    doc["incomplete"] = {"reason": "proof overflow", "since_ms": 1, "transaction": doc["created_by"]}
    doc["revision"] += 1
    with open(world.registry, "w", encoding="utf-8") as handle:
        json.dump(doc, handle)
    os.chmod(world.registry, 0o600)
    code, stdout, _ = world.dry_run("overflow dry run", alpha)
    expect(code in (0, 2), f"overflow dry run exited {code}")
    if code == 0:
        expect(not json.loads(stdout)["candidates"], "an overflow registry yielded candidates")
    tau = world.checkout("tau")
    world.adopt(tau)
    permanent.set_recorded_uuid(world.pool, "00000000-0000-0000-0000-000000000000")
    starter, gate = world.paused(tau, "keyed", "overflow starter")
    reserved = world.receipt(tau)["links"][0]["reserved"]
    expect(reserved.get("overflow", {}).get("registry") == doc["created_by"], reserved)
    expect(all(row["root"] != reserved["root"] for row in world.registry_doc()["rows"]), "an overflow row was invented")
    release(gate)
    code, _, stderr = permanent.finish_child(proof, starter, "overflow starter")
    expect(code == 0, f"overflow starter failed: {stderr}")
    permanent.set_recorded_uuid(world.pool, good)
    expect(len(DONE) == CASES, f"ran {len(DONE)} cases, not {CASES}")

    # The ordinary detach also runs inside the work window.
    permanent.work_left()
    proof.detach()
    proof.phase("after detach")


def main(argv: list[str]) -> int:
    global permanent, handoff, expect, fail
    if argv == ["--plan"]:
        return plan()
    if argv:
        print(f"test-outputs-prune: unknown arguments {argv!r}", file=sys.stderr)
        return 2
    permanent = _load("permanent_outputs", "test-permanent-outputs.py")
    handoff = _load("installed_handoff", "test-installed-handoff.py")
    expect, fail = permanent.expect, permanent.fail
    if sys.platform != "darwin":
        fail("the outputs-prune proof requires macOS")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    expect(bool(scratch) and os.path.realpath(scratch) == scratch, f"scratch {scratch!r} is not canonical")
    for name in ("TMPDIR", "TMP", "TEMP"):
        expect(os.environ.get(name) == scratch, f"{name} differs from the supervisor scratch")
    expect(
        os.path.isdir(os.environ.get("OXIDE_PROOF_EVIDENCE", "")) and "OXIDE_PROOF_DEADLINE_NS" in os.environ,
        "not under the qualified shared proof supervisor",
    )
    root = tempfile.mkdtemp(prefix="outputs-prune.", dir=scratch)
    repo = os.path.dirname(HERE)
    signal.signal(signal.SIGTERM, permanent.terminated)
    signal.signal(signal.SIGALRM, permanent.overdue)
    signal.setitimer(signal.ITIMER_REAL, permanent.work_left())
    proof = permanent.Proof(root)
    try:
        run(proof, repo)
    except BaseException as failure:
        cleanup = permanent.cleanup_after_failure(proof)
        permanent.record(
            "result.txt",
            f"failed after {len(DONE)} case(s) {DONE}: {failure!r}\n"
            f"unclosed children: {sorted(proof.unclosed)}\nroot: {root}\ncleanup: {cleanup}\n",
        )
        print(f"outputs-prune proof failed; scratch {root} left to the shared supervisor", file=sys.stderr)
        raise
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
    elapsed = time.monotonic() - permanent.layout.runner.STARTED
    line = (
        f"outputs-prune proof held: {len(DONE)} cases, {elapsed:.0f} s script time, peak owned "
        f"{proof.peak // permanent.MIB} MiB at phase checks, phase-sampled host free minimum "
        f"{proof.low // permanent.MIB} MiB; one owned 128 MiB scratch image, ordinary detach"
    )
    permanent.record("result.txt", f"passed\n{line}\n")
    print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
