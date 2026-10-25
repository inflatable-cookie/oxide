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




Diagnostic mode (`--diagnostic`, source only in this phase; never executed
here). The default proof still fails fast: the first failure stops it exactly
as before. `--diagnostic` records a recoverable oracle failure and continues
only with cases whose declared dependencies are all intact; a failure in a case
that mutates shared state skips every later case, because that shared state is
no longer positively known; and an unknown child, device, writer or
infrastructure outcome aborts in both modes rather than being recorded as an
oracle mismatch. It runs under the same qualified supervisor, the same 300 s
and 512 MiB bounds and the same custody and cleanup rules, and it writes a
bounded, distinct receipt labelled `qualified: false` with the per-case
outcome, first fault, at most eight secondary faults and every truthful skip
reason. A diagnostic receipt is never counted native evidence and never claims
a PASS. It is dispatched only through `effigy rust:pool-temp-scratch:diagnostic`
(Effigy selectors do not forward passthrough arguments), it is required before
the next counted native run, and a source sweep complements it but never
replaces it. No diagnostic run was made in this source phase; ROOT activates
it separately after independent source review.

Command/state premise walkthrough for all twelve executable cases. Each line
names the executable commands, their working directory, the state the case
needs, where that state is established in this file, and the output grammar the
oracle reads. This is the source-level sweep used instead of a keep-going
native run: the proof mutates one shared pool, account and image, so a
mid-run snapshot cannot be re-entered safely without spending a counted
native; every precondition below is therefore established by explicit code
before the command that needs it.

1. premise and oracle sweep (case 1): `oxide --version` and plain
   `cargo check --offline --locked` in the fixture crate. State: crate files and
   the manifest-consistent lock (`new_fixture` -> `make_fixture` +
   `ensure_fixture_lock`, re-checked by `require_lock_precondition`), private
   HOME/CARGO_HOME, resolved absolute toolchain home, `world.scratch` outside
   the pool. Grammar: identity text; `cargo:warning=OXIDE-TEMP dir=<path>` on
   Cargo's stderr; then `oxide metadata`/`oxide fmt` create no slot.
2. same-volume inherited TMPDIR (case 2): `oxide check` with an inherited
   directory inside the pool. State: `private_dir(inherited)`, same device as
   `world.pool`, no slot scratch. Grammar: build-script stderr and the
   `temp: inherited <canonical>` summary fragment.
3. unusable inherited TMPDIR (case 3): same command over absent/file/relative/
   unwritable/symlink values, plus a device-gated other-device value. State:
   each bad value created in `replaced`; the pool device read from
   `os.stat(canonical(world.pool))`. Grammar: `temp: oxide <path>` and the
   scratch path under the pool payload.
4. existing relative TMPDIR (case 4): `oxide check` from the crate with
   `TMPDIR=rel-tmp` and `FIXTURE_CHDIR`. State: `crate/rel-tmp` exists; the
   chdir target exists; manifest/lock as in case 1. Grammar: the post-chdir
   `OXIDE-TEMP dir-after-chdir=` line and the `after-chdir` marker in the
   qualified directory.
5. aliases (case 5): `oxide check` twice (inherited and owned branches). State:
   an inherited directory on the pool device; conflicting TMP/TEMP. Grammar:
   the three `OXIDE-TEMP <NAME>=` lines.
6. owned scratch observed while held (case 6): `oxide check` with a gated build
   script (`FIXTURE_HOLD`), then `oxide status --measure`. State: the gated
   writer announces `OXIDE-TEMP holding` and writes the 1 MiB marker, symlink
   and hard link; the slot is held while the measurement runs. Grammar:
   `OXIDE-TEMP dir=`, the measured `slot <id> ... allocated` line, and
   `temp: oxide` in the measured status.
7. normal release (case 7): no command; the assertion after case 6. State: the
   released slot's `scratch_dirs()` must be gone.
8. crash with a surviving group (case 8): gated `oxide check` killed by PID,
   then `kill(-pgid, 0)` on the group the fixture reports, then `oxide check`.
   State: the crate already built once (case 1); the gate file exists only when
   the proof writes it; the leftover file is planted while the group is alive.
   Grammar: `OXIDE-TEMP holding`/`dir=`/`pgid=`, then the recovery command's
   `OXIDE-TEMP dir=`.
9. test, exec and run children (case 9): `oxide test`, `oxide exec -- /bin/sh`,
   `oxide run` with and without `RUN_HOLD`, then a competing `oxide check`.
   State: manifest/lock from case 1; one binary so `default-run` resolves
   without ambiguity; the test target writes into its temporary directory.
   Grammar: Cargo's test result on **stdout** (checked in stdout+stderr), the
   exec tool's `$TMPDIR/$TMP/$TEMP` on stdout, the run program's
   `OXIDE-TEMP run=` line, and the competing command's own `temp:` line.
10. refusals (case 10): `oxide check` against a link and a foreign directory in
    the owned position, then against an over-bound pool. State: the crate built
    once so a slot exists (`world.slot_roots()[0]`); the link and foreign entry
    planted by the proof; the over-bound pool nested under the guard scratch.
    Grammar: non-zero exit with `real directory`, `marker` and `Unix-socket
    bound` on stderr, and no `OXIDE-TEMP dir=` line (no child ran).
11. removable image and fallback (case 11): `hdiutil create|attach -owners on`,
    `diskutil info -plist`, `oxide pool register-removable`, `oxide check`,
    `hdiutil detach`, then `oxide check` again. State: one owned 128 MiB sparse
    image in the guard scratch; ownership enabled before registration; the
    internal `world.pool` already exists for the fallback. Grammar: the attach
    plist's mount point, `GlobalPermissionsEnabled`, the build's `temp: oxide`
    path under `<mount>/oxide-pool`, and the fallback path under
    `world.pool/payload`.
12. socket bound (case 12): `oxide check` with no inherited TMPDIR. State: the
    pool and crate from earlier cases. Grammar: the build-script
    `OXIDE-TEMP dir=` path, required to end in `scratch` and fit 91 bytes.

Two analogous state/grammar errors found by this walkthrough and corrected in
the same batch as the lock fix: (a) the `--locked` oracle ran without the lock
precondition (the native-2 failure), and (b) case 9 read `test result: ok` from
stderr, where Cargo never writes it.

Fixed proof-authoring checklist sweep (Supervisor b18d0552), each item either
an executable-body fact or an explicit not-applicable reason. `--self-check`
is a pure, pre-scratch regression of items 1, 2, 3, 5 and 7; it creates no
scratch, no child process and no toolchain, and it dispatches before the
scratch root, the toolchain decision or staging.

1. Toolchain provenance: `resolve_toolchain_home` honours an absolute, usable
   inherited `RUSTUP_HOME`, else resolves the account default `<HOME>/.rustup`
   from the *original* HOME before any private fixture HOME exists, and
   `toolchain_environment` publishes that absolute path to staging and every
   fixture child with `RUSTUP_TOOLCHAIN=1.97.1` and `RUSTUP_AUTO_INSTALL=0`.
   A missing, relative or absent toolchain home refuses before scratch or
   staging instead of hiding behind a fake HOME. (The first authorised author
   native failed exactly here: the fixture HOME hid the installed 1.97.1.)
2. Private account home: every fixture child carries the debug build's
   `OXIDE_TEST_HOME_FROM_ENV=1`, so the private fixture HOME is the account home
   and the real account's config, pool record and status are untouched.
3. No imported ambient controls: the fixture environment is built fresh (it can
   carry only `OXIDE_TEST_HOME_FROM_ENV` plus the proof's own explicit pool
   controls), and the staging environment scrubs `OXIDE_*`, `XDG_*` and
   `CARGO_ENCODED_RUSTFLAGS`; the self-check rejects a hostile parent that
   would import any of them.
4. Auto-adopt premise: `make_fixture` asserts the fresh checkout has no
   `target/`, and the internal pool has no permanent registration, so the first
   build cannot mistake a converted root for fresh work.
5. External Cargo argv: the only external Cargo invocations are the recorded
   `CARGO_BUILD_ARGV` (staging) and `CARGO_ORACLE_ARGV` (premise oracle), both
   with the subcommand name in argv, which the self-check asserts.
6. Owned removable image: case 11 attaches it with `-owners on`, parses the
   mount point from the plist and confirms `GlobalPermissionsEnabled` through
   `diskutil` before registration (the mount-root rule).
7. Frozen build flags: `FROZEN_RUSTFLAGS` is shared by staging and every
   fixture child, and an inherited `CARGO_ENCODED_RUSTFLAGS` (which outranks
   RUSTFLAGS in Cargo) is scrubbed, so the recipe cannot drift.
8. TMPDIR/scratch selection on the selected volume: not applicable inside this
   proof, which *is* the oxide#136 source under test; its fixture TMPDIR is the
   proof's own private scratch, and no oxide#145 default or unqualified temp
   root is borrowed.
9. `mktemp` without a template ignores TMPDIR: not applicable, this proof calls
   no `mktemp`; every scratch directory is an explicit `private_dir(...)` under
   `OXIDE_TASK_SCRATCH_ROOT` inside the qualified guard.

`--plan` prints the allocation and cases and touches nothing. No live pool,
registered drive, install receipt, host setting or consumer is touched; the
fixture image is created, attached and ordinarily detached inside the
supervisor's scratch.
"""

from __future__ import annotations

import contextlib
import hashlib
import io
import json
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
# The one flag set the staging build and every fixture child share.
FROZEN_RUSTFLAGS = "-C strip=symbols"
# External Cargo argv used by this proof. The subcommand name is part of the
# argv, which the pure self-check asserts.
CARGO_BUILD_ARGV = ("cargo", "build", "-j", "1", "--locked", "--offline", "--bin", "oxide")
CARGO_ORACLE_ARGV = ("cargo", "check", "--offline", "--locked")
# Fixture setup, not an oracle: the pinned Cargo writes the fixture crate's own
# lock offline in the fixture's private Cargo home before any `--locked` call.
LOCK_ARGV = ("cargo", "generate-lockfile", "--offline")
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


MODE = {"diagnostic": False}
# Bound on recorded faults in a diagnostic receipt (bytes and count).
MAX_DIAGNOSTIC_FAULTS = 8
MAX_DIAGNOSTIC_RECEIPT_BYTES = 64 * 1024


class Recoverable(Exception):
    """A declared oracle mismatch: the child state is known and closed."""


class UnknownOutcome(Exception):
    """A child, device, writer or infrastructure outcome this proof cannot
    classify. Both modes stop; diagnostic mode never continues past it."""


def fail(message: str) -> None:
    """A declared failure. `drive` stops on it by default and records it in
    diagnostic mode; `main` turns an uncaught one into exit 1 with the message."""
    raise Recoverable(message)


def expect(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def abort(message: str) -> None:
    """Stop in both modes: an unknown state is never recorded as an oracle
    failure, never skipped over and never turned into a partial pass."""
    raise UnknownOutcome(message)


def require(condition: bool, message: str) -> None:
    if not condition:
        abort(message)


class ProofCase:
    """One executable case, its own dependencies and its shared-state effect."""

    def __init__(self, index: int, body, depends=(), mutates=False, label=None):
        self.index = index
        self.body = body
        self.depends = tuple(depends)
        self.mutates = mutates
        self.name = label or CASE_NAMES[index]


def diagnostic_receipt_path(root: str) -> str:
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE")
    base = evidence if evidence and os.path.isdir(evidence) else root
    return os.path.join(base, "diagnostic-receipt.json")


def diagnostic_record(outcomes, faults, skips, aborted) -> str:
    """Bounded, distinct, explicitly unqualified diagnostic record.

    It records what actually ran, the first fault, at most
    `MAX_DIAGNOSTIC_FAULTS` secondary faults, every truthful skip reason and
    whether the run aborted on an unknown outcome. It never claims
    qualification and never invents a PASS.
    """
    record = {
        "mode": "diagnostic",
        "qualified": False,
        "evidence_kind": "diagnostic-only, not counted native evidence",
        "cases": [
            {"case": case.name, "outcome": outcome, "detail": detail[:600]}
            for case, outcome, detail in outcomes
        ],
        "first_fault": (
            {"case": faults[0][0], "detail": faults[0][1][:1200]} if faults else None
        ),
        "secondary_faults": [
            {"case": name, "detail": detail[:600]} for name, detail in faults[1:MAX_DIAGNOSTIC_FAULTS]
        ],
        "faults_recorded": min(len(faults), MAX_DIAGNOSTIC_FAULTS),
        "faults_seen": len(faults),
        "skips": [{"case": name, "reason": reason[:600]} for name, reason in skips],
        "aborted_on_unknown_outcome": aborted,
    }
    text = json.dumps(record, indent=1, sort_keys=True)
    if len(text.encode()) > MAX_DIAGNOSTIC_RECEIPT_BYTES:
        text = text.encode()[:MAX_DIAGNOSTIC_RECEIPT_BYTES].decode(errors="ignore")
    return text


def write_diagnostic_receipt(path: str, outcomes, faults, skips, aborted) -> str:
    with open(path, "wb") as handle:
        handle.write(diagnostic_record(outcomes, faults, skips, aborted).encode())
    return path


def drive(cases, root, receipt=True) -> int:
    """Run the cases, failing fast by default, or diagnosing when asked.

    Default: the first failure stops the proof exactly as before. Diagnostic:
    a declared oracle failure is recorded and the run continues only with cases
    whose declared dependencies are all intact; a failure in a case that
    mutates shared state skips every later case, because that shared state is
    not positively known; an unknown child/device/writer/infrastructure
    outcome aborts in both modes. Returns 0 only when every case passed.
    """
    outcomes = []
    faults = []
    skips = []
    failed = set()
    shared_state_unknown = None
    aborted = False
    for case in cases:
        if shared_state_unknown is not None:
            outcomes.append((case, "skipped", shared_state_unknown))
            skips.append((case.name, shared_state_unknown))
            continue
        blocked = [dep for dep in case.depends if dep in failed]
        if blocked:
            reason = "depends on failed case(s) " + ", ".join(
                str(dep + 1) for dep in blocked
            )
            outcomes.append((case, "skipped", reason))
            skips.append((case.name, reason))
            continue
        try:
            case.body()
        except UnknownOutcome as error:
            aborted = True
            outcomes.append((case, "unknown", str(error)))
            faults.append((case.name, str(error)))
            break
        except Recoverable as error:
            failed.add(case.index)
            outcomes.append((case, "fail", str(error)))
            faults.append((case.name, str(error)))
            if not MODE["diagnostic"]:
                for remaining in cases[case.index + 1 :]:
                    outcomes.append(
                        (remaining, "not_run", "default mode stops at the first failure")
                    )
                break
            if case.mutates:
                shared_state_unknown = (
                    f"case {case.index + 1} failed after mutating shared state"
                )
            continue
        except Exception as error:  # noqa: BLE001 - an unclassified outcome stops
            aborted = True
            outcomes.append((case, "unknown", f"{type(error).__name__}: {error}"))
            faults.append((case.name, f"{type(error).__name__}: {error}"))
            break
        outcomes.append((case, "pass", ""))
    if MODE["diagnostic"] and receipt:
        path = write_diagnostic_receipt(
            diagnostic_receipt_path(root), outcomes, faults, skips, aborted
        )
        say(f"diagnostic receipt {path} (qualified: false)")
        for case, outcome, detail in outcomes:
            say(f"diagnostic {outcome}: {case.name}{(' - ' + detail) if detail else ''}")
    if aborted:
        print(
            "pool-temp-scratch proof stopped on an unknown outcome: "
            + (faults[-1][1] if faults else "unclassified"),
            file=sys.stderr,
        )
        return 1
    if faults:
        print(
            f"pool-temp-scratch proof failed: {faults[0][0]}: {faults[0][1]}",
            file=sys.stderr,
        )
        return 1
    return 0


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


class ToolchainError(Exception):
    """The pinned toolchain cannot be reached, so the run must refuse."""


def resolve_toolchain_home(environ, is_dir=os.path.isdir) -> str:
    """The absolute toolchain home every child of this proof must use.

    Called once, before any private fixture HOME exists, so the choice is made
    from the caller's real account provenance. An explicit `RUSTUP_HOME` is
    honoured when it is absolute and usable; otherwise the account default
    `<HOME>/.rustup` is resolved from that same original HOME. `is_dir` is
    injectable so the pure self-check can exercise the decision with no
    filesystem, child process or toolchain.
    """
    explicit = environ.get("RUSTUP_HOME")
    if explicit:
        if not os.path.isabs(explicit):
            raise ToolchainError(
                f"the inherited RUSTUP_HOME is not an absolute path: {explicit!r}"
            )
        if not is_dir(explicit):
            raise ToolchainError(
                f"the inherited RUSTUP_HOME is not a directory: {explicit}"
            )
        return explicit
    home = environ.get("HOME")
    if not home:
        raise ToolchainError(
            "neither RUSTUP_HOME nor HOME is set, so the pinned toolchain home "
            f"for {BUILDER} cannot be resolved"
        )
    if not os.path.isabs(home):
        raise ToolchainError(f"the inherited HOME is not an absolute path: {home!r}")
    candidate = os.path.join(home, ".rustup")
    if not is_dir(candidate):
        raise ToolchainError(
            f"no usable toolchain home at {candidate}: the pinned {BUILDER} "
            "toolchain would be hidden behind the private fixture HOME"
        )
    return candidate


def toolchain_environment(toolchain_home: str) -> dict:
    """The toolchain facts shared by staging and every fixture child.

    Both call sites build their environment through this one function, so the
    absolute toolchain home, the pinned toolchain, the no-auto-install rule and
    offline Cargo cannot drift apart.
    """
    return {
        "RUSTUP_HOME": toolchain_home,
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_AUTO_INSTALL": "0",
        "CARGO_NET_OFFLINE": "true",
        # Frozen once and shared, so staging and every fixture child compile
        # with the same flags. `CARGO_ENCODED_RUSTFLAGS` outranks RUSTFLAGS in
        # Cargo, so an inherited encoded value is scrubbed rather than mixed.
        "RUSTFLAGS": FROZEN_RUSTFLAGS,
    }


def staging_environment(environ, toolchain_home: str) -> dict:
    """The staging child's environment: the caller's own, minus Cargo/target
    and wrapper overrides, plus the shared toolchain facts."""
    env = {
        key: value
        for key, value in environ.items()
        if not key.startswith(
            ("CARGO_TARGET_", "CARGO_BUILD_", "SCCACHE_", "OXIDE_", "XDG_")
        )
        and key
        not in (
            "CARGO",
            "RUSTC",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_ENCODED_RUSTFLAGS",
        )
    }
    env.update(toolchain_environment(toolchain_home))
    env.update(
        {
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
        }
    )
    return env


def fixture_environment(environ, toolchain_home: str, home: str, cargo_home: str) -> dict:
    """One fixture child's environment: private account and Cargo homes, the
    same absolute toolchain home the staging child used."""
    # Built fresh, never inherited: no ambient OXIDE_*/XDG_* control can leak
    # in. `OXIDE_TEST_HOME_FROM_ENV=1` is the debug build's account-home
    # control, so this private HOME is the account home and the real account's
    # config, pool record and status are untouched.
    env = {
        "PATH": environ.get("PATH", "/usr/bin:/bin"),
        "HOME": home,
        "CARGO_HOME": cargo_home,
        "OXIDE_TEST_HOME_FROM_ENV": "1",
    }
    env.update(toolchain_environment(toolchain_home))
    return env


def stage_subject(root: str, toolchain_home: str) -> str:
    """Stage this HEAD's debug `oxide` offline, then delete the target.

    The staging child gets the same absolute toolchain home as every fixture
    child, so the toolchain the proof stages with is the one its children use.
    """
    stage = os.path.join(root, "stage")
    log = os.path.join(root, "stage.log")
    env = staging_environment(os.environ, toolchain_home)
    env.update(
        {
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
            list(CARGO_BUILD_ARGV),
            cwd=REPO,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            timeout=work_left(),
        )
    if build.returncode:
        with open(log, encoding="utf-8") as handle:
            abort(f"staging exit {build.returncode}: {handle.read()[-2000:]}")
    built = os.path.join(stage, "target", "debug", "oxide")
    require(os.path.isfile(built), f"staging produced no binary at {built}")
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
    # Auto-adopt premise: a fresh checkout with no `target/`, so the first
    # build cannot mistake a converted or converted-ready root for fresh work.
    expect(
        not os.path.exists(os.path.join(crate, "target")),
        "the fixture checkout already has a target directory",
    )
    return crate


def manifest_identity(manifest_text: str) -> tuple:
    """The fixture manifest's own `[package]` name and version.

    The fixture manifest is this script's own text, and the lock precondition
    below is compared against it rather than against a copied constant.
    """
    name = re.search(r'^name\s*=\s*"([^"]+)"', manifest_text, re.MULTILINE)
    version = re.search(r'^version\s*=\s*"([^"]+)"', manifest_text, re.MULTILINE)
    expect(
        name is not None and version is not None,
        "the fixture manifest has no package name/version",
    )
    return name.group(1), version.group(1)


def lock_precondition(manifest_text: str, lock_text: str) -> tuple:
    """Whether a lock file is a valid, manifest-consistent precondition.

    A dependency-free fixture crate's lock must carry a lock version header and
    exactly the root package, with the manifest's name and version. A missing
    lock, a missing header, a different package, a different version or an
    extra package is not silently accepted: Cargo would either refuse the
    `--locked` call or rebuild against a state this proof did not establish.
    Returns `(ok, reason)` so the caller can say why.
    """
    name, version = manifest_identity(manifest_text)
    if not lock_text.strip():
        return False, "no lock file"
    header = re.search(r"^version\s*=\s*(\d+)\s*$", lock_text, re.MULTILINE)
    if header is None:
        return False, "no lock version header"
    if int(header.group(1)) < 3:
        return False, f"unsupported lock version {header.group(1)}"
    packages = re.findall(
        r'^\[\[package\]\]\s*$\nname\s*=\s*"([^"]+)"\s*$\nversion\s*=\s*"([^"]+)"',
        lock_text,
        re.MULTILINE,
    )
    if len(packages) != 1:
        return False, f"{len(packages)} package records for a dependency-free crate"
    if packages[0] != (name, version):
        return False, f"lock package {packages[0]} does not match manifest {(name, version)}"
    return True, "manifest-consistent lock"


def read_text(path: str) -> str:
    try:
        with open(path, encoding="utf-8") as handle:
            return handle.read()
    except OSError:
        return ""


def require_lock_precondition(crate: str) -> None:
    """Refuse unless the crate's lock is a valid precondition right now."""
    ok, reason = lock_precondition(
        read_text(os.path.join(crate, "Cargo.toml")),
        read_text(os.path.join(crate, "Cargo.lock")),
    )
    expect(ok, f"the fixture crate's lock precondition is not met: {reason}")


def ensure_fixture_lock(world: "World", crate: str) -> None:
    """Establish the lock precondition with the pinned Cargo, offline.

    This is a fixture-setup child of the future guarded proof, not an oracle:
    it runs in the fixture's private Cargo home with the pinned toolchain and
    `--offline`, on a dependency-free package, so it resolves nothing and
    touches no network. Generating the lock here rather than hand-writing one
    keeps its lockfile version the one this Cargo actually accepts, which a
    hand-written lock could contradict under `--locked`.
    """
    ok, reason = lock_precondition(
        read_text(os.path.join(crate, "Cargo.toml")),
        read_text(os.path.join(crate, "Cargo.lock")),
    )
    expect(
        not ok,
        f"the fresh fixture crate already carries a lock precondition: {reason}",
    )
    setup = subprocess.run(
        list(LOCK_ARGV),
        cwd=crate,
        env=world.env(),
        capture_output=True,
        text=True,
        timeout=work_left(),
    )
    require(
        setup.returncode == 0,
        f"the fixture lock setup failed: {setup.stderr[-2000:]}",
    )
    require_lock_precondition(crate)


def new_fixture(world: "World", name: str) -> str:
    """The one constructor every fresh fixture crate goes through.

    Crate files first, then the lock precondition, so no `--locked` invocation
    can reach a crate whose lock state was never established.
    """
    crate = make_fixture(world.root, name)
    ensure_fixture_lock(world, crate)
    return crate


class World:
    """The private account, pool, Cargo home and checkout of one proof run."""

    def __init__(self, root: str, subject: str, toolchain_home: str) -> None:
        self.root = root
        self.subject = subject
        self.toolchain_home = toolchain_home
        self.home = private_dir(os.path.join(root, "home"))
        self.cargo_home = private_dir(os.path.join(root, "cargo-home"))
        self.pool = private_dir(os.path.join(root, "pool"))
        self.scratch = private_dir(os.path.join(root, "fixture-scratch"))
        self.base = fixture_environment(
            os.environ, toolchain_home, self.home, self.cargo_home
        )
        self.base.update(
            {
                "TMPDIR": self.scratch,
                "TMP": self.scratch,
                "TEMP": self.scratch,
                "OXIDE_POOL": self.pool,
                "OXIDE_POOL_CAP": "64MiB",
            }
        )

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
    # The `--locked` oracle below requires the lock precondition to hold. It is
    # established by the fixture constructor, and re-checked here so a missing
    # or stale lock fails as a state fact rather than as Cargo's refusal.
    require_lock_precondition(crate)
    # Oracle: plain Cargo's build script reports the inherited directory, so
    # the observation method really sees the child environment.
    plain_env = world.env()
    plain_env.pop("OXIDE_POOL", None)
    plain = subprocess.run(
        list(CARGO_ORACLE_ARGV),
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
        require(
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
    results = tested.stdout + tested.stderr
    expect(
        "test result: ok" in results and "0 failed" in results,
        f"oxide test did not pass: {results[-1500:]}",
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
        require(
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
            abort("the released run program did not exit after its release")
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
    require(created.returncode == 0, f"image creation failed: {created.stderr}")
    # The mount-root rule makes the drive root owner-only, so the owned image
    # is attached with ownership enabled, exactly as the accepted account-home
    # proof does, and that fact is confirmed before registration.
    attached = subprocess.run(
        ["/usr/bin/hdiutil", "attach", "-nobrowse", "-owners", "on", "-plist", image],
        capture_output=True,
        timeout=work_left(),
    )
    require(attached.returncode == 0, f"image attach failed: {attached.stderr!r}")
    mount = ""
    try:
        try:
            entities = plistlib.loads(attached.stdout)["system-entities"]
            mounts = [entity["mount-point"] for entity in entities if entity.get("mount-point")]
        except (ValueError, KeyError, TypeError) as error:
            abort(f"the attach output is unclassifiable: {error}")
        require(
            len(mounts) == 1,
            f"expected one mounted volume for the owned image, got {mounts}",
        )
        mount = mounts[0]
        require(
            mount.startswith("/Volumes/"),
            f"the owned image did not mount under /Volumes: {mount}",
        )
        ownership = subprocess.run(
            ["/usr/sbin/diskutil", "info", "-plist", mount],
            capture_output=True,
            timeout=work_left(),
        )
        require(ownership.returncode == 0, "diskutil info failed for the mount")
        try:
            enabled = plistlib.loads(ownership.stdout).get("GlobalPermissionsEnabled")
        except ValueError as error:
            abort(f"the diskutil output is unclassifiable: {error}")
        require(
            enabled is True,
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
        require(
            detached.returncode == 0, f"ordinary detach failed: {detached.stderr}"
        )
        require(not os.path.exists(mount), f"the mount point {mount} was recreated")
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


def self_check() -> int:
    """Pure regression of the fixture environment construction.

    No filesystem, no scratch, no child process and no toolchain: the toolchain
    home decision takes an injected `is_dir`, and the two environment builders
    are pure functions. The negative parent reproduces the defect the first
    authorised author native hit — a private fixture HOME with no explicit
    toolchain home — and the property must reject it.
    """
    existing = {"/real/rustup", "/home/tom/.rustup", "/opt/toolchains/1.97.1"}
    is_dir = existing.__contains__
    checks: list[str] = []

    def property_true(name: str, condition: bool) -> None:
        expect(bool(condition), f"self-check property failed: {name}")
        checks.append(name)

    # 1. An explicit, absolute, usable inherited RUSTUP_HOME is honoured as is.
    inherited = resolve_toolchain_home(
        {"RUSTUP_HOME": "/opt/toolchains/1.97.1", "HOME": "/home/tom"}, is_dir
    )
    property_true("inherited RUSTUP_HOME honoured", inherited == "/opt/toolchains/1.97.1")

    # 2. Unset RUSTUP_HOME: the account default resolves from the ORIGINAL
    #    HOME, before any private fixture HOME is involved.
    resolved = resolve_toolchain_home({"HOME": "/home/tom"}, is_dir)
    property_true("account default from original HOME", resolved == "/home/tom/.rustup")

    # 3. The staging and fixture environments carry the same absolute path, and
    #    the fixture HOME/CARGO_HOME stay private and different from it.
    staging = staging_environment(
        {"PATH": "/usr/bin", "HOME": "/home/tom", "CARGO_HOME": "/home/tom/.cargo"},
        resolved,
    )
    fixture = fixture_environment(
        {"PATH": "/usr/bin", "HOME": "/home/tom"},
        resolved,
        "/scratch/fixture/home",
        "/scratch/fixture/cargo-home",
    )
    for env in (staging, fixture):
        property_true(
            "absolute toolchain home in every child", env["RUSTUP_HOME"] == resolved
        )
        property_true(
            "toolchain home never the fixture HOME",
            env["RUSTUP_HOME"] != env.get("HOME")
            and not env["RUSTUP_HOME"].startswith("/scratch/fixture/"),
        )
        property_true("pinned toolchain", env["RUSTUP_TOOLCHAIN"] == BUILDER == "1.97.1")
        property_true("no auto-install", env["RUSTUP_AUTO_INSTALL"] == "0")
        property_true("offline Cargo", env["CARGO_NET_OFFLINE"] == "true")
    property_true(
        "staging and fixtures agree on the toolchain facts",
        toolchain_environment(resolved)
        == {name: staging[name] for name in toolchain_environment(resolved)}
        == {name: fixture[name] for name in toolchain_environment(resolved)},
    )
    property_true(
        "debug account-home control for the private fixture HOME",
        fixture["OXIDE_TEST_HOME_FROM_ENV"] == "1",
    )
    property_true("private fixture HOME retained", fixture["HOME"] == "/scratch/fixture/home")
    property_true(
        "private fixture CARGO_HOME retained",
        fixture["CARGO_HOME"] == "/scratch/fixture/cargo-home",
    )

    # 3b. Ambient project/override controls never reach a child: the fixture
    #     environment is built fresh, and the staging environment scrubs
    #     OXIDE_*/XDG_*/CARGO_ENCODED_RUSTFLAGS and freezes RUSTFLAGS once.
    hostile = {
        "PATH": "/usr/bin",
        "HOME": "/home/tom",
        "RUSTUP_HOME": "/home/tom/.rustup",
        "RUSTFLAGS": "-C target-cpu=native",
        "CARGO_ENCODED_RUSTFLAGS": "-Ctarget-cpu=native",
        "OXIDE_POOL": "/Volumes/Builds/oxide-pool",
        "OXIDE_CONFIG": "/home/tom/.config/oxide/config.toml",
        "OXIDE_TEST_BUDGET_BYTES": "1",
        "OXIDE_TEST_WALK_LOG": "/tmp/walk",
        "XDG_CONFIG_HOME": "/home/tom/.config",
        "XDG_CACHE_HOME": "/home/tom/.cache",
    }
    scrubbed_fixture = fixture_environment(
        hostile, "/home/tom/.rustup", "/scratch/fixture/home", "/scratch/fixture/cargo-home"
    )
    scrubbed_staging = staging_environment(hostile, "/home/tom/.rustup")
    for label, env in (("fixture", scrubbed_fixture), ("staging", scrubbed_staging)):
        property_true(
            f"{label} env imports no ambient OXIDE_/XDG_ control",
            not [key for key in env if key.startswith(("OXIDE_", "XDG_"))]
            or label == "fixture"
            and set(key for key in env if key.startswith("OXIDE_"))
            == {"OXIDE_TEST_HOME_FROM_ENV"},
        )
        property_true(
            f"{label} env has no encoded rustflags",
            "CARGO_ENCODED_RUSTFLAGS" not in env,
        )
        property_true(
            f"{label} env freezes RUSTFLAGS",
            env["RUSTFLAGS"] == FROZEN_RUSTFLAGS == "-C strip=symbols",
        )
    property_true(
        "explicit fixture controls are the only OXIDE_* the fixture env carries",
        {key for key in scrubbed_fixture if key.startswith("OXIDE_")}
        == {"OXIDE_TEST_HOME_FROM_ENV"},
    )

    # 3c. Every external Cargo argv keeps the subcommand name.
    for argv in (CARGO_BUILD_ARGV, CARGO_ORACLE_ARGV):
        property_true(
            f"cargo argv names its subcommand: {argv[0]} {argv[1]}",
            argv[0] == "cargo" and argv[1] in {"build", "check"} and len(argv) > 2,
        )

    # 3d. The fixture lock is a real state precondition, not just an argv
    #     shape: a manifest-consistent lock passes; the pre-fix constructor
    #     state (no lock), a missing header, a different package/version and an
    #     undeclared extra package are all refused rather than treated as valid.
    valid_lock = (
        "# This file is automatically @generated by Cargo.\n"
        "version = 4\n\n"
        '[[package]]\nname = "tempcheck"\nversion = "0.1.0"\n'
    )
    ok, _reason = lock_precondition(MANIFEST, valid_lock)
    property_true("manifest-consistent lock is a valid precondition", ok)
    for label, text in (
        ("missing lock", ""),
        ("no lock version header", '[[package]]\nname = "tempcheck"\nversion = "0.1.0"\n'),
        ("mismatched package name", valid_lock.replace('"tempcheck"', '"other"')),
        ("mismatched version", valid_lock.replace('"0.1.0"', '"9.9.9"')),
        (
            "undeclared extra package",
            valid_lock + '\n[[package]]\nname = "dep"\nversion = "1.0.0"\n',
        ),
    ):
        ok, _reason = lock_precondition(MANIFEST, text)
        property_true(f"refuses {label}", not ok)
    property_true(
        "negative parent breaks the lock precondition",
        not lock_precondition(MANIFEST, "")[0],
    )

    # 3e. The diagnostic driver's semantics, with fake cases only: default mode
    #     stops at the first failure; diagnostic mode records it, skips only
    #     cases that declare a dependency on it, still runs independent cases,
    #     skips every later case after a shared-state mutation failed, aborts on
    #     an unknown outcome in both modes, and its receipt is bounded and
    #     explicitly unqualified.
    ran = []

    def body(tag, action=None):
        def run():
            ran.append(tag)
            if action is not None:
                action()
        return run

    def fail_body(tag, message="declared mismatch"):
        def run():
            ran.append(tag)
            fail(message)
        return run

    def unknown_body(tag):
        def run():
            ran.append(tag)
            abort("unknown child state")
        return run

    def scenarios(diagnostic: bool, cases):
        saved = MODE["diagnostic"]
        MODE["diagnostic"] = diagnostic
        ran.clear()
        try:
            # The expected-failure scenarios keep the proof's own stderr clean.
            with contextlib.redirect_stderr(io.StringIO()):
                status = drive(cases, "/nonexistent", receipt=False)
        finally:
            MODE["diagnostic"] = saved
        return status, list(ran)

    failfast_cases = [
        ProofCase(0, body("a"), label="a"),
        ProofCase(1, fail_body("b"), depends=(0,), label="b"),
        ProofCase(2, body("c"), depends=(0,), label="c"),
    ]
    status, ran_cases = scenarios(False, failfast_cases)
    property_true(
        "default mode stops at the first failure", status == 1 and ran_cases == ["a", "b"]
    )

    diagnostic_cases = [
        ProofCase(0, body("a"), label="a"),
        ProofCase(1, fail_body("b"), label="b"),
        ProofCase(2, body("c"), depends=(1,), label="c"),
        ProofCase(3, body("d"), depends=(0,), label="d"),
    ]
    status, ran_cases = scenarios(True, diagnostic_cases)
    property_true(
        "diagnostic records a failure and still runs independent cases",
        status == 1 and ran_cases == ["a", "b", "d"],
    )

    mutating_cases = [
        ProofCase(0, fail_body("a"), mutates=True, label="a"),
        ProofCase(1, body("b"), label="b"),
        ProofCase(2, body("c"), label="c"),
    ]
    status, ran_cases = scenarios(True, mutating_cases)
    property_true(
        "diagnostic skips every later case after a shared-state failure",
        status == 1 and ran_cases == ["a"],
    )

    unknown_cases = [
        ProofCase(0, body("a"), label="a"),
        ProofCase(1, unknown_body("b"), label="b"),
        ProofCase(2, body("c"), label="c"),
    ]
    for diagnostic in (False, True):
        status, ran_cases = scenarios(diagnostic, unknown_cases)
        property_true(
            f"unknown outcome aborts in {'diagnostic' if diagnostic else 'default'} mode",
            status == 1 and ran_cases == ["a", "b"],
        )

    outcomes = [
        (ProofCase(0, body("a"), label="a"), "pass", ""),
        (ProofCase(1, body("b"), label="b"), "fail", "declared mismatch"),
        (ProofCase(2, body("c"), depends=(1,), label="c"), "skipped", "depends on 2"),
    ]
    record = diagnostic_record(
        outcomes, [("b", "declared mismatch")], [("c", "depends on 2")], False
    )
    property_true(
        "diagnostic receipt is explicitly unqualified",
        '"qualified": false' in record and "diagnostic-only" in record,
    )
    property_true(
        "diagnostic receipt records the first fault and the skip",
        '"first_fault"' in record and "declared mismatch" in record and "depends on 2" in record,
    )
    property_true(
        "diagnostic receipt never claims a pass",
        '"passed"' not in record and "qualified: true" not in record,
    )
    many = [(ProofCase(i, body(str(i)), label=str(i)), "fail", "x" * 900) for i in range(40)]
    bounded = diagnostic_record(many, [("0", "x" * 900)] * 40, [], False)
    property_true(
        "diagnostic receipt is bounded",
        len(bounded.encode()) <= MAX_DIAGNOSTIC_RECEIPT_BYTES
        and json.loads(bounded)["faults_recorded"] == MAX_DIAGNOSTIC_FAULTS,
    )

    # 4. Missing or unusable provenance refuses instead of hiding the toolchain
    #    behind a fake HOME, and it refuses for every shape.
    for label, environ in (
        ("no RUSTUP_HOME and no HOME", {}),
        ("relative RUSTUP_HOME", {"RUSTUP_HOME": "rustup", "HOME": "/home/tom"}),
        (
            "missing RUSTUP_HOME directory",
            {"RUSTUP_HOME": "/opt/toolchains/absent", "HOME": "/home/tom"},
        ),
        ("relative HOME", {"HOME": "home/tom"}),
        ("missing account toolchain", {"HOME": "/home/absent-home"}),
    ):
        try:
            resolve_toolchain_home(environ, is_dir)
        except ToolchainError:
            checks.append(f"refuses {label}")
        else:
            fail(f"self-check property failed: {label} must refuse")

    # 5. Negative parent: the pre-fix environment (private fixture HOME, no
    #    explicit toolchain home) must violate the preservation property, so
    #    the property above is not vacuous.
    regressed = {
        "HOME": "/scratch/fixture/home",
        "CARGO_HOME": "/scratch/fixture/cargo-home",
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_AUTO_INSTALL": "0",
        "CARGO_NET_OFFLINE": "true",
    }
    preserved = (
        regressed.get("RUSTUP_HOME") == resolved
        and not regressed["HOME"].startswith("/scratch/fixture/")
    )
    property_true("negative parent breaks preservation", not preserved)

    print(f"self-check ok: {len(checks)} properties, pure (no scratch, child or toolchain)")
    for name in checks:
        print(f"  ok {name}")
    return 0


def release_removed(world: "World") -> None:
    """Case 7: a released slot keeps no owned directory."""
    case(CASE_NAMES[6])
    expect(
        not any(os.path.exists(path) for path in world.scratch_dirs()),
        "a released slot kept an owned directory",
    )


def proof_cases(world: "World", crate: str) -> list:
    """The twelve default cases with their declared dependencies.

    `mutates` marks a case that changes shared state (the pool's slots, the
    removable registration or the fixture checkout). A diagnostic failure in
    such a case skips every later case, because that shared state is no longer
    positively known; a failure in a read-only case skips only its declared
    dependents.
    """
    return [
        ProofCase(0, lambda: premise_sweep(world, crate)),
        ProofCase(1, lambda: inherited_kept(world, crate), depends=(0,)),
        ProofCase(2, lambda: replaced(world, crate), depends=(0,), mutates=True),
        ProofCase(3, lambda: relative_inherited(world, crate), depends=(0,), mutates=True),
        ProofCase(4, lambda: aliases(world, crate), depends=(0,), mutates=True),
        ProofCase(5, lambda: accounting(world, crate), depends=(0,), mutates=True),
        ProofCase(6, lambda: release_removed(world), depends=(5,)),
        ProofCase(7, lambda: crash_and_recovery(world, crate), depends=(0,), mutates=True),
        ProofCase(8, lambda: spawns(world, crate), depends=(0,), mutates=True),
        ProofCase(9, lambda: refusals(world, crate), depends=(0,), mutates=True),
        ProofCase(10, lambda: removable(world, crate), depends=(0,), mutates=True),
        ProofCase(11, lambda: path_bound(world, crate), depends=(0, 10), mutates=True),
    ]


def main(argv: list[str]) -> int:
    try:
        return run_session(argv)
    except Recoverable as error:
        print(f"pool-temp-scratch proof failed: {error}", file=sys.stderr)
        return 1
    except UnknownOutcome as error:
        print(
            f"pool-temp-scratch proof stopped on an unknown outcome: {error}",
            file=sys.stderr,
        )
        return 1


def run_session(argv: list[str]) -> int:
    if argv and argv[0] == "--self-check":
        # Pure, before any scratch, toolchain subprocess or staging.
        return self_check()
    if argv and argv[0] == "--plan":
        plan()
        return 0
    diagnostic = bool(argv) and argv[0] == "--diagnostic"
    if diagnostic and len(argv) != 1:
        fail(f"unknown argument {argv[1]!r}; --diagnostic takes no argument")
    if argv and not diagnostic:
        fail(f"unknown argument {argv[0]!r}; use --plan, --self-check or --diagnostic")
    if diagnostic:
        # Same qualified guard, same bounds: the supervisor's deadline and
        # scratch root are required exactly as in the default proof, and the
        # mode only changes failure handling, never custody or cleanup.
        require(
            os.environ.get("OXIDE_PROOF_DEADLINE_NS") is not None,
            "the diagnostic mode runs only under the qualified supervisor "
            "(OXIDE_PROOF_DEADLINE_NS is absent)",
        )
        MODE["diagnostic"] = True
        say("diagnostic mode: record recoverable oracle failures, abort on unknown outcomes")
    # The toolchain provenance is decided from the caller's real account before
    # any private fixture HOME exists; an unusable one refuses here, before
    # scratch or staging effects.
    try:
        toolchain_home = resolve_toolchain_home(os.environ)
    except ToolchainError as error:
        fail(f"toolchain provenance refused before staging: {error}")
    root = scratch_root()
    world_root = private_dir(os.path.join(root, "pool-temp"))
    say(f"scratch root {root}")
    say(f"toolchain home {toolchain_home} (pinned {BUILDER}, no auto-install)")
    subject = stage_subject(world_root, toolchain_home)
    say(f"staged {subject} sha256={sha256_file(subject)}")
    world = World(world_root, subject, toolchain_home)
    crate = new_fixture(world, "tempcheck")
    status = drive(proof_cases(world, crate), root)
    if status:
        return status
    print(
        f"pool-temp-scratch proof passed: {len(CASES)} cases, "
        f"{time.perf_counter() - STARTED:.1f}s"
    )
    for name in CASES:
        print(f"  pass {name}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
