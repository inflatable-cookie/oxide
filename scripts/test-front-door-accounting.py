"""Front-door accounting routes (oxide#110), on one private directory pool.

Stages this clean HEAD's `oxide` offline with plain Cargo and 1.97.1 as a
debug build, whose walk observer appends the root of every inventory walk to
`OXIDE_TEST_WALK_LOG` and every shallow scope pass to
`OXIDE_TEST_SCOPE_LOG` (release builds have no observer). Each command gets
its own logs. A private HOME, CARGO_HOME and OXIDE_POOL keep the installed
binary and every live pool out of it. One idle slot later holds more than
200,000 entries. Cases, in order:

- new slot (twice): the admission makes no inventory walk at all — recorded
  per-slot accounting (oxide#146) replaced the pre-#146 whole-payload
  admission walk — and exactly one shallow payload scope pass; the release
  then walks only the slot it closed, so the command's walk log is
  `["slots/<new>"]`;
- default `oxide status` walks nothing and makes no scope pass (recorded
  facts only);
- `oxide status --measure` walks the payload first and then every slot, with
  complete accounting on this small pool;
- `oxide new` readiness walks nothing and makes no scope pass: since
  oxide#182 it reads no pool state at all ("use not measured");
- ordinary reuse walks exactly its target slot at admission and again at
  release, and prints the provenance label "slot N was X MiB when leased";
- a live holder's `pool recover-interrupted` refuses with no walk, and so
  does the same after its oxide is killed: the refusal names automatic
  recovery, a build in the same checkout waits with no walk, then recovers
  once the Cargo group is empty, walks the slot it is about to reuse, has
  the freshness gate refuse it, evicts the untrusted slot through the
  oxide#190 holding deletion, and builds a new slot cold, whose release walk
  is the last walk of the command;
- a final default `status` after recovery still walks nothing.

Retired rather than duplicated: the byte-pressure eviction case and its
hard-link control now live in `rust:recorded-slot-accounting` (case D drives
byte pressure and evicts one owned idle whole slot; case E proves a cross-slot
hard link is counted once) and the holding deletion under it in
`rust:holding-delete`. This script keeps the routing facts those proofs do
not cover: readiness, held and killed-holder refusals, the waiting build and
the recovered cold build.

The walk counts show routing only, not a resource gate. Bounds: 20 GiB host
reserve and owned scratch (512 MiB) sampled at phase boundaries only, 300 s
including cleanup under `scripts/bounded-test.py`. `--self-check` is the pure
apparatus check: no child, pool, image or scratch. Not a benchmark and not
full QA. Scratch is removed only after positive completion.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

BUILDER = "1.97.1"
SCRATCH_LIMIT_BYTES = 512 * 1024 * 1024
HOST_RESERVE_BYTES = 20 * 1024 * 1024 * 1024
PROOF_SECONDS = 300
MIB = 1024 * 1024
BIG_ENTRIES = 200_000
STARTED = time.monotonic()
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PEAK = {"bytes": 0, "phase": "none"}
# The product's own readiness line since oxide#182: it reads no pool state.
READINESS_NEEDLE = "use not measured (readiness reads no pool state)"
USAGE = "usage: test-front-door-accounting.py [--self-check]"

HOLD_TEST = """use std::path::PathBuf;
use std::time::Duration;

#[test]
fn optional_hold() {
    let Some(pidfile) = std::env::var_os("OXIDE_FIXTURE_HOLD").map(PathBuf::from) else {
        return;
    };
    std::fs::write(&pidfile, std::process::id().to_string()).expect("pid");
    let release = pidfile.with_extension("release");
    for _ in 0..1200 {
        if release.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("hold timed out");
}
"""


def fail(message: str) -> None:
    raise RuntimeError(message)


def remaining() -> float:
    left = PROOF_SECONDS - (time.monotonic() - STARTED)
    if left <= 0:
        fail(f"proof exceeded {PROOF_SECONDS} s")
    return left


def say(message: str) -> None:
    print(f"front-door {time.monotonic() - STARTED:6.1f}s {message}", flush=True)


def reserve(path: str, phase: str) -> None:
    stat = os.statvfs(path)
    free = stat.f_bavail * stat.f_frsize
    say(f"reserve {phase}: host free {free // MIB} MiB")
    if free < HOST_RESERVE_BYTES:
        fail(f"host free {free} is below the 20 GiB reserve at {phase}")


def tree_bytes(root: str) -> int:
    total = 0
    seen = set()
    for current, dirs, files in os.walk(root, followlinks=False):
        for name in dirs + files:
            stat = os.lstat(os.path.join(current, name))
            if (stat.st_dev, stat.st_ino) not in seen:
                seen.add((stat.st_dev, stat.st_ino))
                total += stat.st_blocks * 512
    return total


def owned(parent: str, phase: str) -> int:
    used = tree_bytes(parent)
    if used > PEAK["bytes"]:
        PEAK.update(bytes=used, phase=phase)
    say(f"owned scratch {phase}: {used // MIB} MiB")
    if used > SCRATCH_LIMIT_BYTES:
        fail(f"owned scratch {used} exceeds 512 MiB at {phase}")
    return used


def sha256(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def stage(parent: str, env: dict) -> str:
    target = os.path.join(parent, "stage")
    log = os.path.join(parent, "stage.log")
    with open(log, "w") as handle:
        build = subprocess.run(
            ["cargo", "build", "--locked", "--offline", "--bin", "oxide"],
            cwd=REPO, env={**env, "CARGO_TARGET_DIR": target, "CARGO_BUILD_BUILD_DIR": target},
            stdout=handle, stderr=subprocess.STDOUT, timeout=remaining(),
        )
    with open(log) as handle:
        text = handle.read()
    if build.returncode:
        fail(f"staging exit {build.returncode}: {text[-2000:]}")
    out = os.path.join(parent, "bin", "oxide")
    os.makedirs(os.path.dirname(out))
    shutil.copy2(os.path.join(target, "debug", "oxide"), out)
    with open(out, "rb") as handle:
        if b"OXIDE_TEST_WALK_LOG" not in handle.read():
            fail("the staged debug binary has no walk observer")
    say(f"staged {out}: sha256 {sha256(out)}")
    owned(parent, "after staging")
    shutil.rmtree(target)
    return out


def write_fixture(root: str, name: str) -> None:
    files = {
        "Cargo.toml": f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n',
        "rust-toolchain.toml": f'[toolchain]\nchannel = "{BUILDER}"\n',
        "src/main.rs": f'fn main() {{\n    println!("{name}");\n}}\n',
        "tests/hold.rs": HOLD_TEST,
    }
    for relative, text in files.items():
        path = os.path.join(root, relative)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as handle:
            handle.write(text)


def fill(root: str, count: int) -> None:
    """`count` entries: directories of 100 empty files in a fresh subtree."""
    base = os.path.join(root, "filler")
    os.mkdir(base)
    made = 1
    group = 0
    while made < count:
        directory = os.path.join(base, f"d{group}")
        os.mkdir(directory)
        made += 1
        for index in range(min(100, count - made)):
            os.close(os.open(os.path.join(directory, f"f{index}"), os.O_CREAT | os.O_WRONLY, 0o600))
            made += 1
        group += 1


def entries(root: str) -> int:
    return sum(len(dirs) + len(files) for _, dirs, files in os.walk(root, followlinks=False))


def relative_walk(payload: str, root: str) -> str:
    """One recorded walk root, relative to the payload where it is inside it."""
    root = os.path.realpath(root)
    if root == payload:
        return "payload"
    if root.startswith(payload + "/"):
        relative = root[len(payload) + 1:]
        # A labelled slot root reads as its legacy `slots/<id>`.
        labelled = re.fullmatch(r"slots/[^/]+/[a-z0-9_.-]+-(\d+)", relative)
        return f"slots/{labelled.group(1)}" if labelled else relative
    return root


def read_roots(log: str, payload: str) -> list:
    if not os.path.exists(log):
        return []
    with open(log, "rb") as handle:
        return [relative_walk(payload, line.decode()) for line in handle.read().splitlines()]


def expect_roots(label: str, kind: str, roots: list, expected: list) -> None:
    if roots != expected:
        fail(f"{label}: {kind} {roots}, expected {expected}")


class Proof:
    def __init__(self, parent: str, oxide: str, env: dict):
        self.parent = parent
        self.oxide = oxide
        self.pool = os.path.join(parent, "pool")
        self.env = {**env, "OXIDE_POOL": self.pool, "OXIDE_POOL_CREATE": "1"}
        self.logs = 0
        self.dirs = {}

    def payload(self) -> str:
        return os.path.realpath(os.path.join(self.pool, "payload"))

    def slot_dir(self, slot: str) -> str:
        # oxide#104: a new slot records `dir` (`<folder>/<profile>-<id>`);
        # remembered so an evicted slot's path is still checked.
        record = self.slots().get(slot)
        if record is not None:
            self.dirs[slot] = record.get("dir") or slot
        return os.path.join(self.pool, "payload", "slots", self.dirs.get(slot, slot))

    def log_path(self) -> str:
        self.logs += 1
        return os.path.join(self.parent, "walks", f"{self.logs:02d}.log")

    def walks(self, log: str) -> list:
        """Walk roots in order, relative to the payload where inside it."""
        return read_roots(log, self.payload())

    def scopes(self, log: str) -> list:
        """Shallow scope passes in order, on the separate scope log."""
        return read_roots(log + ".scope", self.payload())

    def start(self, cwd: str, args: list, extra: dict | None = None, stderr=subprocess.PIPE):
        log = self.log_path()
        process = subprocess.Popen([self.oxide, *args], cwd=cwd, stdout=subprocess.PIPE, stderr=stderr,
                                   text=True, env={**self.env, "OXIDE_TEST_WALK_LOG": log,
                                                   "OXIDE_TEST_SCOPE_LOG": log + ".scope", **(extra or {})})
        return process, log

    def call(self, cwd: str, args: list, extra: dict | None = None) -> tuple:
        process, log = self.start(cwd, args, extra)
        started = time.monotonic()
        try:
            stdout, stderr = process.communicate(timeout=min(120, remaining()))
        except subprocess.TimeoutExpired:
            process.kill()
            process.communicate()
            fail(f"oxide {args} did not finish")
        if "walk log" in stderr and "not written" in stderr:
            fail(f"oxide {args} could not write its walk log: {stderr[-800:]}")
        return (process.returncode, stdout, stderr, self.walks(log), self.scopes(log),
                time.monotonic() - started)

    def ledger(self) -> dict:
        with open(os.path.join(self.pool, "ledger.json")) as handle:
            return json.load(handle)

    def slots(self) -> dict:
        return {str(record["id"]): record for record in self.ledger()["slots"]}


def summary(stderr: str, label: str) -> tuple:
    match = re.search(r"oxide: \w+ profile, [\w+]+ strategy, slot (\d+) (warm|cold), [\d.]+s, (.*)", stderr)
    if not match:
        fail(f"{label}: no summary line in {stderr[-2000:]!r}")
    return match.group(1), match.group(2), match.group(3)


def build(proof: Proof, cwd: str, label: str, expected_walks: list,
          expected_scopes: list | None = None, extra: dict | None = None) -> tuple:
    code, _, stderr, walks, scopes, elapsed = proof.call(cwd, ["build"], extra)
    if code:
        fail(f"{label}: exit {code}: {stderr[-2000:]}")
    slot, warmth, tail = summary(stderr, label)
    expected = [walk.format(slot=slot) for walk in expected_walks]
    say(f"{label}: slot {slot} {warmth} in {elapsed:.1f} s, walks {walks}, scopes {scopes}, "
        f"summary tail: {tail}")
    expect_roots(label, "walks", walks, expected)
    if expected_scopes is not None:
        expect_roots(label, "scopes", scopes, [scope.format(slot=slot) for scope in expected_scopes])
    if "pool total: see oxide status" not in tail or "allocated of" in tail:
        fail(f"{label}: summary does not point to oxide status: {tail}")
    return slot, warmth, tail, stderr


def recorded_status(proof: Proof, cwd: str, label: str) -> str:
    """Default status reads recorded facts only: no walk, no scope, no reconcile."""
    code, stdout, stderr, walks, scopes, elapsed = proof.call(cwd, ["status"])
    if code:
        fail(f"{label} recorded status exit {code}: {stderr[-1200:]}")
    say(f"{label} recorded status in {elapsed:.1f} s: walks {walks}, scopes {scopes}")
    if walks or scopes:
        fail(f"{label}: default status walked {walks} or scoped {scopes}")
    if "slot sizes: recorded observations only" not in stdout:
        fail(f"{label}: default status did not report recorded sizes:\n{stdout}")
    if "slot accounting walk: " in stdout:
        fail(f"{label}: default status printed an accounting walk:\n{stdout}")
    return stdout


def status(proof: Proof, cwd: str, label: str, slots: list) -> str:
    """`--measure` is the audit: the payload first, then every slot."""
    code, stdout, stderr, walks, scopes, elapsed = proof.call(cwd, ["status", "--measure"])
    if code:
        fail(f"{label} status --measure exit {code}: {stderr[-1200:]}")
    line = next((line for line in stdout.splitlines() if line.startswith("  slot accounting walk: ")), "")
    counted = re.search(r"complete, (\d+) entries", line)
    say(f"{label} status --measure in {elapsed:.1f} s: {line.strip()}; walks {walks}, scopes {scopes}")
    if not counted or "accounting walk: partial" in stdout or "accounting walk: unknown" in stdout:
        fail(f"{label}: status --measure accounting is not complete:\n{stdout}")
    expect_roots(label, "walks", walks, ["payload", *[f"slots/{slot}" for slot in slots]])
    return stdout


def prove_slots(proof: Proof, app: str, big: str) -> tuple:
    """Two new slots: no admission walk, one payload scope pass, a release walk."""
    app_slot, warmth, tail, _ = build(proof, app, "new slot", ["slots/{slot}"], ["payload"])
    if warmth != "cold" or not tail.startswith("new slot, "):
        fail(f"new slot summary: {warmth} {tail}")
    big_slot, warmth, tail, _ = build(proof, big, "second new slot", ["slots/{slot}"], ["payload"])
    if warmth != "cold" or not tail.startswith("new slot, "):
        fail(f"second new slot summary: {warmth} {tail}")
    return app_slot, big_slot


def prove_reuse(proof: Proof, app: str, app_slot: str, big_slot: str) -> None:
    """Fill the idle slot, then prove ordinary reuse: its slot, twice."""
    started = time.monotonic()
    fill(os.path.join(proof.slot_dir(big_slot), "build"), BIG_ENTRIES)
    with open(os.path.join(proof.slot_dir(big_slot), "build", "big.bin"), "wb") as handle:
        handle.write(b"\x01" * (8 * MIB))
    count = entries(os.path.join(proof.pool, "payload"))
    say(f"fixture: {count} payload entries, {entries(proof.slot_dir(big_slot))} in idle slot {big_slot}, "
        f"written in {time.monotonic() - started:.1f} s")
    if count < BIG_ENTRIES:
        fail(f"payload has only {count} entries")
    generation = proof.slots()[big_slot]
    before = proof.slots()[app_slot]
    slot, warmth, tail, _ = build(proof, app, "ordinary reuse",
                                  ["slots/{slot}", "slots/{slot}"])
    label = re.match(rf"slot {app_slot} was (\d+) MiB when leased, \d+ MiB budget, pool total: see oxide status", tail)
    if slot != app_slot or warmth != "warm" or not label:
        fail(f"reuse did not take slot {app_slot} warm with its provenance label: {slot} {warmth} {tail}")
    after = proof.slots()[app_slot]
    if (after["id"], after["incarnation"]) != (before["id"], before["incarnation"]):
        fail(f"reuse changed the slot generation: {before} -> {after}")
    if proof.slots()[big_slot] != generation:
        fail("the idle slot beside the reuse changed")


def prove_readiness(proof: Proof) -> None:
    """Readiness (oxide#182) reads no pool state: no walk, no scope pass."""
    code, stdout, stderr, walks, scopes, elapsed = proof.call(proof.parent, ["new", "readiness"])
    lines = stdout.splitlines()
    selected = next((line for line in lines if line.startswith("  selected pool: ")), "")
    configured = next((line for line in lines if line.startswith("  configured internal pool: ")), "")
    say(f"readiness in {elapsed:.1f} s: {configured.strip()}; walks {walks}, scopes {scopes}")
    if code:
        fail(f"readiness exit {code}:\n{stdout}\n{stderr[-1200:]}")
    if not selected:
        fail(f"readiness printed no selected pool line:\n{stdout[-1500:]}")
    if READINESS_NEEDLE not in configured:
        fail(f"readiness did not report reading no pool state:\n{stdout[-1500:]}")
    if walks or scopes:
        fail(f"readiness walked inventory {walks} or scoped {scopes}")


def wait_for(path: str, process, label: str, seconds: float = 90) -> None:
    deadline = time.monotonic() + min(seconds, remaining())
    while not os.path.exists(path):
        if process.poll() is not None or time.monotonic() > deadline:
            fail(f"{label}: {'exited ' + str(process.returncode) if process.poll() is not None else 'timeout'}")
        time.sleep(0.1)


def wait_text(path: str, text: str, process, label: str, seconds: float = 60) -> None:
    deadline = time.monotonic() + min(seconds, remaining())
    while True:
        with open(path) as handle:
            if text in handle.read():
                return
        if process.poll() is not None or time.monotonic() > deadline:
            fail(f"{label}: no {text!r}")
        time.sleep(0.1)


def prove_recovery(proof: Proof, app: str, app_slot: str) -> str:
    pidfile = os.path.join(proof.parent, "hold.pid")
    release = os.path.join(proof.parent, "hold.release")
    holder_err = open(os.path.join(proof.parent, "holder.err"), "w")
    holder, holder_log = proof.start(app, ["test"], {"OXIDE_FIXTURE_HOLD": pidfile}, stderr=holder_err)
    waiter = None
    try:
        wait_for(pidfile, holder, "held test start")
        record = proof.slots()[app_slot]
        if record["state"] != "held":
            fail(f"slot {app_slot} is not held: {record}")
        code, _, stderr, walks, scopes, _ = proof.call(proof.parent, ["pool", "recover-interrupted", app_slot])
        say(f"live holder: recover-interrupted exit {code}, walks {walks}, scopes {scopes}: {stderr.strip()[-200:]}")
        if code == 0 or "a live oxide command holds slot" not in stderr or walks or scopes:
            fail("recover-interrupted did not refuse a live holder without walking")
        holder.send_signal(signal.SIGKILL)
        holder.wait(timeout=30)
        code, _, stderr, walks, scopes, _ = proof.call(proof.parent, ["pool", "recover-interrupted", app_slot])
        say(f"killed holder: recover-interrupted exit {code}, walks {walks}, scopes {scopes}: {stderr.strip()[-200:]}")
        if code == 0 or "recovered automatically by the next command" not in stderr or walks or scopes:
            fail("recover-interrupted did not defer to automatic recovery without walking")
        if proof.slots()[app_slot] != record:
            fail("a refused recovery changed the ledger")
        waiter_path = os.path.join(proof.parent, "waiter.err")
        waiter_err = open(waiter_path, "w")
        waiter, waiter_log = proof.start(app, ["build"], stderr=waiter_err)
        wait_text(waiter_path, "waiting for another oxide command in this checkout", waiter, "waiter")
        time.sleep(1.0)
        waited = proof.walks(waiter_log)
        waited_scopes = proof.scopes(waiter_log)
        say(f"waiting build: walks while waiting {waited}, scopes {waited_scopes}")
        if waited or waited_scopes:
            fail(f"a waiting build walked {waited} or scoped {waited_scopes}")
        with open(release, "w"):
            pass
        code = waiter.wait(timeout=min(120, remaining()))
        waiter_err.close()
        with open(waiter_path) as handle:
            stderr = handle.read()
        walks = proof.walks(waiter_log)
        slot, warmth, tail = summary(stderr, "recovered build")
        say(f"recovered build: exit {code}, slot {slot} {warmth}, walks {walks}, summary tail: {tail}")
        if code or not re.search(rf"recovered slot {app_slot} after oxide pid {holder.pid} died \(Cargo group \d+ empty\)", stderr):
            fail(f"the build did not recover slot {app_slot}: {stderr[-2000:]}")
        if f"cannot vouch for slot {app_slot}" not in stderr or warmth != "cold" or not tail.startswith("new slot, "):
            fail(f"the untrusted recovered slot was not evicted for a cold build: {stderr[-2000:]}")
        # The command re-leases the recovered slot (its own reuse walk), the
        # freshness gate refuses it, and the new slot's release is the last
        # walk. Nothing walks the payload or the idle slot beside it.
        expect_roots("recovered build", "walks", walks, [f"slots/{app_slot}", f"slots/{slot}"])
        if app_slot in proof.slots() or slot in ("", app_slot):
            fail(f"recovered build ledger {sorted(proof.slots())}, slot {slot}")
        if proof.walks(holder_log) != [f"slots/{app_slot}"]:
            fail(f"the holder's lease walked {proof.walks(holder_log)}")
        return slot
    finally:
        with open(release, "w"):
            pass
        holder_err.close()
        for process in (holder, waiter):
            if process is not None and process.poll() is None:
                process.kill()
                process.wait()


def self_check() -> int:
    """Pure apparatus check: no child, pool, image, scratch or supervisor."""
    root = tempfile.mkdtemp(prefix="front-door-self-check.")
    try:
        proof = Proof.__new__(Proof)
        proof.parent = root
        proof.pool = os.path.join(root, "pool")
        proof.dirs = {}
        payload = os.path.join(root, "pool", "payload")
        log = os.path.join(root, "walk.log")
        scope = log + ".scope"
        slot = os.path.join(payload, "slots", "app-1a2b", "quick-3")
        with open(log, "w") as handle:
            for line in (payload, payload + "/slots/1", slot, os.path.join(root, "outside")):
                handle.write(line + "\n")
        with open(scope, "w") as handle:
            handle.write(payload + "\n")
        # Walk/scope roots map to their documented names; the labelled slot
        # root reads as its legacy `slots/<id>` and an outside root stays
        # absolute.
        expect_roots("self-check", "walks", proof.walks(log),
                     ["payload", "slots/1", "slots/3", os.path.realpath(os.path.join(root, "outside"))])
        expect_roots("self-check", "scopes", proof.scopes(log), ["payload"])
        expect_roots("self-check", "absent logs", proof.walks(os.path.join(root, "missing")), [])
        # The oracle compares exact ordered lists. A passing list must pass,
        # and each wrong shape - the pre-#146 whole-payload admission walk, a
        # missing release walk, a reordered list, a dropped duplicate - must
        # fail rather than being accepted as close enough.
        expect_roots("self-check", "walks", ["slots/1"], ["slots/1"])
        expect_roots("self-check", "walks", [], [])
        for bad, expected, why in (
            (["payload"], ["slots/1"], "a whole-payload admission walk"),
            (["slots/1"], ["payload", "slots/1"], "a missing release walk"),
            (["slots/1", "slots/1"], ["slots/2", "slots/1"], "a reordered walk list"),
            (["slots/1"], ["slots/1", "slots/1"], "a dropped duplicate walk"),
        ):
            try:
                expect_roots("self-check", "walks", bad, expected)
            except RuntimeError:
                print(f"walk oracle refused {why}")
            else:
                fail(f"the oracle accepted {why}")
        # The two product lines the oracles read, as the product prints them.
        slot_id, warmth, tail = summary(
            "oxide: quick profile, build strategy, slot 3 warm, 1.2s, slot 3 was 8 MiB when leased, "
            "64 MiB budget, pool total: see oxide status (directory-budget-v1, budget-only), "
            "temp: inherited /fixture/tmp\n",
            "self-check",
        )
        expect_true((slot_id, warmth) == ("3", "warm"), f"{slot_id} {warmth}")
        label = re.match(rf"slot 3 was (\d+) MiB when leased, \d+ MiB budget, pool total: see oxide status", tail)
        expect_true(label is not None, f"the reuse provenance label was not parsed: {tail}")
        expect_true(
            re.search(r"complete, (\d+) entries", "  slot accounting walk: complete, 42 entries, 3 ms") is not None,
            "the measure line was not parsed",
        )
        expect_true(
            READINESS_NEEDLE
            in "  configured internal pool: /fixture/pool, use not measured (readiness reads no pool state)",
            "the readiness needle is not the product's line",
        )
        # The product's own readiness sentence is a source string, not a guess.
        with open(os.path.join(REPO, "src", "bin", "oxide", "adopt.rs"), encoding="utf-8") as handle:
            expect_true(READINESS_NEEDLE in handle.read(), "adopt.rs lost the readiness sentence")
    finally:
        shutil.rmtree(root, ignore_errors=True)
    # Only no arguments or `--self-check` select a mode.
    expect_true(mode_from_args([]) == "run", "no-argument mode")
    expect_true(mode_from_args(["--self-check"]) == "self-check", "self-check mode")
    for refused in (["--bogus"], ["--self-check", "--self-check"], ["run"]):
        expect_true(mode_from_args(refused) is None, f"{refused} selected a mode")
    print("front-door self-check ok: walk/scope root mapping, exact ordered walk oracles, "
          "summary and provenance parsing, measure line, readiness needle, dispatch")
    return 0


def expect_true(condition: bool, message: str = "expectation") -> None:
    if not condition:
        fail(message)


def mode_from_args(arguments: list) -> str | None:
    if not arguments:
        return "run"
    if arguments == ["--self-check"]:
        return "self-check"
    return None


def main() -> int:
    arguments = list(sys.argv[1:])
    mode = mode_from_args(arguments)
    if mode is None:
        print(USAGE, file=sys.stderr)
        return 2
    if mode == "self-check":
        return self_check()
    if sys.platform != "darwin":
        fail("the front-door accounting proof requires macOS")
    if subprocess.run(["git", "status", "--porcelain"], cwd=REPO, capture_output=True,
                      text=True, timeout=30).stdout.strip():
        fail("the checkout is not clean, so HEAD does not name its source")
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True,
                          text=True, timeout=30).stdout.strip()
    reserve("/private/tmp", "admission")
    made = subprocess.run(["mktemp", "-d", "/private/tmp/oxide-front-door.XXXXXX"],
                          capture_output=True, text=True, timeout=30)
    if made.returncode:
        fail("mktemp failed")
    parent = os.path.realpath(made.stdout.strip())
    os.chmod(parent, 0o700)
    say(f"source {head}, scratch {parent}")
    try:
        stage_env = {name: value for name, value in os.environ.items()
                     if not name.startswith(("CARGO_BUILD_", "CARGO_TARGET_", "RUSTC", "SCCACHE", "OXIDE_"))
                     and name not in ("CARGO", "CARGO_INCREMENTAL", "RUSTUP_TOOLCHAIN")}
        stage_env.update({"RUSTUP_TOOLCHAIN": BUILDER, "RUSTC_WRAPPER": "", "CARGO_PROFILE_DEV_DEBUG": "0",
                          "CARGO_NET_OFFLINE": "true"})
        oxide = stage(parent, stage_env)
        reserve(parent, "after staging")

        home = os.path.join(parent, "home")
        for path in (home, os.path.join(parent, "cargo-home"), os.path.join(parent, "tmp"),
                     os.path.join(parent, "walks")):
            os.makedirs(path, mode=0o700)
        env = {name: value for name, value in stage_env.items()
               if not name.startswith("CARGO_") and name not in ("RUSTUP_TOOLCHAIN", "RUSTC_WRAPPER")}
        env.update({"OXIDE_TEST_HOME_FROM_ENV": "1", "HOME": home, "CARGO_HOME": os.path.join(parent, "cargo-home"),
                    "RUSTUP_HOME": os.environ.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup"),
                    "CARGO_NET_OFFLINE": "true", "CARGO_TERM_COLOR": "never",
                    "TMPDIR": os.path.join(parent, "tmp")})
        proof = Proof(parent, oxide, env)
        checkouts = {}
        for name in ("app", "big"):
            checkouts[name] = os.path.join(parent, "checkouts", name)
            write_fixture(checkouts[name], name)
        app_slot, big_slot = prove_slots(proof, checkouts["app"], checkouts["big"])
        recorded_status(proof, checkouts["app"], "after two new slots")
        status(proof, checkouts["app"], "after two new slots", [app_slot, big_slot])
        prove_readiness(proof)
        owned(parent, "after readiness")
        reserve(parent, "after readiness")
        prove_reuse(proof, checkouts["app"], app_slot, big_slot)
        owned(parent, "after reuse")
        reserve(parent, "after reuse")
        kept_slot = prove_recovery(proof, checkouts["app"], app_slot)
        if kept_slot not in proof.slots() or kept_slot == big_slot:
            fail(f"the recovered cold slot {kept_slot} is not a new slot in {sorted(proof.slots())}")
        owned(parent, "after recovery")
        reserve(parent, "after recovery")
        recorded_status(proof, checkouts["app"], "after recovery")
        reserve(parent, "after recovery status")
    except Exception:
        print(f"front-door accounting proof retained its owned scratch at {parent}", file=sys.stderr)
        raise
    shutil.rmtree(parent)
    elapsed = time.monotonic() - STARTED
    if elapsed > PROOF_SECONDS:
        fail(f"front-door accounting proof took {elapsed:.0f} s, over {PROOF_SECONDS} s")
    print(f"oxide front-door accounting proof held: source {head}, {elapsed:.0f} s including cleanup, "
          f"peak owned scratch sampled {PEAK['bytes'] // MIB} MiB ({PEAK['phase']}), scratch removed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
