#!/usr/bin/env python3
"""Focused native proof for recorded per-slot accounting (oxide#146).

One fixed script and the whole command of the shared proof supervisor
(`bounded-test.py`, guard sha256 7acca44c..., the oxide#191 bytes that add the
opt-in post-census image detach to the qualified oxide#151 guard, plus
oxide#197's ancestor and launcher-sibling exclusions). That pin is
checked against the actual runner file before any effect, so a branch that
still carries different guard bytes refuses here instead of spending a run;
these bytes are unqualified until their own author and reviewer qualifications
pass at the merged head. The
approved recipe is an INTERNAL directory pool, so both heavy selectors pass
`--root-parent /private/tmp` explicitly (Tom's internal-root exception): a
pool inside a selected-volume root would be refused by the product itself.
Bounds are unchanged: 300 s wall, 512 MiB monitored scratch, 270 s work and a
30 s shutdown margin.

Everything is private: each fixture has its own `HOME`, `CARGO_HOME`,
`OXIDE_POOL`, `TMPDIR`/`TMP`/`TEMP` and walk/scope logs beside the pool, all
inside the one owned monitored root. Inherited `OXIDE_*`/`XDG_*` and encoded
compiler-flag values are scrubbed; `OXIDE_TEST_HOME_FROM_ENV=1` makes the
private `HOME` authoritative; the **invoking** toolchain home is resolved
before that HOME exists and handed to staging and every fixture child with
`RUSTUP_TOOLCHAIN=1.97.1`, `RUSTUP_AUTO_INSTALL=0` and offline Cargo; every
`--locked` call is preceded by a manifest-consistent `Cargo.lock`; a pool path
is absent until the product creates it. Byte pressure is driven by the
documented `OXIDE_POOL_CAP` soft byte budget (applied at pool creation), never
by a proof-invented variable.

The internal-pool fact is the product's own serialized `config.json`
(`backend: "directory-budget-v1"`, and no `volume` entry), read through
`recorded_backend()`; a missing, malformed, unknown or removable backend
refuses by name. The ledger has no backend field at all, so a case may never
read one from there.

## Fixed case map (command, cwd, prerequisite, observed phase)

A release always measures the slot it closes (`measure_for_release`), so an
admission claim can only be read from the log *while the command is still
Held*. Every admission oracle below is therefore taken at that boundary and
the release walk is asserted separately.

| Case | Command | cwd | Prerequisite | Observed phase |
| --- | --- | --- | --- | --- |
| A all-recorded admission | `oxide check` (gated) | fixture crate B | slot A idle with a current record | admission (`Held`), then release |
| B missing record | `oxide check` (gated) | fixture crate C | A and B idle, B's record dropped, `capacity_policy` 2 | admission, then release |
| C release/reuse | `oxide check` (gated) | fixture crate A | A idle with a current record | admission (reuse), then release; stdout read after release |
| D byte pressure | `oxide check` (gated) + held writer | pressure crate | one idle slot, one held slot, an out-of-slot stray, byte budget, internal backend recorded in `config.json` | admission, then releases |
| E hard-link overcount | `oxide check` (gated) | pressure crate | two idle slots sharing one inode, budget between union and sum | admission, then release |
| F legacy + crash | `oxide check` (gated, then killed) | third crate, then a fresh crate | every record dropped, `capacity_policy` 1 | admission, then release, then product recovery |
| G status/audit | `oxide status`, `oxide status --measure` | fixture work dir | one idle slot with a record | no `oxide check`, so no release walk |

Honest not-applicable items: no case adopts permanent outputs (#4), invokes an
external Cargo subcommand (#5) or attaches a volume/image (#6), and the script
creates no `mktemp` directory (#9); see `PREMISE_CHECKLIST`.

`--diagnostic` is the safe uncounted collection mode: it records recoverable
oracle failures, continues only into dependency-independent cases, skips
tainted dependents with their reasons, aborts on any unknown child, writer,
custody, protocol or receipt outcome, preserves the first fault and writes one
bounded `qualified: false` receipt in the guard's evidence directory on every
outcome. A case that dies of an unexpected exception is recorded as that
case's failure before any state is computed and aborts the collection, so a
reached-but-crashed case is never a pass; every abort goes through one
`mark_abort()` state, so the durable receipt and the console cannot disagree.
The default run stays fail-fast. Neither mode is a counted PASS
substitute, a benchmark, or an alias for full QA.
"""

from __future__ import annotations

import hashlib
import inspect
import json
import os
import re
import shutil
import subprocess
import sys
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILDER = "1.97.1"
# The qualified shared supervisor this proof's whole command runs under.
GUARD_SHA256 = "8fc1f114edb9d439259225b04c05a65b880a53fc5ee3628ef6e148e3ce9bb992"
GUARD_PATH = os.path.join(REPO, "scripts", "bounded-test.py")

# The product's serialized pool facts, bound to source by `--self-test`. The
# backend lives in the pool's `config.json`, never in the ledger.
CONFIG_NAME = "config.json"
BUDGET_BACKEND = "directory-budget-v1"
VOLUME_BACKEND = "apfs-volume-v1"
MIB = 1024 * 1024
# Staging recipe (oxide#140): 4 Cargo jobs, 16 codegen units, frozen profile.
STAGE_JOBS = 4
STAGE_CODEGEN_UNITS = 16
# Fixtures stay sequential at one Cargo job.
FIXTURE_JOBS = 1
SHUTDOWN_MARGIN_S = 30
# Bytes the fixture build script writes into its slot, so a slot is visibly
# large enough that a walk would be visible in the log.
SLOT_FILLER_BYTES = 3 * MIB
# The documented `OXIDE_POOL_CAP` soft byte budgets (applied at creation).
GENEROUS_BUDGET = 64 * MIB
PRESSURE_BUDGET = 8 * MIB
OVERCOUNT_BUDGET = 6 * MIB
# The shared hard-link payload for case E.
SHARED_INODE_BYTES = 4 * MIB
RECORDED_TOTAL_NEEDLE = "recorded slot sizes (ledger):"
RECORDED_SLOT_NEEDLE = "at slot release"
CAPABILITIES_PROBE = "__oxide-build-capabilities"
HOLD_ENV = "OXIDE_PROOF_HOLD"
DIAGNOSTIC = "--diagnostic"
# Receipt bounds: bounded truth, explicit omissions, never a closure claim.
MAX_RECEIPT_BYTES = 64 * 1024
MAX_RECEIPT_ENTRIES = 64
MAX_RECEIPT_EXCERPT = 600
# The invoking toolchain home, resolved once before any private HOME exists.
INVOKING_RUSTUP_HOME = ""
# The installed `<pin>-<host>` directory proven beneath that home before use.
TOOLCHAIN_INSTALLED = ""

CASES: list[str] = []
FAILURES: list[tuple[str, str]] = []
SKIPS: list[tuple[str, str]] = []
REACHED: list[str] = []
# Cases whose function returned normally. Reachability is not success.
COMPLETED: list[str] = []
TAINTED: set[str] = set()
PRIMARY: list[str] = []
SECONDARY: list[str] = []
ABORTED: bool = False
# Set by `abort()`: an unknown outcome is in flight and stays unknown.
PENDING_ABORT: bool = False
# Set by the pure modes: a check there always raises, never exits the process.
PURE_MODE: bool = False
DIAGNOSTIC_MODE = DIAGNOSTIC in sys.argv


# --------------------------------------------------------------------------
# Reporting. Counted mode fails fast; diagnostic mode records and continues.


class CaseFailure(Exception):
    def __init__(self, message: str, abort: bool) -> None:
        super().__init__(message)
        self.abort = abort


def fail(message: str) -> None:
    # Once an unknown outcome is in flight, no later check may reclassify it as
    # a recoverable oracle failure: the unknown keeps the collection aborted.
    if PENDING_ABORT:
        raise CaseFailure(f"{message} (after an unknown outcome)", abort=True)
    if DIAGNOSTIC_MODE or PURE_MODE:
        raise CaseFailure(message, abort=False)
    print(f"FAILED: {message}", flush=True)
    raise SystemExit(1)


def abort(message: str) -> None:
    """An outcome the diagnostic may never turn into an oracle continuation."""
    global PENDING_ABORT
    PENDING_ABORT = True
    # Record the fault at the moment it happens, so a teardown that raises
    # afterwards cannot replace it as the primary error.
    note_fault(message)
    raise CaseFailure(message, abort=True)


def expect(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def record_unknown(error: BaseException) -> None:
    """main's unknown/unexpected boundary: one abort transition, first fault kept."""
    mark_abort(f"unknown outcome: {type(error).__name__}: {error}")


def mark_abort(reason: str = "") -> None:
    """The ONE abort transition: module state, first fault, receipt and console.

    An abort that were assigned to a local name would leave the durable receipt
    disagreeing with the console, so every abort path goes through here.
    """
    global ABORTED
    ABORTED = True
    if reason:
        note_fault(reason)


def note_fault(message: str) -> None:
    """Preserve the first fault; later ones stay visible as secondary."""
    if not PRIMARY:
        PRIMARY.append(message)
    else:
        SECONDARY.append(message)


def case(name: str) -> None:
    """Announce a case. Reachability itself is recorded by `dispatch`."""
    CASES.append(name)
    print(f"case {name}", flush=True)


def work_left() -> float:
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    left = (deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 3:
        abort("the supervisor's work deadline was reached before the shutdown margin")
    return left


def child_timeout(limit: float = 120.0) -> float:
    """Every child timeout is clipped to the guard's existing work cutoff.

    Only a counted or diagnostic run under the supervisor exports that
    deadline; a pure invocation has no guard clock and uses the fixed limit.
    """
    if "OXIDE_PROOF_DEADLINE_NS" not in os.environ:
        return limit
    return min(limit, work_left())


def sha256_file(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


# --------------------------------------------------------------------------
# Toolchain and staging. One recipe function and one real validator, so the
# pure checks can exercise the same object the counted run uses.


def invoking_rustup_home() -> str:
    """The invoking toolchain home, resolved before a private HOME exists.

    A relative or unusable inherited value is refused before any effect: a
    guessed absolute path could hide the toolchain behind a private HOME, and
    no fixture may download or install one.
    """
    explicit = os.environ.get("RUSTUP_HOME")
    if explicit is not None:
        if not os.path.isabs(explicit):
            abort(f"the inherited RUSTUP_HOME {explicit!r} is not an absolute path")
        if not os.path.isdir(explicit):
            abort(f"the inherited RUSTUP_HOME {explicit!r} is not a directory")
        return os.path.realpath(explicit)
    home = os.environ.get("HOME")
    if not home:
        abort("no invoking HOME to resolve the toolchain home from")
    resolved = os.path.join(home, ".rustup")
    if not os.path.isdir(resolved):
        abort(f"no usable toolchain home at {resolved}")
    return os.path.realpath(resolved)


def toolchain_pin_problems(rustup_home: str, toolchain_entries: list[str]) -> list[str]:
    """Why `<RUSTUP_HOME>/toolchains` does not carry the pinned toolchain.

    Existence of the home is not evidence: rustup installs each toolchain as
    `<home>/toolchains/<pin>-<host>/`, so the pin is proven from that directory
    naming scheme, never from the home alone. `--self-check` exercises this with
    listings it supplies, so no rustup probe, install or toolchain is touched.
    """
    problems: list[str] = []
    if not rustup_home or not os.path.isabs(rustup_home):
        problems.append("toolchain_home_not_absolute")
        return problems
    pinned = [entry for entry in toolchain_entries if entry.startswith(BUILDER + "-")]
    if not pinned:
        problems.append("pinned_toolchain_absent")
        return problems
    installed = os.path.join(rustup_home, "toolchains")
    if not os.path.isdir(installed):
        problems.append("toolchains_directory_absent")
        return problems
    if not any(os.path.isdir(os.path.join(installed, entry)) for entry in pinned):
        problems.append("pinned_toolchain_not_a_directory")
    return problems


def installed_toolchain(home: str) -> str:
    """The installed `<pin>-<host>` name under `<home>/toolchains`."""
    installed = os.path.join(home, "toolchains")
    try:
        entries = sorted(os.listdir(installed))
    except OSError as error:
        abort(f"cannot list {installed}: {error}")
    problems = toolchain_pin_problems(home, entries)
    if problems:
        abort(f"the invoking toolchain home {home} is unusable for the pin: {problems}")
    for entry in entries:
        if entry.startswith(BUILDER + "-"):
            return entry
    abort(f"no installed {BUILDER}-<host> toolchain beneath {installed}")
    raise AssertionError("unreachable")


def toolchain_env(rustup_home: str) -> dict[str, str]:
    """The pinned, offline toolchain and compiler-flag environment.

    It is the single source for staging and for every fixture child, and it
    removes any inherited encoded or plain compiler-flag override so the
    frozen values below cannot be silently replaced.
    """
    return {
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_HOME": rustup_home,
        "RUSTUP_AUTO_INSTALL": "0",
        "CARGO_NET_OFFLINE": "true",
        "RUSTFLAGS": "-C strip=symbols",
    }


def staging_recipe(scratch: str, rustup_home: str | None = None) -> tuple[list[str], dict[str, str]]:
    stage = os.path.join(scratch, "stage")
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("CARGO_TARGET_", "CARGO_BUILD_", "SCCACHE_", "OXIDE_"))
        and key not in ("CARGO", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
                        "CARGO_ENCODED_RUSTFLAGS", "RUSTFLAGS", "RUSTUP_HOME",
                        "RUSTUP_TOOLCHAIN", "RUSTUP_AUTO_INSTALL", "CARGO_NET_OFFLINE")
    }
    env.update(toolchain_env(rustup_home if rustup_home is not None else INVOKING_RUSTUP_HOME))
    env.update(
        {
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(stage, "target"),
            "CARGO_BUILD_BUILD_DIR": os.path.join(stage, "build"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_PROFILE_DEV_DEBUG_ASSERTIONS": "true",
            "CARGO_PROFILE_DEV_OPT_LEVEL": "s",
            "CARGO_PROFILE_DEV_CODEGEN_UNITS": str(STAGE_CODEGEN_UNITS),
            "CARGO_PROFILE_DEV_PANIC": "abort",
            "CARGO_INCREMENTAL": "0",
        }
    )
    argv = ["cargo", "build", "-j", str(STAGE_JOBS), "--locked", "--offline", "--bin", "oxide"]
    return argv, env


def recipe_problems(argv: list[str], env: dict[str, str], stage: str = "") -> list[str]:
    """Every way the given recipe departs from the reviewed one.

    A real validator: `--self-check` mutates one ingredient at a time and
    requires the *specific* problem to be reported, so a counterfactual cannot
    pass by merely differing from a constant.
    """
    problems: list[str] = []
    if argv[:2] != ["cargo", "build"]:
        problems.append("argv_not_cargo_build")
    if "-j" not in argv or argv[argv.index("-j") + 1] != str(STAGE_JOBS):
        problems.append("job_count")
    for flag in ("--locked", "--offline", "--bin"):
        if flag not in argv:
            problems.append(f"argv_missing_{flag.lstrip('-')}")
    for key, expected in (
        ("CARGO_PROFILE_DEV_CODEGEN_UNITS", str(STAGE_CODEGEN_UNITS)),
        ("CARGO_PROFILE_DEV_DEBUG", "0"),
        ("CARGO_PROFILE_DEV_DEBUG_ASSERTIONS", "true"),
        ("CARGO_PROFILE_DEV_OPT_LEVEL", "s"),
        ("CARGO_INCREMENTAL", "0"),
        ("RUSTUP_TOOLCHAIN", BUILDER),
        ("RUSTUP_AUTO_INSTALL", "0"),
        ("CARGO_NET_OFFLINE", "true"),
    ):
        if env.get(key) != expected:
            problems.append(f"env_{key.lower()}")
    if env.get("RUSTFLAGS") != "-C strip=symbols":
        problems.append("rustflags")
    if "CARGO_ENCODED_RUSTFLAGS" in env:
        problems.append("encoded_rustflags_present")
    rustup_home = env.get("RUSTUP_HOME", "")
    if not os.path.isabs(rustup_home) or not os.path.isdir(rustup_home):
        problems.append("rustup_home_unusable")
    if stage:
        for key in ("CARGO_TARGET_DIR", "CARGO_BUILD_BUILD_DIR"):
            if not env.get(key, "").startswith(stage + os.sep):
                problems.append(f"{key.lower()}_outside_stage")
    return problems


def source_identity() -> str:
    """The clean checkout head, read the same bounded way every time."""
    try:
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=REPO,
            capture_output=True,
            text=True,
            timeout=child_timeout(10),
        )
    except (OSError, subprocess.SubprocessError) as error:
        abort(f"cannot read the source head: {error}")
    head = (result.stdout or "").strip()
    if result.returncode != 0 or len(head) != 40 or not all(c in "0123456789abcdef" for c in head):
        abort(f"the source head is not a commit id: {head!r} ({result.stderr.strip()[:200]})")
    return head


def recorded_backend(config: object, where: str = "the pool config") -> str:
    """The pool's recorded backend from its serialized `config.json`.

    Only a positively recorded internal directory pool passes. A missing,
    malformed, unknown or removable backend refuses by name: the fixture never
    assumes its own placement, and it never reads a backend out of the ledger,
    which has no such field.
    """
    if not isinstance(config, dict):
        abort(f"{where} is not a JSON object")
    backend = config.get("backend")
    if not isinstance(backend, str) or not backend:
        abort(f"{where} records no backend")
    if backend == VOLUME_BACKEND:
        abort(f"{where} records the removable backend {backend!r}, not an internal directory pool")
    if backend != BUDGET_BACKEND:
        abort(f"{where} records an unknown backend {backend!r}")
    if "volume" in config:
        abort(f"{where} carries a volume entry for the directory backend")
    return backend


def guard_identity(guard_path: str = "", expected: str = "") -> str:
    """The shared supervisor's actual hash, refusing any drift from the pin.

    The default pin is the adopted qualified runner; the pure check calls this
    with the real file so a stale pin (or a runner swapped under a matching
    pin) is caught without a subprocess, scratch directory or fixture.
    """
    path = guard_path or GUARD_PATH
    pinned = expected or GUARD_SHA256
    actual = sha256_file(path)
    if actual != pinned:
        abort(f"the shared supervisor bytes changed: {actual} != {pinned}")
    return actual


def stage_subject(scratch: str) -> str:
    argv, env = staging_recipe(scratch)
    problems = recipe_problems(argv, env, os.path.join(scratch, "stage"))
    if problems:
        abort(f"the staging recipe is not the reviewed one: {problems}")
    log = os.path.join(scratch, "stage.log")
    with open(log, "w", encoding="utf-8") as handle:
        build = subprocess.run(
            argv,
            cwd=REPO,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            timeout=child_timeout(),
        )
    if build.returncode:
        with open(log, encoding="utf-8") as handle:
            abort(f"staging exit {build.returncode}: {handle.read()[-2000:]}")
    built = os.path.join(scratch, "stage", "target", "debug", "oxide")
    if not os.path.isfile(built):
        abort(f"staging produced no binary at {built}")
    subject = os.path.join(scratch, "subject-oxide")
    shutil.copy2(built, subject)
    os.chmod(subject, 0o755)
    shutil.rmtree(os.path.join(scratch, "stage"))
    os.unlink(log)
    return subject


# --------------------------------------------------------------------------
# Rust fixture generation. One render function per template, substituting only
# the intended payload: Rust braces and `format!` placeholders are preserved.


BUILD_RS_TEMPLATE = r'''
use std::io::Write;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=nonce.txt");
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let path = std::path::Path::new(&out).join("slot-filler.bin");
    let bytes = vec![7_u8; __SLOT_FILLER_BYTES__];
    std::fs::File::create(&path)
        .and_then(|mut file| file.write_all(&bytes))
        .expect("slot filler");
    // A gated writer: publish readiness, then wait for the release file, so a
    // proof can observe this slot held while another admission runs. The wait
    // is bounded so a lost release file cannot hang the run.
    if let Ok(gate) = std::env::var("OXIDE_PROOF_HOLD") {
        std::fs::write(format!("{}.ready", gate), b"ready\n").expect("readiness marker");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        while !std::path::Path::new(&gate).exists() {
            if std::time::Instant::now() > deadline {
                panic!("the release file never appeared");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}
'''

HARD_LINK_RS_TEMPLATE = r'''
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=nonce.txt");
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let shared = std::path::Path::new(__SHARED_PATH__);
    std::fs::hard_link(shared, std::path::Path::new(&out).join("shared.bin"))
        .expect("shared inode link");
    if let Ok(gate) = std::env::var("OXIDE_PROOF_HOLD") {
        std::fs::write(format!("{}.ready", gate), b"ready\n").expect("readiness marker");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        while !std::path::Path::new(&gate).exists() {
            if std::time::Instant::now() > deadline {
                panic!("the release file never appeared");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}
'''

FIXTURE_MAIN = 'fn main() { println!("recorded-accounting-fixture"); }\n'

FIXTURE_MANIFEST = """[package]
name = "__CRATE_NAME__"
version = "__CRATE_VERSION__"
edition = "2021"
build = "build.rs"

[workspace]
"""

CRATE_VERSION = "0.1.0"


def render_build_rs(filler_bytes: int) -> str:
    """Substitute only the filler count; every Rust brace stays literal."""
    if not isinstance(filler_bytes, int) or filler_bytes <= 0:
        raise ValueError("filler bytes must be a positive integer")
    text = BUILD_RS_TEMPLATE.replace("__SLOT_FILLER_BYTES__", str(filler_bytes))
    if "__SLOT_FILLER_BYTES__" in text:
        raise ValueError("the filler placeholder survived")
    return text


def rust_string_literal(value: str) -> str:
    """A Rust string literal for `value`, escaping what Rust requires.

    Rust source is UTF-8, so non-ASCII characters stay literal; a backslash or
    quote is escaped and a control character becomes `\\u{..}`, which keeps the
    emitted token a single well-formed literal for any controlled path.
    """
    if not isinstance(value, str):
        raise ValueError("a Rust string literal needs text")
    parts = ['"']
    for character in value:
        if character == "\\":
            parts.append("\\\\")
        elif character == '"':
            parts.append('\\"')
        elif character == "\n":
            parts.append("\\n")
        elif character == "\r":
            parts.append("\\r")
        elif character == "\t":
            parts.append("\\t")
        elif ord(character) < 0x20 or ord(character) == 0x7F:
            parts.append(f"\\u{{{ord(character):x}}}")
        else:
            parts.append(character)
    parts.append('"')
    return "".join(parts)


def render_hard_link_build_rs(shared_path: str) -> str:
    """Substitute one correctly encoded Rust literal for the shared path."""
    literal = rust_string_literal(shared_path)
    text = HARD_LINK_RS_TEMPLATE.replace("__SHARED_PATH__", literal)
    if "__SHARED_PATH__" in text:
        raise ValueError("the shared-path placeholder survived")
    return text


def render_manifest(name: str, version: str = CRATE_VERSION) -> str:
    text = FIXTURE_MANIFEST.replace("__CRATE_NAME__", name).replace("__CRATE_VERSION__", version)
    if "__CRATE_" in text:
        raise ValueError("a manifest placeholder survived")
    return text


def lock_is_manifest_consistent(lock_text: str, name: str, version: str) -> bool:
    """A lock that actually pins this package at this version.

    `generate-lockfile` writes the package block itself; a missing, header-less
    or mismatched lock must be refused rather than treated as valid.
    """
    if not lock_text.startswith("# This file is automatically @generated by Cargo."):
        return False
    package = f'name = "{name}"'
    pinned = f'version = "{version}"'
    return package in lock_text and pinned in lock_text


# --------------------------------------------------------------------------
# Phase-scoped walk observation.


def read_log(path: str) -> list[str]:
    if not os.path.isfile(path):
        return []
    with open(path, encoding="utf-8") as handle:
        return [line.strip() for line in handle if line.strip()]


def phase_slice(before: list[str], after: list[str]) -> list[str]:
    """The lines appended after `before`: one phase's walks, unfiltered."""
    if after[: len(before)] != before:
        raise CaseFailure("a phase log lost lines it already held", abort=True)
    return after[len(before):]


def distinct(roots: list[str]) -> list[str]:
    return sorted(set(roots))


def canon(roots: list[str]) -> list[str]:
    """Compare paths by canonical real path on both sides.

    `/tmp` and `/private/tmp`, or any other alias, are the same slot root; a
    substring or suffix test would misattribute a walk.
    """
    return sorted({os.path.realpath(root) for root in roots})


def same_roots(observed: list[str], expected: list[str]) -> bool:
    return canon(observed) == canon(expected)


def tree_facts(root: str) -> tuple[int, int, int]:
    """Bounded own-fixture facts: (bytes, distinct inodes, largest size).

    Used only to establish a premise about the proof's own small fixture; it is
    not a second accounting engine and never replaces a product figure.
    """
    total = 0
    inodes: set[tuple[int, int]] = set()
    largest = 0
    for base, directories, files in os.walk(root):
        directories.sort()
        for name in sorted(files):
            path = os.path.join(base, name)
            try:
                info = os.lstat(path)
            except OSError as error:
                raise CaseFailure(f"cannot stat own fixture {path}: {error}", abort=True)
            if not os.path.isfile(path) or os.path.islink(path):
                continue
            key = (info.st_dev, info.st_ino)
            if key in inodes:
                continue
            inodes.add(key)
            total += info.st_blocks * 512
            largest = max(largest, info.st_size)
    return total, len(inodes), largest


def union_allocated(roots: list[str]) -> tuple[int, int]:
    """Exact allocated bytes over the union of these fixture trees."""
    seen: set[tuple[int, int]] = set()
    total = 0
    for root in roots:
        for base, directories, files in os.walk(root):
            directories.sort()
            for name in sorted(files):
                path = os.path.join(base, name)
                info = os.lstat(path)
                if os.path.islink(path) or not os.path.isfile(path):
                    continue
                key = (info.st_dev, info.st_ino)
                if key in seen:
                    continue
                seen.add(key)
                total += info.st_blocks * 512
    return total, len(seen)


def file_identity(path: str) -> tuple[int, int, int, int]:
    """(device, inode, size, allocated) of one owned fixture file."""
    info = os.lstat(path)
    return info.st_dev, info.st_ino, info.st_size, info.st_blocks * 512


def recorded_total_line(output: str) -> str:
    """The exact rendered ledger-total line, or an empty string."""
    for line in output.splitlines():
        if RECORDED_TOTAL_NEEDLE in line:
            return line.strip()
    return ""


def recorded_total_grammar(line: str) -> bool:
    """`recorded slot sizes (ledger): <size> across <n> of <m> idle slots`.

    The rendered grammar is `main.rs`'s; the proof binds the exact shape rather
    than a loose substring, so a missing or reworded total cannot pass.
    """
    pattern = (
        re.escape(RECORDED_TOTAL_NEEDLE)
        + r" \S+(?: \S+)*? across (\d+) of (\d+) idle slots"
    )
    return re.search(pattern, line) is not None


def recorded_total_counts(line: str) -> tuple[int, int] | None:
    match = re.search(r"across (\d+) of (\d+) idle slots", line)
    if match is None:
        return None
    return int(match.group(1)), int(match.group(2))


# --------------------------------------------------------------------------
# Fixtures.


class Fixture:
    """One private fixture: its own roots, its own absent pool path."""

    def __init__(self, root: str, label: str, budget_bytes: int = GENEROUS_BUDGET) -> None:
        self.root = os.path.join(root, label)
        self.home = os.path.join(self.root, "home")
        self.cargo_home = os.path.join(self.root, "cargo")
        self.pool = os.path.join(self.root, "pool")
        self.work = os.path.join(self.root, "work")
        self.temp = os.path.join(self.root, "temp")
        self.walk_log = os.path.join(self.root, "walk.log")
        self.scope_log = os.path.join(self.root, "scope.log")
        self.budget_bytes = budget_bytes
        for path in (self.home, self.cargo_home, self.temp, self.work):
            os.makedirs(path, exist_ok=True)
        os.chmod(self.root, 0o700)

    # -- fixture crates -------------------------------------------------

    def crate(self, name: str, hard_link: str | None = None) -> str:
        path = os.path.join(self.work, name)
        os.makedirs(path, exist_ok=True)
        with open(os.path.join(path, "Cargo.toml"), "w", encoding="utf-8") as handle:
            handle.write(render_manifest(name))
        src = os.path.join(path, "src")
        os.makedirs(src, exist_ok=True)
        with open(os.path.join(src, "main.rs"), "w", encoding="utf-8") as handle:
            handle.write(FIXTURE_MAIN)
        build_rs = (
            render_hard_link_build_rs(hard_link)
            if hard_link is not None
            else render_build_rs(SLOT_FILLER_BYTES)
        )
        with open(os.path.join(path, "build.rs"), "w", encoding="utf-8") as handle:
            handle.write(build_rs)
        with open(os.path.join(path, "nonce.txt"), "w", encoding="utf-8") as handle:
            handle.write("0\n")
        self.establish_lock(path, name)
        return path

    def bump_nonce(self, checkout: str) -> None:
        """A fresh build-script input, so a gated writer always re-runs it."""
        with open(os.path.join(checkout, "nonce.txt"), "w", encoding="utf-8") as handle:
            handle.write(f"{time.time_ns()}\n")

    def establish_lock(self, checkout: str, name: str) -> None:
        """A fresh crate has no lock: `--locked` would refuse without this."""
        lock = os.path.join(checkout, "Cargo.lock")
        if os.path.isfile(lock):
            with open(lock, encoding="utf-8") as handle:
                if not lock_is_manifest_consistent(handle.read(), name, CRATE_VERSION):
                    abort(f"the fixture lock at {lock} is not manifest-consistent")
            return
        setup = subprocess.run(
            ["cargo", "generate-lockfile", "--offline"],
            cwd=checkout,
            env=self.env(),
            capture_output=True,
            text=True,
            timeout=child_timeout(60),
        )
        if setup.returncode:
            abort(f"offline lock generation failed: {setup.stderr[-800:]}")
        if not os.path.isfile(lock):
            abort("offline lock generation produced no Cargo.lock")
        with open(lock, encoding="utf-8") as handle:
            if not lock_is_manifest_consistent(handle.read(), name, CRATE_VERSION):
                abort(f"the generated lock at {lock} is not manifest-consistent")

    # -- environment ----------------------------------------------------

    def env(self, extra: dict[str, str] | None = None) -> dict[str, str]:
        env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith(("OXIDE_", "XDG_"))
            and key not in ("TMPDIR", "TMP", "TEMP", "CARGO_ENCODED_RUSTFLAGS", "RUSTFLAGS",
                            "RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "RUSTUP_AUTO_INSTALL",
                            "CARGO_NET_OFFLINE")
        }
        env.update(toolchain_env(INVOKING_RUSTUP_HOME))
        env.update(
            {
                "HOME": self.home,
                "CARGO_HOME": self.cargo_home,
                "OXIDE_TEST_HOME_FROM_ENV": "1",
                "OXIDE_POOL": self.pool,
                # The fixture creates its own pool, outside the published
                # task scratch root, so it takes the explicit opt-in.
                "OXIDE_POOL_CREATE": "1",
                # The documented soft byte budget for the next pool creation.
                "OXIDE_POOL_CAP": str(self.budget_bytes),
                "TMPDIR": self.temp,
                "TMP": self.temp,
                "TEMP": self.temp,
                "OXIDE_TEST_WALK_LOG": self.walk_log,
                "OXIDE_TEST_SCOPE_LOG": self.scope_log,
            }
        )
        if extra:
            env.update(extra)
        return env

    def run(
        self,
        subject: str,
        argv: list[str],
        cwd: str,
        extra: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess:
        return subprocess.run(
            [subject, *argv],
            cwd=cwd,
            env=self.env(extra=extra),
            capture_output=True,
            text=True,
            timeout=child_timeout(),
        )

    def run_gated(
        self,
        subject: str,
        checkout: str,
        label: str,
    ) -> tuple[subprocess.Popen, str, str, str]:
        """Start a gated writer whose output goes to owned files.

        Its stdout/stderr are redirected to an owned file, so an undrained pipe
        can never block the child before readiness. The child stays in this
        session: no `start_new_session`, no extra signal authority.
        """
        gate = os.path.join(self.root, f"release-{label}")
        ready = gate + ".ready"
        for path in (gate, ready):
            if os.path.exists(path):
                os.unlink(path)
        output = os.path.join(self.root, f"writer-{label}.log")
        self.bump_nonce(checkout)
        handle = open(output, "wb")
        try:
            child = subprocess.Popen(
                [subject, "check"],
                cwd=checkout,
                env=self.env(extra={HOLD_ENV: gate}),
                stdout=handle,
                stderr=subprocess.STDOUT,
                text=False,
            )
        finally:
            handle.close()
        return child, gate, ready, output

    # -- observations ---------------------------------------------------

    def reset_logs(self) -> None:
        for path in (self.walk_log, self.scope_log):
            if os.path.exists(path):
                os.unlink(path)

    def walks(self) -> list[str]:
        return read_log(self.walk_log)

    def scopes(self) -> list[str]:
        return read_log(self.scope_log)

    def status(self, subject: str) -> str:
        result = self.run(subject, ["status"], self.work)
        expect(result.returncode == 0, f"status exit {result.returncode}: {result.stderr[-800:]}")
        return result.stdout

    def pool_config(self) -> dict:
        path = os.path.join(self.pool, CONFIG_NAME)
        try:
            with open(path, encoding="utf-8") as handle:
                return json.load(handle)
        except OSError as error:
            abort(f"the pool records no readable config at {path}: {error}")
        except ValueError as error:
            abort(f"the pool config at {path} is not JSON: {error}")

    def require_internal_directory_backend(self) -> str:
        return recorded_backend(self.pool_config(), f"the pool config at {self.pool}")

    def ledger(self) -> dict:
        with open(os.path.join(self.pool, "ledger.json"), encoding="utf-8") as handle:
            return json.load(handle)

    def slot_ids(self) -> list[str]:
        return [str(slot["id"]) for slot in self.ledger()["slots"]]

    def held_slots(self) -> list[str]:
        return [str(slot["id"]) for slot in self.ledger()["slots"] if slot["state"] == "held"]

    def recorded(self, slot_id: str) -> dict | None:
        for slot in self.ledger()["slots"]:
            if str(slot["id"]) == slot_id:
                return slot.get("recorded")
        return None

    def current_records(self) -> list[str]:
        """Idle slots whose recorded observation still matches its generation."""
        current = []
        for slot in self.ledger()["slots"]:
            record = slot.get("recorded")
            if record is None:
                continue
            if slot["state"] == "idle" and slot.get("lease_id") in (None, ""):
                if record.get("for_lease") == slot.get("last_lease_id"):
                    current.append(str(slot["id"]))
        return current

    def payload_root(self) -> str:
        return os.path.join(self.pool, "payload")

    def idle_slots(self) -> list[str]:
        return [
            str(slot["id"])
            for slot in self.ledger()["slots"]
            if slot["state"] == "idle" and slot.get("lease_id") in (None, "")
        ]

    def slot_dir(self, slot_id: str) -> str:
        slots_root = os.path.join(self.pool, "payload", "slots")
        for slot in self.ledger()["slots"]:
            if str(slot["id"]) != slot_id:
                continue
            recorded = slot.get("dir")
            if recorded is None:
                return os.path.join(slots_root, slot_id)
            return os.path.join(slots_root, *recorded.split("/"))
        abort(f"slot {slot_id} is absent from the ledger")


def replace_ledger_quiescent(fixture: Fixture, document: dict) -> None:
    """Atomically replace an owned fixture's ledger while nothing holds it.

    The product reads a private `ledger.json` with no digest or control
    binding, and tolerates a crash-left `ledger.tmp-*` file, so an edit uses
    exactly that temporary-name rule, mode 0600, one write, fsync and a
    replace. Only a quiescent pool may be edited: no writer may hold a slot.
    """
    if fixture.held_slots():
        abort(f"the fixture is not quiescent: held {fixture.held_slots()}")
    payload = json.dumps(document).encode()
    temp = os.path.join(fixture.pool, "ledger.tmp-146-edit")
    descriptor = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
    try:
        os.write(descriptor, payload)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    os.replace(temp, os.path.join(fixture.pool, "ledger.json"))


def built(fixture: Fixture, subject: str, checkout: str) -> None:
    """A complete ungated command, used only where no phase claim is made."""
    result = fixture.run(subject, ["check"], checkout)
    if result.returncode:
        abort(f"fixture build failed ({result.returncode}): {result.stderr[-1200:]}")


def select_new_held(held: list[str], before: set[str]) -> str:
    """The one held lease this launch created, excluding pre-existing holders.

    A fresh readiness marker alone cannot associate an arbitrary held row with
    this launch: with another writer already holding a slot, `held[0]` may be
    the wrong one. Exactly one new held lease must appear.
    """
    fresh = [slot for slot in held if slot not in before]
    if not fresh:
        raise CaseFailure(
            f"no new held lease appeared; already held before the launch: {sorted(before)}",
            abort=True,
        )
    if len(fresh) > 1:
        raise CaseFailure(f"several new held leases appeared at once: {sorted(fresh)}", abort=True)
    return fresh[0]


def await_held(
    fixture: Fixture,
    child: subprocess.Popen,
    ready: str,
    output: str,
    before: set[str],
) -> str:
    """Wait for this launch's own readiness marker and its own new held lease."""
    deadline = time.monotonic() + child_timeout(90)
    while time.monotonic() < deadline:
        if child.poll() is not None:
            abort(f"the gated writer exited early: {read_log(output)[-6:]}")
        held = fixture.held_slots()
        if held and os.path.exists(ready):
            return select_new_held(held, before)
        time.sleep(0.2)
    child.kill()
    child.wait()
    abort("the gated writer never reached a genuinely held slot")


def release_gated(
    fixture: Fixture,
    child,
    gate: str,
    output: str,
    signal=None,
) -> str:
    """Release one gated writer and return everything it wrote.

    A release that has to force its owned child, or that fails after any
    unknown outcome is already in flight, is an unknown custody outcome: it
    aborts and never downgrades the earlier fault. `signal` is the release
    channel, injectable so the pure check can drive this decision without
    touching the filesystem.
    """
    if signal is None:
        def signal() -> None:
            with open(gate, "w", encoding="utf-8") as handle:
                handle.write("release\n")
    signal()
    try:
        child.wait(timeout=child_timeout(90))
    except subprocess.TimeoutExpired:
        child.kill()
        child.wait()
        abort("the gated writer did not exit on its release file; forced teardown")
    if child.returncode != 0:
        # `fail` escalates to abort when an unknown outcome is already latched.
        fail(f"the released writer exited {child.returncode}")
    return "\n".join(read_log(output))


# --------------------------------------------------------------------------
# Cases. Each admission oracle is read while its command is still Held; the
# release walk (which a release always makes) is asserted separately.


def case_a_all_recorded_admission_walks_nothing(root: str, subject: str) -> None:
    case("a_all_recorded_admission_walks_nothing")
    fx = Fixture(root, "a")
    built(fx, subject, fx.crate("first"))
    idle = fx.idle_slots()
    expect(len(idle) == 1, f"expected one idle slot, saw {idle}")
    expect(fx.recorded(idle[0]) is not None, "the first release recorded nothing")

    # A second project needs a new slot; every record is current, so the
    # admission owes no inventory walk at all.
    fx.reset_logs()
    before = fx.walks()
    scopes_before = fx.scopes()
    held_before = set(fx.held_slots())
    child, gate, ready, output = fx.run_gated(subject, fx.crate("second"), "a")
    new_slot = ""
    try:
        new_slot = await_held(fx, child, ready, output, held_before)
        admission = phase_slice(before, fx.walks())
        expect(admission == [], f"admission walked inventory: {admission}")
        phases = phase_slice(scopes_before, fx.scopes())
        expect(len(phases) == 1, f"expected exactly one shallow scope pass: {phases}")
        expect(same_roots(phases, [fx.payload_root()]), f"the scope pass was not the payload: {phases}")
        expect(fx.recorded(new_slot) is None, "a held slot carries an idle observation")
        expect(len(fx.slot_ids()) == 2, f"the second project got no own slot: {fx.slot_ids()}")
    finally:
        at_release = fx.walks()
        release_gated(fx, child, gate, output)
    release = phase_slice(at_release, fx.walks())
    expect(
        same_roots(release, [fx.slot_dir(new_slot)]) and len(release) == 1,
        f"the release walk was not exactly the closed slot: {release}",
    )
    expect(fx.recorded(new_slot) is not None, "the release published no observation")
    text = fx.status(subject)
    line = recorded_total_line(text)
    expect(line != "", "status did not report the ledger recorded total")
    expect(recorded_total_grammar(line), f"the recorded total line lost its grammar: {line!r}")
    expect(
        recorded_total_counts(line) == (2, 2),
        f"status did not report both idle records: {line!r}",
    )
    expect(
        len(fx.current_records()) == 2,
        f"the ledger does not hold two current idle records: {fx.current_records()}",
    )


def case_b_missing_record_walks_only_that_slot(root: str, subject: str) -> None:
    case("b_missing_record_walks_only_that_slot")
    fx = Fixture(root, "b")
    built(fx, subject, fx.crate("first"))
    built(fx, subject, fx.crate("second"))
    idle = fx.idle_slots()
    expect(len(idle) == 2, f"expected two idle slots, saw {idle}")
    victim, keeper = idle[0], idle[1]

    # A legacy-shaped ledger: one slot keeps its observation, the other has
    # none. The product owns the 0600 mode of this file.
    document = fx.ledger()
    for slot in document["slots"]:
        if str(slot["id"]) == victim:
            slot.pop("recorded", None)
    replace_ledger_quiescent(fx, document)

    # The product must read exactly this legacy shape before the admission.
    read_back = fx.ledger()
    expect(
        read_back["capacity_policy"] == 2
        and all(str(slot["id"]) != victim or slot.get("recorded") is None for slot in read_back["slots"]),
        "the ledger edit did not produce the intended missing-record shape",
    )
    fx.reset_logs()
    before = fx.walks()
    held_before = set(fx.held_slots())
    child, gate, ready, output = fx.run_gated(subject, fx.crate("third"), "b")
    new_slot = ""
    try:
        new_slot = await_held(fx, child, ready, output, held_before)
        admission = phase_slice(before, fx.walks())
        expect(
            same_roots(admission, [fx.slot_dir(victim)]) and len(admission) == 1,
            f"admission walked {admission}, not exactly the unrecorded slot",
        )
        expect(fx.recorded(victim) is not None, "the completed walk was not published")
        expect(fx.recorded(keeper) is not None, "the untouched record was lost")
    finally:
        at_release = fx.walks()
        release_gated(fx, child, gate, output)
    release = phase_slice(at_release, fx.walks())
    expect(
        same_roots(release, [fx.slot_dir(new_slot)]) and len(release) == 1,
        f"the release walk was not exactly the closed slot: {release}",
    )


def case_c_release_publishes_and_reuse_clears(root: str, subject: str) -> None:
    case("c_release_publishes_and_reuse_clears")
    fx = Fixture(root, "c")
    checkout = fx.crate("reuse")
    built(fx, subject, checkout)
    idle = fx.idle_slots()
    expect(len(idle) == 1, f"expected one idle slot, saw {idle}")
    first = fx.recorded(idle[0])
    expect(first is not None, "release published no closed observation")
    expect(
        first["unique_bytes"] >= SLOT_FILLER_BYTES,
        f"the record undercounts its own slot: {first['unique_bytes']}",
    )

    fx.reset_logs()
    before = fx.walks()
    held_before = set(fx.held_slots())
    child, gate, ready, output = fx.run_gated(subject, checkout, "c")
    provenance = ""
    try:
        held = await_held(fx, child, ready, output, held_before)
        expect(held == idle[0], f"the reuse took slot {held}, not {idle[0]}")
        admission = phase_slice(before, fx.walks())
        expect(
            same_roots(admission, [fx.slot_dir(idle[0])]) and len(admission) == 1,
            f"reuse walked {admission}, not exactly its own slot",
        )
        expect(fx.recorded(held) is None, "a held slot kept the idle record")
    finally:
        at_release = fx.walks()
        provenance = release_gated(fx, child, gate, output)
    release = phase_slice(at_release, fx.walks())
    expect(
        same_roots(release, [fx.slot_dir(idle[0])]) and len(release) == 1,
        f"the release walk was not exactly the closed slot: {release}",
    )
    expect(fx.recorded(idle[0]) is not None, "the reuse release published no observation")
    expect(
        f"slot {idle[0]} was " in provenance and "MiB when leased" in provenance,
        f"the reuse result line lost its provenance: {provenance[-600:]}",
    )
    record = fx.recorded(idle[0])
    expect(record is not None, "the released reuse recorded nothing")
    expect(
        record["for_lease"] != first["for_lease"],
        "the new generation kept the previous generation's record",
    )


def case_d_byte_pressure_evicts_only_the_idle_slot(root: str, subject: str) -> None:
    case("d_byte_pressure_evicts_only_the_idle_slot")
    fx = Fixture(root, "d", budget_bytes=PRESSURE_BUDGET)
    built(fx, subject, fx.crate("first"))
    idle = fx.idle_slots()
    expect(len(idle) == 1, f"expected one idle slot, saw {idle}")
    expect(
        fx.ledger()["cap_bytes"] == PRESSURE_BUDGET,
        f"the fixture did not get its byte budget: {fx.ledger()['cap_bytes']}",
    )
    expect(
        fx.require_internal_directory_backend() == BUDGET_BACKEND,
        f"the fixture is not the internal directory pool: {fx.pool_config()}",
    )
    idle_record = fx.recorded(idle[0])
    expect(idle_record is not None, "the idle slot has no current record to count")
    evicted_path = fx.slot_dir(idle[0])
    held_checkout = fx.crate("held")
    stray = os.path.join(fx.payload_root(), "slots", "stray.bin")
    stray_bytes = 2 * MIB

    writer_before = set(fx.held_slots())
    writer, gate, ready, writer_output = fx.run_gated(subject, held_checkout, "d-held")
    held_slot = ""
    pressure_gate = ""
    pressure_output = ""
    try:
        held_slot = await_held(fx, writer, ready, writer_output, writer_before)
        expect(
            fx.recorded(held_slot) is None,
            "a held slot still carried an idle observation",
        )
        held_dir = fx.slot_dir(held_slot)
        # The stray is the supported out-of-slot path: inside `payload/slots`,
        # counted by the scope pass and owned by nobody.
        with open(stray, "wb") as handle:
            handle.write(b"s" * stray_bytes)
        stray_identity = file_identity(stray)
        held_bytes = tree_facts(held_dir)[0]

        # Before the pressure command: the product's own current figure is the
        # idle record plus the fresh held walk plus the scope's stray.
        before_used = idle_record["unique_bytes"] + held_bytes + stray_identity[3]
        after_victim = held_bytes + stray_identity[3]
        print(
            f"  case d premise: before={before_used} cap={PRESSURE_BUDGET}"
            f" after-victim={after_victim} (idle record {idle_record['unique_bytes']},"
            f" held {held_bytes}, stray {stray_identity[3]})",
            flush=True,
        )
        expect(
            before_used >= PRESSURE_BUDGET,
            f"the premise is not non-vacuous: before {before_used} < cap {PRESSURE_BUDGET}",
        )
        expect(
            after_victim < PRESSURE_BUDGET,
            f"removing the victim does not relieve pressure: {after_victim} >= {PRESSURE_BUDGET}",
        )

        fx.reset_logs()
        before = fx.walks()
        pressure_before = set(fx.held_slots())
        pressure, pressure_gate, pressure_ready, pressure_output = fx.run_gated(
            subject, fx.crate("pressure"), "d-pressure"
        )
        try:
            new_slot = await_held(fx, pressure, pressure_ready, pressure_output, pressure_before)
            admission = phase_slice(before, fx.walks())
            # Eviction pressure may re-walk the held slot several times (the
            # initial figure, each loop iteration and the post-decision check).
            # Repeated identical held roots are the required rechecks; no idle
            # root and no whole-payload root may appear.
            expect(admission, "the pressure admission walked no held slot at all")
            expect(
                same_roots(admission, [held_dir]),
                f"the pressure admission walked {distinct(admission)},"
                f" not only the held slot {held_dir}",
            )
            expect(
                not any(os.path.realpath(root) == os.path.realpath(evicted_path) for root in admission),
                "an idle slot with a current record was walked",
            )
            expect(
                not any(os.path.realpath(root) == os.path.realpath(fx.payload_root()) for root in admission),
                "the whole payload was walked during admission",
            )
            ids = fx.slot_ids()
            expect(idle[0] not in ids, "recorded byte pressure did not evict the idle whole slot")
            expect(held_slot in ids, "the held slot was deleted or treated as evictable")
            expect(new_slot in ids, "the pressure command took no slot")
            expect(os.path.exists(stray), "the foreign out-of-slot entry was deleted")
            expect(
                file_identity(stray) == stray_identity,
                "the foreign out-of-slot entry changed bytes, inode or size",
            )
            expect(
                not os.path.exists(evicted_path),
                "the evicted slot's tree was not removed whole",
            )
            expect(
                fx.recorded(held_slot) is None,
                "the held slot gained an idle observation while still held",
            )
        finally:
            pressure_at_release = fx.walks()
            pressure_release = release_gated(fx, pressure, pressure_gate, pressure_output)
        pressure_release_walks = phase_slice(pressure_at_release, fx.walks())
        expect(
            same_roots(pressure_release_walks, [fx.slot_dir(new_slot)])
            and len(pressure_release_walks) == 1,
            f"the pressure command's release walk was {pressure_release_walks}",
        )
        expect(pressure_release != "", "the pressure writer wrote no result line")
    finally:
        writer_release = release_gated(fx, writer, gate, writer_output)
    # After the original writer's release its observation must exist.
    expect(writer_release != "", "the held writer wrote no result line")
    expect(
        fx.recorded(held_slot) is not None,
        "the released held writer published no observation",
    )


def case_e_cross_slot_hard_link_overcount(root: str, subject: str) -> None:
    case("e_cross_slot_hard_link_overcount")
    fx = Fixture(root, "e", budget_bytes=OVERCOUNT_BUDGET)
    shared = os.path.join(fx.root, "shared inode.bin")
    with open(shared, "wb") as handle:
        handle.write(b"h" * SHARED_INODE_BYTES)
    shared_info = os.lstat(shared)
    shared_key = (shared_info.st_dev, shared_info.st_ino)
    for name in ("one", "two"):
        built(fx, subject, fx.crate(name, hard_link=shared))

    idle = fx.idle_slots()
    expect(len(idle) == 2, f"expected two idle slots, saw {idle}")
    recorded_sum = 0
    for slot_id in idle:
        record = fx.recorded(slot_id)
        expect(record is not None, f"slot {slot_id} has no closed observation")
        recorded_sum += record["unique_bytes"]
    root_a, root_b = (fx.slot_dir(slot) for slot in idle)
    expect(
        os.lstat(root_a).st_dev == os.lstat(root_b).st_dev,
        "the two fixture slots are not on one device, so a union is meaningless",
    )
    expect(
        shared_inode_precondition([root_a, root_b], shared_key),
        "the shared inode is not present in both slot trees",
    )
    # The union is the *fixture's own* device/inode union, not a product
    # measurement claim: it exists only to prove the 6 MiB budget sits between
    # the two figures the design distinguishes.
    union_bytes, union_inodes = union_allocated([root_a, root_b])
    survivor_record = min(fx.recorded(slot)["unique_bytes"] for slot in idle)
    print(
        f"  case e premise: fixture union={union_bytes} ({union_inodes} inodes)"
        f" recorded sum={recorded_sum} survivor record={survivor_record}"
        f" (budget {OVERCOUNT_BUDGET})",
        flush=True,
    )
    expect(
        recorded_sum >= 2 * SHARED_INODE_BYTES,
        f"the recorded sum ({recorded_sum}) did not count the shared inode twice",
    )
    expect(
        recorded_sum >= OVERCOUNT_BUDGET,
        f"the recorded sum ({recorded_sum}) is not at or above the budget",
    )
    expect(
        union_bytes < OVERCOUNT_BUDGET,
        f"the fixture union ({union_bytes}) is already at or above the budget",
    )
    expect(
        survivor_record < OVERCOUNT_BUDGET,
        f"one surviving record ({survivor_record}) is at or above the budget",
    )

    fx.reset_logs()
    before = fx.walks()
    held_before = set(fx.held_slots())
    child, gate, ready, output = fx.run_gated(subject, fx.crate("pressure"), "e")
    new_slot = ""
    try:
        new_slot = await_held(fx, child, ready, output, held_before)
        admission = phase_slice(before, fx.walks())
        expect(admission == [], f"the overcount admission walked inventory: {admission}")
        ids = fx.slot_ids()
        expect(
            len(ids) == 2,
            f"the accepted overcount did not replace exactly one idle slot: {ids}",
        )
        survivors = [slot for slot in ids if slot in idle]
        expect(len(survivors) == 1, f"neither or both idle slots were evicted: {ids}")
        expect(new_slot in ids, "the pressure command took no slot")
    finally:
        at_release = fx.walks()
        release_gated(fx, child, gate, output)
    release = phase_slice(at_release, fx.walks())
    expect(
        same_roots(release, [fx.slot_dir(new_slot)]) and len(release) == 1,
        f"the release walk was not exactly the closed slot: {release}",
    )


def case_f_legacy_convergence_and_crash_replay(root: str, subject: str) -> None:
    case("f_legacy_convergence_and_crash_replay")
    fx = Fixture(root, "f")
    built(fx, subject, fx.crate("first"))
    built(fx, subject, fx.crate("second"))
    unrecorded = [fx.slot_dir(slot) for slot in fx.idle_slots()]
    expect(len(unrecorded) == 2, f"expected two idle slots, saw {unrecorded}")

    # Every record gone and the version-1 fence: the state a previous binary
    # leaves behind.
    document = fx.ledger()
    for slot in document["slots"]:
        slot.pop("recorded", None)
    document["capacity_policy"] = 1
    replace_ledger_quiescent(fx, document)
    read_back = fx.ledger()
    expect(
        read_back["capacity_policy"] == 1
        and all(slot.get("recorded") is None for slot in read_back["slots"]),
        "the ledger edit did not produce the intended legacy shape",
    )

    fx.reset_logs()
    before = fx.walks()
    held_before = set(fx.held_slots())
    child, gate, ready, output = fx.run_gated(subject, fx.crate("third"), "f")
    new_slot = ""
    try:
        new_slot = await_held(fx, child, ready, output, held_before)
        admission = phase_slice(before, fx.walks())
        expect(
            same_roots(admission, unrecorded) and len(admission) == len(unrecorded),
            f"legacy convergence walked {admission}, not exactly {sorted(unrecorded)}",
        )
        published = [slot for slot in fx.idle_slots() if fx.recorded(slot) is not None]
        expect(
            len(published) == len(unrecorded),
            "completed walks were not published before the decision",
        )
    finally:
        release_gated(fx, child, gate, output)

    # A crash replay: kill a genuinely held writer, release its gate, and let
    # the product's own recovery settle the slot. The proof decides nothing
    # about closure itself, and it creates the recovery checkout once so no
    # attempt changes the command's inputs.
    crash_checkout = fx.crate("crash")
    crash_before = set(fx.held_slots())
    crashed, crash_gate, crash_ready, crash_output = fx.run_gated(subject, crash_checkout, "f-crash")
    crashed_slot = await_held(fx, crashed, crash_ready, crash_output, crash_before)
    crashed_record_before = fx.recorded(crashed_slot)
    expect(crashed_record_before is None, "the crashed slot was already recorded while Held")
    crashed.kill()
    crashed.wait()
    with open(crash_gate, "w", encoding="utf-8") as handle:
        handle.write("release\n")
    recovery = fx.crate("recovery")
    deadline = time.monotonic() + work_left()
    recovered = None
    attempts = 0
    while time.monotonic() < deadline:
        attempts += 1
        attempt = fx.run(subject, ["check"], recovery)
        if attempt.returncode == 0:
            recovered = attempt
            break
    if recovered is None:
        # An unsettled crash recovery is an unknown custody outcome, never a
        # recoverable oracle failure: abort the whole collection.
        abort(f"no later command settled the crashed slot in {attempts} bounded attempts")
    for slot in fx.ledger()["slots"]:
        if slot["state"] == "idle":
            expect(
                slot.get("recorded") is not None,
                f"idle slot {slot['id']} was left unrecorded after a settled recovery",
            )
        if slot["state"] == "held":
            expect(
                slot.get("lease_id") is not None,
                "a held slot lost its lease without being recorded",
            )
    # Both settled outcomes are expected and named: the product either idled
    # the dead slot with a fresh record, or retired it whole.
    if crashed_slot in fx.idle_slots():
        expect(
            fx.recorded(crashed_slot) is not None,
            "the crashed slot was idled without a published observation",
        )
        recovery_outcome = f"idled slot {crashed_slot} with a record after {attempts} attempt(s)"
    else:
        expect(
            crashed_slot not in fx.slot_ids(),
            f"the crashed slot is neither idle-with-record nor retired: {fx.slot_ids()}",
        )
        recovery_outcome = f"retired slot {crashed_slot} whole after {attempts} attempt(s)"
    print(f"  case f recovery: {recovery_outcome}", flush=True)
    expect(new_slot in fx.slot_ids(), "the convergence slot disappeared during recovery")


def case_g_status_readonly_and_measure_audit(root: str, subject: str) -> None:
    case("g_status_readonly_and_measure_audit")
    fx = Fixture(root, "g")
    built(fx, subject, fx.crate("first"))

    def digest() -> str:
        digest = hashlib.sha256()
        for base, directories, files in os.walk(fx.pool):
            directories.sort()
            for name in sorted(files):
                path = os.path.join(base, name)
                digest.update(os.path.relpath(path, fx.pool).encode())
                with open(path, "rb") as handle:
                    digest.update(handle.read())
        return digest.hexdigest()

    before_digest = digest()
    fx.reset_logs()
    text = fx.status(subject)
    expect(not fx.walks(), f"default status walked inventory: {fx.walks()}")
    expect(RECORDED_TOTAL_NEEDLE in text, "default status hid the ledger recorded total")
    expect(RECORDED_SLOT_NEEDLE in text, "default status did not label a slot's record")
    expect(digest() == before_digest, "default status mutated the pool")

    fx.reset_logs()
    measured = fx.run(subject, ["status", "--measure"], fx.work)
    expect(measured.returncode == 0, f"measure exit {measured.returncode}: {measured.stderr[-800:]}")
    expect(fx.walks(), "--measure walked nothing, so it is not the audit")
    expect(digest() == before_digest, "--measure mutated the pool")


# --------------------------------------------------------------------------
# The fixed nine-item premise checklist: one disposition per item, each naming
# the executable helper the pure checks exercise (not a text promise).


PREMISE_CHECKLIST = {
    "1_private_home_and_rustup": "invoking_rustup_home()/toolchain_env()/establish_lock()",
    "2_account_home_control": "Fixture.env() sets OXIDE_TEST_HOME_FROM_ENV=1; capability probe",
    "3_environment_isolation": "Fixture.env() scrub of OXIDE_*/XDG_*/flag/TMP overrides",
    "4_automatic_adoption": "not applicable: no case uses permanent outputs or a target/ placement",
    "5_cargo_external_argv": "not applicable: no case invokes an external Cargo subcommand",
    "6_owned_images": "not applicable: internal directory pool only; no image, mount or device",
    "7_compiler_flags": "toolchain_env() frozen RUSTFLAGS, encoded flags removed, recipe_problems()",
    "8_temporary_storage": "internal-root selectors; Fixture.env() TMPDIR inside the owned root",
    "9_macos_mktemp": "not applicable: fixtures use explicit os.makedirs paths, never mktemp",
}
NA_PREMISES = ("4_automatic_adoption", "5_cargo_external_argv", "6_owned_images", "9_macos_mktemp")
# Shared runtime premises every case depends on: a failed one taints the run.
PREMISES = ["toolchain", "staging", "capabilities", "fixture_root"]
DEPENDS = {
    "a_all_recorded_admission_walks_nothing": [],
    "b_missing_record_walks_only_that_slot": ["a_all_recorded_admission_walks_nothing"],
    "c_release_publishes_and_reuse_clears": ["a_all_recorded_admission_walks_nothing"],
    "d_byte_pressure_evicts_only_the_idle_slot": ["c_release_publishes_and_reuse_clears"],
    "e_cross_slot_hard_link_overcount": ["c_release_publishes_and_reuse_clears"],
    "f_legacy_convergence_and_crash_replay": ["b_missing_record_walks_only_that_slot"],
    "g_status_readonly_and_measure_audit": ["c_release_publishes_and_reuse_clears"],
}
CASE_FUNCS = [
    ("a_all_recorded_admission_walks_nothing", case_a_all_recorded_admission_walks_nothing),
    ("b_missing_record_walks_only_that_slot", case_b_missing_record_walks_only_that_slot),
    ("c_release_publishes_and_reuse_clears", case_c_release_publishes_and_reuse_clears),
    ("d_byte_pressure_evicts_only_the_idle_slot", case_d_byte_pressure_evicts_only_the_idle_slot),
    ("e_cross_slot_hard_link_overcount", case_e_cross_slot_hard_link_overcount),
    ("f_legacy_convergence_and_crash_replay", case_f_legacy_convergence_and_crash_replay),
    ("g_status_readonly_and_measure_audit", case_g_status_readonly_and_measure_audit),
]
CASE_NAMES = [name for name, _ in CASE_FUNCS]


# --------------------------------------------------------------------------
# Dispatch.


def dispatch(root: str, subject: str, cases, diagnostic: bool) -> None:
    """Run the fixed cases in dependency order.

    Counted mode propagates the first failure. Diagnostic mode records it,
    skips only cases whose declared prerequisites are now tainted, and stops
    the whole collection on any unknown outcome.
    """
    global ABORTED, PENDING_ABORT
    for name, function in cases:
        blocked = [dependency for dependency in DEPENDS[name] if dependency in TAINTED]
        if blocked:
            SKIPS.append((name, f"dependency failed: {', '.join(blocked)}"))
            TAINTED.add(name)
            print(f"skip {name} (dependency failed: {', '.join(blocked)})", flush=True)
            continue
        PENDING_ABORT = False
        REACHED.append(name)
        if not diagnostic:
            function(root, subject)
            COMPLETED.append(name)
            continue
        try:
            function(root, subject)
        except CaseFailure as failure:
            FAILURES.append((name, str(failure)))
            TAINTED.add(name)
            note_fault(f"{name}: {failure}")
            if failure.abort:
                mark_abort()
                print(f"abort {name}: {failure}", flush=True)
                return
            print(f"failed {name}: {failure}", flush=True)
        except subprocess.TimeoutExpired as timeout:
            FAILURES.append((name, f"child timed out: {timeout}"))
            mark_abort(f"{name}: child timed out")
            print(f"abort {name}: child timed out", flush=True)
            return
        except OSError as error:
            FAILURES.append((name, f"infrastructure error: {error}"))
            mark_abort(f"{name}: infrastructure error: {error}")
            print(f"abort {name}: {error}", flush=True)
            return
        except Exception as error:  # noqa: BLE001 - never an oracle continuation
            # An unexpected exception is this case's failure, recorded before
            # any state or receipt is computed, and it aborts the collection.
            # A reached-but-crashed case is never a pass. Interrupts are
            # BaseException and stay outside this handler.
            FAILURES.append((name, f"unexpected {type(error).__name__}: {error}"))
            TAINTED.add(name)
            mark_abort(f"{name}: unexpected {type(error).__name__}: {error}")
            print(f"abort {name}: unexpected {type(error).__name__}: {error}", flush=True)
            return
        else:
            # Only a normal return completes the case. A BaseException such as
            # an interrupt leaves it reached-but-incomplete, which the state
            # table reports as a failure rather than a pass.
            COMPLETED.append(name)


# --------------------------------------------------------------------------
# Diagnosis receipt. It lives in the guard's evidence directory and is written
# on every outcome, with bounded truth and explicit omissions.


def evidence_topology_problems(evidence_real: str, scratch_real: str) -> list[str]:
    """Why this evidence directory is not the guard's own sibling of scratch.

    The guard creates one owned root holding `scratch/` and `evidence/` as
    siblings and exports that evidence path. Evidence inside the scratch tree
    would be deleted with the scratch it describes, a scratch tree inside the
    evidence directory is the same mistake reversed, a different parent or `/`
    is not one owned root, and a path that only shares characters is not
    contained. Pure, so both directions are falsifiable without any effect.
    """
    problems: list[str] = []
    if evidence_real == scratch_real:
        problems.append("evidence_is_scratch")
    if evidence_real.startswith(scratch_real + os.sep):
        problems.append("evidence_inside_scratch")
    if scratch_real.startswith(evidence_real + os.sep):
        problems.append("scratch_inside_evidence")
    if os.path.dirname(evidence_real) != os.path.dirname(scratch_real):
        problems.append("not_siblings")
    if os.path.dirname(evidence_real) in ("", os.sep):
        problems.append("no_owned_root")
    return problems


def shared_inode_precondition(roots: list[str], key: tuple[int, int]) -> bool:
    """Is this exact (device, inode) present in every one of these trees?

    The fixture's union figure is only meaningful when the trees really do
    share one inode and sit on one device.
    """
    present = 0
    for root in roots:
        found = False
        for base, directories, files in os.walk(root):
            directories.sort()
            for name in sorted(files):
                info = os.lstat(os.path.join(base, name))
                if not os.path.islink(os.path.join(base, name)) and (info.st_dev, info.st_ino) == key:
                    found = True
                    break
            if found:
                break
        present += 1 if found else 0
    return present == len(roots) and len(roots) > 1


def evidence_dir_from(
    environment: dict[str, str],
    scratch: str,
    exists=os.path.isdir,
    resolve=os.path.realpath,
    is_link=os.path.islink,
) -> str:
    """The guard's evidence directory, refused if its provenance is unsafe.

    Only the guard's exact export counts: `OXIDE_PROOF_EVIDENCE`, never a
    `_DIR` spelling, an inherited guess or a scratch-tree parent. A symlink is
    resolved first and must still land on the guard's own sibling topology, so
    a link that escapes the owned root refuses. The filesystem predicates are
    injectable so the pure check drives this same decision with fakes and no
    external effect.
    """
    if "OXIDE_PROOF_EVIDENCE" not in environment:
        if "OXIDE_PROOF_EVIDENCE_DIR" in environment:
            abort(
                "the guard exported OXIDE_PROOF_EVIDENCE_DIR, which is not its"
                " documented OXIDE_PROOF_EVIDENCE; refusing an unverified path"
            )
        abort("the guard exported no OXIDE_PROOF_EVIDENCE directory")
    evidence = environment["OXIDE_PROOF_EVIDENCE"]
    if not evidence:
        abort("the guard exported an empty OXIDE_PROOF_EVIDENCE")
    if not os.path.isabs(evidence):
        abort(f"OXIDE_PROOF_EVIDENCE {evidence!r} is not absolute")
    if not exists(evidence):
        abort(f"OXIDE_PROOF_EVIDENCE {evidence!r} is not a directory")
    evidence_real = resolve(evidence)
    if is_link(evidence) and not exists(evidence_real):
        abort(f"OXIDE_PROOF_EVIDENCE {evidence!r} is a link to a non-directory")
    scratch_real = resolve(scratch)
    problems = evidence_topology_problems(evidence_real, scratch_real)
    if problems:
        abort(
            f"OXIDE_PROOF_EVIDENCE {evidence_real} is not the guard's own evidence"
            f" directory beside scratch {scratch_real}: {problems}"
        )
    return evidence_real


def evidence_dir(scratch: str) -> str:
    """The real environment's evidence directory."""
    return evidence_dir_from(dict(os.environ), scratch)


def bounded_excerpt(value: str) -> str:
    text = value if isinstance(value, str) else str(value)
    return text[:MAX_RECEIPT_EXCERPT]


def case_states() -> dict[str, str]:
    """Exactly one truthful state per planned case."""
    states: dict[str, str] = {}
    failed = {name for name, _ in FAILURES}
    skipped = {name for name, _ in SKIPS}
    for name in CASE_NAMES:
        if name in failed:
            states[name] = "fail"
        elif name in skipped:
            states[name] = "skip"
        elif name in COMPLETED:
            states[name] = "pass"
        elif name in REACHED:
            # Reached but never returned: an exception left the case before it
            # completed (for example a propagated interrupt). Never a pass.
            states[name] = "fail"
        else:
            states[name] = "not_run"
    if ABORTED:
        for name, state in states.items():
            if state == "not_run":
                states[name] = "not_run_aborted"
    return states


def receipt_document(head: str, script_hash: str, guard_hash: str, subject_hash: str | None,
                      evidence: str) -> dict:
    states = case_states()
    counts = {
        "pass": sum(1 for state in states.values() if state == "pass"),
        "fail": sum(1 for state in states.values() if state == "fail"),
        "skip": sum(1 for state in states.values() if state == "skip"),
        "not_run": sum(1 for state in states.values() if state.startswith("not_run")),
        "planned": len(states),
    }
    document = {
        "schema": "oxide.recorded-slot-accounting.receipt/2",
        "mode": "diagnostic",
        "qualified": False,
        "head": head,
        "script_sha256": script_hash,
        "guard_sha256": guard_hash,
        "staged_subject_sha256": subject_hash,
        "toolchain": {"home": bounded_excerpt(os.path.dirname(os.path.dirname(GUARD_PATH))),
                      "installed": TOOLCHAIN_INSTALLED},
        "premises": PREMISES,
        "premise_checklist": PREMISE_CHECKLIST,
        "dependencies": DEPENDS,
        "case_states": states,
        "counts": counts,
        "failures": [(name, bounded_excerpt(reason)) for name, reason in FAILURES][:MAX_RECEIPT_ENTRIES],
        "skips": [(name, bounded_excerpt(reason)) for name, reason in SKIPS][:MAX_RECEIPT_ENTRIES],
        "tainted": sorted(TAINTED),
        "aborted": ABORTED,
        "primary_error": bounded_excerpt(PRIMARY[0]) if PRIMARY else None,
        "secondary_errors": [bounded_excerpt(entry) for entry in SECONDARY][:MAX_RECEIPT_ENTRIES // 4],
        "collection_complete": (not ABORTED) and counts["not_run"] == 0 and counts["skip"] == 0,
        "limits": {
            "wall_limit_s": 300,
            "work_margin_s": SHUTDOWN_MARGIN_S,
            "scratch_budget_mib": 512,
            "staging_jobs": STAGE_JOBS,
            "staging_codegen_units": STAGE_CODEGEN_UNITS,
            "fixture_jobs": FIXTURE_JOBS,
            "receipt_bytes": MAX_RECEIPT_BYTES,
            "receipt_entries": MAX_RECEIPT_ENTRIES,
        },
        "claim": "uncounted diagnostic collection; never a PASS, closure or native credit",
    }
    encoded = json.dumps(document, indent=2, sort_keys=True)
    if len(encoded.encode()) > MAX_RECEIPT_BYTES:
        document["failures"] = document["failures"][: MAX_RECEIPT_ENTRIES // 4]
        document["skips"] = document["skips"][: MAX_RECEIPT_ENTRIES // 4]
        document["secondary_errors"] = document["secondary_errors"][: MAX_RECEIPT_ENTRIES // 8]
        document["omitted"] = "entries and excerpts truncated to the receipt byte bound"
        encoded = json.dumps(document, indent=2, sort_keys=True)
    if len(encoded.encode()) > MAX_RECEIPT_BYTES:
        raise CaseFailure("the diagnostic receipt cannot fit its byte bound", abort=True)
    return document


def write_receipt(evidence: str, document: dict) -> str:
    path = os.path.join(
        evidence,
        f"recorded-slot-accounting-diagnostic-{os.getpid()}-{time.time_ns()}.json",
    )
    try:
        with open(path, "w", encoding="utf-8") as handle:
            json.dump(document, handle, indent=2, sort_keys=True)
    except OSError as error:
        raise CaseFailure(f"cannot write the diagnostic receipt: {error}", abort=True)
    return path


# --------------------------------------------------------------------------
# Effect-free modes.


ALLOCATION = {
    "product_controls": {
        "budget_env": "OXIDE_POOL_CAP",
        "budget_semantics": "soft byte budget applied at pool creation",
        "generous_budget_bytes": GENEROUS_BUDGET,
        "pressure_budget_bytes": PRESSURE_BUDGET,
        "overcount_budget_bytes": OVERCOUNT_BUDGET,
    },
    "phases": {
        "admission": "read while the gated command is Held, before its release",
        "release": "read after the release, separated by the phase slice",
    },
    "staging_estimate_s": 55,
    "cases_estimate_s": 135,
    "cleanup_estimate_s": 15,
    "wall_limit_s": 300,
    "work_margin_s": SHUTDOWN_MARGIN_S,
    "scratch_budget_mib": 512,
    "estimated_peak_mib": 120,
    "staging_jobs": STAGE_JOBS,
    "staging_codegen_units": STAGE_CODEGEN_UNITS,
    "fixture_jobs": FIXTURE_JOBS,
    "cases": CASE_NAMES,
    "measured": False,
}


def self_check() -> None:
    """Pure: real rendering, the recipe validator and the receipt rules."""
    global PURE_MODE
    PURE_MODE = True
    # R1 — the render functions the counted run actually calls.
    rendered = render_build_rs(SLOT_FILLER_BYTES)
    expect("__SLOT_FILLER_BYTES__" not in rendered, "the filler placeholder survived")
    expect(f"vec![7_u8; {SLOT_FILLER_BYTES}]" in rendered, "the filler count is not rendered")
    expect('format!("{}.ready", gate)' in rendered, "a Rust format! placeholder was interpreted")
    expect("if let Ok(gate) = std::env::var(" in rendered, "the gate block was altered")
    expect(rendered.count("{") == rendered.count("}"), "the rendered Rust braces are unbalanced")
    for name, version in (("plain", CRATE_VERSION), ("with space", CRATE_VERSION)):
        manifest = render_manifest(name, version)
        expect("__CRATE_" not in manifest, "a manifest placeholder survived")
        expect(f'name = "{name}"' in manifest, "the manifest name is not rendered")
    for path, expected in (
        ("/tmp/plain/shared.bin", '"/tmp/plain/shared.bin"'),
        ("/tmp/with space/shared.bin", '"/tmp/with space/shared.bin"'),
        ("/tmp/it's/shared.bin", '"/tmp/it\'s/shared.bin"'),
        ('/tmp/quo"te/shared.bin', '"/tmp/quo\\"te/shared.bin"'),
        ("/tmp/back\\slash/shared.bin", '"/tmp/back\\\\slash/shared.bin"'),
        ("/tmp/naïve/shared.bin", '"/tmp/naïve/shared.bin"'),
        ("/tmp/tab\tname/shared.bin", '"/tmp/tab\\tname/shared.bin"'),
    ):
        literal = rust_string_literal(path)
        expect(literal == expected, f"{path!r} rendered as {literal!r}, expected {expected!r}")
        expect(literal.startswith('"') and literal.endswith('"'), "not a quoted literal")
        interior = literal[1:-1]
        escaped = False
        for character in interior:
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == '"':
                fail(f"an unescaped quote survived in {literal!r}")
    # The naive parent forms the proof must never emit.
    tricky = '/tmp/quo"te/shared.bin'
    expect(
        repr(tricky) != rust_string_literal(tricky),
        "the Rust literal is Python repr: an unescaped path would be emitted",
    )
    expect(
        f"'{tricky}'" != rust_string_literal(tricky),
        "the Rust literal is a single-quoted Python literal",
    )
    linked = render_hard_link_build_rs("/tmp/with space/shared.bin")
    expect("/tmp/with space/shared.bin" in linked, "the shared path is missing from the build script")
    expect("'/" not in linked.replace('"', '"'), "a single-quoted path leaked into Rust")
    expect("__SHARED_PATH__" not in linked, "the shared-path placeholder survived")

    # R2 — the real recipe validator, one mutation at a time.
    # A real, absolute, existing directory stands in for the toolchain home,
    # so the validator runs the same check the counted staging does.
    stage = "/tmp/nonexistent-recorded-accounting/stage"
    argv, env = staging_recipe("/tmp/nonexistent-recorded-accounting", rustup_home=REPO)
    expect(
        recipe_problems(argv, env, stage) == [],
        f"the recipe is not the reviewed one: {recipe_problems(argv, env, stage)}",
    )
    def with_env(key: str, value: str):
        def mutate(argv: list[str], env: dict[str, str]):
            changed = dict(env)
            changed[key] = value
            return argv, changed
        return mutate

    def with_jobs(value: str):
        def mutate(argv: list[str], env: dict[str, str]):
            changed = list(argv)
            changed[changed.index("-j") + 1] = value
            return changed, env
        return mutate

    def without_flag(flag: str):
        def mutate(argv: list[str], env: dict[str, str]):
            return [item for item in argv if item != flag], env
        return mutate

    mutations = [
        ("job_count", with_jobs("1")),
        ("argv_missing_locked", without_flag("--locked")),
        ("argv_missing_offline", without_flag("--offline")),
        ("env_cargo_profile_dev_codegen_units", with_env("CARGO_PROFILE_DEV_CODEGEN_UNITS", "1")),
        ("env_cargo_profile_dev_debug", with_env("CARGO_PROFILE_DEV_DEBUG", "2")),
        ("env_cargo_profile_dev_debug_assertions", with_env("CARGO_PROFILE_DEV_DEBUG_ASSERTIONS", "false")),
        ("env_cargo_profile_dev_opt_level", with_env("CARGO_PROFILE_DEV_OPT_LEVEL", "3")),
        ("env_cargo_incremental", with_env("CARGO_INCREMENTAL", "1")),
        ("env_rustup_toolchain", with_env("RUSTUP_TOOLCHAIN", "stable")),
        ("env_rustup_auto_install", with_env("RUSTUP_AUTO_INSTALL", "1")),
        ("env_cargo_net_offline", with_env("CARGO_NET_OFFLINE", "false")),
        ("rustflags", with_env("RUSTFLAGS", "")),
        ("encoded_rustflags_present", with_env("CARGO_ENCODED_RUSTFLAGS", "-Cdebuginfo=2")),
        ("rustup_home_unusable", with_env("RUSTUP_HOME", "relative/not-absolute")),
        ("cargo_target_dir_outside_stage", with_env("CARGO_TARGET_DIR", "/tmp/elsewhere/target")),
    ]
    for expected_problem, mutate in mutations:
        mutated_argv, mutated_env = mutate(list(argv), dict(env))
        problems = recipe_problems(mutated_argv, mutated_env, stage)
        expect(
            expected_problem in problems,
            f"a {expected_problem} regression was not detected: {problems}",
        )

    # R2 — the lock predicate, and relative toolchain provenance.
    good_lock = (
        "# This file is automatically @generated by Cargo.\n"
        "# It is not intended for manual editing.\n"
        'version = 3\n\n[[package]]\nname = "fixture"\nversion = "0.1.0"\n'
    )
    expect(lock_is_manifest_consistent(good_lock, "fixture", CRATE_VERSION), "a valid lock was refused")
    for broken in (
        good_lock.replace("# This file is automatically @generated by Cargo.\n", ""),
        good_lock.replace('name = "fixture"', 'name = "other"'),
        good_lock.replace(f'version = "{CRATE_VERSION}"', 'version = "9.9.9"'),
    ):
        expect(
            not lock_is_manifest_consistent(broken, "fixture", CRATE_VERSION),
            "a broken lock was accepted",
        )

    # R3/R4 — the phase helpers the cases use.
    expect(phase_slice([], ["a"]) == ["a"], "the phase slice lost a first line")
    expect(phase_slice(["a"], ["a", "b"]) == ["b"], "the phase slice did not isolate the phase")
    try:
        phase_slice(["a", "b"], ["b"])
        fail("a lost phase line was not refused")
    except CaseFailure as failure:
        expect(failure.abort, "a lost phase line must abort, not continue")
    expect(distinct(["b", "a", "b"]) == ["a", "b"], "distinct roots are not a stable set")
    expect(
        phase_slice(["admission"], ["admission", "release"]) != ["admission"],
        "the phase slice cannot tell an admission walk from a release walk",
    )

    # R5 — the receipt bounds and the evidence-path refusals.
    document = receipt_document("a" * 40, "b" * 64, GUARD_SHA256, "c" * 64, "/nonexistent")
    expect(document["mode"] == "diagnostic", "the receipt is not a diagnostic receipt")
    expect(document["qualified"] is False, "a diagnostic receipt claimed qualification")
    expect(len(json.dumps(document).encode()) <= MAX_RECEIPT_BYTES, "the receipt exceeds its bound")
    expect(document["counts"]["planned"] == len(CASE_NAMES), "the receipt lost planned cases")
    long_failure = "x" * (MAX_RECEIPT_EXCERPT * 4)
    expect(len(bounded_excerpt(long_failure)) == MAX_RECEIPT_EXCERPT, "the excerpt bound is not applied")
    saved_evidence = os.environ.get("OXIDE_PROOF_EVIDENCE")
    try:
        os.environ.pop("OXIDE_PROOF_EVIDENCE", None)
        try:
            evidence_dir("/private/tmp/nowhere/scratch")
            fail("a missing OXIDE_PROOF_EVIDENCE was accepted")
        except CaseFailure as failure:
            expect(failure.abort, "a missing evidence directory must abort")
        os.environ["OXIDE_PROOF_EVIDENCE"] = "relative/evidence"
        try:
            evidence_dir("/private/tmp/nowhere/scratch")
            fail("a relative OXIDE_PROOF_EVIDENCE was accepted")
        except CaseFailure as failure:
            expect(failure.abort, "a relative evidence directory must abort")
        os.environ["OXIDE_PROOF_EVIDENCE"] = "/nonexistent/evidence"
        try:
            evidence_dir("/private/tmp/nowhere/scratch")
            fail("a nonexistent OXIDE_PROOF_EVIDENCE was accepted")
        except CaseFailure as failure:
            expect(failure.abort, "a nonexistent evidence directory must abort")
    finally:
        if saved_evidence is None:
            os.environ.pop("OXIDE_PROOF_EVIDENCE", None)
        else:
            os.environ["OXIDE_PROOF_EVIDENCE"] = saved_evidence

    # ROOT observation 1 — a fresh readiness marker cannot bind an arbitrary
    # held row: exactly one new lease must appear, whatever the row order.
    expect(select_new_held(["7", "9"], {"7"}) == "9", "the new held lease was not selected")
    expect(select_new_held(["9", "7"], {"7"}) == "9", "the selection depends on row order")
    expect(select_new_held(["2"], set()) == "2", "a single new held lease was not selected")
    for held, before, label in (
        (["7"], {"7"}, "no new lease"),
        (["7", "8"], set(), "two new leases"),
        ([], set(), "no leases at all"),
    ):
        try:
            select_new_held(held, before)
            fail(f"an ambiguous held selection was accepted ({label})")
        except CaseFailure as failure:
            expect(failure.abort, f"{label} must abort, not continue")

    # ROOT observation 3 — the rendered recorded-total grammar, exactly.
    good = (
        "  recorded slot sizes (ledger): 42.0 MiB across 2 of 2 idle slots;"
        " the admission basis, not free space"
    )
    expect(recorded_total_grammar(good), "a realistic recorded-total line was rejected")
    expect(recorded_total_counts(good) == (2, 2), "the counts were not parsed from the line")
    expect(recorded_total_line("junk\n" + good + "\nmore") == good.strip(), "the line was not found")
    for bad, label in (
        ("  recorded slot sizes (ledger): 42.0 MiB across 2 of 3 idle slots", "partial counts"),
        ("  recorded slot sizes (ledger): 42.0 MiB", "missing counts"),
        ("  recorded slot sizes (ledger): 42.0 MiB across two of two idle slots", "words not digits"),
        ("  payload unique allocated bytes: 42", "unrelated payload line"),
        ("  recorded slot sizes (ledger): none recorded yet", "the absent-total label"),
        ("", "an empty status"),
    ):
        if label == "partial counts":
            expect(
                recorded_total_counts(bad) == (2, 3) and recorded_total_grammar(bad),
                "a partial-counts line was not parsed as such",
            )
            continue
        expect(not recorded_total_grammar(bad), f"{label} was accepted as a recorded total")

    # ROOT evidence boundary — the sibling topology, both directions.
    root = "/private/tmp/oxide-task.abc"
    scratch = root + "/scratch"
    expect(evidence_topology_problems(root + "/evidence", scratch) == [], "the guard layout was refused")
    for bad, expected, label in (
        (scratch, "evidence_is_scratch", "evidence equals scratch"),
        (scratch + "/evidence", "evidence_inside_scratch", "evidence inside the scratch tree"),
        (root, "scratch_inside_evidence", "scratch inside evidence"),
        ("/private/tmp/other/evidence", "not_siblings", "another root"),
        ("/evidence", "no_owned_root", "a filesystem-root parent"),
        ("/private/tmp/oxide-task.abc-evil/evidence", "not_siblings", "a shared-prefix decoy"),
    ):
        problems = evidence_topology_problems(bad, scratch)
        expect(expected in problems, f"{label} was accepted: {problems}")

    # The pin is proven from `<home>/toolchains/<pin>-<host>`, never from the
    # home alone: an absent pin, a relative home and a missing toolchains
    # directory each refuse with their own reason.
    expect(
        "pinned_toolchain_absent"
        in toolchain_pin_problems(REPO, ["stable-aarch64-apple-darwin"]),
        "a toolchains listing without the pin was accepted",
    )
    expect(
        "toolchain_home_not_absolute"
        in toolchain_pin_problems("relative/home", [BUILDER + "-aarch64-apple-darwin"]),
        "a relative toolchain home was accepted",
    )
    expect(
        "toolchains_directory_absent"
        in toolchain_pin_problems("/nonexistent-rustup-home", [BUILDER + "-host"]),
        "a home without a toolchains directory was accepted",
    )

    # The evidence boundary refuses the wrong export name, a symlink that
    # escapes, and a symlink to a non-directory — all through the real
    # decision with injected filesystem predicates and no external effect.
    guard_root = "/private/tmp/oxide-task.abc"
    scratch_path = guard_root + "/scratch"
    for environment, exists, resolve, is_link, expected, label in (
        ({"OXIDE_PROOF_EVIDENCE_DIR": guard_root + "/evidence"}, lambda path: True,
         lambda path: path, lambda path: False, "OXIDE_PROOF_EVIDENCE_DIR", "_DIR-only export"),
        ({}, lambda path: True, lambda path: path, lambda path: False,
         "no OXIDE_PROOF_EVIDENCE", "missing export"),
        ({"OXIDE_PROOF_EVIDENCE": "relative/evidence"}, lambda path: True,
         lambda path: path, lambda path: False, "is not absolute", "relative export"),
        ({"OXIDE_PROOF_EVIDENCE": guard_root + "/evidence"}, lambda path: False,
         lambda path: path, lambda path: False, "is not a directory", "absent directory"),
        ({"OXIDE_PROOF_EVIDENCE": guard_root + "/evidence"}, lambda path: True,
         lambda path: "/private/tmp/elsewhere/evidence", lambda path: True,
         "not the guard's own evidence", "symlink escaping the owned root"),
        ({"OXIDE_PROOF_EVIDENCE": guard_root + "/evidence"}, lambda path: "/target" not in path,
         lambda path: "/target", lambda path: True, "link to a non-directory",
         "symlink to a non-directory"),
    ):
        try:
            evidence_dir_from(environment, scratch_path, exists=exists, resolve=resolve, is_link=is_link)
            fail(f"an unsafe evidence provenance was accepted ({label})")
        except CaseFailure as failure:
            expect(failure.abort, f"{label} must abort before effects")
            expect(expected in str(failure), f"{label} reported {failure!r}, not {expected!r}")
    expect(
        evidence_dir_from(
            {"OXIDE_PROOF_EVIDENCE": guard_root + "/evidence"},
            scratch_path,
            exists=lambda path: True,
            resolve=lambda path: path,
            is_link=lambda path: False,
        )
        == guard_root + "/evidence",
        "the guard's own evidence sibling was refused",
    )
    expect(
        not shared_inode_precondition(["/nonexistent-a"], (0, 0)),
        "a single tree cannot satisfy the shared-inode precondition",
    )

    # The map, the case set and the fixed premises.
    expect(len(PREMISE_CHECKLIST) == 9, "the premise checklist is incomplete")
    expect(set(NA_PREMISES) <= set(PREMISE_CHECKLIST), "a not-applicable premise is missing")
    for item in PREMISE_CHECKLIST:
        if item in NA_PREMISES:
            expect("not applicable" in PREMISE_CHECKLIST[item], f"{item} is not an honest N/A")
        else:
            expect("(" in PREMISE_CHECKLIST[item], f"{item} names no executable helper")
    expect(len(CASE_FUNCS) == 7, "the fixed case set changed")
    expect(all(name in DEPENDS for name in CASE_NAMES), "a case has no dependency entry")
    expect(
        all(dependency in CASE_NAMES for deps in DEPENDS.values() for dependency in deps),
        "a dependency names a case that does not exist",
    )
    for attribute in ("command", "cwd", "prerequisite", "Observed phase"):
        expect(attribute in (__doc__ or ""), f"the case map does not name {attribute}")
    expect(ALLOCATION["measured"] is False, "the estimate must not claim measurement")
    # The executable pin must match the runner actually in this tree, and the
    # owning docs must name the same bytes. This is the cross-file seam a
    # prefix-only pin check missed when the qualified oxide#147 runner replaced
    # its predecessor: measure the real file and a real mismatch, no subprocess.
    expect(len(GUARD_SHA256) == 64 and all(c in "0123456789abcdef" for c in GUARD_SHA256),
           "the guard pin is not a sha256 hex digest")
    measured = sha256_file(GUARD_PATH)
    expect(measured == GUARD_SHA256,
           f"the guard pin {GUARD_SHA256} is not the runner in this tree ({measured})")
    expect(guard_identity() == measured, "guard_identity disagrees with the measured runner")
    for document in ("docs/knowledge/contracts/workflow.md",
                     "docs/knowledge/contracts/local-lease.md"):
        with open(os.path.join(REPO, document), encoding="utf-8") as handle:
            expect(measured in handle.read(),
                   f"{document} does not name the actual qualified supervisor bytes")
    # Negative: any other pin is refused before effects, naming both hashes.
    try:
        guard_identity(GUARD_PATH, "0" * 64)
    except CaseFailure as failure:
        expect(failure.abort, "a mismatched guard pin must abort, not continue")
        expect(GUARD_SHA256 in str(failure) and "0" * 64 in str(failure),
               f"the refusal does not name both pins: {failure}")
    else:
        fail("a mismatched guard pin was accepted")
    print("self-check: ok (rendering, recipe validator, phase/receipt rules; not evidence)")


def self_test() -> None:
    """Pure: the diagnostic decision machine, with no child, image or pool."""
    global ABORTED, PURE_MODE
    PURE_MODE = True
    saved = (list(FAILURES), list(SKIPS), list(REACHED), set(TAINTED), list(PRIMARY),
             list(SECONDARY), ABORTED)

    def reset() -> None:
        global ABORTED, PENDING_ABORT
        PENDING_ABORT = False
        ABORTED = False
        FAILURES.clear()
        SKIPS.clear()
        REACHED.clear()
        COMPLETED.clear()
        TAINTED.clear()
        PRIMARY.clear()
        SECONDARY.clear()
        ABORTED = False

    class FakeChild:
        """A child stand-in for the release/custody decision only."""

        def __init__(self, returncode: int = 0, hang: bool = False) -> None:
            self.returncode = returncode
            self.hang = hang
            self.killed = False

        def wait(self, timeout: float | None = None) -> int:
            if self.hang:
                raise subprocess.TimeoutExpired("fake-oxide", timeout or 0)
            return self.returncode

        def kill(self) -> None:
            self.killed = True
            self.hang = False
            self.returncode = -9

    def stubs(failing: str, aborting: bool, calls: list[str]) -> list[tuple[str, object]]:
        made = []
        for name, _ in CASE_FUNCS:
            if name == failing:
                def explode(root, subject, message=failing, hard=aborting):
                    calls.append(f"{message}:raised")
                    raise CaseFailure(f"{message} oracle", hard)
                made.append((name, explode))
            else:
                made.append((name, (lambda label: lambda root, subject: calls.append(label))(name)))
        return made

    try:
        # A recoverable oracle failure in a leaf taints only itself.
        reset()
        calls: list[str] = []
        dispatch("stub", "stub", stubs("g_status_readonly_and_measure_audit", False, calls), True)
        states = case_states()
        expect(states["g_status_readonly_and_measure_audit"] == "fail", "the fault was not recorded")
        expect(states["c_release_publishes_and_reuse_clears"] == "pass", "an independent case did not pass")
        expect(not SKIPS, f"a leaf failure skipped independent cases: {SKIPS}")
        expect(not ABORTED, "a recoverable failure aborted collection")
        expect(PRIMARY and PRIMARY[0].startswith("g_status"), "the primary error was lost")
        expect(
            sum(1 for state in states.values() if state == "pass") == 6,
            f"the pass count is wrong: {states}",
        )

        # A shared prerequisite's failure skips every dependent, transitively.
        reset()
        dispatch("stub", "stub", stubs("a_all_recorded_admission_walks_nothing", False, []), True)
        skipped = {name for name, _ in SKIPS}
        expect(len(skipped) == 6, f"the taint set is not the full dependent closure: {skipped}")
        states = case_states()
        expect(states["a_all_recorded_admission_walks_nothing"] == "fail", "the root fault is not a fail")
        expect(states["b_missing_record_walks_only_that_slot"] == "skip", "a dependent is not a skip")
        expect(
            sum(1 for state in states.values() if state == "pass") == 0,
            "a skipped case was also counted as a pass",
        )

        # Counted mode still fails fast: the first failure propagates.
        reset()
        try:
            dispatch("stub", "stub", stubs("a_all_recorded_admission_walks_nothing", False, []), False)
        except CaseFailure:
            pass
        else:
            fail("counted dispatch did not propagate its first failure")
        reset()

        # An unknown outcome aborts, even after an otherwise successful case,
        # and every case keeps exactly one truthful state.
        calls.clear()
        dispatch("stub", "stub", stubs("b_missing_record_walks_only_that_slot", True, calls), True)
        expect(ABORTED, "an unknown outcome did not abort")
        expect("a_all_recorded_admission_walks_nothing" in calls, "the earlier case did not run")
        expect(calls[-1].endswith(":raised"), "the collection continued past the abort")
        states = case_states()
        expect(states["a_all_recorded_admission_walks_nothing"] == "pass", "the earlier pass was lost")
        expect(states["b_missing_record_walks_only_that_slot"] == "fail", "the unknown is not a fail")
        expect(
            states["c_release_publishes_and_reuse_clears"] == "not_run_aborted",
            "a case after the abort is not marked not_run_aborted",
        )
        expect(len(FAILURES) == 1, "an abort recorded more than the first fault")
        document = receipt_document("a" * 40, "b" * 64, GUARD_SHA256, None, "/nonexistent")
        expect(document["aborted"] is True, "the receipt hid the abort")
        expect(document["collection_complete"] is False, "an aborted receipt claimed completeness")
        expect(document["case_states"] == states, "the receipt states disagree with the run")

        # F1: the internal-pool fact is the pool's serialized `config.json`
        # backend, bound to product source. The ledger struct has no backend
        # field, which is exactly what used to crash case D.
        pool_source = open(os.path.join(REPO, "src", "lease", "pool.rs"), encoding="utf-8").read()
        mod_source = open(os.path.join(REPO, "src", "lease", "mod.rs"), encoding="utf-8").read()
        expect(f'const CONFIG_NAME: &str = "{CONFIG_NAME}";' in mod_source,
               f"the config name {CONFIG_NAME!r} is not the product's")
        expect(f'BUDGET_BACKEND_DIRECTORY_V1: &str = "{BUDGET_BACKEND}";' in mod_source,
               f"the internal backend tag {BUDGET_BACKEND!r} is not the product's")
        expect(f'VOLUME_BACKEND_APFS_V1: &str = "{VOLUME_BACKEND}";' in mod_source,
               f"the removable backend tag {VOLUME_BACKEND!r} is not the product's")
        ledger_struct = pool_source.split("struct Ledger {", 1)[1].split("\n}", 1)[0]
        expect("backend" not in ledger_struct,
               "the ledger now records a backend: rebind the F1 premise before trusting it again")
        expect(
            recorded_backend({"version": 5, "backend": BUDGET_BACKEND, "cap_bytes": 1}) == BUDGET_BACKEND,
            "the real internal-pool config was not accepted",
        )
        for rejected in (
            {"version": 5, "store_id": "x", "cap_bytes": 1, "next_slot": 1, "next_lease": 1, "slots": []},
            {}, {"backend": ""}, {"backend": None},
            {"backend": VOLUME_BACKEND, "volume": {"mount": "/Volumes/x"}},
            {"backend": "apfs-sparse-image-v1"},
            {"backend": BUDGET_BACKEND, "volume": {"mount": "/Volumes/x"}},
            [], None, "backend",
        ):
            try:
                recorded_backend(rejected)
            except CaseFailure as failure:
                expect(failure.abort, f"a wrong backend was treated as recoverable: {rejected!r}")
            else:
                fail(f"a pool without a recorded internal backend was accepted: {rejected!r}")

        # F2: a case that dies of an unexpected exception is a failure, never a
        # pass, and the remaining cases are explicitly not run after the abort.
        middle = CASE_NAMES[3]

        def middle_cases(failing, error):
            made = []
            for name in CASE_NAMES:
                if name == failing:
                    def explode(root, subject, error=error):
                        raise error
                    made.append((name, explode))
                else:
                    made.append((name, (lambda r, s: None)))
            return made

        for error, expected in ((KeyError("backend"), "KeyError"),
                                (RuntimeError("a custom unexpected failure"), "RuntimeError")):
            reset()
            dispatch("stub", "stub", middle_cases(middle, error), True)
            states = case_states()
            expect(ABORTED, f"an unexpected {expected} did not abort the collection")
            expect(states[middle] == "fail", f"a crashed case is not a fail: {states[middle]}")
            expect(
                sum(1 for state in states.values() if state == "pass")
                == CASE_NAMES.index(middle),
                f"the crashed case or a later one was counted as a pass: {states}",
            )
            expect(
                all(states[name] == "not_run_aborted" for name in CASE_NAMES[CASE_NAMES.index(middle) + 1:]),
                f"cases after the crash are not not_run_aborted: {states}",
            )
            expect(any(f"unexpected {expected}" in reason for _, reason in FAILURES),
                   f"the crash is not recorded as a failure: {FAILURES}")
            expect(PRIMARY and expected in PRIMARY[0],
                   f"the crash is not the primary fault: {PRIMARY}")
            document = receipt_document("a" * 40, "b" * 64, GUARD_SHA256, None, "/nonexistent")
            expect(document["aborted"] is True, "the receipt hid an unexpected-exception abort")
            expect(document["case_states"] == states, "the receipt states disagree with the crash")
            expect(document["counts"]["fail"] == 1, f"the receipt miscounted the crash: {document['counts']}")
            expect(document["collection_complete"] is False, "a crashed collection claimed completeness")

        # F2, last planned case: no trailing case exists to hide the truth bug.
        reset()
        dispatch("stub", "stub", middle_cases(CASE_NAMES[-1], KeyError("backend")), True)
        states = case_states()
        expect(states[CASE_NAMES[-1]] == "fail", "a crash in the last case was reported as a pass")
        expect(ABORTED, "a crash in the last case did not abort")
        expect(
            sum(1 for state in states.values() if state == "pass") == len(CASE_NAMES) - 1,
            f"the last-case crash was counted as a pass: {states}",
        )
        document = receipt_document("a" * 40, "b" * 64, GUARD_SHA256, None, "/nonexistent")
        expect(document["counts"]["not_run"] == 0 and document["aborted"] is True,
               f"the last-case crash receipt is wrong: {document['counts']}")

        # F2, setup failure before any case: every case is explicitly not run.
        reset()
        mark_abort("unknown outcome: the staged candidate could not be built")
        states = case_states()
        expect(all(state == "not_run_aborted" for state in states.values()),
               f"a setup failure did not mark every case not_run_aborted: {states}")
        document = receipt_document("a" * 40, "b" * 64, GUARD_SHA256, None, "/nonexistent")
        expect(document["aborted"] is True and document["collection_complete"] is False,
               "a setup failure produced a dishonest receipt")

        # Completion truth: PASS requires that the case returned. A propagated
        # interrupt leaves the case reached-but-incomplete, so it is never a
        # pass, and main's real boundary marks the abort before any receipt.
        for interrupt in (KeyboardInterrupt(), SystemExit(3)):
            for position in ("middle", "last"):
                reset()
                failing = CASE_NAMES[3] if position == "middle" else CASE_NAMES[-1]

                def interrupted(root, subject, error=interrupt):
                    raise error

                made = [(name, interrupted) if name == failing else (name, (lambda r, s: None))
                        for name in CASE_NAMES]
                try:
                    dispatch("stub", "stub", made, True)
                except BaseException as escaped:  # exactly what main() catches
                    expect(isinstance(escaped, type(interrupt)),
                           f"dispatch swallowed a {type(interrupt).__name__}")
                    record_unknown(escaped)  # main's real abort boundary
                else:
                    fail(f"a {type(interrupt).__name__} did not escape dispatch")
                states = case_states()
                expect(ABORTED, f"a {type(interrupt).__name__} did not abort")
                expect(states[failing] == "fail",
                       f"an interrupted {position} case is reported {states[failing]}, not fail")
                expect(
                    sum(1 for state in states.values() if state == "pass")
                    == CASE_NAMES.index(failing),
                    f"an interrupted {position} case or a later one was counted as a pass: {states}",
                )
                expect(
                    all(states[name] == "not_run_aborted"
                        for name in CASE_NAMES[CASE_NAMES.index(failing) + 1:]),
                    f"cases after an interrupted {position} case are not not_run_aborted: {states}",
                )
                expect(PRIMARY and type(interrupt).__name__ in PRIMARY[0],
                       f"the interrupt is not the primary fault: {PRIMARY}")
                document = receipt_document("a" * 40, "b" * 64, GUARD_SHA256, None, "/nonexistent")
                expect(document["aborted"] is True, "the receipt hid an interrupted collection")
                expect(document["case_states"] == states, "the receipt states disagree with the interrupt")
                expect(document["collection_complete"] is False,
                       "an interrupted collection claimed completeness")
                expect(document["counts"]["pass"] == CASE_NAMES.index(failing),
                       f"the receipt counted an interrupted case as a pass: {document['counts']}")

        # The old REACHED==PASS shape must fail: a reached case that never
        # returned is not success, even with no recorded failure.
        reset()
        REACHED.append(CASE_NAMES[0])
        COMPLETED.clear()
        expect(case_states()[CASE_NAMES[0]] == "fail",
               "a reached-but-unreturned case was reported as a pass")
        expect("ABORTED =" not in inspect.getsource(main) and "record_unknown(" in inspect.getsource(main),
               "main does not use the shared unknown/abort boundary")

        # F3: one real abort state. The old bug was a shadowing local assignment
        # in main, which left the durable receipt saying `aborted: false`.
        reset()
        def shadowing_local() -> bool:
            ABORTED = True  # noqa: F841 - the bug shape: a local, not the shared state
            return ABORTED
        expect(shadowing_local() is True, "the counterfactual did not shadow a local")
        expect(ABORTED is False, "a shadowing local silently changed the shared abort state")
        mark_abort("unknown outcome: KeyError: 'backend'")
        expect(ABORTED is True, "the shared abort transition did not set the state")
        document = receipt_document("a" * 40, "b" * 64, GUARD_SHA256, None, "/nonexistent")
        expect(document["aborted"] is True,
               "the durable receipt disagrees with the console abort state")
        expect("ABORTED =" not in inspect.getsource(main),
               "main assigns a shadowing local ABORTED instead of using mark_abort")
        expect("mark_abort(" in inspect.getsource(main),
               "main does not use the shared abort transition")
        expect("ABORTED =" not in inspect.getsource(dispatch),
               "dispatch assigns ABORTED outside the shared transition")

        # R6: an unknown outcome is never downgraded by the teardown after it.
        reset()
        hanging = FakeChild(returncode=-9, hang=True)
        try:
            try:
                abort("await_held timed out: the gated writer never reached a held slot")
            finally:
                release_gated("fixture", hanging, "/nonexistent-gate", "/nonexistent-out",
                              signal=lambda: None)
        except CaseFailure as failure:
            expect(failure.abort, "the teardown downgraded the unknown outcome")
        expect(PENDING_ABORT, "the abort latch was cleared by the teardown")
        expect("timed out" in (PRIMARY[0] if PRIMARY else ""),
               f"the original timeout was not the primary fault: {PRIMARY}")
        expect(any("forced teardown" in entry for entry in SECONDARY),
               f"the teardown failure is not a visible secondary fault: {SECONDARY}")
        expect(hanging.killed, "the hanging writer was not torn down")

        # A release that fails with nothing unknown in flight stays recoverable.
        reset()
        try:
            release_gated("fixture", FakeChild(returncode=7), "/nonexistent-gate",
                          "/nonexistent-out", signal=lambda: None)
        except CaseFailure as failure:
            expect(not failure.abort, "a plain release failure must stay recoverable")
            expect("exited 7" in str(failure), f"the release failure lost its exit: {failure}")
        else:
            fail("a non-zero release exit was accepted")

        # The same failure after a latched unknown aborts instead.
        reset()
        try:
            try:
                abort("await_held timed out")
            finally:
                release_gated("fixture", FakeChild(returncode=7), "/nonexistent-gate",
                              "/nonexistent-out", signal=lambda: None)
        except CaseFailure as failure:
            expect(failure.abort, "a latched unknown was downgraded by a failed release")

        # Through the real dispatcher: the latched abort keeps every later case
        # explicitly not run, and the receipt tells the same story.
        reset()
        ordered: list[str] = []

        def timed_out(root, subject):
            ordered.append("a")
            try:
                abort("await_held timed out")
            finally:
                release_gated("fixture", FakeChild(returncode=7), "/nonexistent",
                              "/nonexistent", signal=lambda: None)

        stub_cases = [("a_all_recorded_admission_walks_nothing", timed_out)]
        for name, _ in CASE_FUNCS[1:]:
            stub_cases.append((name, (lambda label: lambda root, subject: ordered.append(label))(name)))
        dispatch("stub", "stub", stub_cases, True)
        states = case_states()
        expect(ABORTED, "the collection did not abort")
        expect(ordered == ["a"], f"a case ran after the unknown outcome: {ordered}")
        expect(states["a_all_recorded_admission_walks_nothing"] == "fail", states)
        expect(all(states[name] == "not_run_aborted" for name in CASE_NAMES[1:]), states)
        document = receipt_document("a" * 40, "b" * 64, GUARD_SHA256, None, "/nonexistent")
        expect(document["aborted"] is True, "the receipt hid the abort")
        expect("timed out" in (document["primary_error"] or ""),
               f"the receipt lost the original fault: {document['primary_error']}")

        # A receipt that cannot fit its bound with the caps applied aborts.
        reset()
        REACHED.extend(CASE_NAMES)
        FAILURES.extend((name, "y" * (MAX_RECEIPT_EXCERPT * 3)) for name in CASE_NAMES)
        try:
            receipt_document("a" * 40, "b" * 64, GUARD_SHA256, None, "/nonexistent")
        except CaseFailure as failure:
            expect(failure.abort, "an unbounded receipt must abort")
        else:
            document = receipt_document("a" * 40, "b" * 64, GUARD_SHA256, None, "/nonexistent")
            expect(len(json.dumps(document).encode()) <= MAX_RECEIPT_BYTES, "the receipt bound failed")
    finally:
        FAILURES[:], SKIPS[:], REACHED[:], restored, PRIMARY[:], SECONDARY[:], ABORTED = saved
        TAINTED.clear()
        TAINTED.update(restored)
    print("self-test: ok (dispatch, taint, abort, states and receipt bounds; not evidence)")


# --------------------------------------------------------------------------
# Entry point.


def main() -> None:
    arguments = [argument for argument in sys.argv[1:] if argument != DIAGNOSTIC]
    if arguments == ["--plan"]:
        print(json.dumps(ALLOCATION, indent=2, sort_keys=True))
        return
    if arguments == ["--self-check"]:
        self_check()
        return
    if arguments == ["--self-test"]:
        self_test()
        return
    if arguments:
        print(f"unknown argument: {arguments[0]}", file=sys.stderr)
        raise SystemExit(2)
    if not os.environ.get("OXIDE_TASK_SCRATCH_ROOT"):
        print("this proof runs only under the qualified shared supervisor", file=sys.stderr)
        raise SystemExit(2)

    root = os.path.abspath(os.environ["OXIDE_TASK_SCRATCH_ROOT"])
    head = ""
    script_hash = sha256_file(os.path.abspath(__file__))
    guard_hash = ""
    subject_hash: str | None = None
    evidence = ""
    exit_code = 0
    try:
        # Identity and provenance before any effect.
        head = source_identity()
        guard_hash = guard_identity()
        evidence = evidence_dir(root)
        global INVOKING_RUSTUP_HOME, TOOLCHAIN_INSTALLED
        INVOKING_RUSTUP_HOME = invoking_rustup_home()
        TOOLCHAIN_INSTALLED = installed_toolchain(INVOKING_RUSTUP_HOME)
        scratch = os.path.join(root, "recorded-accounting")
        os.makedirs(scratch, exist_ok=True)
        os.chmod(scratch, 0o700)
        subject = stage_subject(scratch)
        subject_hash = sha256_file(subject)
        probe = subprocess.run(
            [subject, CAPABILITIES_PROBE],
            env={**os.environ, "OXIDE_TEST_HOME_FROM_ENV": "1"},
            capture_output=True,
            text=True,
            timeout=child_timeout(30),
        )
        if probe.returncode != 0 or '"test_controls":true' not in probe.stdout.replace(" ", ""):
            abort(f"the staged candidate has no test controls: {probe.stdout!r} {probe.stderr!r}")
        dispatch(scratch, subject, CASE_FUNCS, DIAGNOSTIC_MODE)
        if DIAGNOSTIC_MODE:
            states = case_states()
            if any(state != "pass" for state in states.values()):
                exit_code = 1
    except CaseFailure as failure:
        mark_abort(str(failure))
        if DIAGNOSTIC_MODE:
            exit_code = 1
        else:
            print(f"ABORTED: {failure}", flush=True)
            raise SystemExit(1)
    except BaseException as error:  # noqa: BLE001 - re-raised after the receipt
        # An unrecordable or unexpected outcome is never an oracle: record it,
        # then re-raise so the failure is not swallowed.
        record_unknown(error)
        if DIAGNOSTIC_MODE:
            exit_code = 1
        else:
            raise
    finally:
        if DIAGNOSTIC_MODE and evidence:
            try:
                document = receipt_document(head, script_hash, guard_hash, subject_hash or "", evidence)
                path = write_receipt(evidence, document)
                print(f"diagnostic receipt: {path}")
            except CaseFailure as receipt_failure:
                SECONDARY.append(f"receipt: {receipt_failure}")
                print(f"diagnostic receipt failed: {receipt_failure}", flush=True)
                exit_code = 1

    if DIAGNOSTIC_MODE:
        print(
            json.dumps(
                {
                    "counts": case_states(),
                    "aborted": ABORTED,
                    "primary_error": PRIMARY[0] if PRIMARY else None,
                    "qualified": False,
                },
                sort_keys=True,
            )
        )
        raise SystemExit(exit_code)
    for name in CASES:
        print(f"pass {name}")
    print(f"all {len(CASES)} cases passed")


if __name__ == "__main__":
    main()
