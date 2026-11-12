#!/usr/bin/env python3
"""Apparatus for the Relax quick-run trial (oxide#307).

The trial (Tom, decision d6a33913-cc2d-460f-bce0-25c424c176d1 v2, 2026-11-11)
asks whether a pinned Relax engine offers useful edit-to-result gains over
current stable-LLVM quick execution on isolated fixtures. This script is the
minimal reproducible apparatus: effect-free readiness and controls now, and a
bounded measurement body that refuses before any effect until a planner-issued
one-use activation binds it.

Modes (exactly one; an unrecognized argument refuses before anything else):

  --plan      Effect-free. Prints the fixed commands, case tables, bounds,
              premise-checklist dispositions and a readiness/blocker list
              derived from actual composition metadata: this script's own
              digest, the qualified supervisor bytes, the repository's HEAD
              and tracked-clean state read-only, the quick recipe read from
              the product source, fixture digests, the installed-toolchain
              list and host platform. It compiles and runs nothing.
  --self-check
              Pure checks in this process: the argument boundary, the
              activation-binding parser against valid and mutated documents,
              the verdict oracles against fixture transcripts, the
              predeclared edits against the committed fixture bytes, and the
              digest pins. No scratch, no child, no network.
  --diagnostic --binding P --sha256 H
              The uncounted keep-going collection, the whole command of the
              qualified shared supervisor. Refuses before any effect until P
              names a planner-issued activation whose digest is H and whose
              fields bind this exact source. Records recoverable case
              failures and continues into dependency-independent cases;
              infrastructure faults abort. Always unqualified.
  --measure --binding P --sha256 H
              The measured fail-fast session under the same guard and
              binding. Requires an adequate diagnostic at the unchanged head
              before separate owner activation; the activation document
              carries the diagnostic receipt digest it follows.
  --help      This text.

Nothing in this preparation builds Relax, runs a third-party executable,
stages, prewarms, smokes, diagnoses or measures. The measurement body below
runs only under the bound activation, inside the qualified guard.

Future actual-run bounds (prepared here; not execution permission)
------------------------------------------------------------------
One fixed command under the unchanged qualified supervisor
(`scripts/bounded-test.py`, SHA-256
fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b), Effigy
heavy admission, 600 s total INCLUDING upstream acquisition, engine build,
staging and every probe, 4096 MiB monitored owned scratch on fresh private
task-owned Stash (volume UUID 8A660440-C919-45C6-A3F1-3365B9BD2006), the
original 30 s stop-work margin, no image, the supervisor's existing 20 GiB
reserve safeguards. Every per-command cap is a minimum of its fixed value and
the remaining deadline minus the stop margin; no clock resets and no separate
stage or prewarm allowance. On fit failure the run stops honestly and the
receipt recommends a bounded prospective amendment; it never enlarges its own
caps.

What one measured session does (per arm, predeclared)
------------------------------------------------------
Arms over identical source, dependencies, features, inputs and expected
outputs: `cargo-quick` (plain Cargo, the product's actual quick recipe
`CARGO_PROFILE_DEV_DEBUG=line-tables-only` with
`CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG` cleared, read from
`src/bin/oxide/cli.rs` and pinned by --plan/--self-check), `oxide-quick` (the
staged Oxide front door of the same head, quick default, private HOME with
`OXIDE_TEST_HOME_FROM_ENV=1` and a private pool) where the isolated fixture
safely supports it, and `relax` (the pinned standalone runner, strict ordinary
Rust: `--check-first --no-live --quiet`, never `--relaxed`, never
`--cached`). Each arm keeps only its own declared compilation cache between
warm cases; processes are fresh; tool build, std preparation, upstream
acquisition and dependency lock establishment are separately charged and
reported, never mixed into first-run latency. Filesystem caches cannot be
globally flushed under this scope, and every report says so.

Workloads: (1) the committed `greet` fixture, a small two-member workspace
with a serde/proc-macro dependency path and deterministic observable output;
(2) the pinned task-base Oxide snapshot (this repository's tree at the bound
head) running its read-only `--help` path under isolated test-home controls.
Cases per workload: cold cache, five unchanged warm reruns, a predeclared
body edit with changed observable output, and (greet) a predeclared interface
edit that invalidates the dependant; the snapshot's edits are cumulative, so
its interface case expects the body edit's `[trial]` marker still applied.
Five repetitions when deadline fit permits, arm order rotated per repetition,
all samples reported with median and range, absolute and relative savings. A
replacement synthetic fixture would be labelled as such, not as evidence that
Oxide's real graph runs.

Correspondence set (each with a concrete expected result; a known upstream
bug is recorded, never worked around by weakening a program): argv/env/cwd/
stdout/stderr/exit; `catch_unwind` plus the uncaught panic's file:line and a
`RUST_BACKTRACE=1` frame expectation; overflow/debug assertions; serde derive
round trip plus the Relax KNOWN_GAPS #1 same-name cross-crate derive
collision; a tiny build.rs/generated constant; threads and a libc FFI path
(this repository's exact libc pin); and basic test execution as a
correspondence probe, not test-route qualification. No storage mutation,
deletion or recovery case exists anywhere in the trial.

Measurement: end-to-end edit-to-correct-result wall time per case (spawn to
reap), stage costs separately, occupied cache bytes per arm after cold and
warm blocks (occupied size, never an SSD-write claim), and per-command
process-tree peak RSS from `os.wait4` `ru_maxrss`, attributed to the direct
child and its reaped descendants only — grandchildren still running at reap
are not attributed, and that limit is stated in every report.

Receipt and custody: one bounded receipt (64 KiB cap) beside the guard's own
under `OXIDE_PROOF_EVIDENCE`, with identities, per-case records (bounded
prefixes, full-output SHA-256s, omission counters), a per-case timing
summary (sample counts, median/min/max, relative time and absolute saving
against the cargo baseline arm, matching cases only), the network time and
received-byte figure parsed from the forced clone progress, owned-scratch
and evidence-packet occupied-byte totals, declared-but-unreached cases as
explicit `unexecuted` rows, and the honest statements that this receipt
never claims guard closure, session census, qualification or a performance
recommendation from failed or unclosed execution; closure stays the
supervisor's. Evidence is published beside the guard receipt, outside the
disposable execution scratch. A guarded run exits 0 only when nothing
aborted, every declared case executed and every verdict is `match`; any
divergence (including the recorded known gap), unexecuted case or abort
exits 1, so a divergence-only run can never read as a clean pass.
"""

from __future__ import annotations

import hashlib
import json
import os
import platform
import re
import shutil
import signal
import subprocess
import sys
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPT = os.path.abspath(__file__)
FIXTURES = os.path.join(REPO, "fixtures", "relax-quick-trial")

SUPERVISOR = os.path.join(REPO, "scripts", "bounded-test.py")
EXPECTED_RUNNER_SHA256 = (
    "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b"
)
CLI_SOURCE = os.path.join(REPO, "src", "bin", "oxide", "cli.rs")

UPSTREAM_URL = "https://github.com/makepad/relax"
UPSTREAM_COMMIT = "aa540c1468844af159e2f106d31966b84be9fdf1"
# `git rev-parse HEAD^{tree}` of the pinned commit, read from the read-only
# inspection clone; the guarded occurrence re-derives and re-verifies it.
UPSTREAM_TREE = "ec2f2ce24b0f82cc5ea1528608d69af0f5e3c438"
UPSTREAM_DATE = "2026-11-11"
TOOLCHAIN = "1.97.1"

LIMIT_SECONDS = 600
SCRATCH_MIB = 4096
STOP_MARGIN_S = 30
SELECTED_VOLUME_UUID = "8A660440-C919-45C6-A3F1-3365B9BD2006"
BINDING_SCHEMA = "oxide.relax-quick-trial-activation/1"

QUICK_DEBUG_ENV = ("CARGO_PROFILE_DEV_DEBUG", "line-tables-only")
QUICK_OVERRIDE_ENV = "CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG"
QUICK_RECIPE_ANCHOR = '(\"CARGO_PROFILE_DEV_DEBUG\", Some(\"line-tables-only\"))'

MAX_RECEIPT_BYTES = 64 * 1024
MAX_CASE_ROWS = 48
MAX_PREFIX_BYTES = 512
# Fixed per-command caps, each clipped to the remaining deadline margin.
CAP_CLONE_S = 90
CAP_ENGINE_BUILD_S = 240
CAP_STAGE_S = 240
CAP_SEED_S = 45
CAP_LOCK_S = 60
CAP_SMALL_S = 90
CAP_SNAPSHOT_S = 180

GREET_EXPECTED_V1 = (
    '{"record":{"count":42,"name":"Tom"},"rendered":"hello Tom count=42"}'
)
GREET_EXPECTED_V2 = (
    '{"record":{"count":42,"name":"Tom"},"rendered":"howdy Tom count=42"}'
)
SNAPSHOT_HELP_MARKER = "oxide: Cargo with managed build storage and profiles"
SNAPSHOT_HELP_EDITED = SNAPSHOT_HELP_MARKER + " [trial]"
PANIC_LOCATION = "main.rs:11:5"
COLLISION_RUSTC = "app=a b=b"
COLLISION_KNOWN_GAP = "app=a b=a"

# --- Predeclared edits. Each is exact bytes against the committed fixtures. ---

GREET_BODY_OLD = 'pub const GREETING: &str = "hello";'
GREET_BODY_NEW = 'pub const GREETING: &str = "howdy";'
GREET_INTERFACE_OLD = "    pub count: u32,"
GREET_INTERFACE_NEW = "    pub count: u64,"
SNAPSHOT_BODY_OLD = (
    "oxide: Cargo with managed build storage and profiles\n"
)
SNAPSHOT_BODY_NEW = (
    "oxide: Cargo with managed build storage and profiles [trial]\n"
)
SNAPSHOT_INTERFACE_OLD = "pub const MAX_STRING_BYTES: usize = 2048;"
SNAPSHOT_INTERFACE_NEW = "pub const MAX_STRING_BYTES: u64 = 2048;"
SNAPSHOT_INTERFACE_CAST_OLD = "reader.string(MAX_STRING_BYTES)?"
SNAPSHOT_INTERFACE_CAST_NEW = "reader.string(MAX_STRING_BYTES as usize)?"
SNAPSHOT_INTERFACE_CASTS = 4

ARMS = ("cargo", "oxide", "relax")

PREMISE_CHECKLIST = {
    "private_home_rustup": (
        "Applies. The guarded body captures the invoking RUSTUP_HOME (or "
        "HOME/.rustup when unset) before replacing HOME, preserves it in "
        "every fixture child, pins RUSTUP_TOOLCHAIN=1.97.1, sets "
        "RUSTUP_AUTO_INSTALL=0, and never installs a toolchain. Each "
        "fixture lock is established offline against the seeded private "
        "Cargo home before any --locked call, inside the owned scratch."
    ),
    "account_home_control": (
        "Applies wherever the staged Oxide honours a private HOME: every "
        "oxide-arm and snapshot command sets OXIDE_TEST_HOME_FROM_ENV=1 "
        "with a per-arm private HOME, so Oxide uses that HOME and never "
        "the account home."
    ),
    "environment_isolation": (
        "Applies. Every child environment is built from a scrubbed base "
        "that drops inherited OXIDE_*, CARGO_*, every RUST* value (RUSTC, "
        "RUSTC_WRAPPER, RUSTDOCFLAGS, RUST_MIN_STACK and friends), MAKEPAD_*, "
        "XDG_* and SCCACHE_*, then sets only the declared controls: "
        "RUSTFLAGS is declared empty, RUSTUP_HOME is restored from the "
        "invoking environment, and PATH is kept. Host pool, account or "
        "wrapper provenance cannot leak into an arm."
    ),
    "automatic_adoption": (
        "Does not apply: the trial converts nothing and owns no permanent-"
        "mode checkout; the Oxide arms run the default internal pool under "
        "a private HOME on fresh fixtures with no target, whose first "
        "conversion (if any) is an observation recorded per case, never an "
        "oracle premise."
    ),
    "cargo_external_argv": (
        "Does not apply: the trial invokes no Cargo external subcommand. "
        "The Relax runner is invoked directly, not as a Cargo subcommand."
    ),
    "owned_images": (
        "Does not apply: the trial creates and attaches no disk image. "
        "Scratch is a directory pool inside the supervisor's owned root."
    ),
    "compiler_flags": (
        "Applies. RUSTFLAGS and CARGO_ENCODED_RUSTFLAGS are scrubbed and "
        "then declared empty for staging and every fixture child, so an "
        "inherited encoded value can neither override the declared quick "
        "recipe nor change a staged candidate's capabilities."
    ),
    "temporary_storage": (
        "Applies. The guarded body runs only under the supervisor's "
        "selected-volume default: OXIDE_TASK_SCRATCH_ROOT, private HOME, "
        "CARGO_HOME and TMPDIR/TMP/TEMP all inside one fresh run root; a "
        "/private/tmp fallback is refused before any effect. No image and "
        "no internal-placement exception."
    ),
    "macos_mktemp": (
        "Applies in the guarded body by construction, not by mktemp: one fresh\n"
        "run root is created from the activation's single safe component under\n"
        "the guard-exported OXIDE_TASK_SCRATCH_ROOT and refused if it already\n"
        "exists; every per-arm HOME, CARGO_HOME and TMPDIR/TMP/TEMP sits inside\n"
        "that owned root; there is no bare mktemp, no fixed /tmp path and no\n"
        "write outside the exported scratch, and the guard exports are verified\n"
        "before any child runs."
    ),
}


# --------------------------------------------------------------------------
# Pure helpers (no effects)
# --------------------------------------------------------------------------


def sha256_file(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 16), b""):
            digest.update(chunk)
    return digest.hexdigest()


def parse_arguments(argv: list[str]) -> tuple[str, str | None, str | None]:
    """Exactly one mode; --diagnostic/--measure require --binding + --sha256."""
    if not argv:
        return ("refuse", None, "a mode argument is required: --plan, --self-check, "
                                "--diagnostic, --measure or --help")
    if len(argv) > 5:
        return ("refuse", None, f"too many arguments, got {argv!r}")
    modes = ("--plan", "--self-check", "--diagnostic", "--measure", "--help")
    if argv[0] not in modes:
        return ("refuse", None, f"unrecognized first argument {argv[0]!r}")
    mode = argv[0][2:]
    if mode in ("plan", "self-check", "help"):
        if len(argv) != 1:
            return ("refuse", None, f"{argv[0]} accepts no further arguments")
        return (mode, None, None)
    binding = None
    sha = None
    index = 1
    while index < len(argv):
        if argv[index] == "--binding" and index + 1 < len(argv):
            binding = argv[index + 1]
            index += 2
        elif argv[index] == "--sha256" and index + 1 < len(argv):
            sha = argv[index + 1]
            index += 2
        else:
            return ("refuse", None, f"unrecognized argument {argv[index]!r} for {argv[0]}")
    if not binding or not sha:
        return ("refuse", None, f"{argv[0]} requires --binding and --sha256")
    if not re.fullmatch(r"[0-9a-f]{64}", sha or ""):
        return ("refuse", None, "--sha256 must be one lowercase 64-hex digest")
    return (mode, binding, sha)


def is_hex(value: object, length: int) -> bool:
    return isinstance(value, str) and bool(re.fullmatch(r"[0-9a-f]{%d}" % length, value))


def safe_component(value: object) -> bool:
    return (
        isinstance(value, str)
        and value not in ("", ".", "..")
        and "/" not in value
        and not value.startswith(".")
        and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}", value) is not None
    )


def parse_binding(payload: bytes, mode: str) -> dict:
    """One planner-issued activation document. Every field is exact."""
    try:
        document = json.loads(payload)
    except ValueError as error:
        raise Refusal(f"the activation document is not JSON: {error}")
    if not isinstance(document, dict) or document.get("schema") != BINDING_SCHEMA:
        raise Refusal("the activation document is not the current schema")
    if not safe_component(document.get("planner_reference")):
        raise Refusal("the activation names no planner reference")
    if document.get("mode") != mode:
        raise Refusal(f"the activation binds mode {document.get('mode')!r}, not {mode!r}")
    if document.get("script_sha256") != sha256_file(SCRIPT):
        raise Refusal("the activation does not bind this script's exact bytes")
    guard = document.get("guard")
    if not isinstance(guard, dict):
        raise Refusal("the activation has no guard object")
    if guard.get("supervisor_sha256") != EXPECTED_RUNNER_SHA256:
        raise Refusal("the activation does not name the qualified supervisor")
    if guard.get("limit_seconds") != LIMIT_SECONDS or guard.get("scratch_mib") != SCRATCH_MIB:
        raise Refusal("the activation's guard bounds are not this route's fixed bounds")
    if guard.get("stop_margin_seconds") != STOP_MARGIN_S:
        raise Refusal("the activation's stop margin is not the original 30 s")
    upstream = document.get("upstream")
    if not isinstance(upstream, dict):
        raise Refusal("the activation has no upstream object")
    if upstream.get("commit") != UPSTREAM_COMMIT or upstream.get("tree") != UPSTREAM_TREE:
        raise Refusal("the activation does not bind the pinned upstream commit and tree")
    if not is_hex(document.get("head"), 40) or not is_hex(document.get("tree"), 40):
        raise Refusal("the activation's repository head and tree are not exact")
    if document.get("toolchain") != TOOLCHAIN:
        raise Refusal("the activation does not bind the pinned toolchain")
    volume = document.get("volume")
    if not isinstance(volume, dict) or volume.get("uuid") != SELECTED_VOLUME_UUID:
        raise Refusal("the activation does not name the selected Stash volume")
    if document.get("diagnostic_receipt_sha256") and not is_hex(
        document.get("diagnostic_receipt_sha256"), 64
    ):
        raise Refusal("the activation's diagnostic receipt digest is not a SHA-256")
    if mode == "measure" and not document.get("diagnostic_receipt_sha256"):
        raise Refusal("a measured activation must name the diagnostic receipt it follows")
    paths = document.get("paths")
    if not isinstance(paths, dict):
        raise Refusal("the activation has no paths object")
    for key in ("run_root_name", "packet_name"):
        if not safe_component(paths.get(key)):
            raise Refusal(f"the activation's {key} is not one safe component")
    return document


class Refusal(Exception):
    """A refuse-before-any-effect failure."""


def apply_edit(text: str, old: str, new: str, count: int = 1) -> str:
    if text.count(old) != count:
        raise Refusal(
            f"the predeclared edit anchor occurs {text.count(old)} times, expected {count}"
        )
    return text.replace(old, new)


def fixture_path(*parts: str) -> str:
    return os.path.join(FIXTURES, *parts)


def read_fixture(*parts: str) -> str:
    with open(fixture_path(*parts), "r", encoding="utf-8") as handle:
        return handle.read()


# --------------------------------------------------------------------------
# Verdict oracles (pure; --self-check drives these with fixture transcripts)
# --------------------------------------------------------------------------


def oracle_exact_stdout(record: dict, expected_stdout: str) -> str:
    stdout = record.get("stdout", "")
    if record.get("timed_out"):
        return "timeout"
    if stdout == expected_stdout and record.get("exit") == 0:
        return "match"
    return "mismatch"


def oracle_exact_with_exit(
    record: dict, expected_stdout: str, expected_exit: int,
    expected_stderr: str | None = None,
    stderr_contains: tuple[str, ...] = (),
) -> str:
    if record.get("timed_out"):
        return "timeout"
    if record.get("exit") != expected_exit:
        return "mismatch"
    if record.get("stdout", "") != expected_stdout:
        return "mismatch"
    if expected_stderr is not None and record.get("stderr", "") != expected_stderr:
        return "mismatch"
    for needle in stderr_contains:
        if needle not in record.get("stderr", ""):
            return "mismatch"
    return "match"


def oracle_fragments(
    record: dict,
    *,
    exit_codes: tuple[int, ...],
    stdout_contains: tuple[str, ...] = (),
    stdout_not_contains: tuple[str, ...] = (),
    stderr_contains: tuple[str, ...] = (),
    combined_contains: tuple[str, ...] = (),
) -> str:
    if record.get("timed_out"):
        return "timeout"
    if record.get("exit") not in exit_codes:
        return "mismatch"
    stdout = record.get("stdout", "")
    stderr = record.get("stderr", "")
    combined = stdout + stderr
    for needle in stdout_contains:
        if needle not in stdout:
            return "mismatch"
    for needle in stdout_not_contains:
        if needle in stdout:
            return "mismatch"
    for needle in stderr_contains:
        if needle not in stderr:
            return "mismatch"
    for needle in combined_contains:
        if needle not in combined:
            return "mismatch"
    return "match"


def oracle_pid_line(record: dict) -> str:
    if record.get("timed_out"):
        return "timeout"
    if record.get("exit") != 0:
        return "mismatch"
    for line in record.get("stdout", "").splitlines():
        found = re.fullmatch(r"sum=10 pid=([1-9][0-9]*)", line)
        if found:
            return "match"
    return "mismatch"


def oracle_collision(record: dict) -> str:
    """rustc truth is app=a b=b; the documented Relax gap is app=a b=a."""
    if record.get("timed_out"):
        return "timeout"
    stdout = record.get("stdout", "")
    if record.get("exit") == 0 and stdout == COLLISION_RUSTC + "\n":
        return "match"
    if record.get("exit") == 0 and stdout == COLLISION_KNOWN_GAP + "\n":
        return "mismatch-known-gap-1"
    return "mismatch"


def greeting_expectation(edit: str | None) -> str:
    if edit is None or edit == "warm":
        return GREET_EXPECTED_V1
    if edit == "body":
        return GREET_EXPECTED_V2
    raise Refusal(f"unknown greet edit {edit!r}")


# --------------------------------------------------------------------------
# Plan document
# --------------------------------------------------------------------------


def read_only_head() -> tuple[str | None, str | None, list[str]]:
    """(head, tree, blockers) from read-only git queries. Never mutates."""
    blockers: list[str] = []
    try:
        head = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=REPO, capture_output=True, text=True, timeout=20,
        ).stdout.strip()
        tree = subprocess.run(
            ["git", "rev-parse", "HEAD^{tree}"],
            cwd=REPO, capture_output=True, text=True, timeout=20,
        ).stdout.strip()
        status = subprocess.run(
            ["git", "status", "--porcelain", "--untracked-files=no"],
            cwd=REPO, capture_output=True, text=True, timeout=20,
        )
        dirty = [line for line in status.stdout.splitlines() if line.strip()]
        if status.returncode != 0:
            blockers.append(f"git status failed: {status.stderr.strip()[:200]}")
        elif dirty:
            blockers.append(f"tracked worktree is dirty: {len(dirty)} paths")
        if not is_hex(head, 40):
            blockers.append("git rev-parse HEAD did not answer one 40-hex identity")
        if not is_hex(tree, 40):
            blockers.append("git rev-parse HEAD^{tree} did not answer one 40-hex identity")
        return (head or None, tree or None, blockers)
    except (OSError, subprocess.SubprocessError) as error:
        return (None, None, [f"git queries failed: {error}"])


def quick_recipe_from_source() -> tuple[str | None, str | None]:
    """The actual quick recipe, read from the product source (no compile)."""
    try:
        with open(CLI_SOURCE, "r", encoding="utf-8") as handle:
            source = handle.read()
    except OSError as error:
        return (None, f"cannot read {CLI_SOURCE}: {error}")
    if QUICK_RECIPE_ANCHOR not in source:
        return (
            None,
            f"the quick recipe anchor {QUICK_RECIPE_ANCHOR} is absent from cli.rs; "
            "the trial's cargo arm must be re-pinned to the actual recipe",
        )
    if "Self::Quick => &[" not in source:
        return (None, "the Profile::Quick dev-recipe arm is absent from cli.rs")
    return (
        f"{QUICK_DEBUG_ENV[0]}={QUICK_DEBUG_ENV[1]} with {QUICK_OVERRIDE_ENV} cleared",
        None,
    )


def toolchain_inventory() -> list[str]:
    """Read-only rustup metadata; a missing pin is a blocker, not an install."""
    try:
        result = subprocess.run(
            ["rustup", "toolchain", "list"],
            capture_output=True, text=True, timeout=20,
        )
        if result.returncode != 0:
            return [f"rustup toolchain list failed: {result.stderr.strip()[:200]}"]
        return [line for line in result.stdout.splitlines() if line.strip()]
    except (OSError, subprocess.SubprocessError) as error:
        return [f"rustup toolchain list failed: {error}"]


def fixture_digests() -> dict[str, str]:
    digests: dict[str, str] = {}
    for root, dirs, files in os.walk(FIXTURES):
        dirs.sort()
        for name in sorted(files):
            path = os.path.join(root, name)
            digests[os.path.relpath(path, REPO)] = sha256_file(path)
    return digests


def readiness() -> tuple[list[str], list[str]]:
    """(readiness lines, blockers) from actual metadata, no compile or run."""
    notes: list[str] = []
    blockers: list[str] = []
    notes.append(f"platform: {platform.system()} {platform.machine()}")
    if platform.system() != "Darwin" or platform.machine() not in ("arm64", "aarch64"):
        blockers.append("the trial's prepared scope is macOS arm64 on this host")
    script_digest = sha256_file(SCRIPT)
    notes.append(f"script sha256: {script_digest}")
    try:
        supervisor_digest = sha256_file(SUPERVISOR)
        if supervisor_digest != EXPECTED_RUNNER_SHA256:
            blockers.append(
                f"scripts/bounded-test.py is {supervisor_digest}, not the pinned "
                f"{EXPECTED_RUNNER_SHA256}; the guarded commands must not run"
            )
        else:
            notes.append("supervisor bytes match the pinned qualified digest")
    except OSError as error:
        blockers.append(f"cannot read the supervisor: {error}")
    head, tree, head_blockers = read_only_head()
    if head:
        notes.append(f"repository head: {head}")
        notes.append(f"repository tree: {tree}")
    blockers.extend(head_blockers)
    recipe, recipe_blocker = quick_recipe_from_source()
    if recipe:
        notes.append(f"quick recipe (from cli.rs): {recipe}")
    if recipe_blocker:
        blockers.append(recipe_blocker)
    toolchains = toolchain_inventory()
    notes.append("installed toolchains: " + "; ".join(toolchains[:8]))
    if not any(line.startswith(TOOLCHAIN) or f" {TOOLCHAIN} " in f" {line} " for line in toolchains):
        blockers.append(f"toolchain {TOOLCHAIN} is not installed; the trial never installs one")
    digests = fixture_digests()
    notes.append(f"committed fixture files: {len(digests)}")
    required = [
        "fixtures/relax-quick-trial/greet/Cargo.toml",
        "fixtures/relax-quick-trial/greet/greet-core/src/lib.rs",
        "fixtures/relax-quick-trial/greet/greet/src/main.rs",
        "fixtures/relax-quick-trial/corr-suite/Cargo.toml",
        "fixtures/relax-quick-trial/corr-suite/basics/src/main.rs",
        "fixtures/relax-quick-trial/corr-suite/panic/src/main.rs",
        "fixtures/relax-quick-trial/corr-suite/overflow/src/main.rs",
        "fixtures/relax-quick-trial/corr-suite/gen/build.rs",
        "fixtures/relax-quick-trial/corr-suite/gen/src/main.rs",
        "fixtures/relax-quick-trial/corr-suite/threads-ffi/src/main.rs",
        "fixtures/relax-quick-trial/corr-suite/testkit/src/lib.rs",
        "fixtures/relax-quick-trial/corr-suite/mac-a/src/lib.rs",
        "fixtures/relax-quick-trial/corr-suite/mac-b/src/lib.rs",
        "fixtures/relax-quick-trial/corr-suite/lib-b/src/lib.rs",
        "fixtures/relax-quick-trial/corr-suite/collision-app/src/main.rs",
    ]
    for path in required:
        if path not in digests:
            blockers.append(f"required fixture input is missing: {path}")
    return (notes, blockers)


CASE_TABLE = [
    {"workload": "greet", "case": "cold", "arms": ARMS,
     "oracle": "exact stdout GREET_EXPECTED_V1, exit 0",
     "figure": "cold edit-to-result wall time"},
    {"workload": "greet", "case": "warm-unchanged", "reps": 5, "arms": ARMS,
     "oracle": "exact stdout GREET_EXPECTED_V1, exit 0",
     "figure": "warm no-edit wall time, median/range over reps"},
    {"workload": "greet", "case": "edit-body", "reps": 5, "arms": ARMS,
     "edit": "GREETING hello->howdy (greet-core)",
     "oracle": "exact stdout GREET_EXPECTED_V2, exit 0",
     "figure": "first rep after the edit is the edit-to-result figure"},
    {"workload": "greet", "case": "edit-interface", "reps": 5, "arms": ARMS,
     "edit": "Record.count u32->u64 (greet-core)",
     "oracle": "exact stdout GREET_EXPECTED_V2, exit 0 (unchanged bytes)",
     "figure": "dependant-invalidating edit"},
    {"workload": "snapshot", "case": "cold", "arms": ARMS,
     "oracle": "exit 0, stdout contains SNAPSHOT_HELP_MARKER, not '[trial]'",
     "figure": "cold build+run of the read-only --help path"},
    {"workload": "snapshot", "case": "warm-unchanged", "reps": 5, "arms": ARMS,
     "oracle": "as cold", "figure": "warm no-edit wall time"},
    {"workload": "snapshot", "case": "edit-body", "reps": 5, "arms": ARMS,
     "edit": "USAGE first line += ' [trial]' (src/bin/oxide/main.rs)",
     "oracle": "exit 0, stdout contains SNAPSHOT_HELP_EDITED, not the unedited marker",
     "figure": "the oxide#075 loop shape"},
    {"workload": "snapshot", "case": "edit-interface", "reps": 5, "arms": ARMS,
     "edit": "MAX_STRING_BYTES usize->u64 + 4 reader.string casts (src/dep_info.rs)",
     "oracle": "exit 0, stdout contains SNAPSHOT_HELP_EDITED (the body edit's [trial] stays applied)",
     "figure": "library type change invalidating the bin unit"},
]
CASE_TABLE.extend(
    {
        "workload": "corr",
        "case": name,
        "arms": ("cargo", "relax"),
        "oracle": oracle,
        "figure": "correspondence",
    }
    for name, oracle in [
        ("basics", "exact stdout and exit 7; cargo's stderr compared exactly, relax's "
         "stderr must carry the program's line (its runner logs on stderr)"),
        ("panic-caught", "exact stdout CAUGHT:corr-caught-boom, exit 0"),
        ("panic-uncaught", "exit 101, stderr names " + PANIC_LOCATION),
        ("panic-uncaught-backtrace", "RUST_BACKTRACE=1, a frame names " + PANIC_LOCATION),
        ("overflow", "exit 101, stderr names attempt to add with overflow"),
        ("gen", "exact stdout generated=gen-constant-v1, exit 0"),
        ("threads-ffi", "exit 0, stdout line sum=10 pid=<positive>"),
        ("testkit", "exit 0, both test names in the combined output"),
        ("collision", COLLISION_RUSTC + "; the known gap " + COLLISION_KNOWN_GAP
         + " is recorded as mismatch-known-gap-1, never passed"),
    ]
)

GUARDED_COMMANDS = [
    "effigy research:relax:diagnostic  # planner-issued RELAX_TRIAL_ACTIVATION binding",
    "effigy research:relax:measure     # after an adequate diagnostic at the unchanged head",
    "python3 scripts/bounded-test.py --limit 600 --scratch-budget-mib 4096 \\",
    "    -- python3 scripts/relax-quick-trial.py --diagnostic \\",
    "    --binding \"$RELAX_TRIAL_ACTIVATION\" --sha256 \"$RELAX_TRIAL_ACTIVATION_SHA256\"",
]


def plan_text() -> str:
    notes, blockers = readiness()
    lines: list[str] = []
    lines.append("relax-quick-trial plan (oxide#307) — effect-free; nothing is compiled,")
    lines.append("acquired, staged, run or measured by this mode.")
    lines.append("")
    lines.append("Readiness from actual metadata:")
    for note in notes:
        lines.append(f"  - {note}")
    lines.append("")
    lines.append(f"Boundaries (prepared, not execution permission): {LIMIT_SECONDS} s total")
    lines.append(f"including acquisition/staging/probes; {SCRATCH_MIB} MiB monitored owned")
    lines.append("scratch on fresh private task-owned Stash volume "
                 f"{SELECTED_VOLUME_UUID};")
    lines.append(f"{STOP_MARGIN_S} s stop-work margin; no image; the supervisor's 20 GiB")
    lines.append("reserve safeguards; per-command caps are minima with remaining time.")
    lines.append("")
    lines.append("Guarded commands (never run by --plan):")
    for command in GUARDED_COMMANDS:
        lines.append(f"  {command}")
    lines.append("")
    lines.append("Case table (predeclared edits and oracles):")
    for case in CASE_TABLE:
        parts = [f"{case['workload']}/{case['case']}"]
        if case.get("reps"):
            parts.append(f"reps={case['reps']}")
        parts.append(f"arms={','.join(case['arms'])}")
        if case.get("edit"):
            parts.append(f"edit: {case['edit']}")
        parts.append(f"oracle: {case['oracle']}")
        parts.append(f"figure: {case['figure']}")
        lines.append("  - " + "; ".join(parts))
    lines.append("")
    lines.append("Premise checklist dispositions (workflow.md, proof authoring):")
    for item, disposition in PREMISE_CHECKLIST.items():
        lines.append(f"  - {item}: {disposition}")
    lines.append("")
    if blockers:
        lines.append("Blockers:")
        for blocker in blockers:
            lines.append(f"  - {blocker}")
        lines.append("The guarded modes refuse until every blocker above is resolved;")
        lines.append("this plan itself grants no execution authority.")
    else:
        lines.append("No metadata blocker found; activation stays a separate planner step.")
    lines.append("")
    return "\n".join(lines)


# --------------------------------------------------------------------------
# Self-check (pure)
# --------------------------------------------------------------------------


def expect(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def valid_binding_document(mode: str, script_sha: str) -> bytes:
    return json.dumps(
        {
            "schema": BINDING_SCHEMA,
            "planner_reference": "trial-activation-1",
            "mode": mode,
            "script_sha256": script_sha,
            "head": "a" * 40,
            "tree": "b" * 40,
            "toolchain": TOOLCHAIN,
            "diagnostic_receipt_sha256": "c" * 64 if mode == "measure" else "",
            "guard": {
                "supervisor_sha256": EXPECTED_RUNNER_SHA256,
                "limit_seconds": LIMIT_SECONDS,
                "scratch_mib": SCRATCH_MIB,
                "stop_margin_seconds": STOP_MARGIN_S,
            },
            "upstream": {"commit": UPSTREAM_COMMIT, "tree": UPSTREAM_TREE},
            "volume": {"uuid": SELECTED_VOLUME_UUID},
            "paths": {"run_root_name": "relax-trial-run", "packet_name": "packet"},
        }
    ).encode()


def self_check() -> None:
    script_sha = sha256_file(SCRIPT)

    # Argument boundary.
    expect(parse_arguments([])[0] == "refuse", "bare invocation must refuse")
    expect(parse_arguments(["--plan", "--plan"])[0] == "refuse", "double mode must refuse")
    expect(parse_arguments(["--bogus"])[0] == "refuse", "unknown mode must refuse")
    expect(parse_arguments(["--measure"])[0] == "refuse", "measure without binding refuses")
    expect(parse_arguments(["--diagnostic", "--binding", "/tmp/x"])[0] == "refuse",
           "diagnostic without sha256 refuses")
    expect(parse_arguments(["--diagnostic", "--binding", "/tmp/x", "--sha256", "zz"])[0] == "refuse",
           "non-hex sha256 refuses")
    expect(parse_arguments(["--plan"])[0] == "plan", "--plan parses")
    expect(parse_arguments(["--self-check"])[0] == "self-check", "--self-check parses")
    ok = parse_arguments(["--measure", "--binding", "/p", "--sha256", "a" * 64])
    expect(ok[0] == "measure" and ok[1] == "/p", "measure with binding parses")
    expect(parse_arguments(["--measure", "--binding", "/p", "--sha256", "a" * 64, "extra"])[0] == "refuse",
           "unexpected extra argument refuses")

    # Binding parser: accept, then mutate every load-bearing field.
    good = parse_binding(valid_binding_document("diagnostic", script_sha), "diagnostic")
    expect(good["planner_reference"] == "trial-activation-1", "binding accepted")
    expect(parse_binding(valid_binding_document("measure", script_sha), "measure") is not None,
           "measure binding accepts")
    try:
        parse_binding(valid_binding_document("diagnostic", script_sha), "measure")
        raise AssertionError("mode mismatch must refuse")
    except Refusal:
        pass
    mutations = {
        "schema": {"schema": "other/1"},
        "mode": {"mode": "measure"},
        "script_sha256": {"script_sha256": "0" * 64},
        "supervisor": {"guard": {"supervisor_sha256": "0" * 64, "limit_seconds": LIMIT_SECONDS,
                                 "scratch_mib": SCRATCH_MIB, "stop_margin_seconds": STOP_MARGIN_S}},
        "limit": {"guard": {"supervisor_sha256": EXPECTED_RUNNER_SHA256, "limit_seconds": 300,
                            "scratch_mib": SCRATCH_MIB, "stop_margin_seconds": STOP_MARGIN_S}},
        "scratch": {"guard": {"supervisor_sha256": EXPECTED_RUNNER_SHA256, "limit_seconds": LIMIT_SECONDS,
                              "scratch_mib": 512, "stop_margin_seconds": STOP_MARGIN_S}},
        "margin": {"guard": {"supervisor_sha256": EXPECTED_RUNNER_SHA256, "limit_seconds": LIMIT_SECONDS,
                             "scratch_mib": SCRATCH_MIB, "stop_margin_seconds": 60}},
        "upstream-commit": {"upstream": {"commit": "0" * 40, "tree": UPSTREAM_TREE}},
        "upstream-tree": {"upstream": {"commit": UPSTREAM_COMMIT, "tree": "0" * 40}},
        "head": {"head": "z" * 40},
        "tree": {"tree": "z" * 40},
        "toolchain": {"toolchain": "1.99.1"},
        "volume": {"volume": {"uuid": "00000000-0000-0000-0000-000000000000"}},
        "paths": {"paths": {"run_root_name": "../escape", "packet_name": "packet"}},
        "planner": {"planner_reference": ""},
    }
    for label, patch in mutations.items():
        document = json.loads(valid_binding_document("diagnostic", script_sha))
        document.update(patch)
        try:
            parse_binding(json.dumps(document).encode(), "diagnostic")
            raise AssertionError(f"mutation {label} must refuse")
        except Refusal:
            continue
    unqualified = json.loads(valid_binding_document("diagnostic", script_sha))
    del unqualified["diagnostic_receipt_sha256"]
    document = json.loads(valid_binding_document("measure", script_sha))
    del document["diagnostic_receipt_sha256"]
    try:
        parse_binding(json.dumps(document).encode(), "measure")
        raise AssertionError("measure without a diagnostic receipt must refuse")
    except Refusal:
        pass

    # Digest pins read from disk (read-only).
    expect(sha256_file(SUPERVISOR) == EXPECTED_RUNNER_SHA256,
           "scripts/bounded-test.py must stay at the pinned qualified digest")
    recipe, blocker = quick_recipe_from_source()
    expect(recipe is not None and blocker is None,
           f"the quick recipe anchor must bind: {blocker}")

    # Predeclared edits apply exactly to the committed fixture bytes.
    core = read_fixture("greet", "greet-core", "src", "lib.rs")  # (pure read)
    expect(core.count(GREET_BODY_OLD) == 1, "greet body anchor occurs once")
    expect(core.count(GREET_INTERFACE_OLD) == 1, "greet interface anchor occurs once")
    edited = apply_edit(apply_edit(core, GREET_BODY_OLD, GREET_BODY_NEW),
                        GREET_INTERFACE_OLD, GREET_INTERFACE_NEW)
    expect("howdy" in edited and "pub count: u64," in edited, "greet edits compose")
    with open(os.path.join(REPO, "src", "bin", "oxide", "main.rs"), encoding="utf-8") as h:
        usage = h.read()
    expect(usage.count(SNAPSHOT_BODY_OLD) == 1, "snapshot body anchor occurs once")
    dep_info = open(os.path.join(REPO, "src", "dep_info.rs"), encoding="utf-8").read()
    expect(dep_info.count(SNAPSHOT_INTERFACE_OLD) == 1, "snapshot const anchor occurs once")
    expect(dep_info.count(SNAPSHOT_INTERFACE_CAST_OLD) == SNAPSHOT_INTERFACE_CASTS,
           f"snapshot cast anchor occurs exactly {SNAPSHOT_INTERFACE_CASTS} times")
    snapshot_body_edited = apply_edit(usage, SNAPSHOT_BODY_OLD, SNAPSHOT_BODY_NEW)
    expect(SNAPSHOT_HELP_EDITED.splitlines()[0] + "\n" in snapshot_body_edited,
           "the edited help line is byte-exact")
    dep_info_edited = apply_edit(apply_edit(dep_info, SNAPSHOT_INTERFACE_OLD,
                                            SNAPSHOT_INTERFACE_NEW),
                                 SNAPSHOT_INTERFACE_CAST_OLD,
                                 SNAPSHOT_INTERFACE_CAST_NEW, SNAPSHOT_INTERFACE_CASTS)
    expect("pub const MAX_STRING_BYTES: u64 = 2048;" in dep_info_edited,
           "the snapshot interface edit changes the const type")
    expect(dep_info_edited.count("reader.string(MAX_STRING_BYTES as usize)?")
           == SNAPSHOT_INTERFACE_CASTS, "every cast site is rewritten")

    # Oracles against fixture transcripts.
    expect(oracle_exact_stdout({"stdout": GREET_EXPECTED_V1 + "\n", "exit": 0},
                               GREET_EXPECTED_V1 + "\n") == "match", "exact oracle matches")
    expect(oracle_exact_stdout({"stdout": GREET_EXPECTED_V1 + "\n", "exit": 101},
                               GREET_EXPECTED_V1 + "\n") == "mismatch", "exit matters")
    expect(greeting_expectation(None) == GREET_EXPECTED_V1, "warm expects v1")
    expect(greeting_expectation("body") == GREET_EXPECTED_V2, "body edit expects v2")
    try:
        greeting_expectation("nonsense")
        raise AssertionError("unknown greet edit must refuse")
    except Refusal:
        pass
    expect(oracle_exact_with_exit({"stdout": "args=one,two,7\nenv=corridor\ncwd=corr-suite\n",
                                   "stderr": "corr-trial-stderr-line\n", "exit": 7},
                                  "args=one,two,7\nenv=corridor\ncwd=corr-suite\n", 7,
                                  expected_stderr="corr-trial-stderr-line\n") == "match",
           "basics oracle matches its transcript including stderr")
    expect(oracle_exact_with_exit({"stdout": "args=one,two,7\nenv=corridor\ncwd=corr-suite\n",
                                   "stderr": "WRONG\n", "exit": 7},
                                  "args=one,two,7\nenv=corridor\ncwd=corr-suite\n", 7,
                                  expected_stderr="corr-trial-stderr-line\n") == "mismatch",
           "basics oracle refuses wrong stderr bytes")
    expect(oracle_exact_with_exit({"stdout": "args=one,two,7\nenv=corridor\ncwd=corr-suite\n",
                                   "stderr": "relax-remote: pid 42 app basics socket /x\n"
                                             "corr-trial-stderr-line\n", "exit": 7},
                                  "args=one,two,7\nenv=corridor\ncwd=corr-suite\n", 7,
                                  stderr_contains=("corr-trial-stderr-line",)) == "match",
           "relax's basics stderr only needs the program line beside its own log")
    expect(oracle_exact_with_exit({"stdout": "args=one,two,7\nenv=corridor\ncwd=corr-suite\n",
                                   "stderr": "relax-remote: pid 42 app basics socket /x\n", "exit": 7},
                                  "args=one,two,7\nenv=corridor\ncwd=corr-suite\n", 7,
                                  stderr_contains=("corr-trial-stderr-line",)) == "mismatch",
           "a relax basics run missing the program's stderr line is a mismatch")
    expect(oracle_fragments({"exit": 101, "stdout": "",
                             "stderr": f"thread 'main' panicked at {PANIC_LOCATION}: "
                                       "corr-uncaught-boom"},
                            exit_codes=(101,), stderr_contains=(PANIC_LOCATION,)) == "match",
           "uncaught-panic oracle matches its transcript")
    expect(oracle_fragments({"exit": 101, "stdout": "",
                             "stderr": "attempt to add with overflow"},
                            exit_codes=(101,),
                            stderr_contains=("attempt to add with overflow",)) == "match",
           "overflow oracle matches its transcript")
    expect(oracle_pid_line({"exit": 0, "stdout": "sum=10 pid=41041\n"}) == "match",
           "pid oracle matches its transcript")
    expect(oracle_pid_line({"exit": 0, "stdout": "sum=6 pid=41041\n"}) == "mismatch",
           "pid oracle refuses a wrong sum")
    expect(oracle_pid_line({"exit": 0, "stdout": "sum=10 pid=0\n"}) == "mismatch",
           "pid oracle refuses a zero pid")
    expect(oracle_collision({"exit": 0, "stdout": COLLISION_RUSTC + "\n"}) == "match",
           "collision rustc truth matches")
    expect(oracle_collision({"exit": 0, "stdout": COLLISION_KNOWN_GAP + "\n"})
           == "mismatch-known-gap-1", "collision gap is recorded, never passed")
    expect(oracle_fragments({"exit": 0, "stdout": "running 2 tests\ntest adds ... ok\n"
                                             "test concats ... ok\n", "stderr": ""},
                            exit_codes=(0,),
                            combined_contains=("adds", "concats")) == "match",
           "testkit oracle matches a libtest transcript")
    expect(oracle_exact_stdout({"stdout": "generated=gen-constant-v1\n", "exit": 0},
                               "generated=gen-constant-v1\n") == "match",
           "gen oracle matches its transcript")

    # The plan document is well-formed and carries every disposition.
    document = plan_text()
    for item in PREMISE_CHECKLIST:
        expect(f"  - {item}: " in document, f"plan carries disposition {item}")
    expect(str(LIMIT_SECONDS) in document and str(SCRATCH_MIB) in document,
           "plan carries the fixed bounds")

    # Median, timing summary, unexecuted rows and the exit rule.
    expect(median([3.0]) == 3.0 and median([1.0, 2.0, 4.0]) == 2.0
           and median([1.0, 2.0]) == 1.5 and median([]) is None,
           "median covers odd, even, single and empty samples")
    fake_cases = [
        {"workload": "greet", "case": "cold", "arm": "cargo", "verdict": "match", "seconds": 1.0},
        {"workload": "greet", "case": "cold", "arm": "relax", "verdict": "match", "seconds": 0.5},
        {"workload": "greet", "case": "cold", "arm": "relax", "verdict": "match", "seconds": 1.5},
        {"workload": "greet", "case": "cold", "arm": "oxide", "verdict": "mismatch", "seconds": 2.0},
    ]
    summary = timing_summary(fake_cases)
    cold = summary["greet/cold"]
    expect(cold["cargo"]["median_s"] == 1.0, "cargo baseline median recorded")
    expect(cold["relax"]["median_s"] == 1.0 and cold["relax"]["relative_to_cargo"] == 1.0,
           "relax median 1.0 s against the cargo baseline")
    expect("oxide" not in cold, "a non-matching arm contributes no timing figure")
    expect(cold["relax"]["absolute_saving_s"] == 0.0, "absolute saving against the baseline")
    full_cold = [{"workload": "greet", "case": "cold", "arm": arm, "verdict": "match",
                  "seconds": 1.0} for arm in ARMS]
    expect(all((row["workload"], row["case"]) != ("greet", "cold")
               for row in declared_unexecuted(full_cold)),
           "a fully reached declared case yields no unexecuted rows for itself")
    sparse = [{"workload": "greet", "case": "cold", "arm": "cargo", "verdict": "match",
               "seconds": 1.0}]
    rows = declared_unexecuted(sparse)
    expect(any(row["case"] == "cold" and row["arm"] == "relax" and row["verdict"] == "unexecuted"
               for row in rows), "unreached declared arms become unexecuted rows")
    declared_total = sum(len(case["arms"]) for case in CASE_TABLE)
    expect(len(rows) == declared_total - 1,
           "unexecuted rows cover exactly the unreached declared combinations")
    expect(collection_exit(None, [{"verdict": "match"}]) == 0, "clean collection exits 0")
    expect(collection_exit(None, [{"verdict": "match"}, {"verdict": "mismatch"}]) == 1,
           "a divergence-only run exits 1, never a clean pass")
    expect(collection_exit(None, [{"verdict": "mismatch-known-gap-1"}]) == 1,
           "the recorded known gap keeps the exit nonzero")
    expect(collection_exit(None, [{"verdict": "unexecuted"}]) == 1,
           "an unreached declared case keeps the exit nonzero")
    expect(collection_exit("greet: boom", [{"verdict": "match"}]) == 1,
           "an aborted collection exits 1")

    print("self-check: all controls passed (no scratch, child, network or engine)\n")


# --------------------------------------------------------------------------
# Guarded run body (bound activation only; refuses before any effect)
# --------------------------------------------------------------------------

GUARD_EXPORTS = ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_PROOF_DEADLINE_NS", "OXIDE_PROOF_EVIDENCE")
# Every key equal to or prefixed by one of these is dropped from child
# environments; RUST covers RUSTC, RUSTC_WRAPPER, RUSTDOCFLAGS, RUST_MIN_STACK
# and friends. RUSTUP_HOME is restored explicitly below and RUSTFLAGS is
# declared empty, so a host wrapper or flag set cannot reach a fixture.
SCRUB_PREFIXES = ("OXIDE_", "CARGO_", "RUST", "MAKEPAD_", "XDG_", "SCCACHE_")


def guard_exports() -> dict[str, str]:
    values: dict[str, str] = {}
    for name in ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_PROOF_EVIDENCE"):
        value = os.environ.get(name)
        if not value or not os.path.isabs(value):
            raise Refusal(f"{name} is not exported as one absolute path; this body runs only under the guard")
        values[name] = value
    deadline = os.environ.get("OXIDE_PROOF_DEADLINE_NS")
    if not deadline or not deadline.isdigit():
        raise Refusal(
            "OXIDE_PROOF_DEADLINE_NS is not exported as one integer nanosecond "
            "CLOCK_UPTIME_RAW deadline; this body runs only under the guard"
        )
    values["OXIDE_PROOF_DEADLINE_NS"] = deadline
    if sha256_file(SUPERVISOR) != EXPECTED_RUNNER_SHA256:
        raise Refusal("scripts/bounded-test.py is not the pinned qualified digest")
    return values


def remaining_ns(deadline_ns: int) -> int:
    return deadline_ns - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)


def budget_s(deadline_ns: int, cap_s: float) -> float:
    """A per-command cap is a minimum of its fixed value and the remaining
deadline minus the stop margin; it never resets or extends a clock."""
    return max(0.0, min(float(cap_s), remaining_ns(deadline_ns) / 1e9 - STOP_MARGIN_S))


def scrubbed_env() -> dict[str, str]:
    env = {
        key: value
        for key, value in os.environ.items()
        if not any(key == prefix or key.startswith(prefix) for prefix in SCRUB_PREFIXES)
        and key != "RUSTUP_TOOLCHAIN"
    }
    env["RUSTUP_AUTO_INSTALL"] = "0"
    env["RUSTFLAGS"] = ""
    env["PATH"] = os.environ.get("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
    # The invoking toolchain home is the one deliberate RUST* survivor: the
    # pinned toolchain resolves through it and is never installed.
    if os.environ.get("RUSTUP_HOME"):
        env["RUSTUP_HOME"] = os.environ["RUSTUP_HOME"]
    elif os.environ.get("HOME"):
        env["RUSTUP_HOME"] = os.path.join(os.environ["HOME"], ".rustup")
    return env


def child_env(base: dict[str, str], **overrides: str | None) -> dict[str, str]:
    env = dict(base)
    for key, value in overrides.items():
        if value is None:
            env.pop(key, None)
        else:
            env[key] = value
    return env


def quick_recipe_env(env: dict[str, str]) -> dict[str, str]:
    """The product's actual quick recipe (cli.rs Profile::Quick dev_recipe)."""
    return child_env(
        env,
        **{QUICK_DEBUG_ENV[0]: QUICK_DEBUG_ENV[1], QUICK_OVERRIDE_ENV: None},
    )


def mount_of(path: str) -> str:
    current = os.path.realpath(path)
    while not os.path.ismount(current):
        parent = os.path.dirname(current)
        if parent == current:
            raise Refusal(f"no mount boundary found above {path}")
        current = parent
    return current


def volume_uuid_of(mount: str) -> str:
    import plistlib

    result = subprocess.run(
        ["/usr/sbin/diskutil", "info", "-plist", mount],
        capture_output=True, timeout=20,
    )
    if result.returncode != 0:
        raise Refusal(f"diskutil refused {mount}: {result.stderr[:200]!r}")
    try:
        facts = plistlib.loads(result.stdout)
    except ValueError as error:
        raise Refusal(f"diskutil output for {mount} is unreadable: {error}")
    uuid = facts.get("VolumeUUID")
    if not isinstance(uuid, str) or not uuid:
        raise Refusal(f"diskutil names no VolumeUUID for {mount}")
    return str(uuid).upper()


def resolve_source(binding: dict) -> tuple[str, str]:
    queries = [
        ["git", "rev-parse", "HEAD"],
        ["git", "rev-parse", "HEAD^{tree}"],
        ["git", "status", "--porcelain", "--untracked-files=no"],
    ]
    results = []
    for query in queries:
        run = subprocess.run(query, cwd=REPO, capture_output=True, text=True, timeout=20)
        if run.returncode != 0:
            raise Refusal(f"source gate: {' '.join(query)} failed: {run.stderr.strip()[:200]}")
        results.append(run.stdout)
    head, tree, status = results[0].strip(), results[1].strip(), results[2]
    if not is_hex(head, 40) or not is_hex(tree, 40):
        raise Refusal("source gate: git did not answer exact 40-hex identities")
    if any(line.strip() for line in status.splitlines()):
        raise Refusal("source gate: the tracked worktree is dirty")
    if head != binding["head"] or tree != binding["tree"]:
        raise Refusal(
            f"source gate: HEAD/tree {head}/{tree} is not the bound {binding['head']}/{binding['tree']}"
        )
    return head, tree


def bounded_text(body: bytes, limit: int = MAX_PREFIX_BYTES) -> dict:
    return {
        "sha256": hashlib.sha256(body).hexdigest(),
        "bytes": len(body),
        "prefix": body[:limit].decode("utf-8", "replace"),
        "truncated": len(body) > limit,
    }


def walk_bytes(root: str, cap_entries: int = 200_000) -> dict:
    """Occupied apparent bytes and entries, capped; size, never a write claim."""
    total = 0
    entries = 0
    omitted = 0
    for base, dirs, files in os.walk(root):
        dirs.sort()
        for name in files:
            entries += 1
            if entries > cap_entries:
                omitted += 1
                continue
            try:
                total += os.lstat(os.path.join(base, name)).st_size
            except OSError:
                omitted += 1
    return {"path": root, "bytes_apparent": total, "entries": min(entries, cap_entries), "omitted": omitted}


def measure_one(
    argv: list[str], cwd: str, env: dict[str, str], cap_s: float, deadline_ns: int
) -> dict:
    """One fresh child, spawn to reap. Wall time is end-to-end. Peak RSS is
`os.wait4` ru_maxrss for this child and its reaped descendants only:
grandchildren still running at reap are not attributed, and reports say so.
A timed-out child is killed by its own unreaped PID and positively reaped."""
    allowed = budget_s(deadline_ns, cap_s)
    if allowed <= 0.0:
        raise Refusal("the deadline plus stop margin leaves no budget for the next child")
    stdout_path = cwd + ".stdout"
    stderr_path = cwd + ".stderr"
    started = time.monotonic()
    with open(stdout_path, "wb") as out, open(stderr_path, "wb") as err:
        process = subprocess.Popen(
            argv, cwd=cwd, env=env, stdout=out, stderr=err,
            stdin=subprocess.DEVNULL,
        )
        timed_out = False
        while True:
            pid, status, usage = os.wait4(process.pid, os.WNOHANG)
            if pid == process.pid:
                break
            if time.monotonic() - started >= allowed:
                timed_out = True
                try:
                    os.kill(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                pid, status, usage = os.wait4(process.pid, 0)
                break
            time.sleep(0.01)
        # Keep Popen from double-waiting: the child is positively reaped above.
        process.returncode = os.waitstatus_to_exitcode(status)
    elapsed = time.monotonic() - started
    record = {
        "argv": argv,
        "cwd": cwd,
        "exit": process.returncode,
        "seconds": round(elapsed, 4),
        "rss_max_kib": int(usage.ru_maxrss),
        "timed_out": timed_out,
        "cap_s": round(allowed, 2),
    }
    with open(stdout_path, "rb") as handle:
        record["stdout"] = handle.read().decode("utf-8", "replace")
    with open(stderr_path, "rb") as handle:
        record["stderr"] = handle.read().decode("utf-8", "replace")
    record["stdout_evidence"] = bounded_text(record["stdout"].encode("utf-8", "replace"))
    record["stderr_evidence"] = bounded_text(record["stderr"].encode("utf-8", "replace"))
    try:
        os.remove(stdout_path)
        os.remove(stderr_path)
    except OSError:
        pass
    return record


def declared_unexecuted(cases: list[dict]) -> list[dict]:
    """Every declared case/arm combination the collection never reached,
    recorded as unexecuted rather than silently absent."""
    present = {
        (row.get("workload"), row.get("case"), row.get("arm"))
        for row in cases
    }
    rows: list[dict] = []
    for case in CASE_TABLE:
        for arm in case["arms"]:
            key = (case["workload"], case["case"], arm)
            if key not in present:
                rows.append({"workload": case["workload"], "case": case["case"],
                             "arm": arm, "verdict": "unexecuted"})
    return rows


def median(samples: list[float]) -> float | None:
    ordered = sorted(samples)
    if not ordered:
        return None
    mid = len(ordered) // 2
    if len(ordered) % 2:
        return ordered[mid]
    return (ordered[mid - 1] + ordered[mid]) / 2


def timing_summary(cases: list[dict]) -> dict:
    """Per workload/case and arm: all sample counts, median/min/max, and the
    relative wall time and absolute saving against the cargo baseline arm.
    Only matching cases carry a time figure; slower arms report >1.0."""
    groups: dict[str, dict[str, list[float]]] = {}
    for row in cases:
        if row.get("verdict") != "match" or row.get("seconds") is None:
            continue
        key = f"{row.get('workload')}/{row.get('case')}"
        groups.setdefault(key, {}).setdefault(row.get("arm"), []).append(row["seconds"])
    summary: dict = {}
    for key, by_arm in sorted(groups.items()):
        baseline = by_arm.get("cargo") or next(iter(by_arm.values()))
        base_median = median(baseline)
        entry: dict = {}
        for arm, samples in sorted(by_arm.items()):
            arm_median = median(samples)
            figure: dict = {
                "samples": len(samples),
                "median_s": round(arm_median, 4),
                "min_s": round(min(samples), 4),
                "max_s": round(max(samples), 4),
            }
            if arm_median is not None and base_median:
                figure["relative_to_cargo"] = round(arm_median / base_median, 3)
                figure["absolute_saving_s"] = round(base_median - arm_median, 4)
            entry[arm] = figure
        summary[key] = entry
    return summary


def collection_exit(aborted: str | None, cases: list[dict]) -> int:
    """Exit 0 only for a complete, clean collection: nothing aborted, every
    declared case executed, and every verdict `match` (the documented known
    gap `mismatch-known-gap-1` is a finding and keeps the exit nonzero, so a
    divergence-only run can never read as a clean pass)."""
    if aborted:
        return 1
    for row in cases:
        if row.get("verdict") != "match":
            return 1
    return 0


def rotate(arms: tuple[str, ...], rep: int) -> tuple[str, ...]:
    shift = rep % len(arms)
    return arms[shift:] + arms[:shift]


class TrialRun:
    """One bound guarded occurrence. Every effect happens under the guard's
exported scratch, evidence and deadline, after the source and volume gates."""

    def __init__(self, mode: str, binding: dict) -> None:
        self.mode = mode
        self.binding = binding
        self.exports = guard_exports()
        self.deadline_ns = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
        self.env = scrubbed_env()
        self.cases: list[dict] = []
        self.figures: dict = {}
        self.aborted: str | None = None
        scratch = self.exports["OXIDE_TASK_SCRATCH_ROOT"]
        self.run_root = os.path.join(scratch, self.binding["paths"]["run_root_name"])
        self.packet_dir = os.path.join(
            self.exports["OXIDE_PROOF_EVIDENCE"], self.binding["paths"]["packet_name"]
        )

    # -- setup phases (infrastructure; a failure aborts) -------------------

    def setup(self) -> None:
        # Refuse before creating anything: mount/volume identity and the
        # source gate come first, then the owned roots exist.
        mount = mount_of(self.exports["OXIDE_TASK_SCRATCH_ROOT"])
        uuid = volume_uuid_of(mount)
        if uuid.upper() != self.binding["volume"]["uuid"].upper():
            raise Refusal(f"the selected volume is {uuid}, not the bound {self.binding['volume']['uuid']}")
        head, tree = resolve_source(self.binding)
        self.figures["volume"] = {"mount": mount, "uuid": uuid}
        self.figures["source"] = {"head": head, "tree": tree}
        if os.path.exists(self.run_root):
            raise Refusal(f"the bound run root already exists: {self.run_root}")
        os.makedirs(self.run_root)
        os.makedirs(self.packet_dir, exist_ok=True)
        self.private_home()
        self.acquire_upstream()
        self.build_relax()
        self.seed_cargo_home()
        self.stage_oxide()
        self.materialize()

    def private_home(self) -> None:
        home = os.path.join(self.run_root, "home")
        for leaf in (".makepad-run", "tmp"):
            os.makedirs(os.path.join(home, leaf), exist_ok=True)
        os.chmod(home, 0o700)
        self.env["HOME"] = home
        self.env["TMPDIR"] = os.path.join(home, "tmp")
        self.env["TMP"] = self.env["TMPDIR"]
        self.env["TEMP"] = self.env["TMPDIR"]
        self.env["OXIDE_TEST_HOME_FROM_ENV"] = "1"
        self.env["MAKEPAD_RUN_HOME"] = os.path.join(home, ".makepad-run")
        self.figures["home"] = home

    def acquire_upstream(self) -> None:
        dest = os.path.join(self.run_root, "relax-src")
        # --progress forces the transfer lines into the captured stderr even
        # when it is a file, so the received byte figure is parseable.
        clone = ["git", "clone", "--progress", UPSTREAM_URL, dest]
        started = time.monotonic()
        record = measure_one(clone, self.run_root, child_env(self.env), CAP_CLONE_S, self.deadline_ns)
        if record["exit"] != 0:
            raise Refusal(f"upstream clone failed: {record['stderr'][:300]!r}")
        network_bytes = None
        for match in re.finditer(
            r"Receiving objects:\s+\d+% \(\d+/\d+\), ([0-9.]+) (KiB|MiB|GiB)",
            record["stderr"],
        ):
            value, unit = float(match.group(1)), match.group(2)
            network_bytes = int(value * {"KiB": 1024, "MiB": 1024**2, "GiB": 1024**3}[unit])
        checkout = measure_one(
            ["git", "checkout", UPSTREAM_COMMIT], dest, child_env(self.env), 30, self.deadline_ns
        )
        if checkout["exit"] != 0:
            raise Refusal(f"upstream checkout failed: {checkout['stderr'][:300]!r}")
        verify = subprocess.run(
            ["git", "rev-parse", "HEAD^{tree}"], cwd=dest, capture_output=True, text=True, timeout=20
        )
        if verify.returncode != 0 or verify.stdout.strip() != UPSTREAM_TREE:
            raise Refusal(
                f"upstream tree is {verify.stdout.strip()!r}, not the bound {UPSTREAM_TREE}"
            )
        self.relax_src = dest
        self.figures["acquisition"] = {
            "commit": UPSTREAM_COMMIT,
            "tree": UPSTREAM_TREE,
            "network_seconds": round(time.monotonic() - started, 3),
            "network_bytes": network_bytes,
            "network_bytes_source": (
                "parsed from the forced git clone progress output" if network_bytes is not None
                else "unparsed; occupied bytes below bound the checkout instead"
            ),
            "occupied": walk_bytes(dest),
        }

    def build_relax(self) -> None:
        cargo_home = os.path.join(self.run_root, "cargo-homes", "relax-build")
        os.makedirs(cargo_home)
        env = child_env(
            self.env,
            CARGO_HOME=cargo_home,
            RUSTUP_TOOLCHAIN=TOOLCHAIN,
            CARGO_TARGET_DIR=os.path.join(self.run_root, "stage-relax-target"),
        )
        record = measure_one(
            ["cargo", "build", "--release", "-p", "relax-runner", "--bin", "relax"],
            self.relax_src, env, CAP_ENGINE_BUILD_S, self.deadline_ns,
        )
        if record["exit"] != 0:
            raise Refusal(
                "the pinned Relax engine did not build with the pinned toolchain "
                f"(exit {record['exit']}); this is a compatibility blocker: {record['stderr'][:400]!r}"
            )
        binary = os.path.join(self.run_root, "bin")
        os.makedirs(binary, exist_ok=True)
        staged = os.path.join(binary, "relax")
        shutil.copy2(
            os.path.join(self.run_root, "stage-relax-target", "release", "relax"), staged
        )
        version = measure_one([staged, "--version"], binary, child_env(self.env), 20, self.deadline_ns)
        self.relax_bin = staged
        self.figures["engine"] = {
            "sha256": sha256_file(staged),
            "build_seconds": record["seconds"],
            "version": version["stdout"].strip()[:200],
            "command": record["argv"],
        }

    def seed_cargo_home(self) -> None:
        import offline_seed

        host_home = os.environ.get("CARGO_HOME") or os.path.join(
            os.environ.get("HOME", os.path.expanduser("~")), ".cargo"
        )
        with open(os.path.join(REPO, "Cargo.lock"), "rb") as handle:
            lock_bytes = handle.read()
        destination = os.path.join(self.run_root, "cargo-homes", "shared")
        os.makedirs(destination)
        started = time.monotonic()
        seed = offline_seed.seed_cargo_home(
            host_home,
            destination,
            lock_bytes,
            deadline_ns=self.deadline_ns - int(STOP_MARGIN_S * 1e9),
        )
        self.shared_cargo_home = destination
        self.figures["seed"] = {
            "host_home": str(host_home),
            "destination": destination,
            "aggregate_bytes": seed.get("aggregate_bytes"),
            "seconds": round(time.monotonic() - started, 3),
            "network_seconds": 0.0,
            "note": "offline seed from the invoking Cargo home; no download",
        }

    def stage_oxide(self) -> None:
        # The staged front door is a dev build (its test controls stay present)
        # with declared debug-info reductions for fit; the measured fixture
        # commands get their recipe from the declared environment, never from
        # this binary's own build profile.
        env = child_env(
            self.env,
            CARGO_HOME=self.shared_cargo_home,
            RUSTUP_TOOLCHAIN=TOOLCHAIN,
            CARGO_TARGET_DIR=os.path.join(self.run_root, "stage-oxide-target"),
            CARGO_BUILD_BUILD_DIR=os.path.join(self.run_root, "stage-oxide-build"),
            CARGO_PROFILE_DEV_DEBUG="false",
            CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG="false",
        )
        record = measure_one(
            ["cargo", "build", "--locked", "--offline", "--bin", "oxide"],
            REPO, env, CAP_STAGE_S, self.deadline_ns,
        )
        if record["exit"] != 0:
            raise Refusal(f"staging the Oxide front door failed: {record['stderr'][:400]!r}")
        binary = os.path.join(self.run_root, "bin")
        staged = os.path.join(binary, "oxide")
        shutil.copy2(os.path.join(self.run_root, "stage-oxide-target", "debug", "oxide"), staged)
        version = measure_one([staged, "--version"], binary, child_env(self.env), 20, self.deadline_ns)
        self.oxide_bin = staged
        self.figures["front_door"] = {
            "sha256": sha256_file(staged),
            "stage_seconds": record["seconds"],
            "stage_recipe": "dev profile with CARGO_PROFILE_DEV_DEBUG=false",
            "version": version["stdout"].strip()[:120],
        }

    def materialize(self) -> None:
        self.works: dict[str, dict[str, str]] = {"greet": {}, "corr": {}, "snapshot": {}}
        base = self.env
        for workload in ("greet", "corr-suite"):
            for arm in ARMS:
                source_dir = fixture_path(workload)
                dest = os.path.join(self.run_root, "workloads", workload, arm, workload)
                shutil.copytree(source_dir, dest)
                self.works["corr" if workload == "corr-suite" else workload][arm] = dest
        # Locks: established offline against the seeded home before any --locked call.
        for workload, key in (("greet", "greet"), ("corr-suite", "corr")):
            for arm in ARMS:
                workdir = self.works[key][arm]
                env = child_env(base, CARGO_HOME=self.shared_cargo_home, RUSTUP_TOOLCHAIN=TOOLCHAIN)
                record = measure_one(
                    ["cargo", "generate-lockfile", "--offline"], workdir, env, CAP_LOCK_S, self.deadline_ns
                )
                if record["exit"] != 0:
                    raise Refusal(
                        f"offline lock establishment failed for {workload}/{arm}: {record['stderr'][:300]!r}"
                    )
        # The snapshot: this repository's bound tree, materialized per arm.
        for arm in ARMS:
            dest = os.path.join(self.run_root, "workloads", "snapshot", arm, "tree")
            os.makedirs(dest, exist_ok=True)
            archive = subprocess.run(
                ["git", "archive", self.binding["head"]],
                cwd=REPO, capture_output=True, timeout=30,
            )
            if archive.returncode != 0:
                raise Refusal("git archive of the bound head failed")
            untar = subprocess.run(
                ["tar", "-x", "-C", dest], input=archive.stdout, timeout=60
            )
            if untar.returncode != 0:
                raise Refusal("materializing the snapshot failed")
            self.works["snapshot"][arm] = dest
        self.figures["workloads"] = {
            key: {arm: path for arm, path in arms.items()} for key, arms in self.works.items()
        }

    # -- arms --------------------------------------------------------------

    def arm_argv_env(self, arm: str, workload: str, program_args: list[str]) -> tuple[list[str], dict]:
        workdir = self.works[workload][arm]
        if workload == "snapshot":
            package_flags = ["--bin", "oxide"]
            relax_target = "oxide-storage"
        else:
            package_flags = ["--bin", workload]
            relax_target = workload
        trailing = ["--"] + program_args if program_args else []
        if arm == "cargo":
            env = child_env(
                quick_recipe_env(self.env),
                CARGO_HOME=self.shared_cargo_home,
                RUSTUP_TOOLCHAIN=TOOLCHAIN,
                CARGO_TARGET_DIR=os.path.join(os.path.dirname(workdir), "target"),
            )
            argv = ["cargo", "run", "--locked", "--offline", "--quiet",
                    "--bin", package_flags[1], *trailing]
            return argv, env
        if arm == "oxide":
            arm_home = os.path.join(os.path.dirname(workdir), "home")
            env = child_env(
                self.env,
                HOME=arm_home,
                OXIDE_TEST_HOME_FROM_ENV="1",
                OXIDE_POOL=os.path.join(os.path.dirname(workdir), "pool"),
                CARGO_HOME=self.shared_cargo_home,
                TMPDIR=os.path.join(arm_home, "tmp"),
                TMP=os.path.join(arm_home, "tmp"),
                TEMP=os.path.join(arm_home, "tmp"),
            )
            os.makedirs(arm_home, exist_ok=True)
            os.makedirs(os.path.join(arm_home, "tmp"), exist_ok=True)
            argv = [self.oxide_bin, "run", "--bin", package_flags[1], *trailing]
            return argv, env
        env = child_env(
            self.env,
            HOME=self.env["HOME"],
            MAKEPAD_RUN_HOME=self.env["MAKEPAD_RUN_HOME"],
            MAKEPAD_RUN_OFFLINE="1",
            CARGO_HOME=self.shared_cargo_home,
        )
        argv = [self.relax_bin, "run", "--check-first", "--no-live", "--quiet",
                relax_target, *package_flags, *trailing]
        return argv, env

    def run_arm(self, arm: str, workload: str, program_args: list[str], cap_s: float) -> dict:
        argv, env = self.arm_argv_env(arm, workload, program_args)
        cwd = self.works[workload][arm]
        record = measure_one(argv, cwd, env, cap_s, self.deadline_ns)
        record["arm"] = arm
        record["workload"] = workload
        return record

    # -- cases -------------------------------------------------------------

    def record_case(self, workload: str, case: str, record: dict, verdict: str) -> None:
        row = {
            "workload": workload,
            "case": case,
            "arm": record.get("arm"),
            "seconds": record.get("seconds"),
            "rss_max_kib": record.get("rss_max_kib"),
            "exit": record.get("exit"),
            "timed_out": record.get("timed_out"),
            "verdict": verdict,
            "stdout_evidence": record.get("stdout_evidence"),
            "stderr_evidence": record.get("stderr_evidence"),
        }
        self.cases.append(row)

    def unexecuted_rows(self) -> list[dict]:
        return declared_unexecuted(self.cases)

    def apply_edit_everywhere(self, path_relative: str, old: str, new: str, count: int = 1) -> str:
        digest = None
        for workload_key, workdir in self._edit_targets(path_relative):
            full = os.path.join(workdir, path_relative)
            with open(full, "r", encoding="utf-8") as handle:
                text = handle.read()
            edited = apply_edit(text, old, new, count)
            with open(full, "w", encoding="utf-8") as handle:
                handle.write(edited)
            current = hashlib.sha256(edited.encode()).hexdigest()
            if digest is None:
                digest = current
            elif digest != current:
                raise Refusal(f"the edit produced different bytes across arms at {path_relative}")
        if digest is None:
            raise Refusal("the edit reached no workdir")
        return digest

    def _edit_targets(self, path_relative: str) -> list[tuple[str, str]]:
        if path_relative.startswith("greet-core/"):
            return [("greet", self.works["greet"][arm]) for arm in ARMS]
        if path_relative.startswith("src/"):
            return [("snapshot", self.works["snapshot"][arm]) for arm in ARMS]
        raise Refusal(f"no edit mapping for {path_relative}")

    def greet_block(self) -> None:
        cap = CAP_SMALL_S
        for arm in ARMS:
            record = self.run_arm(arm, "greet", ["Tom"], cap)
            self.record_case("greet", "cold", record,
                             oracle_exact_stdout(record, GREET_EXPECTED_V1 + "\n"))
        self.cache_snapshot("greet-cold")
        for rep in range(5):
            if budget_s(self.deadline_ns, cap) <= 0:
                self.figures.setdefault("reps_reached", {})["greet-warm"] = rep
                break
            for arm in rotate(ARMS, rep):
                record = self.run_arm(arm, "greet", ["Tom"], cap)
                self.record_case("greet", "warm-unchanged", record,
                                 oracle_exact_stdout(record, GREET_EXPECTED_V1 + "\n"))
        self.apply_edit_everywhere("greet-core/src/lib.rs", GREET_BODY_OLD, GREET_BODY_NEW)
        for rep in range(5):
            if budget_s(self.deadline_ns, cap) <= 0:
                self.figures.setdefault("reps_reached", {})["greet-body"] = rep
                break
            for arm in rotate(ARMS, rep):
                record = self.run_arm(arm, "greet", ["Tom"], cap)
                self.record_case("greet", "edit-body", record,
                                 oracle_exact_stdout(record, GREET_EXPECTED_V2 + "\n"))
        self.apply_edit_everywhere("greet-core/src/lib.rs", GREET_INTERFACE_OLD, GREET_INTERFACE_NEW)
        for rep in range(5):
            if budget_s(self.deadline_ns, cap) <= 0:
                self.figures.setdefault("reps_reached", {})["greet-interface"] = rep
                break
            for arm in rotate(ARMS, rep):
                record = self.run_arm(arm, "greet", ["Tom"], cap)
                self.record_case("greet", "edit-interface", record,
                                 oracle_exact_stdout(record, GREET_EXPECTED_V2 + "\n"))
        self.cache_snapshot("greet-final")

    def snapshot_block(self) -> None:
        cap = CAP_SNAPSHOT_S
        program = ["--help"]
        for arm in ARMS:
            record = self.run_arm(arm, "snapshot", program, cap)
            verdict = oracle_fragments(
                record, exit_codes=(0,), stdout_contains=(SNAPSHOT_HELP_MARKER,),
                stdout_not_contains=("[trial]",),
            )
            self.record_case("snapshot", "cold", record, verdict)
        self.cache_snapshot("snapshot-cold")
        for label, edit in (("warm-unchanged", None), ("edit-body", "body"),
                            ("edit-interface", "interface")):
            if edit == "body":
                self.apply_edit_everywhere("src/bin/oxide/main.rs", SNAPSHOT_BODY_OLD, SNAPSHOT_BODY_NEW)
            elif edit == "interface":
                text_target = "src/dep_info.rs"
                self.apply_edit_everywhere(text_target, SNAPSHOT_INTERFACE_OLD, SNAPSHOT_INTERFACE_NEW)
                self.apply_edit_everywhere(text_target, SNAPSHOT_INTERFACE_CAST_OLD,
                                           SNAPSHOT_INTERFACE_CAST_NEW, SNAPSHOT_INTERFACE_CASTS)
            # Cumulative sequence: after the body edit the [trial] marker stays
            # applied, so both later cases expect the edited help line.
            expected_contains = (
                (SNAPSHOT_HELP_MARKER,) if edit is None else (SNAPSHOT_HELP_EDITED,)
            )
            expected_absent = ("[trial]",) if edit is None else ()
            for rep in range(5):
                if budget_s(self.deadline_ns, cap) <= 0:
                    self.figures.setdefault("reps_reached", {})[f"snapshot-{label}"] = rep
                    break
                for arm in rotate(ARMS, rep):
                    record = self.run_arm(arm, "snapshot", program, cap)
                    verdict = oracle_fragments(
                        record, exit_codes=(0,), stdout_contains=expected_contains,
                        stdout_not_contains=expected_absent,
                    )
                    self.record_case("snapshot", label, record, verdict)
        self.cache_snapshot("snapshot-final")

    def corr_block(self) -> None:
        cap = CAP_SMALL_S
        suite = self.works["corr"]
        # The test-execution probe keeps only `--quiet`: the strict run flags
        # belong to the measured run path, and this case is a correspondence
        # probe, not test-route qualification (the brief's own carve-out).
        for arm in ("cargo", "relax"):
            argv, env = self.corr_command(arm, "basics", ["one", "two", "7"],
                                          env_overrides={"CORR_TRIAL_ENV": "corridor"})
            record = measure_one(argv, suite[arm], env, cap, self.deadline_ns)
            record["arm"], record["workload"] = arm, "corr"
            # stdout is exact for both arms. The cargo arm's stderr is exact;
            # the relax arm only requires the program's stderr line, because
            # its runner unconditionally logs `relax-remote: pid ...` on
            # stderr (inspect/mod.rs start()), which is harness output, not
            # program semantics.
            verdict = oracle_exact_with_exit(
                record, "args=one,two,7\nenv=corridor\ncwd=corr-suite\n", 7,
                expected_stderr="corr-trial-stderr-line\n" if arm == "cargo" else None,
                stderr_contains=() if arm == "cargo" else ("corr-trial-stderr-line",),
            )
            self.record_case("corr", "basics", record, verdict)
        # panic-caught / panic-uncaught / overflow / gen / threads-ffi / testkit / collision.
        for arm in ("cargo", "relax"):
            for package, args, case_name, oracle in (
                ("panic", ["caught"], "panic-caught",
                 lambda r: oracle_exact_stdout(r, "CAUGHT:corr-caught-boom\n")),
                ("panic", ["uncaught"], "panic-uncaught",
                 lambda r: oracle_fragments(
                     r, exit_codes=(101,), stderr_contains=(PANIC_LOCATION,))),
                ("overflow", [], "overflow",
                 lambda r: oracle_fragments(
                     r, exit_codes=(101,), stderr_contains=("attempt to add with overflow",))),
                ("gen", [], "gen",
                 lambda r: oracle_exact_stdout(r, "generated=gen-constant-v1\n")),
                ("threads-ffi", [], "threads-ffi", oracle_pid_line),
                ("collision-app", [], "collision", oracle_collision),
            ):
                argv, env = self.corr_command(arm, package, args)
                record = measure_one(argv, suite[arm], env, cap, self.deadline_ns)
                record["arm"], record["workload"] = arm, "corr"
                self.record_case("corr", case_name, record, oracle(record))
            # The uncaught-panic backtrace expectation.
            argv, env = self.corr_command(arm, "panic", ["uncaught"])
            env = child_env(env, RUST_BACKTRACE="1", RUST_LIB_BACKTRACE="1")
            record = measure_one(argv, suite[arm], env, cap, self.deadline_ns)
            record["arm"], record["workload"] = arm, "corr"
            combined = record["stdout"] + record["stderr"]
            verdict = "match" if (
                record["exit"] == 101 and PANIC_LOCATION in combined
            ) else ("timeout" if record["timed_out"] else "mismatch")
            self.record_case("corr", "panic-uncaught-backtrace", record, verdict)
            # Basic test execution: a correspondence probe, not qualification.
            # `relax test` carries leading flags through to the harness run, so
            # `--quiet` reaches it; the strict run flags are not claimed here.
            argv, env = self.corr_command(arm, "testkit", [], test=True)
            record = measure_one(argv, suite[arm], env, cap, self.deadline_ns)
            record["arm"], record["workload"] = arm, "corr"
            verdict = oracle_fragments(record, exit_codes=(0,), combined_contains=("adds", "concats"))
            self.record_case("corr", "testkit", record, verdict)

    def corr_command(self, arm: str, package: str, args: list[str], *, test: bool = False,
                     env_overrides: dict | None = None) -> tuple[list[str], dict]:
        suite = self.works["corr"][arm]
        if arm == "cargo":
            env = child_env(
                quick_recipe_env(self.env),
                CARGO_HOME=self.shared_cargo_home,
                RUSTUP_TOOLCHAIN=TOOLCHAIN,
                CARGO_TARGET_DIR=os.path.join(os.path.dirname(suite), "target"),
            )
            if test:
                argv = ["cargo", "test", "--locked", "--offline", "--quiet", "-p", package]
            else:
                argv = ["cargo", "run", "--locked", "--offline", "--quiet", "-p", package]
                if args:
                    argv += ["--"] + args
        else:
            env = child_env(
                self.env,
                HOME=self.env["HOME"],
                MAKEPAD_RUN_HOME=self.env["MAKEPAD_RUN_HOME"],
                MAKEPAD_RUN_OFFLINE="1",
                CARGO_HOME=self.shared_cargo_home,
            )
            if test:
                argv = [self.relax_bin, "test", "--quiet", package]
            else:
                argv = [self.relax_bin, "run", "--check-first", "--no-live", "--quiet", package]
                if args:
                    argv += ["--"] + args
        if env_overrides:
            env = child_env(env, **env_overrides)
        return argv, env

    def cache_snapshot(self, label: str) -> None:
        caches: dict[str, dict] = {}
        for workload, key in (("greet", "greet"), ("corr-suite", "corr"), (None, "snapshot")):
            for arm in ARMS:
                workdir = self.works[key][arm]
                if arm == "cargo":
                    cache = os.path.join(os.path.dirname(workdir), "target")
                elif arm == "oxide":
                    cache = os.path.join(os.path.dirname(workdir), "pool")
                else:
                    cache = os.path.join(workdir, "target", "makepad-run")
                    if not os.path.isdir(cache):
                        cache = os.path.join(os.path.dirname(workdir), "home", ".makepad-run")
                caches[f"{workload or 'snapshot'}/{arm}"] = (
                    walk_bytes(cache) if os.path.isdir(cache) else {"path": cache, "absent": True}
                )
        self.figures.setdefault("cache_bytes", {})[label] = caches

    # -- execution and receipt ---------------------------------------------

    def execute(self) -> None:
        blocks = (
            ("greet", self.greet_block, ()),
            ("snapshot", self.snapshot_block, ()),
            ("corr", self.corr_block, ()),
        )
        for name, block, _deps in blocks:
            if self.aborted:
                break
            try:
                block()
            except Refusal as error:
                # A deadline exhausted mid-block or an edit refusal is
                # infrastructure: stop, record, publish what was reached.
                self.aborted = f"{name}: {error}"
                break
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                if self.mode == "diagnostic":
                    self.cases.append({"workload": name, "case": "infra",
                                       "verdict": "aborted",
                                       "error": str(error)[:300]})
                    self.aborted = f"{name}: {error}"
                    break
                self.aborted = f"{name}: {error}"
                break

    def receipt(self) -> dict:
        verdicts: dict[str, int] = {}
        for row in self.cases:
            verdicts[row.get("verdict", "?")] = verdicts.get(row.get("verdict", "?"), 0) + 1
        document = {
            "schema": "oxide.relax-quick-trial/1",
            "mode": self.mode,
            "qualification": "none: this receipt is research evidence, never a QA pass",
            "closure": "the guard's post-leader census decides closure; this receipt claims none",
            "planner_reference": self.binding["planner_reference"],
            "upstream": self.figures.get("acquisition", {}).get("commit"),
            "toolchain": TOOLCHAIN,
            "source": self.figures.get("source"),
            "figures": {k: v for k, v in self.figures.items() if k != "acquisition"},
            "acquisition": self.figures.get("acquisition"),
            "timing_summary": timing_summary(self.cases),
            "verdicts": verdicts,
            "cases": self.cases,
            "cases_total": len(self.cases),
            "declared_unexecuted": sum(
                1 for row in self.cases if row.get("verdict") == "unexecuted"
            ),
            "aborted": self.aborted,
            "exit_rule": (
                "exit 0 only when nothing aborted, every declared case executed, and "
                "every verdict is match; a divergence-only run exits 1 and is a finding"
            ),
            "limits": {
                "wall_limit_s": LIMIT_SECONDS,
                "scratch_mib": SCRATCH_MIB,
                "stop_margin_s": STOP_MARGIN_S,
                "attribution": "ru_maxrss covers the direct child and its reaped descendants only",
                "cache_bytes": "occupied apparent size; never an SSD-write claim",
                "filesystem_caches": "not globally flushable under this scope",
            },
        }
        encoded = json.dumps(document, indent=1).encode()
        while len(encoded) > MAX_RECEIPT_BYTES and len(document["cases"]) > 1:
            document["cases"].pop()
            document["cases_omitted"] = document.get("cases_omitted", 0) + 1
            document["cases_total"] = len(self.cases)
            encoded = json.dumps(document, indent=1).encode()
        return encoded

    def publish(self) -> None:
        # Unreached declared cases become unexecuted rows before the receipt
        # is built, so absence is never silent.
        self.cases.extend(self.unexecuted_rows())
        self.figures["owned_bytes"] = {
            "run_root": walk_bytes(self.run_root) if os.path.isdir(self.run_root)
            else {"path": self.run_root, "absent": True},
            "packet_dir": walk_bytes(self.packet_dir) if os.path.isdir(self.packet_dir)
            else {"path": self.packet_dir, "absent": True},
            "note": "occupied apparent bytes of the owned scratch and evidence packet; "
                    "never an SSD-write claim",
        }
        self._receipt_encoded = self.receipt()
        destination = os.path.join(
            self.packet_dir, f"receipt-{self.binding['paths']['packet_name']}.json"
        )
        temporary = destination + ".tmp"
        with open(temporary, "wb") as handle:
            handle.write(self._receipt_encoded)
        os.replace(temporary, destination)
        print(f"receipt: {destination} ({len(self._receipt_encoded)} bytes)")


def guarded_main(mode: str, binding_path: str, binding_sha: str) -> int:
    guard_exports()  # refuse before reading anything else
    try:
        with open(binding_path, "rb") as handle:
            payload = handle.read()
    except OSError as error:
        raise Refusal(f"the activation document is unreadable: {error}")
    if hashlib.sha256(payload).hexdigest() != binding_sha:
        raise Refusal("the activation document digest is not the supplied --sha256")
    binding = parse_binding(payload, mode)
    run = TrialRun(mode, binding)
    try:
        run.setup()
    except (Refusal, OSError, ValueError, subprocess.SubprocessError) as error:
        run.aborted = f"setup: {error}"
        run.publish()
        print(f"blocked before any case: {error}", file=sys.stderr)
        return 1
    run.execute()
    run.publish()
    exit_code = collection_exit(run.aborted, run.cases)
    summary = {
        "verdicts": {
            verdict: sum(1 for row in run.cases if row.get("verdict") == verdict)
            for verdict in sorted({row.get("verdict", "?") for row in run.cases})
        },
        "cases": len(run.cases),
        "declared_unexecuted": sum(
            1 for row in run.cases if row.get("verdict") == "unexecuted"
        ),
        "aborted": run.aborted,
        "exit": exit_code,
    }
    print(json.dumps(summary))
    if exit_code != 0:
        print(
            "collection carries findings, unreached cases or an abort; exit 1 "
            "(uncounted diagnostic, never a pass)",
            file=sys.stderr,
        )
        return 1
    return 0


def main(argv: list[str]) -> int:
    mode, binding, sha = parse_arguments(argv)
    if mode == "refuse":
        print(__doc__.splitlines()[0])
        print(f"refused: {sha}", file=sys.stderr)
        return 2
    if mode == "help":
        print(__doc__)
        return 0
    if mode == "plan":
        print(plan_text())
        return 0
    if mode == "self-check":
        self_check()
        return 0
    assert binding is not None and sha is not None
    try:
        return guarded_main(mode, binding, sha)
    except Refusal as error:
        print(f"refused before any effect: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
