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
  --compat-plan, --compat-self-check
              The effect-free readiness and the pure controls of the focused
              compatibility recheck (oxide#308, below). `--compat-self-check`
              drives the real custody core (census parser, custody decision,
              spawn flags, timeout kill+reap, exceptional wait, survivor and
              unreadable census, spent-deadline refusal), the disposal gate,
              the whole setup→verification→disposal→seed→materialize→probe→
              closeout composition and its abort receipt through named fake
              external boundaries, changed-input refusals, a later stop after a
              real divergence, the pinned runner's frozen/unattended argv, and
              the activation parser and oracles. It compiles, acquires and runs
              nothing.
  --compat-diagnostic --binding P --sha256 H
              The uncounted keep-going compatibility collection: the SAME
              prior Oxide workload plus the float-formatting and CRC32 probes
              under the plain-Cargo baseline and the patched pinned Relax arm.
              Refuses before any effect until P names a planner-issued
              activation bound to this exact source, the workload pin and the
              new upstream pin. Always unqualified.
  --compat-qualify --binding P --sha256 H
              The counted author compatibility qualification under the same
              guard, pin and case set. Its activation must name the digest of
              an adequate diagnostic receipt at the unchanged head.
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
`OXIDE_TEST_HOME_FROM_ENV=1` and a private pool, its actual argv enforcing
`--locked --offline`) where the isolated fixture safely supports it, and
`relax` (the pinned standalone runner, strict ordinary Rust:
`--check-first --no-live --quiet`, never `--relaxed`, never `--cached`).
Each arm keeps only its own declared compilation cache between warm cases;
processes are fresh; tool build, std preparation, upstream acquisition and
dependency lock establishment are separately charged and reported, never
mixed into first-run latency. The offline seed normalizes the invoking
cache SOURCE at the established external boundary first — the declared
registry and git roots are resolved once, positively, and must keep the
expected namespace names under ONE canonical common parent (the host_cache
convention of scripts/test-outputs-delegation.py at 304 head 8346df5e) —
before the unchanged anchored seeder runs, whose no-follow, private,
checksum and content rules still guard every component below that root;
split roots, renamed namespaces, unresolved roots and any interior link
refuse, and a real `offline_seed.SeedRefusal` is translated at that
boundary into this entry's bounded refusal so a setup abort always passes
through the actual publication with every declared case unexecuted; the
normalized source facts and seeder limits are bound into the receipt
BEFORE the seeder runs, so even an abort receipt carries them, and the
effect-free controls drive that exact composition — the real seeder
exception through the real entry, gates, setup, translation and
publication, with injected external seams for the source identities and
the heavy acquisition/build phases. The upstream Relax workspace ships no
committed lock, so the guarded allowance explicitly establishes it
offline (`cargo generate-lockfile --offline`, recorded) before the
`--locked --offline` engine build. Filesystem caches cannot be globally
flushed under this scope, and every report says so.

Workloads: (1) the committed `greet` fixture, a small two-member workspace
with a serde/proc-macro dependency path and deterministic observable output;
(2) the pinned task-base Oxide snapshot (this repository's tree at the bound
head) running its read-only `--help` path under isolated test-home controls.
Cases per workload: cold cache, a genuinely unchanged warm block (no edit
between repetitions), and two edit-labelled blocks of five repetitions each —
EVERY edit-labelled repetition follows its own fresh predeclared change,
applied identically across arms, with its own expected observation and
(greet interface, snapshot interface) dependant invalidation: greet's body
steps walk GREETING through five declared values, greet's interface
alternates the `Record.count` type u32<->u64 five times, the snapshot's body
rewrites a cumulative `[trial-N]` help marker five times, and the snapshot's
interface alternates `MAX_STRING_BYTES` usize<->u64 with the cast sites
added once (the snapshot's edits are cumulative, so its interface case
expects the final `[trial-5]` marker). Every sample records its repetition
index and the shared post-edit source digest, so an unchanged-source sample
labelled as an edit is impossible. Arm order rotates per repetition, all
samples reported with median and range, absolute and relative savings. A
replacement synthetic fixture would be labelled as such, not as evidence
that Oxide's real graph runs.

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
are not attributed, and that limit is stated in every report. The raw
`ru_maxrss` is platform-scaled (macOS: bytes, verified on this host by a
child with a known 64 MiB unique allocation reporting raw 75,677,696; Linux:
KiB), so each sample retains the raw figure with its unit name and a
normalized explicit `rss_bytes` figure; no whole-session monitored RSS is
claimed. Every real arm and correspondence child carries the declared
toolchain (`RUSTUP_TOOLCHAIN=1.97.1`), the preserved invoking rustup home
and `RUSTUP_AUTO_INSTALL=0`, regardless of inherited values.

Receipt and custody: one bounded summary receipt (64 KiB cap) beside the
guard's own under `OXIDE_PROOF_EVIDENCE`, with identities, per-case records
(bounded prefixes, full-output SHA-256s, omission counters), a per-case
timing summary (sample counts, median/min/max, relative time and absolute
saving against the cargo baseline arm, matching cases only), the network
time and received-byte figure parsed from the forced clone progress,
owned-scratch and evidence-packet occupied-byte totals,
declared-but-unreached cases as explicit `unexecuted` rows, and the honest
statements that this receipt never claims guard closure, session census,
qualification or a performance recommendation from failed or unclosed
execution; closure stays the supervisor's. Statistics — verdicts,
`declared_unexecuted`, `timing_summary` — are always computed over the
COMPLETE recorded set (`statistics_basis: complete`), never silently over a
carried suffix; the summary's `cases` array is a disclosed excerpt
(`cases_in_receipt` + `cases_omitted` + `cases_total`), and the complete
untrimmed per-sample artifact (`samples-<packet>.json`: every reached,
unexecuted and aborted row with repetition and edit identities) is
published beside it and bound into the summary by SHA-256. Evidence is
published beside the guard receipt, outside the disposable execution
scratch. A guarded run exits 0 only when nothing aborted, every declared
case executed and every verdict is `match`; any divergence (including the
recorded known gap), unexecuted case or abort exits 1, so a divergence-only
run can never read as a clean pass.

Focused compatibility recheck (oxide#308; separate from the 307 trial)
---------------------------------------------------------------------
Tom's 2026-11-12 ruling (`7b5a88ce-96cb-4fab-a584-2e5deffa28ce` v2) asks one
question: does a NEWER Relax pin clear the ARM64 intrinsic blocker the 307
occurrence actually hit, on ACTUAL float formatting and the SAME prior Oxide
workload? The 307 no-go stays exactly as recorded for its own pin; this is new
work, never a reopening of that result, and a pass authorizes no integration,
adoption, installation, default change or performance claim.

The compat modes share this script, guard helpers, activation discipline and
receipt practices but keep their own pin, case table and receipt schema
(`oxide.relax-compat/1`), so the 307 commands and their history are untouched.
Two identities are bound separately and required to match exactly:

  * apparatus: this repository's HEAD/tree, verified clean and exact by the
    source gate;
  * workload: the UNCHANGED prior snapshot commit
    `61a55ebb98fba45cb708cbe9473a0db09a1e086e` (tree
    `d672048298332d8ebb60073575a167c63dde4dc5`, lock SHA-256
    `e7dcf4647e41e3a1b5449e107ac1dac9a1da98076a868a10b1bdfeafa6f37b28`),
    never this apparatus head. Its lock is the offline seed source, and its
    serde_json 1.0.151 / zmij 1.0.23 / crc32fast 1.5.3 versions and checksums
    are verified before any child runs.

Upstream is pinned to makepad/relax
`6ea9d0635f198b9a4510de1c87ac4d7b067f5cf7` (tree
`514321c9f9677cd4d0e7e11a73b5a01d0f98dc69`, 2026-11-12, MIT, bundled-std
notices upstream). The guarded acquisition re-verifies the tree identity and
requires the three named fix commits (`78cb555a`, `c0c1a4b8`, `e91301e8`) to
be present in the checkout. Source-level eligibility is NOT executed
correctness: `zmij-1.0.23` imports 43 `core::arch::aarch64` names, 20 of which
the old pin's bundled core lacked and none of which the new pin lacks, and that
is exactly what the probes must now confirm by running.

Declared probes (each run by the plain-Cargo baseline and the pinned Relax arm
with the runner's own strict argv read from the pin: `--check-first --frozen
--unattended --quiet` — `--frozen` is offline and lock-PREFERRING, NOT Cargo's
fail-on-drift, `--unattended` disables parking as well as the live session — and
never `--relaxed` or `--cached`). Every Relax child also receives
`MAKEPAD_RUN_CACHE_DIR` pointing at an out-of-tree run dir, so caches and build
outputs never land in a source tree that is being compared against its own
recorded identity:

  1. `compat-fmt` (required): the committed float fixture pins
     `serde_json =1.0.151`, whose aarch64 float formatting goes through
     `zmij`, and walks 30 declared f64 bit patterns, 18 f32 patterns and 12
     decimal decode inputs covering signs, negative zero, fractions, the
     scientific-notation thresholds, both finite extremes and the
     normal/subnormal and rounding boundaries. It prints the input bits, the
     serialized text and the decoded bits and exits nonzero on any round-trip
     mismatch; the apparatus independently decodes every serialized field and
     requires exact cross-arm stdout equality.
  2. `compat-crc32` (required): `crc32fast =1.5.3` from the same workload lock
     is the aarch64 `__crc32b`/`__crc32d` `target_feature("crc")` path, with a
     deterministic multi-stream buffer, a 7-byte streaming fold and the
     standard `123456789` -> `cbf43926` check value.
  3. `snapshot` (required): the unchanged workload's read-only `--help` route,
     required to print its marker, exit cleanly and reproduce the baseline's
     exact stdout.

Because `--frozen` does not enforce lock drift, graph equality rests on the
verified private source set: the seeded Cargo home's registry crates are
re-hashed against the workload lock's checksums, its index must hold exactly the
lock's packages, and every git database must be one of the lock's repositories
at its locked commit. A fourth, REQUIRED control (`source-constrained`) runs a
scratch fixture whose manifest requires a version absent from that verified set
and whose lock omits the package: the pinned runner must fail, or resolution was
not source-constrained and compatibility cannot be cleared. Resolution is thus
bounded by sources rather than by a flag claim.

Fit correction (the reason this mode is separate): the 307 occurrence was
stopped at 4100.6 MiB of the fixed 4096 MiB owned-scratch budget, with the
engine build target, a staged Oxide front door it did not need for this
question, and three snapshot trees alive at once. This mode stages no Oxide
front door (plain Cargo and the Relax runner are the only arms), keeps one
snapshot copy per arm, and disposes ONLY this occurrence's own stage output in
an explicit allowlist: after the copied executable, its retained bundled std
source and a tiny std-only check/run have all been verified, the disposable
engine build target is removed; then each probe's own cargo target and Relax
cache are removed after that probe's two arms completed. Closure stays INSIDE the
qualified boundary: every child runs in the guard's own session and process
group (no setsid and no separate group), so the supervisor can see and stop it.
Every disposal first requires every child positively reaped and then classifies
the guard session with a bounded process table: only the guard and its
descendants, this script and its transient census helper may appear; this
script's own subtree must be empty and any other row (for example an orphaned
producer whose parent was killed) is unclassified and refuses the removal. An
unreadable table is unknown, never closed. Every disposal is recorded with its
closure classification and the bytes removed. The checkout (Relax reads its std
from the build-time checkout path) is never disposed, and the bounds, supervisor
bytes, volume, reserve watchdog and stop margin are unchanged.

Compatibility success requires EVERY required probe and the resolution control,
exact COMPLETE-byte baseline correspondence, a clean exit and positive custody of
every child (each one reaped, with the in-boundary closure classification
closed); a body over the analysis bound, invalid UTF-8 or an unreadable capture
can never match, and a timed-out arm is a recorded
`timeout`, a divergence a `mismatch`, and an unclassified outcome an abort with
the remaining probes `unexecuted`. Each reached arm's bounded RAW stdout/stderr prefix is
retained in the packet before the next child runs, with its exact full-body
hash, byte count, completeness and strict-decode facts. The identity of the
actual materialized inputs — every fixture, snapshot and control source entry
and lock, the staged executable and the bundled std — is recorded before the
probes and re-read after them; a changed, missing or ADDED source entry aborts
the run (only the declared output-only `target/` prefix may grow), and an
extracted registry package the lock does not list also refuses. The receipt therefore reports
`cleared`, `blocked` or `incomplete`, states that it measures no timing, and
claims no closure: the guard's post-leader census owns that.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import pathlib
import platform
import re
import shutil
import signal
import struct
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
# Self-check seam: when set, TrialRun.setup substitutes the heavy
# acquisition/build phases with this callable so the effect-free controls can
# drive a later phase (the real seeder refusal) through the real entry, gates
# and publication. A guarded run never sets it.
SETUP_HEAVY_SEAM = None
# Self-check seam: when set, resolve_source returns the activation binding's
# identities without verifying them against git (a revision worktree is dirty
# by definition). A guarded run never sets it.
SOURCE_GATE_SEAM = None
# The summary receipt's `cases` array is a disclosed excerpt of the complete
# record; the complete per-sample artifact carries every row (see
# `build_receipt` and the samples companion). This constant documents the
# expected excerpt scale for the declared matrix and is not a statistics
# boundary: statistics always cover the complete set.
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
SNAPSHOT_HELP_MARKER = "oxide: Cargo with managed build storage and profiles"
PANIC_LOCATION = "main.rs:10:5"
PANIC_PROBE_LINE = '    panic!("corr-uncaught-boom");'
COLLISION_RUSTC = "app=a b=b"
COLLISION_KNOWN_GAP = "app=a b=a"

# Memory-observation units. wait4's ru_maxrss is platform-scaled: macOS
# reports BYTES (verified on this host: a child with a known 64 MiB unique
# allocation reported raw 75,677,696, consistent with bytes and absurd as
# KiB), Linux reports KiB. The raw figure is retained with its unit name and
# normalized to an explicit byte figure before any comparison.
RSS_UNIT_NAME = "bytes"
RSS_UNIT_BYTES = 1

# --- Predeclared edit sequences. Every edit-labelled repetition follows its
# own fresh, valid, deterministic change applied identically across arms, so
# each edit-labelled sample is a real edit-to-result observation; the
# unchanged warm block stays genuinely unchanged.

GREET_BODY_SEQUENCE = ["howdy", "hiya", "greetings", "welcome", "howdys"]
# Alternating valid field types: each direction is a fresh change that
# invalidates the greet dependant; the observable bytes stay identical.
GREET_INTERFACE_SEQUENCE = [
    ("u32", "u64"), ("u64", "u32"), ("u32", "u64"), ("u64", "u32"), ("u32", "u64"),
]
# Cumulative help-line markers: repetition i rewrites the marker value, so
# every sample observes a fresh change.
SNAPSHOT_BODY_SEQUENCE = [f"[trial-{n}]" for n in range(1, 6)]
# Alternating const types; the four cast sites are added on the first step
# only (`as usize` is valid for both source types, so later steps change only
# the const).
SNAPSHOT_INTERFACE_SEQUENCE = [
    ("usize", "u64"), ("u64", "usize"), ("usize", "u64"), ("u64", "usize"), ("usize", "u64"),
]


def greet_expected(greeting: str) -> str:
    """The deterministic greet stdout for one cumulative GREETING value."""
    return f'{{"record":{{"count":42,"name":"Tom"}},"rendered":"{greeting} Tom count=42"}}'


def greet_body_step(index: int) -> tuple[str, str]:
    """(old, new) for cumulative body step index; `hello` seeds the sequence."""
    values = ["hello"] + list(GREET_BODY_SEQUENCE)
    return (
        f'pub const GREETING: &str = "{values[index]}";',
        f'pub const GREETING: &str = "{values[index + 1]}";',
    )


def greet_interface_step(index: int) -> tuple[str, str]:
    first, second = GREET_INTERFACE_SEQUENCE[index]
    return (f"    pub count: {first},", f"    pub count: {second},")


def snapshot_body_step(index: int) -> tuple[str, str]:
    line = "oxide: Cargo with managed build storage and profiles"
    if index == 0:
        return (line + "\n", line + " [trial-1]\n")
    return (line + f" [trial-{index}]\n", line + f" [trial-{index + 1}]\n")


def snapshot_interface_steps(index: int) -> list[tuple[str, str, int]]:
    """The const-type swap for step index, plus the four cast-site rewrites on
    the first step only."""
    first, second = SNAPSHOT_INTERFACE_SEQUENCE[index]
    ops = [(f"pub const MAX_STRING_BYTES: {first} = 2048;",
            f"pub const MAX_STRING_BYTES: {second} = 2048;", 1)]
    if index == 0:
        ops.append(("reader.string(MAX_STRING_BYTES)?",
                    "reader.string(MAX_STRING_BYTES as usize)?", 4))
    return ops

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
        "mode checkout; the Oxide arms run an explicit private OXIDE_POOL "
        "directory pool inside the run root under a private HOME on fresh "
        "fixtures with no target, whose first conversion (if any) is an "
        "observation recorded per case, never an oracle premise."
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
                                "--diagnostic, --measure, --compat-plan, "
                                "--compat-self-check, --compat-diagnostic, "
                                "--compat-qualify or --help")
    if len(argv) > 5:
        return ("refuse", None, f"too many arguments, got {argv!r}")
    modes = ("--plan", "--self-check", "--diagnostic", "--measure",
             "--compat-plan", "--compat-self-check", "--compat-diagnostic",
             "--compat-qualify", "--help")
    if argv[0] not in modes:
        return ("refuse", None, f"unrecognized first argument {argv[0]!r}")
    mode = argv[0][2:]
    if mode in ("plan", "self-check", "help", "compat-plan", "compat-self-check"):
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
        return greet_expected("howdy")
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
     "figure": "genuinely unchanged warm block, no edit between repetitions"},
    {"workload": "greet", "case": "edit-body", "reps": 5, "arms": ARMS,
     "edit": "five distinct predeclared GREETING edits, one per repetition "
             "(hello->howdy->hiya->greetings->welcome->howdys), identical across arms",
     "oracle": "exact stdout greet_expected(step value), exit 0; every rep changes bytes",
     "figure": "first rep after each edit is that rep's edit-to-result figure"},
    {"workload": "greet", "case": "edit-interface", "reps": 5, "arms": ARMS,
     "edit": "five alternating Record.count type edits (u32<->u64), one per repetition",
     "oracle": "exact stdout greet_expected(howdys), exit 0 (unchanged bytes); dependant invalidated each rep",
     "figure": "dependant-invalidating edit per repetition"},
    {"workload": "snapshot", "case": "cold", "arms": ARMS,
     "oracle": "exit 0, stdout contains SNAPSHOT_HELP_MARKER, not '[trial]'",
     "figure": "cold build+run of the read-only --help path"},
    {"workload": "snapshot", "case": "warm-unchanged", "reps": 5, "arms": ARMS,
     "oracle": "as cold", "figure": "genuinely unchanged warm block"},
    {"workload": "snapshot", "case": "edit-body", "reps": 5, "arms": ARMS,
     "edit": "five distinct cumulative [trial-N] marker edits, one per repetition",
     "oracle": "exit 0, stdout contains marker + [trial-N] for that rep and not the previous marker",
     "figure": "the oxide#075 loop shape, one fresh change per repetition"},
    {"workload": "snapshot", "case": "edit-interface", "reps": 5, "arms": ARMS,
     "edit": "five alternating MAX_STRING_BYTES type edits (usize<->u64; cast sites on the first)",
     "oracle": "exit 0, stdout contains the cumulative [trial-5] help line",
     "figure": "library type change invalidating the bin unit, per repetition"},
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

    # The real edit/iteration composition, driven effect-free against the
    # committed fixture bytes: every edit-labelled repetition must change
    # bytes (a no-op step refuses) and its expected observation must match
    # the derivation the guarded loop will use. Text-anchor presence alone
    # proves nothing; each step's before/after bytes and the per-repetition
    # expected stdout are checked here end to end.
    core = read_fixture("greet", "greet-core", "src", "lib.rs")  # (pure read)
    greet_text = core
    for index in range(5):
        old, new = greet_body_step(index)
        before = greet_text
        greet_text = apply_edit(greet_text, old, new, 1)
        expect(greet_text != before, f"greet body repetition {index} changes bytes")
        expected = greet_expected(GREET_BODY_SEQUENCE[index])
        expect(f'"rendered":"{GREET_BODY_SEQUENCE[index]} Tom count=42"' in expected
               and expected.startswith('{"record":{"count":42,"name":"Tom"}'),
               f"greet body repetition {index} derives its expected observation")
    for index in range(5):
        old, new = greet_interface_step(index)
        before = greet_text
        greet_text = apply_edit(greet_text, old, new, 1)
        expect(greet_text != before,
               f"greet interface repetition {index} changes bytes and invalidates the dependant")
    final_greeting = GREET_BODY_SEQUENCE[-1]
    expect(f'pub const GREETING: &str = "{final_greeting}";' in greet_text
           and "pub count: u64," in greet_text,
           "the cumulative greet sequence lands on its declared final state")
    # The full expected-output ladder, seed through cumulative final value.
    expect(greet_expected("hello") == GREET_EXPECTED_V1, "seed expectation is byte-stable")
    for index, value in enumerate(GREET_BODY_SEQUENCE):
        expect(greet_expected(value)
               == f'{{"record":{{"count":42,"name":"Tom"}},"rendered":"{value} Tom count=42"}}',
               f"greet expectation for repetition {index} is deterministic")
    with open(os.path.join(REPO, "src", "bin", "oxide", "main.rs"), encoding="utf-8") as h:
        usage = h.read()
    snapshot_text = usage
    for index in range(5):
        old, new = snapshot_body_step(index)
        before = snapshot_text
        snapshot_text = apply_edit(snapshot_text, old, new, 1)
        expect(snapshot_text != before,
               f"snapshot body repetition {index} changes bytes")
        expected_marker = SNAPSHOT_HELP_MARKER + " " + SNAPSHOT_BODY_SEQUENCE[index]
        expect(expected_marker + "\n" in snapshot_text
               and (index == 0 or SNAPSHOT_BODY_SEQUENCE[index - 1] not in snapshot_text),
               f"snapshot body repetition {index} checks its own cumulative marker")
    dep_info = open(os.path.join(REPO, "src", "dep_info.rs"), encoding="utf-8").read()
    snapshot_dep = dep_info
    for index in range(5):
        before = snapshot_dep
        for old, new, count in snapshot_interface_steps(index):
            snapshot_dep = apply_edit(snapshot_dep, old, new, count)
        expect(snapshot_dep != before,
               f"snapshot interface repetition {index} changes bytes")
        expect("2048;" in snapshot_dep and "reader.string(MAX_STRING_BYTES as usize)?"
               in snapshot_dep,
               f"snapshot interface repetition {index} keeps the const and casts")
    expect(snapshot_dep.count("reader.string(MAX_STRING_BYTES as usize)?") == 4,
           "all four cast sites are rewritten once on the first step")
    cumulative_line = SNAPSHOT_HELP_MARKER + " " + SNAPSHOT_BODY_SEQUENCE[-1]
    expect(cumulative_line + "\n" in snapshot_text,
           "the cumulative snapshot body lands on [trial-5]")

    # Oracles against fixture transcripts.
    expect(oracle_exact_stdout({"stdout": GREET_EXPECTED_V1 + "\n", "exit": 0},
                               GREET_EXPECTED_V1 + "\n") == "match", "exact oracle matches")
    expect(oracle_exact_stdout({"stdout": GREET_EXPECTED_V1 + "\n", "exit": 101},
                               GREET_EXPECTED_V1 + "\n") == "mismatch", "exit matters")
    expect(greeting_expectation(None) == GREET_EXPECTED_V1, "warm expects v1")
    expect(greeting_expectation("body") == greet_expected("howdy"), "body edit expects howdy")
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

    # Panic probe location: derived from the committed bytes, so line drift
    # fails here instead of as a truth-arm mismatch in a guarded run.
    panic_source = read_fixture("corr-suite", "panic", "src", "main.rs")
    panic_lines = panic_source.splitlines()
    matches = [i for i, line in enumerate(panic_lines, start=1)
               if line == PANIC_PROBE_LINE]
    expect(len(matches) == 1, "the uncaught panic probe line occurs exactly once")
    derived = f"main.rs:{matches[0]}:5"
    expect(derived == PANIC_LOCATION,
           f"PANIC_LOCATION {PANIC_LOCATION} must equal the committed probe line {derived}")

    # The derive macros return only their generated impl and TokenStream is
    # actually in scope: a #[proc_macro_derive] result is appended to the
    # annotated item, so re-emitting the input would define the struct twice
    # (E0412/E0428), and an unimported TokenStream spelling fails E0412
    # before any derive runs.
    for mac in ("mac-a", "mac-b"):
        source = read_fixture("corr-suite", mac, "src", "lib.rs")
        expect("input.clone()" not in source and "let mut out = input" not in source,
               f"{mac} must not re-emit the annotated item")
        expect(parse_to_tokenstream(source),
               f"{mac} returns its parsed impl body")
        expect('impl {name} {{ pub fn who()' in source,
               f"{mac} emits an inherent impl on the derived name")
        expect(tokenstream_in_scope(source),
               f"{mac} must import TokenStream or fully qualify every mention")
    # The scope control is the property, not a text pin: it must refuse the
    # exact past defect (TokenTree-only import with bare TokenStream uses)
    # and accept a fully qualified variant with no import.
    defect = read_fixture("corr-suite", "mac-a", "src", "lib.rs").replace(
        "use proc_macro::{TokenStream, TokenTree};", "use proc_macro::TokenTree;"
    )
    expect(not tokenstream_in_scope(defect),
           "the TokenTree-only import defect must fail the scope control")
    qualified_variant = "\n".join(
        line for line in defect.splitlines() if not line.strip().startswith("use ")
    )
    qualified_variant = re.sub(r"(?<![\w:])TokenStream\b", "proc_macro::TokenStream",
                               qualified_variant)
    expect(tokenstream_in_scope(qualified_variant),
           "a fully qualified TokenStream with no import satisfies the control")

    # Receipt trimming: statistics are computed ONCE over the COMPLETE set
    # (statistics_basis complete) and never recomputed from the carried
    # suffix, while the summary's `cases` array is a disclosed excerpt:
    # cases_omitted + cases_in_receipt == cases_total, and the complete
    # per-sample artifact is hash-bound into the summary.
    huge_row = {
        "workload": "greet", "case": "cold", "arm": "cargo", "verdict": "match",
        "seconds": 1.0, "stdout_evidence": {"prefix": "x" * 4096, "truncated": True},
    }
    trim_cases = [dict(huge_row, case=f"cold-{index}") for index in range(64)]
    trim_cases.append({"workload": "greet", "case": "cold", "arm": "relax",
                       "verdict": "unexecuted"})
    trimmed = build_receipt({"schema": "t"}, trim_cases, cap_bytes=32 * 1024)
    expect(trimmed["cases_omitted"] + trimmed["cases_in_receipt"] == trimmed["cases_total"],
           "omitted + carried equals the total")
    expect(sum(trimmed["verdicts"].values()) == len(trim_cases),
           "verdicts cover the complete set, not the carried suffix")
    expect(trimmed["verdicts"] == verdict_tally(trim_cases),
           "verdicts equal the full-set tally after forced trimming")
    expect(trimmed["declared_unexecuted"] == 1,
           "declared_unexecuted counts the complete set's unexecuted row even when its row is trimmed")
    expect(all(f"greet/{row['case']}" in trimmed["timing_summary"]
               for row in trim_cases if row["verdict"] == "match"),
           "timing_summary covers every matching complete-set case, carried or not")
    expect(trimmed["cases_in_receipt"] < trimmed["cases_total"],
           "the forced trim actually trimmed the excerpt")
    untrimmed = build_receipt({"schema": "t"}, trim_cases, cap_bytes=8 * 1024 * 1024)
    expect(untrimmed["cases_omitted"] == 0
           and untrimmed["cases_in_receipt"] == len(trim_cases),
           "a fitting receipt carries every row")

    # Complete samples artifact, hash-bound into the summary, plus the real
    # publication seam under one private temporary directory that this check
    # removes; no child, no network, no engine, no scratch outside it.
    full_matrix = []
    for case in ("cold", "warm-unchanged", "edit-body", "edit-interface"):
        for rep in (None, 0, 1, 2, 3, 4):
            for arm in ARMS:
                full_matrix.append({"workload": "greet", "case": case, "arm": arm,
                                    "rep": rep, "verdict": "match", "seconds": 1.0})
    samples = {
        "schema": "oxide.relax-quick-trial-samples/1",
        "cases_total": len(full_matrix),
        "cases": full_matrix,
    }
    samples_bytes = json.dumps(samples, indent=1).encode()
    bound = build_receipt({"schema": "t", "samples_sha256":
                           hashlib.sha256(samples_bytes).hexdigest(),
                           "samples_rows": len(full_matrix)},
                          full_matrix, cap_bytes=8 * 1024)
    expect(bound["samples_sha256"] == hashlib.sha256(samples_bytes).hexdigest(),
           "the summary binds the samples artifact's exact bytes")
    expect(bound["samples_rows"] == len(full_matrix), "the summary names the row count")
    expect(bound["declared_unexecuted"] == 0 and len(bound["verdicts"]) == 1,
           "the complete statistics survive a forced summary trim")

    import tempfile
    import shutil
    seam_dir = tempfile.mkdtemp(dir=os.environ.get("TMPDIR") or None)
    try:
        # Complete samples artifact, hash-bound into the summary, published
        # through the real publish() with a forced summary cap.
        seam = TrialRun.__new__(TrialRun)
        seam.mode = "diagnostic"
        seam.binding = {"paths": {"run_root_name": "seam-run", "packet_name": "seam-packet"},
                        "planner_reference": "self-check-seam"}
        seam.figures = {}
        seam.cases = [dict(row) for row in full_matrix]
        seam.cases.extend(declared_unexecuted(seam.cases))
        seam.run_root = os.path.join(seam_dir, "run")
        seam.packet_dir = os.path.join(seam_dir, "packet")
        seam.aborted = None
        module_cap = MAX_RECEIPT_BYTES
        globals()["MAX_RECEIPT_BYTES"] = 16 * 1024  # force the excerpt path
        try:
            seam.publish()
        finally:
            globals()["MAX_RECEIPT_BYTES"] = module_cap
        receipt_path = os.path.join(seam.packet_dir, "receipt-seam-packet.json")
        samples_path = os.path.join(seam.packet_dir, "samples-seam-packet.json")
        published_receipt = json.load(open(receipt_path))
        published_samples = open(samples_path, "rb").read()
        expect(published_receipt["samples_sha256"]
               == hashlib.sha256(published_samples).hexdigest(),
               "the published summary binds the published samples bytes")
        expect(json.loads(published_samples.decode())["cases_total"] == len(seam.cases),
               "the published samples artifact carries the complete row set")
        expect(published_receipt["cases_in_receipt"] < published_receipt["cases_total"],
               "the forced-cap publication trimmed only the summary excerpt")
        expect(sum(published_receipt["verdicts"].values()) == published_receipt["cases_total"],
               "the published statistics cover the complete set despite trimming")

        # The real arm env seams pin every child toolchain: RUSTUP_TOOLCHAIN
        # is the declared 1.97.1 for all arms (overriding any inherited
        # value), RUSTUP_AUTO_INSTALL is 0, the rustup home survives, and an
        # inherited wrong toolchain cannot leak through. The synthetic works
        # live inside seam_dir, so the oxide seam's real per-arm home creation
        # lands in the removed directory.
        seam_run = TrialRun.__new__(TrialRun)
        seam_run.env = {
            "HOME": os.path.join(seam_dir, "home"), "PATH": "/bin",
            "RUSTUP_HOME": "/rustup/invoking",
            "RUSTUP_AUTO_INSTALL": "0",
            "TMPDIR": os.path.join(seam_dir, "home", "tmp"),
            "MAKEPAD_RUN_HOME": os.path.join(seam_dir, "home", ".makepad-run"),
            "OXIDE_TEST_HOME_FROM_ENV": "1",
            "RUSTUP_TOOLCHAIN": "1.99.1",  # a hostile inherited value must be overridden
        }
        seam_run.shared_cargo_home = os.path.join(seam_dir, "cargo")
        seam_run.relax_bin = "/bin/relax"
        seam_run.oxide_bin = "/bin/oxide"
        seam_run.works = {
            "greet": {arm: os.path.join(seam_dir, "w", "greet", arm, "greet") for arm in ARMS},
            "corr": {arm: os.path.join(seam_dir, "w", "corr-suite", arm, "corr-suite") for arm in ARMS},
            "snapshot": {arm: os.path.join(seam_dir, "w", "snapshot", arm, "tree") for arm in ARMS},
        }
        for arm in ARMS:
            argv, env = seam_run.arm_argv_env(arm, "greet", ["Tom"])
            expect(env.get("RUSTUP_TOOLCHAIN") == TOOLCHAIN,
                   f"{arm} arm pins the declared toolchain over any inherited value")
            expect(env.get("RUSTUP_AUTO_INSTALL") == "0", f"{arm} arm keeps auto-install off")
            expect(env.get("RUSTUP_HOME") == "/rustup/invoking", f"{arm} arm preserves the rustup home")
        oxide_argv, oxide_env = seam_run.arm_argv_env("oxide", "greet", ["Tom"])
        expect(oxide_env.get("OXIDE_TEST_HOME_FROM_ENV") == "1"
               and oxide_env.get("CARGO_HOME") == seam_run.shared_cargo_home
               and oxide_env.get("OXIDE_POOL").startswith(os.path.join(seam_dir, "w", "greet", "oxide")),
               "oxide arm keeps its private home, pool and shared cargo home")
        relax_argv, relax_env = seam_run.arm_argv_env("relax", "greet", ["Tom"])
        expect(relax_env.get("MAKEPAD_RUN_OFFLINE") == "1"
               and relax_env.get("CARGO_HOME") == seam_run.shared_cargo_home,
               "relax arm stays offline on the shared seeded home")
        corr_argv, corr_env = seam_run.corr_command("relax", "basics", ["one"])
        expect(corr_env.get("RUSTUP_TOOLCHAIN") == TOOLCHAIN,
               "correspondence relax children pin the declared toolchain too")

        # Engine and oxide-arm argv flags at the real seams: the engine build
        # is --locked --offline after an explicit offline lock establishment,
        # and every oxide-arm run passes --locked --offline.
        expect(engine_lock_command() == ["cargo", "generate-lockfile", "--offline"],
               "the engine lock is established offline inside the guarded allowance")
        expect("--locked" in engine_build_command() and "--offline" in engine_build_command(),
               "the engine build is locked and offline")
        snapshot_argv, _snapshot_env = seam_run.arm_argv_env("oxide", "snapshot", ["--help"])
        expect("--locked" in snapshot_argv and "--offline" in snapshot_argv,
               "the oxide arm's actual argv enforces --locked/--offline")

        # Seed-source normalization on tiny real fixture directories and
        # links, composed with the REAL anchored seeder (its own fixture
        # builder and lock shape; no copied model):
        import offline_seed
        common = os.path.join(seam_dir, "common")
        host, rev, checksum = offline_seed._fixture_host(pathlib.Path(common))
        minimal_lock = offline_seed._lock_text(rev, checksum)
        # Accepted common-root relocation: the declared cargo home holds
        # boundary symlinks to one canonical real parent; the resolved root
        # is handed to the unchanged seeder, which seeds successfully.
        relocated_home = os.path.join(seam_dir, "cargo-home")
        os.makedirs(relocated_home)
        os.symlink(os.path.join(common, "host-cargo-home", "registry"), os.path.join(relocated_home, "registry"))
        os.symlink(os.path.join(common, "host-cargo-home", "git"), os.path.join(relocated_home, "git"))
        facts = canonical_cache_source(relocated_home)
        expect(facts["common_root"] == os.path.join(common, "host-cargo-home"),
               "boundary links resolve to one canonical common root")
        seed_dest = os.path.join(seam_dir, "seed-dest")
        os.makedirs(seed_dest)
        seeded = offline_seed.seed_cargo_home(
            facts["common_root"], seed_dest, minimal_lock.encode(),
            limits=offline_seed.Limits(work_seconds=20),
        )
        expect(seeded.get("aggregate_bytes", 0) > 0,
               "the real seeder accepts the normalized relocated source")
        # Interior links inside the namespace stay refused by the real
        # seeder, and this entry translates that exact exception class into
        # its bounded Refusal preserving the phase and reason.
        interior_home = os.path.join(seam_dir, "interior-home")
        common2 = os.path.join(seam_dir, "common2")
        offline_seed._fixture_host(pathlib.Path(common2))
        os.makedirs(interior_home)
        os.symlink(os.path.join(common2, "host-cargo-home", "registry"), os.path.join(interior_home, "registry"))
        os.symlink(os.path.join(common2, "host-cargo-home", "git"), os.path.join(interior_home, "git"))
        namespace = offline_seed.CRATES_NAMESPACE
        real_index = os.path.join(common2, "host-cargo-home", "registry", "index", namespace)
        os.rename(real_index, os.path.join(common2, "host-cargo-home", "registry", "index", "moved"))
        os.symlink(os.path.join(common2, "host-cargo-home", "registry", "index", "moved"), real_index)
        facts2 = canonical_cache_source(interior_home)  # boundary passes
        seam_run2 = TrialRun.__new__(TrialRun)
        seam_run2.deadline_ns = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) + int(60e9)
        seam_run2.run_root = os.path.join(seam_dir, "interior-run")
        os.makedirs(seam_run2.run_root)
        seam_run2.figures = {}
        os.environ["CARGO_HOME"] = interior_home
        try:
            seam_run2.seed_cargo_home()
            raise AssertionError("an interior-link seed must refuse")
        except Refusal as refusal:
            expect(str(refusal).startswith("seed:")
                   and "not a real directory" in str(refusal),
                   "the real SeedRefusal is translated with its phase and reason")
        finally:
            os.environ.pop("CARGO_HOME", None)
        # Split roots, renamed namespaces and unresolved roots refuse.
        split_a = os.path.join(seam_dir, "split-a", "registry")
        split_b = os.path.join(seam_dir, "split-b", "git")
        os.makedirs(split_a); os.makedirs(split_b)
        split_home = os.path.join(seam_dir, "split-home")
        os.makedirs(split_home)
        os.symlink(split_a, os.path.join(split_home, "registry"))
        os.symlink(split_b, os.path.join(split_home, "git"))
        try:
            canonical_cache_source(split_home)
            raise AssertionError("split roots must refuse")
        except Refusal:
            pass
        renamed = os.path.join(seam_dir, "renamed")
        renamed_common = os.path.join(seam_dir, "renamed-common")
        os.makedirs(os.path.join(renamed_common, "reg-x"))
        os.makedirs(os.path.join(renamed_common, "git"))
        os.makedirs(renamed)
        # The declared names are registry/git, but the boundary links resolve
        # to directories with different basenames under one parent, so only
        # the namespace-names rule can fire.
        os.symlink(os.path.join(renamed_common, "reg-x"), os.path.join(renamed, "registry"))
        os.symlink(os.path.join(renamed_common, "git"), os.path.join(renamed, "git"))
        try:
            canonical_cache_source(renamed)
            raise AssertionError("a renamed registry namespace must refuse")
        except Refusal as refusal:
            # The namespace-names branch, specifically: both roots resolve as
            # real directories under one parent, so only the basename rule
            # can have fired.
            expect("expected namespace names" in str(refusal),
                   "the renamed control exercises the basename branch")
        try:
            canonical_cache_source(os.path.join(seam_dir, "missing-home"))
            raise AssertionError("unresolved roots must refuse")
        except Refusal:
            pass

        # The ACTUAL seeder exception class through the real entry, gates,
        # setup and publication: a real activation binding (live head/tree)
        # passes the volume and source gates, the injected external seam
        # substitutes the heavy acquisition/build phases, and the real
        # seeder raises SeedRefusal on the interior link — translated at the
        # trial boundary and aborted through guarded_main's publication with
        # both artifacts, the bound source/limit facts and every declared
        # case unexecuted.
        # Under the source seam the gate returns the binding's identities
        # without git verification, so the document carries exact 40-hex
        # placeholders; parse_binding validates their shape.
        head_now = "a" * 40
        tree_now = "b" * 40
        binding_doc = json.dumps({
            "schema": BINDING_SCHEMA,
            "planner_reference": "self-check-guarded-main",
            "mode": "diagnostic",
            "script_sha256": sha256_file(SCRIPT),
            "head": head_now,
            "tree": tree_now,
            "toolchain": TOOLCHAIN,
            "diagnostic_receipt_sha256": "",
            "guard": {"supervisor_sha256": EXPECTED_RUNNER_SHA256,
                      "limit_seconds": LIMIT_SECONDS, "scratch_mib": SCRATCH_MIB,
                      "stop_margin_seconds": STOP_MARGIN_S},
            "upstream": {"commit": UPSTREAM_COMMIT, "tree": UPSTREAM_TREE},
            "volume": {"uuid": SELECTED_VOLUME_UUID},
            "paths": {"run_root_name": "guarded-seam-run", "packet_name": "guarded-seam-packet"},
        }).encode()
        binding_path = os.path.join(seam_dir, "guarded-binding.json")
        open(binding_path, "wb").write(binding_doc)
        env_overrides = {
            "OXIDE_TASK_SCRATCH_ROOT": os.path.join(seam_dir, "guarded-scratch"),
            "OXIDE_PROOF_DEADLINE_NS": str(time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) + int(120e9)),
            "OXIDE_PROOF_EVIDENCE": os.path.join(seam_dir, "guarded-evidence"),
            # The invoking cache the real normalization will resolve: the
            # interior-link fixture, whose boundary passes and whose interior
            # namespace link makes the REAL seeder refuse.
            "CARGO_HOME": interior_home,
        }
        saved = {k: os.environ.get(k) for k in env_overrides}
        os.environ.update(env_overrides)
        # Both injected external seams: the source gate binds the activation
        # identities without git (a revision worktree is dirty by
        # definition) and the heavy acquisition/build phases are substituted,
        # so the REAL seeder refusal is driven through the real entry,
        # remaining gates, setup, translation and publication.
        globals()["SOURCE_GATE_SEAM"] = True
        globals()["SETUP_HEAVY_SEAM"] = lambda run: None
        import contextlib
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                guarded_exit = guarded_main("diagnostic", binding_path,
                                            hashlib.sha256(binding_doc).hexdigest())
        finally:
            globals()["SOURCE_GATE_SEAM"] = None
            globals()["SETUP_HEAVY_SEAM"] = None
            for key, value in saved.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value
        expect(guarded_exit == 1, "an aborted guarded occurrence exits 1")
        aborted_receipt = json.load(open(os.path.join(
            env_overrides["OXIDE_PROOF_EVIDENCE"], "guarded-seam-packet",
            "receipt-guarded-seam-packet.json")))
        expect("seed directory is not a real directory" in aborted_receipt["aborted"],
               "the REAL SeedRefusal aborts setup through guarded_main, phase and reason preserved")
        expect(aborted_receipt["figures"]["seed_source"]["common_root"].endswith("common2")
               or str(common2) in json.dumps(aborted_receipt["figures"]["seed_source"]),
               "the abort receipt binds the normalized source facts")
        expect(aborted_receipt["figures"]["seed_source"]["limits"]["aggregate_bytes"] > 0,
               "the abort receipt binds the seeder limits")
        expect(aborted_receipt["declared_unexecuted"]
               == sum(len(case["arms"]) for case in CASE_TABLE),
               "every declared case is recorded unexecuted")
        expect(aborted_receipt["samples_rows"] == aborted_receipt["cases_total"],
               "the complete samples artifact accompanies the setup abort")
        expect(collection_exit(aborted_receipt["aborted"],
                               aborted_receipt["cases"]) == 1,
               "the aborted collection exits nonzero through the real rule")
    finally:
        shutil.rmtree(seam_dir)

    # Memory normalization: the raw figure is retained with its unit and the
    # explicit byte figure is derived by the declared unit scale.
    expect(RSS_UNIT_NAME == "bytes" and RSS_UNIT_BYTES == 1,
           "this host's ru_maxrss unit is verified bytes")
    raw_sample = 75_677_696
    expect(raw_sample * RSS_UNIT_BYTES == raw_sample
           and 64 * 1024 * 1024 < raw_sample < 128 * 1024 * 1024,
           "the verification probe's magnitude stays consistent with bytes")

    print("self-check: all controls passed (one removed temp directory; no child, "
          "network, engine or scratch outside it)")


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
    # Self-check seam: a revision worktree is dirty by definition, so the
    # effect-free drive of a later phase binds the source identities from
    # the real activation document instead of verifying them against git. A
    # guarded run never sets this.
    if SOURCE_GATE_SEAM is not None:
        return binding["head"], binding["tree"]
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
The raw value is platform-scaled (macOS: bytes, verified on this host;
Linux: KiB), so it is retained with its unit name and normalized to the
explicit rss_bytes figure; no whole-session monitored RSS is claimed.
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
        "rss_maxrss_raw": int(usage.ru_maxrss),
        "rss_unit": RSS_UNIT_NAME,
        "rss_bytes": int(usage.ru_maxrss) * RSS_UNIT_BYTES,
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


def verdict_tally(cases: list[dict]) -> dict[str, int]:
    tally: dict[str, int] = {}
    for row in cases:
        verdict = row.get("verdict", "?")
        tally[verdict] = tally.get(verdict, 0) + 1
    return tally


def build_receipt(base: dict, cases: list[dict], cap_bytes: int | None = None) -> dict:
    """One bounded summary receipt. Statistics — verdicts, declared_unexecuted,
    timing_summary — are computed ONCE over the COMPLETE case set and never
    recomputed from a carried suffix, so a divergence or figure is never
    silently reduced by trimming. When the rows exceed the byte cap, rows
    drop from the end of the `cases` excerpt only, and `cases_in_receipt`,
    `cases_omitted` and `cases_total` disclose exactly what was dropped; the
    complete hash-bound samples artifact carries every row."""
    cap = MAX_RECEIPT_BYTES if cap_bytes is None else cap_bytes
    document = dict(base)
    document["cases"] = list(cases)
    document["cases_total"] = len(cases)
    document["cases_omitted"] = 0
    document["cases_in_receipt"] = len(document["cases"])
    document["verdicts"] = verdict_tally(cases)
    document["declared_unexecuted"] = sum(
        1 for row in cases if row.get("verdict") == "unexecuted"
    )
    document["timing_summary"] = timing_summary(cases)
    encoded = json.dumps(document, indent=1).encode()
    while len(encoded) > cap and len(document["cases"]) > 1:
        document["cases"].pop()
        document["cases_omitted"] += 1
        document["cases_in_receipt"] = len(document["cases"])
        encoded = json.dumps(document, indent=1).encode()
    return document


def tokenstream_in_scope(source: str) -> bool:
    """Every `TokenStream` mention in code (outside `use` lines) must resolve:
    either an import line brings TokenStream into scope, or every code
    mention is fully qualified as proc_macro::TokenStream. The lookbehind
    already isolates the unqualified mentions — `proc_macro::TokenStream`
    never matches them — so any unqualified mention without an import is
    unresolved. This is the property the text-only pin
    ("body.parse::<TokenStream>() in source") missed: that spelling is
    exactly the unimported one."""
    lines = source.splitlines()
    imports = "\n".join(line for line in lines if line.strip().startswith("use "))
    code = "\n".join(line for line in lines if not line.strip().startswith("use "))
    mentions = re.findall(r"(?<![\w:])TokenStream\b", code)
    if not mentions:
        return True
    return "TokenStream" in imports


def parse_to_tokenstream(source: str) -> bool:
    """The macro returns its parsed body: `.parse::<TokenStream>()` in either
    the imported or the fully qualified spelling."""
    return re.search(r"\.parse::<(proc_macro::)?TokenStream>\(\)", source) is not None


def canonical_cache_source(cargo_home: str) -> dict:
    """Normalize the invoking cache SOURCE at the established external
    boundary before the unchanged anchored seeder runs (the host_cache
    convention of scripts/test-outputs-delegation.py at 304 head 8346df5e):
    resolve the declared registry and git roots positively (strict), require
    the expected namespace names under ONE canonical common parent, and hand
    that common root to the seeder — whose no-follow, private, checksum and
    content rules still guard every component below it. Boundary links are
    resolved once, here; links INSIDE the namespace stay refused by the
    seeder. Split roots, unexpected namespace names and unresolved roots
    refuse."""
    registry_declared = os.path.join(cargo_home, "registry")
    git_declared = os.path.join(cargo_home, "git")
    facts: dict = {
        "cargo_home": cargo_home,
        "registry_declared": registry_declared,
        "git_declared": git_declared,
    }
    registry_real = os.path.realpath(registry_declared)
    git_real = os.path.realpath(git_declared)
    facts["registry_real"] = registry_real
    facts["git_real"] = git_real
    if not os.path.isdir(registry_real) or not os.path.isdir(git_real):
        raise Refusal(
            f"seed source: the invoking registry/git roots do not resolve to "
            f"real directories: {registry_real} / {git_real}"
        )
    if os.path.basename(registry_real) != "registry" or os.path.basename(git_real) != "git":
        raise Refusal(
            "seed source: the resolved registry and git roots do not keep the "
            f"expected namespace names under one canonical source: {registry_real} / {git_real}"
        )
    common = os.path.dirname(registry_real)
    if common != os.path.dirname(git_real):
        raise Refusal(
            "seed source: registry and git caches do not share one canonical "
            f"source root (split roots): {registry_real} / {git_real}"
        )
    facts["common_root"] = common
    facts["resolution"] = (
        "boundary links resolved once, positively; interior links stay "
        "refused by the unchanged anchored seeder"
    )
    return facts


def engine_lock_command() -> list[str]:
    """The upstream Relax workspace ships no committed lock, so the guarded
    allowance explicitly establishes it OFFLINE (the engine has zero registry
    dependencies; path-only resolution needs no index) before any locked
    engine build. No separate prewarm, network fallback or install."""
    return ["cargo", "generate-lockfile", "--offline"]


def engine_build_command() -> list[str]:
    return ["cargo", "build", "--release", "--locked", "--offline",
            "-p", "relax-runner", "--bin", "relax"]


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
        # The evidence packet directory exists before any gate so a refusal
        # still publishes its bounded receipt; the disposable run root is
        # only created after the volume and source gates pass.
        os.makedirs(self.packet_dir, exist_ok=True)
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
        self.private_home()
        if SETUP_HEAVY_SEAM is not None:
            # Self-check seam only: substitute the heavy acquisition/build
            # phases so a later phase (the seeder refusal) is driven through
            # the real entry, gates and publication. A guarded run never
            # sets this.
            SETUP_HEAVY_SEAM(self)
        else:
            self.acquire_upstream()
            self.build_relax()
        self.seed_cargo_home()
        if SETUP_HEAVY_SEAM is None:
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
        # The upstream workspace ships no committed lock: establish it
        # OFFLINE inside this guarded allowance before the locked build (the
        # engine has no registry dependencies, so offline resolution needs no
        # index). No separate prewarm and no network fallback; a failure
        # refuses the occurrence.
        lock_record = measure_one(
            engine_lock_command(), self.relax_src, env, CAP_LOCK_S, self.deadline_ns,
        )
        if lock_record["exit"] != 0:
            raise Refusal(
                "the upstream Relax workspace lock could not be established "
                f"offline (exit {lock_record['exit']}): {lock_record['stderr'][:400]!r}"
            )
        record = measure_one(
            engine_build_command(),
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
            "lock_establishment": {
                "command": lock_record["argv"],
                "seconds": lock_record["seconds"],
                "offline": True,
                "note": "explicit offline generate-lockfile inside the guarded allowance",
            },
            "version": version["stdout"].strip()[:200],
            "command": record["argv"],
        }

    def seed_cargo_home(self) -> None:
        import offline_seed

        host_home = os.environ.get("CARGO_HOME") or os.path.join(
            os.environ.get("HOME", os.path.expanduser("~")), ".cargo"
        )
        # Normalize the invoking cache SOURCE at the established external
        # boundary (resolved once, positively) before the unchanged anchored
        # seeder runs; every component below the canonical common root stays
        # under the seeder's own no-follow/checksum/content rules.
        source = canonical_cache_source(host_home)
        limits = {
            "anchor": "no-follow per component below the canonical common root",
            "aggregate_bytes": offline_seed.LIMITS.aggregate_bytes,
            "work_seconds": offline_seed.LIMITS.work_seconds,
            "git_candidates": offline_seed.LIMITS.git_candidates,
            "note": "private config, checksum-verified content; host cache read-only",
        }
        # Bound the normalized source facts and seeder limits BEFORE the
        # seeder runs, so even a setup-abort receipt carries them.
        self.figures["seed_source"] = {**source, "limits": limits}
        with open(os.path.join(REPO, "Cargo.lock"), "rb") as handle:
            lock_bytes = handle.read()
        destination = os.path.join(self.run_root, "cargo-homes", "shared")
        os.makedirs(destination)
        started = time.monotonic()
        try:
            seed = offline_seed.seed_cargo_home(
                source["common_root"],
                destination,
                lock_bytes,
                deadline_ns=self.deadline_ns - int(STOP_MARGIN_S * 1e9),
            )
        except offline_seed.SeedRefusal as error:
            # Translate the real seeder exception at this boundary into this
            # entry's bounded Refusal, preserving the original phase and
            # reason; setup then aborts through the actual guarded_main
            # publication with every declared case unexecuted.
            raise Refusal(f"seed: {error}") from error
        self.shared_cargo_home = destination
        self.figures["seed"] = {
            "source": source,
            "destination": destination,
            "aggregate_bytes": seed.get("aggregate_bytes"),
            "seconds": round(time.monotonic() - started, 3),
            "network_seconds": 0.0,
            "limits": limits,
            "note": "offline seed from the normalized invoking Cargo home; no download",
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
                RUSTUP_TOOLCHAIN=TOOLCHAIN,
                TMPDIR=os.path.join(arm_home, "tmp"),
                TMP=os.path.join(arm_home, "tmp"),
                TEMP=os.path.join(arm_home, "tmp"),
            )
            os.makedirs(arm_home, exist_ok=True)
            os.makedirs(os.path.join(arm_home, "tmp"), exist_ok=True)
            argv = [self.oxide_bin, "run", "--locked", "--offline",
                    "--bin", package_flags[1], *trailing]
            return argv, env
        env = child_env(
            self.env,
            HOME=self.env["HOME"],
            MAKEPAD_RUN_HOME=self.env["MAKEPAD_RUN_HOME"],
            MAKEPAD_RUN_OFFLINE="1",
            CARGO_HOME=self.shared_cargo_home,
            RUSTUP_TOOLCHAIN=TOOLCHAIN,
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
            "rep": record.get("rep"),
            "source_sha256": record.get("source_sha256"),
            "seconds": record.get("seconds"),
            "rss_maxrss_raw": record.get("rss_maxrss_raw"),
            "rss_unit": record.get("rss_unit"),
            "rss_bytes": record.get("rss_bytes"),
            "exit": record.get("exit"),
            "timed_out": record.get("timed_out"),
            "verdict": verdict,
            "stdout_evidence": record.get("stdout_evidence"),
            "stderr_evidence": record.get("stderr_evidence"),
        }
        self.cases.append(row)

    def unexecuted_rows(self) -> list[dict]:
        return declared_unexecuted(self.cases)

    def apply_ops_everywhere(self, path_relative: str, ops: list[tuple[str, str, int]]) -> str:
        """Apply one repetition edit's operations identically across arms and
        return the shared post-edit digest. Every repetition edit must change
        bytes (a no-op repetition is refused, never silently recorded)."""
        digest = None
        for _key, workdir in self._edit_targets(path_relative):
            full = os.path.join(workdir, path_relative)
            with open(full, "r", encoding="utf-8") as handle:
                text = handle.read()
            edited = text
            for old, new, count in ops:
                edited = apply_edit(edited, old, new, count)
            if edited == text:
                raise Refusal(
                    f"the predeclared repetition edit at {path_relative} produced no byte change"
                )
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

    def repetition_block(self, workload: str, case: str, program_args: list[str],
                         cap_s: float, steps: list[dict], oracle_factory) -> None:
        """One edit-labelled block: every repetition applies its own fresh
        predeclared change identically across arms (each step must change
        bytes; the shared post-edit digest is recorded per sample), then all
        arms launch in balanced rotation against that repetition's expected
        observation. Every sample carries its rep index and edit identity."""
        for rep, step in enumerate(steps):
            digest = self.apply_ops_everywhere(step["path"], step["ops"])
            oracle = oracle_factory(step)
            if budget_s(self.deadline_ns, cap_s) <= 0:
                self.figures.setdefault("reps_reached", {})[f"{workload}-{case}"] = rep
                break
            for arm in rotate(ARMS, rep):
                record = self.run_arm(arm, workload, program_args, cap_s)
                record["rep"] = rep
                record["source_sha256"] = digest
                self.record_case(workload, case, record, oracle(record))

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
                             oracle_exact_stdout(record, greet_expected("hello") + "\n"))
        self.cache_snapshot("greet-cold")
        # The genuinely unchanged warm block: no edit between repetitions.
        for rep in range(5):
            if budget_s(self.deadline_ns, cap) <= 0:
                self.figures.setdefault("reps_reached", {})["greet-warm"] = rep
                break
            for arm in rotate(ARMS, rep):
                record = self.run_arm(arm, "greet", ["Tom"], cap)
                record["rep"] = rep
                self.record_case("greet", "warm-unchanged", record,
                                 oracle_exact_stdout(record, greet_expected("hello") + "\n"))
        # Five distinct predeclared body edits, one per repetition.
        body_steps = []
        for index in range(5):
            old, new = greet_body_step(index)
            body_steps.append({
                "path": "greet-core/src/lib.rs",
                "ops": [(old, new, 1)],
                "greeting": GREET_BODY_SEQUENCE[index],
            })
        self.repetition_block(
            "greet", "edit-body", ["Tom"], cap, body_steps,
            lambda step: (lambda record: oracle_exact_stdout(
                record, greet_expected(step["greeting"]) + "\n")),
        )
        # Five alternating interface edits; the observable bytes stay the
        # cumulative body expectation and the greet dependant is invalidated
        # by each type change.
        interface_steps = []
        for index in range(5):
            old, new = greet_interface_step(index)
            interface_steps.append({
                "path": "greet-core/src/lib.rs",
                "ops": [(old, new, 1)],
                "greeting": GREET_BODY_SEQUENCE[-1],
            })
        self.repetition_block(
            "greet", "edit-interface", ["Tom"], cap, interface_steps,
            lambda step: (lambda record: oracle_exact_stdout(
                record, greet_expected(step["greeting"]) + "\n")),
        )
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
        # The genuinely unchanged warm block: no edit between repetitions.
        for rep in range(5):
            if budget_s(self.deadline_ns, cap) <= 0:
                self.figures.setdefault("reps_reached", {})["snapshot-warm"] = rep
                break
            for arm in rotate(ARMS, rep):
                record = self.run_arm(arm, "snapshot", program, cap)
                record["rep"] = rep
                verdict = oracle_fragments(
                    record, exit_codes=(0,), stdout_contains=(SNAPSHOT_HELP_MARKER,),
                    stdout_not_contains=("[trial]",),
                )
                self.record_case("snapshot", "warm-unchanged", record, verdict)

        def snapshot_body_oracle(step):
            expected = SNAPSHOT_HELP_MARKER + " " + step["marker"]
            previous = step["previous_marker"]
            absent = () if previous is None else (previous,)
            return lambda record: oracle_fragments(
                record, exit_codes=(0,), stdout_contains=(expected,),
                stdout_not_contains=absent,
            )

        body_steps = []
        for index in range(5):
            old, new = snapshot_body_step(index)
            body_steps.append({
                "path": "src/bin/oxide/main.rs",
                "ops": [(old, new, 1)],
                "marker": SNAPSHOT_BODY_SEQUENCE[index],
                "previous_marker": None if index == 0 else SNAPSHOT_BODY_SEQUENCE[index - 1],
            })
        self.repetition_block("snapshot", "edit-body", program, cap, body_steps,
                              snapshot_body_oracle)

        def snapshot_interface_oracle(step):
            cumulative = SNAPSHOT_HELP_MARKER + " " + SNAPSHOT_BODY_SEQUENCE[-1]
            return lambda record: oracle_fragments(
                record, exit_codes=(0,), stdout_contains=(cumulative,),
            )

        interface_steps = []
        for index in range(5):
            interface_steps.append({
                "path": "src/dep_info.rs",
                "ops": snapshot_interface_steps(index),
            })
        self.repetition_block("snapshot", "edit-interface", program, cap,
                              interface_steps, snapshot_interface_oracle)
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
                RUSTUP_TOOLCHAIN=TOOLCHAIN,
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

    def samples_bytes(self) -> bytes:
        """The complete, untrimmed per-sample artifact: every recorded case
        row (reached, unexecuted, aborted) with its repetition and edit
        identities, so statistics never silently use only a carried suffix."""
        document = {
            "schema": "oxide.relax-quick-trial-samples/1",
            "cases_total": len(self.cases),
            "cases": self.cases,
        }
        return json.dumps(document, indent=1).encode()

    def receipt(self) -> bytes:
        samples = self.samples_bytes()
        packet_name = self.binding["paths"]["packet_name"]
        base = {
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
            "samples_file": f"samples-{packet_name}.json",
            "samples_rows": len(self.cases),
            "samples_sha256": hashlib.sha256(samples).hexdigest(),
            "statistics_basis": (
                "complete: verdicts, declared_unexecuted and timing_summary cover "
                "every recorded case in the hash-bound samples artifact, whether "
                "this summary carries its row or not"
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
                "attribution": (
                    "rss covers the direct child and its reaped descendants only; "
                    f"raw ru_maxrss is platform-scaled ({RSS_UNIT_NAME} here) and "
                    "normalized to rss_bytes"
                ),
                "cache_bytes": "occupied apparent size; never an SSD-write claim",
                "filesystem_caches": "not globally flushable under this scope",
            },
        }
        document = build_receipt(base, self.cases)
        return json.dumps(document, indent=1).encode()

    def publish(self) -> None:
        # Unreached declared cases become unexecuted rows before both
        # artifacts are built, so absence is never silent.
        self.cases.extend(self.unexecuted_rows())
        self.figures["owned_bytes"] = {
            "run_root": walk_bytes(self.run_root) if os.path.isdir(self.run_root)
            else {"path": self.run_root, "absent": True},
            "packet_dir": walk_bytes(self.packet_dir) if os.path.isdir(self.packet_dir)
            else {"path": self.packet_dir, "absent": True},
            "note": "occupied apparent bytes of the owned scratch and evidence packet; "
                    "never an SSD-write claim",
        }
        samples = self.samples_bytes()
        os.makedirs(self.packet_dir, exist_ok=True)
        packet_name = self.binding["paths"]["packet_name"]
        samples_destination = os.path.join(self.packet_dir, f"samples-{packet_name}.json")
        destination = os.path.join(self.packet_dir, f"receipt-{packet_name}.json")
        try:
            with open(samples_destination + ".tmp", "wb") as handle:
                handle.write(samples)
            os.replace(samples_destination + ".tmp", samples_destination)
            self._receipt_encoded = self.receipt()
            with open(destination + ".tmp", "wb") as handle:
                handle.write(self._receipt_encoded)
            os.replace(destination + ".tmp", destination)
        except OSError as error:
            raise Refusal(f"the receipt packet could not be written: {error}")
        print(f"samples: {samples_destination} ({len(samples)} bytes)")
        print(f"receipt: {destination} ({len(self._receipt_encoded)} bytes)")


# ==========================================================================
# Focused compatibility recheck (oxide#308)
#
# Tom's 2026-11-12 ruling (decision 7b5a88ce-96cb-4fab-a584-2e5deffa28ce v2)
# asks whether a NEWER Relax pin clears the ARM64 intrinsic blocker the 307
# occurrence actually hit, on actual float formatting and the SAME prior Oxide
# workload. The 307 no-go stays exactly as recorded for its own pin. Nothing
# here is a performance comparison: no timing is measured and no integration,
# adoption, installation or default change is authorized by a pass.
# ==========================================================================

COMPAT_UPSTREAM_URL = "https://github.com/makepad/relax"
COMPAT_UPSTREAM_COMMIT = "6ea9d0635f198b9a4510de1c87ac4d7b067f5cf7"
COMPAT_UPSTREAM_TREE = "514321c9f9677cd4d0e7e11a73b5a01d0f98dc69"
COMPAT_UPSTREAM_DATE = "2026-11-12"
COMPAT_UPSTREAM_FIXES = {
    "78cb555a603777ec9bf3905fd303d1f5db15f72c":
        "the NEON intrinsic families as tables in core::arch::aarch64",
    "c0c1a4b895dc1836880278d1099f591bae3696eb":
        "target-feature and safety correction",
    "e91301e873f58c5af56f13e9c04b0028c9e099c3":
        "immediate-argument and vext correction",
}
WORKLOAD_COMMIT = "61a55ebb98fba45cb708cbe9473a0db09a1e086e"
WORKLOAD_TREE = "d672048298332d8ebb60073575a167c63dde4dc5"
WORKLOAD_LOCK_SHA256 = "e7dcf4647e41e3a1b5449e107ac1dac9a1da98076a868a10b1bdfeafa6f37b28"
COMPAT_BINDING_SCHEMA = "oxide.relax-compat-activation/1"
COMPAT_LIMIT_SECONDS = 600
COMPAT_SCRATCH_MIB = 4096
COMPAT_STOP_MARGIN_S = 30
COMPAT_ARMS = ("cargo", "relax")
# Fixed per-command caps, each clipped to the remaining deadline margin.
CAP_COMPAT_VERIFY_S = 60
CAP_COMPAT_FMT_S = 90
CAP_COMPAT_CRC_S = 90
CAP_COMPAT_SNAPSHOT_S = 180
CAP_COMPAT_GIT_S = 60
CAP_COMPAT_ARCHIVE_S = 120
CAP_COMPAT_CENSUS_S = 20

# The pinned runner's own strict argv, read from
# libs/relax/compiler/src/run.rs at the bound commit: `--check-first` waits for
# the borrow checker before a first call, `--frozen` is carried into the
# resolver as a lock + offline flag (resolve.rs: `"--offline" | "--frozen"` sets
# offline, locked registry versions and git commits are read from Cargo.lock,
# and Cargo.lock is never written), `--unattended` sets no-live AND disables
# parking (a compile error is the call's panic, a panic unwinds at once, a crash
# ends the run), and `--quiet` keeps runner chatter off. Never `--relaxed`
# (Relaxed Rust), never `--cached` (newest-snapshot execution).
RELAX_STRICT_FLAGS = ("--check-first", "--frozen", "--unattended", "--quiet")
RELAX_CHECK_FLAGS = ("--frozen",)
# Per-arm diagnostics retained as they land (bounded, with omission facts).
MAX_ARM_EVIDENCE_BYTES = 8 * 1024
# The analysis bound: a body larger than this cannot be compared as complete
# text, so its probe can never match. The exact full-body hash and byte count
# are still recorded.
MAX_CAPTURE_MEMORY_BYTES = 4 * 1024 * 1024
# Pinned runner knob (libs/relax/base/src/dirs.rs): replaces the whole run dir,
# so Relax's caches and build outputs never land inside a materialized source
# tree. Declared as an output-only location for the input manifest too.
RELAX_CACHE_ENV = "MAKEPAD_RUN_CACHE_DIR"
MANIFEST_EXCLUDED_PREFIXES = ("target/",)
# The negative control's dependency requirement: a version the verified private
# source set does not contain.
COMPAT_SOURCE_CONTROL_REQUIREMENT = 'crc32fast = "=1.5.4"'
# The one crates.io sparse namespace the shared seeder serves; the source-set
# verification reads the seeder's own manifest paths under it.
COMPAT_SEED_NAMESPACE = "index.crates.io-1949cf8c6b5b557f"
MAX_MANIFEST_ENTRIES = 4000

# Effect-free control seams; a guarded run never sets any of them.
#   COMPAT_SETUP_SEAM   substitutes the whole heavy setup (entry abort control)
#   COMPAT_STAGE_SEAM   substitutes acquire_upstream + build_relax only, so the
#                       real verification/disposal/seed/materialize composition
#                       runs against a named fake external boundary
#   COMPAT_SEED_SEAM    substitutes the offline seeder (heavy) for that control
#   COMPAT_CHILD_RUNNER substitutes the whole bounded child runner
#   COMPAT_SPAWNER / COMPAT_WAIT / COMPAT_KILL / COMPAT_CENSUS substitute the
#                       real runner's individual external boundaries
COMPAT_SETUP_SEAM = None
COMPAT_STAGE_SEAM = None
COMPAT_SEED_SEAM = None
COMPAT_CHILD_RUNNER = None
COMPAT_SPAWNER = None
COMPAT_WAIT = None
COMPAT_KILL = None
COMPAT_PROCESS_TABLE_SEAM = None
COMPAT_VOLUME_SEAM = None

# The workload lock's own registry identities. The compat fixtures must resolve
# exactly these versions and checksums, so the probes exercise the observer the
# snapshot uses and nothing newer.
PINNED_WORKLOAD_DEPS = {
    "serde_json": ("1.0.151",
                   "c841b55ecdae098c80dcae9cf767f6f8a0c2cdb3416bbef72181df4d0fe73f14"),
    "zmij": ("1.0.23",
             "29666d0abbfad1e3dc4dcf6144730dd3a3ab225bbbdac83319345b1b44ccfc1b"),
    "crc32fast": ("1.5.3",
                  "0ba106e844045ddc02c20768ac4b0b7cb45f2cc2ecbd833ac9f5dacb183c2055"),
}

COMPAT_CASES = [
    {"case": "format", "workload": "compat-fmt", "required": True, "arms": COMPAT_ARMS,
     "probe": "serde_json =1.0.151 float formatting through zmij 1.0.23: 30 f64 and 18 "
              "f32 declared bit patterns plus 12 decimal decode inputs",
     "oracle": "exit 0, every printed decode equals its input bits, every serialized "
               "field independently decodes to those bits, declared coverage meets its "
               "floor and the required sign/subnormal/extreme patterns, and the Relax "
               "arm's stdout equals the plain-Cargo baseline byte for byte"},
    {"case": "crc32", "workload": "compat-crc32", "required": True, "arms": COMPAT_ARMS,
     "probe": "crc32fast =1.5.3 aarch64 target_feature(\"crc\") path: five named "
              "patterns, an 8200-byte multi-stream buffer and a 7-byte streaming fold",
     "oracle": "exit 0, the five ordered named digests, the streaming fold equal to the "
               "one-shot digest of the same bytes, the standard 123456789 -> cbf43926 "
               "check value, and exact stdout equality with the baseline"},
    {"case": "source-constrained", "workload": "compat-crc32", "required": True,
     "control": True, "arms": ("relax",),
     "probe": "a scratch copy of compat-crc32 whose manifest requires "
              f"{COMPAT_SOURCE_CONTROL_REQUIREMENT}, a version absent from the verified "
              "private Cargo source set",
     "oracle": "the pinned runner must FAIL (any nonzero exit, no timeout): the private "
               "source set, whose registry and git contents are verified to be exactly the "
               "workload lock's, constrains resolution. A zero exit means resolution was "
               "not source-constrained and compatibility cannot be cleared"},
    {"case": "snapshot", "workload": "snapshot", "required": True, "arms": COMPAT_ARMS,
     "probe": "the unchanged prior workload's read-only --help route "
              f"({WORKLOAD_COMMIT})",
     "oracle": "exit 0, stdout carries SNAPSHOT_HELP_MARKER and no edit marker, and the "
               "Relax arm's stdout equals the baseline byte for byte"},
]

COMPAT_FMT_MIN = {"f64": 20, "f32": 14, "parse_f64": 6, "parse_f32": 3}
COMPAT_FMT_REQUIRED_F64 = (
    "8000000000000000",  # negative zero
    "0000000000000001",  # smallest subnormal
    "7fefffffffffffff",  # f64::MAX
    "0010000000000000",  # smallest normal
    "000fffffffffffff",  # largest subnormal
)
COMPAT_FMT_REQUIRED_F32 = (
    "80000000",
    "00000001",
    "7f7fffff",
    "00800000",
    "007fffff",
)
COMPAT_FMT_HEADER = re.compile(r"^declared f64=(\d+) f32=(\d+) parse_f64=(\d+) parse_f32=(\d+)$")
COMPAT_FMT_F64 = re.compile(r"^f64 (\d+) ([0-9a-f]{16}) (\S+) ([0-9a-f]{16})$")
COMPAT_FMT_F32 = re.compile(r"^f32 (\d+) ([0-9a-f]{8}) (\S+) ([0-9a-f]{8})$")
COMPAT_FMT_PARSE_F64 = re.compile(r"^parse-f64 (\d+) (\S+) ([0-9a-f]{16}) (\S+) ([0-9a-f]{16})$")
COMPAT_FMT_PARSE_F32 = re.compile(r"^parse-f32 (\d+) (\S+) ([0-9a-f]{8}) (\S+) ([0-9a-f]{8})$")
COMPAT_FMT_SUMMARY = re.compile(r"^checked=(\d+) failures=(\d+)$")
COMPAT_CRC32_ROWS = ("empty", "check", "fox", "long", "zeros")
COMPAT_CRC32_ROW = re.compile(r"^crc32 (\d) ([a-z]+) ([0-9a-f]{8})$")
COMPAT_CRC32_FOLD = re.compile(r"^crc32-fold long ([0-9a-f]{8})$")
COMPAT_CRC32_STREAM = re.compile(r"^crc32-stream check ([0-9a-f]{8})$")
COMPAT_CRC32_CHECK_LINE = "crc32-check 123456789 cbf43926"
COMPAT_CRC32_CHECK_VALUE = "cbf43926"
COMPAT_HELLO_STDOUT = "hello-relax-compat\n"

# The ONLY stage output this occurrence may remove, after that phase's children
# were positively reaped and the copied executable (with its retained checkout
# and bundled std) was verified. The checkout, the seeded Cargo homes, the
# private home, the staged binaries and every workload source are NOT listed
# and are never disposed: Relax reads its std from the build-time checkout.
COMPAT_DISPOSABLE_PATHS = (
    "stage-relax-target",
    "workloads/compat-fmt/cargo/target",
    "workloads/compat-fmt/relax/compat-fmt/target",
    "workloads/compat-crc32/cargo/target",
    "workloads/compat-crc32/relax/compat-crc32/target",
    "workloads/snapshot/cargo/target",
)

# Unmeasured planning allocation for the effect-free plan, in seconds. It sums
# to the 570 s work window (600 s total less the 30 s stop margin). The fixed
# caps above remain the maxima, each clipped to the remaining time.
COMPAT_PLAN_ALLOCATION = (
    ("upstream acquisition", 40),
    ("engine lock establishment", 10),
    ("engine build", 185),
    ("engine verification", 20),
    ("post-disposal engine smoke", 10),
    ("offline seed from the workload lock", 15),
    ("workload materialization and offline fixture locks", 25),
    ("format probe (both arms)", 35),
    ("crc32 probe (both arms)", 25),
    ("snapshot baseline arm", 70),
    ("snapshot Relax arm", 105),
    ("input identity baseline and closeout recheck", 15),
    ("receipt publication", 15),
)

COMPAT_PREMISE_CHECKLIST = {
    "private_home_rustup": (
        "Applies. The invoking RUSTUP_HOME (or HOME/.rustup when unset) is captured "
        "before HOME is replaced, preserved in every child, RUSTUP_TOOLCHAIN=1.97.1 is "
        "pinned and RUSTUP_AUTO_INSTALL=0 is set; no toolchain is ever installed. "
        "Each fixture lock is established offline inside the owned scratch before any "
        "--locked call. Source: CompatRun.private_home, CompatRun.verify_env, "
        "CompatRun.relax_env, CompatRun.materialize, scrubbed_env."
    ),
    "account_home_control": (
        "Does not apply: this mode stages no Oxide front door and runs no oxide "
        "command, so no account home is consulted. The private HOME still isolates "
        "every arm's Relax stores and temporary directories. Source: "
        "CompatRun.arm_argv_env (the only argv are `cargo run` and `<relax> run`)."
    ),
    "environment_isolation": (
        "Applies. Every child environment is built from the scrubbed base that drops "
        "inherited OXIDE_*, CARGO_*, every RUST* value, MAKEPAD_*, XDG_* and SCCACHE_*, "
        "then declares RUSTFLAGS empty, restores the invoking RUSTUP_HOME and pins the "
        "toolchain. No host pool, wrapper or flag set can reach an arm. Source: "
        "scrubbed_env, child_env, CompatRun.arm_argv_env."
    ),
    "automatic_adoption": (
        "Does not apply: the compat probes convert nothing and run no product "
        "command. Source: CompatRun.arm_argv_env (no `oxide` invocation exists)."
    ),
    "cargo_external_argv": (
        "Does not apply: the compat probes invoke no Cargo external subcommand. The "
        "Relax runner is invoked directly as its own executable, never through Cargo. "
        "Source: CompatRun.arm_argv_env."
    ),
    "owned_images": (
        "Does not apply: no disk image is created or attached. Scratch is a directory "
        "inside the supervisor's owned root, and only allowlisted stage output there is "
        "ever removed. Source: CompatRun.setup, CompatRun.dispose_stage_output."
    ),
    "compiler_flags": (
        "Applies. RUSTFLAGS and CARGO_ENCODED_RUSTFLAGS are scrubbed and declared "
        "empty for every arm, so an inherited encoded value cannot change either arm. "
        "Source: scrubbed_env (RUSTFLAGS empty, every CARGO_* value dropped, which "
        "includes CARGO_ENCODED_RUSTFLAGS), quick_recipe_env."
    ),
    "temporary_storage": (
        "Applies. The body runs only under the supervisor's selected-volume default: "
        "run root, private HOME, CARGO_HOME and TMPDIR/TMP/TEMP live inside one fresh "
        "run root; a /private/tmp fallback is refused before any effect. No image. "
        "Source: guard_exports, CompatRun.setup, CompatRun.private_home."
    ),
    "macos_mktemp": (
        "Applies by construction, not by mktemp: one run root is created from the "
        "activation's single safe component under the guard-exported scratch root and "
        "refused if it already exists; every per-arm home, Cargo home and temp "
        "directory sits inside that owned root. Source: CompatRun.setup, "
        "CompatRun.private_home (no mktemp call exists in this mode)."
    ),
}


# --------------------------------------------------------------------------
# Compat helpers (pure except the read-only git queries)
# --------------------------------------------------------------------------


def git_bytes(args: list[str]) -> bytes:
    """One read-only git query in this repository; a failure is a refusal."""
    run = subprocess.run(["git", *args], cwd=REPO, capture_output=True, timeout=60)
    if run.returncode != 0:
        detail = run.stderr[:300].decode("utf-8", "replace")
        raise Refusal(f"git {' '.join(args)} failed: {detail!r}")
    return run.stdout


def lock_registry_identities(lock_text: str) -> dict:
    """Registry package identities from one Cargo.lock text, by block parse.

    Only `[[package]]` blocks with a registry source and a checksum qualify; a
    pinned name without a registry source or checksum is a refusal, never a
    silently skipped package."""
    identities: dict = {}
    for block in lock_text.split("[[package]]")[1:]:
        name = re.search(r'^name = "([^"]+)"$', block, re.M)
        version = re.search(r'^version = "([^"]+)"$', block, re.M)
        source = re.search(r'^source = "([^"]*)"$', block, re.M)
        checksum = re.search(r'^checksum = "([^"]*)"$', block, re.M)
        if name is None or version is None:
            raise Refusal("the lock has an unreadable package block")
        if source is None or not source.group(1).startswith("registry+"):
            continue
        if checksum is None:
            raise Refusal(f"the lock's registry package {name.group(1)} carries no checksum")
        identities[name.group(1)] = {
            "version": version.group(1),
            "checksum": checksum.group(1),
        }
    return identities


def lock_sources(lock_text: str) -> dict:
    """The full package sets one Cargo.lock declares: registry packages with
    their exact checksums, and git packages with their repository and commit."""
    registry: list = []
    git: list = []
    for block in lock_text.split("[[package]]")[1:]:
        name = re.search(r'^name = "([^"]+)"$', block, re.M)
        version = re.search(r'^version = "([^"]+)"$', block, re.M)
        source = re.search(r'^source = "([^"]*)"$', block, re.M)
        checksum = re.search(r'^checksum = "([^"]*)"$', block, re.M)
        if name is None or version is None:
            raise Refusal("the lock has an unreadable package block")
        if source is None:
            continue  # a path or workspace member: no registry or git identity
        value = source.group(1)
        if value.startswith("registry+"):
            if checksum is None:
                raise Refusal(f"the lock's {name.group(1)} carries no checksum")
            registry.append({"name": name.group(1), "version": version.group(1),
                             "checksum": checksum.group(1)})
        elif value.startswith("git+"):
            spec = value[len("git+"):]
            url, _, fragment = spec.partition("#")
            url = url.split("?", 1)[0]
            git.append({"name": name.group(1), "version": version.group(1),
                        "url": url, "commit": fragment})
    for package in git:
        if not package["commit"]:
            raise Refusal(f"the lock's git package {package['name']} names no commit")
    return {"registry": registry, "git": git,
            "lock_sha256": hashlib.sha256(lock_text.encode("utf-8")).hexdigest()}


def pinned_dependency_identities(lock_text: str) -> dict:
    """The three declared workload pins, all required, all exact."""
    identities = lock_registry_identities(lock_text)
    facts: dict = {}
    for name, (version, checksum) in PINNED_WORKLOAD_DEPS.items():
        found = identities.get(name)
        if found is None:
            raise Refusal(f"the lock does not pin {name}")
        if found["version"] != version or found["checksum"] != checksum:
            raise Refusal(
                f"the lock's {name} is {found['version']} ({found['checksum'][:12]}...), "
                f"not the declared {version} ({checksum[:12]}...)"
            )
        facts[name] = dict(found)
    return facts


def verify_workload(spec: dict) -> dict:
    """The UNCHANGED prior snapshot identity: commit, tree and lock bytes.

    This is bound separately from the apparatus head, so the workload can never
    drift to this repository's newer tree. All queries are read-only."""
    if not isinstance(spec, dict):
        raise Refusal("the activation has no workload object")
    if spec.get("commit") != WORKLOAD_COMMIT or spec.get("tree") != WORKLOAD_TREE:
        raise Refusal("the activation does not bind the prepared workload commit and tree")
    if spec.get("lock_sha256") != WORKLOAD_LOCK_SHA256:
        raise Refusal("the activation does not bind the prepared workload lock digest")
    git_bytes(["cat-file", "-e", f"{WORKLOAD_COMMIT}^{{commit}}"])
    tree = git_bytes(["rev-parse", f"{WORKLOAD_COMMIT}^{{tree}}"]).decode().strip()
    if tree != WORKLOAD_TREE:
        raise Refusal(f"workload commit {WORKLOAD_COMMIT} has tree {tree!r}, not {WORKLOAD_TREE}")
    lock_bytes = git_bytes(["show", f"{WORKLOAD_COMMIT}:Cargo.lock"])
    digest = hashlib.sha256(lock_bytes).hexdigest()
    if digest != WORKLOAD_LOCK_SHA256:
        raise Refusal(f"the workload lock is {digest}, not the prepared {WORKLOAD_LOCK_SHA256}")
    dependencies = pinned_dependency_identities(lock_bytes.decode("utf-8"))
    return {
        "commit": WORKLOAD_COMMIT,
        "tree": tree,
        "lock_sha256": digest,
        "lock_bytes": len(lock_bytes),
        "dependencies": dependencies,
    }


def compat_declared_unexecuted(cases: list[dict]) -> list[dict]:
    present = {(row.get("case"), row.get("arm")) for row in cases}
    rows: list[dict] = []
    for case in COMPAT_CASES:
        for arm in case["arms"]:
            if (case["case"], arm) not in present:
                rows.append({"case": case["case"], "arm": arm, "verdict": "unexecuted"})
    return rows


def compat_correspondence(verdict: str, baseline_record: dict, arm_record: dict) -> str:
    """The Relax arm's verdict: its own structural result, then exact COMPLETE
    byte correspondence with the baseline (full-body SHA-256 and byte count, both
    bodies complete and readable). A structural failure is reported as itself,
    never replaced by a comparison result."""
    if verdict != "match":
        return verdict
    for record in (baseline_record, arm_record):
        if not record.get("stdout_complete") or not record.get("stdout_readable"):
            return "mismatch"
    if (baseline_record.get("stdout_sha256") is None
            or baseline_record.get("stdout_sha256") != arm_record.get("stdout_sha256")
            or baseline_record.get("stdout_bytes") != arm_record.get("stdout_bytes")):
        return "mismatch"
    return "match"


def compat_collection_exit(aborted: str | None, cases: list[dict]) -> int:
    if aborted:
        return 1
    if not cases:
        return 1
    return 0 if all(row.get("verdict") == "match" for row in cases) else 1


def _json_number(text: str) -> float:
    value = json.loads(text)
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{text!r} is not one JSON number")
    return float(value)


def compat_fmt_oracle(record: dict) -> tuple[str, dict]:
    """The independent float-formatting oracle: structure, self-declared
    coverage, both decodes (the program's and this process's) and the summary.
    Only the cross-arm byte comparison is left to the caller."""
    text, refusal = compat_output_text(record)
    if refusal is not None:
        return ("timeout" if record.get("timed_out") else "mismatch", {"reason": refusal})
    if record.get("exit") != 0:
        return ("mismatch", {"reason": f"exit {record.get('exit')}"})
    lines = text.splitlines()
    if not lines:
        return ("mismatch", {"reason": "empty stdout"})
    header = COMPAT_FMT_HEADER.match(lines[0])
    if header is None:
        return ("mismatch", {"reason": "no declared header line"})
    counts = {
        name: int(value)
        for name, value in zip(("f64", "f32", "parse_f64", "parse_f32"), header.groups())
    }
    for name, floor in COMPAT_FMT_MIN.items():
        if counts[name] < floor:
            return ("mismatch", {"reason": f"declared {name}={counts[name]} below the floor {floor}"})
    body = lines[1:]
    summary = COMPAT_FMT_SUMMARY.match(body[-1]) if body else None
    if summary is None:
        return ("mismatch", {"reason": "no checked/failures summary line"})
    total = counts["f64"] + counts["f32"] + counts["parse_f64"] + counts["parse_f32"]
    if int(summary.group(1)) != total or int(summary.group(2)) != 0:
        return ("mismatch", {"reason": f"summary {body[-1]!r} does not cover {total} inputs"})
    rows = body[:-1]
    if len(rows) != total:
        return ("mismatch", {"reason": f"{len(rows)} rows, expected {total}"})
    position = 0
    seen_f64: list[str] = []
    for index in range(counts["f64"]):
        match = COMPAT_FMT_F64.match(rows[position])
        position += 1
        if match is None or int(match.group(1)) != index:
            return ("mismatch", {"reason": f"f64 row {index} is malformed or out of order"})
        bits, text, back = match.group(2), match.group(3), match.group(4)
        if back != bits:
            return ("mismatch", {"reason": f"f64 row {index} decoded {back} for input {bits}"})
        try:
            decoded = _json_number(text)
        except (ValueError, TypeError) as error:
            return ("mismatch", {"reason": f"f64 row {index} text is not a JSON number: {error}"})
        if struct.unpack("<Q", struct.pack("<d", decoded))[0] != int(bits, 16):
            return ("mismatch", {"reason": f"f64 row {index} independent decode differs from {bits}"})
        seen_f64.append(bits)
    seen_f32: list[str] = []
    for index in range(counts["f32"]):
        match = COMPAT_FMT_F32.match(rows[position])
        position += 1
        if match is None or int(match.group(1)) != index:
            return ("mismatch", {"reason": f"f32 row {index} is malformed or out of order"})
        bits, text, back = match.group(2), match.group(3), match.group(4)
        if back != bits:
            return ("mismatch", {"reason": f"f32 row {index} decoded {back} for input {bits}"})
        try:
            decoded = _json_number(text)
        except (ValueError, TypeError) as error:
            return ("mismatch", {"reason": f"f32 row {index} text is not a JSON number: {error}"})
        if struct.unpack("<I", struct.pack("<f", decoded))[0] != int(bits, 16):
            return ("mismatch", {"reason": f"f32 row {index} independent decode differs from {bits}"})
        seen_f32.append(bits)
    for index in range(counts["parse_f64"]):
        match = COMPAT_FMT_PARSE_F64.match(rows[position])
        position += 1
        if match is None or int(match.group(1)) != index:
            return ("mismatch", {"reason": f"parse-f64 row {index} is malformed or out of order"})
        bits, again, back = match.group(3), match.group(4), match.group(5)
        if back != bits:
            return ("mismatch", {"reason": f"parse-f64 row {index} re-encode differs from {bits}"})
        try:
            decoded = _json_number(again)
        except (ValueError, TypeError) as error:
            return ("mismatch", {"reason": f"parse-f64 row {index} re-encoding is unreadable: {error}"})
        if struct.unpack("<Q", struct.pack("<d", decoded))[0] != int(bits, 16):
            return ("mismatch", {"reason": f"parse-f64 row {index} independent decode differs from {bits}"})
    for index in range(counts["parse_f32"]):
        match = COMPAT_FMT_PARSE_F32.match(rows[position])
        position += 1
        if match is None or int(match.group(1)) != index:
            return ("mismatch", {"reason": f"parse-f32 row {index} is malformed or out of order"})
        bits, again, back = match.group(3), match.group(4), match.group(5)
        if back != bits:
            return ("mismatch", {"reason": f"parse-f32 row {index} re-encode differs from {bits}"})
        try:
            decoded = _json_number(again)
        except (ValueError, TypeError) as error:
            return ("mismatch", {"reason": f"parse-f32 row {index} re-encoding is unreadable: {error}"})
        if struct.unpack("<I", struct.pack("<f", decoded))[0] != int(bits, 16):
            return ("mismatch", {"reason": f"parse-f32 row {index} independent decode differs from {bits}"})
    missing = [bits for bits in COMPAT_FMT_REQUIRED_F64 if bits not in seen_f64]
    if missing:
        return ("mismatch", {"reason": f"required f64 patterns absent: {missing}"})
    missing = [bits for bits in COMPAT_FMT_REQUIRED_F32 if bits not in seen_f32]
    if missing:
        return ("mismatch", {"reason": f"required f32 patterns absent: {missing}"})
    return ("match", {"counts": counts, "checked": total, "f64_rows": len(seen_f64),
                      "f32_rows": len(seen_f32)})


def compat_crc32_oracle(record: dict) -> tuple[str, dict]:
    text, refusal = compat_output_text(record)
    if refusal is not None:
        return ("timeout" if record.get("timed_out") else "mismatch", {"reason": refusal})
    if record.get("exit") != 0:
        return ("mismatch", {"reason": f"exit {record.get('exit')}"})
    lines = text.splitlines()
    digests: dict = {}
    for position, name in enumerate(COMPAT_CRC32_ROWS):
        match = COMPAT_CRC32_ROW.match(lines[position]) if position < len(lines) else None
        if match is None or int(match.group(1)) != position or match.group(2) != name:
            return ("mismatch", {"reason": f"crc32 row {position} ({name}) is malformed or out of order"})
        digests[name] = match.group(3)
    tail = lines[len(COMPAT_CRC32_ROWS):]
    fold = COMPAT_CRC32_FOLD.match(tail[0]) if tail else None
    if fold is None:
        return ("mismatch", {"reason": "no crc32-fold line"})
    if fold.group(1) != digests["long"]:
        return ("mismatch", {"reason": "the streaming fold differs from the one-shot digest"})
    stream = COMPAT_CRC32_STREAM.match(tail[1]) if len(tail) > 1 else None
    if stream is None or stream.group(1) != COMPAT_CRC32_CHECK_VALUE:
        return ("mismatch", {"reason": "the streamed 123456789 digests is not the check value"})
    if digests["check"] != COMPAT_CRC32_CHECK_VALUE:
        return ("mismatch", {"reason": f"the one-shot check digest is {digests['check']}"})
    if len(tail) < 3 or tail[2] != COMPAT_CRC32_CHECK_LINE:
        return ("mismatch", {"reason": "the explicit check line is absent or different"})
    return ("match", {"digests": digests, "fold": fold.group(1)})


def compat_snapshot_oracle(record: dict) -> tuple[str, dict]:
    text, refusal = compat_output_text(record)
    if refusal is not None:
        return ("timeout" if record.get("timed_out") else "mismatch", {"reason": refusal})
    if record.get("exit") != 0:
        return ("mismatch", {"reason": f"exit {record.get('exit')}"})
    stdout = text
    if SNAPSHOT_HELP_MARKER not in stdout:
        return ("mismatch", {"reason": "the help marker is absent"})
    if "[trial" in stdout:
        return ("mismatch", {"reason": "the help output carries an edit marker"})
    return ("match", {"stdout_bytes": len(stdout)})


def compat_source_control_oracle(record: dict) -> tuple[str, dict]:
    """The negative control: a requirement the verified private source set does
    not contain must FAIL on the pinned runner. A zero exit proves resolution was
    not source-constrained, so no compatibility credit can be claimed."""
    if record.get("timed_out"):
        return ("timeout", {"reason": "the control did not finish inside its cap"})
    if record.get("exit") == 0:
        return ("mismatch", {
            "reason": "the pinned runner resolved a requirement that the verified "
                      "private source set does not contain",
            "stdout_sha256": record.get("stdout_sha256"),
            "stdout_bytes": record.get("stdout_bytes"),
        })
    return ("match", {"exit": record.get("exit"),
                      "stderr_sha256": record.get("stderr_sha256"),
                      "stderr_bytes": record.get("stderr_bytes")})


def parse_compat_binding(payload: bytes, mode: str) -> dict:
    """One planner-issued compat activation. Every field is exact."""
    if mode not in ("compat-diagnostic", "compat-qualify"):
        raise Refusal(f"{mode!r} is not a compat mode")
    try:
        document = json.loads(payload)
    except ValueError as error:
        raise Refusal(f"the activation document is not JSON: {error}")
    if not isinstance(document, dict) or document.get("schema") != COMPAT_BINDING_SCHEMA:
        raise Refusal("the activation document is not the compat schema")
    if not safe_component(document.get("planner_reference")):
        raise Refusal("the activation names no planner reference")
    if document.get("mode") != mode:
        raise Refusal(f"the activation binds mode {document.get('mode')!r}, not {mode!r}")
    if document.get("script_sha256") != sha256_file(SCRIPT):
        raise Refusal("the activation does not bind this script's exact bytes")
    apparatus = document.get("apparatus")
    if not isinstance(apparatus, dict):
        raise Refusal("the activation has no apparatus object")
    if not is_hex(apparatus.get("head"), 40) or not is_hex(apparatus.get("tree"), 40):
        raise Refusal("the activation's apparatus head and tree are not exact")
    workload = document.get("workload")
    if not isinstance(workload, dict):
        raise Refusal("the activation has no workload object")
    if (workload.get("commit") != WORKLOAD_COMMIT
            or workload.get("tree") != WORKLOAD_TREE
            or workload.get("lock_sha256") != WORKLOAD_LOCK_SHA256):
        raise Refusal("the activation does not bind the prepared workload commit, tree and lock")
    upstream = document.get("upstream")
    if not isinstance(upstream, dict):
        raise Refusal("the activation has no upstream object")
    if upstream.get("commit") != COMPAT_UPSTREAM_COMMIT or upstream.get("tree") != COMPAT_UPSTREAM_TREE:
        raise Refusal("the activation does not bind the pinned upstream commit and tree")
    if document.get("toolchain") != TOOLCHAIN:
        raise Refusal("the activation does not bind the pinned toolchain")
    guard = document.get("guard")
    if not isinstance(guard, dict):
        raise Refusal("the activation has no guard object")
    if guard.get("supervisor_sha256") != EXPECTED_RUNNER_SHA256:
        raise Refusal("the activation does not name the qualified supervisor")
    if (guard.get("limit_seconds") != COMPAT_LIMIT_SECONDS
            or guard.get("scratch_mib") != COMPAT_SCRATCH_MIB):
        raise Refusal("the activation's guard bounds are not this route's fixed bounds")
    if guard.get("stop_margin_seconds") != COMPAT_STOP_MARGIN_S:
        raise Refusal("the activation's stop margin is not the original 30 s")
    volume = document.get("volume")
    if not isinstance(volume, dict) or volume.get("uuid") != SELECTED_VOLUME_UUID:
        raise Refusal("the activation does not name the selected Stash volume")
    receipt = document.get("diagnostic_receipt_sha256")
    if receipt and not is_hex(receipt, 64):
        raise Refusal("the activation's diagnostic receipt digest is not a SHA-256")
    if mode == "compat-qualify" and not receipt:
        raise Refusal("a counted compat activation must name the diagnostic receipt it follows")
    paths = document.get("paths")
    if not isinstance(paths, dict):
        raise Refusal("the activation has no paths object")
    for key in ("run_root_name", "packet_name"):
        if not safe_component(paths.get(key)):
            raise Refusal(f"the activation's {key} is not one safe component")
    return document


def compat_binding_document(mode: str, script_sha: str) -> bytes:
    return json.dumps(
        {
            "schema": COMPAT_BINDING_SCHEMA,
            "planner_reference": "compat-activation-1",
            "mode": mode,
            "script_sha256": script_sha,
            "apparatus": {"head": "a" * 40, "tree": "b" * 40},
            "workload": {"commit": WORKLOAD_COMMIT, "tree": WORKLOAD_TREE,
                         "lock_sha256": WORKLOAD_LOCK_SHA256},
            "upstream": {"commit": COMPAT_UPSTREAM_COMMIT, "tree": COMPAT_UPSTREAM_TREE},
            "toolchain": TOOLCHAIN,
            "diagnostic_receipt_sha256": "c" * 64 if mode == "compat-qualify" else "",
            "guard": {"supervisor_sha256": EXPECTED_RUNNER_SHA256,
                      "limit_seconds": COMPAT_LIMIT_SECONDS,
                      "scratch_mib": COMPAT_SCRATCH_MIB,
                      "stop_margin_seconds": COMPAT_STOP_MARGIN_S},
            "volume": {"uuid": SELECTED_VOLUME_UUID},
            "paths": {"run_root_name": "relax-compat-run", "packet_name": "compat-packet"},
        }
    ).encode()


def compat_fixture_digests() -> dict[str, str]:
    return {path: digest for path, digest in fixture_digests().items() if "/compat-" in path}


def compat_readiness() -> tuple[list[str], list[str]]:
    """(readiness notes, blockers) from actual metadata. No compile or run."""
    notes: list[str] = []
    blockers: list[str] = []
    notes.append(f"platform: {platform.system()} {platform.machine()}")
    if platform.system() != "Darwin" or platform.machine() not in ("arm64", "aarch64"):
        blockers.append("the prepared compat scope is macOS arm64 on this host")
    notes.append(f"script sha256: {sha256_file(SCRIPT)}")
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
        notes.append(f"apparatus head/tree: {head} / {tree}")
    blockers.extend(head_blockers)
    try:
        workload = verify_workload({"commit": WORKLOAD_COMMIT, "tree": WORKLOAD_TREE,
                                    "lock_sha256": WORKLOAD_LOCK_SHA256})
        notes.append(
            "workload identity: " + workload["commit"] + " / " + workload["tree"]
            + " / lock " + workload["lock_sha256"] + f" ({workload['lock_bytes']} bytes)"
        )
        notes.append(
            "workload dependency pins: " + ", ".join(
                f"{name} {facts['version']}" for name, facts in workload["dependencies"].items()
            )
        )
    except Refusal as error:
        blockers.append(f"workload identity: {error}")
    notes.append(
        "upstream pin: " + COMPAT_UPSTREAM_COMMIT + " / " + COMPAT_UPSTREAM_TREE
        + f" ({COMPAT_UPSTREAM_DATE}); the guarded acquisition re-verifies the tree"
    )
    notes.append(
        "upstream fix commits required in the checkout: "
        + ", ".join(f"{sha[:12]} ({label})" for sha, label in COMPAT_UPSTREAM_FIXES.items())
    )
    toolchains = toolchain_inventory()
    notes.append("installed toolchains: " + "; ".join(toolchains[:8]))
    if not any(line.startswith(TOOLCHAIN) or f" {TOOLCHAIN} " in f" {line} " for line in toolchains):
        blockers.append(f"toolchain {TOOLCHAIN} is not installed; the recheck never installs one")
    digests = compat_fixture_digests()
    notes.append(f"committed compat fixture files: {len(digests)}")
    required = [
        "fixtures/relax-quick-trial/compat-hello/Cargo.toml",
        "fixtures/relax-quick-trial/compat-hello/src/main.rs",
        "fixtures/relax-quick-trial/compat-fmt/Cargo.toml",
        "fixtures/relax-quick-trial/compat-fmt/src/main.rs",
        "fixtures/relax-quick-trial/compat-crc32/Cargo.toml",
        "fixtures/relax-quick-trial/compat-crc32/src/main.rs",
    ]
    for path in required:
        if path not in digests:
            blockers.append(f"required compat fixture input is missing: {path}")
    return (notes, blockers)


def compat_plan_text() -> str:
    notes, blockers = compat_readiness()
    lines: list[str] = []
    lines.append("relax compatibility recheck plan (oxide#308) — effect-free; nothing is")
    lines.append("compiled, acquired, staged, run or measured by this mode.")
    lines.append("")
    lines.append("Readiness from actual metadata:")
    for note in notes:
        lines.append(f"  - {note}")
    lines.append("")
    lines.append(f"Boundaries (prepared, not execution permission): {COMPAT_LIMIT_SECONDS} s total")
    lines.append("including acquisition/engine build/seed/staging and every probe;")
    lines.append(f"{COMPAT_SCRATCH_MIB} MiB monitored owned scratch on the selected Stash volume")
    lines.append(f"{SELECTED_VOLUME_UUID}; {COMPAT_STOP_MARGIN_S} s stop-work margin; no image;")
    lines.append("the supervisor's 20 GiB reserve safeguards and heavy admission. Each")
    lines.append("per-command cap is a minimum of its fixed value and the remaining deadline")
    lines.append("minus the stop margin.")
    lines.append("")
    lines.append("Guarded commands (never run by --compat-plan):")
    lines.append('  effigy research:relax:compat:diagnostic  # planner-issued RELAX_COMPAT_ACTIVATION')
    lines.append('  effigy research:relax:compat:qualify     # after an adequate diagnostic')
    lines.append("  python3 scripts/bounded-test.py --limit 600 --scratch-budget-mib 4096 \\")
    lines.append("      -- python3 scripts/relax-quick-trial.py --compat-diagnostic \\")
    lines.append('      --binding "$RELAX_COMPAT_ACTIVATION" --sha256 "$RELAX_COMPAT_ACTIVATION_SHA256"')
    lines.append("")
    lines.append("Case table (required probes; each runs the plain-Cargo baseline and the")
    lines.append("pinned Relax arm in that order, exactly these argv):")
    lines.append("  - format (compat-fmt, serde_json =1.0.151 -> zmij 1.0.23)")
    lines.append("      cargo: cargo run --locked --offline --quiet --bin compat-fmt")
    lines.append("      relax: <relax> run --check-first --frozen --unattended --quiet "
                 "compat-fmt")
    lines.append("      oracle: exit 0, declared coverage, both decodes bit-exact, exact "
                 "cross-arm stdout")
    lines.append("  - crc32 (compat-crc32, crc32fast =1.5.3)")
    lines.append("      cargo: cargo run --locked --offline --quiet --bin compat-crc32")
    lines.append("      relax: <relax> run --check-first --frozen --unattended --quiet "
                 "compat-crc32")
    lines.append("      oracle: exit 0, ordered digests, fold == one-shot, check value "
                 "cbf43926, exact cross-arm stdout")
    lines.append(f"  - snapshot ({WORKLOAD_COMMIT}, unchanged --help route)")
    lines.append("      cargo: cargo run --locked --offline --quiet --bin oxide -- --help")
    lines.append("      relax: <relax> run --check-first --frozen --unattended --quiet "
                 "oxide-storage --bin oxide -- --help")
    lines.append("      oracle: exit 0, help marker, no edit marker, exact cross-arm stdout")
    lines.append("  - source-constrained (the resolution control; relax arm only)")
    lines.append("      a scratch compat-crc32 copy whose manifest requires")
    lines.append(f"      {COMPAT_SOURCE_CONTROL_REQUIREMENT} (absent from the verified sources) and whose")
    lines.append("      lock omits crc32fast")
    lines.append("      oracle: the pinned runner must FAIL (nonzero exit, no timeout); a zero exit")
    lines.append("      means resolution was not source-constrained and compatibility cannot clear")
    lines.append("")
    lines.append("Strict Relax argv (read from the pinned runner): --check-first, --frozen")
    lines.append("(offline and lock-PREFERRING: the pinned resolver does not fail on lock drift,")
    lines.append("so --frozen is NOT Cargo's fail-on-drift and the source set below is the")
    lines.append("authority), --unattended (no live session AND")
    lines.append("no parking on a compile error, panic or crash) and --quiet; never --relaxed")
    lines.append("or --cached. Every Relax child also gets " + RELAX_CACHE_ENV + " pointing at")
    lines.append("an out-of-tree run dir, so caches and build outputs never land in a source")
    lines.append("tree that is being compared against its own recorded identity.")
    lines.append("")
    lines.append("Engine lifecycle, custody and disposal (fit correction for the 307 stop):")
    lines.append("  1. clone the pinned upstream, re-verify its tree and require the three fix")
    lines.append("     commits; establish the lock offline; build the engine offline. Every")
    lines.append("     setup child (clone, checkout, git queries, archive, extract, patch probes)")
    lines.append("     runs through the same bounded boundary: the private scrubbed environment,")
    lines.append("     its own session, a cap clipped by the remaining deadline and stop margin.")
    lines.append("  2. copy the runner executable, hash it, and verify the COPIED executable")
    lines.append("     with --version, a frozen check and a strict run of the std-only")
    lines.append("     compat-hello package, with the retained bundled std positively present.")
    lines.append("  3. only then remove stage-relax-target, and only under positive IN-BOUNDARY")
    lines.append("     closure: every child is in the guard's own session and process group (no")
    lines.append("     setsid and no separate group, so the qualified supervisor can see and stop")
    lines.append("     it), every child is positively reaped, and a bounded process-table")
    lines.append("     classification of the guard session shows only the guard and its")
    lines.append("     descendants, this script and its transient census helper — this script's own")
    lines.append("     subtree empty and no unclassified row. An orphaned producer, a live child or")
    lines.append("     an unreadable table refuses the removal and fails the run closed.")
    lines.append("  4. re-run a cheap smoke through the copied executable to confirm the retained")
    lines.append("     checkout/bundled std still serve it after disposal.")
    lines.append("  5. seed the private Cargo home from the WORKLOAD lock, then verify that the")
    lines.append("     private source set is EXACTLY the lock: every registry crate re-hashed")
    lines.append("     against its lock checksum, the index holding only the lock's packages, and")
    lines.append("     every git database one of the lock's repositories at its locked commit.")
    lines.append("     --frozen alone does not enforce lock drift, so this source set (not the flag)")
    lines.append("     is what bounds the resolved graph to the lock. A negative control then")
    lines.append("     requires an unsatisfiable requirement to fail on the pinned runner.")
    lines.append("  6. materialize the two fixture copies plus one snapshot copy per arm and the")
    lines.append("     resolution-control copy through bounded children, establish each fixture")
    lines.append("     lock offline and verify its pinned identities.")
    lines.append("  7. record the identity of the ACTUAL materialized inputs before any probe")
    lines.append("     (every fixture, snapshot and control source entry and lock, the staged")
    lines.append("     executable, the bundled std) and re-read all of them after the probes. A")
    lines.append("     changed, missing or ADDED source entry aborts the run: only the declared")
    lines.append("     output-only prefix target/ may gain entries, and added Rust modules, build")
    lines.append("     inputs or configuration files always refuse. Git commit identity alone is")
    lines.append("     never treated as proof of what ran.")
    lines.append("  8. remove each probe's cargo target and Relax cache after that probe's two")
    lines.append("     arms completed, under the same custody gate; the snapshot baseline target")
    lines.append("     is removed before the Relax arm for headroom. Only paths in")
    lines.append("     COMPAT_DISPOSABLE_PATHS may be removed. Never the checkout or bundled std.")
    lines.append("  9. each reached arm's bounded RAW stdout/stderr prefix is written into the")
    lines.append("     evidence packet with its exact full-body hash, byte count, completeness and")
    lines.append("     strict-decode facts BEFORE the next child runs, so a later stop cannot erase")
    lines.append("     an earlier real refusal. Cross-arm correspondence compares the COMPLETE")
    lines.append("     bodies by hash and length; a body over the analysis bound, invalid UTF-8 or")
    lines.append("     an unreadable capture can never match.")
    lines.append("")
    lines.append("Planning allocation (UNMEASURED estimates; the caps above remain the maxima):")
    for phase, seconds in COMPAT_PLAN_ALLOCATION:
        lines.append(f"  - {phase}: {seconds} s")
    lines.append(f"  total: {sum(seconds for _, seconds in COMPAT_PLAN_ALLOCATION)} s of the "
                 f"{COMPAT_LIMIT_SECONDS - COMPAT_STOP_MARGIN_S} s work window")
    lines.append("")
    lines.append("Refusal gates (before any effect; the activation document's shape,")
    lines.append("script bytes, pins and bounds are checked in-process, and every repository")
    lines.append("identity is re-verified inside setup through the same bounded child boundary")
    lines.append("as every other occurrence child, so a pre-effect metadata read never stands")
    lines.append("in for it):")
    lines.append("  - activation document digest, schema, mode, script bytes and planner reference")
    lines.append("  - apparatus head/tree exact and the tracked worktree clean")
    lines.append("  - workload commit/tree and lock bytes bound and re-verified read-only; the")
    lines.append("    three registry pins required at their exact versions and checksums")
    lines.append("  - upstream commit/tree in the activation, re-verified in the checkout, with")
    lines.append("    the three fix commits present as ancestors")
    lines.append("  - selected volume UUID, qualified supervisor digest, fixed bounds and the")
    lines.append("    absence of the bound run root; a counted activation also names the")
    lines.append("    diagnostic receipt digest it follows")
    lines.append("  - the offline seed's canonical source boundary and the unchanged anchored")
    lines.append("    seeder rules; no network fallback, toolchain install or upstream patch")
    lines.append("  - the private source set: an extra or missing registry crate, a checksum")
    lines.append("    mismatch, a foreign git source, a missing locked commit or an extra extracted")
    lines.append("    package refuses; the resolution control must fail on the pinned runner")
    lines.append("  - the materialized-input identity map: a changed, missing or added fixture,")
    lines.append("    snapshot or control entry, lock, staged executable or bundled-std byte after")
    lines.append("    the probes is a refusal (only the declared target/ output prefix may grow)")
    lines.append("")
    lines.append("Premise checklist dispositions (workflow.md, proof authoring):")
    for item, disposition in COMPAT_PREMISE_CHECKLIST.items():
        lines.append(f"  - {item}: {disposition}")
    lines.append("")
    lines.append("History: the 307 trial's no-go for Relax aa540c14… (20 unresolved")
    lines.append("core::arch::aarch64 intrinsics in zmij-1.0.23, and an UNCLOSED 4100.6/4096 MiB")
    lines.append("occurrence with no timings) stays exact for that pin. Nothing here reopens it;")
    lines.append("this mode is new work on a different pin and measures no performance.")
    lines.append("")
    if blockers:
        lines.append("Blockers:")
        for blocker in blockers:
            lines.append(f"  - {blocker}")
        lines.append("The guarded compat modes refuse until every blocker above is resolved;")
        lines.append("this plan itself grants no execution authority.")
    else:
        lines.append("No metadata blocker found; activation stays a separate planner step.")
    lines.append("")
    return "\n".join(lines)


def build_compat_receipt(base: dict, cases: list[dict], cap_bytes: int | None = None) -> dict:
    """One bounded summary receipt. Statistics cover the COMPLETE recorded set;
    the `cases` array is a disclosed excerpt when the cap forces trimming, and
    the complete untrimmed rows live in the hash-bound samples artifact."""
    cap = MAX_RECEIPT_BYTES if cap_bytes is None else cap_bytes
    document = dict(base)
    document["cases"] = list(cases)
    document["cases_total"] = len(cases)
    document["cases_omitted"] = 0
    document["cases_in_receipt"] = len(cases)
    document["verdicts"] = verdict_tally(cases)
    document["declared_unexecuted"] = sum(
        1 for row in cases if row.get("verdict") == "unexecuted"
    )
    encoded = json.dumps(document, indent=1).encode()
    while len(encoded) > cap and len(document["cases"]) > 1:
        document["cases"].pop()
        document["cases_omitted"] += 1
        document["cases_in_receipt"] = len(document["cases"])
        encoded = json.dumps(document, indent=1).encode()
    return document


# --------------------------------------------------------------------------
# Bounded children, in-boundary closure and materialized-input identities
#
# Every child runs in the guard's OWN session and process group: nothing leaves
# the qualified supervisor's cooperative boundary, so the supervisor can see
# and stop it, and a guard stop reaches it. Closure is therefore NOT claimed
# per child session; it is established by classifying the guard's session: a
# disposal is allowed only when the ONLY rows in that session are the guard
# and its own descendants plus our script (and this script's transient census
# helper). Our own subtree must be empty, and any row that is neither ours nor
# the guard's (an orphaned producer, for example) is unclassified and refuses
# disposal. An unreadable table is unknown, never closed.
# --------------------------------------------------------------------------


def _parse_ps_census(text: str, session_id: int) -> dict:
    """Parse `ps -A -o pid=,ppid=,pgid=,sess=` rows for one session.

    A malformed, non-numeric or duplicate row makes the whole table unreadable;
    a readable table with no row of this session is a positive zero."""
    rows: dict = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) != 4 or not all(part.isdigit() for part in parts):
            return {"readable": False, "rows": {}, "error": f"unreadable census row {line!r}"}
        pid, ppid, pgid, sess = (int(part) for part in parts)
        if pid in rows:
            return {"readable": False, "rows": {}, "error": f"duplicate census pid {pid}"}
        rows[pid] = {"ppid": ppid, "pgid": pgid, "sess": sess}
    return {"readable": True, "rows": rows, "error": None}


def _descendants(rows: dict, root: int) -> set:
    children: dict = {}
    for pid, facts in rows.items():
        children.setdefault(facts["ppid"], []).append(pid)
    seen: set = set()
    stack = list(children.get(root, []))
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        stack.extend(children.get(pid, []))
    return seen


def classify_closure(rows: dict, our_pid: int, guard_pid: int, session_id: int,
                     transient: set) -> dict:
    """Classify the guard session's rows for one closure decision.

    Closed means: our own subtree holds nothing but the transient census helper,
    and every other row in the session is the guard itself or a descendant of
    the guard. Anything else (an orphaned producer, a foreign session member, an
    unclassifiable ppid 0 row) is unclassified and refuses disposal."""
    session_rows = {pid for pid, facts in rows.items() if facts["sess"] == session_id}
    ours = _descendants(rows, our_pid) & session_rows
    guard = _descendants(rows, guard_pid) & session_rows
    ours_live = ours - transient
    explained = ours | guard | {guard_pid, our_pid} | transient
    unclassified = session_rows - explained
    closed = not ours_live and not unclassified
    return {
        "closed": closed,
        "session_rows": len(session_rows),
        "our_subtree": sorted(ours_live)[:16],
        "guard_rows": len(guard),
        "unclassified": sorted(unclassified)[:16],
        "unclassified_count": len(unclassified),
        "transient": sorted(transient),
    }


def compat_process_table(env: dict, cwd: str, deadline_ns: int,
                         cap_s: float = CAP_COMPAT_CENSUS_S) -> dict:
    """One bounded, private, positively reaped process table read.

    This is the observation helper for a closure decision; it is a child of this
    script in the guard's group, so the census excludes its own PID. Unreadable,
    timed-out, non-zero or malformed output is UNKNOWN, never closed."""
    allowed = budget_s(deadline_ns, cap_s)
    if allowed <= 0.0:
        return {"readable": False, "rows": {}, "helper_pid": None,
                "error": "no census allowance left before the stop margin"}
    if COMPAT_PROCESS_TABLE_SEAM is not None:
        return COMPAT_PROCESS_TABLE_SEAM(env, cwd, deadline_ns, cap_s)
    try:
        process = subprocess.Popen(
            ["/bin/ps", "-A", "-o", "pid=,ppid=,pgid=,sess="],
            cwd=cwd, env=env, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
    except OSError as error:
        return {"readable": False, "rows": {}, "helper_pid": None,
                "error": f"{type(error).__name__}: {error}"}
    helper_pid = process.pid
    try:
        out, err = process.communicate(timeout=allowed)
    except subprocess.TimeoutExpired:
        process.kill()
        process.communicate()
        return {"readable": False, "rows": {}, "helper_pid": helper_pid,
                "error": "the census helper did not finish inside its allowance"}
    if process.returncode != 0:
        return {"readable": False, "rows": {}, "helper_pid": helper_pid,
                "error": f"ps exit {process.returncode}: {err[:200].decode('utf-8', 'replace')}"}
    parsed = _parse_ps_census(out.decode("utf-8", "replace"), os.getsid(0))
    parsed["helper_pid"] = helper_pid
    return parsed


def compat_child_custody(spawn_error: str | None, reaped: bool, wait_error: str | None) -> dict:
    """The per-child custody facts. Closure is NOT decided here: it is the
    session classification above, taken at each disposal and at closeout."""
    return {
        "spawned": spawn_error is None,
        "spawn_error": spawn_error,
        "reaped": bool(reaped),
        "wait_error": wait_error,
    }


def compat_spawn_real(argv: list[str], cwd: str, env: dict,
                      stdout_path: str, stderr_path: str) -> int:
    """The real spawn boundary: one fresh child with the caller's private
    environment, its standard input on the null device and its output on the
    capture files. It is deliberately NOT given its own session or process
    group: it stays in the guard's group so the qualified supervisor can see and
    stop it. This function never waits; `compat_run_child` reaps the PID."""
    with open(stdout_path, "wb") as out, open(stderr_path, "wb") as err:
        process = subprocess.Popen(
            argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
            stdout=out, stderr=err,
        )
    pid = process.pid
    # This boundary reaps through wait4 below, never through Popen.
    process.returncode = 0
    return pid


def _capture_summary(path: str, analysis_cap: int = MAX_CAPTURE_MEMORY_BYTES) -> dict:
    """Stream one raw capture file: exact full-body SHA-256 and byte count, a
    bounded raw prefix for retention, and — only when the body fits the analysis
    bound — its strict UTF-8 text. A body over the bound or invalid UTF-8 can
    never be used as matching output; the exact hash and length are still
    reported so a cross-arm difference is visible."""
    digest = hashlib.sha256()
    total = 0
    prefix = b""
    text = None
    decode_error = None
    try:
        with open(path, "rb") as handle:
            while True:
                chunk = handle.read(1 << 16)
                if not chunk:
                    break
                digest.update(chunk)
                if len(prefix) < MAX_ARM_EVIDENCE_BYTES:
                    prefix += chunk[:MAX_ARM_EVIDENCE_BYTES - len(prefix)]
                total += len(chunk)
        complete = total <= analysis_cap
        if complete:
            with open(path, "rb") as handle:
                body = handle.read()
            try:
                text = body.decode("utf-8")
            except UnicodeDecodeError as error:
                decode_error = f"{error}"
    except OSError as error:
        return {"readable": False, "sha256": None, "bytes": None, "complete": False,
                "text": None, "decode_error": None, "prefix": b"",
                "error": f"{type(error).__name__}: {error}"}
    return {
        "readable": True,
        "sha256": digest.hexdigest(),
        "bytes": total,
        "complete": complete,
        "text": text,
        "decode_error": decode_error,
        "prefix": prefix,
        "error": None,
    }


def compat_run_child(argv: list[str], cwd: str, env: dict, cap_s: float,
                     deadline_ns: int, capture_dir: str, label: str) -> dict:
    """One fresh child of THIS script, bounded by the shared deadline, captured
    raw and positively reaped.

    The child stays in the guard's session and process group. Its stdout and
    stderr are hashed and counted as COMPLETE bytes (never a decoded prefix), a
    bounded raw prefix is kept for evidence retention, and the text is available
    only when the body fits the analysis bound and is valid UTF-8. Closure is
    decided later by the session classification, never by this call."""
    allowed = budget_s(deadline_ns, cap_s)
    if allowed <= 0.0:
        raise Refusal("the deadline plus stop margin leaves no budget for the next child")
    os.makedirs(capture_dir, exist_ok=True)
    stdout_path = os.path.join(capture_dir, f"{label}.stdout")
    stderr_path = os.path.join(capture_dir, f"{label}.stderr")
    started = time.monotonic()
    spawn = COMPAT_SPAWNER or compat_spawn_real
    pid: int | None = None
    spawn_error: str | None = None
    try:
        pid = spawn(argv, cwd, env, stdout_path, stderr_path)
    except OSError as error:
        spawn_error = f"{type(error).__name__}: {error}"
    wait = COMPAT_WAIT or (lambda target, block: os.wait4(target, 0 if block else os.WNOHANG))
    kill = COMPAT_KILL or (lambda target: os.kill(target, signal.SIGKILL))
    timed_out = False
    reaped = False
    wait_error: str | None = None
    status = None
    usage = None
    if pid is not None:
        while True:
            try:
                done, status, usage = wait(pid, False)
            except ChildProcessError as error:
                wait_error = f"ChildProcessError: {error}"
                break
            except OSError as error:
                wait_error = f"OSError: {error}"
                break
            if done == pid:
                reaped = True
                break
            if time.monotonic() - started >= allowed:
                timed_out = True
                try:
                    kill(pid)
                except ProcessLookupError:
                    pass
                except OSError as error:
                    wait_error = f"kill: {type(error).__name__}: {error}"
                try:
                    done, status, usage = wait(pid, True)
                except ChildProcessError as error:
                    wait_error = f"wait after kill: ChildProcessError: {error}"
                except OSError as error:
                    wait_error = f"wait after kill: OSError: {error}"
                else:
                    reaped = done == pid
                break
            time.sleep(0.01)
    custody = compat_child_custody(spawn_error, reaped, wait_error)
    stdout = _capture_summary(stdout_path)
    stderr = _capture_summary(stderr_path)
    exit_code = None
    if reaped and status is not None:
        exit_code = os.waitstatus_to_exitcode(status)
    record = {
        "argv": argv,
        "cwd": cwd,
        "label": label,
        "exit": exit_code,
        "seconds": round(time.monotonic() - started, 4),
        "timed_out": timed_out,
        "cap_s": round(allowed, 2),
        "rss_maxrss_raw": _usage_maxrss(usage),
        "rss_unit": RSS_UNIT_NAME,
        "rss_bytes": _usage_maxrss(usage) * RSS_UNIT_BYTES,
        "stdout": stdout["text"],
        "stderr": stderr["text"],
        "stdout_sha256": stdout["sha256"],
        "stdout_bytes": stdout["bytes"],
        "stdout_complete": stdout["complete"],
        "stdout_readable": stdout["readable"],
        "stdout_decode_error": stdout["decode_error"],
        "stdout_prefix": stdout["prefix"],
        "stdout_evidence": {
            "sha256": stdout["sha256"],
            "bytes": stdout["bytes"],
            "prefix": stdout["prefix"][:MAX_PREFIX_BYTES].decode("utf-8", "replace"),
            "truncated": bool(stdout["bytes"] is not None
                              and stdout["bytes"] > MAX_PREFIX_BYTES),
            "complete": stdout["complete"],
        },
        "stderr_sha256": stderr["sha256"],
        "stderr_bytes": stderr["bytes"],
        "stderr_complete": stderr["complete"],
        "stderr_readable": stderr["readable"],
        "stderr_decode_error": stderr["decode_error"],
        "stderr_prefix": stderr["prefix"],
        "stderr_evidence": {
            "sha256": stderr["sha256"],
            "bytes": stderr["bytes"],
            "prefix": stderr["prefix"][:MAX_PREFIX_BYTES].decode("utf-8", "replace"),
            "truncated": bool(stderr["bytes"] is not None
                              and stderr["bytes"] > MAX_PREFIX_BYTES),
            "complete": stderr["complete"],
        },
        "custody": custody,
    }
    for path in (stdout_path, stderr_path):
        try:
            os.remove(path)
        except OSError:
            pass
    return record


def _usage_maxrss(usage) -> int:
    try:
        return int(getattr(usage, "ru_maxrss", 0) or 0)
    except (TypeError, ValueError):
        return 0


def compat_output_text(record: dict) -> tuple[str | None, str | None]:
    """(complete stdout text, refusal reason). Only a complete, readable,
    strictly UTF-8 body can be matching output; anything else refuses."""
    if record.get("timed_out"):
        return (None, "the probe did not finish inside its cap")
    if not record.get("stdout_readable"):
        return (None, "the stdout capture was unreadable")
    if not record.get("stdout_complete"):
        return (None, "stdout exceeds the analysis bound; its bytes cannot be compared as text")
    if record.get("stdout_decode_error"):
        return (None, f"stdout is not valid UTF-8: {record['stdout_decode_error']}")
    text = record.get("stdout")
    if not isinstance(text, str):
        return (None, "no complete stdout text was captured")
    return (text, None)


def tree_manifest(root: str, limit_entries: int = MAX_MANIFEST_ENTRIES) -> dict:
    """A bounded content manifest of one materialized tree.

    Every regular file contributes its relative path, size and SHA-256; a
    symlink contributes its target text (never followed), so a swapped link is a
    changed input. Directories are recorded as directory rows so an added empty
    directory is visible. Rows are sorted by path and one cumulative digest
    covers them. Beyond the entry cap the manifest is INCOMPLETE and says so; an
    incomplete manifest is never compared as unchanged."""
    rows: list[dict] = []
    omitted = 0
    for base, dirs, files in os.walk(root):
        dirs.sort()
        for name in sorted(dirs) + sorted(files):
            path = os.path.join(base, name)
            if len(rows) >= limit_entries:
                omitted += 1
                continue
            relative = os.path.relpath(path, root)
            try:
                if os.path.islink(path):
                    target = os.readlink(path).encode("utf-8", "replace")
                    rows.append({"path": relative, "kind": "symlink", "size": len(target),
                                 "sha256": hashlib.sha256(b"link:" + target).hexdigest()})
                elif os.path.isdir(path):
                    rows.append({"path": relative, "kind": "dir", "size": 0,
                                 "sha256": hashlib.sha256(b"dir").hexdigest()})
                else:
                    rows.append({"path": relative, "kind": "file",
                                 "size": os.lstat(path).st_size,
                                 "sha256": sha256_file(path)})
            except OSError as error:
                rows.append({"path": relative, "kind": "unreadable",
                             "error": f"{type(error).__name__}"})
    cumulative = hashlib.sha256()
    for row in rows:
        cumulative.update(
            (row["path"] + "\0" + row["kind"] + "\0"
             + str(row.get("sha256", "unreadable")) + "\n").encode()
        )
    return {"root": root, "entries": len(rows), "omitted": omitted,
            "complete": omitted == 0 and not any(row["kind"] == "unreadable" for row in rows),
            "digest": cumulative.hexdigest(), "rows": rows}


def write_manifest(path: str, manifest: dict) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path + ".tmp", "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, indent=1)
    os.replace(path + ".tmp", path)


def read_manifest(path: str) -> dict:
    with open(path, "r", encoding="utf-8") as handle:
        return json.load(handle)


def recheck_manifest(manifest: dict, root: str,
                     excluded_prefixes: tuple = MANIFEST_EXCLUDED_PREFIXES) -> dict:
    """Re-read a recorded manifest under `root` and decide input identity.

    A recorded file that changed, disappeared, became unreadable or changed kind
    is a changed input. So is ANY entry that appeared inside the tree, unless it
    is under a declared output-only prefix — an added Rust module, build input or
    configuration file can change resolution or compilation, so an added source
    entry is never tolerated as "extra output"."""
    recorded = {row["path"]: row for row in manifest.get("rows", []) if "path" in row}
    changed: list[str] = []
    missing: list[str] = []
    unreadable: list[str] = []
    for relative, row in sorted(recorded.items()):
        path = os.path.join(root, relative)
        try:
            if not os.path.lexists(path):
                missing.append(relative)
                continue
            kind = row.get("kind")
            if kind == "symlink":
                if not os.path.islink(path):
                    changed.append(relative)
                    continue
                target = os.readlink(path).encode("utf-8", "replace")
                digest = hashlib.sha256(b"link:" + target).hexdigest()
            elif kind == "dir":
                if os.path.islink(path) or not os.path.isdir(path):
                    changed.append(relative)
                    continue
                digest = hashlib.sha256(b"dir").hexdigest()
            else:
                if os.path.islink(path) or not os.path.isfile(path):
                    changed.append(relative)
                    continue
                digest = sha256_file(path)
        except OSError:
            unreadable.append(relative)
            continue
        if digest != row.get("sha256"):
            changed.append(relative)
    present: set = set()
    for base, dirs, files in os.walk(root):
        dirs.sort()
        for name in sorted(dirs) + sorted(files):
            present.add(os.path.relpath(os.path.join(base, name), root))
    needed_dirs = set()
    for relative in recorded:
        parent = os.path.dirname(relative)
        while parent and parent not in needed_dirs:
            needed_dirs.add(parent)
            parent = os.path.dirname(parent)
    def output_entry(relative: str) -> bool:
        return any(relative == prefix.rstrip("/") or relative.startswith(prefix)
                   for prefix in excluded_prefixes)
    added = sorted(present - set(recorded) - needed_dirs)
    added_outputs = [entry for entry in added if output_entry(entry)]
    added_inputs = [entry for entry in added if not output_entry(entry)]
    complete = bool(manifest.get("complete"))
    unchanged = (complete and not changed and not missing and not unreadable
                 and not added_inputs)
    return {"recorded": len(recorded), "present": len(present),
            "changed": changed[:16], "changed_count": len(changed),
            "missing": missing[:16], "missing_count": len(missing),
            "unreadable": unreadable[:16], "unreadable_count": len(unreadable),
            "added_inputs": added_inputs[:16], "added_input_count": len(added_inputs),
            "added_outputs": len(added_outputs),
            "manifest_complete": complete, "unchanged": unchanged}


class CompatRun:
    """One bound focused compatibility occurrence. Every effect happens under
    the guard's exported scratch, evidence and deadline, after the volume,
    apparatus and workload gates. The compatibility question is answered only
    by the declared probes; nothing here measures timing."""

    def __init__(self, mode: str, binding: dict) -> None:
        self.mode = mode
        self.binding = binding
        self.exports = guard_exports()
        self.deadline_ns = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
        self.env = scrubbed_env()
        self.cases: list[dict] = []
        self.figures: dict = {}
        self.disposals: list[dict] = []
        self.aborted: str | None = None
        self.children_spawned = 0
        self.children_closed = 0
        # One custody record per child this body started. Disposal is authorized
        # only when every record is positively closed; a counter is never proof.
        self.custody: list[dict] = []
        self.progress_records = 0
        self.progress_failures = 0
        # The last in-boundary closure classification (see `closure_check`).
        self.closure: dict | None = None
        self.seed_record: dict | None = None
        scratch = self.exports["OXIDE_TASK_SCRATCH_ROOT"]
        self.run_root = os.path.join(scratch, self.binding["paths"]["run_root_name"])
        self.packet_dir = os.path.join(
            self.exports["OXIDE_PROOF_EVIDENCE"], self.binding["paths"]["packet_name"]
        )

    # -- bounded progress evidence -----------------------------------------

    def progress_name(self) -> str:
        return f"progress-{self.binding['paths']['packet_name']}.jsonl"

    def progress(self, phase: str, **fields) -> None:
        """Append one bounded JSON line to the packet's progress log, outside the
        disposable scratch, so a later guard stop cannot erase an earlier real
        refusal or a landed phase. Publication is best effort: a failure is
        counted and disclosed, never allowed to change a case verdict."""
        if "phase" in fields:
            raise Refusal(
                "a progress field may not shadow the fixed `phase` key; name it "
                "explicitly (for example verification_stage)"
            )
        entry = {"phase": phase, "monotonic_s": round(time.monotonic(), 3), **fields}
        line = json.dumps(entry, separators=(",", ":")).encode()[:4096] + b"\n"
        try:
            os.makedirs(self.packet_dir, exist_ok=True)
            with open(os.path.join(self.packet_dir, self.progress_name()), "ab") as handle:
                handle.write(line)
                handle.flush()
                os.fsync(handle.fileno())
            self.progress_records += 1
        except OSError:
            self.progress_failures += 1

    # -- phases ------------------------------------------------------------

    def setup(self) -> None:
        os.makedirs(self.packet_dir, exist_ok=True)
        # The run-root absence gate runs before ANY effect below it.
        if os.path.exists(self.run_root):
            raise Refusal(f"the bound run root already exists: {self.run_root}")
        os.makedirs(self.run_root, exist_ok=False)
        # The private environment exists BEFORE the first runtime child, so
        # every gate and setup child runs with the private HOME/CARGO_HOME/
        # TMPDIR/TMP/TEMP and the pinned toolchain, not the invoking host's.
        self.private_home()
        mount, uuid = self.volume_gate()
        if uuid.upper() != self.binding["volume"]["uuid"].upper():
            raise Refusal(f"the selected volume is {uuid}, not the bound {self.binding['volume']['uuid']}")
        head, tree = self.source_gate()
        workload = self.verify_workload_runtime()
        self.figures["volume"] = {"mount": mount, "uuid": uuid}
        self.figures["source"] = {
            "apparatus": {"head": head, "tree": tree},
            "workload": {key: workload[key] for key in
                         ("commit", "tree", "lock_sha256", "lock_bytes", "dependencies")},
        }
        self.figures["fixtures"] = compat_fixture_digests()
        self.progress("gates", mount=mount, uuid=uuid, apparatus_head=head,
                      apparatus_tree=tree, workload_commit=workload["commit"],
                      workload_lock_sha256=workload["lock_sha256"])
        if COMPAT_SETUP_SEAM is not None:
            COMPAT_SETUP_SEAM(self)
            return
        if COMPAT_STAGE_SEAM is not None:
            COMPAT_STAGE_SEAM(self)
        else:
            self.acquire_upstream()
            self.build_relax()
        self.stage_hello()
        self.verify_engine("before disposal")
        self.dispose_stage_output(
            "stage-relax-target",
            "the fresh engine build exited, its own session was positively closed, and "
            "the copied executable plus the retained checkout's bundled std were verified",
        )
        self.verify_engine("after disposal")
        if COMPAT_SEED_SEAM is not None:
            COMPAT_SEED_SEAM(self)
        else:
            self.seed_cargo_home()
        self.verify_source_set()
        self.materialize()
        self.identity_baseline()

    def private_home(self) -> None:
        home = os.path.join(self.run_root, "home")
        for leaf in (".makepad-run", "tmp"):
            os.makedirs(os.path.join(home, leaf), exist_ok=True)
        os.chmod(home, 0o700)
        setup_home = os.path.join(self.run_root, "cargo-homes", "setup")
        os.makedirs(setup_home, exist_ok=True)
        self.env["HOME"] = home
        self.env["TMPDIR"] = os.path.join(home, "tmp")
        self.env["TMP"] = self.env["TMPDIR"]
        self.env["TEMP"] = self.env["TMPDIR"]
        self.env["CARGO_HOME"] = setup_home
        self.env["RUSTUP_TOOLCHAIN"] = TOOLCHAIN
        self.env["MAKEPAD_RUN_HOME"] = os.path.join(home, ".makepad-run")
        self.figures["home"] = home

    def child(self, argv: list[str], cwd: str, env: dict, cap_s: float,
              label: str | None = None) -> dict:
        """One fresh child through the bounded session-closing runner (or the
        named fake runner the effect-free controls inject), with its custody
        recorded. The returned record carries the capture evidence and the
        custody decision; a counter alone is never treated as a reap."""
        self.children_spawned += 1
        label = label or f"child-{self.children_spawned:02d}"
        if COMPAT_CHILD_RUNNER is not None:
            record = COMPAT_CHILD_RUNNER(argv, cwd, env, cap_s, label)
        else:
            record = compat_run_child(argv, cwd, env, cap_s, self.deadline_ns,
                                      os.path.join(self.run_root, "captures"), label)
        custody = dict(record.get("custody") or {})
        custody.update({"label": label, "argv": " ".join(str(part) for part in argv[:6])})
        self.custody.append(custody)
        if custody.get("session_closed"):
            self.children_closed += 1
        return record

    def closure_check(self, reason: str) -> dict:
        """Classify the guard's session INSIDE the qualified boundary: the only
        rows may be the guard and its descendants, our script, and this
        script's transient census helper. Our own subtree must be empty and no
        row may be unclassified. This is the closure gate for every disposal and
        for the compatibility result; it never creates a session of its own."""
        table = compat_process_table(child_env(self.env), self.run_root, self.deadline_ns)
        if table.get("readable"):
            result = classify_closure(table["rows"], os.getpid(), os.getppid(),
                                      os.getsid(0), {table.get("helper_pid")})
            result["error"] = None
        else:
            result = {"closed": False, "session_rows": None, "our_subtree": [],
                      "guard_rows": None, "unclassified": [], "unclassified_count": None,
                      "transient": [], "error": table.get("error")}
        result["reason"] = reason
        result["census_helper_pid"] = table.get("helper_pid")
        result["at_monotonic_s"] = round(time.monotonic(), 3)
        self.closure = result
        self.progress("closure", reason=reason, closed=result["closed"],
                      session_rows=result.get("session_rows"),
                      our_subtree=result.get("our_subtree"),
                      unclassified=result.get("unclassified"),
                      error=result.get("error"))
        return result

    def custody_clean(self) -> tuple[bool, str | None]:
        """(all closed, first unclean detail). Every child must be positively
        reaped, and the in-boundary closure classification must be positively
        closed. With no children started and no census taken there is nothing to
        close, so the ledger is vacuously clean."""
        for entry in self.custody:
            if entry.get("spawned") is False:
                continue
            if not entry.get("reaped"):
                return (False, (
                    f"{entry.get('label')}: reaped={entry.get('reaped')} "
                    f"wait_error={entry.get('wait_error')}"
                ))
        if not self.custody and self.closure is None:
            return (True, None)
        if not self.closure or not self.closure.get("closed"):
            return (False, f"closure not established: {self.closure}")
        return (True, None)

    def git_text(self, args: list[str], cwd: str | None = None,
                 cap_s: float = CAP_COMPAT_GIT_S, label: str | None = None) -> str:
        """One read-only git query through the same bounded, private, closed
        child boundary as every other setup child."""
        record = self.child(["git", *args], cwd or self.run_root,
                            child_env(self.env), cap_s, label=label)
        if record.get("exit") != 0:
            raise Refusal(
                f"git {' '.join(args)} failed (exit {record.get('exit')}): "
                f"{record.get('stderr', '')[:300]!r}"
            )
        return record.get("stdout", "")

    def volume_gate(self) -> tuple:
        """The selected-volume gate through the bounded child boundary (or the
        named fake volume boundary the effect-free controls inject)."""
        if COMPAT_VOLUME_SEAM is not None:
            return COMPAT_VOLUME_SEAM()
        import plistlib
        mount = mount_of(self.exports["OXIDE_TASK_SCRATCH_ROOT"])
        record = self.child(["diskutil", "info", "-plist", mount], self.run_root,
                            child_env(self.env), CAP_COMPAT_GIT_S, label="volume-info")
        if record.get("exit") != 0:
            raise Refusal(
                f"diskutil refused {mount} (exit {record.get('exit')}): "
                f"{(record.get('stderr') or '')[:200]!r}"
            )
        try:
            facts = plistlib.loads((record.get("stdout") or "").encode("utf-8"))
        except ValueError as error:
            raise Refusal(f"diskutil output for {mount} is unreadable: {error}")
        uuid = facts.get("VolumeUUID")
        if not isinstance(uuid, str) or not uuid:
            raise Refusal(f"diskutil names no VolumeUUID for {mount}")
        return (mount, uuid)

    def source_gate(self) -> tuple:
        """The apparatus source gate through the same bounded child boundary."""
        apparatus = self.binding["apparatus"]
        if SOURCE_GATE_SEAM is not None:
            return (apparatus["head"], apparatus["tree"])
        head = self.git_text(["rev-parse", "HEAD"], cwd=REPO,
                             label="apparatus-head").strip()
        tree = self.git_text(["rev-parse", "HEAD^{tree}"], cwd=REPO,
                             label="apparatus-tree").strip()
        status = self.git_text(["status", "--porcelain", "--untracked-files=no"],
                               cwd=REPO, label="apparatus-status")
        if not is_hex(head, 40) or not is_hex(tree, 40):
            raise Refusal("source gate: git did not answer exact 40-hex identities")
        if any(line.strip() for line in status.splitlines()):
            raise Refusal("source gate: the tracked worktree is dirty")
        if head != apparatus["head"] or tree != apparatus["tree"]:
            raise Refusal(
                f"source gate: HEAD/tree {head}/{tree} is not the bound "
                f"{apparatus['head']}/{apparatus['tree']}"
            )
        return (head, tree)

    def verify_workload_runtime(self) -> dict:
        """The workload pin re-verified at run time through the same bounded,
        private child boundary: commit, tree and lock bytes, and the three exact
        registry identities."""
        spec = self.binding["workload"]
        if (spec.get("commit") != WORKLOAD_COMMIT or spec.get("tree") != WORKLOAD_TREE
                or spec.get("lock_sha256") != WORKLOAD_LOCK_SHA256):
            raise Refusal("the activation does not bind the prepared workload commit, tree and lock")
        tree_now = self.git_text(["rev-parse", f"{WORKLOAD_COMMIT}^{{tree}}"], cwd=REPO,
                                 label="workload-tree").strip()
        if tree_now != WORKLOAD_TREE:
            raise Refusal(
                f"workload commit {WORKLOAD_COMMIT} has tree {tree_now!r}, not {WORKLOAD_TREE}"
            )
        lock_text = self.git_text(["show", f"{WORKLOAD_COMMIT}:Cargo.lock"], cwd=REPO,
                                  label="workload-lock")
        lock_bytes = lock_text.encode("utf-8")
        digest = hashlib.sha256(lock_bytes).hexdigest()
        if digest != WORKLOAD_LOCK_SHA256:
            raise Refusal(f"the workload lock is {digest}, not the prepared {WORKLOAD_LOCK_SHA256}")
        dependencies = pinned_dependency_identities(lock_text)
        self.workload_lock_text = lock_text
        return {"commit": WORKLOAD_COMMIT, "tree": WORKLOAD_TREE, "lock_sha256": digest,
                "lock_bytes": len(lock_bytes), "dependencies": dependencies}

    def acquire_upstream(self) -> None:
        dest = os.path.join(self.run_root, "relax-src")
        clone = ["git", "clone", "--progress", COMPAT_UPSTREAM_URL, dest]
        started = time.monotonic()
        record = self.child(clone, self.run_root, child_env(self.env), CAP_CLONE_S)
        if record["exit"] != 0:
            raise Refusal(f"upstream clone failed: {record['stderr'][:300]!r}")
        network_bytes = None
        for match in re.finditer(
            r"Receiving objects:\s+\d+% \(\d+/\d+\), ([0-9.]+) (KiB|MiB|GiB)",
            record["stderr"],
        ):
            value, unit = float(match.group(1)), match.group(2)
            network_bytes = int(value * {"KiB": 1024, "MiB": 1024 ** 2, "GiB": 1024 ** 3}[unit])
        checkout = self.child(
            ["git", "checkout", COMPAT_UPSTREAM_COMMIT], dest, child_env(self.env), 30
        )
        if checkout["exit"] != 0:
            raise Refusal(f"upstream checkout failed: {checkout['stderr'][:300]!r}")
        tree_now = self.git_text(["rev-parse", "HEAD^{tree}"], cwd=dest,
                                 label="upstream-tree").strip()
        if tree_now != COMPAT_UPSTREAM_TREE:
            raise Refusal(
                f"upstream tree is {tree_now!r}, not the bound {COMPAT_UPSTREAM_TREE}"
            )
        fixes: dict = {}
        for commit, label in COMPAT_UPSTREAM_FIXES.items():
            present_record = self.child(
                ["git", "cat-file", "-e", f"{commit}^{{commit}}"], dest,
                child_env(self.env), CAP_COMPAT_GIT_S, label=f"fix-present-{commit[:8]}",
            )
            ancestor_record = self.child(
                ["git", "merge-base", "--is-ancestor", commit, "HEAD"], dest,
                child_env(self.env), CAP_COMPAT_GIT_S, label=f"fix-ancestor-{commit[:8]}",
            )
            present = present_record["exit"] == 0
            ancestor = ancestor_record["exit"] == 0
            fixes[commit] = {"label": label, "present": present, "ancestor": ancestor}
            if not (present and ancestor):
                raise Refusal(f"the upstream checkout does not contain fix commit {commit}")
        self.relax_src = dest
        self.relax_std_dir = os.path.join(dest, "libs", "relax", "compiler", "std")
        self.figures["acquisition"] = {
            "commit": COMPAT_UPSTREAM_COMMIT,
            "tree": COMPAT_UPSTREAM_TREE,
            "network_seconds": round(time.monotonic() - started, 3),
            "network_bytes": network_bytes,
            "network_bytes_source": (
                "parsed from the forced git clone progress output" if network_bytes is not None
                else "unparsed; occupied bytes below bound the checkout instead"
            ),
            "occupied": walk_bytes(dest),
            "fix_commits": fixes,
        }
        self.progress("upstream", commit=COMPAT_UPSTREAM_COMMIT,
                      tree=COMPAT_UPSTREAM_TREE, network_bytes=network_bytes,
                      fixes={sha: facts["ancestor"] for sha, facts in fixes.items()})

    def build_relax(self) -> None:
        cargo_home = os.path.join(self.run_root, "cargo-homes", "relax-build")
        os.makedirs(cargo_home)
        env = child_env(
            self.env,
            CARGO_HOME=cargo_home,
            RUSTUP_TOOLCHAIN=TOOLCHAIN,
            CARGO_TARGET_DIR=os.path.join(self.run_root, "stage-relax-target"),
        )
        lock_record = self.child(engine_lock_command(), self.relax_src, env, CAP_LOCK_S)
        if lock_record["exit"] != 0:
            raise Refusal(
                "the upstream Relax workspace lock could not be established offline "
                f"(exit {lock_record['exit']}): {lock_record['stderr'][:400]!r}"
            )
        record = self.child(engine_build_command(), self.relax_src, env, CAP_ENGINE_BUILD_S)
        if record["exit"] != 0:
            raise Refusal(
                "the pinned Relax engine did not build with the pinned toolchain "
                f"(exit {record['exit']}); this is a compatibility blocker: {record['stderr'][:400]!r}"
            )
        binary_dir = os.path.join(self.run_root, "bin")
        os.makedirs(binary_dir, exist_ok=True)
        staged = os.path.join(binary_dir, "relax")
        shutil.copy2(os.path.join(self.run_root, "stage-relax-target", "release", "relax"), staged)
        self.relax_bin = staged
        self.figures["engine"] = {
            "sha256": sha256_file(staged),
            "build_seconds": record["seconds"],
            "build_command": record["argv"],
            "lock_establishment": {
                "command": lock_record["argv"],
                "seconds": lock_record["seconds"],
                "offline": True,
                "note": "explicit offline generate-lockfile inside the guarded allowance",
            },
            "build_target": os.path.join(self.run_root, "stage-relax-target"),
            "retained_checkout": self.relax_src,
            "bundled_std": self.relax_std_dir,
            "runtime_requirement": (
                "the copied executable reads Relax's std from the retained checkout at the "
                "build-time path recorded here; the checkout is never disposed"
            ),
            "fix_commits": self.figures.get("acquisition", {}).get("fix_commits"),
        }
        self.progress("engine_built", sha256=self.figures["engine"]["sha256"],
                      build_seconds=record["seconds"])

    def stage_hello(self) -> None:
        dest = os.path.join(self.run_root, "workloads", "compat-hello", "relax", "compat-hello")
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        shutil.copytree(fixture_path("compat-hello"), dest)
        self.hello_dir = dest

    def verify_env(self, phase: str) -> dict:
        """The engine-verification environment: the std-only package needs no
        seeded registry sources, and the shared home does not exist yet. Its run
        dir is the pinned runner's own cache override, outside the tree."""
        home = os.path.join(self.run_root, "cargo-homes", "verify")
        os.makedirs(home, exist_ok=True)
        cache = os.path.join(self.run_root, "relax-caches",
                             f"verify-{phase.split()[0]}")
        os.makedirs(cache, exist_ok=True)
        return child_env(
            self.env,
            MAKEPAD_RUN_OFFLINE="1",
            RUSTUP_TOOLCHAIN=TOOLCHAIN,
            CARGO_HOME=home,
            **{RELAX_CACHE_ENV: cache},
        )

    def verify_engine(self, phase: str) -> None:
        """Verify the COPIED executable plus the retained checkout's bundled std
        with real children: --version, a check of the std-only package and a run
        with its exact declared stdout. Runs before and (cheaply) after the
        disposable build target is removed."""
        if not os.path.isdir(self.relax_std_dir):
            raise Refusal(f"the pinned checkout's bundled std is absent: {self.relax_std_dir}")
        binary_dir = os.path.dirname(self.relax_bin)
        version = self.child([self.relax_bin, "--version"], binary_dir, child_env(self.env),
                             CAP_COMPAT_VERIFY_S, label=f"verify-version-{phase.split()[0]}")
        if version["exit"] != 0:
            raise Refusal(
                f"the copied relax executable refused --version ({phase}, exit {version['exit']})"
            )
        check = self.child([self.relax_bin, "check", *RELAX_CHECK_FLAGS, "compat-hello"],
                           self.hello_dir, self.verify_env(phase), CAP_COMPAT_VERIFY_S,
                           label=f"verify-check-{phase.split()[0]}")
        if check["exit"] != 0:
            raise Refusal(
                f"the copied relax executable cannot check the std-only package ({phase}): "
                f"{check['stderr'][:300]!r}"
            )
        run = self.child(
            [self.relax_bin, "run", *RELAX_STRICT_FLAGS, "compat-hello"],
            self.hello_dir, self.verify_env(phase), CAP_COMPAT_VERIFY_S,
            label=f"verify-run-{phase.split()[0]}",
        )
        verdict = oracle_exact_stdout(run, COMPAT_HELLO_STDOUT)
        if verdict != "match":
            raise Refusal(
                f"the copied relax executable cannot run the std-only package ({phase}): "
                f"{verdict}, exit {run['exit']}, stdout {run['stdout'][:120]!r}"
            )
        self.figures.setdefault("engine_verification", {})[phase] = {
            "version": version["stdout"].strip()[:200],
            "check_exit": check["exit"],
            "check_seconds": check["seconds"],
            "run_verdict": verdict,
            "run_seconds": run["seconds"],
            "std_dir": self.relax_std_dir,
            "std_dir_present": os.path.isdir(self.relax_std_dir),
        }
        self.progress("engine_verified", verification_stage=phase,
                      check_exit=check["exit"], run_verdict=verdict,
                      std_dir_present=os.path.isdir(self.relax_std_dir))

    def dispose_stage_output(self, relative: str, reason: str) -> None:
        """Remove ONE declared stage output inside this run's guard root.

        Only an exact allowlist entry, never a symlink, never a path that
        resolves outside the owned root, and only when EVERY child this body
        started is positively closed: its own PID reaped and its own session
        census readable and empty. A counter, a `finally` or an exited direct
        child is never accepted as closure. The bytes removed are recorded, and
        the checkout, bundled std, seeded Cargo homes, private home, staged
        binaries and workload sources are not on the list at all."""
        if relative not in COMPAT_DISPOSABLE_PATHS:
            raise Refusal(f"refusing to dispose undeclared stage output {relative!r}")
        self.closure_check(f"disposal {relative}")
        clean, detail = self.custody_clean()
        if not clean:
            raise Refusal(
                "refusing to dispose while a child's custody is unknown: " + str(detail)
            )
        full = os.path.join(self.run_root, relative)
        root_real = os.path.realpath(self.run_root)
        full_real = os.path.realpath(full)
        if full_real == root_real or not full_real.startswith(root_real + os.sep):
            raise Refusal(f"the disposal target {relative!r} escapes the owned run root")
        entry = {
            "path": relative,
            "reason": reason,
            "children_spawned": self.children_spawned,
            "custody_records": len(self.custody),
            "closure": {key: self.closure.get(key) for key in
                        ("closed", "reason", "session_rows", "our_subtree",
                         "unclassified", "error")} if self.closure else None,
        }
        if not os.path.exists(full):
            entry["absent"] = True
        else:
            if os.path.islink(full):
                raise Refusal(f"refusing to dispose the symlink {relative!r}")
            entry["bytes_before"] = walk_bytes(full)
            shutil.rmtree(full)
            entry["after"] = {"absent": not os.path.exists(full)}
            if os.path.exists(full):
                raise Refusal(f"the disposal of {relative!r} left bytes behind")
        self.disposals.append(entry)
        self.progress("disposed", path=relative, absent=entry.get("absent", False),
                      removed_bytes=(entry.get("bytes_before") or {}).get("bytes_apparent"))

    def seed_cargo_home(self) -> None:
        import offline_seed

        lock_text = self.git_text(["show", f"{WORKLOAD_COMMIT}:Cargo.lock"],
                                  cwd=REPO, label="workload-lock")
        lock_bytes = lock_text.encode("utf-8")
        digest = hashlib.sha256(lock_bytes).hexdigest()
        if digest != WORKLOAD_LOCK_SHA256:
            raise Refusal(f"the workload lock is {digest}, not the prepared {WORKLOAD_LOCK_SHA256}")
        host_home = os.environ.get("CARGO_HOME") or os.path.join(
            os.environ.get("HOME", os.path.expanduser("~")), ".cargo"
        )
        source = canonical_cache_source(host_home)
        limits = {
            "anchor": "no-follow per component below the canonical common root",
            "aggregate_bytes": offline_seed.LIMITS.aggregate_bytes,
            "work_seconds": offline_seed.LIMITS.work_seconds,
            "git_candidates": offline_seed.LIMITS.git_candidates,
            "note": "private config, checksum-verified content; host cache read-only; the "
                    "seed source is the WORKLOAD lock, never this apparatus head's lock",
        }
        self.figures["seed_source"] = {**source, "limits": limits}
        destination = os.path.join(self.run_root, "cargo-homes", "shared")
        os.makedirs(destination)
        started = time.monotonic()
        try:
            seed = offline_seed.seed_cargo_home(
                source["common_root"],
                destination,
                lock_bytes,
                deadline_ns=self.deadline_ns - int(COMPAT_STOP_MARGIN_S * 1e9),
            )
        except offline_seed.SeedRefusal as error:
            raise Refusal(f"seed: {error}") from error
        self.shared_cargo_home = destination
        self.seed_record = seed
        self.figures["seed"] = {
            "source": source,
            "destination": destination,
            "lock_sha256": digest,
            "aggregate_bytes": seed.get("aggregate_bytes"),
            "seconds": round(time.monotonic() - started, 3),
            "network_seconds": 0.0,
            "limits": limits,
            "note": "offline seed from the normalized invoking Cargo home; no download",
        }
        self.progress("seeded", aggregate_bytes=seed.get("aggregate_bytes"),
                      seconds=self.figures["seed"]["seconds"])

    def verify_source_set(self) -> dict:
        """Fail closed unless the private Cargo home offers EXACTLY the workload
        lock's sources.

        `--frozen` makes the pinned resolver offline and lock-preferring; it is
        NOT Cargo's fail-on-drift. Graph equality therefore rests on the source
        set: if the only registry crates and git databases available offline are
        the lock's own, any resolution the runner performs is covered by the
        declared lock. Every seeded registry crate is re-hashed against the
        lock's checksum, every seeded git source must be one of the lock's
        repositories at its locked commit, and nothing else may be present."""
        seed = self.seed_record
        if not isinstance(seed, dict):
            raise Refusal("no seed record is available to verify the private source set")
        sources = lock_sources(self.workload_lock_text)
        registry = {(package["name"], package["version"]): package["checksum"]
                    for package in sources["registry"]}
        git = {(package["url"], package["commit"]): package
               for package in sources["git"]}
        crates = {}
        index_entries = 0
        for entry in seed.get("files", []):
            path = entry.get("path", "")
            if path.startswith("registry/cache/"):
                name = os.path.basename(path)
                if not name.endswith(".crate"):
                    raise Refusal(f"the seed holds an unexpected registry file {path!r}")
                key = name[:-len(".crate")]
                crates[key] = entry.get("sha256")
            elif path.startswith("registry/index/"):
                index_entries += 1
        expected_crates = {f"{name}-{version}": checksum
                           for (name, version), checksum in registry.items()}
        if set(crates) != set(expected_crates):
            raise Refusal(
                "the private registry source set is not the workload lock: "
                f"extra {sorted(set(crates) - set(expected_crates))[:4]}, "
                f"missing {sorted(set(expected_crates) - set(crates))[:4]}"
            )
        for name, checksum in crates.items():
            if checksum != expected_crates[name]:
                raise Refusal(f"the private crate {name} does not match the workload lock's checksum")
        if index_entries != len(registry) + 1:
            raise Refusal(
                "the private registry index does not hold exactly the lock's packages "
                f"(+ config.json): {index_entries} entries for {len(registry)} packages"
            )
        seen_git = {}
        for source in seed.get("git_sources", []):
            key = (source.get("repository"), source.get("rev"))
            if key not in git:
                raise Refusal(
                    f"the private git source {key} is not in the workload lock"
                )
            if not any(candidate.get("commit_present") for candidate in source.get("candidates", [])):
                raise Refusal(f"the private git source {key} does not carry the locked commit")
            seen_git[key] = len(source.get("candidates", []))
        missing_git = sorted(set(git) - set(seen_git))
        if missing_git:
            raise Refusal(f"the private source set is missing locked git sources {missing_git[:3]}")
        facts = {
            "lock_sha256": sources["lock_sha256"],
            "registry_packages": len(registry),
            "registry_crates": len(crates),
            "index_entries": index_entries,
            "git_sources": {f"{url}#{commit[:12]}": candidates
                            for (url, commit), candidates in seen_git.items()},
            "note": (
                "--frozen is offline + lock-preferring, not Cargo's fail-on-drift; the "
                "verified private source set is what bounds the resolved graph to the lock"
            ),
        }
        self.figures["source_set"] = facts
        self.progress("source_set", registry_packages=facts["registry_packages"],
                      index_entries=index_entries, git_sources=len(seen_git))
        return facts

    def materialize(self) -> None:
        self.works = {}
        for fixture in ("compat-fmt", "compat-crc32"):
            self.works[fixture] = {}
            for arm in COMPAT_ARMS:
                dest = os.path.join(self.run_root, "workloads", fixture, arm, fixture)
                os.makedirs(os.path.dirname(dest), exist_ok=True)
                shutil.copytree(fixture_path(fixture), dest)
                self.works[fixture][arm] = dest
        identities: dict = {}
        for fixture, names in (("compat-fmt", ("serde_json", "zmij")),
                               ("compat-crc32", ("crc32fast",))):
            identities[fixture] = {}
            for arm in COMPAT_ARMS:
                workdir = self.works[fixture][arm]
                env = child_env(self.env, CARGO_HOME=self.shared_cargo_home,
                                RUSTUP_TOOLCHAIN=TOOLCHAIN)
                record = self.child(["cargo", "generate-lockfile", "--offline"], workdir, env,
                                    CAP_LOCK_S)
                if record["exit"] != 0:
                    raise Refusal(
                        f"offline lock establishment failed for {fixture}/{arm}: "
                        f"{record['stderr'][:300]!r}"
                    )
                with open(os.path.join(workdir, "Cargo.lock"), encoding="utf-8") as handle:
                    lock_text = handle.read()
                found = lock_registry_identities(lock_text)
                for name in names:
                    version, checksum = PINNED_WORKLOAD_DEPS[name]
                    current = found.get(name)
                    if current is None or current["version"] != version or current["checksum"] != checksum:
                        raise Refusal(
                            f"{fixture}/{arm} resolved {name} to {current}, not the workload pin "
                            f"{version} ({checksum[:12]}...)"
                        )
                    identities[fixture].setdefault(name, {})[arm] = dict(current)
        self.figures["fixture_locks"] = identities
        # The negative control: a scratch copy of compat-crc32 whose manifest
        # requires a version the verified source set does not contain and whose
        # lock omits that package entirely, so the pinned runner's resolution is
        # exercised against a requirement it cannot satisfy from the sources.
        control_root = os.path.join(self.run_root, "workloads", "source-control", "relax")
        control_dest = os.path.join(control_root, "compat-crc32")
        os.makedirs(control_root, exist_ok=True)
        shutil.copytree(self.works["compat-crc32"]["relax"], control_dest)
        control_manifest = os.path.join(control_dest, "Cargo.toml")
        with open(control_manifest, encoding="utf-8") as handle:
            text = handle.read()
        edited = apply_edit(text, 'crc32fast = "=1.5.3"', COMPAT_SOURCE_CONTROL_REQUIREMENT, 1)
        with open(control_manifest, "w", encoding="utf-8") as handle:
            handle.write(edited)
        control_lock = os.path.join(control_dest, "Cargo.lock")
        with open(control_lock, encoding="utf-8") as handle:
            lock_text = handle.read()
        blocks = lock_text.split("[[package]]")
        kept = [blocks[0]] + [block for block in blocks[1:]
                              if 'name = "crc32fast"' not in block]
        if len(kept) == len(blocks):
            raise Refusal("the source control's lock has no crc32fast entry to remove")
        with open(control_lock, "w", encoding="utf-8") as handle:
            handle.write("[[package]]".join(kept))
        self.works["source-control"] = {"relax": control_dest}
        self.figures["source_control"] = {
            "path": control_dest,
            "requirement": COMPAT_SOURCE_CONTROL_REQUIREMENT,
            "lock_omits": "crc32fast",
            "premise": (
                "the requirement is absent from the verified private source set, so the "
                "pinned runner must fail; a zero exit means resolution was not source-"
                "constrained"
            ),
        }
        self.works["snapshot"] = {}
        staging = os.path.join(self.run_root, "snapshot-archive")
        os.makedirs(staging, exist_ok=True)
        for arm in COMPAT_ARMS:
            dest = os.path.join(self.run_root, "workloads", "snapshot", arm, "tree")
            os.makedirs(dest, exist_ok=True)
            # Both materialization children go through the same bounded, private,
            # deadline-clipped and session-closing boundary as every other child;
            # there is no fixed standalone timeout and no inherited host env.
            archive_path = os.path.join(staging, f"snapshot-{arm}.tar")
            archive = self.child(
                ["git", "archive", "--format=tar", "-o", archive_path, WORKLOAD_COMMIT],
                REPO, child_env(self.env), CAP_COMPAT_ARCHIVE_S,
                label=f"snapshot-archive-{arm}",
            )
            if archive["exit"] != 0:
                raise Refusal(
                    f"git archive of the workload commit failed for {arm} "
                    f"(exit {archive['exit']}): {archive['stderr'][:300]!r}"
                )
            untar = self.child(
                ["tar", "-x", "-f", archive_path, "-C", dest],
                staging, child_env(self.env), CAP_COMPAT_ARCHIVE_S,
                label=f"snapshot-extract-{arm}",
            )
            if untar["exit"] != 0:
                raise Refusal(
                    f"materializing the workload snapshot failed for {arm} "
                    f"(exit {untar['exit']}): {untar['stderr'][:300]!r}"
                )
            try:
                os.remove(archive_path)
            except OSError:
                pass
            lock_digest = sha256_file(os.path.join(dest, "Cargo.lock"))
            if lock_digest != WORKLOAD_LOCK_SHA256:
                raise Refusal(
                    f"the materialized snapshot lock is {lock_digest}, not the workload pin"
                )
            self.works["snapshot"][arm] = dest
        self.figures["workloads"] = {
            key: {arm: path for arm, path in arms.items()} for key, arms in self.works.items()
        }
        self.progress("workloads", fixture_locks=self.figures.get("fixture_locks"),
                      snapshot_lock_sha256=WORKLOAD_LOCK_SHA256)

    # -- arms and probes ---------------------------------------------------

    def relax_env(self, cache_label: str) -> dict:
        """One Relax arm environment: offline, the one seeded private Cargo
        home, the pinned toolchain, and the runner's own cache override pointing
        OUTSIDE every materialized tree, so a probe cannot add files to a source
        tree it is being compared against."""
        cache = os.path.join(self.run_root, "relax-caches", cache_label)
        os.makedirs(cache, exist_ok=True)
        return child_env(
            self.env,
            MAKEPAD_RUN_OFFLINE="1",
            CARGO_HOME=self.shared_cargo_home,
            RUSTUP_TOOLCHAIN=TOOLCHAIN,
            **{RELAX_CACHE_ENV: cache},
        )

    def arm_argv_env(self, probe: str, arm: str) -> tuple[list[str], dict, str]:
        if probe == "snapshot":
            root = os.path.join(self.run_root, "workloads", "snapshot", arm)
            workdir = os.path.join(root, "tree")
            if arm == "cargo":
                env = child_env(
                    quick_recipe_env(self.env),
                    CARGO_HOME=self.shared_cargo_home,
                    RUSTUP_TOOLCHAIN=TOOLCHAIN,
                    CARGO_TARGET_DIR=os.path.join(root, "target"),
                )
                return (["cargo", "run", "--locked", "--offline", "--quiet", "--bin", "oxide",
                         "--", "--help"], env, workdir)
            return ([self.relax_bin, "run", *RELAX_STRICT_FLAGS,
                     "oxide-storage", "--bin", "oxide", "--", "--help"],
                    self.relax_env(f"{probe}-{arm}"), workdir)
        fixture = "compat-fmt" if probe == "format" else "compat-crc32"
        root = os.path.join(self.run_root, "workloads", fixture, arm)
        workdir = os.path.join(root, fixture)
        if arm == "cargo":
            env = child_env(
                quick_recipe_env(self.env),
                CARGO_HOME=self.shared_cargo_home,
                RUSTUP_TOOLCHAIN=TOOLCHAIN,
                CARGO_TARGET_DIR=os.path.join(root, "target"),
            )
            return (["cargo", "run", "--locked", "--offline", "--quiet", "--bin", fixture],
                    env, workdir)
        return ([self.relax_bin, "run", *RELAX_STRICT_FLAGS, fixture],
                self.relax_env(f"{probe}-{arm}"), workdir)

    def retain_capture(self, probe: str, arm: str, stream: str, record: dict) -> dict:
        """Write ONE bounded RAW prefix of a per-arm capture into the evidence
        packet BEFORE the next child runs, so a later stop or budget exhaustion
        cannot erase this arm's actual diagnostics. The row keeps the exact
        full-body SHA-256, byte count, completeness and strict-decode facts; the
        retained file holds the raw prefix bytes and its own hash. A retention
        failure aborts rather than losing evidence."""
        name = f"case-{probe}-{arm}.{stream}.txt"
        path = os.path.join(self.packet_dir, name)
        body = record.get(f"{stream}_prefix") or b""
        try:
            os.makedirs(self.packet_dir, exist_ok=True)
            with open(path + ".tmp", "wb") as handle:
                handle.write(body)
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(path + ".tmp", path)
        except OSError as error:
            raise Refusal(
                f"the {probe}/{arm} {stream} evidence could not be retained: {error}"
            )
        total = record.get(f"{stream}_bytes")
        return {
            "file": name,
            "retained_bytes": len(body),
            "retained_sha256": sha256_file(path),
            "body_sha256": record.get(f"{stream}_sha256"),
            "body_bytes": total,
            "body_complete": bool(record.get(f"{stream}_complete")),
            "body_decode_error": record.get(f"{stream}_decode_error"),
            "omitted_bytes": max(0, int(total or 0) - len(body)),
            "truncated": bool(total is not None and total > len(body)),
        }

    def record(self, probe: str, arm: str, record: dict, verdict: str, facts: dict,
               baseline_record: dict | None = None) -> None:
        captures = {
            stream: self.retain_capture(probe, arm, stream, record)
            for stream in ("stdout", "stderr")
        }
        row = {
            "case": probe,
            "arm": arm,
            "exit": record.get("exit"),
            "timed_out": record.get("timed_out"),
            "seconds": record.get("seconds"),
            "verdict": verdict,
            "stdout_sha256": record.get("stdout_sha256"),
            "stdout_bytes": record.get("stdout_bytes"),
            "stdout_complete": record.get("stdout_complete"),
            "stdout_readable": record.get("stdout_readable"),
            "stdout_decode_error": record.get("stdout_decode_error"),
            "stderr_sha256": record.get("stderr_sha256"),
            "stderr_bytes": record.get("stderr_bytes"),
            "stderr_complete": record.get("stderr_complete"),
            "stderr_readable": record.get("stderr_readable"),
            "stderr_decode_error": record.get("stderr_decode_error"),
            "custody": record.get("custody"),
            "captures": captures,
            "facts": facts,
            "stdout_evidence": record.get("stdout_evidence"),
            "stderr_evidence": record.get("stderr_evidence"),
        }
        if baseline_record is not None:
            row["baseline_stdout_sha256"] = baseline_record.get("stdout_sha256")
            row["baseline_stdout_bytes"] = baseline_record.get("stdout_bytes")
            row["stdout_equal_to_baseline"] = (
                baseline_record.get("stdout_sha256") == record.get("stdout_sha256")
                and baseline_record.get("stdout_bytes") == record.get("stdout_bytes")
            )
        self.cases.append(row)
        stdout_prefix = (row.get("stdout_evidence") or {}).get("prefix", "")[:384]
        stderr_prefix = (row.get("stderr_evidence") or {}).get("prefix", "")[:384]
        custody = row.get("custody") or {}
        self.progress(
            "case", case=probe, arm=arm, verdict=verdict, exit=row["exit"],
            timed_out=row["timed_out"], seconds=row["seconds"],
            stdout_sha256=row["stdout_sha256"], stdout_bytes=row["stdout_bytes"],
            stdout_complete=row["stdout_complete"],
            stderr_sha256=row["stderr_sha256"], stderr_bytes=row["stderr_bytes"],
            stderr_complete=row["stderr_complete"],
            captures=captures, reaped=custody.get("reaped"),
            stdout_prefix=stdout_prefix, stderr_prefix=stderr_prefix,
        )

    def baseline(self, probe: str, cap_s: float, oracle) -> tuple[dict | None, str, dict]:
        """Run the plain-Cargo baseline and require it to satisfy the STRUCTURAL
        oracle: a baseline failure invalidates the probe's expected observation,
        so it aborts the run instead of being reported as an incompatibility."""
        argv, env, cwd = self.arm_argv_env(probe, "cargo")
        record = self.child(argv, cwd, env, cap_s, label=f"arm-{probe}-cargo")
        verdict, facts = oracle(record)
        self.record(probe, "cargo", record, verdict, facts)
        return (record, verdict, facts)

    def arm(self, probe: str, cap_s: float, oracle, baseline_record: dict) -> None:
        argv, env, cwd = self.arm_argv_env(probe, "relax")
        record = self.child(argv, cwd, env, cap_s, label=f"arm-{probe}-relax")
        verdict, facts = oracle(record)
        verdict = compat_correspondence(verdict, baseline_record, record)
        self.record(probe, "relax", record, verdict, facts, baseline_record=baseline_record)

    def dispose_probe(self, probe: str) -> None:
        if probe == "snapshot":
            self.dispose_stage_output(
                "workloads/snapshot/cargo/target",
                "the snapshot baseline arm completed; the Relax arm does not read it",
            )
            return
        fixture = "compat-fmt" if probe == "format" else "compat-crc32"
        for relative in (f"workloads/{fixture}/cargo/target",
                         f"workloads/{fixture}/relax/{fixture}/target"):
            self.dispose_stage_output(
                relative, f"the {probe} probe completed for both arms; no later case reads it",
            )

    def case_format(self) -> None:
        baseline, verdict, facts = self.baseline("format", CAP_COMPAT_FMT_S, compat_fmt_oracle)
        if verdict != "match":
            self.aborted = (
                "format: the plain-Cargo baseline did not satisfy the structural float "
                f"oracle ({facts})"
            )
            return
        self.arm("format", CAP_COMPAT_FMT_S, compat_fmt_oracle, baseline)
        self.dispose_probe("format")

    def case_crc32(self) -> None:
        baseline, verdict, facts = self.baseline("crc32", CAP_COMPAT_CRC_S, compat_crc32_oracle)
        if verdict != "match":
            self.aborted = (
                "crc32: the plain-Cargo baseline did not satisfy the structural CRC32 "
                f"oracle ({facts})"
            )
            return
        self.arm("crc32", CAP_COMPAT_CRC_S, compat_crc32_oracle, baseline)
        self.dispose_probe("crc32")

    def case_source_constrained(self) -> None:
        """The negative resolution control: an unsatisfiable requirement must
        fail on the pinned runner. This is the check that the private source set
        bounds the resolved graph, so `--frozen` alone is never claimed to be
        Cargo's fail-on-drift."""
        workdir = (self.works.get("source-control") or {}).get("relax")
        if not workdir:
            self.aborted = "source-constrained: the control fixture was not materialized"
            return
        record = self.child(
            [self.relax_bin, "run", *RELAX_STRICT_FLAGS, "compat-crc32"],
            workdir, self.relax_env("source-constrained"), CAP_COMPAT_CRC_S,
            label="source-constrained",
        )
        verdict, facts = compat_source_control_oracle(record)
        self.record("source-constrained", "relax", record, verdict, facts)

    def case_snapshot(self) -> None:
        baseline, verdict, facts = self.baseline("snapshot", CAP_COMPAT_SNAPSHOT_S,
                                                 compat_snapshot_oracle)
        if verdict != "match":
            self.aborted = (
                "snapshot: the plain-Cargo baseline did not print the expected help output "
                f"({facts})"
            )
            return
        self.dispose_probe("snapshot")
        self.arm("snapshot", CAP_COMPAT_SNAPSHOT_S, compat_snapshot_oracle, baseline)

    def identity_baseline(self) -> None:
        """Record the identities of the ACTUAL materialized inputs, before any
        probe: every fixture source tree and lock, both snapshot source trees and
        locks, the staged executable and the retained checkout's bundled std.
        Closeout re-reads exactly these; the Git commit alone is never treated as
        proof of what ran."""
        identities: dict = {"fixtures": {}, "snapshot": {}, "engine": {}}
        store = os.path.join(self.run_root, "identities")
        os.makedirs(store, exist_ok=True)
        for fixture in ("compat-fmt", "compat-crc32"):
            identities["fixtures"][fixture] = {}
            for arm in COMPAT_ARMS:
                workdir = self.works[fixture][arm]
                manifest = tree_manifest(workdir)
                path = os.path.join(store, f"{fixture}-{arm}.json")
                write_manifest(path, manifest)
                with open(os.path.join(workdir, "Cargo.lock"), encoding="utf-8") as handle:
                    lock_text = handle.read()
                identities["fixtures"][fixture][arm] = {
                    "manifest_file": path,
                    "entries": manifest["entries"],
                    "omitted": manifest["omitted"],
                    "complete": manifest["complete"],
                    "digest": manifest["digest"],
                    "lock_sha256": hashlib.sha256(lock_text.encode()).hexdigest(),
                    "lock_identities": lock_registry_identities(lock_text),
                }
        for arm in COMPAT_ARMS:
            tree = self.works["snapshot"][arm]
            manifest = tree_manifest(tree)
            path = os.path.join(store, f"snapshot-{arm}.json")
            write_manifest(path, manifest)
            identities["snapshot"][arm] = {
                "manifest_file": path,
                "entries": manifest["entries"],
                "omitted": manifest["omitted"],
                "complete": manifest["complete"],
                "digest": manifest["digest"],
                "lock_sha256": sha256_file(os.path.join(tree, "Cargo.lock")),
            }
        control = (self.works.get("source-control") or {}).get("relax")
        if control:
            control_manifest = tree_manifest(control)
            control_path = os.path.join(store, "source-control.json")
            write_manifest(control_path, control_manifest)
            identities["source_control"] = {
                "manifest_file": control_path,
                "entries": control_manifest["entries"],
                "omitted": control_manifest["omitted"],
                "complete": control_manifest["complete"],
                "digest": control_manifest["digest"],
            }
        std_manifest = tree_manifest(self.relax_std_dir)
        std_path = os.path.join(store, "bundled-std.json")
        write_manifest(std_path, std_manifest)
        # The checkout's own state AFTER the lock we established offline is the
        # baseline: the upstream workspace ships no lock, so `Cargo.lock` is a
        # known untracked file. Closeout requires exactly this state back.
        checkout_head = self.git_text(["rev-parse", "HEAD"], cwd=self.relax_src,
                                      label="baseline-checkout-head").strip()
        checkout_status = sorted(
            line for line in self.git_text(
                ["status", "--porcelain", "--untracked-files=normal"],
                cwd=self.relax_src, label="baseline-checkout-status",
            ).splitlines() if line.strip()
        )
        identities["engine"] = {
            "staged_sha256": sha256_file(self.relax_bin),
            "std_manifest_file": std_path,
            "std_entries": std_manifest["entries"],
            "std_omitted": std_manifest["omitted"],
            "std_complete": std_manifest["complete"],
            "std_digest": std_manifest["digest"],
            "checkout": {"root": self.relax_src, "head": checkout_head,
                         "status_lines": checkout_status},
        }
        self.figures["input_identities"] = identities
        self.progress("input_identities", fixtures=len(identities["fixtures"]),
                      snapshot_arms=len(identities["snapshot"]),
                      std_digest=identities["engine"]["std_digest"],
                      staged_sha256=identities["engine"]["staged_sha256"])

    def verify_sources_at_closeout(self) -> None:
        """Re-read the ACTUAL inputs around the probes: the apparatus and
        workload pins, the retained checkout and its bundled std, the staged
        executable, and every materialized fixture/snapshot source tree and lock.
        Any changed or missing recorded byte is a refusal; files that appeared
        since (build output) are counted and disclosed, never a source change."""
        apparatus = self.binding["apparatus"]
        head, tree = resolve_source({"head": apparatus["head"], "tree": apparatus["tree"]})
        workload = self.verify_workload_runtime()
        recorded = self.figures.get("input_identities") or {}
        if not recorded:
            raise Refusal("no materialized input identities were recorded before the probes")
        engine = recorded["engine"]
        if sha256_file(self.relax_bin) != engine["staged_sha256"]:
            raise Refusal("the staged relax executable changed during the occurrence")
        std_now = tree_manifest(self.relax_std_dir)
        if not std_now["complete"] or std_now["digest"] != engine["std_digest"]:
            raise Refusal(
                "the retained bundled std changed or became unreadable during the occurrence"
            )
        checkout = engine["checkout"]
        checkout_head = self.git_text(["rev-parse", "HEAD"], cwd=self.relax_src,
                                      label="closeout-checkout-head").strip()
        checkout_status = sorted(
            line for line in self.git_text(
                ["status", "--porcelain", "--untracked-files=normal"],
                cwd=self.relax_src, label="closeout-checkout-status",
            ).splitlines() if line.strip()
        )
        if checkout_head != COMPAT_UPSTREAM_COMMIT or checkout_head != checkout["head"]:
            raise Refusal(f"the retained checkout is now {checkout_head}, not the pinned commit")
        if checkout_status != checkout["status_lines"]:
            raise Refusal(
                "the retained checkout's tree changed during the occurrence: "
                f"{checkout_status[:8]} != {checkout['status_lines'][:8]}"
            )
        fixture_rechecks: dict = {}
        for fixture in ("compat-fmt", "compat-crc32"):
            fixture_rechecks[fixture] = {}
            for arm in COMPAT_ARMS:
                entry = recorded["fixtures"][fixture][arm]
                result = recheck_manifest(read_manifest(entry["manifest_file"]),
                                          self.works[fixture][arm],
                                          MANIFEST_EXCLUDED_PREFIXES)
                fixture_rechecks[fixture][arm] = result
                if not result["unchanged"]:
                    raise Refusal(
                        f"the materialized {fixture}/{arm} fixture source changed: "
                        f"{result}"
                    )
                lock = os.path.join(self.works[fixture][arm], "Cargo.lock")
                if sha256_file(lock) != entry["lock_sha256"]:
                    raise Refusal(f"the materialized {fixture}/{arm} lock changed")
        snapshot_rechecks: dict = {}
        for arm in COMPAT_ARMS:
            entry = recorded["snapshot"][arm]
            result = recheck_manifest(read_manifest(entry["manifest_file"]),
                                      self.works["snapshot"][arm],
                                      MANIFEST_EXCLUDED_PREFIXES)
            snapshot_rechecks[arm] = result
            if not result["unchanged"]:
                raise Refusal(
                    f"the materialized {arm} snapshot source changed: {result}"
                )
            lock = os.path.join(self.works["snapshot"][arm], "Cargo.lock")
            if sha256_file(lock) != WORKLOAD_LOCK_SHA256 or sha256_file(lock) != entry["lock_sha256"]:
                raise Refusal(f"the materialized {arm} snapshot lock changed")
        control = (self.works.get("source-control") or {}).get("relax")
        control_facts: dict = {}
        if control:
            entry = recorded.get("source_control")
            if not entry:
                raise Refusal("no identity baseline was recorded for the source control")
            control_result = recheck_manifest(read_manifest(entry["manifest_file"]),
                                              control, MANIFEST_EXCLUDED_PREFIXES)
            if not control_result["unchanged"]:
                raise Refusal(f"the source-control fixture changed: {control_result}")
            with open(os.path.join(control, "Cargo.lock"), encoding="utf-8") as handle:
                control_lock = handle.read()
            if 'name = "crc32fast"' in control_lock:
                raise Refusal("the source control's lock gained the omitted package")
            control_facts = {"unchanged": True, "omitted_package_absent": True}
        registry_extractions = 0
        extras = []
        shared = getattr(self, "shared_cargo_home", None)
        locked_images = set()
        for package in lock_sources(self.workload_lock_text)["registry"]:
            locked_images.add(f"{package['name']}-{package['version']}")
        if shared:
            src_root = os.path.join(shared, "registry", "src")
            if os.path.isdir(src_root):
                for namespace in sorted(os.listdir(src_root)):
                    base = os.path.join(src_root, namespace)
                    if not os.path.isdir(base):
                        continue
                    for name in sorted(os.listdir(base)):
                        registry_extractions += 1
                        if name not in locked_images:
                            extras.append(name)
        if extras:
            raise Refusal(
                "the private Cargo home extracted a registry package the workload lock "
                f"does not list: {sorted(extras)[:6]}"
            )
        self.figures["closeout_source_check"] = {
            "apparatus": {"head": head, "tree": tree},
            "source_control": control_facts,
            "registry_extractions": registry_extractions,
            "workload": {"commit": workload["commit"], "lock_sha256": workload["lock_sha256"]},
            "checkout": {"head": checkout_head,
                         "status_lines": len(checkout_status),
                         "matches_baseline": checkout_status == checkout["status_lines"]},
            "engine": {"staged_sha256": engine["staged_sha256"],
                       "std_digest": engine["std_digest"]},
            "fixtures": fixture_rechecks,
            "snapshot": snapshot_rechecks,
        }
        self.progress("closeout_sources", apparatus_head=head,
                      workload_commit=workload["commit"], checkout_head=checkout_head,
                      staged_sha256=engine["staged_sha256"],
                      changed_fixture_rows=sum(
                          result["changed_count"] + result["missing_count"]
                          for arms in fixture_rechecks.values() for result in arms.values()),
                      changed_snapshot_rows=sum(
                          result["changed_count"] + result["missing_count"]
                          for result in snapshot_rechecks.values()))

    def execute(self) -> None:
        probes = (
            ("format", self.case_format),
            ("crc32", self.case_crc32),
            ("snapshot", self.case_snapshot),
            ("source-constrained", self.case_source_constrained),
        )
        for name, method in probes:
            if self.aborted:
                break
            try:
                method()
            except Refusal as error:
                self.aborted = f"{name}: {error}"
                break
            except (OSError, ValueError, KeyError, TypeError,
                    subprocess.SubprocessError) as error:
                self.aborted = f"{name}: {type(error).__name__}: {error}"
                break
        try:
            self.verify_sources_at_closeout()
        except Refusal as error:
            detail = f"closeout source check: {error}"
            self.aborted = f"{self.aborted}; {detail}" if self.aborted else detail
        # The final closure census runs AFTER every closeout child, so the
        # published classification covers the whole occurrence.
        try:
            self.closure_check("closeout")
        except (Refusal, OSError, subprocess.SubprocessError) as error:
            detail = f"closeout closure check: {type(error).__name__}: {error}"
            self.aborted = f"{self.aborted}; {detail}" if self.aborted else detail

    # -- receipt -----------------------------------------------------------

    def compatibility(self) -> dict:
        verdicts: dict = {}
        for case in COMPAT_CASES:
            rows = {row.get("arm"): row for row in self.cases if row.get("case") == case["case"]}
            if len(rows) != len(case["arms"]):
                verdicts[case["case"]] = "unexecuted"
                continue
            failures = [row for row in rows.values() if row.get("verdict") != "match"]
            verdicts[case["case"]] = "match" if not failures else failures[0]["verdict"]
        custody_closed, custody_detail = self.custody_clean()
        if (self.aborted or not custody_closed
                or any(verdict == "unexecuted" for verdict in verdicts.values())):
            result = "incomplete"
        elif all(verdict == "match" for verdict in verdicts.values()):
            result = "cleared"
        else:
            result = "blocked"
        return {
            "required_probes": [case["case"] for case in COMPAT_CASES if case["required"]],
            "probe_verdicts": verdicts,
            "result": result,
            "children_custody_closed": custody_closed,
            "children_custody_detail": custody_detail,
            "success_rule": (
                "compatibility is cleared only when EVERY required probe ran on both arms, "
                "its structural oracle matched, the Relax arm reproduced the baseline's "
                "exact stdout, both children exited cleanly and every child this body "
                "started was positively reaped with an empty census of its own session; "
                "whole-group closure stays the guard's"
            ),
            "scope": (
                f"only makepad/relax {COMPAT_UPSTREAM_COMMIT} on this platform, against the "
                f"unchanged workload {WORKLOAD_COMMIT}; no integration, adoption, "
                "installation, default change or performance claim"
            ),
        }

    def samples_bytes(self) -> bytes:
        document = {
            "schema": "oxide.relax-compat-samples/1",
            "cases_total": len(self.cases),
            "cases": self.cases,
        }
        return json.dumps(document, indent=1).encode()

    def receipt(self) -> bytes:
        samples = self.samples_bytes()
        packet_name = self.binding["paths"]["packet_name"]
        base = {
            "schema": "oxide.relax-compat/1",
            "mode": self.mode,
            "qualification": (
                "none: uncounted keep-going diagnostic; it grants no compatibility credit"
                if self.mode == "compat-diagnostic" else
                "counted author compatibility qualification at the bound head; not product QA"
            ),
            "closure": (
                "the guard's post-leader census decides closure; this receipt claims none"
            ),
            "planner_reference": self.binding["planner_reference"],
            "apparatus": self.figures.get("source", {}).get("apparatus"),
            "workload": self.figures.get("source", {}).get("workload"),
            "upstream": {
                "url": COMPAT_UPSTREAM_URL,
                "commit": COMPAT_UPSTREAM_COMMIT,
                "tree": COMPAT_UPSTREAM_TREE,
                "date": COMPAT_UPSTREAM_DATE,
                "fix_commits": self.figures.get("acquisition", {}).get("fix_commits"),
                "license": (
                    "MIT; Relax's bundled-std notices live upstream under "
                    "libs/relax/compiler/std/CREDITS.md and no upstream bytes are copied here"
                ),
            },
            "toolchain": TOOLCHAIN,
            "platform": f"{platform.system()} {platform.machine()}",
            "figures": {key: value for key, value in self.figures.items() if key != "acquisition"},
            "acquisition": self.figures.get("acquisition"),
            "disposals": self.disposals,
            "children": {
                "spawned": self.children_spawned,
                "custody_clean": self.custody_clean()[0],
                "custody": [
                    {key: entry.get(key) for key in
                     ("label", "argv", "spawned", "reaped", "wait_error")}
                    for entry in self.custody[:24]
                ],
                "custody_records": len(self.custody),
                "closure": self.closure,
                "boundary": (
                    "every child runs in the guard's OWN session and process group — no "
                    "setsid and no separate group — so the qualified supervisor can see and "
                    "stop it and a guard stop reaches it. Closure is decided by classifying "
                    "the guard session: only the guard and its descendants, this script and "
                    "its transient census helper may appear; this script's own subtree must "
                    "be empty and any other row (an orphaned producer, for example) is "
                    "unclassified and refuses every disposal. An unreadable table is unknown, "
                    "never closed. A descendant that calls setsid leaves the guard session "
                    "(the supervisor's census has the same limit) and no compatibility credit "
                    "is claimed from such a run"
                ),
            },
            "retained_evidence": sorted(
                capture["file"]
                for row in self.cases for capture in (row.get("captures") or {}).values()
                if capture.get("file")
            ),
            "compatibility": self.compatibility(),
            "resolution_basis": {
                "frozen_flag": (
                    "the pinned resolver treats --frozen as offline + lock-preferring; it is "
                    "not Cargo's fail-on-drift"
                ),
                "source_set": (
                    "the private Cargo home's registry crates and git databases are verified "
                    "to be exactly the workload lock's, so any resolution the runner performs "
                    "offline is covered by the declared lock"
                ),
                "source_control": (
                    "a scratch fixture requiring a version absent from that verified set must "
                    "fail on the pinned runner; a zero exit blocks compatibility"
                ),
            },
            "performance_claim": "none: this focused recheck measures no timing and no cache cost",
            "integration": (
                "none: no product behaviour, recipe, default, dependency, installation or "
                "consumer change"
            ),
            "samples_file": f"samples-{packet_name}.json",
            "samples_rows": len(self.cases),
            "progress_file": self.progress_name(),
            "progress_records": self.progress_records,
            "progress_failures": self.progress_failures,
            "samples_sha256": hashlib.sha256(samples).hexdigest(),
            "statistics_basis": (
                "complete: verdicts, declared_unexecuted and the compatibility result cover "
                "every recorded case in the hash-bound samples artifact, whether this summary "
                "carries its row or not"
            ),
            "aborted": self.aborted,
            "exit_rule": (
                "exit 0 only when nothing aborted, every declared probe ran on both arms and "
                "every verdict is match; a divergence, timeout or unexecuted probe exits 1"
            ),
            "limits": {
                "wall_limit_s": COMPAT_LIMIT_SECONDS,
                "scratch_mib": COMPAT_SCRATCH_MIB,
                "stop_margin_s": COMPAT_STOP_MARGIN_S,
                "cap_basis": (
                    "each per-command cap is the minimum of its fixed value and the remaining "
                    "deadline minus the stop margin; no clock resets and no phase allowance"
                ),
                "disposal_basis": (
                    "only newly created stage output inside this run's guard root, and only "
                    "after every child this body started is positively reaped with an empty "
                    "census of its own session; the checkout, bundled std, seeded Cargo homes "
                    "and workload sources are never disposed"
                ),
                "evidence_basis": (
                    "each reached arm's bounded stdout/stderr is written into the evidence "
                    "packet before the next child runs, with its full-body SHA-256, byte count "
                    "and omission facts, so a later stop cannot erase an earlier refusal"
                ),
                "filesystem_caches": "not globally flushable under this scope",
            },
        }
        return json.dumps(build_compat_receipt(base, self.cases), indent=1).encode()

    def publish(self) -> None:
        self.cases.extend(compat_declared_unexecuted(self.cases))
        self.figures["owned_bytes"] = {
            "run_root": walk_bytes(self.run_root) if os.path.isdir(self.run_root)
            else {"path": self.run_root, "absent": True},
            "packet_dir": walk_bytes(self.packet_dir) if os.path.isdir(self.packet_dir)
            else {"path": self.packet_dir, "absent": True},
            "note": "occupied apparent bytes of the owned scratch and evidence packet; "
                    "never an SSD-write claim",
        }
        samples = self.samples_bytes()
        os.makedirs(self.packet_dir, exist_ok=True)
        packet_name = self.binding["paths"]["packet_name"]
        samples_destination = os.path.join(self.packet_dir, f"samples-{packet_name}.json")
        destination = os.path.join(self.packet_dir, f"receipt-{packet_name}.json")
        try:
            with open(samples_destination + ".tmp", "wb") as handle:
                handle.write(samples)
            os.replace(samples_destination + ".tmp", samples_destination)
            encoded = self.receipt()
            with open(destination + ".tmp", "wb") as handle:
                handle.write(encoded)
            os.replace(destination + ".tmp", destination)
        except OSError as error:
            raise Refusal(f"the compat receipt packet could not be written: {error}")
        manifest = os.path.join(self.packet_dir, "SHA256SUMS")
        try:
            entries = []
            for name in sorted(os.listdir(self.packet_dir)):
                path = os.path.join(self.packet_dir, name)
                if name == "SHA256SUMS" or name.endswith(".tmp") or not os.path.isfile(path):
                    continue
                entries.append(f"{sha256_file(path)}  {name}\n")
            with open(manifest + ".tmp", "w", encoding="utf-8") as handle:
                handle.writelines(entries)
            os.replace(manifest + ".tmp", manifest)
        except OSError as error:
            raise Refusal(f"the compat evidence manifest could not be written: {error}")
        self.published_receipt = destination
        print(f"samples: {samples_destination} ({len(samples)} bytes)")
        print(f"progress: {os.path.join(self.packet_dir, self.progress_name())} "
              f"({self.progress_records} records, {self.progress_failures} failures)")
        print(f"receipt: {destination} ({len(encoded)} bytes)")
        print(f"manifest: {manifest}")


def finish_compat_run(run: "CompatRun") -> int:
    """The ONE finish path, used by the real entry and by the effect-free
    controls: record an abort progress row, publish the bounded packet in every
    outcome (including a setup refusal before any case), then exit by the
    collection rule. A packet that cannot be published is a bounded failure,
    never a silent success."""
    if run.aborted:
        run.progress("aborted", reason=run.aborted)
    try:
        run.publish()
    except Refusal as error:
        print(f"the compat evidence packet could not be published: {error}", file=sys.stderr)
        return 1
    compatibility = run.compatibility()
    exit_code = compat_collection_exit(run.aborted, run.cases)
    summary = {
        "compatibility": compatibility["result"],
        "probes": compatibility["probe_verdicts"],
        "cases": len(run.cases),
        "verdicts": verdict_tally(run.cases),
        "aborted": run.aborted,
        "disposals": len(run.disposals),
        "exit": exit_code,
    }
    print(json.dumps(summary))
    if exit_code != 0:
        print(
            "the compatibility collection carries a divergence, a timeout, an unreached "
            "probe or an abort; exit 1",
            file=sys.stderr,
        )
        return 1
    if run.mode == "compat-diagnostic":
        print(
            "uncounted diagnostic: every required probe matched; this grants no "
            "qualification or performance credit",
            file=sys.stderr,
        )
    return 0


def guarded_compat_main(mode: str, binding_path: str, binding_sha: str) -> int:
    guard_exports()  # refuse before reading anything else
    try:
        with open(binding_path, "rb") as handle:
            payload = handle.read()
    except OSError as error:
        raise Refusal(f"the activation document is unreadable: {error}")
    if hashlib.sha256(payload).hexdigest() != binding_sha:
        raise Refusal("the activation document digest is not the supplied --sha256")
    binding = parse_compat_binding(payload, mode)
    run = CompatRun(mode, binding)
    try:
        run.setup()
    except (Refusal, OSError, ValueError, subprocess.SubprocessError) as error:
        run.aborted = f"setup: {error}"
        print(f"blocked before any case: {error}", file=sys.stderr)
    if not run.aborted:
        run.execute()
    return finish_compat_run(run)


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
        print(f"blocked before any case: {error}", file=sys.stderr)
    if not run.aborted:
        run.execute()
    # One bounded receipt in every outcome, including a refusal before the
    # owned roots exist; a receipt-write failure is itself a bounded refusal.
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


def compat_self_check() -> None:
    """Effect-free controls for the compat apparatus.

    Real orchestration runs against named fake external boundaries: a fake
    process-table boundary for the in-boundary closure classification, a fake
    spawn/wait triple for the bounded child runner, a fake child runner and fake
    heavy-stage/seed/volume boundaries for the
    setup→verification→disposal→source-set→materialize→probe→closeout
    composition. Nothing is compiled, acquired, staged, executed or measured,
    and no child process is started here."""
    import contextlib
    import shutil
    import tarfile
    import tempfile

    script_sha = sha256_file(SCRIPT)
    workload_lock_text = git_bytes(["show", f"{WORKLOAD_COMMIT}:Cargo.lock"]).decode("utf-8")
    lock_sets = lock_sources(workload_lock_text)

    def out_record(text: str, exit_code: int = 0, timed_out: bool = False,
                   **override) -> dict:
        body = text.encode("utf-8")
        record = {
            "stdout": text, "stderr": "", "exit": exit_code, "timed_out": timed_out,
            "stdout_sha256": hashlib.sha256(body).hexdigest(), "stdout_bytes": len(body),
            "stdout_complete": True, "stdout_readable": True, "stdout_decode_error": None,
            "stdout_prefix": body[:MAX_ARM_EVIDENCE_BYTES],
            "stderr_sha256": hashlib.sha256(b"").hexdigest(), "stderr_bytes": 0,
            "stderr_complete": True, "stderr_readable": True, "stderr_decode_error": None,
            "stderr_prefix": b"", "custody": {"spawned": True, "reaped": True,
                                              "wait_error": None},
        }
        record.update(override)
        return record

    # -- argument boundary -------------------------------------------------
    expect(parse_arguments(["--compat-plan"])[0] == "compat-plan", "--compat-plan parses")
    expect(parse_arguments(["--compat-plan", "--plan"])[0] == "refuse",
           "--compat-plan refuses a second mode")
    expect(parse_arguments(["--compat-self-check"])[0] == "compat-self-check",
           "--compat-self-check parses")
    expect(parse_arguments(["--compat-diagnostic"])[0] == "refuse",
           "a compat diagnostic without a binding refuses")
    expect(parse_arguments(["--compat-qualify", "--binding", "/p", "--sha256", "a" * 64])[0]
           == "compat-qualify", "a compat qualification with a binding parses")
    expect(parse_arguments(["--compat-qualify", "--binding", "/p", "--sha256", "zz"])[0]
           == "refuse", "a non-hex compat digest refuses")
    expect(parse_arguments(["--plan"])[0] == "plan", "the 307 plan mode still parses")
    expect(parse_arguments(["--diagnostic", "--binding", "/p", "--sha256", "a" * 64])[0]
           == "diagnostic", "the 307 diagnostic mode still parses")

    # -- activation parser -------------------------------------------------
    good = parse_compat_binding(compat_binding_document("compat-diagnostic", script_sha),
                                "compat-diagnostic")
    expect(good["planner_reference"] == "compat-activation-1", "the compat binding is accepted")
    expect(parse_compat_binding(compat_binding_document("compat-qualify", script_sha),
                                "compat-qualify") is not None,
           "a counted compat binding accepts")
    for label, patch in {
        "schema": {"schema": "other/1"},
        "mode": {"mode": "compat-qualify"},
        "script": {"script_sha256": "0" * 64},
        "apparatus-head": {"apparatus": {"head": "z" * 40, "tree": "b" * 40}},
        "apparatus-tree": {"apparatus": {"head": "a" * 40, "tree": "z" * 40}},
        "workload-commit": {"workload": {"commit": "0" * 40, "tree": WORKLOAD_TREE,
                                        "lock_sha256": WORKLOAD_LOCK_SHA256}},
        "workload-tree": {"workload": {"commit": WORKLOAD_COMMIT, "tree": "0" * 40,
                                      "lock_sha256": WORKLOAD_LOCK_SHA256}},
        "workload-lock": {"workload": {"commit": WORKLOAD_COMMIT, "tree": WORKLOAD_TREE,
                                      "lock_sha256": "0" * 64}},
        "upstream-commit": {"upstream": {"commit": "0" * 40, "tree": COMPAT_UPSTREAM_TREE}},
        "upstream-tree": {"upstream": {"commit": COMPAT_UPSTREAM_COMMIT, "tree": "0" * 40}},
        "toolchain": {"toolchain": "1.99.1"},
        "volume": {"volume": {"uuid": "00000000-0000-0000-0000-000000000000"}},
        "paths": {"paths": {"run_root_name": "../escape", "packet_name": "packet"}},
        "planner": {"planner_reference": ""},
        "guard-limit": {"guard": {"supervisor_sha256": EXPECTED_RUNNER_SHA256,
                                 "limit_seconds": 300,
                                 "scratch_mib": COMPAT_SCRATCH_MIB,
                                 "stop_margin_seconds": COMPAT_STOP_MARGIN_S}},
        "guard-margin": {"guard": {"supervisor_sha256": EXPECTED_RUNNER_SHA256,
                                  "limit_seconds": COMPAT_LIMIT_SECONDS,
                                  "scratch_mib": COMPAT_SCRATCH_MIB,
                                  "stop_margin_seconds": 60}},
    }.items():
        document = json.loads(compat_binding_document("compat-diagnostic", script_sha))
        document.update(patch)
        try:
            parse_compat_binding(json.dumps(document).encode(), "compat-diagnostic")
            raise AssertionError(f"compat binding mutation {label} must refuse")
        except Refusal:
            continue
    document = json.loads(compat_binding_document("compat-qualify", script_sha))
    del document["diagnostic_receipt_sha256"]
    try:
        parse_compat_binding(json.dumps(document).encode(), "compat-qualify")
        raise AssertionError("a counted compat activation without a receipt must refuse")
    except Refusal:
        pass

    # -- real workload identity and its pins ------------------------------
    workload = verify_workload({"commit": WORKLOAD_COMMIT, "tree": WORKLOAD_TREE,
                                "lock_sha256": WORKLOAD_LOCK_SHA256})
    expect(workload["lock_sha256"] == WORKLOAD_LOCK_SHA256,
           "the workload lock bytes match the prepared digest")
    for name, (version, checksum) in PINNED_WORKLOAD_DEPS.items():
        expect(workload["dependencies"][name]["version"] == version,
               f"the workload lock pins {name} {version}")
    for label, mutate in {
        "version": lambda text: text.replace('version = "1.0.23"', 'version = "1.0.24"', 1),
        "checksum": lambda text: text.replace(
            "29666d0abbfad1e3dc4dcf6144730dd3a3ab225bbbdac83319345b1b44ccfc1b", "0" * 64, 1),
        "unpinned": lambda text: text.replace('name = "zmij"', 'name = "zmij-gone"', 1),
    }.items():
        try:
            pinned_dependency_identities(mutate(workload_lock_text))
            raise AssertionError(f"a {label} mutation of the workload lock must refuse")
        except Refusal:
            continue
    for label, spec in {
        "commit": {"commit": "0" * 40, "tree": WORKLOAD_TREE, "lock_sha256": WORKLOAD_LOCK_SHA256},
        "tree": {"commit": WORKLOAD_COMMIT, "tree": "0" * 40, "lock_sha256": WORKLOAD_LOCK_SHA256},
        "lock": {"commit": WORKLOAD_COMMIT, "tree": WORKLOAD_TREE, "lock_sha256": "0" * 64},
    }.items():
        try:
            verify_workload(spec)
            raise AssertionError(f"a mutated workload {label} must refuse")
        except Refusal:
            continue
    expect(len(lock_sets["registry"]) >= 3 and lock_sets["git"],
           "the workload lock declares registry packages and git sources")

    # -- oracle transcripts ------------------------------------------------
    def fmt_transcript(counts: dict) -> str:
        lines = [f"declared f64={counts['f64']} f32={counts['f32']} "
                 f"parse_f64={counts['parse_f64']} parse_f32={counts['parse_f32']}"]
        f64_bits = [int(bits, 16) for bits in COMPAT_FMT_REQUIRED_F64]
        while len(f64_bits) < counts["f64"]:
            f64_bits.append(0x3ff0000000000000)
        for index, bits in enumerate(f64_bits):
            text = repr(struct.unpack("<d", struct.pack("<Q", bits))[0])
            lines.append(f"f64 {index} {bits:016x} {text} {bits:016x}")
        f32_bits = [int(bits, 16) for bits in COMPAT_FMT_REQUIRED_F32]
        while len(f32_bits) < counts["f32"]:
            f32_bits.append(0x3f800000)
        for index, bits in enumerate(f32_bits):
            text = repr(struct.unpack("<f", struct.pack("<I", bits))[0])
            lines.append(f"f32 {index} {bits:08x} {text} {bits:08x}")
        for index in range(counts["parse_f64"]):
            bits = 0x3ff0000000000000 + index
            again = repr(struct.unpack("<d", struct.pack("<Q", bits))[0])
            lines.append(f"parse-f64 {index} 1.{index:01d}e-7 {bits:016x} {again} {bits:016x}")
        for index in range(counts["parse_f32"]):
            bits = 0x3f800000 + index
            again = repr(struct.unpack("<f", struct.pack("<I", bits))[0])
            lines.append(f"parse-f32 {index} 1.{index:01d}e-7 {bits:08x} {again} {bits:08x}")
        total = (counts["f64"] + counts["f32"] + counts["parse_f64"] + counts["parse_f32"])
        lines.append(f"checked={total} failures=0")
        return "\n".join(lines) + "\n"

    counts = {"f64": 24, "f32": 16, "parse_f64": 8, "parse_f32": 4}
    transcript = fmt_transcript(counts)
    expect(compat_fmt_oracle(out_record(transcript))[0] == "match",
           "a complete synthetic float transcript matches")
    expect(compat_fmt_oracle(out_record(transcript, exit_code=1))[0] == "mismatch",
           "a float transcript with a nonzero exit mismatches")
    expect(compat_fmt_oracle(out_record(transcript, timed_out=True))[0] == "timeout",
           "a timed-out float probe is a timeout")
    fmt_mutations = {
        "summary": transcript.replace("failures=0", "failures=1"),
        "short": transcript.replace(f"checked={sum(counts.values())} failures=0",
                                    f"checked={sum(counts.values()) - 1} failures=0", 1),
        "count": fmt_transcript({**counts, "f64": 4}),
        "back": transcript.replace("f64 0 8000000000000000 -0.0 8000000000000000",
                                   "f64 0 8000000000000000 -0.0 0000000000000000", 1),
        "decode": transcript.replace("f64 0 8000000000000000 -0.0", "f64 0 8000000000000000 1.5", 1),
        "order": transcript.replace("f64 1 ", "f64 9 ", 1),
        "coverage": fmt_transcript(counts).replace(
            "f64 0 8000000000000000 -0.0 8000000000000000",
            "f64 0 3ff0000000000000 1.0 3ff0000000000000", 1),
        "header": transcript.replace("declared", "declare", 1),
    }
    for label, mutated in fmt_mutations.items():
        expect(compat_fmt_oracle(out_record(mutated))[0] != "match",
               f"a {label}-mutated float transcript must not match")

    crc_transcript = (
        "crc32 0 empty 00000000\n"
        "crc32 1 check cbf43926\n"
        "crc32 2 fox 414fa339\n"
        "crc32 3 long 1234abcd\n"
        "crc32 4 zeros 62c8e5f2\n"
        "crc32-fold long 1234abcd\n"
        "crc32-stream check cbf43926\n"
        f"{COMPAT_CRC32_CHECK_LINE}\n"
    )
    expect(compat_crc32_oracle(out_record(crc_transcript))[0] == "match",
           "a complete synthetic CRC32 transcript matches")
    for label, mutated in {
        "check": crc_transcript.replace("crc32 1 check cbf43926", "crc32 1 check deadbeef"),
        "fold": crc_transcript.replace("crc32-fold long 1234abcd", "crc32-fold long 00000000"),
        "order": crc_transcript.replace("crc32 2 fox", "crc32 2 zerox"),
        "line": crc_transcript.replace(COMPAT_CRC32_CHECK_LINE, "crc32-check 123456789 0"),
    }.items():
        expect(compat_crc32_oracle(out_record(mutated))[0] != "match",
               f"a {label}-mutated CRC32 transcript must not match")
    expect(compat_crc32_oracle(out_record(crc_transcript, exit_code=1))[0] == "mismatch",
           "a nonzero CRC32 exit mismatches")

    help_transcript = SNAPSHOT_HELP_MARKER + "\nUSAGE:\n    oxide status\n"
    expect(compat_snapshot_oracle(out_record(help_transcript))[0] == "match",
           "a help transcript with the marker matches")
    expect(compat_snapshot_oracle(out_record(help_transcript, exit_code=1))[0] == "mismatch",
           "a nonzero help exit mismatches")
    expect(compat_snapshot_oracle(out_record(help_transcript + "[trial-1]\n"))[0]
           == "mismatch", "an edited help transcript mismatches")
    expect(compat_snapshot_oracle(out_record(help_transcript, timed_out=True))[0]
           == "timeout", "a timed-out help probe is a timeout")

    # -- complete-byte identity and the truncation refusal -----------------
    # The same retained prefix with a different suffix must NOT compare equal:
    # comparison is over the full-body hash and byte count, not a prefix.
    shared_prefix = help_transcript
    baseline = out_record(shared_prefix + "tail-one\n")
    same_prefix_other_tail = out_record(shared_prefix + "tail-two\n")
    expect(compat_correspondence("match", baseline, baseline) == "match",
           "identical complete bodies correspond")
    expect(compat_correspondence("match", baseline, same_prefix_other_tail) == "mismatch",
           "the same prefix with a different suffix is a mismatch")
    truncated = out_record(help_transcript, stdout_complete=False)
    expect(compat_snapshot_oracle(truncated)[0] == "mismatch",
           "a body over the analysis bound can never match")
    expect(compat_correspondence("match", out_record(help_transcript), truncated) == "mismatch",
           "an incomplete body never corresponds")
    invalid = out_record(help_transcript, stdout=None, stdout_decode_error="invalid byte 0xff")
    expect(compat_snapshot_oracle(invalid)[0] == "mismatch",
           "invalid UTF-8 can never match")
    unreadable = out_record(help_transcript, stdout=None, stdout_readable=False)
    expect(compat_snapshot_oracle(unreadable)[0] == "mismatch",
           "an unreadable capture can never match")
    expect(compat_output_text(out_record("ok\n"))[0] == "ok\n",
           "a complete, readable, strict body is used as text")

    # -- the source-constrained negative control --------------------------
    expect(compat_source_control_oracle(out_record("", exit_code=1))[0] == "match",
           "the control requires a nonzero exit")
    expect(compat_source_control_oracle(out_record("resolved anyway\n"))[0] == "mismatch",
           "a zero exit from the control blocks compatibility")
    expect(compat_source_control_oracle(out_record("", exit_code=1, timed_out=True))[0]
           == "timeout", "a timed-out control is not a match")

    # -- verdict rules -----------------------------------------------------
    expect(compat_correspondence("timeout", baseline, baseline) == "timeout",
           "a timeout is never masked by correspondence")
    expect(compat_collection_exit(None, [{"verdict": "match"}]) == 0, "a clean collection exits 0")
    expect(compat_collection_exit(None, [{"verdict": "mismatch"}]) == 1,
           "a divergence-only collection exits 1")
    expect(compat_collection_exit(None, []) == 1, "an empty collection exits 1")
    expect(compat_collection_exit("format: boom", [{"verdict": "match"}]) == 1,
           "an aborted collection exits 1")
    declared_total = sum(len(case["arms"]) for case in COMPAT_CASES)
    expect(declared_total == 7,
           "the declared matrix is six probe arms plus the one-arm resolution control")
    rows = compat_declared_unexecuted([{"case": "format", "arm": "cargo", "verdict": "match"}])
    expect(len(rows) == declared_total - 1,
           "unexecuted rows cover exactly the unreached declared combinations")

    # -- the process-table parser and in-boundary closure classification ---
    session = 4242
    parsed = _parse_ps_census(
        f"  1  0  1 {session}\n100 {session} 100 {session}\n101 100 100 {session}\n"
        f"200  1  1 {session}\n300  1  1 9999\n", session)
    expect(parsed["readable"] and len(parsed["rows"]) == 5,
           "the process-table parser reads every row")
    expect(not _parse_ps_census("  1  0  1\n", session)["readable"],
           "a short census row is unreadable")
    expect(not _parse_ps_census("  1 x 1 4242\n", session)["readable"],
           "a non-numeric census row is unreadable")
    expect(not _parse_ps_census(f"  1 0 1 {session}\n  1 0 1 {session}\n", session)["readable"],
           "a duplicate census pid is unreadable")

    guard, ours, helper, guard_child = 500, 600, 601, 501
    rows = {
        1: {"ppid": 0, "pgid": 1, "sess": 0},
        guard: {"ppid": 1, "pgid": guard, "sess": session},
        guard_child: {"ppid": guard, "pgid": guard, "sess": session},
        ours: {"ppid": guard, "pgid": guard, "sess": session},
        helper: {"ppid": ours, "pgid": guard, "sess": session},
        900: {"ppid": 1, "pgid": guard, "sess": 9999},
    }
    closed = classify_closure(rows, ours, guard, session, {helper})
    expect(closed["closed"] and closed["session_rows"] == 4,
           "guard rows plus the transient helper are a closed session")
    orphaned = dict(rows)
    orphaned[700] = {"ppid": 1, "pgid": guard, "sess": session}
    unclassified = classify_closure(orphaned, ours, guard, session, {helper})
    expect(not unclassified["closed"] and unclassified["unclassified"] == [700],
           "an orphaned producer in the session refuses closure")
    own_child = dict(rows)
    own_child[701] = {"ppid": ours, "pgid": guard, "sess": session}
    live = classify_closure(own_child, ours, guard, session, {helper})
    expect(not live["closed"] and live["our_subtree"] == [701],
           "a live child of this script refuses closure")
    foreign = dict(rows)
    foreign[800] = {"ppid": 1, "pgid": 800, "sess": session}
    expect(not classify_closure(foreign, ours, guard, session, {helper})["closed"],
           "an unclassifiable in-session row refuses closure")

    # -- the real spawn boundary stays inside the guard's group -----------
    spawn_dir = tempfile.mkdtemp(dir=os.environ.get("TMPDIR") or None)
    try:
        recorded = {}

        class FakeProcess:
            pid = 424242
            returncode = None

        def fake_popen(argv, **kwargs):
            recorded["argv"] = argv
            recorded["kwargs"] = kwargs
            return FakeProcess()

        saved_popen = subprocess.Popen
        subprocess.Popen = fake_popen
        try:
            stdout_path = os.path.join(spawn_dir, "child.stdout")
            stderr_path = os.path.join(spawn_dir, "child.stderr")
            private_env = {"HOME": spawn_dir, "PATH": "/bin", "RUSTFLAGS": ""}
            pid = compat_spawn_real(["/bin/echo", "hi"], spawn_dir, private_env,
                                    stdout_path, stderr_path)
        finally:
            subprocess.Popen = saved_popen
        expect(pid == 424242, "the real spawn boundary returns the child PID")
        expect("start_new_session" not in recorded["kwargs"]
               and "process_group" not in recorded["kwargs"],
               "the real spawn boundary never leaves the guard's session or process group")
        expect(recorded["kwargs"].get("stdin") is subprocess.DEVNULL,
               "the real spawn boundary gives the child the null device as stdin")
        expect(recorded["kwargs"].get("env") is private_env,
               "the real spawn boundary passes exactly the caller's private environment")
        expect(recorded["kwargs"].get("cwd") == spawn_dir, "the real spawn boundary keeps the cwd")
        expect(os.path.isfile(stdout_path) and os.path.isfile(stderr_path),
               "the real spawn boundary points the child at its capture files")
    finally:
        shutil.rmtree(spawn_dir)

    # -- the bounded runner: complete bytes, timeout, exceptional wait ----
    capture_dir = tempfile.mkdtemp(dir=os.environ.get("TMPDIR") or None)
    saved_core = {name: globals()[name] for name in
                  ("COMPAT_SPAWNER", "COMPAT_WAIT", "COMPAT_KILL")}
    try:
        deadline = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) + int(120e9)

        def scripted_runner(waits=None, kills=None):
            def spawner(argv, cwd, env, stdout_path, stderr_path):
                with open(stdout_path, "wb") as handle:
                    handle.write(b"captured-stdout\n")
                with open(stderr_path, "wb") as handle:
                    handle.write(b"captured-stderr\n")
                return 777

            def waiter(pid, block):
                if waits == "timeout":
                    return (pid, 0, None) if pid in kills else (0, 0, None)
                if waits:
                    item = waits.pop(0)
                    if isinstance(item, Exception):
                        raise item
                    return item
                return (pid, 0, None)

            def killer(pid):
                kills.append(pid)

            return spawner, waiter, killer

        runs = []
        for case, waits in (("normal", None), ("timeout", "timeout"),
                            ("wait-error", [ChildProcessError(10, "No child processes")])):
            kills: list = []
            spawner, waiter, killer = scripted_runner(waits, kills)
            globals()["COMPAT_SPAWNER"] = spawner
            globals()["COMPAT_WAIT"] = waiter
            globals()["COMPAT_KILL"] = killer
            case_deadline = deadline
            if case == "timeout":
                case_deadline = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) + int(30.05e9)
            record = compat_run_child(["/bin/true"], capture_dir, {"HOME": capture_dir},
                                      CAP_COMPAT_VERIFY_S, case_deadline, capture_dir,
                                      f"seam-{case}")
            runs.append((case, record, kills))
        for name in ("COMPAT_SPAWNER", "COMPAT_WAIT", "COMPAT_KILL"):
            globals()[name] = saved_core[name]

        normal = {case: record for case, record, _ in runs}["normal"]
        expect(normal["custody"]["reaped"] and normal["exit"] == 0,
               "a clean child is positively reaped")
        expect(normal["stdout"] == "captured-stdout\n"
               and normal["stdout_complete"] and normal["stdout_readable"],
               "the runner uses a complete, readable capture as text")
        expect(normal["stdout_sha256"] == hashlib.sha256(b"captured-stdout\n").hexdigest()
               and normal["stdout_bytes"] == len(b"captured-stdout\n"),
               "the runner hashes and counts the COMPLETE body")
        expect(normal["stdout_prefix"] == b"captured-stdout\n",
               "the runner keeps the raw prefix for retention")
        expect(not os.path.exists(os.path.join(capture_dir, "seam-normal.stdout")),
               "the runner removes its raw capture files after reading them")
        timeouts = {case: (record, kills) for case, record, kills in runs}["timeout"]
        expect(timeouts[0]["timed_out"] and timeouts[0]["custody"]["reaped"],
               "a timed-out child is killed and positively reaped")
        expect(timeouts[1] == [777], "the timeout kills exactly the child PID it started")
        wait_error = {case: record for case, record, _ in runs}["wait-error"]
        expect(not wait_error["custody"]["reaped"] and wait_error["custody"]["wait_error"],
               "an exceptional wait never claims a reap")
        past = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) - int(1e9)
        try:
            compat_run_child(["/bin/true"], capture_dir, {}, CAP_COMPAT_VERIFY_S,
                             past, capture_dir, "seam-spent")
            raise AssertionError("a spent deadline must refuse before spawning")
        except Refusal:
            pass
    finally:
        shutil.rmtree(capture_dir)

    # -- input manifests: added source entries refuse, outputs do not -----
    manifest_dir = tempfile.mkdtemp(dir=os.environ.get("TMPDIR") or None)
    try:
        tree = os.path.join(manifest_dir, "tree")
        os.makedirs(os.path.join(tree, "src"))
        with open(os.path.join(tree, "Cargo.toml"), "w", encoding="utf-8") as handle:
            handle.write("[package]\nname = \"x\"\n")
        with open(os.path.join(tree, "src", "main.rs"), "w", encoding="utf-8") as handle:
            handle.write("fn main() {}\n")
        manifest = tree_manifest(tree)
        expect(manifest["complete"] and manifest["entries"] >= 3,
               "a manifest covers files and their directories")
        expect(recheck_manifest(manifest, tree)["unchanged"],
               "an unchanged tree is unchanged")
        with open(os.path.join(tree, "src", "evil.rs"), "w", encoding="utf-8") as handle:
            handle.write("pub fn injected() {}\n")
        added_source = recheck_manifest(manifest, tree)
        expect(not added_source["unchanged"] and added_source["added_inputs"] == ["src/evil.rs"],
               "an added source file refuses identity")
        os.remove(os.path.join(tree, "src", "evil.rs"))
        os.makedirs(os.path.join(tree, "target", "debug"), exist_ok=True)
        with open(os.path.join(tree, "target", "debug", "out.bin"), "wb") as handle:
            handle.write(b"output\n")
        expect(recheck_manifest(manifest, tree)["unchanged"],
               "an added entry under the declared output prefix is tolerated")
        os.symlink("/etc/passwd", os.path.join(tree, "src", "link.rs"))
        added_link = recheck_manifest(manifest, tree)
        expect(not added_link["unchanged"] and added_link["added_inputs"] == ["src/link.rs"],
               "an added symlink outside the output prefix refuses identity")
        os.remove(os.path.join(tree, "src", "link.rs"))
        with open(os.path.join(tree, "src", "main.rs"), "a", encoding="utf-8") as handle:
            handle.write("// changed\n")
        expect(not recheck_manifest(manifest, tree)["unchanged"],
               "a changed source file refuses identity")
        os.remove(os.path.join(tree, "Cargo.toml"))
        expect(not recheck_manifest(manifest, tree)["unchanged"],
               "a removed source file refuses identity")
    finally:
        shutil.rmtree(manifest_dir)

    # -- private source set verification ----------------------------------
    def fake_seed_record() -> dict:
        files = []
        for package in lock_sets["registry"]:
            files.append({"path": f"registry/cache/{COMPAT_SEED_NAMESPACE}/"
                                  f"{package['name']}-{package['version']}.crate",
                          "sha256": package["checksum"]})
        for package in lock_sets["registry"]:
            files.append({"path": f"registry/index/{COMPAT_SEED_NAMESPACE}/.cache/"
                                  f"{package['name']}", "sha256": "0" * 64})
        files.append({"path": f"registry/index/{COMPAT_SEED_NAMESPACE}/config.json",
                      "sha256": "0" * 64})
        git_sources = [{"source": f"git+{package['url']}#{package['commit']}",
                        "repository": package["url"], "rev": package["commit"],
                        "name": package["name"], "version": package["version"],
                        "candidates": [{"commit_present": True}]}
                       for package in lock_sets["git"]]
        return {"files": files, "git_sources": git_sources,
                "namespace": COMPAT_SEED_NAMESPACE}

    seed_record = fake_seed_record()

    def source_set_run(seed: dict) -> CompatRun:
        run = CompatRun.__new__(CompatRun)
        run.seed_record = seed
        run.workload_lock_text = workload_lock_text
        run.figures = {}
        run.packet_dir = os.path.join(manifest_dir, "packet")
        run.binding = {"paths": {"run_root_name": "run", "packet_name": "packet"}}
        run.progress_records = 0
        run.progress_failures = 0
        return run

    good_run = source_set_run(seed_record)
    facts = good_run.verify_source_set()
    expect(facts["registry_packages"] == len(lock_sets["registry"])
           and facts["git_sources"],
           "the verified source set is exactly the workload lock's")
    for label, mutate in {
        "extra-crate": lambda seed: seed["files"].append(
            {"path": f"registry/cache/{COMPAT_SEED_NAMESPACE}/extra-9.9.9.crate",
             "sha256": "0" * 64}),
        "missing-crate": lambda seed: seed["files"].__delitem__(
            next(index for index, entry in enumerate(seed["files"])
                 if entry["path"].endswith(".crate"))),
        "bad-checksum": lambda seed: seed["files"].__setitem__(
            next(index for index, entry in enumerate(seed["files"])
                 if entry["path"].endswith(".crate")),
            {**next(entry for entry in seed["files"] if entry["path"].endswith(".crate")),
             "sha256": "0" * 64}),
        "extra-index": lambda seed: seed["files"].append(
            {"path": f"registry/index/{COMPAT_SEED_NAMESPACE}/.cache/extra",
             "sha256": "0" * 64}),
        "git-source": lambda seed: seed["git_sources"][0].__setitem__(
            "repository", "https://example.invalid/other"),
        "git-commit": lambda seed: seed["git_sources"][0].__setitem__(
            "candidates", [{"commit_present": False}]),
    }.items():
        mutated_seed = json.loads(json.dumps(seed_record))
        mutate(mutated_seed)
        try:
            source_set_run(mutated_seed).verify_source_set()
            raise AssertionError(f"a {label} source set must refuse")
        except Refusal:
            continue

    # -- disposal policy: allowlist, containment, closure -----------------
    seam_dir = tempfile.mkdtemp(dir=os.environ.get("TMPDIR") or None)
    saved_table = globals()["COMPAT_PROCESS_TABLE_SEAM"]
    try:
        our_pid, guard_pid, session_id = os.getpid(), os.getppid(), os.getsid(0)

        def table_from(rows: list) -> dict:
            return {"readable": True, "rows": {pid: facts for pid, facts in rows},
                    "helper_pid": None, "error": None}

        def closed_table() -> dict:
            helper = 900001
            return {"readable": True, "error": None, "helper_pid": helper, "rows": {
                helper: {"ppid": our_pid, "pgid": guard_pid, "sess": session_id},
                our_pid: {"ppid": guard_pid, "pgid": guard_pid, "sess": session_id},
                guard_pid: {"ppid": 1, "pgid": guard_pid, "sess": session_id},
            }}

        def fresh_run(table: dict) -> CompatRun:
            run = CompatRun.__new__(CompatRun)
            run.run_root = os.path.join(seam_dir, "run")
            run.packet_dir = os.path.join(seam_dir, "packet")
            run.binding = {"paths": {"run_root_name": "run", "packet_name": "packet"}}
            run.disposals = []
            run.custody = [{"label": "clean", "spawned": True, "reaped": True}]
            run.children_spawned = 1
            run.progress_records = 0
            run.progress_failures = 0
            run.closure = None
            run.env = {"HOME": seam_dir}
            run.deadline_ns = time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) + int(60e9)
            globals()["COMPAT_PROCESS_TABLE_SEAM"] = lambda *_args, **_kwargs: table
            return run

        run = fresh_run(closed_table())
        os.makedirs(os.path.join(run.run_root, "stage-relax-target", "release"))
        with open(os.path.join(run.run_root, "stage-relax-target", "release", "relax"), "wb") as h:
            h.write(b"x" * 4096)
        run.dispose_stage_output("stage-relax-target", "self-check: closed in-boundary")
        expect(not os.path.exists(os.path.join(run.run_root, "stage-relax-target")),
               "an allowlisted stage output is removed under closed in-boundary custody")
        expect(run.disposals[0]["closure"]["closed"] is True
               and run.disposals[0]["bytes_before"]["entries"] == 1,
               "the disposal records its closure classification and removed bytes")
        run.dispose_stage_output("stage-relax-target", "self-check: absent is recorded")
        expect(run.disposals[-1]["absent"] is True, "an absent stage output is recorded as absent")
        for relative in ("relax-src", "cargo-homes/shared", "bin",
                         "workloads/snapshot/cargo/tree", "../escape",
                         "stage-relax-target/../relax-src", "/etc"):
            try:
                run.dispose_stage_output(relative, "self-check: undeclared")
                raise AssertionError(f"disposing {relative!r} must refuse")
            except Refusal:
                continue
        os.makedirs(os.path.join(run.run_root, "workloads", "snapshot", "cargo"))
        target = os.path.join(seam_dir, "outside")
        os.makedirs(target)
        with open(os.path.join(target, "keep"), "wb") as h:
            h.write(b"keep")
        os.symlink(target, os.path.join(run.run_root, "workloads", "snapshot", "cargo", "target"))
        try:
            run.dispose_stage_output("workloads/snapshot/cargo/target", "self-check: symlink")
            raise AssertionError("disposing a symlink must refuse")
        except Refusal:
            expect(os.path.exists(os.path.join(target, "keep")),
                   "the symlink refusal leaves the outside target untouched")

        # Unknown closure refuses the removal and leaves the bytes in place.
        for label, table in {
            "orphan": table_from([
                (our_pid, {"ppid": guard_pid, "pgid": guard_pid, "sess": session_id}),
                (guard_pid, {"ppid": 1, "pgid": guard_pid, "sess": session_id}),
                (700, {"ppid": 1, "pgid": guard_pid, "sess": session_id}),
            ]),
            "live-child": table_from([
                (our_pid, {"ppid": guard_pid, "pgid": guard_pid, "sess": session_id}),
                (guard_pid, {"ppid": 1, "pgid": guard_pid, "sess": session_id}),
                (701, {"ppid": our_pid, "pgid": guard_pid, "sess": session_id}),
            ]),
            "unreadable": {"readable": False, "rows": {}, "helper_pid": None,
                           "error": "ps failed"},
        }.items():
            os.makedirs(os.path.join(run.run_root, "stage-relax-target"), exist_ok=True)
            with open(os.path.join(run.run_root, "stage-relax-target", "keep"), "wb") as h:
                h.write(b"keep")
            globals()["COMPAT_PROCESS_TABLE_SEAM"] = lambda *_a, _t=table, **_k: _t
            try:
                run.dispose_stage_output("stage-relax-target", f"self-check: {label}")
                raise AssertionError(f"disposal under {label} closure must refuse")
            except Refusal:
                expect(os.path.exists(os.path.join(run.run_root, "stage-relax-target", "keep")),
                       f"the {label} refusal leaves the stage output in place")
        # An unreaped child refuses even when the table is closed.
        run.custody = [{"label": "u", "spawned": True, "reaped": False, "wait_error": "ECHILD"}]
        globals()["COMPAT_PROCESS_TABLE_SEAM"] = lambda *_a, **_k: closed_table()
        try:
            run.dispose_stage_output("stage-relax-target", "self-check: unreaped")
            raise AssertionError("disposal with an unreaped child must refuse")
        except Refusal:
            pass
        run.custody = [{"label": "clean", "spawned": True, "reaped": True}]
        expect(fresh_run({"readable": False, "rows": {}, "helper_pid": None, "error": "x"})
               .custody_clean()[0] is False,
               "an unknown closure is never reported clean")
    finally:
        globals()["COMPAT_PROCESS_TABLE_SEAM"] = saved_table
        shutil.rmtree(seam_dir)

    # -- real setup/verification/source-set/closeout composition -----------
    lock_text = workload_lock_text

    def fake_runner(probe_stdout: dict, relax_override: dict | None = None,
                    fail_on: tuple[str, ...] = (), records: list | None = None,
                    stop_on: tuple[str, ...] = ()):
        relax_override = relax_override or {}

        def run(argv, cwd, env, cap_s, label):
            joined = " ".join(str(part) for part in argv)
            if any(needle in joined for needle in stop_on):
                raise Refusal("simulated later stop before this child started")
            if records is not None:
                records.append({"argv": [str(part) for part in argv], "cwd": cwd,
                                "env": dict(env), "cap_s": cap_s, "label": label})
            exit_code = 0
            stdout = ""
            stderr = ""
            if any(needle in joined for needle in fail_on):
                exit_code = 1
                stderr = f"injected failure for {label}\n"
            elif "archive" in joined and "--format=tar" in joined:
                archive_path = argv[argv.index("-o") + 1]
                with tarfile.open(archive_path, "w") as archive:
                    for name, body in (("Cargo.lock", lock_text.encode()),
                                       ("src/main.rs", b"fn main() {}\n")):
                        info = tarfile.TarInfo(name)
                        info.size = len(body)
                        import io as _io
                        archive.addfile(info, _io.BytesIO(body))
            elif argv[0] == "tar" and "-x" in joined:
                archive_path = argv[argv.index("-f") + 1]
                dest = argv[argv.index("-C") + 1]
                with tarfile.open(archive_path, "r") as archive:
                    archive.extractall(dest)
            elif "generate-lockfile" in joined:
                with open(os.path.join(cwd, "Cargo.lock"), "w", encoding="utf-8") as handle:
                    handle.write(_fake_lock_text())
            elif joined.endswith("--version"):
                stdout = "relax 0.0.0-compat-check\n"
            elif " check " in f" {joined} " and "compat-hello" in joined:
                stdout = ""
            elif "compat-hello" in joined:
                stdout = COMPAT_HELLO_STDOUT
            elif " show " in f" {joined} " and ":Cargo.lock" in joined:
                stdout = lock_text
            elif " rev-parse " in f" {joined} " and joined.endswith("^{tree}"):
                stdout = (WORKLOAD_TREE if cwd == REPO else COMPAT_UPSTREAM_TREE) + "\n"
            elif " rev-parse HEAD" in f" {joined} ":
                stdout = COMPAT_UPSTREAM_COMMIT + "\n"
            elif " status --porcelain" in joined:
                stdout = ""
            elif " cat-file " in f" {joined} " or " merge-base " in f" {joined} ":
                stdout = ""
            elif "compat-crc32" in joined and COMPAT_SOURCE_CONTROL_REQUIREMENT in open(
                    os.path.join(cwd, "Cargo.toml"), encoding="utf-8").read():
                exit_code = 1
                stderr = "resolution failed: crc32fast =1.5.4 is not available offline\n"
            else:
                key = ("format" if "compat-fmt" in joined
                       else "crc32" if "compat-crc32" in joined else "snapshot")
                arm = "cargo" if argv[0] == "cargo" else "relax"
                stdout = (relax_override[key] if arm == "relax" and key in relax_override
                          else probe_stdout[key])
            body = stdout.encode()
            err = stderr.encode()
            return {
                "argv": [str(part) for part in argv], "cwd": cwd, "label": label,
                "exit": exit_code, "seconds": 0.001, "timed_out": False, "cap_s": cap_s,
                "rss_maxrss_raw": 0, "rss_unit": RSS_UNIT_NAME, "rss_bytes": 0,
                "stdout": stdout, "stderr": stderr,
                "stdout_sha256": hashlib.sha256(body).hexdigest(), "stdout_bytes": len(body),
                "stdout_complete": True, "stdout_readable": True, "stdout_decode_error": None,
                "stdout_prefix": body[:MAX_ARM_EVIDENCE_BYTES],
                "stdout_evidence": {"sha256": hashlib.sha256(body).hexdigest(),
                                    "bytes": len(body), "prefix": stdout[:MAX_PREFIX_BYTES],
                                    "truncated": False, "complete": True},
                "stderr_sha256": hashlib.sha256(err).hexdigest(), "stderr_bytes": len(err),
                "stderr_complete": True, "stderr_readable": True, "stderr_decode_error": None,
                "stderr_prefix": err[:MAX_ARM_EVIDENCE_BYTES],
                "stderr_evidence": {"sha256": hashlib.sha256(err).hexdigest(),
                                    "bytes": len(err), "prefix": stderr[:MAX_PREFIX_BYTES],
                                    "truncated": False, "complete": True},
                "custody": {"spawned": True, "reaped": True, "wait_error": None},
            }
        return run

    def _fake_lock_text() -> str:
        blocks = ["version = 4\n"]
        for name, (version, checksum) in PINNED_WORKLOAD_DEPS.items():
            blocks.append(
                "[[package]]\n"
                f'name = "{name}"\n'
                f'version = "{version}"\n'
                'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
                f'checksum = "{checksum}"\n'
            )
        return "\n".join(blocks)

    def fake_stage(run: CompatRun) -> None:
        run.relax_src = os.path.join(run.run_root, "relax-src")
        run.relax_std_dir = os.path.join(run.relax_src, "libs", "relax", "compiler", "std")
        os.makedirs(os.path.join(run.relax_std_dir, "core", "arch"))
        with open(os.path.join(run.relax_std_dir, "core", "arch", "aarch64.rs"), "w",
                  encoding="utf-8") as handle:
            handle.write("// fake pinned bundled std for the control\n")
        target = os.path.join(run.run_root, "stage-relax-target", "release")
        os.makedirs(target)
        with open(os.path.join(target, "relax"), "wb") as handle:
            handle.write(b"fake-relax-binary\n")
        binary = os.path.join(run.run_root, "bin")
        os.makedirs(binary, exist_ok=True)
        shutil.copy2(os.path.join(target, "relax"), os.path.join(binary, "relax"))
        run.relax_bin = os.path.join(binary, "relax")
        run.figures["acquisition"] = {"commit": COMPAT_UPSTREAM_COMMIT,
                                      "tree": COMPAT_UPSTREAM_TREE,
                                      "network_bytes": 0,
                                      "network_seconds": 0.0,
                                      "fix_commits": {sha: {"label": label, "present": True,
                                                            "ancestor": True}
                                                      for sha, label in
                                                      COMPAT_UPSTREAM_FIXES.items()}}
        run.figures["engine"] = {"sha256": sha256_file(run.relax_bin),
                                 "build_seconds": 0.0,
                                 "build_target": os.path.join(run.run_root, "stage-relax-target"),
                                 "retained_checkout": run.relax_src,
                                 "bundled_std": run.relax_std_dir,
                                 "fix_commits": run.figures["acquisition"]["fix_commits"]}

    def fake_seed(run: CompatRun) -> None:
        destination = os.path.join(run.run_root, "cargo-homes", "shared")
        os.makedirs(destination, exist_ok=True)
        run.shared_cargo_home = destination
        run.seed_record = json.loads(json.dumps(seed_record))
        run.figures["seed"] = {"destination": destination, "aggregate_bytes": 0,
                               "seconds": 0.0}

    saved_seams = {name: globals()[name] for name in
                   ("COMPAT_CHILD_RUNNER", "COMPAT_STAGE_SEAM", "COMPAT_SEED_SEAM",
                    "COMPAT_VOLUME_SEAM", "COMPAT_SETUP_SEAM", "SOURCE_GATE_SEAM",
                    "COMPAT_PROCESS_TABLE_SEAM")}
    composition_dir = tempfile.mkdtemp(dir=os.environ.get("TMPDIR") or None)
    saved_env = {key: os.environ.get(key) for key in GUARD_EXPORTS}
    os.environ["OXIDE_TASK_SCRATCH_ROOT"] = os.path.join(composition_dir, "scratch")
    os.environ["OXIDE_PROOF_EVIDENCE"] = os.path.join(composition_dir, "evidence")
    os.environ["OXIDE_PROOF_DEADLINE_NS"] = str(
        time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) + int(600e9))
    os.makedirs(os.environ["OXIDE_TASK_SCRATCH_ROOT"], exist_ok=True)
    os.makedirs(os.environ["OXIDE_PROOF_EVIDENCE"], exist_ok=True)
    globals()["SOURCE_GATE_SEAM"] = True
    globals()["COMPAT_VOLUME_SEAM"] = lambda: (composition_dir, SELECTED_VOLUME_UUID)
    globals()["COMPAT_STAGE_SEAM"] = fake_stage
    globals()["COMPAT_SEED_SEAM"] = fake_seed
    good_stdout = {"format": transcript, "crc32": crc_transcript, "snapshot": help_transcript}
    try:
        def composed_run(name: str) -> CompatRun:
            document = json.loads(compat_binding_document("compat-diagnostic", script_sha))
            document["apparatus"] = {"head": "a" * 40, "tree": "b" * 40}
            document["paths"] = {"run_root_name": f"run-{name}",
                                 "packet_name": f"packet-{name}"}
            return CompatRun("compat-diagnostic", document)

        # A. Successful composition: setup, verification, source set,
        #    materialization, probes, control, closure, closeout, publication.
        records: list = []
        globals()["COMPAT_CHILD_RUNNER"] = fake_runner(dict(good_stdout), records=records)
        globals()["COMPAT_PROCESS_TABLE_SEAM"] = lambda *_a, **_k: closed_table()
        run = composed_run("clean")
        with contextlib.redirect_stdout(io.StringIO()):
            with contextlib.redirect_stderr(io.StringIO()):
                run.setup()
            expect(run.aborted is None, "the real setup/verification composition completes")
            expect(set(run.figures.get("engine_verification", {}))
                   == {"before disposal", "after disposal"},
                   "the copied executable is verified both before and after disposal")
            expect(not os.path.exists(os.path.join(run.run_root, "stage-relax-target")),
                   "the engine build target is disposed after verification")
            expect(run.disposals and run.disposals[0]["closure"]["closed"] is True,
                   "the disposal records its closed in-boundary classification")
            expect(run.figures.get("source_set", {}).get("registry_packages")
                   == len(lock_sets["registry"]),
                   "the private source set is verified against the workload lock")
            run.execute()
            with contextlib.redirect_stderr(io.StringIO()):
                exit_code = finish_compat_run(run)
        expect(exit_code == 0, "a fully matching composition exits 0")
        expect(run.compatibility()["result"] == "cleared",
               "a fully matching composition is cleared")
        expect(run.custody_clean()[0] and len(run.cases) == declared_total,
               "every composed child is positively reaped and all declared arms recorded")
        receipt_path = os.path.join(run.packet_dir,
                                    f"receipt-{run.binding['paths']['packet_name']}.json")
        receipt = json.load(open(receipt_path))
        expect(receipt["compatibility"]["result"] == "cleared",
               "the published receipt carries the cleared result")
        expect(receipt["children"]["closure"]["closed"] is True
               and receipt["children"]["boundary"].startswith("every child runs in the guard's OWN"),
               "the receipt publishes the in-boundary closure ledger")
        expect(receipt["figures"].get("input_identities")
               and receipt["figures"].get("closeout_source_check")
               and receipt["resolution_basis"]["source_set"],
               "the receipt carries the input baseline, closeout recheck and resolution basis")
        expect(all(os.path.isfile(os.path.join(run.packet_dir, name))
                   for name in receipt["retained_evidence"]),
               "every retained per-arm evidence file exists in the packet")
        manifest = open(os.path.join(run.packet_dir, "SHA256SUMS"), encoding="utf-8").read()
        expect(all(name in manifest for name in receipt["retained_evidence"]),
               "the manifest covers the retained per-arm evidence")
        expect(not any("--relaxed" in record["argv"] or "--cached" in record["argv"]
                       for record in records),
               "no Relax child ever passes a relaxed-dialect or cached flag")

        def is_relax(record: dict) -> bool:
            return bool(record["argv"]) and str(record["argv"][0]).endswith("relax")

        relax_argv = [record["argv"] for record in records
                      if is_relax(record) and "run" in record["argv"]]
        expect(len(relax_argv) >= 6
               and all("--frozen" in argv and "--unattended" in argv
                       and "--check-first" in argv for argv in relax_argv),
               "every Relax run child (verification, both arms, the control) passes "
               "--check-first --frozen --unattended")
        check_argv = [record["argv"] for record in records
                      if is_relax(record) and "check" in record["argv"]]
        expect(check_argv and all("--frozen" in argv for argv in check_argv),
               "the engine-verification check also passes --frozen")
        relax_work_envs = [record["env"] for record in records if is_relax(record)
                           and ("run" in record["argv"] or "check" in record["argv"])]
        expect(relax_work_envs and all(env.get("MAKEPAD_RUN_OFFLINE") == "1"
                                       and env.get("RUSTUP_TOOLCHAIN") == TOOLCHAIN
                                       and env.get("RUSTFLAGS") == ""
                                       and env.get(RELAX_CACHE_ENV)
                                       and os.path.join(run.run_root, "relax-caches")
                                       in env.get(RELAX_CACHE_ENV, "")
                                       and not any(key.startswith("OXIDE_") for key in env)
                                       for env in relax_work_envs),
               "every Relax run/check child keeps offline resolution, the pinned toolchain, "
               "the out-of-tree cache override and no inherited OXIDE_* control")
        version_envs = [record["env"] for record in records if is_relax(record)
                        and "--version" in record["argv"]]
        expect(version_envs and all(env.get("RUSTUP_TOOLCHAIN") == TOOLCHAIN
                                    and env.get("RUSTFLAGS") == ""
                                    and env.get("HOME", "").startswith(run.run_root)
                                    for env in version_envs),
               "the version probe still runs with the private HOME, pinned toolchain and "
               "declared empty RUSTFLAGS")
        probe_envs = [record["env"] for record in records if is_relax(record)
                      and "run" in record["argv"]
                      and os.sep + "source-control" + os.sep not in record["cwd"]
                      and any(os.sep + name + os.sep in record["cwd"]
                              for name in ("compat-fmt", "compat-crc32", "snapshot"))]
        expect(len(probe_envs) == 3
               and all(env.get("CARGO_HOME") == run.shared_cargo_home
                       for env in probe_envs),
               "every Relax probe child (three probes) reads the one seeded private "
               "Cargo home")
        control_envs = [record["env"] for record in records if is_relax(record)
                        and os.sep + "source-control" + os.sep in record["cwd"]]
        expect(len(control_envs) == 1
               and control_envs[0].get("CARGO_HOME") == run.shared_cargo_home
               and control_envs[0].get("MAKEPAD_RUN_OFFLINE") == "1",
               "the resolution control also reads only the seeded private home, offline")
        setup_envs = [record["env"] for record in records
                      if not record["argv"][0].endswith("relax")]
        expect(setup_envs and all(env.get("HOME", "").startswith(run.run_root)
                                  and env.get("TMPDIR", "").startswith(run.run_root)
                                  and env.get("RUSTUP_TOOLCHAIN") == TOOLCHAIN
                                  and env.get("RUSTFLAGS") == ""
                                  for env in setup_envs),
               "every setup child already runs with the private HOME/TMPDIR and pinned "
               "toolchain (the environment exists before the first runtime child)")
        materialize_records = [record for record in records
                               if any(("archive" in " ".join(record["argv"]),
                                       "tar" in " ".join(record["argv"])))]
        expect(materialize_records and all(record["cap_s"] <= CAP_COMPAT_ARCHIVE_S
                                           for record in materialize_records),
               "materialization children are bounded by their declared cap")

        # B. Changed or added materialized inputs are refused at the real seam.
        run = composed_run("changed")
        globals()["COMPAT_CHILD_RUNNER"] = fake_runner(dict(good_stdout))
        with contextlib.redirect_stdout(io.StringIO()):
            with contextlib.redirect_stderr(io.StringIO()):
                run.setup()
        snapshot_source = os.path.join(run.works["snapshot"]["cargo"], "src", "main.rs")
        with open(snapshot_source, "a", encoding="utf-8") as handle:
            handle.write("// mutated after the baseline\n")
        try:
            run.verify_sources_at_closeout()
            raise AssertionError("a changed materialized snapshot source must refuse")
        except Refusal as refusal:
            expect("snapshot source changed" in str(refusal),
                   "the changed snapshot source is named by the closeout refusal")
        shutil.rmtree(run.run_root, ignore_errors=True)
        run = composed_run("added")
        with contextlib.redirect_stdout(io.StringIO()):
            with contextlib.redirect_stderr(io.StringIO()):
                run.setup()
        with open(os.path.join(run.works["compat-fmt"]["cargo"], "src", "injected.rs"),
                  "w", encoding="utf-8") as handle:
            handle.write("pub fn injected() {}\n")
        try:
            run.verify_sources_at_closeout()
            raise AssertionError("an added materialized source file must refuse")
        except Refusal as refusal:
            expect("fixture source changed" in str(refusal),
                   "an added source file is refused by the closeout identity check")
        shutil.rmtree(run.run_root, ignore_errors=True)
        run = composed_run("changed-lock")
        with contextlib.redirect_stdout(io.StringIO()):
            with contextlib.redirect_stderr(io.StringIO()):
                run.setup()
        with open(os.path.join(run.works["compat-fmt"]["cargo"], "Cargo.lock"),
                  "a", encoding="utf-8") as handle:
            handle.write("# drift\n")
        try:
            run.verify_sources_at_closeout()
            raise AssertionError("a changed materialized fixture lock must refuse")
        except Refusal:
            pass
        shutil.rmtree(run.run_root, ignore_errors=True)
        run = composed_run("changed-binary")
        with contextlib.redirect_stdout(io.StringIO()):
            with contextlib.redirect_stderr(io.StringIO()):
                run.setup()
        with open(run.relax_bin, "ab") as handle:
            handle.write(b"tampered")
        try:
            run.verify_sources_at_closeout()
            raise AssertionError("a changed staged executable must refuse")
        except Refusal:
            pass
        shutil.rmtree(run.run_root, ignore_errors=True)

        # C. A setup child failure publishes a bounded abort receipt.
        run = composed_run("setup-fail")
        globals()["COMPAT_CHILD_RUNNER"] = fake_runner(dict(good_stdout),
                                                       fail_on=("--version",))
        run.aborted = None
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                with contextlib.redirect_stderr(io.StringIO()):
                    run.setup()
        except (Refusal, OSError, ValueError) as error:
            run.aborted = f"setup: {error}"
        expect(run.aborted is not None, "a failing engine verification aborts setup")
        with contextlib.redirect_stdout(io.StringIO()):
            with contextlib.redirect_stderr(io.StringIO()):
                exit_code = finish_compat_run(run)
        expect(exit_code == 1, "an aborted setup exits 1")
        receipt = json.load(open(os.path.join(
            run.packet_dir, f"receipt-{run.binding['paths']['packet_name']}.json")))
        expect(receipt["compatibility"]["result"] == "incomplete"
               and receipt["declared_unexecuted"] == declared_total,
               "the setup-abort receipt records every declared arm unexecuted")
        expect(receipt["aborted"].startswith("setup:"),
               "the setup-abort receipt names the setup fault")
        shutil.rmtree(run.run_root, ignore_errors=True)

        # D. A later stop cannot erase an earlier real refusal.
        divergent = transcript.replace(
            "f64 0 8000000000000000 -0.0 8000000000000000",
            "f64 0 8000000000000000 -0.0 0000000000000001", 1)
        run = composed_run("later-stop")
        globals()["COMPAT_CHILD_RUNNER"] = fake_runner(
            dict(good_stdout), {"format": divergent}, stop_on=("--bin compat-crc32",))
        with contextlib.redirect_stdout(io.StringIO()):
            with contextlib.redirect_stderr(io.StringIO()):
                run.setup()
                run.execute()
                exit_code = finish_compat_run(run)
        expect(exit_code == 1, "a later stop with an earlier divergence exits 1")
        expect(run.aborted is not None and "crc32" in str(run.aborted)
               and "simulated later stop" in str(run.aborted),
               "the later stop aborts the next probe in the real driver")
        retained = sorted(name for name in os.listdir(run.packet_dir)
                          if name.startswith("case-format-relax."))
        expect(retained == ["case-format-relax.stderr.txt",
                            "case-format-relax.stdout.txt"],
               "the failing arm's bounded stdout and stderr were retained before the stop")
        body = open(os.path.join(run.packet_dir, "case-format-relax.stdout.txt"),
                    encoding="utf-8").read()
        expect("f64 0 8000000000000000" in body and len(body) <= MAX_ARM_EVIDENCE_BYTES,
               "the retained evidence carries the real arm output, bounded")
        progress_lines = [json.loads(line) for line in open(
            os.path.join(run.packet_dir, run.progress_name()), encoding="utf-8")]
        case_rows = [row for row in progress_lines if row.get("phase") == "case"]
        expect(case_rows and case_rows[-1]["verdict"] == "mismatch"
               and case_rows[-1]["stderr_sha256"],
               "the progress log keeps the failing arm's verdict and hashes")
        receipt = json.load(open(os.path.join(
            run.packet_dir, f"receipt-{run.binding['paths']['packet_name']}.json")))
        expect(receipt["verdicts"].get("mismatch") == 1
               and receipt["compatibility"]["result"] == "incomplete",
               "the receipt keeps both the divergence and the abort")
        shutil.rmtree(run.run_root, ignore_errors=True)

        # E. A zero-exit resolution control blocks compatibility.
        run = composed_run("control-zero")
        globals()["COMPAT_CHILD_RUNNER"] = fake_runner(dict(good_stdout))
        globals()["COMPAT_PROCESS_TABLE_SEAM"] = lambda *_a, **_k: closed_table()
        with contextlib.redirect_stdout(io.StringIO()):
            with contextlib.redirect_stderr(io.StringIO()):
                run.setup()
                run.case_source_constrained = lambda: run.record(
                    "source-constrained", "relax", out_record("resolved\n"), "mismatch", {})
                run.execute()
                exit_code = finish_compat_run(run)
        expect(exit_code == 1 and run.compatibility()["result"] == "blocked",
               "a resolution control that resolves anyway blocks compatibility")
        shutil.rmtree(run.run_root, ignore_errors=True)
    finally:
        for name, value in saved_seams.items():
            globals()[name] = value
        for key, value in saved_env.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        shutil.rmtree(composition_dir)

    # -- the real guarded entry: gates, abort publication and exit ---------
    entry_dir = tempfile.mkdtemp(dir=os.environ.get("TMPDIR") or None)
    try:
        document = json.loads(compat_binding_document("compat-diagnostic", script_sha))
        document["apparatus"] = {"head": "a" * 40, "tree": "b" * 40}
        binding_bytes = json.dumps(document).encode()
        binding_path = os.path.join(entry_dir, "binding.json")
        with open(binding_path, "wb") as handle:
            handle.write(binding_bytes)
        exports = {
            "OXIDE_TASK_SCRATCH_ROOT": os.path.join(entry_dir, "scratch"),
            "OXIDE_PROOF_DEADLINE_NS": str(
                time.clock_gettime_ns(time.CLOCK_UPTIME_RAW) + int(120e9)),
            "OXIDE_PROOF_EVIDENCE": os.path.join(entry_dir, "evidence"),
        }
        os.makedirs(exports["OXIDE_TASK_SCRATCH_ROOT"])
        saved_env = {key: os.environ.get(key) for key in exports}
        os.environ.update(exports)
        globals()["SOURCE_GATE_SEAM"] = True
        globals()["COMPAT_VOLUME_SEAM"] = lambda: (entry_dir, SELECTED_VOLUME_UUID)
        globals()["COMPAT_CHILD_RUNNER"] = fake_runner(dict(good_stdout))
        globals()["COMPAT_SETUP_SEAM"] = lambda _run: (_ for _ in ()).throw(
            Refusal("self-check injected setup refusal"))
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                with contextlib.redirect_stderr(io.StringIO()):
                    entry_exit = guarded_compat_main(
                        "compat-diagnostic", binding_path,
                        hashlib.sha256(binding_bytes).hexdigest())
        finally:
            for name, value in saved_seams.items():
                globals()[name] = value
            for name in ("COMPAT_SETUP_SEAM", "COMPAT_VOLUME_SEAM",
                         "COMPAT_CHILD_RUNNER", "SOURCE_GATE_SEAM"):
                globals()[name] = None
            for key, value in saved_env.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value
        expect(entry_exit == 1, "an aborted compat occurrence exits 1")
        receipt = json.load(open(os.path.join(
            exports["OXIDE_PROOF_EVIDENCE"], document["paths"]["packet_name"],
            f"receipt-{document['paths']['packet_name']}.json")))
        expect(receipt["compatibility"]["result"] == "incomplete",
               "the aborted receipt reports an incomplete compatibility result")
        expect(receipt["declared_unexecuted"] == declared_total,
               "every declared arm is recorded unexecuted in the abort receipt")
        expect(receipt["qualification"].startswith("none"),
               "a diagnostic receipt never claims qualification")
        expect("self-check injected setup refusal" in receipt["aborted"],
               "the injected setup refusal is recorded through the real entry")
        expect(receipt["children"]["custody_clean"] is False,
               "a refusal before any closure census never claims clean custody")
    finally:
        shutil.rmtree(entry_dir)

    # -- plan document -----------------------------------------------------
    document = compat_plan_text()
    for case in COMPAT_CASES:
        expect(f"  - {case['case']} " in document, f"the plan names the {case['case']} probe")
    expect("research:relax:compat:diagnostic" in document
           and "research:relax:compat:qualify" in document,
           "the plan names both guarded compat selectors")
    expect(str(COMPAT_LIMIT_SECONDS) in document and str(COMPAT_SCRATCH_MIB) in document,
           "the plan carries the fixed bounds")
    expect("--frozen" in document and "--unattended" in document,
           "the plan prints the pinned runner's frozen/unattended argv")
    expect(RELAX_CACHE_ENV in document, "the plan prints the out-of-tree cache override")
    expect("NOT Cargo's fail-on-drift" in document,
           "the plan states the resolver's frozen-flag limit and the source-set proof")
    expect("guard session" in document,
           "the plan states the in-boundary closure classification")
    expect("materialized inputs" in document, "the plan states the identity recheck")
    for item in COMPAT_PREMISE_CHECKLIST:
        expect(f"  - {item}: " in document, f"the plan carries the compat disposition {item}")
    expect("aa540c14" in document, "the plan preserves the 307 no-go pin as history")

    print("compat self-check: all controls passed (removed temp directories; no child, "
          "compile, network or engine)")


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
    if mode == "compat-plan":
        print(compat_plan_text())
        return 0
    if mode == "compat-self-check":
        compat_self_check()
        return 0
    assert binding is not None and sha is not None
    try:
        if mode in ("compat-diagnostic", "compat-qualify"):
            return guarded_compat_main(mode, binding, sha)
        return guarded_main(mode, binding, sha)
    except Refusal as error:
        print(f"refused before any effect: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
