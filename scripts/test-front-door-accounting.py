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
- a final default `status` after recovery still walks nothing;
- the same-slot hard-link retention control, restored natively on its own
  pool: one retained slot holds an 8 MiB file with four names for one inode, its
  release publishes the inode union and not the per-name sum, an owned 12 MiB
  filler slot supplies pressure under the pool's documented 16 MiB
  `OXIDE_POOL_CAP`, and an admission over that cap evicts the filler slot whole
  (one eviction per attempt, deleted outside the pool lock, then retried), keeps
  the retained slot with all four names and admits a new slot; per-name
  accounting would refuse the same admission. The case never writes a foreign
  entry into a payload root (the product refuses that by design).

Retired rather than duplicated: the whole-pool byte-pressure case
(`rust:recorded-slot-accounting` case D, with the holding deletion under it in
`rust:holding-delete`) is not repeated. That case evicts under independent slots
and case E is a *cross-slot* overcount, so neither proves that four names for one
inode inside ONE retained slot are charged as the inode union and that an
admission over a cap between that union and the per-name sum keeps the slot; that
control stays here as its own case. This script keeps the routing facts those
proofs do not cover: readiness, held and killed-holder refusals, the waiting
build and the recovered cold build.

The walk counts show routing only, not a resource gate. The scratch is one
fresh directory inside the qualified supervisor's `OXIDE_TASK_SCRATCH_ROOT`
(using the guard's selected-volume default), so the supervisor's continuous reserve watchdog, scratch
budget and closure census cover the actual work. The script stops 30 s before
the guard's exported deadline, and its own phase samples are observations,
not gates. Bounds: 300 s and 512 MiB monitored scratch under
`scripts/bounded-test.py`.

`--diagnostic` is the uncounted keep-going collection: it records reached,
completed, failed, dependent-skipped and unexecuted cases and the latched
first fault in a bounded `qualified: false` receipt beside the guard's,
continues only into dependency-independent cases, and aborts on an outcome it
cannot classify. Writer closure is never something this script can conclude:
`communicate_bounded`/`wait_child`/`wait_for`/`wait_text`/`stage` signal and
reap what they can and then raise `UnknownOutcome` (`uncertain_child`), and
once *any* `oxide` child has been launched (`LAUNCHES`) every later failure —
in any case, including `hardlink` — is an `UnknownOutcome` too, because reaping
a direct leader and draining its pipes proves nothing about its Cargo
descendants. Only a failure before the first launch may be a recoverable
oracle failure. An abort leaves the scratch in place for the guard's later
certain census. The receipt is written in every outcome, including a failure
before the first case ran. It is never a counted PASS. `--plan` prints the case table, the
allocation and the nine premise dispositions without any effect.
`--self-check` is the pure apparatus check: no child, pool, image or scratch.
Not a benchmark and not full QA. The script never removes its own staging
tree or run root: both stay inside the guard's owned root so its post-leader
census decides closure, and the guard's retention policy owns cleanup.
"""
from __future__ import annotations

import hashlib
import inspect
import json
import os
import re
import shutil
import signal
import stat
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
# The same-slot hard-link retention control: one 8 MiB file with four names for
# one inode inside ONE retained slot, beside an owned 12 MiB filler slot, on a
# pool whose documented soft cap sits between the retained slot's inode union and
# the per-name sum. Pressure never comes from a payload-root foreign entry: the
# product refuses those by design.
LINK_BYTES = 8 * MIB
LINK_NAMES = 4
HARDLINK_FILLER_BYTES = 12 * MIB
HARDLINK_CAP_BYTES = 16 * MIB
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
        "private; staging keeps exactly one account directory, the invoking CARGO_HOME set "
        "explicitly by `invoking_cargo_home()` (this repository's committed lock resolves its "
        "offline registry cache there), and no other inherited CARGO_* value; staging is the only "
        "--locked call, against that committed lock, and the dependency-free fixtures make their own "
        "lock offline",
    ),
    (
        "account-home control",
        "applicable: `fixture_environment` sets OXIDE_TEST_HOME_FROM_ENV=1 beside the private HOME",
    ),
    (
        "environment isolation",
        "applicable: both environment builders drop every OXIDE_/XDG_/CARGO_/SCCACHE_/RUSTC* key, "
        "CARGO, CARGO_INCREMENTAL, RUSTUP_TOOLCHAIN, RUSTFLAGS and CARGO_ENCODED_RUSTFLAGS, then set "
        "only the declared values (`staging_environment` re-adds the invoking CARGO_HOME, "
        "`CARGO_NET_OFFLINE` and the fixed profile debug setting; `fixture_environment` re-adds its "
        "private HOME/CARGO_HOME, the pinned toolchain and the frozen flags)",
    ),
    (
        "automatic adoption",
        "not applicable: no permanent-drive mode, no Git repository in any fixture and no outputs "
        "conversion; every checkout keeps its target local",
    ),
    ("Cargo external argv", "not applicable: no external Cargo subcommand runs in this proof"),
    ("owned images", "not applicable: no owned image or mount; pools use the selected volume under the guard"),
    (
        "compiler flags",
        "applicable: RUSTFLAGS is the fixed FROZEN_RUSTFLAGS and CARGO_ENCODED_RUSTFLAGS is removed for "
        "staging and every child; the staged binary must carry its walk observer",
    ),
    (
        "temporary storage",
        "applicable: the scratch is one fresh directory inside the qualified supervisor's "
        "OXIDE_TASK_SCRATCH_ROOT; pools use a private scratch root there, and TMPDIR/TMP/TEMP "
        "name one owned directory inside it. A /private/tmp fallback is internal, not scratch-pool coverage",
    ),
    (
        "macOS mktemp",
        "applicable: tempfile.mkdtemp with a template inside OXIDE_TASK_SCRATCH_ROOT; the executable "
        "fixture inherits that TMPDIR",
    ),
)

LINK_BUILD_RS = """use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let first = out.join("linked.bin");
    fs::write(&first, vec![2u8; 8 * 1024 * 1024]).unwrap();
    for index in 0..3 {
        fs::hard_link(&first, out.join(format!("linked.bin.{index}"))).unwrap();
    }
    println!("cargo:rerun-if-changed=build.rs");
}
"""

FILLER_BUILD_RS = """use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let mut left = 12 * 1024 * 1024;
    let mut index = 0;
    while left > 0 {
        let chunk = left.min(1024 * 1024);
        fs::write(out.join(format!("filler.{index}")), vec![3u8; chunk]).unwrap();
        left -= chunk;
        index += 1;
    }
    println!("cargo:rerun-if-changed=build.rs");
}
"""

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


def uncertain_child(label: str, detail: str) -> None:
    """A child that did not finish in its bound. Its own group's closure is
    unknown to this script, so a diagnostic must abort rather than record a
    recoverable oracle failure: only the guard's census can decide closure."""
    raise UnknownOutcome(f"{label}: {detail}; its Cargo group's closure is unknown")


def communicate_bounded(process, label: str, bound: float) -> tuple:
    """Wait for a child; a timeout kills its leader, reaps it if it can and
    always raises [`UnknownOutcome`], never an [`OracleFailure`]."""
    try:
        return process.communicate(timeout=bound)
    except subprocess.TimeoutExpired:
        process.kill()
        try:
            process.communicate(timeout=10)
        except BaseException:
            uncertain_child(label, "it did not finish and its leader was not reaped")
        uncertain_child(label, f"it did not finish within {bound:.0f} s")


def wait_child(process, label: str, seconds: float) -> int:
    """Reap a child within a bound; a timeout is an unknown writer outcome."""
    try:
        return process.wait(timeout=min(seconds, remaining()))
    except subprocess.TimeoutExpired:
        uncertain_child(label, f"it did not exit within {seconds:.0f} s")


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


def invoking_cargo_home() -> str:
    """The invoking Cargo home, resolved before any fixture HOME exists.

    Staging deliberately keeps this one account directory — this repository's
    committed lock resolves its registry cache there offline — and it is set
    explicitly rather than inherited, so no unknown `CARGO_*` value reaches the
    build. Every other inherited `CARGO_*` and `SCCACHE_*` value is dropped.
    """
    return os.environ.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo")


def staging_environment() -> dict:
    """Staging: the invoking Cargo home (explicitly), this repository's
    committed lock offline, the fixed compiler flags and no wrapper."""
    env = {
        name: value
        for name, value in os.environ.items()
        if not name.startswith(("CARGO_", "SCCACHE_", "OXIDE_", "XDG_", "RUSTC"))
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
            "CARGO_HOME": invoking_cargo_home(),
            "CARGO_NET_OFFLINE": "true",
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTFLAGS": FROZEN_RUSTFLAGS,
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
    pool_root = os.path.join(parent, "scratch-pools")
    for path in (home, cargo_home, tmp, pool_root, os.path.join(parent, "walks")):
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
            "OXIDE_SCRATCH_ROOT": pool_root,
            "OXIDE_POOL_CAP": str(SCRATCH_LIMIT_BYTES),
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


def run_staging(env: dict, target: str, log_handle) -> subprocess.CompletedProcess:
    """One bounded Cargo staging call, the seam the pure check drives.

    A staging build is a writer: its Cargo group can outlive the direct leader.
    So the target tree is never removed here — the guard's post-leader census
    decides closure and the guard's retention policy owns cleanup.
    """
    return subprocess.run(
        ["cargo", "build", "--locked", "--offline", "--bin", "oxide"],
        cwd=REPO,
        env={**env, "CARGO_TARGET_DIR": target, "CARGO_BUILD_BUILD_DIR": target},
        stdout=log_handle,
        stderr=subprocess.STDOUT,
        timeout=remaining(),
    )


def stage(parent: str, env: dict) -> str:
    target = os.path.join(parent, "stage")
    log = os.path.join(parent, "stage.log")
    with open(log, "w") as handle:
        try:
            build = run_staging(env, target, handle)
        except subprocess.TimeoutExpired:
            uncertain_child("staging", "the staged build did not finish in its bound")
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
    # The staging tree is retained inside the run root for the same reason the
    # run root is: its Cargo group may still have a descendant writing, only
    # the guard's post-leader census can establish closure, and the guard's
    # retention policy owns cleanup. Removing it here would delete that
    # evidence before the verdict.
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


def write_build_fixture(root: str, name: str, build_rs: str) -> str:
    """A tiny crate whose build script writes its own slot tree."""
    files = {
        "Cargo.toml": (
            f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n'
            f'build = "build.rs"\n'
        ),
        "build.rs": build_rs,
        "rust-toolchain.toml": f'[toolchain]\nchannel = "{BUILDER}"\n',
        "src/main.rs": f'fn main() {{\n    println!("{name}");\n}}\n',
    }
    for relative, text in files.items():
        path = os.path.join(root, relative)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as handle:
            handle.write(text)
    return root


def hardlink_names(slot_dir: str) -> list:
    """Every name of the hard-linked set inside one slot."""
    found = []
    for current, _dirs, files in os.walk(slot_dir, followlinks=False):
        for name in files:
            if name == "linked.bin" or name.startswith("linked.bin."):
                found.append(os.path.join(current, name))
    return sorted(found)


def hardlink_admission_discriminates(recorded: int, union: int, filler: int, per_name: int, cap: int) -> bool:
    """Whether this fixture makes the retention admission discriminating.

    The retained slot's published figure and the independent union measurement
    must both be a once-per-inode union for one 8 MiB file with four names; the
    older owned filler slot plus that union must reach the pool's soft cap; and
    the cap must fit the union while staying below the per-name sum, so
    union accounting is admitted after one eviction and per-name accounting
    would be refused.
    """
    return (
        LINK_BYTES <= recorded < 2 * LINK_BYTES
        and LINK_BYTES <= union < 2 * LINK_BYTES
        and per_name == union + (LINK_NAMES - 1) * LINK_BYTES
        and union < cap < per_name
        and filler + union >= cap
        and recorded < cap
    )


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
        relative = os.path.relpath(parent, os.path.dirname(env["OXIDE_SCRATCH_ROOT"]))
        self.pool = os.path.join(env["OXIDE_SCRATCH_ROOT"], "pool" if relative == "." else relative + "-pool")
        self.env = {**env, "OXIDE_POOL": self.pool, "OXIDE_POOL_CREATE": "1"}
        self.logs = 0
        self.dirs = {}
        self.last_log = ""
        # `log_path()` writes under `<parent>/walks`, so every Proof parent
        # owns that directory. The main run root gets it from
        # `fixture_environment`; a second fixture pool (the hard-link case's
        # `<run>/hardlink`) only exists because this creates it. Without it the
        # debug walk observer cannot append, the product warns honestly, and
        # the case aborts as an unknown outcome.
        os.makedirs(os.path.join(parent, "walks"), mode=0o700, exist_ok=True)

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
        note_child_launch()
        process = subprocess.Popen([self.oxide, *args], cwd=cwd, stdout=subprocess.PIPE, stderr=stderr,
                                   text=True, env={**self.env, "OXIDE_TEST_WALK_LOG": log,
                                                   "OXIDE_TEST_SCOPE_LOG": log + ".scope", **(extra or {})})
        return process, log

    def call(self, cwd: str, args: list, extra: dict | None = None) -> tuple:
        bound = min(120.0, remaining())
        process, log = self.start(cwd, args, extra)
        started = time.monotonic()
        stdout, stderr = communicate_bounded(process, f"oxide {args}", bound)
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
    """Wait for a fixture file. A child that exited without producing it is a
    premise failure; a child that is still running when the wait expires is an
    unknown writer outcome and aborts a diagnostic."""
    deadline = time.monotonic() + min(seconds, remaining())
    while not os.path.exists(path):
        if process.poll() is not None:
            fail(f"{label}: exited {process.returncode} without {path}")
        if time.monotonic() > deadline:
            uncertain_child(label, f"it did not produce {path} within {seconds:.0f} s")
        time.sleep(0.1)


def wait_text(path: str, text: str, process, label: str, seconds: float = 60) -> None:
    """Wait for a line in a child's own file, with the same classification."""
    deadline = time.monotonic() + min(seconds, remaining())
    while True:
        with open(path) as handle:
            if text in handle.read():
                return
        if process.poll() is not None:
            fail(f"{label}: exited {process.returncode} without {text!r}")
        if time.monotonic() > deadline:
            uncertain_child(label, f"it printed no {text!r} within {seconds:.0f} s")
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
            # A group that still has a member is a live writer. The script
            # cannot conclude its closure, so this is an unknown outcome that
            # aborts the collection; only the guard's later census decides.
            need(f"{label}: Cargo group {group} still has a member after {seconds:.0f} s")
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
        code = wait_child(waiter, "the woken build", 120)
        waiter_err.close()
        with open(waiter_path) as handle:
            stderr = handle.read()
        slot, warmth, tail = summary(stderr, "woken build")
        say(f"woken build: exit {code}, slot {slot} {warmth}, walks {proof.walks(waiter_log)}, summary tail: {tail}")
        if code or slot != app_slot or warmth != "warm":
            fail(f"the woken build did not take slot {app_slot} warm: {code} {stderr[-2000:]}")
        expect_roots("woken build", "walks", proof.walks(waiter_log),
                     [f"slots/{app_slot}", f"slots/{app_slot}"])
        if wait_child(holder, "the clean holder", 60):
            fail(f"the clean holder exited {holder.returncode}")
        expect_roots("holder", "walks", proof.walks(holder_log),
                     [f"slots/{app_slot}", f"slots/{app_slot}"])
        holder_err.close()
    finally:
        with open(release, "w"):
            pass
        holder_err.close()
        # Best effort only: killing and reaping a direct leader does not prove
        # its Cargo group has no descendant left. A failure on this path has
        # already been converted to an unknown outcome, so the guard's census
        # decides closure and the scratch is retained.
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
        wait_child(killed, "the killed holder", 30)
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
    """The recovery case, whose failures are unknown writer outcomes.

    This case starts fixture children (`oxide test` and `oxide build`) and can
    only kill and reap the direct leaders. That does not prove their Cargo
    group has no descendant still writing in the slot, so an oracle failure
    here is not a recoverable one: it aborts the collection, records the fault
    and leaves every later case unexecuted, and `run_proof` retains the scratch
    instead of removing it. The guard's later certain census is the closure
    authority, never this script.
    """
    try:
        kept = prove_recovery(proof, checkouts["app"], state["app_slot"])
        if kept not in proof.slots() or kept == state["big_slot"]:
            fail(f"the recovered cold slot {kept} is not a new slot in {sorted(proof.slots())}")
        state["kept_slot"] = kept
        owned(proof.parent, "after recovery")
        reserve(proof.parent, "after recovery")
        recorded_status(proof, checkouts["app"], "after recovery")
        status(proof, checkouts["app"], "after recovery", list(proof.slots()))
    except UnknownOutcome:
        raise
    except OracleFailure as failure:
        need(
            "the recovery case cannot be concluded while its fixture group may still be "
            f"writing: {failure}"
        )


def case_hardlink(proof: Proof, checkouts: dict, state: dict) -> None:
    """The same-slot hard-link retention control, restored natively.

    `rust:recorded-slot-accounting` case D evicts under independent slots and
    case E is a *cross-slot* overcount, so neither proves this: four names for
    one 8 MiB inode inside ONE retained slot must be charged as the inode union,
    so an admission on a pool whose soft cap fits that union (after evicting the
    older owned filler slot, the one eviction the design takes per attempt)
    succeeds and keeps the retained slot with all four names. Per-name
    accounting would charge ~4x and refuse the same admission.

    The pressure is the pool's own documented soft cap (`OXIDE_POOL_CAP`,
    applied at creation) plus an owned filler slot inside the pool. The case
    never writes a foreign entry into a payload root: the product refuses that
    by design (`unknown_accounting`), so a stray there can never supply
    pressure.
    """
    root = os.path.join(proof.parent, "hardlink")
    os.makedirs(root, mode=0o700, exist_ok=True)
    env = dict(proof.env, OXIDE_POOL_CAP=str(HARDLINK_CAP_BYTES))
    link_proof = Proof(root, proof.oxide, env)
    filler_checkout = write_build_fixture(os.path.join(root, "checkouts", "filler"), "filler", FILLER_BUILD_RS)
    retained_checkout = write_build_fixture(os.path.join(root, "checkouts", "linked"), "linked", LINK_BUILD_RS)
    fresh_checkout = os.path.join(root, "checkouts", "fresh")
    write_fixture(fresh_checkout, "fresh")
    filler, _, _, _ = build(link_proof, filler_checkout, "hard-link filler slot",
                            ["slots/{slot}"], ["payload"])
    retained, _, _, _ = build(link_proof, retained_checkout, "hard-link retained slot",
                              ["slots/{slot}"], ["payload"])
    slot_dir = link_proof.slot_dir(retained)
    names = hardlink_names(slot_dir)
    inodes = {os.stat(path).st_ino for path in names}
    record = (link_proof.slots().get(retained) or {}).get("recorded") or {}
    recorded = record.get("unique_bytes")
    filler_record = (link_proof.slots().get(filler) or {}).get("recorded") or {}
    filler_bytes = filler_record.get("unique_bytes")
    if not isinstance(recorded, int) or not isinstance(filler_bytes, int):
        fail(f"the hard-link pool published no recorded sizes: {record!r} / {filler_record!r}")
    union = tree_bytes(slot_dir)
    per_name = union + (LINK_NAMES - 1) * LINK_BYTES
    say(f"hard link: {len(names)} names for {len(inodes)} inode(s) in slot {retained}, "
        f"recorded {recorded} B, union {union} B, per-name {per_name} B, "
        f"filler slot {filler} {filler_bytes} B, cap {HARDLINK_CAP_BYTES} B")
    if len(names) != LINK_NAMES or len(inodes) != 1:
        fail(f"the retained slot does not hold {LINK_NAMES} names for one inode: {names}")
    if not hardlink_admission_discriminates(recorded, union, filler_bytes, per_name,
                                            HARDLINK_CAP_BYTES):
        fail(f"the fixtures do not separate once-per-inode from per-name accounting: "
             f"recorded {recorded} B, union {union} B, per-name {per_name} B, "
             f"filler {filler_bytes} B, cap {HARDLINK_CAP_BYTES} B")
    # One admission over the cap: the older owned filler slot is evicted whole
    # (deleted outside the pool lock, then the admission is retried), the
    # retained slot is kept with its four names, and a new slot is admitted.
    slot, warmth, tail, _ = build(link_proof, fresh_checkout, "hard-link admission",
                                  ["slots/{slot}"])
    left = link_proof.slots()
    if filler in left or os.path.lexists(link_proof.slot_dir(filler)):
        fail(f"the older idle slot {filler} was not evicted whole")
    if retained not in left or not os.path.isdir(slot_dir):
        fail(f"the retained slot {retained} was evicted")
    again = hardlink_names(slot_dir)
    if again != names or len({os.stat(path).st_ino for path in again}) != 1:
        fail(f"the retained slot lost its hard-link names: {again}")
    if warmth != "cold" or not tail.startswith("new slot, ") or slot in ("", retained, filler):
        fail(f"no new slot was admitted: {slot} {warmth} {tail}")
    say(f"hard link: older idle slot {filler} evicted, slot {retained} with {LINK_NAMES} names "
        f"kept, new slot {slot} admitted over the {HARDLINK_CAP_BYTES} B cap")
    state["hardlink"] = retained


CASES = (
    ("slots", (), "two new slots: no admission walk, one payload scope pass, a release walk"),
    ("status", ("slots",), "recorded status walks nothing; --measure walks the payload then every slot"),
    ("readiness", ("slots",), "oxide new readiness walks nothing and reads no pool state"),
    ("reuse", ("slots",), "the 200,000-entry idle slot, then ordinary reuse walks its target twice"),
    ("recovery", ("reuse",), "a live holder refusal and its woken build, then the killed holder's recovery"),
    ("hardlink", (),
     f"one retained slot holds four names for one {LINK_BYTES // MIB} MiB inode: its release "
     "publishes the inode union rather than the per-name sum, and an admission over the pool's "
     f"{HARDLINK_CAP_BYTES // MIB} MiB soft cap evicts the older owned filler slot whole, keeps the "
     "retained slot with all four names, and admits a new slot; per-name accounting would refuse "
     "the same admission"),
)

BODIES = {
    "slots": case_slots,
    "hardlink": case_hardlink,
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


# Every `oxide` child this proof launches can leave a Cargo descendant
# writing after its direct leader is reaped: `communicate_bounded` reaps the
# leader and drains its pipes, which proves nothing about the group. So any
# failure after the first launch is an unknown writer outcome — the collection
# aborts and the scratch is retained for the guard's later certain census.
# Only a failure before the first launch can be a recoverable oracle failure.
LAUNCHES = {"count": 0}


def note_child_launch() -> None:
    LAUNCHES["count"] += 1


def launched_children() -> int:
    return LAUNCHES["count"]


def record_abort(outcome: dict, case: str, failure) -> None:
    """Record one abort truthfully: the fault, the aborted case and everything
    that never ran. Undecided cases stay unexecuted, never skipped."""
    detail = failure if isinstance(failure, str) else (str(failure) or repr(failure))
    outcome["aborted"] = outcome["aborted"] or f"{case}: {detail}"
    outcome["first_fault"] = outcome["first_fault"] or outcome["aborted"]
    outcome["unexecuted"] = [name for name, _, _ in CASES if name not in outcome["reached"]]


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
            if not diagnostic:
                raise
            if launched_children():
                # A child (and therefore a Cargo group) was already started:
                # neither the direct leader's reap nor its drained pipes prove
                # that no descendant is still writing. This is an unknown
                # writer outcome, not a recoverable one — abort and keep the
                # scratch for the guard's census.
                record_abort(outcome, case, failure)
                print(f"front-door {case}: ABORTED {failure} (a child was already launched: "
                      f"{launched_children()})", file=sys.stderr, flush=True)
                raise UnknownOutcome(
                    f"{case} failed after a child launch, so writer closure is unknown: {failure}"
                ) from failure
            outcome["failed"].append([case, str(failure)[:DIAGNOSTIC_RECEIPT_EXCERPT]])
            outcome["first_fault"] = outcome["first_fault"] or f"{case}: {failure}"
            print(f"front-door {case}: FAILED {failure} (no child launched yet)", file=sys.stderr,
                  flush=True)
            continue
        except UnknownOutcome as failure:
            record_abort(outcome, case, failure)
            print(f"front-door {case}: ABORTED {failure}", file=sys.stderr, flush=True)
            raise
        except (KeyboardInterrupt, SystemExit):
            record_abort(outcome, case, "interrupted")
            raise
        except BaseException as failure:
            # An unexpected exception is an outcome this collection cannot
            # classify (never a recoverable oracle failure, never a pass).
            record_abort(outcome, case, failure)
            print(f"front-door {case}: ABORTED unexpected {failure!r}", file=sys.stderr, flush=True)
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
    print("selector: python3 scripts/bounded-test.py "
          "--limit 300 --scratch-budget-mib 512 -- python3 scripts/test-front-door-accounting.py")
    print("allocation: one main scratch directory-budget pool with two slots, one idle slot of more "
          f"than {BIG_ENTRIES:,} entries, two fixture checkouts and one `oxide new` crate; plus one "
          f"separate pool for the hard-link control, whose one retained slot holds a "
          f"{LINK_BYTES // MIB} MiB file with {LINK_NAMES} names for one inode and no foreign entry "
          "in the payload root; all inside one owned scratch inside OXIDE_TASK_SCRATCH_ROOT "
          "(measured staging peak 283 MiB of the 512 MiB monitored budget)")
    print(f"limits: {PROOF_SECONDS} s wall under the guard (which owns closure and cleanup), stop "
          f"{SHUTDOWN_MARGIN_S} s before its deadline, {SCRATCH_LIMIT_BYTES // MIB} MiB monitored "
          "scratch, one Cargo call at a time")
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
    LAUNCHES["count"] = 0
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
                    "OXIDE_SCRATCH_ROOT": "/fixture/host-scratch",
                    "OXIDE_POOL_CAP": "1",
                    "OXIDE_CONFIG": "/fixture/host-config",
                    "OXIDE_TEST_HOME_FROM_ENV": "0",
                    "XDG_CONFIG_HOME": "/fixture/xdg",
                    "RUSTFLAGS": "-Ctarget-cpu=native",
                    "CARGO_ENCODED_RUSTFLAGS": "-Ctarget-cpu=native",
                    "RUSTUP_TOOLCHAIN": "nightly",
                    "SCCACHE_DIR": "/fixture/sccache",
                    "SCCACHE_BUCKET": "/fixture/bucket",
                    "CARGO_UNSTABLE_ALLOW_FEATURES": "fixture",
                    "CARGO_REGISTRIES_FIXTURE_INDEX": "/fixture/index",
                    "CARGO_TARGET_DIR": "/fixture/target",
                    "CARGO_BUILD_JOBS": "9",
                }
            )
            fixture_root = os.path.join(root, "fixture")
            env = fixture_environment(fixture_root)
            expect_true(env["OXIDE_SCRATCH_ROOT"] == os.path.join(fixture_root, "scratch-pools"), "scratch root is not private")
            expect_true(env["OXIDE_POOL_CAP"] == str(SCRATCH_LIMIT_BYTES), "pool cap is not explicit")
            main_pool = Proof(fixture_root, "oxide", env).pool
            other_pool = Proof(os.path.join(fixture_root, "hardlink"), "oxide", env).pool
            expect_true(main_pool == os.path.join(env["OXIDE_SCRATCH_ROOT"], "pool"), "main pool escaped scratch root")
            expect_true(other_pool == os.path.join(env["OXIDE_SCRATCH_ROOT"], "hardlink-pool"), "hardlink pool escaped scratch root")
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
            # Staging keeps exactly the invoking Cargo home, set explicitly,
            # and drops every other inherited CARGO_*/SCCACHE_* value.
            expect_true(staging["CARGO_HOME"] == invoking_cargo_home(),
                        "staging did not keep the invoking Cargo home explicitly")
            for name in ("CARGO_UNSTABLE_ALLOW_FEATURES", "CARGO_REGISTRIES_FIXTURE_INDEX",
                         "CARGO_TARGET_DIR", "CARGO_BUILD_JOBS", "SCCACHE_DIR",
                         "SCCACHE_BUCKET", "CARGO_INCREMENTAL"):
                expect_true(name not in staging, f"staging inherited {name}")
            declared = {"CARGO_HOME", "CARGO_NET_OFFLINE", "CARGO_PROFILE_DEV_DEBUG"}
            inherited = {
                name for name in os.environ
                if name.startswith(("CARGO_", "SCCACHE_"))
            } - declared
            expect_true(not (inherited & set(staging)), f"staging copied {sorted(inherited & set(staging))}")
        finally:
            os.environ.clear()
            os.environ.update(holder)
        # A child that does not finish in its bound is an unknown writer
        # outcome, never a recoverable oracle failure: the script cannot
        # conclude its group's closure, only the guard's census can.
        class StubbornChild:
            def __init__(self, reapable: bool):
                self.reapable = reapable
                self.killed = False
                self.calls = 0

            def communicate(self, timeout=None):
                self.calls += 1
                if self.calls == 1:
                    raise subprocess.TimeoutExpired("oxide", timeout)
                if not self.reapable:
                    raise subprocess.TimeoutExpired("oxide", timeout)
                return ("", "")

            def kill(self):
                self.killed = True

        slow = StubbornChild(reapable=True)
        try:
            communicate_bounded(slow, "oxide build", 5)
        except UnknownOutcome as error:
            expect_true("closure is unknown" in str(error), str(error))
            expect_true(slow.killed, "the timed-out child was not signalled")
        except OracleFailure:
            fail("a timed-out child was classified as a recoverable oracle failure")
        else:
            fail("a timed-out child did not abort")
        unreapable = StubbornChild(reapable=False)
        try:
            communicate_bounded(unreapable, "oxide build", 5)
        except UnknownOutcome as error:
            expect_true("not reaped" in str(error), str(error))
        except OracleFailure:
            fail("an unreaped child was classified as a recoverable oracle failure")
        else:
            fail("an unreaped child did not abort")

        class NeverExits:
            def wait(self, timeout=None):
                raise subprocess.TimeoutExpired("oxide", timeout)

        try:
            wait_child(NeverExits(), "the holder", 1)
        except UnknownOutcome:
            pass
        except OracleFailure:
            fail("a child that never exits was classified as a recoverable oracle failure")
        else:
            fail("a child that never exits did not abort")
        # The same classification is pinned at each waiting site, so a future
        # edit cannot turn a timeout back into an oracle failure.
        for function in (Proof.call, wait_for, wait_text, stage):
            source = inspect.getsource(function)
            expect_true("uncertain_child" in source or "communicate_bounded" in source,
                        f"{function.__name__} has no unknown-outcome timeout path")
        expect_true("communicate_bounded" in inspect.getsource(Proof.call),
                    "Proof.call does not wait through communicate_bounded")

        # A driver case that cannot classify its outcome aborts the
        # collection, records the fault and leaves the rest unexecuted; it is
        # never recorded as a recoverable failure or a pass.
        def stub_timeout(proof, checkouts, state):
            raise subprocess.TimeoutExpired("oxide build", 5)

        def stub_unexpected(proof, checkouts, state):
            raise KeyError("unexpected")

        def stub_interrupt(proof, checkouts, state):
            raise KeyboardInterrupt()

        def stub_noop(proof, checkouts, state):
            return None

        for stub in (stub_timeout, stub_unexpected, stub_interrupt):
            outcome = diagnostic_outcome()
            bodies = {case: stub_noop for case, _, _ in CASES}
            bodies["status"] = stub
            try:
                run_cases(True, None, {}, {}, outcome, bodies)
            except (UnknownOutcome, KeyError, KeyboardInterrupt, subprocess.TimeoutExpired):
                pass
            else:
                fail(f"{stub.__name__} did not abort the collection")
            expect_true(outcome["aborted"] is not None, f"{stub.__name__} left no aborted fault")
            expect_true([case for case, _ in outcome["failed"]] == [],
                        f"{stub.__name__} was recorded as a recoverable failure: {outcome['failed']}")
            expect_true(outcome["completed"] == ["slots"],
                        f"{stub.__name__} completed a case it aborted: {outcome['completed']}")
            expect_true(
                outcome["unexecuted"] == [case for case, _, _ in CASES if case not in outcome["reached"]],
                f"{stub.__name__} unexecuted {outcome['unexecuted']}",
            )
            # The receipt keeps the abort and the unexecuted set.
            holder = dict(os.environ)
            os.environ["OXIDE_PROOF_EVIDENCE"] = root
            try:
                with open(diagnostic_receipt(outcome), encoding="utf-8") as handle:
                    receipt = json.load(handle)
            finally:
                os.environ.clear()
                os.environ.update(holder)
            expect_true(receipt["qualified"] is False and receipt["aborted"] == outcome["aborted"],
                        f"receipt abort {receipt.get('aborted')!r}")
            expect_true(receipt["unexecuted"] == outcome["unexecuted"],
                        f"receipt unexecuted {receipt.get('unexecuted')}")

        # The restored same-slot hard-link retention control under a byte
        # budget: the case's own pool carries a documented soft cap between the
        # retained slot's inode union and the per-name sum, so the same
        # admission is admitted and keeps the retained slot on union accounting
        # and would be refused (or evict it) on per-name accounting.
        union = LINK_BYTES + 512 * 1024
        recorded = union
        filler = HARDLINK_FILLER_BYTES
        per_name = union + (LINK_NAMES - 1) * LINK_BYTES
        cap = HARDLINK_CAP_BYTES
        expect_true(
            hardlink_admission_discriminates(recorded, union, filler, per_name, cap),
            "the hard-link admission oracle does not accept the union fixture",
        )
        for label, args in (
            ("a per-name record", (per_name, union, filler, per_name, cap)),
            ("a record below one file", (LINK_BYTES // 2, union, filler, per_name, cap)),
            ("a per-name sum equal to the union", (recorded, union, filler, union, cap)),
            ("a union below one file", (recorded, LINK_BYTES // 2, filler, per_name, cap)),
            ("a cap above the per-name sum", (recorded, union, filler, per_name, per_name + 1)),
            ("a cap below the union", (recorded, union, filler, per_name, union - 1)),
            ("a filler too small for pressure", (recorded, union, 1, per_name, cap)),
        ):
            expect_true(
                not hardlink_admission_discriminates(*args),
                f"the hard-link admission oracle accepted {label}: {args}",
            )
        expect_true(
            ("hardlink", ()) in [(case, deps) for case, deps, _ in CASES],
            "the hard-link case is missing or depends on an unrelated premise",
        )
        expect_true("hardlink" in BODIES, "the hard-link case has no body")
        # Pressure comes from the pool's own soft cap and an owned filler slot;
        # the case must never plant a foreign payload entry (the product refuses
        # that by design) and must keep its retention assertions.
        hardlink_case = inspect.getsource(case_hardlink)
        expect_true("OXIDE_POOL_CAP" in hardlink_case,
                    "the hard-link case does not use the documented soft cap")
        expect_true("LINK_STRAY_BYTES" not in hardlink_case and "stray.bin" not in hardlink_case,
                    "the hard-link case still plants a stray payload entry")
        expect_true("link_proof.payload()" not in hardlink_case,
                    "the hard-link case writes into the pool's payload root")
        expect_true("OXIDE_TEST_BUDGET_BYTES" not in hardlink_case,
                    "the hard-link case still uses the debug budget override")
        for needle in ("was not evicted whole", "was evicted", "lost its hard-link names"):
            expect_true(needle in hardlink_case,
                        f"the hard-link case lost its retention assertion {needle!r}")

        # A Cargo group that still has a member is a live writer: the wait is
        # an unknown outcome, never a recoverable oracle failure. The check
        # uses this process's own group, which always has a member, and sends
        # no signal (`killpg(..., 0)` only probes).
        try:
            await_group_empty(os.getpgid(0), "self-check", seconds=0)
        except UnknownOutcome as error:
            expect_true("still has a member" in str(error), str(error))
        except OracleFailure:
            fail("a non-empty group was classified as a recoverable oracle failure")
        else:
            fail("a non-empty group did not abort")

        # The recovery case starts fixture children and can only reap the
        # direct leaders, so its oracle failures are unknown writer outcomes:
        # the collection aborts, no later case (including the independent
        # hard-link case) runs, and the receipt keeps the fault and the
        # unexecuted set.
        saved_recovery = globals()["prove_recovery"]
        recovery_calls = []

        def failing_recovery(proof, app, app_slot):
            recovery_calls.append(app_slot)
            raise OracleFailure("the woken build did not take slot 1 warm")

        globals()["prove_recovery"] = failing_recovery
        try:
            outcome = diagnostic_outcome()
            bodies = {case: (lambda *args: None) for case, _, _ in CASES}
            bodies["recovery"] = lambda proof, checkouts, state: case_recovery(
                proof, {"app": "/fixture/app"}, {"app_slot": "1", "big_slot": "2"}
            )
            try:
                run_cases(True, None, {}, {}, outcome, bodies)
            except UnknownOutcome as error:
                expect_true("may still be writing" in str(error), str(error))
            except OracleFailure:
                fail("a recovery oracle failure was left recoverable")
            else:
                fail("a recovery oracle failure did not abort the collection")
        finally:
            globals()["prove_recovery"] = saved_recovery
        expect_true(recovery_calls == ["1"], f"the recovery stub ran {recovery_calls}")
        expect_true(outcome["failed"] == [],
                    f"a recovery abort was recorded as a recoverable failure: {outcome['failed']}")
        expect_true(outcome["aborted"] is not None and "may still be writing" in outcome["aborted"],
                    f"aborted {outcome.get('aborted')!r}")
        expect_true(
            outcome["unexecuted"] == [case for case, _, _ in CASES if case not in outcome["reached"]],
            f"unexecuted {outcome['unexecuted']}",
        )
        expect_true("hardlink" in outcome["unexecuted"] and "hardlink" not in outcome["completed"],
                    f"the independent hard-link case followed an unclosed recovery: {outcome['completed']}")
        holder = dict(os.environ)
        evidence = tempfile.mkdtemp(prefix="fd-recovery-abort.")
        os.environ["OXIDE_PROOF_EVIDENCE"] = evidence
        try:
            with open(diagnostic_receipt(outcome), encoding="utf-8") as handle:
                receipt = json.load(handle)
        finally:
            os.environ.clear()
            os.environ.update(holder)
            shutil.rmtree(evidence, ignore_errors=True)
        expect_true(receipt["qualified"] is False and receipt["aborted"] == outcome["aborted"],
                    f"receipt abort {receipt.get('aborted')!r}")
        expect_true(receipt["failed"] == [] and receipt["unexecuted"] == outcome["unexecuted"],
                    f"receipt state {receipt.get('failed')} / {receipt.get('unexecuted')}")

        # Every Proof parent owns the `walks` directory its walk log needs —
        # including the hard-link case's second pool root, which is the only
        # one not created by `fixture_environment`. The check is effect-free:
        # no child is started, only the directories are inspected.
        proof_source = inspect.getsource(Proof.__init__)
        expect_true('os.makedirs(os.path.join(parent, "walks")' in proof_source,
                    "Proof.__init__ does not create its parent's walks directory")
        expect_true('mode=0o700' in proof_source and "exist_ok=True" in proof_source,
                    "the walks directory is not created 0700 with exist_ok")
        hardlink_source = inspect.getsource(case_hardlink)
        expect_true(
            hardlink_source.index('os.makedirs(root, mode=0o700') < hardlink_source.index("Proof(root,"),
            "case_hardlink constructs its second Proof before its parent exists",
        )
        walks_holder = os.path.realpath(tempfile.mkdtemp(prefix="fd-walks-check."))
        try:
            for label, parent in (
                ("main run root", os.path.join(walks_holder, "run")),
                ("hard-link second pool", os.path.join(walks_holder, "run", "hardlink")),
            ):
                if label == "main run root":
                    os.makedirs(parent, mode=0o700)
                else:
                    os.makedirs(parent, mode=0o700)
                case = Proof(parent, "/fixture/oxide", {"OXIDE_SCRATCH_ROOT": os.path.join(parent, "scratch-pools")})
                walks = os.path.join(parent, "walks")
                expect_true(os.path.isdir(walks), f"{label}: no walks directory at {walks}")
                expect_true(stat.S_IMODE(os.stat(walks).st_mode) == 0o700,
                            f"{label}: walks directory is {oct(os.stat(walks).st_mode)}")
                log = case.log_path()
                expect_true(os.path.dirname(log) == walks,
                            f"{label}: log_path {log} is not inside {walks}")
                # The directory is usable for the observer's append, and the
                # oracle reads it back.
                with open(log, "a") as handle:
                    handle.write(os.path.realpath(parent) + "\n")
                expect_true(read_roots(log, case.payload())[-1] == os.path.realpath(parent),
                            f"{label}: the walk log is not readable back")
        finally:
            shutil.rmtree(walks_holder, ignore_errors=True)

        # Staging is a writer too, so its target tree is retained for the
        # guard's post-leader census as well. The pure check drives the real
        # `stage` through the `run_staging` seam — no writer is launched — and
        # asserts the tree and the copied binary are still there afterwards.
        stage_source = inspect.getsource(stage)
        expect_true(
            "shutil.rmtree(target)" not in stage_source,
            "stage removes its target tree before the guard's census",
        )
        expect_true("run_staging(" in stage_source, "stage does not use the staging seam")
        staging_root = tempfile.mkdtemp(prefix="fd-staging-retained.")
        saved_staging = globals()["run_staging"]
        try:
            def fake_staging(env, target, log_handle):
                binary = os.path.join(target, "debug", "oxide")
                os.makedirs(os.path.dirname(binary), exist_ok=True)
                with open(binary, "wb") as handle:
                    handle.write(b"OXIDE_TEST_WALK_LOG")
                return subprocess.CompletedProcess(["cargo"], 0, stdout="", stderr="")

            globals()["run_staging"] = fake_staging
            staged = stage(staging_root, {})
            expect_true(os.path.isfile(staged), f"stage did not copy {staged}")
            expect_true(
                os.path.isdir(os.path.join(staging_root, "stage")),
                "stage removed its target tree before the guard's census",
            )
            expect_true(
                os.path.isfile(os.path.join(staging_root, "stage", "debug", "oxide")),
                "the staged target's binary was removed",
            )
        finally:
            globals()["run_staging"] = saved_staging
            shutil.rmtree(staging_root, ignore_errors=True)

        # `run_proof` NEVER removes its own run root either: only the guard's
        # post-leader census can establish that no child descendant is still
        # writing, and the guard's retention policy owns cleanup. That rule is
        # pinned structurally and behaviourally, on both the aborted and the
        # successful path; the staging tree is covered above.
        run_source = inspect.getsource(run_proof)
        expect_true(
            "shutil.rmtree(parent)" not in run_source,
            "run_proof removes its own run root before the guard's census",
        )
        expect_true(
            "retained for the guard" in run_source,
            "run_proof does not report that the scratch is retained",
        )
        expect_true(
            "retained its owned scratch" in run_source
            and "raise" in run_source[run_source.index("except Exception:"):],
            "run_proof does not retain the scratch and re-raise on a failure",
        )
        if sys.platform == "darwin":
            abort_root = tempfile.mkdtemp(prefix="fd-retain-check.")
            canonical = os.path.realpath(abort_root)
            saved_functions = {
                name: globals()[name]
                for name in ("resolve_source", "stage", "fixture_environment", "run_cases")
            }
            holder = dict(os.environ)
            os.environ.update(
                {
                    "OXIDE_TASK_SCRATCH_ROOT": canonical,
                    "OXIDE_PROOF_EVIDENCE": os.path.join(canonical, "evidence"),
                    "TMPDIR": canonical,
                    "TMP": canonical,
                    "TEMP": canonical,
                    "OXIDE_PROOF_DEADLINE_NS": str(
                        time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) + 300 * 10**9
                    ),
                }
            )
            os.makedirs(os.environ["OXIDE_PROOF_EVIDENCE"], mode=0o700, exist_ok=True)

            def aborting_cases(diagnostic, proof, checkouts, state, outcome):
                raise UnknownOutcome(
                    "status failed after a child launch, so writer closure is unknown"
                )

            globals()["resolve_source"] = lambda: "a" * 40
            globals()["stage"] = lambda parent, env: "/fixture/oxide"
            globals()["fixture_environment"] = lambda parent: {"OXIDE_SCRATCH_ROOT": os.path.join(parent, "scratch-pools")}
            globals()["run_cases"] = aborting_cases
            outcome = diagnostic_outcome()
            try:
                try:
                    run_proof(True, outcome)
                except UnknownOutcome as error:
                    expect_true("writer closure is unknown" in str(error), str(error))
                else:
                    fail("an aborted collection did not fail the run")
                roots = [name for name in os.listdir(canonical) if name.startswith("front-door.")]
                expect_true(len(roots) == 1, f"the aborted run's scratch was not retained: {roots}")
                # The successful path retains its run root too: nothing is
                # deleted before the guard's census.
                globals()["run_cases"] = lambda *args: None
                success = diagnostic_outcome()
                code = run_proof(False, success)
                expect_true(code == 0, f"a successful run returned {code}")
                roots = [name for name in os.listdir(canonical) if name.startswith("front-door.")]
                expect_true(len(roots) == 2,
                            f"the successful run's scratch was not retained: {roots}")
                expect_true(outcome["root"] is not None and outcome["root"].startswith(canonical),
                            f"the abort did not name its retained root: {outcome.get('root')!r}")
            finally:
                for name, function in saved_functions.items():
                    globals()[name] = function
                os.environ.clear()
                os.environ.update(holder)
                shutil.rmtree(abort_root, ignore_errors=True)

        # The case map: every case has a body and a purpose, and every
        # dependency names an earlier case, so the diagnostic can skip only
        # dependents of what did not complete.
        expect_true(len(CASES) == 6, f"case table {CASES}")
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
        # Only the independent cases still run; every chained case depends
        # transitively on `slots`, so all of them are skipped.
        independent = {case for case, deps, _ in CASES if not deps}
        expect_true(set(outcome["completed"]) == independent - {"slots"},
                    f"a failed case's dependents ran: {outcome['completed']}")
        expect_true([case for case, _ in outcome["failed"]] == ["slots"], f"failed {outcome['failed']}")
        expect_true(
            [row[0] for row in outcome["skipped"]] == [case for case, deps, _ in CASES if deps],
            f"skipped {outcome['skipped']}",
        )
        expect_true("fail" in calls, f"the failed case did not run: {calls}")
        # A recoverable failure is only allowed before any child has launched:
        # here no stub launched one, which is the demonstrably safe case.
        expect_true(launched_children() == 0,
                    f"a child was launched before the recoverable-continuation case: {LAUNCHES}")
        # An independent case still runs after another case's recoverable
        # failure, and the first fault is latched.
        calls.clear()
        outcome = diagnostic_outcome()
        run_cases(True, None, {}, {}, outcome, stubs(status=stub_fail))
        expect_true(set(outcome["completed"]) == {case for case, _, _ in CASES} - {"status"},
                    f"completed {outcome['completed']}")
        expect_true([case for case, _ in outcome["failed"]] == ["status"], f"failed {outcome['failed']}")
        expect_true(outcome["first_fault"] == "status: stub failure", f"first fault {outcome['first_fault']}")
        # The same oracle failure after a child launch is NOT recoverable:
        # the direct leader's reap and drained pipes prove nothing about its
        # Cargo descendants, so the collection aborts, the independent
        # hard-link case never runs, and the receipt keeps the fault and every
        # unexecuted case.
        LAUNCHES["count"] = 0
        outcome = diagnostic_outcome()
        ran = []

        def launching_failure(proof, checkouts, state):
            note_child_launch()
            fail("the build exited 1 after a child launch")

        bodies = {case: stub_noop for case, _, _ in CASES}
        bodies["status"] = launching_failure
        bodies["hardlink"] = lambda *args: ran.append("hardlink")
        try:
            run_cases(True, None, {}, {}, outcome, bodies)
        except UnknownOutcome as error:
            expect_true("writer closure is unknown" in str(error), str(error))
        except OracleFailure:
            fail("a post-launch oracle failure was left recoverable")
        else:
            fail("a post-launch oracle failure did not abort the collection")
        expect_true(ran == [], f"the independent hard-link case ran after an unclosed writer: {ran}")
        expect_true(outcome["failed"] == [],
                    f"a post-launch failure was recorded as recoverable: {outcome['failed']}")
        expect_true(outcome["aborted"] is not None and "status" in outcome["aborted"],
                    f"aborted {outcome.get('aborted')!r}")
        expect_true("hardlink" in outcome["unexecuted"] and "hardlink" not in outcome["completed"],
                    f"the independent hard-link case followed an unclosed writer: {outcome['completed']}")
        expect_true(outcome["completed"] == ["slots"],
                    f"completed {outcome['completed']}")
        holder = dict(os.environ)
        evidence = tempfile.mkdtemp(prefix="fd-post-launch-abort.")
        os.environ["OXIDE_PROOF_EVIDENCE"] = evidence
        try:
            with open(diagnostic_receipt(outcome), encoding="utf-8") as handle:
                receipt = json.load(handle)
        finally:
            os.environ.clear()
            os.environ.update(holder)
            shutil.rmtree(evidence, ignore_errors=True)
        expect_true(receipt["qualified"] is False and receipt["aborted"] == outcome["aborted"],
                    f"receipt abort {receipt.get('aborted')!r}")
        expect_true(receipt["failed"] == [] and receipt["unexecuted"] == outcome["unexecuted"],
                    f"receipt state {receipt.get('failed')} / {receipt.get('unexecuted')}")
        LAUNCHES["count"] = 0

        # An unknown outcome aborts and leaves the rest unexecuted.
        outcome = diagnostic_outcome()
        try:
            run_cases(True, None, {}, {}, outcome, stubs(status=stub_abort))
        except UnknownOutcome:
            pass
        else:
            fail("an unknown outcome did not abort the collection")
        expect_true(outcome["aborted"] == "status: stub unknown outcome", f"aborted {outcome['aborted']}")
        expect_true(
            outcome["unexecuted"] == [case for case, _, _ in CASES if case not in outcome["reached"]],
            f"unexecuted {outcome['unexecuted']}",
        )
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
    # The receipt-finishing path: a diagnostic that ran to completion cannot
    # report success without its receipt, a run that is already failing keeps
    # its own error (the receipt failure is printed, never masking it), and a
    # counted run writes no receipt at all.
    writes = []

    def good_writer(outcome):
        writes.append("write")
        return "/fixture/front-door.diagnostic.json"

    def bad_writer(outcome):
        writes.append("fail")
        raise RuntimeError("receipt directory is gone")

    outcome = diagnostic_outcome()
    expect_true(finish_run(0, True, outcome, good_writer) == 0,
                "a written receipt did not return the run's code")
    expect_true(writes == ["write"], f"receipt writer calls {writes}")
    writes.clear()
    try:
        finish_run(0, True, outcome, bad_writer)
    except RuntimeError as error:
        expect_true("receipt directory is gone" in str(error), str(error))
    else:
        fail("a successful diagnostic returned without writing its receipt")
    writes.clear()
    expect_true(finish_run(0, True, outcome, bad_writer, failed=True) == 0,
                "a failing diagnostic did not keep its own error")
    expect_true(writes == ["fail"], f"failing-run writer calls {writes}")
    writes.clear()
    expect_true(finish_run(0, False, outcome, bad_writer) == 0, "a counted run failed on a receipt")
    expect_true(writes == [], f"a counted run called the receipt writer: {writes}")
    # `main` finishes through that one path, so no future edit can call the
    # writer directly and suppress its failure again.
    source = inspect.getsource(main)
    expect_true("finish_run(" in source, "main does not finish through finish_run")
    expect_true("diagnostic_receipt(" not in source,
                "main calls diagnostic_receipt directly, outside finish_run")
    handler = source[source.index("except BaseException as failure:"):]
    expect_true(
        "record_abort(" in handler
        and "finish_run(0, True, outcome, failed=True)" in handler
        and handler.index("finish_run(") < handler.index("raise"),
        f"main's failure path does not record, write best-effort and re-raise: {handler}",
    )
    # The same path end to end through `main` with the run itself stubbed: a
    # successful diagnostic whose receipt cannot be written raises instead of
    # returning 0, and a failing diagnostic keeps its own error.
    holder = dict(os.environ)
    saved_argv = sys.argv
    saved_run = globals()["run_proof"]
    evidence_root = tempfile.mkdtemp(prefix="fd-receipt-path.")
    try:
        blocker = os.path.join(evidence_root, "not-a-directory")
        with open(blocker, "w", encoding="utf-8") as handle:
            handle.write("")
        os.environ["OXIDE_PROOF_EVIDENCE"] = blocker
        globals()["run_proof"] = lambda diagnostic, outcome: 0
        sys.argv = ["test-front-door-accounting.py", "--diagnostic"]
        try:
            main()
        except RuntimeError as error:
            expect_true("evidence" in str(error), f"unexpected receipt error {error!r}")
        else:
            fail("the diagnostic reported success without writing its receipt")

        def failing_run(diagnostic, outcome):
            raise RuntimeError("the collection failed")

        globals()["run_proof"] = failing_run
        try:
            main()
        except RuntimeError as error:
            expect_true(str(error) == "the collection failed", f"the receipt path masked {error!r}")
        else:
            fail("a failing diagnostic did not propagate its own error")
    finally:
        sys.argv = saved_argv
        globals()["run_proof"] = saved_run
        os.environ.clear()
        os.environ.update(holder)
        shutil.rmtree(evidence_root, ignore_errors=True)
    # The front-door source gate is fail-closed, exactly like the removal
    # proof's: both git calls must exit 0, their answers must be an exact
    # 40-hex identity and an empty tracked status, and a failed/empty answer
    # aborts with the fault recorded and every case unexecuted before any
    # scratch, staging or child exists.
    def canned(returncode: int, stdout: str = "", stderr: str = "") -> subprocess.CompletedProcess:
        return subprocess.CompletedProcess(["git"], returncode, stdout=stdout, stderr=stderr)

    original_git = globals()["run_git"]
    gate_root = tempfile.mkdtemp(prefix="fd-source-gate.")
    holder = dict(os.environ)
    try:
        for label, head, status, needle in (
            (
                "status nonzero with empty stdout",
                canned(0, "a" * 40 + "\n"),
                canned(1, "", "fatal: not a git repository"),
                "worktree status exited 1",
            ),
            (
                "rev-parse nonzero",
                canned(128, "", "fatal: not a git repository"),
                canned(0, ""),
                "source lookup exited 128",
            ),
            ("rev-parse empty", canned(0, "\n"), canned(0, ""), "no exact source identity"),
            ("non-exact identity", canned(0, "deadbeef\n"), canned(0, ""), "no exact source identity"),
            (
                "dirty tracked worktree",
                canned(0, "a" * 40 + "\n"),
                canned(0, " M scripts/test-front-door-accounting.py\n"),
                "tracked worktree is not clean",
            ),
        ):
            answers = [head, status]
            calls = []

            def respond(arguments, answers=answers, calls=calls):
                calls.append(list(arguments))
                return answers[len(calls) - 1]

            globals()["run_git"] = respond
            try:
                resolve_source()
            except UnknownOutcome as error:
                expect_true(needle in str(error), f"{label}: {error}")
            else:
                fail(f"{label}: the source gate accepted it")
            expect_true(
                calls == [["rev-parse", "HEAD"], ["status", "--porcelain", "--untracked-files=no"]],
                f"{label}: gate calls {calls}",
            )

        def clean(arguments):
            return canned(0, "a" * 40 + "\n") if arguments[0] == "rev-parse" else canned(0, "")

        globals()["run_git"] = clean
        expect_true(resolve_source() == "a" * 40, "the clean source gate did not return its identity")

        # End to end through `run_proof`: a failed status is recorded on the
        # outcome with the fault, no completed/failed case, every case
        # unexecuted, and no scratch or staging effect at all.
        if sys.platform == "darwin":
            canonical = os.path.realpath(gate_root)
            os.environ.update(
                {
                    "OXIDE_TASK_SCRATCH_ROOT": canonical,
                    "OXIDE_PROOF_EVIDENCE": os.path.join(canonical, "evidence"),
                    "TMPDIR": canonical,
                    "TMP": canonical,
                    "TEMP": canonical,
                    "OXIDE_PROOF_DEADLINE_NS": str(
                        time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) + 300 * 10**9
                    ),
                }
            )
            os.makedirs(os.environ["OXIDE_PROOF_EVIDENCE"], mode=0o700, exist_ok=True)
            globals()["run_git"] = lambda arguments: (
                canned(0, "a" * 40 + "\n") if arguments[0] == "rev-parse"
                else canned(1, "", "fatal: not a git repository")
            )
            outcome = diagnostic_outcome()
            try:
                run_proof(True, outcome)
            except UnknownOutcome as error:
                expect_true("worktree status exited 1" in str(error), f"unexpected abort {error}")
            else:
                fail("a failed status did not abort the run")
            expect_true(outcome["aborted"] is not None and "worktree status exited 1" in outcome["aborted"],
                        f"outcome abort {outcome.get('aborted')!r}")
            expect_true(outcome["first_fault"] == outcome["aborted"],
                        f"outcome fault {outcome.get('first_fault')!r}")
            expect_true(outcome["completed"] == [] and outcome["failed"] == [],
                        f"outcome cases {outcome['completed']} / {outcome['failed']}")
            expect_true(outcome["unexecuted"] == [case for case, _, _ in CASES],
                        f"outcome unexecuted {outcome.get('unexecuted')}")
            expect_true(outcome["source"] is None and outcome["root"] is None,
                        f"a refused source recorded {outcome.get('source')!r} / {outcome.get('root')!r}")
            expect_true(os.listdir(gate_root) == ["evidence"],
                        f"the refused source left an effect: {os.listdir(gate_root)}")
    finally:
        globals()["run_git"] = original_git
        os.environ.clear()
        os.environ.update(holder)
        shutil.rmtree(gate_root, ignore_errors=True)

    # A setup failure (before the first case) is recorded as an abort with the
    # fault and every case unexecuted, in a real receipt — never an empty,
    # fault-less document.
    holder = dict(os.environ)
    saved_argv = sys.argv
    saved_run = globals()["run_proof"]
    setup_root = tempfile.mkdtemp(prefix="fd-setup-abort.")
    try:
        os.environ["OXIDE_PROOF_EVIDENCE"] = setup_root

        def failing_setup(diagnostic, outcome):
            outcome["root"] = "/fixture/guard-root"
            raise KeyError("OXIDE_PROOF_DEADLINE_NS")

        globals()["run_proof"] = failing_setup
        sys.argv = ["test-front-door-accounting.py", "--diagnostic"]
        try:
            main()
        except KeyError:
            pass
        else:
            fail("a setup failure did not fail the diagnostic")
        receipts = [name for name in os.listdir(setup_root) if name.startswith(DIAGNOSTIC_RECEIPT)]
        expect_true(len(receipts) == 1, f"setup wrote {len(receipts)} receipts")
        with open(os.path.join(setup_root, receipts[0]), encoding="utf-8") as handle:
            receipt = json.load(handle)
        expect_true(receipt["qualified"] is False, "a setup receipt claimed qualification")
        expect_true(receipt["aborted"] is not None
                    and "OXIDE_PROOF_DEADLINE_NS" in receipt["aborted"],
                    f"setup receipt abort {receipt.get('aborted')!r}")
        expect_true(receipt["first_fault"] == receipt["aborted"],
                    f"setup receipt fault {receipt.get('first_fault')!r}")
        expect_true(receipt["completed"] == [] and receipt["failed"] == [],
                    f"setup receipt cases {receipt['completed']} / {receipt['failed']}")
        expect_true(receipt["unexecuted"] == [case for case, _, _ in CASES],
                    f"setup receipt unexecuted {receipt.get('unexecuted')}")
        expect_true(receipt["root"] == "/fixture/guard-root", f"setup receipt root {receipt.get('root')!r}")
    finally:
        sys.argv = saved_argv
        globals()["run_proof"] = saved_run
        os.environ.clear()
        os.environ.update(holder)
        shutil.rmtree(setup_root, ignore_errors=True)
    print("front-door self-check ok: walk/scope root mapping, exact ordered walk oracles, "
          "summary and provenance parsing, measure line, readiness needle, "
          "premise checklist and fixture/staging environments (private home, rustup, flags, temp, "
          "isolation, drop of every inherited CARGO_/SCCACHE_ value), timeout classification, "
          "case map, keep-going/skip/abort driver, bounded unqualified receipt, "
          "successful-run/failed-receipt and failing-run/failed-receipt paths, dispatch")
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


def run_git(arguments: list) -> subprocess.CompletedProcess:
    """One bounded git call for the source gate."""
    return subprocess.run(
        ["git", *arguments], cwd=REPO, capture_output=True, text=True, timeout=30
    )


def resolve_source() -> str:
    """The exact source identity this run binds to, or an abort.

    Checked before any scratch, staging or child. Both git calls must exit 0,
    their answers must be an exact 40-hex commit id and an empty tracked status,
    and an empty stdout from a failed call is never a clean worktree or an
    identity. A failing or interrupted call, a nonzero exit, an empty or
    non-exact identity and a dirty tracked worktree are outcomes this proof
    cannot classify: they raise [`UnknownOutcome`] and the caller records the
    fault with every planned case unexecuted, before any effect.
    """
    try:
        head = run_git(["rev-parse", "HEAD"])
        status = run_git(["status", "--porcelain", "--untracked-files=no"])
    except (KeyboardInterrupt, SystemExit):
        raise
    except BaseException as failure:
        need(f"source lookup failed: {failure!r}")
    if head.returncode != 0:
        need(
            f"the source lookup exited {head.returncode}: "
            f"{(head.stderr or '').strip()[-400:]!r}"
        )
    if status.returncode != 0:
        need(
            f"the worktree status exited {status.returncode} (its output is not a clean "
            f"worktree): {(status.stderr or '').strip()[-400:]!r}"
        )
    source = (head.stdout or "").strip()
    if not re.fullmatch(r"[0-9a-f]{40}", source):
        need(f"no exact source identity: {source!r}")
    dirty = (status.stdout or "").strip()
    if dirty:
        need(f"the tracked worktree is not clean: {dirty.splitlines()[:4]}")
    return source


def run_proof(diagnostic: bool, outcome: dict) -> int:
    """The one counted or diagnostic run. Mutates `outcome` so the caller can
    write one truthful receipt whatever path ends this run."""
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
    try:
        head = resolve_source()
    except (KeyboardInterrupt, SystemExit):
        record_abort(outcome, "source", "interrupted")
        raise
    except BaseException as failure:
        # Fail closed before any scratch, staging or child: no effects at all,
        # and the fault and every planned case are recorded for the receipt.
        record_abort(outcome, "source", failure)
        print(f"source: ABORTED {failure}", file=sys.stderr)
        raise
    outcome["source"] = head
    reserve(scratch, "admission")
    parent = os.path.realpath(tempfile.mkdtemp(prefix="front-door.", dir=scratch))
    os.chmod(parent, 0o700)
    outcome["root"] = parent
    say(f"source {head}, scratch {parent} inside the guard's known shared root")
    try:
        oxide = stage(parent, staging_environment())
        reserve(parent, "after staging")
        proof = Proof(parent, oxide, fixture_environment(parent))
        checkouts = {}
        for name in ("app", "big"):
            checkouts[name] = os.path.join(parent, "checkouts", name)
            write_fixture(checkouts[name], name)
        run_cases(diagnostic, proof, checkouts, {}, outcome)
    except Exception:
        print(f"front-door accounting proof retained its owned scratch at {parent}", file=sys.stderr)
        raise
    # The run root is NEVER removed here. It sits inside the guard's owned
    # root, and only the guard's post-leader census can establish that no
    # child descendant is still writing; removing the pool, slots and logs
    # before that verdict would destroy the evidence closure has to be judged
    # on. The guard's own retention policy owns cleanup.
    say(f"scratch {parent} retained for the guard's closure census")
    elapsed = time.monotonic() - STARTED
    if elapsed > PROOF_SECONDS + SHUTDOWN_MARGIN_S:
        need(f"front-door accounting proof took {elapsed:.0f} s, over its guard window")
    if diagnostic or outcome["failed"]:
        line = (
            f"front-door diagnostic collected (UNCOUNTED, never a PASS): "
            f"{len(outcome['completed'])}/{len(CASES)} cases completed, failed "
            f"{[case for case, _ in outcome['failed']]}, skipped "
            f"{[case for case, _ in outcome['skipped']]}, unexecuted {outcome['unexecuted']}; "
            f"{elapsed:.0f} s of script time, sampled peak owned scratch "
            f"{PEAK['bytes'] // MIB} MiB ({PEAK['phase']}), scratch retained for the guard"
        )
    else:
        line = (
            f"oxide front-door accounting proof held: source {head}, {elapsed:.0f} s of script "
            f"time, peak owned scratch sampled {PEAK['bytes'] // MIB} MiB ({PEAK['phase']}), "
            f"scratch retained for the guard"
        )
    print(line)
    return 0


def finish_run(code: int, diagnostic: bool, outcome: dict, writer=None, failed: bool = False) -> int:
    """The one place a diagnostic's receipt is written.

    A diagnostic that ran to completion cannot report success without its
    receipt: `writer` raising on the success path propagates, so `main` never
    returns 0 for a run whose receipt could not be written. A run that is
    already failing keeps its own error and only prints the receipt failure, so
    the receipt path never masks the fault that ended the collection. A counted
    run writes no receipt at all.
    """
    if not diagnostic:
        return code
    writer = diagnostic_receipt if writer is None else writer

    def write(best_effort: bool) -> None:
        try:
            print(f"front-door diagnostic receipt {writer(outcome)}")
        except BaseException as error:
            print(f"front-door diagnostic receipt not written: {error!r}", file=sys.stderr)
            if not best_effort:
                raise

    write(best_effort=failed)
    return code


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
    outcome = diagnostic_outcome()
    try:
        code = run_proof(diagnostic, outcome)
    except BaseException as failure:
        if diagnostic:
            record_abort(outcome, "run", failure)
            finish_run(0, True, outcome, failed=True)
        raise
    return finish_run(code, diagnostic, outcome)


if __name__ == "__main__":
    sys.exit(main())
