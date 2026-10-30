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
  complete accounting; it also measures the checkout's `target` and the
  Cargo home, which are not pool roots and are excluded from the pool walk
  oracle;
- `oxide new` readiness walks nothing and makes no scope pass: since
  oxide#182 it reads no pool state at all ("use not measured");
- ordinary reuse walks exactly its target slot at admission and again at
  release, and prints the provenance label "slot N was X MiB when leased";
- a live holder: `pool recover-interrupted` refuses with no walk, and a build
  that must wait for that holder's slot (one host-wide slot, so it cannot
  take a second) waits with no walk or scope pass, then takes the released
  slot warm (one admission walk and one release walk);
- a killed holder: `pool recover-interrupted` refuses again, naming automatic
  recovery, with no walk, and the next build — started only once the
  fixture's own Cargo group is positively empty — recovers the slot, walks it
  as it re-leases it, has the freshness gate refuse it, evicts the untrusted
  slot through the oxide#190 holding deletion (its release measurement and
  the eviction's own accounting walk that slot again), and builds a new slot
  cold, whose single release walk is the last walk; nothing walks the payload
  or the idle slot beside it;
- a final default `status` after recovery still walks nothing.

Retired rather than duplicated: the byte-pressure eviction case and its
hard-link control now live in `rust:recorded-slot-accounting` (case D drives
byte pressure and evicts one owned idle whole slot; case E proves a cross-slot
hard link is counted once) and the holding deletion under it in
`rust:holding-delete`. This script keeps the routing facts those proofs do
not cover: readiness, held and killed-holder refusals, the waiting build and
the recovered cold build.

The walk counts show routing only, not a resource gate. The scratch is one
fresh directory inside the qualified supervisor's `OXIDE_TASK_SCRATCH_ROOT`
(the selector passes `--root-parent /private/tmp`, the owner-accepted
internal-placement exception `bd357a4c`, because the fixture's pool is a
genuine internal `directory-budget` pool that the product refuses on an
external volume), so the supervisor's continuous reserve watchdog, scratch
budget and closure census cover the actual work. The script stops 30 s before
the guard's exported deadline, and its own phase samples are observations,
not gates. Bounds: 300 s and 512 MiB monitored scratch under
`scripts/bounded-test.py`.

`--diagnostic` is the uncounted keep-going collection: it records reached,
completed, failed, dependent-skipped and unexecuted cases and the latched
first fault in a bounded `qualified: false` receipt beside the guard's,
continues only into dependency-independent cases, and aborts on an outcome it
cannot classify (an unclassified child, writer, mount, resource or deadline
outcome). It is never a counted PASS. `--plan` prints the case table, the
allocation and the nine premise dispositions without any effect.
`--self-check` is the pure apparatus check: no child, pool, image or scratch.
Not a benchmark and not full QA. Scratch is removed only after positive
completion.
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
SHUTDOWN_MARGIN_S = 30
MIB = 1024 * 1024
BIG_ENTRIES = 200_000
STARTED = time.monotonic()
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PEAK = {"bytes": 0, "phase": "none"}
# The product's own readiness line since oxide#182: it reads no pool state.
READINESS_NEEDLE = "use not measured (readiness reads no pool state)"
# One fixed compiler-flag value for staging and every fixture child (premise 7).
FROZEN_RUSTFLAGS = ""
# The uncounted collection's one bounded receipt.
DIAGNOSTIC_RECEIPT = "front-door.diagnostic"
DIAGNOSTIC_RECEIPT_BYTES = 16 * 1024
DIAGNOSTIC_RECEIPT_EXCERPT = 600
USAGE = ("usage: test-front-door-accounting.py "
         "[--diagnostic | --plan | --self-check]")

# The nine-item proof-authoring premise checklist, traced through the actual
# child environments below and bound by `--self-check`. `--plan` prints it.
PREMISE_CHECKLIST = (
    (
        "private HOME and rustup",
        "applicable: `staging_environment` and `fixture_environment` capture the invoking "
        "RUSTUP_HOME (or HOME/.rustup) before the private HOME exists, pass it to staging and every "
        "child with RUSTUP_TOOLCHAIN=1.97.1 and RUSTUP_AUTO_INSTALL=0, and keep HOME/CARGO_HOME "
        "private; staging is the only --locked call, against this repository's committed lock, and "
        "the dependency-free fixtures make their own lock offline",
    ),
    (
        "account-home control",
        "applicable: `fixture_environment` sets OXIDE_TEST_HOME_FROM_ENV=1 beside the private HOME",
    ),
    (
        "environment isolation",
        "applicable: both environment builders drop every OXIDE_/XDG_/CARGO_/SCCACHE_/RUSTC* key and "
        "CARGO, CARGO_INCREMENTAL, RUSTUP_TOOLCHAIN, RUSTFLAGS, CARGO_ENCODED_RUSTFLAGS before setting "
        "only the fixture's own controls",
    ),
    (
        "automatic adoption",
        "not applicable: no permanent-drive mode, no Git repository in any fixture and no outputs "
        "conversion; every checkout keeps its target local",
    ),
    ("Cargo external argv", "not applicable: no external Cargo subcommand runs in this proof"),
    ("owned images", "not applicable: no disk image, mount or removable media"),
    (
        "compiler flags",
        "applicable: RUSTFLAGS is the fixed FROZEN_RUSTFLAGS and CARGO_ENCODED_RUSTFLAGS is removed for "
        "staging and every child; the staged binary must carry its walk observer",
    ),
    (
        "temporary storage",
        "applicable: the scratch is one fresh directory inside the qualified supervisor's "
        "OXIDE_TASK_SCRATCH_ROOT under --root-parent /private/tmp (the owner-accepted "
        "internal-placement exception bd357a4c), and TMPDIR/TMP/TEMP name one owned directory inside it",
    ),
    (
        "macOS mktemp",
        "applicable: tempfile.mkdtemp with a template inside OXIDE_TASK_SCRATCH_ROOT; the executable "
        "fixture inherits that TMPDIR",
    ),
)

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


class OracleFailure(RuntimeError):
    """A product expectation that did not hold: recoverable in a diagnostic."""


class UnknownOutcome(RuntimeError):
    """An outcome this proof cannot classify: never continued past."""


DEADLINE = {"at": STARTED + PROOF_SECONDS}


def fail(message: str) -> None:
    """A product expectation that did not hold: recoverable in a diagnostic."""
    raise OracleFailure(message)


def need(message: str) -> None:
    """An outcome this proof cannot classify: never continued past."""
    raise UnknownOutcome(message)


def work_deadline() -> float:
    """The guard's exported deadline less its cleanup margin, when present."""
    guard = os.environ.get("OXIDE_PROOF_DEADLINE_NS")
    if guard:
        try:
            left = int(guard) / 1e9 - time.monotonic() - SHUTDOWN_MARGIN_S
        except ValueError:
            need(f"OXIDE_PROOF_DEADLINE_NS {guard!r} is not an integer")
        DEADLINE["at"] = min(DEADLINE["at"], time.monotonic() + max(1.0, left))
    return DEADLINE["at"]


def remaining() -> float:
    left = DEADLINE["at"] - time.monotonic()
    if left <= 0:
        need(f"proof exceeded its {PROOF_SECONDS} s guard window")
    return left


def say(message: str) -> None:
    print(f"front-door {time.monotonic() - STARTED:6.1f}s {message}", flush=True)


def reserve(path: str, phase: str) -> None:
    stat = os.statvfs(path)
    free = stat.f_bavail * stat.f_frsize
    say(f"reserve {phase}: host free {free // MIB} MiB")
    if free < HOST_RESERVE_BYTES:
        need(f"host free {free} is below the 20 GiB reserve at {phase}")


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
        need(f"owned scratch {used} exceeds 512 MiB at {phase}")
    return used


def sha256(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def invoking_rustup() -> str:
    """The invoking toolchain home, resolved before any private HOME exists."""
    return os.environ.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")


def staging_environment() -> dict:
    """Staging uses the invoking Cargo home (this repository's committed lock,
    offline) with the fixed compiler flags and no wrapper."""
    env = {
        name: value
        for name, value in os.environ.items()
        if not name.startswith(("CARGO_BUILD_", "CARGO_TARGET_", "OXIDE_", "XDG_", "RUSTC"))
        and name
        not in (
            "CARGO",
            "CARGO_INCREMENTAL",
            "RUSTUP_TOOLCHAIN",
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
        )
    }
    env.update(
        {
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTFLAGS": FROZEN_RUSTFLAGS,
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_NET_OFFLINE": "true",
        }
    )
    return env


def fixture_environment(parent: str) -> dict:
    """The one environment every fixture child gets: a private HOME, CARGO_HOME
    and TMPDIR/TMP/TEMP inside this run's scratch, the invoking rustup home,
    the pinned toolchain, the fixed compiler flags and only this proof's own
    controls. Every inherited OXIDE_/XDG_/CARGO_/SCCACHE_/RUSTC* key and every
    encoded compiler-flag value is dropped first."""
    home = os.path.join(parent, "home")
    cargo_home = os.path.join(parent, "cargo-home")
    tmp = os.path.join(parent, "tmp")
    for path in (home, cargo_home, tmp, os.path.join(parent, "walks")):
        os.makedirs(path, mode=0o700, exist_ok=True)
    env = {
        name: value
        for name, value in os.environ.items()
        if not name.startswith(("OXIDE_", "XDG_", "CARGO_", "SCCACHE_", "RUSTC"))
        and name
        not in (
            "CARGO",
            "CARGO_INCREMENTAL",
            "RUSTUP_TOOLCHAIN",
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
        )
    }
    env.update(
        {
            "OXIDE_TEST_HOME_FROM_ENV": "1",
            "HOME": home,
            "CARGO_HOME": cargo_home,
            "RUSTUP_HOME": invoking_rustup(),
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTFLAGS": FROZEN_RUSTFLAGS,
            "CARGO_NET_OFFLINE": "true",
            "CARGO_TERM_COLOR": "never",
            "TMPDIR": tmp,
            "TMP": tmp,
            "TEMP": tmp,
        }
    )
    return env


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
        need(f"staging exit {build.returncode}: {text[-2000:]}")
    out = os.path.join(parent, "bin", "oxide")
    os.makedirs(os.path.dirname(out))
    shutil.copy2(os.path.join(target, "debug", "oxide"), out)
    with open(out, "rb") as handle:
        if b"OXIDE_TEST_WALK_LOG" not in handle.read():
            need("the staged debug binary has no walk observer")
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


def pool_roots(walks: list) -> list:
    """The pool's own walk roots from a command's log, in order. A measurement
    also walks the checkout's `target` and the Cargo home, which are absolute
    roots outside the payload and are never pool routing."""
    return [root for root in walks if not root.startswith("/")]


class Proof:
    def __init__(self, parent: str, oxide: str, env: dict):
        self.parent = parent
        self.oxide = oxide
        self.pool = os.path.join(parent, "pool")
        self.env = {**env, "OXIDE_POOL": self.pool, "OXIDE_POOL_CREATE": "1"}
        self.logs = 0
        self.dirs = {}
        self.last_log = ""

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
        self.last_log = log
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


def build(proof: Proof, cwd: str, label: str, expected_walks: list | None,
          expected_scopes: list | None = None, extra: dict | None = None) -> tuple:
    code, _, stderr, walks, scopes, elapsed = proof.call(cwd, ["build"], extra)
    if code:
        fail(f"{label}: exit {code}: {stderr[-2000:]}")
    slot, warmth, tail = summary(stderr, label)
    say(f"{label}: slot {slot} {warmth} in {elapsed:.1f} s, walks {walks}, scopes {scopes}, "
        f"summary tail: {tail}")
    if expected_walks is not None:
        expect_roots(label, "walks", walks, [walk.format(slot=slot) for walk in expected_walks])
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
    expect_roots(label, "pool walks", pool_roots(walks), ["payload", *[f"slots/{slot}" for slot in slots]])
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


def holder_group(proof: Proof, checkout: str) -> int:
    """The recorded Cargo group of this checkout's one active record, read from
    the pool control state (never guessed from a pid)."""
    path = os.path.join(proof.pool, "oxide-managed", "state.json")
    with open(path) as handle:
        state = json.load(handle)
    for record in state.get("records", []):
        if record.get("checkout") == checkout and record.get("process_group"):
            return int(record["process_group"])
    fail(f"no active record with a Cargo group for {checkout}")


def await_group_empty(group: int, label: str, seconds: float = 60) -> None:
    """Positive observation that the fixture's own Cargo group has no member
    left: `killpg(group, 0)` delivers no signal and fails only when the group
    is gone. The product makes the same closure decision before it recovers."""
    deadline = time.monotonic() + min(seconds, remaining())
    while True:
        try:
            os.killpg(group, 0)
        except ProcessLookupError:
            say(f"{label}: Cargo group {group} is empty")
            return
        except PermissionError:
            pass
        if time.monotonic() > deadline:
            fail(f"{label}: Cargo group {group} still has a member")
        time.sleep(0.1)


def prove_recovery(proof: Proof, app: str, app_slot: str) -> str:
    """A live holder, the wait it forces, then a killed holder's recovery."""
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
        say(f"live holder: recover-interrupted exit {code}, walks {walks}, scopes {scopes}: {stderr.strip()[-200:]}".replace("\n", " "))
        if code == 0 or "a live oxide command holds slot" not in stderr or walks or scopes:
            fail("recover-interrupted did not refuse a live holder without walking")
        # A command that must wait for the holder's slot polls without walking
        # (the #110 defect). One host-wide slot keeps the live holder's slot
        # from being duplicated by a second slot, so the waiter really waits.
        waiter_path = os.path.join(proof.parent, "waiter.err")
        waiter_err = open(waiter_path, "w")
        waiter, waiter_log = proof.start(app, ["build"], {"OXIDE_MAX_ACTIVE": "1"}, stderr=waiter_err)
        wait_text(waiter_path, "waiting for a build slot", waiter, "waiter")
        time.sleep(1.0)
        waited = proof.walks(waiter_log)
        waited_scopes = proof.scopes(waiter_log)
        say(f"waiting build: walks while waiting {waited}, scopes {waited_scopes}")
        if waited or waited_scopes:
            fail(f"a waiting build walked {waited} or scoped {waited_scopes}")
        # The holder finishes cleanly; the waiter then takes its released slot
        # warm: one walk at admission and one at release, nothing else.
        with open(release, "w"):
            pass
        code = waiter.wait(timeout=min(120, remaining()))
        waiter_err.close()
        with open(waiter_path) as handle:
            stderr = handle.read()
        slot, warmth, tail = summary(stderr, "woken build")
        say(f"woken build: exit {code}, slot {slot} {warmth}, walks {proof.walks(waiter_log)}, summary tail: {tail}")
        if code or slot != app_slot or warmth != "warm":
            fail(f"the woken build did not take slot {app_slot} warm: {code} {stderr[-2000:]}")
        expect_roots("woken build", "walks", proof.walks(waiter_log),
                     [f"slots/{app_slot}", f"slots/{app_slot}"])
        if holder.wait(timeout=min(60, remaining())):
            fail(f"the clean holder exited {holder.returncode}")
        expect_roots("holder", "walks", proof.walks(holder_log),
                     [f"slots/{app_slot}", f"slots/{app_slot}"])
        holder_err.close()
    finally:
        with open(release, "w"):
            pass
        holder_err.close()
        for process in (holder, waiter):
            if process is not None and process.poll() is None:
                process.kill()
                process.wait()

    # A killed holder: the refused explicit recovery, then the next command's
    # own recovery, eviction and cold build.
    killed_pid = os.path.join(proof.parent, "killed.pid")
    killed_release = os.path.join(proof.parent, "killed.release")
    killed_err = open(os.path.join(proof.parent, "killed.err"), "w")
    killed, killed_log = proof.start(app, ["test"], {"OXIDE_FIXTURE_HOLD": killed_pid}, stderr=killed_err)
    try:
        wait_for(killed_pid, killed, "killed holder start")
        held = proof.slots()[app_slot]
        if held["state"] != "held":
            fail(f"slot {app_slot} is not held by the killed holder: {held}")
        group = holder_group(proof, app)
        killed.send_signal(signal.SIGKILL)
        killed.wait(timeout=30)
        with open(killed_release, "w"):
            pass
        # The product only recovers on a positively empty Cargo group; observe
        # that the fixture's own group is empty before the next command.
        await_group_empty(group, "killed holder")
        code, _, stderr, walks, scopes, _ = proof.call(proof.parent, ["pool", "recover-interrupted", app_slot])
        say(f"killed holder: recover-interrupted exit {code}, walks {walks}, scopes {scopes}: {stderr.strip()[-200:]}".replace("\n", " "))
        if code == 0 or "recovered automatically by the next command" not in stderr or walks or scopes:
            fail("recover-interrupted did not defer to automatic recovery without walking")
        if proof.slots()[app_slot] != held:
            fail("a refused recovery changed the ledger")
        # One walk re-leases the recovered slot, which the freshness gate
        # refuses; the new cold slot's release is the last walk. The untrusted
        # slot is walked more than once by design (the re-lease, then its
        # release measurement inside the discard, then the eviction's own
        # accounting), so its count is observed and printed, never gated. What
        # is gated is routing: no walk of the payload, the idle slot beside it,
        # or anything else, and exactly one walk of the new slot, last.
        new_slot, warmth, tail, stderr = build(proof, app, "recovered build", None)
        recovered, fresh = f"slots/{app_slot}", f"slots/{new_slot}"
        walks = proof.walks(proof.last_log)
        if set(walks) != {recovered, fresh} or walks[-1] != fresh or walks.count(fresh) != 1:
            fail(f"recovered build walks {walks}, expected only {recovered} and one {fresh} last")
        say(f"recovered build walks the untrusted slot {walks.count(recovered)} times, "
            f"then its new slot once")
        if not re.search(rf"recovered slot {app_slot} after oxide pid {killed.pid} died \(Cargo group \d+ empty\)", stderr):
            fail(f"the build did not recover slot {app_slot}: {stderr[-2000:]}")
        if f"cannot vouch for slot {app_slot}" not in stderr or warmth != "cold" or not tail.startswith("new slot, "):
            fail(f"the untrusted recovered slot was not evicted for a cold build: {stderr[-2000:]}")
        if app_slot in proof.slots() or new_slot in ("", app_slot):
            fail(f"recovered build ledger {sorted(proof.slots())}, slot {new_slot}")
        expect_roots("killed holder", "walks", proof.walks(killed_log), [f"slots/{app_slot}"])
        return new_slot
    finally:
        with open(killed_release, "w"):
            pass
        killed_err.close()
        if killed.poll() is None:
            killed.kill()
            killed.wait()


# --------------------------------------------------------------------------
# The fixed case map and its driver. Each case is one named premise; a case
# takes `(proof, checkouts, state)` and shares only what an earlier case
# established. The counted run is fail-fast; the diagnostic records a
# recoverable oracle failure and continues only into dependency-independent
# cases, and aborts on any outcome it cannot classify.


def case_slots(proof: Proof, checkouts: dict, state: dict) -> None:
    app_slot, big_slot = prove_slots(proof, checkouts["app"], checkouts["big"])
    state.update(app_slot=app_slot, big_slot=big_slot)
    owned(proof.parent, "after two new slots")
    reserve(proof.parent, "after two new slots")


def case_status(proof: Proof, checkouts: dict, state: dict) -> None:
    recorded_status(proof, checkouts["app"], "after two new slots")
    status(proof, checkouts["app"], "after two new slots", [state["app_slot"], state["big_slot"]])


def case_readiness(proof: Proof, checkouts: dict, state: dict) -> None:
    prove_readiness(proof)
    owned(proof.parent, "after readiness")
    reserve(proof.parent, "after readiness")


def case_reuse(proof: Proof, checkouts: dict, state: dict) -> None:
    prove_reuse(proof, checkouts["app"], state["app_slot"], state["big_slot"])
    owned(proof.parent, "after reuse")
    reserve(proof.parent, "after reuse")


def case_recovery(proof: Proof, checkouts: dict, state: dict) -> None:
    kept = prove_recovery(proof, checkouts["app"], state["app_slot"])
    if kept not in proof.slots() or kept == state["big_slot"]:
        fail(f"the recovered cold slot {kept} is not a new slot in {sorted(proof.slots())}")
    state["kept_slot"] = kept
    owned(proof.parent, "after recovery")
    reserve(proof.parent, "after recovery")
    recorded_status(proof, checkouts["app"], "after recovery")
    status(proof, checkouts["app"], "after recovery", list(proof.slots()))


CASES = (
    ("slots", (), "two new slots: no admission walk, one payload scope pass, a release walk"),
    ("status", ("slots",), "recorded status walks nothing; --measure walks the payload then every slot"),
    ("readiness", ("slots",), "oxide new readiness walks nothing and reads no pool state"),
    ("reuse", ("slots",), "the 200,000-entry idle slot, then ordinary reuse walks its target twice"),
    ("recovery", ("reuse",), "a live holder refusal and its woken build, then the killed holder's recovery"),
)

BODIES = {
    "slots": case_slots,
    "status": case_status,
    "readiness": case_readiness,
    "reuse": case_reuse,
    "recovery": case_recovery,
}


def diagnostic_outcome() -> dict:
    return {
        "version": "oxide.front-door-accounting-diagnostic/1",
        "mode": "diagnostic",
        "qualified": False,
        "claims": "diagnostic case coverage only; never a counted native PASS, "
                  "closure, census or resource claim",
        "cases": [case for case, _, _ in CASES],
        "purposes": {case: purpose for case, _, purpose in CASES},
        "dependencies": {case: list(deps) for case, deps, _ in CASES},
        "source": None,
        "root": None,
        "limits": {
            "seconds": PROOF_SECONDS,
            "shutdown_margin_s": SHUTDOWN_MARGIN_S,
            "scratch_mib": SCRATCH_LIMIT_BYTES // MIB,
            "fixture_entries": BIG_ENTRIES,
        },
        "reached": [],
        "completed": [],
        "failed": [],
        "skipped": [],
        "unexecuted": [],
        "first_fault": None,
        "aborted": None,
    }


def run_cases(diagnostic: bool, proof: Proof, checkouts: dict, state: dict, outcome: dict,
              bodies: dict | None = None) -> dict:
    """Run the fixed case map; return the same `outcome` document."""
    bodies = BODIES if bodies is None else bodies
    for case, deps, _ in CASES:
        missing = [dep for dep in deps if dep not in outcome["completed"]]
        if diagnostic and missing:
            outcome["skipped"].append([case, missing])
            print(f"front-door {case}: skipped (dependency {', '.join(missing)} did not complete)",
                  flush=True)
            continue
        outcome["reached"].append(case)
        try:
            bodies[case](proof, checkouts, state)
        except OracleFailure as failure:
            outcome["failed"].append([case, str(failure)[:DIAGNOSTIC_RECEIPT_EXCERPT]])
            outcome["first_fault"] = outcome["first_fault"] or f"{case}: {failure}"
            print(f"front-door {case}: FAILED {failure}", file=sys.stderr, flush=True)
            if not diagnostic:
                raise
            continue
        except UnknownOutcome as failure:
            outcome["aborted"] = f"{case}: {failure}"
            outcome["unexecuted"] = [name for name, _, _ in CASES if name not in outcome["reached"]]
            print(f"front-door {case}: ABORTED {failure}", file=sys.stderr, flush=True)
            raise
        outcome["completed"].append(case)
        print(f"front-door {case}: completed", flush=True)
    outcome["unexecuted"] = [case for case, _, _ in CASES if case not in outcome["reached"]]
    return outcome


def diagnostic_receipt(outcome: dict) -> str:
    """Write ONE bounded, uniquely named, explicitly unqualified receipt."""
    recorded = {
        **outcome,
        "failed": [[case, message[:DIAGNOSTIC_RECEIPT_EXCERPT]] for case, message in outcome["failed"][:8]],
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
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
    if not os.path.isdir(evidence):
        raise RuntimeError(f"no evidence directory for the diagnostic receipt: {evidence!r}")
    name = f"{DIAGNOSTIC_RECEIPT}-{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}-{os.getpid()}.json"
    path = os.path.join(evidence, name)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(body)
    return path


def plan() -> int:
    """Print the fixed case table, allocation and premise dispositions. No effect."""
    print("front-door accounting proof plan (effect-free)")
    print(f"candidate: this HEAD's debug oxide, staged offline with {BUILDER} from {REPO}")
    print("selector: python3 scripts/bounded-test.py --root-parent /private/tmp "
          "--limit 300 --scratch-budget-mib 512 -- python3 scripts/test-front-door-accounting.py")
    print("allocation: one internal directory-budget pool with two slots, one idle slot of more than "
          f"{BIG_ENTRIES:,} entries and one 8 MiB hard-linked file, two fixture checkouts and one "
          "`oxide new` crate, all inside one owned scratch inside OXIDE_TASK_SCRATCH_ROOT "
          "(measured staging peak 283 MiB of the 512 MiB monitored budget)")
    print(f"limits: {PROOF_SECONDS} s wall including cleanup, stop {SHUTDOWN_MARGIN_S} s before the guard "
          f"deadline, {SCRATCH_LIMIT_BYTES // MIB} MiB monitored scratch, one Cargo call at a time")
    print("cases:")
    for case, deps, purpose in CASES:
        print(f"  {case}: {purpose} (after {', '.join(deps) or 'nothing'})")
    print("premise checklist:")
    for item, disposition in PREMISE_CHECKLIST:
        print(f"  {item}: {disposition}")
    print("modes: --diagnostic (uncounted keep-going collection), --plan (this), "
          "--self-check (pure apparatus); the counted run takes no argument")
    return 0


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
        # A measurement also walks the checkout target and the Cargo home; the
        # pool oracle excludes those absolute roots and keeps the payload-relative
        # ones in order.
        expect_roots(
            "self-check",
            "pool walks",
            pool_roots(["payload", "/fixture/checkout/target", "slots/1", "/fixture/cargo-home"]),
            ["payload", "slots/1"],
        )
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
        # The premise checklist is complete, and its environment claims are
        # checked against the builders with hostile inherited values in place:
        # the private HOME and toolchain, the account-home control, the fixed
        # compiler flags, the owned temporary storage and the isolation scrub.
        expect_true(len(PREMISE_CHECKLIST) == 9, f"premise checklist has {len(PREMISE_CHECKLIST)} items")
        names = [item for item, _ in PREMISE_CHECKLIST]
        expect_true(len(set(names)) == 9, f"duplicate premise names {names}")
        for item, disposition in PREMISE_CHECKLIST:
            expect_true(disposition.strip() != "", f"premise {item} has no disposition")
        holder = dict(os.environ)
        try:
            os.environ.update(
                {
                    "OXIDE_POOL": "/fixture/host-pool",
                    "OXIDE_CONFIG": "/fixture/host-config",
                    "OXIDE_TEST_HOME_FROM_ENV": "0",
                    "XDG_CONFIG_HOME": "/fixture/xdg",
                    "RUSTFLAGS": "-Ctarget-cpu=native",
                    "CARGO_ENCODED_RUSTFLAGS": "-Ctarget-cpu=native",
                    "RUSTUP_TOOLCHAIN": "nightly",
                    "SCCACHE_DIR": "/fixture/sccache",
                }
            )
            fixture_root = os.path.join(root, "fixture")
            env = fixture_environment(fixture_root)
            expect_true(env["OXIDE_TEST_HOME_FROM_ENV"] == "1", "account-home control was not set")
            expect_true(env["HOME"] == os.path.join(fixture_root, "home"), "HOME is not private")
            expect_true(env["CARGO_HOME"] == os.path.join(fixture_root, "cargo-home"),
                        "CARGO_HOME is not private")
            expect_true(env["RUSTUP_HOME"] == invoking_rustup(), "the invoking rustup home was not kept")
            expect_true(env["RUSTUP_TOOLCHAIN"] == BUILDER, "the toolchain is not pinned")
            expect_true(env["RUSTUP_AUTO_INSTALL"] == "0", "auto-install is not disabled")
            expect_true(env["RUSTFLAGS"] == FROZEN_RUSTFLAGS, f"flags not frozen: {env['RUSTFLAGS']!r}")
            expect_true("CARGO_ENCODED_RUSTFLAGS" not in env, "encoded flags were inherited")
            for name in ("TMPDIR", "TMP", "TEMP"):
                expect_true(env[name] == os.path.join(fixture_root, "tmp"),
                            f"{name} is not the owned temporary directory")
            for name in ("OXIDE_POOL", "OXIDE_CONFIG", "XDG_CONFIG_HOME", "SCCACHE_DIR"):
                expect_true(name not in env, f"inherited {name} was copied into the fixture")
            staging = staging_environment()
            expect_true(staging["RUSTFLAGS"] == FROZEN_RUSTFLAGS
                        and "CARGO_ENCODED_RUSTFLAGS" not in staging, "staging flags are not frozen")
            expect_true(staging["RUSTUP_TOOLCHAIN"] == BUILDER
                        and staging["RUSTUP_AUTO_INSTALL"] == "0", "staging toolchain is not pinned")
            expect_true("OXIDE_POOL" not in staging and "XDG_CONFIG_HOME" not in staging,
                        "staging environment is not isolated")
        finally:
            os.environ.clear()
            os.environ.update(holder)
        # The case map: every case has a body and a purpose, and every
        # dependency names an earlier case, so the diagnostic can skip only
        # dependents of what did not complete.
        expect_true(len(CASES) == 5, f"case table {CASES}")
        seen = []
        for case, deps, purpose in CASES:
            expect_true(purpose.strip() != "", f"case {case} has no purpose")
            expect_true(case not in seen, f"duplicate case {case}")
            for dep in deps:
                expect_true(dep in seen, f"case {case} depends on {dep}, which is not earlier")
            expect_true(case in BODIES, f"case {case} has no body")
            seen.append(case)
        expect_true(set(BODIES) == set(seen), f"bodies {sorted(BODIES)} vs cases {seen}")

        calls = []

        def stub_ok(proof, checkouts, state):
            calls.append("ok")

        def stub_fail(proof, checkouts, state):
            calls.append("fail")
            fail("stub failure")

        def stub_abort(proof, checkouts, state):
            calls.append("abort")
            need("stub unknown outcome")

        def stubs(**overrides) -> dict:
            bodies = {case: stub_ok for case, _, _ in CASES}
            bodies.update(overrides)
            return bodies

        # A recoverable failure in the first case skips its dependents and runs
        # nothing that a missing premise could satisfy.
        outcome = diagnostic_outcome()
        run_cases(True, None, {}, {}, outcome, stubs(slots=stub_fail))
        expect_true(outcome["completed"] == [], f"a failed case was completed: {outcome['completed']}")
        expect_true([case for case, _ in outcome["failed"]] == ["slots"], f"failed {outcome['failed']}")
        expect_true(
            outcome["skipped"] == [["status", ["slots"]], ["readiness", ["slots"]],
                                   ["reuse", ["slots"]], ["recovery", ["reuse"]]],
            f"skipped {outcome['skipped']}",
        )
        expect_true(calls == ["fail"], f"calls {calls}")
        # An independent case still runs after another case's recoverable
        # failure, and the first fault is latched.
        calls.clear()
        outcome = diagnostic_outcome()
        run_cases(True, None, {}, {}, outcome, stubs(status=stub_fail))
        expect_true(outcome["completed"] == ["slots", "readiness", "reuse", "recovery"],
                    f"completed {outcome['completed']}")
        expect_true([case for case, _ in outcome["failed"]] == ["status"], f"failed {outcome['failed']}")
        expect_true(outcome["first_fault"] == "status: stub failure", f"first fault {outcome['first_fault']}")
        # An unknown outcome aborts and leaves the rest unexecuted.
        outcome = diagnostic_outcome()
        try:
            run_cases(True, None, {}, {}, outcome, stubs(status=stub_abort))
        except UnknownOutcome:
            pass
        else:
            fail("an unknown outcome did not abort the collection")
        expect_true(outcome["aborted"] == "status: stub unknown outcome", f"aborted {outcome['aborted']}")
        expect_true(outcome["unexecuted"] == ["readiness", "reuse", "recovery"],
                    f"unexecuted {outcome['unexecuted']}")
        # The receipt is bounded, unique, explicitly unqualified and refused
        # without an evidence directory.
        holder = dict(os.environ)
        os.environ["OXIDE_PROOF_EVIDENCE"] = root
        try:
            outcome = diagnostic_outcome()
            outcome["failed"] = [["slots", "x" * 4000]]
            path = diagnostic_receipt(outcome)
            with open(path, "rb") as handle:
                body = handle.read()
            receipt = json.loads(body)
            expect_true(receipt["qualified"] is False and receipt["mode"] == "diagnostic",
                        f"receipt {receipt.get('mode')}/{receipt.get('qualified')}")
            expect_true(len(body) <= DIAGNOSTIC_RECEIPT_BYTES, f"receipt {len(body)} bytes")
            path = diagnostic_receipt({**diagnostic_outcome(), "huge": "z" * DIAGNOSTIC_RECEIPT_BYTES})
            with open(path, "rb") as handle:
                body = handle.read()
            expect_true(json.loads(body).get("incomplete") is True
                        and len(body) <= DIAGNOSTIC_RECEIPT_BYTES, f"oversized receipt {len(body)} bytes")
            os.environ["OXIDE_PROOF_EVIDENCE"] = os.path.join(root, "missing-evidence")
            try:
                diagnostic_receipt(diagnostic_outcome())
            except RuntimeError as error:
                expect_true("no evidence directory" in str(error), str(error))
            else:
                fail("the diagnostic receipt was written without an evidence directory")
        finally:
            os.environ.clear()
            os.environ.update(holder)
    finally:
        shutil.rmtree(root, ignore_errors=True)
    expect_true(mode_from_args([]) == "run", "no-argument mode")
    expect_true(mode_from_args(["--diagnostic"]) == "diagnostic", "diagnostic mode")
    expect_true(mode_from_args(["--plan"]) == "plan", "plan mode")
    expect_true(mode_from_args(["--self-check"]) == "self-check", "self-check mode")
    for refused in (["--bogus"], ["--diagnostic", "--plan"], ["--self-check", "--self-check"], ["run"]):
        expect_true(mode_from_args(refused) is None, f"{refused} selected a mode")
    print("front-door self-check ok: walk/scope root mapping, exact ordered walk oracles, "
          "summary and provenance parsing, measure line, readiness needle, "
          "premise checklist and fixture environment (private home, rustup, flags, temp, isolation), "
          "case map, keep-going/skip/abort driver, bounded unqualified receipt, dispatch")
    return 0


def expect_true(condition: bool, message: str = "expectation") -> None:
    if not condition:
        fail(message)


def mode_from_args(arguments: list) -> str | None:
    if not arguments:
        return "run"
    if arguments == ["--diagnostic"]:
        return "diagnostic"
    if arguments == ["--plan"]:
        return "plan"
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
    if mode == "plan":
        return plan()
    diagnostic = mode == "diagnostic"
    if sys.platform != "darwin":
        need("the front-door accounting proof requires macOS")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    if not (scratch and os.path.isdir(scratch) and os.path.realpath(scratch) == scratch):
        need(f"OXIDE_TASK_SCRATCH_ROOT {scratch!r} is not a canonical directory")
    for name in ("TMPDIR", "TMP", "TEMP"):
        if os.environ.get(name) != scratch:
            need(f"{name} is not the guard's scratch")
    if not (os.path.isdir(os.environ.get("OXIDE_PROOF_EVIDENCE", "")) and "OXIDE_PROOF_DEADLINE_NS" in os.environ):
        need("not under the qualified shared guard")
    work_deadline()
    if subprocess.run(["git", "status", "--porcelain"], cwd=REPO, capture_output=True,
                      text=True, timeout=30).stdout.strip():
        need("the checkout is not clean, so HEAD does not name its source")
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True,
                          text=True, timeout=30).stdout.strip()
    reserve(scratch, "admission")
    parent = os.path.realpath(tempfile.mkdtemp(prefix="front-door.", dir=scratch))
    os.chmod(parent, 0o700)
    say(f"source {head}, scratch {parent} inside the guard's known shared root")
    outcome = diagnostic_outcome()
    try:
        oxide = stage(parent, staging_environment())
        reserve(parent, "after staging")
        proof = Proof(parent, oxide, fixture_environment(parent))
        checkouts = {}
        for name in ("app", "big"):
            checkouts[name] = os.path.join(parent, "checkouts", name)
            write_fixture(checkouts[name], name)
        if diagnostic:
            outcome.update(source=head, root=parent)
            try:
                run_cases(True, proof, checkouts, {}, outcome)
            finally:
                path = diagnostic_receipt(outcome)
                print(f"front-door diagnostic receipt {path}")
        else:
            run_cases(False, proof, checkouts, {}, outcome)
    except Exception:
        print(f"front-door accounting proof retained its owned scratch at {parent}", file=sys.stderr)
        raise
    shutil.rmtree(parent)
    elapsed = time.monotonic() - STARTED
    if elapsed > PROOF_SECONDS + SHUTDOWN_MARGIN_S:
        need(f"front-door accounting proof took {elapsed:.0f} s, over its guard window")
    if diagnostic or outcome["failed"]:
        line = (
            f"front-door diagnostic collected (UNCOUNTED, never a PASS): "
            f"{len(outcome['completed'])}/{len(CASES)} cases completed, failed "
            f"{[case for case, _ in outcome['failed']]}, skipped "
            f"{[case for case, _ in outcome['skipped']]}, unexecuted {outcome['unexecuted']}; "
            f"{elapsed:.0f} s including cleanup, sampled peak owned scratch "
            f"{PEAK['bytes'] // MIB} MiB ({PEAK['phase']}), scratch removed"
        )
    else:
        line = (
            f"oxide front-door accounting proof held: source {head}, {elapsed:.0f} s including "
            f"cleanup, peak owned scratch sampled {PEAK['bytes'] // MIB} MiB ({PEAK['phase']}), "
            f"scratch removed"
        )
    print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
