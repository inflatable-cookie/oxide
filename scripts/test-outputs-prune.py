#!/usr/bin/env python3
"""Owned-output prune proof (oxide#129, oxide#152): the fixed 22-case script.

Run only as an Effigy heavy task under the qualified shared supervisor
(`scripts/bounded-test.py`, SHA-256
bc24b238a2ec6f0a68719f59b14bce2d69df0e30c2a351366fdab07db4975924, the
oxide#151 supervisor merged from main af08749c with its exact qualification
files — bootstrap `scripts/qualify-supervisor.py` 585e33956b0e9b0cce2d45bc85726e1202952c1b500d64da6fa630ade78e4a95
and suite `scripts/test_supervisor.py` 37ea34f049341b28d7ca2ba4e87dd666527a0146d249e307f5c78b3461ccc9c7;
earlier declarations named 5e5b6617 and d89115f1, which are historical):

  rust:outputs-prune             the counted proof (fail-fast)
  rust:outputs-prune:diagnostic  the same script with `--diagnostic`
  proof:outputs-prune:test       this script's pure `--self-test` mode

Both heavy selectors are
`python3 scripts/bounded-test.py --root-parent /private/tmp --limit 360
--scratch-budget-mib 512 -- python3 scripts/test-outputs-prune.py
[--diagnostic]`. `--root-parent /private/tmp` is the owner-accepted
internal-placement exception (decision bd357a4c v4): cases 10 and 21 prove
checkout absence on the boot Data volume, and the fixture's directory pool is
a genuine internal pool that the product correctly refuses on an external
volume. The supervisor monitors that same root, owns the 360 s deadline
(root-approved one larger bound; the historical 300 s estimate is retained),
the 512 MiB monitored scratch, process closure and cleanup; work stops 30 s
before its exported deadline, and every case start and the ordinary detach
check the remaining work window first. `--diagnostic` is an UNCOUNTED
keep-going collection that writes a bounded `qualified: false` receipt and is
never a PASS; only ROOT activates it, and only after it does ROOT activate
one counted author run. `--plan` and `--self-test` touch nothing.

Static allocation (one 128 MiB owned APFS sparse image, private HOME,
CARGO_HOME and directory pool; no real Builds drive, consumer, Windows,
provider or install):

    staging (constrained debug oxide recipe)      288 MiB
    APFS sparse image (payload + quotas)          128 MiB
    fixtures (incl. ~2.7 MiB old-binary copy)      32 MiB
    evidence                                       32 MiB
    margin                                         32 MiB
    ---------------------------------------------  ------
    total                                         512 MiB

Staging is the rust:installed-handoff recipe (this HEAD's debug `oxide`,
plain Cargo offline with 1.97.1, -j1, one codegen unit, no debug info,
symbols stripped, opt-level s, debug assertions kept so the debug-only
OXIDE_TEST_PRUNE_CRASH and OXIDE_TEST_PAUSE hooks exist; the staged target is
deleted after the copy).

Static time allocation, UNMEASURED (a successful run must finish everything,
including the ordinary detach, by the exported deadline minus 30 s, with the
detach helper's 5 s refusal margin: about 325 s of the 360 s limit):

    supervisor start offset                         <= 5 s
    staging (constrained debug oxide recipe)       37-60 s
    image attach, registration, first build        10-20 s
    old-binary verify/copy, probe, two refusals     2-5 s
    cases 01-22 (base 60-120 s + source-fit
      additions: gated writer build, kappa and
      recreated builds, image checkout, 600-entry
      root, extra prune runs, status reads, the
      version-1 fixture and its runs)            100-175 s
    ordinary detach                                 2-10 s
    ---------------------------------------------  --------
    total                                        156-270 s   (~60 s margin)

No timing fit is claimed. An overrun fails at the deadline (never a timeout
success); no split, extra session or automatic retry.

Prune-worker monitoring call chain exercised by the deletion cases (product
source, not a bespoke harness): prune::prune_one -> Store::begin_clean ->
lock_binding -> recheck_candidate/recheck_receiptless -> RootSession::open
(removable::Binding::open + UUID, device, mount; registration) ->
RootSession::check(control, deleted) [internal control volume host reserve,
Binding::verify, registered reserve; unknown readings stop] before the
rename -> journal_intent -> rename -> finish_quarantine: Deleter::check
before the first and after the last delete and before the quarantine leaf
removal, Deleter::check after each directory's names are sorted,
check_due (>= 1 s, watchdog INTERVAL) before each enumeration, every 512
names while listing and after each unlink, and a forced check every 512
deleted entries. Any stop leaves the intent pending (exit 3). The debug-only
injections OXIDE_TEST_PRUNE_LOW_RESERVE_AT / _AFTER_ENTRIES drive the
pre-rename refusal and the mid-batch stop.

Cases (22 = the original 19-case matrix plus two resolve-intent cases and the
oxide#152 version-1 receipt scenario), in execution order, each on tiny owned
fixtures with exact exit codes and state assertions. This list is the proof's
own machine-checked matrix: `--self-test` fails if it drifts from
CASE_PLAN/CASE_NAMES, and the dependency column is exactly what a
`--diagnostic` run uses to decide which cases may still run.

  01 init: adopt publishes the current control format (state version 4 with
     the `oxide.outputs/3` owned-outputs fence and a ready transaction in both
     controls) and the current registry document is exactly
     `oxide.outputs.registry/2` with schema 2 and `complete: true` — the pair
     this head writes, checked against the frozen `prune.rs` literals by the
     pure selector; a legacy 1/1 document is readable by the product but is
     not this proof's expected current write. Older-binary control
     (root-authorized read-only artifact): the exact released d246d400
     control-2 binary is read from its durable release-audit copy
     (`OLD_ARTIFACT_PATH`, recovered and independently hash/size verified)
     together with its original install receipt, and verified (pin, SHA-256
     bd52cb42..., size 2756576, no-follow regular owned files, metadata stable
     across the read). The receipt's recorded `installed.path` is the
     historical `/Users/tom/.local/bin/oxide`, which now holds the current
     4df3dfd binary: it is provenance text only and is never opened. Only a
     private copy inside owned scratch runs: its capability probe reports
     control state 2, and its `check` and `outputs adopt` in the fixture both
     exit nonzero with no handoff, no host path and complete pool, volume-tree
     and checkout bytes and metadata unchanged. The version-1 fixture for case
     21 is written here, before the registry exists, so the product's own
     receipt seed records its legacy rows. Its image's `oxide-outputs`
     container is created first, exclusively and 0700, before any nested
     version-1 root: nested-first creation would make Python create that
     intermediate 0755 and the product's owner-only check would correctly
     refuse it (the diagnostic-1 finding this correction fixes). An unexpected
     pre-existing container refuses rather than being adopted or chmodded.
  02 missing ready registry: dry run exits 2, adopt of another checkout
     refuses, nothing is rebuilt; restoring the exact bytes recovers.
  03 root nonce: the external root's v2 marker nonce (32 hex) equals the
     receipt's selected nonce and every placed member's registry row nonce
     (a current profile receipt records one row per member for one root).
  04 shared reservation: an ELIGIBLE reservation path (a committed profile
     receipt whose placed members are external on the healthy registered
     volume, with the selected root leaf gone, so the fresh selection is still
     external and `reserve` publishes a fresh nonce for the missing root before
     the slot key): two starters paused after slot keying (their captured
     destination keys compared before release) share one reserved nonce; both
     bind it and the recreated root carries it. The reservation is read from
     the receipt's ROOT-LEVEL `reserved` field. A NON-RESERVING control is kept
     inside the same case: with the registered volume made unusable the fresh
     selection is local against committed external members, which is binding's
     P4 precondition, so the command publishes no reservation, changes no
     receipt byte and the prune preview keeps the root.
  05 cancel + stale starter: on the same eligible path, a starter paused after
     keying has its real reservation cancelled by `outputs cancel-reservation`;
     the starter refuses before Cargo with "cancelled or replaced"; the root is
     never created; the row is gone.
  06 dry run: canonical preview bytes on stdout match the SHA-256 on stderr,
     schema prune-preview/1, control bytes unchanged.
  07 malformed receipt: one corrupt receipt keeps the whole batch.
  08 eligibility: a live checkout is kept; a live (present) detached
     checkout's root is kept; the same released root is a candidate once its
     checkout is gone; a registered checkout on the attached image is a
     candidate when gone (absence through the verified mount and recorded
     UUID).
  09 unreadable ancestor kept: a debug-only EACCES (OXIDE_TEST_PRUNE_EACCES,
     bound to the supervisor's scratch/evidence layout) keeps the root, its
     firing is recorded in evidence, and the same fixture without injection is
     a genuine-absence candidate; no real unreadable directory exists. A
     listed relative path and a duplicate refuse with exit 2.
  10 receiptless legacy roots: the one whose checkout is on the boot Data
     volume (device + APFS UUID proof) gets its exclusive pruning receipt and
     is deleted (receipt `pruned`/`deleted`); the one whose checkout is on the
     attached scratch image, a foreign directory and a marker whose checkout
     identity is not its path digest are kept, bytes unchanged.
  11 active writer: a build script holds Cargo Running in a vanished checkout;
     its root is kept with a writer reason.
  12 containment and reserve: a root replaced by a symlink is kept; a symlink
     descendant stops deletion (exit 3, target untouched) and replay
     completes; an injected low reserve at the pre-rename check refuses (exit
     2, root bytes unchanged, nothing journaled); one between delete batches of
     a 600-entry root stops (exit 3) with some but not all entries deleted, and
     replay completes.
  13 crash after intent: exit 3, root in place; replay rolls back and deletes.
  14 crash after rename: exit 3, quarantine present; a recreated checkout's
     build refuses while the intent is unsettled; replay completes.
  15 crash during delete: exit 3; replay completes.
  16 crash after registry terminal: exit 3, receipt not yet pruned; replay
     settles the receipt.
  17 completion: deleted root, registry row and intent gone, tombstone,
     prune-observations recorded; logical bytes equal an independent by-name
     oracle and allocated bytes equal an independent unique-inode oracle (a
     hard-linked 64 KiB inode counted once; complete coverage); `oxide status`
     shows the observation.
  18 recreation: a pruned checkout re-created at the same path adopts a fresh
     root nonce different from the deleted root's recorded nonce, and its first
     build keys a different, cold slot.
  19 resolve-intent with both leaves absent: resolution receipt,
     `resolved-by-operator`.
  20 resolve-intent refuses while the quarantine is present: exit 2, intent
     unchanged; replay then completes.
  21 version-1 receipt compatibility (oxide#152): a whole-target
     `oxide.outputs.transition/1` receipt and its `root/1` root (written before
     the registry existed, so the product's own receipt seed recorded a legacy
     row with no nonce); the live checkout keeps it; a valid NONCOMMITTED
     version-1 receipt (one committed root plus a prepared transition) keeps
     its own reference group without poisoning the batch; a dry run changes no
     receipt, root or control byte; the exact preview bytes and the same
     operator list prune only the committed legacy root and publish the current
     receipt shape with its recorded terminal outcome; the kept root's
     version-1 receipt bytes never change. It shares case 10's boot-Data-volume
     absence proof: on a host whose supervisor scratch is not on the boot Data
     volume the legacy root stays kept and this case fails loudly rather than
     asserting a false pass.
  22 overflow: a sticky-incomplete registry keeps every root and a new
     reservation carries an overflow tag naming the registry; the starter binds
     it with no invented row, on the same eligible path (a committed external
     root whose leaf is gone). Run last: the flag is never cleared.

Dependency matrix (id, declared dependencies, taints fixture state on failure):

    01  deps -  no taint
    02  deps 01  taints
    03  deps 01  no taint
    04  deps 01,02  taints
    05  deps 01,04  taints
    06  deps 01  no taint
    07  deps 01,06  taints
    08  deps 01,07  no taint
    09  deps 01,08  no taint
    10  deps 01,09  taints
    11  deps 01,10  taints
    12  deps 01,10  taints
    13  deps 01,12  taints
    14  deps 01,12  taints
    15  deps 01,12  taints
    16  deps 01,12  taints
    17  deps 01,12,16  taints
    18  deps 01,17  no taint
    19  deps 01,18  taints
    20  deps 01,19  taints
    21  deps 01,20  taints
    22  deps 01,21  taints

Receipts from runs before this matrix was published used a different thematic
numbering; they stay historical and their ids are never read as the matrix
above.

Nine-premise sweep (source fit, UNMEASURED): private `HOME`/`CARGO_HOME` with
the invoking `RUSTUP_HOME`, `RUSTUP_AUTO_INSTALL=0` and `OXIDE_TEST_HOME_FROM_ENV=1`;
`OXIDE_*`/`XDG_*`/`CARGO_*`/`GIT_*` scrubbed by `proof_env`; `CARGO_ENCODED_RUSTFLAGS`
removed so the declared `RUSTFLAGS` are the only ones in force; the owned image
attached with `hdiutil -owners on`; one ordinary detach with unknown custody
left ambiguous; `mktemp` always with a template under owned scratch. Automatic
adoption is not assumed: every converted checkout is adopted explicitly and
each receipt state is asserted. No fake Cargo subcommand is used.

Not run here: real-drive qualification, live inventory or cleanup. Case 01's
older-binary fence control reads the recovered, independently hash/size
verified artifact at
/Users/tom/.local/share/northstar/release-audits/oxide/2026-10-25/d246d400-artifact/old-oxide-d246d400
(sha256 bd52cb42…, 2756576 bytes, pin d246d400, control state 2) beside its
provenance-only custody JSON, together with the original receipt
/Users/tom/.local/state/oxide/install/20261023T095314Z-d246d400f5cc.json. The
receipt's `installed.path` /Users/tom/.local/bin/oxide is historical: it now
holds the current 4df3dfd binary, which this proof never reads or copies. A
different identity at either input aborts naming the requirement instead of
retargeting, stubbing or rebuilding anything.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import ast
import glob
import hashlib
import inspect
import importlib.util
import json
import os
import plistlib
import re
import secrets
import shutil
import signal
import stat
import subprocess
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))


def _load(name: str, file: str):
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, file))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SUPERVISOR = "bc24b238a2ec6f0a68719f59b14bce2d69df0e30c2a351366fdab07db4975924"
LIMIT_SECONDS = 360
SCRATCH_MIB = 512
CASES = 22

ALLOCATION = [
    ("staging (constrained debug oxide)", 288),
    ("APFS sparse image", 128),
    ("fixtures", 32),
    ("evidence", 32),
    ("margin", 32),
]

CASE_ALLOCATION = [
    ("forward fence + initialization", 2),
    ("nonce / reservation / cancel / recreation key", 4),
    ("registry / snapshot / overflow", 3),
    ("eligibility / operator list / absence", 3),
    ("custody / containment / removal", 2),
    ("crash boundaries", 4),
    ("completion / diagnostic / logical-allocated", 1),
    ("resolve-intent", 2),
    ("version-1 receipt compatibility", 1),
]

PLAIN = '[package]\nname = "plain"\nversion = "0.1.0"\nedition = "2021"\n'

# The older-binary fence control (Opus 9a6af8bb, root-authorized READ-ONLY):
# the exact released d246d400 control-2 artifact and its original install
# receipt. Two SEPARATE paths, deliberately not interchangeable:
#
#   OLD_INSTALLED_PATH  the historical installation path the original receipt
#                       records as `installed.path`. It is provenance text
#                       only: that path now holds the current 4df3dfd binary,
#                       which this proof must NEVER read or copy.
#   OLD_ARTIFACT_PATH   the durable, independently hash/size-verified copy of
#                       the recovered artifact, which is the actual proof
#                       input. It is read with a bounded no-follow read, its
#                       identity is rechecked before and after, and only a
#                       private copy inside owned scratch is ever executed.
#
# The custody JSON beside the durable copy is provenance only; this proof
# trusts nothing in it and requires the ORIGINAL receipt's pin, SHA-256 and
# size plus the read bytes' own SHA-256.
OLD_INSTALLED_PATH = "/Users/tom/.local/bin/oxide"
OLD_ARTIFACT_PATH = (
    "/Users/tom/.local/share/northstar/release-audits/oxide/2026-10-25/"
    "d246d400-artifact/old-oxide-d246d400"
)
OLD_RECEIPT = "/Users/tom/.local/state/oxide/install/20261023T095314Z-d246d400f5cc.json"
OLD_PIN = "d246d400f5ccb8e0a601ea6415b93b8346bc2698"
OLD_SHA = "bd52cb42842c06451b0f8a9bc364ba7de74cfe6f014d33804890c4adbc51fc2b"
OLD_SIZE = 2_756_576
OLD_BINARY_CAP = 8 * 1024 * 1024
OLD_RECEIPT_CAP = 64 * 1024
CAPABILITIES_PROBE = "__oxide-build-capabilities"
# The control state and fence this proof's head writes (src/bin/oxide/store.rs
# STATE_VERSION / OWNED_OUTPUTS_FENCE) and the exact capability probe the staged
# candidate must report. A bump is a deliberate proof correction, never a
# silently accepted drift.
CONTROL_STATE_VERSION = 4
OWNED_OUTPUTS_FENCE = "oxide.outputs/3"
# The registry document this head WRITES (prune.rs REGISTRY_VERSION /
# REGISTRY_SCHEMA): initialization creates 2/2 and a profile receipt promotes a
# legacy 1/1 document to 2/2, so 2/2 is the current-write format case 01 must
# see. The legacy 1/1 document stays readable by the product, but it is a
# different (read-only) truth and is deliberately NOT accepted here.
REGISTRY_VERSION = "oxide.outputs.registry/2"
REGISTRY_SCHEMA = 2
CANDIDATE_CAPABILITY = {
    "control_state_version": CONTROL_STATE_VERSION,
    "registration_version": 2,
    "test_controls": True,
}
HANDOFF_NOTICE = "handing off to the installed binary"
# A build script that holds Cargo running until its gate releases it
# (bounded at 60 s), so a writer is positively Running, not pre-launch.
GATED_BUILD = r"""
use std::{env, fs, path::Path, thread, time::{Duration, Instant}};
fn main() {
    println!("cargo:rerun-if-env-changed=PRUNE_GATE");
    let Ok(gate) = env::var("PRUNE_GATE") else { return };
    let _ = fs::write(Path::new(&gate).join("started"), b"");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !Path::new(&gate).join("release").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
}
"""
DONE: list[str] = []


def plan() -> int:
    print("oxide#129 owned-output prune proof plan")
    print(f"supervisor bounded-test.py sha256 {SUPERVISOR}")
    print(f"wall limit {LIMIT_SECONDS} s; monitored owned scratch {SCRATCH_MIB} MiB")
    print("static allocation:")
    total = 0
    for label, mib in ALLOCATION:
        total += mib
        print(f"  {label:<42} {mib:>4} MiB")
    print(f"  {'total':<42} {total:>4} MiB")
    print(f"cases ({CASES}):")
    for label, count in CASE_ALLOCATION:
        print(f"  {count:>2}  {label}")
    assert sum(count for _, count in CASE_ALLOCATION) == CASES
    assert total == SCRATCH_MIB
    print(f"diagnostic selector: rust:outputs-prune:diagnostic (uncounted, qualified=false)")
    print("pure apparatus selector: proof:outputs-prune:test (--self-test, no child)")
    print("activation: separate counted owner gate per side; --plan touches nothing")
    return 0


# Loaded only for a real run, so `--plan` imports nothing that executes.
permanent = None
handoff = None
expect = fail = None


# ---------------------------------------------------------------------------
# Case dispatch: counted fail-fast (the default) or the opt-in keep-going
# diagnostic (`--diagnostic`, an uncounted collection that is never a PASS).
# Both modes share this one table and dispatcher, so the diagnostic cannot
# drift from the counted case order. The dispatcher adds no second supervisor,
# session, process group or accounting harness: it only decides whether the
# next case body inside this one run is executed, skipped or abandoned.
# ---------------------------------------------------------------------------

# (case id, dependency ids, failure leaves fixture state uncertain). A case
# that moves the registry aside, leaves a reservation, writer or journal
# behind, or rewrites the control documents taints the fixture, so a
# keep-going run skips every later case rather than trusting it.
CASE_PLAN = [
    ("01", (), False),
    ("02", ("01",), True),
    ("03", ("01",), False),
    ("04", ("01", "02"), True),
    ("05", ("01", "04"), True),
    ("06", ("01",), False),
    ("07", ("01", "06"), True),
    ("08", ("01", "07"), False),
    ("09", ("01", "08"), False),
    ("10", ("01", "09"), True),
    ("11", ("01", "10"), True),
    ("12", ("01", "10"), True),
    ("13", ("01", "12"), True),
    ("14", ("01", "12"), True),
    ("15", ("01", "12"), True),
    ("16", ("01", "12"), True),
    ("17", ("01", "12", "16"), True),
    ("18", ("01", "17"), False),
    ("19", ("01", "18"), True),
    ("20", ("01", "19"), True),
    ("21", ("01", "20"), True),
    ("22", ("01", "21"), True),
]
# The four crash boundaries run from one loop; each iteration is its own case.
CRASH_CASES = {"intent": "13", "renamed": "14", "deleting": "15", "terminal": "16"}
CASE_NAMES = [
    "init: forward fence and ready registry",
    "missing ready registry refuses and is never rebuilt",
    "root nonce in marker, receipt and registry",
    "identical starters share one reserved future-root nonce",
    "cancelled reservation: the stale starter refuses before Cargo",
    "dry run: canonical preview, digest, no mutation",
    "malformed receipt keeps the whole batch",
    "live checkout and live detached root kept",
    "injected EACCES ancestor kept; genuine absence is a candidate; bad lists refuse",
    "receiptless legacy root: exclusive pruning receipt, deleted",
    "active writer keeps a vanished checkout's root",
    "symlink root kept; symlink descendant stops deletion",
    "crash after intent: pending, then exact replay",
    "crash after rename: pending, then exact replay",
    "crash during delete: pending, then exact replay",
    "crash after registry terminal: pending, then exact replay",
    "completion: terminal state, tombstone, observations, status",
    "recreated checkout adopts a fresh root nonce",
    "resolve-intent: both leaves absent",
    "resolve-intent refuses while the quarantine is present",
    "version-1 whole-target receipts: read for prune, never migrated",
    "overflow: keep every root; tagged reservation binds with no row",
]
CASE_INDEX = {entry[0]: entry for entry in CASE_PLAN}
assert len(CASE_PLAN) == len(CASE_NAMES) == CASES
assert [entry[0] for entry in CASE_PLAN] == [f"{index:02d}" for index in range(1, CASES + 1)]
for _id, _deps, _taints in CASE_PLAN:
    assert all(dep in CASE_INDEX and dep < _id for dep in _deps), (_id, _deps)
DIAGNOSTIC_RECEIPT_VERSION = "oxide.outputs-prune-diagnostic/1"
MAX_DIAGNOSTIC_RECEIPT_BYTES = 32 * 1024
MAX_FAULT_TEXT = 400


class Aborted(Exception):
    """A non-continuable outcome: custody, device, protocol, deadline or a
    receipt/registry outcome that is not a case's own oracle."""


class Skipped(Exception):
    """The case body was not executed because a dependency failed or the
    fixture is tainted; recorded, never counted as a pass."""


def aborting(label: str, call):
    """Run one custody/device/protocol/receipt seam so that its failure
    aborts a keep-going run instead of being read as an oracle failure."""
    try:
        return call()
    except Aborted:
        raise
    except (KeyboardInterrupt, SystemExit):
        raise
    except BaseException as error:
        raise Aborted(f"{label}: {type(error).__name__}: {error}") from error


class Diagnostics:
    """The case outcomes of one run, the latched first fault and the bounded
    unqualified receipt of a diagnostic collection."""

    def __init__(self, enabled: bool, source: str = "") -> None:
        self.enabled = enabled
        self.source = source
        self.states: dict[str, str] = {}
        self.details: dict[str, str] = {}
        self.first_fault: str | None = None
        self.aborted: str | None = None
        self.taint: str | None = None
        self.notes: list[str] = []

    def name(self, case_id: str) -> str:
        return CASE_NAMES[int(case_id) - 1]

    def begin(self, case_id: str) -> None:
        self.states[case_id] = "reached"
        print(f"===== case {case_id}: {self.name(case_id)} =====", flush=True)

    def blocked(self, case_id: str) -> str | None:
        if self.taint is not None:
            return f"{self.taint}, so the fixture is not trusted"
        for dep in CASE_INDEX[case_id][1]:
            if self.states.get(dep) != "completed":
                return f"dependency {dep} did not complete ({self.states.get(dep, 'unreached')})"
        return None

    def completed(self, case_id: str) -> None:
        self.states[case_id] = "completed"
        DONE.append(self.name(case_id))

    def skipped(self, case_id: str, reason: str) -> None:
        self.states[case_id] = "skipped"
        self.details[case_id] = reason[:MAX_FAULT_TEXT]

    def failed(self, case_id: str, message: str) -> None:
        text = f"case {case_id}: {message}"[:MAX_FAULT_TEXT]
        self.states[case_id] = "failed"
        self.details[case_id] = text
        if self.first_fault is None:
            self.first_fault = text
        if CASE_INDEX[case_id][2]:
            self.taint = f"case {case_id} left uncertain fixture, registry, writer or journal state"

    def abort(self, case_id: str, message: str) -> None:
        text = f"case {case_id}: {message}"[:MAX_FAULT_TEXT]
        self.states[case_id] = "aborted"
        self.details[case_id] = text
        if self.first_fault is None:
            self.first_fault = text
        if self.aborted is None:
            self.aborted = text

    def exit(self, case_id: str, kind, value) -> bool:
        """One case's outcome. Suppression is the ONLY keep-going mechanism:
        the counted run never suppresses anything, so it stays fail-fast."""
        if kind is None:
            self.completed(case_id)
            return False
        if kind is Skipped:
            return True
        if kind is Aborted:
            self.abort(case_id, str(value))
            return False
        message = str(value)
        # The exported deadline is never an oracle failure.
        if kind is RuntimeError and not message.startswith("work deadline"):
            self.failed(case_id, message)
            return self.enabled
        self.abort(case_id, f"{type(value).__name__}: {message}")
        return False

    def results(self) -> list[dict]:
        rows = []
        for case_id, _deps, _taints in CASE_PLAN:
            state = self.states.get(case_id, "unexecuted")
            row = {"id": case_id, "name": self.name(case_id), "state": state}
            if case_id in self.details:
                row["detail"] = self.details[case_id]
            rows.append(row)
        return rows

    def receipt(self) -> dict:
        by_state: dict[str, list[str]] = {}
        for row in self.results():
            by_state.setdefault(row["state"], []).append(row["id"])
        return {
            "version": DIAGNOSTIC_RECEIPT_VERSION,
            "qualified": False,
            "mode": "diagnostic",
            "source": self.source,
            "declared_supervisor": SUPERVISOR,
            "limits": {
                "seconds": LIMIT_SECONDS,
                "scratch_mib": SCRATCH_MIB,
                "shutdown_margin_s": permanent_margin(),
            },
            "cases": self.results(),
            "completed": by_state.get("completed", []),
            "failed": by_state.get("failed", []),
            "skipped": by_state.get("skipped", []),
            "aborted": by_state.get("aborted", []),
            "unexecuted": by_state.get("unexecuted", []),
            "dependencies": {cid: list(deps) for cid, deps, _ in CASE_PLAN},
            "first_fault": self.first_fault,
            "abort_reason": self.aborted,
            "taint": self.taint,
            "notes": self.notes,
            "observed_ms": int(time.time() * 1000),
        }


DIAG = Diagnostics(enabled=False)


def permanent_margin() -> int:
    """The recipe's declared cleanup margin, as the selectors declare it."""
    return 30


class _Case:
    """`with case("07") as scope, scope.run():` runs one case body, or records
    why it did not run. The inner `scope.run()` context manager raises
    `Skipped` from its own `__enter__`, which this outer manager swallows, so
    the next case in the same run is still reached."""

    def __init__(self, case_id: str) -> None:
        assert case_id in CASE_INDEX, case_id
        self.case_id = case_id

    def __enter__(self):
        DIAG.begin(self.case_id)
        try:
            # Every case starts inside the exported work window.
            permanent.work_left()
        except RuntimeError as error:
            DIAG.abort(self.case_id, str(error))
            raise Aborted(str(error)) from error
        return self

    def run(self):
        return _Skippable(self.case_id)

    def __exit__(self, kind, value, tb) -> bool:
        return DIAG.exit(self.case_id, kind, value)


class _Skippable:
    def __init__(self, case_id: str) -> None:
        self.case_id = case_id

    def __enter__(self):
        reason = DIAG.blocked(self.case_id)
        if reason is not None:
            DIAG.skipped(self.case_id, reason)
            raise Skipped(reason)
        return self

    def __exit__(self, kind, value, tb) -> bool:
        return False


def case(case_id: str) -> _Case:
    return _Case(case_id)


def registry_is_current(registry: dict) -> bool:
    """The exact current registry document this proof's head writes: version
    `oxide.outputs.registry/2` with schema 2 and `complete: true`.

    This is case 01's production oracle. It is an exact pair, never "either
    format", a mixed pair, or a value derived from the document it checks: a
    legacy 1/1 document, a mixed pair, an unknown version, a missing/invalid
    field or an incomplete registry all fail, so a product format bump fails
    this proof loudly instead of silently following it.
    """
    return (
        registry.get("version") == REGISTRY_VERSION
        and registry.get("schema") == REGISTRY_SCHEMA
        and registry.get("complete") is True
    )


EVIDENCE_ENV = "OXIDE_PROOF_EVIDENCE"
CUSTODY_VERSION = "oxide.outputs-prune.image-custody/1"


class CustodyEffects:
    """The filesystem and hdiutil effects image custody needs, so the pure
    selector can drive this real seam with in-memory stubs."""

    def stat(self, path: str):
        return os.stat(path, follow_symlinks=False)

    def realpath(self, path: str) -> str:
        return os.path.realpath(path)

    def euid(self) -> int:
        return os.geteuid()

    def exists(self, path: str) -> bool:
        return os.path.lexists(path)

    def write_record(self, path: str, text: str) -> None:
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(text)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(path, 0o600)

    def create_image(self, path: str, name: str) -> None:
        created = subprocess.run(
            ["/usr/bin/hdiutil", "create", "-size", "128m", "-fs", "APFS", "-volname", name]
            + ["-type", "SPARSE", path],
            capture_output=True,
            timeout=min(60, permanent.work_left()),
        )
        if created.returncode:
            raise RuntimeError(f"image creation failed: {created.stderr!r}")

    def attach(self, path: str) -> dict:
        attached = subprocess.run(
            ["/usr/bin/hdiutil", "attach", "-nobrowse", "-owners", "on", "-plist", path],
            capture_output=True,
            timeout=min(60, permanent.work_left()),
        )
        if attached.returncode:
            raise RuntimeError(f"attach failed; attachment unresolved, evidence kept: {attached.stderr!r}")
        entities = plistlib.loads(attached.stdout)["system-entities"]
        mounts = [entity["mount-point"] for entity in entities if entity.get("mount-point")]
        devices = [entity["dev-entry"] for entity in entities if entity.get("dev-entry")]
        if len(mounts) != 1 or len(devices) != 1:
            raise RuntimeError(f"expected one mount and device, got {mounts} / {devices}")
        return {"mount": mounts[0], "device": devices[0]}

    def detach(self, mount: str) -> tuple:
        detached = subprocess.run(
            ["/usr/bin/hdiutil", "detach", mount],
            capture_output=True,
            timeout=min(60, permanent.work_left()),
        )
        return detached.returncode, detached.stderr.decode(errors="replace")

    def remove(self, path: str) -> None:
        os.remove(path)


REAL_CUSTODY_EFFECTS = CustodyEffects()


def verify_evidence_root(evidence: str, scratch: str, effects=REAL_CUSTODY_EFFECTS) -> str:
    """The guard's never-auto-cleaned evidence directory, verified before any
    effect: canonical, this user's owner-only real directory, a sibling of the
    exported scratch root inside one supervisor-owned root, never the
    workspace. Anything else refuses before creating, attaching or writing."""
    repository = os.path.dirname(HERE)
    if not evidence:
        raise Aborted("no OXIDE_PROOF_EVIDENCE directory is exported; image custody cannot be allocated")
    if not scratch:
        raise Aborted("no OXIDE_TASK_SCRATCH_ROOT is exported; the owned root cannot be verified")
    if evidence != effects.realpath(evidence):
        raise Aborted(f"the evidence directory {evidence} is not canonical; it is never used")
    if scratch != effects.realpath(scratch):
        raise Aborted(f"the scratch root {scratch} is not canonical; it is never used")
    if evidence == repository or evidence.startswith(repository + os.sep):
        raise Aborted(f"the evidence directory {evidence} is inside the workspace; it is never used")
    if os.path.dirname(evidence) != os.path.dirname(scratch):
        raise Aborted(
            f"the evidence directory {evidence} is not a sibling of the hidden scratch root {scratch}"
        )
    try:
        facts = effects.stat(evidence)
    except OSError as error:
        raise Aborted(f"the evidence directory {evidence} is not inspectable ({error})") from error
    if not stat.S_ISDIR(facts.st_mode) or facts.st_uid != effects.euid():
        raise Aborted(f"the evidence directory {evidence} is not this user's real directory")
    if facts.st_mode & 0o077:
        raise Aborted(f"the evidence directory {evidence} is not owner-only")
    return evidence


class ImageCustody:
    """One owned 128 MiB image whose backing file and custody record live in the
    guard's never-auto-cleaned evidence directory, allocated BEFORE attachment,
    with exactly one recorded ordinary-detach disposition."""

    def __init__(self, name: str, effects=REAL_CUSTODY_EFFECTS):
        self.name = name
        self.effects = effects
        self.evidence = ""
        self.backing = ""
        self.record = ""
        self.mount = ""
        self.device = ""
        self.state = "unallocated"
        self.disposition: str | None = None
        self.notes: list[str] = []

    def _write(self, state: str, **extra) -> None:
        document = {
            "version": CUSTODY_VERSION,
            "state": state,
            "image": self.backing,
            "mount": self.mount or None,
            "device": self.device or None,
            "disposition": self.disposition,
            "notes": self.notes[-8:],
            "updated_ms": int(time.time() * 1000),
        }
        document.update(extra)
        self.effects.write_record(self.record, json.dumps(document, indent=2, sort_keys=True) + "\n")
        self.state = state

    def allocate(self, evidence: str, scratch: str) -> str:
        self.evidence = verify_evidence_root(evidence, scratch, self.effects)
        self.backing = os.path.join(self.evidence, f"152-image-{self.name}.sparseimage")
        self.record = os.path.join(self.evidence, f"152-image-{self.name}-custody.json")
        if self.effects.exists(self.backing) or self.effects.exists(self.record):
            raise Aborted(
                f"image custody for {self.name} already exists in {self.evidence}; it is never overwritten"
            )
        self._write("allocated", allocated_ms=int(time.time() * 1000))
        return self.record

    def attach(self) -> str:
        if self.state != "allocated":
            raise Aborted(f"image custody is {self.state}, not allocated; nothing is attached")
        if self.effects.realpath(self.backing) != self.backing or os.path.dirname(self.backing) != self.evidence:
            raise Aborted(f"the image backing {self.backing} left its evidence directory; nothing is attached")
        try:
            self.effects.create_image(self.backing, self.name)
            facts = self.effects.attach(self.backing)
        except BaseException as error:
            self.notes.append(f"attach: {type(error).__name__}: {error}")
            self._write("unknown", attach_failed=str(error)[:400])
            raise Aborted(f"image attach is unknown; backing kept: {error}") from error
        self.mount = facts["mount"]
        self.device = facts["device"]
        self._write("attached")
        return self.mount

    def detach_once(self, sessions_closed: bool) -> str:
        """One recorded ordinary-detach disposition; never replays an attempt."""
        if self.disposition is not None:
            self.notes.append(f"detach replay refused (already {self.disposition})")
            return self.disposition
        if self.state != "attached":
            self.disposition = "refused:not-attached"
            self.notes.append("no attachment was recorded, so no detach ran")
            if self.record:
                self._write("unknown", detach_refused="not-attached")
            return self.disposition
        if not sessions_closed:
            self.disposition = "refused:unclosed-sessions"
            self.notes.append("sessions were not positively closed, so no detach ran and the backing is kept")
            self._write("unknown", detach_refused="unclosed-sessions")
            return self.disposition
        self.disposition = "attempted"
        self._write("attached")
        code, stderr = self.effects.detach(self.mount)
        still_there = self.effects.exists(self.mount) or (
            bool(self.device) and self.effects.exists(self.device)
        )
        if code != 0 or still_there:
            self.disposition = "unknown"
            self.notes.append(
                f"ordinary detach gave code {code} and "
                f"{'a lingering' if still_there else 'no'} mount/device evidence; backing kept"
            )
            self._write("unknown", detach_stderr=stderr[:400])
            return self.disposition
        self.disposition = "completed"
        self.notes.append("ordinary detach completed and the mount/device are positively gone")
        try:
            self.effects.remove(self.backing)
        except OSError as error:
            self.notes.append(f"the backing was kept after a completed detach ({error})")
        self._write("detached")
        return self.disposition


class Starters:
    """Every keyed starter one case owns.

    A case releases and awaits its own starters through `finish`, which uses the
    existing positively bound child helper. On ANY other exit path (a fault, a
    taint or an abort) `__exit__` releases that case's gates and awaits every
    remaining child; a child that cannot be proven closed leaves custody
    UNKNOWN for the run instead of a guessed signal or detach.
    """

    def __init__(self, label: str, release_gate=None):
        self.label = label
        self.entries: list[tuple] = []
        self.unknown: list[str] = []
        # The real gate release is defined later in this module; a pure test
        # passes its own recorder instead.
        self.release_gate = release_gate if release_gate is not None else release

    def pause(self, world, checkout: str, point: str, label: str):
        child, gate = world.paused(checkout, point, f"{self.label} {label}")
        self.entries.append((child, gate, f"{self.label} {label}"))
        return child, gate

    def finish(self, world, child, label: str) -> tuple:
        code, stdout, stderr = world.finish(child, label)
        self.entries = [entry for entry in self.entries if entry[0] is not child]
        return code, stdout, stderr

    def close(self, world) -> None:
        for _child, gate, _label in self.entries:
            if not os.path.exists(gate):
                self.release_gate(gate)
        for child, _gate, label in list(self.entries):
            try:
                world.finish(child, label)
                self.entries = [entry for entry in self.entries if entry[0] is not child]
            except Aborted as error:
                self.unknown.append(f"{label}: {error}")
        if self.unknown:
            DIAG.notes.append(f"starter custody unknown: {'; '.join(self.unknown)}")

    def __enter__(self):
        return self

    def __exit__(self, kind, value, tb) -> bool:
        self.close(_START_WORLD)
        if self.unknown and kind is None:
            raise Aborted(f"a keyed starter was not positively closed ({self.unknown[0]})")
        return False


_START_WORLD = None


def current_reservation(receipt: dict) -> dict | None:
    """The reservation of a receipt THIS head writes: ROOT-LEVEL `reserved`.

    `oxide outputs adopt` publishes a profile receipt
    (`oxide.outputs.transition/3`), whose reservation is root-level. The
    per-placement `links[<n>].reserved` shape is emitted only for the legacy
    whole-target `transition/2` document, which the product still writes on a
    legacy-only path and reads as legacy. This proof's current-write oracle
    therefore reads the root-level field only, so a case can never silently
    pass on that stale placement path.
    """
    return receipt.get("reserved")


class FixtureEffects:
    """The filesystem effects the fixture's private output container needs, so
    the pure selector can drive this real seam with in-memory stubs."""

    def mkdir(self, path: str, mode: int) -> None:
        os.mkdir(path, mode)

    def open_dir(self, path: str) -> int:
        return os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)

    def fstat(self, fd: int):
        return os.fstat(fd)

    def fchmod(self, fd: int, mode: int) -> None:
        os.fchmod(fd, mode)

    def stat(self, path: str):
        return os.stat(path, follow_symlinks=False)

    def close(self, fd: int) -> None:
        os.close(fd)

    def euid(self) -> int:
        return os.geteuid()


REAL_FIXTURE_EFFECTS = FixtureEffects()


def create_private_outputs_container(
    outer: str, mount: str, effects=REAL_FIXTURE_EFFECTS
) -> str:
    """Exclusively create this fixture's own `<mount>/oxide-outputs`, 0700.

    The product creates that directory itself, owner-only, and refuses an
    existing entry that is not an owner-only directory of this user
    (`volume::dir_at`). A fixture that planted nested roots first would let
    Python create the intermediate container with the default `0o777 & ~umask`
    (0755) and the product would correctly refuse it, so the container is
    created HERE, first, by an exclusive mkdir on this run's own image. An
    unexpected pre-existing entry refuses: it is never adopted, chmodded or
    followed, and a mode is only ever tightened through the descriptor this
    call created.
    """
    try:
        effects.mkdir(outer, 0o700)
    except FileExistsError as error:
        raise Aborted(
            f"the fixture's private output container {outer} already exists; it is never "
            "adopted, chmodded or followed"
        ) from error
    fd = effects.open_dir(outer)
    try:
        facts = effects.fstat(fd)
        if not stat.S_ISDIR(facts.st_mode):
            raise Aborted(f"the fixture's output container {outer} is not a real directory")
        if facts.st_uid != effects.euid():
            raise Aborted(f"the fixture's output container {outer} is not owned by this user")
        if facts.st_mode & 0o077:
            effects.fchmod(fd, 0o700)
            facts = effects.fstat(fd)
        if facts.st_mode & 0o077:
            raise Aborted(f"the fixture's output container {outer} is not owner-only")
        if facts.st_dev != effects.stat(mount).st_dev:
            raise Aborted(f"the fixture's output container {outer} is on another device")
    finally:
        effects.close(fd)
    return outer


class World:
    """The fixture: one staged binary, private HOME/CARGO_HOME/pool and one
    owned scratch volume registered in permanent mode."""

    def __init__(self, proof, root: str):
        self.proof = proof
        self.root = root
        self.pool = os.path.join(root, "pool")
        self.control = os.path.join(self.pool, "oxide-managed")
        self.outputs = os.path.join(self.control, "outputs")
        self.registry = os.path.join(self.outputs, "registry.json")
        self.gone = os.path.join(root, "gone")
        self.gate = os.path.join(root, "gate")
        self.mount = ""
        os.makedirs(self.gone)

    def oxide(self, args: list, label: str, cwd: str, extra: dict | None = None) -> tuple:
        env = dict(self.proof.env)
        env.update(extra or {})
        return permanent.oxide_env(self.proof, args, label, cwd, env)

    def checkout(self, name: str, base: str | None = None) -> str:
        path = os.path.join(base or self.root, "checkouts", name)
        os.makedirs(os.path.join(path, "src"))
        with open(os.path.join(path, "Cargo.toml"), "w", encoding="utf-8") as handle:
            handle.write(PLAIN)
        with open(os.path.join(path, "src", "lib.rs"), "w", encoding="utf-8") as handle:
            handle.write("pub fn v() -> u32 { 1 }\n")
        self.proof.run(["git", "init", "-q"], cwd=path, timeout=30)
        with open(os.path.join(path, ".gitignore"), "w", encoding="utf-8") as handle:
            handle.write("/target\n/.oxide-outputs/\n")
        return os.path.realpath(path)

    def adopt(self, checkout: str) -> dict:
        code, _, stderr = self.oxide(["outputs", "adopt"], f"adopt {os.path.basename(checkout)}", checkout)
        expect(code == 0, f"adopt failed: {stderr}")
        return self.receipt(checkout)

    def receipt(self, checkout: str) -> dict:
        # A receipt outcome that cannot be read or parsed is never an oracle
        # failure of the case that happens to read it: it aborts a keep-going
        # diagnostic instead of being read as a pass or a plain fault.
        return aborting(
            f"receipt outcome for {checkout}",
            lambda: permanent.read_json(permanent.receipt_path(self.pool, checkout)),
        )

    def selected(self, checkout: str) -> dict:
        receipt = self.receipt(checkout)
        expect(receipt["state"] == "committed", f"{checkout} receipt is {receipt['state']}")
        return receipt["links"][0]["selected"]

    def vanish(self, checkout: str) -> str:
        # A rename never crosses devices: an image checkout moves within it.
        base = self.gone
        if self.mount and checkout.startswith(self.mount + "/"):
            base = os.path.join(self.mount, "gone")
            os.makedirs(base, exist_ok=True)
        away = os.path.join(base, os.path.basename(checkout) + "-" + secrets.token_hex(3))
        os.rename(checkout, away)
        expect(not os.path.lexists(checkout), f"{checkout} still exists")
        return away

    def restore(self, checkout: str, away: str) -> None:
        os.rename(away, checkout)

    def registry_doc(self) -> dict:
        return aborting("registry outcome", lambda: permanent.read_json(self.registry))

    def finish(self, child, label: str) -> tuple:
        """Finish one owned child: its custody is never an oracle failure."""
        return aborting(f"child custody ({label})", lambda: permanent.finish_child(self.proof, child, label))

    def private_outputs_container(self) -> str:
        """This run's own `<mount>/oxide-outputs`, created exclusively 0700
        before any nested fixture root lands in it."""
        return create_private_outputs_container(
            os.path.join(self.mount, "oxide-outputs"), self.mount
        )

    def version_one_fixture(self) -> dict:
        """Two whole-target `oxide.outputs.transition/1` receipts and their
        `root/1` roots, written BEFORE the registry exists, so the product's own
        receipt seed records their legacy rows (no nonce, no invented
        identity). Nothing here is a hand-made registry: the schema comes from
        the shape oxide#117 published.

        The image's `oxide-outputs` container is created first and exclusively
        0700. Creating the nested version-1 roots first would let Python create
        that intermediate with the default 0755, and the product's own
        owner-only check would (correctly) refuse it."""
        uuid = permanent.real_uuid(self.pool)
        os.makedirs(self.outputs, mode=0o700, exist_ok=True)
        os.chmod(self.outputs, 0o700)
        self.private_outputs_container()

        def marker(checkout: str, checkout_id: str, label: str) -> dict:
            return {
                "version": "oxide.outputs.root/1",
                "checkout": checkout,
                "checkout_id": checkout_id,
                "label": label,
                "recipe": "target-debug-v1",
                "kind": "external",
                "volume_uuid": uuid,
                "generation": 1,
                "created_ms": 1,
            }

        def root_for(checkout_id: str, label: str) -> str:
            # The container above exists, so this is one exclusive leaf mkdir
            # inside it: no intermediate directory is ever created here.
            root = os.path.join(self.mount, "oxide-outputs", f"{label}-{checkout_id[:8]}")
            os.mkdir(root, 0o700)
            os.chmod(root, 0o700)
            with open(os.path.join(root, "artifact"), "wb") as handle:
                handle.write(b"x" * 4096)
            return root

        def selected(root: str, generation: int) -> dict:
            return {"kind": "external", "root": root, "volume_uuid": uuid, "generation": generation}

        def publish(checkout: str, receipt: dict) -> str:
            checkout_id = receipt["checkout_id"]
            path = os.path.join(self.outputs, checkout_id + ".json")
            with open(path, "w", encoding="utf-8") as handle:
                json.dump(receipt, handle, indent=2)
            os.chmod(path, 0o600)
            return path

        fixture = {}
        checkout = self.checkout("legacy-v1")
        checkout_id = hashlib.sha256(checkout.encode()).hexdigest()
        root = root_for(checkout_id, "legacy-v1")
        with open(os.path.join(root, ".oxide-output.json"), "w", encoding="utf-8") as handle:
            json.dump(marker(checkout, checkout_id, "legacy-v1"), handle)
        fixture["committed"] = (checkout, root)
        fixture["committed_receipt"] = publish(
            checkout,
            {
                "version": "oxide.outputs.transition/1",
                "checkout": checkout,
                "checkout_id": checkout_id,
                "generation": 1,
                "state": "committed",
                "selected": selected(root, 1),
                "roots": [selected(root, 1)],
                "updated_ms": 1,
            },
        )
        # A valid NONCOMMITTED version-1 state: one committed root and one
        # prepared transition. Neither may be pruned or rewritten, and neither
        # may poison the batch.
        prepared = self.checkout("legacy-v1-prepared")
        prepared_id = hashlib.sha256(prepared.encode()).hexdigest()
        prepared_root = root_for(prepared_id, "legacy-v1-prepared")
        with open(os.path.join(prepared_root, ".oxide-output.json"), "w", encoding="utf-8") as handle:
            json.dump(marker(prepared, prepared_id, "legacy-v1-prepared"), handle)
        next_root = os.path.join(self.mount, "oxide-outputs", f"legacy-v1-next-{prepared_id[:8]}")
        fixture["prepared"] = (prepared, prepared_root)
        fixture["prepared_next"] = next_root
        fixture["prepared_receipt"] = publish(
            prepared,
            {
                "version": "oxide.outputs.transition/1",
                "checkout": prepared,
                "checkout_id": prepared_id,
                "generation": 1,
                "state": "prepared",
                "selected": selected(prepared_root, 1),
                "prepared": {
                    "generation": 2,
                    "to": selected(next_root, 2),
                    "link": os.path.join(prepared, "target"),
                },
                "roots": [selected(prepared_root, 1), selected(next_root, 2)],
                "updated_ms": 2,
            },
        )
        return fixture

    def dry_run(self, label: str, cwd: str, listed: str | None = None, extra: dict | None = None) -> tuple:
        args = ["outputs", "prune", "--dry-run"] + (["--operator-list", listed] if listed else [])
        env = dict(self.proof.env)
        env.update(extra or {})
        result = self.proof.run([self.proof.binary, *args], cwd=cwd, env=env)
        stdout, stderr = result.stdout, result.stderr.decode(errors="replace")
        print(f"----- {label}: exit {result.returncode} -----\n{stderr}", flush=True)
        return result.returncode, stdout, stderr

    def plan_file(self, label: str, cwd: str, listed: str | None = None, extra: dict | None = None) -> tuple:
        code, stdout, stderr = self.dry_run(label, cwd, listed, extra)
        expect(code == 0, f"{label}: dry run exited {code}: {stderr}")
        sha = hashlib.sha256(stdout).hexdigest()
        expect(sha in stderr, f"{label}: stderr does not carry the preview digest")
        path = os.path.join(self.root, f"plan-{secrets.token_hex(4)}.json")
        with open(path, "wb") as handle:
            handle.write(stdout)
        return path, sha, json.loads(stdout)

    def real_run(self, label: str, cwd: str, plan: str, sha: str, listed: str | None = None, extra: dict | None = None) -> tuple:
        args = ["outputs", "prune", "--plan", plan, "--plan-sha256", sha]
        if listed:
            args += ["--operator-list", listed]
        return self.oxide(args, label, cwd, extra)

    def candidates(self, preview: dict) -> set:
        return {candidate["root"] for candidate in preview["candidates"]}

    def kept(self, preview: dict, root: str) -> str:
        for kept in preview["kept"]:
            if kept.get("root") == root:
                return kept["reason"]
        return ""

    def intents(self) -> list:
        return self.registry_doc().get("intents", [])

    def quarantines(self) -> list:
        return sorted(glob.glob(os.path.join(self.mount, "oxide-outputs", ".prune-*")))

    def paused(self, checkout: str, point: str, label: str):
        """Start `oxide check` paused at a debug-only point; returns the child
        once it has reached the point."""
        gate = os.path.join(self.gate, secrets.token_hex(4))
        os.makedirs(self.gate, exist_ok=True)
        env = dict(self.proof.env)
        env["OXIDE_TEST_PAUSE"] = f"{point}:{gate}"
        child = aborting(
            f"child custody ({label})",
            lambda: permanent.start_child(self.proof, [self.proof.binary, "check"], checkout, env),
        )
        deadline = time.monotonic() + min(60, permanent.work_left())
        while not glob.glob(f"{gate}.{point}-*"):
            if child.poll() is not None or time.monotonic() >= deadline:
                code, out, err = self.finish(child, label)
                fail(f"{label}: never reached {point} (exit {code}): {err}")
            time.sleep(0.05)
        return child, gate


def keyed_destination(gate: str) -> str:
    """The destination facts a starter's slot key captured (keyed pause)."""
    [marker_file] = glob.glob(f"{gate}.keyed-*")
    return permanent.read_bytes(marker_file).decode()


def accounting_oracle(root: str) -> tuple[int, int]:
    """Independent expectation: regular files' sizes by name, and allocated
    blocks of each regular-file inode counted once (hard links once)."""
    logical, allocated, seen = 0, 0, set()
    for directory, _, files in os.walk(root):
        for name in files:
            facts = os.lstat(os.path.join(directory, name))
            if not stat.S_ISREG(facts.st_mode):
                continue
            logical += facts.st_size
            if (facts.st_dev, facts.st_ino) not in seen:
                seen.add((facts.st_dev, facts.st_ino))
                allocated += facts.st_blocks * 512
    return logical, allocated


def release(gate: str) -> None:
    with open(gate, "wb") as handle:
        handle.write(b"go")


def tree_bytes(root: str) -> dict:
    digest = {}
    for directory, _, files in os.walk(root):
        for name in files:
            path = os.path.join(directory, name)
            if os.path.islink(path):
                continue
            with open(path, "rb") as handle:
                digest[os.path.relpath(path, root)] = hashlib.sha256(handle.read()).hexdigest()
    return digest


def read_nofollow(path: str, cap: int) -> tuple[bytes, tuple]:
    """A bounded no-follow read of one regular file owned by this user;
    returns the bytes and the (dev, ino, size, mtime_ns) identity."""
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        facts = os.fstat(fd)
        expect(stat.S_ISREG(facts.st_mode), f"{path} is not a regular file")
        expect(facts.st_uid == os.getuid(), f"{path} is not owned by this user")
        expect(facts.st_size <= cap, f"{path} exceeds its {cap}-byte cap")
        chunks, left = [], cap + 1
        while left > 0:
            chunk = os.read(fd, min(left, 1 << 20))
            if not chunk:
                break
            chunks.append(chunk)
            left -= len(chunk)
        data = b"".join(chunks)
        expect(len(data) == facts.st_size, f"{path} changed size while read")
        return data, (facts.st_dev, facts.st_ino, facts.st_size, facts.st_mtime_ns)
    finally:
        os.close(fd)


def copy_old_artifact(root: str) -> str:
    """Verify the exact receipt and installed binary, copy the binary into
    owned scratch and return the copy. Any mismatch fails; nothing is
    reinstalled, skipped or substituted."""
    # An unavailable or substituted artifact is a PREREQUISITE failure, never
    # an oracle outcome: it aborts a keep-going diagnostic and it is never
    # skipped, stubbed or retargeted to whatever happens to be installed now.
    try:
        return _copy_old_artifact(root)
    except Aborted:
        raise
    except (KeyboardInterrupt, SystemExit):
        raise
    except BaseException as error:
        raise Aborted(
            "older-binary fence control: the durable artifact or the original receipt did not "
            f"verify ({type(error).__name__}: {error}); the proof requires binary sha256 "
            f"{OLD_SHA}, size {OLD_SIZE} bytes (pin {OLD_PIN}, control state 2) at "
            f"{OLD_ARTIFACT_PATH} plus the original receipt {OLD_RECEIPT}. Nothing is "
            "substituted: the current installed binary and the custody JSON are never read"
        ) from error


def _copy_old_artifact(root: str) -> str:
    receipt_bytes, receipt_id = read_nofollow(OLD_RECEIPT, OLD_RECEIPT_CAP)
    receipt = json.loads(receipt_bytes)
    installed = receipt.get("installed", {})
    expect(receipt.get("pin") == OLD_PIN, f"receipt pin {receipt.get('pin')} is not {OLD_PIN}")
    # The receipt's `installed.path` is the HISTORICAL installation path, which
    # now holds the current 4df3dfd binary: it is checked as provenance text
    # and never opened. The proof input is the separate durable artifact.
    expect(
        installed.get("path") == OLD_INSTALLED_PATH,
        f"receipt installed.path {installed.get('path')} is not the historical {OLD_INSTALLED_PATH}",
    )
    expect(installed.get("sha256") == OLD_SHA and receipt.get("built_sha256") == OLD_SHA, "receipt hash mismatch")
    expect(receipt.get("control_state_version") == 2, f"receipt control state {receipt.get('control_state_version')}")
    expect(installed.get("size") == OLD_SIZE, f"receipt size {installed.get('size')} is not {OLD_SIZE}")
    expect(OLD_ARTIFACT_PATH != OLD_INSTALLED_PATH, "the artifact source path is the installed path")
    binary, binary_id = read_nofollow(OLD_ARTIFACT_PATH, OLD_BINARY_CAP)
    expect(len(binary) == OLD_SIZE, f"artifact size {len(binary)} is not the pinned {OLD_SIZE}")
    expect(hashlib.sha256(binary).hexdigest() == OLD_SHA, "artifact hash differs from the pin")
    for path, before in ((OLD_RECEIPT, receipt_id), (OLD_ARTIFACT_PATH, binary_id)):
        after = os.lstat(path)
        expect(
            (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns) == before,
            f"{path} changed while it was copied",
        )
    copy = os.path.join(root, "old-oxide-d246d400")
    fd = os.open(copy, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o700)
    try:
        os.write(fd, binary)
    finally:
        os.close(fd)
    expect(hashlib.sha256(permanent.read_bytes(copy)).hexdigest() == OLD_SHA, "the copy's hash differs")
    permanent.record(
        "old-artifact.json",
        json.dumps(
            {
                "source_pin": OLD_PIN,
                "binary_sha256": OLD_SHA,
                "binary_size": len(binary),
                "control_state_version": 2,
                "source_artifact_path": OLD_ARTIFACT_PATH,
                "historical_installed_path": OLD_INSTALLED_PATH,
                "historical_installed_path_note": (
                    "recorded by the original receipt as installed.path; that path now holds the "
                    "current 4df3dfd binary and is never opened by this proof"
                ),
                "custody_note": (
                    "the custody JSON beside the durable copy is provenance only; identity comes "
                    "from the original receipt's pin/sha/size and the read bytes' SHA-256"
                ),
                "receipt_path": OLD_RECEIPT,
                "receipt_sha256": hashlib.sha256(receipt_bytes).hexdigest(),
                "copy": copy,
            },
            indent=2,
        ),
    )
    return copy


def tree_snapshot(root: str) -> dict:
    """Complete bounded bytes + metadata of a fixture tree (no atime)."""
    snapshot = {}
    for directory, directories, files in os.walk(root):
        for name in directories + files:
            path = os.path.join(directory, name)
            facts = os.lstat(path)
            entry = (facts.st_mode, facts.st_size, facts.st_mtime_ns, facts.st_ino)
            if stat.S_ISREG(facts.st_mode):
                entry += (hashlib.sha256(permanent.read_bytes(path)).hexdigest(),)
            elif stat.S_ISLNK(facts.st_mode):
                entry += (os.readlink(path),)
            snapshot[os.path.relpath(path, root)] = entry
    return snapshot


def marker(root: str) -> dict:
    return permanent.read_json(os.path.join(root, ".oxide-output.json"))


def run(proof, repo: str, custody: ImageCustody) -> None:
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    DIAG.source = source
    name = custody.name
    mount = f"/Volumes/{name}"
    expect(not os.path.lexists(mount), f"{mount} exists before the proof; never touched")
    print(f"outputs-prune proof {os.getpid()} source {source} mount {mount}", flush=True)
    proof.phase("start")

    # The staged candidate's compiled capabilities, pinned exactly for THIS
    # head: state version 4 and the `oxide.outputs/3` owned-outputs fence,
    # debug-only proof controls. The shared helper's default stays unchanged
    # for its other caller.
    proof.binary = handoff.stage_subject(proof.root, capability=CANDIDATE_CAPABILITY)
    proof.phase("after staging")

    world = World(proof, proof.root)
    home = os.path.join(proof.root, "home")
    cargo_home = os.path.join(proof.root, "cargo-home")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    proof.env = permanent.proof_env(world.pool, home, cargo_home)
    # Premise 7: the declared flags are the only ones in force. An inherited
    # encoded value would silently override them in every fixture Cargo call.
    proof.env.pop("CARGO_ENCODED_RUSTFLAGS", None)

    # The pre-registration first build runs in a distinct bootstrap checkout
    # (its real ephemeral target is expected); alpha is created only after
    # registration and stays target-free until its own adopt in case 01.
    bootstrap = world.checkout("bootstrap")
    code, _, stderr = world.oxide(["check"], "first build creates the internal pool", bootstrap)
    expect(code == 0, stderr)
    global _START_WORLD
    _START_WORLD = world  # Starters' failure path needs the bound child helper.
    custody.allocate(os.environ.get(EVIDENCE_ENV, ""), os.environ.get("OXIDE_TASK_SCRATCH_ROOT", ""))
    world.mount = custody.attach()
    expect(world.mount == mount, f"mounted at {world.mount}, not {mount}")
    code, stdout, stderr = world.oxide(
        ["pool", "register-removable", mount, "--reserve", "16MiB", "--cap", "64MiB", "--mode", "permanent"],
        "register permanent drive",
        bootstrap,
    )
    expect(code == 0 and "registered removable pool" in stdout, stderr)
    good = permanent.real_uuid(world.pool)
    alpha = world.checkout("alpha")
    expect(not os.path.lexists(os.path.join(alpha, "target")), "alpha has a target before its adopt")
    proof.phase("after registration")

    # ---- fence / init ------------------------------------------------------
    with case("01") as scope, scope.run():
        # The version-1 fixture (case 21) must exist before the registry does, so
        # the product's own bounded receipt seed records its legacy rows. No
        # registry or row is written by hand.
        world.version_one = world.version_one_fixture()
        world.adopt(alpha)
        for control in (world.control, os.path.join(world.control, "removable", "oxide-managed")):
            state_file = os.path.join(control, "state.json")
            expect(os.path.isfile(state_file), f"paired control state {state_file} is missing")
            state = permanent.read_json(state_file)
            # The current control format at this head (store.rs STATE_VERSION and
            # OWNED_OUTPUTS_FENCE): a bump fails this proof loudly instead of
            # letting it check a stale format.
            expect(state["version"] == CONTROL_STATE_VERSION, f"{control}: state version {state['version']}")
            expect(
                state.get("owned_outputs") == OWNED_OUTPUTS_FENCE,
                f"{control}: {state.get('owned_outputs')}",
            )
            expect("ready" in json.dumps(state.get("outputs_init")), f"{control}: {state.get('outputs_init')}")
        registry = world.registry_doc()
        expect(registry_is_current(registry), registry)
        # Older-binary fence control: the exact d246d400 artifact's copy reports
        # control state 2, then its `check` and `outputs adopt` in this fixture
        # (private HOME without any install receipt, private CARGO_HOME, pool and
        # registration) both refuse before any effect.
        old = copy_old_artifact(proof.root)
        probe = proof.run([old, CAPABILITIES_PROBE], cwd=alpha, env=proof.env, timeout=30)
        expect(probe.returncode == 0, f"old capability probe exit {probe.returncode}")
        expect(json.loads(probe.stdout).get("control_state_version") == 2, f"old probe {probe.stdout!r}")
        watched = [world.pool, os.path.join(mount, "oxide-outputs"), alpha]
        before = [tree_snapshot(path) for path in watched]
        for args in (["check"], ["outputs", "adopt"]):
            result = proof.run([old, *args], cwd=alpha, env=proof.env, timeout=30)
            stderr = result.stderr.decode(errors="replace")
            print(f"----- old binary {args}: exit {result.returncode} -----\n{stderr}", flush=True)
            expect(result.returncode != 0, f"the older binary's {args} did not refuse")
            expect(HANDOFF_NOTICE not in stderr, f"the older binary's {args} selected a handoff; STOP")
            expect(
                OLD_INSTALLED_PATH not in stderr
                and OLD_ARTIFACT_PATH not in stderr
                and "/.local/state/oxide" not in stderr,
                f"a host path appeared: {stderr}",
            )
            expect([tree_snapshot(path) for path in watched] == before, f"the older binary's {args} changed fixture state")

    with case("02") as scope, scope.run():
        saved = permanent.read_bytes(world.registry)
        os.rename(world.registry, world.registry + ".aside")
        code, _, stderr = world.dry_run("dry run without the registry", alpha)
        expect(code == 2, f"dry run without the registry exited {code}")
        beta = world.checkout("beta")
        code, _, stderr = world.oxide(["outputs", "adopt"], "adopt without the registry", beta)
        expect(code != 0, "adopt succeeded without the registry")
        expect(not os.path.exists(world.registry), "the registry was rebuilt")
        os.rename(world.registry + ".aside", world.registry)
        expect(permanent.read_bytes(world.registry) == saved, "registry bytes changed")
        proof.phase("after fence cases")

    # ---- nonce / reservation ----------------------------------------------
    with case("03") as scope, scope.run():
        selected = world.selected(alpha)
        root_marker = marker(selected["root"])
        expect(root_marker["version"] == "oxide.outputs.root/2", root_marker)
        nonce = root_marker["nonce"]
        expect(len(nonce) == 32 and all(c in "0123456789abcdef" for c in nonce), nonce)
        expect(selected.get("nonce") == nonce, f"receipt nonce {selected.get('nonce')} != {nonce}")
        # A current profile receipt places one link per member, so the
        # registry records one row per member for the same root (the legacy
        # whole-target document had exactly one). Every row must agree on the
        # nonce and the selection, and the member set must be the placed set.
        rows = [row for row in world.registry_doc()["rows"] if row["root"] == selected["root"]]
        expect(
            len(rows) == 2
            and all(row["nonce"] == nonce and row["selected"] for row in rows)
            and sorted(row["member"] for row in rows) == ["debug", "release"],
            rows,
        )

    with case("04") as scope, scope.run():
        # ELIGIBLE reservation path (source-anchored): a committed profile
        # receipt whose placed members are external on the healthy registered
        # volume, with the root leaf gone so the fresh selection is still
        # EXTERNAL but its root does not exist. `reserve` publishes a fresh
        # root nonce for exactly that missing root before the slot key
        # (outputs.rs:4842-4905; `refuses_committed_external` is false because
        # wanted.kind is External, and `read_marker(wanted.root)` fails).
        world.adopt(beta)
        beta_root = world.selected(beta)["root"]
        shutil.rmtree(beta_root)
        expect(not os.path.lexists(beta_root), "the recreated root is still present")
        with Starters("case 04") as starters:
            first, first_gate = starters.pause(world, beta, "keyed", "starter one")
            second, second_gate = starters.pause(world, beta, "keyed", "starter two")
            reserved = current_reservation(world.receipt(beta))
            expect(reserved is not None and len(reserved["nonce"]) == 32, f"no reservation: {reserved}")
            expect(
                reserved["kind"] == "external" and reserved["root"] == beta_root,
                f"the reservation names {reserved.get('kind')} {reserved.get('root')}, not the recreated external root",
            )
            # Both starters keyed the same reserved nonce, before either proceeds.
            first_key, second_key = keyed_destination(first_gate), keyed_destination(second_gate)
            expect(first_key == second_key and reserved["nonce"] in first_key, f"keys {first_key!r} / {second_key!r}")
            release(first_gate)
            release(second_gate)
            for child, label in ((first, "starter one"), (second, "starter two")):
                code, _, stderr = starters.finish(world, child, label)
                expect(code == 0, f"{label} failed: {stderr}")
        expect(
            marker(beta_root)["nonce"] == reserved["nonce"],
            "the recreated root does not carry the reserved nonce",
        )
        expect(current_reservation(world.receipt(beta)) is None, "the reservation was not consumed")
        # NON-RESERVING control, retained from the old premise as an added
        # assertion (not a removal or a relabelled case): a fresh selection
        # that is not external against a committed external profile member is
        # exactly binding's P4 precondition, so the command changes no
        # authority at all (oxide#133 case14) and prune keeps the root.
        permanent.set_recorded_uuid(world.pool, "00000000-0000-0000-0000-000000000000")
        try:
            settled = permanent.read_bytes(permanent.receipt_path(world.pool, beta))
            with Starters("case 04 control") as control:
                refused, refused_gate = control.pause(world, beta, "keyed", "local-fallback starter")
                expect(
                    permanent.read_bytes(permanent.receipt_path(world.pool, beta)) == settled,
                    "a command binding will refuse changed the receipt",
                )
                expect(
                    current_reservation(world.receipt(beta)) is None,
                    "a command binding will refuse published a reservation",
                )
                release(refused_gate)
                code, _, stderr = control.finish(world, refused, "local-fallback starter")
                expect(code != 0, f"the refused local-fallback starter exited {code}: {stderr}")
            _, _, preview = world.plan_file("non-reserving control dry run", alpha)
            reason = world.kept(preview, beta_root)
            expect("present" in reason, f"the non-reserving control root was not kept: {reason}")
        finally:
            permanent.set_recorded_uuid(world.pool, good)

    with case("05") as scope, scope.run():
        gamma = world.checkout("gamma")
        world.adopt(gamma)
        gamma_root = world.selected(gamma)["root"]
        shutil.rmtree(gamma_root)
        with Starters("case 05") as starters:
            stale, stale_gate = starters.pause(world, gamma, "keyed", "stale starter")
            reserved = current_reservation(world.receipt(gamma))
            expect(reserved is not None, f"no reservation: {reserved}")
            expect(
                reserved["nonce"] in keyed_destination(stale_gate),
                "the stale starter did not key the reserved nonce",
            )
            code, stdout, stderr = world.oxide(["outputs", "cancel-reservation"], "cancel the reservation", gamma)
            expect(code == 0 and "cancelled" in stdout, stdout + stderr)
            expect(current_reservation(world.receipt(gamma)) is None, "the receipt kept the reservation")
            expect(
                not any(row.get("reservation") == reserved["id"] or row.get("cancelling") for row in world.registry_doc()["rows"]),
                "the cancelled reservation's row remains",
            )
            release(stale_gate)
            code, _, stderr = starters.finish(world, stale, "stale starter")
            expect(code != 0 and "cancelled or replaced" in stderr, f"stale starter exit {code}: {stderr}")
        expect(not os.path.lexists(reserved["root"]), "the cancelled root was created")
        proof.phase("after reservation cases")

    # ---- registry / snapshot ----------------------------------------------
    with case("06") as scope, scope.run():
        before = tree_bytes(world.control)
        volume_before = tree_bytes(os.path.join(mount, "oxide-outputs"))
        expect("outputs/registry.json" in before and len(volume_before) > 0, "the byte oracle saw nothing")
        path, sha, preview = world.plan_file("dry run", alpha)
        expect(preview["version"] == "oxide.outputs.prune-preview/1", preview["version"])
        expect(tree_bytes(world.control) == before, "a dry run changed control bytes")
        expect(tree_bytes(os.path.join(mount, "oxide-outputs")) == volume_before, "a dry run changed volume bytes")

    with case("07") as scope, scope.run():
        delta = world.checkout("delta")
        world.adopt(delta)
        delta_root = world.selected(delta)["root"]
        delta_away = world.vanish(delta)
        gamma_receipt = permanent.receipt_path(world.pool, gamma)
        saved_receipt = permanent.read_bytes(gamma_receipt)
        with open(gamma_receipt, "wb") as handle:
            handle.write(b"{corrupt")
        code, stdout, _ = world.dry_run("dry run with a corrupt receipt", alpha)
        preview = json.loads(stdout)
        expect(code == 0 and not preview["candidates"], f"candidates despite a corrupt receipt: {preview['candidates']}")
        expect("whole batch" in world.kept(preview, delta_root), world.kept(preview, delta_root))
        with open(gamma_receipt, "wb") as handle:
            handle.write(saved_receipt)

    # ---- eligibility --------------------------------------------------------
    with case("08") as scope, scope.run():
        _, _, preview = world.plan_file("eligibility dry run", alpha)
        expect(world.candidates(preview) == {delta_root}, f"candidates {world.candidates(preview)}")
        expect("present" in world.kept(preview, world.selected(alpha)["root"]), "alpha not kept as present")
        epsilon = world.checkout("epsilon")
        epsilon_root = world.adopt(epsilon)["links"][0]["selected"]["root"]
        code, _, stderr = world.oxide(["outputs", "detach"], "detach epsilon", epsilon)
        expect(code == 0, stderr)
        _, _, preview = world.plan_file("live detached dry run", alpha)
        expect(epsilon_root not in world.candidates(preview), "a live detached root became a candidate")
        expect("present" in world.kept(preview, epsilon_root), world.kept(preview, epsilon_root))
        # The same released root becomes eligible once its checkout is gone.
        epsilon_away = world.vanish(epsilon)
        _, _, preview = world.plan_file("released vanished dry run", alpha)
        expect(epsilon_root in world.candidates(preview), f"a released vanished root was kept: {world.kept(preview, epsilon_root)}")
        world.restore(epsilon, epsilon_away)

        # A registered checkout on the attached image: absence is proved through
        # the verified mount and its recorded volume UUID (the /Volumes branch).
        omega = world.checkout("omega", base=mount)
        omega_root = world.adopt(omega)["links"][0]["selected"]["root"]
        omega_away = world.vanish(omega)
        _, _, preview = world.plan_file("registered image checkout dry run", alpha)
        expect(omega_root in world.candidates(preview), f"a registered /Volumes checkout was kept: {world.kept(preview, omega_root)}")
        world.restore(omega, omega_away)

    with case("09") as scope, scope.run():
        # No real unreadable directory: a debug-only EACCES is injected inside the
        # supervisor-owned scratch, its firing is recorded in the evidence
        # directory, and the same fixture without injection is genuine absence.
        locked = os.path.realpath(os.path.join(proof.root, "locked"))
        os.makedirs(locked)
        eta = os.path.join(locked, "eta")
        shutil.move(world.checkout("eta-src"), eta)
        eta_root = world.adopt(eta)["links"][0]["selected"]["root"]
        eta_away = world.vanish(eta)
        fired = os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], "prune-eacces-fired")
        expect(not os.path.exists(fired), "the EACCES marker exists before injection")
        _, _, preview = world.plan_file("injected EACCES dry run", alpha, extra={"OXIDE_TEST_PRUNE_EACCES": locked})
        expect(eta_root not in world.candidates(preview), "an injected unreadable ancestor became absence")
        reason = world.kept(preview, eta_root)
        expect("unknown" in reason and "Permission denied" in reason, reason)
        expect(os.path.isfile(fired) and locked in permanent.read_bytes(fired).decode(), "the EACCES injection did not fire")
        _, _, preview = world.plan_file("genuine absence control dry run", alpha)
        expect(eta_root in world.candidates(preview), f"genuine absence not a candidate: {world.kept(preview, eta_root)}")
        world.restore(eta, eta_away)
        for label, body in (("relative", '["relative/root"]'), ("duplicate", json.dumps([delta_root, delta_root]))):
            listed = os.path.join(proof.root, f"list-{label}.json")
            with open(listed, "w", encoding="utf-8") as handle:
                handle.write(body)
            code, _, _ = world.dry_run(f"{label} operator list", alpha, listed)
            expect(code == 2, f"{label} operator list exited {code}")

    with case("10") as scope, scope.run():
        legacy_checkout = os.path.join(proof.root, "never", "legacy-app")
        os.makedirs(os.path.dirname(legacy_checkout))
        legacy_id = hashlib.sha256(legacy_checkout.encode()).hexdigest()
        legacy_root = os.path.join(mount, "oxide-outputs", "legacy-" + legacy_id[:8])
        os.makedirs(legacy_root, mode=0o700)
        os.chmod(legacy_root, 0o700)
        legacy_marker = dict(marker(world.selected(alpha)["root"]))
        legacy_marker.pop("nonce", None)
        legacy_marker.pop("checkout_volume_uuid", None)
        legacy_marker.update({"version": "oxide.outputs.root/1", "checkout": legacy_checkout, "checkout_id": legacy_id, "generation": 1})
        with open(os.path.join(legacy_root, ".oxide-output.json"), "w", encoding="utf-8") as handle:
            json.dump(legacy_marker, handle)
        with open(os.path.join(legacy_root, "artifact"), "wb") as handle:
            handle.write(b"x" * 4096)
        # A second legacy root whose recorded checkout is on the separately
        # attached scratch image (not the boot Data volume): kept.
        image_checkout = os.path.join(mount, "never", "image-app")
        image_id = hashlib.sha256(image_checkout.encode()).hexdigest()
        image_root = os.path.join(mount, "oxide-outputs", "legacy-" + image_id[:8])
        os.makedirs(image_root, mode=0o700)
        os.chmod(image_root, 0o700)
        image_marker = dict(legacy_marker)
        image_marker.update({"checkout": image_checkout, "checkout_id": image_id})
        with open(os.path.join(image_root, ".oxide-output.json"), "w", encoding="utf-8") as handle:
            json.dump(image_marker, handle)
        # A foreign directory (no marker) and a marker whose checkout identity
        # is not the digest of its checkout path: both kept, bytes unchanged.
        foreign_root = os.path.join(mount, "oxide-outputs", "foreign-dir")
        os.makedirs(foreign_root, mode=0o700)
        with open(os.path.join(foreign_root, "data"), "wb") as handle:
            handle.write(b"foreign")
        forged_root = os.path.join(mount, "oxide-outputs", "legacy-forged")
        os.makedirs(forged_root, mode=0o700)
        os.chmod(forged_root, 0o700)
        forged_marker = dict(legacy_marker)
        forged_marker.update({"checkout": os.path.join(proof.root, "never", "forged"), "checkout_id": "f" * 64})
        with open(os.path.join(forged_root, ".oxide-output.json"), "w", encoding="utf-8") as handle:
            json.dump(forged_marker, handle)
        kept_before = {root: tree_bytes(root) for root in (foreign_root, forged_root, image_root)}
        listed = os.path.join(proof.root, "list-legacy.json")
        with open(listed, "w", encoding="utf-8") as handle:
            json.dump([legacy_root, image_root, foreign_root, forged_root], handle)
        # Delta (an ordinary eligible root) is present for this run so only the
        # listed legacy roots are candidates; it vanishes again afterwards.
        world.restore(delta, delta_away)
        plan, sha, preview = world.plan_file("legacy dry run", alpha, listed)
        expect(legacy_root in world.candidates(preview), f"legacy root not a candidate: {world.kept(preview, legacy_root)}")
        expect(image_root not in world.candidates(preview), "a legacy root of an image checkout became a candidate")
        expect("volume" in world.kept(preview, image_root), world.kept(preview, image_root))
        expect(foreign_root not in world.candidates(preview), "a foreign directory became a candidate")
        expect("digest" in world.kept(preview, forged_root), world.kept(preview, forged_root))
        code, stdout, stderr = world.real_run("legacy real run", alpha, plan, sha, listed)
        expect(code == 0, f"legacy real run exited {code}: {stderr}")
        expect(not os.path.lexists(legacy_root), "the legacy root remains")
        expect(os.path.isdir(image_root), "the kept image-checkout legacy root was deleted")
        delta_away = world.vanish(delta)
        for root, digest in kept_before.items():
            expect(tree_bytes(root) == digest, f"kept listed root {root} changed")
        legacy_receipt = permanent.read_json(permanent.receipt_file(world.pool, legacy_checkout))
        expect(legacy_receipt["state"] == "pruned" and legacy_receipt["pruned"]["outcome"] == "deleted", legacy_receipt)
        proof.phase("after eligibility cases")

    # ---- custody / containment -------------------------------------------
    with case("11") as scope, scope.run():
        theta = world.checkout("theta")
        with open(os.path.join(theta, "build.rs"), "w", encoding="utf-8") as handle:
            handle.write(GATED_BUILD)
        theta_root = world.adopt(theta)["links"][0]["selected"]["root"]
        writer_gate = os.path.join(proof.root, "writer-gate")
        os.makedirs(writer_gate)
        writer_env = dict(proof.env)
        writer_env["PRUNE_GATE"] = writer_gate
        writer = permanent.start_child(proof, [proof.binary, "check"], theta, writer_env)
        permanent.wait_for(os.path.join(writer_gate, "started"), "running writer")
        theta_away = world.vanish(theta)
        _, _, preview = world.plan_file("running writer dry run", alpha)
        expect(theta_root not in world.candidates(preview), "a running writer's root became a candidate")
        expect("writer" in world.kept(preview, theta_root), world.kept(preview, theta_root))
        world.restore(theta, theta_away)
        with open(os.path.join(writer_gate, "release"), "wb") as handle:
            handle.write(b"go")
        code, _, stderr = world.finish(writer, "running writer")
        expect(code == 0, f"the gated writer failed: {stderr}")

    with case("12") as scope, scope.run():
        iota = world.checkout("iota")
        iota_root = world.adopt(iota)["links"][0]["selected"]["root"]
        outside = os.path.join(proof.root, "outside")
        os.makedirs(outside)
        with open(os.path.join(outside, "keep"), "wb") as handle:
            handle.write(b"keep")
        os.symlink(outside, os.path.join(iota_root, "escape"))
        world.vanish(iota)
        plan, sha, preview = world.plan_file("symlink descendant dry run", alpha)
        expect(iota_root in world.candidates(preview), world.kept(preview, iota_root))
        expect(delta_root in world.candidates(preview), "delta dropped out")
        code, _, stderr = world.real_run("symlink descendant real run", alpha, plan, sha)
        expect(code == 3, f"a symlink descendant did not stop deletion (exit {code})")
        expect(os.path.exists(os.path.join(outside, "keep")), "the symlink target was touched")
        quarantined = [path for path in world.quarantines() if os.path.lexists(os.path.join(path, "escape"))]
        expect(len(quarantined) == 1, f"quarantines {world.quarantines()}")
        os.unlink(os.path.join(quarantined[0], "escape"))
        code, _, stderr = world.real_run("replay after the link is removed", alpha, plan, sha)
        expect(code == 0, f"replay exited {code}: {stderr}")
        expect(not world.quarantines(), f"quarantine left: {world.quarantines()}")
        expect(not os.path.lexists(delta_root), "delta remains")
        kappa = world.checkout("kappa")
        kappa_selected = world.adopt(kappa)["links"][0]["selected"]
        kappa_root = kappa_selected["root"]
        kappa_nonce = kappa_selected.get("nonce")
        expect(kappa_nonce and len(kappa_nonce) == 32, f"kappa has no root nonce: {kappa_selected}")
        code, _, stderr = world.oxide(["check"], "kappa build before prune", kappa)
        expect(code == 0, stderr)
        kappa_slot, _ = permanent.slot_state(stderr)
        kappa_pool = "removable pool:" if "removable" in permanent.summary(stderr) else "internal pool:"
        kappa_away = world.vanish(kappa)
        os.rename(kappa_root, kappa_root + ".real")
        os.symlink(kappa_root + ".real", kappa_root)
        _, _, preview = world.plan_file("symlink root dry run", alpha)
        expect(kappa_root not in world.candidates(preview), "a symlink root became a candidate")
        os.unlink(kappa_root)
        os.rename(kappa_root + ".real", kappa_root)
        world.restore(kappa, kappa_away)
        # Reserve: a low reserve at the check before the rename refuses with the
        # root unchanged (exit 2); a low reserve between delete batches of a
        # root with more than one batch of entries stops deletion (exit 3) and
        # exact replay completes. Each injection's firing is recorded.
        lam = world.checkout("lambda")
        lam_root = world.adopt(lam)["links"][0]["selected"]["root"]
        bulk = os.path.join(lam_root, "bulk")
        os.makedirs(bulk)
        for index in range(600):
            with open(os.path.join(bulk, f"f{index:04d}"), "wb") as handle:
                handle.write(b"x")
        world.vanish(lam)
        fired = os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], "prune-low-reserve-fired")
        plan, sha, preview = world.plan_file("reserve dry run", alpha)
        expect(world.candidates(preview) == {lam_root}, f"candidates {world.candidates(preview)}")
        before = tree_bytes(lam_root)
        code, _, stderr = world.real_run("low reserve before rename", alpha, plan, sha, extra={"OXIDE_TEST_PRUNE_LOW_RESERVE_AT": "1"})
        expect(code == 2 and "reserve" in stderr, f"low reserve before rename exited {code}")
        expect(tree_bytes(lam_root) == before and not world.intents(), "a refused reserve changed the root or journaled")
        expect(os.path.isfile(fired), "the low-reserve injection did not fire")
        os.unlink(fired)
        code, _, stderr = world.real_run("low reserve mid-batch", alpha, plan, sha, extra={"OXIDE_TEST_PRUNE_LOW_RESERVE_AFTER_ENTRIES": "1"})
        expect(code == 3 and "reserve" in stderr, f"low reserve mid-batch exited {code}")
        expect(os.path.isfile(fired) and world.intents() and world.quarantines(), "no pending mid-batch stop")
        expect("entries:" in permanent.read_bytes(fired).decode(), "the entry-count injection did not fire")
        # The entry-count trigger fires at the first check after one entry was
        # actually deleted: some entries are gone, none after the stop.
        remaining = sum(len(files) for _, _, files in os.walk(world.quarantines()[0]))
        expect(0 < remaining < 601, f"the reserve stop was not mid-batch ({remaining} entries left)")
        code, _, stderr = world.real_run("replay after the reserve stop", alpha, plan, sha)
        expect(code == 0 and not world.quarantines() and not world.intents(), f"reserve replay exited {code}: {stderr}")
        proof.phase("after custody cases")

    # ---- crash boundaries -------------------------------------------------
    for point, label in (
        ("intent", "after intent"),
        ("renamed", "after rename"),
        ("deleting", "during delete"),
        ("terminal", "after registry terminal"),
    ):
        with case(CRASH_CASES[point]) as scope, scope.run():
            checkout = world.checkout("crash-" + point)
            root = world.adopt(checkout)["links"][0]["selected"]["root"]
            world.vanish(checkout)
            plan, sha, preview = world.plan_file(f"crash {point} dry run", alpha)
            expect(root in world.candidates(preview), world.kept(preview, root))
            code, _, stderr = world.real_run(f"crash {point}", alpha, plan, sha, extra={"OXIDE_TEST_PRUNE_CRASH": point})
            expect(code == 3, f"crash at {point} exited {code}")
            pending = [intent for intent in world.intents() if intent["identity"]["root"] == root]
            expect(len(pending) == 1, f"no pending intent after {point}: {world.intents()}")
            if point == "intent":
                expect(os.path.isdir(root), "the root moved before its rename")
            if point == "renamed":
                expect(not os.path.lexists(root) and world.quarantines(), "no quarantine after the rename")
                # A recreated checkout cannot bind while the intent is pending.
                os.makedirs(os.path.join(checkout, "src"))
                with open(os.path.join(checkout, "Cargo.toml"), "w", encoding="utf-8") as handle:
                    handle.write(PLAIN)
                with open(os.path.join(checkout, "src", "lib.rs"), "w", encoding="utf-8") as handle:
                    handle.write("pub fn v() -> u32 { 1 }\n")
                code, _, stderr = world.oxide(["check"], "recreated checkout during a pending intent", checkout)
                expect(code != 0 and "unsettled" in stderr, f"recreated checkout exit {code}: {stderr}")
                expect(not os.path.lexists(root), "the pending root was recreated")
                shutil.rmtree(checkout)
            if point == "terminal":
                expect(not os.path.lexists(root), "the root remains after its registry terminal state")
                expect(world.receipt(checkout)["state"] == "committed", "the receipt settled before its replay")
            code, _, stderr = world.real_run(f"replay {point}", alpha, plan, sha)
            expect(code == 0, f"replay after {point} exited {code}: {stderr}")
            expect(not os.path.lexists(root) and not world.quarantines(), f"{point}: root or quarantine left")
            expect(not world.intents(), f"{point}: intents left {world.intents()}")
    proof.phase("after crash cases")

    # ---- completion / diagnostics ----------------------------------------
    with case("17") as scope, scope.run():
        # Hard-link control: one 64 KiB inode under two names counts twice by
        # name and once allocated.
        with open(os.path.join(kappa_root, "payload"), "wb") as handle:
            handle.write(os.urandom(64 * 1024))
        os.link(os.path.join(kappa_root, "payload"), os.path.join(kappa_root, "payload-link"))
        expected_logical, expected_allocated = accounting_oracle(kappa_root)
        world.vanish(kappa)
        plan, sha, preview = world.plan_file("completion dry run", alpha)
        expect(kappa_root in world.candidates(preview), world.kept(preview, kappa_root))
        [observed] = [candidate for candidate in preview["candidates"] if candidate["root"] == kappa_root]
        expect(observed["accounting_complete"], observed)
        expect(observed["logical_bytes"] == expected_logical, f"logical {observed['logical_bytes']} != oracle {expected_logical}")
        # Inventory counts regular files only, allocated once per inode: the
        # oracle is exact, and the hard-linked inode is not allocated twice.
        expect(observed["allocated_bytes"] == expected_allocated, f"allocated {observed['allocated_bytes']} != oracle {expected_allocated}")
        expect(preview["coverage"]["source"] and preview["coverage"]["observed_ms"] > 0, preview["coverage"])
        code, stdout, stderr = world.real_run("completion real run", alpha, plan, sha)
        expect(code == 0 and "deleted" in stdout, f"completion exited {code}: {stderr}")
        expect(not os.path.lexists(kappa_root), "kappa remains")
        expect(all(row["root"] != kappa_root for row in world.registry_doc()["rows"]), "kappa's row remains")
        tombstones = permanent.read_json(os.path.join(world.outputs, "tombstones.json"))
        expect(any(entry["root"] == kappa_root for entry in tombstones["entries"]), "no tombstone for kappa")
        observations = permanent.read_json(os.path.join(world.outputs, "prune-observations.json"))
        expect(observations["version"] == "oxide.outputs.prune-observations/1", observations["version"])
        code, stdout, _ = world.oxide(["status"], "status shows the recorded observation", alpha)
        expect(code == 0 and "prune" in stdout, stdout)

    with case("18") as scope, scope.run():
        old_nonce = kappa_nonce
        os.makedirs(kappa)
        shutil.copytree(world.checkout("kappa-src"), kappa, dirs_exist_ok=True)
        code, _, stderr = world.oxide(["outputs", "adopt"], "re-adopt the recreated checkout", kappa)
        expect(code == 0, stderr)
        # The old root's slot is still present and idle (read-only status), so a
        # cold, different slot below proves key separation, not eviction.
        code, stdout, _ = world.oxide(["status"], "old kappa slot still idle", alpha)
        expect(code == 0, "status failed")
        section, idle = None, False
        for line in stdout.splitlines():
            if line.startswith(("internal pool:", "removable pool:")):
                section = line.split(":")[0] + ":"
            elif section == kappa_pool and line.split()[:3] == ["slot", kappa_slot, "idle"] and kappa in line:
                idle = True
        expect(idle, f"the old kappa slot {kappa_slot} ({kappa_pool} {kappa}) is not present and idle before the recreated build")
        new_nonce = world.selected(kappa)["nonce"]
        expect(new_nonce and new_nonce != old_nonce, f"recreated root nonce {new_nonce} reused {old_nonce}")
        code, _, stderr = world.oxide(["check"], "recreated root build", kappa)
        expect(code == 0, stderr)
        slot, state = permanent.slot_state(stderr)
        expect(state == "cold" and slot != kappa_slot, f"the recreated root reused slot {slot} ({state}); key not separated")
        proof.phase("after completion")

    # ---- resolve-intent ---------------------------------------------------
    with case("19") as scope, scope.run():
        rho = world.checkout("rho")
        rho_root = world.adopt(rho)["links"][0]["selected"]["root"]
        world.vanish(rho)
        plan, sha, _ = world.plan_file("rho dry run", alpha)
        code, _, _ = world.real_run("rho crash after rename", alpha, plan, sha, extra={"OXIDE_TEST_PRUNE_CRASH": "renamed"})
        expect(code == 3, f"rho crash exited {code}")
        [intent] = [intent for intent in world.intents() if intent["identity"]["root"] == rho_root]
        for quarantine in world.quarantines():
            shutil.rmtree(quarantine)
        code, stdout, stderr = world.oxide(["outputs", "resolve-intent", intent["id"]], "resolve rho", alpha)
        expect(code == 0 and "resolved by operator" in stdout, stdout + stderr)
        rho_receipt = permanent.read_json(permanent.receipt_file(world.pool, rho))
        expect(rho_receipt["pruned"]["outcome"] == "resolved-by-operator", rho_receipt)
        expect(os.path.isfile(os.path.join(world.outputs, "prune-resolutions", intent["id"] + ".json")), "no resolution receipt")

    with case("20") as scope, scope.run():
        sigma = world.checkout("sigma")
        sigma_root = world.adopt(sigma)["links"][0]["selected"]["root"]
        world.vanish(sigma)
        plan, sha, _ = world.plan_file("sigma dry run", alpha)
        code, _, _ = world.real_run("sigma crash after rename", alpha, plan, sha, extra={"OXIDE_TEST_PRUNE_CRASH": "renamed"})
        expect(code == 3, f"sigma crash exited {code}")
        [intent] = [intent for intent in world.intents() if intent["identity"]["root"] == sigma_root]
        before = json.dumps(intent, sort_keys=True)
        code, _, stderr = world.oxide(["outputs", "resolve-intent", intent["id"]], "resolve sigma", alpha)
        expect(code == 2, f"resolve with a present quarantine exited {code}")
        [after] = [record for record in world.intents() if record["id"] == intent["id"]]
        expect(json.dumps(after, sort_keys=True) == before, "a refused resolution changed the intent")
        code, _, stderr = world.real_run("sigma replay", alpha, plan, sha)
        expect(code == 0, f"sigma replay exited {code}: {stderr}")
        proof.phase("after resolve cases")

    # ---- version-1 whole-target receipts (oxide#152) -----------------------
    with case("21") as scope, scope.run():
        # Both receipts were seeded from the product's own receipt seed in case 01:
        # the committed one has a legacy row (no nonce), the prepared one has a
        # selected row and a pending row. Any positive prune of a legacy identity
        # still needs the explicit operator list and the boot-Data-volume absence
        # proof (the same precondition case 10 uses).
        v1_checkout, v1_root = world.version_one["committed"]
        prepared_checkout, prepared_root = world.version_one["prepared"]
        v1_receipt = world.version_one["committed_receipt"]
        prepared_receipt = world.version_one["prepared_receipt"]
        rows = [row for row in world.registry_doc()["rows"] if row["root"] == v1_root]
        expect(len(rows) == 1 and rows[0]["selected"], rows)
        expect("nonce" not in rows[0], f"the receipt seed invented a nonce: {rows}")
        expect(world.registry_doc()["complete"], "the receipt seed is incomplete")
        expect(permanent.read_bytes(v1_receipt)[:1] == b"{", "the receipt is not JSON")
        expect(b"oxide.outputs.transition/1" in permanent.read_bytes(v1_receipt), "the receipt was not version 1")
        listed = os.path.join(proof.root, "list-version-one.json")
        with open(listed, "w", encoding="utf-8") as handle:
            json.dump([v1_root, prepared_root], handle)
        # A live checkout keeps its legacy root.
        _, _, preview = world.plan_file("version-1 live dry run", alpha, listed)
        expect(v1_root not in world.candidates(preview), "a live checkout's legacy root became a candidate")
        expect("present" in world.kept(preview, v1_root), world.kept(preview, v1_root))
        receipt_before = permanent.read_bytes(v1_receipt)
        prepared_before = permanent.read_bytes(prepared_receipt)
        root_before = tree_bytes(v1_root)
        prepared_root_before = tree_bytes(prepared_root)
        control_before = tree_bytes(world.control)
        world.vanish(v1_checkout)
        prepared_away = world.vanish(prepared_checkout)
        plan, sha, preview = world.plan_file("version-1 vanished dry run", alpha, listed)
        expect(v1_root in world.candidates(preview), f"a version-1 root was kept: {world.kept(preview, v1_root)}")
        expect(preview["operator_list_sha256"] == hashlib.sha256(permanent.read_bytes(listed)).hexdigest(), "the preview does not bind the operator list")
        # The valid noncommitted version-1 receipt keeps its own reference group.
        expect(prepared_root not in world.candidates(preview), "a prepared version-1 root became a candidate")
        reason = world.kept(preview, prepared_root)
        expect("not committed" in reason, reason)
        pending_root = world.version_one["prepared_next"]
        expect(pending_root not in world.candidates(preview), "a pending version-1 root became a candidate")
        reason = world.kept(preview, pending_root)
        expect("prepared transition" in reason, reason)
        # Reading and the dry run are not a migration: no receipt or root byte moved.
        expect(permanent.read_bytes(v1_receipt) == receipt_before, "the dry run rewrote the version-1 receipt")
        expect(permanent.read_bytes(prepared_receipt) == prepared_before, "the dry run rewrote the noncommitted receipt")
        expect(tree_bytes(v1_root) == root_before, "the dry run changed the version-1 root")
        expect(tree_bytes(prepared_root) == prepared_root_before, "the dry run changed the kept root")
        expect(tree_bytes(world.control) == control_before, "the dry run changed control bytes")
        # The exact preview bytes and the same operator list are the real run.
        code, stdout, stderr = world.real_run("version-1 real run", alpha, plan, sha, listed)
        expect(code == 0 and "deleted" in stdout, f"version-1 real run exited {code}: {stderr}")
        expect(not os.path.lexists(v1_root), "the version-1 root remains")
        # The kept namespace is untouched, including its version-1 receipt bytes.
        expect(permanent.read_bytes(prepared_receipt) == prepared_before, "the kept version-1 receipt was rewritten")
        expect(tree_bytes(prepared_root) == prepared_root_before, "the kept root changed")
        world.restore(prepared_checkout, prepared_away)
        # The pruned root's receipt is the current shape and the exact image its
        # intent pinned; version 1 cannot express a prune state.
        published = permanent.read_json(v1_receipt)
        expect(published["version"] == "oxide.outputs.transition/2", published)
        expect(published["state"] == "pruned" and published["pruned"]["outcome"] == "deleted", published)
        expect(published["roots"][0]["root"] == v1_root, published)
        # A fresh dry run offers nothing further and still binds the same list.
        _, _, preview = world.plan_file("version-1 follow-up dry run", alpha, listed)
        expect(v1_root not in world.candidates(preview), "a pruned root returned as a candidate")

    # ---- overflow (last: the sticky flag is never cleared) -----------------
    with case("22") as scope, scope.run():
        doc = world.registry_doc()
        doc["complete"] = False
        doc["incomplete"] = {"reason": "proof overflow", "since_ms": 1, "transaction": doc["created_by"]}
        doc["revision"] += 1
        with open(world.registry, "w", encoding="utf-8") as handle:
            json.dump(doc, handle)
        os.chmod(world.registry, 0o600)
        code, stdout, _ = world.dry_run("overflow dry run", alpha)
        expect(code in (0, 2), f"overflow dry run exited {code}")
        if code == 0:
            expect(not json.loads(stdout)["candidates"], "an overflow registry yielded candidates")
        tau = world.checkout("tau")
        world.adopt(tau)
        tau_root = world.selected(tau)["root"]
        # The same eligible reservation path as case 04: the committed external
        # root's leaf is gone, so the fresh selection is external and its root
        # does not exist; with a sticky-incomplete registry the publication must
        # carry the overflow tag and invent no row.
        shutil.rmtree(tau_root)
        with Starters("case 22") as starters:
            starter, gate = starters.pause(world, tau, "keyed", "overflow starter")
            reserved = current_reservation(world.receipt(tau))
            expect(reserved is not None, f"no reservation: {reserved}")
            expect(reserved.get("overflow", {}).get("registry") == doc["created_by"], reserved)
            expect(
                all(row["root"] != reserved["root"] for row in world.registry_doc()["rows"]),
                "an overflow row was invented",
            )
            release(gate)
            code, _, stderr = starters.finish(world, starter, "overflow starter")
            expect(code == 0, f"overflow starter failed: {stderr}")
    finish_cases()

    # The image's ONE ordinary-detach disposition runs in main()'s finally, so
    # every path (success, fault, taint, abort) records exactly one attempt and
    # never replays it.


def assert_assigned_before_use(function, name: str) -> None:
    """Pure static guard for a defect class the review found once: a name that
    a function only reads (for example in a success summary) but never assigns,
    which would NameError on the path a pure selector never executes."""
    tree = ast.parse(inspect.getsource(function))
    stores = [
        node.lineno
        for node in ast.walk(tree)
        if isinstance(node, ast.Name) and node.id == name and isinstance(node.ctx, ast.Store)
    ]
    loads = [
        node.lineno
        for node in ast.walk(tree)
        if isinstance(node, ast.Name) and node.id == name and isinstance(node.ctx, ast.Load)
    ]
    assert stores, f"{function.__name__} never assigns {name}"
    assert loads, f"{function.__name__} never reads {name}"
    assert min(stores) < min(loads), (function.__name__, name, stores, loads)


def case_matrix_text() -> str:
    """The dependency matrix, generated from CASE_PLAN. The module docstring
    embeds exactly this text, and `--self-test` fails if the two drift, so the
    owning documentation can never describe a different case identity than the
    dispatcher executes."""
    return "\n".join(
        f"    {case_id}  deps {','.join(deps) if deps else '-'}  "
        f"{'taints' if taints else 'no taint'}"
        for case_id, deps, taints in CASE_PLAN
    )


def finish_cases() -> None:
    """The run's own completeness check. The counted run must have executed
    every case; a diagnostic must have recorded an outcome for each one."""
    if DIAG.enabled:
        unreached = [row["id"] for row in DIAG.results() if row["state"] == "unexecuted"]
        if DIAG.aborted is None:
            expect(
                not unreached,
                f"the diagnostic recorded no outcome for cases {unreached}",
            )
        return
    expect(len(DONE) == CASES, f"ran {len(DONE)} cases, not {CASES}")


def receipt_payload(document: dict) -> bytes:
    """The bounded receipt bytes. Detail is trimmed first, so the document
    always stays inside its cap instead of failing the collection."""
    payload = json.dumps(document, indent=2, sort_keys=True).encode()
    if len(payload) <= MAX_DIAGNOSTIC_RECEIPT_BYTES:
        return payload
    trimmed = dict(document)
    trimmed["cases"] = [
        {"id": row["id"], "name": row["name"], "state": row["state"]} for row in document["cases"]
    ]
    trimmed["notes"] = [note[:80] for note in document["notes"]][:4]
    payload = json.dumps(trimmed, indent=2, sort_keys=True).encode()
    expect(
        len(payload) <= MAX_DIAGNOSTIC_RECEIPT_BYTES,
        f"the diagnostic receipt is {len(payload)} bytes over its cap",
    )
    return payload


def write_diagnostic_receipt() -> str | None:
    """Write this run's uncounted receipt beside the guard's own. It is never a
    PASS: `qualified` is false and the mode is diagnostic."""
    if not DIAG.enabled:
        return None
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
    if not os.path.isdir(evidence):
        DIAG.notes.append("the evidence directory is unavailable; no receipt was written")
        return None
    payload = receipt_payload(DIAG.receipt())
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    path = os.path.join(evidence, f"outputs-prune-diagnostic.{stamp}.{secrets.token_hex(3)}.json")
    expect(not os.path.exists(path), f"the diagnostic receipt path {path} already exists")
    with open(path, "wb") as handle:
        handle.write(payload + b"\n")
    os.chmod(path, 0o600)
    print(f"diagnostic receipt: {path} (qualified=false, never a counted PASS)")
    return path


def self_test() -> int:
    """Pure checks of this script's own dispatch and receipt logic.

    No child, staging, fixture, pool, image or supervisor: the count dispatch
    (counted fail-fast, keep-going, taint skips, abort precedence) and the
    bounded unqualified receipt are driven with synthetic bodies.
    """
    global permanent, DIAG
    restore = permanent
    fake = type(
        "FakePermanent",
        (),
        {
            "work_left": staticmethod(lambda: 60.0),
            "expect": staticmethod(
                lambda condition, message: None
                if condition
                else (_ for _ in ()).throw(RuntimeError(message))
            ),
        },
    )
    permanent = fake
    failures = 0

    def check(label: str, body) -> None:
        nonlocal failures
        try:
            body()
        except BaseException as error:  # a self-test failure, never a pass
            failures += 1
            print(f"self-test FAIL {label}: {type(error).__name__}: {error}")

    def drive(script: list, enabled: bool) -> str:
        global DIAG
        DIAG = Diagnostics(enabled=enabled)
        for case_id, body in script:
            with case(case_id) as scope, scope.run():
                body()
        return "completed"

    def fault(message: str) -> None:
        raise RuntimeError(message)

    def abort_case(message: str) -> None:
        raise Aborted(message)

    def table() -> None:
        assert len(CASE_PLAN) == CASES == len(CASE_NAMES) == sum(
            count for _, count in CASE_ALLOCATION
        )
        assert [entry[0] for entry in CASE_PLAN] == [f"{index:02d}" for index in range(1, CASES + 1)]
        for case_id, deps, _ in CASE_PLAN:
            assert all(dep in CASE_INDEX and dep < case_id for dep in deps), (case_id, deps)
        assert CRASH_CASES == {"intent": "13", "renamed": "14", "deleting": "15", "terminal": "16"}
        assert CASE_NAMES[12:16] == [f"crash {label}: pending, then exact replay" for label in (
            "after intent",
            "after rename",
            "during delete",
            "after registry terminal",
        )]

    check("case table", table)

    def documented_matrix() -> None:
        # The owning docstring must describe exactly the bodies this run
        # dispatches: same ids, same order, same names, same dependencies.
        assert case_matrix_text() in (__doc__ or ""), "the docstring matrix drifted from CASE_PLAN"
        documented = [line.strip() for line in (__doc__ or "").splitlines() if line.startswith("  21 ")
                      or line.startswith("  22 ")]
        assert documented and documented[0].startswith("21 version-1"), documented
        assert documented[1].startswith("22 overflow"), documented
        assert "cases 10 and 21 prove" in (__doc__ or "")

    check("documented case matrix", documented_matrix)

    def helper_compatibility() -> None:
        # The shared installed-handoff staging helper gained only an optional
        # exact capability expectation: its default (None) still means this
        # helper's own declared versions, so its other caller is unchanged. The
        # encoded-RUSTFLAGS scrub is premise 7 and applies to every caller.
        helper = _load("installed_handoff_selftest", "test-installed-handoff.py")
        parameters = inspect.signature(helper.stage_subject).parameters
        assert list(parameters) == ["root", "capability"], list(parameters)
        assert parameters["capability"].default is None, parameters["capability"].default
        assert "CARGO_ENCODED_RUSTFLAGS" in inspect.getsource(helper.stage_subject)
        # This proof pins the current control format literally; a product bump
        # must update this constant, case 01 and the staged capability together.
        assert CONTROL_STATE_VERSION == 4 and OWNED_OUTPUTS_FENCE == "oxide.outputs/3"
        assert CANDIDATE_CAPABILITY == {
            "control_state_version": CONTROL_STATE_VERSION,
            "registration_version": 2,
            "test_controls": True,
        }

    check("shared staging helper compatibility", helper_compatibility)

    def artifact_inputs() -> None:
        # The older-binary fence control's INPUT is the durable recovered
        # artifact. The original receipt's recorded installation path is
        # provenance text only (that path now holds the current 4df3dfd
        # binary), so the two must never be the same constant and the proof
        # must read/execute only the durable copy.
        assert OLD_INSTALLED_PATH == "/Users/tom/.local/bin/oxide", OLD_INSTALLED_PATH
        assert OLD_ARTIFACT_PATH.startswith(
            "/Users/tom/.local/share/northstar/release-audits/oxide/2026-10-25/"
            "d246d400-artifact/"
        ), OLD_ARTIFACT_PATH
        assert OLD_ARTIFACT_PATH.endswith("/old-oxide-d246d400"), OLD_ARTIFACT_PATH
        assert OLD_ARTIFACT_PATH != OLD_INSTALLED_PATH
        assert (
            OLD_PIN,
            OLD_SHA,
            OLD_SIZE,
        ) == (
            "d246d400f5ccb8e0a601ea6415b93b8346bc2698",
            "bd52cb42842c06451b0f8a9bc364ba7de74cfe6f014d33804890c4adbc51fc2b",
            2_756_576,
        ), (OLD_PIN, OLD_SHA, OLD_SIZE)
        body = inspect.getsource(_copy_old_artifact)
        assert "read_nofollow(OLD_ARTIFACT_PATH" in body, body
        assert "read_nofollow(OLD_INSTALLED_PATH" not in body, body
        assert 'installed.get("path") == OLD_INSTALLED_PATH' in body, body
        assert 'os.path.join(root, "old-oxide-d246d400")' in body, body
        assert "hashlib.sha256(binary).hexdigest() == OLD_SHA" in body, body
        assert "((OLD_RECEIPT, receipt_id), (OLD_ARTIFACT_PATH, binary_id))" in body, body
        case_one = inspect.getsource(run)
        assert "copy_old_artifact(proof.root)" in case_one, "case 01 does not verify the artifact"
        assert "probe = proof.run([old, CAPABILITIES_PROBE]" in case_one, case_one
        assert 'json.loads(probe.stdout).get("control_state_version") == 2' in case_one, case_one
        assert '(["check"], ["outputs", "adopt"])' in case_one

    check("older-binary artifact input path", artifact_inputs)

    class MemoryEffects:
        """In-memory stand-in for FixtureEffects: the fixture's private
        container seam driven without a mount, image, child or host effect.
        `umask` models the process umask that produced the OLD implicit
        intermediate mode."""

        def __init__(self, mount_dev: int = 7, euid: int = 501, umask: int = 0o022):
            self.entries: dict[str, dict] = {}
            self.order: list[tuple] = []
            self.mount_dev = mount_dev
            self.uid = euid
            self.umask = umask

        def _entry(self, path: str, mode: int, dev: int | None = None) -> None:
            self.entries[path] = {
                "mode": mode,
                "uid": self.uid,
                "dev": self.mount_dev if dev is None else dev,
            }

        def mkdir(self, path: str, mode: int) -> None:
            if path in self.entries:
                raise FileExistsError(path)
            self._entry(path, mode & ~self.umask)
            self.order.append(("mkdir", path, self.entries[path]["mode"]))

        def open_dir(self, path: str) -> str:
            if path not in self.entries:
                raise FileNotFoundError(path)
            self.order.append(("open_dir", path))
            return path

        def fstat(self, fd: str):
            entry = self.entries[fd]
            return type(
                "Facts",
                (),
                {
                    "st_mode": stat.S_IFDIR | entry["mode"],
                    "st_uid": entry["uid"],
                    "st_dev": entry["dev"],
                },
            )()

        def fchmod(self, fd: str, mode: int) -> None:
            self.entries[fd]["mode"] = mode
            self.order.append(("fchmod", fd, mode))

        def stat(self, path: str):
            if path == "/mnt":
                return type(
                    "Facts",
                    (),
                    {"st_mode": stat.S_IFDIR, "st_uid": self.uid, "st_dev": self.mount_dev},
                )()
            entry = self.entries[path]
            return type(
                "Facts",
                (),
                {
                    "st_mode": stat.S_IFDIR | entry["mode"],
                    "st_uid": entry["uid"],
                    "st_dev": entry["dev"],
                },
            )()

        def close(self, fd: str) -> None:
            self.order.append(("close", fd))

        def euid(self) -> int:
            return self.uid

    def owner_only_ok(effects, path: str) -> bool:
        """The product's own predicate (volume::dir_at): a real directory,
        owned by this user, with no group or other permission bits."""
        facts = effects.stat(path)
        return (
            stat.S_ISDIR(facts.st_mode)
            and facts.st_uid == effects.euid()
            and facts.st_mode & 0o077 == 0
        )

    def private_container() -> None:
        container = "/mnt/oxide-outputs"
        # (a) The fixed setup: an exclusive 0700 container first, so the
        # product's predicate holds for it.
        effects = MemoryEffects()
        created = create_private_outputs_container(container, "/mnt", effects)
        assert created == container
        assert effects.entries[container]["mode"] == 0o700, effects.entries[container]
        assert effects.order[0][:2] == ("mkdir", container), effects.order
        assert owner_only_ok(effects, container)
        # (b) The OLD premise is now a proven failure: a nested root created
        # first makes the intermediate container 0755, which the product
        # refuses. This is the counterfactual the fixture correction removes.
        old = MemoryEffects()
        old._entry(container, 0o777 & ~old.umask)  # Python's makedirs intermediate
        assert old.entries[container]["mode"] == 0o755, old.entries[container]
        assert not owner_only_ok(old, container), "the 0755 premise passed the product predicate"
        # (c) An unexpected pre-existing container refuses; nothing is adopted
        # or chmodded, not even an already owner-only one.
        for mode in (0o755, 0o700):
            existing = MemoryEffects()
            existing._entry(container, mode)
            before = dict(existing.entries[container])
            try:
                create_private_outputs_container(container, "/mnt", existing)
            except Aborted as error:
                assert "already exists" in str(error), error
            else:
                raise AssertionError("an existing container was adopted")
            assert existing.entries[container] == before, existing.entries[container]
            assert ("fchmod", container, 0o700) not in existing.order, existing.order
        # (d) A container that does not end up as this user's owner-only
        # directory on the mount's device refuses instead of being used; the
        # only mode repair is through the descriptor this call created.
        class Tampered(MemoryEffects):
            def __init__(self, *, dev: int | None = None, uid: int | None = None, mode: int | None = None):
                super().__init__()
                self.tampered_dev = dev
                self.tampered_uid = uid
                self.tampered_mode = mode

            def mkdir(self, path: str, mode: int) -> None:
                super().mkdir(path, mode)
                if self.tampered_dev is not None:
                    self.entries[path]["dev"] = self.tampered_dev
                if self.tampered_uid is not None:
                    self.entries[path]["uid"] = self.tampered_uid
                if self.tampered_mode is not None:
                    self.entries[path]["mode"] = self.tampered_mode

        for tampered, expected in (
            (Tampered(dev=8), "another device"),
            (Tampered(uid=502), "not owned by this user"),
        ):
            try:
                create_private_outputs_container(container, "/mnt", tampered)
            except Aborted as error:
                assert expected in str(error), error
            else:
                raise AssertionError(f"a tampered container was accepted ({expected})")
        repaired = Tampered(mode=0o755)
        create_private_outputs_container(container, "/mnt", repaired)
        assert repaired.entries[container]["mode"] == 0o700, repaired.entries[container]
        assert ("fchmod", container, 0o700) in repaired.order, repaired.order
        # (e) The seam is what the fixture actually calls, and it is called
        # before the first nested root, with no exist_ok/chmod shortcut.
        fixture_source = inspect.getsource(World.version_one_fixture)
        assert "self.private_outputs_container()" in fixture_source, fixture_source
        assert fixture_source.index("self.private_outputs_container()") < fixture_source.index(
            "root_for("
        ), "the private container is not created before the nested roots"
        assert "exist_ok" not in fixture_source.replace(
            'os.makedirs(self.outputs, mode=0o700, exist_ok=True)', ""
        ), "a broad exist_ok shortcut remains in the version-1 fixture"
        seam = inspect.getsource(create_private_outputs_container)
        assert "effects.mkdir(outer, 0o700)" in seam, seam
        assert "FileExistsError" in seam and "exist_ok" not in seam, seam
        assert "effects.fchmod(fd, 0o700)" in seam, seam
        assert "effects.open_dir(outer)" in seam, seam
        # The no-follow, descriptor-anchored effects it uses.
        assert "O_NOFOLLOW" in inspect.getsource(FixtureEffects.open_dir)
        assert "os.fchmod" in inspect.getsource(FixtureEffects.fchmod)
        parameters = inspect.signature(create_private_outputs_container).parameters
        assert parameters["effects"].default is REAL_FIXTURE_EFFECTS, parameters["effects"]

    check("private fixture output container", private_container)

    def registry_oracle() -> None:
        # (a) The production oracle accepts exactly the current 2/2 complete
        # document.
        current = {"version": REGISTRY_VERSION, "schema": REGISTRY_SCHEMA, "complete": True}
        assert registry_is_current(current), current
        # (b) Stale, mixed, unknown, missing and incomplete documents all fail:
        # the oracle never accepts "either format", a mixed pair, or the
        # document it is checking as its own expectation.
        refuse = [
            {"version": "oxide.outputs.registry/1", "schema": 1, "complete": True},
            {"version": REGISTRY_VERSION, "schema": 1, "complete": True},
            {"version": "oxide.outputs.registry/1", "schema": REGISTRY_SCHEMA, "complete": True},
            {"version": REGISTRY_VERSION, "schema": REGISTRY_SCHEMA, "complete": False},
            {"version": REGISTRY_VERSION, "schema": REGISTRY_SCHEMA, "complete": 1},
            {"version": REGISTRY_VERSION, "schema": REGISTRY_SCHEMA},
            {"version": REGISTRY_VERSION, "complete": True},
            {"schema": REGISTRY_SCHEMA, "complete": True},
            {"version": "oxide.outputs.registry/3", "schema": 3, "complete": True},
            {"version": REGISTRY_VERSION, "schema": str(REGISTRY_SCHEMA), "complete": True},
            {"version": REGISTRY_VERSION, "schema": REGISTRY_SCHEMA + 1, "complete": True},
            {},
        ]
        for document in refuse:
            assert not registry_is_current(document), document
        # (c) The pinned pair must equal the FROZEN product's own literal
        # constants, read from source (bounded) rather than assumed: a product
        # bump fails this proof loudly instead of being followed silently.
        source_path = os.path.join(os.path.dirname(HERE), "src", "bin", "oxide", "prune.rs")
        with open(source_path, encoding="utf-8") as handle:
            text = handle.read(2 * 1024 * 1024)
        version = re.search(
            r'^pub const REGISTRY_VERSION: &str = "([^"]+)";$', text, re.MULTILINE
        )
        schema = re.search(r"^const REGISTRY_SCHEMA: u32 = (\d+);$", text, re.MULTILINE)
        assert version is not None, "prune.rs no longer declares REGISTRY_VERSION"
        assert schema is not None, "prune.rs no longer declares REGISTRY_SCHEMA"
        assert version.group(1) == REGISTRY_VERSION, (version.group(1), REGISTRY_VERSION)
        assert int(schema.group(1)) == REGISTRY_SCHEMA, (schema.group(1), REGISTRY_SCHEMA)
        # The legacy pair stays a distinct readable truth, never this proof's
        # expected current write.
        legacy_version = re.search(
            r'^const LEGACY_REGISTRY_VERSION: &str = "([^"]+)";$', text, re.MULTILINE
        )
        legacy_schema = re.search(
            r"^const LEGACY_REGISTRY_SCHEMA: u32 = (\d+);$", text, re.MULTILINE
        )
        assert legacy_version is not None and legacy_schema is not None
        assert REGISTRY_VERSION != legacy_version.group(1)
        assert REGISTRY_SCHEMA != int(legacy_schema.group(1))
        # (d) The production case-01 body calls the same predicate.
        case_one = inspect.getsource(run)
        assert "expect(registry_is_current(registry), registry)" in case_one, case_one
        assert 'registry["version"] ==' not in case_one, "case 01 still compares the version inline"

    check("current registry format oracle", registry_oracle)

    def receipt_field_paths() -> None:
        # (a) The current-write reservation is ROOT-LEVEL. A profile receipt
        # (`oxide.outputs.transition/3`) carries `reserved` at the top level; a
        # placement-level reservation belongs to the legacy whole-target
        # `transition/2` document and must never satisfy this oracle.
        reservation = {"id": "a" * 32, "nonce": "b" * 32}
        assert current_reservation({"reserved": reservation}) is reservation
        assert current_reservation(
            {"links": [{"member": "debug", "reserved": reservation}]}
        ) is None, "a stale per-placement reservation satisfied the current oracle"
        # Both shapes present: the current root-level field wins and nothing is
        # merged from the legacy placement field.
        mixed = {"reserved": reservation, "links": [{"member": "debug", "reserved": {"id": "c" * 32}}]}
        assert current_reservation(mixed) is reservation
        assert current_reservation({"links": [{"member": "debug"}]}) is None
        assert current_reservation({}) is None
        # (b) Tie the rule to the FROZEN product's serializer: the placement
        # field is emitted only for the whole-target version and the root-level
        # field for the others, and those versions are /2 and /3.
        source_path = os.path.join(os.path.dirname(HERE), "src", "bin", "oxide", "outputs.rs")
        with open(source_path, encoding="utf-8") as handle:
            text = handle.read(2 * 1024 * 1024)
        assert 'const RECEIPT_VERSION: &str = "oxide.outputs.transition/2";' in text
        assert 'const PROFILE_RECEIPT_VERSION: &str = "oxide.outputs.transition/3";' in text
        assert "let legacy = self.version == RECEIPT_VERSION;" in text, "serializer layout rule changed"
        assert "reserved: legacy.then_some(self.reserved.as_ref()).flatten()," in text, (
            "the per-placement reservation is no longer gated on the legacy version"
        )
        assert "reserved: (!legacy).then_some(self.reserved.as_ref()).flatten()," in text, (
            "the root-level reservation is no longer the current write"
        )
        # (c) The production cases use the accessor, and no case still reads a
        # placement-level reservation or assumes one registry row per root.
        case_source = inspect.getsource(run)
        assert '["links"][0]["reserved"]' not in case_source, case_source
        assert '["links"][0].get("reserved")' not in case_source, case_source
        assert case_source.count("current_reservation(") >= 5, case_source
        # Case 03 no longer assumes one registry row per root; case 21 keeps
        # its own legacy whole-target one-row assertion (the legacy receipt
        # places exactly one `target` link).
        assert 'rows[0]["nonce"] == nonce and rows[0]["selected"]' not in case_source, (
            case_source
        )
        assert 'sorted(row["member"] for row in rows) == ["debug", "release"]' in case_source
        # (case 21 keeps its own legacy whole-target one-row assertion.)
        assert 'expect(len(rows) == 1 and rows[0]["selected"], rows)' in case_source
        # (d) A current profile receipt places one link per member, so the
        # registry's per-member rows are the current shape (prune.rs
        # note_placement keys rows by checkout, member and root).
        prune_path = os.path.join(os.path.dirname(HERE), "src", "bin", "oxide", "prune.rs")
        with open(prune_path, encoding="utf-8") as handle:
            prune = handle.read(2 * 1024 * 1024)
        assert "row.checkout_id == checkout_id && row.member == member" in prune, (
            "registry rows are no longer keyed by member"
        )

    check("receipt field paths", receipt_field_paths)

    def image_custody() -> None:
        # (a) The evidence directory is verified BEFORE any effect.
        class MemoryCustody(CustodyEffects):
            def __init__(self, *, mode: int = 0o700, uid: int = 501, sibling: bool = True):
                self.paths = {"/root/evidence", "/root/scratch" if sibling else "/other/scratch"}
                self.modes = {
                    "/root/evidence": mode,
                    "/root/scratch" if sibling else "/other/scratch": 0o700,
                }
                self.uids = {"/root/evidence": uid}
                self.created: list[str] = []
                self.attached: list[str] = []
                self.detached: list[str] = []
                self.removed: list[str] = []
                self.records: list[str] = []
                self.mounted = False
                self.fail_attach = False
                self.detach_result = (0, "")
                self.leave_mount = False

            def realpath(self, path: str) -> str:
                return path

            def stat(self, path: str):
                if path not in self.paths:
                    raise FileNotFoundError(path)
                return type(
                    "Facts",
                    (),
                    {
                        "st_mode": stat.S_IFDIR | self.modes.get(path, 0o700),
                        "st_uid": self.uids.get(path, 501),
                        "st_dev": 1,
                    },
                )()

            def exists(self, path: str) -> bool:
                if path in ("/Volumes/OxMem1", "/dev/disk9s1"):
                    return self.mounted
                return path in self.paths

            def write_record(self, path: str, text: str) -> None:
                self.records.append(path)
                self.paths.add(path)

            def create_image(self, path: str, name: str) -> None:
                self.created.append(path)
                self.paths.add(path)

            def attach(self, path: str) -> dict:
                if self.fail_attach:
                    raise RuntimeError("attach boom")
                self.attached.append(path)
                self.mounted = True
                return {"mount": "/Volumes/OxMem1", "device": "/dev/disk9s1"}

            def detach(self, mount: str) -> tuple:
                self.detached.append(mount)
                code, error = self.detach_result
                if code == 0 and not self.leave_mount:
                    self.mounted = False
                return self.detach_result

            def remove(self, path: str) -> None:
                self.removed.append(path)
                self.paths.discard(path)

        def refuses(evidence: str, scratch: str, label: str) -> None:
            effects = MemoryCustody()
            custody = ImageCustody("cap", effects)
            try:
                custody.allocate(evidence, scratch)
            except Aborted:
                pass
            else:
                raise AssertionError(f"{label}: a bad evidence root was accepted")
            assert effects.created == [] and effects.attached == [], (label, effects.created, effects.attached)
            assert effects.records == [], (label, effects.records)

        refuses("", "/root/scratch", "missing evidence")
        refuses("/root/evidence", "", "missing scratch")
        refuses("/root/../root/evidence", "/root/scratch", "non-canonical")
        refuses(os.path.dirname(HERE) + "/evidence", os.path.dirname(HERE) + "/scratch", "workspace")
        refuses("/root/evidence", "/other/scratch", "non-sibling")
        for label, mode, uid in (("group-readable", 0o755, 501), ("foreign-owner", 0o700, 502)):
            effects = MemoryCustody(mode=mode, uid=uid)
            custody = ImageCustody("cap", effects)
            try:
                custody.allocate("/root/evidence", "/root/scratch")
            except Aborted:
                pass
            else:
                raise AssertionError(f"{label}: a bad evidence directory was accepted")
            assert effects.created == [] and effects.attached == []

        # (b) Success: record before attach, one completed detach, backing
        # removed only after the positive post-state, record kept.
        effects = MemoryCustody()
        custody = ImageCustody("cap", effects)
        custody.allocate("/root/evidence", "/root/scratch")
        assert effects.records and not effects.created and not effects.attached
        mount = custody.attach()
        assert mount == "/Volumes/OxMem1" and effects.created and effects.attached
        assert custody.detach_once(True) == "completed"
        assert effects.removed == [custody.backing], effects.removed
        assert effects.records[-1] == custody.record
        assert custody.detach_once(True) == "completed" and effects.detached == [mount]

        # (c) Unclosed sessions: no OS detach, backing kept, no replay later.
        effects = MemoryCustody()
        custody = ImageCustody("cap", effects)
        custody.allocate("/root/evidence", "/root/scratch")
        custody.attach()
        assert custody.detach_once(False) == "refused:unclosed-sessions"
        assert effects.detached == [] and effects.removed == []
        assert custody.detach_once(True) == "refused:unclosed-sessions"
        assert effects.detached == [] and effects.removed == []

        # (d) Unknown detach outcomes keep the backing and are never replayed.
        for configured in ((1, "resource busy"), (0, "")):
            effects = MemoryCustody()
            effects.detach_result = configured
            effects.leave_mount = configured[0] == 0
            custody = ImageCustody("cap", effects)
            custody.allocate("/root/evidence", "/root/scratch")
            custody.attach()
            assert custody.detach_once(True) == "unknown", configured
            assert effects.removed == [], configured
            assert custody.detach_once(True) == "unknown" and len(effects.detached) == 1

        # (e) No attachment, no detach; an outer closure never authorizes it.
        effects = MemoryCustody()
        custody = ImageCustody("cap", effects)
        custody.allocate("/root/evidence", "/root/scratch")
        assert custody.detach_once(True) == "refused:not-attached"
        assert effects.detached == [] and effects.removed == []
        # attach failure keeps the backing and refuses a later detach
        failing = MemoryCustody()
        failing.fail_attach = True
        broken = ImageCustody("cap", failing)
        broken.allocate("/root/evidence", "/root/scratch")
        try:
            broken.attach()
        except Aborted:
            pass
        else:
            raise AssertionError("a failed attach was reported as attached")
        assert broken.state == "unknown" and failing.attached == []
        assert broken.detach_once(True) == "refused:not-attached" and failing.detached == []

        # (f) The production run allocates before attaching and performs its one
        # detach in main's finally, never through the shared helper's own path.
        run_source = inspect.getsource(run)
        assert run_source.index("custody.allocate(") < run_source.index("world.mount = custody.attach()")
        main_source = inspect.getsource(main)
        assert "finally:" in main_source and "custody.detach_once(" in main_source
        assert main_source.index("custody.detach_once(") > main_source.index("finally:")
        assert "proof.detach()" not in run_source and "cleanup_after_failure" not in main_source

    check("image custody", image_custody)

    def success_path_names() -> None:
        # The clean-finish summaries and the failure record must only read
        # names their function assigns; a dropped assignment once NameError'd
        # a clean collection after the detach (review of b58542).
        for name in ("elapsed", "custody", "disposition", "failure", "proof", "root", "receipt"):
            assert_assigned_before_use(main, name)
        main_source = inspect.getsource(main)
        assert "elapsed = time.monotonic() - permanent.layout.runner.STARTED" in main_source
        assert main_source.count("return 0") >= 2, main_source
        assert "raise failure" in main_source

    check("success-path names", success_path_names)

    def starters() -> None:
        released: list[str] = []

        class Child:
            pass

        class FakeWorld:
            def __init__(self, fail: bool = False):
                self.finished: list[str] = []
                self.fail = fail

            def finish(self, child, label: str) -> tuple:
                if self.fail:
                    raise Aborted(f"{label}: cannot prove closure")
                self.finished.append(label)
                return (0, "", "")

        # Both concurrent starters are released and awaited when a case faults.
        world = FakeWorld()
        group = Starters("case 04", release_gate=released.append)
        first, second = Child(), Child()
        group.entries.extend([(first, "/g/one", "one"), (second, "/g/two", "two")])
        group.close(world)
        assert released == ["/g/one", "/g/two"], released
        assert world.finished == ["one", "two"], world.finished
        assert group.entries == [] and group.unknown == []
        # An await that cannot prove closure is recorded UNKNOWN, never guessed.
        DIAG.notes.clear()
        unproven = FakeWorld(fail=True)
        group = Starters("case 05", release_gate=released.append)
        group.entries.append((Child(), "/g/three", "three"))
        try:
            group.close(unproven)
        finally:
            pass
        assert group.unknown, group.unknown
        assert any("starter custody unknown" in note for note in DIAG.notes), DIAG.notes
        # A case that otherwise succeeded turns an unproven starter into Aborted.
        global _START_WORLD
        _START_WORLD = FakeWorld(fail=True)
        group = Starters("case 22", release_gate=released.append)
        group.entries.append((Child(), "/g/four", "four"))
        try:
            with group:
                pass
        except Aborted:
            pass
        else:
            raise AssertionError("an unproven starter was accepted on a successful case")
        finally:
            _START_WORLD = None
        # The production cases use the tracked helper for every keyed pause and
        # never call the untracked starter path directly.
        case_source = inspect.getsource(run)
        assert "starters.pause(" in case_source, case_source
        assert "starters.finish(" in case_source, case_source
        assert "world.paused(" not in case_source, case_source

    check("keyed starter custody", starters)

    def counted_fail_fast() -> None:
        ran: list[str] = []
        try:
            drive(
                [
                    ("01", lambda: ran.append("01")),
                    ("03", lambda: fault("counted oracle failure")),
                    ("06", lambda: ran.append("06")),
                ],
                False,
            )
        except RuntimeError as error:
            assert str(error) == "counted oracle failure", error
        else:
            raise AssertionError("the counted run suppressed its first fault")
        assert ran == ["01"], ran
        assert DIAG.states.get("01") == "completed" and DIAG.states.get("03") == "failed", DIAG.states
        assert (DIAG.first_fault or "").startswith("case 03:"), DIAG.first_fault
        assert not DIAG.enabled

    check("counted fail-fast", counted_fail_fast)

    def keep_going() -> None:
        ran: list[str] = []
        drive(
            [
                ("01", lambda: ran.append("01")),
                ("06", lambda: fault("recoverable oracle failure")),
                ("03", lambda: ran.append("03")),
                ("08", lambda: ran.append("08")),
                ("09", lambda: ran.append("09")),
            ],
            True,
        )
        # 03 is independent of the failed read-only case and still runs; 08 and
        # 09 depend on 07, which never ran, and are skipped rather than guessed.
        assert ran == ["01", "03"], ran
        assert DIAG.states["06"] == "failed" and DIAG.states["03"] == "completed"
        assert DIAG.states["08"] == "skipped" and DIAG.states["09"] == "skipped"
        assert "dependency 07" in DIAG.details["08"], DIAG.details["08"]
        assert DIAG.first_fault == "case 06: recoverable oracle failure", DIAG.first_fault
        assert DIAG.taint is None

    check("diagnostic keep-going and dependency skip", keep_going)

    def taint_skips() -> None:
        ran: list[str] = []
        drive(
            [
                ("01", lambda: ran.append("01")),
                ("06", lambda: ran.append("06")),
                ("07", lambda: fault("registry state uncertain now")),
                ("03", lambda: ran.append("03")),
                ("08", lambda: ran.append("08")),
            ],
            True,
        )
        # 03 is independent of the failed case, but a registry/control taint is
        # state the whole fixture shares: every later case is skipped.
        assert ran == ["01", "06"], ran
        assert DIAG.taint is not None and "case 07" in DIAG.taint, DIAG.taint
        for case_id in ("03", "08"):
            assert DIAG.states[case_id] == "skipped", DIAG.states
            assert "not trusted" in DIAG.details[case_id], DIAG.details[case_id]

    check("tainted state skips later cases", taint_skips)

    def abort_after_fault() -> None:
        ran: list[str] = []
        try:
            drive(
                [
                    ("01", lambda: ran.append("01")),
                    ("06", lambda: fault("first oracle failure")),
                    ("03", lambda: abort_case("child custody unknown")),
                    ("08", lambda: ran.append("08")),
                ],
                True,
            )
        except Aborted as error:
            assert str(error) == "child custody unknown", error
        else:
            raise AssertionError("a custody abort was suppressed")
        assert ran == ["01"], ran
        states = {row["id"]: row["state"] for row in DIAG.results()}
        assert states["06"] == "failed" and states["03"] == "aborted", states
        assert states["08"] == "unexecuted", states
        assert DIAG.first_fault == "case 06: first oracle failure", DIAG.first_fault
        assert DIAG.aborted is not None and "child custody unknown" in DIAG.aborted

    check("abort after a prior fault keeps the first fault", abort_after_fault)

    def deadline_aborts() -> None:
        global permanent, DIAG
        ran = type(fake)(
            "DeadlinePermanent",
            (),
            {
                "work_left": staticmethod(
                    lambda: (_ for _ in ()).throw(
                        RuntimeError("work deadline reached before the runner's shutdown margin")
                    )
                )
            },
        )
        permanent = ran
        DIAG = Diagnostics(enabled=True)
        try:
            with case("01") as scope, scope.run():
                raise AssertionError("a body ran past the deadline")
        except Aborted as error:
            assert "work deadline" in str(error), error
        else:
            raise AssertionError("the deadline was treated as an oracle failure")
        assert DIAG.states["01"] == "aborted" and DIAG.aborted is not None
        permanent = fake

    check("deadline aborts", deadline_aborts)

    def receipts() -> None:
        global DIAG
        DIAG = Diagnostics(enabled=True)
        DIAG.source = "0" * 40
        for case_id, body in [
            ("01", lambda: None),
            ("06", lambda: fault("x" * 5000)),
        ]:
            try:
                with case(case_id) as scope, scope.run():
                    body()
            except RuntimeError:
                pass
        document = DIAG.receipt()
        assert document["qualified"] is False and document["mode"] == "diagnostic"
        assert document["completed"] == ["01"] and document["failed"] == ["06"], document
        assert document["unexecuted"] == [
            entry[0] for entry in CASE_PLAN if entry[0] not in ("01", "06")
        ], document["unexecuted"]
        assert len(document["cases"]) == CASES
        assert len(document["first_fault"]) <= MAX_FAULT_TEXT
        assert document["source"] == "0" * 40
        payload = receipt_payload(document)
        assert len(payload) <= MAX_DIAGNOSTIC_RECEIPT_BYTES, len(payload)
        assert json.loads(payload)["qualified"] is False

    check("bounded unqualified receipt", receipts)

    def counted_never_writes() -> None:
        global DIAG
        DIAG = Diagnostics(enabled=False)
        assert write_diagnostic_receipt() is None

    check("counted mode writes no diagnostic receipt", counted_never_writes)

    permanent = restore
    DIAG = Diagnostics(enabled=False)
    if failures:
        print(f"test-outputs-prune self-test: {failures} check(s) failed", file=sys.stderr)
        return 1
    print(
        f"test-outputs-prune self-test: dispatch, skips, aborts and receipt logic hold "
        f"({CASES} cases, {sum(len(deps) for _, deps, _ in CASE_PLAN)} dependencies)"
    )
    return 0


def main(argv: list[str]) -> int:
    global permanent, handoff, expect, fail, DIAG
    if argv == ["--plan"]:
        return plan()
    if argv == ["--self-test"]:
        return self_test()
    if argv == ["--diagnostic"]:
        DIAG = Diagnostics(enabled=True)
    elif argv:
        print(f"test-outputs-prune: unknown arguments {argv!r}", file=sys.stderr)
        return 2
    else:
        DIAG = Diagnostics(enabled=False)
    permanent = _load("permanent_outputs", "test-permanent-outputs.py")
    handoff = _load("installed_handoff", "test-installed-handoff.py")
    expect, fail = permanent.expect, permanent.fail
    if sys.platform != "darwin":
        fail("the outputs-prune proof requires macOS")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    expect(bool(scratch) and os.path.realpath(scratch) == scratch, f"scratch {scratch!r} is not canonical")
    for name in ("TMPDIR", "TMP", "TEMP"):
        expect(os.environ.get(name) == scratch, f"{name} differs from the supervisor scratch")
    expect(
        os.path.isdir(os.environ.get("OXIDE_PROOF_EVIDENCE", "")) and "OXIDE_PROOF_DEADLINE_NS" in os.environ,
        "not under the qualified shared proof supervisor",
    )
    root = tempfile.mkdtemp(prefix="outputs-prune.", dir=scratch)
    repo = os.path.dirname(HERE)
    signal.signal(signal.SIGTERM, permanent.terminated)
    signal.signal(signal.SIGALRM, permanent.overdue)
    signal.setitimer(signal.ITIMER_REAL, permanent.work_left())
    custody = ImageCustody("OxPr" + secrets.token_hex(4))
    proof = permanent.Proof(root)
    failure = None
    try:
        run(proof, repo, custody)
    except BaseException as error:
        failure = error
    finally:
        # ONE ordinary-detach disposition for the one attachment, on every
        # path, recorded in the custody record and never replayed.
        signal.setitimer(signal.ITIMER_REAL, 0)
        disposition = custody.detach_once(sessions_closed=not proof.unclosed)
    DIAG.notes.append(f"image custody {custody.state} ({custody.mount or 'no mount'}): {disposition}")
    if failure is not None:
        DIAG.notes.append(f"run ended: {type(failure).__name__}: {failure}")
        receipt = write_diagnostic_receipt()
        permanent.record(
            "result.txt",
            f"{'diagnostic collection' if DIAG.enabled else 'failed'} after {len(DONE)} case(s) {DONE}: {failure!r}\n"
            f"case states: {DIAG.states}\nfirst fault: {DIAG.first_fault}\nabort: {DIAG.aborted}\n"
            f"image custody: {custody.state} disposition {disposition}\n"
            f"custody record: {custody.record}\nunclosed children: {sorted(proof.unclosed)}\n"
            f"root: {root}\n",
        )
        print(f"outputs-prune proof failed; scratch {root} kept", file=sys.stderr)
        raise failure
    elapsed = time.monotonic() - permanent.layout.runner.STARTED
    if DIAG.enabled:
        receipt = write_diagnostic_receipt()
        unreached = [row["id"] for row in DIAG.results() if row["state"] == "unexecuted"]
        line = (
            f"outputs-prune diagnostic collected (UNCOUNTED, never a PASS): "
            f"{len(DIAG.results()) - len(unreached)}/{CASES} cases recorded, "
            f"failed {[row['id'] for row in DIAG.results() if row['state'] == 'failed']}, "
            f"skipped {[row['id'] for row in DIAG.results() if row['state'] == 'skipped']}, "
            f"unexecuted {unreached}; {elapsed:.0f} s script time, peak owned "
            f"{proof.peak // permanent.MIB} MiB at phase checks, phase-sampled host free minimum "
            f"{proof.low // permanent.MIB} MiB; one owned 128 MiB scratch image, ordinary detach; "
            f"receipt {receipt}"
        )
        permanent.record("result.txt", f"diagnostic (not a PASS)\n{line}\n")
        print(line)
        expect(DIAG.aborted is None, f"the diagnostic aborted: {DIAG.aborted}")
        return 0
    line = (
        f"outputs-prune proof held: {len(DONE)} cases, {elapsed:.0f} s script time, peak owned "
        f"{proof.peak // permanent.MIB} MiB at phase checks, phase-sampled host free minimum "
        f"{proof.low // permanent.MIB} MiB; one owned 128 MiB scratch image, ordinary detach"
    )
    permanent.record("result.txt", f"passed\n{line}\n")
    print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
