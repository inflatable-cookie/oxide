#!/usr/bin/env python3
"""Owned-output prune proof (oxide#129): the fixed 21-case native script.

Run only as the Effigy heavy task `rust:outputs-prune`, whose whole command is
`python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 --
python3 scripts/test-outputs-prune.py` under the qualified shared supervisor
(`scripts/bounded-test.py`, SHA-256
d89115f1ca03587e1d9134efa153cc9c94860d2904f52e2a11bc8c8fbe21f25e). The
supervisor owns the 300 s deadline, the 512 MiB monitored scratch, process
closure and cleanup; work stops 30 s before its exported deadline. Activation
is a separate counted owner gate for the author and for the Elevated
reviewer; `--plan` prints the allocation and case list and touches nothing.

Static allocation (one 128 MiB owned APFS sparse image, private HOME,
CARGO_HOME and directory pool; no real Builds drive, consumer, Windows,
provider or install):

    staging (constrained debug oxide recipe)      288 MiB
    APFS sparse image (payload + quotas)          128 MiB
    fixtures                                       32 MiB
    evidence                                       32 MiB
    margin                                         32 MiB
    ---------------------------------------------  ------
    total                                         512 MiB

Staging is the rust:installed-handoff recipe (this HEAD's debug `oxide`,
plain Cargo offline with 1.97.1, -j1, one codegen unit, no debug info,
symbols stripped, opt-level s, debug assertions kept so the debug-only
OXIDE_TEST_PRUNE_CRASH and OXIDE_TEST_PAUSE hooks exist; the staged target is
deleted after the copy). Expected: staging 37-60 s, cases 60-120 s,
attach/setup 20 s, cleanup 30 s = 147-230 s before the source-fit
additions (a gated build-script fixture, two extra fixture builds, one
600-entry root and three extra prune runs, estimated +15-30 s): 162-260 s,
UNMEASURED. If a run does not
fit, it fails at the deadline (never a timeout success) and one honest larger
bound is proposed before any further activation; never split or extended.

Watchdog / reserve call chain exercised by the deletion cases (source, not a
bespoke harness): prune::prune_one -> Store::begin_clean -> lock_binding ->
RootSession::open (removable::Binding::open + verify of UUID, device, mount;
registration reserve) -> RootSession::check (Binding::verify + available
bytes above the recorded reserve) before the rename and every DELETE_BATCH
entries inside delete_contents_at; any loss stops further writes and leaves
the intent pending (exit 3).

Cases (21 = the original 19-case matrix plus two resolve-intent cases), each
on tiny owned fixtures with exact exit codes and state assertions:

  fence / init (2)
    01 init: adopt publishes state version 3 with oxide.outputs/2 and a ready
       transaction in both controls, and oxide.outputs.registry/1 exists.
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
       the same released root is a candidate once its checkout is gone.
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
       by-name oracle and allocated bytes count a hard-linked 64 KiB inode
       once (complete coverage); `oxide status` shows the observation.
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
LIMIT_SECONDS = 300
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

    def checkout(self, name: str) -> str:
        path = os.path.join(self.root, "checkouts", name)
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
        away = os.path.join(self.gone, os.path.basename(checkout) + "-" + secrets.token_hex(3))
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
    code, _, stderr = world.real_run("low reserve mid-batch", alpha, plan, sha, extra={"OXIDE_TEST_PRUNE_LOW_RESERVE_AT": "3"})
    expect(code == 3 and "reserve" in stderr, f"low reserve mid-batch exited {code}")
    expect(os.path.isfile(fired) and world.intents() and world.quarantines(), "no pending mid-batch stop")
    # Checks 1 (pre-rename) and 2 (before the first delete) passed; check 3
    # fired between batches: some entries were deleted, none after the stop.
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
    expect(observed["allocated_bytes"] >= expected_allocated, f"allocated {observed['allocated_bytes']} < oracle {expected_allocated}")
    expect(observed["allocated_bytes"] < observed["logical_bytes"], "the hard link was allocated twice")
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
