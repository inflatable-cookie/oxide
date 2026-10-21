"""Front-door accounting routes (oxide#110), on one private directory pool.

Stages this clean HEAD's `oxide` offline with plain Cargo and 1.97.1 as a
debug build, whose walk observer appends the root of every inventory walk to
`OXIDE_TEST_WALK_LOG` (release builds have no observer). Each command gets
its own log. A private HOME, CARGO_HOME and OXIDE_POOL keep the installed
binary and every live pool out of it. One idle slot holds more than 200,000
entries. Cases, in order:

- new slot: one whole-payload admission walk, summary "new slot";
- ordinary reuse: exactly one walk, of the target slot, and the summary's
  provenance label "slot N was X MiB when leased";
- `oxide status` and `oxide new` readiness keep their full walks;
- a live holder: `pool recover-interrupted` refuses with no walk; its oxide
  killed while Cargo's group runs: the refusal names automatic recovery, a
  build in the same checkout waits with no walk, then recovers once the
  group is empty, evicts the untrusted slot and builds cold, walking only
  the reuse target and the new-slot admission;
- new-slot admission with eviction: four whole-payload walks, the idle slot
  ahead in the ledger evicted whole, a slot whose 8 MiB file has four
  hard-link names kept because it counts once, the new slot admitted.

The walk counts show routing only, not a resource gate. Bounds: 20 GiB host
reserve and owned scratch (512 MiB) sampled at phase boundaries only, 300 s
including cleanup under `scripts/bounded-test.py`. Not a benchmark and not
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


class Proof:
    def __init__(self, parent: str, oxide: str, env: dict):
        self.parent = parent
        self.oxide = oxide
        self.pool = os.path.join(parent, "pool")
        self.env = {**env, "OXIDE_POOL": self.pool}
        self.logs = 0

    def payload(self) -> str:
        return os.path.realpath(os.path.join(self.pool, "payload"))

    def slot_dir(self, slot: str) -> str:
        return os.path.join(self.pool, "payload", "slots", slot)

    def log_path(self) -> str:
        self.logs += 1
        return os.path.join(self.parent, "walks", f"{self.logs:02d}.log")

    def walks(self, log: str) -> list:
        """Walk roots in order, relative to the payload where inside it."""
        if not os.path.exists(log):
            return []
        payload = self.payload()
        roots = []
        with open(log, "rb") as handle:
            for line in handle.read().decode().splitlines():
                root = os.path.realpath(line)
                if root == payload:
                    roots.append("payload")
                elif root.startswith(payload + "/"):
                    roots.append(root[len(payload) + 1:])
                else:
                    roots.append(root)
        return roots

    def start(self, cwd: str, args: list, extra: dict | None = None, stderr=subprocess.PIPE):
        log = self.log_path()
        process = subprocess.Popen([self.oxide, *args], cwd=cwd, stdout=subprocess.PIPE, stderr=stderr,
                                   text=True, env={**self.env, "OXIDE_TEST_WALK_LOG": log, **(extra or {})})
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
        return process.returncode, stdout, stderr, self.walks(log), time.monotonic() - started

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


def build(proof: Proof, cwd: str, label: str, expected_walks: list, extra: dict | None = None) -> tuple:
    code, _, stderr, walks, elapsed = proof.call(cwd, ["build"], extra)
    if code:
        fail(f"{label}: exit {code}: {stderr[-2000:]}")
    slot, warmth, tail = summary(stderr, label)
    expected = [walk.format(slot=slot) for walk in expected_walks]
    say(f"{label}: slot {slot} {warmth} in {elapsed:.1f} s, walks {walks}, summary tail: {tail}")
    if walks != expected:
        fail(f"{label}: walks {walks}, expected {expected}")
    if "pool total: see oxide status" not in tail or "allocated of" in tail:
        fail(f"{label}: summary does not point to oxide status: {tail}")
    return slot, warmth, tail, stderr


def status(proof: Proof, cwd: str, label: str, slots: list) -> str:
    code, stdout, stderr, walks, elapsed = proof.call(cwd, ["status"])
    if code:
        fail(f"{label} status exit {code}: {stderr[-1200:]}")
    line = next((line for line in stdout.splitlines() if line.startswith("  slot accounting walk: ")), "")
    counted = re.search(r"complete, (\d+) entries", line)
    say(f"{label} status in {elapsed:.1f} s: {line.strip()}; walks {walks}")
    if not counted or "accounting walk: partial" in stdout or "accounting walk: unknown" in stdout:
        fail(f"{label}: status accounting is not complete:\n{stdout}")
    missing = [root for root in ["payload", *[f"slots/{slot}" for slot in slots]] if root not in walks]
    if walks[:1] != ["payload"] or missing:
        fail(f"{label}: status did not walk the payload first and every slot: {walks}")
    return stdout


def prove_reuse(proof: Proof, app: str, big: str) -> tuple:
    app_slot, warmth, tail, _ = build(proof, app, "new slot", ["payload"])
    if warmth != "cold" or not tail.startswith("new slot, "):
        fail(f"new slot summary: {warmth} {tail}")
    big_slot, _, _, _ = build(proof, big, "second new slot", ["payload"])
    started = time.monotonic()
    fill(os.path.join(proof.slot_dir(big_slot), "build"), BIG_ENTRIES)
    with open(os.path.join(proof.slot_dir(big_slot), "build", "big.bin"), "wb") as handle:
        handle.write(b"\x01" * (8 * MIB))
    count = entries(os.path.join(proof.pool, "payload"))
    say(f"fixture: {count} payload entries, {entries(proof.slot_dir(big_slot))} in idle slot {big_slot}, "
        f"written in {time.monotonic() - started:.1f} s")
    if count < BIG_ENTRIES:
        fail(f"payload has only {count} entries")
    generation = proof.slots()[app_slot]
    slot, warmth, tail, _ = build(proof, app, "ordinary reuse", ["slots/{slot}"])
    label = re.match(rf"slot {app_slot} was (\d+) MiB when leased, \d+ MiB budget, pool total: see oxide status", tail)
    if slot != app_slot or warmth != "warm" or not label:
        fail(f"reuse did not take slot {app_slot} warm with its provenance label: {slot} {warmth} {tail}")
    after = proof.slots()[app_slot]
    if (after["id"], after["incarnation"]) != (generation["id"], generation["incarnation"]):
        fail(f"reuse changed the slot generation: {generation} -> {after}")
    status(proof, app, "after reuse", [app_slot, big_slot])
    return app_slot, big_slot


def prove_readiness(proof: Proof) -> None:
    code, stdout, stderr, walks, elapsed = proof.call(proof.parent, ["new", "readiness"])
    line = next((line for line in stdout.splitlines() if line.startswith("  pool: ")), "")
    say(f"readiness in {elapsed:.1f} s: {line.strip()}; walks {walks}")
    if code or not re.search(r"cap \d+ MiB, \d+ of \d+ MiB used$", line):
        fail(f"readiness: exit {code}, no full pool use line:\n{stdout}\n{stderr[-1200:]}")
    if "payload" not in walks:
        fail(f"readiness made no whole-payload walk: {walks}")


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
        code, _, stderr, walks, _ = proof.call(proof.parent, ["pool", "recover-interrupted", app_slot])
        say(f"live holder: recover-interrupted exit {code}, walks {walks}: {stderr.strip()[-200:]}")
        if code == 0 or "a live oxide command holds slot" not in stderr or walks:
            fail("recover-interrupted did not refuse a live holder without walking")
        holder.send_signal(signal.SIGKILL)
        holder.wait(timeout=30)
        code, _, stderr, walks, _ = proof.call(proof.parent, ["pool", "recover-interrupted", app_slot])
        say(f"killed holder: recover-interrupted exit {code}, walks {walks}: {stderr.strip()[-200:]}")
        if code == 0 or "recovered automatically by the next command" not in stderr or walks:
            fail("recover-interrupted did not defer to automatic recovery without walking")
        if proof.slots()[app_slot] != record:
            fail("a refused recovery changed the ledger")
        waiter_path = os.path.join(proof.parent, "waiter.err")
        waiter_err = open(waiter_path, "w")
        waiter, waiter_log = proof.start(app, ["build"], stderr=waiter_err)
        wait_text(waiter_path, "waiting for another oxide command in this checkout", waiter, "waiter")
        time.sleep(1.0)
        waited = proof.walks(waiter_log)
        say(f"waiting build: walks while waiting {waited}")
        if waited:
            fail(f"a waiting build walked: {waited}")
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
        if walks != [f"slots/{app_slot}", "payload"] or app_slot in proof.slots():
            fail(f"recovered build walks {walks}, ledger {sorted(proof.slots())}")
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


def allocated(root: str) -> int:
    return tree_bytes(root)


def prove_eviction(proof: Proof, fresh: str, big_slot: str, kept_slot: str) -> None:
    linked = os.path.join(proof.slot_dir(kept_slot), "build", "linked.bin")
    with open(linked, "wb") as handle:
        handle.write(b"\x02" * (8 * MIB))
    for index in range(3):
        os.link(linked, f"{linked}.{index}")
    payload = allocated(os.path.join(proof.pool, "payload"))
    big = allocated(proof.slot_dir(big_slot))
    kept = allocated(proof.slot_dir(kept_slot))
    budget = kept + 4 * MIB
    say(f"eviction: payload {payload} B, idle slot {big_slot} {big} B, slot {kept_slot} {kept} B "
        f"once per inode ({kept + 24 * MIB} B per name), soft budget {budget} B")
    if not (payload >= budget and kept < budget <= kept + 24 * MIB):
        fail("the budget does not separate eviction of one slot from once-per-inode accounting")
    order = [record for record in proof.ledger()["slots"]]
    if [str(record["id"]) for record in order][:2] != [big_slot, kept_slot]:
        fail(f"ledger order is not idle slot {big_slot} then {kept_slot}: {order}")
    slot, warmth, tail, _ = build(proof, fresh, "eviction admission", ["payload"] * 4,
                                  {"OXIDE_TEST_BUDGET_BYTES": str(budget)})
    left = proof.slots()
    if os.path.lexists(proof.slot_dir(big_slot)) or big_slot in left:
        fail(f"idle slot {big_slot} was not evicted whole")
    if kept_slot not in left or not os.path.exists(f"{linked}.2"):
        fail(f"slot {kept_slot} with hard links was evicted")
    if warmth != "cold" or not tail.startswith("new slot, ") or slot not in left:
        fail(f"no new slot was admitted: {slot} {warmth} {tail}")
    say(f"eviction: idle slot {big_slot} evicted whole, slot {kept_slot} kept, new slot {slot} admitted")


def main() -> int:
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
        env.update({"HOME": home, "CARGO_HOME": os.path.join(parent, "cargo-home"),
                    "RUSTUP_HOME": os.environ.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup"),
                    "CARGO_NET_OFFLINE": "true", "CARGO_TERM_COLOR": "never",
                    "TMPDIR": os.path.join(parent, "tmp")})
        proof = Proof(parent, oxide, env)
        checkouts = {}
        for name in ("app", "big", "fresh"):
            checkouts[name] = os.path.join(parent, "checkouts", name)
            write_fixture(checkouts[name], name)
        app_slot, big_slot = prove_reuse(proof, checkouts["app"], checkouts["big"])
        owned(parent, "after reuse")
        reserve(parent, "after reuse")
        prove_readiness(proof)
        kept_slot = prove_recovery(proof, checkouts["app"], app_slot)
        owned(parent, "after recovery")
        reserve(parent, "after recovery")
        prove_eviction(proof, checkouts["fresh"], big_slot, kept_slot)
        owned(parent, "after eviction")
        status(proof, checkouts["fresh"], "final", sorted(proof.slots()))
        reserve(parent, "after eviction")
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
