"""Pool accounting at build-cache scale (oxide#108), on private directory pools.

Stages the candidate `oxide` (this clean HEAD) and the reference `oxide` (the
merge-base with origin/main, old 50,000-entry pool bound) offline with plain
Cargo and 1.97.1, no rustc wrapper, into owned scratch: each from its own
source tree into its own fresh target and build directory. Before any fixture
it records each source commit, tree, manifest and lock hash, the staged binary
SHA-256 and Cargo's compiled/fresh lines, and requires the two binaries to
differ and only the candidate to carry its new accounting diagnostic. Fixture builds use a private HOME, CARGO_HOME and OXIDE_POOL, never
the installed binary or a live pool. Cases, each with more than 50,000 real
entries:

- single slot: one known-idle slot holds 60,000 extra entries; the old binary
  refuses `unknown_accounting`, the candidate reuses the SAME slot generation
  (id, incarnation, marker inode) warm, below the recycle threshold;
- aggregate: two idle slots of 30,000 entries each; the old binary still
  reuses one but refuses a new slot, the candidate admits a new slot;
- eviction: a held slot whose 4 MiB file has four hard-link names, beside an
  idle slot of 60,000 entries; a new checkout under a soft budget between the
  held slot's once-per-inode and per-name totals evicts only the idle slot.

Bounds: 20 GiB host reserve sampled at each phase, 512 MiB owned scratch,
300 s including cleanup. Reports counts, allocation and walk times; not a
benchmark and not full QA. Scratch is removed only after positive completion.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time

BUILDER = "1.97.1"
SCRATCH_LIMIT_BYTES = 512 * 1024 * 1024
HOST_RESERVE_BYTES = 20 * 1024 * 1024 * 1024
PROOF_SECONDS = 300
OLD_BOUND = 50_000
MIB = 1024 * 1024
STARTED = time.monotonic()
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

FIXTURE = {
    "Cargo.toml": '[package]\nname = "scale"\nversion = "0.1.0"\nedition = "2021"\n',
    "rust-toolchain.toml": f'[toolchain]\nchannel = "{BUILDER}"\n',
    "src/main.rs": 'fn main() {\n    println!("scale");\n}\n',
    "tests/hold.rs": """use std::path::PathBuf;
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
""",
}


def fail(message: str) -> None:
    raise RuntimeError(message)


def remaining() -> float:
    left = PROOF_SECONDS - (time.monotonic() - STARTED)
    if left <= 0:
        fail(f"proof exceeded {PROOF_SECONDS} s")
    return left


def host_free(path: str) -> int:
    stat = os.statvfs(path)
    return stat.f_bavail * stat.f_frsize


def reserve(path: str, phase: str) -> None:
    free = host_free(path)
    print(f"pool-scale reserve {phase}: host free {free // MIB} MiB")
    if free < HOST_RESERVE_BYTES:
        fail(f"host free {free} is below the 20 GiB reserve at {phase}")


def tree_bytes(root: str) -> int:
    total = 0
    for current, dirs, files in os.walk(root, followlinks=False):
        for name in dirs + files:
            total += os.lstat(os.path.join(current, name)).st_blocks * 512
    return total


def owned(parent: str, phase: str) -> int:
    used = tree_bytes(parent)
    print(f"pool-scale owned scratch {phase}: {used // MIB} MiB")
    if used > SCRATCH_LIMIT_BYTES:
        fail(f"owned scratch {used} exceeds 512 MiB at {phase}")
    return used


DIAGNOSTIC = b"owned accounting is incomplete"


def sha256(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def stage(source: str, target: str, out: str, log: str, env: dict) -> str:
    """Build `oxide` from `source` into its own fresh target and build dir."""
    if os.path.lexists(target):
        fail(f"staging target {target} already exists")
    with open(log, "w") as handle:
        build = subprocess.run(
            ["cargo", "build", "--locked", "--offline", "-v", "--bin", "oxide",
             "--manifest-path", os.path.join(source, "Cargo.toml")],
            cwd=source, env={**env, "CARGO_TARGET_DIR": target, "CARGO_BUILD_BUILD_DIR": target},
            stdout=handle, stderr=subprocess.STDOUT, timeout=remaining(),
        )
    with open(log) as handle:
        text = handle.read()
    if build.returncode:
        fail(f"staging {source} exit {build.returncode}: {text[-2000:]}")
    if "sccache" in text:
        fail(f"staging {source} ran a rustc wrapper")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    shutil.copy2(os.path.join(target, "debug", "oxide"), out)
    units = [line.strip() for line in text.splitlines()
             if re.match(r"\s*(Compiling|Fresh) oxide-storage ", line)]
    print(f"pool-scale staged {out}: sha256 {sha256(out)}; {units}")
    if not any(unit.startswith("Compiling oxide-storage ") for unit in units):
        fail(f"staging {source} did not compile oxide-storage: {units}")
    return out


def identity(source: str, commit: str) -> dict:
    tree = subprocess.run(["git", "rev-parse", f"{commit}^{{tree}}"], cwd=REPO,
                          capture_output=True, text=True, timeout=30).stdout.strip()
    return {"commit": commit, "tree": tree,
            "Cargo.toml": sha256(os.path.join(source, "Cargo.toml")),
            "Cargo.lock": sha256(os.path.join(source, "Cargo.lock"))}


def write_fixture(root: str) -> None:
    for name, text in FIXTURE.items():
        path = os.path.join(root, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as handle:
            handle.write(text)


def fill(root: str, count: int, label: str) -> None:
    """`count` entries (files plus directories of 100) in a fresh subtree."""
    base = os.path.join(root, f"scale-{label}")
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


class Pool:
    def __init__(self, parent: str, name: str, base_env: dict):
        self.root = os.path.join(parent, name)
        self.env = {**base_env, "OXIDE_POOL": self.root}

    def call(self, oxide: str, cwd: str, args: list, extra: dict | None = None, timeout: float = 90):
        result = subprocess.run([oxide, *args], cwd=cwd, env={**self.env, **(extra or {})},
                                capture_output=True, text=True, timeout=min(timeout, remaining()))
        return result.returncode, result.stdout, result.stderr

    def ledger(self) -> dict:
        with open(os.path.join(self.root, "ledger.json")) as handle:
            return json.load(handle)

    def slot_dir(self, slot: str) -> str:
        # oxide#104: a new slot sits at its recorded `dir`.
        for record in self.ledger()["slots"]:
            if str(record["id"]) == slot:
                return os.path.join(self.root, "payload", "slots", record.get("dir") or slot)
        return os.path.join(self.root, "payload", "slots", slot)

    def generation(self, slot: str) -> tuple:
        for record in self.ledger()["slots"]:
            if str(record["id"]) == slot:
                return record["id"], record["incarnation"], record["state"]
        return None


def summary(stderr: str, label: str) -> tuple:
    match = re.search(r"oxide: \w+ profile, [\w+]+ strategy, slot (\d+) (warm|cold), ", stderr)
    if not match:
        fail(f"{label}: no summary line in {stderr[-2000:]!r}")
    return match.group(1), match.group(2)


def built(pool: Pool, oxide: str, cwd: str, label: str, extra: dict | None = None) -> tuple:
    code, _, stderr = pool.call(oxide, cwd, ["build"], extra)
    if code:
        fail(f"{label}: exit {code}: {stderr[-2000:]}")
    return summary(stderr, label)


def refused(pool: Pool, oxide: str, cwd: str, label: str) -> None:
    before = pool.ledger()
    code, _, stderr = pool.call(oxide, cwd, ["build"])
    if code == 0 or "unknown_accounting" not in stderr:
        fail(f"{label}: old bound did not refuse unknown_accounting: exit {code}, {stderr[-1200:]!r}")
    if pool.ledger() != before:
        fail(f"{label}: the refusal changed the ledger")
    print(f"pool-scale {label}: old bound refused, exit {code}: {stderr.strip().splitlines()[-1]}")


def walk_line(pool: Pool, oxide: str, cwd: str, label: str) -> str:
    code, stdout, stderr = pool.call(oxide, cwd, ["status"])
    if code:
        fail(f"{label} status exit {code}: {stderr[-1200:]}")
    line = next((line for line in stdout.splitlines() if line.startswith("  slot accounting walk: ")), None)
    if line is None or not line.startswith("  slot accounting walk: complete, "):
        fail(f"{label}: payload walk not complete: {line!r}\n{stdout}")
    if "accounting walk: partial" in stdout or "accounting walk: unknown" in stdout:
        fail(f"{label}: a slot walk is incomplete:\n{stdout}")
    print(f"pool-scale {label} status:{line[len('  slot accounting'):]}")
    return stdout


def checkout(parent: str, name: str) -> str:
    path = os.path.join(parent, "checkouts", name)
    write_fixture(path)
    return path


def prove_single(parent: str, env: dict, old: str, new: str) -> None:
    pool = Pool(parent, "pool-single", env)
    app = checkout(parent, "single")
    slot, _ = built(pool, new, app, "single first build")
    marker = os.path.join(pool.slot_dir(slot), "build", "scale-marker")
    with open(marker, "w") as handle:
        handle.write("same generation")
    fill(os.path.join(pool.slot_dir(slot), "build"), 60_000, "single")
    count = entries(pool.slot_dir(slot))
    if count <= OLD_BOUND + 10_000 - 1:
        fail(f"single slot has only {count} entries")
    generation = pool.generation(slot)
    inode = os.lstat(marker).st_ino
    refused(pool, old, app, "single slot")
    started = time.monotonic()
    again, warmth = built(pool, new, app, "single reuse")
    print(f"pool-scale single slot: {count} entries in slot {slot}; candidate reuse {warmth} "
          f"slot {again} in {time.monotonic() - started:.1f} s")
    if again != slot or warmth != "warm" or pool.generation(slot) != generation \
            or os.lstat(marker).st_ino != inode or len(pool.ledger()["slots"]) != 1:
        fail(f"single slot was not reused as the same generation: {again} {warmth} "
             f"{pool.generation(slot)} vs {generation}")
    walk_line(pool, new, app, "single")


def prove_aggregate(parent: str, env: dict, old: str, new: str) -> None:
    pool = Pool(parent, "pool-aggregate", env)
    first = checkout(parent, "aggregate-a")
    second = checkout(parent, "aggregate-b")
    third = checkout(parent, "aggregate-c")
    slots = []
    for label, app in (("a", first), ("b", second)):
        slot, _ = built(pool, new, app, f"aggregate {label} build")
        fill(os.path.join(pool.slot_dir(slot), "build"), 30_000, label)
        slots.append(slot)
    sizes = [entries(pool.slot_dir(slot)) for slot in slots]
    total = entries(os.path.join(pool.root, "payload", "slots"))
    if max(sizes) >= OLD_BOUND or total <= OLD_BOUND + 10_000 - 1:
        fail(f"aggregate shape wrong: slots {sizes}, total {total}")
    slot, warmth = built(pool, old, first, "aggregate old reuse")
    if slot != slots[0] or warmth != "warm":
        fail(f"old bound did not reuse a slot under it: {slot} {warmth}")
    refused(pool, old, third, "aggregate new slot")
    slot, warmth = built(pool, new, third, "aggregate new slot")
    if slot in slots or warmth != "cold" or len(pool.ledger()["slots"]) != 3:
        fail(f"candidate did not admit a new slot: {slot} {warmth}")
    print(f"pool-scale aggregate: slots {sizes} entries, payload {total}; candidate admitted new slot {slot}")
    walk_line(pool, new, third, "aggregate")


def allocation(root: str) -> tuple:
    inodes = {}
    by_name = 0
    for current, _, files in os.walk(root, followlinks=False):
        for name in files:
            stat = os.lstat(os.path.join(current, name))
            inodes[(stat.st_dev, stat.st_ino)] = stat.st_blocks * 512
            by_name += stat.st_blocks * 512
    return sum(inodes.values()), by_name


def prove_eviction(parent: str, env: dict, new: str) -> None:
    pool = Pool(parent, "pool-evict", env)
    idle = checkout(parent, "evict-idle")
    held = checkout(parent, "evict-held")
    fresh = checkout(parent, "evict-new")
    idle_slot, _ = built(pool, new, idle, "evict idle build")
    held_slot, _ = built(pool, new, held, "evict held build")
    fill(os.path.join(pool.slot_dir(idle_slot), "build"), 60_000, "idle")
    with open(os.path.join(pool.slot_dir(idle_slot), "build", "idle.bin"), "wb") as handle:
        handle.write(b"\x01" * (3 * MIB))
    linked = os.path.join(pool.slot_dir(held_slot), "build", "linked.bin")
    with open(linked, "wb") as handle:
        handle.write(b"\x02" * (4 * MIB))
    for index in range(3):
        os.link(linked, f"{linked}.{index}")
    pidfile = os.path.join(parent, "hold.pid")
    holder = subprocess.Popen([new, "test"], cwd=held, env={**pool.env, "OXIDE_FIXTURE_HOLD": pidfile},
                              stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    try:
        deadline = time.monotonic() + 90
        while not os.path.exists(pidfile):
            if holder.poll() is not None or time.monotonic() > deadline:
                fail(f"held test did not start: {holder.stderr.read()[-1200:] if holder.poll() is not None else 'timeout'}")
            time.sleep(0.1)
        if pool.generation(held_slot)[2] != "held":
            fail(f"slot {held_slot} is not held: {pool.generation(held_slot)}")
        held_unique, held_by_name = allocation(pool.slot_dir(held_slot))
        idle_unique, _ = allocation(pool.slot_dir(idle_slot))
        budget = (held_unique // MIB + 2) * MIB
        print(f"pool-scale eviction: held slot {held_slot} {held_unique} B once per inode ({held_by_name} per name), "
              f"idle slot {idle_slot} {idle_unique} B with {entries(pool.slot_dir(idle_slot))} entries, soft budget {budget}")
        if not (held_unique < budget <= held_by_name and held_unique + idle_unique >= budget):
            fail("eviction budget does not separate once-per-inode from per-name accounting")
        slot, warmth = built(pool, new, fresh, "eviction admission", {"OXIDE_POOL_CAP": str(budget)})
        if os.path.lexists(pool.slot_dir(idle_slot)) or pool.generation(idle_slot) is not None:
            fail("the idle slot was not evicted whole")
        if not os.path.exists(linked) or pool.generation(held_slot)[2] != "held":
            fail("the held slot was touched")
        print(f"pool-scale eviction: idle slot {idle_slot} evicted whole, held slot {held_slot} kept, new slot {slot} {warmth}")
    finally:
        with open(os.path.join(parent, "hold.release"), "w"):
            pass
        try:
            _, err = holder.communicate(timeout=min(60, remaining()))
        except subprocess.TimeoutExpired:
            holder.kill()
            holder.communicate()
            fail("held test did not finish after release")
    if holder.returncode:
        fail(f"held test exit {holder.returncode}: {err[-1200:]}")
    walk_line(pool, new, fresh, "eviction")


def main() -> int:
    if sys.platform != "darwin":
        fail("the pool-scale proof requires macOS")
    repo = REPO
    if subprocess.run(["git", "status", "--porcelain"], cwd=repo, capture_output=True,
                      text=True, timeout=30).stdout.strip():
        fail("the candidate checkout is not clean, so HEAD does not name its source")
    reserve("/private/tmp", "admission")
    made = subprocess.run(["mktemp", "-d", "/private/tmp/oxide-pool-scale.XXXXXX"],
                          capture_output=True, text=True, timeout=30)
    if made.returncode:
        fail("mktemp failed")
    parent = os.path.realpath(made.stdout.strip())
    os.chmod(parent, 0o700)
    try:
        base = subprocess.run(["git", "merge-base", "HEAD", "origin/main"], cwd=repo,
                              capture_output=True, text=True, timeout=30).stdout.strip()
        head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo,
                              capture_output=True, text=True, timeout=30).stdout.strip()
        old_src = os.path.join(parent, "base-src")
        os.mkdir(old_src)
        archive = subprocess.run(f"git archive {base} | tar -x -C {old_src}", shell=True, cwd=repo, timeout=60)
        with open(os.path.join(old_src, "src", "lease", "mod.rs")) as handle:
            if "ACCOUNTING_MAX_ENTRIES: u64 = 50_000;" not in handle.read() or archive.returncode:
                fail(f"base {base} does not carry the old 50,000-entry pool bound")
        stage_env = {name: value for name, value in os.environ.items()
                     if not name.startswith(("CARGO_BUILD_", "CARGO_TARGET_", "RUSTC", "SCCACHE"))
                     and name not in ("CARGO", "CARGO_INCREMENTAL", "RUSTUP_TOOLCHAIN")}
        stage_env.update({"RUSTUP_TOOLCHAIN": BUILDER, "RUSTC_WRAPPER": "", "CARGO_PROFILE_DEV_DEBUG": "0",
                          "CARGO_NET_OFFLINE": "true"})
        identities = {"reference": identity(old_src, base), "candidate": identity(repo, head)}
        print(f"pool-scale sources: {json.dumps(identities, sort_keys=True)}")
        old = stage(old_src, os.path.join(parent, "stage-reference"), os.path.join(parent, "bin-old", "oxide"),
                    os.path.join(parent, "stage-reference.log"), stage_env)
        new = stage(repo, os.path.join(parent, "stage-candidate"), os.path.join(parent, "bin-new", "oxide"),
                    os.path.join(parent, "stage-candidate.log"), stage_env)
        shutil.rmtree(old_src)
        with open(old, "rb") as handle:
            old_bytes = handle.read()
        with open(new, "rb") as handle:
            new_bytes = handle.read()
        if old_bytes == new_bytes or DIAGNOSTIC in old_bytes or DIAGNOSTIC not in new_bytes:
            fail("staged binaries are not a distinct reference and candidate carrying the new diagnostic")
        print(f"pool-scale staging: reference {base}, candidate {head}, binaries differ, "
              f"candidate diagnostic present, {time.monotonic() - STARTED:.0f} s")
        owned(parent, "after staging")
        reserve(parent, "after staging")

        home = os.path.join(parent, "home")
        cargo_home = os.path.join(parent, "cargo-home")
        for path in (home, cargo_home, os.path.join(parent, "tmp")):
            os.makedirs(path, mode=0o700)
        env = {name: value for name, value in stage_env.items()
               if not name.startswith(("CARGO_", "OXIDE_")) and name not in ("RUSTUP_TOOLCHAIN", "RUSTC_WRAPPER")}
        env.update({"OXIDE_TEST_HOME_FROM_ENV": "1", "HOME": home, "CARGO_HOME": cargo_home, "RUSTUP_HOME": os.environ.get("RUSTUP_HOME")
                    or os.path.join(os.path.expanduser("~"), ".rustup"), "CARGO_NET_OFFLINE": "true",
                    "CARGO_TERM_COLOR": "never", "TMPDIR": os.path.join(parent, "tmp")})
        prove_single(parent, env, old, new)
        owned(parent, "after single")
        reserve(parent, "after single")
        prove_aggregate(parent, env, old, new)
        owned(parent, "after aggregate")
        reserve(parent, "after aggregate")
        prove_eviction(parent, env, new)
        used = owned(parent, "after eviction")
        reserve(parent, "after eviction")
    except Exception:
        print(f"pool-scale proof retained its owned scratch at {parent}", file=sys.stderr)
        raise
    shutil.rmtree(parent)
    elapsed = time.monotonic() - STARTED
    if elapsed > PROOF_SECONDS:
        fail(f"pool-scale proof took {elapsed:.0f} s, over {PROOF_SECONDS} s")
    print(f"oxide pool-scale proof held: {elapsed:.0f} s including cleanup, peak owned scratch checked {used // MIB} MiB, scratch removed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
