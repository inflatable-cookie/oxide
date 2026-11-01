#!/usr/bin/env python3
"""Focused proof for the bounded Finder-metadata race in owned-tree deletion
(oxide#155).

A short fixed script and the whole command of the qualified shared proof
supervisor:

    effigy rust:finder-metadata-race
    effigy rust:finder-metadata-race:diagnostic
    effigy rust:finder-metadata-race:test

whose recipes are `python3 scripts/bounded-test.py --limit 420
--scratch-budget-mib 512 -- python3 scripts/test-finder-metadata-race.py
[--diagnostic]`. Admission, the wall-clock limit, the host reserve, the owned
scratch bound, child sessions and cleanup belong to the supervisor; this script
starts no session, signals no group, and keeps no clock, reserve reading or
scratch of its own. Fixture scratch is written only inside
`OXIDE_TASK_SCRATCH_ROOT`; image backings and their custody records only inside
`OXIDE_PROOF_EVIDENCE`.

Every fixture pool here is a genuine `directory-budget-v1` pool, so each case
declares one private `OXIDE_SCRATCH_ROOT` inside its own run root and keeps its
`OXIDE_POOL` strictly inside that root with an explicit `OXIDE_POOL_CAP`
(oxide#173 scratch pools, slice S4). On the supervisor's selected volume that
is a scratch pool on removable media; on a `/private/tmp` fallback the
declaration is inert and the same recipe makes an ordinary internal pool there,
which is therefore not scratch-pool coverage and is reported as such.

Stages this HEAD's debug `oxide` offline with the pinned 1.97.1 toolchain
through plain Cargo and covers both callers. The **lease** half runs the
post-oxide#190 holding model: an eviction renames the whole idle slot tree into
`<pool>/payload/.oxide-holding/<entry>` under the pool lock, records one holding
row, and deletes the tree outside the lock (`src/lease/pool.rs:112`,
`holding_root`), so a refused item stays recorded and charged until a later
command finishes it. The **prune** half still drives the debug-only race
injection (`OXIDE_TEST_FINDER_RACE`, absent from release builds, armed only
under the supervisor's exported layout) at the boundary the product documents —
after the caller's listing, before the removal — because that hook is reached
only from that drain (`src/owned_delete.rs:247`, `src/bin/oxide/prune.rs:4076`):

- `lease_control`: two admissions under a 1 MiB budget; the second evicts the
  first project's idle whole slot with no content planted. The slot *leaf* is
  gone, the holding row and its tree are gone, and `status` reports no held or
  pending deletion. Fixture premise.
- `lease_metadata`: the same eviction with real `.DS_Store` and `._*` files
  planted at the tree root and in a nested directory. They go with the held
  tree and never refuse the deletion: it completes and leaves no row, no tree
  and no held line in `status`.
- `lease_foreign`: the same eviction with one ordinary foreign file in the
  tree. The holding drain deletes a regular file by name
  (`src/lease/pool.rs:5552`), so the tree and the file go together and the
  deletion completes with nothing left charged.
- `lease_link`: the same eviction with a `.DS_Store`-named symbolic link to an
  owned sentinel directory *outside* the pool. The drain unlinks the link
  itself and never follows it (`unlinkat(..., 0)` at `src/lease/pool.rs:5557`),
  so the deletion completes and the sentinel and its content are untouched.
- `lease_exhaust`: the same eviction with an entry the drain cannot classify (a
  FIFO) in the tree. The drain refuses it by name
  (`src/lease/pool.rs:5566`, "an unknown entry type"), so the tree stays under
  `.oxide-holding` with its row and its recorded charge, the command reports
  that it stays recorded and charged (`src/bin/oxide/main.rs:1931`) and exits
  zero, and `status` lists it as held for deletion with the tree still present
  (`src/bin/oxide/main.rs:2444`). The fixture then removes its own FIFO and the
  next command finishes the held deletion, dropping both the tree and the row.
- `prune_metadata`: one owned 128 MiB APFS image, registered as the permanent
  outputs drive, with one real private Git checkout on it (ignored `target`)
  whose first build is asserted to have adopted an owned root into a real
  `target` directory, with the committed member links the pool-owned registry
  (`<pool>/oxide-managed/outputs/registry.json`) names for that root.
  The checkout is then removed and `oxide outputs prune` runs with exactly its
  preview plan: the deletion completes, no quarantined leaf is left, the live
  registry keeps no intent for that root and no row selects it.
- `prune_foreign`: the same, raced with an ordinary file. The run exits 3, the
  `.prune-<intent>` leaf is kept with the file untouched, and the registry's
  intent for that quarantine is in a non-terminal phase with
  `receipt_settled: false` and no `deleted`/`resolved` terminal state.
- `prune_exhaust`: the same, raced with recreation on every attempt: exit 3 and
  the leaf kept, with the same unfinished journal assertions.
- `prune_missing_quarantine`: a pending intent whose quarantine leaf is then
  removed by an "unknown writer" and replayed: the run exits 3, never reports
  completion, and the intent stays unfinished.
- `prune_replaced_quarantine`: a pending intent whose quarantine leaf is
  replaced by a different directory (same name, new inode): the replay exits 3,
  the replacement and its content are untouched, and the intent stays
  unfinished.
- `prune_drain_fault`: tolerated metadata is recreated and the product's
  debug-only low-reserve reading fires on the first check *after one recovered
  entry was unlinked*, so the per-unlink check stops the drain mid-way: exit 3,
  the `.prune-<intent>` leaf kept with its remaining metadata, the intent
  non-terminal and `receipt_settled: false`.
- `prune_completion_fault`: the product's terminal crash point fires after the
  quarantine leaf is removed and the registry terminal write lands but before
  the checkout receipt settles: exit 3 with `deleted` yet `receipt_settled:
  false` and no leaf, and the receipt-step replay then settles it exactly.

Limits, stated rather than implied: `oxide clean` is deliberately unchanged (it
removes exactly its enumerated plan, so new content is left and counted) and has
no case here. Two pre-#190 cases are retired by Oxide Supervisor decision
`6818efde` and are not declared, run or counted: `lease_unlinked_file`
(oxide#190 removed the per-entry capture journal and its
`OXIDE_TEST_CAPTURE_UNLINK_STOP` seam) and `lease_old_counterfactual` (the
pinned pre-#190 base binary and its legacy journal replay are no longer required
here; `rust:holding-delete` and the #190 units cover that ground). No run of
this script has passed: its fit is UNMEASURED. The uncounted diagnostic and
counted author native each need owner activation, and the independent reviewer
native was waived for oxide#175 only.

Diagnostic safety (`--diagnostic`): only a *recoverable oracle* failure is
recorded and followed by the next independent case. An unknown writer, device,
custody, detach or deadline outcome aborts the whole collection immediately,
retains every image backing and its custody record, and writes a bounded
`qualified: false` receipt. The receipt covers setup and staging failures too
(a staging timeout, a setup `OSError` or any other non-classified setup
exception is unknown closure, and an unwritable receipt aborts explicitly),
records the first fault and a per-case reason, and lists reached, completed,
failed, skipped and unexecuted cases. Unknown journal or receipt state (an
absent, unreadable, incomplete or malformed pool ledger or holding row, an
absent or unreadable holding area, an absent, unreadable, incomplete or
malformed pool-owned registry, or an intent count other than exactly one for
the recorded quarantine) aborts; it is never counted as a recoverable oracle
failure. `subprocess` timeouts are unknown
closure, never a recoverable failure.

Nine-premise checklist (docs/knowledge/contracts/workflow.md
#proof-authoring-premise-checklist), traced to executable source:

1. private HOME and rustup: `INVOKING_RUSTUP_HOME` is captured before any HOME
   override and handed to staging and every fixture child with
   `RUSTUP_TOOLCHAIN=1.97.1` and `RUSTUP_AUTO_INSTALL=0`; fixture crates have
   no dependencies and every fixture Cargo call is unlocked, so no lock is
   generated for a `--locked` call; both offline staging calls are `--locked`
   and use the lock committed in their respective source trees.
2. account-home control: `OXIDE_TEST_HOME_FROM_ENV=1` in `case_env`.
3. environment isolation: `fixture_environment` drops every inherited
   `OXIDE_*`, `XDG_*`, `CARGO_*`, `GIT_*`, `RUSTUP_*`, `RUSTC*` and `RUSTFLAGS`
   before setting only the fixture's explicit values — including this case's
   own `OXIDE_SCRATCH_ROOT`, its pool strictly inside it and its explicit
   `OXIDE_POOL_CAP` — then re-adds the supervisor's own
   `OXIDE_TASK_SCRATCH_ROOT`, `OXIDE_PROOF_DEADLINE_NS` and
   `OXIDE_PROOF_EVIDENCE` (required for the hook guard) and, per prune case,
   its own `OXIDE_TEST_FINDER_RACE`. No lease case arms that injection: the
   eviction path has no such seam.
4. automatic adoption: `workspace()` makes a real private Git repository whose
   `.gitignore` covers `target`, so the first conversion is eligible; each
   prune case then asserts the adopted owned root, the checkout's member link
   into it and the committed registry row before any race or prune.
5. Cargo external argv: not applicable, no external subcommand is invoked.
6. owned images: `ImageCustody` writes a custody record inside the guard's
   evidence directory before its first effect, creates the backing there,
   attaches it with `hdiutil attach -owners on`, parses the raw plist and
   requires exactly one mounted entity whose mount point is this run's expected
   `/Volumes/<name>` and whose device entry is a block device matching the
   mount's `st_dev`. The image is created with `-volname <this run's unique
   name>` (hdiutil's default volume name would mount at `/Volumes/untitled`),
   an explicit `-mountpoint` is passed when that path already exists, and any
   other resulting mount point is unknown custody. That name is the short
   `o155-<slug>-<4 hex>`, because the product refuses a per-command temporary
   directory longer than its 91-byte Unix-socket bound and a prune checkout on
   the image cannot keep an inherited same-volume `TMPDIR`; the fixture asserts
   its worst-case path before any effect and the pure `--self-check` keeps the
   layouts the first diagnostic refused (92/102/103 bytes) as a counterfactual. Before detaching it revalidates that attachment (the mount
   is a real directory on that recorded device, the device entry is the same
   block device), and after detaching it accepts absence only from `ENOENT`:
   a failed metadata read is unknown, our device still mounted is unknown, and
   only a *different* volume at the same path is recorded as reuse. Detach is
   ordinary only, and the backing is removed only after the device entry is
   positively gone.
7. compiler flags: `RUSTFLAGS` is frozen for staging and every fixture child and
   `CARGO_ENCODED_RUSTFLAGS` is removed everywhere.
8. temporary storage: scratch and `TMPDIR`/`TMP`/`TEMP` come from the
   supervisor's owned scratch; each case declares its own `OXIDE_SCRATCH_ROOT`
   there and keeps its pool strictly inside it, so the pool, the temp root and
   the images all stay inside the monitored owned root.
9. macOS mktemp: no `mktemp` is used; each case makes its own directory with a
   random suffix under the owned scratch.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

from dataclasses import dataclass
import hashlib
import inspect
import json
import os
import plistlib
import secrets
import shutil
import stat
import subprocess
import time

MIB = 1024 * 1024
BUILDER = "1.97.1"
POOL_BUDGET = 1 * MIB
IMAGE_MIB = 128
RESERVE = "16MiB"
IMAGE_CAP = "64MiB"
# Tom's decision 138a8155 keeps the approved 30 s shutdown margin under the
# 420 s counted and diagnostic selectors: a running child is cut off with 30 s
# left for closure and cleanup, never the 25 s an earlier draft used.
SHUTDOWN_MARGIN_SECONDS = 30
MUTED = ("OXIDE_", "XDG_", "CARGO_", "GIT_", "RUSTUP_", "RUSTC", "RUSTFLAGS")
MANAGED_NAME = "oxide-managed"
REGISTRY_RELATIVE = ("outputs", "registry.json")
# The private holding area at the payload root (oxide#190): an evicted slot tree
# is renamed whole into it and deleted outside the pool lock.
HOLDING_NAME = ".oxide-holding"
# The product refuses a per-command temporary directory whose path is longer
# than its Unix-socket bound of 91 bytes.
SOCKET_PATH_BUDGET = 91
# One declared scratch root per case, and its pool strictly inside it (§S4).
SCRATCH_DIRNAME = "scratch-pools"
POOL_DIRNAME = "pool"
# Every prune checkout lives on the removable image, so the product cannot keep
# an inherited same-volume `TMPDIR` and instead makes an owned per-command
# directory inside the slot. Its longest path is
# `<mount>/oxide-pool/slots/<checkout label>/<profile>-<slot>/scratch`, where the
# label carries the per-slot 8-hex nonce and the leaf a small slot number. The
# volume name must therefore stay short: `o155-<slug>-<4 hex>`.
PRUNE_SLUGS = {
    "prune_metadata": "meta",
    "prune_foreign": "frgn",
    "prune_exhaust": "exh",
    "prune_missing_quarantine": "miss",
    "prune_replaced_quarantine": "repl",
    "prune_drain_fault": "dfault",
    "prune_completion_fault": "cflt",
}
# Debug-only product injections for the two fault cases: a low reserve reading
# on the first check after one metadata entry was unlinked (mid-drain), and the
# product's terminal crash point after the registry terminal write but before
# the checkout receipt settles (pre-completion).
CASE_ENV = {
    # Keyed on the bounded seam's *recovered Finder metadata* count, so the
    # reading cannot be satisfied by the ordinary deletions the pass makes.
    "prune_drain_fault": {"OXIDE_TEST_PRUNE_LOW_RESERVE_AFTER_RECOVERED": "1"},
    "prune_completion_fault": {"OXIDE_TEST_PRUNE_CRASH": "terminal"},
}
SLOT_FOLDER_HEX = "0" * 8
SLOT_LEAF_WORST = "quick-999"
# The exact layouts the first owner-activated diagnostic refused, with the byte
# counts the product reported (evidence `155-diagnostic1-7572541-owner`). They
# are the counterfactual the pure check must keep refusing.
OBSERVED_OVERLONG = (
    (
        "prune_metadata",
        "/Volumes/oxide155-prune_metadata-2d11c435/oxide-pool/slots/checkout-ed114324/quick-1/scratch",
        92,
    ),
    (
        "prune_missing_quarantine",
        "/Volumes/oxide155-prune_missing_quarantine-6dcacb61/oxide-pool/slots/checkout-2635d603/quick-1/scratch",
        102,
    ),
    (
        "prune_replaced_quarantine",
        "/Volumes/oxide155-prune_replaced_quarantine-38ec9486/oxide-pool/slots/checkout-11b186d0/quick-1/scratch",
        103,
    ),
)
TERMINAL_PHASES = {"deleted", "resolved"}
UNFINISHED_PHASES = {"intent", "renamed", "deleting", "marker_last"}
CRATE = {
    "Cargo.toml": '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n',
    "src/lib.rs": "pub fn value() -> u32 {\n    7\n}\n",
    # The slot's recorded figure has to exceed the pool budget so the second
    # admission evicts it; a build script writing a few MiB makes that
    # independent of how a one-file crate is sized.
    "build.rs": (
        "use std::io::Write;\n"
        "fn main() {\n"
        "    println!(\"cargo:rerun-if-changed=build.rs\");\n"
        "    let out = std::env::var(\"OUT_DIR\").expect(\"OUT_DIR\");\n"
        "    let path = std::path::Path::new(&out).join(\"filler\");\n"
        "    let mut file = std::fs::File::create(path).expect(\"filler\");\n"
        "    file.write_all(&vec![0u8; 3 * 1024 * 1024]).expect(\"write\");\n"
        "}\n"
    ),
}
# The first conversion publishes owned member links, so the checkout has to be
# a real Git repository whose ignore rules cover them.
GITIGNORE = "/target\n"
# The lease half runs the post-oxide#190 holding model and the prune half drives
# the Finder-race drain; the two are separate case sets.
LEASE_CASES = (
    "lease_control",
    "lease_metadata",
    "lease_foreign",
    "lease_link",
    "lease_exhaust",
)
CASES = (
    *LEASE_CASES,
    "prune_metadata",
    "prune_foreign",
    "prune_exhaust",
    "prune_missing_quarantine",
    "prune_replaced_quarantine",
    "prune_drain_fault",
    "prune_completion_fault",
)
# Cases retired by Oxide Supervisor decision 6818efde. They are declared
# nowhere: `--self-check` refuses the proof if either name reappears in the case
# table, the race map or a dispatch branch.
RETIRED_CASES = ("lease_unlinked_file", "lease_old_counterfactual")
# The race each prune case arms: `repeat` recreates on every attempt, so the
# fixed bound is exercised; `None` is the unraced premise control. No lease
# case arms it: oxide#190's eviction has no race seam (the debug hook is reached
# only from the outputs-prune drain, src/owned_delete.rs:247), so the lease half
# plants its content instead and proves the holding model's own outcomes.
RACES = {
    "lease_control": None,
    "lease_metadata": None,
    "lease_foreign": None,
    "lease_link": None,
    "lease_exhaust": None,
    "prune_metadata": "*:metadata",
    "prune_foreign": "*:foreign",
    "prune_exhaust": "*:repeat",
    "prune_missing_quarantine": "*:foreign",
    "prune_replaced_quarantine": "*:foreign",
    "prune_drain_fault": "*:metadata",
    "prune_completion_fault": "*:metadata",
}
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
INVOKING_RUSTUP_HOME = os.environ.get("RUSTUP_HOME") or os.path.join(
    os.environ.get("HOME", ""), ".rustup"
)


class Recoverable(Exception):
    """An oracle failed. Only an independent case may follow."""


class Unknown(Exception):
    """Custody, device, writer or deadline uncertainty: abort, retain, report."""


def fail(message: str) -> None:
    raise Recoverable(message)


def unknown(message: str) -> None:
    raise Unknown(message)


def scratch_root() -> str:
    root = os.environ.get("OXIDE_TASK_SCRATCH_ROOT")
    if not root:
        unknown("OXIDE_TASK_SCRATCH_ROOT is missing; run this through effigy")
    root = os.path.abspath(root)
    if not os.path.isdir(root):
        unknown(f"owned scratch {root} is not a directory")
    return root


def evidence_dir() -> str:
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE")
    if not evidence:
        unknown("OXIDE_PROOF_EVIDENCE is missing; run this through effigy")
    evidence = os.path.abspath(evidence)
    if not os.path.isdir(evidence):
        unknown(f"evidence {evidence} is not a directory")
    facts = os.stat(evidence)
    if facts.st_uid != os.getuid() or facts.st_mode & 0o022:
        unknown(f"evidence {evidence} is not an owned, non-writable directory")
    return evidence


def window_left(deadline_ns: int, now_ns: int) -> float:
    """The supervisor's remaining work window minus the approved margin.

    Pure so the self-check can assert the cutoff arithmetic: a 420 s session
    leaves 390 s of work (420 - 30), not the 395 s a 25 s margin would give.
    """
    return (deadline_ns - now_ns) / 1e9 - SHUTDOWN_MARGIN_SECONDS


def work_left() -> float:
    """What is left of the supervisor's work window.

    `OXIDE_PROOF_DEADLINE_NS` is in the supervisor's own `CLOCK_UPTIME_RAW`
    domain, not wall time, so the remaining window is measured with the same
    clock the guard used. A spent window is unknown closure, not a failure.
    """
    deadline = int(os.environ.get("OXIDE_PROOF_DEADLINE_NS", "0") or 0)
    if deadline <= 0:
        unknown("OXIDE_PROOF_DEADLINE_NS is missing; run this through effigy")
    left = window_left(deadline, time.clock_gettime_ns(time.CLOCK_UPTIME_RAW))
    if left <= 5:
        unknown("the supervisor's work window is spent")
    return left


def toolchain_env() -> dict:
    """The pinned, offline toolchain and frozen compiler flags."""
    return {
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_HOME": INVOKING_RUSTUP_HOME,
        "RUSTUP_AUTO_INSTALL": "0",
        "CARGO_NET_OFFLINE": "true",
        "RUSTFLAGS": "-C strip=symbols",
    }


def scrubbed(env_source: dict) -> dict:
    return {
        key: value
        for key, value in env_source.items()
        if not key.startswith(MUTED) and key not in ("RUSTFLAGS", "RUSTUP_TOOLCHAIN")
    }


def run(command: list, *, cwd: str, env: dict, timeout: float = 120.0):
    bound = min(timeout, work_left())
    try:
        return subprocess.run(
            command,
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
            timeout=bound,
        )
    except subprocess.TimeoutExpired:
        # A child that outlived its bound has unknown closure: never continue.
        unknown(f"child {command[0]} timed out after {bound:.0f}s; closure is unknown")


def prune_volume_name(case: str) -> str:
    """This case's short, unique APFS volume name."""
    slug = PRUNE_SLUGS.get(case)
    if slug is None:
        fail(f"{case}: no prune slug is declared")
    return f"o155-{slug}-{secrets.token_hex(2)}"


def prune_temp_budget(volume: str) -> tuple[str, int]:
    """The longest per-command temporary path this fixture can produce."""
    path = os.path.join(
        f"/Volumes/{volume}",
        "oxide-pool",
        "slots",
        "checkout-" + SLOT_FOLDER_HEX,
        SLOT_LEAF_WORST,
        "scratch",
    )
    return path, len(path.encode())


def assert_prune_path_budget(case: str, volume: str) -> None:
    """Refuse an overlong fixture layout before it has any effect.

    A build on an over-budget layout would fail deep inside the product with
    its own socket-bound refusal, which this proof must not confuse with a
    product finding: the fixture layout is the proof's own premise.
    """
    path, size = prune_temp_budget(volume)
    if size > SOCKET_PATH_BUDGET:
        fail(
            f"{case}: the fixture's per-command temporary path would be {size} bytes "
            f"({path}), over the product's {SOCKET_PATH_BUDGET}-byte Unix-socket bound; "
            "the fixture layout is refused before any effect"
        )


def lstat_state(path: str) -> tuple[str, object]:
    """`present`, `absent` (ENOENT: certain) or `error` (metadata unknown).

    `os.path.exists` collapses every error into "absent", which would let a
    failed metadata read pass for positive absence. This never does.
    """
    try:
        return "present", os.lstat(path)
    except FileNotFoundError:
        return "absent", None
    except OSError as error:
        return "error", error


def git(env: dict, cwd: str, *args: str):
    return run(
        [
            "git",
            "-c",
            "user.email=proof@oxide.invalid",
            "-c",
            "user.name=oxide proof",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
            *args,
        ],
        cwd=cwd,
        env=env,
    )


def stage(root: str, source: str, label: str) -> str:
    stage_dir = os.path.join(root, f"stage-{label}")
    env = scrubbed(os.environ)
    env.update(toolchain_env())
    env.update(
        {
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(stage_dir, "target"),
            "CARGO_BUILD_BUILD_DIR": os.path.join(stage_dir, "build"),
            "CARGO_INCREMENTAL": "0",
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_PROFILE_DEV_DEBUG_ASSERTIONS": "true",
            "CARGO_PROFILE_DEV_OPT_LEVEL": "s",
            "CARGO_PROFILE_DEV_CODEGEN_UNITS": "16",
        }
    )
    log = os.path.join(root, f"stage-{label}.log")
    try:
        handle = open(log, "w", encoding="utf-8")
    except OSError as error:
        unknown(f"cannot open the staging log {log}: {error}")
    with handle:
        try:
            build = subprocess.run(
                ["cargo", "build", "-j", "4", "--locked", "--offline", "--bin", "oxide"],
                cwd=source,
                env=env,
                stdout=handle,
                stderr=subprocess.STDOUT,
                timeout=work_left(),
            )
        except subprocess.TimeoutExpired:
            # A staging child that outlived the work window has unknown
            # closure: never a recoverable failure.
            unknown("staging cargo timed out; its child closure is unknown")
        except OSError as error:
            unknown(f"cannot run the staging cargo: {error}")
    if build.returncode:
        try:
            with open(log, encoding="utf-8") as handle:
                text = handle.read()[-2000:]
        except OSError as error:
            unknown(f"cannot read the staging log {log}: {error}")
        # A staging build that simply fails is an ordinary (recoverable)
        # failure; only an unreadable log or a filesystem fault is unknown.
        fail(f"staging exit {build.returncode}: {text}")
    built = os.path.join(stage_dir, "target", "debug", "oxide")
    if not os.path.isfile(built):
        fail(f"staging produced no binary at {built}")
    subject = os.path.join(root, f"oxide-{label}")
    try:
        shutil.copy2(built, subject)
        os.chmod(subject, 0o755)
        shutil.rmtree(stage_dir, ignore_errors=True)
        os.unlink(log)
    except OSError as error:
        unknown(f"cannot publish the staged binary at {subject}: {error}")
    return subject



def case_root(root: str, case: str) -> str:
    path = os.path.join(root, "cases", case, secrets.token_hex(4))
    os.makedirs(path, exist_ok=True)
    return path


def fixture_paths(base: str) -> dict:
    """This case's private paths, all inside its own run root. Pure.

    The pool is a genuine `directory-budget-v1` pool, so it is a *scratch*
    pool: one root the case declares, canonical (the declaration has no symlink
    component) and with the pool strictly below it. Nothing here is outside
    `OXIDE_TASK_SCRATCH_ROOT`, so the supervisor's census, reserve watchdog and
    scratch budget cover every byte the fixture writes.
    """
    scratch = os.path.realpath(os.path.join(base, SCRATCH_DIRNAME))
    return {
        "home": os.path.join(base, "home"),
        "cargo_home": os.path.join(base, "cargo-home"),
        "temp": os.path.join(base, "temp"),
        "scratch": scratch,
        "pool": os.path.join(scratch, POOL_DIRNAME),
    }


def fixture_environment(
    base: str, case: str, race: str | None, inherited: dict
) -> dict:
    """One case's complete fixture environment. Pure: it creates nothing.

    Pure so the effect-free `--self-check` can assert the same contract a native
    case runs under: the declaration, the pool strictly inside it and the
    explicit cap, with a hostile inherited environment in place.
    """
    paths = fixture_paths(base)
    env = scrubbed(inherited)
    env.update(toolchain_env())
    env.update(
        {
            "HOME": paths["home"],
            "CARGO_HOME": paths["cargo_home"],
            "TMPDIR": paths["temp"],
            "TMP": paths["temp"],
            "TEMP": paths["temp"],
            "OXIDE_TEST_HOME_FROM_ENV": "1",
            # The scratch declaration, the pool strictly inside it and the
            # explicit budget the product requires for a scratch pool.
            "OXIDE_SCRATCH_ROOT": paths["scratch"],
            "OXIDE_POOL": paths["pool"],
            "OXIDE_POOL_CREATE": "1",
            "OXIDE_POOL_CAP": str(POOL_BUDGET),
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
        }
    )
    # The supervisor's own layout is what arms the debug-only injection.
    for key in ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_PROOF_DEADLINE_NS", "OXIDE_PROOF_EVIDENCE"):
        if key in inherited:
            env[key] = inherited[key]
    if race is not None:
        env["OXIDE_TEST_FINDER_RACE"] = race
    for key, value in CASE_ENV.get(case, {}).items():
        env[key] = value
    return env


def case_env(root: str, case: str, race: str | None) -> tuple[dict, str]:
    base = case_root(root, case)
    paths = fixture_paths(base)
    for path in (paths["home"], paths["cargo_home"], paths["temp"], paths["scratch"]):
        os.makedirs(path, mode=0o700, exist_ok=True)
    return fixture_environment(base, case, race, os.environ), base


def workspace(env: dict, base: str, name: str, package: str = "app") -> str:
    """A tiny private Git checkout whose ignore rule covers `target`."""
    path = os.path.join(base, "workspaces", name)
    os.makedirs(os.path.join(path, "src"), exist_ok=True)
    for relative, contents in CRATE.items():
        if relative == "Cargo.toml" and package != "app":
            contents = contents.replace('name = "app"', f'name = "{package}"', 1)
        with open(os.path.join(path, relative), "w", encoding="utf-8") as handle:
            handle.write(contents)
    with open(os.path.join(path, ".gitignore"), "w", encoding="utf-8") as handle:
        handle.write(GITIGNORE)
    for argv in (("init", "-q"), ("add", "-A"), ("commit", "-q", "-m", "fixture")):
        result = git(env, path, *argv)
        if result.returncode:
            fail(f"cannot prepare the private Git fixture: {result.stderr[-800:]}")
    return path


def slot_leaves(pool: str) -> dict:
    """Every slot *leaf* (`<profile>-<n>`), never its project-label folder."""
    root = os.path.join(pool, "payload", "slots")
    leaves = {}
    if not os.path.isdir(root):
        return leaves
    for folder in sorted(os.listdir(root)):
        directory = os.path.join(root, folder)
        if not os.path.isdir(directory):
            continue
        for entry in sorted(os.listdir(directory)):
            path = os.path.join(directory, entry)
            if os.path.isdir(path):
                leaves[path] = sum(1 for _, _, files in os.walk(path) for _ in files)
    return leaves


def tree_files(path: str) -> list:
    return [
        os.path.join(dirpath, name)
        for dirpath, _, names in os.walk(path)
        for name in names
    ]


def evidence_mark() -> int:
    path = os.path.join(evidence_dir(), "finder-race-fired")
    return os.path.getsize(path) if os.path.isfile(path) else 0


def evidence_fired(mark: int) -> str:
    """Only what this case appended: an earlier case's firing proves nothing."""
    path = os.path.join(evidence_dir(), "finder-race-fired")
    if not os.path.isfile(path):
        return ""
    with open(path, encoding="utf-8") as handle:
        handle.seek(mark)
        return handle.read()


def firing_tail(mark: int) -> str:
    """The first `prune-low-reserve-fired` line this case appended."""
    path = os.path.join(evidence_dir(), "prune-low-reserve-fired")
    if not os.path.isfile(path):
        return ""
    with open(path, encoding="utf-8") as handle:
        handle.seek(mark)
        for line in handle:
            if line.strip():
                return line.strip()
    return ""


def require_fired(case: str, mark: int, kind: str) -> None:
    if kind not in evidence_fired(mark):
        fail(f"{case}: the armed {kind} race never fired; the case proves nothing")


def registry(env: dict) -> tuple[str, dict]:
    """The pool-owned live registry: `<pool>/oxide-managed/outputs/registry.json`.

    This is the control directory the product itself reads and writes
    (`prune::registry_path`), not a guessed path under the private HOME. An
    absent, unreadable, incomplete or oddly shaped document is unknown journal
    state: the collection aborts rather than treating it as an oracle failure.
    """
    control = os.path.join(env["OXIDE_POOL"], MANAGED_NAME)
    path = os.path.join(control, *REGISTRY_RELATIVE)
    state, _ = lstat_state(path)
    if state == "absent":
        unknown(f"the pool-owned registry {path} is missing; journal state is unknown")
    if state == "error":
        unknown(f"the pool-owned registry {path} cannot be inspected; journal state is unknown")
    try:
        with open(path, encoding="utf-8") as handle:
            document = json.load(handle)
    except Exception as error:  # noqa: BLE001 - unreadable journal state
        unknown(f"the pool-owned registry {path} is unreadable ({error!r})")
    if not isinstance(document, dict) or document.get("complete") is not True:
        unknown(f"the pool-owned registry {path} is not a complete document")
    for key in ("rows", "intents"):
        if document.get(key) is not None and not isinstance(document.get(key), list):
            unknown(f"the pool-owned registry {path} has a malformed {key} list")
    return path, document


def intent_for(env: dict, leaf: str) -> tuple[dict, dict]:
    _, document = registry(env)
    intents = [
        intent
        for intent in document.get("intents") or []
        if isinstance(intent, dict) and intent.get("quarantine") == leaf
    ]
    if len(intents) != 1:
        unknown(f"the registry holds {len(intents)} intents for {leaf}, expected exactly one")
    return document, intents[0]


def selected_members(document: dict, root_path: str) -> list:
    """The committed member links the registry says select this root."""
    return sorted(
        {
            row.get("member")
            for row in document.get("rows") or []
            if isinstance(row, dict)
            and row.get("root") == root_path
            and row.get("selected") is True
            and row.get("pending") is not True
            and row.get("pruning") is None
            and isinstance(row.get("member"), str)
            and row.get("member")
        }
    )



def ledger_document(env: dict) -> dict:
    """The pool's own ledger, read after the command that wrote it exited.

    An absent, unreadable or malformed document is unknown journal state, never
    an oracle failure: nothing is concluded from what could not be read.
    """
    path = os.path.join(env["OXIDE_POOL"], "ledger.json")
    state, _ = lstat_state(path)
    if state == "absent":
        unknown(f"the pool ledger {path} is missing; journal state is unknown")
    if state == "error":
        unknown(f"the pool ledger {path} cannot be inspected; journal state is unknown")
    try:
        with open(path, encoding="utf-8") as handle:
            document = json.load(handle)
    except Exception as error:  # noqa: BLE001 - unreadable journal state
        unknown(f"the pool ledger {path} is unreadable ({error!r})")
    if not isinstance(document, dict):
        unknown(f"the pool ledger {path} is not a document")
    holding = document.get("holding")
    if holding is not None and not isinstance(holding, list):
        unknown(f"the pool ledger {path} has a malformed holding list")
    return document


def holding_rows(env: dict) -> list:
    """The recorded pending deletions (oxide#190 `holding` rows)."""
    rows = ledger_document(env).get("holding") or []
    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get("name"), str):
            unknown(f"the ledger holds a malformed holding row: {row!r}")
    return rows


def holding_trees(env: dict) -> dict:
    """The holding entries actually present, by name.

    `<pool>/payload/.oxide-holding/<store>-<slot>-<incarnation>-<nonce>` is the
    area an eviction renames a whole slot tree into (oxide#190,
    `src/lease/pool.rs:112`), so a presence check of one entry is an `lstat`,
    never a walk.
    """
    root = os.path.join(env["OXIDE_POOL"], "payload", HOLDING_NAME)
    state, _ = lstat_state(root)
    if state == "absent":
        return {}
    if state == "error":
        unknown(f"the holding area {root} cannot be inspected; custody is unknown")
    if not os.path.isdir(root):
        unknown(f"the holding area {root} is not a directory")
    return {name: os.path.join(root, name) for name in sorted(os.listdir(root))}


def plant(case: str, victim: str, base: str):
    """Put this case's content inside the idle slot's own tree.

    Nothing is injected: oxide#190's eviction has no race seam (the debug hook is
    reached only from the outputs-prune drain), so the lease half plants the state
    its case is about and the oracles read the holding model's own outcomes.
    Returns the link case's sentinel path, otherwise a short description.
    """
    nested = os.path.join(victim, "src", "fixture")
    os.makedirs(nested, exist_ok=True)
    if case == "lease_control":
        return "no planted content (premise control)"
    if case == "lease_metadata":
        for directory in (victim, nested):
            for name, data in ((".DS_Store", b"finder"), ("._lib.rs", b"appledouble")):
                with open(os.path.join(directory, name), "wb") as handle:
                    handle.write(data)
        return "Finder metadata at the tree root and one nested directory"
    if case == "lease_foreign":
        with open(os.path.join(victim, "foreign.txt"), "w", encoding="utf-8") as handle:
            handle.write("foreign\n")
        return "one ordinary foreign file"
    if case == "lease_link":
        sentinel = os.path.join(base, "link-referent")
        os.makedirs(sentinel, mode=0o700, exist_ok=True)
        with open(os.path.join(sentinel, "kept.txt"), "w", encoding="utf-8") as handle:
            handle.write("referent\n")
        os.symlink(sentinel, os.path.join(victim, ".DS_Store"))
        return sentinel
    if case == "lease_exhaust":
        os.mkfifo(os.path.join(victim, "unemptiable"), mode=0o600)
        return "one entry the drain cannot classify (a FIFO)"
    fail(f"{case}: no lease fixture is declared")


def sentinel_identity(path: str) -> tuple:
    """The link sentinel's identity and content, read without following it."""
    state, facts = lstat_state(path)
    if state != "present":
        unknown(f"the link sentinel {path} is {state}; its custody is unknown")
    if not stat.S_ISDIR(facts.st_mode) or facts.st_uid != os.getuid():
        unknown(f"the link sentinel {path} is not our own directory")
    with open(os.path.join(path, "kept.txt"), encoding="utf-8") as handle:
        return facts.st_dev, facts.st_ino, handle.read()


def lease_case(oxide: str, root: str, case: str, race: str | None) -> str:
    """One holding-model case: evict an idle whole slot and read what happened.

    The eviction renames the tree into the payload's holding area and records one
    row; the command then joins its own deletion outside the pool lock. A tree the
    drain can empty leaves neither a row nor a tree; one it cannot keeps both,
    charged, until a later command finishes it.
    """
    env, base = case_env(root, case, race)
    first = run([oxide, "check"], cwd=workspace(env, base, "a"), env=env)
    if first.returncode:
        fail(f"{case}: the first admission failed: {first.stderr[-1500:]}")
    before = slot_leaves(env["OXIDE_POOL"])
    if len(before) != 1:
        fail(
            f"{case}: the first admission left {len(before)} slot leaves, expected one: {sorted(before)}"
        )
    victim = next(iter(before))
    planted = plant(case, victim, base)
    sentinel = sentinel_identity(planted) if case == "lease_link" else None
    second_checkout = workspace(env, base, "b", "second-project")
    second = run([oxide, "check"], cwd=second_checkout, env=env)
    after = slot_leaves(env["OXIDE_POOL"])
    rows = holding_rows(env)
    trees = holding_trees(env)
    status = run([oxide, "status"], cwd=base, env=env)
    if victim in after:
        fail(f"{case}: the evicted slot leaf {victim} is still under payload/slots")
    if "recorded deletion pending reconciliation" in status.stdout:
        fail(f"{case}: status reports a legacy pending reconciliation: {status.stdout[-1500:]}")

    if case != "lease_exhaust":
        if second.returncode:
            fail(f"{case}: the eviction did not complete: {second.stderr[-1500:]}")
        if rows or trees:
            fail(
                f"{case}: a held deletion survived a completed eviction: rows {rows}, "
                f"trees {sorted(trees)}"
            )
        if "held for deletion" in status.stdout:
            fail(f"{case}: status reports a held deletion that completed: {status.stdout[-1500:]}")
        if case == "lease_link":
            if sentinel_identity(planted) != sentinel:
                fail(f"{case}: the drain followed the symbolic link into its referent")
            return f"{case}: the link went with the held tree and its referent was not followed"
        return f"{case}: completed; the victim went with the held tree ({planted})"

    # The unemptiable entry. The eviction commits (the leaf is gone and one row
    # is recorded), the drain refuses the entry by name and the tree stays
    # charged, so an admission that needed that space is refused rather than
    # deciding as if the bytes were free.
    if second.returncode == 0:
        fail(f"{case}: an admission over budget admitted beside an undeletable held tree")
    if "cannot lease a slot" not in second.stderr:
        fail(f"{case}: the capacity refusal was not reported: {second.stderr[-1500:]}")
    if "admit a new slot" not in second.stderr:
        fail(f"{case}: the refusal does not name the capacity result: {second.stderr[-1500:]}")
    if "kept the held slot tree" not in second.stderr:
        fail(f"{case}: the refused drain was not reported: {second.stderr[-1500:]}")
    if "stays recorded and charged" not in second.stderr:
        fail(f"{case}: the refusal did not report the charged row: {second.stderr[-1500:]}")
    if len(rows) != 1:
        fail(f"{case}: the ledger holds {len(rows)} holding rows, expected exactly one: {rows}")
    row = rows[0]
    if row.get("bytes", 0) <= 0:
        fail(f"{case}: the held tree is not charged: {row}")
    if row["name"] not in trees:
        fail(f"{case}: the recorded holding tree {row['name']!r} is not present: {sorted(trees)}")
    held = trees[row["name"]]
    if not [path for path in tree_files(held) if os.path.basename(path) == "unemptiable"]:
        fail(f"{case}: the held tree lost its unemptiable entry")
    held_lines = [
        line
        for line in status.stdout.splitlines()
        if f"slot {row.get('slot')} held for deletion" in line
    ]
    if len(held_lines) != 1 or "tree still present" not in held_lines[0]:
        fail(f"{case}: status does not report the held tree: {status.stdout[-1500:]}")
    # The fixture removes its own item; the next command finishes the deletion.
    os.unlink(os.path.join(held, "unemptiable"))
    finish = run([oxide, "check"], cwd=workspace(env, base, "c", "third-project"), env=env)
    if finish.returncode:
        fail(f"{case}: the finishing command failed: {finish.stderr[-1500:]}")
    if "deleted the held slot tree" not in finish.stderr:
        fail(f"{case}: the finishing command did not report the deletion: {finish.stderr[-1500:]}")
    remaining_rows, remaining_trees = holding_rows(env), holding_trees(env)
    if remaining_rows or remaining_trees:
        fail(
            f"{case}: the held deletion was not finished: rows {remaining_rows}, "
            f"trees {sorted(remaining_trees)}"
        )
    closing = run([oxide, "status"], cwd=base, env=env)
    if "held for deletion" in closing.stdout:
        fail(f"{case}: status still reports a held deletion: {closing.stdout[-1500:]}")
    return (
        f"{case}: the unemptiable entry kept the tree charged under {HOLDING_NAME}; "
        "the next command finished it"
    )


class ImageCustody:
    """One owned sparse image whose backing and custody record live in the
    guard's evidence directory, written before the first effect."""

    def __init__(self, case: str):
        self.case = case
        self.name = prune_volume_name(case)
        self.expected_mount = f"/Volumes/{self.name}"
        # Before any effect: the layout must fit the product's socket bound.
        assert_prune_path_budget(case, self.name)
        directory = os.path.join(evidence_dir(), "images")
        os.makedirs(directory, mode=0o700, exist_ok=True)
        facts = os.stat(directory)
        if facts.st_uid != os.getuid() or facts.st_mode & 0o077:
            unknown(f"image custody directory {directory} is not owner-only")
        self.directory = directory
        self.backing = os.path.join(directory, f"{self.name}.sparseimage")
        self.record = os.path.join(directory, f"{self.name}-custody.json")
        self.mount = None
        self.device_entry = None
        self.device = None
        self.state = {"case": case, "backing": self.backing, "phases": []}
        self.write("declared", expected_mount=self.expected_mount)

    def write(self, phase: str, **facts) -> None:
        self.state["phases"].append({"phase": phase, **facts})
        with open(self.record, "w", encoding="utf-8") as handle:
            json.dump(self.state, handle, indent=2, sort_keys=True)
            handle.write("\n")

    def attach(self) -> str:
        # The volume name is this run's unique name. Without it the created APFS
        # volume is called `untitled` (hdiutil's default) and would mount at
        # `/Volumes/untitled` (or `/Volumes/untitled 1`), never at the expected
        # mount this proof binds — so the name is part of the fixture, not
        # decoration.
        created = run(
            [
                "hdiutil",
                "create",
                "-size",
                f"{IMAGE_MIB}m",
                "-fs",
                "APFS",
                "-type",
                "SPARSE",
                "-volname",
                self.name,
                "-quiet",
                self.backing,
            ],
            cwd=self.directory,
            env=scrubbed(os.environ),
            timeout=60,
        )
        if created.returncode:
            unknown(f"{self.case}: cannot create the owned image: {created.stderr[-800:]}")
        facts = os.lstat(self.backing)
        if not stat.S_ISREG(facts.st_mode) or facts.st_uid != os.getuid():
            unknown(f"{self.case}: the created backing is not an owned regular file")
        self.write(
            "created",
            size=facts.st_size,
            inode=facts.st_ino,
            device=facts.st_dev,
            volume_name=self.name,
        )
        # An explicit mount point is used when it already exists as an empty
        # directory we can bind; `/Volumes` itself is root-owned, so the normal
        # route is the unique volume name above, and the attach must then land
        # on exactly that path or it is unknown custody.
        attach = ["hdiutil", "attach", "-owners", "on", "-nobrowse", "-plist"]
        mount_state, mount_facts = lstat_state(self.expected_mount)
        if mount_state == "error":
            unknown(
                f"{self.case}: cannot inspect the expected mount {self.expected_mount} "
                f"({mount_facts}); retained"
            )
        if mount_state == "present":
            unknown(
                f"{self.case}: the expected mount {self.expected_mount} already exists; retained"
            )
        if os.path.isdir(self.expected_mount):
            attach += ["-mountpoint", self.expected_mount]
            self.write("mountpoint-explicit", mount=self.expected_mount)
        else:
            self.write("mountpoint-volume-name", expected=self.expected_mount)
        attach.append(self.backing)
        attached = run(
            attach,
            cwd=self.directory,
            env=scrubbed(os.environ),
            timeout=60,
        )
        if attached.returncode:
            unknown(f"{self.case}: cannot attach the owned image: {attached.stderr[-800:]}")
        self.write(
            "attach-raw",
            plist_sha256=hashlib.sha256(attached.stdout.encode()).hexdigest(),
        )
        try:
            document = plistlib.loads(attached.stdout.encode())
        except Exception as error:  # noqa: BLE001 - custody is unknown here
            unknown(f"{self.case}: the attach plist is unreadable ({error!r}); retained")
        mounted = [
            entity
            for entity in document.get("system-entities", [])
            if entity.get("mount-point") and entity.get("dev-entry")
        ]
        if len(mounted) != 1:
            unknown(
                f"{self.case}: the attach returned {len(mounted)} mounted entities, expected one; retained"
            )
        mount = mounted[0]["mount-point"]
        entry = mounted[0]["dev-entry"]
        if mount != self.expected_mount:
            unknown(
                f"{self.case}: the attach mounted {mount}, not the expected {self.expected_mount}; retained"
            )
        try:
            mount_facts = os.stat(mount)
            entry_facts = os.stat(entry)
        except OSError as error:
            unknown(f"{self.case}: cannot bind the mount to its device ({error}); retained")
        if not stat.S_ISBLK(entry_facts.st_mode):
            unknown(f"{self.case}: the mount's device entry {entry} is not a block device; retained")
        if entry_facts.st_rdev != mount_facts.st_dev:
            unknown(
                f"{self.case}: the mount {mount} is device {mount_facts.st_dev}, "
                f"not its recorded device entry {entry} ({entry_facts.st_rdev}); retained"
            )
        self.mount, self.device_entry, self.device = mount, entry, entry_facts.st_rdev
        self.write(
            "attached",
            mount=mount,
            device_entry=entry,
            device=self.device,
            mount_mode=oct(mount_facts.st_mode),
        )
        return mount

    def verify_owned_attachment(self) -> None:
        """Revalidate this run's attachment before detaching it.

        The mount must still be *our* mounted directory (its `st_dev` equals the
        device recorded at attach) and its device entry must still be the same
        block device. A metadata read that fails is unknown, never absence.
        """
        mount_state, mount_facts = lstat_state(self.mount)
        device_state, device_facts = lstat_state(self.device_entry)
        if mount_state == "error":
            unknown(
                f"{self.case}: cannot read the attachment's mount {self.mount} ({mount_facts}); retained"
            )
        if device_state == "error":
            unknown(
                f"{self.case}: cannot read the attachment's device entry {self.device_entry} "
                f"({device_facts}); retained"
            )
        if mount_state != "present" or device_state != "present":
            unknown(
                f"{self.case}: the owned attachment is no longer present "
                f"(mount {mount_state}, device {device_state}); retained"
            )
        if stat.S_ISLNK(mount_facts.st_mode) or not stat.S_ISDIR(mount_facts.st_mode):
            unknown(f"{self.case}: {self.mount} is not a mounted directory; retained")
        if mount_facts.st_dev != self.device:
            unknown(
                f"{self.case}: the mount {self.mount} is device {mount_facts.st_dev}, "
                f"not our attachment {self.device}; retained"
            )
        if not stat.S_ISBLK(device_facts.st_mode) or device_facts.st_rdev != self.device:
            unknown(
                f"{self.case}: the device entry {self.device_entry} no longer binds our "
                f"device {self.device}; retained"
            )
        self.write("attachment-revalidated", device=self.device)

    def detach(self) -> None:
        """Ordinary detach only, then positive device/mount absence, then the
        backing. Never forced, never retried, never inferred from a swallowed
        error."""
        if self.mount is None:
            self.write("detach-unnecessary")
            return
        self.verify_owned_attachment()
        result = run(
            ["hdiutil", "detach", self.mount],
            cwd="/",
            env=scrubbed(os.environ),
            timeout=60,
        )
        if result.returncode:
            self.write("detach-failed", stderr=result.stderr[-400:], retained=True)
            unknown(f"{self.case}: the ordinary detach of {self.mount} failed; retained")
        mount_state, mount_facts = lstat_state(self.mount)
        device_state, device_facts = lstat_state(self.device_entry)
        if mount_state == "error" or device_state == "error":
            self.write("detach-unreadable", retained=True)
            unknown(
                f"{self.case}: cannot prove absence after detach "
                f"(mount {mount_state}/{mount_facts}, device {device_state}/{device_facts}); retained"
            )
        if device_state == "present":
            self.write("detach-unproven", device_still_present=True, retained=True)
            unknown(
                f"{self.case}: the device entry {self.device_entry} is still present after detach; retained"
            )
        if mount_state == "present":
            if mount_facts.st_dev == self.device:
                self.write("detach-unproven", mount_still_ours=True, retained=True)
                unknown(f"{self.case}: our mount {self.mount} is still attached; retained")
            # A different volume took the path: our attachment is positively
            # gone, and that observation is recorded rather than assumed.
            self.write("mount-path-reused", device=mount_facts.st_dev)
        self.write("detached")
        os.unlink(self.backing)
        self.write("backing-removed")

    def retain(self, reason: str) -> None:
        self.write("retained", reason=reason[-400:])


def adopt_and_assert(oxide: str, env: dict, checkout: str, mount: str) -> tuple:
    """Build the fresh Git checkout and assert the conversion really happened.

    The profile layout publishes owned member links *inside* a real `target`
    directory (oxide#160), so the oracle asks the committed registry which
    members this root selected and then requires each of those links to resolve
    into that root. It never assumes a whole-target `target` symlink.
    """
    built = run([oxide, "check"], cwd=checkout, env=env)
    if built.returncode:
        fail(f"the fixture build on the drive failed: {built.stderr[-1500:]}")
    outputs = os.path.join(mount, "oxide-outputs")
    if not os.path.isdir(outputs):
        fail("the first build published no owned outputs tree on the drive")
    roots = [
        entry
        for entry in sorted(os.listdir(outputs))
        if os.path.isdir(os.path.join(outputs, entry)) and not entry.startswith(".prune-")
    ]
    if len(roots) != 1:
        fail(f"the build left {len(roots)} owned roots on the drive, expected one: {roots}")
    root_path = os.path.join(outputs, roots[0])
    target = os.path.join(checkout, "target")
    state, facts = lstat_state(target)
    if state != "present" or stat.S_ISLNK(facts.st_mode) or not stat.S_ISDIR(facts.st_mode):
        fail(f"the converted checkout has no real target directory at {target}")
    _, document = registry(env)
    members = selected_members(document, root_path)
    if not members:
        fail(f"the committed registry names no selected member row for {root_path}")
    root_real = os.path.realpath(root_path)
    for member in members:
        link = os.path.join(target, member)
        link_state, link_facts = lstat_state(link)
        if link_state != "present" or not stat.S_ISLNK(link_facts.st_mode):
            fail(f"the member link {link} is {link_state}, not an owned link")
        resolved = os.path.realpath(link)
        if not (resolved == root_real or resolved.startswith(root_real + os.sep)):
            fail(f"the member link {link} resolves to {resolved}, outside the root {root_path}")
    return outputs, root_path, members


def preview_plan(oxide: str, env: dict, base: str) -> tuple[str, str]:
    preview = run([oxide, "outputs", "prune", "--dry-run"], cwd=base, env=env)
    if preview.returncode:
        fail(f"the prune dry run refused: {preview.stderr[-1500:]}")
    digest = ""
    for line in preview.stderr.splitlines():
        for word in line.replace(":", " ").split():
            if len(word) == 64 and all(c in "0123456789abcdef" for c in word):
                digest = word
    if not digest:
        fail(f"the dry run printed no plan digest: {preview.stderr[-800:]}")
    plan = os.path.join(base, "plan.json")
    with open(plan, "w", encoding="utf-8", newline="") as handle:
        handle.write(preview.stdout)
    return plan, digest


def prune_case(oxide: str, root: str, case: str, race: str | None) -> str:
    env, base = case_env(root, case, race)
    mark = evidence_mark()
    # The product's low-reserve firing record is case-local too: remember what
    # this case appended, so an earlier case's firing cannot satisfy it.
    low_mark = os.path.getsize(os.path.join(evidence_dir(), "prune-low-reserve-fired")) if os.path.isfile(
        os.path.join(evidence_dir(), "prune-low-reserve-fired")
    ) else 0
    custody = ImageCustody(case)
    try:
        mount = custody.attach()
        register = run(
            [
                oxide,
                "pool",
                "register-removable",
                mount,
                "--reserve",
                RESERVE,
                "--cap",
                IMAGE_CAP,
                "--mode",
                "permanent",
            ],
            cwd=base,
            env=env,
        )
        if register.returncode:
            fail(f"{case}: cannot register the owned drive: {register.stderr[-1500:]}")
        checkout = os.path.join(mount, "checkout")
        shutil.copytree(workspace(env, base, "a"), checkout)
        outputs, root_path, members = adopt_and_assert(oxide, env, checkout, mount)
        # The checkout must be positively gone before the root is eligible.
        shutil.rmtree(checkout)
        if os.path.exists(checkout):
            fail(f"{case}: the fixture checkout was not removed")
        plan, digest = preview_plan(oxide, env, base)
        real = run(
            [oxide, "outputs", "prune", "--plan", plan, "--plan-sha256", digest],
            cwd=base,
            env=env,
        )
        quarantined = sorted(
            entry for entry in os.listdir(outputs) if entry.startswith(".prune-")
        )
        kept_root = os.path.isdir(root_path)
        settle = Settle(
            oxide=oxide,
            env=env,
            base=base,
            case=case,
            mark=mark,
            low_mark=low_mark,
            real=real,
            outputs=outputs,
            root_path=root_path,
            members=members,
            quarantined=quarantined,
            kept_root=kept_root,
            plan=plan,
            digest=digest,
        )
        result = settle_target(case)(settle)
        custody.detach()
        return result
    except Unknown as error:
        custody.retain(f"unknown outcome: {error}")
        raise
    except Recoverable as error:
        # A recoverable oracle failure still means this fixture's image has to
        # leave cleanly, or its absence is unknown: treat a failed detach as
        # unknown.
        try:
            custody.detach()
        except Unknown as detach_error:
            custody.retain(f"after a recoverable failure: {detach_error}")
            raise detach_error from error
        raise


@dataclass
class Settle:
    """One real prune run's inputs for the settle oracles.

    A single bundle rather than a long positional list: a dispatch that drifts
    from a handler's parameters would otherwise raise `TypeError` inside the
    case instead of failing a pure check.
    """

    oxide: str
    env: dict
    base: str
    case: str
    mark: int
    low_mark: int
    real: object
    outputs: str
    root_path: str
    members: list
    quarantined: list
    kept_root: bool
    plan: str
    digest: str


def settle_target(case: str):
    """The settle handler this case's real run routes to."""
    return settle_completion_fault if case == "prune_completion_fault" else settle_case


def replay_prune(oxide: str, env: dict, base: str, plan: str, digest: str):
    """The one replay invocation both settle handlers use."""
    return run(
        [oxide, "outputs", "prune", "--plan", plan, "--plan-sha256", digest],
        cwd=base,
        env=env,
    )


def settle_case(settle: Settle) -> str:
    oxide = settle.oxide
    env = settle.env
    base = settle.base
    case = settle.case
    mark = settle.mark
    low_mark = settle.low_mark
    real = settle.real
    outputs = settle.outputs
    root_path = settle.root_path
    quarantined = settle.quarantined
    kept_root = settle.kept_root
    plan = settle.plan
    digest = settle.digest
    if case == "prune_metadata":
        if real.returncode:
            fail(f"{case}: the prune did not complete: {real.stderr[-1500:]}")
        if quarantined:
            fail(f"{case}: a quarantined leaf was left behind: {quarantined}")
        if kept_root:
            fail(f"{case}: the pruned root is still present")
        _, document = registry(env)
        if json.dumps(document.get("intents") or []).find(root_path) >= 0:
            fail(f"{case}: the live registry still holds an intent for {root_path}")
        if selected_members(document, root_path):
            fail(f"{case}: the live registry still selects the pruned root {root_path}")
        require_fired(case, mark, "Metadata")
        return f"{case}: deleted, exit {real.returncode}"

    if case == "prune_completion_fault":
        return settle_completion_fault(settle)

    if real.returncode != 3:
        fail(
            f"{case}: an unsettled deletion did not exit 3 "
            f"(exit {real.returncode}: {real.stderr[-1500:]})"
        )
    if not quarantined:
        fail(f"{case}: the pending intent left no quarantined leaf")
    if kept_root:
        fail(f"{case}: the root is still under its live name while a quarantine exists")
    leaf = os.path.join(outputs, quarantined[0])
    document, intent = intent_for(env, leaf)
    phase = intent.get("phase")
    if phase not in UNFINISHED_PHASES:
        fail(f"{case}: the journal phase is {phase!r}, not an unfinished one")
    if intent.get("receipt_settled") is not False:
        fail(f"{case}: the intent claims a settled receipt: {intent.get('receipt_settled')}")
    if phase in TERMINAL_PHASES:
        fail(f"{case}: the journal reached a terminal phase {phase!r}")

    if case == "prune_drain_fault":
        # The intended injected stop, bound to *this* case: the product's
        # low-reserve reading fired on the first check after one recovered
        # Finder metadata unlink, so the drain stopped with the remaining
        # tolerated metadata still there.
        fired = firing_tail(low_mark)
        if not fired:
            fail(f"{case}: the low-reserve injection recorded no firing for this case")
        if not fired.startswith("recovered: "):
            fail(f"{case}: the firing is not the recovered-metadata mode: {fired!r}")
        if "1 recovered" not in fired:
            fail(f"{case}: the firing does not name one recovered entry: {fired!r}")
        survivors = [
            path
            for path in tree_files(leaf)
            if os.path.basename(path) == ".DS_Store"
            or os.path.basename(path).startswith("._")
        ]
        if not survivors:
            fail(f"{case}: the mid-drain stop deleted every recovered metadata entry")
        if phase not in UNFINISHED_PHASES:
            fail(f"{case}: the journal phase is {phase!r}, not the pending one the stop leaves")
        require_fired(case, mark, "Metadata")
        return f"{case}: pending mid-drain, quarantine {quarantined[0]}, phase {phase}"

    if case == "prune_foreign":
        if not [
            path for path in tree_files(leaf) if os.path.basename(path) == "foreign.txt"
        ]:
            fail(f"{case}: the foreign entry was deleted instead of kept")
        require_fired(case, mark, "Foreign")
        return f"{case}: pending, quarantine {quarantined[0]}, phase {phase}"

    if case == "prune_exhaust":
        require_fired(case, mark, "Metadata")
        return f"{case}: pending, quarantine {quarantined[0]}, phase {phase}"

    # The two replay cases start from this pending intent and mutate its
    # quarantine the way an unknown writer would.
    if case == "prune_missing_quarantine":
        shutil.rmtree(leaf)
        if os.path.exists(leaf):
            fail(f"{case}: cannot remove the quarantine for this control")
    elif case == "prune_replaced_quarantine":
        moved = leaf + ".moved"
        os.rename(leaf, moved)
        os.makedirs(leaf)
        with open(os.path.join(leaf, "replacement.txt"), "w", encoding="utf-8") as handle:
            handle.write("replacement\n")
    else:
        fail(f"{case}: unknown prune case")

    replay = replay_prune(oxide, env, base, plan, digest)
    if replay.returncode != 3:
        fail(
            f"{case}: a replay over a mutated quarantine did not exit 3 "
            f"(exit {replay.returncode}: {replay.stderr[-1500:]})"
        )
    document, intent = intent_for(env, leaf)
    phase = intent.get("phase")
    if phase in TERMINAL_PHASES:
        fail(f"{case}: the replay declared completion ({phase!r})")
    if case == "prune_replaced_quarantine":
        if not os.path.isfile(os.path.join(leaf, "replacement.txt")):
            fail(f"{case}: the replacement directory or its content was removed")
    return f"{case}: pending after replay, phase {phase}"


# Values this proof's remaining oracles read out of product source, re-checked
# effect-free against the current tree (the S4 pre-review current-main sweep,
# oxide#213). Each one is the product truth behind an assertion the script makes
# at a native head, so a moved constant or reworded line fails here instead of
# inside a native case. Nothing here is a claim about a run.
ORACLE_PINS = (
    (
        "src/bin/oxide/child_temp.rs",
        "pub(crate) const MAX_SCRATCH_PATH_BYTES: usize = 91;",
        "the product's per-command temporary path bound",
    ),
    (
        "src/lease/mod.rs",
        'const SLOTS_NAME: &str = "slots";',
        "the slot leaves the lease oracles read",
    ),
    (
        "src/lease/pool.rs",
        'const HOLDING_NAME: &str = ".oxide-holding";',
        "the payload's private holding area",
    ),
    (
        "src/lease/pool.rs",
        "libc::S_IFREG | libc::S_IFLNK => {",
        "the drain's by-name unlink of files and links",
    ),
    (
        "src/lease/pool.rs",
        '"an unknown entry type"',
        "the drain's refusal the exhaustion case reads",
    ),
    (
        "src/bin/oxide/main.rs",
        "kept the held slot tree",
        "the reported refusal of a held deletion",
    ),
    (
        "src/bin/oxide/main.rs",
        "stays recorded and charged",
        "the charged row a refusal reports",
    ),
    (
        "src/bin/oxide/main.rs",
        "deleted the held slot tree",
        "the reported finish of a held deletion",
    ),
    (
        "src/bin/oxide/main.rs",
        '"  slot {} held for deletion: {} recorded, {}s old, {}"',
        "the status line the exhaustion case reads",
    ),
    (
        "src/bin/oxide/main.rs",
        '"tree still present, deletion pending outside the pool lock"',
        "the held tree's recorded presence",
    ),
    (
        "src/lease/mod.rs",
        '"pool cap cannot admit a new slot"',
        "the capacity refusal the exhaustion case reads",
    ),
    (
        "src/bin/oxide/prune.rs",
        'const REGISTRY_FILE: &str = "registry.json";',
        "the pool-owned registry leaf",
    ),
    (
        "src/bin/oxide/prune.rs",
        'const QUARANTINE_PREFIX: &str = ".prune-";',
        "the quarantined leaf prefix",
    ),
    (
        "src/bin/oxide/prune.rs",
        'crash_point("terminal")',
        "the pre-completion crash point",
    ),
    (
        "src/bin/oxide/prune.rs",
        "{mode}: check {count}, {deleted} entries deleted, {recovered} recovered",
        "the low-reserve firing line",
    ),
    (
        "src/bin/oxide/prune.rs",
        "oxide: prune preview SHA-256",
        "the preview digest line the plan oracle reads",
    ),
    ("src/bin/oxide/prune.rs", '["prune", "--dry-run"]', "the preview dispatch"),
    (
        "src/bin/oxide/outputs.rs",
        'pub const EXTERNAL_DIR: &str = "oxide-outputs";',
        "the owned outputs tree on the drive",
    ),
    (
        "src/bin/oxide/removable.rs",
        'root.join("oxide-managed")',
        "the pool control directory the registry lives in",
    ),
)
SELECTOR_RECIPE = {
    "rust:finder-metadata-race": "python3 scripts/test-finder-metadata-race.py",
    "rust:finder-metadata-race:diagnostic": (
        "python3 scripts/test-finder-metadata-race.py --diagnostic"
    ),
}


def selector_blocks(text: str) -> dict:
    """The task tables of an `effigy.toml`, keyed by their own header."""
    blocks = {}
    current = None
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            current = stripped
            blocks[current] = []
        elif current is not None:
            blocks[current].append(line)
    return blocks


def scratch_contract_problems(base: str = "/fixture/run") -> list:
    """The declared scratch pool and the selectors' recipe, with no effect.

    A script left half-migrated — a pool on the boot volume, no declaration, no
    explicit cap, or a pool outside the declared root — fails here instead of
    spending a native run whose pool the product would refuse, and a selector
    that still names the internal root fails with it.
    """
    problems = []
    case_base = os.path.join(base, "cases", "lease_control", "0badc0de")
    # Hostile inherited values: the declaration, the pool, the cap, the account
    # and the flag overrides must all come from the fixture, never the caller.
    hostile = {
        "OXIDE_SCRATCH_ROOT": "/fixture/host-scratch",
        "OXIDE_POOL": "/fixture/host-pool",
        "OXIDE_POOL_CAP": "1",
        "OXIDE_TASK_SCRATCH_ROOT": base,
        "OXIDE_PROOF_DEADLINE_NS": "981",
        "OXIDE_PROOF_EVIDENCE": "/fixture/evidence",
        "OXIDE_TEST_FINDER_RACE": "*:directory",
        "HOME": "/fixture/host-home",
        "CARGO_HOME": "/fixture/host-cargo",
        "TMPDIR": "/fixture/host-temp",
        "XDG_CONFIG_HOME": "/fixture/xdg",
        "RUSTFLAGS": "-Ctarget-cpu=native",
        "CARGO_ENCODED_RUSTFLAGS": "-Ctarget-cpu=native",
        "RUSTUP_TOOLCHAIN": "nightly",
    }
    for case in ("lease_control", "prune_drain_fault"):
        env = fixture_environment(
            case_base, case, RACES[case], hostile
        )
        scratch = env.get("OXIDE_SCRATCH_ROOT", "")
        pool = env.get("OXIDE_POOL", "")
        expected = os.path.join(case_base, SCRATCH_DIRNAME)
        if scratch != expected:
            problems.append(
                f"{case}: the declared scratch root is {scratch!r}, not the case's own {expected}"
            )
        if not os.path.isabs(scratch) or os.path.normpath(scratch) != scratch:
            problems.append(f"{case}: the declared scratch root {scratch!r} is not canonical")
        elif os.path.commonpath([scratch, base]) != base:
            problems.append(f"{case}: the declared scratch root {scratch!r} escapes the run root")
        if os.path.dirname(pool) != scratch or pool == scratch:
            problems.append(f"{case}: the pool {pool!r} is not strictly inside {scratch!r}")
        if env.get("OXIDE_POOL_CAP") != str(POOL_BUDGET):
            problems.append(f"{case}: the pool cap is not the explicit fixture budget")
        if env.get("OXIDE_POOL_CREATE") != "1":
            problems.append(f"{case}: the pool creation opt-in is missing")
        if env.get("OXIDE_TEST_HOME_FROM_ENV") != "1":
            problems.append(f"{case}: the account-home control is missing")
        for name, path in (
            ("HOME", "home"),
            ("CARGO_HOME", "cargo-home"),
            ("TMPDIR", "temp"),
        ):
            if env.get(name) != os.path.join(case_base, path):
                problems.append(f"{case}: {name} is not the case's private {path}")
        if env.get("TMPDIR") != env.get("TMP") or env.get("TMPDIR") != env.get("TEMP"):
            problems.append(f"{case}: TMPDIR/TMP/TEMP do not agree")
        if env.get("RUSTFLAGS") != "-C strip=symbols":
            problems.append(f"{case}: the compiler flags were not frozen")
        if "CARGO_ENCODED_RUSTFLAGS" in env or "XDG_CONFIG_HOME" in env:
            problems.append(f"{case}: an encoded flag or XDG override survived the scrub")
        for key in ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_PROOF_DEADLINE_NS", "OXIDE_PROOF_EVIDENCE"):
            if env.get(key) != hostile[key]:
                problems.append(f"{case}: the supervisor's {key} was not re-added")
        if case in CASE_ENV:
            for key, value in CASE_ENV[case].items():
                if env.get(key) != value:
                    problems.append(f"{case}: the case's {key} was not set")
        if RACES[case] is not None and env.get("OXIDE_TEST_FINDER_RACE") != RACES[case]:
            problems.append(f"{case}: the race injection is not this case's own")
    if os.path.sep in SCRATCH_DIRNAME or SCRATCH_DIRNAME in (".", ".."):
        problems.append("the scratch directory name is not a bare leaf name")
    if os.path.basename(POOL_DIRNAME) != POOL_DIRNAME or POOL_DIRNAME in (".", ".."):
        problems.append("the pool directory name is not a bare leaf name")
    path = os.path.join(REPO, "effigy.toml")
    try:
        with open(path, encoding="utf-8") as handle:
            blocks = selector_blocks(handle.read())
    except OSError as error:
        return problems + [f"cannot read {path}: {error}"]
    for selector, body in SELECTOR_RECIPE.items():
        lines = blocks.get(f'[tasks."{selector}"]')
        if lines is None:
            problems.append(f"{selector}: no such selector in effigy.toml")
            continue
        run_line = next(
            (line for line in lines if line.strip().startswith("run =")), None
        )
        if run_line is None:
            problems.append(f"{selector}: the selector has no run command")
            continue
        if "--root-parent" in run_line:
            problems.append(f"{selector}: the selector still names an internal root")
        if "--limit 420 --scratch-budget-mib 512" not in run_line:
            problems.append(f"{selector}: the unchanged 420 s / 512 MiB bounds drifted")
        if not run_line.strip().endswith(f"{body}\""):
            problems.append(f"{selector}: the selector does not run this proof's body")
        if "admission = \"heavy\"" not in "".join(lines):
            problems.append(f"{selector}: heavy admission is missing")
    return problems


def self_check() -> int:
    """Pure check of the fixture layout budget, the scratch pool contract, the
    current-main oracles and the shutdown cutoff: no scratch, child or image."""
    problems = []
    # The S4 fixture contract and the current-main oracle values, effect-free.
    problems.extend(scratch_contract_problems())
    for relative, needle, what in ORACLE_PINS:
        try:
            with open(os.path.join(REPO, relative), encoding="utf-8") as handle:
                source = handle.read()
        except OSError as error:
            problems.append(f"{relative}: cannot be read ({error})")
            continue
        if needle not in source:
            problems.append(f"{relative}: {what} drifted (missing {needle!r})")
    # Counterfactual: the 420 s selector leaves exactly 390 s of work, and the
    # earlier 25 s margin's 395 s is refused.
    if SHUTDOWN_MARGIN_SECONDS != 30:
        problems.append(f"shutdown margin is {SHUTDOWN_MARGIN_SECONDS} s, not the approved 30 s")
    left = window_left(420 * 10**9 + 1, 1)
    if left != 390.0:
        problems.append(f"a 420 s session leaves {left} s of work, not 390 s")
    if left == 395.0:
        problems.append("the cutoff still uses the 25 s margin")
    if window_left(35 * 10**9, 0) != 5.0:
        problems.append("the spent-window floor is not measured from the 30 s margin")
    if window_left(34 * 10**9, 0) >= 5:
        problems.append("a window below the margin does not reach the spent floor")
    # Counterfactual: the layouts the first diagnostic refused, with the byte
    # counts the product reported. They must still exceed the bound, and every
    # one of those cases must now have a short slug.
    for case, path, size in OBSERVED_OVERLONG:
        actual = len(path.encode())
        if actual != size:
            problems.append(f"{case}: counterfactual path is {actual} bytes, evidence says {size}")
        if actual <= SOCKET_PATH_BUDGET:
            problems.append(
                f"{case}: counterfactual path {actual} does not exceed {SOCKET_PATH_BUDGET}"
            )
        if case not in PRUNE_SLUGS:
            problems.append(f"{case}: no short prune slug is declared")
    # Every prune case's current naming must fit with margin, for many draws of
    # the random suffix.
    for case in PRUNE_SLUGS:
        worst = 0
        for _ in range(64):
            path, size = prune_temp_budget(prune_volume_name(case))
            worst = max(worst, size)
            if size > SOCKET_PATH_BUDGET:
                problems.append(f"{case}: current layout is {size} bytes ({path})")
                break
        print(f"  {case:<26} worst temp path {worst} bytes of {SOCKET_PATH_BUDGET}")
    # The retired pre-#190 cases are declared nowhere (decision 6818efde), and
    # nothing in the proof still reaches for the capture journal or the pinned
    # base binary: the case table, the race map and the dispatch are all checked
    # by name, and the retired helpers no longer exist.
    for retired in RETIRED_CASES:
        if retired in CASES or retired in RACES:
            problems.append(f"{retired}: a retired case is still declared")
    if [case for case in CASES if case.startswith("lease_")] != list(LEASE_CASES):
        problems.append("the lease half is not the declared holding-model case set")
    for retired_name in ("BASE_COMMIT", "base_source", "verified_capture_name",
                         "old_counterfactual", "upgraded_counterfactual_ledger"):
        if retired_name in globals():
            problems.append(f"a retired pre-#190 mechanism survived: {retired_name}")
    for case in LEASE_CASES:
        lease_env = fixture_environment(
            "/fixture/run/cases/" + case + "/0badc0de", case, RACES[case], {}
        )
        if [key for key in lease_env if key.startswith("OXIDE_TEST_CAPTURE_")]:
            problems.append(f"{case}: the retired capture-unlink seam is still armed")
        if "OXIDE_TEST_BUDGET_BYTES" in lease_env:
            problems.append(f"{case}: the retired budget override is still armed")
    # No lease case arms the race injection: oxide#190's eviction has no seam,
    # and a lease case that armed one would be claiming a firing it cannot get.
    for case in LEASE_CASES:
        if RACES[case] is not None:
            problems.append(f"{case}: a lease case still arms the prune-only race injection")
    for case in CASES:
        if case.startswith("prune_") and RACES[case] is None:
            problems.append(f"{case}: a prune case arms no race")
    # The holding model the lease half asserts, as executed structure: one record
    # per pending deletion, presence read from the payload's private area, and
    # the two fixture states the case bodies plant.
    probe_env = {"OXIDE_POOL": "/nonexistent/pool"}
    try:
        ledger_document(probe_env)
    except Unknown:
        pass
    else:
        problems.append("an absent pool ledger was accepted as journal state")
    if holding_trees(probe_env) != {}:
        problems.append("an absent holding area was not read as no held entry")
    # Every lease case must have its own fixture branch: a case dispatched into
    # `plant` without one would fail only at a native head.
    planted_source = inspect.getsource(plant)
    for case in LEASE_CASES:
        if f'"{case}"' not in planted_source:
            problems.append(f"{case}: plant() declares no fixture branch")
    if HOLDING_NAME != ".oxide-holding" or os.path.basename(HOLDING_NAME) != HOLDING_NAME:
        problems.append("the holding area name drifted from the product's payload entry")
    if set(PRUNE_SLUGS) - set(CASES):
        problems.append(f"slugs declared for unknown cases: {sorted(set(PRUNE_SLUGS) - set(CASES))}")
    if {case for case in CASES if case.startswith("prune_")} != set(PRUNE_SLUGS):
        problems.append("every prune case needs exactly one slug")
    # The settle dispatch, executed for real: every case's handler must accept
    # the exact bundle the dispatch site builds, and the completion-fault case
    # must route to its own handler. An arity or routing drift fails here
    # instead of raising TypeError inside a native case.
    probe = Settle(
        oxide="oxide",
        env={},
        base="/nonexistent",
        case="prune_completion_fault",
        mark=0,
        low_mark=0,
        real=None,
        outputs="/nonexistent/out",
        root_path="/nonexistent/root",
        members=[],
        quarantined=[],
        kept_root=False,
        plan="/nonexistent/plan.json",
        digest="0" * 64,
    )
    for case in CASES:
        handler = settle_target(case)
        try:
            inspect.signature(handler).bind(probe)
        except TypeError as error:
            problems.append(f"{case}: the settle handler rejects the dispatch bundle ({error})")
    if settle_target("prune_completion_fault") is not settle_completion_fault:
        problems.append("the completion-fault case does not route to its own handler")
    if settle_target("prune_metadata") is not settle_case:
        problems.append("an ordinary prune case does not route to the shared settle handler")
    for handler, label in (
        (settle_case, "settle_case"),
        (settle_completion_fault, "settle_completion_fault"),
    ):
        if "replay_prune(" not in inspect.getsource(handler):
            problems.append(f"{label} does not route its replay through replay_prune")
    # Execute the replay body itself against a fake runner, so a name or shape
    # drift there fails a pure check instead of a native case.
    seen = []
    real_runner = globals()["run"]

    def fake_runner(command, *, cwd, env, timeout=120.0):
        seen.append((list(command), cwd))
        return type("Done", (), {"returncode": 0, "stdout": "", "stderr": ""})()

    globals()["run"] = fake_runner
    try:
        outcome = replay_prune("oxide", {}, "/nonexistent", "plan.json", "0" * 64)
    finally:
        globals()["run"] = real_runner
    expected = (
        [
            "oxide",
            "outputs",
            "prune",
            "--plan",
            "plan.json",
            "--plan-sha256",
            "0" * 64,
        ],
        "/nonexistent",
    )
    if outcome.returncode != 0 or seen != [expected]:
        problems.append(f"the replay invocation drifted: {seen!r}")
    print("  replay dispatch: both settle handlers share one executed replay body")
    print(f"  settle dispatch: {len(CASES)} cases bind one {Settle.__name__} bundle")
    print(
        f"  scratch pools: one declared root per case, the pool strictly inside it, "
        f"{len(SELECTOR_RECIPE)} selectors on the selected-volume recipe"
    )
    print(f"  current-main oracles: {len(ORACLE_PINS)} product values still hold")
    # An overlong volume name must be refused, not attached.
    try:
        assert_prune_path_budget("counterfactual", "oxide155-" + "x" * 40)
        problems.append("an overlong volume name was accepted")
    except Recoverable:
        pass
    if problems:
        for problem in problems:
            print(f"self-check: {problem}", file=sys.stderr)
        return 1
    print("self-check: fixture layout budget holds, old layouts still refused, "
          "scratch pool contract holds")
    return 0


def unsettled_intent(env: dict, outputs: str) -> dict:
    """The single unfinished prune intent whose quarantine is under `outputs`."""
    _, document = registry(env)
    prefix = os.path.join(outputs, ".prune-")
    intents = [
        intent
        for intent in document.get("intents") or []
        if isinstance(intent, dict)
        and isinstance(intent.get("quarantine"), str)
        and intent["quarantine"].startswith(prefix)
    ]
    if len(intents) != 1:
        fail(f"the registry holds {len(intents)} unfinished intents under {outputs}, expected one")
    return intents[0]


def settle_completion_fault(settle: Settle) -> str:
    oxide = settle.oxide
    env = settle.env
    base = settle.base
    case = settle.case
    mark = settle.mark
    real = settle.real
    outputs = settle.outputs
    plan = settle.plan
    digest = settle.digest
    """The pre-completion fault: the leaf and the registry terminal write commit,
    the checkout receipt does not settle, and a *clean* replay settles it.

    The product's terminal crash point is one-shot only in the sense that it
    fires whenever its environment variable is set, so the replay runs without
    it: replaying the same one-shot injection would just crash again and prove
    nothing about the receipt step.
    """
    if real.returncode != 3:
        fail(
            f"{case}: the pre-completion fault did not exit 3 "
            f"(exit {real.returncode}: {real.stderr[-1500:]})"
        )
    leaf_dir = [
        entry for entry in sorted(os.listdir(outputs)) if entry.startswith(".prune-")
    ] if os.path.isdir(outputs) else []
    if leaf_dir:
        fail(f"{case}: the pre-completion fault left a quarantined leaf: {leaf_dir}")
    intent = unsettled_intent(env, outputs)
    phase = intent.get("phase")
    if phase != "deleted":
        fail(f"{case}: expected the journal phase `deleted`, got {phase!r}")
    if intent.get("receipt_settled") is not False:
        fail(f"{case}: the intent claims a settled receipt: {intent.get('receipt_settled')!r}")
    leaf = intent["quarantine"]
    if os.path.exists(leaf):
        fail(f"{case}: the recorded quarantine path still exists: {leaf}")

    replay_env = dict(env)
    replay_env.pop("OXIDE_TEST_PRUNE_CRASH", None)
    replay = replay_prune(oxide, replay_env, base, plan, digest)
    if replay.returncode:
        fail(f"{case}: the receipt-step replay did not settle: {replay.stderr[-1500:]}")
    _, document = registry(env)
    if json.dumps(document.get("intents") or []).find(leaf) >= 0:
        fail(f"{case}: the settled intent is still in the live registry")
    require_fired(case, mark, "Metadata")
    return f"{case}: pending pre-completion (phase deleted, receipt unsettled), clean replay settled it"


def plan() -> int:
    print("cases:")
    for case in CASES:
        race = RACES[case]
        print(f"  {case:<26} race {race or 'none (control)'}")
    print(
        f"allocation: {sum(1 for c in CASES if c.startswith('lease_'))} 1 MiB "
        "directory-budget scratch pools, one per case, each declared with its own "
        "OXIDE_SCRATCH_ROOT inside the case's run root and an explicit "
        f"OXIDE_POOL_CAP, {len(PRUNE_SLUGS)} owned 128 MiB sparse images with custody "
        "records in the guard's evidence directory (one attached at a time and "
        "ordinarily detached), one private HOME/CARGO_HOME and TMPDIR per case, "
        "staged debug oxide in owned scratch"
    )
    print("prune fixture layout (product Unix-socket bound %d bytes):" % SOCKET_PATH_BUDGET)
    for case in CASES:
        if case in PRUNE_SLUGS:
            name = f"o155-{PRUNE_SLUGS[case]}-xxxx"
            path, size = prune_temp_budget(name)
            print(f"  {case:<26} volume {name:<16} worst temp path {size} bytes")
    print("premises: 9 traced in this script's docstring")
    print("diagnostic: recoverable failures continue; unknown custody/device/writer/deadline aborts")
    return 0


def write_receipt(state: dict) -> str:
    """One bounded `qualified: false` receipt for every diagnostic outcome,
    including a setup or staging failure before any case ran."""
    receipt = {
        "proof": "oxide#155-finder-metadata-race",
        "qualified": False,
        "mode": "diagnostic",
        "phase": state["phase"],
        "reached": state["reached"],
        "completed": state["completed"],
        "failed": state["failed"],
        "skipped": state["skipped"],
        "unexecuted": state["unexecuted"],
        "reasons": state["reasons"],
        "first_fault": state["first_fault"],
        "aborted": state["abort"] is not None,
        "abort": state["abort"],
    }
    path = os.path.join(evidence_dir(), "finder-metadata-race-diagnostic.json")
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(receipt, handle, indent=2, sort_keys=True)
        handle.write("\n")
    return path


def main(argv: list) -> int:
    if argv == ["--plan"]:
        return plan()
    if argv == ["--self-check"]:
        return self_check()
    diagnostic = False
    if argv:
        if argv != ["--diagnostic"]:
            fail(f"unknown argument {argv[0]!r}; known: --plan, --self-check, --diagnostic")
        diagnostic = True

    state = {
        "phase": "setup",
        "reached": [],
        "completed": [],
        "failed": [],
        "skipped": [],
        "unexecuted": list(CASES),
        "reasons": {},
        "first_fault": None,
        "abort": None,
    }

    def finish(code: int) -> int:
        if not diagnostic:
            return code
        try:
            receipt = write_receipt(state)
        except Exception as error:  # noqa: BLE001 - a missing receipt is reported
            # An unwritable receipt aborts the collection explicitly instead of
            # reporting a diagnostic that does not exist.
            print(
                f"diagnostic: the receipt could not be written ({error!r}); "
                "the collection is aborted and reports nothing",
                file=sys.stderr,
                flush=True,
            )
            return 1
        print(
            f"diagnostic: qualified=false, phase {state['phase']}, reached "
            f"{len(state['reached'])}, completed {len(state['completed'])}, failed "
            f"{len(state['failed'])}, skipped {len(state['skipped'])}, unexecuted "
            f"{len(state['unexecuted'])}, aborted {state['abort'] is not None}, "
            f"first fault {state['first_fault']!r}, receipt {receipt}",
            flush=True,
        )
        return code

    # Setup and staging are inside the receipt lifecycle: a failure there is a
    # recorded diagnostic outcome, never an unrecorded exit.
    try:
        root = scratch_root()
        evidence_dir()
        work_left()
        oxide = stage(root, REPO, "candidate")

        def digest(path):
            value = hashlib.sha256()
            with open(path, "rb") as handle:
                for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                    value.update(chunk)
            return value.hexdigest()

        print(f"candidate sha256 {digest(oxide)}", flush=True)
    except (Unknown, Recoverable) as error:
        state["first_fault"] = f"setup: {type(error).__name__}: {error}"
        if isinstance(error, Unknown):
            state["abort"] = state["first_fault"]
        return finish(1)
    except (OSError, subprocess.TimeoutExpired) as error:
        # Setup or staging raised outside the classified paths: its closure is
        # unknown, so it is recorded as an abort, never as an oracle failure.
        state["first_fault"] = f"setup: {type(error).__name__}: {error}"
        state["abort"] = state["first_fault"]
        return finish(1)
    print(f"staged oxide from this HEAD: {oxide}", flush=True)
    state["phase"] = "body"

    for index, case in enumerate(CASES):
        state["reached"].append(case)
        try:
            line = (
                lease_case(oxide, root, case, RACES[case])
                if case.startswith("lease_")
                else prune_case(oxide, root, case, RACES[case])
            )
            state["completed"].append(case)
            print(line, flush=True)
        except Unknown as error:
            text = f"{case}: {type(error).__name__}: {error}"
            state["reasons"][case] = text
            state["first_fault"] = state["first_fault"] or text
            state["abort"] = text
            state["skipped"] = list(CASES[index + 1 :])
            break
        except Recoverable as error:
            text = f"{case}: {type(error).__name__}: {error}"
            state["reasons"][case] = text
            state["first_fault"] = state["first_fault"] or text
            if not diagnostic:
                raise
            state["failed"].append(case)
            print(f"{case}: FAILED {text}", flush=True)
        except Exception as error:  # noqa: BLE001 - unknown outcome, never continued
            text = f"{case}: unexpected {type(error).__name__}: {error}"
            state["reasons"][case] = text
            state["first_fault"] = state["first_fault"] or text
            state["abort"] = text
            state["skipped"] = list(CASES[index + 1 :])
            break

    state["unexecuted"] = [case for case in CASES if case not in state["reached"]]
    if state["abort"] is not None and not diagnostic:
        raise Unknown(state["abort"])
    if state["failed"] and not diagnostic:
        raise Recoverable(state["first_fault"])
    if diagnostic:
        return finish(0 if not state["failed"] and state["abort"] is None else 1)
    print(f"proof: {len(state['completed'])} cases passed", flush=True)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except Unknown as error:
        print(f"proof aborted (unknown outcome, nothing continued): {error}", file=sys.stderr, flush=True)
        sys.exit(1)
    except Recoverable as error:
        print(f"proof failed: {error}", file=sys.stderr, flush=True)
        sys.exit(1)
    except Exception as error:  # noqa: BLE001 - one bounded failure line
        print(f"proof failed (unexpected): {type(error).__name__}: {error}", file=sys.stderr, flush=True)
        sys.exit(1)
