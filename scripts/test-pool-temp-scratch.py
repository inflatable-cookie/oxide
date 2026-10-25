#!/usr/bin/env python3
"""Focused native proof for child temporary files on the selected pool (oxide#136).

One fixed script under the qualified shared supervisor (`bounded-test.py
--limit 300 --scratch-budget-mib 512`), and the whole command of that
supervisor: the supervisor owns the absolute deadline, the 20 GiB reserve
watchdog, the monitored scratch and cleanup, and closure; this script keeps no
clock, sampler or session of its own. The brief's tier is ELEVATED (it deletes
owned scratch, decides writer closure and writes to an owned removable fixture
volume).

It stages this clean HEAD's debug `oxide` with plain Cargo and the pinned
1.97.1, offline, into its own scratch target, copies the binary out and
deletes the target before the fixtures. Every fixture is a tiny std-only
crate: a build script, a test and a `run` program report `std::env::temp_dir()`
and the three alias variables, and write a marker into that directory.

Cases (each one is a real child, nothing is simulated):

1. premise and oracle sweep: the observation method is proved against plain
   `cargo` first, then `oxide --version` and an effect-free pass-through are
   shown to allocate nothing; the script's own `TMPDIR` is not inside the
   fixture pool (a void premise fails).
2. same-volume inherited `TMPDIR` kept: the child reports the canonical
   directory, no Oxide directory is created, and a planted file inside it is
   untouched after the command.
3. absent, file, relative, unwritable and symlink-escaping inherited
   directories are replaced by one owned directory; an other-device value is
   replaced too when this host has a writable directory on another device, and
   the case says so instead of asserting a control the fixture cannot build.
4. an existing **relative** `TMPDIR` resolves against the parent's working
   directory and reaches the child as the canonical absolute path, so a child
   that changes directory still uses the qualified directory; a relative
   value that does not resolve is replaced.
5. aliases: `TMPDIR`, `TMP` and `TEMP` all follow the chosen value, never the
   caller's conflicting values.
6. the owned directory is observed on the selected pool while a gated writer
   holds it; `status --measure` counts a hard-linked pair once (asserted
   against the measured slot figure, not a vacuous bound); the writer's marker
   is inside it.
7. normal release removes it; the released slot keeps none.
8. a crash with a surviving writer group keeps the directory and the slot; the
   proof observes that group's closure the way the product does (`kill(-pgid,
   0)` on the pgid the fixture reports), then a later command must neither adopt
   nor keep the dead command's leftover: this slot's same-incarnation directory
   is reconciled by its next lease, or the untrusted slot is evicted whole —
   both existing rules, and the case asserts the outcome without assuming
   which one runs.
9. `oxide test`, `oxide exec -- <tool>` and `oxide run` children each observe
   the chosen directory: a test binary asserts the prefix it was given, the
   fake exec tool records `$TMPDIR`, and the run program reports and writes
   into it. A run program using an owned directory runs under the existing
   held-lease custody, so its directory is removed only after its group has
   closed; a **live** program plus a competing lease in the same checkout is
   the counterexample case: the competing command takes another slot and the
   live directory and its bytes stay intact until the program ends.
10. a foreign entry and a link in the scratch position refuse the command
    without following, deleting or spawning anything, and the entry is kept;
    an over-bound derived path refuses the command rather than leaving the
    child on the machine default.
11. one owned 128 MiB scratch APFS image registered as the removable pool: the
    child's temporary files land on the image, the inherited fixture scratch
    (another device) is replaced, release removes the directory, and after an
    ordinary detach the same registration's selection falls back to the
    internal pool, whose own scratch is used with no stale `/Volumes/<name>`
    directory created. With an inherited same-device `TMPDIR`, the fallback
    keeps it and creates nothing.
12. the longest injected path is reported in bytes and characters against the
    91-byte socket bound.

`--plan` prints the allocation and cases and touches nothing. No live pool,
registered drive, install receipt, host setting or consumer is touched; the
fixture image is created, attached and ordinarily detached inside the
supervisor's scratch.
"""

from __future__ import annotations

import hashlib
import os
import plistlib
import re
import shutil
import subprocess
import sys
import time
from typing import Optional

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILDER = "1.97.1"
# macOS `sun_path`, including the NUL, less `/` and an eleven-byte socket name.
SOCKET_PATH_BOUND = 91
MIB = 1024 * 1024
IMAGE_MIB = 128
SHUTDOWN_MARGIN_S = 30
CASES: list[str] = []
STARTED = time.perf_counter()


def plan() -> None:
    print(
        """pool-temp-scratch proof, planned allocation (unmeasured):
  supervisor: bounded-test.py --limit 300 --scratch-budget-mib 512
  work stops before the supervisor's 30 s shutdown margin
  staging: plain Cargo, pinned 1.97.1, offline, 1 job, target deleted after
  fixtures: one private HOME, CARGO_HOME, OXIDE_POOL and tiny std-only crates
  removable: one owned 128 MiB APFS scratch image at a unique /Volumes/<name>,
             ordinary detach only, never a real drive
  cases:"""
    )
    for index, name in enumerate(CASE_NAMES, start=1):
        print(f"    {index}. {name}")
    print("  writes only inside OXIDE_TASK_SCRATCH_ROOT; no live pool is touched")


CASE_NAMES = [
    "premise and oracle sweep",
    "same-volume inherited TMPDIR kept canonically",
    "unusable inherited TMPDIR replaced",
    "existing relative TMPDIR resolved canonically",
    "aliases follow the chosen value",
    "owned scratch observed while held, hard link counted once",
    "normal release removes the owned scratch",
    "crash with a surviving group keeps it; the leftover never survives closure",
    "test, exec and run children observe the chosen directory",
    "foreign, linked and over-bound positions refuse",
    "removable image and absent-volume internal fallback",
    "longest injected path against the socket bound",
]


def fail(message: str) -> None:
    print(f"pool-temp-scratch proof failed: {message}", file=sys.stderr)
    sys.exit(1)


def expect(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def say(message: str) -> None:
    print(f"[{time.perf_counter() - STARTED:7.1f}s] {message}", flush=True)


def case(name: str) -> None:
    CASES.append(name)
    say(f"case {name}")


def work_left() -> float:
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    left = (
        deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)
    ) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 5:
        fail("work deadline reached before the supervisor's shutdown margin")
    return left


def scratch_root() -> str:
    root = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
    expect(bool(root), "OXIDE_TASK_SCRATCH_ROOT is required")
    return os.path.realpath(root)


def private_dir(path: str) -> str:
    os.makedirs(path, exist_ok=True)
    os.chmod(path, 0o700)
    return path


def write_private(path: str, data: str, mode: int = 0o600) -> None:
    private_dir(os.path.dirname(path))
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(data)
    os.chmod(path, mode)


def sha256_file(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def children_bytes(path: str) -> int:
    total = 0
    for root, _, files in os.walk(path):
        for name in files:
            try:
                total += os.lstat(os.path.join(root, name)).st_size
            except OSError:
                pass
    return total


def stage_subject(root: str) -> str:
    """Stage this HEAD's debug `oxide` offline, then delete the target."""
    stage = os.path.join(root, "stage")
    log = os.path.join(root, "stage.log")
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("CARGO_TARGET_", "CARGO_BUILD_", "SCCACHE_"))
        and key not in ("CARGO", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER")
    }
    env.update(
        {
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(stage, "target"),
            "CARGO_BUILD_BUILD_DIR": os.path.join(stage, "build"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_PROFILE_DEV_OPT_LEVEL": "s",
            "CARGO_PROFILE_DEV_CODEGEN_UNITS": "1",
            "CARGO_PROFILE_DEV_PANIC": "abort",
            "CARGO_INCREMENTAL": "0",
            "RUSTFLAGS": "-C strip=symbols",
        }
    )
    with open(log, "w", encoding="utf-8") as handle:
        build = subprocess.run(
            ["cargo", "build", "-j", "1", "--locked", "--offline", "--bin", "oxide"],
            cwd=REPO,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            timeout=work_left(),
        )
    if build.returncode:
        with open(log, encoding="utf-8") as handle:
            fail(f"staging exit {build.returncode}: {handle.read()[-2000:]}")
    built = os.path.join(stage, "target", "debug", "oxide")
    expect(os.path.isfile(built), f"staging produced no binary at {built}")
    subject = os.path.join(root, "subject-oxide")
    shutil.copy2(built, subject)
    os.chmod(subject, 0o755)
    shutil.rmtree(stage, ignore_errors=True)
    os.unlink(log)
    return subject


# ---------------------------------------------------------------------------
# Fixtures: a tiny std-only crate whose child processes report their temp dir.

BUILD_SCRIPT = r'''
use std::io::Write;

extern "C" {
    // The build script's own process group: Cargo's group, because Cargo is
    // this command's group leader. The proof uses it to observe closure from
    // outside, exactly as the product does.
    fn getpgrp() -> i32;
}

fn main() {
    let dir = std::env::temp_dir();
    for name in ["TMPDIR", "TMP", "TEMP"] {
        println!(
            "cargo:warning=OXIDE-TEMP {name}={}",
            std::env::var(name).unwrap_or_else(|_| "<unset>".to_owned())
        );
    }
    println!("cargo:warning=OXIDE-TEMP dir={}", dir.display());
    // SAFETY: getpgrp has no preconditions.
    println!("cargo:warning=OXIDE-TEMP pgid={}", unsafe { getpgrp() });
    if let Ok(target) = std::env::var("FIXTURE_CHDIR") {
        // A child that changes directory must still use the directory it was
        // given. A raw relative value would put this marker under the new
        // working directory instead.
        std::env::set_current_dir(&target).expect("chdir");
        let after = std::env::temp_dir();
        println!("cargo:warning=OXIDE-TEMP dir-after-chdir={}", after.display());
        std::fs::write(after.join("after-chdir"), b"after").expect("after-chdir");
    }
    if let Ok(gate) = std::env::var("FIXTURE_HOLD") {
        // A gated writer: it announces itself, writes into the temporary
        // directory and waits for the release file the proof creates.
        let marker = dir.join("held-marker");
        let mut file = std::fs::File::create(&marker).expect("marker");
        file.write_all(&vec![b'x'; 1 << 20]).expect("write");
        drop(file);
        std::os::unix::fs::symlink("held-marker", dir.join("held-link")).expect("link");
        let clobber = dir.join("clobber");
        let _ = std::fs::hard_link(dir.join("held-marker"), &clobber);
        println!("cargo:warning=OXIDE-TEMP holding");
        while !std::path::Path::new(&gate).exists() {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::fs::write(dir.join("released-marker"), b"released").expect("released");
        println!("cargo:warning=OXIDE-TEMP released");
    }
}
'''

MAIN_RS = r'''
fn main() {
    let dir = std::env::temp_dir();
    println!("OXIDE-TEMP run={}", dir.display());
    let _ = std::fs::write(dir.join("run-marker"), b"run");
    if let Ok(gate) = std::env::var("RUN_HOLD") {
        // A live program: it announces itself inside its chosen temporary
        // directory and stays a writer until the proof releases it.
        std::fs::write(dir.join("program-holding"), b"live").expect("holding");
        while !std::path::Path::new(&gate).exists() {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::fs::write(dir.join("program-released"), b"closed").expect("released");
    }
}
'''

TEST_RS = r'''
#[test]
fn reports_the_child_temporary_directory() {
    let dir = std::env::temp_dir();
    println!("OXIDE-TEMP test={}", dir.display());
    std::fs::write(dir.join("test-marker"), b"test").expect("marker");
    if let Ok(prefix) = std::env::var("TEMP_EXPECT_PREFIX") {
        assert!(
            dir.starts_with(&prefix),
            "the test binary's temporary directory {} is not under {}",
            dir.display(),
            prefix
        );
    }
}
'''

MANIFEST = """[package]
name = "tempcheck"
version = "0.1.0"
edition = "2021"

[workspace]
"""


def make_fixture(root: str, name: str) -> str:
    crate = private_dir(os.path.join(root, name))
    write_private(os.path.join(crate, "Cargo.toml"), MANIFEST)
    write_private(os.path.join(crate, "build.rs"), BUILD_SCRIPT)
    write_private(os.path.join(crate, "src", "main.rs"), MAIN_RS)
    write_private(os.path.join(crate, "src", "lib.rs"), "")
    write_private(os.path.join(crate, "tests", "temp.rs"), TEST_RS)
    return crate


class World:
    """The private account, pool, Cargo home and checkout of one proof run."""

    def __init__(self, root: str, subject: str) -> None:
        self.root = root
        self.subject = subject
        self.home = private_dir(os.path.join(root, "home"))
        self.cargo_home = private_dir(os.path.join(root, "cargo-home"))
        self.pool = private_dir(os.path.join(root, "pool"))
        self.scratch = private_dir(os.path.join(root, "fixture-scratch"))
        self.base = {
            "PATH": os.environ["PATH"],
            "HOME": self.home,
            "CARGO_HOME": self.cargo_home,
            "CARGO_NET_OFFLINE": "true",
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "TMPDIR": self.scratch,
            "TMP": self.scratch,
            "TEMP": self.scratch,
            "OXIDE_POOL": self.pool,
            "OXIDE_POOL_CAP": "64MiB",
        }

    def env(self, **overrides: Optional[str]) -> dict:
        env = dict(self.base)
        for key, value in overrides.items():
            if value is None:
                env.pop(key, None)
            else:
                env[key] = value
        return env

    def run(
        self,
        args: list[str],
        *,
        cwd: str,
        env: dict,
        timeout: Optional[float] = None,
        check: bool = True,
    ) -> subprocess.CompletedProcess:
        left = work_left()
        result = subprocess.run(
            [self.subject, *args],
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
            timeout=min(left, timeout or left),
        )
        if check and result.returncode != 0:
            fail(
                f"`oxide {' '.join(args)}` exit {result.returncode}: "
                f"{result.stderr[-2000:]}"
            )
        return result

    def slot_roots(self) -> list[str]:
        slots = os.path.join(self.pool, "payload", "slots")
        if not os.path.isdir(slots):
            return []
        found = []
        for root, dirs, _ in os.walk(slots):
            if os.path.basename(root) == "build":
                dirs[:] = []
                found.append(os.path.dirname(root))
        return found

    def scratch_dirs(self) -> list[str]:
        return [os.path.join(slot, "scratch") for slot in self.slot_roots()]

    def slot_bytes(self) -> int:
        return sum(children_bytes(slot) for slot in self.slot_roots())


def reported(stderr: str, kind: str) -> str:
    marker = f"OXIDE-TEMP {kind}="
    for line in stderr.splitlines():
        if marker in line:
            return line.split(marker, 1)[1].strip()
    fail(f"no {kind} observation in the child output: {stderr[-2000:]}")


def reported_aliases(stderr: str) -> dict:
    return {
        name: reported(stderr, name) for name in ("TMPDIR", "TMP", "TEMP")
    }


# ---------------------------------------------------------------------------


def held_by_pool(world: World, path: str) -> None:
    real = os.path.realpath(path)
    pool = os.path.realpath(world.pool)
    expect(real.startswith(pool + os.sep), f"{real} is not inside the pool {pool}")


def premise_sweep(world: World, crate: str) -> None:
    case(CASE_NAMES[0])
    # The script's own TMPDIR is the fixture scratch, never inside the pool:
    # otherwise every later "kept" assertion would be a void premise.
    expect(
        not os.path.realpath(world.scratch).startswith(
            os.path.realpath(world.pool) + os.sep
        ),
        "the fixture scratch is inside the pool, so the premise is void",
    )
    version = world.run(["--version"], cwd=crate, env=world.env())
    expect(
        "oxide" in version.stdout + version.stderr,
        f"`oxide --version` printed no identity: {version.stdout}{version.stderr}",
    )
    # Oracle: plain Cargo's build script reports the inherited directory, so
    # the observation method really sees the child environment.
    plain_env = world.env()
    plain_env.pop("OXIDE_POOL", None)
    plain = subprocess.run(
        ["cargo", "check", "--offline", "--locked"],
        cwd=crate,
        env=plain_env,
        capture_output=True,
        text=True,
        timeout=work_left(),
    )
    expect(
        plain.returncode == 0,
        f"plain cargo premise failed: {plain.stderr[-2000:]}",
    )
    expect(
        reported(plain.stderr, "dir") == os.path.realpath(world.scratch)
        or reported(plain.stderr, "dir").rstrip("/")
        == os.path.realpath(world.scratch),
        f"plain cargo did not report the inherited TMPDIR: {plain.stderr[-1000:]}",
    )
    # Effect-free commands allocate nothing.
    before = len(world.slot_roots())
    world.run(["metadata", "--format-version", "1"], cwd=crate, env=world.env())
    world.run(["fmt", "--", "--check"], cwd=crate, env=world.env(), check=False)
    expect(
        len(world.slot_roots()) == before,
        "an effect-free command created a slot",
    )


def canonical(path: str) -> str:
    return os.path.realpath(path)


def slot_id_of(slot: str) -> str:
    return os.path.basename(slot).rsplit("-", 1)[-1]


def measured_slot_bytes(text: str, slot_id: str) -> int:
    """The measured allocated figure for one slot, from `status --measure`."""
    for line in text.splitlines():
        if not re.match(rf"\s+slot {re.escape(slot_id)} ", line):
            continue
        match = re.search(r"([0-9]+) (KiB|MiB) allocated", line)
        expect(match is not None, f"no measured figure in {line!r}")
        value = int(match.group(1))
        return value * (1024 if match.group(2) == "KiB" else 1024 * 1024)
    fail(f"no measured line for slot {slot_id} in the status output")


def inherited_kept(world: World, crate: str) -> None:
    case(CASE_NAMES[1])
    inherited = private_dir(os.path.join(world.pool, "inherited-tmp"))
    keep = os.path.join(inherited, "effigy-evidence")
    write_private(keep, "retained")
    result = world.run(
        ["check"], cwd=crate, env=world.env(TMPDIR=inherited, TMP=None, TEMP=None)
    )
    observed = reported(result.stderr, "dir")
    expect(
        observed.rstrip("/") == canonical(inherited),
        f"the child did not keep the canonical inherited directory: {observed}",
    )
    expect(
        not any(os.path.exists(path) for path in world.scratch_dirs()),
        "Oxide created its own directory although the inherited one was kept",
    )
    expect(
        open(keep, encoding="utf-8").read() == "retained",
        "the inherited directory's content changed",
    )
    expect(
        "temp: inherited" in result.stderr,
        f"the summary did not name the inherited directory: {result.stderr[-1000:]}",
    )
    summary = [line for line in result.stderr.splitlines() if "temp: inherited" in line][-1]
    expect(
        len(summary.encode()) < 4096,
        "the temp fragment is not one bounded line",
    )
    expect(
        canonical(inherited) in summary,
        f"the summary did not name the canonical directory: {summary}",
    )


def replaced(world: World, crate: str) -> None:
    case(CASE_NAMES[2])
    missing = os.path.join(world.pool, "missing-tmp")
    closed = private_dir(os.path.join(world.pool, "closed-tmp"))
    os.chmod(closed, 0o500)
    file_path = os.path.join(world.pool, "not-a-dir")
    write_private(file_path, "file")
    escape = os.path.join(world.root, "escape-link")
    if os.path.lexists(escape):
        os.unlink(escape)
    os.symlink(os.path.join(world.root, "absent-target"), escape)
    # A directory on another device is only a control when one exists on this
    # host: the fixture scratch is usually a sibling of the directory pool and
    # therefore on the same device, where the inherited directory is correctly
    # kept. The image case proves the other-device rule against a real second
    # volume.
    pool_device = os.stat(canonical(world.pool)).st_dev
    other_device = None
    for candidate in (world.scratch, os.environ.get("TMPDIR"), "/private/tmp"):
        if not candidate or not os.path.isdir(candidate):
            continue
        if os.stat(canonical(candidate)).st_dev != pool_device:
            other_device = candidate
            break
    cases = [
        ("absent", missing),
        ("file", file_path),
        ("relative", "relative/tmp"),
        ("unwritable", closed),
        ("symlink escape", escape),
    ]
    if other_device is None:
        say(
            "no writable directory on another device here; the other-device "
            "rule is exercised against the owned image in the removable case"
        )
    else:
        cases.append(("other device", other_device))
    try:
        for label, value in cases:
            result = world.run(["check"], cwd=crate, env=world.env(TMPDIR=value))
            observed = reported(result.stderr, "dir")
            held_by_pool(world, observed)
            expect(
                os.path.basename(observed.rstrip("/")) == "scratch",
                f"{label}: the child did not use the owned scratch: {observed}",
            )
            expect(
                "temp: oxide" in result.stderr,
                f"{label}: the summary did not name the owned scratch",
            )
            expect(
                not os.path.exists(observed.rstrip("/")),
                f"{label}: the owned directory survived a healthy release",
            )
        # The unwritable inherited directory itself is never removed.
        expect(os.path.isdir(closed), "Oxide removed an inherited directory")
    finally:
        os.chmod(closed, 0o700)


def relative_inherited(world: World, crate: str) -> None:
    case(CASE_NAMES[3])
    relative = private_dir(os.path.join(crate, "rel-tmp"))
    keep = os.path.join(relative, "keep")
    write_private(keep, "relative")
    # The parent's working directory is the crate, so `rel-tmp` names the
    # fixture directory. A child that changed directory would resolve the raw
    # text somewhere else; the canonical absolute path cannot.
    result = world.run(
        ["check"], cwd=crate, env=world.env(TMPDIR="rel-tmp", TMP=None, TEMP=None)
    )
    observed = reported(result.stderr, "dir")
    expect(
        os.path.isabs(observed),
        f"the child was given a relative path: {observed}",
    )
    expect(
        observed.rstrip("/") == canonical(relative),
        f"the relative TMPDIR was not resolved canonically: {observed}",
    )
    expect(
        open(keep, encoding="utf-8").read() == "relative",
        "the relative inherited directory's content changed",
    )
    expect(
        not any(os.path.exists(path) for path in world.scratch_dirs()),
        "Oxide created its own directory for a relative inherited value",
    )
    expect(
        "temp: inherited" in result.stderr,
        "the summary did not name the relative inherited directory",
    )
    # The counterexample the rule exists for: the same relative `TMPDIR`, but
    # the child changes its working directory before using it. The value it
    # was given is absolute, so the write lands in the qualified directory and
    # not under the new working directory.
    chdir_target = private_dir(os.path.join(crate, "chdir-target"))
    chdir_run = world.run(
        ["check"],
        cwd=crate,
        env=world.env(
            TMPDIR="rel-tmp",
            TMP=None,
            TEMP=None,
            FIXTURE_CHDIR=chdir_target,
        ),
    )
    expect(
        reported(chdir_run.stderr, "dir-after-chdir").rstrip("/") == canonical(relative),
        f"the child re-resolved the relative value after chdir: {chdir_run.stderr[-1500:]}",
    )
    expect(
        os.path.isfile(os.path.join(relative, "after-chdir")),
        "the child's post-chdir write did not land in the qualified directory",
    )
    expect(
        not os.path.exists(os.path.join(chdir_target, "after-chdir")),
        "the child's post-chdir write followed a relative value",
    )
    # A relative value that does not resolve anywhere is replaced.
    replaced_run = world.run(
        ["check"],
        cwd=crate,
        env=world.env(TMPDIR="missing-rel", TMP=None, TEMP=None),
    )
    observed = reported(replaced_run.stderr, "dir")
    held_by_pool(world, observed)
    expect(
        os.path.basename(observed.rstrip("/")) == "scratch",
        f"an unresolved relative value was not replaced: {observed}",
    )


def aliases(world: World, crate: str) -> None:
    case(CASE_NAMES[4])
    inherited = private_dir(os.path.join(world.pool, "alias-tmp"))
    env = world.env(
        TMPDIR=inherited,
        TMP=os.path.join(world.root, "elsewhere-tmp"),
        TEMP=os.path.join(world.root, "third-tmp"),
    )
    result = world.run(["check"], cwd=crate, env=env)
    names = reported_aliases(result.stderr)
    expect(
        names["TMPDIR"] == names["TMP"] == names["TEMP"],
        f"the aliases did not follow one chosen value: {names}",
    )
    expect(
        names["TMPDIR"].rstrip("/") == canonical(inherited),
        f"the aliases did not follow the inherited TMPDIR: {names}",
    )
    # The same, in the owned branch.
    owned = world.run(
        ["check"],
        cwd=crate,
        env=world.env(
            TMPDIR=None,
            TMP=os.path.join(world.root, "elsewhere-tmp"),
            TEMP=os.path.join(world.root, "third-tmp"),
        ),
    )
    names = reported_aliases(owned.stderr)
    expect(
        names["TMPDIR"] == names["TMP"] == names["TEMP"],
        f"the aliases did not follow the owned directory: {names}",
    )
    held_by_pool(world, names["TMPDIR"])


def accounting(world: World, crate: str) -> None:
    case(CASE_NAMES[5])
    gate = os.path.join(world.root, "release")
    if os.path.exists(gate):
        os.unlink(gate)
    env = world.env(TMPDIR=None, TMP=None, TEMP=None, FIXTURE_HOLD=gate)
    child = subprocess.Popen(
        [world.subject, "check"],
        cwd=crate,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    scratch = ""
    try:
        expect(child.stderr is not None, "no stderr pipe")
        holding = False
        lines: list[str] = []
        deadline = time.monotonic() + min(work_left(), 120)
        while time.monotonic() < deadline:
            line = child.stderr.readline()
            if not line:
                break
            lines.append(line)
            if "OXIDE-TEMP holding" in line:
                holding = True
                break
        expect(holding, "the gated writer never reported holding the slot")
        observed = reported("".join(lines), "dir")
        held_by_pool(world, observed)
        scratch = observed.rstrip("/")
        expect(
            os.path.basename(scratch) == "scratch",
            f"the unset TMPDIR did not become the owned scratch: {observed}",
        )
        expect(
            os.path.isfile(os.path.join(scratch, "held-marker")),
            "the writer's marker is not inside the owned directory",
        )
        # 16 MiB of unique data plus a hard link to one 8 MiB file: a walk
        # that counted the link twice would measure at least 24 MiB.
        for name in ("first", "second"):
            with open(os.path.join(scratch, name), "wb") as handle:
                handle.write(b"x" * (8 * MIB))
        os.link(os.path.join(scratch, "first"), os.path.join(scratch, "first-link"))
        slot = os.path.dirname(scratch)
        measured = world.run(
            ["status", "--measure"], cwd=crate, env=world.env(), check=False
        )
        expect(
            measured.returncode == 0,
            f"status --measure failed: {measured.stderr[-1500:]}",
        )
        expect(
            "temp: oxide" in measured.stdout,
            f"measured status did not name the pool scratch: {measured.stdout[-1500:]}",
        )
        figure = measured_slot_bytes(measured.stdout, slot_id_of(slot))
        say(f"measured slot {slot_id_of(slot)}: {figure} bytes for 16 MiB unique")
        expect(
            figure >= 16 * MIB,
            f"the measured figure {figure} is below the 16 MiB written",
        )
        expect(
            figure < 24 * MIB,
            f"the hard-linked pair was counted twice ({figure} bytes for 16 MiB unique)",
        )
        expect(
            not os.path.exists(os.path.join(scratch, "first-link")) is False,
            "the hard link is missing",
        )
        # The measured status reconciled nothing.
        expect(
            os.path.isfile(os.path.join(scratch, "held-marker")),
            "status --measure removed the held scratch",
        )
    finally:
        write_private(gate, "")
        try:
            child.wait(timeout=min(work_left(), 60))
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=10)
    expect(
        child.returncode == 0,
        f"the released command failed with {child.returncode}",
    )
    expect(
        not os.path.exists(scratch),
        "normal release did not remove the owned directory",
    )


def group_is_empty(pgid: int) -> bool:
    try:
        os.kill(-pgid, 0)
    except ProcessLookupError:
        return True
    except PermissionError:
        return False
    return False


def crash_and_recovery(world: World, crate: str) -> None:
    case(CASE_NAMES[7])
    gate = os.path.join(world.root, "crash-release")
    if os.path.exists(gate):
        os.unlink(gate)
    env = world.env(TMPDIR=None, TMP=None, TEMP=None, FIXTURE_HOLD=gate)
    child = subprocess.Popen(
        [world.subject, "check"],
        cwd=crate,
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    scratch = ""
    slot = ""
    pgid = 0
    try:
        expect(child.stderr is not None, "no stderr pipe")
        lines: list[str] = []
        deadline = time.monotonic() + min(work_left(), 120)
        while time.monotonic() < deadline:
            line = child.stderr.readline()
            if not line:
                break
            lines.append(line)
            if "OXIDE-TEMP holding" in line:
                break
        text = "".join(lines)
        scratch = reported(text, "dir").rstrip("/")
        held_by_pool(world, scratch)
        expect(os.path.isdir(scratch), "no owned directory while holding")
        slot = os.path.dirname(scratch)
        pgid = int(reported(text, "pgid"))
        expect(pgid > 0, f"the fixture reported no process group: {text[-500:]}")
        # Kill only Oxide itself: its writer group survives, so closure is
        # unknown and the directory must stay with the held slot.
        child.kill()
        child.wait(timeout=min(work_left(), 30))
        expect(
            os.path.isdir(scratch),
            "the owned directory was removed while its writer survived",
        )
        expect(os.path.isdir(slot), "the held slot was deleted")
        # A leftover this dead command could never reconcile itself.
        leftover = os.path.join(scratch, "proof-leftover")
        write_private(leftover, "left behind by the interrupted command")
        # Positive closure of the writer group, observed the way the product
        # observes it: the recorded group is gone. A dead `oxide` removes
        # nothing, so nothing here waits for it to.
        write_private(gate, "")
        deadline = time.monotonic() + min(work_left(), 60)
        while time.monotonic() < deadline and not group_is_empty(pgid):
            time.sleep(0.1)
        expect(
            group_is_empty(pgid),
            f"the interrupted command's group {pgid} did not close",
        )
        expect(
            os.path.isdir(scratch) and os.path.isfile(leftover),
            "the interrupted command's directory changed before any recovery",
        )
        # The next command succeeds and never adopts the dead command's
        # leftover: either this slot's same-incarnation directory is
        # reconciled by its new lease, or the untrusted slot is evicted whole.
        # Both are the existing product rules; the assertion does not assume
        # which one runs.
        after = world.run(
            ["check"], cwd=crate, env=world.env(TMPDIR=None, TMP=None, TEMP=None)
        )
        observed = reported(after.stderr, "dir").rstrip("/")
        held_by_pool(world, observed)
        expect(
            not os.path.exists(leftover),
            "the interrupted command's leftover survived the next command",
        )
        if not os.path.isdir(slot):
            say("the untrusted interrupted slot was evicted whole, leftover included")
        else:
            expect(
                os.path.dirname(observed) == slot,
                "the interrupted slot was neither reused nor evicted",
            )
        expect(
            not os.path.exists(observed),
            "the next command kept an owned directory after its own release",
        )
    finally:
        write_private(gate, "")
        if child.poll() is None:
            child.kill()
            child.wait(timeout=10)


def spawns(world: World, crate: str) -> None:
    case(CASE_NAMES[8])
    # An inherited directory reaches every child and is never cleaned.
    inherited = private_dir(os.path.join(world.pool, "spawn-tmp"))
    env = world.env(TMPDIR=inherited, TMP=None, TEMP=None)
    tested = world.run(["test"], cwd=crate, env=dict(env, TEMP_EXPECT_PREFIX=inherited))
    expect(
        "test result: ok" in tested.stderr,
        f"oxide test did not pass: {tested.stderr[-1500:]}",
    )
    tool = world.run(
        ["exec", "--", "/bin/sh", "-c", 'printf "%s\\n%s\\n%s" "$TMPDIR" "$TMP" "$TEMP"'],
        cwd=crate,
        env=env,
    )
    lines = [line.strip() for line in tool.stdout.splitlines() if line.strip()]
    expect(
        {line.rstrip("/") for line in lines} == {canonical(inherited)},
        f"the exec tool did not inherit the chosen directory: {lines}",
    )
    ran = world.run(["run"], cwd=crate, env=env)
    expect(
        reported(ran.stdout, "run").rstrip("/") == canonical(inherited),
        f"the run program did not inherit the chosen directory: {ran.stdout[-1000:]}",
    )
    expect(
        os.path.isdir(inherited),
        "the inherited directory was removed after a run program",
    )
    expect(
        not any(os.path.exists(path) for path in world.scratch_dirs()),
        "an inherited-only child tree still created an owned directory",
    )

    # The owned branch reaches the same children. The program is still the
    # directory's writer, so it runs under the held lease and the directory is
    # removed only after its group has closed.
    owned_env = world.env(TMPDIR=None, TMP=None, TEMP=None)
    prefix = os.path.join(os.path.realpath(world.pool), "payload")
    world.run(["test"], cwd=crate, env=dict(owned_env, TEMP_EXPECT_PREFIX=prefix))
    tool = world.run(
        ["exec", "--", "/bin/sh", "-c", 'printf "%s" "$TMPDIR"'],
        cwd=crate,
        env=owned_env,
    )
    held_by_pool(world, tool.stdout.strip())
    ran = world.run(["run"], cwd=crate, env=owned_env)
    observed = reported(ran.stdout, "run").rstrip("/")
    held_by_pool(world, observed)
    expect(
        not os.path.exists(observed),
        "the run program's owned directory survived its own release",
    )

    # A live program plus a competing lease: the lease fences the owned
    # directory from reconciliation and eviction, so a further command in the
    # same checkout must take another slot and leave the live directory and
    # its bytes intact. Same-incarnation ownership alone is not closure.
    gate = os.path.join(world.root, "run-release")
    if os.path.exists(gate):
        os.unlink(gate)
    live = subprocess.Popen(
        [world.subject, "run"],
        cwd=crate,
        env=dict(owned_env, RUN_HOLD=gate),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    live_dir = ""
    try:
        deadline = time.monotonic() + min(work_left(), 120)
        while time.monotonic() < deadline:
            for candidate in world.scratch_dirs():
                if os.path.isfile(os.path.join(candidate, "program-holding")):
                    live_dir = candidate
                    break
            if live_dir or live.poll() is not None:
                break
            time.sleep(0.1)
        expect(
            bool(live_dir),
            "the live run program never announced itself in its owned directory",
        )
        held_by_pool(world, live_dir)
        holding = os.path.join(live_dir, "program-holding")
        before = os.path.getsize(holding)
        competing = world.run(["check"], cwd=crate, env=owned_env)
        competing_dir = reported(competing.stderr, "dir").rstrip("/")
        expect(
            competing_dir != live_dir,
            "the competing command reused the live program's slot",
        )
        expect(
            os.path.isdir(live_dir),
            "a competing lease removed the live program's owned directory",
        )
        expect(
            os.path.getsize(holding) >= before,
            "the live program's bytes changed",
        )
        # Positive closure: the program ends, its group is observed empty, and
        # only then is the directory removed.
        write_private(gate, "")
        try:
            live.wait(timeout=min(work_left(), 120))
        except subprocess.TimeoutExpired:
            live.kill()
            live.wait(timeout=10)
            fail("the released run program did not exit")
        expect(live.returncode == 0, f"the run program exited {live.returncode}")
        expect(
            not os.path.exists(live_dir),
            "the owned directory survived the program's positive closure",
        )
    finally:
        write_private(gate, "")
        if live.poll() is None:
            live.kill()
            live.wait(timeout=10)


def refusals(world: World, crate: str) -> None:
    case(CASE_NAMES[9])
    outside = private_dir(os.path.join(world.root, "outside"))
    keep = os.path.join(outside, "keep")
    write_private(keep, "foreign")
    world.run(["check"], cwd=crate, env=world.env(TMPDIR=None, TMP=None, TEMP=None))
    slot = world.slot_roots()[0]
    scratch = os.path.join(slot, "scratch")
    # A link in the owned position: refuse, never follow, keep the target.
    os.symlink(outside, scratch)
    try:
        refused = world.run(
            ["check"],
            cwd=crate,
            env=world.env(TMPDIR=None, TMP=None, TEMP=None),
            check=False,
        )
        expect(
            refused.returncode != 0,
            "Oxide accepted a link in the owned position",
        )
        expect(
            "real directory" in refused.stderr,
            f"the linked position was not named: {refused.stderr[-1500:]}",
        )
        expect(
            "OXIDE-TEMP dir=" not in refused.stderr,
            "a child ran with an unqualified temporary directory",
        )
        expect(os.path.islink(scratch), "the foreign link was removed")
        expect(
            open(keep, encoding="utf-8").read() == "foreign",
            "the foreign target changed",
        )
    finally:
        if os.path.islink(scratch):
            os.unlink(scratch)
    # A foreign directory in the owned position: refuse and keep it.
    private_dir(scratch)
    write_private(os.path.join(scratch, "not-ours"), "keep")
    refused = world.run(
        ["check"],
        cwd=crate,
        env=world.env(TMPDIR=None, TMP=None, TEMP=None),
        check=False,
    )
    expect(refused.returncode != 0, "Oxide accepted a foreign scratch directory")
    expect(
        "marker" in refused.stderr,
        f"the missing ownership marker was not named: {refused.stderr[-1500:]}",
    )
    expect(
        "OXIDE-TEMP dir=" not in refused.stderr,
        "a child ran with an unqualified temporary directory",
    )
    expect(
        open(os.path.join(scratch, "not-ours"), encoding="utf-8").read() == "keep",
        "the foreign entry was changed",
    )
    shutil.rmtree(scratch)
    # An over-bound derived path refuses instead of leaving the child on the
    # machine default.
    long_pool = private_dir(os.path.join(world.root, "nested" * 8, "pool"))
    refused = world.run(
        ["check"],
        cwd=crate,
        env=world.env(
            TMPDIR=None,
            TMP=None,
            TEMP=None,
            OXIDE_POOL=long_pool,
            OXIDE_POOL_CAP="64MiB",
        ),
        check=False,
    )
    expect(refused.returncode != 0, "Oxide accepted an over-bound scratch path")
    expect(
        "Unix-socket bound" in refused.stderr,
        f"the over-bound path was not named: {refused.stderr[-1500:]}",
    )
    expect(
        "OXIDE-TEMP dir=" not in refused.stderr,
        "a child ran with an unqualified temporary directory",
    )


def removable(world: World, crate: str) -> None:
    case(CASE_NAMES[10])
    if sys.platform != "darwin":
        say("not macOS: no removable fixture, the internal pool stands")
        return
    image = os.path.join(world.root, "dragontail.sparseimage")
    created = subprocess.run(
        [
            "/usr/bin/hdiutil",
            "create",
            "-size",
            f"{IMAGE_MIB}m",
            "-fs",
            "APFS",
            "-volname",
            "oxide-temp-proof",
            "-type",
            "SPARSE",
            "-quiet",
            image,
        ],
        capture_output=True,
        text=True,
        timeout=work_left(),
    )
    expect(created.returncode == 0, f"image creation failed: {created.stderr}")
    # The mount-root rule makes the drive root owner-only, so the owned image
    # is attached with ownership enabled, exactly as the accepted account-home
    # proof does, and that fact is confirmed before registration.
    attached = subprocess.run(
        ["/usr/bin/hdiutil", "attach", "-nobrowse", "-owners", "on", "-plist", image],
        capture_output=True,
        timeout=work_left(),
    )
    expect(attached.returncode == 0, f"image attach failed: {attached.stderr!r}")
    mount = ""
    try:
        entities = plistlib.loads(attached.stdout)["system-entities"]
        mounts = [entity["mount-point"] for entity in entities if entity.get("mount-point")]
        expect(
            len(mounts) == 1,
            f"expected one mounted volume for the owned image, got {mounts}",
        )
        mount = mounts[0]
        expect(
            mount.startswith("/Volumes/"),
            f"the owned image did not mount under /Volumes: {mount}",
        )
        ownership = subprocess.run(
            ["/usr/sbin/diskutil", "info", "-plist", mount],
            capture_output=True,
            timeout=work_left(),
        )
        expect(ownership.returncode == 0, "diskutil info failed for the mount")
        expect(
            plistlib.loads(ownership.stdout).get("GlobalPermissionsEnabled") is True,
            "the owned image is mounted without ownership enabled",
        )
        say(f"owned scratch image mounted at {mount} with ownership enabled")
        world.run(
            [
                "pool",
                "register-removable",
                mount,
                "--reserve",
                "16MiB",
                "--cap",
                "64MiB",
            ],
            cwd=crate,
            env=world.env(),
        )
        # The inherited fixture scratch is on another device, so the
        # removable build's children must get the owned directory on the
        # image instead.
        result = world.run(["check"], cwd=crate, env=world.env())
        observed = reported(result.stderr, "dir")
        expect(
            os.path.realpath(observed).startswith(
                os.path.realpath(os.path.join(mount, "oxide-pool")) + os.sep
            ),
            f"the removable build's temporary files are not on the image: {observed}",
        )
        expect(
            os.path.realpath(observed).startswith(os.path.realpath(mount) + os.sep),
            f"the inherited other-device directory was not replaced: {observed}",
        )
        expect(
            "temp: oxide" in result.stderr,
            "the removable build did not report the owned scratch",
        )
        expect(
            not os.path.exists(observed.rstrip("/")),
            "the removable build's owned directory survived release",
        )
        # Ordinary detach, then the internal fallback: no inherited TMPDIR, so
        # the internal pool's own scratch is used and no stale mount point is
        # created.
        detached = subprocess.run(
            ["hdiutil", "detach", mount], capture_output=True, text=True
        )
        expect(
            detached.returncode == 0, f"ordinary detach failed: {detached.stderr}"
        )
        expect(not os.path.exists(mount), f"the mount point {mount} was recreated")
        # The registration still exists and its volume is gone, so this build
        # must go through the product's own fallback selection on the
        # original pool; it is not pointed at a fresh pool.
        fallback = world.run(
            ["check"], cwd=crate, env=world.env(TMPDIR=None, TMP=None, TEMP=None)
        )
        observed = reported(fallback.stderr, "dir").rstrip("/")
        expect(
            os.path.realpath(observed).startswith(
                os.path.realpath(os.path.join(world.pool, "payload")) + os.sep
            ),
            f"the fallback did not use the internal pool's scratch: {observed}",
        )
        say(
            "post-detach selection: "
            + " | ".join(
                line
                for line in fallback.stderr.splitlines()
                if "select" in line or "removable" in line
            )[:400]
        )
        expect(not os.path.exists(mount), f"the fallback recreated {mount}")
        # With a same-device inherited TMPDIR the fallback keeps it and
        # creates nothing.
        inherited = private_dir(os.path.join(world.pool, "keep-tmp"))
        kept = world.run(
            ["check"],
            cwd=crate,
            env=world.env(TMPDIR=inherited, TMP=None, TEMP=None),
        )
        expect(
            reported(kept.stderr, "dir").rstrip("/") == canonical(inherited),
            f"the fallback did not keep the same-device inherited directory: {kept.stderr[-1500:]}",
        )
        slots = os.path.join(world.pool, "payload", "slots")
        for root, _, _ in os.walk(slots):
            expect(
                not os.path.exists(os.path.join(root, "scratch")),
                "the fallback created an owned directory it did not need",
            )
    finally:
        if mount and os.path.ismount(mount):
            subprocess.run(["hdiutil", "detach", mount], capture_output=True)
        if os.path.exists(image):
            os.unlink(image)


def path_bound(world: World, crate: str) -> None:
    case(CASE_NAMES[11])
    env = world.env(TMPDIR=None, TMP=None, TEMP=None)
    result = world.run(["check"], cwd=crate, env=env)
    observed = reported(result.stderr, "dir").rstrip("/")
    held_by_pool(world, observed)
    leaf = os.path.basename(observed)
    raw = observed.encode()
    say(
        f"longest injected internal path: {raw!r} = {len(raw)} bytes, "
        f"{len(observed)} characters, leaf {leaf!r}"
    )
    expect(
        len(raw) <= SOCKET_PATH_BOUND,
        f"the injected path is {len(raw)} bytes, over the {SOCKET_PATH_BOUND}-byte bound",
    )
    # A guard against a silently longer derivation: the observation must be
    # the documented `<slot>/scratch` shape.
    expect(leaf == "scratch", f"the injected path does not end in scratch: {observed}")


def main(argv: list[str]) -> int:
    if argv and argv[0] == "--plan":
        plan()
        return 0
    if argv:
        fail(f"unknown argument {argv[0]!r}; use --plan")
    root = scratch_root()
    world_root = private_dir(os.path.join(root, "pool-temp"))
    say(f"scratch root {root}")
    subject = stage_subject(world_root)
    say(f"staged {subject} sha256={sha256_file(subject)}")
    world = World(world_root, subject)
    crate = make_fixture(world_root, "tempcheck")
    premise_sweep(world, crate)
    inherited_kept(world, crate)
    replaced(world, crate)
    relative_inherited(world, crate)
    aliases(world, crate)
    accounting(world, crate)
    case(CASE_NAMES[6])
    expect(
        not any(os.path.exists(path) for path in world.scratch_dirs()),
        "a released slot kept an owned directory",
    )
    crash_and_recovery(world, crate)
    spawns(world, crate)
    refusals(world, crate)
    removable(world, crate)
    path_bound(world, crate)
    print(
        f"pool-temp-scratch proof passed: {len(CASES)} cases, "
        f"{time.perf_counter() - STARTED:.1f}s"
    )
    for name in CASES:
        print(f"  pass {name}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
